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
    sync::{Arc, Mutex, mpsc},
    thread,
};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    pub message: String,
    /// 1-based line number (converted from LSP 0-based)
    pub line: u32,
    /// 1-based column
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    /// e.g. `"E0308"` or `"unused_variables"`
    pub code: Option<String>,
    /// LSP `source`: `"rust-analyzer"` for native (in-memory) diagnostics, or
    /// `"rustc"` / `"clippy"` for flycheck (cargo check) ones. Flycheck positions
    /// can't be re-mapped after an edit until the next check runs, so the inline
    /// overlay hides them while a re-check is pending (see `flycheck_stale`).
    pub source: String,
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

    /// `true` for a numbered rustc compiler-error code (`E0425`, `E0308`, …) —
    /// as opposed to a named lint (`unused_variables`, `dead_code`, …).
    ///
    /// Both are reported with `source == "rustc"` over LSP, but they come from
    /// different places: a numbered error is a type/name-resolution "hard
    /// error" rust-analyzer's own in-memory analyzer computes and re-publishes
    /// on every edit — confirmed empirically (`rust-analyzer diagnostics
    /// --disable-build-scripts`, no cargo-check involved, finds `E0425`
    /// instantly even in a nested-module project). A named lint like
    /// `unused_variables`/`dead_code` genuinely requires an actual `cargo
    /// check`/`cargo clippy` pass (confirmed the same way: zero native
    /// diagnostics for those with `checkOnSave` off) — see
    /// `editor_panel::usages`'s doc comment for that investigation.
    ///
    /// `flycheck_stale()` exists to hide *genuinely* flycheck-sourced
    /// diagnostics once an edit shifts their line/col — but it must NOT also
    /// hide numbered hard errors, since those are live and always current.
    /// Same code-shape check already used for `rustc_error_doc_url` in the
    /// inline diagnostics overlay.
    pub fn is_rustc_error_code(&self) -> bool {
        self.code
            .as_deref()
            .and_then(|c| c.strip_prefix('E'))
            .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
    }
}

/// A single item returned by a `textDocument/completion` response.
#[derive(Clone, Debug, Default)]
pub struct CompletionItem {
    pub label: String,
    /// LSP CompletionItemKind (1=Text, 2=Method, 3=Function, 5=Field, 6=Variable, …)
    pub kind: u8,
    /// Short type / signature string shown inline next to the label
    pub detail: String,
    /// Text actually inserted when the item is accepted
    /// (falls back to `label` when the server doesn't send `insertText`)
    pub insert_text: String,
    /// True when `insert_text` is an LSP snippet (`insertTextFormat == 2`,
    /// e.g. `foo(${1:a})$0`) — expanded on accept by `snippet::expand`.
    pub insert_is_snippet: bool,
    /// Plain-text documentation (stripped from LSP `documentation` markdown).
    /// Shown as a tooltip when the item is hovered.
    pub documentation: String,
}

/// One text replacement from a `textDocument/rename` WorkspaceEdit. Positions
/// are 0-based LSP (line, character) ranges. Applied across files to perform a
/// project-wide rename.
#[derive(Clone, Debug)]
pub struct RenameEdit {
    /// Path relative to the workspace root, e.g. `"src/main.rs"`.
    pub rel_path: String,
    pub start_line: u32,
    pub start_char: u32,
    pub end_line: u32,
    pub end_char: u32,
    pub new_text: String,
}

/// One `textDocument/codeAction` result (an RA assist / quick-fix). The `edits`
/// are `None` when RA returned the action lazily — the caller then sends
/// `codeAction/resolve` with `raw` (RA requires the whole action object back,
/// not just its `data`) to obtain them.
#[derive(Clone, Debug)]
pub struct CodeAction {
    pub title: String,
    /// Parsed `WorkspaceEdit`, or `None` until resolved.
    pub edits: Option<Vec<RenameEdit>>,
    /// The original JSON action, sent verbatim to `codeAction/resolve`.
    pub raw: serde_json::Value,
}

impl CodeAction {
    /// True when this action can produce edits — either inline or via resolve
    /// (has a `data` field). Command-only actions (no edit, no data) are
    /// skipped: we don't run `workspace/executeCommand` in v1.
    pub fn is_applicable(&self) -> bool {
        self.edits.is_some() || !self.raw["data"].is_null()
    }
}

/// A `textDocument/definition` target: the file + 0-based position RA points to.
#[derive(Clone, Debug)]
pub struct DefinitionLoc {
    /// Absolute filesystem path (decoded from the `file://` URI).
    pub path: String,
    pub line: u32,
    pub character: u32,
}

/// One item from `textDocument/documentSymbol` (fn/struct/enum/const/static/
/// trait/method/field/…) in a file — used to fade never-referenced items and
/// offer a "references" list on the rest. Positions are 0-based LSP (line,
/// UTF-16 character), like the rest of this module.
#[derive(Clone, Debug)]
pub struct SymbolInfo {
    pub name: String,
    /// LSP `SymbolKind` (6=Method, 8=Field, 9=Constructor, 10=Enum, 11=Interface
    /// [trait], 12=Function, 13=Variable [static], 14=Constant, 22=EnumMember,
    /// 23=Struct); see [`is_trackable_symbol_kind`].
    pub kind: u8,
    /// The whole item's span — used to fade it when unused.
    pub start_line: u32,
    pub start_char: u32,
    pub end_line: u32,
    pub end_char: u32,
    /// The name's own position — used as the query point for `references`.
    pub sel_line: u32,
    pub sel_char: u32,
    /// True when the symbol sits inside an `impl Trait for Type` block.
    /// `references` on such members misses calls dispatched through a generic
    /// trait bound (those bind to the TRAIT's declaration), so an empty result
    /// here doesn't mean "unused" — these items must never be faded.
    pub in_trait_impl: bool,
}

/// `true` for the `SymbolKind`s worth tracking (fn/method/struct/enum/const/
/// static/trait/field/…) — containers like Module/Namespace/File are excluded
/// (we still recurse INTO them, just don't fade/count them as items themselves).
pub fn is_trackable_symbol_kind(kind: u8) -> bool {
    matches!(kind, 6 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 22 | 23)
}

/// One usage site from `textDocument/references`.
#[derive(Clone, Debug)]
pub struct ReferenceLoc {
    /// Absolute filesystem path (decoded from the `file://` URI), like `DefinitionLoc`.
    pub path: String,
    pub line: u32,
    pub character: u32,
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
            Self::Stopped => "Stopped",
            Self::Starting => "Starting…",
            Self::Indexing => "Indexing…",
            Self::Ready => "Ready",
            Self::Failed(_) => "Failed",
        }
    }
}

// ── Per-file open state ───────────────────────────────────────────────────────

/// Tracks the LSP state for one open file.
struct OpenFileState {
    /// LSP document version (incremented on every `textDocument/didChange`).
    doc_version: u64,
    /// The text last sent to RA so no-op frames are skipped.
    last_sent_code: String,
}

// ── LspState ──────────────────────────────────────────────────────────────────

