//! Center-left "Code Editor" panel.
//!
//! Owns: the toolbar (Copy / Build / Scan / Flash buttons), the code editor
//! widget itself, the embedded bottom diagnostics panel, the LSP completion
//! popup, and the inline-diagnostic overlays.  It also writes the edited text
//! back into `generated_code` (main.rs) or the matching user source file.
//!
//! Implemented as one inherent method on `AppIde`; it consumes the
//! `project_files` snapshot (nothing after this panel needs it).

use super::AppIde;
use super::ProjectFileId;
use crate::panels::mcu_module::project_gen::ProjectFiles;
use eframe::egui;
use egui_code_editor::{CodeEditor, ColorTheme};

mod brace_block;
pub(crate) mod cargo_complete;
mod comment;
mod completion;
mod context_menu;
mod delete_line;
mod duplicate_line;
mod diag_embed;
pub(crate) mod find_replace;
mod format;
mod move_lines;
mod rename;
mod toolbar;

/// Default code-editor font size (points); the zoom baseline (Ctrl+0 resets to it).
pub(crate) const DEFAULT_EDITOR_FONT_SIZE: f32 = 13.0;
/// Zoom clamp range for the editor font.
const MIN_EDITOR_FONT_SIZE: f32 = 7.0;
const MAX_EDITOR_FONT_SIZE: f32 = 40.0;

