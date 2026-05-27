//! USB DFU flashing support for the Embedded IDE.
//!
//! Workflow:
//!   1. `detect_dfu(state, ctx)` — background thread: runs `dfu-util -l`,
//!      sets state to `DeviceFound` or `NoDevice`.
//!   2. `start_flash(...)` — background thread:
//!      a. `cargo build --release` → ELF binary
//!      b. `objcopy` (tries three tools)  → firmware.bin
//!      c. `dfu-util` → flashes the device

use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

// ── DFU state ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub enum DfuState {
    #[default]
    Idle,
    /// Scanning USB for a DFU device
    Detecting,
    /// `dfu-util -l` found a device; inner string is the description line
    DeviceFound(String),
    /// `dfu-util -l` completed but no DFU device was seen
    NoDevice,
    /// Running `cargo build --release`
    Building,
    /// Running `dfu-util` to program the device
    Flashing,
    /// Flash completed successfully
    Success,
    /// Any step failed; inner string is the user-readable error
    Error(String),
}

impl DfuState {
    /// True while any background operation is running.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            DfuState::Detecting | DfuState::Building | DfuState::Flashing
        )
    }

    /// Short status label for the toolbar.
    pub fn status_label(&self) -> &str {
        match self {
            DfuState::Idle           => "—",
            DfuState::Detecting      => "Scanning…",
            DfuState::DeviceFound(_) => "DFU device found",
            DfuState::NoDevice       => "No DFU device",
            DfuState::Building       => "Building…",
            DfuState::Flashing       => "Flashing…",
            DfuState::Success        => "Flash OK ✔",
            DfuState::Error(_)       => "Error",
        }
    }

    /// Color for the status label.
    pub fn status_color(&self) -> eframe::egui::Color32 {
        use eframe::egui::Color32;
        match self {
            DfuState::DeviceFound(_) => Color32::from_rgb(80, 220, 100),
            DfuState::Success        => Color32::from_rgb(80, 220, 100),
            DfuState::NoDevice       => Color32::from_rgb(180, 180, 180),
            DfuState::Error(_)       => Color32::from_rgb(230, 80, 60),
            DfuState::Building
            | DfuState::Flashing
            | DfuState::Detecting    => Color32::from_rgb(220, 180, 60),
            _                        => Color32::GRAY,
        }
    }

    /// Tooltip / detail text shown on hover.
    pub fn detail(&self) -> Option<String> {
        match self {
            DfuState::DeviceFound(desc) => Some(desc.clone()),
            DfuState::Error(msg)        => Some(msg.clone()),
            _                           => None,
        }
    }
}

// ── Detection ─────────────────────────────────────────────────────────────────

/// Spawn a long-lived background thread that periodically polls `dfu-util -l`
/// so the UI always reflects the current USB state without manual button clicks.
///
/// Poll interval:
/// - 2 s when no device is present (fast enough to catch plug-in events)
/// - 4 s when a device is already found (no point checking as often)
/// - skips the poll while any operation is in progress (`is_busy()`)
///
/// The first poll is delayed 3 s so it doesn't race with the explicit
/// `detect_dfu()` call made at startup.
pub fn start_usb_monitor(state: Arc<Mutex<DfuState>>, ctx: eframe::egui::Context) {
    thread::spawn(move || {
        // Let the startup detect_dfu() finish before we start competing.
        thread::sleep(std::time::Duration::from_secs(3));

        loop {
            // Skip while building / flashing / detecting
            let busy = state.lock().unwrap().is_busy();
            if !busy {
                let next = run_detect();
                let mut guard = state.lock().unwrap();
                // Only repaint when something actually changed
                if !guard.is_busy() && *guard != next {
                    *guard = next;
                    drop(guard);
                    ctx.request_repaint();
                }
            }

            // Slow down polling when we already know a device is present
            let device_present = matches!(*state.lock().unwrap(), DfuState::DeviceFound(_));
            thread::sleep(std::time::Duration::from_secs(if device_present { 4 } else { 2 }));
        }
    });
}

