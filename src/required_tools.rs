//! Tool availability checker and one-click installer.
//!
//! Each [`RequiredTool`] entry knows how to verify and (where possible) install
//! a dependency on the host machine.  All blocking operations run in background
//! threads and write back through the shared [`Arc<Mutex<ToolsState>>`], then
//! call `ctx.request_repaint()` so the UI stays in sync without polling.
//!
//! # Tool catalog
//!
//! | Tool                        | Toolchain    | Auto-install | Platforms |
//! |-----------------------------|--------------|--------------|-----------|
//! | rustup                      | All          | No (manual)  | all       |
//! | rustc                       | All          | Yes          | all       |
//! | git                         | All          | No (manual)  | all       |
//! | cargo-bloat                 | All          | Yes (cargo)  | all       |
//! | host C toolchain            | All          | Win only     | all †     |
//! | serial port access          | All          | No (manual)  | Linux     |
//! | thumbv7m-none-eabi target   | RustEmbedded | Yes          | all       |
//! | probe-rs                    | RustEmbedded | Yes (cargo)  | all       |
//! | dfu-util                    | RustEmbedded | Win + macOS  | all       |
//! | openocd                     | RustEmbedded | Win + macOS  | all       |
//! | objcopy                     | RustEmbedded | Yes (cargo)  | all       |
//! | USB probe udev rules        | RustEmbedded | No (needs root) | Linux  |
//! | riscv32imc-unknown-none-elf | EspRust      | Yes          | all       |
//! | rust-src component          | EspRust      | Yes          | all       |
//! | espflash                    | EspRust      | Yes (cargo)  | all       |
//!
//! † The host C toolchain is a different beast per platform — MSVC on Windows
//! (file-probed, see [`MSVC_CHECK`]), `cc` from the Xcode Command Line Tools on
//! macOS, `cc` from build-essential on Linux — so it is ONE catalog entry whose
//! name, check and installer are chosen for the host.
//!
//! # Per-platform policy
//!
//! Auto-install is offered only where it can succeed **without root**: `cargo` /
//! `rustup` everywhere, `winget` on Windows, `brew` on macOS. Linux package
//! managers need root AND differ per distro (apt / dnf / pacman), so those
//! entries are deliberately manual — a button that always fails with a sudo
//! prompt the IDE can't answer is worse than a link that tells you the command.

use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use eframe::egui;
use std::{
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

// ── ToolStatus ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ToolStatus {
    #[default]
    Unknown,
    Checking,
    /// Tool found; inner string is the version reported by the tool.
    Ok(String),
    Missing,
    Installing,
    /// Last check or install failed; inner string is a short error message.
    Failed(String),
    /// Present, but older than the version this IDE needs. Deliberately NOT the
    /// same as missing: the tool may well still work, so it warns rather than
    /// disabling the features that use it.
    Outdated {
        found: String,
        min: &'static str,
    },
}

impl ToolStatus {
    pub fn is_busy(&self) -> bool {
        matches!(self, ToolStatus::Checking | ToolStatus::Installing)
    }

    pub fn label(&self) -> &str {
        match self {
            ToolStatus::Unknown => "—",
            ToolStatus::Checking => "Checking…",
            ToolStatus::Ok(_) => "OK",
            ToolStatus::Missing => "Missing",
            ToolStatus::Installing => "Installing…",
            ToolStatus::Failed(_) => "Failed",
            ToolStatus::Outdated { .. } => "Outdated",
        }
    }

    pub fn color(&self) -> egui::Color32 {
        match self {
            ToolStatus::Ok(_) => egui::Color32::from_rgb(80, 200, 100),
            ToolStatus::Missing => egui::Color32::from_rgb(230, 160, 50),
            ToolStatus::Failed(_) => egui::Color32::from_rgb(220, 70, 60),
            ToolStatus::Checking | ToolStatus::Installing => egui::Color32::from_rgb(180, 180, 80),
            ToolStatus::Outdated { .. } => egui::Color32::from_rgb(215, 165, 70),
            ToolStatus::Unknown => egui::Color32::GRAY,
        }
    }
}

// ── RequiredTool ───────────────────────────────────────────────────────────────

/// How badly a missing tool hurts — drives the startup banner (which only
/// reports [`Severity::Blocking`]) and the ordering in the Tools tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    /// Nothing builds without it.
    Blocking,
    /// One feature / tab stops working; the rest of the IDE is fine.
    Feature,
    /// Convenience only.
    Optional,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Blocking => "required",
            Severity::Feature => "feature",
            Severity::Optional => "optional",
        }
    }
    pub fn color(self) -> egui::Color32 {
        match self {
            Severity::Blocking => egui::Color32::from_rgb(230, 110, 100),
            Severity::Feature => egui::Color32::from_rgb(220, 180, 90),
            Severity::Optional => egui::Color32::from_gray(150),
        }
    }
}

pub struct RequiredTool {
    pub name: &'static str,
    pub description: &'static str,
    /// `None` = required for all toolchains.
    pub toolchain: Option<ToolchainKind>,
    /// How much breaks without it.
    pub severity: Severity,
    /// What the user LOSES when it's missing, in plain words — the answer to
    /// "why do I need this?" (shown in the Tools tab + the startup banner).
    pub impact: &'static str,
    // ── Check ────────────────────────────────────────────────────────────────
    pub check_cmd: &'static str,
    pub check_args: &'static [&'static str],
    /// If non-empty: stdout+stderr must contain this substring after a
    /// successful exit code for the tool to be considered present.
    /// Used for `rustup target list --installed` pattern checks.
    pub check_pattern: &'static str,
    /// Lowest version this IDE is known to need, e.g. `"1.74"`. `None` = don't
    /// version-check — the honest default: an invented minimum would nag users
    /// whose older build works fine. Only set it where the requirement is real
    /// and documented.
    pub min_version: Option<&'static str>,
    // ── Install ──────────────────────────────────────────────────────────────
    /// `None` = cannot be auto-installed; direct the user to `manual_url`.
    pub install_cmd: Option<&'static str>,
    pub install_args: &'static [&'static str],
    pub manual_url: &'static str,
    // ── Runtime state (mutated by background threads) ─────────────────────────
    pub status: ToolStatus,
}

// ── ToolsState ─────────────────────────────────────────────────────────────────

pub struct ToolsState {
    pub tools: Vec<RequiredTool>,
    pub log: Vec<String>,
}

impl ToolsState {
    fn push_log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
    }

    /// Same, for the Tools TAB — the udev "Generate rules…" action reports
    /// through the shared log rather than a dialog, so its output stays
    /// selectable for the rest of the session.
    pub fn push_log_public(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
    }

    pub fn any_busy(&self) -> bool {
        self.tools.iter().any(|t| t.status.is_busy())
    }

    /// Every tool that is confirmed broken (Missing / Failed / Outdated) —
    /// `Unknown` and the busy states don't count, so an unchecked catalog
    /// reports nothing. `toolchain` filters to the chip in use (`None` = don't
    /// filter). `Outdated` warns here but never disables a feature (see
    /// [`Self::unavailable`]).
    pub fn problems(
        &self,
        toolchain: Option<&ToolchainKind>,
    ) -> Vec<(&'static str, Severity, &'static str)> {
        self.tools
            .iter()
            .filter(|t| {
                matches!(
                    t.status,
                    ToolStatus::Missing | ToolStatus::Failed(_) | ToolStatus::Outdated { .. }
                )
            })
            .filter(|t| match (&t.toolchain, toolchain) {
                (Some(tc), Some(sel)) => tc == sel,
                (Some(_), None) => false, // toolchain-specific, no chip selected
                (None, _) => true,        // needed by everything
            })
            .map(|t| (t.name, t.severity, t.impact))
            .collect()
    }

    /// Blocking problems only — what the startup banner reports.
    pub fn blocking_problems(
        &self,
        toolchain: Option<&ToolchainKind>,
    ) -> Vec<(&'static str, Severity, &'static str)> {
        self.problems(toolchain)
            .into_iter()
            .filter(|(_, s, _)| *s == Severity::Blocking)
            .collect()
    }

    /// Names of the tools CONFIRMED unusable (`Missing` / `Failed`). Deliberately
    /// excludes `Unknown` and the busy states: a feature must never be greyed out
    /// just because the check hasn't run (or couldn't run) yet — the UI stays
    /// permissive until there is proof of a problem. Used to gate the buttons
    /// that shell out to that tool.
    pub fn unavailable(&self) -> Vec<&'static str> {
        self.tools
            .iter()
            .filter(|t| matches!(t.status, ToolStatus::Missing | ToolStatus::Failed(_)))
            .map(|t| t.name)
            .collect()
    }

    /// Count tools that are Missing or Failed AND have an auto-installer.
    pub fn missing_installable_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|t| {
                matches!(t.status, ToolStatus::Missing | ToolStatus::Failed(_))
                    && t.install_cmd.is_some()
            })
            .count()
    }
}

