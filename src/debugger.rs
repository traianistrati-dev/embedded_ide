//! On-target debugger — a DAP client over `probe-rs dap-server` (TCP).
//!
//! probe-rs ships a Debug Adapter Protocol server (the same one its VS Code
//! extension uses). Speaking DAP gets source-level debugging — breakpoints by
//! file:line, step over/in/out, stack traces with source locations, locals and
//! registers — without linking probe-rs as a library dependency.
//!
//! Session pipeline (`Debugger::start`):
//!  1. `cargo build --release` (streamed into the console — shared with RTT).
//!  2. Spawn `probe-rs dap-server --port <free port>`, connect over TCP.
//!  3. DAP handshake: `initialize` → `launch` (flash + reset) → on the
//!     `initialized` event send every breakpoint + `configurationDone`.
//!  4. Event-driven from there: a `stopped` event chains `threads` →
//!     `stackTrace` → `scopes` → `variables`, filling [`DebugState`]; the UI
//!     issues `continue`/`next`/`stepIn`/`stepOut`/`pause` and breakpoint
//!     updates directly over the same socket.
//!
//! Framing is LSP-style `Content-Length: N\r\n\r\n{json}` — see `read_message`.
//! Requests are correlated to responses by `seq` via the `pending` map.

use crate::build::no_window;
use crate::rtt::cargo_build_streamed;
use crate::terminal::{LineKind, TerminalState, spawn_reader};
use eframe::egui;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ── Shared state (UI ↔ reader thread) ─────────────────────────────────────────

#[derive(Clone, Default, PartialEq)]
pub enum DebugPhase {
    #[default]
    Idle,
    /// `cargo build --release` runs.
    Building,
    /// Server spawned; initialize/launch (incl. flashing) in progress.
    Launching,
    /// The target is executing.
    Running,
    /// Halted — the inner string is the DAP stop reason ("breakpoint", "step").
    Stopped(String),
    Error(String),
}

/// One stack frame (top-first). `file_rel` is workspace-relative (`src/…`)
/// when the frame's source lives in the generated project; frames without
/// source (HAL internals, asm) keep `None` and are shown greyed.
#[derive(Clone)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub file_rel: Option<String>,
    pub line: u32,
}

/// One row of the Locals / Registers panes.
#[derive(Clone)]
pub struct VarRow {
    pub name: String,
    pub value: String,
    pub ty: Option<String>,
}

/// One watch expression + its most recent evaluation against the selected
/// frame. `error` = the last `evaluate` failed (out of scope / unsupported);
/// `value` then holds the reason. Expressions persist across halts; the value
/// refreshes on each stop and on frame selection.
#[derive(Clone)]
pub struct WatchRow {
    pub expr: String,
    pub value: String,
    pub ty: Option<String>,
    pub error: bool,
}

/// The current hover-to-evaluate request: the identifier under the pointer and
/// its value once the target answers. `generation` discards stale responses
/// when the pointer moved to another identifier before the reply arrived.
/// `value: None` = still awaiting the `evaluate` response (no tooltip yet).
#[derive(Clone)]
pub struct HoverEval {
    pub generation: u64,
    pub expr: String,
    pub value: Option<String>,
    pub ty: Option<String>,
}

#[derive(Default)]
pub struct DebugState {
    pub phase: DebugPhase,
    pub thread_id: Option<i64>,
    pub stack: Vec<Frame>,
    pub locals: Vec<VarRow>,
    pub registers: Vec<VarRow>,
    /// User watch expressions + their latest values (see [`WatchRow`]). Owned
    /// here (not on `Debugger`) so the reader thread can re-evaluate them on a
    /// halt. NOT cleared when the target runs — only the values go stale.
    pub watches: Vec<WatchRow>,
    /// Hover-to-evaluate for the identifier under the editor pointer (see
    /// [`HoverEval`]); `None` when not hovering an identifier / not halted.
    pub hover: Option<HoverEval>,
    /// Set by the reader when the target halts somewhere navigable; the UI
    /// consumes it (opens the file, scrolls, tints the line).
    pub nav: Option<(String, u32)>,
    /// The frame whose scopes are currently shown (highlighted in the list).
    pub sel_frame: Option<i64>,
    /// What probe-rs answered for each requested breakpoint, per file:
    /// `rel path → requested line → status`. Filled from every `setBreakpoints`
    /// response; empty outside a session (nothing has been asked yet).
    pub bp_status: BTreeMap<String, BTreeMap<u32, BpStatus>>,
}

/// probe-rs's verdict on ONE requested breakpoint. A red dot in the gutter only
/// means "the IDE asked for it" — this is whether the target actually got it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BpStatus {
    /// The debugger armed it. `false` = the line has no code of its own (very
    /// common in the optimised `--release` build the Debug tab produces), or the
    /// core ran out of hardware breakpoint comparators.
    pub verified: bool,
    /// Where it ACTUALLY landed, when probe-rs moved it to the nearest line that
    /// has code. `None` when it stayed put.
    pub moved_to: Option<u32>,
    /// probe-rs's own explanation, when the response carried one.
    pub message: Option<String>,
}

// ── Wire (writer half + request bookkeeping) ──────────────────────────────────

