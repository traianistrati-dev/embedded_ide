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
        // The caret, and the far end of the selection when there is one. Both,
        // because rust-analyzer's most useful assists are offered for a SPAN and
        // not for a point — see `LspState::request_code_actions`.
        cursor_char_idx: Option<usize>,
        sel_end_char_idx: Option<usize>,
        anchor: egui::Pos2,
        slot: crate::app::EditorSlot,
    ) {
        let lsp_file = matches!(
            self.selected_file,
            ProjectFileId::MainRs | ProjectFileId::UserFile(_)
        );
        if !lsp_file || self.ed.code_action_in_flight {
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
        // A selection is asked about as-is: re-targeting it to a `let` pattern
        // would throw the span away, which is the whole point of sending one.
        let sel = sel_end_char_idx.filter(|&e| e != idx);
        let (start, end) = match sel {
            Some(e) => (idx.min(e), idx.max(e)),
            None => {
                let t = super::let_annotation::let_binding_pos(&chars, idx).unwrap_or(idx);
                (t, t)
            }
        };
        let (line, col) = lsp_cursor_pos(display_code, start);
        let (end_line, end_col) = lsp_cursor_pos(display_code, end);
        // Our own row — rust-analyzer never offers this one, it does not know
        // Cargo.toml exists. Computed BEFORE the LSP is consulted, and offered
        // even when it is down: a missing dependency is a fact about Cargo.toml,
        // and needing a running analyzer to be told about it would be absurd.
        self.ed.code_action_add_dep = self.add_dep_candidate(display_code, idx);
        self.ed.code_action_popup_pos = anchor;
        {
            let mut lsp = self.lsp_state.lock().unwrap();
            if !matches!(lsp.status, crate::lsp::LspStatus::Ready) {
                // Nothing to wait for — show what we have, or nothing at all.
                self.ed.code_actions.clear();
                self.ed.code_action_sel = 0;
                self.ed.code_action_popup_open = self.ed.code_action_add_dep.is_some();
                return;
            }
            lsp.did_change(&rel, display_code, false);
            lsp.request_code_actions(&rel, line, col, end_line, end_col);
        }
        // The answer lands at frame top, before any view has drawn — record
        // who asked so it is written into the right one.
        self.lsp_asker.code_action = slot;
        self.ed.code_action_in_flight = true;
        self.ed.code_action_popup_open = false;
    }

    /// Poll code-action responses each frame (called from `init_frame`, so any
    /// resulting edit applies at frame top). Handles the list arrival, a
    /// deferred popup choice, and the resolve result.
    pub(crate) fn poll_code_actions(&mut self) {
        // 1) The action list arrived.
        if self.ed.code_action_in_flight {
            let actions = self.lsp_state.lock().unwrap().take_code_actions_result();
            if let Some(actions) = actions {
                self.ed.code_action_in_flight = false;
                // With our row present, neither shortcut holds: 0 actions is
                // still a list of one, and 1 action must not auto-apply over
                // the choice the user has not made yet.
                let ours = self.ed.code_action_add_dep.is_some();
                match actions.len() {
                    0 if !ours => {}
                    1 if !ours => self.begin_code_action(actions.into_iter().next().unwrap()),
                    _ => {
                        self.ed.code_actions = actions;
                        self.ed.code_action_sel = 0;
                        self.ed.code_action_popup_open = true;
                    }
                }
            }
        }

        // 2) A popup choice deferred from last frame's render.
        if let Some(i) = self.ed.code_action_choice.take() {
            match (i, self.ed.code_action_add_dep.clone()) {
                // Row 0 is ours when it is there — it applies no edit, it opens
                // the crate chooser.
                (0, Some(ident)) => {
                    let pos = self.ed.code_action_popup_pos;
                    self.open_add_dep_chooser(&ident, pos);
                }
                (i, ours) => {
                    let offset = usize::from(ours.is_some());
                    if let Some(a) = self.ed.code_actions.get(i - offset).cloned() {
                        self.begin_code_action(a);
                    }
                }
            }
            self.ed.code_actions.clear();
            self.ed.code_action_add_dep = None;
            self.ed.code_action_popup_open = false;
        }
        self.poll_add_dep();

        // 3) The resolve result arrived → apply.
        if self.ed.code_action_resolve_in_flight {
            let res = self
                .lsp_state
                .lock()
                .unwrap()
                .take_code_action_resolve_result();
            if let Some(edits) = res {
                self.ed.code_action_resolve_in_flight = false;
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
                self.ed.code_action_resolve_in_flight = true;
            }
        }
    }

    /// Draw the code-action chooser popup (shown when > 1 action). A click or
    /// Enter defers the choice to next frame's `poll_code_actions`; Esc closes.
    /// Called after the editor renders (like the completion popup).
    pub(super) fn show_code_action_popup(&mut self, ui: &mut egui::Ui) {
        if !self.ed.code_action_popup_open
            || (self.ed.code_actions.is_empty() && self.ed.code_action_add_dep.is_none())
        {
            return;
        }
        // NOTE: keyboard nav / accept (Up/Down/Enter/Esc) is consumed BEFORE the
        // editor renders (see `editor_panel/mod.rs`), not here — otherwise the
        // editor would process Enter first and insert a newline into the code.
        // This method only renders the list and handles mouse clicks.
        if self.ed.code_action_choice.is_some() {
            return;
        }

        let sel = self.ed.code_action_sel;
        let mut chosen: Option<usize> = None;
        egui::Area::new(egui::Id::new("code_action_popup"))
            .fixed_pos(self.ed.code_action_popup_pos)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.set_min_width(260.0);
                    ui.set_max_width(460.0);
                    let mut offset = 0;
                    if let Some(ident) = &self.ed.code_action_add_dep {
                        offset = 1;
                        let title = format!("Add dependency: {}", super::add_dep::dash_form(ident));
                        let row =
                            ui.selectable_label(sel == 0, egui::RichText::new(title).size(12.0));
                        if sel == 0 {
                            row.scroll_to_me(None);
                        }
                        if row.clicked() {
                            chosen = Some(0);
                        }
                    }
                    for (i, a) in self.ed.code_actions.iter().enumerate() {
                        let i = i + offset;
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
            self.ed.code_action_choice = Some(i);
        }
    }
}
