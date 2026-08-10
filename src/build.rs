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

// ── Where diagnostics go when there is no console ────────────────────────────
// The app is a GUI-subsystem binary in every profile, so stdout/stderr have
// nowhere to land unless it was started from a terminal. These two cover both
// halves: adopt that terminal when it exists, and write crashes to a file for
// when it doesn't.

/// The crash log's path: `<per-user config dir>/crash.log`, falling back to the
/// temp dir when no config home resolves — a crash report nobody can find is no
/// better than none.
pub fn crash_log_path() -> PathBuf {
    crate::panels::mcu_module::registry::user_config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("crash.log")
}

/// `YYYY-MM-DD HH:MM:SS UTC` for a `SystemTime`. Pure — tested below.
///
/// Hand-rolled for the same reason as [`crate::activity::fmt_clock`]: the crate
/// has no `chrono` / `time` dependency. UTC, not local: a crash log is read
/// later, often on another machine, and an unlabelled local time is a trap.
/// The date conversion is Howard Hinnant's `civil_from_days`, simplified for
/// post-epoch input only.
pub fn fmt_utc(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, tod) = (secs / 86_400, secs % 86_400);

    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = era * 400 + yoe + u64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60,
    )
}

/// Install a panic hook that appends a full report to [`crash_log_path`], then
/// chains to whatever hook was already installed.
///
/// Without this a panic is INVISIBLE when the app was double-clicked: a
/// GUI-subsystem process has no stderr, so the window simply vanishes. Chaining
/// to the previous hook keeps the normal report for the case that does have a
/// console ([`attach_parent_console`]).
///
/// The hook must not panic itself — every write is best-effort and ignored on
/// failure; a second panic inside the hook aborts the process and loses the
/// original report, which is the one that mattered.
pub fn install_panic_logger() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_crash_report(info);
        previous(info);
    }));
}

/// Keep the log from growing without bound across many crashes. Small enough to
/// open in any editor, large enough for a good handful of reports.
const CRASH_LOG_MAX: u64 = 256 * 1024;

/// Render one report. Pure, so the format is testable without a real panic —
/// installing the process-wide hook from a test would make the whole (parallel)
/// suite write crash files.
fn crash_report(
    message: &str,
    location: &str,
    thread: &str,
    now: std::time::SystemTime,
    backtrace: &str,
) -> String {
    format!(
        "\n==== embedded_ide_0 panic ====\n\
         when:      {}\n\
         version:   {}\n\
         thread:    {thread}\n\
         location:  {location}\n\
         message:   {message}\n\
         backtrace:\n{backtrace}\n",
        fmt_utc(now),
        env!("CARGO_PKG_VERSION"),
    )
}

/// Append `report` to `path`, creating the folder and rotating an oversized log.
fn append_report(path: &Path, report: &str) {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Start fresh once the file gets large rather than trimming it: the newest
    // report is the interesting one, and truncating mid-file would corrupt it.
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > CRASH_LOG_MAX {
        let _ = std::fs::remove_file(path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(report.as_bytes());
    }
}

fn write_crash_report(info: &std::panic::PanicHookInfo<'_>) {
    // A `panic!("...")` payload is `&str` or `String` depending on whether it
    // was formatted; anything else (a non-string `panic_any`) has no text.
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned());
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_owned());
    let thread = std::thread::current()
        .name()
        .unwrap_or("<unnamed>")
        .to_owned();
    // `force_capture` ignores RUST_BACKTRACE: the user who double-clicked the
    // exe never set it, and they are exactly who this file is for.
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();

    let report = crash_report(
        &message,
        &location,
        &thread,
        std::time::SystemTime::now(),
        &backtrace,
    );
    append_report(&crash_log_path(), &report);
}

