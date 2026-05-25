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

// ── LspState ──────────────────────────────────────────────────────────────────

pub struct LspState {
    pub status:      LspStatus,
    /// Diagnostics keyed by relative path, e.g. `"src/main.rs"`.
    pub diagnostics: HashMap<String, Vec<LspDiagnostic>>,
    /// Incremented on every `start()` so stale threads know to bail out.
    pub generation:  u64,
    /// Channel to the write thread; `None` while stopped.
    sender:          Option<mpsc::Sender<String>>,
    /// Tracks document version for `textDocument/didChange`.
    doc_version:     u64,
    /// Whether we have sent `textDocument/didOpen` yet this session.
    pub did_open_sent: bool,
    /// The last code text we sent so we skip no-op frames.
    pub last_sent_code: String,
    /// Workspace root URI (e.g. `file:///tmp/embedded_ide_0_check`).
    pub root_uri:    String,
}

impl Default for LspState {
    fn default() -> Self {
        Self {
            status:        LspStatus::Stopped,
            diagnostics:   HashMap::new(),
            generation:    0,
            sender:        None,
            doc_version:   0,
            did_open_sent: false,
            last_sent_code: String::new(),
            root_uri:      String::new(),
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

    /// Send `textDocument/didOpen` and record the text.
    pub fn did_open(&mut self, text: &str) {
        self.did_open_sent = true;
        self.doc_version   = 1;
        self.last_sent_code = text.to_owned();
        let uri = format!("{}/src/main.rs", self.root_uri);
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

    /// Send `textDocument/didChange` only when the text actually changed.
    pub fn did_change(&mut self, text: &str) {
        if text == self.last_sent_code {
            return;
        }
        self.last_sent_code = text.to_owned();
        self.doc_version += 1;
        let version = self.doc_version;
        let uri = format!("{}/src/main.rs", self.root_uri);
        self.send_raw(serde_json::json!({
            "jsonrpc": "2.0",
            "method":  "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }],
            }
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
        self.did_open_sent = false;
        self.last_sent_code = String::new();
        self.doc_version   = 0;
        self.root_uri      = String::new();
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
        s.doc_version   = 0;
        s.did_open_sent = false;
        s.last_sent_code = String::new();
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
                },
                "window": { "workDoneProgress": true },
            },
            "initializationOptions": {
                "checkOnSave":  { "enable": false },
                "procMacro":    { "enable": false },
                "diagnostics":  { "enable": true  },
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

        // ── Window progress (indexing feedback) ───────────────────────────────
        "$/progress" => {
            let kind  = msg["params"]["value"]["kind"].as_str().unwrap_or("");
            let token = msg["params"]["token"].as_str().unwrap_or("");
            // Many progress tokens signal ongoing indexing; "end" means done.
            if kind == "end"
                && (token.contains("rust") || token.contains("index"))
            {
                let mut s = state.lock().unwrap();
                if s.generation == my_gen && s.status == LspStatus::Indexing {
                    s.status = LspStatus::Ready;
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
    // Canonicalize to resolve symlinks and normalise separators.
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.to_string_lossy();
    #[cfg(windows)]
    {
        // Windows: C:\foo\bar → file:///C:/foo/bar
        let s = s.replace('\\', "/");
        format!("file:///{s}")
    }
    #[cfg(not(windows))]
    {
        // Unix: /foo/bar → file:///foo/bar  (file:// + /foo/bar)
        format!("file://{s}")
    }
}

/// Strip `root_uri + "/"` from an absolute URI to get the relative path.
/// `"file:///tmp/proj/src/main.rs"` → `"src/main.rs"`
fn uri_to_rel(uri: &str, root_uri: &str) -> String {
    let prefix = format!("{root_uri}/");
    uri.strip_prefix(&prefix)
        .unwrap_or(uri)
        .to_owned()
}
