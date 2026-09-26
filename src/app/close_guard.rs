//! The window-close gate: a close with unsaved work is cancelled and the
//! "Unsaved changes" prompt goes up instead.
//!
//! It runs from `App::logic`, never from `App::ui`. eframe calls `ui` only
//! while the window is visible — 0.34 skips it when `ViewportInfo::visible()`
//! is `Some(false)`, 0.36 runs no egui pass at all then (`update_logic_only`) —
//! but it calls `logic` on every pass. A close sent to a MINIMIZED window (the
//! taskbar's "Close window", its thumbnail's X) therefore never reached a check
//! at the top of `ui`: eframe saw no `CancelClose` and exited over the work.
//!
//! What it may use is the window state, which `logic` sees fresh in both
//! versions; 0.36 hands `logic` the LAST shown frame's events and time while
//! minimized, so nothing here reads either.

use eframe::egui;

#[derive(Default)]
pub(super) struct CloseGuard {
    /// The "Unsaved changes" prompt is up: a close was cancelled.
    pub(super) prompt: bool,
    /// The user decided, so the close we send next must go through.
    pub(super) allow: bool,
    /// Close as soon as the in-flight Save lands ("Save and close").
    pub(super) after_save: bool,
}

impl CloseGuard {
    /// A close is being asked for and has not been decided yet.
    ///
    /// Asked first, on its own, because deciding means diffing the project
    /// against the disk — work for the rare pass that carries a close, not for
    /// every pass.
    pub(super) fn wants_decision(&self, ctx: &egui::Context) -> bool {
        !self.allow && ctx.input(|i| i.viewport().close_requested())
    }

