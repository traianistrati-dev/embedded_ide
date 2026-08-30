//! Stopping a flash that is already running — shared by all three flashing
//! paths (probe-rs `cargo flash`, OpenOCD SWD, espflash).
//!
//! Why a handle instead of "kill the pid we stored":
//!
//! * Two of the three paths run **two children in sequence** — `cargo build
//!   --release` first, then the flashing tool. A Stop pressed during the build
//!   must kill the build AND stop the run from going on to open the probe, so a
//!   pid alone is not enough: the intent has to survive the gap between the two
//!   children.
//! * The pid must be forgotten the moment its child exits. The OS reuses pids,
//!   and a stale one aimed at a stranger is the worst kind of bug.
//!
//! Always the process TREE, never the process: `cargo flash` / `cargo build`
//! IS cargo, which spawns the real work as a child. Killing only the parent
//! leaves that child holding the probe or the serial port, and the next flash
//! fails to open it — which is the very state the user pressed Stop to escape.

use std::sync::{Arc, Mutex};

/// The child a flash is running right now, plus whether the user asked to stop.
#[derive(Default)]
pub struct FlashChild {
    /// pid of the child running at this instant, if any.
    pid: Option<u32>,
    /// Set by [`request_stop`], cleared by [`arm`] when the next run starts.
    stop_requested: bool,
}

/// Shared with the flashing thread; one per flashing path.
pub type FlashHandle = Arc<Mutex<FlashChild>>;

/// A fresh handle for an `AppIde` field.
pub fn handle() -> FlashHandle {
    Arc::new(Mutex::new(FlashChild::default()))
}

/// Start of a run: forget the previous one's stop request.
pub fn arm(h: &FlashHandle) {
    let mut g = h.lock().unwrap();
    g.pid = None;
    g.stop_requested = false;
}

/// Publish the child that is running now.
///
/// Returns `false` when a stop was requested while this child was being
/// spawned — the pid was not in the slot yet, so [`request_stop`] could not
/// reach it and the caller must kill it itself and give up.
#[must_use]
pub fn publish(h: &FlashHandle, pid: u32) -> bool {
    let mut g = h.lock().unwrap();
    if g.stop_requested {
        return false;
    }
    g.pid = Some(pid);
    true
}

/// The child just exited: drop its pid so nobody can kill it later, and report
/// whether this run was stopped by the user (`true`) or ended on its own.
pub fn finished(h: &FlashHandle) -> bool {
    let mut g = h.lock().unwrap();
    g.pid = None;
    g.stop_requested
}

/// Between two phases — a build that succeeded, before the flashing tool is
/// spawned: did the user ask to stop while the build was running?
pub fn stop_requested(h: &FlashHandle) -> bool {
    h.lock().unwrap().stop_requested
}

/// Stop the running flash: remember the request (there may be no child at this
/// instant — see the module note) and kill the tree of the one there is.
///
/// `what` names the tool in the log line, e.g. "cargo flash".
pub fn request_stop(h: &FlashHandle, log: &Arc<Mutex<Vec<String>>>, what: &str) {
    let pid = {
        let mut g = h.lock().unwrap();
        g.stop_requested = true;
        g.pid.take()
    };
    log.lock()
        .unwrap()
        .push(format!("> stopping flash (killing {what})…"));
    if let Some(pid) = pid {
        crate::lsp::kill_process_tree(pid);
    }
}

/// The message a stopped run reports. It is the user's own doing, so it must
/// not read like a failure with a cause to hunt for.
pub const STOPPED: &str = "flash stopped";

#[cfg(test)]
mod tests {
    use super::*;

    fn log() -> Arc<Mutex<Vec<String>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    #[test]
    fn a_run_that_ends_on_its_own_is_not_reported_as_stopped() {
        let h = handle();
        arm(&h);
        assert!(publish(&h, 4242));
        assert!(!finished(&h));
    }

    #[test]
    fn a_stop_between_phases_is_remembered_until_the_next_phase_looks() {
        let h = handle();
        arm(&h);
        // Build phase over, its child already reaped…
        assert!(publish(&h, 1));
        assert!(!stop_requested(&h));
        finished(&h);
        // …Stop pressed in the gap before the flashing tool is spawned.
        request_stop(&h, &log(), "cargo build");
        assert!(
            stop_requested(&h),
            "the flashing tool would be spawned anyway"
        );
    }

    #[test]
    fn a_child_spawned_after_the_stop_refuses_the_slot() {
        let h = handle();
        arm(&h);
        request_stop(&h, &log(), "cargo build");
        assert!(
            !publish(&h, 99),
            "the caller must kill this one itself — nothing else can reach it"
        );
    }

    #[test]
    fn arming_the_next_run_forgets_the_previous_stop() {
        let h = handle();
        arm(&h);
        request_stop(&h, &log(), "cargo flash");
        arm(&h);
        assert!(!stop_requested(&h));
        assert!(publish(&h, 7));
        assert!(!finished(&h), "a fresh run must not report itself stopped");
    }

    #[test]
    fn an_exited_child_leaves_no_pid_behind_to_kill() {
        let h = handle();
        arm(&h);
        assert!(publish(&h, 1234));
        finished(&h);
        // A Stop arriving late must find nothing: the OS may have handed 1234
        // to an unrelated process by now.
        request_stop(&h, &log(), "cargo flash");
        assert!(h.lock().unwrap().pid.is_none());
    }

    #[test]
    fn stopping_says_so_in_the_flash_log() {
        let h = handle();
        let l = log();
        arm(&h);
        request_stop(&h, &l, "espflash");
        assert_eq!(l.lock().unwrap().len(), 1);
        assert!(l.lock().unwrap()[0].contains("espflash"));
    }
}
