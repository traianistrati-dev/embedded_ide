//! ESP serial console via the `espflash monitor` CLI (bottom-panel "Monitor"
//! tab) — where `esp_println::println!` output ends up after a flash.
//!
//! Why a subprocess and not [`crate::serial::SerialMonitor`], which already
//! speaks to serial ports:
//!
//! - **Boot output.** The interesting `println!`s happen in the first
//!   milliseconds of `main`. Connecting AFTER espflash reset the chip loses
//!   them — on a USB Serial/JTAG board the port re-enumerates and is gone for
//!   ~1–2 s. `espflash monitor` attaches FIRST and resets the target itself, so
//!   nothing before the first line is missed.
//! - **Panics.** Given `--elf`, it turns an ESP exception backtrace's raw
//!   addresses into function names and source lines.
//! - **USB Serial/JTAG.** It knows the reset dance for the native-USB peripheral
//!   (`--before usb-reset`), which a plain DTR/RTS toggle does not do.
//!
//! The Serial tab is still the right tool for TALKING to the device (TX, the
//! plotter, the frames view); the two must not hold the same port at once, which
//! the caller enforces.
//!
//! Structurally a twin of [`crate::rtt::RttConsole`] (subprocess → scrollback →
//! phase → killable child), and it reuses the Terminal tab's scrollback
//! machinery ([`TerminalState`], [`spawn_reader`]) for ANSI colours and
//! throttled repaints.

use crate::build::no_window;
use crate::terminal::{LineKind, TerminalState, spawn_reader};
use eframe::egui;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Line espflash prints naming the port it settled on, e.g.
/// `Serial port: 'COM7'`. Parsing it is what lets the monitor follow an
/// AUTO-DETECTED flash — the IDE never chose the port in that case, and
/// guessing a second time could land on a different board.
const PORT_LOG_PREFIX: &str = "Serial port:";

/// Extract the port from one espflash output line, or `None` when the line is
/// something else. Tolerates the quoting espflash uses (`'COM7'`) and any log
/// prefix (timestamp / level) ahead of the marker.
pub fn parse_port_line(line: &str) -> Option<String> {
    let idx = line.find(PORT_LOG_PREFIX)?;
    let rest = line[idx + PORT_LOG_PREFIX.len()..].trim();
    let port = rest.trim_matches(['\'', '"', ' ']).trim();
    // Strip a trailing ANSI reset the log line may carry.
    let port = port.split('\u{1b}').next().unwrap_or(port).trim();
    (!port.is_empty()).then(|| port.to_owned())
}

#[derive(Clone, Default, PartialEq)]
pub enum MonitorPhase {
    #[default]
    Idle,
    /// espflash is opening the port / resetting the target.
    Starting,
    /// Attached; device output is streaming into the scrollback.
    Streaming,
    /// The last session ended with an error (shown in the tab header row).
    Error(String),
}

/// The ESP monitor session (owned by `AppIde.esp_monitor`): scrollback, phase,
/// and the running `espflash monitor` child so Stop can kill it.
pub struct EspMonitor {
    pub state: Arc<Mutex<TerminalState>>,
    pub phase: Arc<Mutex<MonitorPhase>>,
    /// The port the live (or last) session used — shown in the tab so it is
    /// obvious WHICH board is being watched when several are attached.
    pub port: Arc<Mutex<String>>,
    /// Per-session stop flag (readers + orchestrator bail out).
    stop: Option<Arc<AtomicBool>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Default for EspMonitor {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(TerminalState::default())),
            phase: Arc::new(Mutex::new(MonitorPhase::Idle)),
            port: Arc::new(Mutex::new(String::new())),
            stop: None,
            child: Arc::new(Mutex::new(None)),
        }
    }
}