impl AppIde {
    /// Render the central-left code editor panel (toolbar + editor + diagnostics).
    pub(super) fn show_editor_panel(
        &mut self,
        ui: &mut egui::Ui,
        project_files: Option<ProjectFiles>,
    ) {
        // ── Compute editor content AFTER the project tree ─────────────────────
        // IMPORTANT: display_code must be computed AFTER the project tree panel
        // so that self.selected_file reflects any click the user just made.
        // Computing it before the tree caused a write-back bug: when the user
        // clicked a user file, self.selected_file was already updated by the
        // click handler, but display_code still held the OLD file's content.
        // The write-back then wrongly stored the old content into the new file.
        let mut display_code: String = if let ProjectFileId::UserFile(i) = self.selected_file {
            self.project_tree
                .user_src_files
                .get(i)
                .map(|(_, c)| c.clone())
                .unwrap_or_default()
        } else if self.selected_file == ProjectFileId::MainRs {
            // Always read from self.generated_code — not from the project_files
            // snapshot built at the start of this frame.  The snapshot is stale
            // whenever load_project_from_dir() runs in the same frame (Open
            // Project), which would otherwise show the previous project's code
            // and then immediately overwrite generated_code via the write-back.
            self.generated_code.clone()
        } else {
            match &project_files {
                Some(files) => self.selected_file.content(files).to_owned(),
                None => self.generated_code.clone(),
            }
        };
        let display_syntax = self.selected_file.syntax();
        // The file `display_code` was built for. Captured before the bottom diag
        // panel (rendered below) can switch `selected_file` on a diagnostic
        // click, so a queued scroll-to-line only fires once the editor actually
        // shows that file (next frame for a cross-file jump).
        let displayed_file = self.selected_file;

        // ── Panel 2: Code Editor ──────────────────────────────────────────────
        let editor_width = ui.available_width() * 0.5;
        egui::Panel::left("code_editor")
            .resizable(true)
            .default_size(editor_width)
            .show_inside(ui, |ui| {
                // Header row
                self.show_editor_toolbar(ui, &display_code, &project_files);

                ui.separator();

                // ── Diagnostics panel (bottom, manually resizable) ────
                // Its top Y bounds the editor region below, so the inline
                // diagnostic overlay can be clipped to what's actually visible.
                let diag_panel_top = self.show_editor_diag_panel(ui);

                // Use a unique id per file so egui's TextEditState (galley,
                // cursor, undo stack) is never shared between files.
                // A fixed id caused the editor to keep the previous file's
                // rendered galley when switching to a new file.
                let editor_id: String = match &self.selected_file {
                    ProjectFileId::UserFile(i) => {
                        let path = self
                            .project_tree
                            .user_src_files
                            .get(*i)
                            .map(|(p, _)| p.as_str())
                            .unwrap_or("?");
                        format!("code_editor:user:{path}")
                    }
                    ProjectFileId::MainRs => "code_editor:main_rs".into(),
                    ProjectFileId::CargoToml => "code_editor:cargo_toml".into(),
                    ProjectFileId::CargoConfig => "code_editor:cargo_config".into(),
                    ProjectFileId::MemoryX => "code_editor:memory_x".into(),
                    ProjectFileId::BuildRs => "code_editor:build_rs".into(),
                    ProjectFileId::GitIgnore => "code_editor:gitignore".into(),
                };

                // ── LSP completion: pre-editor key consumption ───────────────
                // Consume navigation / accept keys BEFORE show_with_completer
                // so the built-in Completer never sees them when our popup is open.
                //
                // Mouse clicks on popup items set `completion_pending_insert` last
                // frame; apply them here so the same accept path is used for both
                // keyboard and mouse.
                let mut lsp_accepted: Option<String> = self.completion_pending_insert.take();
                if lsp_accepted.is_some() {
                    self.completion_open = false;
                }

                if self.completion_open {
                    let has_items = !self.completion_filtered_items.is_empty();
                    if has_items {
                        let count = self.completion_filtered_items.len();
                        ui.input_mut(|inp| {
                            if inp.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                                self.completion_open = false;
                            } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                                // Clamp against the FILTERED count so selection never
                                // goes out of the visible list.
                                self.completion_sel =
                                    (self.completion_sel + 1).min(count.saturating_sub(1));
                            } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                                self.completion_sel = self.completion_sel.saturating_sub(1);
                            } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                                || inp.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                            {
                                // Use the filtered list — guaranteed same items as shown.
                                let sel = self.completion_sel.min(count.saturating_sub(1));
                                if let Some(item) = self.completion_filtered_items.get(sel) {
                                    lsp_accepted = Some(item.insert_text.clone());
                                }
                                self.completion_open = false;
                            }
                        });
                    }
                }
                // ── Cargo.toml completion popup: navigation / accept keys ────
                // Same key set as the LSP popup; consumed before the editor so
                // Enter/Tab don't reach the TextEdit. Accept is deferred through
                // `cargo_complete.pending` (the same path mouse clicks use).
                if self.cargo_complete.open && !self.cargo_complete.items.is_empty() {
                    let count = self.cargo_complete.items.len();
                    ui.input_mut(|inp| {
                        if inp.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                            self.cargo_complete.open = false;
                        } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                            self.cargo_complete.sel =
                                (self.cargo_complete.sel + 1).min(count.saturating_sub(1));
                        } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                            self.cargo_complete.sel = self.cargo_complete.sel.saturating_sub(1);
                        } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                            || inp.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                        {
                            let sel = self.cargo_complete.sel.min(count.saturating_sub(1));
                            if let Some(item) = self.cargo_complete.items.get(sel) {
                                self.cargo_complete.pending = Some(item.action.clone());
                            }
                            self.cargo_complete.open = false;
                        }
                    });
                }
                // Detect Ctrl+Space BEFORE the editor so egui doesn't pass it
                // to the TextEdit as a literal character.
                // These flags are `mut` so the right-click context menu (handled
                // after the editor renders) can drive the exact same code paths.
                let mut ctrl_space_pressed =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Space));
                // Ctrl+/ → toggle line comments on the selection (consumed before
                // the editor so `/` is never typed into the text).
                let mut ctrl_slash_pressed =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Slash));
                // Ctrl+Up / Ctrl+Down → move the selected lines up / down.
                let mut ctrl_up_pressed =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp));
                let mut ctrl_down_pressed =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown));
                // Ctrl+C state (peeked, not consumed — the editor still copies any
                // selection). When the pointer is over a diagnostic, the overlay
                // overwrites the clipboard with the error message instead.
                let copy_requested =
                    ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
                // Ctrl+Shift+X → cut the whole line(s) at the cursor/selection;
                // plain Ctrl+X keeps egui's native cut-the-*selection* behaviour.
                // egui maps BOTH to `Event::Cut` (Shift is ignored for the cut
                // shortcut) and may not deliver a `Key::X` at all — so distinguish
                // by the live Shift state: Shift held → strip the `Event::Cut` so
                // the native selection-cut doesn't fire, and do the whole-line cut
                // ourselves; no Shift → leave the native cut alone. The `consume_key`
                // is a fallback for platforms that send the key instead of a Cut.
                let mut cut_line_pressed = ui.input_mut(|i| {
                    let cut_event = i.events.iter().any(|e| matches!(e, egui::Event::Cut));
                    let line = cut_event && i.modifiers.shift;
                    if line {
                        i.events.retain(|e| !matches!(e, egui::Event::Cut));
                    }
                    let key =
                        i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::X);
                    line || key
                });
                // Ctrl+D → duplicate the line(s) at the cursor / selection.
                let mut ctrl_d_pressed =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::D));
                // Shift+Alt+F → re-indent the whole file by block nesting.
                // (Moved off Ctrl+Shift+F, which now opens project-wide search.)
                // Unlike the Ctrl-based shortcuts, Alt+Shift doesn't suppress the
                // character event, so egui also delivers `Event::Text("F")` — strip
                // it too, or the formatter would type an "F" into the code.
                let mut format_pressed = ui.input_mut(|i| {
                    let pressed =
                        i.consume_key(egui::Modifiers::ALT | egui::Modifiers::SHIFT, egui::Key::F);
                    if pressed {
                        i.events.retain(
                            |e| !matches!(e, egui::Event::Text(t) if t.eq_ignore_ascii_case("f")),
                        );
                    }
                    pressed
                });
                // Ctrl+R → rename the symbol at the cursor project-wide.
                let mut ctrl_r_pressed =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::R));
                // F12 → show the definition of the symbol at the cursor.
                let mut f12_pressed =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F12));

                // Find / Replace bar shortcuts (consumed before the editor so the
                // keys never reach the TextEdit). Shift variants are checked first
                // so Ctrl+Shift+F isn't swallowed by the plain Ctrl+F branch.
                ui.input_mut(|i| {
                    use find_replace::FindMode as M;
                    let ctrl = egui::Modifiers::CTRL;
                    let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
                    if i.consume_key(ctrl_shift, egui::Key::F) {
                        self.find.open_with(M::FindProject);
                    } else if i.consume_key(ctrl_shift, egui::Key::H) {
                        self.find.open_with(M::ReplaceProject);
                    } else if i.consume_key(ctrl, egui::Key::F) {
                        self.find.open_with(M::FindFile);
                    } else if i.consume_key(ctrl, egui::Key::H) {
                        self.find.open_with(M::ReplaceFile);
                    }
                });

                // Ctrl + `+` / `-` / `0` → zoom the editor text in / out / reset.
                // (egui's global keyboard zoom is disabled in `app::new`, so these
                // reach us here.) `consume_key` matches Shift loosely, so Ctrl++
                // (= Ctrl+Shift+=) is caught by the `Plus` arm.
                ui.input_mut(|i| {
                    let cmd = egui::Modifiers::COMMAND;
                    if i.consume_key(cmd, egui::Key::Num0) {
                        self.editor_font_size = DEFAULT_EDITOR_FONT_SIZE;
                    } else if i.consume_key(cmd, egui::Key::Plus)
                        || i.consume_key(cmd, egui::Key::Equals)
                    {
                        self.editor_font_size =
                            (self.editor_font_size + 1.0).min(MAX_EDITOR_FONT_SIZE);
                    } else if i.consume_key(cmd, egui::Key::Minus) {
                        self.editor_font_size =
                            (self.editor_font_size - 1.0).max(MIN_EDITOR_FONT_SIZE);
                    }
                });

                // Find / Replace bar, drawn above the editor when open. Renders
                // before the editor-height calc so the editor sizes below it.
                self.show_find_replace_bar(ui, &mut display_code, displayed_file);

                // Size the editor to fill the height left over after the
                // (resizable) diagnostics panel, so dragging that panel's handle
                // grows/shrinks the code area in lock-step.  `available_height`
                // here is already the space remaining below the toolbar and
                // above the bottom diagnostics panel.
                // The (zoomable) editor font, captured before the mutable
                // `self.completer` borrow below. `row_h` tracks it so the min-row
                // estimate stays right as the user zooms.
                let font_size = self.editor_font_size;
                let row_h = ui
                    .fonts_mut(|f| f.row_height(&egui::FontId::monospace(font_size)))
                    .max(1.0);
                let editor_rows =
                    (((ui.available_height() - 10.0) / row_h).floor() as usize).max(3);

                // The on-screen editor region. `available_rect_before_wrap` does
                // NOT exclude the bottom panel (egui only moves the cursor, not
                // max_rect), so the editor's scroll area actually overflows under
                // the panel. Bound the bottom explicitly to the diagnostics panel's
                // top so the inline overlay can't paint over (or into) it.
                let editor_clip = {
                    let mut r = ui.available_rect_before_wrap();
                    if let Some(top) = diag_panel_top {
                        r.max.y = r.max.y.min(top);
                    }
                    r
                };

                // Rust files (main.rs / user src / build.rs / memory.x) use our
                // lifetime-aware renderer so `'a` doesn't spill the string colour;
                // the `#`-comment config files (Cargo.toml/.cargo/config/.gitignore)
                // keep the stock CodeEditor. Both return a `TextEditOutput`.
                let is_rust_file = !matches!(
                    self.selected_file,
                    ProjectFileId::CargoToml
                        | ProjectFileId::CargoConfig
                        | ProjectFileId::GitIgnore
                );
                let editor_resp = if is_rust_file {
                    crate::editor::gui::code_editor::show_rust_with_completer(
                        ui,
                        &mut display_code,
                        &ColorTheme::GRUVBOX,
                        font_size,
                        editor_rows,
                        &display_syntax,
                        &editor_id,
                        &mut self.completer,
                    )
                } else {
                    CodeEditor::default()
                        .id_source(editor_id.clone())
                        .with_rows(editor_rows)
                        .with_fontsize(font_size)
                        .with_theme(ColorTheme::GRUVBOX)
                        .with_numlines(true)
                        .show_with_completer(
                            ui,
                            &mut display_code,
                            &display_syntax,
                            &mut self.completer,
                        )
                };

                // ── Keep the caret in view when it moves off-screen ───────────
                // egui_code_editor nests a horizontal ScrollArea *inside* the
                // vertical one; the inner area consumes BOTH axes' scroll
                // targets and only applies its own, so egui's own
                // "scroll caret into view" never reaches the outer (vertical)
                // ScrollArea. Result: Shift+Up/Down (or typing) past the visible
                // area extends the selection but the window doesn't follow. We
                // drive the outer ScrollArea's offset ourselves.
                self.scroll_caret_into_view(ui, &editor_resp, &editor_id, editor_clip);
                // Jump to a clicked diagnostic's line (queued by the bottom
                // panel). Runs after caret-follow so its precise offset wins.
                self.apply_pending_scroll(ui, &editor_resp, &editor_id, displayed_file);

                // Highlight every occurrence of the word the user selected
                // (double-click / Ctrl+Shift+Left/Right). Painted here — while
                // `display_code` still matches the galley the editor just built —
                // and before the diagnostics overlay so squiggles render on top.
                Self::highlight_selected_word(&editor_resp, &display_code, editor_clip, ui);
                // Highlight all occurrences of the active find query (current one
                // in amber), so matches show even when the find field has focus.
                self.paint_find_matches(&editor_resp, &display_code, editor_clip, ui);
                // When a `{`/`}` is selected, darken the whole block and let
                // Ctrl+C copy it.
                self.highlight_brace_block(
                    &editor_resp,
                    &display_code,
                    editor_clip,
                    ui,
                    copy_requested,
                );

                // ── Right-click context menu ──────────────────────────────────
                // Lists every editor command with its shortcut. A click drives
                // the same flags the keyboard shortcut sets (so both share one
                // code path); Copy / Select-All are applied directly. The menu
                // acts on the current caret (right-click doesn't move it), which
                // matches the "…where the cursor is" shortcut semantics.
                let is_rs = matches!(
                    self.selected_file,
                    ProjectFileId::MainRs | ProjectFileId::UserFile(_)
                );
                let is_cargo = self.selected_file == ProjectFileId::CargoToml;
                let mut menu_action: Option<context_menu::EditorAction> = None;
                editor_resp.response.context_menu(|ui| {
                    menu_action = context_menu::editor_menu(ui, is_rs, is_cargo);
                });
                {
                    use context_menu::EditorAction as A;
                    match menu_action {
                        Some(A::DeleteLine) => cut_line_pressed = true,
                        Some(A::DuplicateLine) => ctrl_d_pressed = true,
                        Some(A::Comment) => ctrl_slash_pressed = true,
                        Some(A::MoveUp) => ctrl_up_pressed = true,
                        Some(A::MoveDown) => ctrl_down_pressed = true,
                        Some(A::Format) => format_pressed = true,
                        Some(A::Rename) => ctrl_r_pressed = true,
                        Some(A::GoToDef) => f12_pressed = true,
                        Some(A::Completion) => ctrl_space_pressed = true,
                        Some(A::Find) => self.find.open_with(find_replace::FindMode::FindFile),
                        Some(A::Replace) => {
                            self.find.open_with(find_replace::FindMode::ReplaceFile)
                        }
                        Some(A::FindInProject) => {
                            self.find.open_with(find_replace::FindMode::FindProject)
                        }
                        Some(A::ReplaceInProject) => {
                            self.find.open_with(find_replace::FindMode::ReplaceProject)
                        }
                        Some(A::Cut) => {
                            // Cut the selection (mirrors the native Ctrl+X): copy
                            // it, remove it, collapse the cursor to the cut point.
                            if let Some(r) = editor_resp.state.cursor.char_range() {
                                let lo = r.primary.index.min(r.secondary.index);
                                let hi = r.primary.index.max(r.secondary.index);
                                if lo != hi {
                                    let chars: Vec<char> = display_code.chars().collect();
                                    let hi = hi.min(chars.len());
                                    ui.ctx().copy_text(chars[lo..hi].iter().collect::<String>());
                                    let mut new: String = chars[..lo].iter().collect();
                                    new.extend(&chars[hi..]);
                                    display_code = new;
                                    let mut st = editor_resp.state.clone();
                                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                        egui::text::CCursor::new(lo),
                                        egui::text::CCursor::new(lo),
                                    )));
                                    st.store(ui.ctx(), editor_resp.response.id);
                                }
                            }
                        }
                        Some(A::Copy) => {
                            if let Some(r) = editor_resp.state.cursor.char_range() {
                                let lo = r.primary.index.min(r.secondary.index);
                                let hi = r.primary.index.max(r.secondary.index);
                                let chars: Vec<char> = display_code.chars().collect();
                                let text = if lo != hi {
                                    chars[lo..hi.min(chars.len())].iter().collect::<String>()
                                } else {
                                    current_line(&chars, lo)
                                };
                                if !text.is_empty() {
                                    ui.ctx().copy_text(text);
                                }
                            }
                        }
                        Some(A::SelectAll) => {
                            let len = display_code.chars().count();
                            let mut st = editor_resp.state.clone();
                            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(len),
                            )));
                            st.store(ui.ctx(), editor_resp.response.id);
                        }
                        Some(A::ZoomIn) => {
                            self.editor_font_size =
                                (self.editor_font_size + 1.0).min(MAX_EDITOR_FONT_SIZE)
                        }
                        Some(A::ZoomOut) => {
                            self.editor_font_size =
                                (self.editor_font_size - 1.0).max(MIN_EDITOR_FONT_SIZE)
                        }
                        Some(A::ZoomReset) => self.editor_font_size = DEFAULT_EDITOR_FONT_SIZE,
                        None => {}
                    }
                }

                // Ctrl+Shift+X cuts (not just deletes) the line(s): copy them to
                // the clipboard first so they can be pasted, then the line op below
                // removes them.
                if cut_line_pressed {
                    if let Some(r) = editor_resp.state.cursor.char_range() {
                        let lo = r.primary.index.min(r.secondary.index);
                        let hi = r.primary.index.max(r.secondary.index);
                        let cut = delete_line::cut_text(&display_code, lo, hi);
                        if !cut.is_empty() {
                            ui.ctx().copy_text(cut);
                        }
                    }
                }

                // ── Editor line operations on the selection ───────────────────
                // Ctrl+/ toggles line comments (`//` for .rs, `#` for TOML /
                // .gitignore); Ctrl+Up / Ctrl+Down move the selected lines;
                // Ctrl+X deletes the line(s) at the cursor/selection; Ctrl+Shift+F
                // re-indents the whole file by block nesting. Each re-selects /
                // re-positions the cursor so the result persists next frame.
                // Applied before the write-back below so the new text persists;
                // the cursor is stored (on a clone — `store()` consumes the state,
                // which handle_editor_completion still reads) for the next frame.
                let line_op: Option<(String, usize, usize)> = editor_resp
                    .state
                    .cursor
                    .char_range()
                    .and_then(|r| {
                        let lo = r.primary.index.min(r.secondary.index);
                        let hi = r.primary.index.max(r.secondary.index);
                        if ctrl_slash_pressed {
                            Some(comment::toggle_line_comments(
                                &display_code,
                                lo,
                                hi,
                                display_syntax.comment(),
                            ))
                        } else if ctrl_up_pressed {
                            Some(move_lines::move_lines(&display_code, lo, hi, false))
                        } else if ctrl_down_pressed {
                            Some(move_lines::move_lines(&display_code, lo, hi, true))
                        } else if cut_line_pressed {
                            Some(delete_line::delete_lines(&display_code, lo, hi))
                        } else if ctrl_d_pressed {
                            Some(duplicate_line::duplicate_lines(&display_code, lo, hi))
                        } else if format_pressed {
                            let (new, c) = format::format_code(&display_code, lo);
                            Some((new, c, c))
                        } else {
                            None
                        }
                    });
                if let Some((new_code, new_lo, new_hi)) = line_op {
                    display_code = new_code;
                    let mut st = editor_resp.state.clone();
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(new_lo),
                        egui::text::CCursor::new(new_hi),
                    )));
                    st.store(ui.ctx(), editor_resp.response.id);
                }

                // ── Select the current Find match ─────────────────────────────
                // The find bar (drawn above the editor) records the match's char
                // range; apply it to the editor's cursor here, now that we have
                // `editor_resp`. Takes effect next frame (scroll was already queued
                // via `pending_scroll_to_line`).
                if let Some((s, e)) = self.find.pending_select.take() {
                    let mut st = editor_resp.state.clone();
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(s),
                        egui::text::CCursor::new(e),
                    )));
                    st.store(ui.ctx(), editor_resp.response.id);
                    ui.ctx().request_repaint();
                }

                // ── Write user edits back ────────────────────────────────────
                // display_code is a local clone; persist changes here. Use
                // `displayed_file` (the file this text was built for), NOT
                // `self.selected_file`, which the bottom diag panel may have just
                // switched to a different file on a click — writing back to that
                // would overwrite the newly-opened file with this file's content.
                if let ProjectFileId::UserFile(i) = displayed_file {
                    if let Some(entry) = self.project_tree.user_src_files.get_mut(i) {
                        if display_code != entry.1 {
                            // In-memory only; the debounced LSP flush (3 s idle or
                            // Project Save, see app::init_frame) writes it to the
                            // workspace and notifies RA — not on every keystroke.
                            entry.1 = display_code.clone();
                        }
                    }
                } else if displayed_file == ProjectFileId::MainRs
                    && display_code != self.generated_code
                {
                    self.generated_code = display_code.clone();
                } else {
                    // Editable project config files — persist edits to the
                    // matching field (the per-frame snapshot reads them back).
                    let slot = match displayed_file {
                        ProjectFileId::CargoToml => Some(&mut self.cargo_toml),
                        ProjectFileId::CargoConfig => Some(&mut self.cargo_config),
                        ProjectFileId::MemoryX => Some(&mut self.memory_x),
                        ProjectFileId::BuildRs => Some(&mut self.build_rs),
                        ProjectFileId::GitIgnore => Some(&mut self.gitignore),
                        _ => None,
                    };
                    if let Some(slot) = slot {
                        if *slot != display_code {
                            *slot = display_code.clone();
                        }
                    }
                }

                if self.selected_file == ProjectFileId::CargoToml {
                    // Cargo.toml gets crate-name + crates.io-version completion
                    // instead of the rust-analyzer driver.
                    self.handle_cargo_completion(
                        ui,
                        &editor_resp,
                        &mut display_code,
                        ctrl_space_pressed,
                    );
                } else {
                    // Highlight the clicked diagnostic's line (colour keyed by
                    // severity) and the F12 definition line (yellow), but only
                    // while the editor shows the file each belongs to.
                    let highlight: Option<(u32, egui::Color32)> =
                        match self.highlighted_error_line {
                            Some((f, line, color)) if f == displayed_file => {
                                Some((line as u32, color))
                            }
                            _ => None,
                        };
                    let def_line: Option<u32> = match self.highlighted_def_line {
                        Some((f, line)) if f == displayed_file => Some(line as u32),
                        _ => None,
                    };
                    self.handle_editor_completion(
                        ui,
                        &editor_resp,
                        editor_clip,
                        display_code,
                        lsp_accepted,
                        ctrl_space_pressed,
                        copy_requested,
                        ctrl_r_pressed,
                        f12_pressed,
                        highlight,
                        def_line,
                    );
                }
                // Rename input popup (shown while active; sends the request on
                // submit). Rendered after the editor so it overlays the code.
                self.show_rename_popup(ui);
            });
    }

    /// Scroll the editor vertically so the primary caret stays visible when it
    /// moves off-screen (keyboard navigation / selection). Only acts when the
    /// caret actually moved this frame, so it never fights the user scrolling
    /// the wheel away from the caret. See the call site for why egui's built-in
    /// caret-follow doesn't reach the editor's outer (vertical) ScrollArea.
    fn scroll_caret_into_view(
        &mut self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        editor_id: &str,
        visible: egui::Rect,
    ) {
        let Some(range) = editor_resp.state.cursor.char_range() else {
            return;
        };
        let primary = range.primary.index;
        // Only follow when the caret moved (typing / arrows / selection), so the
        // user can still freely scroll the wheel while the caret sits off-screen.
        let moved = self.last_caret_idx != Some(primary);
        self.last_caret_idx = Some(primary);
        if !moved {
            return;
        }

        // Caret rectangle in screen space (galley_pos already includes the
        // current scroll offset).
        let caret = editor_resp
            .galley
            .pos_from_cursor(egui::text::CCursor::new(primary));
        let caret_top = editor_resp.galley_pos.y + caret.min.y;
        let caret_bottom = editor_resp.galley_pos.y + caret.max.y;
        let margin = caret.height().max(8.0); // keep ~one line of context

        // How far (and which way) to move so the caret sits inside the band.
        let delta = if caret_top < visible.top() + margin {
            caret_top - (visible.top() + margin) // negative → scroll up
        } else if caret_bottom > visible.bottom() - margin {
            caret_bottom - (visible.bottom() - margin) // positive → scroll down
        } else {
            0.0
        };
        if delta == 0.0 {
            return;
        }

        // The outer vertical ScrollArea egui_code_editor builds with
        // `id_salt("{id}_outer_scroll")` on this same `ui`.
        let scroll_id = ui
            .id()
            .with(egui::Id::new(format!("{editor_id}_outer_scroll")));
        if let Some(mut state) =
            egui::containers::scroll_area::State::load(ui.ctx(), scroll_id)
        {
            state.offset.y = (state.offset.y + delta).max(0.0);
            state.store(ui.ctx(), scroll_id);
            ui.ctx().request_repaint();
        }
    }

    /// Apply a queued "jump to diagnostic line": scroll the editor so the target
    /// line sits on roughly the 10th row from the top. Only fires once the
    /// editor is displaying the target file (`displayed_file`), so a cross-file
    /// jump waits one frame for the file switch to take effect.
    fn apply_pending_scroll(
        &mut self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        editor_id: &str,
        displayed_file: ProjectFileId,
    ) {
        let Some((file, line_1based)) = self.pending_scroll_to_line else {
            return;
        };
        // Wait until the editor actually shows that file (display_code matches).
        if file != displayed_file {
            return;
        }
        self.pending_scroll_to_line = None;

        // Put the error line on the ~10th visible row (9 lines of context above).
        const ROWS_ABOVE: f32 = 9.0;
        let row_h = editor_resp
            .galley
            .pos_from_cursor(egui::text::CCursor::new(0))
            .height()
            .max(1.0);
        let line0 = line_1based.saturating_sub(1) as f32;
        let offset_y = ((line0 - ROWS_ABOVE) * row_h).max(0.0);

        let scroll_id = ui
            .id()
            .with(egui::Id::new(format!("{editor_id}_outer_scroll")));
        if let Some(mut state) =
            egui::containers::scroll_area::State::load(ui.ctx(), scroll_id)
        {
            state.offset.y = offset_y;
            state.store(ui.ctx(), scroll_id);
            // Suppress caret-follow from snapping back to the (stale) caret.
            self.last_caret_idx = editor_resp.state.cursor.char_range().map(|r| r.primary.index);
            ui.ctx().request_repaint();
        }
    }

    /// Paint a translucent cyan band over every occurrence of the identifier the
    /// user currently has selected — the classic "highlight all references of the
    /// symbol under the cursor" behaviour. A selection appears on a double-click
    /// (selects the word) or Ctrl+Shift+Left/Right (extends by word).
    ///
    /// Only a single whole identifier counts as a "variable": an empty / multi-
    /// token / non-identifier selection paints nothing. Matches are whole-word
    /// (so selecting `x` doesn't light up the `x` inside `max`).
    fn highlight_selected_word(
        editor_resp: &egui::text_edit::TextEditOutput,
        display_code: &str,
        clip: egui::Rect,
        ui: &egui::Ui,
    ) {
        let Some(range) = editor_resp.state.cursor.char_range() else {
            return;
        };
        let lo = range.primary.index.min(range.secondary.index);
        let hi = range.primary.index.max(range.secondary.index);
        if lo == hi {
            return; // no selection — nothing to highlight
        }

        let is_ident = |c: char| c.is_alphanumeric() || c == '_';
        let chars: Vec<char> = display_code.chars().collect();
        if hi > chars.len() {
            return;
        }
        let target = &chars[lo..hi];
        // Reject anything that isn't one identifier token (whitespace, symbols,
        // multi-word selections, or a leading digit → not a variable name).
        if target.iter().any(|&c| !is_ident(c)) || target[0].is_ascii_digit() {
            return;
        }

        // RGB (52, 232, 235) at 20% opacity → alpha ≈ 0.20 × 255 = 51, so the
        // code stays readable through the band (like the diagnostic tints).
        let color = egui::Color32::from_rgba_unmultiplied(52, 232, 235, 51);
        let gp = editor_resp.galley_pos;
        let galley = &editor_resp.galley;
        let painter = ui.painter().with_clip_rect(clip);

        let wl = hi - lo;
        let n = chars.len();
        let mut i = 0;
        while i + wl <= n {
            if &chars[i..i + wl] == target
                && (i == 0 || !is_ident(chars[i - 1]))
                && (i + wl == n || !is_ident(chars[i + wl]))
            {
                // Map the match's char range to a screen rect via the galley.
                let loc_s = galley.pos_from_cursor(egui::text::CCursor::new(i));
                let loc_e = galley.pos_from_cursor(egui::text::CCursor::new(i + wl));
                let y_top = gp.y + loc_s.min.y;
                let y_bot = gp.y + loc_s.max.y;
                // A whole identifier never wraps, but guard anyway: if start/end
                // land on different rows, extend to the line end.
                let same_row = (loc_s.min.y - loc_e.min.y).abs() < (y_bot - y_top).max(1.0) * 0.5;
                let x_l = gp.x + loc_s.min.x;
                let x_r = if same_row {
                    gp.x + loc_e.min.x
                } else {
                    gp.x + galley.rect.width()
                };
                // Skip occurrences scrolled out of the visible editor region.
                if y_bot >= clip.top() && y_top <= clip.bottom() && x_r > x_l {
                    painter.rect_filled(
                        egui::Rect::from_min_max(egui::pos2(x_l, y_top), egui::pos2(x_r, y_bot)),
                        2.0,
                        color,
                    );
                }
                i += wl; // jump past this match
            } else {
                i += 1;
            }
        }
    }
}

/// The text of the line containing char index `idx` (no trailing newline).
/// Used by the context-menu "Copy" when there is no selection.
fn current_line(chars: &[char], idx: usize) -> String {
    let idx = idx.min(chars.len());
    let start = chars[..idx]
        .iter()
        .rposition(|&c| c == '\n')
        .map(|p| p + 1)
        .unwrap_or(0);
    let end = chars[idx..]
        .iter()
        .position(|&c| c == '\n')
        .map(|p| idx + p)
        .unwrap_or(chars.len());
    chars[start..end].iter().collect()
}
