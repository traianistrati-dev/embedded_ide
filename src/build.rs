//! Async `cargo check` runner and diagnostic types.
//!
//! Usage:
//! ```
//! let state = Arc::new(Mutex::new(BuildState::Idle));
//! build::start_build(project_dir, Arc::clone(&state), ctx.clone());
//! // later, in ui():
//! let s = state.lock().unwrap();
//! if let BuildState::Done(ref result) = *s { ... }
//! ```

use std::{
    io::BufRead,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

// ── Diagnostic ────────────────────────────────────────────────────────────────

/// A single machine-applicable source edit suggested by clippy: replace the byte
/// range `[start, end)` of `file` with `replacement`.
#[derive(Clone, Debug)]
pub struct SpanEdit {
    /// Relative path, e.g. `"src/main.rs"`.
    pub file: String,
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

/// One compiler message (error or warning).
#[derive(Clone, Debug)]
pub struct Diagnostic {
    /// `"error"` or `"warning"`
    pub level: String,
    /// Short single-line description
    pub message: String,
    /// Full rustc-formatted block (multi-line, ANSI-stripped)
    pub rendered: String,
    /// Relative path as reported by rustc, e.g. `"src/main.rs"`
    pub file: Option<String>,
    pub line: Option<u32>,
    pub col: Option<u32>,
    /// Rustc error code, e.g. `"E0308"`
    pub code: Option<String>,
    /// Machine-applicable source edits (clippy auto-fix). Empty when none.
    pub fixes: Vec<SpanEdit>,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        self.level == "error"
    }
    pub fn is_warning(&self) -> bool {
        self.level == "warning"
    }
    /// True when clippy offers an auto-applicable fix for this lint.
    pub fn has_fix(&self) -> bool {
        !self.fixes.is_empty()
    }
}

// ── BuildResult ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct BuildResult {
    pub success: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl BuildResult {
    pub fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.is_error()).count()
    }

    pub fn warning_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.is_warning()).count()
    }

    /// All diagnostics whose primary span is in `file` (e.g. `"src/main.rs"`).
    pub fn for_file<'a>(&'a self, file: &str) -> Vec<&'a Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.file.as_deref() == Some(file))
            .collect()
    }

    pub fn has_errors_in(&self, file: &str) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.is_error() && d.file.as_deref() == Some(file))
    }

    pub fn has_warnings_in(&self, file: &str) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.is_warning() && d.file.as_deref() == Some(file))
    }
}

// ── BuildState ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub enum BuildState {
    #[default]
    Idle,
    Building,
    Done(BuildResult),
    /// Fatal failure — `cargo` not found, I/O error, etc.
    Failed(String),
}

impl BuildState {
    pub fn is_building(&self) -> bool {
        matches!(self, BuildState::Building)
    }

    pub fn result(&self) -> Option<&BuildResult> {
        if let BuildState::Done(r) = self {
            Some(r)
        } else {
            None
        }
    }
}

// ── Runner ────────────────────────────────────────────────────────────────────

/// Marks state as `Building`, then spawns a thread running `cargo check`.
///
/// `target` is the Rust target triple for this MCU (e.g. `"thumbv7m-none-eabi"`).
/// The background thread calls `rustup target add <target>` first so the build
/// works even when the user hasn't installed the target manually.  The call is
/// a no-op if the target is already present, so it adds only negligible overhead.
///
/// The caller must have already written the project files to `project_dir`.
pub fn start_build(
    project_dir: PathBuf,
    target: String,
    state: Arc<Mutex<BuildState>>,
    ctx: eframe::egui::Context,
) {
    *state.lock().unwrap() = BuildState::Building;
    ctx.request_repaint();

    thread::spawn(move || {
        let next = run_cargo(&project_dir, &target, "check");
        *state.lock().unwrap() = next;
        ctx.request_repaint();
    });
}

