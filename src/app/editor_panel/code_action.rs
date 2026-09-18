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
//!
//! Two guards, because what rust-analyzer sends is not always what it says:
//! - The chooser resolves the SELECTED row ahead of time and shows what it will
//!   change ("replaces lines 334-631 with 612"). "Inline variable" on a long
//!   closure rewrites hundreds of lines under a two-word title.
//! - An edit that would turn a file that parses into one that does not is
//!   refused. rust-analyzer's "Inline variable" on an `async` closure produced
//!   exactly that — tokens glued together (`letselected_mode`), 128 syntax
//!   errors — and it applied without a word.

use crate::app::editor_state::CodeActionResolve;
use crate::app::{AppIde, ProjectFileId};
use crate::editor::gui::text_pos::{lsp_cursor_pos, selected_file_rel_path};
use crate::lsp::RenameEdit;
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

/// The first syntax error in `text` as `(1-based line, message)`, `None` when it
/// parses as a Rust file.
fn syntax_error(text: &str) -> Option<(usize, String)> {
    // `span-locations` keeps a copy of every parsed source per thread until
    // this is called (see `flow_map::parse::charts_of`); no span outlives us.
    proc_macro2::extra::invalidate_current_thread_spans();
    syn::parse_file(text)
        .err()
        .map(|e| (e.span().start().line.max(1), e.to_string()))
}

