//! Full-window "the project is loading" overlay.
//!
//! Switching project (Open, New, a git branch switch, the startup restore…)
//! looks instant but isn't: replacing the in-memory files is only the first
//! step. Afterwards the temp workspace is rewritten, rust-analyzer restarts and
//! re-indexes, the flush + flycheck run, and `cargo metadata` re-validates the
//! workspace — seconds to tens of seconds during which the UI is interactive
//! but shows a half-loaded project (stale errors, empty Structure, dead
//! completions). The only sign of it was an 11 px word in the bottom status bar.
//!
//! So while that chain runs, a near-opaque black overlay covers the whole
//! window with one big spinner and the CURRENT phase spelled out underneath.
//! It is an `egui::Modal`, which blocks input to everything below it — the
//! point is that the half-loaded state is not usable anyway.
//!
//! It can never wedge the app: it lifts as soon as the work goes quiet, and in
//! any case after [`MAX_VISIBLE`] or on Escape.

use super::AppIde;
use eframe::egui;
use std::time::{Duration, Instant};

/// Shortest time the overlay stays up. Without it a fast local project would
/// make it flash for two frames, which reads as a glitch rather than progress.
const MIN_VISIBLE: Duration = Duration::from_millis(400);

/// How long everything must stay idle before the overlay lifts.
///
/// The busy signal has real GAPS: the LSP flush is only *requested* during the
/// load frame and starts on the next one, and RA's flycheck begins later still
/// (after `didSave`). Lifting on the first idle frame would flicker the overlay
/// two or three times per project switch.
const QUIET_GRACE: Duration = Duration::from_millis(700);

/// Hard cap. A wedged rust-analyzer must never hold the UI hostage.
const MAX_VISIBLE: Duration = Duration::from_secs(25);

/// What put the overlay up — only the wording differs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LoadKind {
    /// An existing project replaced the current one (Open, clone, git reload).
    Open,
    /// New Project — every user file cleared, fresh config for the chip.
    New,
    /// The startup reload of the last opened project.
    Restore,
}

impl LoadKind {
    fn title(self) -> &'static str {
        match self {
            Self::Open => "Opening project",
            Self::New => "Creating new project",
            Self::Restore => "Restoring project",
        }
    }
}

/// In-flight overlay state. `None` on `AppIde` means no overlay.
pub(super) struct ProjectLoading {
    kind: LoadKind,
    /// When the project change started (drives `MIN_VISIBLE` / `MAX_VISIBLE`).
    started: Instant,
    /// When the busy signal last went quiet; reset to `None` by any activity.
    quiet_since: Option<Instant>,
}

impl AppIde {
    /// Put the loading overlay up (or restart its clock if one is already up).
    ///
    /// Called from the project-change entry points, not from Save/Build — those
    /// have the status bar and leave a usable project on screen.
    pub(super) fn begin_project_loading(&mut self, kind: LoadKind) {
        self.project_loading = Some(ProjectLoading {
            kind,
            started: Instant::now(),
            quiet_since: None,
        });
        // The load itself runs synchronously inside this frame, so the overlay
        // first paints on the NEXT one — make sure that frame comes even if the
        // window is otherwise idle (the startup restore has no pending input).
        self.egui_ctx.request_repaint();
    }

    /// Tick the overlay and draw it. Call LAST in the frame, after every panel.
    ///
    /// Ticking and drawing are one step on purpose: the decision to lift needs
    /// this frame's busy state, and the spinner's own `request_repaint_after`
    /// is what keeps the grace timer advancing while nothing else repaints.
    pub(super) fn show_project_loading_overlay(&mut self, ui: &mut egui::Ui) {
        let Some(mut state) = self.project_loading.take() else {
            return;
        };

        // Re-read the busy signal HERE rather than reusing the one computed at
        // the top of the frame: a project change applied mid-frame (a tree
        // click, a finished save) only shows up in this later read.
        let status = self.activity_status();
        // The bool is "has a spinner" — i.e. work in flight, as opposed to the
        // transient "saved ✓" message, which must not hold the overlay up.
        let busy = status.as_ref().is_some_and(|(spinner, ..)| *spinner);

        let now = Instant::now();
        if busy {
            state.quiet_since = None;
        } else {
            state.quiet_since.get_or_insert(now);
        }
        let elapsed = now.saturating_duration_since(state.started);
        let settled = elapsed >= MIN_VISIBLE
            && state
                .quiet_since
                .is_some_and(|q| now.saturating_duration_since(q) >= QUIET_GRACE);
        // Read without consuming: the editor's popups consume Escape earlier in
        // the frame, but none of them are open during a project switch.
        let escaped = ui.ctx().input(|i| i.key_pressed(egui::Key::Escape));
        // Closing the window mid-load puts the unsaved-changes prompt up. It
        // renders BELOW this overlay and its buttons would be unclickable, so
        // the prompt wins — the overlay is only about visibility, never about
        // holding a decision hostage.
        if settled || escaped || self.exit_prompt || elapsed >= MAX_VISIBLE {
            return; // dropped — no overlay from here on
        }

        let title = state.kind.title();
        let project = self.project_name.clone();
        // The live phase, straight from the status bar's own wording
        // ("Indexing…", "Checking… 4s", "Saving…") — the thing that was
        // previously invisible.
        let (phase, phase_color) = match &status {
            Some((_, text, color)) => (text.clone(), *color),
            None => ("Finishing up…".to_owned(), egui::Color32::from_gray(150)),
        };
        let secs = elapsed.as_secs();

        egui::Modal::new(egui::Id::new("project_loading_overlay"))
            // 95 % black over the whole window (242 / 255).
            .backdrop_color(egui::Color32::from_black_alpha(242))
            // No popup frame — the centred content IS the dialog.
            .frame(egui::Frame::NONE)
            .show(ui.ctx(), |ui| {
                ui.set_width(420.0);
                ui.vertical_centered(|ui| {
                    super::helpers::spinner::throttled_spinner_stroked(ui, 68.0, 5.0);
                    ui.add_space(22.0);
                    ui.label(
                        egui::RichText::new(match &project {
                            Some(name) if state.kind != LoadKind::New => {
                                format!("{title} — {name}")
                            }
                            _ => title.to_owned(),
                        })
                        .size(17.0)
                        .strong()
                        .color(egui::Color32::from_gray(235)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(if secs >= 2 {
                            format!("{phase}  ({secs}s)")
                        } else {
                            phase
                        })
                        .size(12.5)
                        .color(phase_color),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "Loading files, workspace and code analysis — the project is \
                             only half-loaded until this finishes.",
                        )
                        .size(10.5)
                        .color(egui::Color32::from_gray(130)),
                    );
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new("Esc — continue in the background")
                            .size(10.0)
                            .italics()
                            .color(egui::Color32::from_gray(105)),
                    );
                });
            });

        self.project_loading = Some(state);
    }
}