// ── ToolRow – lock-free snapshot for rendering ─────────────────────────────────

/// A cheap snapshot of one tool row used for lock-free egui rendering.
pub struct ToolRow {
    pub name: &'static str,
    pub description: &'static str,
    pub toolchain: Option<ToolchainKind>,
    pub severity: Severity,
    pub impact: &'static str,
    pub status: ToolStatus,
    pub can_auto_install: bool,
    pub manual_url: &'static str,
}

impl ToolsState {
    pub fn snapshot(&self) -> Vec<ToolRow> {
        self.tools
            .iter()
            .map(|t| ToolRow {
                name: t.name,
                description: t.description,
                toolchain: t.toolchain.clone(),
                severity: t.severity,
                impact: t.impact,
                status: t.status.clone(),
                can_auto_install: t.install_cmd.is_some(),
                manual_url: t.manual_url,
            })
            .collect()
    }
}

// ── Per-platform selection ─────────────────────────────────────────────────────

/// Catalog name of the Linux udev-rules entry. A constant because the Tools tab
/// matches on it to offer "Generate rules…" — a typo there would silently drop
/// the only action that entry has.
pub const UDEV_RULES_TOOL: &str = "USB probe udev rules";

/// Pick the value for the host OS. Everything that is not Windows or macOS is
/// treated as Linux — the other unixes this could run on use the same package
/// managers and the same udev/serial-group story.
///
/// A plain runtime `if` rather than `#[cfg]` on every field: the catalog is
/// built once at startup, and keeping all three variants **visible in one place**
/// is what stops a platform from quietly losing an entry.
fn per_os<T>(windows: T, macos: T, linux: T) -> T {
    if cfg!(target_os = "windows") {
        windows
    } else if cfg!(target_os = "macos") {
        macos
    } else {
        linux
    }
}

// ── Tool catalog ───────────────────────────────────────────────────────────────

