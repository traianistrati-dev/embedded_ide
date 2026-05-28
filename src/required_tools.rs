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
}

impl ToolStatus {
    pub fn is_busy(&self) -> bool {
        matches!(self, ToolStatus::Checking | ToolStatus::Installing)
    }

    pub fn label(&self) -> &str {
        match self {
            ToolStatus::Unknown    => "—",
            ToolStatus::Checking   => "Checking…",
            ToolStatus::Ok(_)      => "OK ✔",
            ToolStatus::Missing    => "Missing",
            ToolStatus::Installing => "Installing…",
            ToolStatus::Failed(_)  => "Failed",
        }
    }

    pub fn color(&self) -> egui::Color32 {
        match self {
            ToolStatus::Ok(_)      => egui::Color32::from_rgb(80, 200, 100),
            ToolStatus::Missing    => egui::Color32::from_rgb(230, 160, 50),
            ToolStatus::Failed(_)  => egui::Color32::from_rgb(220, 70, 60),
            ToolStatus::Checking
            | ToolStatus::Installing => egui::Color32::from_rgb(180, 180, 80),
            ToolStatus::Unknown    => egui::Color32::GRAY,
        }
    }
}

// ── RequiredTool ───────────────────────────────────────────────────────────────

pub struct RequiredTool {
    pub name:         &'static str,
    pub description:  &'static str,
    /// `None` = required for all toolchains.
    pub toolchain:    Option<ToolchainKind>,
    // ── Check ────────────────────────────────────────────────────────────────
    pub check_cmd:    &'static str,
    pub check_args:   &'static [&'static str],
    /// If non-empty: stdout+stderr must contain this substring after a
    /// successful exit code for the tool to be considered present.
    /// Used for `rustup target list --installed` pattern checks.
    pub check_pattern: &'static str,
    // ── Install ──────────────────────────────────────────────────────────────
    /// `None` = cannot be auto-installed; direct the user to `manual_url`.
    pub install_cmd:  Option<&'static str>,
    pub install_args: &'static [&'static str],
    pub manual_url:   &'static str,
    // ── Runtime state (mutated by background threads) ─────────────────────────
    pub status: ToolStatus,
}

// ── ToolsState ─────────────────────────────────────────────────────────────────

pub struct ToolsState {
    pub tools: Vec<RequiredTool>,
    pub log:   Vec<String>,
}

impl ToolsState {
    fn push_log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
    }

    pub fn any_busy(&self) -> bool {
        self.tools.iter().any(|t| t.status.is_busy())
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
    pub name:             &'static str,
    pub description:      &'static str,
    pub toolchain:        Option<ToolchainKind>,
    pub status:           ToolStatus,
    pub can_auto_install: bool,
    pub manual_url:       &'static str,
}

impl ToolsState {
    pub fn snapshot(&self) -> Vec<ToolRow> {
        self.tools
            .iter()
            .map(|t| ToolRow {
                name:             t.name,
                description:      t.description,
                toolchain:        t.toolchain.clone(),
                status:           t.status.clone(),
                can_auto_install: t.install_cmd.is_some(),
                manual_url:       t.manual_url,
            })
            .collect()
    }
}

// ── Tool catalog ───────────────────────────────────────────────────────────────