/// What a pending request's response should be routed to.
///
/// NOT `Copy` since `Breakpoints` carries the request's path + lines: the DAP
/// response lists results positionally, with no source path of its own, so the
/// question has to travel with the answer.
#[derive(Clone, PartialEq)]
enum Pending {
    Initialize,
    Launch,
    Threads,
    StackTrace,
    Scopes,
    VarsLocals,
    VarsRegisters,
    /// `evaluate` for watch expression `i` (index into `DebugState::watches`).
    Watch(usize),
    /// `evaluate` for a hover tooltip; the `u64` is the hover generation so a
    /// stale reply (pointer already moved on) is dropped.
    Hover(u64),
    /// `setBreakpoints` for one file: `(rel path, the lines we asked for, in
    /// request order)`. The response's array is positional against that list.
    Breakpoints(String, Vec<u32>),
    Other,
}

/// The write half of the DAP socket + seq/pending bookkeeping. Cloned into the
/// reader thread so it can fire follow-up requests (event-driven chains).
#[derive(Clone)]
struct Wire {
    writer: Arc<Mutex<Option<TcpStream>>>,
    seq: Arc<AtomicI64>,
    pending: Arc<Mutex<HashMap<i64, Pending>>>,
}

impl Wire {
    fn request(&self, command: &str, arguments: Value, kind: Pending) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let msg = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        self.pending.lock().unwrap().insert(seq, kind);
        if let Some(stream) = self.writer.lock().unwrap().as_mut() {
            let body = msg.to_string();
            let _ = write!(stream, "Content-Length: {}\r\n\r\n{body}", body.len());
            let _ = stream.flush();
        }
    }
}

/// Read one `Content-Length`-framed DAP message; `None` on EOF / bad frame.
fn read_message(stream: &mut TcpStream) -> Option<Value> {
    // Headers: byte-by-byte until the blank line (no BufReader — a buffered
    // reader would eat bytes of the next message between calls).
    let mut header = Vec::new();
    let mut b = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        match stream.read(&mut b) {
            Ok(1) => header.push(b[0]),
            _ => return None,
        }
        if header.len() > 4096 {
            return None;
        }
    }
    let text = String::from_utf8_lossy(&header);
    let len: usize = text
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length:"))
        .and_then(|v| v.trim().parse().ok())?;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

// ── Session config ────────────────────────────────────────────────────────────

/// Everything the reader thread needs to drive the handshake.
struct SessionCfg {
    project_dir: PathBuf,
    chip: String,
    /// The `--probe VID:PID[:Serial]` selector for the DAP `launch` `probe`
    /// field, or `None` to let probe-rs auto-pick (see [`crate::probe`]).
    probe: Option<String>,
    elf: PathBuf,
    /// Breakpoints at session start (rel path → 1-based lines). Later edits
    /// go over the wire directly (`Debugger::sync_breakpoints`).
    breakpoints: BTreeMap<String, Vec<u32>>,
}

// ── The debugger ──────────────────────────────────────────────────────────────

/// Owned by `AppIde.debugger`. All methods are UI-thread safe; the heavy
/// lifting happens on the orchestrator + reader threads.
pub struct Debugger {
    pub state: Arc<Mutex<DebugState>>,
    /// Build progress + DAP `output` events (defmt/RTT prints land here too).
    pub console: Arc<Mutex<TerminalState>>,
    wire: Wire,
    /// The `probe-rs dap-server` child (killed on Stop / app exit).
    server: Arc<Mutex<Option<Child>>>,
    /// Cargo child during the build phase (killable) — reuses the RTT helper.
    build_child: Arc<Mutex<Option<Child>>>,
    stop: Option<Arc<AtomicBool>>,
    cfg: Arc<Mutex<Option<Arc<SessionCfg>>>>,
    /// Debug-tab pane split boundaries as fractions of the row width (three
    /// separators → four panes: Console | Call stack | Variables | Watch).
    /// UI-only, single-threaded. Draggable; see `debug_tab::split_widths`.
    pub pane_splits: [f32; 3],
    /// The Watch pane's "add expression" input text. UI-only.
    pub watch_draft: String,
    /// Monotonic generation for hover-evaluate requests (drops stale replies).
    hover_gen: AtomicU64,
}

impl Default for Debugger {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(DebugState::default())),
            console: Arc::new(Mutex::new(TerminalState::default())),
            wire: Wire {
                writer: Arc::new(Mutex::new(None)),
                seq: Arc::new(AtomicI64::new(1)),
                pending: Arc::new(Mutex::new(HashMap::new())),
            },
            server: Arc::new(Mutex::new(None)),
            build_child: Arc::new(Mutex::new(None)),
            stop: None,
            cfg: Arc::new(Mutex::new(None)),
            // Console widest; stack / variables / watch share the rest.
            pane_splits: [0.34, 0.56, 0.78],
            watch_draft: String::new(),
            hover_gen: AtomicU64::new(0),
        }
    }
}

