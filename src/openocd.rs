//! SWD programming via OpenOCD.
//!
//! Workflow:
//!   1. `cargo build --release`  → ELF binary  (stderr streamed live)
//!   2. `openocd -f <interface_cfg> -f <target_cfg>
//!               -c "program <elf> verify reset exit"`
//!      (stdout + stderr streamed live)
//!
//! OpenOCD flashes the ELF directly — no objcopy / .bin conversion needed.

use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

// ── OpenOCD state ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub enum OpenOcdState {
    #[default]
    Idle,
    /// Running `cargo build --release`
    Building,
    /// Running `openocd` to program the device
    Flashing,
    /// openocd completed successfully
    Success,
    /// Any step failed; inner string is the user-readable error
    Error(String),
}

impl OpenOcdState {
    /// True while any background operation is running.
    pub fn is_busy(&self) -> bool {
        matches!(self, OpenOcdState::Building | OpenOcdState::Flashing)
    }

    /// Short status label for the toolbar badge.
    pub fn status_label(&self) -> &str {
        match self {
            OpenOcdState::Idle     => "—",
            OpenOcdState::Building => "Building…",
            OpenOcdState::Flashing => "Flashing (SWD)…",
            OpenOcdState::Success  => "SWD Flash OK ✔",
            OpenOcdState::Error(_) => "SWD Error",
        }
    }

    /// Color for the status label / badge.
    pub fn status_color(&self) -> eframe::egui::Color32 {
        use eframe::egui::Color32;
        match self {
            OpenOcdState::Success  => Color32::from_rgb(80, 220, 100),
            OpenOcdState::Error(_) => Color32::from_rgb(230, 80, 60),
            OpenOcdState::Building
            | OpenOcdState::Flashing => Color32::from_rgb(220, 180, 60),
            _                        => Color32::GRAY,
        }
    }
}

// ── Interface config mapping ──────────────────────────────────────────────────

/// Returns the OpenOCD interface config file path for the given programmer kind.
/// The returned string is ready to pass to `openocd -f <value>`.
pub fn interface_cfg_for_kind(kind: &str) -> &'static str {
    match kind {
        "ST-Link"   => "interface/stlink.cfg",
        "J-Link"    => "interface/jlink.cfg",
        "CMSIS-DAP" => "interface/cmsis-dap.cfg",
        _           => "interface/stlink.cfg", // safe default
    }
}

// ── Flash ─────────────────────────────────────────────────────────────────────

