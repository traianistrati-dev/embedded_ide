//! Ctrl+Enter code actions — rust-analyzer assists / quick-fixes at the cursor
//! (e.g. "Replace qualified path with use" → adds a `use` and shortens the
//! path). Mirrors the rename pipeline: request over LSP, poll in `init_frame`,
//! apply the returned `WorkspaceEdit` via [`AppIde::apply_rename_edits`].
//!
//! Flow: Ctrl+Enter → `request_code_actions`. When the list lands (polled in
//! `init_frame`): 0 → nothing; 1 → apply directly; >1 → a chooser popup. A
//! chosen action with an inline edit applies immediately; a lazy one is
//! `codeAction/resolve`d first. All applies run at frame TOP so the editor's
//! end-of-frame write-back can't revert them (the Clippy-fix gotcha).

use crate::app::{AppIde, ProjectFileId};
use crate::editor::gui::text_pos::{lsp_cursor_pos, selected_file_rel_path};
use eframe::egui;

impl AppIde {
    /// Fire a codeAction request for the cursor position (Ctrl+Enter). Syncs
    /// the live text to RA first so the position matches. `cursor_char_idx` is
    /// the caret char index; `anchor` the caret's screen rect for the popup.
    pub(super) fn trigger_code_actions(
        &mut self,
        display_code: &str,
        cursor_char_idx: Option<usize>,
        anchor: egui::Pos2,
    ) {
        let lsp_file = matches!(
            self.selected_file,
            ProjectFileId::MainRs | ProjectFileId::UserFile(_)
        );
        if !lsp_file || self.code_action_in_flight {
            return;
        }
        let Some(rel) =
            selected_file_rel_path(&self.selected_file, &self.project_tree.user_src_files)
        else {
            return;
        };
        let Some(idx) = cursor_char_idx else { return };
        // When the line is a `let x = …` without a type, re-target the request
        // to the binding name — rust-analyzer only offers "Add explicit type"
        // on the `let` pattern, not on the initializer where the cursor usually
        // sits. This makes Ctrl+Enter add the type from anywhere on the line.
        let chars: Vec<char> = display_code.chars().collect();
        let target = super::let_annotation::let_binding_pos(&chars, idx).unwrap_or(idx);
        let (line, col) = lsp_cursor_pos(display_code, target);
        {
            let mut lsp = self.lsp_state.lock().unwrap();
            if !matches!(lsp.status, crate::lsp::LspStatus::Ready) {
                return;
            }
            lsp.did_change(&rel, display_code, false);
            lsp.request_code_actions(&rel, line, col);
        }
        self.code_action_in_flight = true;
        self.code_action_popup_open = false;
        self.code_action_popup_pos = anchor;
    }

    /// Poll code-action responses each frame (called from `init_frame`, so any
    /// resulting edit applies at frame top). Handles the list arrival, a
    /// deferred popup choice, and the resolve result.
    pub(crate) fn poll_code_actions(&mut self) {
        // 1) The action list arrived.
        if self.code_action_in_flight {
            let actions = self.lsp_state.lock().unwrap().take_code_actions_result();
            if let Some(actions) = actions {
                self.code_action_in_flight = false;
                match actions.len() {
                    0 => {}
                    1 => self.begin_code_action(actions.into_iter().next().unwrap()),
                    _ => {
                        self.code_actions = actions;
                        self.code_action_sel = 0;
                        self.code_action_popup_open = true;
                    }
                }
            }
        }

        // 2) A popup choice deferred from last frame's render.
        if let Some(i) = self.code_action_choice.take() {
            if let Some(a) = self.code_actions.get(i).cloned() {
                self.begin_code_action(a);
            }
            self.code_actions.clear();
            self.code_action_popup_open = false;
        }

        // 3) The resolve result arrived → apply.
        if self.code_action_resolve_in_flight {
            let res = self
                .lsp_state
                .lock()
                .unwrap()
                .take_code_action_resolve_result();
            if let Some(edits) = res {
                self.code_action_resolve_in_flight = false;
                if let Some(edits) = edits {
                    if !edits.is_empty() {
                        self.apply_rename_edits(edits);
                    }
                }
            }
        }
    }

    /// Apply an action's inline edit, or `codeAction/resolve` it when the edit
    /// was deferred by RA.
    fn begin_code_action(&mut self, action: crate::lsp::CodeAction) {
        match action.edits {
            Some(edits) if !edits.is_empty() => self.apply_rename_edits(edits),
            _ => {
                self.lsp_state
                    .lock()
                    .unwrap()
                    .request_code_action_resolve(action.raw);
                self.code_action_resolve_in_flight = true;
            }
        }
    }

    /// Draw the code-action chooser popup (shown when > 1 action). A click or
    /// Enter defers the choice to next frame's `poll_code_actions`; Esc closes.
    /// Called after the editor renders (like the completion popup).
    pub(super) fn show_code_action_popup(&mut self, ui: &mut egui::Ui) {
        if !self.code_action_popup_open || self.code_actions.is_empty() {
            return;
        }
        let count = self.code_actions.len();
        // Keyboard navigation (consumed here so the editor doesn't see them).
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
        if !self.code_action_popup_open || self.code_action_choice.is_some() {
            return;
        }

        let sel = self.code_action_sel;
        let mut chosen: Option<usize> = None;
        egui::Area::new(egui::Id::new("code_action_popup"))
            .fixed_pos(self.code_action_popup_pos)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.set_min_width(260.0);
                    ui.set_max_width(460.0);
                    for (i, a) in self.code_actions.iter().enumerate() {
                        let selected = i == sel;
                        let row =
                            ui.selectable_label(selected, egui::RichText::new(&a.title).size(12.0));
                        if selected {
                            row.scroll_to_me(None);
                        }
                        if row.clicked() {
                            chosen = Some(i);
                        }
                    }
                });
            });
        if let Some(i) = chosen {
            self.code_action_choice = Some(i);
        }
    }
}