pub struct LspState {
    pub status: LspStatus,
    /// Diagnostics keyed by relative path, e.g. `"src/main.rs"`.
    pub diagnostics: HashMap<String, Vec<LspDiagnostic>>,
    /// Incremented on every `start()` so stale threads know to bail out.
    pub generation: u64,
    /// Channel to the write thread; `None` while stopped.
    sender: Option<mpsc::Sender<String>>,
    /// Per-file open state.  Key = relative path, e.g. `"src/main.rs"`.
    open_files: HashMap<String, OpenFileState>,
    /// Files for which a `didChange` was sent but no `publishDiagnostics` has
    /// arrived since — their current diagnostics are stale (computed for the
    /// previous text). The inline overlay hides them until RA re-publishes, so a
    /// fixed/deleted error never lingers at a stale position. Key = rel path.
    awaiting_diagnostics: std::collections::HashSet<String>,
    /// Edit generation: bumped on every real `didChange`. Compared against
    /// `fresh_check_gen` to know whether flycheck (cargo check) diagnostics are
    /// stale (an edit happened since the last completed check).
    edit_gen: u64,
    /// `edit_gen` captured when the current cargo-check pass began.
    check_begin_gen: u64,
    /// The `edit_gen` the most recently COMPLETED cargo-check reflects.
    fresh_check_gen: u64,
    /// Workspace root URI (e.g. `file:///tmp/embedded_ide_0_check`).
    pub root_uri: String,
    /// True while RA is running a background `cargo check` pass.
    /// Set to `true` on `$/progress begin` for check tokens; cleared on `end`.
    pub checking: bool,
    /// When the last `didSave` went out — starts the flycheck queue-latency clock.
    last_did_save_at: Option<std::time::Instant>,
    /// When RA reported the current cargo-check began (`$/progress` "begin").
    check_started_at: Option<std::time::Instant>,
    /// Queue latency of the current check (didSave → begin), captured at begin.
    check_queued: std::time::Duration,
    /// Completed flycheck spans `(queued, ran)`, drained by the app into the
    /// Activity log — the post-save "Checking…" wall time that no in-app
    /// recorder can wrap (it runs inside rust-analyzer).
    pub finished_checks: Vec<(std::time::Duration, std::time::Duration)>,
    /// Most recent completion items from rust-analyzer.
    pub completion_items: Vec<CompletionItem>,
    /// Set to `true` when a completion response (success OR error) arrives.
    pub completion_response_received: bool,
    /// The request id of the pending completion request, if any.
    completion_req_id: Option<u64>,
    /// Counter for outgoing requests (starts at 1; incremented before each send → first = 2).
    next_req_id: u64,
    /// When the last completion request was sent (for spinner timeout).
    pub completion_request_sent_at: Option<std::time::Instant>,
    /// The request id of the pending `textDocument/rename`, if any.
    rename_req_id: Option<u64>,
    /// Set when a rename response (success OR error) arrives; the app then
    /// applies `rename_edits` and clears this.
    pub rename_response_received: bool,
    /// The edits returned by the last rename (empty on error / no-op).
    pub rename_edits: Vec<RenameEdit>,
    /// The request id of the pending `textDocument/codeAction`, if any.
    code_action_req_id: Option<u64>,
    /// Set when a codeAction list response arrives; consumed by the app.
    pub code_action_response_received: bool,
    /// The code actions returned by the last request.
    pub code_actions: Vec<CodeAction>,
    /// The request id of the pending `codeAction/resolve`, if any.
    code_action_resolve_req_id: Option<u64>,
    /// Set when a resolve response arrives; consumed by the app.
    pub code_action_resolve_received: bool,
    /// The resolved edits (`None` when resolve produced no edit).
    pub code_action_resolved: Option<Vec<RenameEdit>>,
    /// The request id of the pending `textDocument/definition`, if any.
    definition_req_id: Option<u64>,
    /// The request id of the pending `textDocument/implementation` (Ctrl+F12),
    /// if any. Its response funnels into the SAME `definition_result` slot, so
    /// the whole F12 navigation pipeline downstream serves both.
    implementation_req_id: Option<u64>,
    /// Set when a definition response arrives; consumed by the app.
    pub definition_response_received: bool,
    /// The definition target from the last F12, if any.
    pub definition_result: Option<DefinitionLoc>,
    /// The request id of the pending `textDocument/documentSymbol`, if any.
    symbols_req_id: Option<u64>,
    /// The rel_path the pending/last `symbols_result` was requested for.
    symbols_for_file: String,
    /// Set when a documentSymbol response arrives; consumed by the app.
    pub symbols_response_received: bool,
    pub symbols_result: Vec<SymbolInfo>,
    /// In-flight `textDocument/references` requests: request id → the caller's
    /// own index for that symbol (its position in the app's item list) — lets
    /// many reference lookups run concurrently for one file (one per symbol),
    /// unlike the single-slot `_req_id` fields above.
    references_pending: HashMap<u64, usize>,
    /// Completed reference results, keyed by that same index; drained by the app.
    pub references_results: HashMap<usize, Vec<ReferenceLoc>>,
    /// The running rust-analyzer process. Held so it can be KILLED on restart /
    /// app exit — dropping a `std::process::Child` only detaches it (it does NOT
    /// terminate the process), which used to leave orphaned rust-analyzer
    /// instances accumulating across restarts, each still watching and
    /// re-analyzing the workspace on every file write.
    child: Option<std::process::Child>,
}

