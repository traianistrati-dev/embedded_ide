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
use std::sync::{Arc, Mutex};

// ── Module structure ──────────────────────────────────────────────────────────
mod tabs;

mod helpers;
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

/// How long editing must pause before rust-analyzer is asked to re-verify
/// (diagnostics / cargo-check). A Project Save flushes immediately regardless.
const LSP_IDLE_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(3);

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

/// Read the file a definition points to and build the snippet shown in the
/// "Definition" tab (a window of lines around the target). `None` if unreadable.
fn build_definition_view(loc: &lsp::DefinitionLoc) -> Option<DefinitionView> {
    let content = std::fs::read_to_string(&loc.path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let target = (loc.line as usize).min(lines.len() - 1);
    let from = target.saturating_sub(2); // a little context above
    let to = (target + 150).min(lines.len());
    let code = lines[from..to].join("\n");
    Some(DefinitionView {
        header: format!("{}  (line {})", short_path(&loc.path), loc.line + 1),
        code,
        highlight: target - from, // the def line's index within the snippet
    })
}

/// Shorten a definition path for the tab header: the `src/…` tail when present,
/// else the bare file name.
fn short_path(path: &str) -> String {
    let norm = path.replace('\\', "/");
    if let Some(i) = norm.rfind("/src/") {
        norm[i + 1..].to_string()
    } else {
        norm.rsplit('/').next().unwrap_or(&norm).to_string()
    }
}

/// Apply LSP text edits to `text`. Edits are non-overlapping; applying them
/// back-to-front (by start position) keeps earlier offsets valid.
fn apply_text_edits(text: &str, mut edits: Vec<lsp::RenameEdit>) -> String {
    edits.sort_by(|a, b| {
        (b.start_line, b.start_char).cmp(&(a.start_line, a.start_char))
    });
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
    /// Editable project config files. Each holds the current content (a
    /// `<<< GENERATED >>>` block the IDE refreshes on chip change, plus whatever
    /// the user adds outside it). `memory_x`/`build_rs` are empty for ESP.
    cargo_toml: String,
    cargo_config: String,
    memory_x: String,
    build_rs: String,
    gitignore: String,
    /// Active tab in the MCU configurator
    active_tab: McuTab,
    /// Currently selected file in the project tree
    selected_file: ProjectFileId,
    /// Shown briefly after a successful copy
    copy_flash: u8,
    /// >0: show export status message countdown
    export_flash: u8,
    /// Last export result message
    export_msg: String,
    // ── Build ────────────────────────────────────────────────────────────────
    /// egui context stored for cross-thread repaint requests
    egui_ctx: egui::Context,
    /// Shared state written by the background build thread
    build_state: Arc<Mutex<BuildState>>,
    /// Index of the diagnostic currently expanded in the cargo build panel
    selected_diagnostic: Option<usize>,
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
    /// Insert text deferred from a mouse-click on a completion item.
    /// Applied at the start of the next frame (before the editor renders).
    completion_pending_insert: Option<String>,
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
    /// The file + 1-based line of the last-clicked diagnostic. Highlighted with a
    /// translucent dark-red band in the editor until another diagnostic is clicked.
    highlighted_error_line: Option<(ProjectFileId, usize)>,
    // ── rust-analyzer LSP ────────────────────────────────────────────────────
    /// Shared LSP client state (updated from background threads)
    lsp_state: Arc<Mutex<lsp::LspState>>,
    /// LSP edit-debounce: RA only re-verifies (didChange + workspace disk write)
    /// 3 s after editing stops, or on an explicit Project Save — NOT on every
    /// keystroke. `lsp_prev_hash` detects a change this frame (resets the idle
    /// timer); `lsp_synced_hash` is what RA last received (so we know if there
    /// is anything to flush). See `init_frame`.
    lsp_prev_hash: u64,
    lsp_synced_hash: u64,
    /// When the LSP-relevant content (main.rs + user files) last changed.
    lsp_last_edit: Option<std::time::Instant>,
    /// Set by a Project Save to flush pending changes to RA immediately.
    lsp_flush_requested: bool,
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
    /// Request keyboard focus for the rename input on the frame it opens.
    rename_focus: bool,
    // ── Go to definition (F12 → textDocument/definition) ─────────────────────
    /// `true` after an F12 request, until the definition arrives.
    definition_in_flight: bool,
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
        cc.egui_ctx
            .options_mut(|o| o.input_options.horizontal_scroll_modifier = egui::Modifiers::NONE);

        // ── Load persisted project state ─────────────────────────────────────
        let persisted: PersistedState = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();

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
            cargo_toml: init_files.cargo_toml,
            cargo_config: init_files.cargo_config,
            memory_x: init_files.memory_x,
            build_rs: init_files.build_rs,
            gitignore: init_files.gitignore,
            mcu: Some(mcu),
            active_tab: McuTab::Pins,
            selected_file: ProjectFileId::MainRs,
            copy_flash: 0,
            export_flash: 0,
            export_msg: String::new(),
            egui_ctx: cc.egui_ctx.clone(),
            build_state: Arc::new(Mutex::new(BuildState::Idle)),
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
            lsp_state: Arc::new(Mutex::new(lsp::LspState::default())),
            lsp_prev_hash: 0,
            lsp_synced_hash: 0,
            lsp_last_edit: None,
            lsp_flush_requested: false,
            rename_active: false,
            rename_input: String::new(),
            rename_rel: String::new(),
            rename_line: 0,
            rename_char: 0,
            rename_popup_pos: egui::Pos2::ZERO,
            rename_in_flight: false,
            rename_focus: false,
            definition_in_flight: false,
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
    /// files (with their current user edits). This — not a fresh regeneration —
    /// is what the editor shows and what `write_project` persists.
    fn current_project_files(&self) -> ProjectFiles {
        ProjectFiles {
            main_rs: self.generated_code.clone(),
            cargo_toml: self.cargo_toml.clone(),
            cargo_config: self.cargo_config.clone(),
            memory_x: self.memory_x.clone(),
            build_rs: self.build_rs.clone(),
            gitignore: self.gitignore.clone(),
        }
    }

    /// The `mcu.config` text for the live MCU (virtual modules + clock), written
    /// alongside the project by `write_project`. Empty when no chip is selected.
    fn mcu_config_text(&self) -> String {
        self.mcu.as_ref().map(|m| m.mcu_config_text()).unwrap_or_default()
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
        }
    }

    // ── Frame initialization (frame state, LSP, MCU synchronization) ───────────
    fn init_frame(&mut self, _ui: &mut egui::Ui) {
        // ── Poll filesystem watcher events ────────────────────────────────────
        self.poll_fs_events();

        // ── Update generated section when pin config changes ─────────────────
        if let Some(mcu) = &self.mcu {
            let updated = mcu.update_main_rs(&self.generated_code);
            if updated != self.generated_code {
                self.generated_code = updated;
            }
        }

        // Tick flash counters down
        if self.copy_flash > 0 {
            self.copy_flash -= 1;
        }
        if self.export_flash > 0 {
            self.export_flash -= 1;
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
                // Debounced verification: push edits to rust-analyzer (and the
                // workspace on disk for cargo-check) ONLY 3 s after editing stops
                // or on an explicit Project Save — never on every keystroke.
                let cur_hash = self.lsp_content_hash();
                if cur_hash != self.lsp_prev_hash {
                    // Content changed this frame → reset the idle timer.
                    self.lsp_prev_hash = cur_hash;
                    self.lsp_last_edit = Some(std::time::Instant::now());
                }
                let dirty = cur_hash != self.lsp_synced_hash;
                let idle_done = self
                    .lsp_last_edit
                    .map(|t| t.elapsed() >= LSP_IDLE_DEBOUNCE)
                    .unwrap_or(true);
                // Flush (→ RA re-verifies) on a Project Save unconditionally, or
                // 3 s after editing stops. A Save re-writes the workspace + re-sends
                // the document even when unchanged, so verification always restarts.
                let force = self.lsp_flush_requested;
                if force || (dirty && idle_done) {
                    self.flush_lsp_to_workspace(force);
                    self.lsp_synced_hash = cur_hash;
                    self.lsp_flush_requested = false;
                } else if dirty {
                    // Still typing — wake up after the debounce so the flush
                    // fires even with no further input events.
                    self.egui_ctx.request_repaint_after(LSP_IDLE_DEBOUNCE);
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
                self.egui_ctx.request_repaint();
            }
        }

        // ── Show a completed F12 go-to-definition in the Definition tab ───────
        if self.definition_in_flight {
            let result = self.lsp_state.lock().unwrap().take_definition_result();
            if let Some(loc) = result {
                self.definition_in_flight = false;
                if let Some(loc) = loc.and_then(|l| build_definition_view(&l)) {
                    self.definition_view = Some(loc);
                    self.build_tab = BuildPanelTab::Definition;
                }
                self.egui_ctx.request_repaint();
            }
        }
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
                if let Some(entry) =
                    self.project_tree.user_src_files.iter_mut().find(|(p, _)| p == sub)
                {
                    entry.1 = apply_text_edits(&entry.1, es);
                }
            }
        }
    }

    /// Hash of the LSP-relevant content (main.rs + every user source file), used
    /// to detect edits between frames without sending anything to RA.
    fn lsp_content_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.generated_code.hash(&mut h);
        for (rel, content) in &self.project_tree.user_src_files {
            rel.hash(&mut h);
            content.hash(&mut h);
        }
        h.finish()
    }

    /// Single point where rust-analyzer is asked to re-verify: writes main.rs +
    /// every user source file to the LSP workspace (so cargo-check/flycheck sees
    /// them) and pushes `didChange` (so RA's in-memory analysis updates). Called
    /// only from the debounced path — never on every keystroke.
    ///
    /// `force` (a Project Save) re-sends every document even if unchanged, so RA
    /// re-runs its analysis/flycheck — i.e. Save restarts verification.
    fn flush_lsp_to_workspace(&mut self, force: bool) {
        let workspace = std::env::temp_dir().join("embedded_ide_0_check");
        let write = |rel: &str, content: &str| {
            let dest = workspace.join("src").join(rel);
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&dest, content.as_bytes());
        };
        write("main.rs", &self.generated_code);
        for (rel, content) in &self.project_tree.user_src_files {
            write(rel, content);
        }

        let mut lsp = self.lsp_state.lock().unwrap();
        lsp.did_change("src/main.rs", &self.generated_code, force);
        for (rel, content) in &self.project_tree.user_src_files {
            lsp.did_change(&format!("src/{rel}"), content, force);
        }
        // Trigger RA's `checkOnSave` flycheck so its cargo-check diagnostics
        // re-run against the just-flushed text (otherwise they stay frozen at
        // the startup check and fixed errors linger). One didSave re-checks the
        // whole workspace; RA coalesces, and flush is already debounced (3 s idle
        // / Project Save).
        lsp.did_save("src/main.rs");
        for (rel, _) in &self.project_tree.user_src_files {
            lsp.did_save(&format!("src/{rel}"));
        }
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

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Initialize frame state (polling, LSP, MCU updates)
        self.init_frame(ui);

        // Detect Ctrl+S for Save/Export project
        let ctrl_s_pressed = ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S));

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
        if save_project_clicked {
            if self.selected_build_cfg().is_some() {
                let dest: Option<std::path::PathBuf> = match &self.project_dir {
                    Some(dir) => Some(dir.clone()),
                    None => rfd::FileDialog::new()
                        .set_title("Choose folder to save the new project")
                        .pick_folder(),
                };
                if let Some(dest) = dest {
                    match project_gen::write_project(
                        &dest,
                        &self.current_project_files(),
                        &self.project_tree.user_src_files,
                        &self.mcu_config_text(),
                    ) {
                        Ok(()) => {
                            let name = dest
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("saved")
                                .to_string();
                            self.export_msg =
                                format!("{}  {name}", egui_phosphor::regular::CHECK_CIRCLE);
                            self.export_flash = 180;
                            self.project_name = Some(name);
                            // A new project now has a home — subsequent saves
                            // write here without prompting.
                            self.project_dir = Some(dest);
                        }
                        Err(e) => {
                            self.export_msg =
                                format!("{}  {e}", egui_phosphor::regular::X_CIRCLE);
                            self.export_flash = 180;
                        }
                    }
                }
            }
        }

        // ── Modal dialogs: New Project / New File / New Folder ─────
        self.show_new_project_dialog(ui, &mut save_project_needed);
        self.show_new_file_dialog(ui, &mut save_project_needed);
        self.show_new_folder_dialog(ui, &mut save_project_needed);

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
        // pending edits to rust-analyzer next frame, so RA re-verifies on save —
        // outside the 3 s idle debounce.
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
    fn short_path_keeps_src_tail_or_filename() {
        use super::short_path;
        assert_eq!(short_path("C:/x/proj/src/pins/utils/i2c1.rs"), "src/pins/utils/i2c1.rs");
        assert_eq!(short_path(r"C:\x\proj\src\main.rs"), "src/main.rs");
        assert_eq!(short_path("/home/u/.cargo/.../esp-hal/lib.rs"), "lib.rs");
    }
}
