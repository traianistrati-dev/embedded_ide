//! Leftmost "Code Editor" panel (the [Editor][MCU][Project] layout).
//!
//! Owns: the toolbar (Copy + Errors/Types toggles), the code editor widget
//! itself, the embedded bottom diagnostics panel, the LSP completion popup,
//! and the inline-diagnostic overlays.  It also writes the edited text back
//! into `generated_code` (main.rs) or the matching user source file.
//!
//! Implemented as one inherent method on `AppIde`; it borrows the
//! `project_files` snapshot (the project tree, rendered after it, needs the
//! same snapshot).

use super::AppIde;
use super::ProjectFileId;
use crate::lsp;
use crate::panels::mcu_module::project_gen::ProjectFiles;
use eframe::egui;
use egui_code_editor::{CodeEditor, ColorTheme};

mod brace_block;
mod breakpoint_gutter;
pub(crate) mod cargo_complete;
mod code_action;
mod comment;
mod completion;
mod context_menu;
mod debug_hover;
mod delete_line;
mod diag_embed;
pub(crate) mod diff_gutter;
mod doc_md;
mod duplicate_line;
pub(crate) mod file_cycle;
pub(crate) mod find_replace;
mod fold;
mod fold_ui;
mod format;
mod generics;
mod inlay_hint;
mod let_annotation;
mod move_lines;
pub(crate) mod multi_cursor;
mod rename;
mod snippet;
mod toolbar;
pub(crate) mod usages;
mod word_select;

/// Default code-editor font size (points); the zoom baseline (Ctrl+0 resets to it).
pub(crate) const DEFAULT_EDITOR_FONT_SIZE: f32 = 13.0;
/// Zoom clamp range for the editor font.
const MIN_EDITOR_FONT_SIZE: f32 = 7.0;
const MAX_EDITOR_FONT_SIZE: f32 = 40.0;

impl AppIde {
    /// Render the leftmost code editor panel (toolbar + editor + diagnostics).
    pub(super) fn show_editor_panel(
        &mut self,
        ui: &mut egui::Ui,
        project_files: &Option<ProjectFiles>,
    ) {
        // ── Ordering invariant vs. the project tree ───────────────────────────
        // IMPORTANT: the tree panel must NEVER run BETWEEN computing
        // display_code and the end-of-frame write-back — a tree click would
        // switch `selected_file` mid-frame and the write-back would store the
        // OLD file's text into the NEW file. Running the WHOLE editor panel
        // before the tree (the [Editor][MCU][Project] layout since 2026-07-10)
        // keeps the pair atomic: a click this frame takes effect next frame.
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
            match project_files {
                Some(files) => self.selected_file.content(files).to_owned(),
                None => self.generated_code.clone(),
            }
        };
        // The selected file's project-root-relative path, needed wherever a
        // `UserFile` has to be classified by extension — a user file can now be
        // a library crate's `Cargo.toml`, not just Rust source.
        let selected_path = match self.selected_file {
            ProjectFileId::UserFile(i) => self
                .project_tree
                .user_src_files
                .get(i)
                .map(|(p, _)| p.clone())
                .unwrap_or_default(),
            _ => String::new(),
        };
        let display_syntax = self.selected_file.syntax(&selected_path);
        let selected_is_manifest = self.selected_file.is_cargo_manifest(&selected_path);
        // Token re-spacing (Shift+Alt+F) is Rust syntax — `:` → `: `, `,` → `, `
        // would rewrite a linker script (`memory.x`), a .ron or a Markdown table.
        // Those files still get the indent-only pass.
        let respace_on_format = match self.selected_file {
            ProjectFileId::MainRs | ProjectFileId::BuildRs => true,
            ProjectFileId::UserFile(_) => selected_path.ends_with(".rs"),
            _ => false,
        };
        // The file `display_code` was built for. Captured before the bottom diag
        // panel (rendered below) can switch `selected_file` on a diagnostic
        // click, so a queued scroll-to-line only fires once the editor actually
        // shows that file (next frame for a cross-file jump).
        let displayed_file = self.selected_file;