    /// Decide the close [`Self::wants_decision`] found.
    ///
    /// Nothing unsaved: let it through. Otherwise cancel it, put the prompt up,
    /// and bring the window back if it was minimized — nothing is drawn while it
    /// is, so a prompt raised then would wait, invisible, for the user to find it.
    pub(super) fn decide(&mut self, ctx: &egui::Context, has_unsaved: bool) {
        if !has_unsaved {
            self.allow = true;
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.prompt = true;
        if ctx.input(|i| i.viewport().minimized) == Some(true) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        }
        // After the restore: winit's `focus_window` does nothing to a window
        // that is still minimized.
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    /// The user settled it (Close without saving, or a save that landed): let
    /// our own close through and send it.
    pub(super) fn close_now(&mut self, ctx: &egui::Context) {
        self.prompt = false;
        self.after_save = false;
        self.allow = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::CloseGuard;
    use eframe::egui::{self, ViewportCommand, ViewportEvent, ViewportId};

    /// One pass the way eframe runs it for the root window: `logic` first, and
    /// `ui` only while visible. A minimized pass therefore runs the gate and
    /// nothing else — exactly what used to lose the close.
    fn pass(
        guard: &mut CloseGuard,
        minimized: bool,
        close: bool,
        has_unsaved: bool,
    ) -> (Vec<ViewportCommand>, bool) {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        let root = input.viewports.entry(ViewportId::ROOT).or_default();
        root.minimized = Some(minimized);
        if close {
            root.events.push(ViewportEvent::Close);
        }
        let mut asked = false;
        let mut out = crate::headless::run_ui(&ctx, input, |ui| {
            // App::logic
            if guard.wants_decision(ui.ctx()) {
                asked = true;
                guard.decide(ui.ctx(), has_unsaved);
            }
            // App::ui would follow here only when !minimized.
        });
        let cmds = out
            .viewport_output
            .remove(&ViewportId::ROOT)
            .map(|o| o.commands)
            .unwrap_or_default();
        (cmds, asked)
    }

    #[test]
    fn a_close_while_minimized_is_cancelled_and_the_window_comes_back() {
        let mut guard = CloseGuard::default();
        let (cmds, _) = pass(&mut guard, true, true, true);
        assert!(cmds.contains(&ViewportCommand::CancelClose), "{cmds:?}");
        assert!(
            cmds.contains(&ViewportCommand::Minimized(false)),
            "{cmds:?}"
        );
        let restore = cmds
            .iter()
            .position(|c| *c == ViewportCommand::Minimized(false));
        let focus = cmds.iter().position(|c| *c == ViewportCommand::Focus);
        assert!(restore < focus, "restore BEFORE focus: {cmds:?}");
        assert!(guard.prompt, "the prompt is up for the next visible frame");
        assert!(!guard.allow);
    }

    #[test]
    fn a_close_of_a_visible_window_does_not_touch_its_size() {
        let mut guard = CloseGuard::default();
        let (cmds, _) = pass(&mut guard, false, true, true);
        assert!(cmds.contains(&ViewportCommand::CancelClose), "{cmds:?}");
        assert!(
            !cmds.contains(&ViewportCommand::Minimized(false)),
            "{cmds:?}"
        );
        assert!(guard.prompt);
    }

    #[test]
    fn nothing_unsaved_lets_the_close_through() {
        let mut guard = CloseGuard::default();
        let (cmds, asked) = pass(&mut guard, true, true, false);
        assert!(asked);
        assert!(!cmds.contains(&ViewportCommand::CancelClose), "{cmds:?}");
        assert!(guard.allow && !guard.prompt);
    }

    /// The close sent after the user decided must not be caught again - that
    /// loop is what `allow` is for.
    #[test]
    fn our_own_close_is_not_intercepted() {
        let mut guard = CloseGuard::default();
        // "Close without saving": outside a pass is fine, commands queue on ROOT.
        guard.close_now(&egui::Context::default());
        let (cmds, asked) = pass(&mut guard, false, true, true);
        assert!(!asked, "decided already - no second diff");
        assert!(!cmds.contains(&ViewportCommand::CancelClose), "{cmds:?}");
    }

    /// eframe 0.36's pass for a minimized root window: `App::logic` alone,
    /// through `Context::run_logic` - no egui pass, only the window state
    /// refreshed. The gate must work from exactly that.
    #[test]
    fn a_minimized_close_through_run_logic_is_cancelled_and_restores() {
        let ctx = egui::Context::default();
        let mut guard = CloseGuard::default();
        let mut input = egui::RawInput::default();
        let root = input.viewports.entry(ViewportId::ROOT).or_default();
        root.minimized = Some(true);
        root.events.push(ViewportEvent::Close);
        let out = ctx.run_logic(&input, |ctx| {
            if guard.wants_decision(ctx) {
                guard.decide(ctx, true);
            }
        });
        let cmds = out
            .viewport_commands
            .get(&ViewportId::ROOT)
            .cloned()
            .unwrap_or_default();
        assert!(cmds.contains(&ViewportCommand::CancelClose), "{cmds:?}");
        assert!(
            cmds.contains(&ViewportCommand::Minimized(false)),
            "{cmds:?}"
        );
        let restore = cmds
            .iter()
            .position(|c| *c == ViewportCommand::Minimized(false));
        let focus = cmds.iter().position(|c| *c == ViewportCommand::Focus);
        assert!(restore < focus, "restore BEFORE focus: {cmds:?}");
        assert!(guard.prompt);
    }

    /// Deciding diffs the project against the disk: never on a pass that
    /// carries no close.
    #[test]
    fn a_pass_without_a_close_asks_nothing() {
        let mut guard = CloseGuard::default();
        let (cmds, asked) = pass(&mut guard, true, false, true);
        assert!(!asked);
        // Not `is_empty`: egui 0.36's first pass emits `SetTheme` of its own.
        let ours = |c: &ViewportCommand| {
            matches!(
                c,
                ViewportCommand::CancelClose
                    | ViewportCommand::Close
                    | ViewportCommand::Minimized(_)
                    | ViewportCommand::Focus
            )
        };
        assert!(!cmds.iter().any(ours), "{cmds:?}");
        assert!(!guard.prompt);
    }
}
