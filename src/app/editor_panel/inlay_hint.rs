//! Cursor-line inferred-type hint.
//!
//! When the caret sits on an untyped `let` binding, rust-analyzer's inlay hint
//! for that ONE line is shown as dim ghost text after the name (e.g. `: u32`),
//! and pressing **Tab** splices the type into the source. Only the caret's line
//! is ever requested, so traffic to RA stays tiny.
//!
//! Flow (three touch points, mirroring the code-action pipeline):
//!   * [`AppIde::update_inlay_hint`] — called from `completion.rs` after the
//!     editor renders: decides whether the caret is on an untyped `let`;
//!     (re)requests the hint when the line or its text changed; clears the hint
//!     otherwise. Returns the caret's untyped-`let` line so the overlay only
//!     draws a hint that still matches the current line.
//!   * [`AppIde::poll_inlay_hint`] — called from `init_frame` at frame TOP:
//!     receives the async result into `self.ed.inlay_hint`, and applies a pending
//!     Tab accept. The edit runs at frame top so the editor's end-of-frame
//!     write-back can't revert it (the same rule code actions follow).

use super::AppIde;
use crate::editor::gui::text_pos::lsp_cursor_pos;
use crate::lsp;

impl AppIde {
    /// (Re)issue or clear the cursor-line inlay request. `cursor_char_idx` is the
    /// caret char index in `display_code`; `rel` the file's workspace-relative
    /// path. Returns `Some(line_0based)` when the caret is on an untyped `let`
    /// (so the overlay knows which line a stored hint is allowed to draw on).
    pub(super) fn update_inlay_hint(
        &mut self,
        display_code: &str,
        cursor_char_idx: Option<usize>,
        rel: Option<&str>,
        slot: crate::app::EditorSlot,
    ) -> Option<u32> {
        // Feature off, no caret, or not an LSP-tracked file → no hint.
        if !self.inlay_types_enabled {
            self.clear_inlay_hint();
            return None;
        }
        let (Some(idx), Some(rel)) = (cursor_char_idx, rel) else {
            self.clear_inlay_hint();
            return None;
        };
        let chars: Vec<char> = display_code.chars().collect();
        // Only untyped `let` bindings get a hint. `let_binding_pos` returns the
        // name position from anywhere on the (possibly multi-line) statement and
        // yields `None` once an explicit type is present — so the hint clears
        // itself the instant a type is inserted.
        let Some(target) = super::let_annotation::let_binding_pos(&chars, idx) else {
            self.clear_inlay_hint();
            return None;
        };
        let (line, col) = lsp_cursor_pos(display_code, target);

        // CRITICAL — we send NO `did_change` here. A did_change bumps RA's
        // document version, and every request issued against the older version
        // is then answered "content modified" / "stale code action". Nothing
        // wedges (each reply path clears its own id), but a lost `references`
        // reply is recorded as "0 references", which fades live code as dead.
        //
        // This comment used to add a second reason — that a did_change
        // "re-triggers analysis, which reintroduced the slow 1-minute save
        // degradation". That was wrong twice over: the save slowness was a
        // leaked thread plus a deleted Cargo.lock (see `initialization_options`),
        // and the "1-minute save" was a dropped repaint wake-up. Neither
        // involved did_change, and no cargo runs without a `did_save`. The
        // cancellation reason above is the real one, and it is enough.
        //
        // Sync is no longer Save-only: `editor_panel::idle_sync` re-syncs the
        // visible file once typing pauses, which is what lets this path find a
        // matching document at all while you type. It waits on
        // `any_request_in_flight`, so it cannot cancel the request below.
        // The inlay path itself still only QUERIES RA while its document already
        // matches what's on screen (`last_sent_matches`); a plain inlayHint
        // request does not bump
        // the version, so it's cheap and side-effect-free. While the file is
        // dirty we hide the hint (its line/cols would be stale against RA's older
        // text) and re-request once RA catches up (next save / completion /
        // code-action sync). Ctrl+Enter still works on dirty files — it syncs
        // itself.
        // One lock for all three: whether rust-analyzer holds this text, and the
        // two facts that decide whether an earlier empty answer is worth
        // re-asking (see `EditorState::inlay_asked_at`).
        let (in_sync, stamp) = {
            let lsp = self.lsp_state.lock().unwrap();
            (
                lsp.last_sent_matches(rel, display_code),
                (lsp.generation, lsp.indexed),
            )
        };
        if !in_sync {
            self.ed.inlay_hint = None;
            self.ed.inlay_requested = None; // re-request once RA catches up
            return Some(line);
        }

        // In sync → request once per (file, line).
        let already = self
            .ed
            .inlay_requested
            .as_ref()
            .is_some_and(|(r, l, _)| r == rel && *l == line)
            && self.ed.inlay_asked_at == stamp;
        if !already {
            let sent = {
                let mut lsp = self.lsp_state.lock().unwrap();
                if matches!(lsp.status, lsp::LspStatus::Ready) {
                    lsp.request_inlay_hints(rel, line);
                    true
                } else {
                    false
                }
            };
            if sent {
                self.ed.inlay_requested = Some((rel.to_owned(), line, col));
                self.ed.inlay_asked_at = stamp;
                // The answer is applied at frame top, before any view has drawn.
                self.lsp_asker.inlay = slot;
                // Drop a hint from the previous line while the new request is in
                // flight, so a stale type never flashes.
                if self.ed.inlay_hint.as_ref().map(|h| h.line) != Some(line) {
                    self.ed.inlay_hint = None;
                }
            }
        }
        Some(line)
    }

