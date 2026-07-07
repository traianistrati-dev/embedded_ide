use crate::build::BuildState;
use crate::dfu::{self, DfuState};
use crate::espflash::EspFlashState;
use crate::lsp::{self, LspStatus};
use crate::openocd::OpenOcdState;
use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use crate::panels::mcu_module::mcu_def::{McuDefinition, ProjectDef};
use crate::panels::mcu_module::{project_gen, project_gen::ProjectFiles, registry};
use crate::project_tree::ProjectTreeState;
use crate::required_tools;
use eframe::egui;
use egui_code_editor::{Completer, Syntax};
use notify::Watcher as _;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

// ── Module structure ──────────────────────────────────────────────────────────
mod tabs;

pub(crate) mod helpers;
use helpers::apply_dark_theme;

mod dialogs;

mod diag_panel;

mod project_panel;

mod mcu_panel;

mod editor_panel;

mod project_io;

// ── Project file selector ─────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug, Default)]
pub enum ProjectFileId {
    #[default]
    MainRs,
    CargoToml,
    CargoConfig,
    MemoryX,
    BuildRs,
    GitIgnore,
    /// Index into `AppIde::user_src_files`
    UserFile(usize),
}

impl ProjectFileId {
    fn label(self) -> &'static str {
        match self {
            Self::MainRs => "src/main.rs",
            Self::CargoToml => "Cargo.toml",
            Self::CargoConfig => ".cargo/config.toml",
            Self::MemoryX => "memory.x",
            Self::BuildRs => "build.rs",
            Self::GitIgnore => ".gitignore",
            Self::UserFile(_) => "src/???", // resolved at call site
        }
    }

    fn content<'a>(self, files: &'a ProjectFiles) -> &'a str {
        match self {
            Self::MainRs => &files.main_rs,
            Self::CargoToml => &files.cargo_toml,
            Self::CargoConfig => &files.cargo_config,
            Self::MemoryX => &files.memory_x,
            Self::BuildRs => &files.build_rs,
            Self::GitIgnore => &files.gitignore,
            Self::UserFile(_) => "", // handled separately before calling this
        }
    }

    fn syntax(self) -> Syntax {
        match self {
            // TOML (Cargo.toml, .cargo/config.toml) and .gitignore use `#` line
            // comments — give them a syntax whose comment marker is `#` so those
            // lines render in the comment (gray) colour, matching `//` comments
            // in .rs files (same theme → same Comment token colour).
            Self::CargoToml | Self::CargoConfig | Self::GitIgnore => Syntax::simple("#"),
            // main.rs / build.rs are Rust; memory.x uses C-style `/* */`, which
            // Rust highlighting already renders as comments.
            _ => Syntax::rust(),
        }
    }

    /// Path as reported by `rustc` in JSON diagnostics (relative to project root).
    /// Returns `None` for files that rustc never reports errors for.
    pub fn cargo_path(self) -> Option<&'static str> {
        match self {
            Self::MainRs => Some("src/main.rs"),
            Self::BuildRs => Some("build.rs"),
            Self::CargoToml => Some("Cargo.toml"),
            _ => None,
        }
    }
}

/// Translucent band colour for a clicked diagnostic's line in the editor, keyed
/// by severity — same alpha (26 ≈ 0.1) as the error red. Error → red,
/// Warning → yellow, Info / Hint → blue.
pub fn diag_highlight_color(sev: lsp::DiagSeverity) -> egui::Color32 {
    match sev {
        lsp::DiagSeverity::Error => egui::Color32::from_rgba_unmultiplied(255, 0, 100, 26),
        lsp::DiagSeverity::Warning => egui::Color32::from_rgba_unmultiplied(255, 210, 0, 26),
        lsp::DiagSeverity::Info | lsp::DiagSeverity::Hint => {
            egui::Color32::from_rgba_unmultiplied(60, 150, 255, 26)
        }
    }
}

/// Resolve a diagnostic's project-relative path (as reported by rustc /
/// rust-analyzer) to the editor file it should open — including user source
/// files under `src/`. `user_files` is `(name, content)` where `name` is the
/// path below `src/` (e.g. `pins.rs`).
pub fn resolve_diag_file(path: &str, user_files: &[(String, String)]) -> Option<ProjectFileId> {
    match path {
        "src/main.rs" => Some(ProjectFileId::MainRs),
        "build.rs" => Some(ProjectFileId::BuildRs),
        "Cargo.toml" => Some(ProjectFileId::CargoToml),
        "memory.x" => Some(ProjectFileId::MemoryX),
        ".cargo/config.toml" => Some(ProjectFileId::CargoConfig),
        ".gitignore" => Some(ProjectFileId::GitIgnore),
        _ => user_files
            .iter()
            .position(|(name, _)| path == format!("src/{name}") || path == name)
            .map(ProjectFileId::UserFile),
    }
}

/// Byte ranges of main.rs's GENERATED blocks (`GEN_BEGIN` marker → end of the
/// `GEN_END` marker line). The code inside is owned by the MCU Configurator and
/// regenerated on config changes, so a Clippy auto-fix landing there must be
/// blocked (it would be reverted). A fix span `[s, e)` is "locked" when it
/// overlaps any returned range: `s < range.1 && e > range.0`.
pub fn generated_byte_ranges(code: &str) -> Vec<(usize, usize)> {
    use crate::panels::mcu_module::codegen::common::{GEN_BEGIN, GEN_END};
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(rel_b) = code[from..].find(GEN_BEGIN) {
        let b = from + rel_b;
        let after_begin = b + GEN_BEGIN.len();
        match code[after_begin..].find(GEN_END) {
            Some(rel_e) => {
                let end = after_begin + rel_e + GEN_END.len();
                ranges.push((b, end));
                from = end;
            }
            None => {
                // Unterminated block — lock to end of file.
                ranges.push((b, code.len()));
                break;
            }
        }
    }
    ranges
}

/// Splice `edits` into `buf` (one file's content), largest-offset first so that
/// earlier byte offsets stay valid as the text changes. An edit is skipped when
/// its span is reversed, out of bounds, off a UTF-8 char boundary, overlaps an
/// already-applied edit, or overlaps any `locked` byte range (main.rs's
/// GENERATED block). Returns how many edits were applied.
pub fn apply_edits_to_buffer(
    buf: &mut String,
    edits: &[&crate::build::SpanEdit],
    locked: &[(usize, usize)],
) -> usize {
    let mut sorted: Vec<&crate::build::SpanEdit> = edits.to_vec();
    sorted.sort_by(|a, b| b.start.cmp(&a.start));
    let mut applied = 0usize;
    let mut last_start = buf.len() + 1;
    for e in sorted {
        if e.start > e.end || e.end > buf.len() {
            continue;
        }
        if e.end > last_start {
            continue; // overlaps an already-applied (later) edit
        }
        if !buf.is_char_boundary(e.start) || !buf.is_char_boundary(e.end) {
            continue;
        }
        if locked.iter().any(|&(b, end)| e.start < end && e.end > b) {
            continue; // inside the GENERATED block — never edit
        }
        buf.replace_range(e.start..e.end, &e.replacement);
        last_start = e.start;
        applied += 1;
    }
    applied
}

// ── Tab bar ──────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug)]
enum McuTab {
    Pins,
    Peripherals,
    Clock,
    System,
}

impl McuTab {
    fn label(self) -> &'static str {
        match self {
            Self::Pins => "Pins",
            Self::Peripherals => "Peripherals",
            Self::Clock => "Clock",
            Self::System => "System",
        }
    }
}

// ── Build panel tab ──────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug, Default)]
enum BuildPanelTab {
    #[default]
    RustAnalyzer,
    Cargo,
    Dfu,
    /// Built-in USART/UART serial console.
    Serial,
    /// `cargo clippy` improvement suggestions.
    Clippy,
    /// Built-in command console (streaming `powershell` runner).
    Terminal,
    /// Per-action timing breakdown (Save / Build / Flash / Clippy).
    Activity,
    /// Git status / commit / push / pull in the project directory.
    Git,
    RequiredTools,
    /// F12 "Go to definition" result. Only selectable while `definition_view` is
    /// set (the tab is hidden otherwise).
    Definition,
}

/// The source snippet shown in the F12 "Definition" bottom tab.
struct DefinitionView {
    /// Header line, e.g. `src/pins/utils/i2c1.rs  (line 42)`.
    header: String,
    /// The code snippet around the definition.
    code: String,
    /// 0-based index (within `code`'s lines) of the definition line, drawn
    /// coloured so it stands out from the rest.
    highlight: usize,
}

// ── Persisted project state ───────────────────────────────────────────────────
// Everything that must survive an application restart.
// Stored via eframe's platform storage (Registry on Windows, ~/.local on Linux).

const STORAGE_KEY: &str = "embedded_ide_project_v1";

/// Byte offset in `text` of the 0-based LSP position `(line, character)`.
/// Character is treated as a char count (correct for ASCII identifiers), clamped
/// to the line's end.
fn lsp_pos_to_byte(text: &str, line: u32, character: u32) -> usize {
    let mut offset = 0usize;
    for (li, l) in text.split_inclusive('\n').enumerate() {
        if li as u32 == line {
            let mut c = 0u32;
            for (b, _) in l.char_indices() {
                if c == character {
                    return offset + b;
                }
                c += 1;
            }
            return offset + l.trim_end_matches('\n').len(); // char beyond EOL
        }
        offset += l.len();
    }
    text.len()
}

/// If a definition's absolute path points to a file in the *current project*
/// (under the LSP workspace `…/embedded_ide_0_check/`), return its editor file
/// id — so F12 can open it editable in the main editor instead of the read-only
/// snippet tab. `None` for external files (crates / std), which keep the snippet.
fn project_file_for_def(abs_path: &str, user_files: &[(String, String)]) -> Option<ProjectFileId> {
    let ws = std::env::temp_dir().join("embedded_ide_0_check");
    let ws_str = ws.to_string_lossy().replace('\\', "/");
    let p = abs_path.replace('\\', "/");
    let prefix = format!("{ws_str}/");
    // Case-insensitive prefix (Windows drive-letter / temp-dir case can differ).
    let head = p.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(&prefix) {
        return None;
    }
    resolve_diag_file(&p[prefix.len()..], user_files)
}

