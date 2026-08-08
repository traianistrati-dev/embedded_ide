//! Tool availability checker and one-click installer.
//!
//! Each [`RequiredTool`] entry knows how to verify and (where possible) install
//! a dependency on the host machine.  All blocking operations run in background
//! threads and write back through the shared [`Arc<Mutex<ToolsState>>`], then
//! call `ctx.request_repaint()` so the UI stays in sync without polling.
//!
//! # Tool catalog
//!
//! | Tool                        | Toolchain       | Auto-install |
//! |-----------------------------|-----------------|--------------|
//! | rustup                      | All             | No (manual)  |
//! | rustc                       | All             | Yes          |
//! | thumbv7m-none-eabi target   | RustEmbedded    | Yes          |
//! | probe-rs                    | RustEmbedded    | Yes          |
//! | riscv32imc-unknown-none-elf | EspRust         | Yes          |
//! | rust-src component          | EspRust         | Yes          |
//! | espflash                    | EspRust         | Yes          |

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
            impact: "No RTT logs, on-target Debug, runtime flamegraph or probe-rs flashing. \
                     Below 0.32 the Debug tab is unreliable: 0.31.0 panics inside its own USB \
                     probe enumeration (glasgow/mux.rs), which kills the session before it starts.",
            check_cmd: "probe-rs",
            check_args: &["--version"],
            check_pattern: "",
            // A REAL, reproduced minimum, not an invented one: probe-rs 0.31.0
            // aborts with `unreachable!()` while enumerating probes, so the
            // dap-server dies on launch. See `failure_hint`'s [PROBE_RS_PANIC].
            // Installing again (`cargo install probe-rs-tools --locked`) upgrades.
            min_version: Some("0.32.0"),
            install_cmd: Some("cargo"),
            install_args: &["install", "probe-rs-tools", "--locked"],
            manual_url: "https://probe.rs/docs/getting-started/installation/",
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

    // ── Windows host linker prerequisite ─────────────────────────────────────
    // Not an embedded tool: Rust links every build-script / proc-macro for the
    // HOST with the MSVC linker, so without this NOTHING builds (LNK1104
    // msvcrt.lib). Probed by file, not by binary presence — see `MSVC_CHECK`.
    #[cfg(windows)]
    tools.push(RequiredTool {
        name:          "MSVC build tools",
        description:   "Microsoft C++ x64 toolchain (msvcrt.lib + headers) — required to link Rust build-scripts on Windows",
        toolchain:     None,
        severity:      Severity::Blocking,
        impact:        "NOTHING builds: every build-script fails to link (LNK1104 msvcrt.lib / C1083 vcruntime.h).",
        check_cmd:     MSVC_CHECK,
        check_args:    &[],
        check_pattern: "",
                min_version:   None,
        install_cmd:   Some("winget"),
        install_args:  &[
            "install",
            "--id",
            "Microsoft.VisualStudio.2022.BuildTools",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--override",
            "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended",
        ],
        manual_url:    "https://visualstudio.microsoft.com/visual-cpp-build-tools/",
        status:        ToolStatus::Unknown,
    });

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
    if cmd == MSVC_CHECK {
        return check_msvc_toolchain();
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
            // probe-rs: 0.31.0 panics (`unreachable!()`) while enumerating USB
            // probes, so the Debug tab's dap-server dies on launch — a minimum
            // backed by a reproduction, which is the bar for adding one here.
            vec!["rustc", "probe-rs"],
            "unexpected min_version set: {with_min:?}"
        );
        // And the ones we declare must be parseable by our comparator.
        assert!(!version_lt("1.74.0", "1.74"));
        // The real banners: 0.31.0 is flagged, 0.32.0 is not.
        let probe_rs_min = s
            .tools
            .iter()
            .find(|t| t.name == "probe-rs")
            .and_then(|t| t.min_version)
            .expect("probe-rs carries a minimum");
        let ver = |banner: &str| parse_version(banner).expect("version in banner");
        assert!(version_lt(
            &ver("probe-rs 0.31.0 (git commit: crates.io)"),
            probe_rs_min
        ));
        assert!(!version_lt(
            &ver("probe-rs 0.32.0 (git commit: crates.io)"),
            probe_rs_min
        ));
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