pub fn make_tools_state() -> Arc<Mutex<ToolsState>> {
    #[allow(unused_mut)]
    let mut tools = vec![
        // ── Common to all toolchains ─────────────────────────────────────
        RequiredTool {
            name: "rustup",
            description: "Rust toolchain installer — manages Rust versions and targets",
            toolchain: None,
            severity: Severity::Blocking,
            impact: "No Rust toolchain management: targets can't be installed and nothing builds.",
            check_cmd: "rustup",
            check_args: &["--version"],
            check_pattern: "",
            min_version: None,
            install_cmd: None, // must be installed manually from rustup.rs
            install_args: &[],
            manual_url: "https://rustup.rs",
            status: ToolStatus::Unknown,
        },
        RequiredTool {
            name: "rustc",
            description: "Rust compiler (stable toolchain)",
            toolchain: None,
            severity: Severity::Blocking,
            impact: "No Rust compiler: Build, Check, Clippy and Flash all fail.",
            check_cmd: "rustc",
            check_args: &["--version"],
            check_pattern: "",
            min_version: Some("1.74"), // Cargo `[lints]` table (strict-lints)
            install_cmd: Some("rustup"),
            install_args: &["install", "stable"],
            manual_url: "https://www.rust-lang.org/tools/install",
            status: ToolStatus::Unknown,
        },
        RequiredTool {
            name: "git",
            description: "Version control — powers the Git tab (commit / push / pull)",
            toolchain: None,
            severity: Severity::Feature,
            impact: "The Git tab (commit / push / pull) and library cloning are unavailable.",
            check_cmd: "git",
            check_args: &["--version"],
            check_pattern: "",
            min_version: None,
            install_cmd: None, // installed manually from git-scm.com
            install_args: &[],
            manual_url: "https://git-scm.com",
            status: ToolStatus::Unknown,
        },
        RequiredTool {
            name: "cargo-bloat",
            description: "Code-size profiler — powers the Profile tab (.text/Flash per function)",
            toolchain: None,
            severity: Severity::Feature,
            impact: "The Profile tab's Static (size) view can't run; the rest of the IDE is fine.",
            check_cmd: "cargo",
            check_args: &["bloat", "--version"],
            check_pattern: "",
            min_version: None,
            install_cmd: Some("cargo"),
            install_args: &["install", "cargo-bloat"],
            manual_url: "https://github.com/RazrFalcon/cargo-bloat",
            status: ToolStatus::Unknown,
        },
        // ── RustEmbedded (STM32 / ARM Cortex-M) ─────────────────────────
        RequiredTool {
            name: "thumbv7m-none-eabi",
            description: "Rust target for ARM Cortex-M3 (STM32F1xx bare-metal)",
            toolchain: Some(ToolchainKind::RustEmbedded),
            severity: Severity::Blocking,
            impact: "This STM32 chip cannot be compiled at all until the target is installed.",
            check_cmd: "rustup",
            check_args: &["target", "list", "--installed"],
            check_pattern: "thumbv7m-none-eabi",
            min_version: None,
            install_cmd: Some("rustup"),
            install_args: &["target", "add", "thumbv7m-none-eabi"],
            manual_url: "https://docs.rust-embedded.org/book/intro/install.html",
            status: ToolStatus::Unknown,
        },
        RequiredTool {
            name: "probe-rs",
            description: "Debug probe runner — powers the Debug + RTT tabs (breakpoints, defmt logs) and SWD/JTAG flashing",
            toolchain: Some(ToolchainKind::RustEmbedded),
            severity: Severity::Feature,
            // The version advice is WINDOWS-ONLY: both known-bad releases fail on
            // Windows driver binding (WinUSB), which has no equivalent elsewhere.
            // Repeating it on Linux/macOS would pin those users to an old release
            // for a bug they cannot hit.
            impact: per_os(
                "No RTT logs, on-target Debug, runtime flamegraph or probe-rs flashing. \
                 NEWER IS NOT ALWAYS BETTER here: 0.31.0 panics inside its own USB probe \
                 enumeration, and 0.32.0 can't open an ST-Link bound to the WinUSB driver \
                 (\"reset not supported by WinUSB\"). 0.29.0 is the version verified to work \
                 on such a setup: cargo install probe-rs-tools --locked --version 0.29.0",
                "No RTT logs, on-target Debug, runtime flamegraph or probe-rs flashing.",
                "No RTT logs, on-target Debug, runtime flamegraph or probe-rs flashing. \
                 If it reports no probe while one is plugged in, the tool is fine — \
                 see \"USB probe udev rules\" below.",
            ),
            check_cmd: "probe-rs",
            check_args: &["--version"],
            check_pattern: "",
            // Deliberately NONE. A minimum here would mark a WORKING install
            // (0.29.0) "Outdated" and offer an upgrade that breaks debugging on
            // a WinUSB-bound ST-Link — the failures are version RANGES, not a
            // floor. `failure_hint`'s [PROBE_RS_PANIC] / [PROBE_OPEN_FAILED]
            // name the actual problem when it happens, which is the honest way
            // to handle "some releases are broken for some hardware".
            min_version: None,
            install_cmd: Some("cargo"),
            install_args: &["install", "probe-rs-tools", "--locked"],
            manual_url: "https://probe.rs/docs/getting-started/installation/",
            status: ToolStatus::Unknown,
        },
        RequiredTool {
            name: "dfu-util",
            description: "USB DFU flasher — programs an STM32 held in its ROM bootloader",
            toolchain: Some(ToolchainKind::RustEmbedded),
            severity: Severity::Feature,
            impact: "The Flash tab's DFU (USB bootloader) path can't run; SWD flashing via \
                     probe-rs is unaffected.",
            check_cmd: "dfu-util",
            check_args: &["--version"],
            check_pattern: "",
            min_version: None,
            install_cmd: per_os(Some("winget"), Some("brew"), None),
            install_args: per_os(
                &[
                    "install",
                    "--id",
                    "dfu-util.dfu-util",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                ][..],
                &["install", "dfu-util"][..],
                &[][..],
            ),
            manual_url: "https://dfu-util.sourceforge.net/",
            status: ToolStatus::Unknown,
        },
        RequiredTool {
            name: "openocd",
            description: "On-chip debugger — the alternative SWD/JTAG flash path",
            toolchain: Some(ToolchainKind::RustEmbedded),
            severity: Severity::Feature,
            impact: "The Flash tab's OpenOCD path can't run; probe-rs flashing is unaffected.",
            check_cmd: "openocd",
            check_args: &["--version"],
            check_pattern: "",
            min_version: None,
            install_cmd: per_os(Some("winget"), Some("brew"), None),
            install_args: per_os(
                &[
                    "install",
                    "--id",
                    "OpenOCD.OpenOCD",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                ][..],
                &["install", "open-ocd"][..],
                &[][..],
            ),
            manual_url: "https://openocd.org/pages/getting-openocd.html",
            status: ToolStatus::Unknown,
        },
        RequiredTool {
            name: "objcopy",
            description: "ELF -> raw binary converter - the step between `cargo build` and a DFU flash",
            toolchain: Some(ToolchainKind::RustEmbedded),
            severity: Severity::Feature,
            impact: "DFU flashing stops right after the build: firmware.bin can't be produced. \
                     Any ONE of llvm-objcopy, arm-none-eabi-objcopy or `cargo objcopy` is enough \
                     (`cargo objcopy` also needs `rustup component add llvm-tools`).",
            // Sentinel: the flash path tries three different binaries in order,
            // so a single-command check would report Missing whenever the user
            // happens to have one of the other two. See `OBJCOPY_CHECK`.
            check_cmd: OBJCOPY_CHECK,
            check_args: &[],
            check_pattern: "",
            min_version: None,
            // cargo-binutils provides fallback #3 and needs no root anywhere.
            install_cmd: Some("cargo"),
            install_args: &["install", "cargo-binutils"],
            manual_url: "https://github.com/rust-embedded/cargo-binutils",
            status: ToolStatus::Unknown,
        },
        // ── EspRust (ESP32-C3 / RISC-V) ──────────────────────────────────
        RequiredTool {
            name: "riscv32imc-unknown-none-elf",
            description: "Rust target for RISC-V ESP32-C3 bare-metal",
            toolchain: Some(ToolchainKind::EspRust),
            severity: Severity::Blocking,
            impact: "This ESP32-C3 chip cannot be compiled at all until the target is installed.",
            check_cmd: "rustup",
            check_args: &["target", "list", "--installed"],
            check_pattern: "riscv32imc-unknown-none-elf",
            min_version: None,
            install_cmd: Some("rustup"),
            install_args: &["target", "add", "riscv32imc-unknown-none-elf"],
            manual_url: "https://esp-rs.github.io/book/installation/riscv.html",
            status: ToolStatus::Unknown,
        },
        RequiredTool {
            name: "rust-src",
            description: "Rust source component — required by build-std for ESP32-C3",
            toolchain: Some(ToolchainKind::EspRust),
            severity: Severity::Blocking,
            impact: "ESP32-C3 builds fail: build-std needs the Rust source component.",
            check_cmd: "rustup",
            check_args: &["component", "list", "--installed"],
            check_pattern: "rust-src",
            min_version: None,
            install_cmd: Some("rustup"),
            install_args: &["component", "add", "rust-src"],
            manual_url: "https://esp-rs.github.io/book/installation/riscv.html",
            status: ToolStatus::Unknown,
        },
        RequiredTool {
            name: "espflash",
            description: "ESP32 USB flash tool — programs the chip over the built-in USB serial",
            toolchain: Some(ToolchainKind::EspRust),
            severity: Severity::Feature,
            impact: "The ESP32 cannot be flashed from the Flash tab.",
            check_cmd: "espflash",
            check_args: &["--version"],
            check_pattern: "",
            min_version: None,
            install_cmd: Some("cargo"),
            install_args: &["install", "espflash"],
            manual_url: "https://github.com/esp-rs/espflash",
            status: ToolStatus::Unknown,
        },
    ];

    // ── Host C toolchain ─────────────────────────────────────────────────────
    // Not an embedded tool: Rust links every build-script / proc-macro for the
    // HOST, so without a working C toolchain NOTHING builds — on any platform.
    // Only the shape differs. Windows is probed BY FILE, not by binary presence:
    // a half-installed Visual Studio has `cl.exe` but no libs/headers, which is
    // exactly the failure that must be caught (see `MSVC_CHECK`). The unixes just
    // need `cc` to answer.
    tools.push(RequiredTool {
        name: per_os("MSVC build tools", "Xcode Command Line Tools", "C build tools"),
        description: per_os(
            "Microsoft C++ x64 toolchain (msvcrt.lib + headers) — required to link Rust build-scripts on Windows",
            "Apple clang + linker — required to link Rust build-scripts on macOS",
            "gcc + ld (build-essential) — required to link Rust build-scripts on Linux",
        ),
        toolchain: None,
        severity: Severity::Blocking,
        impact: per_os(
            "NOTHING builds: every build-script fails to link (LNK1104 msvcrt.lib / C1083 vcruntime.h).",
            "NOTHING builds: every build-script fails to link (no linker / missing SDK headers). \
             Run `xcode-select --install`.",
            "NOTHING builds: every build-script fails to link (`cc` not found). \
             Debian/Ubuntu: build-essential · Fedora: @development-tools · Arch: base-devel.",
        ),
        check_cmd: per_os(MSVC_CHECK, "cc", "cc"),
        check_args: per_os(&[][..], &["--version"][..], &["--version"][..]),
        check_pattern: "",
        min_version: None,
        // macOS: `xcode-select --install` opens a GUI installer and returns
        // immediately, so it is NOT an auto-install we can report on — manual.
        // Linux: needs root and the package name differs per distro — manual.
        install_cmd: per_os(Some("winget"), None, None),
        install_args: per_os(
            &[
                "install",
                "--id",
                "Microsoft.VisualStudio.2022.BuildTools",
                "--accept-package-agreements",
                "--accept-source-agreements",
                "--override",
                "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended",
            ][..],
            &[][..],
            &[][..],
        ),
        manual_url: per_os(
            "https://visualstudio.microsoft.com/visual-cpp-build-tools/",
            "https://developer.apple.com/xcode/resources/",
            "https://doc.rust-lang.org/book/ch01-01-installation.html#installing-rustup-on-linux-or-macos",
        ),
        status: ToolStatus::Unknown,
    });

    // ── Serial-tab Bridge (MITM) prerequisite ────────────────────────────────
    // A virtual serial pair, which is a completely different kind of thing per
    // platform: a spawnable CLI on unix, a kernel driver on Windows. Hence one
    // entry whose name, probe and installer are all per-OS.
    tools.push(RequiredTool {
        name: per_os("com0com", "socat", "socat"),
        description: per_os(
            "Virtual serial-port pair driver — required by the Serial tab's Bridge (MITM) mode",
            "Multipurpose relay — the Serial tab's Bridge (MITM) mode uses it to create a PTY pair",
            "Multipurpose relay — the Serial tab's Bridge (MITM) mode uses it to create a PTY pair",
        ),
        toolchain: None,
        severity: Severity::Optional,
        impact: per_os(
            "The Serial tab's Bridge (MITM) mode can't run: there is no way to make a virtual \
             port pair. Everything else in the Serial tab is unaffected.",
            "The Serial tab's Bridge (MITM) mode can't create its PTY pair. Everything else in \
             the Serial tab is unaffected.",
            "The Serial tab's Bridge (MITM) mode can't create its PTY pair. Everything else in \
             the Serial tab is unaffected.",
        ),
        // com0com is a driver with no CLI on PATH, so it is probed by registry
        // key rather than by running anything — see `COM0COM_CHECK`.
        check_cmd: per_os(COM0COM_CHECK, "socat", "socat"),
        check_args: per_os(&[][..], &["-V"][..], &["-V"][..]),
        check_pattern: "",
        min_version: None,
        install_cmd: per_os(None, Some("brew"), None),
        install_args: per_os(&[][..], &["install", "socat"][..], &[][..]),
        manual_url: per_os(
            "https://com0com.sourceforge.net/",
            "http://www.dest-unreach.org/socat/",
            "http://www.dest-unreach.org/socat/",
        ),
        status: ToolStatus::Unknown,
    });

    // ── Linux-only: access to the hardware ───────────────────────────────────
    // Neither of these is a program to install — they are PERMISSIONS, and they
    // are the number-one reason flashing "doesn't work" on a Linux box that has
    // every tool present. Windows solves the same problem with drivers (WinUSB /
    // Zadig) and macOS needs nothing at all, so both entries are Linux-only.
    if cfg!(target_os = "linux") {
        tools.push(RequiredTool {
            name: UDEV_RULES_TOOL,
            description:
                "udev rules granting non-root access to debug probes (ST-Link, J-Link, CMSIS-DAP, DFU)",
            toolchain: Some(ToolchainKind::RustEmbedded),
            severity: Severity::Feature,
            impact:
                "Debug probes are visible but cannot be OPENED: probe-rs / OpenOCD / dfu-util fail \
                 with \"Permission denied\" or find no probe unless run with sudo. Install \
                 probe-rs' 69-probe-rs.rules (and 60-openocd.rules), then `sudo udevadm control \
                 --reload && sudo udevadm trigger`.",
            check_cmd: UDEV_CHECK,
            check_args: &[],
            check_pattern: "",
            min_version: None,
            install_cmd: None, // writing to /etc/udev/rules.d needs root
            install_args: &[],
            manual_url: "https://probe.rs/docs/getting-started/probe-setup/#linux%3A-udev-rules",
            status: ToolStatus::Unknown,
        });
        tools.push(RequiredTool {
            name: SERIAL_ACCESS_TOOL,
            description: "Permission to open /dev/ttyUSB* and /dev/ttyACM*",
            toolchain: None,
            severity: Severity::Feature,
            impact: "The Serial tab and espflash can't open the port (\"Permission denied\"). \
                 The owning group differs by distro — `dialout` on Debian/Ubuntu/Fedora, `uucp` \
                 on Arch and openSUSE — so use the Tools tab's \"Fix access…\", which reads the \
                 group off the device actually plugged in. The change takes effect at the next \
                 LOGIN, not immediately.",
            // Sentinel: not a program, and "am I in a group called dialout?" is
            // the wrong question anyway — see `SERIAL_ACCESS_CHECK`.
            check_cmd: SERIAL_ACCESS_CHECK,
            check_args: &[],
            check_pattern: "",
            min_version: None,
            install_cmd: None, // needs root, and takes effect only after re-login
            install_args: &[],
            manual_url: "https://wiki.archlinux.org/title/Users_and_groups",
            status: ToolStatus::Unknown,
        });
    }

    Arc::new(Mutex::new(ToolsState {
        log: Vec::new(),
        tools,
    }))
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Asynchronously check one tool; updates its status from a background thread.
pub fn start_check(idx: usize, state: Arc<Mutex<ToolsState>>, ctx: egui::Context) {
    {
        let mut s = state.lock().unwrap();
        if s.tools[idx].status.is_busy() {
            return;
        }
        s.tools[idx].status = ToolStatus::Checking;
    }
    ctx.request_repaint();
    thread::spawn(move || {
        // Extract check parameters without holding the lock
        let (cmd, args, pat, minv) = {
            let s = state.lock().unwrap();
            let t = &s.tools[idx];
            (t.check_cmd, t.check_args, t.check_pattern, t.min_version)
        };
        let result = run_check_blocking(cmd, args, pat, minv);
        {
            let mut s = state.lock().unwrap();
            let name = s.tools[idx].name; // &'static str — copy out before mut borrow
            s.push_log(format!("[check] {} -> {}", name, result.label()));
            s.tools[idx].status = result;
        }
        ctx.request_repaint();
    });
}

/// Check all tools sequentially in one background thread.
pub fn start_check_all(state: Arc<Mutex<ToolsState>>, ctx: egui::Context) {
    let count = state.lock().unwrap().tools.len();
    thread::spawn(move || {
        {
            state.lock().unwrap().push_log("> Checking all tools…");
        }
        ctx.request_repaint();

        for idx in 0..count {
            // Skip tools currently being operated on by another thread
            {
                let mut s = state.lock().unwrap();
                if s.tools[idx].status.is_busy() {
                    continue;
                }
                s.tools[idx].status = ToolStatus::Checking;
            }
            ctx.request_repaint();

            // Perform check without holding the lock
            let (cmd, args, pat, minv) = {
                let s = state.lock().unwrap();
                let t = &s.tools[idx];
                (t.check_cmd, t.check_args, t.check_pattern, t.min_version)
            };
            let result = run_check_blocking(cmd, args, pat, minv);

            {
                let mut s = state.lock().unwrap();
                let name = s.tools[idx].name; // &'static str — copy out before mut borrow
                s.push_log(format!("  {} -> {}", name, result.label()));
                s.tools[idx].status = result;
            }
            ctx.request_repaint();
        }

        {
            state.lock().unwrap().push_log("[OK] Check complete");
        }
        ctx.request_repaint();
    });
}

/// Asynchronously install one tool (then re-checks it).
pub fn start_install(idx: usize, state: Arc<Mutex<ToolsState>>, ctx: egui::Context) {
    {
        let mut s = state.lock().unwrap();
        if s.tools[idx].status.is_busy() {
            return;
        }
        if s.tools[idx].install_cmd.is_none() {
            return; // manual-only — caller should show the URL instead
        }
        s.tools[idx].status = ToolStatus::Installing;
    }
    ctx.request_repaint();
    thread::spawn(move || {
        do_install_blocking(idx, &state, &ctx);
    });
}

/// Install all tools that are Missing or Failed (with auto-installers), sequentially.
pub fn start_install_missing(state: Arc<Mutex<ToolsState>>, ctx: egui::Context) {
    let count = state.lock().unwrap().tools.len();
    thread::spawn(move || {
        {
            state
                .lock()
                .unwrap()
                .push_log("> Installing missing tools…");
        }
        ctx.request_repaint();

        let mut installed_any = false;
        for idx in 0..count {
            let should_install = {
                let s = state.lock().unwrap();
                let t = &s.tools[idx];
                matches!(t.status, ToolStatus::Missing | ToolStatus::Failed(_))
                    && t.install_cmd.is_some()
                    && !t.status.is_busy()
            };
            if should_install {
                installed_any = true;
                {
                    state.lock().unwrap().tools[idx].status = ToolStatus::Installing;
                }
                ctx.request_repaint();
                do_install_blocking(idx, &state, &ctx);
            }
        }

        if !installed_any {
            state.lock().unwrap().push_log("  (nothing to install)");
        }
        {
            state.lock().unwrap().push_log("[OK] Install pass complete");
        }
        ctx.request_repaint();
    });
}

// ── Internal helpers ───────────────────────────────────────────────────────────

/// Run the check command synchronously and return the resulting `ToolStatus`.
/// Does **not** hold any mutex while running the external command.
/// Sentinel [`RequiredTool::check_cmd`] for the MSVC toolchain: it is NOT a CLI
/// on PATH, and "the binary exists" is exactly the wrong test (a half-installed
/// Visual Studio has `cl.exe` but no libs/headers), so it gets a file-based probe
/// instead of a spawned command. See [`crate::msvc`].
pub const MSVC_CHECK: &str = "@msvc-toolchain";

/// Sentinel for the ELF→bin converter: the DFU flash path tries `llvm-objcopy`,
/// then `arm-none-eabi-objcopy`, then `cargo objcopy` ([`crate::dfu`]), so the
/// catalog must answer the same question — "is ANY of the three here?" — instead
/// of picking one and calling the other two setups broken.
pub const OBJCOPY_CHECK: &str = "@objcopy-any";

/// Sentinel for the Linux udev rules that grant non-root access to debug probes.
/// Not a program, so there is nothing to run: it is answered by looking for rules
/// files on disk.
pub const UDEV_CHECK: &str = "@udev-rules";

/// Sentinel for the com0com virtual-pair driver (Windows). It installs no CLI on
/// PATH, so "run it and see" is impossible; the driver's service key is the
/// evidence, the same thing its own docs tell you to look for.
pub const COM0COM_CHECK: &str = "@com0com";

#[cfg(windows)]
fn check_com0com() -> ToolStatus {
    // `reg query` rather than a registry crate: one spawn, no dependency, and
    // the exit code alone answers "is the driver there?".
    let installed = crate::build::no_window(&mut Command::new("reg"))
        .args(["query", r"HKLM\SYSTEM\CurrentControlSet\Services\com0com"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !installed {
        return ToolStatus::Missing;
    }
    // "Driver installed" is NOT the same as "Bridge can work", and reporting Ok
    // for it was a false pass found on this very machine: the service key is
    // present with a pair configured as `COM#` (auto-assign) and NO virtual port
    // is actually enumerated. Same lesson as the half-installed Visual Studio in
    // `check_msvc_toolchain` — probe the capability, not the installation.
    let live: Vec<String> = serialport::available_ports()
        .map(|ps| ps.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default();
    let pairs = crate::serial_bridge::com0com_pairs(&live);
    if pairs.is_empty() {
        return ToolStatus::Failed(
            "com0com is installed but no pair has two live ports — create one in its setup \
             (a pair left on the `COM#` placeholder doesn't count until Windows assigns \
             numbers). Bridge mode stays unavailable until then."
                .to_string(),
        );
    }
    ToolStatus::Ok(
        pairs
            .iter()
            .map(|(a, b)| format!("{a} <-> {b}"))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

#[cfg(not(windows))]
fn check_com0com() -> ToolStatus {
    ToolStatus::Ok("n/a (not Windows)".to_string())
}

/// Sentinel for "can this user open a serial port?".
///
/// The obvious check — `id -nG | grep dialout` — answers the WRONG question in
/// two directions: Arch and openSUSE call the group `uucp`, and on a systemd
/// machine the logged-in user gets an ACL on the device through `uaccess` with no
/// group membership at all. So when a port is actually present the check asks the
/// kernel directly (`access(R_OK|W_OK)`, which does NOT open the device — opening
/// a tty asserts DTR and would reset the attached board), and only falls back to
/// group membership when there is nothing plugged in to test.
pub const SERIAL_ACCESS_CHECK: &str = "@serial-access";

/// Catalog name of that entry — the Tools tab matches on it to offer "Fix
/// access…", same arrangement as [`UDEV_RULES_TOOL`].
pub const SERIAL_ACCESS_TOOL: &str = "serial port access";

/// Groups that conventionally own serial devices. Only consulted when no device
/// is plugged in; with one present its REAL group is read off the node.
const SERIAL_GROUP_CANDIDATES: [&str; 3] = ["dialout", "uucp", "plugdev"];

/// Exact-token membership test over `id -nG` output (space-separated names).
/// A substring test would pass on a group merely CONTAINING the name.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn groups_contain(id_output: &str, group: &str) -> bool {
    id_output.split_whitespace().any(|g| g == group)
}

/// Serial device nodes present right now, sorted. `/dev/ttyUSB*` for USB-serial
/// bridges (CH340, CP210x, FTDI), `/dev/ttyACM*` for CDC devices (ESP32-S3/C3
/// native USB, many dev boards).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn serial_device_nodes() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/dev") {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("ttyUSB") || name.starts_with("ttyACM") {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// The group that owns `path`, by name. `None` when the gid has no entry.
#[cfg(target_os = "linux")]
fn owning_group(path: &std::path::Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let gid = std::fs::metadata(path).ok()?.gid();
    // SAFETY: getgrgid returns a pointer into a static buffer, valid until the
    // next call; the name is copied out immediately, before anything else can
    // call it. Null = no such group.
    unsafe {
        let grp = libc::getgrgid(gid);
        if grp.is_null() {
            return None;
        }
        let name = (*grp).gr_name;
        if name.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned())
    }
}

/// Can we read AND write `path` right now? Answers the real question, and unlike
/// opening the device it has no side effects on the attached board.
#[cfg(target_os = "linux")]
fn can_use_device(path: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated string for the duration of the call.
    unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) == 0 }
}

/// The group the user should be added to, and the command that does it — used by
/// the Tools tab's "Fix access…". Reads the group off a plugged-in device when
/// there is one, so the suggestion is right on Arch (`uucp`) as well as Debian.
#[cfg(target_os = "linux")]
pub fn serial_access_fix() -> (String, String) {
    let group = serial_device_nodes()
        .iter()
        .find_map(|d| owning_group(d))
        .unwrap_or_else(|| SERIAL_GROUP_CANDIDATES[0].to_string());
    let cmd = format!("sudo usermod -aG {group} $USER");
    (group, cmd)
}

#[cfg(not(target_os = "linux"))]
pub fn serial_access_fix() -> (String, String) {
    let group = SERIAL_GROUP_CANDIDATES[0].to_string();
    let cmd = format!("sudo usermod -aG {group} $USER");
    (group, cmd)
}

#[cfg(target_os = "linux")]
fn check_serial_access() -> ToolStatus {
    let devices = serial_device_nodes();

    // A device is plugged in: ask the kernel, which accounts for uaccess ACLs,
    // group membership and plain permissions all at once.
    if let Some(usable) = devices.iter().find(|d| can_use_device(d)) {
        return ToolStatus::Ok(format!("can open {}", usable.display()));
    }
    if let Some(blocked) = devices.first() {
        let group = owning_group(blocked).unwrap_or_else(|| "?".to_string());
        return ToolStatus::Failed(format!(
            "{} is present but not writable by you (owned by group `{group}`). \
             Add yourself with `sudo usermod -aG {group} $USER`, then LOG OUT and back in.",
            blocked.display()
        ));
    }

    // Nothing plugged in — the honest answer is "can't tell", so fall back to
    // the conventional groups and say which one matched.
    let Ok(out) = Command::new("id").arg("-nG").output() else {
        return ToolStatus::Unknown;
    };
    let mine = String::from_utf8_lossy(&out.stdout);
    match SERIAL_GROUP_CANDIDATES
        .iter()
        .find(|g| groups_contain(&mine, g))
    {
        Some(g) => ToolStatus::Ok(format!("in group `{g}` (no port plugged in to verify)")),
        None => ToolStatus::Missing,
    }
}

#[cfg(not(target_os = "linux"))]
fn check_serial_access() -> ToolStatus {
    ToolStatus::Ok("n/a (not Linux)".to_string())
}

/// The three ELF→bin converters, in the order [`crate::dfu::objcopy`] tries them.
/// Kept next to the sentinel so the two lists can be compared at a glance.
const OBJCOPY_CANDIDATES: [(&str, &[&str]); 3] = [
    ("llvm-objcopy", &["--version"]),
    ("arm-none-eabi-objcopy", &["--version"]),
    ("cargo", &["objcopy", "--version"]),
];

/// `Ok` naming the first converter found, `Missing` when none of the three is.
fn check_objcopy_any() -> ToolStatus {
    for (cmd, args) in OBJCOPY_CANDIDATES {
        let mut c = Command::new(cmd);
        c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        crate::build::no_window_raw(&mut c);
        if matches!(c.output(), Ok(out) if out.status.success()) {
            return ToolStatus::Ok(format!("{cmd} ({args:?} ok)").replace('"', ""));
        }
    }
    ToolStatus::Missing
}

/// Look for udev rules that mention a debug-probe tool. Both the system
/// directories and the admin one are searched, because packages install into
/// `/usr/lib` (or `/lib`) while a hand-installed rule lands in `/etc`.
///
/// Deliberately a NAME match, not a parse: rule files are matched by vendor/
/// product id in a syntax we have no business interpreting, and "a file called
/// 69-probe-rs.rules exists" is the same thing every setup guide tells the user
/// to check.
/// Where udev rules live. Both the system directories and the admin one, because
/// packages install into `/usr/lib` (or `/lib`) while a hand-installed rule lands
/// in `/etc`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const UDEV_DIRS: [&str; 3] = [
    "/etc/udev/rules.d",
    "/usr/lib/udev/rules.d",
    "/lib/udev/rules.d",
];

/// Does this rules-file name look like a debug-probe rule? Pure, so the matching
/// is testable on any host — only the directory walk around it is Linux-only.
/// (Compiled everywhere for exactly that reason; only Linux CALLS it.)
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn udev_rule_matches(file_name: &str) -> bool {
    const MARKERS: [&str; 4] = ["probe-rs", "openocd", "stlink", "dfu"];
    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(".rules") && MARKERS.iter().any(|m| lower.contains(m))
}

/// Probe-related rules files present in `dirs`, sorted and deduplicated.
///
/// Compiled on EVERY platform — only the call is Linux-only. A Linux-only body
/// is a body nobody here can compile, let alone test; keeping the whole walk
/// portable means a mistake in it fails the build on this machine too.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn scan_udev_dirs(dirs: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue; // an absent directory is normal, not an error
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if udev_rule_matches(&name) {
                found.push(name);
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

#[cfg(target_os = "linux")]
fn check_udev_rules() -> ToolStatus {
    let found = scan_udev_dirs(&UDEV_DIRS);
    if found.is_empty() {
        ToolStatus::Missing
    } else {
        ToolStatus::Ok(found.join(", "))
    }
}

#[cfg(not(target_os = "linux"))]
fn check_udev_rules() -> ToolStatus {
    ToolStatus::Ok("n/a (not Linux)".to_string())
}

/// First dotted number in `text`, e.g. `"rustc 1.89.0 (abc 2026-01-01)"` →
/// `"1.89.0"`. Tools print their version in wildly different shapes, so we scan
/// rather than assume a position. `None` when there is no number at all.
pub fn parse_version(text: &str) -> Option<String> {
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut seen_dot = false;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == '.') {
                // A trailing dot ("1.2." / end of sentence) isn't part of it.
                if bytes[i] == '.' {
                    if i + 1 >= bytes.len() || !bytes[i + 1].is_ascii_digit() {
                        break;
                    }
                    seen_dot = true;
                }
                i += 1;
            }
            if seen_dot {
                return Some(bytes[start..i].iter().collect());
            }
        } else {
            i += 1;
        }
    }
    None
}