impl Default for LspState {
    fn default() -> Self {
        Self {
            status: LspStatus::Stopped,
            diagnostics: HashMap::new(),
            generation: 0,
            sender: None,
            open_files: HashMap::new(),
            awaiting_diagnostics: std::collections::HashSet::new(),
            edit_gen: 0,
            check_begin_gen: 0,
            fresh_check_gen: 0,
            root_uri: String::new(),
            checking: false,
            last_did_save_at: None,
            check_started_at: None,
            check_queued: std::time::Duration::ZERO,
            finished_checks: Vec::new(),
            completion_items: Vec::new(),
            completion_response_received: false,
            completion_req_id: None,
            next_req_id: 1,
            completion_request_sent_at: None,
            rename_req_id: None,
            rename_response_received: false,
            rename_edits: Vec::new(),
            code_action_req_id: None,
            code_action_response_received: false,
            code_actions: Vec::new(),
            code_action_resolve_req_id: None,
            code_action_resolve_received: false,
            code_action_resolved: None,
            definition_req_id: None,
            implementation_req_id: None,
            definition_response_received: false,
            definition_result: None,
            symbols_req_id: None,
            symbols_for_file: String::new(),
            symbols_response_received: false,
            symbols_result: Vec::new(),
            references_pending: HashMap::new(),
            references_results: HashMap::new(),
            child: None,
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

    /// `true` when flycheck (cargo check) diagnostics may be stale: there has
    /// been a real edit since the last COMPLETED check began, so rustc's
    /// reported line/cols no longer match the current text (a fixed/commented
    /// error lingers on its old line). The inline overlay hides flycheck-sourced
    /// diagnostics while this holds; RA's own (native) diagnostics are re-mapped
    /// on every `didChange`, so they stay visible. Cleared when the next check
    /// completes (`$/progress` "end").
    pub fn flycheck_stale(&self) -> bool {
        self.fresh_check_gen < self.edit_gen
    }

    /// Send `textDocument/didOpen` for `rel_path` and record the text.
    ///
    /// `rel_path` is relative to the workspace root, e.g. `"src/main.rs"`.
    pub fn did_open(&mut self, rel_path: &str, text: &str) {
        if self.sender.is_none() {
            return;
        }
        self.open_files.insert(
            rel_path.to_owned(),
            OpenFileState {
                doc_version: 1,
                last_sent_code: text.to_owned(),
            },
        );
        let uri = format!("{}/{}", self.root_uri, rel_path);
        self.send_raw(
            serde_json::json!({
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
            })
            .to_string(),
        );
    }

    /// Send `textDocument/didChange` for `rel_path` when the text has changed.
    /// Auto-opens the file via `didOpen` if it hasn't been opened yet.
    ///
    /// `force` re-sends (with a bumped version) even when the text is unchanged,
    /// so rust-analyzer re-runs its analysis. `rel_path` is relative to the
    /// workspace root. Returns `true` when a message actually went out
    /// (didOpen or didChange); `false` when the text was already in sync.
    pub fn did_change(&mut self, rel_path: &str, text: &str, force: bool) -> bool {
        if self.sender.is_none() {
            return false;
        }
        // Auto-open the file on first access.
        if !self.open_files.contains_key(rel_path) {
            self.did_open(rel_path, text);
            return true;
        }
        let file = self.open_files.get_mut(rel_path).unwrap();
        let changed = text != file.last_sent_code;
        if !changed && !force {
            return false;
        }
        file.last_sent_code = text.to_owned();
        file.doc_version += 1;
        let version = file.doc_version;
        // A real text change makes the current diagnostics stale (their line/col
        // now cling to shifted/removed code) until RA re-publishes — gate them
        // out via `diagnostics_fresh`. A *forced* no-op re-send (Project Save
        // re-verify with identical text) must NOT mark them stale: RA won't
        // re-publish for unchanged text, so the gate would get stuck hiding
        // perfectly valid diagnostics forever.
        if changed {
            self.awaiting_diagnostics.insert(rel_path.to_owned());
            // A real edit invalidates the last cargo-check's diagnostics until a
            // fresh check runs (see `flycheck_stale`).
            self.edit_gen += 1;
        }
        let uri = format!("{}/{}", self.root_uri, rel_path);
        self.send_raw(
            serde_json::json!({
                "jsonrpc": "2.0",
                "method":  "textDocument/didChange",
                "params": {
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [{ "text": text }],
                }
            })
            .to_string(),
        );
        true
    }

    /// Send `textDocument/didSave` for `rel_path`. With `checkOnSave: true` this
    /// makes rust-analyzer re-run its flycheck (cargo check) so its flycheck
    /// diagnostics refresh — without it they stay frozen at the startup check
    /// and a fixed error lingers forever in the panel.
    pub fn did_save(&mut self, rel_path: &str) {
        if self.sender.is_none() {
            return;
        }
        if !self.open_files.contains_key(rel_path) {
            return;
        }
        // Start the flycheck queue-latency clock (stopped at $/progress begin).
        self.last_did_save_at = Some(std::time::Instant::now());
        let uri = format!("{}/{}", self.root_uri, rel_path);
        self.send_raw(
            serde_json::json!({
                "jsonrpc": "2.0",
                "method":  "textDocument/didSave",
                "params":  { "textDocument": { "uri": uri } }
            })
            .to_string(),
        );
    }

    /// Seconds the current cargo-check pass has been running, while `checking`.
    /// Drives the live "Checking… Ns" status label.
    pub fn checking_elapsed_secs(&self) -> Option<u64> {
        self.check_started_at.map(|t| t.elapsed().as_secs())
    }

    /// True between a `didSave` and rust-analyzer's `$/progress begin` for the
    /// flycheck it triggers — the QUEUE phase, before `checking` turns on.
    /// The status bar treats this as "Checking…" too: it used to be a gap with
    /// no spinner (and thus no scheduled repaint), where the app could sleep.
    pub fn flycheck_pending(&self) -> bool {
        self.last_did_save_at.is_some()
    }

    /// Request completions at the given cursor position in `rel_path`.
    ///
    /// `trigger_char = None`  → manual invocation (Ctrl+Space, triggerKind=1)
    /// `trigger_char = Some(c)` → auto-trigger (typed `.` or `:`, triggerKind=2)
    /// `rel_path` is relative to the workspace root, e.g. `"src/main.rs"`.
    pub fn request_completion(
        &mut self,
        rel_path: &str,
        line: u32,
        character: u32,
        trigger_char: Option<char>,
    ) {
        if self.sender.is_none() {
            return;
        }
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
        self.send_raw(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id":      id,
                "method":  "textDocument/completion",
                "params":  params
            })
            .to_string(),
        );
    }