/// Marks state as `Building`, then spawns a thread running `cargo clippy` — same
/// machinery as [`start_build`] but with clippy's extra improvement lints. Reuses
/// [`BuildState`]/[`BuildResult`] so the results render like a Cargo Check.
pub fn start_clippy(
    project_dir: PathBuf,
    target: String,
    state: Arc<Mutex<BuildState>>,
    ctx: eframe::egui::Context,
) {
    *state.lock().unwrap() = BuildState::Building;
    ctx.request_repaint();

    thread::spawn(move || {
        let next = run_cargo(&project_dir, &target, "clippy");
        *state.lock().unwrap() = next;
        ctx.request_repaint();
    });
}

/// Delete `target/` build artifacts (`cargo clean`) to recover disk space.
///
/// Sets the state to `Building` while cleaning (shows the spinner) then resets
/// it to `Idle` when done so the user can trigger a fresh build.
pub fn start_clean(
    workspace_dir: PathBuf,
    state: Arc<Mutex<BuildState>>,
    ctx: eframe::egui::Context,
) {
    *state.lock().unwrap() = BuildState::Building;
    ctx.request_repaint();

    thread::spawn(move || {
        let _ = Command::new("cargo")
            .current_dir(&workspace_dir)
            .args(["clean"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        *state.lock().unwrap() = BuildState::Idle;
        ctx.request_repaint();
    });
}

// ── Internal ──────────────────────────────────────────────────────────────────

/// Ensure the rustup target is installed, then run `cargo <subcommand>` (either
/// `"check"` or `"clippy"`) and parse its JSON diagnostics.
fn run_cargo(dir: &Path, target: &str, subcommand: &str) -> BuildState {
    // ── Step 1: install target if needed ────────────────────────────────────
    // `rustup target add` is idempotent — exits 0 immediately when already
    // installed, so the overhead on subsequent builds is negligible.
    if let Err(e) = ensure_target(target) {
        return BuildState::Failed(format!(
            "Could not install target `{target}` via rustup: {e}\n\n\
             Make sure `rustup` is in PATH or install the target manually:\n\
             rustup target add {target}"
        ));
    }

    // ── Step 2: cargo check / clippy ─────────────────────────────────────────
    let mut child = match Command::new("cargo")
        .current_dir(dir)
        .args([subcommand, "--message-format=json", "--color=never"])
        .stdout(Stdio::piped())
        // Capture stderr so we can detect disk-full and other fatal OS errors.
        // Without this, cargo crashes silently (no build-finished JSON) and the
        // IDE either reports a false success or a confusing bare "build failed".
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return BuildState::Failed(format!(
                "Could not launch `cargo`: {e}\n\n\
                 Make sure the Rust toolchain is installed and `cargo` is in PATH.\n\
                 Install from https://rustup.rs"
            ))
        }
    };

    let stdout = child.stdout.take().expect("stdout should be piped");
    let stderr = child.stderr.take().expect("stderr should be piped");

    // Drain stderr on a separate thread to avoid deadlock when its pipe buffer fills.
    let stderr_thread = thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        let _ = std::io::BufReader::new(stderr).read_to_string(&mut buf);
        buf
    });

    let mut result = BuildResult::default();
    let mut saw_build_finished = false;

    for line in std::io::BufReader::new(stdout).lines().flatten() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            match v["reason"].as_str() {
                Some("compiler-message") => {
                    if let Some(d) = parse_diagnostic(&v["message"]) {
                        result.diagnostics.push(d);
                    }
                }
                Some("build-finished") => {
                    result.success = v["success"].as_bool().unwrap_or(false);
                    saw_build_finished = true;
                }
                _ => {}
            }
        }
    }

    let _ = child.wait();
    let stderr_text = stderr_thread.join().unwrap_or_default();

    if !saw_build_finished {
        // clippy not installed → cargo prints "no such command" / "not provided".
        if subcommand == "clippy"
            && (stderr_text.contains("no such command")
                || stderr_text.contains("not provided")
                || stderr_text.contains("clippy-driver")
                || stderr_text.contains("is not installed"))
        {
            return BuildState::Failed(
                "[CLIPPY_MISSING] Clippy isn't installed for this toolchain.\n\n\
                 Install it with:\n  rustup component add clippy\n\n\
                 Then press \"Run clippy\" again."
                    .to_string(),
            );
        }
        // cargo exited without emitting build-finished — check why.
        let is_disk_full = stderr_text.contains("not enough space")
            || stderr_text.contains("There is not enough space")
            || stderr_text.contains("os error 112")  // Windows  ERROR_DISK_FULL
            || stderr_text.contains("os error 28");   // POSIX    ENOSPC
        if is_disk_full {
            return BuildState::Failed(
                "[DISK_FULL] The build target/ directory has run out of disk space.\n\n\
                 ESP32 / RISC-V builds generate several GB of LLVM artefacts the first time.\n\
                 -> Click  \"Clean target/\"  to delete cached build artefacts and free space,\n\
                   then press Build again (crates stay cached in ~/.cargo; only rebuilt files\n\
                   are re-compiled).\n\n\
                 Path: <TEMP>\\embedded_ide_0_check\\target\\"
                    .to_string(),
            );
        }
        // Infer from diagnostic content if cargo exited without build-finished
        result.success = result.error_count() == 0;
    }

    BuildState::Done(result)
}