/// `found < min`, comparing dotted components NUMERICALLY (so 1.10 > 1.9, which
/// a string compare gets wrong). Missing components count as 0 — `"1.74"` and
/// `"1.74.0"` are equal. Unparsable input answers `false`: never cry "outdated"
/// over something we failed to read.
pub fn version_lt(found: &str, min: &str) -> bool {
    // Strict: EVERY component must be a number. A string we can't read (empty,
    // "abc", "1.x") yields None → answer `false`, so we never accuse a tool of
    // being outdated on the strength of output we didn't understand.
    let part = |s: &str| -> Option<Vec<u64>> {
        let v: Vec<&str> = s.trim().split('.').collect();
        if v.iter().any(|c| c.trim().is_empty()) {
            return None;
        }
        v.iter().map(|c| c.trim().parse::<u64>().ok()).collect()
    };
    let (Some(f), Some(m)) = (part(found), part(min)) else {
        return false;
    };
    for i in 0..f.len().max(m.len()) {
        let a = f.get(i).copied().unwrap_or(0);
        let b = m.get(i).copied().unwrap_or(0);
        if a != b {
            return a < b;
        }
    }
    false
}

/// File-based probe of the MSVC host toolchain: `Ok` when some install has BOTH
/// `lib\x64\msvcrt.lib` and `include\vcruntime.h`; `Failed` (with the reason)
/// when installs exist but are all incomplete — the case that silently breaks
/// every build; `Missing` when there is none at all.
#[cfg(windows)]
fn check_msvc_toolchain() -> ToolStatus {
    let installs = crate::msvc::installs();
    if let Some(ok) = installs.iter().find(|i| i.is_complete()) {
        // Name the broken ones too: they are why builds can still fail if the
        // env injection is ever bypassed.
        let broken = installs.iter().filter(|i| !i.is_complete()).count();
        return ToolStatus::Ok(if broken > 0 {
            format!("{} (+{broken} incomplete)", ok.label())
        } else {
            ok.label()
        });
    }
    if installs.is_empty() {
        return ToolStatus::Missing;
    }
    let detail: Vec<String> = installs
        .iter()
        .map(|i| {
            let mut miss = Vec::new();
            if !i.has_libs {
                miss.push("libs");
            }
            if !i.has_headers {
                miss.push("headers");
            }
            format!("{} missing {}", i.label(), miss.join("+"))
        })
        .collect();
    ToolStatus::Failed(format!(
        "Visual Studio found but its C++ x64 toolchain is incomplete ({}). \
         Install the \"Desktop development with C++\" workload / Build Tools.",
        detail.join("; ")
    ))
}