/// Spawn a background thread that:
///   1. Runs `dfu-util -l` and updates `state`
///   2. Lists ALL connected USB devices and appends them to `dfu_log`
///      (so the DFU tab shows ST-Link, J-Link, etc., even if not DFU capable)
pub fn detect_dfu(
    state: Arc<Mutex<DfuState>>,
    dfu_log: Arc<Mutex<Vec<String>>>,
    ctx: eframe::egui::Context,
) {
    if state.lock().unwrap().is_busy() {
        return; // already doing something
    }
    *state.lock().unwrap() = DfuState::Detecting;
    {
        let mut log = dfu_log.lock().unwrap();
        log.clear();
        log.push("▶ Scanning for DFU devices (dfu-util -l) …".to_string());
    }
    ctx.request_repaint();

    thread::spawn(move || {
        // ── DFU detection ─────────────────────────────────────────────────────
        let next = run_detect();

        {
            let mut log = dfu_log.lock().unwrap();
            match &next {
                DfuState::DeviceFound(desc) => {
                    log.push(format!("✔ {desc}"));
                }
                DfuState::NoDevice => {
                    log.push("  No DFU bootloader device found.".to_string());
                    log.push(String::new());
                    log.push(
                        "  ⓘ  DFU mode = STM32 MCU in bootloader (NOT ST-Link):".to_string(),
                    );
                    log.push("     1. Set BOOT0 jumper = 1".to_string());
                    log.push("     2. Reconnect USB or press RESET".to_string());
                    log.push("     3. Device appears as VID 0483 : PID DF11".to_string());
                }
                DfuState::Error(e) => {
                    log.push(format!("✗ {}", e.lines().next().unwrap_or(e)));
                    log.push(String::new());
                    log.push("  Install dfu-util:".to_string());
                    log.push("    winget install dfu-util".to_string());
                    log.push(
                        "  Then install WinUSB driver via Zadig for 'STM32 BOOTLOADER'."
                            .to_string(),
                    );
                }
                _ => {}
            }
        }
        ctx.request_repaint();

        // ── List ALL connected USB devices for context ─────────────────────────
        let usb_devs = list_connected_usb();
        {
            let mut log = dfu_log.lock().unwrap();
            log.push(String::new());
            if usb_devs.is_empty() {
                log.push("── No USB devices found ──────────────────────────".to_string());
            } else {
                log.push("── Connected USB devices ─────────────────────────".to_string());
                for dev in &usb_devs {
                    log.push(format!("  {dev}"));
                }
            }
        }
        ctx.request_repaint();

        *state.lock().unwrap() = next;
        ctx.request_repaint();
    });
}

fn run_detect() -> DfuState {
    let output = match Command::new("dfu-util")
        .arg("-l")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            return DfuState::Error(format!(
                "dfu-util not found: {e}\n\n\
                 Install dfu-util:\n\
                 • Windows: winget install dfu-util\n\
                 • Also install WinUSB driver via Zadig for the STM32 BOOTLOADER device."
            ))
        }
    };

    // dfu-util writes device list to both stdout and stderr depending on version
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    for line in combined.lines() {
        if line.trim_start().starts_with("Found DFU:") {
            return DfuState::DeviceFound(line.trim().to_string());
        }
    }

    DfuState::NoDevice
}

// ── Flash ─────────────────────────────────────────────────────────────────────