/// Adopt the console we were launched FROM, if there is one.
///
/// The counterpart of [`no_window`]: that one keeps children from making a
/// console, this one keeps *us* from needing our own. The binary is built with
/// `windows_subsystem = "windows"` in every profile, so Windows never allocates
/// a console for it — no black window at startup, in debug or release. The cost
/// is that `println!` / `eprintln!` / panic messages go nowhere. Attaching to
/// the parent's console buys them back for the case that wants them: started
/// from a terminal.
///
/// Started from Explorer / a shortcut there IS no parent console, `AttachConsole`
/// fails, and we stay silent — which is the whole point.
///
/// **Call this first thing in `main`, before anything prints.** A GUI-subsystem
/// process starts with no standard handles at all, so the console has to be
/// attached AND `CONOUT$` installed as stdout/stderr before Rust's `Stdout`
/// resolves a handle for the first time.
///
/// Known quirk, not a bug: `cmd.exe` does not wait for a GUI-subsystem program,
/// so it prints its next prompt immediately and our output lands underneath it.
/// Nothing in the process can prevent that.
pub fn attach_parent_console() {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::ptr::null_mut;

        const ATTACH_PARENT_PROCESS: u32 = u32::MAX; // (DWORD)-1
        const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
        const STD_ERROR_HANDLE: u32 = -12i32 as u32;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const OPEN_EXISTING: u32 = 3;

        unsafe extern "system" {
            fn AttachConsole(dwProcessId: u32) -> i32;
            fn CreateFileA(
                lpFileName: *const u8,
                dwDesiredAccess: u32,
                dwShareMode: u32,
                lpSecurityAttributes: *mut c_void,
                dwCreationDisposition: u32,
                dwFlagsAndAttributes: u32,
                hTemplateFile: *mut c_void,
            ) -> *mut c_void;
            fn SetStdHandle(nStdHandle: u32, hHandle: *mut c_void) -> i32;
        }

        unsafe {
            // No parent console (Explorer, a shortcut, the debugger) → nothing
            // to adopt. Deliberately silent: this is the common case.
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                return;
            }
            // Attaching alone is not enough — the process still has no std
            // handles. Open the console's own output device and install it.
            let conout = CreateFileA(
                c"CONOUT$".as_ptr() as *const u8,
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null_mut(),
                OPEN_EXISTING,
                0,
                null_mut(),
            );
            let invalid = usize::MAX as *mut c_void; // INVALID_HANDLE_VALUE
            if !conout.is_null() && conout != invalid {
                SetStdHandle(STD_OUTPUT_HANDLE, conout);
                SetStdHandle(STD_ERROR_HANDLE, conout);
            }
        }
    }
}

/// Apply `CREATE_NO_WINDOW` (Windows) to a command so spawning it does NOT flash
/// a console window. On a GUI/`windows_subsystem = "windows"` build every child
/// console process (cargo, rustup, rust-analyzer, …) otherwise pops a console
/// window that steals focus for a frame and vanishes — with flycheck firing on
/// every save that reads as the whole app "flickering" and the taskbar spawning
/// ghost instances. No-op on non-Windows. Returns the same `&mut Command` so it
/// chains inline: `no_window(Command::new("cargo")).args(...)`.
pub fn no_window(cmd: &mut Command) -> &mut Command {
    no_window_raw(cmd);
    // Point the MSVC linker/compiler at a VERIFIED toolchain (Windows): rustc and
    // cc-rs pick an install by little more than "does cl.exe exist", so a
    // partially-installed Visual Studio shadows a complete one and every host
    // build-script fails (LNK1104 msvcrt.lib / C1083 vcruntime.h). No-op when the
    // process already has LIB (a Developer prompt) or nothing complete is found.
    // See [`crate::msvc`].
    #[cfg(windows)]
    for (k, v) in crate::msvc::env_pairs() {
        cmd.env(k, v);
    }
    cmd
}

