//! RTT / defmt live console via the probe-rs CLI (bottom-panel "RTT" tab).
//!
//! RTT (Real-Time Transfer) streams logs through the debug probe — no USART
//! pin, ~100× faster than a UART. probe-rs decodes `defmt` frames itself when
//! the ELF carries a defmt table; plain `rtt_target` strings pass through.
//!
//! Pipeline (one orchestrator thread per session, killable at every phase):
//!  1. `cargo build --release --message-format=json` — human progress from
//!     stderr streams into the console, the ELF path comes from the JSON
//!     `compiler-artifact` messages (same trick as `crate::size`).
//!  2. `probe-rs run --chip <chip> <elf>` (flash + reset + RTT) or
//!     `probe-rs attach --chip <chip> <elf>` (RTT only, no flash), stdout +
//!     stderr streamed live until the session is stopped or the probe drops.
//!
//! Reuses the Terminal tab's scrollback machinery ([`TerminalState`],
//! [`spawn_reader`]) so ANSI colours and throttled repaints come for free.

use crate::build::no_window;
use crate::terminal::{LineKind, TerminalState, spawn_reader};
use eframe::egui;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// What the session does before streaming: flash first, or attach only.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RttMode {
    /// `probe-rs run` — flash `--release`, reset, then stream RTT.
    Run,
    /// `probe-rs attach` — stream RTT from the already-running firmware.
    Attach,
}

#[derive(Clone, Default, PartialEq)]
pub enum RttPhase {
    #[default]
    Idle,
    /// `cargo build --release` is running.
    Building,
    /// probe-rs is attached and streaming.
    Streaming,
    /// The last session ended with an error (shown in the tab header row).
    Error(String),
}

/// The RTT console (owned by `AppIde.rtt`): scrollback + phase + the running
/// child (cargo, then probe-rs) so Stop can kill whichever is active.
pub struct RttConsole {
    pub state: Arc<Mutex<TerminalState>>,
    pub phase: Arc<Mutex<RttPhase>>,
    /// Per-session stop flag (readers + orchestrator bail out).
    stop: Option<Arc<AtomicBool>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Default for RttConsole {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(TerminalState::default())),
            phase: Arc::new(Mutex::new(RttPhase::Idle)),
            stop: None,
            child: Arc::new(Mutex::new(None)),
        }
    }
}

impl RttConsole {
    pub fn phase(&self) -> RttPhase {
        self.phase.lock().unwrap().clone()
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.phase(), RttPhase::Building | RttPhase::Streaming)
    }

    pub fn clear(&mut self) {
        self.state.lock().unwrap().lines.clear();
    }

    /// Kill the running child (cargo or probe-rs) and end the session.
    pub fn stop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
        if self.is_busy() {
            self.state
                .lock()
                .unwrap()
                .push_plain(LineKind::Notice, "[stopped]");
        }
        *self.phase.lock().unwrap() = RttPhase::Idle;
    }

    /// Start a session: build `--release` in `project_dir`, then launch
    /// probe-rs on the produced ELF. No-op while one is already running.
    pub fn start(
        &mut self,
        mode: RttMode,
        project_dir: PathBuf,
        target: String,
        chip: String,
        // The `--probe VID:PID[:Serial]` selector, or `None` to let probe-rs
        // auto-pick the only attached probe (see [`crate::probe`]).
        probe: Option<String>,
        ctx: egui::Context,
    ) {
        if self.is_busy() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(Arc::clone(&stop));
        *self.phase.lock().unwrap() = RttPhase::Building;

        let state = Arc::clone(&self.state);
        let phase = Arc::clone(&self.phase);
        let child_slot = Arc::clone(&self.child);
        thread::spawn(move || {
            let end = run_session(
                mode,
                &project_dir,
                &target,
                &chip,
                probe.as_deref(),
                &state,
                &phase,
                &child_slot,
                &stop,
                &ctx,
            );
            *child_slot.lock().unwrap() = None;
            // A user Stop already set Idle + logged; don't overwrite it.
            if !stop.load(Ordering::Relaxed) {
                *phase.lock().unwrap() = match end {
                    Ok(()) => RttPhase::Idle,
                    Err(e) => {
                        state
                            .lock()
                            .unwrap()
                            .push_plain(LineKind::Notice, format!("[error] {e}"));
                        RttPhase::Error(e)
                    }
                };
            }
            ctx.request_repaint();
        });
    }
}

