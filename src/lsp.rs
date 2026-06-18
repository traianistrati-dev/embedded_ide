//! Async rust-analyzer LSP client.
//!
//! # Lifecycle
//! ```text
//! call start()
//!   → spawn launch thread
//!       → spawn write thread  (rx → RA stdin)
//!       → send initialize
//!       → loop: read RA stdout → handle_incoming()
//!                 on initialize response → send initialized, set Indexing
//!                 on publishDiagnostics  → update LspState.diagnostics, set Ready
//!
//! call did_open() / did_change() from UI thread at any time.
//! ```
//!
//! # Stale-thread safety
//! A `generation` counter in `LspState` is incremented on every `start()`.
//! `handle_incoming` checks the generation before touching shared state, so
//! a lingering read-thread from a previous MCU type never corrupts new results.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

impl DiagSeverity {
    fn from_lsp(n: u64) -> Self {
        match n {
            1 => Self::Error,
            2 => Self::Warning,
            3 => Self::Info,
            _ => Self::Hint,
        }
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error)
    }
    pub fn is_warning(&self) -> bool {
        matches!(self, Self::Warning)
    }
}

#[derive(Clone, Debug)]
pub struct LspDiagnostic {
    pub severity: DiagSeverity,
    pub message:  String,
    /// 1-based line number (converted from LSP 0-based)
    pub line:     u32,
    /// 1-based column
    pub col:      u32,
    pub end_line: u32,
    pub end_col:  u32,
    /// e.g. `"E0308"` or `"unused_variables"`
    pub code:     Option<String>,
}

impl LspDiagnostic {
    /// First line of the message, for compact one-line rows. Rust-analyzer
    /// messages are frequently multi-line (e.g. "mismatched types\nexpected …")
    /// — the remainder is shown only on expand (RA tab detail) or hover.
    pub fn headline(&self) -> &str {
        self.message.lines().next().unwrap_or("").trim_end()
    }

    /// `true` when the message has meaningful content beyond the first line,
    /// so callers can hint that more is available (e.g. append "…").
    pub fn has_more_lines(&self) -> bool {
        self.message.lines().skip(1).any(|l| !l.trim().is_empty())
    }
}

/// A single item returned by a `textDocument/completion` response.
#[derive(Clone, Debug, Default)]
pub struct CompletionItem {
    pub label:       String,
    /// LSP CompletionItemKind (1=Text, 2=Method, 3=Function, 5=Field, 6=Variable, …)
    pub kind:        u8,
    /// Short type / signature string shown inline next to the label
    pub detail:      String,
    /// Text actually inserted when the item is accepted
    /// (falls back to `label` when the server doesn't send `insertText`)
    pub insert_text: String,
    /// Plain-text documentation (stripped from LSP `documentation` markdown).
    /// Shown as a tooltip when the item is hovered.
    pub documentation: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LspStatus {
    #[default]
    Stopped,
    Starting,
    /// initialize response received; waiting for first diagnostic push
    Indexing,
    Ready,
    /// Fatal — rust-analyzer not found, or exited unexpectedly
    Failed(String),
}

impl LspStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Indexing | Self::Ready)
    }
    pub fn label(&self) -> &str {
        match self {
            Self::Stopped     => "Stopped",
            Self::Starting    => "Starting…",
            Self::Indexing    => "Indexing…",
            Self::Ready       => "Ready",
            Self::Failed(_)   => "Failed",
        }
    }
}

// ── Per-file open state ───────────────────────────────────────────────────────

/// Tracks the LSP state for one open file.
struct OpenFileState {
    /// LSP document version (incremented on every `textDocument/didChange`).
    doc_version:    u64,
    /// The text last sent to RA so no-op frames are skipped.
    last_sent_code: String,
}

// ── LspState ──────────────────────────────────────────────────────────────────