    /// Request a project-wide rename of the symbol at `(line, character)` in
    /// `rel_path` to `new_name` (`textDocument/rename`). The result arrives
    /// asynchronously; poll [`take_rename_result`].
    pub fn request_rename(&mut self, rel_path: &str, line: u32, character: u32, new_name: &str) {
        if self.sender.is_none() {
            return;
        }
        self.next_req_id += 1;
        let id = self.next_req_id;
        self.rename_req_id = Some(id);
        self.rename_response_received = false;
        self.rename_edits.clear();
        let uri = format!("{}/{}", self.root_uri, rel_path);
        self.send_raw(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id":      id,
                "method":  "textDocument/rename",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "newName":  new_name,
                }
            })
            .to_string(),
        );
    }

    /// Take the rename edits once RA has responded, clearing the pending state.
    /// Returns `None` while no response is ready (and `Some(vec![])` for a
    /// no-op / failed rename, so the caller can stop waiting).
    pub fn take_rename_result(&mut self) -> Option<Vec<RenameEdit>> {
        if self.rename_response_received {
            self.rename_response_received = false;
            Some(std::mem::take(&mut self.rename_edits))
        } else {
            None
        }
    }

    /// Request the assists / quick-fixes available at `(line, character)` in
    /// `rel_path` (`textDocument/codeAction`, Ctrl+Enter). A zero-width range at
    /// the cursor is enough for import/qualify assists. Poll
    /// [`take_code_actions_result`].
    pub fn request_code_actions(&mut self, rel_path: &str, line: u32, character: u32) {
        if self.sender.is_none() {
            return;
        }
        self.next_req_id += 1;
        let id = self.next_req_id;
        self.code_action_req_id = Some(id);
        self.code_action_response_received = false;
        self.code_actions.clear();
        let uri = format!("{}/{}", self.root_uri, rel_path);
        let pos = serde_json::json!({ "line": line, "character": character });
        self.send_raw(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id":      id,
                "method":  "textDocument/codeAction",
                "params": {
                    "textDocument": { "uri": uri },
                    "range":        { "start": pos, "end": pos },
                    // No diagnostics in context (v1 = assists at the cursor, not
                    // diagnostic quick-fixes); `only` unset → RA returns all.
                    "context":      { "diagnostics": [] },
                }
            })
            .to_string(),
        );
    }

    /// Take the code-action list once RA responded (`Some(vec)`; empty = none).
    pub fn take_code_actions_result(&mut self) -> Option<Vec<CodeAction>> {
        if self.code_action_response_received {
            self.code_action_response_received = false;
            Some(std::mem::take(&mut self.code_actions))
        } else {
            None
        }
    }

    /// Resolve a lazily-returned code action (`codeAction/resolve`). RA requires
    /// the WHOLE action object back (with its `data`), so `action_raw` is sent
    /// verbatim. Poll [`take_code_action_resolve_result`].
    pub fn request_code_action_resolve(&mut self, action_raw: serde_json::Value) {
        if self.sender.is_none() {
            return;
        }
        self.next_req_id += 1;
        let id = self.next_req_id;
        self.code_action_resolve_req_id = Some(id);
        self.code_action_resolve_received = false;
        self.code_action_resolved = None;
        self.send_raw(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id":      id,
                "method":  "codeAction/resolve",
                "params":  action_raw,
            })
            .to_string(),
        );
    }

    /// Take the resolved edits (`Some(Some(edits))` = resolved, `Some(None)` =
    /// resolve produced no edit, `None` = still waiting).
    pub fn take_code_action_resolve_result(&mut self) -> Option<Option<Vec<RenameEdit>>> {
        if self.code_action_resolve_received {
            self.code_action_resolve_received = false;
            Some(self.code_action_resolved.take())
        } else {
            None
        }
    }

    /// Request the definition of the symbol at `(line, character)` in `rel_path`
    /// (`textDocument/definition`). Result arrives async; poll
    /// [`take_definition_result`].
    pub fn request_definition(&mut self, rel_path: &str, line: u32, character: u32) {
        if self.sender.is_none() {
            return;
        }
        self.next_req_id += 1;
        let id = self.next_req_id;
        self.definition_req_id = Some(id);
        self.definition_response_received = false;
        self.definition_result = None;
        let uri = format!("{}/{}", self.root_uri, rel_path);
        self.send_raw(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id":      id,
                "method":  "textDocument/definition",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }
            })
            .to_string(),
        );
    }

    /// Request the implementation(s) of the symbol at `(line, character)` —
    /// `textDocument/implementation` (Ctrl+F12). Where F12 on a trait method
    /// lands on the trait's declaration, this resolves the `impl … for …`
    /// sites instead (the first one, when several exist). Shares the
    /// definition result slot: poll [`take_definition_result`].
    pub fn request_implementation(&mut self, rel_path: &str, line: u32, character: u32) {
        if self.sender.is_none() {
            return;
        }
        self.next_req_id += 1;
        let id = self.next_req_id;
        self.implementation_req_id = Some(id);
        self.definition_response_received = false;
        self.definition_result = None;
        let uri = format!("{}/{}", self.root_uri, rel_path);
        self.send_raw(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id":      id,
                "method":  "textDocument/implementation",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }
            })
            .to_string(),
        );
    }

    /// Take the definition result once RA responded. `Some(Some(loc))` = found,
    /// `Some(None)` = no definition (stop waiting), `None` = not ready yet.
    pub fn take_definition_result(&mut self) -> Option<Option<DefinitionLoc>> {
        if self.definition_response_received {
            self.definition_response_received = false;
            Some(self.definition_result.take())
        } else {
            None
        }
    }

    /// Request every fn/struct/enum/const/… defined in `rel_path`
    /// (`textDocument/documentSymbol`). Result arrives async; poll
    /// [`take_document_symbols_result`].
    pub fn request_document_symbols(&mut self, rel_path: &str) {
        if self.sender.is_none() {
            return;
        }
        self.next_req_id += 1;
        let id = self.next_req_id;
        self.symbols_req_id = Some(id);
        self.symbols_for_file = rel_path.to_owned();
        self.symbols_response_received = false;
        self.symbols_result.clear();
        let uri = format!("{}/{}", self.root_uri, rel_path);
        self.send_raw(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id":      id,
                "method":  "textDocument/documentSymbol",
                "params": { "textDocument": { "uri": uri } }
            })
            .to_string(),
        );
    }

    /// Take the documentSymbol result once ready: `(rel_path it was requested
    /// for, the flattened item list)` — the caller checks the path in case it
    /// switched files while the request was in flight.
    pub fn take_document_symbols_result(&mut self) -> Option<(String, Vec<SymbolInfo>)> {
        if self.symbols_response_received {
            self.symbols_response_received = false;
            Some((
                self.symbols_for_file.clone(),
                std::mem::take(&mut self.symbols_result),
            ))
        } else {
            None
        }
    }

    /// Request every usage site of the symbol at `(line, character)` in
    /// `rel_path` (`textDocument/references`, declaration excluded). `local_idx`
    /// is an opaque caller-assigned key (e.g. the symbol's index in the app's own
    /// list) used to match this specific result when it arrives — lets many
    /// reference lookups for one file's symbols run concurrently. Poll
    /// [`take_reference_results`].
    pub fn request_references(
        &mut self,
        rel_path: &str,
        line: u32,
        character: u32,
        local_idx: usize,
    ) {
        if self.sender.is_none() {
            return;
        }
        self.next_req_id += 1;
        let id = self.next_req_id;
        self.references_pending.insert(id, local_idx);
        let uri = format!("{}/{}", self.root_uri, rel_path);
        self.send_raw(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id":      id,
                "method":  "textDocument/references",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "context": { "includeDeclaration": false }
                }
            })
            .to_string(),
        );
    }

    /// Drain every reference lookup that has completed since the last call,
    /// keyed by the `local_idx` passed to [`request_references`].
    pub fn take_reference_results(&mut self) -> HashMap<usize, Vec<ReferenceLoc>> {
        std::mem::take(&mut self.references_results)
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
        let stale = self.flycheck_stale();
        self.diagnostics
            .values()
            .flat_map(|v| v.iter())
            .filter(|d| {
                d.severity.is_error()
                    && (d.source == "rust-analyzer" || d.is_rustc_error_code() || !stale)
            })
            .count()
    }

    pub fn total_warnings(&self) -> usize {
        let stale = self.flycheck_stale();
        self.diagnostics
            .values()
            .flat_map(|v| v.iter())
            .filter(|d| {
                d.severity.is_warning()
                    && (d.source == "rust-analyzer" || d.is_rustc_error_code() || !stale)
            })
            .count()
    }

    /// Terminate the rust-analyzer child process: a best-effort polite LSP
    /// `exit` notification, then a guaranteed `kill()` + reap. Called on every
    /// restart (`reset`) and on app exit (`AppIde::on_exit`) — without this the
    /// process outlives us (dropping a `Child` only detaches) and keeps
    /// watching + re-analyzing the workspace forever.
    pub fn kill_child(&mut self) {
        // Best effort — the write thread may or may not deliver this before the
        // kill lands; the kill below is the guarantee.
        self.send_raw(r#"{"jsonrpc":"2.0","method":"exit"}"#.to_owned());
        if let Some(mut child) = self.child.take() {
            // Kill the whole process TREE first (/T): rust-analyzer spawns
            // helpers (proc-macro server, flycheck cargo) that survive a plain
            // `kill()` of the parent on Windows and then linger as orphans.
            #[cfg(windows)]
            {
                let _ = crate::build::no_window(&mut Command::new("taskkill"))
                    .args(["/F", "/T", "/PID", &child.id().to_string()])
                    .output();
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Stop any running session and reset to `Stopped`.
    /// Incrementing `generation` makes stale background threads bail out
    /// silently without corrupting the fresh state.
    pub fn reset(&mut self) {
        self.kill_child();
        self.generation += 1;
        self.status = LspStatus::Stopped;
        self.diagnostics.clear();
        self.sender = None; // write thread's Receiver will close → it exits
        self.open_files.clear();
        self.awaiting_diagnostics.clear();
        self.edit_gen = 0;
        self.check_begin_gen = 0;
        self.fresh_check_gen = 0;
        self.root_uri = String::new();
        self.checking = false;
        self.last_did_save_at = None;
        self.check_started_at = None;
        self.check_queued = std::time::Duration::ZERO;
        self.finished_checks.clear();
        self.completion_items.clear();
        self.completion_req_id = None;
        self.rename_req_id = None;
        self.rename_response_received = false;
        self.rename_edits.clear();
        self.definition_req_id = None;
        self.implementation_req_id = None;
        self.definition_response_received = false;
        self.definition_result = None;
        self.symbols_req_id = None;
        self.symbols_for_file.clear();
        self.symbols_response_received = false;
        self.symbols_result.clear();
        self.code_action_req_id = None;
        self.code_action_response_received = false;
        self.code_actions.clear();
        self.code_action_resolve_req_id = None;
        self.code_action_resolve_received = false;
        self.code_action_resolved = None;
        self.references_pending.clear();
        self.references_results.clear();
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Prepare the shared state and spawn `rust-analyzer` for `workspace_dir`.
///
/// The workspace files (Cargo.toml, src/main.rs, …) must already exist on disk.
/// Call `lsp_state.lock().unwrap().did_open(text)` once the status reaches
/// `LspStatus::Indexing`, then `did_change(text)` on every code update.
pub fn start(workspace_dir: &Path, state: Arc<Mutex<LspState>>, ctx: eframe::egui::Context) {
    let workspace_dir = workspace_dir.to_path_buf();
    let root_uri = path_to_uri(&workspace_dir);

    {
        let mut s = state.lock().unwrap();
        // Kill any previous rust-analyzer FIRST — otherwise it lingers as an
        // orphan, still watching + re-analyzing this same workspace on every
        // file write (a main driver of the everything-gets-slower degradation).
        s.kill_child();
        s.generation += 1;
        s.status = LspStatus::Starting;
        s.diagnostics.clear();
        s.root_uri = root_uri.clone();
        s.open_files.clear();
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

    // Reap rust-analyzers orphaned by PREVIOUS sessions (crash, Task-Manager
    // kill, anything that skipped `on_exit`). `kill_child` can only reach the
    // current process's own child — a fresh IDE launch has no handle to
    // yesterday's RA, which keeps watching this same workspace forever.
    // Observed live: three orphaned RA pairs from prior sessions, each
    // re-analyzing every Save and competing for the flycheck target-dir lock —
    // the "save time grows past 20 s over time" degradation. Runs on this
    // background thread (tasklist/taskkill cost ~100 ms each).
    sweep_stale_ras(&workspace_dir);

    let mut child = match crate::build::no_window(&mut Command::new("rust-analyzer"))
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

    // Register the pid so the NEXT launch can reap this RA even if this
    // process dies without running `on_exit` (see `sweep_stale_ras`).
    register_ra_pid(&workspace_dir, child.id());

    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");

    // Channel: any thread with a Sender → write thread → RA stdin
    let (tx, rx) = mpsc::channel::<String>();

    // Store sender + child in LspState (only if our generation is still
    // current). The child handle lives in the shared state so `kill_child`
    // (restart / app exit) can actually terminate the process.
    {
        let mut s = state.lock().unwrap();
        if s.generation != my_gen {
            // Already restarted — this just-spawned RA is already stale; kill
            // it rather than leaking it as an orphan.
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        s.sender = Some(tx.clone());
        s.child = Some(child);
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
                        // We send didSave on flush so RA's `checkOnSave` flycheck
                        // (cargo check) re-runs and refreshes stale flycheck
                        // diagnostics — otherwise it only ran once at startup.
                        "didSave": true,
                    },
                    "publishDiagnostics": {
                        "relatedInformation": false,
                        "versionSupport":     false,
                    },
                    "completion": {
                        "completionItem": {
                            // Snippet completions (`foo(${1:a}, ${2:b})$0`) let
                            // accepting a function insert the full call with
                            // parameters; the editor flattens them via
                            // `editor_panel::snippet::expand` and selects the
                            // first argument.
                            "snippetSupport":      true,
                            "documentationFormat": ["plaintext", "markdown"],
                            "labelDetailsSupport": true,
                        },
                        "completionItemKind": {
                            "valueSet": [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25]
                        },
                        "contextSupport": true,
                    },
                    // Ask RA for the rich, nested `DocumentSymbol[]` shape (with a
                    // separate `range` for the whole item + `selectionRange` for just
                    // the name, and `children` for nested items) instead of the older
                    // flat `SymbolInformation[]` — used to find every fn/struct/enum/
                    // const/… so we can fade unused ones and offer a references list.
                    "documentSymbol": {
                        "hierarchicalDocumentSymbolSupport": true,
                    },
                    // Ctrl+Enter assists / quick-fixes. `codeActionLiteralSupport`
                    // → RA may return `CodeAction` objects (with edits) not just
                    // `Command`s; `resolveSupport` → RA may defer the `edit` and
                    // we fetch it via `codeAction/resolve`.
                    "codeAction": {
                        "codeActionLiteralSupport": {
                            "codeActionKind": {
                                "valueSet": [
                                    "", "quickfix", "refactor", "refactor.extract",
                                    "refactor.inline", "refactor.rewrite", "source"
                                ]
                            }
                        },
                        "resolveSupport": { "properties": ["edit"] },
                    },
                },
                "window": { "workDoneProgress": true },
            },
            "initializationOptions": {
                // cargo-check-on-save is ENABLED so real compiler errors
                // (E0425 "cannot find value", type mismatches, …) show up inline
                // after a Project Save. RA's *native* pass alone does NOT
                // reliably publish these for nested user files, so without
                // flycheck the editor showed no inline errors at all.
                //
                // The save-slowness this once caused was NOT flycheck itself but
                // (a) a leaked serial-reader thread and (b) deleting Cargo.lock on
                // every save, which forced a full dependency re-resolve before
                // each check — both since fixed (Cargo.lock is now kept; see
                // `AppIde::reset_workspace_lock`), so flycheck is a fast
                // *incremental* check that runs asynchronously in RA (it never
                // blocks the app's save). Triggered by the `did_save` sent from
                // `AppIde::flush_lsp_to_workspace`.
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
                //
                // `targetDir: true` → RA runs its cargo (flycheck checkOnSave +
                // build-script probing) in its OWN `target/rust-analyzer/`
                // directory instead of the shared `target/`. Without this, every
                // Save's flycheck held the cargo target-dir file lock, so the
                // Build / Clippy / Flash cargo invocations silently BLOCKED
                // waiting for it — a main driver of the "everything gets slower
                // after a save" degradation. Costs some extra disk space.
                "cargo": {
                    "noDefaultFeatures": false,
                    "targetDir": true,
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
            None => break, // EOF — RA exited
        }
    }

    // RA exited (or we got EOF).
    let mut s = state.lock().unwrap();
    if s.generation == my_gen {
        if s.status.is_active() {
            s.status = LspStatus::Failed("rust-analyzer exited unexpectedly.".into());
            ctx.request_repaint();
        }
        // Reap OUR exited child (releases the process handle). If the
        // generation moved on, `state.child` already belongs to the NEW RA —
        // leave it alone (ours was killed+reaped by `kill_child`).
        if let Some(mut child) = s.child.take() {
            let _ = child.wait();
        }
    }
}

// ── Stale rust-analyzer sweep (pid file) ─────────────────────────────────────
// Every RA spawn is registered in `<workspace>/ra.pids`; every launch first
// kills any registered pid that is STILL a live rust-analyzer. This is what
// catches orphans across app restarts — `kill_child` covers only the clean
// paths (in-session restart, `on_exit`), so a crash or a Task-Manager kill
// used to leave RA pairs alive for days, all re-analyzing this workspace on
// every Save. PID-reuse safety: a pid is killed only after `tasklist` confirms
// its image name is still rust-analyzer.

fn ra_pid_file(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("ra.pids")
}

/// Parse the pid-file contents: one pid per line; junk lines ignored.
fn parse_pid_lines(text: &str) -> Vec<u32> {
    text.lines().filter_map(|l| l.trim().parse().ok()).collect()
}

/// Extract the image name from one `tasklist /FO CSV /NH` output line
/// (`"rust-analyzer.exe","16340",…`) — `None` for the "INFO: No tasks…"
/// message or malformed lines.
fn tasklist_image_name(csv_line: &str) -> Option<String> {
    let first = csv_line.split("\",\"").next()?;
    let name = first.trim().trim_start_matches('"');
    (!name.is_empty() && !name.starts_with("INFO:")).then(|| name.to_owned())
}

/// Kill every pid registered in the workspace pid file that is still a live
/// rust-analyzer (`/T` takes its helper children — proc-macro server,
/// flycheck cargo — down with it), then clear the file. Windows-only; a no-op
/// elsewhere.
fn sweep_stale_ras(workspace_dir: &Path) {
    let path = ra_pid_file(workspace_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    #[cfg(windows)]
    for pid in parse_pid_lines(&text) {
        let is_ra = crate::build::no_window(&mut Command::new("tasklist"))
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .ok()
            .map(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter_map(tasklist_image_name)
                    .any(|name| name.to_lowercase().starts_with("rust-analyzer"))
            })
            .unwrap_or(false);
        if is_ra {
            let _ = crate::build::no_window(&mut Command::new("taskkill"))
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
    }
    #[cfg(not(windows))]
    let _ = text;
    let _ = std::fs::remove_file(&path);
}

/// Append `pid` to the workspace pid file (created on first use).
fn register_ra_pid(workspace_dir: &Path, pid: u32) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ra_pid_file(workspace_dir))
    {
        let _ = writeln!(f, "{pid}");
    }
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
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{}", line);
    }
}
#[cfg(not(debug_assertions))]
fn lsp_log(_: &str) {}

fn handle_incoming(
    msg: serde_json::Value,
    state: &Arc<Mutex<LspState>>,
    ctx: &eframe::egui::Context,
    tx: &mpsc::Sender<String>,
    root_uri: &str,
    my_gen: u64,
) {
    // Guard: if generation advanced we are a stale thread — stop processing.
    if state.lock().unwrap().generation != my_gen {
        return;
    }

    let method = msg["method"].as_str().unwrap_or("");

    // Log all response messages (no "method") for debugging.
    #[cfg(debug_assertions)]
    if method.is_empty() {
        let id = &msg["id"];
        let has_result = msg.get("result").is_some();
        let has_error = msg.get("error").is_some();
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
            let _ = tx.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#.to_owned());
            let mut s = state.lock().unwrap();
            if s.generation == my_gen {
                s.status = LspStatus::Indexing;
            }
            ctx.request_repaint();
        }

        // ── Diagnostics ───────────────────────────────────────────────────────
        "textDocument/publishDiagnostics" => {
            let params = &msg["params"];
            let uri = params["uri"].as_str().unwrap_or("");
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
            let kind = msg["params"]["value"]["kind"].as_str().unwrap_or("");
            let token_raw = msg["params"]["token"].as_str().unwrap_or("");
            // RA may use numeric tokens — convert to string for matching
            let token_num = msg["params"]["token"].as_u64().map(|n| n.to_string());
            let token = token_num.as_deref().unwrap_or(token_raw);

            let is_indexing = token.contains("rust") || token.contains("index");
            let is_check =
                token.contains("cargo") || token.contains("check") || token.contains("flycheck");

            let mut s = state.lock().unwrap();
            if s.generation != my_gen {
                return;
            }

            if is_indexing && kind == "end" && s.status == LspStatus::Indexing {
                s.status = LspStatus::Ready;
                ctx.request_repaint();
            }
            if is_check {
                match kind {
                    "begin" => {
                        s.checking = true;
                        // This check reflects all edits made up to now.
                        s.check_begin_gen = s.edit_gen;
                        // Queue latency: how long RA sat on the didSave before
                        // cargo actually started (a clogged RA shows up HERE,
                        // a slow cargo shows up in the run span).
                        s.check_queued = s
                            .last_did_save_at
                            .take()
                            .map(|t| t.elapsed())
                            .unwrap_or_default();
                        s.check_started_at = Some(std::time::Instant::now());
                        ctx.request_repaint();
                    }
                    "end" => {
                        s.checking = false;
                        // Its diagnostics are now fresh up to the gen it began at;
                        // any edit made *during* the check keeps `flycheck_stale`
                        // true until the next check completes.
                        s.fresh_check_gen = s.check_begin_gen;
                        if let Some(t) = s.check_started_at.take() {
                            let queued = s.check_queued;
                            s.finished_checks.push((queued, t.elapsed()));
                        }
                        ctx.request_repaint();
                    }
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
                if s.generation != my_gen {
                    return;
                }
                // Rename response (a WorkspaceEdit) — must be checked before the
                // completion branch since both are id-keyed result messages.
                if s.rename_req_id == Some(req_id) {
                    s.rename_req_id = None;
                    s.rename_edits = parse_workspace_edit(&msg["result"], root_uri);
                    s.rename_response_received = true;
                    ctx.request_repaint();
                } else if s.definition_req_id == Some(req_id) {
                    s.definition_req_id = None;
                    s.definition_result = parse_definition(&msg["result"]);
                    s.definition_response_received = true;
                    ctx.request_repaint();
                } else if s.implementation_req_id == Some(req_id) {
                    // Same Location | Location[] | LocationLink[] shapes as a
                    // definition response — funneled into the same slot.
                    s.implementation_req_id = None;
                    s.definition_result = parse_definition(&msg["result"]);
                    s.definition_response_received = true;
                    ctx.request_repaint();
                } else if s.symbols_req_id == Some(req_id) {
                    s.symbols_req_id = None;
                    s.symbols_result = parse_document_symbols(&msg["result"]);
                    s.symbols_response_received = true;
                    ctx.request_repaint();
                } else if s.code_action_req_id == Some(req_id) {
                    s.code_action_req_id = None;
                    s.code_actions = parse_code_actions(&msg["result"], root_uri);
                    s.code_action_response_received = true;
                    ctx.request_repaint();
                } else if s.code_action_resolve_req_id == Some(req_id) {
                    s.code_action_resolve_req_id = None;
                    // The resolved action carries its `edit` now.
                    let edits = parse_workspace_edit(&msg["result"]["edit"], root_uri);
                    s.code_action_resolved = (!edits.is_empty()).then_some(edits);
                    s.code_action_resolve_received = true;
                    ctx.request_repaint();
                } else if let Some(local_idx) = s.references_pending.remove(&req_id) {
                    s.references_results
                        .insert(local_idx, parse_references(&msg["result"]));
                    ctx.request_repaint();
                } else if s.completion_req_id == Some(req_id) {
                    s.completion_req_id = None;
                    s.completion_response_received = true;
                    let result = &msg["result"];
                    // CompletionList { items: [...] }  OR  [...] directly
                    // `result` may also be JSON null — treat as empty list.
                    let items_arr = result["items"].as_array().or_else(|| result.as_array());
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
                if s.generation != my_gen {
                    return;
                }
                if s.rename_req_id == Some(req_id) {
                    // Rename failed / not allowed → empty edits, stop waiting.
                    s.rename_req_id = None;
                    s.rename_response_received = true;
                    ctx.request_repaint();
                } else if s.definition_req_id == Some(req_id) {
                    s.definition_req_id = None;
                    s.definition_response_received = true; // no result, stop waiting
                    ctx.request_repaint();
                } else if s.implementation_req_id == Some(req_id) {
                    s.implementation_req_id = None;
                    s.definition_response_received = true; // no result, stop waiting
                    ctx.request_repaint();
                } else if s.symbols_req_id == Some(req_id) {
                    s.symbols_req_id = None;
                    s.symbols_response_received = true; // empty result, stop waiting
                    ctx.request_repaint();
                } else if s.code_action_req_id == Some(req_id) {
                    s.code_action_req_id = None;
                    s.code_action_response_received = true; // empty list, stop waiting
                    ctx.request_repaint();
                } else if s.code_action_resolve_req_id == Some(req_id) {
                    s.code_action_resolve_req_id = None;
                    s.code_action_resolve_received = true; // no edit, stop waiting
                    ctx.request_repaint();
                } else if let Some(local_idx) = s.references_pending.remove(&req_id) {
                    // Treat as "0 references" rather than leaving it pending forever.
                    s.references_results.insert(local_idx, Vec::new());
                    ctx.request_repaint();
                } else if s.completion_req_id == Some(req_id) {
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
    let message = v["message"].as_str()?.to_owned();
    let severity = DiagSeverity::from_lsp(v["severity"].as_u64().unwrap_or(1));
    let start = &v["range"]["start"];
    let end_v = &v["range"]["end"];
    let line = start["line"].as_u64().unwrap_or(0) as u32 + 1;
    let col = start["character"].as_u64().unwrap_or(0) as u32 + 1;
    let end_line = end_v["line"].as_u64().unwrap_or(0) as u32 + 1;
    let end_col = end_v["character"].as_u64().unwrap_or(0) as u32 + 1;
    // code may be a string like "E0308" or an integer
    let code = v["code"]
        .as_str()
        .map(String::from)
        .or_else(|| v["code"].as_u64().map(|n| n.to_string()));
    let source = v["source"].as_str().unwrap_or("").to_owned();
    Some(LspDiagnostic {
        severity,
        message,
        line,
        col,
        end_line,
        end_col,
        code,
        source,
    })
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
        let stripped = s.strip_prefix(r"\\?\").unwrap_or(&s);
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

/// Parse a `textDocument/rename` result (a WorkspaceEdit) into flat edits.
/// Handles both `documentChanges` (RA's default) and the older `changes` map.
fn parse_workspace_edit(result: &serde_json::Value, root_uri: &str) -> Vec<RenameEdit> {
    let mut out = Vec::new();
    if let Some(dcs) = result["documentChanges"].as_array() {
        for dc in dcs {
            // Skip rename/create/delete file ops (they have a "kind"); we only
            // apply text edits to existing documents.
            let uri = dc["textDocument"]["uri"].as_str().unwrap_or("");
            if uri.is_empty() {
                continue;
            }
            let rel = uri_to_rel(uri, root_uri);
            if let Some(edits) = dc["edits"].as_array() {
                out.extend(edits.iter().filter_map(|e| parse_text_edit(e, &rel)));
            }
        }
    } else if let Some(changes) = result["changes"].as_object() {
        for (uri, edits) in changes {
            let rel = uri_to_rel(uri, root_uri);
            if let Some(arr) = edits.as_array() {
                out.extend(arr.iter().filter_map(|e| parse_text_edit(e, &rel)));
            }
        }
    }
    out
}

/// Parse a `textDocument/codeAction` result: `(Command | CodeAction)[]`. Plain
/// `Command`s (no `edit`, no `data`, but a `command` field) are dropped —
/// `is_applicable` filters them anyway. Each `CodeAction`'s inline `edit` is
/// parsed now; lazy ones keep `edits: None` and resolve later.
fn parse_code_actions(result: &serde_json::Value, root_uri: &str) -> Vec<CodeAction> {
    let Some(arr) = result.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|a| {
            let title = a["title"].as_str()?.to_owned();
            let edits = a
                .get("edit")
                .filter(|e| !e.is_null())
                .map(|e| parse_workspace_edit(e, root_uri));
            Some(CodeAction { title, edits, raw: a.clone() })
        })
        .filter(CodeAction::is_applicable)
        .collect()
}

/// Parse a `textDocument/definition` result (Location / Location[] / LocationLink[])
/// into a single target — the first location.
fn parse_definition(result: &serde_json::Value) -> Option<DefinitionLoc> {
    let loc = if result.is_array() {
        result.as_array()?.first()?
    } else if result.is_object() {
        result
    } else {
        return None;
    };
    // Location { uri, range } | LocationLink { targetUri, targetSelectionRange }
    let (uri, range) = if let Some(u) = loc["uri"].as_str() {
        (u, &loc["range"])
    } else if let Some(u) = loc["targetUri"].as_str() {
        let r = if loc["targetSelectionRange"].is_object() {
            &loc["targetSelectionRange"]
        } else {
            &loc["targetRange"]
        };
        (u, r)
    } else {
        return None;
    };
    Some(DefinitionLoc {
        path: uri_to_path(uri),
        line: range["start"]["line"].as_u64()? as u32,
        character: range["start"]["character"].as_u64().unwrap_or(0) as u32,
    })
}

/// Recursively flatten a `textDocument/documentSymbol` response into every
/// trackable named item (fn/struct/enum/const/static/trait/method/field/…,
/// see [`is_trackable_symbol_kind`]), descending into `children` so items
/// nested in a `mod`/`impl`/`trait` body are covered too. Handles both the
/// modern hierarchical `DocumentSymbol[]` shape (has `range` + `selectionRange`
/// + optional `children`) and the older flat `SymbolInformation[]` shape (just
/// `location.range`, used as both spans) some servers fall back to.
fn parse_document_symbols(result: &serde_json::Value) -> Vec<SymbolInfo> {
    /// rust-analyzer names trait-impl block symbols `impl Trait for Type`
    /// (inherent impls are just `impl Type` — no ` for `).
    fn is_trait_impl_symbol(name: &str) -> bool {
        name.starts_with("impl") && name.contains(" for ")
    }

    fn walk(node: &serde_json::Value, out: &mut Vec<SymbolInfo>, in_trait_impl: bool) {
        let name = node["name"].as_str().unwrap_or("").to_owned();
        let kind = node["kind"].as_u64().unwrap_or(0) as u8;
        let child_in_trait_impl = in_trait_impl || is_trait_impl_symbol(&name);

        if let (Some(range), Some(sel)) = (node.get("range"), node.get("selectionRange")) {
            if !name.is_empty() && is_trackable_symbol_kind(kind) {
                if let Some(info) = symbol_from_ranges(name, kind, range, sel, in_trait_impl) {
                    out.push(info);
                }
            }
            if let Some(children) = node["children"].as_array() {
                for c in children {
                    walk(c, out, child_in_trait_impl);
                }
            }
        } else if let Some(loc) = node.get("location") {
            // Flat SymbolInformation — no separate selection span or children.
            if !name.is_empty() && is_trackable_symbol_kind(kind) {
                let r = &loc["range"];
                if let Some(info) = symbol_from_ranges(name, kind, r, r, in_trait_impl) {
                    out.push(info);
                }
            }
        }
    }

    fn symbol_from_ranges(
        name: String,
        kind: u8,
        range: &serde_json::Value,
        sel: &serde_json::Value,
        in_trait_impl: bool,
    ) -> Option<SymbolInfo> {
        Some(SymbolInfo {
            name,
            kind,
            start_line: range["start"]["line"].as_u64()? as u32,
            start_char: range["start"]["character"].as_u64()? as u32,
            end_line: range["end"]["line"].as_u64()? as u32,
            end_char: range["end"]["character"].as_u64()? as u32,
            sel_line: sel["start"]["line"].as_u64()? as u32,
            sel_char: sel["start"]["character"].as_u64()? as u32,
            in_trait_impl,
        })
    }

    let mut out = Vec::new();
    if let Some(arr) = result.as_array() {
        for node in arr {
            walk(node, &mut out, false);
        }
    }
    out
}

/// Parse a `textDocument/references` result (`Location[]`) into usage sites.
fn parse_references(result: &serde_json::Value) -> Vec<ReferenceLoc> {
    result
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|loc| {
                    let uri = loc["uri"].as_str()?;
                    let r = &loc["range"];
                    Some(ReferenceLoc {
                        path: uri_to_path(uri),
                        line: r["start"]["line"].as_u64()? as u32,
                        character: r["start"]["character"].as_u64().unwrap_or(0) as u32,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Decode a `file://` URI to a filesystem path (with minimal `%XX` decoding).
fn uri_to_path(uri: &str) -> String {
    let mut p = uri.strip_prefix("file://").unwrap_or(uri).to_owned();
    // Percent-decode common sequences (spaces etc.) without a full URL crate.
    if p.contains('%') {
        p = percent_decode(&p);
    }
    #[cfg(windows)]
    {
        // file:///C:/foo → /C:/foo → C:/foo, then backslashes.
        let stripped = p.strip_prefix('/').unwrap_or(&p);
        stripped.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        p
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_text_edit(e: &serde_json::Value, rel: &str) -> Option<RenameEdit> {
    let s = &e["range"]["start"];
    let en = &e["range"]["end"];
    Some(RenameEdit {
        rel_path: rel.to_owned(),
        start_line: s["line"].as_u64()? as u32,
        start_char: s["character"].as_u64()? as u32,
        end_line: en["line"].as_u64()? as u32,
        end_char: en["character"].as_u64()? as u32,
        new_text: e["newText"].as_str().unwrap_or("").to_owned(),
    })
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
            let ld_desc = v["labelDetails"]["description"].as_str().unwrap_or("");
            match (ld_detail.is_empty(), ld_desc.is_empty()) {
                (false, false) => format!("{ld_detail}  {ld_desc}"),
                (false, true) => ld_detail.to_owned(),
                (true, false) => ld_desc.to_owned(),
                (true, true) => String::new(),
            }
        }
    };

    // rust-analyzer delivers the replacement text through `textEdit.newText`
    // (both the plain-`TextEdit` and `InsertReplaceEdit` shapes carry it);
    // a bare `insertText` is the exception. Falling back to `label` used to be
    // harmless when labels were plain names, but with `snippetSupport` on RA
    // labels callables as `name(…)` — inserting THAT literally is a bug, so
    // the fallback chain must prefer the real edit text.
    let insert_text = v["textEdit"]["newText"]
        .as_str()
        .or_else(|| v["insertText"].as_str())
        .map(|s| s.to_owned())
        .unwrap_or_else(|| label.clone());

    // LSP InsertTextFormat: 1 = PlainText (default when absent), 2 = Snippet.
    let insert_is_snippet = v["insertTextFormat"].as_u64() == Some(2);

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

    Some(CompletionItem {
        label,
        kind,
        detail,
        insert_text,
        insert_is_snippet,
        documentation,
    })
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
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        } else {
            // Inside a code fence: keep the code but strip leading indent.
            if !out.is_empty() {
                out.push('\n');
            }
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
            source: String::new(),
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

    fn diag_with_code(code: Option<&str>) -> LspDiagnostic {
        let mut d = diag("x");
        d.code = code.map(String::from);
        d
    }

    #[test]
    fn numbered_compiler_codes_are_rustc_error_codes() {
        assert!(diag_with_code(Some("E0425")).is_rustc_error_code());
        assert!(diag_with_code(Some("E0308")).is_rustc_error_code());
    }

    #[test]
    fn named_lints_are_not_rustc_error_codes() {
        // These require an actual cargo-check/clippy pass (confirmed empirically
        // — see `is_rustc_error_code`'s doc comment) and must stay gated by
        // `flycheck_stale`, unlike numbered hard errors.
        assert!(!diag_with_code(Some("unused_variables")).is_rustc_error_code());
        assert!(!diag_with_code(Some("dead_code")).is_rustc_error_code());
        assert!(!diag_with_code(Some("clippy::needless_return")).is_rustc_error_code());
    }

    #[test]
    fn missing_or_malformed_code_is_not_a_rustc_error_code() {
        assert!(!diag_with_code(None).is_rustc_error_code());
        assert!(
            !diag_with_code(Some("E")).is_rustc_error_code(),
            "no digits after E"
        );
        assert!(
            !diag_with_code(Some("E12a4")).is_rustc_error_code(),
            "non-digit in the code"
        );
    }
}

#[cfg(test)]
mod ra_sweep_tests {
    use super::{parse_pid_lines, tasklist_image_name};

    #[test]
    fn pid_lines_parse_and_skip_junk() {
        assert_eq!(parse_pid_lines("16340\n22308\n"), vec![16340, 22308]);
        assert_eq!(parse_pid_lines("  123 \n\nnot-a-pid\n77"), vec![123, 77]);
        assert!(parse_pid_lines("").is_empty());
    }

    #[test]
    fn tasklist_csv_yields_image_name() {
        let line = r#""rust-analyzer.exe","16340","Console","1","540,120 K""#;
        assert_eq!(tasklist_image_name(line).as_deref(), Some("rust-analyzer.exe"));
    }

    #[test]
    fn tasklist_info_line_is_rejected() {
        // `tasklist /FI "PID eq X"` prints this when the pid no longer exists —
        // it must NOT look like a process (or a dead pid would get "killed",
        // i.e. taskkill run against a possibly reused pid).
        let line = "INFO: No tasks are running which match the specified criteria.";
        assert_eq!(tasklist_image_name(line), None);
        assert_eq!(tasklist_image_name(""), None);
    }
}

#[cfg(test)]
mod document_symbol_tests {
    use super::parse_document_symbols;

    fn range(l0: u64, c0: u64, l1: u64, c1: u64) -> serde_json::Value {
        serde_json::json!({ "start": { "line": l0, "character": c0 },
                            "end":   { "line": l1, "character": c1 } })
    }

    fn sym(name: &str, kind: u64, children: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "kind": kind,
            "range": range(0, 0, 9, 0),
            "selectionRange": range(0, 3, 0, 8),
            "children": children,
        })
    }

    /// Members of `impl Trait for Type` blocks are flagged (they must never be
    /// faded as unused — generic dispatch references bind to the trait), while
    /// trait declarations and inherent-impl methods are not.
    #[test]
    fn trait_impl_members_are_flagged() {
        let result = serde_json::json!([
            // The user's exact shape: trait in one file's symbols…
            sym("ParserResult", 11, serde_json::json!([
                sym("new_parser", 12, serde_json::json!([])),
            ])),
            // …its implementation (impl block = kind 19 Object, not tracked
            // itself; its children are).
            sym(
                "impl ParserResult<PAYLOAD_LEN, HAS_CMD_ID, RESERVED_LEN, HmmdFrame> for HmmdFrame",
                19,
                serde_json::json!([
                    sym("new_parser", 12, serde_json::json!([])),
                    sym("decode", 12, serde_json::json!([])),
                ]),
            ),
            // Inherent impl — members keep normal unused-fading behaviour.
            sym("impl HmmdFrame", 19, serde_json::json!([
                sym("helper", 12, serde_json::json!([])),
            ])),
        ]);

        let syms = parse_document_symbols(&result);
        let flag = |name: &str, expect: bool, nth: usize| {
            let s = syms.iter().filter(|s| s.name == name).nth(nth).unwrap();
            assert_eq!(s.in_trait_impl, expect, "{name} #{nth}");
        };
        flag("ParserResult", false, 0); // the trait itself
        flag("new_parser", false, 0); // trait's own declaration
        flag("new_parser", true, 1); // impl-for member
        flag("decode", true, 0); // impl-for member
        flag("helper", false, 0); // inherent impl member
    }
}

#[cfg(test)]
mod completion_item_tests {
    use super::parse_completion_item;

    /// rust-analyzer's usual shape: the replacement lives in `textEdit.newText`
    /// (no `insertText`), the label is the display form `name(…)`. The parser
    /// must take the edit text — inserting the label is the reported bug
    /// (`get_param_value(…)` appearing literally in code).
    #[test]
    fn text_edit_new_text_wins_over_label() {
        let v = serde_json::json!({
            "label": "get_param_value(…)",
            "kind": 3,
            "detail": "fn get_param_value(tx: &mut T) -> Option<u32>",
            "insertTextFormat": 2,
            "textEdit": {
                "range": { "start": { "line": 0, "character": 0 },
                           "end":   { "line": 0, "character": 5 } },
                "newText": "get_param_value(${1:tx})$0",
            },
        });
        let item = parse_completion_item(&v).expect("parses");
        assert_eq!(item.insert_text, "get_param_value(${1:tx})$0");
        assert!(item.insert_is_snippet);
    }

    /// `InsertReplaceEdit` also carries `newText` at the same key.
    #[test]
    fn insert_replace_edit_new_text_is_read() {
        let v = serde_json::json!({
            "label": "foo(…)",
            "kind": 3,
            "textEdit": {
                "insert":  { "start": { "line": 0, "character": 0 },
                             "end":   { "line": 0, "character": 3 } },
                "replace": { "start": { "line": 0, "character": 0 },
                             "end":   { "line": 0, "character": 3 } },
                "newText": "foo($1)$0",
            },
        });
        let item = parse_completion_item(&v).expect("parses");
        assert_eq!(item.insert_text, "foo($1)$0");
    }

    /// Fallback chain: `insertText` when no `textEdit`, label as last resort.
    #[test]
    fn fallback_chain_insert_text_then_label() {
        let with_insert = serde_json::json!({
            "label": "bar(…)",
            "kind": 3,
            "insertText": "bar",
        });
        let item = parse_completion_item(&with_insert).expect("parses");
        assert_eq!(item.insert_text, "bar");
        assert!(!item.insert_is_snippet, "no insertTextFormat → plain text");

        let bare = serde_json::json!({ "label": "baz", "kind": 6 });
        let item = parse_completion_item(&bare).expect("parses");
        assert_eq!(item.insert_text, "baz");
    }
}