#[cfg(not(windows))]
fn check_msvc_toolchain() -> ToolStatus {
    ToolStatus::Ok("n/a (not Windows)".to_string())
}

fn run_check_blocking(
    cmd: &str,
    args: &[&str],
    pattern: &str,
    min_version: Option<&'static str>,
) -> ToolStatus {
    // Sentinels: not programs on PATH, so they never reach the spawn below.
    if cmd == MSVC_CHECK {
        return check_msvc_toolchain();
    }
    if cmd == OBJCOPY_CHECK {
        return check_objcopy_any();
    }
    if cmd == UDEV_CHECK {
        return check_udev_rules();
    }
    if cmd == SERIAL_ACCESS_CHECK {
        return check_serial_access();
    }
    if cmd == COM0COM_CHECK {
        return check_com0com();
    }
    let mut c = Command::new(cmd);
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    match c.output() {
        Err(_) => ToolStatus::Missing,
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}{stderr}");

            if !out.status.success() {
                return ToolStatus::Missing;
            }
            if !pattern.is_empty() && !combined.contains(pattern) {
                return ToolStatus::Missing;
            }

            // Version string:
            // • Pattern-based checks (e.g. `rustup target list --installed`) →
            //   show "installed" since the first stdout line is a random target name.
            // • Direct version checks (e.g. `rustc --version`) →
            //   show first non-empty line of stdout.
            let version = if !pattern.is_empty() {
                "installed".to_string()
            } else {
                stdout
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .to_string()
            };

            // Present — but is it new enough? Only when a real minimum is
            // declared AND a version could actually be read.
            if let (Some(min), Some(found)) = (min_version, parse_version(&version)) {
                if version_lt(&found, min) {
                    return ToolStatus::Outdated { found, min };
                }
            }
            ToolStatus::Ok(version)
        }
    }
}

