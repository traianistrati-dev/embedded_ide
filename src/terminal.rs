//! Built-in command console (bottom-panel "Terminal" tab).
//!
//! A **streaming command runner** — NOT a full PTY. Each command is a fresh
//! shell spawn (`powershell -NoProfile -NonInteractive -Command <cmd>` on
//! Windows, `$SHELL -c <cmd>` on macOS / Linux) whose stdout+stderr are read
//! line-by-line on background threads and appended to a scrollback buffer, so
//! long-running commands (`cargo build`, `probe-rs`, `git`) show output live.
//! A Stop button kills the child; Up/Down recall history; ANSI SGR colour codes
//! are parsed for display. It can't answer interactive prompts mid-run (no TTY).
//!
//! The command text is the user's own, in their own shell's dialect — the
//! console does not translate between them.
//!
//! Mirrors the reader-thread + throttled-repaint + `AtomicBool` stop pattern of
//! [`crate::serial`].

use eframe::egui;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Max scrollback lines kept; older lines are dropped.
const MAX_LINES: usize = 5_000;
const REPAINT_EVERY: Duration = Duration::from_millis(33);

/// Where a scrollback line came from — tints it in the view.
#[derive(Clone, Copy, PartialEq)]
pub enum LineKind {
    /// A command the user ran (echoed with a `> ` prompt).
    Input,
    Stdout,
    Stderr,
    /// IDE-generated notice (exit code, killed, cwd change, errors).
    Notice,
}

/// One scrollback line + its already-parsed colour spans (from ANSI SGR).
pub struct TermLine {
    pub kind: LineKind,
    /// `(text, colour)` runs. `colour == None` → use the kind's default colour.
    pub spans: Vec<(String, Option<egui::Color32>)>,
}

#[derive(Default)]
pub struct TerminalState {
    pub lines: Vec<TermLine>,
    /// `true` while a command's child process is still running.
    pub running: bool,
}

impl TerminalState {
    pub(crate) fn push(&mut self, kind: LineKind, spans: Vec<(String, Option<egui::Color32>)>) {
        self.lines.push(TermLine { kind, spans });
        if self.lines.len() > MAX_LINES {
            let excess = self.lines.len() - MAX_LINES;
            self.lines.drain(..excess);
        }
    }

    pub(crate) fn push_plain(&mut self, kind: LineKind, text: impl Into<String>) {
        self.push(kind, vec![(text.into(), None)]);
    }