/// Read the file a definition points to and build the view shown in the
/// "Definition" tab. The WHOLE file is included (so the user can scroll above
/// and below the target); the tab scrolls the target near the top on open.
/// `None` if unreadable.
fn build_definition_view(loc: &lsp::DefinitionLoc) -> Option<DefinitionView> {
    let content = std::fs::read_to_string(&loc.path).ok()?;
    let line_count = content.lines().count();
    if line_count == 0 {
        return None;
    }
    let target = (loc.line as usize).min(line_count - 1);
    Some(DefinitionView {
        header: format!("{}  (line {})", short_path(&loc.path), loc.line + 1),
        code: content,     // full file
        highlight: target, // the def line's index in the file (0-based)
    })
}

/// Shorten a definition path for the tab header so it's clear which crate it
/// comes from: the crate dir (the segment just before `/src/`) + the `src/…`
/// tail, e.g. `stm32f1xx-hal-0.10.0/src/gpio.rs` instead of a bare `src/gpio.rs`.
/// Falls back to the bare file name when there's no `/src/`.
fn short_path(path: &str) -> String {
    let norm = path.replace('\\', "/");
    if let Some(i) = norm.rfind("/src/") {
        let crate_dir = norm[..i].rsplit('/').next().unwrap_or("");
        let tail = &norm[i + 1..]; // "src/…"
        if crate_dir.is_empty() {
            tail.to_string()
        } else {
            format!("{crate_dir}/{tail}")
        }
    } else {
        norm.rsplit('/').next().unwrap_or(&norm).to_string()
    }
}

/// Apply LSP text edits to `text`. Edits are non-overlapping; applying them
/// back-to-front (by start position) keeps earlier offsets valid.
fn apply_text_edits(text: &str, mut edits: Vec<lsp::RenameEdit>) -> String {
    edits.sort_by(|a, b| (b.start_line, b.start_char).cmp(&(a.start_line, a.start_char)));
    let mut s = text.to_owned();
    for e in edits {
        let start = lsp_pos_to_byte(&s, e.start_line, e.start_char);
        let end = lsp_pos_to_byte(&s, e.end_line, e.end_char);
        if start <= end && end <= s.len() {
            s.replace_range(start..end, &e.new_text);
        }
    }
    s
}