        // ── Panel 1: Code Editor (leftmost) ───────────────────────────────────
        // Cap the width so the editor can never starve the MCU Configurator
        // (the central panel, which has no width of its own and takes only what
        // the side panels leave).
        let avail = ui.available_width();
        // What the other two columns need. The tree's width is last frame's
        // (this panel is built first — see `AppIde::tree_width`); collapsed it
        // costs nothing. Floored at the editor's own minimum, so a window too
        // small for everyone still produces a usable cap instead of one below
        // `min_width`.
        let editor_max =
            (avail - crate::app::MCU_MIN_W - self.tree_width).max(crate::app::EDITOR_MIN_W);
        // Read before the closure borrows `self`.
        let collapsed = self.side_panels_collapsed;
        // The body is bound ONCE and then moved into whichever container runs
        // (only one arm executes, so a single `FnOnce` is fine). Collapsed, the
        // editor IS the central panel and fills the window naturally — no width
        // juggling, and the 70 % cap below simply doesn't apply.
        // NOTE: the body keeps its original indentation so this stays a small,
        // reviewable diff rather than a ~900-line reindent.
        let body = |ui: &mut egui::Ui| {
            // Header row
            self.show_editor_toolbar(ui, &display_code);

            ui.separator();

            // ── Diagnostics panel (bottom, manually resizable) ────
            // Its top Y bounds the editor region below, so the inline
            // diagnostic overlay can be clipped to what's actually visible.
            // `source_rewritten` is set when a Clippy "Fix"/"Apply all"
            // rewrote a source buffer in-place — we then refresh display_code
            // (captured above, before the panel ran) so the editor shows the
            // change and the write-back below doesn't revert it.
            let mut source_rewritten = false;
            let diag_panel_top = self.show_editor_diag_panel(ui, &mut source_rewritten);
            if source_rewritten {
                match displayed_file {
                    ProjectFileId::MainRs => display_code = self.generated_code.clone(),
                    ProjectFileId::UserFile(i) => {
                        if let Some((_, c)) = self.project_tree.user_src_files.get(i) {
                            display_code = c.clone();
                        }
                    }
                    _ => {}
                }
            }

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
            // -- Keyboard-scope gate -------------------------------
            // Every editor shortcut below is consumed GLOBALLY, before
            // any widget sees the key. That was fine while this was the
            // only text field that mattered; with another text input
            // focused (the second Reference editor, the Git commit box,
            // the rename popup, the terminal) those consumes fired on
            // the WRONG file and stole the keystroke from the focused
            // field: Ctrl+Space in the second editor opened the MAIN
            // editor's completion, and accepting it would have edited
            // the other file.
            //
            // Scope rule: shortcuts stay active unless some OTHER
            // text-editing widget owns the keyboard. "Is a text edit"
            // is detected by the focused id having a stored
            // `TextEditState`; a focused button, or no focus at all,
            // keeps the shortcuts live (the pre-second-editor status
            // quo). The editor's and the find bar's own focus come
            // from LAST frame (those widgets render after this code) -
            // a one-frame lag on a focus change is invisible here.
            // The second editor owns the keyboard: shortcuts must not fire
            // here, but Ctrl+Space is FORWARDED to it (it renders later in
            // the frame, so it cannot consume the key itself — the event is
            // swallowed below before any TextEdit sees it).
            let reference_owns_kbd = self.reference_was_focused;
            let editor_kbd_active = !reference_owns_kbd
                && (self.editor_was_focused
                    || self.find.had_focus
                    || match ui.ctx().memory(|m| m.focused()) {
                        None => true,
                        Some(fid) => egui::TextEdit::load_state(ui.ctx(), fid).is_none(),
                    });
            // Close a popup whose OWNER no longer holds the keyboard: it
            // would eat Enter/Escape for a caret the user has left.
            //
            // Scoped BY OWNER, not by `editor_kbd_active`. This panel runs
            // every frame including while the Reference editor has focus —
            // which is exactly when ITS popup is up — so an unscoped close
            // killed that popup one frame after it opened (reported as
            // "the list appears and disappears").
            let owner_lost_kbd = match self.completion_owner {
                crate::app::EditorSlot::Main => !editor_kbd_active,
                crate::app::EditorSlot::Reference => !reference_owns_kbd,
            };
            if owner_lost_kbd {
                self.completion_open = false;
                self.completion_note = None;
            }
            // These two exist only for the main editor.
            if !editor_kbd_active {
                self.cargo_complete.open = false;
                self.code_action_popup_open = false;
            }

            // A deferred accept (mouse click, or a keyboard accept routed
            // from the other editor) — claim it only when the MAIN editor
            // owns the popup, or this would insert the Reference editor's
            // choice into the file shown here.
            let mut lsp_accepted: Option<lsp::CompletionItem> =
                if self.completion_owner == crate::app::EditorSlot::Main {
                    self.completion_pending_insert.take()
                } else {
                    None
                };
            if lsp_accepted.is_some() {
                self.completion_open = false;
            }

            // Popup nav/accept keys are consumed HERE for both editors —
            // this block runs before either renders, and `lsp_accepted` /
            // `completion_pending_insert` carry the choice to the owner.
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
                                // Apply here only if the MAIN editor owns
                                // the popup; otherwise hand it to the
                                // Reference editor through the same
                                // deferred slot mouse accepts already use —
                                // it renders later this frame and applies it
                                // to ITS buffer.
                                if self.completion_owner == crate::app::EditorSlot::Main {
                                    lsp_accepted = Some(item.clone());
                                } else {
                                    self.completion_pending_insert = Some(item.clone());
                                }
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
            // ── Code-action chooser popup: nav / accept keys ─────────────
            // MUST run BEFORE the editor: the popup renders after the editor
            // (`show_code_action_popup`), so if Enter were consumed only there
            // the editor would already have inserted a newline into the code
            // (splitting the identifier the assist targets). Consuming here
            // keeps the accept clean. A choice is deferred to next frame's
            // `poll_code_actions` (so the edit applies at frame top).
            if self.code_action_popup_open && !self.code_actions.is_empty() {
                let count = self.code_actions.len();
                ui.input_mut(|i| {
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                        self.code_action_popup_open = false;
                    } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                        self.code_action_sel = (self.code_action_sel + 1).min(count - 1);
                    } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                        self.code_action_sel = self.code_action_sel.saturating_sub(1);
                    } else if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                        self.code_action_choice = Some(self.code_action_sel.min(count - 1));
                    }
                });
            }
            // ── Inline type hint: Tab accepts (inserts the inferred type) ─
            // Only when a ghost hint is showing, no completion / code-action
            // popup is up (their key handling ran above and would have
            // consumed it first), AND the caret sits on the hint's line
            // at/after the name — so Tab still inserts a tab when indenting at
            // line start. Consumed here so the editor doesn't also type a tab;
            // the edit is applied at frame top next `init_frame`.
            let hint_pos = self.inlay_hint.as_ref().map(|h| (h.line, h.character));
            if let Some((hint_line, hint_char)) = hint_pos {
                let popup_up = self.completion_open
                    || self.code_action_popup_open
                    || (self.cargo_complete.open && !self.cargo_complete.items.is_empty());
                let caret_ok = self.last_caret_idx.map_or(false, |idx| {
                    let (l, c) = crate::editor::gui::text_pos::lsp_cursor_pos(&display_code, idx);
                    l == hint_line && c >= hint_char
                });
                if editor_kbd_active
                    && !popup_up
                    && caret_ok
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab))
                {
                    self.inlay_accept_pending = true;
                }
            }
            // Detect Ctrl+Space BEFORE the editor so egui doesn't pass it
            // to the TextEdit as a literal character.
            // These flags are `mut` so the right-click context menu (handled
            // after the editor renders) can drive the exact same code paths.
            //
            // NOT `consume_key`: holding the shortcut delivers key-REPEAT
            // events every frame, and each one used to re-fire the
            // completion request (clearing the items → the popup flickered
            // open/closed). All Ctrl+Space events are consumed here, but
            // only the initial (non-repeat) press triggers.
            let mut ctrl_space_pressed = false;
            // Forwarded to the Reference editor's own render pass.
            let mut ref_ctrl_space = false;
            ui.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Key {
                        key: egui::Key::Space,
                        pressed: true,
                        repeat,
                        modifiers,
                        ..
                    } if modifiers.ctrl => {
                        if !*repeat {
                            if editor_kbd_active {
                                ctrl_space_pressed = true;
                            } else if reference_owns_kbd {
                                ref_ctrl_space = true;
                            }
                        }
                        false // swallow presses AND repeats
                    }
                    _ => true,
                });
            });
            self.reference_ctrl_space = ref_ctrl_space;
            // Ctrl+/ → toggle line comments on the selection (consumed before
            // the editor so `/` is never typed into the text).
            let mut ctrl_slash_pressed = editor_kbd_active
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Slash));
            // Ctrl+Shift+Q → collapse every function body, or expand everything
            // if anything is folded. Consumed here with the other shortcuts and
            // applied just before the fold projection is built, so the change
            // shows on THIS frame.
            let fold_all_pressed = editor_kbd_active
                && ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Q)
                });
            // Ctrl+Shift+Up / Down → multi-cursor add/undo (see
            // `multi_cursor` module docs). Consumed BEFORE Ctrl+Up/Down
            // below: `consume_key` is lenient about Shift (see that
            // comment), so checking the Shift variant first stops the
            // plain Ctrl+Up/Down (move line) shortcut from also matching
            // the same key-down event.
            let mc_up_pressed = editor_kbd_active
                && ui.input_mut(|i| {
                    i.consume_key(
                        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                        egui::Key::ArrowUp,
                    )
                });
            let mc_down_pressed = editor_kbd_active
                && ui.input_mut(|i| {
                    i.consume_key(
                        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                        egui::Key::ArrowDown,
                    )
                });
            // A PLAIN arrow key moves every caret, not just the primary.
            //
            // Peeked, never consumed: egui's TextEdit needs the very same
            // event to move the primary — we only mirror it onto the
            // extras. Checked AFTER the Ctrl+Shift consumes above, so an
            // "add caret" press is already gone from the queue.
            //
            // Shift+arrow extends every caret's OWN selection; a plain arrow
            // moves and collapses. Ctrl-modified arrows are excluded — those
            // are add-caret (Ctrl+Shift+Up/Down) and move-line (Ctrl+Up/Down),
            // which mean something else entirely.
            let mc_caret_move = ui.input(|i| {
                use multi_cursor::CaretMove;
                let m = i.modifiers;
                if m.ctrl || m.command || m.alt {
                    return None;
                }
                for (key, dir) in [
                    (egui::Key::ArrowLeft, CaretMove::Left),
                    (egui::Key::ArrowRight, CaretMove::Right),
                    (egui::Key::ArrowUp, CaretMove::Up),
                    (egui::Key::ArrowDown, CaretMove::Down),
                ] {
                    if i.key_pressed(key) {
                        return Some((dir, m.shift));
                    }
                }
                None
            });
            let mc_caret_move = mc_caret_move.filter(|_| editor_kbd_active);
            // Escape, peeked twice for two different jobs:
            //  * `escape_pressed_raw` — restore editor focus (see below).
            //  * `mc_escape_pressed`  — drop the extra carets, skipped while
            //    a completion popup is open, because dismissing that wins
            //    (it renders later in the frame and would otherwise never
            //    see the key).
            let escape_pressed_raw = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let popup_open = self.completion_open || self.cargo_complete.open;
            let mc_escape_pressed = editor_kbd_active && !popup_open && escape_pressed_raw;
            // Ctrl+Shift+Tab / Ctrl+Tab → MRU file switching (VS Code style:
            // hold Ctrl to walk the history, release to commit). Consumed
            // BEFORE the editor so Tab never inserts indentation. The Shift
            // variant must be checked first (consume_key is Shift-lenient).
            let mut cycle_prev_pressed = editor_kbd_active
                && ui.input_mut(|i| {
                    i.consume_key(
                        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                        egui::Key::Tab,
                    )
                });
            let mut cycle_next_pressed = editor_kbd_active
                && !cycle_prev_pressed
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Tab));
            // Ctrl+Left / Ctrl+Right (+Shift = select) → word movement.
            // Consumed BEFORE the editor so egui's own word jump never
            // runs: it segments with UAX#29, where `:` is a MidLetter, so
            // `name:Type` is ONE word and the jump swallowed both sides
            // (see `word_select`). The Shift variants are checked first —
            // `consume_key` is Shift-lenient (see the multi-cursor note).
            let word_move: Option<(bool, bool)> = if !editor_kbd_active {
                None
            } else {
                ui.input_mut(|i| {
                    let cs = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
                    if i.consume_key(cs, egui::Key::ArrowRight) {
                        Some((true, true))
                    } else if i.consume_key(cs, egui::Key::ArrowLeft) {
                        Some((false, true))
                    } else if i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowRight) {
                        Some((true, false))
                    } else if i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowLeft) {
                        Some((false, false))
                    } else {
                        None
                    }
                })
            };
            // Ctrl+Up / Ctrl+Down → move the selected lines up / down.
            let mut ctrl_up_pressed = editor_kbd_active
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp));
            let mut ctrl_down_pressed = editor_kbd_active
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown));
            // Ctrl+C state (peeked, not consumed — the editor still copies any
            // selection). When the pointer is over a diagnostic, the overlay
            // overwrites the clipboard with the error message instead.
            let copy_requested = editor_kbd_active
                && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
            // Ctrl+Shift+X → cut the whole line(s) at the cursor/selection;
            // plain Ctrl+X keeps egui's native cut-the-*selection* behaviour.
            // egui maps BOTH to `Event::Cut` (Shift is ignored for the cut
            // shortcut) and may not deliver a `Key::X` at all — so distinguish
            // by the live Shift state: Shift held → strip the `Event::Cut` so
            // the native selection-cut doesn't fire, and do the whole-line cut
            // ourselves; no Shift → leave the native cut alone. The `consume_key`
            // is a fallback for platforms that send the key instead of a Cut.
            let mut cut_line_pressed = editor_kbd_active
                && ui.input_mut(|i| {
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
            let mut ctrl_d_pressed = editor_kbd_active
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::D));
            // Shift+Alt+F → re-indent the whole file by block nesting.
            // (Moved off Ctrl+Shift+F, which now opens project-wide search.)
            // Unlike the Ctrl-based shortcuts, Alt+Shift doesn't suppress the
            // character event, so egui also delivers `Event::Text("F")` — strip
            // it too, or the formatter would type an "F" into the code.
            let mut format_pressed = editor_kbd_active
                && ui.input_mut(|i| {
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
            let mut ctrl_r_pressed = editor_kbd_active
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::R));
            // Ctrl+F12 → go to the IMPLEMENTATION of the symbol at the
            // cursor (the `impl … for …` site, where plain F12 on a trait
            // method lands on the trait's declaration). Consumed before
            // plain F12 so the Ctrl variant never falls through.
            let mut ctrl_f12_pressed = editor_kbd_active
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::F12));
            // Ctrl+[ / Ctrl+] → select the innermost `{ … }` block around
            // the caret and copy it (refactored off the old implicit
            // trigger — selecting a `{`/`}` — which hijacked Ctrl+C).
            let mut select_block_pressed = editor_kbd_active
                && ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::CTRL, egui::Key::OpenBracket)
                        || i.consume_key(egui::Modifiers::CTRL, egui::Key::CloseBracket)
                });
            // Ctrl+Enter → rust-analyzer code actions (assists / quick-fixes)
            // at the cursor. Consumed before the editor so it never inserts
            // a newline. Ignored while the code-action popup is already open
            // (its own Enter handling wins there).
            let ctrl_enter_pressed = editor_kbd_active
                && !self.code_action_popup_open
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Enter));
            // F12 → show the definition of the symbol at the cursor.
            let mut f12_pressed = editor_kbd_active
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F12));

            // Find / Replace bar shortcuts (consumed before the editor so the
            // keys never reach the TextEdit). Shift variants are checked first
            // so Ctrl+Shift+F isn't swallowed by the plain Ctrl+F branch.
            // Replace opens PRE-FILLED with the identifier under the cursor
            // (query searches for it, replace field starts from it + gets
            // focus) — quick rename of the symbol you're on.
            let word_under_cursor = self
                .last_caret_idx
                .map(|idx| rename::identifier_at(&display_code, idx))
                .unwrap_or_default();
            if editor_kbd_active {
                ui.input_mut(|i| {
                    use find_replace::FindMode as M;
                    let ctrl = egui::Modifiers::CTRL;
                    let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
                    if i.consume_key(ctrl_shift, egui::Key::F) {
                        self.find.open_with(M::FindProject);
                    } else if i.consume_key(ctrl_shift, egui::Key::H) {
                        self.find
                            .open_replace_with_word(M::ReplaceProject, &word_under_cursor);
                    } else if i.consume_key(ctrl, egui::Key::F) {
                        self.find.open_with(M::FindFile);
                    } else if i.consume_key(ctrl, egui::Key::H) {
                        self.find
                            .open_replace_with_word(M::ReplaceFile, &word_under_cursor);
                    }
                });
            }

            // Ctrl + `+` / `-` / `0` → zoom the editor text in / out / reset.
            // (egui's global keyboard zoom is disabled in `app::new`, so these
            // reach us here.) `consume_key` matches Shift loosely, so Ctrl++
            // (= Ctrl+Shift+=) is caught by the `Plus` arm.
            // Consumed ONLY while the pointer is over the editor panel — the
            // Structure diagram has its own Ctrl+± zoom, routed by hover.
            if ui.rect_contains_pointer(ui.max_rect()) {
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
            }

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
            let editor_rows = (((ui.available_height() - 10.0) / row_h).floor() as usize).max(3);

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
                ProjectFileId::CargoToml | ProjectFileId::CargoConfig | ProjectFileId::GitIgnore
            ) && !selected_is_manifest;
            // While our LSP completion popup is open (or Ctrl+Space was just
            // pressed to open it), hide the crate's built-in keyword popup so
            // the two don't overlap — the LSP popup is the one that wins.
            // On top of that, the keyword popup is DISABLED outright (user
            // request 2026-07-05: no auto-popup while typing; completion is
            // on-demand via Ctrl+Space / `.` / `::`). Flip the const to
            // bring the auto keyword popup back — nothing was removed.
            const KEYWORD_COMPLETER_ENABLED: bool = false;
            let suppress_keyword_completer =
                !KEYWORD_COMPLETER_ENABLED || self.completion_open || ctrl_space_pressed;

            // ── Live "usages" analysis (fade unused fn/struct/enum/const/…,
            // offer a references popup on the rest) — RA `documentSymbol` +
            // `references`, debounced, kept fresh only for the exact text
            // shown below. `usages_rel_path` is also reused after the editor
            // to place the "N refs" pill overlay.
            let usages_rel_path = crate::editor::gui::text_pos::selected_file_rel_path(
                &self.selected_file,
                &self.project_tree.user_src_files,
            );
            let dead_ranges: Vec<(usize, usize)> = match &usages_rel_path {
                Some(rel) if is_rust_file => {
                    self.tick_usages(rel, &display_code);
                    self.usages_dead_ranges(rel, &display_code)
                }
                _ => Vec::new(),
            };
            // Generic parameters the item declares without using, that an `impl`
            // of it does use: underlined instead of faded (they are live code).
            let underline_ranges: Vec<(usize, usize)> =
                self.generic_underline_ranges(&display_code).to_vec();

            // ── Code folding ──────────────────────────────────────────────
            // An edit and a fold cannot coexist: the editor writes back the
            // text it was GIVEN, which while folded is a projection missing
            // whole lines — writing that back would delete the hidden code.
            // So any keystroke that could modify the buffer unfolds this file
            // FIRST, before the editor renders, and the keystroke then lands on
            // the full text as usual. Everything below therefore runs either
            // fully folded (and read-only for this frame) or not folded at all.
            let fold_key = usages_rel_path.clone().filter(|_| is_rust_file);
            if let Some(rel) = &fold_key {
                // The line-op shortcuts CONSUMED their key events further up, so
                // `edit_pending` can no longer see them — their flags have to be
                // checked directly. Each one rewrites the buffer using caret
                // indices taken from the galley, which while folded belongs to
                // the projection: without this they would edit the wrong lines.
                let line_op = ctrl_slash_pressed
                    || ctrl_up_pressed
                    || ctrl_down_pressed
                    || cut_line_pressed
                    || ctrl_d_pressed
                    || format_pressed
                    || mc_up_pressed
                    || mc_down_pressed;
                let editing = editor_kbd_active && (line_op || fold::edit_pending(ui));
                if editing && self.folds.contains_key(rel) {
                    self.folds.remove(rel);
                }
                // Ctrl+Shift+Q, applied before the projection below so it takes
                // effect this frame. After the unfold-on-edit check: a frame
                // that both edits and toggles should end up unfolded.
                if fold_all_pressed && !editing {
                    let current = self.folds.get(rel).cloned().unwrap_or_default();
                    let next = fold::toggle_all(&display_code, &current);
                    if next.is_empty() {
                        self.folds.remove(rel);
                    } else {
                        self.folds.insert(rel.clone(), next);
                    }
                }
            }
            let fold_map = match &fold_key {
                Some(rel) => match self.folds.get(rel) {
                    Some(set) if !set.is_empty() => fold::FoldMap::new(&display_code, set),
                    _ => fold::FoldMap::identity(&display_code),
                },
                None => fold::FoldMap::identity(&display_code),
            };
            let folded = !fold_map.is_identity();

            // Snapshot right before the editor mutates `display_code`, so
            // the multi-cursor replay below can diff exactly what the
            // editor itself changed this frame (typing / backspace / paste)
            // — not any earlier same-frame mutation like the find/replace
            // bar's own edits, above.
            let text_before_typing = display_code.clone();

            // While folded the editor is handed the projection and its result is
            // discarded; `display_code` keeps holding the real buffer for the
            // write-back and for every analysis below.
            let mut editor_text = if folded {
                fold_map.display().to_owned()
            } else {
                display_code.clone()
            };
            let editor_resp = if is_rust_file {
                crate::editor::gui::code_editor::show_rust_with_completer(
                    ui,
                    &mut editor_text,
                    &ColorTheme::GRUVBOX,
                    font_size,
                    editor_rows,
                    &display_syntax,
                    &editor_id,
                    &mut self.completer,
                    suppress_keyword_completer,
                    crate::editor::gui::code_editor::Marks {
                        dead: &fold_map.map_ranges(&dead_ranges),
                        underline: &fold_map.map_ranges(&underline_ranges),
                    },
                    fold_map.line_numbers(),
                )
            } else {
                // Config files (Cargo.toml, .cargo/config.toml, .gitignore)
                // on the stock editor. `show_with_completer` would drive the
                // crate's keyword popup unconditionally — which is how
                // Cargo.toml kept popping up a list of Rust keywords + words
                // from the file on EVERY character typed, ignoring the
                // `suppress_keyword_completer` decision the Rust path honours.
                // Inline the two completer calls instead, behind the same
                // flag. Cargo.toml keeps its OWN crate/version completion on
                // Ctrl+Space (`handle_cargo_completion`), untouched by this.
                let mut out = CodeEditor::default()
                    .id_source(editor_id.clone())
                    .with_rows(editor_rows)
                    .with_fontsize(font_size)
                    .with_theme(ColorTheme::GRUVBOX)
                    .with_numlines(true)
                    .show(ui, &mut display_code, &display_syntax);
                // Set even when suppressed: the completer keys its popup to
                // this id, and a stale one from another editor would misplace
                // it the moment the flag flips back on.
                self.completer.text_edit_id = Some(out.response.id);
                if !suppress_keyword_completer {
                    self.completer.handle_input(ui.ctx());
                    self.completer
                        .show(&display_syntax, &ColorTheme::GRUVBOX, font_size, &mut out);
                }
                out
            };
            // Adopt what the editor produced — but ONLY on the Rust path, which
            // is the one handed `editor_text`. A config file (Cargo.toml,
            // memory.x, .gitignore) goes through the stock `CodeEditor` in the
            // branch above, which edits `display_code` DIRECTLY; assigning
            // `editor_text` over it there wrote back the pre-edit clone and
            // erased every keystroke as it was typed.
            //
            // Folded: the editor was handed a projection — discard it (nothing
            // can have edited it; see the unfold-first rule above).
            if is_rust_file && !folded {
                display_code = editor_text;
            }

            // ── Keep the caret in view when it moves off-screen ───────────
            // egui_code_editor nests a horizontal ScrollArea *inside* the
            // vertical one; the inner area consumes BOTH axes' scroll
            // targets and only applies its own, so egui's own
            // "scroll caret into view" never reaches the outer (vertical)
            // ScrollArea. Result: Shift+Up/Down (or typing) past the visible
            // area extends the selection but the window doesn't follow. We
            // drive the outer ScrollArea's offset ourselves.
            // A fold toggled last frame: put its header back at the same screen
            // position before anything else touches the scroll offset.
            if let Some(rel) = &fold_key {
                self.apply_fold_anchor(ui, &editor_resp, &editor_id, &fold_map, rel);
            }
            self.scroll_caret_into_view(ui, &editor_resp, &editor_id, editor_clip);
            // Jump to a clicked diagnostic's line (queued by the bottom
            // panel). Runs after caret-follow so its precise offset wins.
            self.apply_pending_scroll(ui, &editor_resp, &editor_id, displayed_file);

            // Everything from here on pairs `display_code` (the buffer) with the
            // galley the editor just built. While folded those two describe
            // different texts, so an overlay would paint on the wrong line —
            // they are skipped for that frame rather than lied to. The fade and
            // underline marks are unaffected: they went into the layout already
            // translated (`fold_map.map_ranges`).
            if !folded {
                // Highlight every occurrence of the word the user selected
                // (double-click / Ctrl+Shift+Left/Right). Painted here — while
                // `display_code` still matches the galley the editor just built —
                // and before the diagnostics overlay so squiggles render on top.
                // Double-click: replace egui's UAX#29 word selection (which
                // glues `name:Type` into one "word" via the `:` MidLetter rule)
                // with the plain identifier run under the pointer.
                self.fix_double_click_selection(ui, &editor_resp, &display_code);
                // Ctrl(+Shift)+Left/Right: our own word jump, for the same
                // reason — the keys were consumed before the editor rendered.
                if let Some((right, extend)) = word_move {
                    self.apply_word_move(ui, &editor_resp, &display_code, right, extend);
                }
                Self::highlight_selected_word(&editor_resp, &display_code, editor_clip, ui);
                // Highlight all occurrences of the active find query (current one
                // in amber), so matches show even when the find field has focus.
                self.paint_find_matches(&editor_resp, &display_code, editor_clip, ui);
                // Triple-clicking a `{`/`}` or a definition's header line
                // highlights the WHOLE definition in white and copies it on
                // Ctrl+C. (The single-block highlight moved off "selecting a
                // brace" to the explicit Ctrl+[ / Ctrl+] shortcut, applied
                // after the context menu below.)
                self.highlight_full_definition(
                    &editor_resp,
                    &display_code,
                    displayed_file,
                    editor_clip,
                    ui,
                    copy_requested,
                );
                // Unused generic parameters pulse a translucent white highlight on
                // top of their fade. Drawn before the "N refs" pills so a pill can
                // never end up under the wash.
                generics::show_unused_generics_overlay(
                    ui,
                    editor_resp.galley_pos,
                    editor_clip,
                    &editor_resp.galley,
                    &display_code,
                    self.generic_pulse_ranges(&display_code),
                );
                // …and the underlined ones explain themselves on hover.
                generics::show_impl_only_tooltips(
                    ui,
                    editor_resp.galley_pos,
                    editor_clip,
                    &editor_resp.galley,
                    &display_code,
                    &underline_ranges,
                );
                // "N refs" indicator + popup on every used item (unused ones were
                // already faded by the highlighter, above, via `dead_ranges`).
                if let Some(rel) = &usages_rel_path {
                    self.show_usages_overlay(
                        ui,
                        editor_resp.galley_pos,
                        editor_clip,
                        &editor_resp.galley,
                        &display_code,
                        rel,
                    );
                }
            } // end `if !folded` — galley-dependent overlays

            // ── Multi-cursor (Ctrl+Shift+Up/Down) ─────────────────────────
            // Add/remove an extra caret, then replay this frame's text edit
            // (if any) at every one of them — mutates `display_code` further.
            // When it does, the line-op shortcuts below are skipped for this
            // frame: they assume a single cursor/selection, and `editor_resp`
            // still reflects positions from BEFORE this replay.
            // ── Escape must never eject the caret from the editor ─────────
            // egui drops the focused widget on Escape in `Focus::begin_pass`
            // — raw events, start of the pass, before any widget or app code
            // — so consuming the key cannot prevent it; the only cure is to
            // take focus back afterwards.
            //
            // In the code editor Escape means "dismiss the popup" or "drop
            // the extra carets", never "leave the editor", so restore it
            // whenever the editor was the focused widget LAST frame. That
            // check matters: without it, an Escape pressed in the Find bar
            // or a tree rename box would yank focus INTO the editor. The
            // flag is forced true when we restore, because `has_focus()` is
            // still false on this very frame — otherwise a second Escape in
            // a row would find it false and give up.
            if escape_pressed_raw && self.editor_was_focused {
                editor_resp.response.request_focus();
                self.editor_was_focused = true;
            } else {
                self.editor_was_focused = editor_resp.response.has_focus();
            }

            let mc_shift = self.handle_multi_cursor(
                &mut display_code,
                &text_before_typing,
                &editor_resp,
                displayed_file,
                mc_up_pressed,
                mc_down_pressed,
                mc_escape_pressed,
                mc_caret_move,
            );
            let mc_replayed = mc_shift.is_some();
            // The primary caret's own edit already landed correctly, but an
            // extra caret ABOVE it (the only place Ctrl+Shift+Up ever adds
            // one) may have changed the buffer's length before it — shift
            // egui's stored cursor to match, or it visibly drifts the next
            // time something is typed. Applies from next frame (this
            // frame's caret was already painted using the un-shifted
            // position — a one-frame lag, same as `apply_pending_scroll`
            // elsewhere in this file).
            if let Some(shift) = mc_shift.filter(|&s| s != 0) {
                if let Some(r) = editor_resp.state.cursor.char_range() {
                    let new_idx = (r.primary.index as isize + shift).max(0) as usize;
                    let mut st = editor_resp.state.clone();
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(new_idx),
                    )));
                    st.store(ui.ctx(), editor_resp.response.id);
                }
            }
            // Same rule as the overlay block above: these all pair the buffer
            // with the folded galley, so they sit out a folded frame.
            if !folded {
                self.paint_extra_cursors(
                    ui,
                    editor_resp.galley_pos,
                    editor_clip,
                    &editor_resp.galley,
                    &display_code,
                );
                self.paint_primary_caret(ui, &editor_resp, editor_clip);
                // Git gutter marks (live diff vs HEAD, sees unsaved edits) +
                // click-to-revert. A revert mutates `display_code`; the
                // write-back below persists it (same as the context-menu Cut).
                self.tick_diff_gutter(&display_code);
                self.paint_diff_gutter(ui, &editor_resp, editor_clip, &display_code);
                // Breakpoint dots + click-to-toggle in the line-number column.
                self.paint_breakpoint_gutter(ui, &editor_resp, editor_clip, &display_code);
                // Hover-to-evaluate: value tooltip for the identifier under the
                // pointer while a debug session is halted.
                self.paint_debug_hover(ui, &editor_resp, editor_clip, &display_code);
            }

            // Fold carets + the "N lines hidden" badge. LAST on purpose: they
            // share the number column with the breakpoint strip, and egui gives
            // a click to the widget registered latest — so the caret wins the
            // primary button while the strip keeps the secondary one. Outside
            // the `!folded` guard, since unfolding must stay possible.
            if let Some(rel) = fold_key.clone() {
                self.paint_fold_gutter(
                    ui,
                    &editor_resp,
                    editor_clip,
                    &display_code,
                    &fold_map,
                    &rel,
                    font_size,
                );
            }

            // ── Ctrl+Enter code actions (RA assists / quick-fixes) ────────
            if ctrl_enter_pressed {
                let cursor_idx = editor_resp
                    .state
                    .cursor
                    .char_range()
                    .map(|r| r.primary.index);
                let anchor = editor_resp
                    .state
                    .cursor
                    .char_range()
                    .map(|cr| {
                        let clamped = cr.primary.index.min(
                            editor_resp
                                .galley
                                .job
                                .text
                                .chars()
                                .count()
                                .saturating_sub(1),
                        );
                        let local = editor_resp
                            .galley
                            .pos_from_cursor(egui::text::CCursor::new(clamped));
                        editor_resp.response.rect.left_top()
                            + local.min.to_vec2()
                            + egui::vec2(0.0, local.height() + 4.0)
                    })
                    .unwrap_or_else(|| editor_resp.response.rect.left_top());
                self.trigger_code_actions(&display_code, cursor_idx, anchor);
            }
            self.show_code_action_popup(ui);

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
            let is_cargo = selected_is_manifest;
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
                    Some(A::NextFile) => cycle_next_pressed = true,
                    Some(A::PrevFile) => cycle_prev_pressed = true,
                    Some(A::Format) => format_pressed = true,
                    Some(A::ToggleFoldAll) => {
                        // Applied straight to the state, not via the keyboard
                        // flag: this runs AFTER the fold projection was built,
                        // so it lands on the next frame either way.
                        if let Some(rel) = &fold_key {
                            let current = self.folds.get(rel).cloned().unwrap_or_default();
                            let next = fold::toggle_all(&display_code, &current);
                            if next.is_empty() {
                                self.folds.remove(rel);
                            } else {
                                self.folds.insert(rel.clone(), next);
                            }
                        }
                    }
                    Some(A::Rename) => ctrl_r_pressed = true,
                    Some(A::GoToDef) => f12_pressed = true,
                    Some(A::GoToImpl) => ctrl_f12_pressed = true,
                    Some(A::AddWatch) => {
                        // Prefer the current selection (lets you watch an
                        // expression like `self.buf[0]`); else the identifier
                        // under the caret. Reveal the Debug tab so the new
                        // watch is visible.
                        let expr = editor_resp
                            .state
                            .cursor
                            .char_range()
                            .and_then(|r| {
                                let lo = r.primary.index.min(r.secondary.index);
                                let hi = r.primary.index.max(r.secondary.index);
                                (lo != hi).then(|| {
                                    let chars: Vec<char> = display_code.chars().collect();
                                    chars[lo..hi.min(chars.len())].iter().collect::<String>()
                                })
                            })
                            .unwrap_or_else(|| word_under_cursor.clone());
                        if !expr.trim().is_empty() {
                            self.debugger.add_watch(expr);
                            self.build_tab = crate::app::BuildPanelTab::Debug;
                        }
                    }
                    Some(A::SelectBlock) => select_block_pressed = true,
                    Some(A::Completion) => ctrl_space_pressed = true,
                    Some(A::Find) => self.find.open_with(find_replace::FindMode::FindFile),
                    Some(A::Replace) => self.find.open_replace_with_word(
                        find_replace::FindMode::ReplaceFile,
                        &word_under_cursor,
                    ),
                    Some(A::FindInProject) => {
                        self.find.open_with(find_replace::FindMode::FindProject)
                    }
                    Some(A::ReplaceInProject) => self.find.open_replace_with_word(
                        find_replace::FindMode::ReplaceProject,
                        &word_under_cursor,
                    ),
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

            // ── Ctrl+[ / Ctrl+] — select + copy the block at the caret ────
            // After the context-menu mapping so both paths land here.
            if select_block_pressed {
                self.select_brace_block(ui, &editor_resp, &display_code);
            }

            // ── MRU file switching (Ctrl+Tab / Ctrl+Shift+Tab) ────────────
            // Runs after the context-menu mapping so both paths land here.
            // Switching mid-frame is safe: the write-back below persists to
            // the captured `displayed_file`, and the editor shows the new
            // file next frame (same as tree-click / diagnostics nav).
            if cycle_next_pressed || cycle_prev_pressed {
                // Drop stale entries first (deleted files, toolchain-hidden
                // fixed files) so session indices stay valid throughout.
                let rust_embedded = matches!(
                    self.selected_toolchain(),
                    Some(crate::panels::mcu_module::mcu_catalog::ToolchainKind::RustEmbedded)
                );
                let user_files = &self.project_tree.user_src_files;
                self.file_cycle.purge(|e| match e {
                    file_cycle::HistEntry::User(p) => user_files.iter().any(|(q, _)| q == p),
                    file_cycle::HistEntry::Fixed(
                        ProjectFileId::MemoryX | ProjectFileId::BuildRs,
                    ) => rust_embedded,
                    file_cycle::HistEntry::Fixed(_) => true,
                });
                if let Some(entry) = self.file_cycle.begin_or_step(cycle_next_pressed) {
                    if let Some(id) = entry.to_id(&self.project_tree.user_src_files) {
                        self.selected_file = id;
                        // Don't re-note during the session — promotion
                        // happens once, on commit (Ctrl release).
                        self.last_selected_file = id;
                    }
                }
            }
            // Commit the session once Ctrl is released; keep repainting
            // while it's open so the release is noticed promptly and the
            // overlay below stays live.
            if self.file_cycle.is_cycling() {
                if !ui.input(|i| i.modifiers.ctrl) {
                    self.file_cycle.commit();
                } else {
                    show_file_cycle_overlay(ui.ctx(), &self.file_cycle);
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(50));
                }
            }

            // Ctrl+Shift+X cuts (not just deletes) the line(s): copy them to
            // the clipboard first so they can be pasted, then the line op below
            // removes them. Skipped when multi-cursor already replayed this
            // frame's edit — `editor_resp`'s positions are stale relative to
            // the just-mutated `display_code` (see `mc_replayed` above).
            if cut_line_pressed && !mc_replayed {
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
            // Skipped entirely when `mc_replayed` (see above).
            let line_op: Option<(String, usize, usize)> = if mc_replayed {
                None
            } else {
                editor_resp.state.cursor.char_range().and_then(|r| {
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
                        let (new, c) = format::format_code(&display_code, lo, respace_on_format);
                        Some((new, c, c))
                    } else {
                        None
                    }
                })
            };
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
            } else if displayed_file == ProjectFileId::MainRs && display_code != self.generated_code
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

            // Any Cargo manifest — the firmware's, or an extracted
            // library's (a plain user file at `<crate>/Cargo.toml`). Gating
            // on the `CargoToml` id alone left library manifests on the
            // rust-analyzer path, so Ctrl+Space there did nothing.
            if selected_is_manifest {
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
                let highlight: Option<(u32, egui::Color32)> = match self.highlighted_error_line {
                    Some((f, line, color)) if f == displayed_file => Some((line as u32, color)),
                    _ => None,
                };
                let def_line: Option<u32> = match self.highlighted_def_line {
                    Some((f, line)) if f == displayed_file => Some(line as u32),
                    _ => None,
                };
                let pin_pulse = self.pin_pulse_bands(ui.ctx(), displayed_file);
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
                    ctrl_f12_pressed,
                    highlight,
                    def_line,
                    pin_pulse,
                    crate::app::EditorSlot::Main,
                    displayed_file,
                );
            }
            // Rename input popup (shown while active; sends the request on
            // submit). Rendered after the editor so it overlays the code.
            self.show_rename_popup(ui);
        };

        if collapsed {
            // The MCU Configurator is hidden, so the editor takes the central
            // slot and fills everything the Project tree (a right panel added
            // before this) leaves — no width juggling, and the 70 % cap below
            // doesn't apply.
            egui::CentralPanel::default().show_inside(ui, body);
        } else {
            // `editor_max` (computed above) reserves the MCU zone's minimum and
            // the tree's current width. It replaced a flat 70 % of the window,
            // which said nothing about whether what was left could actually hold
            // a chip diagram: on a half-screen window the editor kept a width
            // dragged while maximised and squeezed the MCU zone to a strip.
            egui::Panel::left("code_editor")
                .resizable(true)
                .default_size(avail * 0.5)
                .min_width(crate::app::EDITOR_MIN_W)
                .max_width(editor_max)
                .show_inside(ui, body);
        }
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
        // Clamp to the galley: a stale cursor (e.g. left past the end of a file
        // that just shrank from a Clippy "Fix") would otherwise make
        // `pos_from_cursor` index out of bounds and panic.
        let primary = range
            .primary
            .index
            .min(editor_resp.galley.text().chars().count());
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
        if let Some(mut state) = egui::containers::scroll_area::State::load(ui.ctx(), scroll_id) {
            state.offset.y = (state.offset.y + delta).max(0.0);
            state.store(ui.ctx(), scroll_id);
            ui.ctx().request_repaint();
        }
    }

    /// Paint the primary caret ourselves, on top of the editor.
    ///
    /// egui's TextEdit draws its caret only while `input.focused` — the
    /// OS-window focus flag — is true. On Windows that flag goes stale when a
    /// `Focused(true)` event is missed (observed at app start and after
    /// Alt+Tab): typing still works (the widget keeps egui focus) but the
    /// caret is invisible. Painting it here, gated only on WIDGET focus, makes
    /// it impossible to lose; when egui's own caret does draw, the two overlap
    /// pixel-for-pixel (same galley position, same colour, same width formula
    /// as the theme's `modify_style`).
    fn paint_primary_caret(
        &self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        clip: egui::Rect,
    ) {
        if !editor_resp.response.has_focus() {
            return;
        }
        let Some(range) = editor_resp.state.cursor.char_range() else {
            return;
        };
        // Clamp against a stale cursor index (file may have just shrunk).
        let idx = range
            .primary
            .index
            .min(editor_resp.galley.text().chars().count());
        let loc = editor_resp
            .galley
            .pos_from_cursor(egui::text::CCursor::new(idx));
        let x = editor_resp.galley_pos.x + loc.min.x;
        let y_top = editor_resp.galley_pos.y + loc.min.y;
        let y_bot = editor_resp.galley_pos.y + loc.max.y;
        if y_bot < clip.top() || y_top > clip.bottom() {
            return;
        }
        // Same width the editor theme gives egui's caret (`fontsize * 0.1`),
        // same colour egui would use.
        let stroke = egui::Stroke::new(
            self.editor_font_size * 0.1,
            ui.visuals().text_cursor.stroke.color,
        );
        ui.painter()
            .with_clip_rect(clip)
            .line_segment([egui::pos2(x, y_top), egui::pos2(x, y_bot)], stroke);
    }

    /// The lines + band colour of the "here is your pin" pulse on
    /// `displayed_file` — one line for a pin click, one per wired pin for a
    /// module click. Empty when there is nothing to pulse.
    ///
    /// The alpha follows a sine so the bands fade in and out instead of blinking
    /// on/off, and the whole highlight clears itself after `PIN_PULSE_SECS` — a
    /// permanent stripe would just become another thing to dismiss. Repaints are
    /// requested while it runs, otherwise egui would idle mid-pulse.
    fn pin_pulse_bands(
        &mut self,
        ctx: &egui::Context,
        displayed_file: ProjectFileId,
    ) -> Vec<(u32, egui::Color32)> {
        let Some(hl) = &self.highlighted_pin_lines else {
            return Vec::new();
        };
        let elapsed = ctx.input(|i| i.time) - hl.start;
        if elapsed >= crate::app::PIN_PULSE_SECS {
            self.highlighted_pin_lines = None;
            return Vec::new();
        }
        ctx.request_repaint();
        if hl.file != displayed_file {
            return Vec::new(); // still counting down, just not on screen
        }
        let phase = (elapsed * std::f64::consts::TAU * crate::app::PIN_PULSE_HZ).sin();
        let alpha = ((0.5 + 0.5 * phase) as f32 * crate::app::PIN_PULSE_ALPHA) as u8;
        let color = egui::Color32::from_rgba_unmultiplied(255, 214, 90, alpha);
        hl.lines.iter().map(|l| (*l as u32, color)).collect()
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
        if let Some(mut state) = egui::containers::scroll_area::State::load(ui.ctx(), scroll_id) {
            state.offset.y = offset_y;
            state.store(ui.ctx(), scroll_id);
            // Suppress caret-follow from snapping back to the (stale) caret.
            self.last_caret_idx = editor_resp
                .state
                .cursor
                .char_range()
                .map(|r| r.primary.index);
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

/// Floating "recent files" list shown while a Ctrl+Tab cycling session is
/// active (Ctrl held): the MRU history with the current target highlighted —
/// without it the user can't see what they're cycling through.
fn show_file_cycle_overlay(ctx: &egui::Context, fc: &file_cycle::FileCycle) {
    use egui_phosphor::regular as ph;
    let (entries, cursor) = fc.view();
    egui::Area::new(egui::Id::new("__file_cycle_overlay__"))
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Recent files — Tab: next · Shift+Tab: back · release Ctrl: open",
                    )
                    .size(10.0)
                    .color(egui::Color32::from_gray(140)),
                );
                ui.separator();
                for (i, e) in entries.iter().take(10).enumerate() {
                    let current = cursor == Some(i);
                    let marker = if current { ph::ARROW_RIGHT } else { " " };
                    ui.label(
                        egui::RichText::new(format!("{marker}  {}", e.label()))
                            .size(11.5)
                            .monospace()
                            .color(if current {
                                egui::Color32::from_rgb(120, 190, 255)
                            } else {
                                egui::Color32::from_gray(190)
                            }),
                    );
                }
            });
        });
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
