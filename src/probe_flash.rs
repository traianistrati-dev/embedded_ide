//! Flash the firmware over a debug probe with probe-rs' `cargo flash`, using the
//! SAME probe the Debug / RTT / Runtime(flamegraph) tabs use (`selected_probe`).
//! This is the Flash tab's probe-rs path — one shared probe across all four (the
//! existing DFU / OpenOCD-SWD / espflash paths stay as they are).

use eframe::egui::{Color32, Context};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

/// Status of a `cargo flash` run (own state so it never clashes with the DFU
/// detector or the OpenOCD/SWD flash status).
#[derive(Clone, PartialEq)]
pub enum ProbeFlashState {
    Idle,
    Flashing,
    Success,
    Error(String),
}

impl ProbeFlashState {
    pub fn is_busy(&self) -> bool {
        matches!(self, ProbeFlashState::Flashing)
    }
    pub fn label(&self) -> &str {
        match self {
            ProbeFlashState::Idle => "—",
            ProbeFlashState::Flashing => "Flashing (probe-rs)…",
            ProbeFlashState::Success => "probe-rs Flash OK",
            ProbeFlashState::Error(_) => "probe-rs Error",
        }
    }
    pub fn color(&self) -> Color32 {
        match self {
            ProbeFlashState::Idle => Color32::GRAY,
            ProbeFlashState::Success => Color32::from_rgb(80, 220, 100),
            ProbeFlashState::Error(_) => Color32::from_rgb(230, 80, 60),
            ProbeFlashState::Flashing => Color32::from_rgb(220, 180, 60),
        }
    }
}

/// Stop a running flash: kill the process TREE and mark the slot taken, so the
/// waiting thread reports it as stopped rather than as a failure to diagnose.
///
/// The tree, not the process: `cargo flash` is cargo, which spawns the build and
/// then the flashing work — killing only the parent leaves a child holding the
/// probe, and the next Flash fails to open it.
pub fn stop_probe_flash(child_slot: &Arc<Mutex<Option<u32>>>, log: &Arc<Mutex<Vec<String>>>) {
    let Some(pid) = child_slot.lock().unwrap().take() else {
        return; // already finished, or never started
    };
    log.lock()
        .unwrap()
        .push("> stopping flash (killing cargo flash)…".to_owned());
    crate::lsp::kill_process_tree(pid);
}

/// Spawn `cargo flash --release --chip <chip> --target <target> [--probe <sel>]`
/// and stream its output into `log`. `probe` is the exact `VID:PID[:Serial]`
/// selector shared with the other probe-rs tabs (None = let probe-rs pick).
#[allow(clippy::too_many_arguments)]
pub fn start_probe_flash(
    project_dir: PathBuf,
    target: String,
    chip: String,
    probe: Option<String>,
    state: Arc<Mutex<ProbeFlashState>>,
    log: Arc<Mutex<Vec<String>>>,
    // The running child, so the UI can stop it. `cargo flash` can sit for
    // minutes with nothing to show — waiting on an ambiguous probe, on a target
    // that won't halt — and without this the only way out was killing the IDE.
    child_slot: Arc<Mutex<Option<u32>>>,
    ctx: Context,
) {
    if state.lock().unwrap().is_busy() {
        return;
    }
    *state.lock().unwrap() = ProbeFlashState::Flashing;
    log.lock().unwrap().clear();
    ctx.request_repaint();

    thread::spawn(move || {
        let mut args: Vec<String> = vec![
            "flash".into(),
            "--release".into(),
            "--chip".into(),
            chip,
            "--target".into(),
            target,
        ];
        if let Some(p) = probe.filter(|s| !s.is_empty()) {
            args.push("--probe".into());
            args.push(p);
        }
        log.lock()
            .unwrap()
            .push(format!("> cargo {}", args.join(" ")));
        ctx.request_repaint();

        let mut command = Command::new("cargo");
        crate::build::no_window(&mut command);
        command
            .current_dir(&project_dir)
            .args(&args)
            // The log renders plain strings: without this, cargo's colours
            // arrive as ANSI escapes and read as garbage around every word.
            // Covers the build `cargo flash` runs internally too.
            .env("CARGO_TERM_COLOR", "never")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                *state.lock().unwrap() = ProbeFlashState::Error(format!(
                    "cannot run `cargo flash`: {e}\n\
                     Install it with: cargo install probe-rs-tools"
                ));
                ctx.request_repaint();
                return;
            }
        };

        // Publish the pid before reading a single line: a flash that hangs does
        // so at the very first step (opening the probe), which is exactly when
        // Stop has to work.
        *child_slot.lock().unwrap() = Some(child.id());

        // Stream stdout on a helper thread so we never block on a full stderr pipe.
        let out_log = Arc::clone(&log);
        let out_ctx = ctx.clone();
        let out_h = child.stdout.take().map(|o| {
            thread::spawn(move || {
                for line in BufReader::new(o).lines().map_while(Result::ok) {
                    out_log.lock().unwrap().push(line);
                    out_ctx.request_repaint();
                }
            })
        });
        if let Some(err) = child.stderr.take() {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                log.lock().unwrap().push(line);
                ctx.request_repaint();
            }
        }
        if let Some(h) = out_h {
            let _ = h.join();
        }

        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        // Whatever happened, nobody may kill this pid any more — the OS reuses
        // them, and a stale one aimed at a stranger is the worst kind of bug.
        let stopped = child_slot.lock().unwrap().take().is_none();
        *state.lock().unwrap() = if ok {
            ProbeFlashState::Success
        } else if stopped {
            // The slot was already emptied by `stop_probe_flash`: this is the
            // user's own doing, not a failure to explain.
            ProbeFlashState::Error("flash stopped".into())
        } else {
            // A probe that won't open (or a probe-rs crash) has an explanation
            // worth more than "see the log above" — the log is where the raw
            // cause is buried.
            let tail = {
                let l = log.lock().unwrap();
                let start = l.len().saturating_sub(60);
                l[start..].join("\n")
            };
            let explained = crate::failure_hint::probe_rs_panic(&tail)
                .map(|d| crate::failure_hint::probe_rs_panic_message(&d))
                .or_else(|| {
                    crate::failure_hint::probe_open_failure(&tail)
                        .map(|d| crate::failure_hint::probe_open_message(&d))
                });
            ProbeFlashState::Error(
                explained.unwrap_or_else(|| "cargo flash failed — see the log above".into()),
            )
        };
        ctx.request_repaint();
    });
}