/// Milestones of one user-perceived save (all `Instant`s; durations are
/// computed FROM THE CLICK, so they overlap rather than add up). Finished —
/// and logged as "Save (wall clock)" — when the flycheck the save triggered
/// ends (diagnostics fresh), or on a 120 s timeout.
struct SaveWall {
    started: std::time::Instant,
    worker_done: Option<std::time::Instant>,
    flush_done: Option<std::time::Instant>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct PersistedState {
    /// `(path_relative_to_src, content)` for every user-created file.
    user_src_files: Vec<(String, String)>,
    /// Explicitly-created empty folders inside src/.
    user_src_folders: Vec<String>,
    /// Display name of the last opened/exported project folder.
    #[serde(default)]
    project_name: Option<String>,
    /// Full filesystem path of the last opened project root (UTF-8 string).
    /// On startup the IDE reopens this folder automatically if it still exists.
    #[serde(default)]
    project_dir: Option<String>,
}

// ── App state ─────────────────────────────────────────────────────────────────

pub struct AppIde {
    /// All known MCU definitions (built-in + later: imported from a folder).
    mcu_registry: Vec<McuDefinition>,
    /// `id` of the currently selected MCU (key into `mcu_registry`).
    selected_mcu_id: String,
    /// None when the selected chip is not yet implemented
    mcu: Option<Mcu>,
    /// Generated Rust HAL code — rebuilt each frame from pin state
    generated_code: String,
    /// Hash of the MCU state (pins + clock + modules) used to detect changes
    /// and avoid unnecessary code regeneration.
    mcu_state_hash: u64,
    /// Editable project config files. Each holds the current content (a
    /// `<<< GENERATED >>>` block the IDE refreshes on chip change, plus whatever
    /// the user adds outside it). `memory_x`/`build_rs` are empty for ESP.
    cargo_toml: String,
    cargo_config: String,
    memory_x: String,
    build_rs: String,
    gitignore: String,
    /// Cached project files to avoid repeated cloning
    cached_project_files: Option<ProjectFiles>,
    /// Active tab in the MCU configurator
    active_tab: McuTab,
    /// Currently selected file in the project tree
    selected_file: ProjectFileId,
    /// Shown briefly after a successful copy
    copy_flash: u8,
    /// Show the export/save result message until this deadline. Time-based
    /// (not a frame countdown): frame cadence varies from 60+ FPS to the 4 FPS
    /// activity watchdog, so counting frames made the message's lifetime
    /// swing from ~3 s to ~45 s.
    export_status_until: Option<std::time::Instant>,
    /// `Instant` of the previous `ui()` frame — measures inter-frame gaps for
    /// the UI-stall detector (a gap while work was pending = a lost wake-up).
    last_frame_at: Option<std::time::Instant>,
    /// Whether the PREVIOUS frame had background activity (busy status bar).
    /// An inter-frame gap only matters when work was pending through it.
    was_busy_last_frame: bool,
    /// Wall-clock envelope of the current save AS THE USER PERCEIVES IT:
    /// click → project written → LSP flush done → diagnostics fresh. Logged
    /// to Activity as "Save (wall clock)" so the tab has an entry matching
    /// the felt duration, decomposed into milestones.
    save_wall: Option<SaveWall>,
    /// Last export result message
    export_msg: String,
    /// In-flight async project save: `None` until the worker finishes, then
    /// `Some(Ok(name))` / `Some(Err(msg))`. `Some(handle)` ⇒ a save is running
    /// (UI stays responsive; the header shows a "Saving…" spinner).
    #[allow(clippy::type_complexity)]
    save_in_progress: Option<Arc<Mutex<Option<Result<String, String>>>>>,
    /// Destination folder of the running save — becomes `project_dir` on success.
    save_dest: Option<std::path::PathBuf>,
    // ── Build ────────────────────────────────────────────────────────────────
    /// egui context stored for cross-thread repaint requests
    egui_ctx: egui::Context,
    /// Shared state written by the background build thread
    build_state: Arc<Mutex<BuildState>>,
    /// Index of the diagnostic currently expanded in the cargo build panel
    selected_diagnostic: Option<usize>,
    /// Shared state written by the background `cargo clippy` thread (reuses
    /// BuildState) + the expanded-suggestion index for the Clippy tab.
    clippy_state: Arc<Mutex<BuildState>>,
    clippy_sel: Option<usize>,
    /// Shared state for USB DFU detection and flashing
    dfu_state: Arc<Mutex<DfuState>>,
    /// Live output lines from the DFU flash operation (build + objcopy + dfu-util)
    dfu_log: Arc<Mutex<Vec<String>>>,
    /// Filtered list of detected USB programmers (ST-Link, J-Link, DFU, serial, …)
    // dfu_programmers: Arc<Mutex<Vec<dfu::ProgrammerInfo>>>,
    dfu_programmers: Arc<Mutex<HashMap<String, dfu::ProgrammerInfo>>>,
    /// Index of the programmer currently selected in the ComboBox
    // dfu_sel_programmer: usize,
    dfu_sel_programmer: String,
    /// Flash start address sent to dfu-util (editable; default = 0x08000000)
    dfu_flash_addr: String,
    /// Shared state for OpenOCD SWD flash operations
    openocd_state: Arc<Mutex<OpenOcdState>>,
    /// Target config file passed to OpenOCD (e.g. "target/stm32f1x.cfg")
    openocd_target_cfg: String,
    /// Shared state for ESP32 espflash operations
    espflash_state: Arc<Mutex<EspFlashState>>,
    /// Optional serial port override for espflash (e.g. "COM3", "/dev/ttyUSB0").
    /// Empty = auto-detect (espflash scans available ports automatically).
    espflash_port: String,
    /// Shared state for the Required Tools tab (check + install operations)
    tools_state: Arc<Mutex<required_tools::ToolsState>>,
    /// Code-completion engine — stores the trie, current prefix and popup state.
    /// Must live in the App (not a local) so state is preserved across frames.
    completer: Completer,
    /// True when the LSP completion popup is visible.
    completion_open: bool,
    /// Index of the currently highlighted row in the completion popup.
    completion_sel: usize,
    /// Character-offset in the editor text where completion was triggered.
    /// Used to compute the live prefix for filtering and to close the popup
    /// when the cursor moves away.
    completion_trigger_idx: usize,
    /// Completion item deferred from a mouse-click on a popup row.
    /// Applied at the start of the next frame (before the editor renders);
    /// carries the whole item so snippet expansion sees `insert_is_snippet`.
    completion_pending_insert: Option<lsp::CompletionItem>,
    /// Filtered completion list from the last rendered frame.
    /// Key handlers (Tab / Enter / Arrow) use this so they always operate
    /// on the same slice the user sees, not the full unfiltered LSP list.
    completion_filtered_items: Vec<lsp::CompletionItem>,
    /// Cargo.toml dependency-completion popup (crate names + live crates.io
    /// versions). Independent of rust-analyzer.
    cargo_complete: editor_panel::cargo_complete::CargoCompleteState,
    /// Primary caret char-index from the previous frame, used to scroll the
    /// editor so the caret stays in view when it moves off-screen (e.g.
    /// Shift+Up/Down selection past the visible area).
    last_caret_idx: Option<usize>,
    /// Pending "jump to this diagnostic": the target file and its 1-based line.
    /// Set when a row in the Cargo Check / rust-analyzer tab is clicked; applied
    /// once the editor is displaying that file (scrolls the line to row ~10).
    pending_scroll_to_line: Option<(ProjectFileId, usize)>,
    /// The file + 1-based line + band colour of the last-clicked diagnostic.
    /// Highlighted with a translucent band (colour keyed by severity, see
    /// `diag_highlight_color`) in the editor until another diagnostic is clicked.
    highlighted_error_line: Option<(ProjectFileId, usize, egui::Color32)>,
    /// The file + 1-based line of the last F12 go-to-definition that landed in a
    /// project file. Highlighted with a translucent yellow band (like the
    /// Definition tab) until the next F12.
    highlighted_def_line: Option<(ProjectFileId, usize)>,
    /// Live "usages" analysis (fn/struct/enum/const/… fade-if-unused + a
    /// "references" popup) for whichever `.rs` file is currently displayed. See
    /// `editor_panel::usages`.
    usages: editor_panel::usages::UsagesState,
    /// Every `.rs` file's content (`"src/main.rs"` / `"src/{rel}"` → text) at
    /// the moment the last Cargo Check / Clippy run was kicked off — lets the
    /// "unused local variable" fade (which can only come from an on-demand
    /// compile, see `editor_panel::usages`) tell whether a diagnostic still
    /// matches the live text or has gone stale from a later edit.
    build_text_snapshot: HashMap<String, String>,
    /// Extra caret positions for Ctrl+Shift+Up/Down multi-cursor editing (char
    /// indices into the displayed file, in the order they were added — last
    /// added is popped first by Ctrl+Shift+Down). See `editor_panel::multi_cursor`.
    extra_cursors: Vec<usize>,
    /// Which file `extra_cursors` belongs to — cleared on a file switch so
    /// stale positions never leak into an unrelated file.
    extra_cursors_file: Option<ProjectFileId>,
    /// The primary caret's char index at the end of the previous frame — lets
    /// multi-cursor replay tell a Backspace (deletes BEFORE the cursor) apart
    /// from a Delete-key press (deletes AFTER it).
    mc_prev_primary_idx: Option<usize>,
    // ── rust-analyzer LSP ────────────────────────────────────────────────────
    /// Shared LSP client state (updated from background threads)
    lsp_state: Arc<Mutex<lsp::LspState>>,
    /// Set by a Project Save (Ctrl+S / Save button / project reload). RA
    /// re-verifies (didChange + workspace disk write + didSave) ONLY when this is
    /// set — never while typing, so editing stays light. See `init_frame`.
    lsp_flush_requested: bool,
    /// True while a background LSP flush worker is running. Guards overlap: a
    /// save arriving mid-flush keeps `lsp_flush_requested` set and `init_frame`
    /// re-fires it once the worker clears this and wakes the UI.
    lsp_flush_in_flight: Arc<std::sync::atomic::AtomicBool>,
    // ── Rename symbol (Ctrl+R → textDocument/rename) ─────────────────────────
    /// While `true`, the rename input popup is shown.
    rename_active: bool,
    /// The new name being typed in the rename popup (pre-filled with the symbol).
    rename_input: String,
    /// File + 0-based (line, char) where the rename was triggered.
    rename_rel: String,
    rename_line: u32,
    rename_char: u32,
    /// Screen position to anchor the rename popup at.
    rename_popup_pos: egui::Pos2,
    /// `true` after a rename request was sent, until RA's edits are applied.
    rename_in_flight: bool,
    /// `true` when the in-flight rename came from a Clippy "Rename" button — once
    /// RA's edits land, clippy is re-run so its (now-stale) list refreshes.
    clippy_rename_pending: bool,
    /// Remaining renames to apply one-by-one (Clippy "Apply all" batches them, and
    /// a single "Rename" enqueues one). Each is fired only after the previous one's
    /// edits land; a queued entry is skipped if its position no longer matches its
    /// `old_name` (a prior edit shifted it — it'll resurface on the final re-run).
    clippy_rename_queue: std::collections::VecDeque<crate::build::RenameFix>,
    /// Request keyboard focus for the rename input on the frame it opens.
    rename_focus: bool,
    // ── Find / Replace (Ctrl+F / Ctrl+H / Ctrl+Shift+F / Ctrl+Shift+H) ───────
    /// Search bar state: mode, query/replacement text, results, match cursor.
    find: editor_panel::find_replace::FindReplace,
    /// Code-editor font size in points, zoomed with Ctrl + `+`/`-`/`0`.
    editor_font_size: f32,
    /// Full-definition highlight set by a triple-click on a `{`/`}` — `(file,
    /// start, close)` inclusive char range, kept until the selection changes.
    full_block_selection: Option<(ProjectFileId, usize, usize)>,
    // ── Serial monitor (built-in USART/UART console) ─────────────────────────
    serial: crate::serial::SerialMonitor,
    // ── Terminal (built-in streaming command console) ────────────────────────
    terminal: crate::terminal::TerminalConsole,
    // ── Git (commit/push/pull in the project directory) ──────────────────────
    git: crate::git::GitConsole,
    /// Editor gutter diff (live in-memory text vs HEAD) + revert-hunk state.
    diff_gutter: editor_panel::diff_gutter::DiffGutter,
    // ── Activity log (per-Save/Build/Flash timing breakdown) ─────────────────
    activity: Arc<Mutex<crate::activity::ActivityLog>>,
    /// MRU file-switch history + active Ctrl+Tab cycling session
    /// (see `editor_panel::file_cycle`).
    file_cycle: editor_panel::file_cycle::FileCycle,
    /// `selected_file` as of the previous frame — the change detector that
    /// feeds `file_cycle` regardless of what caused the switch (tree click,
    /// F12, diagnostics nav, …).
    last_selected_file: ProjectFileId,
    /// Hash of the content last written to each file in the RA workspace
    /// (`src/<rel>` → content hash). Lets the LSP flush skip the per-file disk
    /// `fs::read` for unchanged files — those reads contended with
    /// rust-analyzer's flycheck reading the same files and grew over a session
    /// (observed as the "write files to RA workspace" phase climbing to 100ms+).
    /// The IDE is the only writer of that workspace, so the cache is
    /// authoritative. Shared with the flush WORKER thread (`spawn_lsp_flush`);
    /// `lsp_flush_in_flight` keeps two flushes from racing it.
    flushed_hashes: Arc<Mutex<std::collections::HashMap<String, u64>>>,
    // ── Go to definition (F12 → textDocument/definition) ─────────────────────
    /// `true` after an F12 request, until the definition arrives.
    definition_in_flight: bool,
    /// One-shot: scroll the Definition tab to the highlighted line on the first
    /// render after a new F12 snippet loads (then the user scrolls freely).
    def_scroll_pending: bool,
    /// The fetched definition snippet — its presence shows the "Definition" tab.
    definition_view: Option<DefinitionView>,
    /// Which tab is active in the bottom diagnostics panel
    build_tab: BuildPanelTab,
    /// Index of the RA diagnostic row that is expanded
    lsp_selected_diagnostic: Option<usize>,
    // ── Diagnostics panel layout ─────────────────────────────────────────────
    /// Height of the bottom diagnostics panel in pixels — persisted so the
    /// user's drag position is remembered across show/hide cycles.
    diag_panel_height: f32,
    // ── User source files ─────────────────────────────────────────────────────
    /// Project tree state (files and folders in src/)
    project_tree: ProjectTreeState,
    /// While `Some(s)`, a text-input for naming a new file is shown in the tree.
    new_src_name: Option<String>,
    /// While `Some(s)`, a text-input for naming a new folder is shown in the tree.
    new_src_folder_name: Option<String>,
    /// Parent folder path when creating a new file (e.g., "src" or "src/utils").
    /// If empty, creates in src/ root. For project-level files, would use "".
    new_file_parent_folder: Option<String>,
    /// Parent folder path when creating a new folder (e.g., "src" or "src/utils").
    new_folder_parent_folder: Option<String>,
    /// While `Some((folder, name))`, an inline file-name input is open inside that folder.
    new_file_in_folder: Option<(String, String)>,
    /// While `Some((idx, new_name))`, an inline rename input is shown for that user file.
    renaming_file: Option<(usize, String)>,
    /// While `Some((old_folder, new_name))`, an inline rename input is shown for that folder.
    renaming_folder: Option<(String, String)>,
    // ── Project management ────────────────────────────────────────────────────
    /// `true` while the "New Project" confirmation dialog is open.
    confirm_new_project: bool,
    /// Chip `id` staged inside the "New Project" popup.
    /// `None` = "Empty" (no chip change on confirm).
    pending_mcu_id: Option<String>,
    /// Last "Import MCU…" result message shown in the New Project popup
    /// (`✔ …` on success, `✗ …` on failure). Cleared when the popup closes.
    mcu_import_status: Option<String>,
    /// Display name of the last opened/exported project (shown in the panel heading).
    project_name: Option<String>,
    /// Full path to the last opened project root folder.
    /// Persisted so the IDE can reopen it automatically on next startup.
    project_dir: Option<std::path::PathBuf>,
    // ── Filesystem watcher ────────────────────────────────────────────────────
    /// Receives filesystem events from the background notify thread.
    /// Polled every frame; used to detect files created/removed by external tools.
    fs_rx: Option<std::sync::mpsc::Receiver<notify::Result<notify::Event>>>,
    /// Kept alive so the watcher thread lives as long as the app.
    _fs_watcher: Option<notify::RecommendedWatcher>,
}

impl AppIde {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // ── Dark IDE theme ────────────────────────────────────────────────────
        apply_dark_theme(&cc.egui_ctx);

        // Load Phosphor icon font alongside egui's default fonts
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        cc.egui_ctx.set_fonts(fonts);

        // Mouse wheel: by default egui makes Shift+wheel scroll *horizontally*,
        // so holding Shift (e.g. while selecting) and scrolling did nothing
        // vertically. Drop Shift as the horizontal-scroll modifier so Shift+wheel
        // scrolls up/down like a plain wheel (used to reach off-screen text while
        // selecting). Horizontal scrolling stays available via the scrollbar.
        cc.egui_ctx.options_mut(|o| {
            o.input_options.horizontal_scroll_modifier = egui::Modifiers::NONE;
            // Repurpose Ctrl + `+`/`-`/`0` to zoom the CODE EDITOR text (handled in
            // the editor panel) instead of egui's global UI zoom.
            o.zoom_with_keyboard = false;
        });

