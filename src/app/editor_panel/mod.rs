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

mod comment;
mod completion;
mod diag_embed;
mod toolbar;

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
                self.show_editor_diag_panel(ui);

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
                // Detect Ctrl+Space BEFORE the editor so egui doesn't pass it
                // to the TextEdit as a literal character.
                let ctrl_space_pressed =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Space));
                // Ctrl+/ → toggle line comments on the selection (consumed before
                // the editor so `/` is never typed into the text).
                let ctrl_slash_pressed =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Slash));

                // Size the editor to fill the height left over after the
                // (resizable) diagnostics panel, so dragging that panel's handle
                // grows/shrinks the code area in lock-step.  `available_height`
                // here is already the space remaining below the toolbar and
                // above the bottom diagnostics panel.
                let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
                let editor_rows =
                    (((ui.available_height() - 10.0) / row_h).floor() as usize).max(3);

                let mut editor_resp = CodeEditor::default()
                    .id_source(editor_id)
                    .with_rows(editor_rows)
                    .with_fontsize(13.0)
                    .with_theme(ColorTheme::GRUVBOX)
                    .with_numlines(true)
                    .show_with_completer(
                        ui,
                        &mut display_code,
                        &display_syntax,
                        &mut self.completer,
                    );

                // ── Ctrl+/ : toggle line comments on the selected lines ───────
                // Uses the file's comment marker (`//` for .rs, `#` for TOML /
                // .gitignore). Re-selects the affected block so repeated presses
                // toggle the same lines. Applied before the write-back below so
                // the new text is persisted; the cursor is stored for next frame.
                if ctrl_slash_pressed {
                    if let Some(range) = editor_resp.state.cursor.char_range() {
                        let lo = range.primary.index.min(range.secondary.index);
                        let hi = range.primary.index.max(range.secondary.index);
                        let (new_code, new_lo, new_hi) =
                            comment::toggle_line_comments(&display_code, lo, hi, display_syntax.comment());
                        display_code = new_code;
                        // Store a clone with the new selection (store() consumes
                        // the state, so don't move `editor_resp.state` itself —
                        // it's still read by handle_editor_completion below).
                        let mut st = editor_resp.state.clone();
                        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(new_lo),
                            egui::text::CCursor::new(new_hi),
                        )));
                        st.store(ui.ctx(), editor_resp.response.id);
                    }
                }

                // ── Write user edits back ────────────────────────────────────
                // display_code is a local clone; persist changes here.
                if let ProjectFileId::UserFile(i) = self.selected_file {
                    if let Some(entry) = self.project_tree.user_src_files.get_mut(i) {
                        if display_code != entry.1 {
                            // In-memory only; the debounced LSP flush (3 s idle or
                            // Project Save, see app::init_frame) writes it to the
                            // workspace and notifies RA — not on every keystroke.
                            entry.1 = display_code.clone();
                        }
                    }
                } else if self.selected_file == ProjectFileId::MainRs
                    && display_code != self.generated_code
                {
                    self.generated_code = display_code.clone();
                } else {
                    // Editable project config files — persist edits to the
                    // matching field (the per-frame snapshot reads them back).
                    let slot = match self.selected_file {
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

                self.handle_editor_completion(
                    ui,
                    &editor_resp,
                    display_code,
                    lsp_accepted,
                    ctrl_space_pressed,
                );
            });
    }
}
