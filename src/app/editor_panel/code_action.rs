//! Ctrl+Enter code actions — rust-analyzer assists / quick-fixes at the cursor
//! (e.g. "Replace qualified path with use" → adds a `use` and shortens the
//! path). Mirrors the rename pipeline: request over LSP, poll in `init_frame`,
//! apply the returned `WorkspaceEdit` via [`AppIde::apply_rename_edits`].
//!
//! Flow: Ctrl+Enter → `request_code_actions`. When the list lands (polled in
//! `init_frame`): 0 → a status message; 1 or more → the chooser popup. Nothing
//! is ever applied without a choice: an action is an edit the user has not
//! seen, and the one rust-analyzer offers alone can be "Inline variable" —
//! which, auto-applied on an `async` closure's `let`, deleted the closure. A
//! chosen action with an inline edit applies immediately; a lazy one is
//! `codeAction/resolve`d first. All applies run at frame TOP so the editor's
//! end-of-frame write-back can't revert them (the Clippy-fix gotcha).

use crate::app::{AppIde, ProjectFileId};
use crate::editor::gui::text_pos::{lsp_cursor_pos, selected_file_rel_path};
use eframe::egui;

/// The spans to ask rust-analyzer about for one Ctrl+Enter, as char ranges in
/// the order their actions are listed.
///
/// A selection is asked about as-is — it is the one thing the user pointed at,
/// and the span-only assists (Extract into function, …) need it whole.
///
/// Without one: the caret FIRST, then the binding of the `let` statement the
/// caret sits in, when that is somewhere else. rust-analyzer offers "Add
/// explicit type" only on the `let` pattern, so the caret alone missed it from
/// the initializer. But asking only at the pattern — the old re-target — lost
/// every assist of the code under the caret: on `let f = async |…| { … }` the
/// closure's own assists were gone, and what remained was the pattern's
/// "Inline variable".
fn code_action_positions(
    chars: &[char],
    caret: usize,
    sel_end: Option<usize>,
) -> Vec<(usize, usize)> {
    if let Some(e) = sel_end.filter(|&e| e != caret) {
        return vec![(caret.min(e), caret.max(e))];
    }
    let mut out = vec![(caret, caret)];
    if let Some(t) = super::let_annotation::let_binding_pos(chars, caret)
        && t != caret
    {
        out.push((t, t));
    }
    out
}

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
        if !lsp_file {
            return;
        }
        // A request older than this was never answered — rust-analyzer restarted
        // under it, or dropped it. Without the deadline the flag stays true and
        // every later Ctrl+Enter returns here instead of asking.
        const CODE_ACTION_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
        if self.ed.code_action_in_flight {
            let stale = self
                .ed
                .code_action_sent_at
                .is_none_or(|t| t.elapsed() > CODE_ACTION_WAIT);
            if !stale {
                return;
            }
            self.ed.code_action_in_flight = false;
        }
        let Some(rel) =
            selected_file_rel_path(&self.selected_file, &self.project_tree.user_src_files)
        else {
            return;
        };
        let Some(idx) = cursor_char_idx else { return };
        let chars: Vec<char> = display_code.chars().collect();
        let ranges: Vec<(u32, u32, u32, u32)> =
            code_action_positions(&chars, idx, sel_end_char_idx)
                .into_iter()
                .map(|(start, end)| {
                    let (line, col) = lsp_cursor_pos(display_code, start);
                    let (end_line, end_col) = lsp_cursor_pos(display_code, end);
                    (line, col, end_line, end_col)
                })
                .collect();
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
            lsp.request_code_actions(&rel, &ranges);
        }
        // The answer lands at frame top, before any view has drawn — record
        // who asked so it is written into the right one.
        self.lsp_asker.code_action = slot;
        self.ed.code_action_in_flight = true;
        self.ed.code_action_sent_at = Some(std::time::Instant::now());
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
                // With our row present, 0 actions is still a list of one.
                let ours = self.ed.code_action_add_dep.is_some();
                match actions.len() {
                    // Say so. An empty answer used to be indistinguishable from
                    // a broken shortcut — which is how "Ctrl+Enter does not
                    // react" arrived as a bug report with nothing to go on. It
                    // is also the most COMMON outcome: rust-analyzer offers
                    // assists at a few positions, not at every caret.
                    0 if !ours => {
                        // ...and say WHY when we can tell. "No action" alone is
                        // true and useless: it reads the same whether the caret
                        // is simply somewhere rust-analyzer offers nothing, or
                        // the analyzer has no type for the expression under it.
                        let asked = self.lsp_state.lock().unwrap().code_action_for.clone();
                        let reason = asked
                            .and_then(|(rel, line)| self.caret_silence_reason(&rel, line))
                            .map(|r| format!(" ({r})"))
                            .unwrap_or_default();
                        self.set_status_msg(format!(
                            "Ctrl+Enter: rust-analyzer has no action at the cursor{reason}"
                        ));
                    }
                    // One action is still a CHOICE — never applied unseen.
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

    /// Draw the code-action chooser popup (shown for 1 action or more). A click or
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

#[cfg(test)]
mod tests {
    use super::code_action_positions;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    /// The report: the caret inside an `async` closure's parameters on a `let`
    /// line. Both places are asked — the caret first, for the closure's own
    /// assists, then the binding, for "Add explicit type".
    #[test]
    fn a_caret_in_a_let_initializer_asks_there_and_at_the_binding() {
        let src = "fn f() {\n    let print_modes = async |x: u8| { x };\n}\n";
        let caret = src.find("|x").unwrap() + 1;
        let binding = src.find("print_modes").unwrap();
        let got = code_action_positions(&chars(src), caret, None);
        assert_eq!(got, vec![(caret, caret), (binding, binding)]);
    }

    #[test]
    fn a_caret_already_on_the_binding_is_asked_once() {
        let src = "fn f() {\n    let total = 1 + 2;\n}\n";
        let binding = src.find("total").unwrap();
        assert_eq!(
            code_action_positions(&chars(src), binding, None),
            vec![(binding, binding)]
        );
    }

    #[test]
    fn a_caret_outside_any_let_is_asked_alone() {
        let src = "fn f() {\n    do_it(1);\n}\n";
        let caret = src.find("do_it").unwrap() + 2;
        assert_eq!(
            code_action_positions(&chars(src), caret, None),
            vec![(caret, caret)]
        );
    }

    /// A selection is the thing pointed at: asked as one span, never moved.
    #[test]
    fn a_selection_is_asked_as_is() {
        let src = "fn f() {\n    let a = 1 + 2;\n}\n";
        let s = src.find("1 +").unwrap();
        let e = s + 5;
        assert_eq!(code_action_positions(&chars(src), e, Some(s)), vec![(s, e)]);
        assert_eq!(
            code_action_positions(&chars(src), s, Some(s)).len(),
            2,
            "an empty selection is a caret"
        );
    }
}