impl Debugger {
    pub fn phase(&self) -> DebugPhase {
        self.state.lock().unwrap().phase.clone()
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.phase(), DebugPhase::Idle | DebugPhase::Error(_))
    }

    /// Take the pending halt-location navigation (UI consumes it once).
    pub fn take_nav(&self) -> Option<(String, u32)> {
        self.state.lock().unwrap().nav.take()
    }

    pub fn clear_console(&mut self) {
        self.console.lock().unwrap().lines.clear();
    }

    /// Start a session: build, flash and halt-ready debug. `breakpoints` is
    /// the current rel-path → lines map. No-op while a session is active.
    pub fn start(
        &mut self,
        project_dir: PathBuf,
        target: String,
        chip: String,
        // The `--probe VID:PID[:Serial]` selector, or `None` for auto-select.
        probe: Option<String>,
        breakpoints: BTreeMap<String, Vec<u32>>,
        ctx: egui::Context,
    ) {
        if self.is_busy() {
            return;
        }
        {
            let mut st = self.state.lock().unwrap();
            // Watch EXPRESSIONS survive a restart (user intent); their values
            // reset and refill on the first halt of the new session.
            let watches = st
                .watches
                .iter()
                .map(|w| WatchRow {
                    expr: w.expr.clone(),
                    value: String::new(),
                    ty: None,
                    error: false,
                })
                .collect();
            *st = DebugState {
                phase: DebugPhase::Building,
                watches,
                ..Default::default()
            };
        }
        self.wire.pending.lock().unwrap().clear();
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(Arc::clone(&stop));

        let console = Arc::clone(&self.console);
        let state = Arc::clone(&self.state);
        let wire = self.wire.clone();
        let server_slot = Arc::clone(&self.server);
        let build_slot = Arc::clone(&self.build_child);
        let cfg_slot = Arc::clone(&self.cfg);
        thread::spawn(move || {
            let end = run_session(
                &project_dir,
                &target,
                &chip,
                probe,
                breakpoints,
                &console,
                &state,
                &wire,
                &server_slot,
                &build_slot,
                &cfg_slot,
                &stop,
                &ctx,
            );
            if let Err(e) = end {
                if !stop.load(Ordering::Relaxed) {
                    console
                        .lock()
                        .unwrap()
                        .push_plain(LineKind::Notice, format!("[error] {e}"));
                    state.lock().unwrap().phase = DebugPhase::Error(e);
                }
                kill_server(&server_slot);
                *wire.writer.lock().unwrap() = None;
            }
            ctx.request_repaint();
        });
    }

    /// End the session: polite `disconnect`, then kill the server.
    pub fn stop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Relaxed);
        }
        self.wire.request(
            "disconnect",
            json!({"terminateDebuggee": false}),
            Pending::Other,
        );
        if let Some(child) = self.build_child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
        // Give the server a moment to honour the disconnect, then kill it.
        let server = Arc::clone(&self.server);
        let writer = Arc::clone(&self.wire.writer);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(400));
            kill_server(&server);
            *writer.lock().unwrap() = None;
        });
        {
            let mut st = self.state.lock().unwrap();
            st.phase = DebugPhase::Idle;
            st.stack.clear();
            st.locals.clear();
            st.registers.clear();
        }
        self.console
            .lock()
            .unwrap()
            .push_plain(LineKind::Notice, "[debug session ended]");
    }

    /// Synchronous teardown for app exit — an orphaned dap-server would keep
    /// the probe locked for the next start (no polite disconnect, just kill).
    pub fn kill_now(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(child) = self.build_child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
        kill_server(&self.server);
        *self.wire.writer.lock().unwrap() = None;
    }

    // ── Execution controls (enabled by the UI per phase) ─────────────────────

    fn thread_id(&self) -> i64 {
        self.state.lock().unwrap().thread_id.unwrap_or(0)
    }

    pub fn continue_run(&self) {
        self.wire.request(
            "continue",
            json!({"threadId": self.thread_id()}),
            Pending::Other,
        );
        self.mark_running();
    }

    pub fn pause(&self) {
        self.wire.request(
            "pause",
            json!({"threadId": self.thread_id()}),
            Pending::Other,
        );
    }

    pub fn step_over(&self) {
        self.wire.request(
            "next",
            json!({"threadId": self.thread_id()}),
            Pending::Other,
        );
        self.mark_running();
    }

    pub fn step_in(&self) {
        self.wire.request(
            "stepIn",
            json!({"threadId": self.thread_id()}),
            Pending::Other,
        );
        self.mark_running();
    }

    pub fn step_out(&self) {
        self.wire.request(
            "stepOut",
            json!({"threadId": self.thread_id()}),
            Pending::Other,
        );
        self.mark_running();
    }

    /// Optimistic phase flip — the next `stopped` event corrects it.
    fn mark_running(&self) {
        let mut st = self.state.lock().unwrap();
        st.phase = DebugPhase::Running;
        st.stack.clear();
        st.locals.clear();
        st.registers.clear();
        st.sel_frame = None;
        st.hover = None;
    }

    /// Show another frame's variables (stack-row click). Also raises the nav
    /// so the editor jumps to that frame's source line.
    pub fn select_frame(&self, frame: &Frame) {
        {
            let mut st = self.state.lock().unwrap();
            st.sel_frame = Some(frame.id);
            if let Some(rel) = &frame.file_rel {
                st.nav = Some((rel.clone(), frame.line));
            }
        }
        self.wire
            .request("scopes", json!({"frameId": frame.id}), Pending::Scopes);
        // The new frame's scope changes what every watch resolves to.
        eval_watches(&self.wire, &self.state, frame.id);
    }

    /// Add a watch expression (from the editor's "Add to Watch" or the Watch
    /// pane's input). Deduplicated; evaluated immediately when halted on a frame.
    pub fn add_watch(&self, expr: String) {
        let expr = expr.trim().to_string();
        if expr.is_empty() {
            return;
        }
        let (idx, frame) = {
            let mut st = self.state.lock().unwrap();
            if st.watches.iter().any(|w| w.expr == expr) {
                return; // already watching it
            }
            st.watches.push(WatchRow {
                expr: expr.clone(),
                value: String::new(),
                ty: None,
                error: false,
            });
            (st.watches.len() - 1, st.sel_frame)
        };
        // Evaluate now if a session is halted on a frame; otherwise it fills in
        // at the next stop.
        if let Some(fid) = frame {
            if self.wire.writer.lock().unwrap().is_some() {
                self.wire.request(
                    "evaluate",
                    json!({"expression": expr, "frameId": fid, "context": "watch"}),
                    Pending::Watch(idx),
                );
            }
        }
    }

    pub fn remove_watch(&self, i: usize) {
        let mut st = self.state.lock().unwrap();
        if i < st.watches.len() {
            st.watches.remove(i);
        }
    }

    /// Evaluate `expr` for a hover tooltip (`context:"hover"`) against the
    /// selected frame. Debounced: a no-op while the same expression is already
    /// the current hover, so at most one request fires per identifier hovered.
    /// Only meaningful while halted on a frame with a live session.
    pub fn hover_eval(&self, expr: String) {
        let expr = expr.trim().to_string();
        if expr.is_empty() {
            self.clear_hover();
            return;
        }
        let fid = {
            let st = self.state.lock().unwrap();
            if !matches!(st.phase, DebugPhase::Stopped(_)) {
                return;
            }
            // Same expression already shown / in flight → nothing to do.
            if st.hover.as_ref().map(|h| h.expr.as_str()) == Some(expr.as_str()) {
                return;
            }
            st.sel_frame
        };
        let Some(fid) = fid else {
            return;
        };
        if self.wire.writer.lock().unwrap().is_none() {
            return;
        }
        let generation = self.hover_gen.fetch_add(1, Ordering::Relaxed) + 1;
        self.state.lock().unwrap().hover = Some(HoverEval {
            generation,
            expr: expr.clone(),
            value: None,
            ty: None,
        });
        self.wire.request(
            "evaluate",
            json!({"expression": expr, "frameId": fid, "context": "hover"}),
            Pending::Hover(generation),
        );
    }

    /// Drop any hover tooltip (pointer left an identifier / the editor, or the
    /// session isn't halted).
    pub fn clear_hover(&self) {
        let mut st = self.state.lock().unwrap();
        if st.hover.is_some() {
            st.hover = None;
        }
    }

    /// Push the current breakpoint set of one file to the live session (call
    /// on every gutter toggle; no-op when no session is up).
    pub fn sync_breakpoints(&self, rel_path: &str, lines: &[u32]) {
        let Some(cfg) = self.cfg.lock().unwrap().clone() else {
            return;
        };
        if self.wire.writer.lock().unwrap().is_none() {
            return;
        }
        send_breakpoints(&self.wire, &cfg.project_dir, rel_path, lines);
    }
}