pub struct LspState {
    pub status:      LspStatus,
    /// Diagnostics keyed by relative path, e.g. `"src/main.rs"`.
    pub diagnostics: HashMap<String, Vec<LspDiagnostic>>,
    /// Incremented on every `start()` so stale threads know to bail out.
    pub generation:  u64,
    /// Channel to the write thread; `None` while stopped.
    sender:          Option<mpsc::Sender<String>>,
    /// Per-file open state.  Key = relative path, e.g. `"src/main.rs"`.
    open_files:      HashMap<String, OpenFileState>,
    /// Files for which a `didChange` was sent but no `publishDiagnostics` has
    /// arrived since — their current diagnostics are stale (computed for the
    /// previous text). The inline overlay hides them until RA re-publishes, so a
    /// fixed/deleted error never lingers at a stale position. Key = rel path.
    awaiting_diagnostics: std::collections::HashSet<String>,
    /// Workspace root URI (e.g. `file:///tmp/embedded_ide_0_check`).
    pub root_uri:    String,
    /// True while RA is running a background `cargo check` pass.
    /// Set to `true` on `$/progress begin` for check tokens; cleared on `end`.
    pub checking: bool,
    /// Most recent completion items from rust-analyzer.
    pub completion_items:  Vec<CompletionItem>,
    /// Set to `true` when a completion response (success OR error) arrives.
    pub completion_response_received: bool,
    /// The request id of the pending completion request, if any.
    completion_req_id:     Option<u64>,
    /// Counter for outgoing requests (starts at 1; incremented before each send → first = 2).
    next_req_id:           u64,
    /// When the last completion request was sent (for spinner timeout).
    pub completion_request_sent_at: Option<std::time::Instant>,
}

impl Default for LspState {
    fn default() -> Self {
        Self {
            status:        LspStatus::Stopped,
            diagnostics:   HashMap::new(),
            generation:    0,
            sender:        None,
            open_files:    HashMap::new(),
            awaiting_diagnostics: std::collections::HashSet::new(),
            root_uri:          String::new(),
            checking:          false,
            completion_items:  Vec::new(),
            completion_response_received: false,
            completion_req_id: None,
            next_req_id:       1,
            completion_request_sent_at: None,
        }
    }
}

impl LspState {
    // ── Sending helpers ───────────────────────────────────────────────────────

    fn send_raw(&self, json: String) {
        if let Some(tx) = &self.sender {
            let _ = tx.send(json);
        }
    }

    /// Returns `true` if `textDocument/didOpen` has been sent for `rel_path`.
    pub fn is_file_open(&self, rel_path: &str) -> bool {
        self.open_files.contains_key(rel_path)
    }

    /// `true` when `rel_path` is open and the text rust-analyzer last received
    /// for it equals `text` — i.e. no edits are pending sync for this file.
    /// Used to gate the inline diagnostic overlay so it never draws diagnostics
    /// computed for a different (older) version of the file at stale positions.
    pub fn last_sent_matches(&self, rel_path: &str, text: &str) -> bool {
        self.open_files
            .get(rel_path)
            .map(|f| f.last_sent_code == text)
            .unwrap_or(false)
    }

    /// `true` when RA has published diagnostics for `rel_path` *since* the last
    /// `didChange` — i.e. the current `diagnostics` reflect the latest text, not
    /// a stale version. `false` between sending an edit and RA's response.
    pub fn diagnostics_fresh(&self, rel_path: &str) -> bool {
        !self.awaiting_diagnostics.contains(rel_path)
    }