        // Steady (non-blinking) text carets everywhere. The editor also paints
        // its primary caret itself (`paint_primary_caret`) because egui hides
        // the caret whenever `input.focused` is false — and that OS-window
        // focus flag goes stale on Windows when a `Focused(true)` event is
        // missed (app start, Alt+Tab), leaving typing functional but the caret
        // invisible. A steady caret keeps the two paints indistinguishable.
        cc.egui_ctx
            .style_mut(|s| s.visuals.text_cursor.blink = false);

        // ── Load persisted project state ─────────────────────────────────────
        let mut persisted: PersistedState = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        // Hygiene: earlier builds' fs watcher pushed directory-create events
        // into `user_src_files` as phantom ("folder", "") FILE entries, and
        // eframe persistence kept them across restarts — where they shadowed
        // the real folder node in the tree. Drop any "file" whose path is a
        // tracked folder.
        {
            let folders = persisted.user_src_folders.clone();
            persisted
                .user_src_files
                .retain(|(p, _)| !folders.contains(p));
        }

        // Load the MCU registry: bundled built-ins + any user `.ron` imports
        // from the per-user `mcus/` folder (Phase 5 — runtime import).
        let mcu_registry = registry::load_registry();
        let selected_mcu_id = "stm32f103c8t6".to_owned();
        let mcu = Self::build_mcu_for(&mcu_registry, &selected_mcu_id)
            .expect("built-in STM32F103 definition must load");
        let generated_code = mcu.fresh_main_rs();

        // Seed the editable project config files from the default chip — each
        // carries a `<<< GENERATED >>>` block plus a user-editable tail.
        let init_files = {
            let d = mcu_registry
                .iter()
                .find(|d| d.id == selected_mcu_id)
                .expect("selected definition exists");
            project_gen::build_project_files(&d.project, &d.toolchain, &generated_code)
        };

        // Pre-compute the saved project dir so we can use it both in the
        // Self initialiser and in the post-construction load call below.
        let saved_project_dir: Option<std::path::PathBuf> = persisted
            .project_dir
            .as_deref()
            .map(std::path::PathBuf::from);

        // ── Start filesystem watcher on the build workspace src/ dir ─────────
        // The watcher runs on a background thread and sends events through a
        // channel.  We poll the channel each frame (non-blocking).
        let workspace_src = std::env::temp_dir()
            .join("embedded_ide_0_check")
            .join("src");
        let (fs_tx, fs_rx) = std::sync::mpsc::channel();
        let ctx_clone = cc.egui_ctx.clone();
        let mut watcher = notify::recommended_watcher(move |ev| {
            let _ = fs_tx.send(ev);
            ctx_clone.request_repaint(); // wake the UI thread on fs event
        });
        if let Ok(ref mut w) = watcher {
            // Watch even if the dir doesn't exist yet — we'll re-watch after
            // write_project creates it for the first time.
            if workspace_src.exists() {
                let _ = w.watch(&workspace_src, notify::RecursiveMode::Recursive);
            }
        }

        // ── USB DFU state — created before Self so we can start monitoring ──
        let dfu_state: Arc<Mutex<DfuState>> = Arc::new(Mutex::new(DfuState::Idle));
        let dfu_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let dfu_programmers: Arc<Mutex<HashMap<String, dfu::ProgrammerInfo>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let openocd_state: Arc<Mutex<OpenOcdState>> = Arc::new(Mutex::new(OpenOcdState::Idle));
        let espflash_state: Arc<Mutex<EspFlashState>> = Arc::new(Mutex::new(EspFlashState::Idle));

        // Scan immediately on startup (non-blocking — runs in background thread)
        dfu::detect_dfu(
            Arc::clone(&dfu_state),
            Arc::clone(&dfu_log),
            Arc::clone(&dfu_programmers),
            cc.egui_ctx.clone(),
        );
        // Start persistent USB hotplug monitor: re-scans every 2-4 s automatically
        dfu::start_usb_monitor(Arc::clone(&dfu_state), cc.egui_ctx.clone());

        let mut app = Self {
            mcu_registry,
            selected_mcu_id,
            generated_code,
            mcu_state_hash: 0,
            cargo_toml: init_files.cargo_toml,
            cargo_config: init_files.cargo_config,
            memory_x: init_files.memory_x,
            build_rs: init_files.build_rs,
            gitignore: init_files.gitignore,
            cached_project_files: None,
            mcu: Some(mcu),
            active_tab: McuTab::Pins,
            selected_file: ProjectFileId::MainRs,
            copy_flash: 0,
            export_status_until: None,
            last_frame_at: None,
            was_busy_last_frame: false,
            save_wall: None,
            export_msg: String::new(),
            save_in_progress: None,
            save_dest: None,
            egui_ctx: cc.egui_ctx.clone(),
            build_state: Arc::new(Mutex::new(BuildState::Idle)),
            clippy_state: Arc::new(Mutex::new(BuildState::Idle)),
            clippy_sel: None,
            selected_diagnostic: None,
            dfu_state,
            dfu_log,
            dfu_programmers,
            dfu_sel_programmer: "".to_owned(),
            // dfu_sel_programmer: 0,
            dfu_flash_addr: "0x08000000".to_string(),
            openocd_state,
            openocd_target_cfg: "target/stm32f1x.cfg".to_string(),
            espflash_state,
            espflash_port: String::new(),
            tools_state: required_tools::make_tools_state(),
            // Completer: seeded with Rust keywords/types + learns words from code
            completer: Completer::new_with_syntax(&Syntax::rust())
                .with_auto_indent()
                .with_user_words(),
            completion_open: false,
            completion_sel: 0,
            completion_trigger_idx: 0,
            completion_pending_insert: None,
            completion_filtered_items: Vec::new(),
            cargo_complete: editor_panel::cargo_complete::CargoCompleteState::default(),
            last_caret_idx: None,
            pending_scroll_to_line: None,
            highlighted_error_line: None,
            highlighted_def_line: None,
            usages: editor_panel::usages::UsagesState::default(),
            build_text_snapshot: HashMap::new(),
            extra_cursors: Vec::new(),
            extra_cursors_file: None,
            mc_prev_primary_idx: None,
            lsp_state: Arc::new(Mutex::new(lsp::LspState::default())),
            lsp_flush_requested: false,
            lsp_flush_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            rename_active: false,
            rename_input: String::new(),
            rename_rel: String::new(),
            rename_line: 0,
            rename_char: 0,
            rename_popup_pos: egui::Pos2::ZERO,
            rename_in_flight: false,
            clippy_rename_pending: false,
            clippy_rename_queue: std::collections::VecDeque::new(),
            rename_focus: false,
            find: editor_panel::find_replace::FindReplace::default(),
            editor_font_size: editor_panel::DEFAULT_EDITOR_FONT_SIZE,
            full_block_selection: None,
            serial: crate::serial::SerialMonitor::default(),
            terminal: crate::terminal::TerminalConsole::default(),
            git: crate::git::GitConsole::default(),
            diff_gutter: editor_panel::diff_gutter::DiffGutter::default(),
            activity: Arc::new(Mutex::new(crate::activity::ActivityLog::default())),
            flushed_hashes: Arc::new(Mutex::new(std::collections::HashMap::new())),
            file_cycle: editor_panel::file_cycle::FileCycle::default(),
            last_selected_file: ProjectFileId::MainRs,
            definition_in_flight: false,
            def_scroll_pending: false,
            definition_view: None,
            build_tab: BuildPanelTab::RustAnalyzer,
            lsp_selected_diagnostic: None,
            diag_panel_height: 180.0,
            project_tree: ProjectTreeState {
                user_src_files: persisted.user_src_files,
                user_src_folders: persisted.user_src_folders,
            },
            new_src_name: None,
            new_src_folder_name: None,
            new_file_parent_folder: None,
            new_folder_parent_folder: None,
            new_file_in_folder: None,
            renaming_file: None,
            renaming_folder: None,
            confirm_new_project: false,
            pending_mcu_id: None,
            mcu_import_status: None,
            project_name: persisted.project_name,
            project_dir: saved_project_dir.clone(),
            fs_rx: Some(fs_rx),
            _fs_watcher: watcher.ok(),
        };

        // ── Restore previously opened project on startup ──────────────────────
        // If the last project directory still exists on disk, reload it:
        // user source files, pin configuration, and generated code are all
        // recovered exactly as they were when the app was last closed.
        if let Some(dir) = &saved_project_dir {
            if dir.exists() {
                app.load_project_from_dir(dir);
            }
        }

        app
    }

    /// Build the runtime `Mcu` for `id` from the registry, if present.
    fn build_mcu_for(registry: &[McuDefinition], id: &str) -> Option<Mcu> {
        registry.iter().find(|d| d.id == id).map(|d| d.build_mcu())
    }

    /// The currently-selected MCU definition (key = `selected_mcu_id`).
    fn selected_def(&self) -> Option<&McuDefinition> {
        self.mcu_registry
            .iter()
            .find(|d| d.id == self.selected_mcu_id)
    }

    /// True when a real chip is selected (replaces `project_config().is_some()`).
    fn has_project(&self) -> bool {
        self.selected_def().is_some()
    }

    /// Display name of the selected chip (empty if none).
    fn selected_label(&self) -> String {
        self.selected_def()
            .map(|d| d.display_name.clone())
            .unwrap_or_default()
    }

    /// CPU family string of the selected chip (empty if none).
    fn selected_family(&self) -> String {
        self.selected_def()
            .map(|d| d.cpu.clone())
            .unwrap_or_default()
    }

    /// Toolchain of the selected chip (None if no chip selected).
    fn selected_toolchain(&self) -> Option<ToolchainKind> {
        self.selected_def().map(|d| d.toolchain.clone())
    }