    /// The last `n` lines as plain text (colour spans flattened), oldest first.
    /// For post-mortems on a console a subprocess wrote into — a crash is always
    /// at the END, and a long session's scrollback isn't worth walking.
    pub(crate) fn tail_text(&self, n: usize) -> String {
        let start = self.lines.len().saturating_sub(n);
        self.lines[start..]
            .iter()
            .map(|l| l.spans.iter().map(|(t, _)| t.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The console UI + process state (owned by `AppIde.terminal`).
pub struct TerminalConsole {
    pub input: String,
    /// Working directory commands run in (default: the generated workspace).
    pub cwd: PathBuf,
    /// Previously-run commands (newest last); `history_pos` walks them with Up/Down.
    history: Vec<String>,
    history_pos: Option<usize>,
    pub state: Arc<Mutex<TerminalState>>,
    /// Per-run stop flag; set on Stop / a new run, so an old reader bails out.
    stop: Option<Arc<AtomicBool>>,
    /// The running child, kept so Stop can kill it.
    child: Arc<Mutex<Option<std::process::Child>>>,
}

impl Default for TerminalConsole {
    fn default() -> Self {
        Self {
            input: String::new(),
            cwd: std::env::temp_dir().join("embedded_ide_0_check"),
            history: Vec::new(),
            history_pos: None,
            state: Arc::new(Mutex::new(TerminalState::default())),
            stop: None,
            child: Arc::new(Mutex::new(None)),
        }
    }
}

impl TerminalConsole {
    pub fn is_running(&self) -> bool {
        self.state.lock().unwrap().running
    }

    pub fn clear(&mut self) {
        self.state.lock().unwrap().lines.clear();
    }

    /// Recall the previous / next history entry into `input` (Up = older).
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let pos = match self.history_pos {
            None => self.history.len() - 1, // first Up → newest entry
            Some(p) => p.saturating_sub(1), // subsequent Ups → older
        };
        self.history_pos = Some(pos);
        self.input = self.history[pos].clone();
    }

    pub fn history_next(&mut self) {
        let Some(pos) = self.history_pos else {
            return;
        };
        if pos + 1 < self.history.len() {
            self.history_pos = Some(pos + 1);
            self.input = self.history[pos + 1].clone();
        } else {
            // Past the newest entry → back to an empty prompt.
            self.history_pos = None;
            self.input.clear();
        }
    }

    /// Run the current `input` as a command. No-op while one is already running
    /// or the input is blank.
    pub fn run(&mut self, ctx: &egui::Context) {
        let cmd = self.input.trim().to_string();
        if cmd.is_empty() || self.is_running() {
            return;
        }
        self.input.clear();
        self.history_pos = None;
        if self.history.last().map(|h| h.as_str()) != Some(cmd.as_str()) {
            self.history.push(cmd.clone());
        }

        // Echo the command with a prompt.
        {
            let mut s = self.state.lock().unwrap();
            s.push_plain(LineKind::Input, format!("> {cmd}"));
        }

        // `cd` / `Set-Location` is handled client-side: each command is a fresh
        // spawn, so a child's `cd` wouldn't persist. Intercept it and move our
        // own `cwd` instead, so the console behaves like a real shell session.
        if let Some(target) = parse_cd(&cmd) {
            self.change_dir(&target);
            return;
        }

        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(Arc::clone(&stop));

        let (shell, shell_args) = shell_invocation();
        let mut command = Command::new(&shell);
        command
            .args(&shell_args)
            .arg(&cmd)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::build::no_window(&mut command);

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                let mut s = self.state.lock().unwrap();
                s.push_plain(
                    LineKind::Notice,
                    format!("[error] couldn't launch {shell}: {e}"),
                );
                self.stop = None;
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        {
            let mut s = self.state.lock().unwrap();
            s.running = true;
        }
        *self.child.lock().unwrap() = Some(child);

        // One reader thread per pipe, plus a waiter that clears `running` and
        // prints the exit status once BOTH pipes hit EOF (child exited).
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0)); // pipes finished
        if let Some(out) = stdout {
            spawn_reader(
                out,
                LineKind::Stdout,
                Arc::clone(&self.state),
                Arc::clone(&stop),
                ctx.clone(),
                Arc::clone(&done),
            );
        } else {
            done.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(err) = stderr {
            spawn_reader(
                err,
                LineKind::Stderr,
                Arc::clone(&self.state),
                Arc::clone(&stop),
                ctx.clone(),
                Arc::clone(&done),
            );
        } else {
            done.fetch_add(1, Ordering::Relaxed);
        }

        // Waiter thread: once both pipes are done, reap the child + report exit.
        let state = Arc::clone(&self.state);
        let child_slot = Arc::clone(&self.child);
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            // Wait for both reader threads to signal EOF.
            while done.load(Ordering::Relaxed) < 2 {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let status = child_slot.lock().unwrap().as_mut().map(|c| c.wait());
            let mut s = state.lock().unwrap();
            match status {
                Some(Ok(st)) if st.success() => s.push_plain(LineKind::Notice, "[exit 0]"),
                Some(Ok(st)) => s.push_plain(
                    LineKind::Notice,
                    format!(
                        "[exit {}]",
                        st.code()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "signal".into())
                    ),
                ),
                _ => {}
            }
            s.running = false;
            *child_slot.lock().unwrap() = None;
            drop(s);
            ctx2.request_repaint();
        });
    }