    /// Receive the async inlay result and apply a pending Tab accept. Runs at
    /// frame TOP (from `init_frame`) so an accepted edit survives the editor's
    /// end-of-frame write-back.
    pub(crate) fn poll_inlay_hint(&mut self) {
        // 1) Receive the latest request's result. Only the newest request's
        //    response sets the flag (stale ids fall through in `handle_incoming`).
        let result = self.lsp_state.lock().unwrap().take_inlay_result();
        if let Some((_rel, line, hints)) = result {
            // The hint for the binding we ASKED about — the one whose column is
            // nearest at-or-after the name, not merely the first on the line.
            //
            // rust-analyzer puts a type hint immediately after the name it
            // belongs to, so on `let a = 1; let b = 2;` the line alone does not
            // identify which binding a hint describes. Chaining and closure
            // hints are switched off in `initialization_options`, but they carry
            // the same LSP kind as type hints, so nothing in the protocol keeps
            // a stray one out of this slot either.
            let want_col = self.ed.inlay_requested.as_ref().map(|(_, _, c)| *c);
            self.ed.inlay_hint = pick_hint(hints, line, want_col);
        }

        // 2) Apply a pending Tab accept.
        if self.ed.inlay_accept_pending {
            self.ed.inlay_accept_pending = false;
            if let Some(hint) = self.ed.inlay_hint.take() {
                if !hint.text_edits.is_empty() {
                    self.apply_rename_edits(hint.text_edits);
                }
            }
            // Force a fresh request next frame for the (now typed) line.
            self.ed.inlay_requested = None;
        }
    }

    /// Forget the current hint and its in-flight request key (so returning to
    /// the line re-requests, picking up any text change made meanwhile).
    fn clear_inlay_hint(&mut self) {
        self.ed.inlay_hint = None;
        self.ed.inlay_requested = None;
        self.ed.inlay_accept_pending = false;
    }
}

/// The hint on `line` that belongs to the binding at `want_col`.
///
/// Nearest at-or-after the name wins; if none sits at or after it (a hint
/// rust-analyzer placed differently than expected), the leftmost hint on the
/// line is used rather than none — a hint in the wrong slot is still better than
/// the silence this whole path is fixing.
fn pick_hint(
    hints: Vec<crate::lsp::InlayHint>,
    line: u32,
    want_col: Option<u32>,
) -> Option<crate::lsp::InlayHint> {
    let on_line: Vec<crate::lsp::InlayHint> =
        hints.into_iter().filter(|h| h.line == line).collect();
    let Some(col) = want_col else {
        return on_line.into_iter().min_by_key(|h| h.character);
    };
    on_line
        .iter()
        .filter(|h| h.character >= col)
        .min_by_key(|h| h.character - col)
        .cloned()
        .or_else(|| on_line.into_iter().min_by_key(|h| h.character))
}

#[cfg(test)]
mod tests {
    use super::pick_hint;
    use crate::lsp::InlayHint;

    fn hint(line: u32, character: u32, label: &str) -> InlayHint {
        InlayHint {
            line,
            character,
            label: label.to_owned(),
            text_edits: Vec::new(),
        }
    }

    /// Two bindings on one line: the hint that belongs to the one we asked
    /// about wins. Taking the first on the line handed back the wrong type with
    /// nothing to show it was wrong.
    #[test]
    fn the_hint_belongs_to_the_binding_we_asked_about() {
        // `let a = 1; let b = 2;` — names at columns 4 and 15.
        let hints = vec![hint(7, 5, ": i32"), hint(7, 16, ": u8")];
        assert_eq!(pick_hint(hints.clone(), 7, Some(15)).unwrap().label, ": u8");
        assert_eq!(pick_hint(hints, 7, Some(4)).unwrap().label, ": i32");
    }

    /// Hints from other lines in the requested range are not ours.
    #[test]
    fn a_hint_from_another_line_is_ignored() {
        let hints = vec![hint(8, 5, ": wrong line")];
        assert!(pick_hint(hints, 7, Some(4)).is_none());
    }

    /// Nothing at or after the name: take what there is rather than nothing.
    /// Silence is the failure mode this whole path exists to remove.
    #[test]
    fn a_hint_before_the_name_is_better_than_no_hint() {
        let hints = vec![hint(7, 2, ": something")];
        assert_eq!(pick_hint(hints, 7, Some(40)).unwrap().label, ": something");
    }

    #[test]
    fn with_no_column_known_the_leftmost_hint_wins() {
        let hints = vec![hint(7, 30, ": late"), hint(7, 5, ": early")];
        assert_eq!(pick_hint(hints, 7, None).unwrap().label, ": early");
    }

    #[test]
    fn an_empty_answer_stays_empty() {
        assert!(pick_hint(Vec::new(), 7, Some(4)).is_none());
    }
}