    /// Owned `(project params, toolchain)` for project generation — cloned so no
    /// borrow of `self` is held across the subsequent `self` mutations.
    fn selected_build_cfg(&self) -> Option<(ProjectDef, ToolchainKind)> {
        self.selected_def()
            .map(|d| (d.project.clone(), d.toolchain.clone()))
    }
    /// The live project files: generated `main.rs` plus the five editable config
    /// The live project files: generated `main.rs` plus the five editable config
    /// files (with their current user edits). This — not a fresh regeneration —
    /// is what the editor shows and what `write_project` persists.
    fn current_project_files(&self) -> ProjectFiles {
        // Return cached version if available and up-to-date
        if let Some(ref cached) = self.cached_project_files {
            // Check if cache is still valid by comparing with current state
            if cached.main_rs == self.generated_code
                && cached.cargo_toml == self.cargo_toml
                && cached.cargo_config == self.cargo_config
                && cached.memory_x == self.memory_x
                && cached.build_rs == self.build_rs
                && cached.gitignore == self.gitignore
            {
                return cached.clone();
            }
        }

        // Create new ProjectFiles and cache it
        let files = ProjectFiles {
            main_rs: self.generated_code.clone(),
            cargo_toml: self.cargo_toml.clone(),
            cargo_config: self.cargo_config.clone(),
            memory_x: self.memory_x.clone(),
            build_rs: self.build_rs.clone(),
            gitignore: self.gitignore.clone(),
        };
        files
    }

    /// Update the cached project files (call this after modifying any config file)
    fn invalidate_project_files_cache(&mut self) {
        self.cached_project_files = None;
    }

    /// Apply clippy's machine-applicable [`SpanEdit`](crate::build::SpanEdit)s to
    /// the in-memory source. Edits are grouped per file and applied back-to-front
    /// (so earlier byte offsets stay valid), skipping any that overlap an
    /// already-applied edit or land off a UTF-8 char boundary. `src/main.rs`
    /// targets the generated buffer; `src/<rel>` targets the matching user source
    /// file. Returns how many edits were applied.
    ///
    /// Edits inside main.rs's GENERATED block are **never** applied: that code is
    /// owned by the MCU Configurator and regenerated on the next config change, so
    /// a hand-applied fix there would be silently reverted (and editing it while
    /// the editor shows it risks a cursor/offset mismatch). The Clippy tab also
    /// disables the "Fix" button for those rows — this is the matching guard.
    fn apply_source_edits(&mut self, edits: &[crate::build::SpanEdit]) -> usize {
        use std::collections::HashMap;
        let gen_ranges = generated_byte_ranges(&self.generated_code);
        let mut by_file: HashMap<&str, Vec<&crate::build::SpanEdit>> = HashMap::new();
        for e in edits {
            by_file.entry(e.file.as_str()).or_default().push(e);
        }
        let mut applied = 0usize;
        for (file, group) in by_file {
            let rel = file.strip_prefix("src/").unwrap_or(file);
            let is_main = rel == "main.rs";
            let target: Option<&mut String> = if is_main {
                Some(&mut self.generated_code)
            } else {
                self.project_tree
                    .user_src_files
                    .iter_mut()
                    .find(|(p, _)| p == rel)
                    .map(|(_, c)| c)
            };
            let Some(buf) = target else { continue };
            // main.rs's GENERATED block is off-limits; user files have no lock.
            let locked: &[(usize, usize)] = if is_main { &gen_ranges } else { &[] };
            applied += apply_edits_to_buffer(buf, &group, locked);
        }
        if applied > 0 {
            self.invalidate_project_files_cache();
        }
        applied
    }

    /// The `mcu.config` text for the live MCU (virtual modules + clock), written
    /// alongside the project by `write_project`. Empty when no chip is selected.
    fn mcu_config_text(&self) -> String {
        self.mcu
            .as_ref()
            .map(|m| m.mcu_config_text())
            .unwrap_or_default()
    }

    /// Regenerate every editable config file fresh from the selected chip,
    /// discarding prior edits. Used when starting a New Project / switching chip
    /// (a clean slate, like clearing `user_src_files`). Toolchains that don't use
    /// a file (e.g. `memory.x`/`build.rs` on ESP) get an empty string.
    fn reset_config_files(&mut self) {
        if let Some((cfg, tc)) = self.selected_build_cfg() {
            let f = project_gen::build_project_files(&cfg, &tc, &self.generated_code);
            self.cargo_toml = f.cargo_toml;
            self.cargo_config = f.cargo_config;
            self.memory_x = f.memory_x;
            self.build_rs = f.build_rs;
            self.gitignore = f.gitignore;
            self.invalidate_project_files_cache();
        }
    }

    // ── Frame initialization (frame state, LSP, MCU synchronization) ───────────
    /// Calculate a hash of the MCU state (pins + clock + modules) for change detection
    fn calculate_mcu_state_hash(&self, mcu: &Mcu) -> u64 {
        let mut hasher = DefaultHasher::new();

        // Hash all pins
        for pin in mcu.iter_all_pins() {
            pin.selected_function.hash(&mut hasher);
            pin.custom_label.hash(&mut hasher);
        }

        // Hash clock config
        format!("{:?}", mcu.clock).hash(&mut hasher);

        // Hash modules
        for module in &mcu.modules {
            module.id.hash(&mut hasher);
            module.kind.hash(&mut hasher);
            module.name.hash(&mut hasher);
            format!("{:?}{:?}", module.pos.0, module.pos.1).hash(&mut hasher);
            // Parameters (baud, mode, …) and pin wiring feed `config_files()`
            // — they must bump the hash, or the hash-gated regeneration in
            // `init_frame` would miss a module edit and keep emitting the old
            // configs/*.rs content.
            format!("{:?}{:?}", module.config, module.connections).hash(&mut hasher);
        }

        hasher.finish()
    }

    fn init_frame(&mut self, _ui: &mut egui::Ui) {
        // ── Poll filesystem watcher events ────────────────────────────────────
        self.poll_fs_events();

        // ── MRU file history (Ctrl+Tab switching) ─────────────────────────────
        // One central change detector: whatever switched `selected_file` (tree
        // click, F12, diagnostics nav, …), promote the new file to the front of
        // the MRU — except while a Ctrl+Tab cycling session drives the switches
        // itself (the promotion happens on commit instead).
        use editor_panel::file_cycle::HistEntry;
        if self.file_cycle.is_empty() {
            // Seed with the file shown at startup.
            if let Some(e) =
                HistEntry::from_id(self.selected_file, &self.project_tree.user_src_files)
            {
                self.file_cycle.note_open(e);
            }
        }
        if self.selected_file != self.last_selected_file {
            if !self.file_cycle.is_cycling() {
                if let Some(e) =
                    HistEntry::from_id(self.selected_file, &self.project_tree.user_src_files)
                {
                    self.file_cycle.note_open(e);
                }
            }
            self.last_selected_file = self.selected_file;
        }

        // ── Update generated code when the MCU state changes ─────────────────
        // ONE hash gates BOTH main.rs regeneration AND the peripheral-config /
        // pin-file sync below. The sync used to run UNCONDITIONALLY every
        // frame — full codegen of every configs/*.rs plus Cargo.toml dep
        // checks, just to no-op compare — which, under spinner-driven
        // continuous repaint, was a big share of the per-frame CPU cost.
        let mut mcu_changed = false;
        if let Some(mcu) = &self.mcu {
            let current_hash = self.calculate_mcu_state_hash(mcu);
            if current_hash != self.mcu_state_hash {
                // Store the hash even when main.rs comes out identical —
                // otherwise this branch (and `update_main_rs`) re-runs every
                // frame until the NEXT state change.
                self.mcu_state_hash = current_hash;
                mcu_changed = true;
                let updated = mcu.update_main_rs(&self.generated_code);
                if updated != self.generated_code {
                    self.generated_code = updated;
                    self.cached_project_files = None; // Invalidate cache
                }
            }
        }

        // Regenerate the per-peripheral init modules (src/pins/configs/) and the
        // pins/ module files from the current pin + Virtual-Module config. Both
        // are no-ops when nothing changed (splice preserves user edits). Configs
        // first so `sync_pin_files` can declare `pub mod configs;` in pins/mod.rs.
        let regen = if mcu_changed {
            self.mcu
                .as_ref()
                .map(|m| (m.all_pin_functions(), m.config_files()))
        } else {
            None
        };
        if let Some((all_pins, config_files)) = regen {
            use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
            // CAN init pulls in the external `bxcan`/`nb` crates — add or drop
            // them in Cargo.toml based on whether codegen emitted `can1.rs`.
            let needs_can = config_files.iter().any(|(name, _)| name == "can1.rs");
            // USB CDC init needs `usb-device`/`usbd-serial` + the `stm32-usbd`
            // HAL feature, keyed on whether the USB D-/D+ pins are configured.
            let needs_usb = all_pins
                .iter()
                .any(|(_, _, f)| matches!(f, PinFunction::UsbDm | PinFunction::UsbDp));
            let new_toml = project_gen::ensure_can_deps(&self.cargo_toml, needs_can);
            let new_toml = project_gen::ensure_usb_deps(&new_toml, needs_usb);
            if new_toml != self.cargo_toml {
                self.cargo_toml = new_toml;
                self.invalidate_project_files_cache();
            }
            self.project_tree.sync_config_files(&config_files);
            self.project_tree.sync_pin_files(&all_pins);
        }

        // Tick flash counters down
        if self.copy_flash > 0 {
            self.copy_flash -= 1;
        }
        if self
            .export_status_until
            .is_some_and(|t| t <= std::time::Instant::now())
        {
            self.export_status_until = None;
        }

        // ── LSP lifecycle ─────────────────────────────────────────────────────
        let lsp_status = self.lsp_state.lock().unwrap().status.clone();
        match lsp_status {
            LspStatus::Stopped => {
                if self.has_project() {
                    let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
                    if self.selected_build_cfg().is_some() {
                        if project_gen::write_project(
                            &build_dir,
                            &self.current_project_files(),
                            &self.project_tree.user_src_files,
                            &self.mcu_config_text(),
                        )
                        .is_ok()
                        {
                            lsp::start(
                                &build_dir,
                                Arc::clone(&self.lsp_state),
                                self.egui_ctx.clone(),
                            );
                        }
                    }
                }
            }
            LspStatus::Indexing => {
                let mut lsp = self.lsp_state.lock().unwrap();
                if !lsp.is_file_open("src/main.rs") {
                    lsp.did_open("src/main.rs", &self.generated_code.clone());
                }
            }
            LspStatus::Ready => {
                // RA re-verifies ONLY on an explicit Project Save (Ctrl+S / Save
                // button / project reload) — never while typing, so editing stays
                // light. The Save re-syncs every file to RA and re-runs its checks
                // (didChange + workspace disk write + didSave). Completions still
                // sync their own text on demand (see completion.rs), so they work
                // on the current text between saves.
                // Runs on a WORKER thread so the save frame stays short (the
                // synchronous flush froze the UI — and the status spinner —
                // for the whole disk-write + didChange span). While a flush is
                // in flight the request stays set; the worker wakes the UI
                // when done and the queued flush fires on that frame.
                if self.lsp_flush_requested
                    && !self
                        .lsp_flush_in_flight
                        .load(std::sync::atomic::Ordering::Acquire)
                {
                    self.lsp_flush_requested = false;
                    self.spawn_lsp_flush();
                }
            }
            _ => {}
        }

        // ── Apply a completed rename (textDocument/rename) across files ───────
        if self.rename_in_flight {
            let result = self.lsp_state.lock().unwrap().take_rename_result();
            if let Some(edits) = result {
                self.rename_in_flight = false;
                if !edits.is_empty() {
                    self.apply_rename_edits(edits);
                }
                // Continue an "Apply all" / queued batch: fire the next rename. When
                // the queue is drained, re-run clippy once so the list reflects all
                // the renamed code (clippy_rename_pending was set when the batch
                // started).
                if !self.start_next_queued_rename() && self.clippy_rename_pending {
                    self.clippy_rename_pending = false;
                    self.start_clippy_run();
                }
                self.egui_ctx.request_repaint();
            }
        }

        // ── Handle a completed F12 go-to-definition ──────────────────────────
        // A definition in the CURRENT project opens editable in the main editor
        // (navigate + scroll the line into view). A definition in another file
        // (crate / std) is shown read-only in the Definition tab snippet.
        if self.definition_in_flight {
            let result = self.lsp_state.lock().unwrap().take_definition_result();
            if let Some(loc) = result {
                self.definition_in_flight = false;
                if let Some(loc) = loc {
                    if let Some(id) =
                        project_file_for_def(&loc.path, &self.project_tree.user_src_files)
                    {
                        // Editable: open the file, scroll to the definition, and
                        // mark the def line with a yellow band (like the Def tab).
                        self.selected_file = id;
                        self.pending_scroll_to_line = Some((id, loc.line as usize + 1));
                        self.highlighted_def_line = Some((id, loc.line as usize + 1));
                        // Not an error — clear any error tint and the snippet tab.
                        self.definition_view = None;
                        if self.build_tab == BuildPanelTab::Definition {
                            self.build_tab = BuildPanelTab::RustAnalyzer;
                        }
                    } else if let Some(view) = build_definition_view(&loc) {
                        // External file → read-only snippet in the Definition tab,
                        // scrolled to the target line on open.
                        self.definition_view = Some(view);
                        self.build_tab = BuildPanelTab::Definition;
                        self.def_scroll_pending = true;
                    }
                }
                self.egui_ctx.request_repaint();
            }
        }

        // ── Commit finished RA flycheck spans to the Activity log ─────────────
        // The post-save "Checking…" wall time lives inside rust-analyzer, so no
        // in-app recorder can wrap it; RA's `$/progress` begin/end timestamps
        // are collected in `finished_checks` and logged here as their own
        // entry — THIS is the seconds-long tail a Save actually costs. The
        // queue span separates a clogged RA (long wait before cargo starts)
        // from a genuinely slow `cargo check` (long run).
        let spans = {
            let mut lsp = self.lsp_state.lock().unwrap();
            std::mem::take(&mut lsp.finished_checks)
        };
        for (queued, ran) in spans {
            let mut rec = crate::activity::Recorder::new("Check (RA flycheck)");
            rec.add(
                "waiting in rust-analyzer's queue (didSave → check start)",
                queued,
            );
            rec.add("cargo check run", ran);
            self.activity
                .lock()
                .unwrap()
                .push(rec.finish_with_total(queued + ran));
        }

        self.tick_save_wall();
    }