impl EspMonitor {
    pub fn phase(&self) -> MonitorPhase {
        self.phase.lock().unwrap().clone()
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.phase(),
            MonitorPhase::Starting | MonitorPhase::Streaming
        )
    }

    /// The port of the running session, or "" when idle.
    pub fn active_port(&self) -> String {
        if self.is_busy() {
            self.port.lock().unwrap().clone()
        } else {
            String::new()
        }
    }

    pub fn clear(&mut self) {
        self.state.lock().unwrap().lines.clear();
    }

    /// Kill `espflash monitor` and end the session. The port is released as the
    /// child dies, which is what frees it for the Serial tab.
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
        *self.phase.lock().unwrap() = MonitorPhase::Idle;
    }

    /// Attach to `port` (empty = let espflash auto-detect) and stream until
    /// stopped. `elf` is optional and only buys symbolised panic backtraces.
    /// No-op while a session is already running.
    ///
    /// `rescue_reset` belongs to the after-a-flash path: that flash ran with
    /// `--after no-reset` so this session could reset the chip itself and catch
    /// the boot output. If the session never gets that far, the board is left
    /// sitting in the ROM bootloader with the new firmware not running — which
    /// looks exactly like a dead board. Set it, and a failed attach is followed
    /// by `espflash reset` so the firmware starts regardless.
    pub fn start(
        &mut self,
        project_dir: PathBuf,
        chip: String,
        port: String,
        elf: Option<PathBuf>,
        rescue_reset: bool,
        ctx: egui::Context,
    ) {
        if self.is_busy() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(Arc::clone(&stop));
        *self.phase.lock().unwrap() = MonitorPhase::Starting;
        *self.port.lock().unwrap() = port.clone();

        let state = Arc::clone(&self.state);
        let phase = Arc::clone(&self.phase);
        let child_slot = Arc::clone(&self.child);
        thread::spawn(move || {
            let end = run_session(
                &project_dir,
                &chip,
                &port,
                elf.as_deref(),
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
                    Ok(()) => MonitorPhase::Idle,
                    Err(e) => {
                        state
                            .lock()
                            .unwrap()
                            .push_plain(LineKind::Notice, format!("[error] {e}"));
                        if rescue_reset {
                            rescue_reset_target(&project_dir, &port, &state);
                        }
                        MonitorPhase::Error(e)
                    }
                };
            }
            ctx.request_repaint();
        });
    }
}

/// Start the firmware after a failed post-flash attach: the flash deliberately
/// left the chip in the ROM bootloader for the monitor to reset, so without this
/// the board would look dead until it is unplugged. Best-effort — a failure here
/// is reported and nothing more, since the session already errored.
fn rescue_reset_target(
    project_dir: &std::path::Path,
    port: &str,
    state: &Arc<Mutex<TerminalState>>,
) {
    let mut args: Vec<String> = vec!["--skip-update-check".into(), "reset".into()];
    if !port.is_empty() {
        args.push("--port".into());
        args.push(port.into());
    }
    let mut cmd = Command::new("espflash");
    let out = no_window(&mut cmd)
        .current_dir(project_dir)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let mut st = state.lock().unwrap();
    match out {
        Ok(s) if s.success() => st.push_plain(
            LineKind::Notice,
            "[the monitor did not attach — reset the chip so the new firmware runs]",
        ),
        _ => st.push_plain(
            LineKind::Notice,
            "[the monitor did not attach AND the chip could not be reset — it is \
             still in the bootloader; unplug/replug the board or press its reset button]",
        ),
    }
}

/// The `espflash monitor` argument list.
///
/// `--non-interactive` is not optional: without it espflash may ask which port
/// to use, and there is no terminal to answer on — the session would hang on a
/// prompt nobody can see. An empty `port` lets espflash auto-detect. `elf` is
/// only passed when the file really exists, since a missing one makes espflash
/// exit instead of falling back to un-symbolised backtraces.
fn monitor_args(chip: &str, port: &str, elf: Option<&std::path::Path>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        // Global option, so it goes BEFORE the subcommand. Without it espflash
        // opens every session with a "new version available" banner, in the
        // middle of the device's own output.
        "--skip-update-check".into(),
        "monitor".into(),
        "--chip".into(),
        chip.into(),
        "--non-interactive".into(),
    ];
    if !port.is_empty() {
        args.push("--port".into());
        args.push(port.into());
    }
    if let Some(elf) = elf.filter(|p| p.exists()) {
        args.push("--elf".into());
        args.push(elf.display().to_string());
    }
    args
}