/// Would these edits break a file that parses today? `(rel_path, line, error)`
/// of the first such file. A file that already fails to parse is not judged —
/// editing one that is mid-change must stay possible.
fn breaks_syntax(
    edits: &[RenameEdit],
    text_of: impl Fn(&str) -> Option<String>,
) -> Option<(String, usize, String)> {
    let mut files: Vec<&str> = edits.iter().map(|e| e.rel_path.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    for rel in files.into_iter().filter(|r| r.ends_with(".rs")) {
        let Some(before) = text_of(rel) else { continue };
        if syntax_error(&before).is_some() {
            continue;
        }
        let mine: Vec<RenameEdit> = edits
            .iter()
            .filter(|e| e.rel_path == rel)
            .cloned()
            .collect();
        let after = crate::app::apply_text_edits(&before, mine);
        if let Some((line, msg)) = syntax_error(&after) {
            return Some((rel.to_owned(), line, msg));
        }
    }
    None
}

/// What an action's edits change, in one line, and whether that is a LOT —
/// enough that the row deserves a second look before Enter.
fn edit_summary(edits: &[RenameEdit]) -> (String, bool) {
    let mut files: Vec<&str> = edits.iter().map(|e| e.rel_path.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let mut large = false;
    let parts: Vec<String> = files
        .iter()
        .map(|rel| {
            let mine: Vec<&RenameEdit> = edits.iter().filter(|e| e.rel_path == *rel).collect();
            let removed: usize = mine
                .iter()
                .map(|e| (e.end_line - e.start_line) as usize + 1)
                .sum();
            let added: usize = mine.iter().map(|e| e.new_text.lines().count().max(1)).sum();
            large |= removed > 20 || added > 40;
            let name = rel.rsplit('/').next().unwrap_or(rel);
            if let [e] = mine.as_slice() {
                let (a, b) = (e.start_line + 1, e.end_line + 1);
                if a == b {
                    format!("{name}: edits line {a}")
                } else {
                    format!("{name}: replaces lines {a}-{b} ({removed} lines) with {added}")
                }
            } else {
                format!("{name}: {} edits, {removed} lines -> {added}", mine.len())
            }
        })
        .collect();
    (parts.join("; "), large)
}

/// Is this inferred type one that cannot be written in a `let`? A closure's
/// type has no name, and rust-analyzer shows it as `impl Fn…` / `{closure…}`,
/// so "Add explicit type" is never offered for it.
fn unnameable_type(hint_label: &str) -> bool {
    ["impl ", "{closure", "{async closure", "{unknown}"]
        .iter()
        .any(|p| hint_label.contains(p))
}

impl AppIde {
    /// The text of `rel` as it is now (main.rs or a user file).
    fn source_text(&self, rel: &str) -> Option<String> {
        if rel == "src/main.rs" {
            return Some(self.generated_code.clone());
        }
        self.project_tree
            .user_src_files
            .iter()
            .find(|(p, _)| p == rel)
            .map(|(_, c)| c.clone())
    }

    /// Apply a chosen action's edits — unless they would break a file's syntax.
    fn apply_code_action_checked(&mut self, title: &str, edits: Vec<RenameEdit>) {
        if let Some((rel, line, msg)) = breaks_syntax(&edits, |r| self.source_text(r)) {
            self.set_status_msg(format!(
                "{} \"{title}\" not applied: rust-analyzer's edit would break {rel} (line {line}: {msg})",
                egui_phosphor::regular::WARNING
            ));
            return;
        }
        let (summary, _) = edit_summary(&edits);
        self.apply_rename_edits(edits);
        self.set_status_msg(format!("Applied \"{title}\" - {summary}"));
    }

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
        // Say why "Add explicit type" will be missing, when the caret is in a
        // `let` whose type cannot be written. Kept only if the answer indeed
        // lacks it (see `poll_code_actions`).
        let on_let = sel_end_char_idx.is_none_or(|e| e == idx)
            && super::let_annotation::let_binding_pos(&chars, idx).is_some();
        self.ed.code_action_note = on_let.then(|| {
            match self
                .ed
                .inlay_hint
                .as_ref()
                .map(|h| h.label.trim_start_matches(':').trim())
            {
                Some(label) if unnameable_type(label) => {
                    let short: String = label.chars().take(48).collect();
                    let more = if label.chars().count() > 48 {
                        "..."
                    } else {
                        ""
                    };
                    format!(
                        "No \"Add explicit type\": `{short}{more}` cannot be written in a `let` - \
                         a closure's type has no name"
                    )
                }
                _ => "No \"Add explicit type\" offered for this binding".to_owned(),
            }
        });
        // A preview belongs to the list it was asked for; the new list replaces it.
        if matches!(
            self.ed.code_action_resolve_for,
            Some(CodeActionResolve::Preview(_))
        ) {
            self.ed.code_action_resolve_for = None;
        }
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
                // The note is about a MISSING "Add explicit type".
                if actions
                    .iter()
                    .any(|a| a.title.to_ascii_lowercase().contains("explicit type"))
                {
                    self.ed.code_action_note = None;
                }
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
                        let note = self
                            .ed
                            .code_action_note
                            .take()
                            .map(|n| format!(" - {n}"))
                            .unwrap_or_default();
                        self.set_status_msg(format!(
                            "Ctrl+Enter: rust-analyzer has no action at the cursor{reason}{note}"
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
            self.ed.code_action_note = None;
            self.ed.code_action_popup_open = false;
        }
        self.poll_add_dep();

        // 3) The resolve result arrived → store the preview, or apply.
        if self.ed.code_action_resolve_for.is_some() {
            let res = self
                .lsp_state
                .lock()
                .unwrap()
                .take_code_action_resolve_result();
            if let Some(edits) = res {
                match self.ed.code_action_resolve_for.take() {
                    Some(CodeActionResolve::Preview(i)) => {
                        if let Some(a) = self.ed.code_actions.get_mut(i) {
                            a.edits = Some(edits.unwrap_or_default());
                        }
                    }
                    Some(CodeActionResolve::Apply(a)) => match edits {
                        Some(e) if !e.is_empty() => self.apply_code_action_checked(&a.title, e),
                        _ => self.set_status_msg(format!("\"{}\" changes nothing here", a.title)),
                    },
                    None => {}
                }
            }
        }

        // 4) Preview the selected row: resolve its edit ahead of the choice, so
        //    the list can say what it changes. One at a time; the next frame
        //    picks up a selection that moved meanwhile.
        if self.ed.code_action_popup_open && self.ed.code_action_resolve_for.is_none() {
            let offset = usize::from(self.ed.code_action_add_dep.is_some());
            let row = self.ed.code_action_sel.checked_sub(offset);
            if let Some(i) = row
                && let Some(a) = self.ed.code_actions.get(i)
                && a.edits.is_none()
            {
                let raw = a.raw.clone();
                if self
                    .lsp_state
                    .lock()
                    .unwrap()
                    .request_code_action_resolve(raw)
                {
                    self.ed.code_action_resolve_for = Some(CodeActionResolve::Preview(i));
                } else if let Some(a) = self.ed.code_actions.get_mut(i) {
                    // No analyzer to ask: say so instead of "checking…" forever.
                    a.edits = Some(Vec::new());
                }
            }
        }
    }

    /// Apply an action's inline edit, or `codeAction/resolve` it when the edit
    /// was deferred by RA.
    fn begin_code_action(&mut self, action: crate::lsp::CodeAction) {
        match action.edits.clone() {
            Some(edits) if !edits.is_empty() => {
                self.apply_code_action_checked(&action.title, edits)
            }
            Some(_) => self.set_status_msg(format!("\"{}\" changes nothing here", action.title)),
            None => {
                // A preview of another row may still be in flight; this request
                // supersedes it (only the newest resolve id is answered).
                let sent = self
                    .lsp_state
                    .lock()
                    .unwrap()
                    .request_code_action_resolve(action.raw.clone());
                if sent {
                    self.ed.code_action_resolve_for = Some(CodeActionResolve::Apply(action));
                } else {
                    self.ed.code_action_resolve_for = None;
                    self.set_status_msg("Ctrl+Enter: rust-analyzer is not running".to_owned());
                }
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
                    // What the selected row will change — before Enter, not after.
                    let preview = sel
                        .checked_sub(offset)
                        .and_then(|i| self.ed.code_actions.get(i))
                        .map(|a| match &a.edits {
                            Some(e) if !e.is_empty() => {
                                let (text, large) = edit_summary(e);
                                let color = if large {
                                    egui::Color32::from_rgb(230, 180, 80)
                                } else {
                                    egui::Color32::from_gray(150)
                                };
                                (text, color)
                            }
                            Some(_) => ("no change".to_owned(), egui::Color32::from_gray(130)),
                            None => (
                                "checking what it changes...".to_owned(),
                                egui::Color32::from_gray(120),
                            ),
                        });
                    if preview.is_some() || self.ed.code_action_note.is_some() {
                        ui.separator();
                    }
                    if let Some((text, color)) = preview {
                        ui.label(egui::RichText::new(text).size(10.5).color(color));
                    }
                    if let Some(note) = &self.ed.code_action_note {
                        ui.label(
                            egui::RichText::new(note)
                                .size(10.5)
                                .italics()
                                .color(egui::Color32::from_gray(135)),
                        );
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
    use super::{breaks_syntax, code_action_positions, edit_summary, unnameable_type};
    use crate::lsp::RenameEdit;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn edit(rel: &str, (sl, sc): (u32, u32), (el, ec): (u32, u32), text: &str) -> RenameEdit {
        RenameEdit {
            rel_path: rel.to_owned(),
            start_line: sl,
            start_char: sc,
            end_line: el,
            end_char: ec,
            new_text: text.to_owned(),
        }
    }

    /// The report: rust-analyzer's "Inline variable" on an async closure glued
    /// tokens together (`letselected_mode`). An edit like that is refused.
    #[test]
    fn an_edit_that_breaks_a_parsing_file_is_refused() {
        let src = "fn main() {
    let f = async |x: u8| { let y = x; y };
    let _ = f;
}
";
        let glued = edit("src/main.rs", (1, 4), (2, 14), "let = selected_mode;");
        let got = breaks_syntax(&[glued], |_| Some(src.to_owned()));
        let (rel, line, _msg) = got.expect("refused");
        assert_eq!(rel, "src/main.rs");
        assert_eq!(line, 2);
    }

    #[test]
    fn an_edit_that_keeps_the_syntax_passes() {
        let src = "fn main() {
    let f = 1;
}
";
        let typed = edit("src/main.rs", (1, 9), (1, 9), ": i32");
        assert!(breaks_syntax(&[typed], |_| Some(src.to_owned())).is_none());
    }

    /// A file that already fails to parse is being edited; judging it would
    /// block every action until the user finished typing.
    #[test]
    fn a_file_that_already_fails_is_not_judged() {
        let src = "fn main() {
    let f = 
}
";
        let e = edit("src/main.rs", (1, 4), (1, 4), "}}}");
        assert!(breaks_syntax(&[e], |_| Some(src.to_owned())).is_none());
        let toml = edit("Cargo.toml", (0, 0), (0, 0), "[[[");
        assert!(
            breaks_syntax(&[toml], |_| Some(
                "x = 1
"
                .to_owned()
            ))
            .is_none()
        );
    }

    #[test]
    fn the_summary_names_the_lines_and_flags_a_large_rewrite() {
        let big = edit(
            "src/main.rs",
            (333, 4),
            (630, 1),
            &"x
"
            .repeat(612),
        );
        let (text, large) = edit_summary(&[big]);
        assert_eq!(text, "main.rs: replaces lines 334-631 (298 lines) with 612");
        assert!(large);
        let small = edit("src/main.rs", (9, 9), (9, 9), ": i32");
        assert_eq!(
            edit_summary(&[small]),
            ("main.rs: edits line 10".to_owned(), false)
        );
    }

    #[test]
    fn closure_types_are_recognised_as_unwritable() {
        assert!(unnameable_type(
            "impl AsyncFn(&mut Ssd1306Async<…>, &[SettingMode])"
        ));
        assert!(unnameable_type("{closure@src/main.rs:12:13}"));
        assert!(!unnameable_type(
            "Ssd1306Async<I2CInterface<I2c<'_, Async>>>"
        ));
        assert!(!unnameable_type("u32"));
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