    /// Send `textDocument/didOpen` for `rel_path` and record the text.
    ///
    /// `rel_path` is relative to the workspace root, e.g. `"src/main.rs"`.
    pub fn did_open(&mut self, rel_path: &str, text: &str) {
        if self.sender.is_none() { return; }
        self.open_files.insert(rel_path.to_owned(), OpenFileState {
            doc_version:    1,
            last_sent_code: text.to_owned(),
        });
        let uri = format!("{}/{}", self.root_uri, rel_path);
        self.send_raw(serde_json::json!({
            "jsonrpc": "2.0",
            "method":  "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri":        uri,
                    "languageId": "rust",
                    "version":    1,
                    "text":       text,
                }
            }
        }).to_string());
    }

    /// Send `textDocument/didChange` for `rel_path` when the text has changed.
    /// Auto-opens the file via `didOpen` if it hasn't been opened yet.
    ///
    /// `force` re-sends (with a bumped version) even when the text is unchanged,
    /// so rust-analyzer re-runs its analysis — used by Project Save to restart
    /// verification. `rel_path` is relative to the workspace root.
    pub fn did_change(&mut self, rel_path: &str, text: &str, force: bool) {
        if self.sender.is_none() { return; }
        // Auto-open the file on first access.
        if !self.open_files.contains_key(rel_path) {
            self.did_open(rel_path, text);
            return;
        }
        let file = self.open_files.get_mut(rel_path).unwrap();
        if text == file.last_sent_code && !force { return; }
        file.last_sent_code = text.to_owned();
        file.doc_version   += 1;
        let version = file.doc_version;
        // New text sent → current diagnostics for this file are now stale until
        // RA re-publishes (see `diagnostics_fresh`).
        self.awaiting_diagnostics.insert(rel_path.to_owned());
        let uri = format!("{}/{}", self.root_uri, rel_path);
        self.send_raw(serde_json::json!({
            "jsonrpc": "2.0",
            "method":  "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }],
            }
        }).to_string());
    }

    /// Request completions at the given cursor position in `rel_path`.
    ///
    /// `trigger_char = None`  → manual invocation (Ctrl+Space, triggerKind=1)
    /// `trigger_char = Some(c)` → auto-trigger (typed `.` or `:`, triggerKind=2)
    /// `rel_path` is relative to the workspace root, e.g. `"src/main.rs"`.
    pub fn request_completion(
        &mut self,
        rel_path:     &str,
        line:         u32,
        character:    u32,
        trigger_char: Option<char>,
    ) {
        if self.sender.is_none() { return; }
        self.next_req_id += 1;
        let id = self.next_req_id;
        self.completion_req_id = Some(id);
        self.completion_items.clear();
        self.completion_response_received = false;
        self.completion_request_sent_at = Some(std::time::Instant::now());
        lsp_log(&format!(
            "COMPLETION_REQ id={id} file={rel_path} line={line} char={character} \
             trigger={trigger_char:?}"
        ));
        let uri = format!("{}/{}", self.root_uri, rel_path);
        let trigger_kind: u32 = if trigger_char.is_some() { 2 } else { 1 };
        let mut params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "triggerKind": trigger_kind }
        });
        if let Some(c) = trigger_char {
            params["context"]["triggerCharacter"] = serde_json::json!(c.to_string());
        }
        self.send_raw(serde_json::json!({
            "jsonrpc": "2.0",
            "id":      id,
            "method":  "textDocument/completion",
            "params":  params
        }).to_string());
    }

    // ── Diagnostic helpers ────────────────────────────────────────────────────

    pub fn error_count_for(&self, path: &str) -> usize {
        self.diagnostics
            .get(path)
            .map(|ds| ds.iter().filter(|d| d.severity.is_error()).count())
            .unwrap_or(0)
    }

    pub fn warning_count_for(&self, path: &str) -> usize {
        self.diagnostics
            .get(path)
            .map(|ds| ds.iter().filter(|d| d.severity.is_warning()).count())
            .unwrap_or(0)
    }

    pub fn total_errors(&self) -> usize {
        self.diagnostics
            .values()
            .flat_map(|v| v.iter())
            .filter(|d| d.severity.is_error())
            .count()
    }

    pub fn total_warnings(&self) -> usize {
        self.diagnostics
            .values()
            .flat_map(|v| v.iter())
            .filter(|d| d.severity.is_warning())
            .count()
    }

    /// Stop any running session and reset to `Stopped`.
    /// Incrementing `generation` makes stale background threads bail out
    /// silently without corrupting the fresh state.
    pub fn reset(&mut self) {
        self.generation   += 1;
        self.status        = LspStatus::Stopped;
        self.diagnostics   .clear();
        self.sender        = None;   // write thread's Receiver will close → it exits
        self.open_files    .clear();
        self.awaiting_diagnostics.clear();
        self.root_uri      = String::new();
        self.checking      = false;
        self.completion_items.clear();
        self.completion_req_id = None;
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Prepare the shared state and spawn `rust-analyzer` for `workspace_dir`.
///
/// The workspace files (Cargo.toml, src/main.rs, …) must already exist on disk.
/// Call `lsp_state.lock().unwrap().did_open(text)` once the status reaches
/// `LspStatus::Indexing`, then `did_change(text)` on every code update.
pub fn start(
    workspace_dir: &Path,
    state: Arc<Mutex<LspState>>,
    ctx: eframe::egui::Context,
) {
    let workspace_dir = workspace_dir.to_path_buf();
    let root_uri = path_to_uri(&workspace_dir);

    {
        let mut s = state.lock().unwrap();
        s.generation   += 1;
        s.status        = LspStatus::Starting;
        s.diagnostics   .clear();
        s.root_uri      = root_uri.clone();
        s.open_files    .clear();
        // Drop any old sender — signals the old write thread to exit.
        s.sender = None;
    }
    ctx.request_repaint();

    thread::spawn(move || {
        launch(workspace_dir, root_uri, state, ctx);
    });
}

// ── Internal ──────────────────────────────────────────────────────────────────

fn launch(
    workspace_dir: PathBuf,
    root_uri: String,
    state: Arc<Mutex<LspState>>,
    ctx: eframe::egui::Context,
) {
    // Snapshot our generation so we can detect restarts.
    let my_gen = state.lock().unwrap().generation;

    let mut child = match Command::new("rust-analyzer")
        .current_dir(&workspace_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let mut s = state.lock().unwrap();
            if s.generation == my_gen {
                s.status = LspStatus::Failed(format!(
                    "Could not launch `rust-analyzer`: {e}\n\
                     Install from https://rust-analyzer.github.io or via rustup:\n\
                     rustup component add rust-analyzer"
                ));
                ctx.request_repaint();
            }
            return;
        }
    };

    let stdin  = child.stdin .take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");

    // Channel: any thread with a Sender → write thread → RA stdin
    let (tx, rx) = mpsc::channel::<String>();

    // Store sender in LspState (only if our generation is still current).
    {
        let mut s = state.lock().unwrap();
        if s.generation != my_gen {
            // Already restarted — bail out silently.
            drop(child);
            return;
        }
        s.sender = Some(tx.clone());
    }

    // ── Write thread ──────────────────────────────────────────────────────────
    thread::spawn(move || {
        let mut stdin = stdin;
        for msg in rx {
            if write_lsp(&mut stdin, &msg).is_err() {
                break;
            }
        }
        // stdin dropped here → RA gets EOF on its stdin
    });

    // ── Send `initialize` ─────────────────────────────────────────────────────
    let _ = tx.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id":      1,
        "method":  "initialize",
        "params": {
            "processId": std::process::id(),
            "rootUri":   root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": "project" }],
            "capabilities": {
                "textDocument": {
                    "synchronization": {
                        "dynamicRegistration": false,
                        "willSave": false,
                        "didSave": false,
                    },
                    "publishDiagnostics": {
                        "relatedInformation": false,
                        "versionSupport":     false,
                    },
                    "completion": {
                        "completionItem": {
                            "snippetSupport":      false,
                            "documentationFormat": ["plaintext", "markdown"],
                            "labelDetailsSupport": true,
                        },
                        "completionItemKind": {
                            "valueSet": [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25]
                        },
                        "contextSupport": true,
                    },
                },
                "window": { "workDoneProgress": true },
            },
            "initializationOptions": {
                // Enable cargo check so ALL compilation errors are reported —
                // not just those from RA's in-memory analysis.  Without this,
                // errors like undefined variables, missing semicolons, and wrong
                // function names in embedded code are not detected because RA's
                // analysis can't fully resolve esp-hal types until crates are
                // indexed.  cargo check catches everything the compiler sees.
                //
                // RA debounces the check (default ~1 s after last edit) so it
                // does not run on every keystroke.  The "checking…" progress
                // notification is tracked in handle_incoming() to update the
                // status indicator in the editor header.
                "checkOnSave":  true,

                // Proc-macro expansion is disabled.
                //
                // WHY: RA looks for the proc-macro DLL (e.g. esp_hal_procmacros-
                // <hash>.dll) in target/debug/deps/.  The DLL only exists after a
                // successful `cargo build`.  Our workspace deletes Cargo.lock on
                // every project write, which changes the resolution hash, so the
                // DLL RA cached from a previous session is no longer present.
                // Enabling proc-macros therefore causes:
                //   "Cannot create expander for <dll>: path not found (os error 3)"
                // on every RA startup until the user manually runs a build.
                //
                // With proc-macros disabled, RA still analyses the full crate
                // graph, provides :: completions, type inference, diagnostics, and
                // go-to-definition — it just can't *expand* attribute macros like
                // #[esp_hal::main].  The function body and all other code are fully
                // analysed, so the IDE experience is not materially affected.
                //
                // To prevent RA from reporting a false "unresolved-proc-macro"
                // warning on #[esp_hal::main] we suppress that diagnostic below.
                "procMacro": { "enable": false },

                "diagnostics": {
                    "enable": true,
                    // Suppress the "proc-macro expansion is disabled" pseudo-error
                    // that RA emits for every attribute macro when expansion is off.
                    // All real compiler errors (type mismatches, borrow errors, …)
                    // are still reported through cargo-check diagnostics.
                    "disabled": ["unresolved-proc-macro"],
                },

                // Ask RA to include full documentation text in completion
                // responses rather than returning only a label.
                "completion": {
                    "fullFunctionSignatures": { "enable": true },
                },

                // Let RA read the target from .cargo/config.toml.
                // For ESP32-C3 this is riscv32imc-unknown-none-elf, which ensures
                // that cfg(target_arch = "riscv32") items in esp-hal are visible.
                "cargo": {
                    "noDefaultFeatures": false,
                },
            },
        }
    }).to_string());

    // ── Read loop (this thread IS the read thread) ─────────────────────────────
    let mut reader = BufReader::new(stdout);
    let tx_read = tx.clone(); // for sending `initialized` + `didOpen` from this thread

    loop {
        match read_lsp(&mut reader) {
            Some(msg) => handle_incoming(msg, &state, &ctx, &tx_read, &root_uri, my_gen),
            None      => break, // EOF — RA exited
        }
    }

    // RA exited (or we got EOF).
    let mut s = state.lock().unwrap();
    if s.generation == my_gen && s.status.is_active() {
        s.status = LspStatus::Failed("rust-analyzer exited unexpectedly.".into());
        ctx.request_repaint();
    }
    // Let child reap itself — drop will send SIGTERM on some OSes.
    drop(child);
}