/// Spawn a background thread that:
///   1. Runs `cargo build --release` in `project_dir` (stderr streamed live)
///   2. Converts the ELF to a raw binary (`firmware.bin`)
///   3. Flashes the binary via `dfu-util` at `flash_addr` (stdout+stderr streamed live)
///
/// `state` is updated at every phase so the UI can show progress.
/// `dfu_log` receives each output line as it arrives for the DFU panel.
pub fn start_flash(
    project_dir: PathBuf,
    target: String,
    pkg_name: String,
    flash_addr: String,
    state: Arc<Mutex<DfuState>>,
    dfu_log: Arc<Mutex<Vec<String>>>,
    ctx: eframe::egui::Context,
) {
    if state.lock().unwrap().is_busy() {
        return;
    }
    *state.lock().unwrap() = DfuState::Building;
    dfu_log.lock().unwrap().clear();
    ctx.request_repaint();

    thread::spawn(move || {
        // ── Phase 1: cargo build --release ────────────────────────────────────
        push_log(&dfu_log, &ctx, "▶ cargo build --release …");

        let child = Command::new("cargo")
            .current_dir(&project_dir)
            .args(["build", "--release", "--target", &target])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                set(&state, &ctx, DfuState::Error(format!("Cannot run cargo: {e}")));
                return;
            }
        };

        // Stream cargo stderr line by line
        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).lines() {
                if let Ok(line) = line {
                    push_log(&dfu_log, &ctx, &line);
                }
            }
        }

        match child.wait() {
            Err(e) => {
                set(&state, &ctx, DfuState::Error(format!("Cannot run cargo: {e}")));
                return;
            }
            Ok(s) if !s.success() => {
                set(
                    &state,
                    &ctx,
                    DfuState::Error(
                        "cargo build --release failed.\n\
                         Fix compilation errors before flashing.\n\
                         See the DFU log for details."
                            .into(),
                    ),
                );
                return;
            }
            _ => {}
        }

        push_log(&dfu_log, &ctx, "✔ Build OK");

        // ── Phase 2: ELF → BIN ────────────────────────────────────────────────
        let elf = project_dir
            .join("target")
            .join(&target)
            .join("release")
            .join(&pkg_name);
        let bin = project_dir.join("firmware.bin");

        push_log(&dfu_log, &ctx, "▶ Converting ELF → BIN …");
        if let Err(e) = objcopy(&elf, &bin, &dfu_log, &ctx) {
            set(&state, &ctx, DfuState::Error(e));
            return;
        }
        push_log(&dfu_log, &ctx, "✔ firmware.bin ready");

        // ── Phase 3: dfu-util flash ───────────────────────────────────────────
        set(&state, &ctx, DfuState::Flashing);

        let addr_spec = format!("{flash_addr}:leave");
        push_log(
            &dfu_log,
            &ctx,
            &format!("▶ dfu-util -a 0 -s {addr_spec} -D firmware.bin"),
        );

        let child = Command::new("dfu-util")
            .args([
                "-a",
                "0",
                "-s",
                &addr_spec,
                "-D",
                bin.to_str().unwrap_or("firmware.bin"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                set(
                    &state,
                    &ctx,
                    DfuState::Error(format!("Cannot run dfu-util: {e}")),
                );
                return;
            }
        };

        // Stream dfu-util stdout in a helper thread so we don't block on stderr
        let stdout_log = Arc::clone(&dfu_log);
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

        // Stream dfu-util stderr in this thread
        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).lines() {
                if let Ok(line) = line {
                    push_log(&dfu_log, &ctx, &line);
                }
            }
        }

        if let Some(h) = stdout_handle {
            let _ = h.join();
        }

        match child.wait() {
            Err(e) => set(&state, &ctx, DfuState::Error(format!("Cannot run dfu-util: {e}"))),
            Ok(s) if !s.success() => set(
                &state,
                &ctx,
                DfuState::Error(
                    "dfu-util failed.\n\
                     Make sure:\n\
                     • BOOT0 jumper = 1 (or device is in DFU mode)\n\
                     • WinUSB driver installed via Zadig\n\
                     • USB cable is data-capable (not charge-only)"
                        .into(),
                ),
            ),
            _ => {
                push_log(&dfu_log, &ctx, "✔ Flash complete!");
                set(&state, &ctx, DfuState::Success);
            }
        }
    });
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Convert ELF → raw binary.  Tries three tools in order:
///   1. `llvm-objcopy`     (Rust's LLVM toolchain / system LLVM)
///   2. `arm-none-eabi-objcopy`  (ARM GNU toolchain)
///   3. `cargo objcopy`   (cargo-binutils)
fn objcopy(
    elf: &Path,
    bin: &Path,
    log: &Arc<Mutex<Vec<String>>>,
    ctx: &eframe::egui::Context,
) -> Result<(), String> {
    // Remove stale .bin so we can detect if a tool silently failed
    let _ = std::fs::remove_file(bin);

    let elf_s = elf.to_str().unwrap_or("");
    let bin_s = bin.to_str().unwrap_or("firmware.bin");

    // 1. llvm-objcopy (fastest, often available via rustup llvm-tools-preview)
    push_log(log, ctx, "  Trying llvm-objcopy …");
    if try_cmd("llvm-objcopy", &["-O", "binary", elf_s, bin_s], None) && bin.exists() {
        push_log(log, ctx, "  ✔ llvm-objcopy succeeded");
        return Ok(());
    }

    // 2. arm-none-eabi-objcopy (ARM GNU toolchain)
    push_log(log, ctx, "  Trying arm-none-eabi-objcopy …");
    if try_cmd("arm-none-eabi-objcopy", &["-O", "binary", elf_s, bin_s], None) && bin.exists() {
        push_log(log, ctx, "  ✔ arm-none-eabi-objcopy succeeded");
        return Ok(());
    }

    // 3. cargo objcopy (requires: cargo install cargo-binutils)
    push_log(log, ctx, "  Trying cargo objcopy …");
    if try_cmd(
        "cargo",
        &["objcopy", "--release", "--", "-O", "binary", bin_s],
        elf.parent(), // run from project dir so cargo finds Cargo.toml
    ) && bin.exists()
    {
        push_log(log, ctx, "  ✔ cargo objcopy succeeded");
        return Ok(());
    }

    Err(
        "Could not convert ELF → BIN. Install one of:\n\
         • llvm-objcopy  — rustup component add llvm-tools-preview\n\
         • arm-none-eabi-objcopy  — ARM GNU Toolchain\n\
         • cargo-binutils  — cargo install cargo-binutils"
            .into(),
    )
}