/// probe-rs answers an unresolved `evaluate` with a `success:true` result whose
/// text is a placeholder marker — `<invalid expression "x">` (name isn't a
/// register / in-scope local / static / SVD peripheral on THIS probe-rs),
/// `<not found …>`, or `<optimized out>` (release build dropped it). Detect
/// those so the UI can grey them out with a short reason instead of showing the
/// raw marker. Returns `(display_text, is_unresolved)`.
fn classify_eval_result(value: &str) -> (String, bool) {
    let v = value.trim();
    if v.starts_with("<invalid expression") {
        ("not in scope / unsupported".to_string(), true)
    } else if v.starts_with("<not found") {
        ("not found in this frame".to_string(), true)
    } else if v.contains("optimized out") {
        ("optimized out".to_string(), true)
    } else {
        (value.to_string(), false)
    }
}

/// Fire a DAP `evaluate` (context "watch") for every watch expression against
/// `frame_id`; each response routes to `Pending::Watch(i)` → fills
/// `DebugState::watches[i]`. Called on every halt and on frame selection.
fn eval_watches(wire: &Wire, state: &Arc<Mutex<DebugState>>, frame_id: i64) {
    let exprs: Vec<String> = state
        .lock()
        .unwrap()
        .watches
        .iter()
        .map(|w| w.expr.clone())
        .collect();
    for (i, expr) in exprs.iter().enumerate() {
        wire.request(
            "evaluate",
            json!({"expression": expr, "frameId": frame_id, "context": "watch"}),
            Pending::Watch(i),
        );
    }
}

/// `setBreakpoints` for one source file (abs path = workspace + rel).
fn send_breakpoints(wire: &Wire, project_dir: &Path, rel_path: &str, lines: &[u32]) {
    let abs = project_dir.join(rel_path);
    let bps: Vec<Value> = lines.iter().map(|l| json!({"line": l})).collect();
    wire.request(
        "setBreakpoints",
        json!({
            "source": { "path": abs.to_string_lossy() },
            "breakpoints": bps,
        }),
        Pending::Breakpoints(rel_path.to_owned(), lines.to_vec()),
    );
}