    /// Advance the "Save (wall clock)" envelope: note when the LSP flush
    /// finishes, and once the flycheck the save triggered is done (or nothing
    /// triggered one, or 120 s passed) log the whole user-perceived span to
    /// Activity — the entry that should match what the save FELT like,
    /// decomposed into milestones.
    fn tick_save_wall(&mut self) {
        let Some(mut w) = self.save_wall.take() else {
            return;
        };
        let now = std::time::Instant::now();

        // The LSP flush is done once its request was consumed and no worker is
        // in flight. (`did_save` — which sets `flycheck_pending` — happens
        // BEFORE the worker clears the in-flight flag, so the completion check
        // below can't race past a just-triggered check.)
        if w.flush_done.is_none()
            && !self.lsp_flush_requested
            && !self
                .lsp_flush_in_flight
                .load(std::sync::atomic::Ordering::Acquire)
        {
            w.flush_done = Some(now);
        }

        let (checking, pending) = {
            let lsp = self.lsp_state.lock().unwrap();
            (lsp.checking, lsp.flycheck_pending())
        };
        let done = w.flush_done.is_some() && !checking && !pending;
        let timed_out = now - w.started > std::time::Duration::from_secs(120);
        if !(done || timed_out) {
            self.save_wall = Some(w); // still running — keep the clock
            return;
        }

        let mut rec = crate::activity::Recorder::new("Save (wall clock)");
        if let Some(t) = w.worker_done {
            rec.add("click → project written to disk", t - w.started);
        }
        if let Some(t) = w.flush_done {
            rec.add("click → LSP flush finished", t - w.started);
        }
        if timed_out {
            rec.mark("timed out waiting for the flycheck (rust-analyzer not Ready?)");
        } else {
            rec.add(
                "click → inline diagnostics fresh (flycheck done)",
                now - w.started,
            );
        }
        rec.mark("milestones measured FROM THE CLICK — they overlap, not add up");
        self.activity
            .lock()
            .unwrap()
            .push(rec.finish_with_total(now - w.started));
    }

    /// Apply rename edits returned by rust-analyzer to the in-memory files
    /// (main.rs + user source files). Per file, edits are applied back-to-front
    /// so earlier positions stay valid; the debounce then re-syncs RA.
    fn apply_rename_edits(&mut self, edits: Vec<lsp::RenameEdit>) {
        use std::collections::HashMap;
        let mut by_file: HashMap<String, Vec<lsp::RenameEdit>> = HashMap::new();
        for e in edits {
            by_file.entry(e.rel_path.clone()).or_default().push(e);
        }
        for (rel, es) in by_file {
            if rel == "src/main.rs" {
                self.generated_code = apply_text_edits(&self.generated_code, es);
            } else if let Some(sub) = rel.strip_prefix("src/") {
                if let Some(entry) = self
                    .project_tree
                    .user_src_files
                    .iter_mut()
                    .find(|(p, _)| p == sub)
                {
                    entry.1 = apply_text_edits(&entry.1, es);
                }
            }
        }
    }

    /// Single point where rust-analyzer is asked to re-verify: writes main.rs +
    /// every user source file to the LSP workspace (so cargo-check/flycheck sees
    /// them), pushes `didChange` (RA's in-memory analysis) for the files that
    /// actually changed and `didSave` (RA's flycheck) when anything did.
    /// Called **only on a Project Save** — never while typing.
    ///
    /// Delete the temp check-workspace's `Cargo.lock` so the next `cargo check`
    /// re-resolves dependencies. Called ONLY when the chip/toolchain changes or a
    /// project is opened (the deps differ then); saves keep the lock so checks
    /// stay fast. The user's own project `Cargo.lock` is left untouched.
    fn reset_workspace_lock(&self) {
        let workspace = std::env::temp_dir().join("embedded_ide_0_check");
        let _ = std::fs::remove_file(workspace.join("Cargo.lock"));
    }