/// Install `tools[idx]` synchronously, stream output to the log, then re-check.
/// Caller must have already set the tool's status to `Installing` and released
/// the lock before calling this function.
fn do_install_blocking(idx: usize, state: &Arc<Mutex<ToolsState>>, ctx: &egui::Context) {
    // Extract all needed data while holding the lock briefly
    let (cmd, args_owned, name) = {
        let s = state.lock().unwrap();
        let t = &s.tools[idx];
        let args: Vec<String> = t.install_args.iter().map(|a| a.to_string()).collect();
        (t.install_cmd.unwrap_or(""), args, t.name.to_string())
    };

    {
        state
            .lock()
            .unwrap()
            .push_log(format!("> Installing {name}…"));
    }
    ctx.request_repaint();

    let mut c = Command::new(cmd);
    c.args(&args_owned)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let result = match c.output() {
        Err(e) => {
            let msg = format!("Cannot run `{cmd}`: {e}");
            state.lock().unwrap().push_log(format!("  [X] {msg}"));
            ctx.request_repaint();
            ToolStatus::Failed(msg)
        }
        Ok(out) => {
            // Append combined stdout + stderr to the log
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            );
            {
                let mut s = state.lock().unwrap();
                for line in combined.lines() {
                    if !line.trim().is_empty() {
                        s.log.push(format!("  {line}"));
                    }
                }
            }
            ctx.request_repaint();

            if !out.status.success() {
                let msg = format!("{cmd} exited with {}", out.status);
                state.lock().unwrap().push_log(format!("  [X] {msg}"));
                ctx.request_repaint();
                ToolStatus::Failed(msg)
            } else {
                state
                    .lock()
                    .unwrap()
                    .push_log(format!("  [OK] {name} installed OK"));
                ctx.request_repaint();

                // Re-check to confirm installation and capture the version string
                let (check_cmd, check_args, check_pattern, min_version) = {
                    let s = state.lock().unwrap();
                    let t = &s.tools[idx];
                    (t.check_cmd, t.check_args, t.check_pattern, t.min_version)
                };
                run_check_blocking(check_cmd, check_args, check_pattern, min_version)
            }
        }
    };

    state.lock().unwrap().tools[idx].status = result;
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure mode this per-platform table invites: picking the command
    /// with `per_os` but forgetting to switch the ARGS with it, leaving a bare
    /// `winget` / `brew` that installs nothing and reports success.
    #[test]
    fn an_auto_installer_always_has_arguments() {
        let s = make_tools_state();
        let s = s.lock().unwrap();
        for t in &s.tools {
            if t.install_cmd.is_some() {
                assert!(
                    !t.install_args.is_empty(),
                    "{} has an install command but no arguments",
                    t.name
                );
            }
        }
    }

    /// The other half: an entry the host can't auto-install MUST tell the user
    /// where to get it, or the Tools tab has nothing to offer but "Missing".
    #[test]
    fn a_manual_tool_always_has_a_url() {
        let s = make_tools_state();
        let s = s.lock().unwrap();
        for t in &s.tools {
            if t.install_cmd.is_none() {
                assert!(
                    t.manual_url.starts_with("http"),
                    "{} can't be auto-installed and has no manual URL",
                    t.name
                );
            }
        }
    }

    /// The host C toolchain is Blocking on every platform — it is the entry that
    /// explains "nothing builds", and losing it on a platform is exactly the gap
    /// this table was made to close.
    #[test]
    fn the_host_toolchain_entry_exists_everywhere() {
        let s = make_tools_state();
        let s = s.lock().unwrap();
        let host = s
            .tools
            .iter()
            .find(|t| t.check_cmd == MSVC_CHECK || (t.check_cmd == "cc" && t.toolchain.is_none()))
            .expect("no host C toolchain entry for this platform");
        assert_eq!(host.severity, Severity::Blocking);
    }

    /// The directory walk itself, exercised on THIS host: a missing directory is
    /// skipped rather than treated as an error, and only rule files count.
    #[test]
    fn udev_scan_reads_real_directories() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["69-probe-rs.rules", "60-openocd.rules", "README.md"] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        let p = dir.path().to_string_lossy().to_string();
        let found = scan_udev_dirs(&[&p, "/definitely/not/here"]);
        assert_eq!(found, vec!["60-openocd.rules", "69-probe-rs.rules"]);
        assert!(scan_udev_dirs(&["/definitely/not/here"]).is_empty());
    }

    /// `id -nG` prints space-separated names. A SUBSTRING test — the obvious
    /// implementation, and what this entry used to do — passes on any group that
    /// merely contains the word, and on Arch it fails a working setup outright
    /// because the group there is `uucp`.
    #[test]
    fn group_membership_matches_whole_names_only() {
        let out = "istrati wheel uucp video\n";
        assert!(groups_contain(out, "uucp"));
        assert!(groups_contain(out, "wheel"));
        assert!(!groups_contain(out, "dialout"));
        // The substring trap: `dialout-admin` must not read as `dialout`.
        assert!(!groups_contain("me dialout-admin\n", "dialout"));
        assert!(!groups_contain("", "dialout"));
    }

    /// The fix must be a runnable command naming a real group, on any host —
    /// with no device plugged in it falls back to the conventional one.
    #[test]
    fn serial_fix_is_a_runnable_command() {
        let (group, cmd) = serial_access_fix();
        assert!(!group.trim().is_empty());
        assert_eq!(cmd, format!("sudo usermod -aG {group} $USER"));
        assert!(SERIAL_GROUP_CANDIDATES.contains(&group.as_str()) || cfg!(target_os = "linux"));
    }

    #[test]
    fn udev_rule_names_are_recognised() {
        assert!(udev_rule_matches("69-probe-rs.rules"));
        assert!(udev_rule_matches("60-openocd.rules"));
        assert!(udev_rule_matches("49-stlinkv2.rules"));
        // Not a rules file, and not about a probe.
        assert!(!udev_rule_matches("70-probe-rs.txt"));
        assert!(!udev_rule_matches("99-systemd.rules"));
        assert!(!udev_rule_matches(""));
    }

    /// Every catalog entry must carry a non-empty, user-facing `impact` — it is
    /// the answer to "why does the IDE need this?" shown in the banner + Tools.
    #[test]
    fn every_tool_explains_its_impact() {
        let s = make_tools_state();
        let s = s.lock().unwrap();
        assert!(!s.tools.is_empty());
        for t in &s.tools {
            assert!(!t.impact.trim().is_empty(), "{} has no impact text", t.name);
            assert!(
                t.impact.len() > 20,
                "{} impact too terse: {:?}",
                t.name,
                t.impact
            );
        }
    }

    /// The core toolchain must be classed Blocking, feature tools must not be —
    /// otherwise the startup banner either misses a fatal gap or cries wolf.
    #[test]
    fn severity_matches_reality() {
        let s = make_tools_state();
        let s = s.lock().unwrap();
        let sev = |n: &str| s.tools.iter().find(|t| t.name == n).map(|t| t.severity);
        assert_eq!(sev("rustup"), Some(Severity::Blocking));
        assert_eq!(sev("rustc"), Some(Severity::Blocking));
        assert_eq!(sev("cargo-bloat"), Some(Severity::Feature));
        assert_eq!(sev("git"), Some(Severity::Feature));
        assert_eq!(sev("probe-rs"), Some(Severity::Feature));
    }

    /// An UNCHECKED catalog must report no problems — the banner may never fire
    /// on `Unknown`, or it would accuse the user before anything was verified.
    #[test]
    fn unchecked_catalog_reports_no_problems() {
        let s = make_tools_state();
        let s = s.lock().unwrap();
        assert!(s.problems(None).is_empty());
        assert!(s.blocking_problems(None).is_empty());
    }

    #[test]
    fn version_is_scanned_out_of_any_banner() {
        assert_eq!(
            parse_version("rustc 1.89.0 (abc 2026-01-01)").as_deref(),
            Some("1.89.0")
        );
        assert_eq!(parse_version("probe-rs 0.31.0").as_deref(), Some("0.31.0"));
        assert_eq!(
            parse_version("git version 2.45.1.windows.1").as_deref(),
            Some("2.45.1")
        );
        // A trailing dot is punctuation, not part of the number.
        assert_eq!(parse_version("v1.2. done").as_deref(), Some("1.2"));
        // Nothing dotted → nothing claimed.
        assert_eq!(parse_version("installed"), None);
        assert_eq!(parse_version("version 7"), None);
    }

    #[test]
    fn versions_compare_numerically_not_as_strings() {
        assert!(
            version_lt("1.9.0", "1.10.0"),
            "1.9 < 1.10 (string compare gets this wrong)"
        );
        assert!(!version_lt("1.10.0", "1.9.0"));
        assert!(version_lt("1.73.0", "1.74"));
        assert!(
            !version_lt("1.74.0", "1.74"),
            "missing components count as 0"
        );
        assert!(!version_lt("1.74", "1.74.0"));
        assert!(!version_lt("2.0", "1.99"));
        // Unreadable input must never be called outdated.
        assert!(!version_lt("", "1.74"));
        assert!(!version_lt("abc", "1.74"));
    }

    /// `Outdated` warns (it shows up in `problems`) but must NOT disable the
    /// features that use the tool — it may well still work.
    #[test]
    fn outdated_warns_but_never_disables() {
        let s = make_tools_state();
        let mut s = s.lock().unwrap();
        if let Some(t) = s.tools.iter_mut().find(|t| t.name == "rustc") {
            t.status = ToolStatus::Outdated {
                found: "1.70.0".into(),
                min: "1.74",
            };
        }
        let names: Vec<&str> = s.problems(None).into_iter().map(|(n, _, _)| n).collect();
        assert!(
            names.contains(&"rustc"),
            "outdated must be reported: {names:?}"
        );
        assert!(
            !s.unavailable().contains(&"rustc"),
            "outdated must NOT gate features"
        );
    }

    /// Only requirements we can actually justify carry a minimum — an invented
    /// one would nag users whose older build works fine.
    #[test]
    fn min_versions_are_declared_sparingly() {
        let s = make_tools_state();
        let s = s.lock().unwrap();
        let with_min: Vec<&str> = s
            .tools
            .iter()
            .filter(|t| t.min_version.is_some())
            .map(|t| t.name)
            .collect();
        assert_eq!(
            with_min,
            vec!["rustc"],
            "unexpected min_version set: {with_min:?}"
        );
        // And the one we declare must itself be parseable by our comparator.
        assert!(!version_lt("1.74.0", "1.74"));
        // probe-rs deliberately carries NONE: its breakages are version RANGES
        // (0.31.0 panics enumerating, 0.32.0 can't open a WinUSB-bound ST-Link),
        // so a floor would mark the WORKING 0.29.0 outdated and push an upgrade
        // that breaks debugging. The failure hints name the problem instead.
        assert!(
            s.tools
                .iter()
                .find(|t| t.name == "probe-rs")
                .is_some_and(|t| t.min_version.is_none()),
            "probe-rs must not carry a minimum — see the comment in the catalog"
        );
    }

    /// A tool must be gated ONLY on proof of absence: `Unknown` (before the
    /// startup check) and the busy states must never grey a button out.
    #[test]
    fn unavailable_needs_proof_not_ignorance() {
        let s = make_tools_state();
        let mut s = s.lock().unwrap();
        assert!(s.unavailable().is_empty(), "Unknown must not gate anything");

        for t in s.tools.iter_mut() {
            t.status = ToolStatus::Checking;
        }
        assert!(s.unavailable().is_empty(), "a running check must not gate");

        for t in s.tools.iter_mut() {
            t.status = ToolStatus::Ok("1.0".into());
        }
        assert!(s.unavailable().is_empty());

        if let Some(t) = s.tools.iter_mut().find(|t| t.name == "probe-rs") {
            t.status = ToolStatus::Missing;
        }
        assert_eq!(s.unavailable(), vec!["probe-rs"]);
    }

    /// Problems are filtered by the selected chip's toolchain: an ESP-only tool
    /// must not be reported while an STM32 chip is selected (and vice-versa).
    #[test]
    fn problems_are_filtered_by_toolchain() {
        let s = make_tools_state();
        let mut s = s.lock().unwrap();
        for t in s.tools.iter_mut() {
            t.status = ToolStatus::Missing;
        }
        let stm: Vec<&str> = s
            .problems(Some(&ToolchainKind::RustEmbedded))
            .into_iter()
            .map(|(n, _, _)| n)
            .collect();
        assert!(
            stm.contains(&"rustup"),
            "common tools always count: {stm:?}"
        );
        assert!(stm.contains(&"probe-rs"), "{stm:?}");
        assert!(
            !stm.contains(&"espflash"),
            "ESP tool leaked into STM32: {stm:?}"
        );

        let esp: Vec<&str> = s
            .problems(Some(&ToolchainKind::EspRust))
            .into_iter()
            .map(|(n, _, _)| n)
            .collect();
        assert!(esp.contains(&"espflash"), "{esp:?}");
        assert!(
            !esp.contains(&"probe-rs"),
            "STM32 tool leaked into ESP: {esp:?}"
        );

        // Blocking is a strict subset of all problems.
        let all = s.problems(Some(&ToolchainKind::RustEmbedded)).len();
        let blocking = s
            .blocking_problems(Some(&ToolchainKind::RustEmbedded))
            .len();
        assert!(
            blocking > 0 && blocking < all,
            "all={all} blocking={blocking}"
        );
    }
}
