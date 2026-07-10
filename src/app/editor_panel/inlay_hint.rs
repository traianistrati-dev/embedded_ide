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
//!     receives the async result into `self.inlay_hint`, and applies a pending
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
        let (line, _col) = lsp_cursor_pos(display_code, target);

        // CRITICAL — we send NO `did_change` here. A did_change bumps RA's
        // document version, which (a) cancels other in-flight requests ("content
        // modified" / "stale code action") AND (b) re-triggers analysis, which
        // reintroduced the slow "1-minute save" degradation. The app keeps RA
        // sync SPARSE on purpose (flushed ONLY on Project Save — see
        // [[lsp-verify-debounce]] / [[session-degradation-fixes]]). So the inlay
        // path only QUERIES RA while its document already matches what's on
        // screen (`last_sent_matches`); a plain inlayHint request does not bump
        // the version, so it's cheap and side-effect-free. While the file is
        // dirty we hide the hint (its line/cols would be stale against RA's older
        // text) and re-request once RA catches up (next save / completion /
        // code-action sync). Ctrl+Enter still works on dirty files — it syncs
        // itself.
        let in_sync = self
            .lsp_state
            .lock()
            .unwrap()
            .last_sent_matches(rel, display_code);
        if !in_sync {
            self.inlay_hint = None;
            self.inlay_requested = None; // re-request once RA catches up
            return Some(line);
        }

        // In sync → request once per (file, line).
        let already = self
            .inlay_requested
            .as_ref()
            .is_some_and(|(r, l)| r == rel && *l == line);
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
                self.inlay_requested = Some((rel.to_owned(), line));
                // Drop a hint from the previous line while the new request is in
                // flight, so a stale type never flashes.
                if self.inlay_hint.as_ref().map(|h| h.line) != Some(line) {
                    self.inlay_hint = None;
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
            // Keep the first type hint that sits on the requested line.
            self.inlay_hint = hints.into_iter().find(|h| h.line == line);
        }

        // 2) Apply a pending Tab accept.
        if self.inlay_accept_pending {
            self.inlay_accept_pending = false;
            if let Some(hint) = self.inlay_hint.take() {
                if !hint.text_edits.is_empty() {
                    self.apply_rename_edits(hint.text_edits);
                }
            }
            // Force a fresh request next frame for the (now typed) line.
            self.inlay_requested = None;
        }
    }

    /// Forget the current hint and its in-flight request key (so returning to
    /// the line re-requests, picking up any text change made meanwhile).
    fn clear_inlay_hint(&mut self) {
        self.inlay_hint = None;
        self.inlay_requested = None;
        self.inlay_accept_pending = false;
    }
}
