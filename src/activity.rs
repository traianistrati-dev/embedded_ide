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
    /// Wall-clock start/end, so the tab can show when it ran and how long the
    /// gap to the next action was. `SystemTime` (not `Instant`) because these
    /// are displayed as clock times.
    pub started_at: std::time::SystemTime,
    pub ended_at: std::time::SystemTime,
    /// The worker died before committing normally (panic / early return). Its
    /// in-flight flag would otherwise have hung the status bar forever, so this
    /// makes the failure visible instead of silent.
    pub aborted: bool,
    /// Which user Save this belongs to. ONE Ctrl+S produces several actions
    /// (project write → LSP flush → wall clock), and they all carry the same
    /// id so the tab can group them. `None` = standalone (Build / Flash /
    /// Clippy / Git).
    ///
    /// An explicit id, not a name prefix: every one of those actions is called
    /// "Save (…)", so grouping on the name gave each its own group — and not a
    /// timestamp either, because two quick saves must stay separate.
    pub session: Option<u64>,
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
    started_wall: std::time::SystemTime,
    phases: Vec<Phase>,
    session: Option<u64>,
}

impl Recorder {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            started: Instant::now(),
            started_wall: std::time::SystemTime::now(),
            phases: Vec::new(),
            session: None,
        }
    }

    /// Time the closure `f`, record it as a phase named `label`, and return its
    /// result. In-process phases (no subprocess) — `cmd`/`exit` are `None`.
    /// Tag this action as part of user Save `id`, so the tab groups it with the
    /// other actions of the same Ctrl+S.
    pub fn in_session(mut self, id: u64) -> Self {
        self.session = Some(id);
        self
    }

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
            started_at: self.started_wall,
            ended_at: std::time::SystemTime::now(),
            aborted: false,
            session: self.session,
        }
    }

    /// Finalise an action whose worker did NOT reach its normal end (panic or
    /// early return). Logged so a hung in-flight flag has a visible cause.
    pub fn finish_aborted(self, why: impl Into<String>) -> Action {
        let mut a = self.finish();
        a.aborted = true;
        a.phases.push(Phase {
            label: format!("ABORTED — {}", why.into()),
            dur: Duration::ZERO,
            cmd: None,
            exit: None,
        });
        a
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
            started_at: self.started_wall,
            ended_at: std::time::SystemTime::now(),
            aborted: false,
            session: self.session,
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
    pub fn new(
        kind: impl Into<String>,
        log: std::sync::Arc<std::sync::Mutex<ActivityLog>>,
    ) -> Self {
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
    /// Covered by `a_committing_recorder_logs_even_when_the_action_bails_out`:
    /// three flash paths return early on a dozen error paths each, and this is
    /// the only thing that makes the entry appear anyway.
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
        r.cmd_phase(
            "cargo check",
            "cargo check",
            Duration::from_millis(10),
            Some(0),
        );
        r.mark("marker");
        let a = r.finish();
        assert_eq!(a.kind, "Build");
        assert_eq!(a.phases.len(), 3);
        assert_eq!(a.phases[1].cmd.as_deref(), Some("cargo check"));
        assert_eq!(a.phases[1].exit, Some(0));
        assert_eq!(a.phases[2].dur, Duration::ZERO);
    }

    /// The property every flash path leans on: the entry appears even when the
    /// operation FAILED.
    ///
    /// `espflash.rs`, `openocd.rs` and `dfu.rs` all build a `Committing` and
    /// then return early on any of a dozen error paths - a missing tool, a
    /// failed build, an unplugged board. Its whole reason to exist is that none
    /// of those has to remember to log, and nothing covered it.
    #[test]
    fn a_committing_recorder_logs_even_when_the_action_bails_out() {
        let log = std::sync::Arc::new(std::sync::Mutex::new(ActivityLog::default()));

        // The shape of a real flash: one phase done, then an early return.
        (|| {
            let mut act = Committing::new("Flash (ESP / espflash)", log.clone());
            act.rec()
                .add("cargo build --release", Duration::from_millis(120));
            // espflash is missing / the board is unplugged - bail.
        })();

        let entries = &log.lock().unwrap().actions;
        assert_eq!(entries.len(), 1, "the failed action still logged");
        assert_eq!(entries[0].kind, "Flash (ESP / espflash)");
        assert_eq!(
            entries[0].phases.len(),
            1,
            "and kept the phase it got through"
        );
    }

    /// An exit code of a FAILED command is recorded, not dropped - the tab's
    /// whole use on a flash that did not work.
    #[test]
    fn a_failing_command_keeps_its_exit_code() {
        let log = std::sync::Arc::new(std::sync::Mutex::new(ActivityLog::default()));
        {
            let mut act = Committing::new("Flash (ESP / espflash)", log.clone());
            act.rec().cmd_phase(
                "espflash flash",
                "espflash flash --chip esp32c6".to_owned(),
                Duration::from_millis(900),
                Some(2),
            );
        }
        let entries = &log.lock().unwrap().actions;
        let p = &entries[0].phases[0];
        assert_eq!(p.exit, Some(2), "a non-zero exit is the interesting one");
        assert!(
            p.cmd
                .as_deref()
                .is_some_and(|c| c.contains("--chip esp32c6")),
            "the command names the chip, so two boards are told apart: {:?}",
            p.cmd
        );
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

/// Local clock time `HH:MM:SS.mmm` for a `SystemTime`. Pure — tested below.
///
/// Deliberately hand-rolled: the crate has no `chrono`/`time` dependency, and
/// the tab only needs a within-the-day stamp to line actions up against each
/// other. UTC-based, so it can be off from the wall clock by the timezone
/// offset — fine for measuring gaps, which is what it is for.
pub fn fmt_clock(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ms = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis())
        .unwrap_or(0);
    let day = secs % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        day / 3600,
        (day % 3600) / 60,
        day % 60,
        ms
    )
}