    /// Kill the running command (Stop button / Ctrl+C).
    pub fn stop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
        let mut s = self.state.lock().unwrap();
        if s.running {
            s.push_plain(LineKind::Notice, "[stopped]");
            s.running = false;
        }
    }

    /// Apply a client-side `cd <target>` (absolute or relative to `cwd`).
    fn change_dir(&mut self, target: &str) {
        let candidate = if target == "~" {
            dirs_home()
        } else {
            let p = PathBuf::from(target);
            if p.is_absolute() { p } else { self.cwd.join(p) }
        };
        let mut s = self.state.lock().unwrap();
        match candidate.canonicalize() {
            Ok(abs) if abs.is_dir() => {
                // Strip Windows verbatim `\\?\` prefix for a tidy display.
                self.cwd = abs;
                let shown = self.cwd.to_string_lossy().replace(r"\\?\", "");
                s.push_plain(LineKind::Notice, format!("[cwd] {shown}"));
            }
            _ => s.push_plain(
                LineKind::Notice,
                format!("[error] no such directory: {target}"),
            ),
        }
    }
}

/// The shell to run one command in: `(program, flags before the command)`.
///
/// Windows gets PowerShell with `-NoProfile` (a user profile can print banners
/// that pollute the scrollback, and it costs startup time on every command) and
/// `-NonInteractive` (this console has no TTY, so a prompt would hang forever).
///
/// The unixes get `$SHELL -c`, so a zsh / fish user types their own dialect
/// rather than the one this file happens to assume. `/bin/sh` is the fallback,
/// because `$SHELL` is simply absent when the app is launched from a desktop
/// entry rather than a login shell. There is no `-NonInteractive` equivalent —
/// `stdin(Stdio::null())` at the call site is what stops a prompt from hanging.
fn shell_invocation() -> (String, Vec<String>) {
    if cfg!(target_os = "windows") {
        (
            "powershell".to_string(),
            ["-NoProfile", "-NonInteractive", "-Command"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )
    } else {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/bin/sh".to_string());
        (shell, vec!["-c".to_string()])
    }
}

/// Display name of the shell commands run in — for the UI, so the user knows
/// which dialect the console expects. Just the file name (`zsh`, not
/// `/bin/zsh`), since the path adds nothing at a glance.
pub fn shell_name() -> String {
    let (shell, _) = shell_invocation();
    std::path::Path::new(&shell)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(shell)
}

/// Home directory (best-effort, Windows `USERPROFILE` / Unix `HOME`).
fn dirs_home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Recognise a whole-command `cd <dir>` (no pipes / `;` / `&`) → the target.
/// Returns `None` for anything else.
///
/// PowerShell's aliases are accepted only ON Windows. `sl` in particular MUST
/// NOT be treated as a cd on the unixes: it is a real program there, and
/// swallowing `sl foo` would make the console lie about what it ran.
///
/// A bare `cd` means "go home" on the unixes, so it maps to `~` — which
/// [`TerminalConsole::change_dir`] already resolves. PowerShell's bare `Set-
/// Location` does the same, so the rule is shared.
fn parse_cd(cmd: &str) -> Option<String> {
    let c = cmd.trim();
    if c.contains('|') || c.contains(';') || c.contains('&') {
        return None;
    }
    // Bare "go home" forms.
    if c == "cd" || (cfg!(target_os = "windows") && (c == "Set-Location" || c == "sl")) {
        return Some("~".to_string());
    }
    let mut rest = c.strip_prefix("cd ");
    if cfg!(target_os = "windows") {
        rest = rest
            .or_else(|| c.strip_prefix("Set-Location "))
            .or_else(|| c.strip_prefix("sl "));
    }
    let target = rest?.trim().trim_matches('"').trim_matches('\'').trim();
    if target.is_empty() {
        // `cd ""` — treat as home rather than an error, same as a bare `cd`.
        Some("~".to_string())
    } else {
        Some(target.to_string())
    }
}

/// Background line reader: reads `pipe` to EOF, parsing each line's ANSI SGR
/// colours, appending to the scrollback (throttled repaint). Bumps `done` on
/// EOF so the waiter can reap the child. (Shared with the RTT console.)
pub(crate) fn spawn_reader(
    pipe: impl std::io::Read + Send + 'static,
    kind: LineKind,
    state: Arc<Mutex<TerminalState>>,
    stop: Arc<AtomicBool>,
    ctx: egui::Context,
    done: Arc<std::sync::atomic::AtomicUsize>,
) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut last_repaint = Instant::now() - REPAINT_EVERY;
        let mut buf = Vec::new();
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            buf.clear();
            // Read up to a newline (bytes, so invalid UTF-8 doesn't abort).
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let raw = String::from_utf8_lossy(&buf);
                    // Strip the line terminator (LF, and the CR of a CRLF pair)
                    // FIRST, then — for progress bars that rewrite the line with
                    // embedded `\r` — keep only the last rewritten segment. Doing
                    // it in this order matters: on Windows every line ends `\r\n`,
                    // so splitting on `\r` before trimming would leave just "\n".
                    let terminated = raw.trim_end_matches('\n').trim_end_matches('\r');
                    let line = terminated.rsplit('\r').next().unwrap_or(terminated);
                    let spans = parse_ansi(line);
                    {
                        let mut s = state.lock().unwrap();
                        s.push(kind, spans);
                    }
                    if last_repaint.elapsed() >= REPAINT_EVERY {
                        ctx.request_repaint();
                        last_repaint = Instant::now();
                    } else {
                        ctx.request_repaint_after(REPAINT_EVERY);
                    }
                }
                Err(_) => break,
            }
        }
        done.fetch_add(1, Ordering::Relaxed);
        ctx.request_repaint();
    });
}