/// Run a command silently; returns true if exit code is 0.
fn try_cmd(program: &str, args: &[&str], cwd: Option<&Path>) -> bool {
    let mut cmd = Command::new(program);
    cmd.args(args).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

// ── USB device enumeration ────────────────────────────────────────────────────

/// Returns a human-readable list of all connected USB devices.
/// Used to explain to the user why a DFU device was not found
/// (e.g., ST-Link v2 is connected but is NOT a DFU bootloader device).
#[cfg(target_os = "windows")]
fn list_connected_usb() -> Vec<String> { list_usb_windows() }

#[cfg(target_os = "linux")]
fn list_connected_usb() -> Vec<String> { list_usb_linux() }

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn list_connected_usb() -> Vec<String> { vec![] }

/// Windows: enumerate USB devices via PowerShell + WMI.
/// Uses CREATE_NO_WINDOW so no console flashes in a GUI app.
#[cfg(target_os = "windows")]
fn list_usb_windows() -> Vec<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // PowerShell script: list VID-bearing USB devices, output "Name|vid:pid" per line.
    let ps = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Get-WmiObject Win32_PnPEntity |
  Where-Object { $_.DeviceID -like 'USB\VID*' } |
  ForEach-Object {
    if ($_.DeviceID -match 'VID_([0-9A-Fa-f]{4}).*PID_([0-9A-Fa-f]{4})') {
      $vp = $Matches[1].ToLower() + ':' + $Matches[2].ToLower()
      if ($_.Name) { "$($_.Name)|$vp" }
    }
  }
"#;

    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    let Ok(out) = out else {
        return vec![];
    };

    let mut devices: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (name, vp) = line.split_once('|')?;
            let name = name.trim();
            let vp = vp.trim();
            if name.is_empty() {
                return None;
            }
            let tag = classify_usb(vp);
            Some(format!("{tag}{name}  [{vp}]"))
        })
        .collect();

    devices.sort();
    devices.dedup();
    devices
}

/// Linux: enumerate USB devices via `lsusb`.
#[cfg(target_os = "linux")]
fn list_usb_linux() -> Vec<String> {
    let out = Command::new("lsusb")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else {
        return vec![];
    };
    let mut devices: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            // "Bus 001 Device 003: ID 0483:3748 STMicroelectronics ST-LINK/V2"
            let id_pos = line.find("ID ")?;
            let rest = &line[id_pos + 3..];
            let sp = rest.find(' ')?;
            let vp = &rest[..sp];
            let name = rest[sp..].trim();
            let tag = classify_usb(vp);
            Some(format!("{tag}{name}  [{vp}]"))
        })
        .collect();
    devices.sort();
    devices
}

/// Tag a device with its programmer type if recognised.
fn classify_usb(vid_pid: &str) -> &'static str {
    let vp = vid_pid.to_lowercase();
    if vp == "0483:df11"          { return "[DFU ⚡]  "; }
    if vp.starts_with("0483:374") { return "[ST-Link] "; }
    if vp.starts_with("1366:")    { return "[J-Link]  "; }
    if vp.starts_with("0d28:")    { return "[DAP]     "; }
    if vp.starts_with("03eb:")    { return "[Atmel]   "; }
    ""
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Append a line to the live DFU log and request a repaint.
fn push_log(log: &Arc<Mutex<Vec<String>>>, ctx: &eframe::egui::Context, line: &str) {
    log.lock().unwrap().push(line.to_string());
    ctx.request_repaint();
}

/// Update shared state and request a repaint.
fn set(state: &Arc<Mutex<DfuState>>, ctx: &eframe::egui::Context, next: DfuState) {
    *state.lock().unwrap() = next;
    ctx.request_repaint();
}