/// Run `rustup target add <target>`, returning an error only if rustup itself
/// couldn't be launched (target already installed → exit 0, not an error).
fn ensure_target(target: &str) -> std::io::Result<()> {
    let status = Command::new("rustup")
        .args(["target", "add", target])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    // rustup exits 0 whether the target was freshly installed or already present.
    // A non-zero exit usually means a network error; we surface it as a string.
    if !status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("`rustup target add {target}` exited with {status}"),
        ));
    }
    Ok(())
}

fn parse_diagnostic(msg: &serde_json::Value) -> Option<Diagnostic> {
    let level = msg["level"].as_str()?.to_string();

    // Skip secondary annotations that aren't actionable in the inline panel
    if matches!(level.as_str(), "note" | "help" | "failure-note") {
        return None;
    }

    let message = msg["message"].as_str().unwrap_or("").to_string();
    let rendered = msg["rendered"].as_str().unwrap_or("").to_string();
    let code = msg["code"]["code"].as_str().map(String::from);

    // Use the primary span, falling back to the first span
    let span = msg["spans"].as_array().and_then(|spans| {
        spans
            .iter()
            .find(|s| s["is_primary"].as_bool() == Some(true))
            .or_else(|| spans.first())
    });

    let (file, line, col) = match span {
        Some(s) => (
            // rustc reports OS-native paths (backslashes on Windows); normalise to
            // forward slashes so `resolve_diag_file` / `apply_source_edits` match.
            s["file_name"].as_str().map(|f| f.replace('\\', "/")),
            s["line_start"].as_u64().map(|n| n as u32),
            s["column_start"].as_u64().map(|n| n as u32),
        ),
        None => (None, None, None),
    };

    Some(Diagnostic {
        level,
        message,
        rendered,
        file,
        line,
        col,
        code,
        fixes: extract_fixes(msg),
    })
}

/// Collect clippy's machine-applicable source edits from a message's spans and
/// its `children` (the "help" sub-diagnostics carry the suggestions). Only
/// `MachineApplicable` replacements are taken — safe to auto-apply.
fn extract_fixes(msg: &serde_json::Value) -> Vec<SpanEdit> {
    let mut out = Vec::new();
    let mut scan = |spans: &serde_json::Value| {
        if let Some(arr) = spans.as_array() {
            for s in arr {
                let Some(repl) = s["suggested_replacement"].as_str() else {
                    continue;
                };
                if s["suggestion_applicability"].as_str() != Some("MachineApplicable") {
                    continue;
                }
                if let (Some(file), Some(start), Some(end)) = (
                    s["file_name"].as_str(),
                    s["byte_start"].as_u64(),
                    s["byte_end"].as_u64(),
                ) {
                    out.push(SpanEdit {
                        // Normalise OS-native separators (see `parse_diagnostic`).
                        file: file.replace('\\', "/"),
                        start: start as usize,
                        end: end as usize,
                        replacement: repl.to_string(),
                    });
                }
            }
        }
    };
    scan(&msg["spans"]);
    if let Some(children) = msg["children"].as_array() {
        for c in children {
            scan(&c["spans"]);
        }
    }
    out
}