/// [`no_window`] without the MSVC environment injection — used by the MSVC probe
/// itself (which must not recurse into it) and anywhere the ambient environment
/// must be left untouched.
pub fn no_window_raw(cmd: &mut Command) -> &mut Command {
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

    // Missing MSVC host C-runtime libraries (an incomplete VS "Desktop C++"
    // install — e.g. only the `onecore` lib variant, no `lib\x64\`): every HOST
    // build-script / proc-macro fails to LINK with `LNK1104: cannot open file
    // 'msvcrt.lib'`. Surface the real cause + fix instead of a raw linker dump.
    let has = |s: &str| {
        stderr_text.contains(s) || result.diagnostics.iter().any(|d| d.rendered.contains(s))
    };
    let link_broken = (has("LNK1104") || has("LNK1181"))
        && (has("msvcrt") || has("libcmt") || has("libvcruntime") || has("libucrt"));
    // Same root cause on the COMPILER side: a crate with C code (ring, libusb…)
    // can't find the MSVC headers.
    let headers_broken = has("C1083") && (has("vcruntime.h") || has("corecrt.h"));
    if link_broken || headers_broken {
        return BuildState::Failed(
            "[MSVC_LIBS] The MSVC toolchain can't find its C-runtime libraries / headers \
             (LNK1104 'msvcrt.lib' or C1083 'vcruntime.h').\n\n\
             Rust links every build-script for the HOST with the MSVC toolchain, so \
             without it NOTHING builds.\n\n\
             -> Open the TOOLS tab and check \"MSVC build tools\": it reports which \
             Visual Studio installs are complete and can install the Build Tools for you.\n\n\
             Note: a PARTIAL Visual Studio (compiler present, `VC\\Tools\\MSVC\\<ver>\\lib\\x64\\` \
             or `include\\` missing) SHADOWS a complete one, because rustc picks an install by \
             whether `cl.exe` exists. Repair/complete that install — VS Installer -> Modify -> \
             workload \"Desktop development with C++\" (or components \"MSVC v143 … x64/x86 build \
             tools\" + a \"Windows 10/11 SDK\").\n\
             Tip: `cargo build` from a plain terminal fails the same way — it's the \
             toolchain, not the IDE."
                .to_string(),
        );
    }

    // The link overflowed the memory declared in memory.x. It arrives as an
    // ordinary `error` diagnostic, so without this it would land in the list as
    // a wall of linker arguments with the one useful line buried inside.
    // Checked before the `saw_build_finished` block: this failure DOES emit one.
    let linker_text = format!(
        "{stderr_text}\n{}",
        result
            .diagnostics
            .iter()
            .map(|d| d.rendered.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    if let Some(detail) = crate::failure_hint::flash_overflow(&linker_text) {
        return BuildState::Failed(crate::failure_hint::flash_full_message(&detail));
    }

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
            || stderr_text.contains("os error 28"); // POSIX    ENOSPC
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
mod crash_log_tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(secs: u64) -> String {
        fmt_utc(UNIX_EPOCH + Duration::from_secs(secs))
    }

    /// The date maths is the only part of the crash report that can be WRONG
    /// rather than merely missing — a mis-dated report sends you looking at the
    /// wrong session.
    #[test]
    fn utc_stamp_covers_epoch_leap_years_and_century_rules() {
        assert_eq!(at(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(at(86_399), "1970-01-01 23:59:59 UTC");
        assert_eq!(at(86_400), "1970-01-02 00:00:00 UTC");
        // 2000 is a leap year (divisible by 400) — the rule a naive `% 4` gets
        // right by accident and a naive `% 100` gets wrong.
        assert_eq!(at(951_782_400), "2000-02-29 00:00:00 UTC");
        // 2100 is NOT a leap year (divisible by 100, not 400).
        assert_eq!(at(4_107_542_400), "2100-03-01 00:00:00 UTC");
        // A known reference point: 2026-08-07 12:34:56 UTC.
        assert_eq!(at(1_786_106_096), "2026-08-07 12:34:56 UTC");
    }

    /// Every field a reader needs must actually be in the file. Tested through
    /// the pure renderer rather than a real panic: installing the process-wide
    /// hook from a test would make the whole (parallel) suite write crash files,
    /// and mutating `APPDATA` to redirect the path would race other tests.
    #[test]
    fn a_report_carries_everything_needed_to_diagnose() {
        let r = crash_report(
            "index out of bounds: the len is 3 but the index is 7",
            "src/app/mcu_panel.rs:412:9",
            "main",
            UNIX_EPOCH + Duration::from_secs(1_786_106_096),
            "   0: embedded_ide_0::app::foo\n   1: core::panicking",
        );
        assert!(r.contains("2026-08-07 12:34:56 UTC"), "{r}");
        assert!(r.contains(env!("CARGO_PKG_VERSION")), "{r}");
        assert!(r.contains("thread:    main"), "{r}");
        assert!(r.contains("src/app/mcu_panel.rs:412:9"), "{r}");
        assert!(r.contains("the len is 3 but the index is 7"), "{r}");
        assert!(r.contains("embedded_ide_0::app::foo"), "{r}");
        // Leading blank line + banner, so consecutive reports stay separable.
        assert!(r.starts_with("\n==== embedded_ide_0 panic ===="), "{r}");
    }

    #[test]
    fn reports_append_and_the_log_rotates_when_oversized() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("nested").join("crash.log");

        append_report(&log, "FIRST\n");
        append_report(&log, "SECOND\n");
        let both = std::fs::read_to_string(&log).unwrap();
        assert!(both.contains("FIRST") && both.contains("SECOND"), "{both}");
        // The parent folder is created on demand — the config dir may not exist
        // yet on a first run that crashes early.
        assert!(log.parent().unwrap().is_dir());

        // Past the cap the file starts over, so the newest report is never lost
        // to a half-trimmed predecessor.
        std::fs::write(&log, "x".repeat(CRASH_LOG_MAX as usize + 1)).unwrap();
        append_report(&log, "AFTER ROTATION\n");
        let rotated = std::fs::read_to_string(&log).unwrap();
        assert_eq!(rotated, "AFTER ROTATION\n");
    }
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
