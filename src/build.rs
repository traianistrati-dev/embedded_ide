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

/// Apply `CREATE_NO_WINDOW` (Windows) to a command so spawning it does NOT flash
/// a console window. On a GUI/`windows_subsystem = "windows"` build every child
/// console process (cargo, rustup, rust-analyzer, …) otherwise pops a console
/// window that steals focus for a frame and vanishes — with flycheck firing on
/// every save that reads as the whole app "flickering" and the taskbar spawning
/// ghost instances. No-op on non-Windows. Returns the same `&mut Command` so it
/// chains inline: `no_window(Command::new("cargo")).args(...)`.
pub fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

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

/// An identifier-rename suggestion (naming-convention lints like
/// `non_camel_case_types`). rustc marks these `MaybeIncorrect` for a plain splice
/// because only renaming *every* reference is correct — so the Clippy tab applies
/// it via rust-analyzer's project-wide rename (`textDocument/rename`), the same
/// path as Ctrl+R, rather than a raw byte splice.
#[derive(Clone, Debug)]
pub struct RenameFix {
    /// Relative path of the identifier's definition, e.g. `"src/pins/foo.rs"`.
    pub file: String,
    /// Byte offset of the identifier (for the GENERATED-block lock check).
    pub byte: usize,
    /// 1-based line / column of the identifier (rustc coords; LSP wants -1).
    pub line: u32,
    pub col: u32,
    /// The current identifier, e.g. `"HOLD_THRESHOLD_15"` — used to verify a
    /// queued rename's position is still valid before firing it (Apply-all).
    pub old_name: String,
    /// The suggested new name, e.g. `"HoldThreshold15"`.
    pub new_name: String,
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
    /// A project-wide rename suggestion (naming-convention lint), applied via RA.
    pub rename: Option<RenameFix>,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        self.level == "error"
    }
    pub fn is_warning(&self) -> bool {
        self.level == "warning"
    }
    /// True when clippy offers a machine-applicable (raw-splice) fix for this lint.
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
    activity: Arc<Mutex<crate::activity::ActivityLog>>,
    // `false` = `cargo check` (fast); `true` = `cargo build --release` (full
    // optimized build). Both parse the same JSON diagnostics.
    release: bool,
) {
    *state.lock().unwrap() = BuildState::Building;
    ctx.request_repaint();

    thread::spawn(move || {
        let sub = if release { "build" } else { "check" };
        let next = run_cargo(&project_dir, &target, sub, release, &activity);
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
    activity: Arc<Mutex<crate::activity::ActivityLog>>,
) {
    *state.lock().unwrap() = BuildState::Building;
    ctx.request_repaint();

    thread::spawn(move || {
        let next = run_cargo(&project_dir, &target, "clippy", false, &activity);
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
        let _ = no_window(&mut Command::new("cargo"))
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
fn run_cargo(
    dir: &Path,
    target: &str,
    subcommand: &str,
    release: bool,
    activity: &Arc<Mutex<crate::activity::ActivityLog>>,
) -> BuildState {
    let kind = match subcommand {
        "clippy" => "Clippy",
        "build" => "Build (cargo build --release)",
        _ => "Build (cargo check)",
    };
    let mut rec = crate::activity::Recorder::new(kind);

    // ── Step 1: install target if needed ────────────────────────────────────
    // `rustup target add` is idempotent — exits 0 immediately when already
    // installed, so the overhead on subsequent builds is negligible.
    let t = std::time::Instant::now();
    let ensured = ensure_target(target);
    rec.cmd_phase(
        "rustup target add",
        format!("rustup target add {target}"),
        t.elapsed(),
        ensured.as_ref().ok().map(|_| 0),
    );
    if let Err(e) = ensured {
        activity.lock().unwrap().push(rec.finish());
        return BuildState::Failed(format!(
            "Could not install target `{target}` via rustup: {e}\n\n\
             Make sure `rustup` is in PATH or install the target manually:\n\
             rustup target add {target}"
        ));
    }

    // ── Step 2: cargo check / clippy ─────────────────────────────────────────
    //
    // `--workspace` matters for CLIPPY specifically: clippy lints only the
    // packages named on the command line, and builds everything else with plain
    // rustc — so lints from an extracted library crate would never appear.
    // (`check` already reports them: `--cap-lints allow` applies to registry
    // dependencies, not to workspace path deps.) Harmless when there is no
    // workspace section — the root package is then the only member.
    let cargo_started = std::time::Instant::now();
    let mut args: Vec<&str> = vec![subcommand, "--workspace"];
    if release {
        args.push("--release");
    }
    args.push("--message-format=json");
    args.push("--color=never");
    let cargo_cmd = format!("cargo {}", args.join(" "));
    let mut child = match no_window(&mut Command::new("cargo"))
        .current_dir(dir)
        .args(&args)
        .stdout(Stdio::piped())
        // Capture stderr so we can detect disk-full and other fatal OS errors.
        // Without this, cargo crashes silently (no build-finished JSON) and the
        // IDE either reports a false success or a confusing bare "build failed".
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            rec.cmd_phase(kind, cargo_cmd, cargo_started.elapsed(), None);
            activity.lock().unwrap().push(rec.finish());
            return BuildState::Failed(format!(
                "Could not launch `cargo`: {e}\n\n\
                 Make sure the Rust toolchain is installed and `cargo` is in PATH.\n\
                 Install from https://rustup.rs"
            ));
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

    let exit = child.wait().ok().and_then(|s| s.code());
    rec.cmd_phase(kind, cargo_cmd, cargo_started.elapsed(), exit);
    activity.lock().unwrap().push(rec.finish());
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
    let status = no_window(&mut Command::new("rustup"))
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
        rename: extract_rename(msg),
    })
}

/// Detect an identifier-rename suggestion (naming-convention lints like
/// `non_camel_case_types`, `non_snake_case`, `non_upper_case_globals`,
/// `clippy::upper_case_acronyms`, …) so the Clippy tab can offer a project-wide
/// rename (RA `textDocument/rename`) instead of an unsafe single-site splice.
///
/// Heuristic (no hard-coded lint list, so future naming lints are covered too): a
/// span qualifies when it is `MaybeIncorrect` (the `MachineApplicable` ones are
/// real splices handled by `extract_fixes`) **and** both the original text and the
/// suggested replacement are a single valid Rust identifier that differ. The
/// original is read from the span's own `text` highlight — `foo()` → `bar` is
/// rejected because `foo()` isn't an identifier, guarding against treating an
/// arbitrary replacement as a rename.
fn extract_rename(msg: &serde_json::Value) -> Option<RenameFix> {
    let consider = |s: &serde_json::Value| -> Option<RenameFix> {
        let new_name = s["suggested_replacement"].as_str()?;
        if new_name.is_empty() || !is_rust_ident(new_name) {
            return None;
        }
        // MachineApplicable spans are plain splices (handled by extract_fixes).
        if s["suggestion_applicability"].as_str() != Some("MaybeIncorrect") {
            return None;
        }
        let original = span_highlighted_text(s);
        if !is_rust_ident(&original) || original == new_name {
            return None;
        }
        Some(RenameFix {
            file: s["file_name"].as_str()?.replace('\\', "/"),
            byte: s["byte_start"].as_u64()? as usize,
            line: s["line_start"].as_u64()? as u32,
            col: s["column_start"].as_u64()? as u32,
            old_name: original,
            new_name: new_name.to_string(),
        })
    };
    // The rename suggestion lives in a child "help" span; fall back to top spans.
    if let Some(children) = msg["children"].as_array() {
        for c in children {
            if let Some(arr) = c["spans"].as_array() {
                for s in arr {
                    if let Some(rn) = consider(s) {
                        return Some(rn);
                    }
                }
            }
        }
    }
    msg["spans"].as_array()?.iter().find_map(consider)
}

/// `true` when `s` is a single valid Rust identifier (ASCII ident; rejects empty,
/// leading digit, paths, calls, whitespace, etc.).
fn is_rust_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The substring a diagnostic span highlights — its `text[]` entries spliced by
/// the per-line `highlight_start`/`highlight_end` (1-based char columns). This is
/// the original source the suggestion replaces (e.g. the identifier being
/// renamed), recovered without reading the file.
fn span_highlighted_text(s: &serde_json::Value) -> String {
    let Some(lines) = s["text"].as_array() else {
        return String::new();
    };
    let mut out = String::new();
    for (i, ln) in lines.iter().enumerate() {
        let text = ln["text"].as_str().unwrap_or("");
        let hs = ln["highlight_start"].as_u64().unwrap_or(1) as usize;
        let he = ln["highlight_end"].as_u64().unwrap_or(1) as usize;
        // Columns are 1-based char indices; slice on char boundaries.
        let chars: Vec<char> = text.chars().collect();
        let lo = hs.saturating_sub(1).min(chars.len());
        let hi = he.saturating_sub(1).min(chars.len()).max(lo);
        if i > 0 {
            out.push('\n');
        }
        out.extend(&chars[lo..hi]);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a child "help" span carrying a rename suggestion: `orig` is the
    /// indented identifier line, highlighted over the identifier itself.
    fn rename_child(file: &str, orig_ident: &str, new_name: &str, app: &str) -> serde_json::Value {
        let line = format!("    {orig_ident} = 0x00,");
        let hs = 5; // 1-based column of the first ident char (after 4 spaces)
        let he = hs + orig_ident.chars().count();
        serde_json::json!({
            "level": "help",
            "message": "rename suggestion",
            "spans": [{
                "file_name": file,
                "is_primary": true,
                "line_start": 11, "column_start": 5, "byte_start": 127, "byte_end": 132,
                "suggested_replacement": new_name,
                "suggestion_applicability": app,
                "text": [{ "text": line, "highlight_start": hs, "highlight_end": he }]
            }]
        })
    }

    /// A `non_camel_case_types` message (Windows backslash path) → a `RenameFix`
    /// with normalised path, 1-based coords and the suggested new name.
    #[test]
    fn extract_rename_picks_up_naming_lint() {
        let msg = serde_json::json!({
            "level": "warning",
            "message": "variant `HOLD_THRESHOLD_15` should have an upper camel case name",
            "code": { "code": "non_camel_case_types" },
            "spans": [{
                "file_name": "src\\pins\\utils\\mw_radar_mmwave.rs",
                "is_primary": true,
                "line_start": 11, "column_start": 5, "byte_start": 127, "byte_end": 132,
                "suggested_replacement": serde_json::Value::Null
            }],
            "children": [rename_child(
                "src\\pins\\utils\\mw_radar_mmwave.rs",
                "HOLD_THRESHOLD_15",
                "HoldThreshold15",
                "MaybeIncorrect",
            )]
        });
        let rn = extract_rename(&msg).expect("naming lint yields a rename");
        assert_eq!(rn.file, "src/pins/utils/mw_radar_mmwave.rs");
        assert_eq!((rn.line, rn.col), (11, 5));
        assert_eq!(rn.byte, 127);
        assert_eq!(rn.old_name, "HOLD_THRESHOLD_15");
        assert_eq!(rn.new_name, "HoldThreshold15");
        // It is NOT a machine-applicable splice (MaybeIncorrect), so no SpanEdit.
        assert!(extract_fixes(&msg).is_empty());
    }

    /// The heuristic also catches `clippy::upper_case_acronyms` (DELAY → Delay)
    /// without any hard-coded lint list.
    #[test]
    fn extract_rename_catches_upper_case_acronyms() {
        let msg = serde_json::json!({
            "level": "warning",
            "message": "name `DELAY` contains a capitalized acronym",
            "code": { "code": "clippy::upper_case_acronyms" },
            "spans": [{ "file_name": "src/foo.rs", "is_primary": true,
                        "line_start": 11, "column_start": 5, "byte_start": 127, "byte_end": 132,
                        "suggested_replacement": serde_json::Value::Null }],
            "children": [rename_child("src/foo.rs", "DELAY", "Delay", "MaybeIncorrect")]
        });
        let rn = extract_rename(&msg).expect("acronym lint yields a rename");
        assert_eq!(rn.new_name, "Delay");
        assert_eq!(rn.file, "src/foo.rs");
    }

    /// Guard: a `MaybeIncorrect` suggestion whose original text isn't a bare
    /// identifier (e.g. `foo()` → `bar`) must NOT be treated as a rename.
    #[test]
    fn extract_rename_rejects_non_identifier_original() {
        let mut child = rename_child("src/foo.rs", "x", "bar", "MaybeIncorrect");
        // Overwrite the highlighted original to `foo()` (a call, not an ident).
        child["spans"][0]["text"][0]["text"] = serde_json::json!("    foo() ;");
        child["spans"][0]["text"][0]["highlight_start"] = serde_json::json!(5);
        child["spans"][0]["text"][0]["highlight_end"] = serde_json::json!(10);
        let msg = serde_json::json!({
            "level": "warning",
            "code": { "code": "clippy::some_lint" },
            "spans": [],
            "children": [child]
        });
        assert!(extract_rename(&msg).is_none());
    }

    /// A non-naming lint (unused_imports) never produces a rename.
    #[test]
    fn extract_rename_ignores_other_lints() {
        let msg = serde_json::json!({
            "level": "warning",
            "code": { "code": "unused_imports" },
            "spans": [{ "file_name": "src/main.rs", "is_primary": true,
                        "line_start": 1, "column_start": 1, "byte_start": 0, "byte_end": 5 }],
            "children": []
        });
        assert!(extract_rename(&msg).is_none());
    }
}