/// The line's visible text, with every ANSI escape sequence removed — for the
/// consoles that render plain `String`s (the Flash tab's log) rather than the
/// parsed spans. A tool that colours its output would otherwise show its escape
/// codes as literal garbage around every word.
pub(crate) fn strip_ansi(line: &str) -> String {
    if !line.contains('\u{1b}') {
        return line.to_owned();
    }
    parse_ansi(line).into_iter().map(|(t, _)| t).collect()
}

/// Split a line into `(text, colour)` runs by parsing ANSI SGR (`\x1b[…m`)
/// sequences. Non-SGR escape sequences (cursor moves, `\x1b[K`, …) are dropped.
/// Only the common colour attributes are honoured; unknown ones reset to default.
fn parse_ansi(line: &str) -> Vec<(String, Option<egui::Color32>)> {
    let mut spans: Vec<(String, Option<egui::Color32>)> = Vec::new();
    let mut cur = String::new();
    let mut color: Option<egui::Color32> = None;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            // Escape — find the sequence. We only interpret CSI `…m` (SGR).
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                // Read until the final byte (a letter).
                let mut j = i + 2;
                while j < bytes.len() && !bytes[j].is_ascii_alphabetic() {
                    j += 1;
                }
                if j < bytes.len() {
                    let final_byte = bytes[j];
                    let params = &line[i + 2..j];
                    if final_byte == b'm' {
                        // Flush the current run before switching colour.
                        if !cur.is_empty() {
                            spans.push((std::mem::take(&mut cur), color));
                        }
                        color = sgr_color(params, color);
                    }
                    // Skip the whole escape sequence (SGR or otherwise).
                    i = j + 1;
                    continue;
                }
            }
            // Lone ESC or unrecognised — skip the ESC byte.
            i += 1;
            continue;
        }
        // Regular UTF-8 char.
        let ch_len = utf8_len(bytes[i]);
        if let Ok(s) = std::str::from_utf8(&bytes[i..(i + ch_len).min(bytes.len())]) {
            cur.push_str(s);
        }
        i += ch_len;
    }
    if !cur.is_empty() {
        spans.push((cur, color));
    }
    if spans.is_empty() {
        spans.push((String::new(), None));
    }
    spans
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// Map an SGR parameter list (`"1;32"`) to a display colour. `prev` is kept for
/// unhandled attributes; `0` (reset) / `39` (default fg) clear the colour.
fn sgr_color(params: &str, prev: Option<egui::Color32>) -> Option<egui::Color32> {
    let mut color = prev;
    for p in params.split(';') {
        match p.trim() {
            "" | "0" | "39" => color = None,
            "30" | "90" => color = Some(egui::Color32::from_gray(120)),
            "31" | "91" => color = Some(egui::Color32::from_rgb(230, 100, 90)),
            "32" | "92" => color = Some(egui::Color32::from_rgb(90, 200, 110)),
            "33" | "93" => color = Some(egui::Color32::from_rgb(220, 180, 60)),
            "34" | "94" => color = Some(egui::Color32::from_rgb(90, 150, 235)),
            "35" | "95" => color = Some(egui::Color32::from_rgb(200, 120, 220)),
            "36" | "96" => color = Some(egui::Color32::from_rgb(80, 200, 220)),
            "37" | "97" => color = Some(egui::Color32::from_gray(210)),
            _ => {} // bold/underline/etc. — ignore, keep colour
        }
    }
    color
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cd_variants() {
        assert_eq!(parse_cd("cd src"), Some("src".into()));
        assert_eq!(parse_cd("cd \"my dir\""), Some("my dir".into()));
        assert_eq!(parse_cd("cargo build"), None);
        assert_eq!(parse_cd("cd src && ls"), None); // compound → let the shell run it
        // Bare `cd` = go home, on every platform.
        assert_eq!(parse_cd("cd"), Some("~".into()));
    }

    /// PowerShell's cd aliases are Windows-only. `sl` is a REAL program on the
    /// unixes (and `Set-Location` is not a shell builtin there), so intercepting
    /// either would make the console silently not run what the user typed.
    #[test]
    fn powershell_cd_aliases_are_windows_only() {
        let expected = |v: Option<&str>| v.map(str::to_string);
        if cfg!(target_os = "windows") {
            assert_eq!(parse_cd("Set-Location .."), expected(Some("..")));
            assert_eq!(parse_cd("sl src"), expected(Some("src")));
            assert_eq!(parse_cd("sl"), expected(Some("~")));
        } else {
            assert_eq!(parse_cd("Set-Location .."), None);
            assert_eq!(parse_cd("sl src"), None);
            assert_eq!(parse_cd("sl"), None);
        }
    }

    /// The invocation must always end with a "run this string" flag, or the
    /// command would be taken as the name of a script FILE.
    #[test]
    fn shell_invocation_takes_a_command_string() {
        let (shell, args) = shell_invocation();
        assert!(!shell.trim().is_empty());
        let last = args.last().expect("no flags before the command");
        if cfg!(target_os = "windows") {
            assert_eq!(shell, "powershell");
            assert_eq!(last, "-Command");
            // A profile banner would pollute the scrollback; a prompt would hang.
            assert!(args.iter().any(|a| a == "-NoProfile"));
            assert!(args.iter().any(|a| a == "-NonInteractive"));
        } else {
            assert_eq!(last, "-c");
            assert_eq!(args.len(), 1);
        }
    }

    #[test]
    fn parse_ansi_splits_colours() {
        // "\x1b[32mgreen\x1b[0m plain"
        let spans = parse_ansi("\u{1b}[32mgreen\u{1b}[0m plain");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].0, "green");
        assert!(spans[0].1.is_some());
        assert_eq!(spans[1].0, " plain");
        assert_eq!(spans[1].1, None);
    }

    #[test]
    fn parse_ansi_strips_non_sgr() {
        // A cursor-clear `\x1b[K` must be dropped, text preserved.
        let spans = parse_ansi("abc\u{1b}[Kdef");
        let text: String = spans.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "abcdef");
    }

    #[test]
    fn parse_ansi_plain_line() {
        let spans = parse_ansi("just text");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, "just text");
        assert_eq!(spans[0].1, None);
    }

    /// The CRLF/progress `\r` handling done in the reader (kept as a free fn
    /// here so it's unit-testable without a live pipe).
    fn strip_line(raw: &str) -> &str {
        let terminated = raw.trim_end_matches('\n').trim_end_matches('\r');
        terminated.rsplit('\r').next().unwrap_or(terminated)
    }

    #[test]
    fn strip_line_handles_crlf_and_progress() {
        // Windows CRLF must keep the text (the bug: `rsplit('\r')` first → "\n").
        assert_eq!(strip_line("Compiling nb v1.1.0\r\n"), "Compiling nb v1.1.0");
        // Plain LF.
        assert_eq!(strip_line("done\n"), "done");
        // Progress bar rewriting the line → last segment wins.
        assert_eq!(strip_line("10%\r50%\r100%\n"), "100%");
    }
}
