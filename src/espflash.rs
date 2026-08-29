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
            EspFlashState::Idle => "—",
            EspFlashState::Building => "Building…",
            EspFlashState::Flashing => "Flashing (ESP)…",
            EspFlashState::ReadingInfo => "Reading chip…",
            EspFlashState::Success => "ESP Flash OK",
            EspFlashState::Error(_) => "ESP Error",
        }
    }

    /// Color for the status label / badge.
    pub fn status_color(&self) -> eframe::egui::Color32 {
        use eframe::egui::Color32;
        match self {
            EspFlashState::Success => Color32::from_rgb(80, 220, 100),
            EspFlashState::Error(_) => Color32::from_rgb(230, 80, 60),
            EspFlashState::ReadingInfo => Color32::from_rgb(100, 180, 255),
            EspFlashState::Building | EspFlashState::Flashing => Color32::from_rgb(220, 180, 60),
            _ => Color32::GRAY,
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
#[allow(clippy::too_many_arguments)]
pub fn start_flash(
    project_dir: PathBuf,
    target: String,
    chip: String,
    // Serial port override (e.g. "COM3").  Pass an empty string for auto-detect.
    port: String,
    // Where the port espflash actually used is written back — the same string on
    // an override, the auto-detected one otherwise (read out of espflash's own
    // log, see `esp_monitor::parse_port_line`). The Monitor session picks it up
    // so it watches the board that was just flashed, not another one.
    used_port: Arc<Mutex<String>>,
    // `true` when the ESP Monitor takes over right after: espflash then leaves
    // the chip in reset (`--after no-reset`) and the monitor resets it once it
    // is attached, so the first `println!` of `main` is not missed. `false`
    // keeps the standalone behaviour of resetting into the new firmware.
    monitor_follows: bool,
    state: Arc<Mutex<EspFlashState>>,
    log: Arc<Mutex<Vec<String>>>,
    ctx: eframe::egui::Context,
    activity: Arc<Mutex<crate::activity::ActivityLog>>,
) {
    if state.lock().unwrap().is_busy() {
        return;
    }
    *state.lock().unwrap() = EspFlashState::Building;
    log.lock().unwrap().clear();
    ctx.request_repaint();

    thread::spawn(move || {
        // Commits on drop, so a failed build / missing espflash still logs.
        let mut act = crate::activity::Committing::new("Flash (ESP / espflash)", activity);
        let t_build = std::time::Instant::now();
        // ── Phase 1: cargo build --release ────────────────────────────────────
        push_log(
            &log,
            &ctx,
            //&format!("▶ cargo build --release"),
            &format!("> cargo build --release --target {target} …"),
        );

        let mut cargo_cmd = Command::new("cargo");
        cargo_cmd
            .current_dir(&project_dir)
            // The Flash tab's log renders plain strings — colours would arrive
            // as ANSI escapes and read as garbage (see `terminal::strip_ansi`).
            .env("CARGO_TERM_COLOR", "never")
            //.args(["build", "--release"])
            .args([
                "build",
                "--release", /* , "--verbose"*/
                "--target",
                &target,
            ])
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

        // Stream cargo stderr line by line, and KEEP the one line that means
        // the build never started. The headline below used to blame the user's
        // code for any non-zero exit, and rustup's "custom toolchain 'esp' ...
        // is not installed" is not a compilation error. Only the three Xtensa
        // parts carry a `rust-toolchain.toml`, so that is where a machine
        // without espup lands - and it lands on advice to go hunting through
        // code that is fine.
        let mut toolchain_error: Option<String> = None;
        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).lines() {
                if let Ok(line) = line {
                    if line.contains("toolchain") && line.contains("is not installed") {
                        toolchain_error = Some(line.trim().to_owned());
                    }
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
                    EspFlashState::Error(match &toolchain_error {
                        // rustup's own line names the file, the toolchain and
                        // the problem, so it is worth more than anything
                        // written here.
                        Some(msg) => format!(
                            "{msg}

Install it with `espup install` - the Tools tab lists espup."
                        ),
                        None => "cargo build --release failed.
Fix compilation errors before flashing.
See the log for details."
                            .to_owned(),
                    }),
                );
                return;
            }
            _ => {}
        }

        push_log(&log, &ctx, "[OK] Build OK");
        act.rec().add("cargo build --release", t_build.elapsed());
        let t_flash = std::time::Instant::now();

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
        //   --ignore-app-descriptor  : belt and braces. It was needed when the
        //                              generator emitted no descriptor at all;
        //                              every generated main.rs now carries
        //                              `esp_bootloader_esp_idf::esp_app_desc!()`
        //                              (checked by `the_generated_main_carries_an_app_descriptor`),
        //                              so espflash's check would pass anyway.
        //                              Kept because it also covers a user who
        //                              deletes that line from their own main.rs.
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
                // "▶ espflash flash --chip {chip} --port {port_display} \
                // --ignore-app-descriptor --after hard-reset {} …",
                "> espflash flash --chip {chip} --port {} --ignore-app-descriptor {}-",
                port_display,
                elf_path.display()
            ),
        );
        if port.is_empty() {
            push_log(
                &log,
                &ctx,
                "  (no port specified — espflash will auto-detect)",
            );
        }

        // Seed the write-back with the override; an auto-detected run overwrites
        // it from espflash's log below.
        *used_port.lock().unwrap() = port.clone();

        let mut esp_cmd = Command::new("espflash");
        esp_cmd
            .current_dir(&project_dir)
            .args(["flash", "--chip", &chip]);
        if !port.is_empty() {
            esp_cmd.args(["--port", &port]);
        }
        // One reset, by whoever is going to watch the output (see
        // `monitor_follows`). Two resets would boot the firmware twice, and the
        // first boot's output has nobody listening.
        let after = if monitor_follows {
            "no-reset"
        } else {
            "hard-reset"
        };
        esp_cmd
            .args(["--ignore-app-descriptor", "--after", after])
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
        let stdout_port = Arc::clone(&used_port);
        let stdout_handle = child.stdout.take().map(|stdout| {
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    if let Ok(line) = line {
                        if let Some(p) = crate::esp_monitor::parse_port_line(&line) {
                            *stdout_port.lock().unwrap() = p;
                        }
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
                    // espflash logs through `env_logger`, i.e. to STDERR — this
                    // is where the port line usually shows up.
                    if let Some(p) = crate::esp_monitor::parse_port_line(&line) {
                        *used_port.lock().unwrap() = p;
                    }
                    push_log(&log, &ctx, &line);
                }
            }
        }

        if let Some(h) = stdout_handle {
            let _ = h.join();
        }

        let esp_status = child.wait();
        act.rec().cmd_phase(
            "espflash flash",
            format!("espflash flash --chip {chip}"),
            t_flash.elapsed(),
            esp_status.as_ref().ok().and_then(|s| s.code()),
        );

        match esp_status {
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
                push_log(&log, &ctx, "[OK] ESP32 flash complete!");
                push_log(
                    &log,
                    &ctx,
                    "  If the board does not start automatically -> press the RST button.",
                );
                push_log(
                    &log,
                    &ctx,
                    "  (Some SuperMini / DevKit boards ignore the USB auto-reset signal.)",
                );
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
    log: Arc<Mutex<Vec<String>>>,
    ctx: eframe::egui::Context,
    port: String,
) {
    if state.lock().unwrap().is_busy() {
        return;
    }
    *state.lock().unwrap() = EspFlashState::ReadingInfo;
    log.lock().unwrap().clear();
    ctx.request_repaint();

    thread::spawn(move || {
        push_log(&log, &ctx, "> espflash board-info …");
        push_log(&log, &ctx, "  (read-only — nothing is written to flash)");

        let mut cmd = Command::new("espflash");
        cmd.args(["board-info", "--port", &port])
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
                push_log(&log, &ctx, "[OK] Chip info read OK.");
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

fn set(state: &Arc<Mutex<EspFlashState>>, ctx: &eframe::egui::Context, next: EspFlashState) {
    *state.lock().unwrap() = next;
    ctx.request_repaint();
}

#[cfg(test)]
mod build_failure_tests {

    /// Every generated `main.rs` carries the ESP-IDF app descriptor.
    ///
    /// This is what makes `--ignore-app-descriptor` unnecessary. Both comments
    /// justifying that flag used to say the opposite - "esp-hal bare-metal ELFs
    /// have no ESP-IDF app descriptor" - and one of them is COPIED INTO the
    /// user's `.cargo/config.toml`, so the false claim shipped with every
    /// project. Measured here instead: nine chips, both runtimes.
    ///
    /// The flag is kept anyway, and the comment now says why: it also covers a
    /// user who deletes the line from their own main.rs.
    #[test]
    fn the_generated_main_carries_an_app_descriptor() {
        use crate::panels::mcu_module::builtins::builtin_definitions;
        use crate::panels::mcu_module::codegen::family::is_esp;
        use crate::panels::mcu_module::mcu::model::Runtime;

        let mut checked = 0;
        for d in builtin_definitions() {
            if !is_esp(&d.family) {
                continue;
            }
            for rt in [Runtime::Blocking, Runtime::Async] {
                let mut mcu = d.build_mcu();
                mcu.runtime = rt;
                assert!(
                    mcu.fresh_main_rs().contains("esp_app_desc!"),
                    "{} / {rt:?}: no app descriptor, so espflash needs the flag                      and the comments explaining it are right again",
                    d.id
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 18, "nine Espressif parts, two runtimes each");
    }

    /// The line that means the build never started, told apart from one that
    /// means the user's code is wrong.
    ///
    /// This mirrors the check in `start_flash`: a non-zero `cargo build` exit
    /// used to produce "Fix compilation errors before flashing" unconditionally,
    /// and on an esp32 / esp32s2 / esp32s3 without espup the real cause is
    /// rustup refusing a toolchain that is not there. There are no compilation
    /// errors to fix, and the user is sent through their own code looking for
    /// one. `profile.rs` had the same shape, from the other side: it read the
    /// same words as "cargo-bloat is missing".
    fn is_toolchain_error(line: &str) -> bool {
        line.contains("toolchain") && line.contains("is not installed")
    }

    #[test]
    fn a_missing_toolchain_is_not_a_compilation_error() {
        // Verbatim from a machine whose rust-toolchain.toml names an absent one.
        let rustup = "error: custom toolchain 'esp' specified in override file \
                      'C:\\p\\rust-toolchain.toml' is not installed";
        assert!(is_toolchain_error(rustup), "{rustup}");
    }

    /// And a real build failure still gets the old, correct headline.
    #[test]
    fn a_real_compile_error_is_still_a_compile_error() {
        for line in [
            "error[E0425]: cannot find value `foo` in this scope",
            "error: could not compile `esp32c6-project` (bin \"esp32c6-project\")",
            "error: linker `xtensa-esp32s3-elf-gcc` not found",
        ] {
            assert!(
                !is_toolchain_error(line),
                "`{line}` would be reported as a missing toolchain"
            );
        }
    }
}
