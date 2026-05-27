//! ESP32 programming via `espflash`.
//!
//! Workflow:
//!   1. `cargo build --release`  → ELF binary  (stderr streamed live)
//!   2. `espflash flash --chip <chip> --release`
//!      (stdout + stderr streamed live)
//!
//! Install espflash: `cargo install espflash`
//! Docs: https://github.com/esp-rs/espflash

use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub enum EspFlashState {
    #[default]
    Idle,
    /// Running `cargo build --release`
    Building,
    /// Running `espflash flash …`
    Flashing,
    /// espflash completed successfully
    Success,
    /// Any step failed; inner string is the user-readable error
    Error(String),
}

impl EspFlashState {
    /// True while any background operation is running.
    pub fn is_busy(&self) -> bool {
        matches!(self, EspFlashState::Building | EspFlashState::Flashing)
    }

    /// Short status label for the toolbar badge.
    pub fn status_label(&self) -> &str {
        match self {
            EspFlashState::Idle     => "—",
            EspFlashState::Building => "Building…",
            EspFlashState::Flashing => "Flashing (ESP)…",
            EspFlashState::Success  => "ESP Flash OK ✔",
            EspFlashState::Error(_) => "ESP Error",
        }
    }

    /// Color for the status label / badge.
    pub fn status_color(&self) -> eframe::egui::Color32 {
        use eframe::egui::Color32;
        match self {
            EspFlashState::Success  => Color32::from_rgb(80, 220, 100),
            EspFlashState::Error(_) => Color32::from_rgb(230, 80, 60),
            EspFlashState::Building
            | EspFlashState::Flashing => Color32::from_rgb(220, 180, 60),
            _                         => Color32::GRAY,
        }
    }
}

// ── Flash ─────────────────────────────────────────────────────────────────────

/// Spawn a background thread that:
///   1. Runs `cargo build --release` in `project_dir` (stderr streamed live)
///   2. Runs `espflash flash --chip <chip> --release`
///      (stdout + stderr streamed live concurrently)
///
/// `state` is updated at every phase so the UI can show progress.
/// `log` receives each output line as it arrives.
pub fn start_flash(
    project_dir: PathBuf,
    target:      String,
    chip:        String,
    state:       Arc<Mutex<EspFlashState>>,
    log:         Arc<Mutex<Vec<String>>>,
    ctx:         eframe::egui::Context,
) {
    if state.lock().unwrap().is_busy() {
        return;
    }
    *state.lock().unwrap() = EspFlashState::Building;
    log.lock().unwrap().clear();
    ctx.request_repaint();

    thread::spawn(move || {
        // ── Phase 1: cargo build --release ────────────────────────────────────
        push_log(
            &log,
            &ctx,
            &format!("▶ cargo build --release --target {target} …"),
        );

        let mut cargo_cmd = Command::new("cargo");
        cargo_cmd
            .current_dir(&project_dir)
            .args(["build", "--release", "--target", &target])
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        // Suppress console window on Windows
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cargo_cmd.creation_flags(0x0800_0000);
        }

        let mut child = match cargo_cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                set(
                    &state,
                    &ctx,
                    EspFlashState::Error(format!("Cannot run cargo: {e}")),
                );
                return;
            }
        };

        // Stream cargo stderr line by line
        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).lines() {
                if let Ok(line) = line {
                    push_log(&log, &ctx, &line);
                }
            }
        }

        match child.wait() {
            Err(e) => {
                set(
                    &state,
                    &ctx,
                    EspFlashState::Error(format!("Cannot run cargo: {e}")),
                );
                return;
            }
            Ok(s) if !s.success() => {
                set(
                    &state,
                    &ctx,
                    EspFlashState::Error(
                        "cargo build --release failed.\n\
                         Fix compilation errors before flashing.\n\
                         See the log for details."
                            .into(),
                    ),
                );
                return;
            }
            _ => {}
        }

        push_log(&log, &ctx, "✔ Build OK");

        // ── Phase 2: espflash flash ────────────────────────────────────────────
        set(&state, &ctx, EspFlashState::Flashing);

        push_log(
            &log,
            &ctx,
            &format!("▶ espflash flash --chip {chip} --release …"),
        );

        let mut esp_cmd = Command::new("espflash");
        esp_cmd
            .current_dir(&project_dir)
            .args(["flash", "--chip", &chip, "--release"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Suppress console window on Windows
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            esp_cmd.creation_flags(0x0800_0000);
        }

        let mut child = match esp_cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                set(
                    &state,
                    &ctx,
                    EspFlashState::Error(format!(
                        "Cannot run espflash: {e}\n\
                         Install: cargo install espflash\n\
                         Docs: https://github.com/esp-rs/espflash"
                    )),
                );
                return;
            }
        };

        // Stream espflash stdout in a helper thread
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

        // Stream espflash stderr in this thread
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
                EspFlashState::Error(format!("Cannot run espflash: {e}")),
            ),
            Ok(s) if !s.success() => set(
                &state,
                &ctx,
                EspFlashState::Error(
                    "espflash failed to program the device.\n\
                     Check:\n\
                     • espflash is installed  (cargo install espflash)\n\
                     • ESP32-C3 is connected via USB\n\
                     • ESP32-C3 is in download mode:\n\
                         hold BOOT button → press RESET → release BOOT\n\
                     • The correct COM/serial port is accessible\n\
                     • riscv32imc-unknown-none-elf target is installed:\n\
                         rustup target add riscv32imc-unknown-none-elf"
                        .into(),
                ),
            ),
            _ => {
                push_log(&log, &ctx, "✔ ESP32 flash complete!");
                set(&state, &ctx, EspFlashState::Success);
            }
        }
    });
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn push_log(log: &Arc<Mutex<Vec<String>>>, ctx: &eframe::egui::Context, line: &str) {
    log.lock().unwrap().push(line.to_string());
    ctx.request_repaint();
}

fn set(
    state: &Arc<Mutex<EspFlashState>>,
    ctx:   &eframe::egui::Context,
    next:  EspFlashState,
) {
    *state.lock().unwrap() = next;
    ctx.request_repaint();
}
