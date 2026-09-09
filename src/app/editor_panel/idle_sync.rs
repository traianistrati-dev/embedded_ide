//! Hand rust-analyzer the visible file once typing has paused.
//!
//! Without this, `last_sent_matches` is false from a user's first keystroke
//! until they press Ctrl+S — `lsp_flush_requested` is set in exactly one place
//! in the whole tree, under a Save. Everything gated on that predicate is
//! therefore dark for the entire editing session: the inline diagnostic
//! overlay, and the inferred-type ghost hint. Three comments in this crate
//! promised the diagnostics would "reappear once the LSP debounce re-verifies
//! (3 s idle / Project Save)". The 3-second half was deleted long ago; the
//! comments outlived the mechanism and are corrected now.
//!
//! **This is not the path that was deleted.** That one (2026-06-23) called the
//! whole save: it rewrote main.rs and every user file to the temp workspace with
//! no compare-read, then sent a `didSave` PER FILE — N cargo flychecks every
//! three seconds, against a shared target dir the Build / Clippy / Flash
//! invocations were also waiting on. This sends ONE `didChange` for ONE file:
//! no disk write, no `didSave`, and therefore no cargo. It is strictly less
//! traffic than F12 and Ctrl+Enter already produce on every single press, and
//! far less than the settle re-verify, which force-resends EVERY open document.
//!
//! What it does NOT restore: flycheck-sourced diagnostics. A real edit bumps
//! `edit_gen`, so `flycheck_stale()` stays true until a Save runs cargo again,
//! and rustc-sourced rows keep being filtered out — correctly, since their
//! line/cols really are from older text. What comes back is rust-analyzer's own
//! analysis, which is where type errors, name resolution and the inferred type
//! live. That is strictly more than the nothing shown today.

use eframe::egui;

/// How long the text must hold still before the re-sync fires.
///
/// Comfortably above typing cadence, and deliberately well under the usages
/// pass's own debounce so an edit burst is synced BEFORE that pass asks
/// rust-analyzer about the same file — see the test at the bottom, which pins
/// the ordering rather than trusting this sentence.
pub(super) const IDLE: std::time::Duration = std::time::Duration::from_millis(600);

/// How soon to look again when the moment was not right (rust-analyzer owes an
/// answer, or a Save is already syncing). Deferring, never dropping: the usages
/// reference chain is serialized and can run for many seconds on a large file,
/// which is exactly the file where a blank overlay hurts most.
const RETRY: std::time::Duration = std::time::Duration::from_millis(200);

/// How soon to look again after sending, so rust-analyzer's publish is picked up
/// even if its cross-thread repaint request is dropped — the lost-wake-up this
/// app has already been bitten by once.
const AWAIT_PUBLISH: std::time::Duration = std::time::Duration::from_millis(250);

/// Why a given frame did not send. Pure, so the decision can be tested without
/// a running analyzer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Decision {
    /// The text changed this frame: restart the clock.
    Restart,
    /// Nothing to do — rust-analyzer already holds this text.
    InSync,
    /// Still settling, or the moment is wrong; look again shortly.
    Wait,
    /// Send `didChange` for this file now.
    Send,
}

/// The whole rule, with every input passed in.
///
/// `changed` is "this view's text differs from the frame before"; `waited` is
/// how long it has held still.
#[allow(clippy::too_many_arguments)]
pub(super) fn decide(
    changed: bool,
    waited: std::time::Duration,
    in_sync: bool,
    ready: bool,
    usable: bool,
    busy: bool,
    saving: bool,
    interacting: bool,
) -> Decision {
    if changed {
        return Decision::Restart;
    }
    if in_sync {
        return Decision::InSync;
    }
    // Not ready, or a file rust-analyzer has never opened: `did_change`
    // auto-opens, and auto-opening before the index is built is what produces
    // phantom type errors on a detached file. The same gate F12 uses.
    if !ready || !usable {
        return Decision::Wait;
    }
    // A Save is about to do this properly; a second version bump under it is
    // noise. And never cut across the user's own popup.
    if saving || interacting || busy {
        return Decision::Wait;
    }
    if waited < IDLE {
        return Decision::Wait;
    }
    Decision::Send
}