fn kill_server(server: &Arc<Mutex<Option<Child>>>) {
    if let Some(mut child) = server.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

// ── Orchestrator ──────────────────────────────────────────────────────────────

/// Build, spawn the server, connect, start the reader, send `initialize`.
#[allow(clippy::too_many_arguments)]
fn run_session(
    project_dir: &Path,
    target: &str,
    chip: &str,
    probe: Option<String>,
    breakpoints: BTreeMap<String, Vec<u32>>,
    console: &Arc<Mutex<TerminalState>>,
    state: &Arc<Mutex<DebugState>>,
    wire: &Wire,
    server_slot: &Arc<Mutex<Option<Child>>>,
    build_slot: &Arc<Mutex<Option<Child>>>,
    cfg_slot: &Arc<Mutex<Option<Arc<SessionCfg>>>>,
    stop: &Arc<AtomicBool>,
    ctx: &egui::Context,
) -> Result<(), String> {
    // ── 1. Build ──────────────────────────────────────────────────────────────
    let Some(elf) = cargo_build_streamed(project_dir, target, console, build_slot, stop, ctx)?
    else {
        return Ok(()); // user stopped mid-build
    };
    state.lock().unwrap().phase = DebugPhase::Launching;

    // ── 2. Server ─────────────────────────────────────────────────────────────
    let port = free_port();
    console.lock().unwrap().push_plain(
        LineKind::Input,
        format!("> probe-rs dap-server --port {port}"),
    );
    ctx.request_repaint();
    let mut server = no_window(&mut Command::new("probe-rs"))
        .current_dir(project_dir)
        .args(["dap-server", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "probe-rs not found in PATH.\n\
                 Install it with:  cargo install probe-rs-tools\n\
                 (or download from https://probe.rs)"
                    .to_string()
            } else {
                format!("could not launch probe-rs dap-server: {e}")
            }
        })?;
    // The server's own log lines → console (its errors are the best clue when
    // a probe / chip problem aborts the session).
    let done = Arc::new(AtomicUsize::new(0));
    if let Some(out) = server.stdout.take() {
        spawn_reader(
            out,
            LineKind::Notice,
            Arc::clone(console),
            Arc::clone(stop),
            ctx.clone(),
            Arc::clone(&done),
        );
    }
    if let Some(err) = server.stderr.take() {
        spawn_reader(
            err,
            LineKind::Stderr,
            Arc::clone(console),
            Arc::clone(stop),
            ctx.clone(),
            Arc::clone(&done),
        );
    }
    *server_slot.lock().unwrap() = Some(server);

    // ── 3. Connect (the server needs a moment to listen) ──────────────────────
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut socket = None;
    for _ in 0..50 {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        match TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
            Ok(s) => {
                socket = Some(s);
                break;
            }
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
    let socket = socket.ok_or("could not connect to probe-rs dap-server (port timeout)")?;
    let read_half = socket
        .try_clone()
        .map_err(|e| format!("socket clone failed: {e}"))?;
    *wire.writer.lock().unwrap() = Some(socket);

    let cfg = Arc::new(SessionCfg {
        project_dir: project_dir.to_path_buf(),
        chip: chip.to_string(),
        probe: probe.filter(|s| !s.is_empty()),
        elf,
        breakpoints,
    });
    *cfg_slot.lock().unwrap() = Some(Arc::clone(&cfg));

    // ── 4. Reader thread drives the handshake from here ──────────────────────
    spawn_dap_reader(
        read_half,
        cfg,
        Arc::clone(state),
        Arc::clone(console),
        wire.clone(),
        Arc::clone(server_slot),
        Arc::clone(stop),
        ctx.clone(),
    );
    wire.request(
        "initialize",
        json!({
            "clientID": "embedded-ide",
            "clientName": "Embedded IDE",
            "adapterID": "probe-rs",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "pathFormat": "path",
            "supportsProgressReporting": false,
        }),
        Pending::Initialize,
    );
    Ok(())
}

/// An unused localhost port (bind to 0, read back, release).
fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(50_999)
}

// ── Reader (event loop) ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn spawn_dap_reader(
    mut socket: TcpStream,
    cfg: Arc<SessionCfg>,
    state: Arc<Mutex<DebugState>>,
    console: Arc<Mutex<TerminalState>>,
    wire: Wire,
    server_slot: Arc<Mutex<Option<Child>>>,
    stop: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    thread::spawn(move || {
        while let Some(msg) = read_message(&mut socket) {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match msg["type"].as_str() {
                Some("response") => {
                    handle_response(&msg, &cfg, &state, &console, &wire);
                }
                Some("event") => {
                    handle_event(&msg, &cfg, &state, &console, &wire);
                }
                _ => {}
            }
            ctx.request_repaint();
        }
        // Socket closed: session over (server exit / disconnect / error).
        if !stop.load(Ordering::Relaxed) {
            // A dap-server that PANICKED — or that refused to open the probe —
            // closes the socket exactly like a clean disconnect does. So before
            // calling it "ended", look for the real failure in what the server
            // printed; otherwise the user gets a calm "[debug session ended]"
            // after a wall of `<unknown>` frames or a WinUSB complaint.
            let crash = crate::rtt::probe_rs_failure(&console.lock().unwrap());
            let mut st = state.lock().unwrap();
            if !matches!(st.phase, DebugPhase::Error(_) | DebugPhase::Idle) {
                match crash {
                    Some(msg) => {
                        st.phase = DebugPhase::Error(msg);
                        console.lock().unwrap().push_plain(
                            LineKind::Stderr,
                            "[probe-rs failed — the session died with it]",
                        );
                    }
                    None => {
                        st.phase = DebugPhase::Idle;
                        console
                            .lock()
                            .unwrap()
                            .push_plain(LineKind::Notice, "[debug session ended]");
                    }
                }
            }
        }
        // The verdicts describe the session that just ended — keeping them would
        // mark rows "not armed" while nothing is even attached.
        state.lock().unwrap().bp_status.clear();
        kill_server(&server_slot);
        *wire.writer.lock().unwrap() = None;
        ctx.request_repaint();
    });
}