// ── LSP framing ───────────────────────────────────────────────────────────────

fn write_lsp(sink: &mut impl Write, json: &str) -> std::io::Result<()> {
    write!(sink, "Content-Length: {}\r\n\r\n", json.len())?;
    sink.write_all(json.as_bytes())?;
    sink.flush()
}

fn read_lsp<R: BufRead>(reader: &mut R) -> Option<serde_json::Value> {
    // Read headers until blank line
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 {
            return None; // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(v) = trimmed.strip_prefix("Content-Length: ") {
            content_length = v.trim().parse().ok()?;
        }
    }
    if content_length == 0 {
        return None;
    }
    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

// ── Message handler ───────────────────────────────────────────────────────────

/// Append a line to the LSP debug log in the system temp dir.
/// File: <TEMP>/embedded_ide_lsp.log
/// Only active in debug builds; no-op in release.
#[cfg(debug_assertions)]
fn lsp_log(line: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("embedded_ide_lsp.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", line);
    }
}
#[cfg(not(debug_assertions))]
fn lsp_log(_: &str) {}

fn handle_incoming(
    msg:      serde_json::Value,
    state:    &Arc<Mutex<LspState>>,
    ctx:      &eframe::egui::Context,
    tx:       &mpsc::Sender<String>,
    root_uri: &str,
    my_gen:   u64,
) {
    // Guard: if generation advanced we are a stale thread — stop processing.
    if state.lock().unwrap().generation != my_gen {
        return;
    }

    let method = msg["method"].as_str().unwrap_or("");

    // Log all response messages (no "method") for debugging.
    #[cfg(debug_assertions)]
    if method.is_empty() {
        let id  = &msg["id"];
        let has_result = msg.get("result").is_some();
        let has_error  = msg.get("error").is_some();
        let preview = if has_result {
            let r = msg["result"].to_string();
            format!("result={}", &r[..r.len().min(200)])
        } else if has_error {
            format!("error={}", msg["error"].to_string())
        } else {
            "?".to_owned()
        };
        lsp_log(&format!("RESPONSE id={id} {preview}"));
    }

    match method {
        // ── Initialize response ───────────────────────────────────────────────
        "" if msg.get("id") == Some(&serde_json::Value::Number(1.into()))
           && msg.get("result").is_some() =>
        {
            // Confirm handshake.
            let _ = tx.send(
                r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#.to_owned(),
            );
            let mut s = state.lock().unwrap();
            if s.generation == my_gen {
                s.status = LspStatus::Indexing;
            }
            ctx.request_repaint();
        }

        // ── Diagnostics ───────────────────────────────────────────────────────
        "textDocument/publishDiagnostics" => {
            let params   = &msg["params"];
            let uri      = params["uri"].as_str().unwrap_or("");
            let rel_path = uri_to_rel(uri, root_uri);

            let diags: Vec<LspDiagnostic> = params["diagnostics"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(parse_diag)
                .collect();

            let mut s = state.lock().unwrap();
            if s.generation != my_gen {
                return;
            }
            // RA has re-evaluated this file → its diagnostics are fresh again,
            // so the inline overlay may show them (see `diagnostics_fresh`).
            s.awaiting_diagnostics.remove(&rel_path);
            if diags.is_empty() {
                s.diagnostics.remove(&rel_path);
            } else {
                s.diagnostics.insert(rel_path, diags);
            }
            // Transition Indexing → Ready on first diagnostic push.
            if matches!(s.status, LspStatus::Indexing | LspStatus::Starting) {
                s.status = LspStatus::Ready;
            }
            ctx.request_repaint();
        }

        // ── Window progress ────────────────────────────────────────────────────
        // RA sends $/progress for two kinds of work:
        //   1. Indexing  — token contains "rust" or "index";  "end" → Ready
        //   2. cargo check — token contains "cargo" or "check";
        //                    "begin" → checking=true, "end" → checking=false
        "$/progress" => {
            let kind  = msg["params"]["value"]["kind"].as_str().unwrap_or("");
            let token_raw = msg["params"]["token"].as_str().unwrap_or("");
            // RA may use numeric tokens — convert to string for matching
            let token_num = msg["params"]["token"].as_u64()
                .map(|n| n.to_string());
            let token = token_num.as_deref().unwrap_or(token_raw);

            let is_indexing = token.contains("rust") || token.contains("index");
            let is_check    = token.contains("cargo") || token.contains("check")
                           || token.contains("flycheck");

            let mut s = state.lock().unwrap();
            if s.generation != my_gen { return; }

            if is_indexing && kind == "end" && s.status == LspStatus::Indexing {
                s.status = LspStatus::Ready;
                ctx.request_repaint();
            }
            if is_check {
                match kind {
                    "begin" => { s.checking = true;  ctx.request_repaint(); }
                    "end"   => { s.checking = false; ctx.request_repaint(); }
                    _ => {}
                }
            }
        }

        // ── Completion response (success) ─────────────────────────────────────
        // Any response (method == "") whose id is not 1 (initialize) and that
        // carries a "result" field is treated as a completion response.
        "" if msg.get("result").is_some()
           && msg.get("id").is_some()
           && msg["id"].as_u64().map_or(false, |n| n != 1) =>
        {
            if let Some(req_id) = msg["id"].as_u64() {
                let mut s = state.lock().unwrap();
                if s.generation == my_gen && s.completion_req_id == Some(req_id) {
                    s.completion_req_id = None;
                    s.completion_response_received = true;
                    let result = &msg["result"];
                    // CompletionList { items: [...] }  OR  [...] directly
                    // `result` may also be JSON null — treat as empty list.
                    let items_arr = result["items"]
                        .as_array()
                        .or_else(|| result.as_array());
                    s.completion_items = items_arr
                        .map(|arr| {
                            arr.iter()
                                .filter_map(parse_completion_item)
                                .take(60)
                                .collect()
                        })
                        .unwrap_or_default();
                    ctx.request_repaint();
                }
            }
        }

        // ── Completion response (error / cancel) ──────────────────────────────
        // RA returns {"id": N, "error": {...}} when it cannot fulfil a request
        // (e.g. the file won't compile, or the request was cancelled).
        // We must handle this or the spinner runs forever.
        "" if msg.get("error").is_some()
           && msg.get("id").is_some()
           && msg["id"].as_u64().map_or(false, |n| n != 1) =>
        {
            if let Some(req_id) = msg["id"].as_u64() {
                let mut s = state.lock().unwrap();
                if s.generation == my_gen && s.completion_req_id == Some(req_id) {
                    s.completion_req_id = None;
                    s.completion_response_received = true;
                    // completion_items stays empty — App will close the popup.
                    ctx.request_repaint();
                }
            }
        }

        // Ignore window/logMessage, telemetry, etc.
        _ => {}
    }
}

fn parse_diag(v: &serde_json::Value) -> Option<LspDiagnostic> {
    let message  = v["message"].as_str()?.to_owned();
    let severity = DiagSeverity::from_lsp(v["severity"].as_u64().unwrap_or(1));
    let start    = &v["range"]["start"];
    let end_v    = &v["range"]["end"];
    let line     = start["line"]      .as_u64().unwrap_or(0) as u32 + 1;
    let col      = start["character"] .as_u64().unwrap_or(0) as u32 + 1;
    let end_line = end_v["line"]      .as_u64().unwrap_or(0) as u32 + 1;
    let end_col  = end_v["character"] .as_u64().unwrap_or(0) as u32 + 1;
    // code may be a string like "E0308" or an integer
    let code = v["code"].as_str().map(String::from)
        .or_else(|| v["code"].as_u64().map(|n| n.to_string()));
    Some(LspDiagnostic { severity, message, line, col, end_line, end_col, code })
}

// ── URI helpers ───────────────────────────────────────────────────────────────

pub fn path_to_uri(path: &Path) -> String {
    // On Windows, Path::canonicalize() returns extended-length paths prefixed
    // with \\?\ (e.g. \\?\C:\Users\...).  Replacing every backslash with a
    // forward slash would produce //?/C:/... — an invalid file URI that causes
    // rust-analyzer to return error -32603 "url is not a file" for every request.
    //
    // Fix: strip the \\?\ prefix before building the URI.
    // If canonicalize fails (directory doesn't exist yet), the fallback path
    // is already an absolute path without the \\?\ prefix.
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.to_string_lossy();

    #[cfg(windows)]
    {
        // Strip the Windows extended-length path prefix \\?\ if present,
        // then normalise backslashes to forward slashes.
        // Result: file:///C:/Users/foo/bar
        let stripped   = s.strip_prefix(r"\\?\").unwrap_or(&s);
        let normalised = stripped.replace('\\', "/");
        format!("file:///{normalised}")
    }
    #[cfg(not(windows))]
    {
        // Unix: /foo/bar  →  file:///foo/bar
        format!("file://{s}")
    }
}

/// Strip `root_uri + "/"` from an absolute URI to get the relative path.
/// `"file:///tmp/proj/src/main.rs"` → `"src/main.rs"`
///
/// On Windows, rust-analyzer may normalise the drive letter to lowercase
/// (`file:///c:/…`) while `path_to_uri` produces uppercase (`file:///C:/…`).
/// The comparison is therefore case-insensitive on Windows so that the key
/// stored in `LspState.diagnostics` is always the short relative path
/// (`src/main.rs`) rather than the full URI.
fn uri_to_rel(uri: &str, root_uri: &str) -> String {
    let prefix = format!("{root_uri}/");

    // Case-sensitive match first (non-Windows, or matching case on Windows)
    if let Some(rel) = uri.strip_prefix(&prefix) {
        return rel.to_owned();
    }

    // Case-insensitive fallback for Windows drive-letter mismatches
    #[cfg(windows)]
    {
        let uri_lc = uri.to_lowercase();
        let pfx_lc = prefix.to_lowercase();
        if uri_lc.starts_with(&pfx_lc) {
            // Preserve the original (potentially mixed-case) suffix
            return uri[prefix.len()..].to_owned();
        }
    }

    // Nothing matched — return the full URI as-is; the diags_for_main_rs
    // helper in app.rs will still find it by suffix matching.
    uri.to_owned()
}

fn parse_completion_item(v: &serde_json::Value) -> Option<CompletionItem> {
    let label = v["label"].as_str()?.to_owned();
    let kind = v["kind"].as_u64().unwrap_or(6) as u8;

    // `detail` is the primary type annotation (e.g. "-> bool", "fn(...)").
    // rust-analyzer may put this in `detail` directly, or in
    // `labelDetails.detail` / `labelDetails.description`.
    let detail = {
        let from_detail = v["detail"].as_str().unwrap_or("").to_owned();
        if !from_detail.is_empty() {
            from_detail
        } else {
            // labelDetails.detail is typically the short type suffix (e.g. "(…) -> T")
            // labelDetails.description is typically the full qualified path
            let ld_detail = v["labelDetails"]["detail"].as_str().unwrap_or("");
            let ld_desc   = v["labelDetails"]["description"].as_str().unwrap_or("");
            match (ld_detail.is_empty(), ld_desc.is_empty()) {
                (false, false) => format!("{ld_detail}  {ld_desc}"),
                (false, true)  => ld_detail.to_owned(),
                (true,  false) => ld_desc.to_owned(),
                (true,  true)  => String::new(),
            }
        }
    };

    let insert_text = v["insertText"]
        .as_str()
        .map(|s| s.to_owned())
        .unwrap_or_else(|| label.clone());

    // `documentation` can be a plain string or { kind: "markdown", value: "..." }.
    // Strip leading `\`\`\`rust … \`\`\`` fences that rust-analyzer wraps code in.
    let documentation = {
        let doc = &v["documentation"];
        let raw = if let Some(s) = doc.as_str() {
            s.to_owned()
        } else if let Some(s) = doc["value"].as_str() {
            s.to_owned()
        } else {
            String::new()
        };
        // Strip markdown code fences (```rust … ```) so text reads cleanly.
        strip_md_fences(&raw)
    };

    Some(CompletionItem { label, kind, detail, insert_text, documentation })
}

/// Remove ` ```lang … ``` ` fences from a markdown string so it reads as
/// plain text in a tooltip.
fn strip_md_fences(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_fence = false;
    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue; // skip the fence line itself
        }
        if !in_fence {
            if !out.is_empty() { out.push('\n'); }
            out.push_str(line);
        } else {
            // Inside a code fence: keep the code but strip leading indent.
            if !out.is_empty() { out.push('\n'); }
            out.push_str(trimmed);
        }
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod diagnostic_headline_tests {
    use super::*;

    fn diag(message: &str) -> LspDiagnostic {
        LspDiagnostic {
            severity: DiagSeverity::Error,
            message: message.to_owned(),
            line: 1,
            col: 1,
            end_line: 1,
            end_col: 1,
            code: None,
        }
    }

    #[test]
    fn headline_is_first_line_only() {
        let d = diag("mismatched types\nexpected `u8`, found `u16`");
        assert_eq!(d.headline(), "mismatched types");
        assert!(d.has_more_lines());
    }

    #[test]
    fn single_line_has_no_more() {
        let d = diag("unused variable: `x`");
        assert_eq!(d.headline(), "unused variable: `x`");
        assert!(!d.has_more_lines());
    }

    #[test]
    fn trailing_blank_lines_do_not_count() {
        let d = diag("cannot find value `foo`\n\n  ");
        assert_eq!(d.headline(), "cannot find value `foo`");
        assert!(!d.has_more_lines(), "blank trailing lines aren't 'more'");
    }
}
