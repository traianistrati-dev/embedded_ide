//! Activity log — a per-action timing breakdown so the user can SEE what runs
//! (and how long it takes) on each Save / Build / Flash / Clippy, to understand
//! where the time goes.
//!
//! Each action mostly runs on a single background thread, so a caller builds a
//! [`Recorder`] locally (timing phases with `Instant`) and commits the whole
//! [`Action`] to the shared [`ActivityLog`] at the end — no cross-thread ids.
//! Rendered by `app/tabs/activity_tab.rs`.

use std::time::{Duration, Instant};

/// One timed step within an action: a label, its duration, and — for a
/// subprocess phase — the exact command line and its exit code.
#[derive(Clone)]
pub struct Phase {
    pub label: String,
    pub dur: Duration,
    /// The exact command line run (e.g. `cargo check --message-format=json`), if
    /// this phase was a subprocess.
    pub cmd: Option<String>,
    /// Process exit code, if applicable (`None` for in-process phases or a
    /// process killed by signal).
    pub exit: Option<i32>,
}

/// One user action (Save / Build / Flash / Clippy) and its phases.
#[derive(Clone)]
pub struct Action {
    pub kind: String,
    pub phases: Vec<Phase>,
    /// Wall-clock duration of the whole action.
    pub total: Duration,
    /// Seconds since the app started, for a rough chronological label.
    pub at: f64,
}

/// Newest-first list of recorded actions (capped).
#[derive(Default)]
pub struct ActivityLog {
    pub actions: Vec<Action>,
}

const MAX_ACTIONS: usize = 200;

impl ActivityLog {
    /// Commit a finished action (newest first).
    pub fn push(&mut self, action: Action) {
        self.actions.insert(0, action);
        self.actions.truncate(MAX_ACTIONS);
    }

    pub fn clear(&mut self) {
        self.actions.clear();
    }
}

/// Builds up one [`Action`]'s phases on the calling thread, then `finish`es it
/// into an [`Action`] the caller commits to the shared log.
pub struct Recorder {
    kind: String,
    started: Instant,
    phases: Vec<Phase>,
}

impl Recorder {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            started: Instant::now(),
            phases: Vec::new(),
        }
    }

    /// Time the closure `f`, record it as a phase named `label`, and return its
    /// result. In-process phases (no subprocess) — `cmd`/`exit` are `None`.
    pub fn phase<T>(&mut self, label: impl Into<String>, f: impl FnOnce() -> T) -> T {
        let t = Instant::now();
        let out = f();
        self.phases.push(Phase {
            label: label.into(),
            dur: t.elapsed(),
            cmd: None,
            exit: None,
        });
        out
    }

    /// Record a subprocess phase: its command line, duration and exit code.
    pub fn cmd_phase(
        &mut self,
        label: impl Into<String>,
        cmd: impl Into<String>,
        dur: Duration,
        exit: Option<i32>,
    ) {
        self.phases.push(Phase {
            label: label.into(),
            dur,
            cmd: Some(cmd.into()),
            exit,
        });
    }

    /// Record a pre-measured in-process phase (when the work can't be wrapped in
    /// a closure — e.g. it needs disjoint mutable borrows of the caller).
    pub fn add(&mut self, label: impl Into<String>, dur: Duration) {
        self.phases.push(Phase {
            label: label.into(),
            dur,
            cmd: None,
            exit: None,
        });
    }

    /// A zero-duration marker phase (e.g. "RA flycheck triggered").
    pub fn mark(&mut self, label: impl Into<String>) {
        self.phases.push(Phase {
            label: label.into(),
            dur: Duration::ZERO,
            cmd: None,
            exit: None,
        });
    }

    /// Finalise into an [`Action`] (total = wall-clock since `new`).
    pub fn finish(self) -> Action {
        Action {
            kind: self.kind,
            phases: self.phases,
            total: self.started.elapsed(),
            at: crate::activity::since_start(),
        }
    }

    /// Finalise with an explicit total — for actions whose span was measured
    /// elsewhere and only COMMITTED here afterwards (e.g. rust-analyzer's
    /// flycheck, timed by its `$/progress` begin/end notifications).
    pub fn finish_with_total(self, total: Duration) -> Action {
        Action {
            kind: self.kind,
            phases: self.phases,
            total,
            at: crate::activity::since_start(),
        }
    }
}

/// A [`Recorder`] that commits its action to a shared [`ActivityLog`] on drop —
/// so a function with many early-return paths (e.g. flashing) still logs the
/// phases it got through, without a `push` at every exit.
pub struct Committing {
    rec: Option<Recorder>,
    log: std::sync::Arc<std::sync::Mutex<ActivityLog>>,
}

impl Committing {
    pub fn new(kind: impl Into<String>, log: std::sync::Arc<std::sync::Mutex<ActivityLog>>) -> Self {
        Self {
            rec: Some(Recorder::new(kind)),
            log,
        }
    }

    /// Mutable access to the underlying recorder (add phases via its methods).
    pub fn rec(&mut self) -> &mut Recorder {
        self.rec.as_mut().expect("recorder present until drop")
    }
}

impl Drop for Committing {
    fn drop(&mut self) {
        if let Some(rec) = self.rec.take() {
            if let Ok(mut log) = self.log.lock() {
                log.push(rec.finish());
            }
        }
    }
}

use std::sync::OnceLock;
static START: OnceLock<Instant> = OnceLock::new();

/// Seconds since the first call (≈ app start) — a cheap chronological stamp.
pub fn since_start() -> f64 {
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_secs_f64()
}

/// Format a duration compactly: `9ms`, `0.12s`, `38.7s`, `1m03s`.
pub fn fmt_dur(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        let secs = d.as_secs_f64();
        if secs < 60.0 {
            format!("{secs:.2}s")
        } else {
            let m = (secs / 60.0) as u64;
            let s = secs - (m as f64) * 60.0;
            format!("{m}m{s:04.1}s")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_dur_scales() {
        assert_eq!(fmt_dur(Duration::from_millis(9)), "9ms");
        assert_eq!(fmt_dur(Duration::from_millis(120)), "120ms");
        assert_eq!(fmt_dur(Duration::from_millis(2410)), "2.41s");
        assert_eq!(fmt_dur(Duration::from_secs_f64(63.4)), "1m03.4s");
    }

    #[test]
    fn recorder_collects_phases() {
        let mut r = Recorder::new("Build");
        r.phase("a", || {});
        r.cmd_phase("cargo check", "cargo check", Duration::from_millis(10), Some(0));
        r.mark("marker");
        let a = r.finish();
        assert_eq!(a.kind, "Build");
        assert_eq!(a.phases.len(), 3);
        assert_eq!(a.phases[1].cmd.as_deref(), Some("cargo check"));
        assert_eq!(a.phases[1].exit, Some(0));
        assert_eq!(a.phases[2].dur, Duration::ZERO);
    }

    #[test]
    fn log_is_newest_first_and_capped() {
        let mut log = ActivityLog::default();
        for i in 0..(MAX_ACTIONS + 5) {
            log.push(Recorder::new(format!("a{i}")).finish());
        }
        assert_eq!(log.actions.len(), MAX_ACTIONS);
        // Last pushed is at the front.
        assert_eq!(log.actions[0].kind, format!("a{}", MAX_ACTIONS + 4));
    }
}
