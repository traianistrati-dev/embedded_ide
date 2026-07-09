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

        // (Re)request only when the line — or the line's text — changed, so a
        // fresh `let x = 5` picks up its type once the initializer is typed.
        // Moving to a new line/file refreshes immediately; a mere text change on
        // the SAME line is throttled (each request carries a full-file
        // `did_change`, and the app deliberately keeps RA sync sparse), with a
        // trailing repaint so the final state still gets requested once typing
        // stops.
        let key = (rel.to_owned(), line, line_hash_at(&chars, target));
        if self.inlay_requested.as_ref() != Some(&key) {
            const THROTTLE: std::time::Duration = std::time::Duration::from_millis(300);
            let same_line = self
                .inlay_requested
                .as_ref()
                .is_some_and(|(r, l, _)| r == rel && *l == line);
            let wait = if same_line {
                self.inlay_last_req_at
                    .map(|t| THROTTLE.saturating_sub(t.elapsed()))
                    .unwrap_or_default()
            } else {
                std::time::Duration::ZERO // new line/file → no throttle
            };

            if wait.is_zero() {
                let sent = {
                    let mut lsp = self.lsp_state.lock().unwrap();
                    if matches!(lsp.status, lsp::LspStatus::Ready) {
                        // Sync the live text so the hint matches what's on screen.
                        lsp.did_change(rel, display_code, false);
                        lsp.request_inlay_hints(rel, line);
                        true
                    } else {
                        false
                    }
                };
                if sent {
                    self.inlay_requested = Some(key);
                    self.inlay_last_req_at = Some(std::time::Instant::now());
                    // Drop a hint that belonged to the previous line while the
                    // new request is in flight, so a stale type never flashes.
                    if self.inlay_hint.as_ref().map(|h| h.line) != Some(line) {
                        self.inlay_hint = None;
                    }
                }
            } else {
                // Throttled — schedule a trailing frame so the final request
                // fires shortly after the user stops typing on this line.
                self.egui_ctx.request_repaint_after(wait);
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

/// Hash of the text of the line containing `idx` — used to detect when the
/// caret's `let` line changed enough to warrant a fresh type request.
fn line_hash_at(chars: &[char], idx: usize) -> u64 {
    use std::hash::{Hash, Hasher};
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
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for &c in &chars[start..end] {
        c.hash(&mut h);
    }
    h.finish()
}
