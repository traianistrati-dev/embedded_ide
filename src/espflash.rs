//! ESP32 programming via `espflash`.
//!
//! Workflow:
//!   1. `cargo build --release`  → ELF binary  (stderr streamed live)
//!   2. `espflash flash --chip <chip> <elf_path>`
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
    /// Running `espflash board-info` (read-only chip identification)
    ReadingInfo,
    /// espflash completed successfully
    Success,
    /// Any step failed; inner string is the user-readable error
    Error(String),
}

impl EspFlashState {
    /// True while any background operation is running.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            EspFlashState::Building | EspFlashState::Flashing | EspFlashState::ReadingInfo
        )
    }

    /// Short status label for the toolbar badge.
    pub fn status_label(&self) -> &str {
        match self {
            EspFlashState::Idle        => "—",
            EspFlashState::Building    => "Building…",
            EspFlashState::Flashing    => "Flashing (ESP)…",
            EspFlashState::ReadingInfo => "Reading chip…",
            EspFlashState::Success     => "ESP Flash OK ✔",
            EspFlashState::Error(_)    => "ESP Error",
        }
    }

    /// Color for the status label / badge.
    pub fn status_color(&self) -> eframe::egui::Color32 {
        use eframe::egui::Color32;
        match self {
            EspFlashState::Success     => Color32::from_rgb(80, 220, 100),
            EspFlashState::Error(_)    => Color32::from_rgb(230, 80, 60),
            EspFlashState::ReadingInfo => Color32::from_rgb(100, 180, 255),
            EspFlashState::Building
            | EspFlashState::Flashing  => Color32::from_rgb(220, 180, 60),
            _                          => Color32::GRAY,
        }
    }
}

// ── Flash ─────────────────────────────────────────────────────────────────────

/// Spawn a background thread that:
///   1. Runs `cargo build --release` in `project_dir` (stderr streamed live)
///   2. Runs `espflash flash --chip <chip> <elf_path>`
///      where `<elf_path>` = `target/<target>/release/<chip>-project`
///      (stdout + stderr streamed live concurrently)
///
/// `state` is updated at every phase so the UI can show progress.
/// `log` receives each output line as it arrives.
pub fn start_flash(
    project_dir: PathBuf,
    target:      String,
    chip:        String,
    // Serial port override (e.g. "COM3").  Pass an empty string for auto-detect.
    port:        String,
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

        // Build the path to the ELF produced by cargo build --release.
        // Cargo names the binary after the [[bin]] name in Cargo.toml, which
        // our generator sets to "<chip>-project" (e.g. "esp32c3-project").
        let elf_path = project_dir
            .join("target")
            .join(&target)
            .join("release")
            .join(format!("{chip}-project"));

        // Full espflash command (shown in log for easy copy-paste / debugging):
        //   --ignore-app-descriptor  : esp-hal bare-metal ELFs have no ESP-IDF
        //                              app descriptor; espflash 4.x rejects them
        //                              without this flag.
        //   --after hard-reset       : explicitly reset the chip via the RTS line
        //                              after flashing so boards with a DTR/RTS
        //                              auto-reset circuit reboot automatically.
        //   --port <port>            : optional; empty string = auto-detect.
        let port_display = if port.is_empty() {
            "auto".to_owned()
        } else {
            port.clone()
        };
        push_log(
            &log,
            &ctx,
            &format!(
                "▶ espflash flash --chip {chip} --port {port_display} \
                 --ignore-app-descriptor --after hard-reset {} …",
                elf_path.display()
            ),
        );
        if port.is_empty() {
            push_log(&log, &ctx, "  (no port specified — espflash will auto-detect)");
        }

        let mut esp_cmd = Command::new("espflash");
        esp_cmd
            .current_dir(&project_dir)
            .args(["flash", "--chip", &chip]);
        if !port.is_empty() {
            esp_cmd.args(["--port", &port]);
        }
        esp_cmd
            .args(["--ignore-app-descriptor", "--after", "hard-reset"])
            .arg(&elf_path)
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
                     \n\
                     Check:\n\
                     • espflash is installed      cargo install espflash\n\
                     • ESP32-C3 is connected via USB (check Device Manager / dmesg)\n\
                     \n\
                     If espflash cannot connect, put the board in download mode manually:\n\
                       1. Hold the BOOT (IO0) button\n\
                       2. Press and release the RST button\n\
                       3. Release BOOT — board is now in bootloader mode\n\
                       4. Press Flash again in the IDE\n\
                     \n\
                     • riscv32imc-unknown-none-elf target must be installed:\n\
                         rustup target add riscv32imc-unknown-none-elf\n\
                     • The COM port must not be open by another program\n\
                         (close Serial Monitor, PuTTY, etc.)"
                        .into(),
                ),
            ),
            _ => {
                push_log(&log, &ctx, "✔ ESP32 flash complete!");
                push_log(&log, &ctx,
                    "  If the board does not start automatically → press the RST button.");
                push_log(&log, &ctx,
                    "  (Some SuperMini / DevKit boards ignore the USB auto-reset signal.)");
                set(&state, &ctx, EspFlashState::Success);
            }
        }
    });
}

// ── Chip identification ───────────────────────────────────────────────────────

/// Run `espflash board-info` — connects to the chip, prints type / MAC / flash
/// size, then disconnects **without writing anything to flash**.
///
/// Use this to verify the chip is connected and the USB-serial link works,
/// without risking a partial flash.
pub fn read_board_info(
    state: Arc<Mutex<EspFlashState>>,
    log:   Arc<Mutex<Vec<String>>>,
    ctx:   eframe::egui::Context,
) {
    if state.lock().unwrap().is_busy() {
        return;
    }
    *state.lock().unwrap() = EspFlashState::ReadingInfo;
    log.lock().unwrap().clear();
    ctx.request_repaint();

    thread::spawn(move || {
        push_log(&log, &ctx, "▶ espflash board-info …");
        push_log(&log, &ctx, "  (read-only — nothing is written to flash)");

        let mut cmd = Command::new("espflash");
        cmd.args(["board-info"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Suppress console window on Windows
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                set(
                    &state,
                    &ctx,
                    EspFlashState::Error(format!(
                        "Cannot run espflash: {e}\n\
                         Install: cargo install espflash"
                    )),
                );
                return;
            }
        };

        // Drain stdout in a helper thread
        let stdout_log = Arc::clone(&log);
        let stdout_ctx = ctx.clone();
        let stdout_handle = child.stdout.take().map(|stdout| {
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines().flatten() {
                    stdout_log.lock().unwrap().push(line);
                    stdout_ctx.request_repaint();
                }
            })
        });

        // Drain stderr in this thread
        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).lines().flatten() {
                push_log(&log, &ctx, &line);
            }
        }

        if let Some(h) = stdout_handle {
            let _ = h.join();
        }

        match child.wait() {
            Ok(s) if s.success() => {
                push_log(&log, &ctx, "✔ Chip info read OK.");
                set(&state, &ctx, EspFlashState::Idle);
            }
            _ => {
                set(
                    &state,
                    &ctx,
                    EspFlashState::Error(
                        "espflash board-info failed — chip not responding.\n\
                         \n\
                         Check:\n\
                         • USB cable is connected (try a different cable / port)\n\
                         • COM port is not open in another program\n\
                         \n\
                         If the board doesn't connect automatically, put it in\n\
                         download mode first:\n\
                           1. Hold the BOOT (IO0) button\n\
                           2. Press and release RST\n\
                           3. Release BOOT\n\
                         Then click Read Chip Info again."
                            .into(),
                    ),
                );
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