/// Clears a "work in flight" flag when dropped — **including while unwinding
/// from a panic**.
///
/// Why this exists: the status bar shows "Saving…" while
/// `lsp_flush_in_flight` is set, and the worker cleared it on its LAST line.
/// A panic anywhere in that thread (a poisoned mutex is enough — every
/// `.lock().unwrap()` in the app panics once any other thread died holding
/// one) skipped that line, so the flag stayed set FOREVER: the status bar hung
/// at "Saving…", and because a visible spinner keeps requesting repaints the
/// app never went idle again — sustained CPU with nothing running. Rare and
/// baffling, exactly as reported.
///
/// `panic = "unwind"` (the default) is what makes the Drop fix work at all.
pub struct FlagGuard {
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl FlagGuard {
    /// Set `flag` and clear it again when the returned guard drops.
    pub fn set(flag: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        flag.store(true, std::sync::atomic::Ordering::Release);
        Self { flag }
    }
}

impl Drop for FlagGuard {
    fn drop(&mut self) {
        self.flag.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod clock_tests {
    use super::fmt_clock;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn clock_formats_within_the_day_and_wraps() {
        // 01:02:03.004 after midnight.
        let t = UNIX_EPOCH + Duration::from_millis(((1 * 3600) + (2 * 60) + 3) * 1000 + 4);
        assert_eq!(fmt_clock(t), "01:02:03.004");
        // Exactly one day later must read the same — it is a within-day stamp.
        assert_eq!(fmt_clock(t + Duration::from_secs(86_400)), "01:02:03.004");
        assert_eq!(fmt_clock(UNIX_EPOCH), "00:00:00.000");
    }
}

#[cfg(test)]
mod flag_guard_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn guard_clears_on_normal_drop() {
        let f = Arc::new(AtomicBool::new(false));
        {
            let _g = FlagGuard::set(Arc::clone(&f));
            assert!(
                f.load(Ordering::Acquire),
                "flag must be set inside the scope"
            );
        }
        assert!(!f.load(Ordering::Acquire), "flag must clear on drop");
    }

    #[test]
    fn guard_clears_even_when_the_worker_panics() {
        // THE case this type exists for: a panicking worker used to leave the
        // flag set, hanging the status bar at "Saving…" for the rest of the
        // session and keeping the app repainting.
        let f = Arc::new(AtomicBool::new(false));
        let f2 = Arc::clone(&f);
        let joined = std::thread::spawn(move || {
            let _g = FlagGuard::set(f2);
            panic!("simulated worker panic");
        })
        .join();
        assert!(joined.is_err(), "the thread was supposed to panic");
        assert!(
            !f.load(Ordering::Acquire),
            "flag must be cleared by unwinding, or the UI hangs forever"
        );
    }
}
