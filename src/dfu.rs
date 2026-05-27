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
///   2. Enumerates relevant USB programmer devices → populates `programmers`
///   3. Writes a human-readable summary to `dfu_log`
pub fn detect_dfu(
    state: Arc<Mutex<DfuState>>,
    dfu_log: Arc<Mutex<Vec<String>>>,
    programmers: Arc<Mutex<Vec<ProgrammerInfo>>>,
    ctx: eframe::egui::Context,
) {
    if state.lock().unwrap().is_busy() {
        return;
    }
    *state.lock().unwrap() = DfuState::Detecting;
    {
        let mut log = dfu_log.lock().unwrap();
        log.clear();
        log.push("▶ Scanning USB …".to_string());
    }
    ctx.request_repaint();

    thread::spawn(move || {
        // ── 1. DFU bootloader detection via dfu-util -l ───────────────────────
        let dfu_result = run_detect();

        // ── 2. Enumerate relevant USB programmer devices ───────────────────────
        let mut found = list_programmer_devices();

        // Make sure a DFU device is listed even if the OS doesn't show it
        // (some DFU drivers hide the device in WMI / lsusb).
        if let DfuState::DeviceFound(ref desc) = dfu_result {
            let already = found.iter().any(|p| p.kind == "DFU Bootloader");
            if !already {
                found.insert(0, ProgrammerInfo {
                    name:    desc.clone(),
                    vid_pid: "0483:df11".to_string(),
                    kind:    "DFU Bootloader",
                });
            }
        }
        *programmers.lock().unwrap() = found.clone();

        // ── 3. Build log text ─────────────────────────────────────────────────
        {
            let mut log = dfu_log.lock().unwrap();
            log.clear();

            // DFU-specific result
            match &dfu_result {
                DfuState::DeviceFound(desc) => {
                    log.push(format!("✔ DFU bootloader: {desc}"));
                }
                DfuState::NoDevice => {
                    log.push("  No DFU bootloader found.".to_string());
                    log.push("  To enter DFU mode on STM32:".to_string());
                    log.push("    1. Set BOOT0 = 1, press RESET".to_string());
                    log.push("    2. Device appears as VID 0483:DF11".to_string());
                }
                DfuState::Error(e) => {
                    log.push(format!("✗ dfu-util: {}", e.lines().next().unwrap_or(e)));
                    log.push("  Install: winget install dfu-util".to_string());
                    log.push("  WinUSB driver: install via Zadig for 'STM32 BOOTLOADER'".to_string());
                }
                _ => {}
            }

            log.push(String::new());

            // Programmer list
            if found.is_empty() {
                log.push("── No known programmer / serial adapter detected ─────".to_string());
            } else {
                log.push("── Detected programmers / adapters ──────────────────".to_string());
                for p in &found {
                    log.push(format!("  [{}]  {}  [{}]", p.kind, p.name, p.vid_pid));
                }
            }
        }
        ctx.request_repaint();

        *state.lock().unwrap() = dfu_result;
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
        // The Cargo.toml template names the binary "{pkg_name}-project"
        let bin_name = format!("{pkg_name}-project");
        let elf = project_dir
            .join("target")
            .join(&target)
            .join("release")
            .join(&bin_name);
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

// ── Programmer device catalogue ───────────────────────────────────────────────

/// A detected USB device relevant to embedded MCU programming.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgrammerInfo {
    /// Device name as reported by the OS (e.g. "STMicroelectronics ST-LINK/V2")
    pub name: String,
    /// Lowercase "vid:pid" (e.g. "0483:3748")
    pub vid_pid: String,
    /// Programmer family (e.g. "ST-Link", "J-Link", "DFU Bootloader")
    pub kind: &'static str,
}

impl ProgrammerInfo {
    /// One-line label for the ComboBox.
    pub fn combo_label(&self) -> String {
        format!("[{}]  {}  [{}]", self.kind, self.name, self.vid_pid)
    }

    /// Contextual guidance shown below the ComboBox.
    pub fn guidance(&self) -> &'static str {
        match self.kind {
            "DFU Bootloader" =>
                "DFU bootloader detected — click Flash USB to program.",
            "ST-Link" =>
                "ST-Link connected. DFU flashing needs the MCU in bootloader mode \
                 (BOOT0=1 + reset). For SWD: use OpenOCD.",
            "J-Link" =>
                "J-Link detected. Use J-Flash or OpenOCD for SWD/JTAG flashing.",
            "CMSIS-DAP" =>
                "CMSIS-DAP / DAPLink detected. Use pyOCD or OpenOCD for SWD flashing.",
            "USB-Serial" =>
                "USB-Serial adapter. STM32 UART boot: STM32CubeProgrammer. \
                 ESP32: esptool.py. Arduino: avrdude.",
            "ESP32" =>
                "ESP32 in USB-CDC mode. Flash with: esptool.py.",
            _ => "",
        }
    }
}

/// Known VID:PID → (display name, programmer kind).
/// Sorted roughly by prevalence; exact match checked before VID-only fallback.
const KNOWN_PROGRAMMERS: &[(&str, &str, &str)] = &[
    // ── DFU Bootloaders ───────────────────────────────────────────────────────
    ("0483:df11", "STM32 DFU Bootloader",     "DFU Bootloader"),
    ("303a:0002", "ESP32-S2 DFU Bootloader",  "DFU Bootloader"),
    ("1915:521f", "nRF52840 Dongle (DFU)",    "DFU Bootloader"),
    ("03eb:6124", "Atmel DFU Bootloader",     "DFU Bootloader"),
    ("239a:0035", "Adafruit Bootloader (DFU)","DFU Bootloader"),
    // ── ST-Link ───────────────────────────────────────────────────────────────
    ("0483:3744", "ST-Link v1",               "ST-Link"),
    ("0483:3748", "ST-Link v2",               "ST-Link"),
    ("0483:3749", "ST-Link v2-1",             "ST-Link"),
    ("0483:374a", "ST-Link v2-1 (MSD)",       "ST-Link"),
    ("0483:374b", "ST-Link v3E",              "ST-Link"),
    ("0483:374d", "ST-Link v3 (VCP)",         "ST-Link"),
    ("0483:374e", "ST-Link v3E (OB)",         "ST-Link"),
    ("0483:374f", "ST-Link v3S",              "ST-Link"),
    // ── CMSIS-DAP / DAPLink ───────────────────────────────────────────────────
    ("0d28:0204", "DAPLink / CMSIS-DAP",      "CMSIS-DAP"),
    ("2e8a:000c", "Picoprobe (RP2040)",        "CMSIS-DAP"),
    ("2e8a:0003", "Raspberry Pi Debug Probe", "CMSIS-DAP"),
    ("1fc9:0143", "NXP LPC-Link2",            "CMSIS-DAP"),
    ("1d50:6018", "Black Magic Probe",        "CMSIS-DAP"),
    ("1a86:8010", "WCH-Link (RISC-V)",        "CMSIS-DAP"),
    ("03eb:2111", "Atmel EDBG",               "CMSIS-DAP"),
    ("03eb:2140", "Atmel ICE",                "CMSIS-DAP"),
    ("03eb:2177", "Atmel EDBG (SAME)",        "CMSIS-DAP"),
    ("04d8:900a", "MPLAB ICD",                "CMSIS-DAP"),
    ("04d8:9012", "PICkit 3",                 "CMSIS-DAP"),
    ("04d8:8f0c", "MPLAB SNAP",               "CMSIS-DAP"),
    ("1915:9010", "nRF9160 DK / CMSIS-DAP",  "CMSIS-DAP"),
    // ── FTDI ──────────────────────────────────────────────────────────────────
    ("0403:6001", "FTDI FT232RL",             "USB-Serial"),
    ("0403:6010", "FTDI FT2232H",             "USB-Serial"),
    ("0403:6011", "FTDI FT4232H",             "USB-Serial"),
    ("0403:6014", "FTDI FT232H",              "USB-Serial"),
    ("0403:6015", "FTDI FT231X",              "USB-Serial"),
    // ── WCH CH340 / CH341 (Arduino, ESP8266, ESP32) ───────────────────────────
    ("1a86:7523", "CH340 USB-Serial",         "USB-Serial"),
    ("1a86:5523", "CH341A USB-Serial",        "USB-Serial"),
    ("1a86:55d4", "CH342/344 USB-Serial",     "USB-Serial"),
    ("1a86:7522", "CH340K USB-Serial",        "USB-Serial"),
    // ── Silicon Labs CP210x (ESP32, various) ──────────────────────────────────
    ("10c4:ea60", "CP2102 USB-Serial",        "USB-Serial"),
    ("10c4:ea61", "CP2103 USB-Serial",        "USB-Serial"),
    ("10c4:ea70", "CP2105 USB-Serial",        "USB-Serial"),
    ("10c4:ea71", "CP2108 USB-Serial",        "USB-Serial"),
    // ── Prolific PL2303 ───────────────────────────────────────────────────────
    ("067b:2303", "PL2303 USB-Serial",        "USB-Serial"),
    ("067b:23a3", "PL2303HXN USB-Serial",     "USB-Serial"),
    // ── ESP32 direct USB (S2 / S3 / C3 in CDC mode) ──────────────────────────
    ("303a:1001", "ESP32-S2 (USB-CDC)",       "ESP32"),
    ("303a:4001", "ESP32-S3 (USB-CDC)",       "ESP32"),
    ("303a:1002", "ESP32-C3 (USB-CDC)",       "ESP32"),
];

/// Look up VID:PID in the programmer catalogue.
/// Returns `Some((display_name, kind))` only for recognised embedded programmers.
fn find_programmer(vid_pid: &str) -> Option<(&'static str, &'static str)> {
    // 1. Exact VID:PID match
    for &(vp, name, kind) in KNOWN_PROGRAMMERS {
        if vid_pid.eq_ignore_ascii_case(vp) {
            return Some((name, kind));
        }
    }
    // 2. VID-only fallback for families with many PIDs
    let vid = vid_pid.split(':').next().unwrap_or("").to_lowercase();
    match vid.as_str() {
        "1366" => Some(("SEGGER J-Link", "J-Link")),
        "2341" => Some(("Arduino", "CMSIS-DAP")), // Arduino LLC
        "2a03" => Some(("Arduino (clone)", "CMSIS-DAP")),
        _ => None,
    }
}

// ── USB device enumeration (platform-specific) ────────────────────────────────

/// Enumerate connected USB devices and return only those relevant for MCU programming.
#[cfg(target_os = "windows")]
pub fn list_programmer_devices() -> Vec<ProgrammerInfo> {
    list_programmers_windows()
}

#[cfg(target_os = "linux")]
pub fn list_programmer_devices() -> Vec<ProgrammerInfo> {
    list_programmers_linux()
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn list_programmer_devices() -> Vec<ProgrammerInfo> {
    vec![]
}

/// Windows: enumerate via PowerShell + WMI, filter by KNOWN_PROGRAMMERS.
#[cfg(target_os = "windows")]
fn list_programmers_windows() -> Vec<ProgrammerInfo> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // Output one "OsName|vid:pid" line per USB device.
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

    let Ok(out) = out else { return vec![] };

    let mut devices: Vec<ProgrammerInfo> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() { return None; }
            let (os_name, vp) = line.split_once('|')?;
            let os_name = os_name.trim();
            let vp      = vp.trim();
            if os_name.is_empty() { return None; }
            let (catalogue_name, kind) = find_programmer(vp)?; // filter: only known programmers
            // Prefer the OS name when it's more descriptive than the catalogue name
            let name = if os_name.len() > catalogue_name.len() {
                os_name.to_string()
            } else {
                catalogue_name.to_string()
            };
            Some(ProgrammerInfo { name, vid_pid: vp.to_string(), kind })
        })
        .collect();

    devices.sort_by(|a, b| a.kind.cmp(b.kind).then(a.name.cmp(&b.name)));
    devices.dedup_by(|a, b| a.vid_pid == b.vid_pid);
    devices
}

/// Linux: enumerate via `lsusb`, filter by KNOWN_PROGRAMMERS.
#[cfg(target_os = "linux")]
fn list_programmers_linux() -> Vec<ProgrammerInfo> {
    let out = Command::new("lsusb")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return vec![] };

    let mut devices: Vec<ProgrammerInfo> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            // "Bus 001 Device 003: ID 0483:3748 STMicroelectronics ST-LINK/V2"
            let id_pos = line.find("ID ")?;
            let rest   = &line[id_pos + 3..];
            let sp     = rest.find(' ')?;
            let vp     = rest[..sp].trim();
            let os_name = rest[sp..].trim();
            let (catalogue_name, kind) = find_programmer(vp)?;
            let name = if os_name.len() > catalogue_name.len() {
                os_name.to_string()
            } else {
                catalogue_name.to_string()
            };
            Some(ProgrammerInfo { name, vid_pid: vp.to_string(), kind })
        })
        .collect();

    devices.sort_by(|a, b| a.kind.cmp(b.kind).then(a.name.cmp(&b.name)));
    devices
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