    /// Content hash used by [`AppIde::flushed_hashes`] (SipHash via `DefaultHasher`).
    fn content_hash(content: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut h);
        h.finish()
    }

    /// Write one file into the RA workspace, skipping disk I/O when possible.
    /// `cache` maps `rel` → the hash of the content we last wrote there. Since
    /// the IDE is the only writer of this workspace, the cache is authoritative:
    /// a hash match means the disk already has this content (no read, no write);
    /// a known-stale entry means we can write directly WITHOUT a compare read;
    /// only a brand-new path falls back to a disk read (to avoid a needless
    /// mtime bump if it already matches, e.g. a reopened project).
    /// Returns `true` if it actually wrote to disk (for the Activity diagnostic).
    fn write_workspace_file(
        cache: &mut std::collections::HashMap<String, u64>,
        workspace: &std::path::Path,
        rel: &str,
        content: &str,
    ) -> bool {
        let hash = Self::content_hash(content);
        match cache.get(rel) {
            Some(&h) if h == hash => return false, // unchanged since last flush
            Some(_) => {}                          // known-stale → write directly
            None => {
                let dest = workspace.join("src").join(rel);
                if std::fs::read(&dest).is_ok_and(|d| d == content.as_bytes()) {
                    cache.insert(rel.to_string(), hash);
                    return false;
                }
            }
        }
        let dest = workspace.join("src").join(rel);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&dest, content.as_bytes()).is_ok() {
            cache.insert(rel.to_string(), hash);
        }
        true
    }

    fn spawn_lsp_flush(&mut self) {
        self.lsp_flush_in_flight
            .store(true, std::sync::atomic::Ordering::Release);

        // Same-frame snapshot of every file, so what reaches disk + RA is
        // exactly what the user saved. "main.rs" first, in the bare-rel shape
        // `write_workspace_file` expects (it prepends `src/`).
        let files: Vec<(String, String)> =
            std::iter::once(("main.rs".to_owned(), self.generated_code.clone()))
                .chain(
                    self.project_tree
                        .user_src_files
                        .iter()
                        .map(|(rel, content)| (rel.clone(), content.clone())),
                )
                .collect();

        let hashes = Arc::clone(&self.flushed_hashes);
        let lsp_state = Arc::clone(&self.lsp_state);
        let activity = Arc::clone(&self.activity);
        let in_flight = Arc::clone(&self.lsp_flush_in_flight);
        let ctx = self.egui_ctx.clone();

        std::thread::spawn(move || {
            let mut rec = crate::activity::Recorder::new("Save (LSP flush)");
            let workspace = std::env::temp_dir().join("embedded_ide_0_check");

            // ── Disk writes (hash-cached) ─────────────────────────────────
            // Diagnostic: count how many files actually hit disk vs were
            // cache-skipped, and which single file was slowest — so the
            // Activity tab shows whether the cost is writes, reads, or one
            // specific file (contention).
            let t_write = std::time::Instant::now();
            let mut wrote = 0usize;
            let mut slowest = (String::new(), std::time::Duration::ZERO);
            {
                let mut cache = hashes.lock().unwrap();
                for (rel, content) in &files {
                    let t = std::time::Instant::now();
                    if Self::write_workspace_file(&mut cache, &workspace, rel, content) {
                        wrote += 1;
                    }
                    let d = t.elapsed();
                    if d > slowest.1 {
                        slowest = (rel.clone(), d);
                    }
                }
            }
            let total = files.len();
            rec.add("write files to RA workspace", t_write.elapsed());
            rec.mark(format!(
                "wrote {wrote}/{total} to disk · slowest: {} {}",
                if slowest.0.is_empty() {
                    "—"
                } else {
                    &slowest.0
                },
                crate::activity::fmt_dur(slowest.1),
            ));

            // ── Sync text to RA ───────────────────────────────────────────
            // Only files whose text differs from what RA already holds are
            // re-sent (`force = false`). The old forced re-send of EVERY
            // document bumped every doc version on every save, making RA
            // re-analyse the whole project each time. One SHORT lock per file
            // (not one big lock around the loop): the UI thread takes this
            // mutex every frame — holding it across the whole loop would just
            // move the freeze from "our frame" into its lock wait.
            let t_sync = std::time::Instant::now();
            let mut synced = 0usize;
            for (rel, content) in &files {
                let workspace_rel = format!("src/{rel}");
                if lsp_state
                    .lock()
                    .unwrap()
                    .did_change(&workspace_rel, content, false)
                {
                    synced += 1;
                }
            }
            rec.add("did_change (sync text to RA)", t_sync.elapsed());
            rec.mark(format!(
                "re-synced {synced}/{total} files (unchanged skipped)"
            ));

            // Trigger RA's `checkOnSave` flycheck (cargo check) so real compiler
            // errors — E0425 "cannot find value", type mismatches, unused vars, … —
            // refresh inline against the just-flushed text. RA's native pass alone
            // does NOT publish these for nested user files, so this is what makes
            // inline errors work at all. It runs asynchronously in RA; its REAL
            // duration (queue + run) lands in the Activity log as its own
            // "Check (RA flycheck)" entry when RA reports it done (that's the
            // seconds-long tail a Save actually costs — see `finished_checks`).
            // Skipped entirely when nothing changed: an idle Ctrl+S must not
            // re-run a whole cargo check on identical code.
            if synced > 0 || wrote > 0 {
                rec.phase("did_save (trigger RA flycheck)", || {
                    lsp_state.lock().unwrap().did_save("src/main.rs");
                });
            } else {
                rec.mark("nothing changed since last flush — flycheck not re-triggered");
            }

            activity.lock().unwrap().push(rec.finish());
            in_flight.store(false, std::sync::atomic::Ordering::Release);
            // Wake `init_frame`: a save made during this flush left
            // `lsp_flush_requested` set and must fire now.
            ctx.request_repaint();
        });
    }
}

impl eframe::App for AppIde {
    // ── Persistence: called by eframe on app exit (and periodically) ──────────
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(
            storage,
            STORAGE_KEY,
            &PersistedState {
                user_src_files: self.project_tree.user_src_files.clone(),
                user_src_folders: self.project_tree.user_src_folders.clone(),
                project_name: self.project_name.clone(),
                project_dir: self
                    .project_dir
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .map(String::from),
            },
        );
    }

    // ── App exit: terminate rust-analyzer ─────────────────────────────────────
    // Nothing else kills the RA child when the app closes (dropping a
    // `std::process::Child` only detaches it), so every app restart used to
    // leave an orphaned rust-analyzer + proc-macro server behind — each still
    // watching and re-analyzing the workspace on every file write, compounding
    // the "everything gets slower" degradation across restarts.
    fn on_exit(&mut self) {
        self.lsp_state.lock().unwrap().kill_child();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // ── UI-stall detector ─────────────────────────────────────────────────
        // A gap between frames while background work was pending means the
        // event loop slept through finished work (the "one-minute save"
        // symptom). The activity watchdog caps this at ~250 ms — an entry
        // showing up here on a current build points at a wake-up path we
        // haven't covered yet.
        let now_frame = std::time::Instant::now();
        if let (Some(prev), true) = (self.last_frame_at, self.was_busy_last_frame) {
            let gap = now_frame - prev;
            if gap >= std::time::Duration::from_secs(1) {
                let mut rec = crate::activity::Recorder::new("UI stall (frames stopped)");
                rec.add("gap between frames while work was pending", gap);
                rec.mark("event loop slept through pending work — lost wake-up?");
                self.activity
                    .lock()
                    .unwrap()
                    .push(rec.finish_with_total(gap));
            }
        }
        self.last_frame_at = Some(now_frame);

        // Initialize frame state (polling, LSP, MCU updates)
        self.init_frame(ui);

        // Detect Ctrl+S for Save/Export project
        let ctrl_s_pressed = ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S));

        // ── Bottom status bar ─────────────────────────────────────────────────
        // A persistent full-width strip for long-running activity (Saving /
        // Building / Flashing / Checking) and the last save result — with room
        // for future statuses. Declared before the side panels so it claims the
        // bottom edge across the whole window.
        let status = self.activity_status();
        // Watchdog: while ANYTHING is running, keep frames coming from the UI
        // thread itself. Cross-thread `request_repaint()` wake-ups (save
        // worker, LSP reader) travel as winit user events stamped with the
        // render-pass number they were issued at, and eframe DROPS them as
        // "outdated" when two or more passes ran in between (run.rs: "Got
        // outdated UserEvent::RequestRepaint"). A dropped LAST wake-up left
        // the event loop asleep with finished work unprocessed until a window
        // event — observed as a "one-minute save" that completed instantly
        // when the window was moved/minimized (Activity meanwhile showed the
        // true, small durations). A UI-thread `request_repaint_after` goes
        // through the per-frame repaint-delay path, which can't be dropped.
        if status.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        // Feed the UI-stall detector at the top of the next frame.
        self.was_busy_last_frame = status.is_some();
        egui::Panel::bottom("status_bar")
            .exact_size(24.0)
            .show_inside(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(8.0);
                    if let Some((spinner, text, color)) = &status {
                        if *spinner {
                            // Throttled (~10 FPS): egui's Spinner forces a
                            // repaint EVERY frame for its whole lifetime —
                            // i.e. the entire Saving/Checking/Flashing span.
                            helpers::spinner::throttled_spinner(ui, 13.0);
                            ui.add_space(5.0);
                        }
                        ui.label(egui::RichText::new(text).size(11.0).color(*color));
                    }
                });
            });

        // Build project files snapshot (used by both tree and editor panels)
        // Snapshot from the live editable state (main.rs + the five editable
        // config files), so the tree/editor show current edits — not a fresh
        // regeneration that would discard them.
        let project_files: Option<ProjectFiles> = self
            .selected_build_cfg()
            .map(|_| self.current_project_files());

        // ── Panel 1: Project Tree ─────
        // `save_project_needed` is set when the tree mutates files/folders, so
        // the whole project gets rewritten to the workspace dir afterwards.
        let mut save_project_needed = false;
        let signals =
            self.show_project_panel(ui, &project_files, ctrl_s_pressed, &mut save_project_needed);
        let open_project_clicked = signals.open_clicked;
        let new_project_clicked = signals.new_clicked;
        let save_project_clicked = signals.save_clicked;

        // ── Handle toolbar button clicks ──────────────────────────────────────

        // "New Project" → ask for confirmation; default chip selection = Empty
        if new_project_clicked {
            self.confirm_new_project = true;
            self.pending_mcu_id = None;
        }

        // "Open Project" → show native folder picker, then load files
        if open_project_clicked {
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Open Embedded IDE Project — pick the project root folder")
                .pick_folder()
            {
                self.load_project_from_dir(&folder);
                save_project_needed = true;
            }
        }

        // "Save Project" → write to the project's folder.
        //   • Existing project (opened/already saved → `project_dir` is set):
        //     save straight to that path, no dialog.
        //   • New project (`project_dir` is None): ask once where to save, then
        //     remember the chosen folder so later saves go there directly.
        // Ignore a fresh Save while one is still running (button left enabled,
        // but the click is a no-op until the worker finishes).
        if save_project_clicked
            && self.save_in_progress.is_none()
            && self.selected_build_cfg().is_some()
        {
            let dest: Option<std::path::PathBuf> = match &self.project_dir {
                Some(dir) => Some(dir.clone()),
                None => rfd::FileDialog::new()
                    .set_title("Choose folder to save the new project")
                    .pick_folder(),
            };
            if let Some(dest) = dest {
                // Run the disk write on a worker thread so the UI stays responsive
                // (the header shows a "Saving…" spinner until it completes).
                let files = self.current_project_files();
                let user_files = self.project_tree.user_src_files.clone();
                let mcu_cfg = self.mcu_config_text();
                let shared: Arc<Mutex<Option<Result<String, String>>>> = Arc::new(Mutex::new(None));
                let out = Arc::clone(&shared);
                let ctx = self.egui_ctx.clone();
                let dest_thread = dest.clone();
                let activity = Arc::clone(&self.activity);
                std::thread::spawn(move || {
                    let mut rec = crate::activity::Recorder::new("Save (project)");
                    let res = rec
                        .phase("write_project", || {
                            project_gen::write_project(&dest_thread, &files, &user_files, &mcu_cfg)
                        })
                        .map(|()| {
                            dest_thread
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("saved")
                                .to_string()
                        })
                        .map_err(|e| e.to_string());
                    activity.lock().unwrap().push(rec.finish());
                    *out.lock().unwrap() = Some(res);
                    ctx.request_repaint();
                });
                self.save_in_progress = Some(shared);
                self.save_dest = Some(dest);
                // Start the user-perceived save clock (see `SaveWall`).
                self.save_wall = Some(SaveWall {
                    started: std::time::Instant::now(),
                    worker_done: None,
                    flush_done: None,
                });
            }
        }

        // Apply a finished async save (set the result message / project home).
        let save_finished = self
            .save_in_progress
            .as_ref()
            .and_then(|s| s.lock().unwrap().take());
        if let Some(res) = save_finished {
            self.save_in_progress = None;
            if let Some(w) = &mut self.save_wall {
                w.worker_done.get_or_insert(std::time::Instant::now());
            }
            match res {
                Ok(name) => {
                    self.export_msg = format!("{}  {name}", egui_phosphor::regular::CHECK_CIRCLE);
                    self.export_status_until =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                    self.project_name = Some(name);
                    // A new project now has a home — later saves go here.
                    self.project_dir = self.save_dest.take();
                }
                Err(e) => {
                    self.export_msg = format!("{}  {e}", egui_phosphor::regular::X_CIRCLE);
                    self.export_status_until =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                    self.save_dest = None;
                }
            }
        }

        // ── Modal dialog: New Project ─────────────────────────────
        // (New File / New Folder are now inline inputs rendered in the project
        // tree at the target folder — see `project_tree::gui::inline_new_item`.)
        self.show_new_project_dialog(ui, &mut save_project_needed);

        // Write the entire project to the workspace directory when the file
        // tree changed (file added, deleted, or project opened/cleared).
        // This ensures Cargo.toml and all other required files are in sync.
        if save_project_needed {
            if self.selected_build_cfg().is_some() {
                let workspace = std::env::temp_dir().join("embedded_ide_0_check");
                let _ = project_gen::write_project(
                    &workspace,
                    &self.current_project_files(),
                    &self.project_tree.user_src_files,
                    &self.mcu_config_text(),
                );
            }
        }

        // A Project Save (or a tree change that rewrote the workspace) flushes
        // pending edits to rust-analyzer next frame — the ONLY moment RA
        // re-verifies (no typing-time evaluation, so editing stays light).
        if save_project_clicked || save_project_needed {
            self.lsp_flush_requested = true;
        }

        // ── Panel 2: Code Editor ─────
        self.show_editor_panel(ui, project_files);

        // ── Panel 3: MCU Configurator ─────
        self.show_mcu_panel(ui);
    }
}