#[allow(clippy::too_many_arguments)]
fn run_session(
    project_dir: &std::path::Path,
    chip: &str,
    port: &str,
    elf: Option<&std::path::Path>,
    state: &Arc<Mutex<TerminalState>>,
    phase: &Arc<Mutex<MonitorPhase>>,
    child_slot: &Arc<Mutex<Option<Child>>>,
    stop: &Arc<AtomicBool>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let args = monitor_args(chip, port, elf);
    state
        .lock()
        .unwrap()
        .push_plain(LineKind::Input, format!("> espflash {}", args.join(" ")));
    ctx.request_repaint();

    let mut cmd = Command::new("espflash");
    let mut child = no_window(&mut cmd)
        .current_dir(project_dir)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "espflash not found in PATH.\n\
                 Install it with:  cargo install espflash"
                    .to_string()
            } else {
                format!("could not launch espflash: {e}")
            }
        })?;

    *phase.lock().unwrap() = MonitorPhase::Streaming;
    ctx.request_repaint();

    let done = Arc::new(AtomicUsize::new(0));
    let mut pipes = 0;
    if let Some(out) = child.stdout.take() {
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
    if let Some(err) = child.stderr.take() {
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
    *child_slot.lock().unwrap() = Some(child);

    // Wait until both pipes close (espflash exited) or the user stopped.
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
                .push_plain(LineKind::Notice, "[espflash monitor exited]");
            Ok(())
        }
        Some(Ok(st)) => Err(format!(
            "espflash monitor exited with {} — is the board still connected, \
             and is the port free? (the Serial tab holds it exclusively)",
            st.code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into())
        )),
        _ => Err("espflash monitor could not be reaped".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The port espflash chose is read back out of its log, which is the only
    /// way to follow an auto-detected flash.
    #[test]
    fn port_is_parsed_from_the_espflash_log() {
        assert_eq!(
            parse_port_line("Serial port: 'COM7'").as_deref(),
            Some("COM7")
        );
        assert_eq!(
            parse_port_line("[2026-08-12T10:00:00Z INFO ] Serial port: '/dev/ttyUSB0'").as_deref(),
            Some("/dev/ttyUSB0")
        );
        // Unquoted + trailing ANSI reset.
        assert_eq!(
            parse_port_line("Serial port: COM3\u{1b}[0m").as_deref(),
            Some("COM3")
        );
        // Unrelated lines and an empty value yield nothing.
        assert!(parse_port_line("Connecting...").is_none());
        assert!(parse_port_line("Chip type:  esp32c3").is_none());
        assert!(parse_port_line("Serial port: ''").is_none());
    }

    /// The command line is verified against the real espflash 4.4 CLI; the two
    /// things that silently break a session are a missing `--non-interactive`
    /// (it stops on a prompt nobody can see) and an `--elf` pointing at a file
    /// that is not there (espflash exits instead of skipping symbols).
    #[test]
    fn monitor_args_are_non_interactive_and_only_pass_an_elf_that_exists() {
        let args = monitor_args("esp32c3", "COM7", None);
        assert_eq!(
            args,
            vec![
                "--skip-update-check",
                "monitor",
                "--chip",
                "esp32c3",
                "--non-interactive",
                "--port",
                "COM7"
            ]
        );
        // The global flag must precede the subcommand or clap rejects it.
        assert_eq!(args[0], "--skip-update-check");
        assert_eq!(args[1], "monitor");

        // No port → espflash auto-detects; the flag must be absent, not empty.
        let auto = monitor_args("esp32c3", "", None);
        assert!(!auto.iter().any(|a| a == "--port"), "{auto:?}");
        assert!(auto.iter().any(|a| a == "--non-interactive"), "{auto:?}");

        // A missing ELF is dropped rather than passed through.
        let missing = monitor_args("esp32c3", "", Some(std::path::Path::new("no/such.elf")));
        assert!(!missing.iter().any(|a| a == "--elf"), "{missing:?}");

        // An existing one is passed (any real file will do — espflash only
        // needs the path to be there for the argument to be valid).
        let here = std::path::Path::new(file!());
        if here.exists() {
            let with_elf = monitor_args("esp32c3", "", Some(here));
            assert!(with_elf.iter().any(|a| a == "--elf"), "{with_elf:?}");
        }
    }
}