/// Spawn a background thread that:
///   1. Runs `cargo build --release` in `project_dir` (stderr streamed live)
///   2. Runs `openocd -f <interface_cfg> -f <target_cfg>
///                    -c "program <elf> verify reset exit"`
///      (stdout + stderr streamed live concurrently)
///
/// `state` is updated at every phase so the UI can show progress.
/// `log` receives each output line as it arrives (shared with the DFU tab's log area).
pub fn start_flash(
    project_dir: PathBuf,
    target: String,
    pkg_name: String,
    interface_cfg: String,
    target_cfg: String,
    state: Arc<Mutex<OpenOcdState>>,
    log: Arc<Mutex<Vec<String>>>,
    ctx: eframe::egui::Context,
) {
    if state.lock().unwrap().is_busy() {
        return;
    }
    *state.lock().unwrap() = OpenOcdState::Building;
    log.lock().unwrap().clear();
    ctx.request_repaint();

    thread::spawn(move || {
        // ── Phase 1: cargo build --release (with auto-clean retry) ───────────
        push_log(
            &log,
            &ctx,
            &format!("▶ cargo build --release --target {target} …"),
        );

        if !run_cargo_build(&project_dir, &target, &log, &ctx) {
            // If the failure is a stale device.x cache, auto-clean and retry once.
            let is_device_x = log
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("cannot find linker script device.x"));

            if is_device_x {
                push_log(
                    &log,
                    &ctx,
                    "⚠ Stale linker-script cache (device.x missing) — \
                     running `cargo clean` and retrying…",
                );
                cargo_clean(&project_dir);
                push_log(
                    &log,
                    &ctx,
                    &format!("▶ cargo build --release --target {target} … (retry)"),
                );
                if !run_cargo_build(&project_dir, &target, &log, &ctx) {
                    set(
                        &state,
                        &ctx,
                        OpenOcdState::Error(
                            "cargo build --release failed after `cargo clean`.\n\
                             Fix compilation errors before flashing.\n\
                             See the log for details."
                                .into(),
                        ),
                    );
                    return;
                }
            } else {
                set(
                    &state,
                    &ctx,
                    OpenOcdState::Error(
                        "cargo build --release failed.\n\
                         Fix compilation errors before flashing.\n\
                         See the log for details."
                            .into(),
                    ),
                );
                return;
            }
        }

        push_log(&log, &ctx, "✔ Build OK");

        // ── Phase 2: openocd flash ─────────────────────────────────────────────
        set(&state, &ctx, OpenOcdState::Flashing);

        // The Cargo.toml template names the binary "{pkg_name}-project"
        let bin_name = format!("{pkg_name}-project");
        let elf = project_dir
            .join("target")
            .join(&target)
            .join("release")
            .join(&bin_name);

        // OpenOCD requires forward slashes in paths, even on Windows.
        let elf_fwd = elf.to_string_lossy().replace('\\', "/");
        let program_cmd = format!("program {elf_fwd} verify reset exit");

        push_log(
            &log,
            &ctx,
            &format!(
                "▶ openocd -f {interface_cfg} -f {target_cfg} -c \"program … verify reset exit\""
            ),
        );

        let mut ocd_cmd = Command::new("openocd");
        ocd_cmd
            .args([
                "-f",
                &interface_cfg,
                "-f",
                &target_cfg,
                "-c",
                &program_cmd,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Suppress console window on Windows
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            ocd_cmd.creation_flags(0x0800_0000);
        }

        let mut child = match ocd_cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                set(
                    &state,
                    &ctx,
                    OpenOcdState::Error(format!(
                        "Cannot run openocd: {e}\n\
                         Install OpenOCD:\n\
                         • Windows: winget install openocd\n\
                         • Or download: https://openocd.org/pages/getting-openocd.html\n\
                         • Linux:   sudo apt install openocd"
                    )),
                );
                return;
            }
        };

        // Stream openocd stdout in a helper thread so we don't block on stderr
        let stdout_log = Arc::clone(&log);
        let stdout_ctx = ctx.clone();
        let stdout_handle = child.stdout.take().map(|stdout| {
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    if let Ok(line) = line {
                        stdout_log.lock().unwrap().push(line);
                        stdout_ctx.request_repaint();
                    }
                }
            })
        });

        // Stream openocd stderr in this thread (OpenOCD uses stderr for progress)
        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).lines() {
                if let Ok(line) = line {
                    push_log(&log, &ctx, &line);
                }
            }
        }

        if let Some(h) = stdout_handle {
            let _ = h.join();
        }

        match child.wait() {
            Err(e) => set(
                &state,
                &ctx,
                OpenOcdState::Error(format!("Cannot run openocd: {e}")),
            ),
            Ok(s) if !s.success() => set(
                &state,
                &ctx,
                OpenOcdState::Error(
                    "openocd failed to program the device.\n\
                     Check:\n\
                     • OpenOCD is installed and in PATH\n\
                     • ST-Link / J-Link driver is installed\n\
                     • Target config matches your MCU (e.g. stm32f1x.cfg for F103)\n\
                     • SWD wiring: SWDIO + SWCLK + GND  (+3.3V optional)\n\
                     • MCU is powered on"
                        .into(),
                ),
            ),
            _ => {
                push_log(&log, &ctx, "✔ SWD flash complete!");
                set(&state, &ctx, OpenOcdState::Success);
            }
        }
    });
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn push_log(log: &Arc<Mutex<Vec<String>>>, ctx: &eframe::egui::Context, line: &str) {
    log.lock().unwrap().push(line.to_string());
    ctx.request_repaint();
}

fn set(state: &Arc<Mutex<OpenOcdState>>, ctx: &eframe::egui::Context, next: OpenOcdState) {
    *state.lock().unwrap() = next;
    ctx.request_repaint();
}

/// Runs `cargo build --release --target <target>` in `project_dir`, streaming
/// each stderr line into `log`.  Returns `true` on success.
fn run_cargo_build(
    project_dir: &std::path::Path,
    target: &str,
    log: &Arc<Mutex<Vec<String>>>,
    ctx: &eframe::egui::Context,
) -> bool {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(project_dir)
        .args(["build", "--release", "--target", target])
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            push_log(log, ctx, &format!("Cannot run cargo: {e}"));
            return false;
        }
    };

    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines().flatten() {
            push_log(log, ctx, &line);
        }
    }

    matches!(child.wait(), Ok(s) if s.success())
}

/// Runs `cargo clean` in `project_dir` (blocking, output suppressed).
fn cargo_clean(project_dir: &std::path::Path) {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(project_dir)
        .args(["clean"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let _ = cmd.output();
}