fn handle_response(
    msg: &Value,
    cfg: &Arc<SessionCfg>,
    state: &Arc<Mutex<DebugState>>,
    console: &Arc<Mutex<TerminalState>>,
    wire: &Wire,
) {
    let req_seq = msg["request_seq"].as_i64().unwrap_or(-1);
    let kind = wire
        .pending
        .lock()
        .unwrap()
        .remove(&req_seq)
        .unwrap_or(Pending::Other);
    let ok = msg["success"].as_bool().unwrap_or(false);

    if !ok {
        let err = msg["message"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| msg["body"]["error"]["format"].as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("{} failed", msg["command"].as_str().unwrap_or("request")));
        // A watch that can't be resolved (out of scope, unsupported expression)
        // is normal — show it on the row, don't spam the console.
        if let Pending::Watch(i) = kind {
            let mut st = state.lock().unwrap();
            if let Some(row) = st.watches.get_mut(i) {
                row.value = err;
                row.ty = None;
                row.error = true;
            }
            return;
        }
        // A hover over a non-evaluable token just shows no tooltip.
        if let Pending::Hover(g) = kind {
            let mut st = state.lock().unwrap();
            if st.hover.as_ref().map(|h| h.generation) == Some(g) {
                st.hover = None;
            }
            return;
        }
        console
            .lock()
            .unwrap()
            .push_plain(LineKind::Stderr, format!("[dap] {err}"));
        // A failed launch is fatal; anything else just logs.
        if kind == Pending::Launch {
            // The DAP layer only says "cancelled" — the REASON (a probe that
            // won't open, a probe-rs crash) is in the server's own output that
            // came before it.
            let real = {
                let c = console.lock().unwrap();
                crate::rtt::probe_rs_failure(&c)
            };
            state.lock().unwrap().phase = DebugPhase::Error(real.unwrap_or(err));
        }
        return;
    }

    match kind {
        Pending::Initialize => {
            // Capabilities received → launch (flash + reset the target).
            // probe-rs's DAP `launch` accepts an optional `probe` selector
            // (VID:PID[:Serial]); omit the key entirely to keep auto-select.
            let mut launch = json!({
                "cwd": cfg.project_dir.to_string_lossy(),
                "chip": cfg.chip,
                "connectUnderReset": false,
                "flashingConfig": {
                    "flashingEnabled": true,
                    "haltAfterReset": false,
                },
                "coreConfigs": [{
                    "coreIndex": 0,
                    "programBinary": cfg.elf.to_string_lossy(),
                    "rttEnabled": true,
                }],
                "consoleLogLevel": "Console",
            });
            if let Some(sel) = &cfg.probe {
                launch["probe"] = json!(sel);
            }
            wire.request("launch", launch, Pending::Launch);
        }
        Pending::Launch => {
            let mut st = state.lock().unwrap();
            if !matches!(st.phase, DebugPhase::Stopped(_)) {
                st.phase = DebugPhase::Running;
            }
            console
                .lock()
                .unwrap()
                .push_plain(LineKind::Notice, "[launched — target running]");
        }
        Pending::Threads => {
            let tid = msg["body"]["threads"]
                .as_array()
                .and_then(|t| t.first())
                .and_then(|t| t["id"].as_i64())
                .unwrap_or(0);
            state.lock().unwrap().thread_id = Some(tid);
            wire.request(
                "stackTrace",
                json!({"threadId": tid, "startFrame": 0, "levels": 24}),
                Pending::StackTrace,
            );
        }
        Pending::StackTrace => {
            let frames: Vec<Frame> = msg["body"]["stackFrames"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|f| Frame {
                            id: f["id"].as_i64().unwrap_or(0),
                            name: f["name"].as_str().unwrap_or("?").to_string(),
                            file_rel: f["source"]["path"]
                                .as_str()
                                .and_then(|p| rel_of(p, &cfg.project_dir)),
                            line: f["line"].as_u64().unwrap_or(0) as u32,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let top = frames.first().cloned();
            {
                let mut st = state.lock().unwrap();
                // Navigate to the topmost frame that has source in the project.
                if let Some(f) = frames.iter().find(|f| f.file_rel.is_some()) {
                    st.nav = Some((f.file_rel.clone().unwrap(), f.line));
                }
                st.sel_frame = top.as_ref().map(|f| f.id);
                st.stack = frames;
            }
            if let Some(f) = top {
                wire.request("scopes", json!({"frameId": f.id}), Pending::Scopes);
                // Refresh every watch against the (new) top frame on each halt.
                eval_watches(wire, state, f.id);
            }
        }
        Pending::Scopes => {
            if let Some(scopes) = msg["body"]["scopes"].as_array() {
                for s in scopes {
                    let name = s["name"].as_str().unwrap_or("");
                    let vref = s["variablesReference"].as_i64().unwrap_or(0);
                    if vref <= 0 {
                        continue;
                    }
                    let kind = if name.to_lowercase().contains("register") {
                        Pending::VarsRegisters
                    } else if name.to_lowercase().contains("local") {
                        Pending::VarsLocals
                    } else {
                        continue; // statics: skipped (often huge)
                    };
                    wire.request("variables", json!({"variablesReference": vref}), kind);
                }
            }
        }
        Pending::VarsLocals | Pending::VarsRegisters => {
            let rows: Vec<VarRow> = msg["body"]["variables"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|v| VarRow {
                            name: v["name"].as_str().unwrap_or("?").to_string(),
                            value: v["value"].as_str().unwrap_or("").to_string(),
                            ty: v["type"].as_str().map(str::to_owned),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut st = state.lock().unwrap();
            if kind == Pending::VarsLocals {
                st.locals = rows;
            } else {
                st.registers = rows;
            }
        }
        Pending::Watch(i) => {
            let (value, unresolved) =
                classify_eval_result(msg["body"]["result"].as_str().unwrap_or(""));
            // A placeholder marker has no meaningful type.
            let ty = if unresolved {
                None
            } else {
                msg["body"]["type"].as_str().map(str::to_owned)
            };
            let mut st = state.lock().unwrap();
            if let Some(row) = st.watches.get_mut(i) {
                row.value = value;
                row.ty = ty;
                row.error = unresolved;
            }
        }
        Pending::Hover(g) => {
            let (value, unresolved) =
                classify_eval_result(msg["body"]["result"].as_str().unwrap_or(""));
            let ty = msg["body"]["type"].as_str().map(str::to_owned);
            let mut st = state.lock().unwrap();
            // Only the current hover generation, and never show a tooltip for an
            // unresolved value (hover is for a quick peek at REAL values).
            if st.hover.as_ref().map(|h| h.generation) == Some(g) {
                if unresolved {
                    st.hover = None;
                } else if let Some(h) = &mut st.hover {
                    h.value = Some(value);
                    h.ty = ty;
                }
            }
        }
        Pending::Breakpoints(rel, asked) => {
            let answers = msg["body"]["breakpoints"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let statuses = bp_statuses(&asked, &answers);
            let unarmed: Vec<u32> = statuses
                .iter()
                .filter(|(_, s)| !s.verified)
                .map(|(l, _)| *l)
                .collect();
            // One console line when something did NOT take — otherwise the red
            // dot is the only feedback and it lies (see the Debug tab's list).
            if !unarmed.is_empty() {
                let list = unarmed
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                console.lock().unwrap().push_plain(
                    LineKind::Stderr,
                    format!(
                        "[breakpoints] {rel}: {} of {} armed — line(s) {list} could not be set \
                         (no code there in the optimised --release build, or the core is out of \
                         hardware breakpoints)",
                        asked.len() - unarmed.len(),
                        asked.len()
                    ),
                );
            }
            // This response is the whole truth for that FILE: replace its map so
            // a removed breakpoint can't leave a stale row behind.
            state.lock().unwrap().bp_status.insert(rel, statuses);
        }
        Pending::Other => {}
    }
}

/// Zip the lines we asked for with the DAP `setBreakpoints` answers, which come
/// back positionally (the response carries no line of its own for a breakpoint
/// the debugger refused). A missing answer counts as unverified — silence is
/// not confirmation.
fn bp_statuses(asked: &[u32], answers: &[Value]) -> BTreeMap<u32, BpStatus> {
    asked
        .iter()
        .enumerate()
        .map(|(i, &line)| {
            let a = answers.get(i);
            (
                line,
                BpStatus {
                    verified: a.and_then(|a| a["verified"].as_bool()).unwrap_or(false),
                    // Only a DIFFERENT line is a relocation worth showing.
                    moved_to: a
                        .and_then(|a| a["line"].as_u64())
                        .map(|l| l as u32)
                        .filter(|l| *l != line),
                    message: a
                        .and_then(|a| a["message"].as_str())
                        .filter(|m| !m.trim().is_empty())
                        .map(str::to_owned),
                },
            )
        })
        .collect()
}

fn handle_event(
    msg: &Value,
    cfg: &Arc<SessionCfg>,
    state: &Arc<Mutex<DebugState>>,
    console: &Arc<Mutex<TerminalState>>,
    wire: &Wire,
) {
    match msg["event"].as_str() {
        Some("initialized") => {
            // Configuration window: breakpoints first, then configurationDone.
            for (rel, lines) in &cfg.breakpoints {
                if !lines.is_empty() {
                    send_breakpoints(wire, &cfg.project_dir, rel, lines);
                }
            }
            wire.request("configurationDone", json!({}), Pending::Other);
        }
        Some("stopped") => {
            let reason = msg["body"]["reason"].as_str().unwrap_or("stopped");
            {
                let mut st = state.lock().unwrap();
                st.phase = DebugPhase::Stopped(reason.to_string());
                if let Some(tid) = msg["body"]["threadId"].as_i64() {
                    st.thread_id = Some(tid);
                }
            }
            wire.request("threads", json!({}), Pending::Threads);
        }
        Some("continued") => {
            let mut st = state.lock().unwrap();
            st.phase = DebugPhase::Running;
            st.stack.clear();
            st.locals.clear();
            st.registers.clear();
            st.sel_frame = None;
            st.hover = None;
        }
        Some("output") => {
            let text = msg["body"]["output"].as_str().unwrap_or("");
            let kind = match msg["body"]["category"].as_str() {
                Some("stderr") => LineKind::Stderr,
                Some("console") => LineKind::Notice,
                _ => LineKind::Stdout, // stdout / RTT prints
            };
            let mut c = console.lock().unwrap();
            for line in text.lines().filter(|l| !l.is_empty()) {
                c.push_plain(kind, line);
            }
        }
        Some("terminated") | Some("exited") => {
            let mut st = state.lock().unwrap();
            if !matches!(st.phase, DebugPhase::Error(_)) {
                st.phase = DebugPhase::Idle;
            }
            console
                .lock()
                .unwrap()
                .push_plain(LineKind::Notice, "[target terminated]");
        }
        _ => {}
    }
}

/// Map an absolute DWARF/DAP source path back to a workspace-relative one
/// (`src/main.rs`). Case-insensitive on the prefix — Windows reports the same
/// dir as `C:\Users\…\Temp` or `C:\Users\…\temp` depending on the producer.
fn rel_of(path: &str, project_dir: &Path) -> Option<String> {
    let norm = path.replace('\\', "/");
    let prefix = project_dir.to_string_lossy().replace('\\', "/");
    if norm.len() > prefix.len() && norm[..prefix.len()].eq_ignore_ascii_case(&prefix) {
        Some(norm[prefix.len()..].trim_start_matches('/').to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DAP answer is positional and carries no line of its own for a
    /// breakpoint the debugger refused — so the request's lines drive the
    /// mapping, a shorter answer array leaves the rest unverified, and only a
    /// DIFFERENT reported line counts as a relocation.
    #[test]
    fn breakpoint_verdicts_zip_with_the_request() {
        let asked = [265, 282, 286, 300];
        let answers = vec![
            json!({"verified": false, "message": "no code at this line"}),
            json!({"verified": true, "line": 282}),
            json!({"verified": true, "line": 291}),
            // 300: probe-rs sent nothing back for it.
        ];
        let got = bp_statuses(&asked, &answers);

        assert_eq!(got.len(), 4);
        let b265 = &got[&265];
        assert!(!b265.verified);
        assert_eq!(b265.message.as_deref(), Some("no code at this line"));
        // Reported line == requested line → not a relocation.
        assert!(got[&282].verified);
        assert_eq!(got[&282].moved_to, None);
        // Moved to the nearest line with code.
        assert_eq!(got[&286].moved_to, Some(291));
        // A missing answer is NOT a confirmation.
        assert_eq!(got[&300], BpStatus::default());
        assert!(!got[&300].verified);
    }

    #[test]
    fn eval_result_placeholders_are_flagged_unresolved() {
        // probe-rs's `success:true` placeholder markers → greyed with a reason.
        assert_eq!(
            classify_eval_result("<invalid expression \"buf_a\">"),
            ("not in scope / unsupported".to_string(), true)
        );
        assert_eq!(
            classify_eval_result("<not found: b>"),
            ("not found in this frame".to_string(), true)
        );
        assert_eq!(
            classify_eval_result("<optimized out>"),
            ("optimized out".to_string(), true)
        );
        // A real value is passed through unchanged.
        assert_eq!(classify_eval_result("42"), ("42".to_string(), false));
        assert_eq!(
            classify_eval_result("Some(5)"),
            ("Some(5)".to_string(), false)
        );
    }

    #[test]
    fn rel_of_strips_workspace_prefix_case_insensitively() {
        let dir = PathBuf::from(r"C:\Users\x\AppData\Local\Temp\embedded_ide_0_check");
        assert_eq!(
            rel_of(
                r"C:\Users\x\AppData\Local\temp\embedded_ide_0_check\src\main.rs",
                &dir
            ),
            Some("src/main.rs".to_string())
        );
        // Forward slashes too.
        assert_eq!(
            rel_of(
                "C:/Users/x/AppData/Local/Temp/embedded_ide_0_check/src/pins/mod.rs",
                &dir
            ),
            Some("src/pins/mod.rs".to_string())
        );
        // Outside the workspace (HAL sources in ~/.cargo) → None.
        assert_eq!(
            rel_of(r"C:\Users\x\.cargo\registry\src\stm32f1xx-hal\lib.rs", &dir),
            None
        );
    }

    /// The frame/variable JSON shapes we rely on — parsed like the reader does.
    #[test]
    fn stack_and_variables_parse_from_dap_json() {
        let msg: Value = serde_json::json!({
            "type": "response", "request_seq": 7, "success": true,
            "command": "stackTrace",
            "body": { "stackFrames": [
                { "id": 1001, "name": "main", "line": 42,
                  "source": { "path": "C:/w/src/main.rs" } },
                { "id": 1002, "name": "Reset", "line": 0 }
            ]}
        });
        let dir = PathBuf::from("C:/w");
        let frames: Vec<Frame> = msg["body"]["stackFrames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| Frame {
                id: f["id"].as_i64().unwrap_or(0),
                name: f["name"].as_str().unwrap_or("?").to_string(),
                file_rel: f["source"]["path"].as_str().and_then(|p| rel_of(p, &dir)),
                line: f["line"].as_u64().unwrap_or(0) as u32,
            })
            .collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].file_rel.as_deref(), Some("src/main.rs"));
        assert_eq!(frames[0].line, 42);
        assert_eq!(frames[1].file_rel, None); // no source → greyed row
    }
}