pub fn make_tools_state() -> Arc<Mutex<ToolsState>> {
    Arc::new(Mutex::new(ToolsState {
        log: Vec::new(),
        tools: vec![
            // ── Common to all toolchains ─────────────────────────────────────
            RequiredTool {
                name:          "rustup",
                description:   "Rust toolchain installer — manages Rust versions and targets",
                toolchain:     None,
                check_cmd:     "rustup",
                check_args:    &["--version"],
                check_pattern: "",
                install_cmd:   None, // must be installed manually from rustup.rs
                install_args:  &[],
                manual_url:    "https://rustup.rs",
                status:        ToolStatus::Unknown,
            },
            RequiredTool {
                name:          "rustc",
                description:   "Rust compiler (stable toolchain)",
                toolchain:     None,
                check_cmd:     "rustc",
                check_args:    &["--version"],
                check_pattern: "",
                install_cmd:   Some("rustup"),
                install_args:  &["install", "stable"],
                manual_url:    "https://www.rust-lang.org/tools/install",
                status:        ToolStatus::Unknown,
            },
            // ── RustEmbedded (STM32 / ARM Cortex-M) ─────────────────────────
            RequiredTool {
                name:          "thumbv7m-none-eabi",
                description:   "Rust target for ARM Cortex-M3 (STM32F1xx bare-metal)",
                toolchain:     Some(ToolchainKind::RustEmbedded),
                check_cmd:     "rustup",
                check_args:    &["target", "list", "--installed"],
                check_pattern: "thumbv7m-none-eabi",
                install_cmd:   Some("rustup"),
                install_args:  &["target", "add", "thumbv7m-none-eabi"],
                manual_url:    "https://docs.rust-embedded.org/book/intro/install.html",
                status:        ToolStatus::Unknown,
            },
            RequiredTool {
                name:          "probe-rs",
                description:   "ARM debug probe runner — flashes and debugs STM32 via SWD / JTAG",
                toolchain:     Some(ToolchainKind::RustEmbedded),
                check_cmd:     "probe-rs",
                check_args:    &["--version"],
                check_pattern: "",
                install_cmd:   Some("cargo"),
                install_args:  &["install", "probe-rs-tools", "--locked"],
                manual_url:    "https://probe.rs/docs/getting-started/installation/",
                status:        ToolStatus::Unknown,
            },
            // ── EspRust (ESP32-C3 / RISC-V) ──────────────────────────────────
            RequiredTool {
                name:          "riscv32imc-unknown-none-elf",
                description:   "Rust target for RISC-V ESP32-C3 bare-metal",
                toolchain:     Some(ToolchainKind::EspRust),
                check_cmd:     "rustup",
                check_args:    &["target", "list", "--installed"],
                check_pattern: "riscv32imc-unknown-none-elf",
                install_cmd:   Some("rustup"),
                install_args:  &["target", "add", "riscv32imc-unknown-none-elf"],
                manual_url:    "https://esp-rs.github.io/book/installation/riscv.html",
                status:        ToolStatus::Unknown,
            },
            RequiredTool {
                name:          "rust-src",
                description:   "Rust source component — required by build-std for ESP32-C3",
                toolchain:     Some(ToolchainKind::EspRust),
                check_cmd:     "rustup",
                check_args:    &["component", "list", "--installed"],
                check_pattern: "rust-src",
                install_cmd:   Some("rustup"),
                install_args:  &["component", "add", "rust-src"],
                manual_url:    "https://esp-rs.github.io/book/installation/riscv.html",
                status:        ToolStatus::Unknown,
            },
            RequiredTool {
                name:          "espflash",
                description:   "ESP32 USB flash tool — programs the chip over the built-in USB serial",
                toolchain:     Some(ToolchainKind::EspRust),
                check_cmd:     "espflash",
                check_args:    &["--version"],
                check_pattern: "",
                install_cmd:   Some("cargo"),
                install_args:  &["install", "espflash"],
                manual_url:    "https://github.com/esp-rs/espflash",
                status:        ToolStatus::Unknown,
            },
        ],
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
        let (cmd, args, pat) = {
            let s = state.lock().unwrap();
            let t = &s.tools[idx];
            (t.check_cmd, t.check_args, t.check_pattern)
        };
        let result = run_check_blocking(cmd, args, pat);
        {
            let mut s = state.lock().unwrap();
            let name = s.tools[idx].name; // &'static str — copy out before mut borrow
            s.push_log(format!("[check] {} → {}", name, result.label()));
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
            state.lock().unwrap().push_log("▶ Checking all tools…");
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
            let (cmd, args, pat) = {
                let s = state.lock().unwrap();
                let t = &s.tools[idx];
                (t.check_cmd, t.check_args, t.check_pattern)
            };
            let result = run_check_blocking(cmd, args, pat);

            {
                let mut s = state.lock().unwrap();
                let name = s.tools[idx].name; // &'static str — copy out before mut borrow
                s.push_log(format!("  {} → {}", name, result.label()));
                s.tools[idx].status = result;
            }
            ctx.request_repaint();
        }

        {
            state.lock().unwrap().push_log("✔ Check complete");
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
            state.lock().unwrap().push_log("▶ Installing missing tools…");
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
            state.lock().unwrap().push_log("✔ Install pass complete");
        }
        ctx.request_repaint();
    });
}

// ── Internal helpers ───────────────────────────────────────────────────────────

/// Run the check command synchronously and return the resulting `ToolStatus`.
/// Does **not** hold any mutex while running the external command.
fn run_check_blocking(cmd: &str, args: &[&str], pattern: &str) -> ToolStatus {
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
            .push_log(format!("▶ Installing {name}…"));
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
            state.lock().unwrap().push_log(format!("  ✘ {msg}"));
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
                state.lock().unwrap().push_log(format!("  ✘ {msg}"));
                ctx.request_repaint();
                ToolStatus::Failed(msg)
            } else {
                state
                    .lock()
                    .unwrap()
                    .push_log(format!("  ✔ {name} installed OK"));
                ctx.request_repaint();

                // Re-check to confirm installation and capture the version string
                let (check_cmd, check_args, check_pattern) = {
                    let s = state.lock().unwrap();
                    let t = &s.tools[idx];
                    (t.check_cmd, t.check_args, t.check_pattern)
                };
                run_check_blocking(check_cmd, check_args, check_pattern)
            }
        }
    };

    state.lock().unwrap().tools[idx].status = result;
    ctx.request_repaint();
}