#[cfg(test)]
mod flush_cache_tests {
    use super::AppIde;
    use std::collections::HashMap;

    #[test]
    fn write_workspace_file_caches_and_writes() {
        let dir = std::env::temp_dir().join(format!("eide_flush_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cache: HashMap<String, u64> = HashMap::new();

        // 1. First write for a new path → file created, hash cached.
        AppIde::write_workspace_file(&mut cache, &dir, "foo/bar.rs", "hello");
        let dest = dir.join("src").join("foo").join("bar.rs");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello");
        assert!(cache.contains_key("foo/bar.rs"));

        // 2. Same content again → cache hit; even if we corrupt the file on disk,
        //    the cached hash means we skip writing (proving no disk touch).
        std::fs::write(&dest, "CORRUPTED").unwrap();
        AppIde::write_workspace_file(&mut cache, &dir, "foo/bar.rs", "hello");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "CORRUPTED"); // untouched

        // 3. Changed content (cache has a stale entry) → written directly.
        AppIde::write_workspace_file(&mut cache, &dir, "foo/bar.rs", "world");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "world");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod rename_apply_tests {
    use super::{apply_text_edits, lsp_pos_to_byte};
    use crate::lsp::RenameEdit;

    fn edit(sl: u32, sc: u32, el: u32, ec: u32, t: &str) -> RenameEdit {
        RenameEdit {
            rel_path: "src/main.rs".into(),
            start_line: sl,
            start_char: sc,
            end_line: el,
            end_char: ec,
            new_text: t.into(),
        }
    }

    #[test]
    fn applies_multiple_edits_same_line() {
        let text = "let foo = foo + 1;";
        let edits = vec![edit(0, 4, 0, 7, "bar"), edit(0, 10, 0, 13, "bar")];
        assert_eq!(apply_text_edits(text, edits), "let bar = bar + 1;");
    }

    #[test]
    fn applies_edits_across_lines_with_length_change() {
        let text = "fn foo() {}\nfoo();\n";
        let edits = vec![edit(0, 3, 0, 6, "longer"), edit(1, 0, 1, 3, "longer")];
        assert_eq!(apply_text_edits(text, edits), "fn longer() {}\nlonger();\n");
    }

    #[test]
    fn pos_to_byte_maps_line_and_char() {
        let text = "ab\ncd";
        assert_eq!(lsp_pos_to_byte(text, 0, 0), 0);
        assert_eq!(lsp_pos_to_byte(text, 0, 2), 2);
        assert_eq!(lsp_pos_to_byte(text, 1, 0), 3);
        assert_eq!(lsp_pos_to_byte(text, 1, 2), 5);
    }

    #[test]
    fn short_path_keeps_crate_dir_and_src_tail_or_filename() {
        use super::short_path;
        // The crate dir (segment before `/src/`) is kept so the crate is clear.
        assert_eq!(
            short_path("/home/u/.cargo/registry/src/index-abc/stm32f1xx-hal-0.10.0/src/gpio.rs"),
            "stm32f1xx-hal-0.10.0/src/gpio.rs"
        );
        assert_eq!(
            short_path("C:/x/proj/src/pins/utils/i2c1.rs"),
            "proj/src/pins/utils/i2c1.rs"
        );
        assert_eq!(short_path(r"C:\x\proj\src\main.rs"), "proj/src/main.rs");
        // No `/src/` → bare file name.
        assert_eq!(short_path("/home/u/.cargo/esp-hal/lib.rs"), "lib.rs");
    }

    #[test]
    fn generated_byte_ranges_finds_blocks_and_locks_fixes() {
        use super::generated_byte_ranges;
        use crate::panels::mcu_module::codegen::common::{GEN_BEGIN, GEN_END};

        let code = format!("use foo;\n{GEN_BEGIN}\nlet x = 1;\n{GEN_END}\nloop {{}}\n");
        let ranges = generated_byte_ranges(&code);
        assert_eq!(ranges.len(), 1, "one generated block");
        let (b, e) = ranges[0];
        // The block spans from GEN_BEGIN to the end of the GEN_END marker.
        assert_eq!(&code[b..b + GEN_BEGIN.len()], GEN_BEGIN);
        assert_eq!(&code[e - GEN_END.len()..e], GEN_END);

        // A fix on `let x = 1;` (inside the block) is locked; one on `use foo;`
        // (before it) and `loop {}` (after it) is not.
        let inside = code.find("let x").unwrap();
        let before = code.find("use foo").unwrap();
        let after = code.find("loop").unwrap();
        let overlaps = |s: usize, end: usize| ranges.iter().any(|&(b, e)| s < e && end > b);
        assert!(overlaps(inside, inside + 5));
        assert!(!overlaps(before, before + 3));
        assert!(!overlaps(after, after + 4));
    }

    #[test]
    fn generated_byte_ranges_handles_none_and_multiple() {
        use super::generated_byte_ranges;
        use crate::panels::mcu_module::codegen::common::{GEN_BEGIN, GEN_END};

        assert!(generated_byte_ranges("no markers here").is_empty());
        let two = format!("{GEN_BEGIN}\na\n{GEN_END}\nmid\n{GEN_BEGIN}\nb\n{GEN_END}\n");
        assert_eq!(generated_byte_ranges(&two).len(), 2);
    }

    fn span_edit(start: usize, end: usize, repl: &str) -> crate::build::SpanEdit {
        crate::build::SpanEdit {
            file: "src/main.rs".into(),
            start,
            end,
            replacement: repl.into(),
        }
    }

    #[test]
    fn apply_edits_to_buffer_applies_all_unlocked_back_to_front() {
        use super::apply_edits_to_buffer;
        // Two unused imports on consecutive lines (Apply-all collects both).
        let mut buf = "use a;\nuse b;\nfn main() {}".to_string();
        let edits = [span_edit(0, 7, ""), span_edit(7, 14, "")]; // remove each "use …;\n"
        let refs: Vec<&_> = edits.iter().collect();
        let n = apply_edits_to_buffer(&mut buf, &refs, &[]);
        assert_eq!(n, 2);
        assert_eq!(buf, "fn main() {}");
    }

    #[test]
    fn apply_edits_to_buffer_skips_locked_applies_rest() {
        use super::apply_edits_to_buffer;
        // "AAAA BBBB CCCC": remove AAAA (locked) + BBBB (free). Only BBBB goes.
        let mut buf = "AAAA BBBB CCCC".to_string();
        let edits = [span_edit(0, 4, ""), span_edit(5, 9, "")];
        let refs: Vec<&_> = edits.iter().collect();
        let n = apply_edits_to_buffer(&mut buf, &refs, &[(0, 4)]);
        assert_eq!(n, 1, "the locked AAAA edit is skipped");
        assert_eq!(buf, "AAAA  CCCC");
    }

    #[test]
    fn apply_edits_to_buffer_dedups_overlapping() {
        use super::apply_edits_to_buffer;
        let mut buf = "hello world".to_string();
        // Two edits over the same span (duplicate suggestion) → applied once.
        let edits = [span_edit(0, 5, "hi"), span_edit(0, 5, "hi")];
        let refs: Vec<&_> = edits.iter().collect();
        let n = apply_edits_to_buffer(&mut buf, &refs, &[]);
        assert_eq!(n, 1);
        assert_eq!(buf, "hi world");
    }
}