/// `cargo build --release` with live console streaming: human progress
/// (stderr) and rendered compile errors go into `state`; the ELF path comes
/// from the JSON `compiler-artifact` messages. The cargo child sits in
/// `child_slot` while running so Stop can kill a long build (the final wait
/// happens on the child taken OUT of the slot — never while holding the lock).
/// Returns `Ok(None)` when the user stopped the build. Shared with the
/// debugger (`crate::debugger`).
pub(crate) fn cargo_build_streamed(
    project_dir: &std::path::Path,
    target: &str,
    state: &Arc<Mutex<TerminalState>>,
    child_slot: &Arc<Mutex<Option<Child>>>,
    stop: &Arc<AtomicBool>,
    ctx: &egui::Context,
) -> Result<Option<PathBuf>, String> {
    state.lock().unwrap().push_plain(
        LineKind::Input,
        format!("> cargo build --release --target {target}"),
    );
    ctx.request_repaint();

    let mut cargo = no_window(&mut Command::new("cargo"))
        .current_dir(project_dir)
        .args([
            "build",
            "--release",
            "--target",
            target,
            "--message-format=json",
            "--color=never",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not launch cargo: {e}"))?;

    // Cargo's human progress ("Compiling …") goes to stderr — stream it live.
    let done = Arc::new(AtomicUsize::new(0));
    if let Some(err) = cargo.stderr.take() {
        spawn_reader(
            err,
            LineKind::Stdout, // progress, not errors — keep it calm grey
            Arc::clone(state),
            Arc::clone(stop),
            ctx.clone(),
            Arc::clone(&done),
        );
    }
    let cargo_stdout = cargo.stdout.take();
    *child_slot.lock().unwrap() = Some(cargo);

    // The JSON stream (this thread): ELF path + rendered compile errors.
    let mut elf: Option<PathBuf> = None;
    let mut success = false;
    // Set when the link overflowed the chip's memory — that failure deserves a
    // real explanation, not "fix the errors above".
    let mut flash_note: Option<String> = None;
    if let Some(out) = cargo_stdout {
        for line in std::io::BufReader::new(out).lines().flatten() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            match v["reason"].as_str() {
                Some("compiler-artifact") => {
                    if let Some(exe) = v["executable"].as_str() {
                        elf = Some(PathBuf::from(exe));
                    }
                }
                Some("compiler-message") => {
                    if v["message"]["level"].as_str() == Some("error") {
                        if let Some(r) = v["message"]["rendered"].as_str() {
                            // A memory.x overflow hides inside the linker dump —
                            // keep the one line that says so for the failure below.
                            if flash_note.is_none() {
                                flash_note = crate::failure_hint::flash_overflow(r);
                            }
                            let mut s = state.lock().unwrap();
                            for l in r.lines() {
                                s.push_plain(LineKind::Stderr, l);
                            }
                        }
                    }
                }
                Some("build-finished") => {
                    success = v["success"].as_bool().unwrap_or(false);
                }
                _ => {}
            }
        }
    }
    // stdout hit EOF → cargo is done (or was killed); reap it outside the lock.
    let cargo_child = child_slot.lock().unwrap().take();
    let cargo_ok = cargo_child
        .map(|mut c| c.wait().ok().is_some_and(|st| st.success()))
        .unwrap_or(false);
    if stop.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if !(success && cargo_ok) {
        if let Some(detail) = flash_note {
            return Err(crate::failure_hint::flash_full_message(&detail));
        }
        return Err("build failed — fix the errors above and try again".into());
    }
    elf.map(Some)
        .ok_or_else(|| "build produced no executable artifact".to_string())
}

/// The probe-rs panic in a console it wrote into, if it crashed. Shared with the
/// Debug tab: a dying probe-rs looks like an ordinary exit / socket close, so
/// both orchestrators have to go looking for the crash themselves.
pub(crate) fn probe_rs_crash(console: &TerminalState) -> Option<String> {
    crate::failure_hint::probe_rs_panic(&console.tail_text(60))
}

/// The orchestrator body: build, then stream probe-rs until it exits.
#[allow(clippy::too_many_arguments)]
fn run_session(
    mode: RttMode,
    project_dir: &std::path::Path,
    target: &str,
    chip: &str,
    probe: Option<&str>,
    state: &Arc<Mutex<TerminalState>>,
    phase: &Arc<Mutex<RttPhase>>,
    child_slot: &Arc<Mutex<Option<Child>>>,
    stop: &Arc<AtomicBool>,
    ctx: &egui::Context,
) -> Result<(), String> {
    // ── Phase 1: cargo build --release ────────────────────────────────────────
    let Some(elf) = cargo_build_streamed(project_dir, target, state, child_slot, stop, ctx)? else {
        return Ok(()); // stopped by the user mid-build
    };
    state
        .lock()
        .unwrap()
        .push_plain(LineKind::Notice, "[OK] build — starting probe-rs…");

    // ── Phase 2: probe-rs run/attach ──────────────────────────────────────────
    let sub = match mode {
        RttMode::Run => "run",
        RttMode::Attach => "attach",
    };
    // `--probe VID:PID[:Serial]` pins the session to one probe; absent, probe-rs
    // auto-selects (and errors when several are attached).
    let probe_arg = probe
        .filter(|s| !s.is_empty())
        .map(|s| format!(" --probe {s}"))
        .unwrap_or_default();
    state.lock().unwrap().push_plain(
        LineKind::Input,
        format!(
            "> probe-rs {sub} --chip {chip}{probe_arg} {}",
            elf.display()
        ),
    );
    ctx.request_repaint();

    let mut cmd = Command::new("probe-rs");
    let builder = no_window(&mut cmd)
        .current_dir(project_dir)
        .args([sub, "--chip", chip]);
    if let Some(sel) = probe.filter(|s| !s.is_empty()) {
        builder.args(["--probe", sel]);
    }
    let mut probe = builder
        .arg(&elf)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "probe-rs not found in PATH.\n\
                 Install it with:  cargo install probe-rs-tools\n\
                 (or download from https://probe.rs)"
                    .to_string()
            } else {
                format!("could not launch probe-rs: {e}")
            }
        })?;

    *phase.lock().unwrap() = RttPhase::Streaming;
    ctx.request_repaint();

    let done = Arc::new(AtomicUsize::new(0));
    let mut pipes = 0;
    if let Some(out) = probe.stdout.take() {
        pipes += 1;
        spawn_reader(
            out,
            LineKind::Stdout,
            Arc::clone(state),
            Arc::clone(stop),
            ctx.clone(),
            Arc::clone(&done),
        );
    }
    if let Some(err) = probe.stderr.take() {
        pipes += 1;
        spawn_reader(
            err,
            LineKind::Stderr,
            Arc::clone(state),
            Arc::clone(stop),
            ctx.clone(),
            Arc::clone(&done),
        );
    }
    *child_slot.lock().unwrap() = Some(probe);

    // Wait until both pipes are done (probe-rs exited) or the user stopped.
    while done.load(Ordering::Relaxed) < pipes && !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(30));
    }
    let status = child_slot.lock().unwrap().take().map(|mut c| c.wait());
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    match status {
        Some(Ok(st)) if st.success() => {
            state
                .lock()
                .unwrap()
                .push_plain(LineKind::Notice, "[probe-rs exited]");
            Ok(())
        }
        Some(Ok(st)) => {
            // probe-rs can die of its own panic (a bug in its USB enumeration,
            // say) — that is not "check your wiring", so say what it really was.
            if let Some(detail) = probe_rs_crash(&state.lock().unwrap()) {
                return Err(crate::failure_hint::probe_rs_panic_message(&detail));
            }
            Err(format!(
                "probe-rs exited with {} — check the probe connection / chip name",
                st.code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into())
            ))
        }
        _ => Err("probe-rs could not be reaped".into()),
    }
}