impl crate::app::AppIde {
    /// Run one frame of the idle re-sync for the view being drawn.
    ///
    /// Called from the shared editor body BEFORE the usages pass and before
    /// `handle_editor_completion`, so a version bump can never cancel the inlay
    /// request that this very sync is what enables.
    pub(super) fn tick_idle_sync(&mut self, ctx: &egui::Context, rel: &str, display_code: &str) {
        let hash = crate::app::AppIde::content_hash(display_code);
        let changed = self
            .ed
            .idle_sync
            .as_ref()
            .is_none_or(|(r, h, _)| r != rel || *h != hash);

        // One lock for every LSP fact the decision needs, and — critically — the
        // same lock the send happens under. Two acquisitions would leave a hole
        // where the usages pump could issue a request between the check and the
        // bump, which is the one cancellation this guard exists to prevent.
        let mut lsp = self.lsp_state.lock().unwrap();
        let d = decide(
            changed,
            self.ed
                .idle_sync
                .as_ref()
                .map(|(_, _, at)| at.elapsed())
                .unwrap_or_default(),
            lsp.last_sent_matches(rel, display_code),
            matches!(lsp.status, crate::lsp::LspStatus::Ready),
            lsp.indexed || lsp.is_file_open(rel),
            lsp.any_request_in_flight(),
            self.lsp_flush_requested
                || self
                    .lsp_flush_in_flight
                    .load(std::sync::atomic::Ordering::Relaxed),
            self.ed.completion_open || self.ed.rename_active,
        );
        match d {
            Decision::Restart => {
                drop(lsp);
                self.ed.idle_sync = Some((rel.to_owned(), hash, std::time::Instant::now()));
                // The frame must come back on its own: the user has stopped
                // typing, so nothing else will wake it.
                ctx.request_repaint_after(IDLE);
            }
            Decision::InSync => {
                let fresh = lsp.diagnostics_fresh(rel);
                drop(lsp);
                // Synced but the publish has not landed. Keep asking, because
                // rust-analyzer's own repaint request can be dropped.
                if !fresh {
                    ctx.request_repaint_after(AWAIT_PUBLISH);
                }
            }
            Decision::Wait => {
                drop(lsp);
                ctx.request_repaint_after(RETRY);
            }
            Decision::Send => {
                // `force = false` on purpose. A forced re-send skips the
                // `changed` branch, so `awaiting_diagnostics` is never set and
                // `diagnostics_fresh` would go true before rust-analyzer had
                // republished — painting the OLD diagnostics at the NEW text's
                // line/cols, which is the very thing the blanking prevents.
                lsp.did_change(rel, display_code, false);
                drop(lsp);
                ctx.request_repaint_after(AWAIT_PUBLISH);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Decision, IDLE, decide};
    use std::time::Duration;

    /// Every input in the "all clear" position, so each test below can move one.
    fn ready(waited: Duration) -> Decision {
        decide(false, waited, false, true, true, false, false, false)
    }

    #[test]
    fn a_settled_dirty_file_is_sent() {
        assert_eq!(ready(IDLE), Decision::Send);
        assert_eq!(ready(IDLE + Duration::from_millis(1)), Decision::Send);
    }

    #[test]
    fn typing_restarts_the_clock() {
        assert_eq!(
            decide(true, IDLE * 10, false, true, true, false, false, false),
            Decision::Restart,
            "a keystroke must never leave a send armed behind it"
        );
    }

    #[test]
    fn a_file_the_analyzer_already_holds_sends_nothing() {
        assert_eq!(
            decide(false, IDLE * 10, true, true, true, false, false, false),
            Decision::InSync
        );
    }

    #[test]
    fn a_pause_shorter_than_the_debounce_waits() {
        assert_eq!(ready(IDLE / 2), Decision::Wait);
    }

    /// The cancellation guard. A `didChange` under an in-flight request makes
    /// rust-analyzer answer "content modified", and a lost `references` reply is
    /// recorded as "0 references" — which fades live code as dead.
    #[test]
    fn a_request_in_flight_defers_the_send() {
        let busy = decide(false, IDLE * 10, false, true, true, true, false, false);
        assert_eq!(
            busy,
            Decision::Wait,
            "must not interrupt an answer we asked for"
        );
    }

    /// Deferred, never dropped: the usages reference chain is serialized and can
    /// run for many seconds, and that is exactly the file whose blank overlay
    /// hurts most. `Wait` re-arms; only `Restart` clears the deadline.
    #[test]
    fn a_deferred_send_still_fires_once_the_analyzer_frees_up() {
        assert_eq!(
            decide(false, IDLE * 10, false, true, true, true, false, false),
            Decision::Wait
        );
        assert_eq!(
            decide(false, IDLE * 10, false, true, true, false, false, false),
            Decision::Send,
            "the same frame with the analyzer idle must send"
        );
    }

    #[test]
    fn a_save_already_syncing_is_left_to_do_it() {
        assert_eq!(
            decide(false, IDLE * 10, false, true, true, false, true, false),
            Decision::Wait
        );
    }

    /// Never cut across the user's own popup: a version bump cancels the
    /// completion or rename request feeding it.
    #[test]
    fn an_open_popup_is_not_interrupted() {
        assert_eq!(
            decide(false, IDLE * 10, false, true, true, false, false, true),
            Decision::Wait
        );
    }

    /// `did_change` AUTO-OPENS an unopened document, and opening one before the
    /// index is built is what produces phantom type errors on a detached file.
    /// This is F12's gate, and skipping it is the one way this path could invent
    /// errors rather than reveal them.
    #[test]
    fn a_file_the_analyzer_has_not_opened_is_never_auto_opened_here() {
        assert_eq!(
            decide(false, IDLE * 10, false, true, false, false, false, false),
            Decision::Wait
        );
    }

    #[test]
    fn a_stopped_analyzer_is_not_talked_to() {
        assert_eq!(
            decide(false, IDLE * 10, false, false, true, false, false, false),
            Decision::Wait
        );
    }

    /// The ordering that makes this safe, pinned rather than commented: an edit
    /// burst is handed to rust-analyzer BEFORE the usages pass asks about the
    /// same file, so that pass always queries a document RA already agrees with.
    #[test]
    fn the_sync_lands_before_the_usages_pass() {
        assert!(
            IDLE < super::super::usages::DEBOUNCE,
            "idle sync at {IDLE:?} must precede the usages debounce at {:?}",
            super::super::usages::DEBOUNCE
        );
    }
}
