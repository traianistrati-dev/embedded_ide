use crate::build::{self, BuildState};
use crate::dfu::{self, DfuState};
use crate::espflash::{self, EspFlashState};
use crate::lsp::{self, LspStatus};
use crate::openocd::{self, OpenOcdState};
use crate::required_tools;
use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::mcu_catalog::{McuType, ToolchainKind};
use crate::panels::mcu_module::mock_esp32c3::create_esp32c3;
use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;
use crate::panels::mcu_module::pin_module::pin::Pin;
use crate::panels::mcu_module::pin_module::pin_function::PinFunction;
use crate::panels::mcu_module::project_gen::{self, ProjectFiles};
use eframe::egui;
use egui_code_editor::{CodeEditor, ColorTheme, Completer, Syntax};
use egui_phosphor::regular as ph;
use notify::Watcher as _;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// ── Project file selector ─────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug, Default)]
enum ProjectFileId {
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
        // All .rs files get full Rust highlighting.
        // TOML/memory.x use the same for now (no other built-in syntax available).
        Syntax::rust()
    }

    /// Path as reported by `rustc` in JSON diagnostics (relative to project root).
    /// Returns `None` for files that rustc never reports errors for.
    fn cargo_path(self) -> Option<&'static str> {
        match self {
            Self::MainRs => Some("src/main.rs"),
            Self::BuildRs => Some("build.rs"),
            Self::CargoToml => Some("Cargo.toml"),
            _ => None,
        }
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
}

// ── Persisted project state ───────────────────────────────────────────────────
// Everything that must survive an application restart.
// Stored via eframe's platform storage (Registry on Windows, ~/.local on Linux).

const STORAGE_KEY: &str = "embedded_ide_project_v1";

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
    selected_mcu_type: McuType,
    /// None when the selected chip is not yet implemented
    mcu: Option<Mcu>,
    /// Generated Rust HAL code — rebuilt each frame from pin state
    generated_code: String,
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
    dfu_programmers: Arc<Mutex<Vec<dfu::ProgrammerInfo>>>,
    /// Index of the programmer currently selected in the ComboBox
    dfu_sel_programmer: usize,
    /// Flash start address sent to dfu-util (editable; default = 0x08000000)
    dfu_flash_addr: String,
    /// Shared state for OpenOCD SWD flash operations
    openocd_state: Arc<Mutex<OpenOcdState>>,
    /// Target config file passed to OpenOCD (e.g. "target/stm32f1x.cfg")
    openocd_target_cfg: String,
    /// Shared state for ESP32 espflash operations
    espflash_state: Arc<Mutex<EspFlashState>>,
    /// Shared state for the Required Tools tab (check + install operations)
    tools_state: Arc<Mutex<required_tools::ToolsState>>,
    /// Code-completion engine — stores the trie, current prefix and popup state.
    /// Must live in the App (not a local) so state is preserved across frames.
    completer: Completer,
    /// True when the LSP completion popup is visible.
    completion_open:        bool,
    /// Index of the currently highlighted row in the completion popup.
    completion_sel:         usize,
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
    // ── rust-analyzer LSP ────────────────────────────────────────────────────
    /// Shared LSP client state (updated from background threads)
    lsp_state: Arc<Mutex<lsp::LspState>>,
    /// Which tab is active in the bottom diagnostics panel
    build_tab: BuildPanelTab,
    /// Index of the RA diagnostic row that is expanded
    lsp_selected_diagnostic: Option<usize>,
    // ── Diagnostics panel layout ─────────────────────────────────────────────
    /// Height of the bottom diagnostics panel in pixels — persisted so the
    /// user's drag position is remembered across show/hide cycles.
    diag_panel_height: f32,
    // ── User source files ─────────────────────────────────────────────────────
    /// Extra .rs files created by the user inside src/
    /// Each entry is `(path_relative_to_src, content)`, e.g. `("utils.rs", "")`.
    user_src_files: Vec<(String, String)>,
    /// Explicitly-created folders inside src/ (may be empty).
    user_src_folders: Vec<String>,
    /// While `Some(s)`, a text-input for naming a new file is shown in the tree.
    new_src_name: Option<String>,
    /// While `Some(s)`, a text-input for naming a new folder is shown in the tree.
    new_src_folder_name: Option<String>,
    /// While `Some((folder, name))`, an inline file-name input is open inside that folder.
    new_file_in_folder: Option<(String, String)>,
    /// While `Some((idx, new_name))`, an inline rename input is shown for that user file.
    renaming_file: Option<(usize, String)>,
    /// While `Some((old_folder, new_name))`, an inline rename input is shown for that folder.
    renaming_folder: Option<(String, String)>,
    // ── Project management ────────────────────────────────────────────────────
    /// `true` while the "New Project" confirmation dialog is open.
    confirm_new_project: bool,
    /// Chip type staged inside the "New Project" popup.
    /// `None` = "Empty" (no chip change on confirm).
    pending_mcu_type: Option<McuType>,
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

        // ── Load persisted project state ─────────────────────────────────────
        let persisted: PersistedState = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();

        let mcu = create_stm32f103c8tx();
        let generated_code = mcu.fresh_main_rs();

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
        let dfu_programmers: Arc<Mutex<Vec<dfu::ProgrammerInfo>>> =
            Arc::new(Mutex::new(Vec::new()));
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
            selected_mcu_type: McuType::Stm32f103c8t6,
            generated_code,
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
            dfu_sel_programmer: 0,
            dfu_flash_addr: "0x08000000".to_string(),
            openocd_state,
            openocd_target_cfg: "target/stm32f1x.cfg".to_string(),
            espflash_state,
            tools_state: required_tools::make_tools_state(),
            // Completer: seeded with Rust keywords/types + learns words from code
            completer: Completer::new_with_syntax(&Syntax::rust())
                .with_auto_indent()
                .with_user_words(),
            completion_open:           false,
            completion_sel:            0,
            completion_trigger_idx:    0,
            completion_pending_insert: None,
            completion_filtered_items: Vec::new(),
            lsp_state: Arc::new(Mutex::new(lsp::LspState::default())),
            build_tab: BuildPanelTab::RustAnalyzer,
            lsp_selected_diagnostic: None,
            diag_panel_height: 180.0,
            user_src_files: persisted.user_src_files,
            user_src_folders: persisted.user_src_folders,
            new_src_name: None,
            new_src_folder_name: None,
            new_file_in_folder: None,
            renaming_file: None,
            renaming_folder: None,
            confirm_new_project: false,
            pending_mcu_type: None,
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

    fn init_mcu(mcu_type: &McuType) -> Option<Mcu> {
        match mcu_type {
            McuType::Stm32f103c8t6 => Some(create_stm32f103c8tx()),
            McuType::Esp32c3       => Some(create_esp32c3()),
            _                      => None,
        }
    }

    // ── Project load ──────────────────────────────────────────────────────────

    /// Loads user source files from an existing Cargo project at `root`.
    /// Only files in `root/src/` are imported; `main.rs` is always skipped
    /// (it is regenerated from MCU pin state).
    /// Any previous user files are replaced.
    fn load_project_from_dir(&mut self, root: &std::path::Path) {
        let src_dir = root.join("src");
        if !src_dir.exists() {
            return;
        }

        let mut files: Vec<(String, String)> = Vec::new();
        let mut folders: Vec<String> = Vec::new();

        Self::scan_src_dir(&src_dir, &src_dir, &mut files, &mut folders);

        self.user_src_files = files;
        self.user_src_folders = folders;
        self.selected_file = ProjectFileId::MainRs;
        self.renaming_file = None;
        self.renaming_folder = None;
        self.new_src_name = None;
        self.new_src_folder_name = None;
        self.new_file_in_folder = None;
        self.project_name = root.file_name().and_then(|n| n.to_str()).map(String::from);
        self.project_dir = Some(root.to_path_buf());

        // ── Detect MCU type from Cargo.toml ──────────────────────────────────
        // Read the dependency block to determine which chip this project targets.
        // Switching the MCU type BEFORE restoring pin state ensures the correct
        // pin diagram is active when parse_main_rs() is applied.
        let cargo_toml = root.join("Cargo.toml");
        if let Ok(cargo) = std::fs::read_to_string(&cargo_toml) {
            let detected = if cargo.contains("stm32f1xx-hal") {
                Some(McuType::Stm32f103c8t6)
            } else if cargo.contains("esp-hal") {
                Some(McuType::Esp32c3)
            } else {
                None
            };
            if let Some(mcu_type) = detected {
                if mcu_type != self.selected_mcu_type {
                    self.selected_mcu_type = mcu_type.clone();
                    self.mcu = Self::init_mcu(&mcu_type);
                    // Reset LSP — it was attached to the previous chip's workspace.
                    self.lsp_state.lock().unwrap().reset();
                    self.lsp_selected_diagnostic = None;
                }
            }
        }

        // ── Restore pin state from src/main.rs ───────────────────────────────
        // Parse the GEN_BEGIN…GEN_END block and apply every recognised pin
        // assignment back to the MCU diagram.  If no markers are found (e.g.
        // an ESP32-C3 project or a hand-written main.rs) this is a silent no-op.
        let main_rs_path = src_dir.join("main.rs");
        if let Ok(source) = std::fs::read_to_string(&main_rs_path) {
            use crate::panels::mcu_module::codegen;
            let saved = codegen::parse_main_rs(&source);
            if !saved.is_empty() {
                if let Some(mcu) = &mut self.mcu {
                    mcu.apply_saved_pins(&saved);
                    // Rebuild generated_code from the restored pin state while
                    // keeping the user's loop body from the existing file.
                    self.generated_code = mcu.update_main_rs(&source);
                }
            } else {
                // No parseable pins (blank STM32 project, ESP32-C3, or
                // hand-written main.rs).  Always reset the MCU diagram so
                // pins configured in the previously-open project do not
                // bleed into this one.
                if let Some(mcu) = &mut self.mcu {
                    mcu.reset_all_pins();
                }
                self.generated_code = source;
            }
        }
    }

    /// Recursively scans `dir` (relative to `root`) and fills `files` and `folders`.
    /// Skips `main.rs` and any non-`.rs` files.
    fn scan_src_dir(
        root: &std::path::Path,
        dir: &std::path::Path,
        files: &mut Vec<(String, String)>,
        folders: &mut Vec<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let Ok(rel) = path.strip_prefix(root) else {
                    continue;
                };
                let rel = rel.to_string_lossy().replace('\\', "/");
                if !folders.contains(&rel) {
                    folders.push(rel);
                }
                Self::scan_src_dir(root, &path, files, folders);
            } else if path.is_file() {
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(rel) = path.strip_prefix(root) else {
                    continue;
                };
                let rel = rel.to_string_lossy().replace('\\', "/");
                if rel == "main.rs" {
                    continue; // always generated — skip
                }
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                files.push((rel, content));
            }
        }
    }

    // ── Filesystem watcher polling ────────────────────────────────────────────
    /// Drains the notify channel and applies any relevant Create / Remove /
    /// Rename events to `user_src_files` and `user_src_folders`.
    ///
    /// Rules:
    /// - Only `.rs` files inside `workspace/src/` are tracked.
    /// - `src/main.rs` is always excluded (it is the generated file).
    /// - Create: add if not already present (avoids duplicates from our own writes).
    /// - Remove: drop from the list (IDE-initiated removes are already gone by
    ///   the time notify fires, so the search is a no-op — safe either way).
    /// - Rename(Both): atomically update the stored path.
    fn poll_fs_events(&mut self) {
        use notify::EventKind::*;
        use notify::event::{ModifyKind, RenameMode};

        let workspace_src = std::env::temp_dir()
            .join("embedded_ide_0_check")
            .join("src");

        // If the watcher hasn't started watching yet (dir didn't exist on
        // startup), try to attach now that write_project may have created it.
        if let (Some(w), true) = (self._fs_watcher.as_mut(), workspace_src.exists()) {
            // `watch` is idempotent for already-watched paths.
            let _ = w.watch(&workspace_src, notify::RecursiveMode::Recursive);
        }

        let Some(rx) = self.fs_rx.as_ref() else {
            return;
        };

        for event in rx.try_iter().flatten() {
            match event.kind {
                // ── New file created externally ──────────────────────────────
                Create(_) => {
                    for abs in &event.paths {
                        if abs.extension().and_then(|e| e.to_str()) != Some("rs") {
                            continue;
                        }
                        let Ok(rel) = abs.strip_prefix(&workspace_src) else {
                            continue;
                        };
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        if rel == "main.rs" {
                            continue;
                        }
                        if !self.user_src_files.iter().any(|(p, _)| p == &rel) {
                            // Read the file content so the editor shows it correctly
                            let content = std::fs::read_to_string(abs).unwrap_or_default();
                            self.user_src_files.push((rel, content));
                        }
                    }
                }
                // ── File removed externally ──────────────────────────────────
                Remove(_) => {
                    for abs in &event.paths {
                        let Ok(rel) = abs.strip_prefix(&workspace_src) else {
                            continue;
                        };
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        self.user_src_files.retain(|(p, _)| p != &rel);
                        // If a whole directory was removed, drop its folder entry
                        let dir_rel = rel.trim_end_matches('/').to_string();
                        self.user_src_folders.retain(|f| f != &dir_rel);
                    }
                }
                // ── File renamed externally (notify sends both paths together)
                Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() == 2 => {
                    let old = &event.paths[0];
                    let new = &event.paths[1];
                    let Ok(old_rel) = old.strip_prefix(&workspace_src) else {
                        continue;
                    };
                    let Ok(new_rel) = new.strip_prefix(&workspace_src) else {
                        continue;
                    };
                    let old_rel = old_rel.to_string_lossy().replace('\\', "/");
                    let new_rel = new_rel.to_string_lossy().replace('\\', "/");
                    // File rename
                    if let Some((p, _)) =
                        self.user_src_files.iter_mut().find(|(p, _)| p == &old_rel)
                    {
                        *p = new_rel.clone();
                    }
                    // Folder rename — update folder list + all child paths
                    if let Some(f) = self.user_src_folders.iter_mut().find(|f| **f == old_rel) {
                        *f = new_rel.clone();
                    }
                    let old_prefix = format!("{old_rel}/");
                    let new_prefix = format!("{new_rel}/");
                    for (path, _) in &mut self.user_src_files {
                        if path.starts_with(&old_prefix) {
                            *path = format!("{new_prefix}{}", &path[old_prefix.len()..]);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // ── Pin-file scaffold ─────────────────────────────────────────────────────

    /// Parses an STM32 pin name ("PA1", "PB12", …) into (port_char, pin_index).
    /// Returns `None` if the name does not match the expected `P[A-Z][0-9]+` format.
    fn parse_stm32_pin(pin_name: &str) -> Option<(char, u8)> {
        let upper = pin_name.to_uppercase();
        let mut chars = upper.chars();
        if chars.next()? != 'P' {
            return None;
        }
        let port = chars.next()?;
        let idx: u8 = chars.as_str().parse().ok()?;
        Some((port, idx))
    }

    /// Generates the HAL type-alias source for `pins/pin{N}_{name}.rs`.
    /// Always called fresh when a function changes, so the alias stays in sync.
    fn generate_pin_content(pin_num: usize, pin_name: &str, func: &PinFunction) -> String {
        let Some(mode) = func.hal_gpio_mode() else {
            return String::new();
        };

        let Some((port, idx)) = Self::parse_stm32_pin(pin_name) else {
            // Non-STM32 name — emit a plain comment stub
            return format!(
                "// Pin {pin_num} — {pin_name}\n// Function: {label}\n",
                label = func.label()
            );
        };

        // Trailing comment only for functions that carry extra parameters
        // (ADC channel numbers, timer/channel, peripheral index, etc.)
        let comment = match func {
            PinFunction::GpioInput | PinFunction::GpioOutput => String::new(),
            other => format!(" // {}", other.label()),
        };

        format!(
            "use stm32f1xx_hal::gpio::{{{mode}, Pin}};\n\
             pub type PinType = Pin<'{port}', {idx}, {mode}>;{comment}\n",
        )
    }

    /// Called whenever a pin receives a non-Unset function.
    ///
    /// * Ensures `pins/` folder and `pins/mod.rs` exist.
    /// Full sync of the `pins/` directory against the current MCU pin state.
    ///
    /// Called after **any** pin function change (including deselection to Unset):
    ///
    /// * Removes `pins/pin*.rs` files whose pin is no longer configured.
    /// * Creates or overwrites `pins/pin{N}_{name}.rs` for every configured pin.
    /// * Rebuilds `pins/mod.rs` from scratch with only the active declarations.
    /// * Ensures the `pins/` folder entry exists.
    fn sync_pin_files(
        files: &mut Vec<(String, String)>,
        folders: &mut Vec<String>,
        all_pins: &[(usize, String, PinFunction)],
    ) {
        const MOD_PATH: &str = "pins/mod.rs";

        // ── Build the authoritative set of configured pins ────────────────────
        // `(slug, pin_num, pin_name, func)` for every pin with a real function.
        let configured: Vec<(String, usize, &str, &PinFunction)> = all_pins
            .iter()
            .filter(|(_, _, f)| *f != PinFunction::Unset)
            .map(|(num, name, func)| {
                let slug = format!("pin{}_{}", num, name.to_lowercase());
                (slug, *num, name.as_str(), func)
            })
            .collect();

        let active_slugs: Vec<&str> = configured.iter().map(|(s, ..)| s.as_str()).collect();

        // ── 1. Ensure pins/ folder is registered ─────────────────────────────
        let folder = "pins".to_string();
        if !folders.contains(&folder) {
            folders.push(folder);
        }

        // ── 2. Ensure pins/mod.rs exists (content rebuilt below) ─────────────
        if !files.iter().any(|(p, _)| p == MOD_PATH) {
            files.push((MOD_PATH.to_string(), String::new()));
        }

        // ── 3. Drop pin files that are no longer configured ───────────────────
        files.retain(|(path, _)| {
            // Only act on paths inside pins/ that look like pin files
            let Some(fname) = path.strip_prefix("pins/") else {
                return true;
            };
            if fname == "mod.rs" {
                return true;
            } // never drop mod.rs itself
            if !fname.starts_with("pin") || !fname.ends_with(".rs") {
                return true;
            }
            let slug = &fname[..fname.len() - 3]; // strip ".rs"
            active_slugs.contains(&slug)
        });

        // ── 4. Create pin files that don't yet exist ─────────────────────────
        // Only write when the file is brand-new.  If it already exists the user
        // may have added custom code below the generated type alias — never
        // overwrite it.  (Removing and re-adding a pin generates a fresh file.)
        for (slug, num, name, func) in &configured {
            let file_path = format!("pins/{slug}.rs");
            if !files.iter().any(|(p, _)| p == &file_path) {
                let content = Self::generate_pin_content(*num, name, func);
                files.push((file_path, content));
            }
        }

        // ── 5. Rebuild mod.rs from scratch (only active pins) ─────────────────
        let new_mod: String = configured
            .iter()
            .map(|(slug, ..)| format!("pub mod {slug};\n"))
            .collect();

        if let Some((_, mod_content)) = files.iter_mut().find(|(p, _)| p == MOD_PATH) {
            *mod_content = new_mod;
        }
    }

    /// Creates the initial `pins/` scaffold (folder + empty mod.rs).
    /// Called once when "New Project" is confirmed so the tree is
    /// pre-populated before any pin is configured.
    fn init_pins_scaffold(files: &mut Vec<(String, String)>, folders: &mut Vec<String>) {
        let folder = "pins".to_string();
        let mod_path = "pins/mod.rs".to_string();
        if !folders.contains(&folder) {
            folders.push(folder);
        }
        if !files.iter().any(|(p, _)| p == &mod_path) {
            files.push((mod_path, String::new()));
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
                user_src_files: self.user_src_files.clone(),
                user_src_folders: self.user_src_folders.clone(),
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
        // ── Poll filesystem watcher events ────────────────────────────────────
        // Called every frame (non-blocking).  We react only to .rs file
        // Create / Remove / Rename events so external tools (VS Code, etc.)
        // are reflected in the project tree automatically.
        self.poll_fs_events();

        // ── Update generated section when pin config changes ─────────────────
        // update_main_rs() splices only the GENERATED block (between markers),
        // leaving custom_config() and loop_code() untouched.
        // Comparing the result avoids touching the string when nothing changed.
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
        // Drive rust-analyzer: start it once, send didOpen/didChange as needed.
        {
            let lsp_status = self.lsp_state.lock().unwrap().status.clone();
            match lsp_status {
                LspStatus::Stopped => {
                    // Auto-start when we have a supported chip (project on disk needed).
                    if self.selected_mcu_type.project_config().is_some() {
                        let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
                        if let Some(config) = self.selected_mcu_type.project_config() {
                            if project_gen::write_project(
                                &build_dir,
                                &config,
                                &self.generated_code,
                                &self.user_src_files,
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
                    // Handshake complete — open the main file so RA starts analysing.
                    let mut lsp = self.lsp_state.lock().unwrap();
                    if !lsp.did_open_sent {
                        let code = self.generated_code.clone();
                        lsp.did_open(&code);
                    }
                }
                LspStatus::Ready => {
                    // Sync any code change incrementally.
                    let code = self.generated_code.clone();
                    self.lsp_state.lock().unwrap().did_change(&code);
                }
                _ => {}
            }
        }

        // ── Build project files snapshot ─────────────────────────────────────
        // Cheap (pure string fmt); used by both tree panel and code editor.
        let project_files: Option<ProjectFiles> = self
            .selected_mcu_type
            .project_config()
            .map(|cfg| project_gen::build_project_files(&cfg, &self.generated_code));

        // ── Panel 1: Project Tree ─────────────────────────────────────────────
        // Set to true inside the tree when files are added/deleted, so we
        // write the whole project to the workspace directory afterwards.
        let mut save_project_needed = false;
        // Signals set inside the panel closure, acted on outside.
        let mut open_project_clicked = false;
        let mut new_project_clicked = false;

        egui::Panel::left("project_tree")
            .resizable(true)
            .default_size(200.0)
            .show_inside(ui, |ui| {
                // ── Panel header row ──────────────────────────────────────────
                ui.horizontal(|ui| {
                    ui.heading("Project");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn = |ui: &mut egui::Ui, icon: &str, label: &str, tip: &str| {
                            ui.add(egui::Button::new(
                                egui::RichText::new(format!("{icon} {label}")).size(11.0),
                            ))
                            .on_hover_text(tip)
                            .clicked()
                        };
                        if btn(
                            ui,
                            ph::FOLDER_OPEN,
                            "Open",
                            "Open an existing project folder",
                        ) {
                            open_project_clicked = true;
                        }
                        ui.add_space(2.0);
                        if btn(ui, ph::NOTE_PENCIL, "New", "Start a new empty project") {
                            new_project_clicked = true;
                        }
                    });
                });

                ui.separator();
                // Show project name under the heading when one is loaded
                if let Some(name) = &self.project_name {
                    ui.label(
                        egui::RichText::new(format!("  {}", name))
                            .size(10.5)
                            .color(egui::Color32::from_rgb(140, 160, 180))
                            .italics(),
                    );
                }

                match (&project_files, self.selected_mcu_type.project_config()) {
                    (Some(_), Some(cfg)) => {
                        let build_guard = self.build_state.lock().unwrap();
                        let build_result = build_guard.result().cloned();
                        drop(build_guard);
                        let lsp_guard = self.lsp_state.lock().unwrap();
                        let workspace_dir = std::env::temp_dir().join("embedded_ide_0_check");
                        show_project_tree(
                            ui,
                            cfg.pkg_name,
                            &cfg.toolchain,
                            &mut self.selected_file,
                            build_result.as_ref(),
                            Some(&*lsp_guard),
                            &mut self.user_src_files,
                            &mut self.user_src_folders,
                            &mut self.new_src_name,
                            &mut self.new_src_folder_name,
                            &mut self.new_file_in_folder,
                            &mut self.renaming_file,
                            &mut self.renaming_folder,
                            &workspace_dir,
                            &mut save_project_needed,
                        );
                    }
                    _ => {
                        ui.add_space(12.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new("Export not available\nfor this chip yet.")
                                    .size(11.0)
                                    .color(egui::Color32::GRAY),
                            );
                        });
                    }
                }
            });

        // ── Handle toolbar button clicks ──────────────────────────────────────

        // "New Project" → ask for confirmation; default chip selection = Empty
        if new_project_clicked {
            self.confirm_new_project = true;
            self.pending_mcu_type = None;
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

        // ── "New Project" confirmation modal ──────────────────────────────────
        if self.confirm_new_project {
            egui::Window::new("New Project")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::LEFT_TOP, [20.0, 10.0])
                .show(ui.ctx(), |ui| {
                    ui.add_space(4.0);
                    ui.label("This will clear all user files and folders.");
                    ui.label(
                        egui::RichText::new("The action cannot be undone.")
                            .color(egui::Color32::from_rgb(220, 160, 60)),
                    );
                    ui.add_space(8.0);

                    // ── Chip selector ─────────────────────────────────────────
                    ui.horizontal(|ui| {
                        ui.label("Chip:");
                        let selected_text = match &self.pending_mcu_type {
                            None    => "— Empty —".to_string(),
                            Some(t) => t.label().to_string(),
                        };
                        egui::ComboBox::from_id_salt("new_project_chip_selector")
                            .selected_text(selected_text)
                            .show_ui(ui, |ui| {
                                // "Empty" — first entry, no chip selected
                                ui.selectable_value(
                                    &mut self.pending_mcu_type,
                                    None,
                                    "— Empty —",
                                );
                                for mcu_type in McuType::all() {
                                    let label = if mcu_type.is_supported() {
                                        mcu_type.label().to_string()
                                    } else {
                                        format!("{} — coming soon", mcu_type.label())
                                    };
                                    ui.selectable_value(
                                        &mut self.pending_mcu_type,
                                        Some(mcu_type),
                                        label,
                                    );
                                }
                            });
                        // Architecture family hint
                        if let Some(t) = &self.pending_mcu_type {
                            ui.label(
                                egui::RichText::new(t.family())
                                    .color(egui::Color32::GRAY)
                                    .size(11.0),
                            );
                        }
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                egui::RichText::new(format!("{} New Project", ph::NOTE_PENCIL))
                                    .color(egui::Color32::from_rgb(220, 80, 60)),
                            )
                            .clicked()
                        {
                            // ── Apply chip change (if any) ────────────────────
                            if let Some(new_chip) = self.pending_mcu_type.take() {
                                if new_chip != self.selected_mcu_type {
                                    self.selected_mcu_type = new_chip;
                                    self.mcu = Self::init_mcu(&self.selected_mcu_type);
                                    self.generated_code = self
                                        .mcu
                                        .as_ref()
                                        .map(|m| m.fresh_main_rs())
                                        .unwrap_or_default();
                                    self.active_tab = McuTab::Pins;
                                    self.lsp_state.lock().unwrap().reset();
                                    self.lsp_selected_diagnostic = None;
                                }
                            }
                            // ── Reset project files ───────────────────────────
                            self.user_src_files.clear();
                            self.user_src_folders.clear();
                            self.selected_file = ProjectFileId::MainRs;
                            self.project_name = None;
                            self.project_dir  = None;
                            self.renaming_file = None;
                            self.renaming_folder = None;
                            self.new_src_name = None;
                            self.new_src_folder_name = None;
                            self.new_file_in_folder = None;
                            self.confirm_new_project = false;
                            // Pre-populate the pins/ scaffold so the tree shows
                            // the folder immediately, before any pin is configured.
                            Self::init_pins_scaffold(
                                &mut self.user_src_files,
                                &mut self.user_src_folders,
                            );
                            save_project_needed = true;
                        }
                        ui.add_space(8.0);
                        if ui.button("Cancel").clicked() {
                            self.confirm_new_project = false;
                            self.pending_mcu_type = None;
                        }
                    });
                    ui.add_space(4.0);
                });
        }

        // Write the entire project to the workspace directory when the file
        // tree changed (file added, deleted, or project opened/cleared).
        // This ensures Cargo.toml and all other required files are in sync.
        if save_project_needed {
            if let Some(config) = self.selected_mcu_type.project_config() {
                let workspace = std::env::temp_dir().join("embedded_ide_0_check");
                let _ = project_gen::write_project(
                    &workspace,
                    &config,
                    &self.generated_code,
                    &self.user_src_files,
                );
            }
        }

        // ── Compute editor content AFTER the project tree ─────────────────────
        // IMPORTANT: display_code must be computed AFTER the project tree panel
        // so that self.selected_file reflects any click the user just made.
        // Computing it before the tree caused a write-back bug: when the user
        // clicked a user file, self.selected_file was already updated by the
        // click handler, but display_code still held the OLD file's content.
        // The write-back then wrongly stored the old content into the new file.
        let mut display_code: String = if let ProjectFileId::UserFile(i) = self.selected_file {
            self.user_src_files
                .get(i)
                .map(|(_, c)| c.clone())
                .unwrap_or_default()
        } else if self.selected_file == ProjectFileId::MainRs {
            // Always read from self.generated_code — not from the project_files
            // snapshot built at the start of this frame.  The snapshot is stale
            // whenever load_project_from_dir() runs in the same frame (Open
            // Project), which would otherwise show the previous project's code
            // and then immediately overwrite generated_code via the write-back.
            self.generated_code.clone()
        } else {
            match &project_files {
                Some(files) => self.selected_file.content(files).to_owned(),
                None => self.generated_code.clone(),
            }
        };
        let display_syntax = self.selected_file.syntax();

        // ── Panel 2: Code Editor ──────────────────────────────────────────────
        let editor_width = ui.available_width() * 0.5;
        egui::Panel::left("code_editor")
            .resizable(true)
            .default_size(editor_width)
            .show_inside(ui, |ui| {
                // Header row
                ui.horizontal(|ui| {
                    ui.heading("Code Editor");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Copy button — copies the currently displayed file
                        let copy_ok = format!("{} Copied!", ph::CHECK);
                        let copy_def = format!("{} Copy", ph::COPY);
                        let copy_label: &str = if self.copy_flash > 0 {
                            &copy_ok
                        } else {
                            &copy_def
                        };
                        let copy_btn = ui.add(egui::Button::new(
                            egui::RichText::new(copy_label).size(11.0),
                        ));
                        if copy_btn.clicked() {
                            ui.output_mut(|o| {
                                o.commands.push(egui::output::OutputCommand::CopyText(
                                    display_code.clone(),
                                ));
                            });
                            self.copy_flash = 60;
                        }

                        ui.add_space(4.0);

                        // Export Project button
                        let can_export = project_files.is_some();
                        let export_idle = format!("{} Export Project", ph::EXPORT);
                        let export_na = format!("{} Export (N/A)", ph::EXPORT);
                        let export_label: &str = if self.export_flash > 0 {
                            &self.export_msg
                        } else if can_export {
                            &export_idle
                        } else {
                            &export_na
                        };

                        let export_color =
                            if self.export_flash > 0 && self.export_msg.starts_with('✔') {
                                egui::Color32::from_rgb(100, 220, 100)
                            } else if self.export_flash > 0 {
                                egui::Color32::from_rgb(230, 100, 80)
                            } else {
                                egui::Color32::WHITE
                            };

                        let export_btn = ui.add_enabled(
                            can_export && self.export_flash == 0,
                            egui::Button::new(
                                egui::RichText::new(export_label)
                                    .size(11.0)
                                    .color(export_color),
                            ),
                        );

                        if export_btn.clicked() {
                            if let Some(config) = self.selected_mcu_type.project_config() {
                                if let Some(dest) = rfd::FileDialog::new()
                                    .set_title("Choose folder for the exported project")
                                    .pick_folder()
                                {
                                    let code = self.generated_code.clone();
                                    match project_gen::write_project(
                                        &dest,
                                        &config,
                                        &code,
                                        &self.user_src_files,
                                    ) {
                                        Ok(()) => {
                                            let name = dest
                                                .file_name()
                                                .and_then(|n| n.to_str())
                                                .unwrap_or("exported")
                                                .to_string();
                                            self.export_msg = format!("✔  {name}");
                                            self.export_flash = 180;
                                            // Track the exported folder name so it is visible
                                            // in the panel heading and persisted across restarts
                                            // even for projects that were never opened via
                                            // "Open Project".
                                            self.project_name = Some(name);
                                        }
                                        Err(e) => {
                                            self.export_msg = format!("✗  {e}");
                                            self.export_flash = 180;
                                        }
                                    }
                                }
                            }
                        }

                        export_btn.on_hover_text(
                            "Exports a complete Cargo project:\n\
                             Cargo.toml · .cargo/config.toml · memory.x · build.rs · src/main.rs",
                        );

                        ui.add_space(4.0);

                        // ── rust-analyzer status badge ────────────────────────
                        {
                            let lsp = self.lsp_state.lock().unwrap();
                            let (icon, color, tip) = match &lsp.status {
                                LspStatus::Stopped => (
                                    ph::PLUGS,
                                    egui::Color32::DARK_GRAY,
                                    "rust-analyzer: not running",
                                ),
                                LspStatus::Starting => (
                                    ph::CIRCLE_NOTCH,
                                    egui::Color32::from_rgb(180, 180, 80),
                                    "rust-analyzer: starting…",
                                ),
                                LspStatus::Indexing => (
                                    ph::CIRCLE_NOTCH,
                                    egui::Color32::from_rgb(180, 180, 80),
                                    "rust-analyzer: indexing…",
                                ),
                                LspStatus::Ready if lsp.total_errors() > 0 => (
                                    ph::X_CIRCLE,
                                    egui::Color32::from_rgb(220, 80, 70),
                                    "rust-analyzer: errors",
                                ),
                                LspStatus::Ready if lsp.total_warnings() > 0 => (
                                    ph::WARNING,
                                    egui::Color32::from_rgb(210, 170, 40),
                                    "rust-analyzer: warnings",
                                ),
                                LspStatus::Ready => (
                                    ph::CHECK_CIRCLE,
                                    egui::Color32::from_rgb(80, 200, 100),
                                    "rust-analyzer: no errors",
                                ),
                                LspStatus::Failed(_) => (
                                    ph::X_CIRCLE,
                                    egui::Color32::from_rgb(220, 80, 70),
                                    "rust-analyzer: failed",
                                ),
                            };
                            // Spin the icon while indexing
                            let is_spinning =
                                matches!(lsp.status, LspStatus::Starting | LspStatus::Indexing);
                            let badge = format!("RA {icon}");
                            ui.add(
                                egui::Button::new(
                                    egui::RichText::new(&badge).size(11.0).color(color),
                                )
                                .frame(false),
                            )
                            .on_hover_text(tip);
                            if is_spinning {
                                ui.ctx()
                                    .request_repaint_after(std::time::Duration::from_millis(200));
                            }
                        }

                        ui.add_space(4.0);

                        // ── Build button ──────────────────────────────────────
                        let build_guard = self.build_state.lock().unwrap();
                        let is_building = build_guard.is_building();

                        // Animate trailing dots while building
                        let build_label = if is_building {
                            let dots = match (ui.ctx().cumulative_frame_nr() / 15) % 3 {
                                0 => ".",
                                1 => "..",
                                _ => "...",
                            };
                            format!("Building{dots}")
                        } else {
                            format!("{} Build", ph::HAMMER)
                        };

                        // Badge: error/warning/ok count shown to the left of the button
                        let badge_text = match &*build_guard {
                            BuildState::Done(r) if r.error_count() > 0 => Some((
                                format!("{} {}", r.error_count(), ph::X_CIRCLE),
                                egui::Color32::from_rgb(230, 90, 80),
                            )),
                            BuildState::Done(r) if r.warning_count() > 0 => Some((
                                format!("{} {}", r.warning_count(), ph::WARNING),
                                egui::Color32::from_rgb(230, 190, 50),
                            )),
                            BuildState::Done(r) if r.success => Some((
                                format!("{}", ph::CHECK_CIRCLE),
                                egui::Color32::from_rgb(80, 200, 100),
                            )),
                            BuildState::Failed(_) => Some((
                                format!("{}", ph::X_CIRCLE),
                                egui::Color32::from_rgb(230, 90, 80),
                            )),
                            _ => None,
                        };
                        drop(build_guard);

                        if let Some((badge, color)) = badge_text {
                            ui.label(egui::RichText::new(badge).size(11.0).color(color));
                        }

                        let build_enabled = !is_building && project_files.is_some();
                        let build_btn = ui.add_enabled(
                            build_enabled,
                            egui::Button::new(egui::RichText::new(&build_label).size(11.0).color(
                                if build_enabled {
                                    egui::Color32::from_rgb(100, 220, 100)
                                } else {
                                    egui::Color32::GRAY
                                },
                            )),
                        );

                        if build_btn.clicked() {
                            if let Some(config) = self.selected_mcu_type.project_config() {
                                let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
                                let code = self.generated_code.clone();
                                match project_gen::write_project(
                                    &build_dir,
                                    &config,
                                    &code,
                                    &self.user_src_files,
                                ) {
                                    Ok(()) => {
                                        self.selected_diagnostic = None;
                                        self.build_tab = BuildPanelTab::Cargo;
                                        build::start_build(
                                            build_dir,
                                            config.target.to_string(),
                                            Arc::clone(&self.build_state),
                                            self.egui_ctx.clone(),
                                        );
                                    }
                                    Err(e) => {
                                        *self.build_state.lock().unwrap() = BuildState::Failed(
                                            format!("Could not write project to temp dir: {e}"),
                                        );
                                    }
                                }
                            }
                        }
                        build_btn.on_hover_text(
                            "Run `cargo check` on the generated project.\n\
                             Requires the Rust toolchain + thumbv7m-none-eabi target:\n\
                             rustup target add thumbv7m-none-eabi",
                        );

                        // Keep UI refreshing while build is running (drives dot animation)
                        if is_building {
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(120));
                        }

                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);

                        // ── USB DFU + SWD section ─────────────────────────────
                        let dfu_guard = self.dfu_state.lock().unwrap();
                        let dfu_busy = dfu_guard.is_busy();
                        let dfu_label = dfu_guard.status_label().to_string();
                        let dfu_color = dfu_guard.status_color();
                        let dfu_detail = dfu_guard.detail();
                        let device_ok = matches!(*dfu_guard, DfuState::DeviceFound(_));
                        drop(dfu_guard);

                        let ocd_busy = self.openocd_state.lock().unwrap().is_busy();
                        let esp_busy = self.espflash_state.lock().unwrap().is_busy();
                        let any_busy = dfu_busy || ocd_busy || esp_busy;

                        // Determine toolchain of the selected chip
                        let chip_toolchain = self.selected_mcu_type.toolchain();

                        // Determine if the selected programmer supports SWD flashing
                        let (is_swd_capable, sel_interface_cfg) = {
                            let progs = self.dfu_programmers.lock().unwrap();
                            let kind = progs
                                .get(self.dfu_sel_programmer)
                                .map(|p| p.kind)
                                .unwrap_or("");
                            let swd = matches!(kind, "ST-Link" | "J-Link" | "CMSIS-DAP");
                            let cfg = openocd::interface_cfg_for_kind(kind).to_string();
                            (swd, cfg)
                        };

                        // Keep UI refreshing while any flash operation is running
                        if any_busy {
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(120));
                        }

                        // 🔍 Scan button — always visible (detects DFU, ST-Link, and serial)
                        let scan_btn = ui.add_enabled(
                            !dfu_busy,
                            egui::Button::new(
                                egui::RichText::new(format!("{} Scan USB", ph::MAGNIFYING_GLASS))
                                    .size(11.0),
                            ),
                        );
                        if scan_btn.clicked() {
                            self.build_tab = BuildPanelTab::Dfu;
                            self.dfu_sel_programmer = 0;
                            dfu::detect_dfu(
                                Arc::clone(&self.dfu_state),
                                Arc::clone(&self.dfu_log),
                                Arc::clone(&self.dfu_programmers),
                                self.egui_ctx.clone(),
                            );
                        }
                        scan_btn.on_hover_text(
                            "Scan for connected USB programmers:\n\
                             • DFU bootloader (STM32 with BOOT0 = 1)\n\
                             • ST-Link / J-Link / CMSIS-DAP\n\
                             • USB-Serial (ESP32-C3, …)",
                        );

                        ui.add_space(2.0);

                        // ── Toolchain-specific flash buttons ──────────────────
                        match chip_toolchain {
                            ToolchainKind::RustEmbedded => {
                                // ⚡ Flash via USB (DFU)
                                let flash_enabled =
                                    device_ok && !any_busy && project_files.is_some();
                                let flash_btn = ui.add_enabled(
                                    flash_enabled,
                                    egui::Button::new(
                                        egui::RichText::new(format!("{} Flash USB", ph::LIGHTNING))
                                            .size(11.0)
                                            .color(if flash_enabled {
                                                egui::Color32::from_rgb(100, 200, 255)
                                            } else {
                                                egui::Color32::GRAY
                                            }),
                                    ),
                                );
                                if flash_btn.clicked() {
                                    if let Some(config) = self.selected_mcu_type.project_config() {
                                        let build_dir =
                                            std::env::temp_dir().join("embedded_ide_0_check");
                                        let code = self.generated_code.clone();
                                        if project_gen::write_project(
                                            &build_dir,
                                            &config,
                                            &code,
                                            &self.user_src_files,
                                        )
                                        .is_ok()
                                        {
                                            self.build_tab = BuildPanelTab::Dfu;
                                            dfu::start_flash(
                                                build_dir,
                                                config.target.to_string(),
                                                config.pkg_name.to_string(),
                                                self.dfu_flash_addr.clone(),
                                                Arc::clone(&self.dfu_state),
                                                Arc::clone(&self.dfu_log),
                                                self.egui_ctx.clone(),
                                            );
                                        }
                                    }
                                }
                                flash_btn.on_hover_text(
                                    "Build with --release, convert to .bin, flash via dfu-util.\n\
                                     Requires:\n\
                                     • STM32 in DFU mode (BOOT0 = 1)\n\
                                     • dfu-util in PATH\n\
                                     • WinUSB driver (install via Zadig)\n\
                                     • llvm-objcopy or arm-none-eabi-objcopy",
                                );

                                ui.add_space(2.0);

                                // 🔗 Flash via SWD (OpenOCD)
                                let flash_swd_enabled =
                                    is_swd_capable && !any_busy && project_files.is_some();
                                let flash_swd_btn = ui.add_enabled(
                                    flash_swd_enabled,
                                    egui::Button::new(
                                        egui::RichText::new(format!("{} Flash SWD", ph::LIGHTNING))
                                            .size(11.0)
                                            .color(if flash_swd_enabled {
                                                egui::Color32::from_rgb(255, 165, 50)
                                            } else {
                                                egui::Color32::GRAY
                                            }),
                                    ),
                                );
                                if flash_swd_btn.clicked() {
                                    if let Some(config) = self.selected_mcu_type.project_config() {
                                        let build_dir =
                                            std::env::temp_dir().join("embedded_ide_0_check");
                                        let code = self.generated_code.clone();
                                        if project_gen::write_project(
                                            &build_dir,
                                            &config,
                                            &code,
                                            &self.user_src_files,
                                        )
                                        .is_ok()
                                        {
                                            self.build_tab = BuildPanelTab::Dfu;
                                            openocd::start_flash(
                                                build_dir,
                                                config.target.to_string(),
                                                config.pkg_name.to_string(),
                                                sel_interface_cfg.clone(),
                                                self.openocd_target_cfg.clone(),
                                                Arc::clone(&self.openocd_state),
                                                Arc::clone(&self.dfu_log),
                                                self.egui_ctx.clone(),
                                            );
                                        }
                                    }
                                }
                                flash_swd_btn.on_hover_text(
                                    "Build with --release, then program via SWD using OpenOCD.\n\
                                     Requires:\n\
                                     • OpenOCD in PATH  (winget install openocd)\n\
                                     • ST-Link/J-Link/CMSIS-DAP driver installed\n\
                                     • Target .cfg selected in the Flash tab\n\
                                     • SWD wiring: SWDIO + SWCLK + GND",
                                );
                            }

                            ToolchainKind::EspRust => {
                                // 🔶 Flash ESP32
                                let flash_esp_enabled = !any_busy && project_files.is_some();
                                let flash_esp_btn = ui.add_enabled(
                                    flash_esp_enabled,
                                    egui::Button::new(
                                        egui::RichText::new(format!(
                                            "{} Flash ESP32",
                                            ph::LIGHTNING
                                        ))
                                        .size(11.0)
                                        .color(
                                            if flash_esp_enabled {
                                                egui::Color32::from_rgb(220, 140, 60)
                                            } else {
                                                egui::Color32::GRAY
                                            },
                                        ),
                                    ),
                                );
                                if flash_esp_btn.clicked() {
                                    if let Some(config) = self.selected_mcu_type.project_config() {
                                        let build_dir =
                                            std::env::temp_dir().join("embedded_ide_0_check");
                                        let code = self.generated_code.clone();
                                        if project_gen::write_project(
                                            &build_dir,
                                            &config,
                                            &code,
                                            &self.user_src_files,
                                        )
                                        .is_ok()
                                        {
                                            self.build_tab = BuildPanelTab::Dfu;
                                            espflash::start_flash(
                                                build_dir,
                                                config.target.to_string(),
                                                config.probe_chip.to_string(),
                                                Arc::clone(&self.espflash_state),
                                                Arc::clone(&self.dfu_log),
                                                self.egui_ctx.clone(),
                                            );
                                        }
                                    }
                                }
                                flash_esp_btn.on_hover_text(
                                    "Build with --release, then flash via espflash.\n\
                                     Requires:\n\
                                     • espflash in PATH  (cargo install espflash)\n\
                                     • ESP32-C3 connected via USB\n\
                                     • ESP32-C3 in download mode:\n\
                                         hold BOOT → press RESET → release BOOT\n\
                                     • Target installed:\n\
                                         rustup target add riscv32imc-unknown-none-elf",
                                );
                            }

                            ToolchainKind::SdccC => {
                                // STM8 — on hold, no flash button
                            }
                        }

                        ui.add_space(4.0);

                        // Status label — shows the most active state
                        let (show_label, show_color, show_detail) = {
                            let ocd = self.openocd_state.lock().unwrap();
                            let esp = self.espflash_state.lock().unwrap();
                            if !matches!(*ocd, OpenOcdState::Idle) {
                                (ocd.status_label().to_string(), ocd.status_color(), None)
                            } else if !matches!(*esp, EspFlashState::Idle) {
                                (esp.status_label().to_string(), esp.status_color(), None)
                            } else {
                                (dfu_label.clone(), dfu_color, dfu_detail)
                            }
                        };
                        let status_widget = ui.label(
                            egui::RichText::new(&show_label)
                                .size(10.5)
                                .color(show_color),
                        );
                        if let Some(detail) = show_detail {
                            status_widget.on_hover_text(detail);
                        }

                        ui.add_space(8.0);
                        // Show which file is open
                        let open_label = match self.selected_file {
                            ProjectFileId::UserFile(i) => self
                                .user_src_files
                                .get(i)
                                .map(|(name, _)| format!("src/{name}"))
                                .unwrap_or_else(|| "src/???".to_string()),
                            other => other.label().to_string(),
                        };
                        ui.label(
                            egui::RichText::new(&open_label)
                                .size(10.0)
                                .color(egui::Color32::from_rgb(120, 160, 200)),
                        );
                    });
                });

                ui.separator();

                // ── Diagnostics panel (bottom, manually resizable) ────────────
                {
                    let cargo_has = !matches!(*self.build_state.lock().unwrap(), BuildState::Idle);
                    let lsp_active = self.lsp_state.lock().unwrap().status.is_active();
                    let dfu_active = !matches!(*self.dfu_state.lock().unwrap(), DfuState::Idle)
                        || !matches!(*self.openocd_state.lock().unwrap(), OpenOcdState::Idle)
                        || !matches!(*self.espflash_state.lock().unwrap(), EspFlashState::Idle)
                        || !self.dfu_log.lock().unwrap().is_empty();
                    let show_panel = cargo_has || lsp_active || dfu_active;

                    if show_panel {
                        const HANDLE_H: f32 = 6.0;
                        const MIN_H: f32 = 56.0;

                        // Keep height in valid range for current window size.
                        let max_h = (ui.available_height() - 60.0).max(MIN_H);
                        self.diag_panel_height = self.diag_panel_height.clamp(MIN_H, max_h);

                        // TopBottomPanel::bottom takes space from the bottom
                        // of the remaining area before the editor is laid out.
                        // exact_height gives us full control — no egui-internal
                        // default_height that would reset on show/hide.
                        egui::TopBottomPanel::bottom("diag_panel")
                            .exact_height(self.diag_panel_height + HANDLE_H)
                            .show_inside(ui, |ui| {
                                // ── Drag handle (top edge of panel) ───────
                                let (handle_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), HANDLE_H),
                                    egui::Sense::hover(),
                                );
                                let drag_resp = ui.interact(
                                    handle_rect,
                                    egui::Id::new("diag_panel_resize"),
                                    egui::Sense::drag(),
                                );

                                let mid_y = handle_rect.center().y;
                                let line_color = if drag_resp.hovered() || drag_resp.dragged() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                                    egui::Color32::from_rgb(100, 140, 200)
                                } else {
                                    egui::Color32::from_gray(65)
                                };

                                // Line + three grip dots
                                ui.painter().hline(
                                    handle_rect.x_range(),
                                    mid_y,
                                    egui::Stroke::new(1.5, line_color),
                                );
                                for dx in [-6.0_f32, 0.0, 6.0] {
                                    ui.painter().circle_filled(
                                        egui::pos2(handle_rect.center().x + dx, mid_y),
                                        1.5,
                                        line_color,
                                    );
                                }

                                if drag_resp.dragged() {
                                    // Dragging up → negative delta.y → panel grows
                                    self.diag_panel_height = (self.diag_panel_height
                                        - drag_resp.drag_delta().y)
                                        .clamp(MIN_H, max_h);
                                }

                                // ── Content ────────────────────────────────
                                show_diag_panel(
                                    ui,
                                    &self.egui_ctx,
                                    &self.build_state,
                                    &self.lsp_state,
                                    &self.dfu_state,
                                    &self.dfu_log,
                                    &self.dfu_programmers,
                                    &mut self.dfu_sel_programmer,
                                    &mut self.dfu_flash_addr,
                                    &self.openocd_state,
                                    &mut self.openocd_target_cfg,
                                    &self.espflash_state,
                                    &self.tools_state,
                                    &self.selected_mcu_type.toolchain(),
                                    &mut self.build_tab,
                                    &mut self.selected_diagnostic,
                                    &mut self.lsp_selected_diagnostic,
                                    &mut self.selected_file,
                                );
                            });
                    }
                }

                // Use a unique id per file so egui's TextEditState (galley,
                // cursor, undo stack) is never shared between files.
                // A fixed id caused the editor to keep the previous file's
                // rendered galley when switching to a new file.
                let editor_id: String = match &self.selected_file {
                    ProjectFileId::UserFile(i) => {
                        let path = self
                            .user_src_files
                            .get(*i)
                            .map(|(p, _)| p.as_str())
                            .unwrap_or("?");
                        format!("code_editor:user:{path}")
                    }
                    ProjectFileId::MainRs => "code_editor:main_rs".into(),
                    ProjectFileId::CargoToml => "code_editor:cargo_toml".into(),
                    ProjectFileId::CargoConfig => "code_editor:cargo_config".into(),
                    ProjectFileId::MemoryX => "code_editor:memory_x".into(),
                    ProjectFileId::BuildRs => "code_editor:build_rs".into(),
                    ProjectFileId::GitIgnore => "code_editor:gitignore".into(),
                };

                // ── LSP completion: pre-editor key consumption ───────────────
                // Consume navigation / accept keys BEFORE show_with_completer
                // so the built-in Completer never sees them when our popup is open.
                //
                // Mouse clicks on popup items set `completion_pending_insert` last
                // frame; apply them here so the same accept path is used for both
                // keyboard and mouse.
                let mut lsp_accepted: Option<String> =
                    self.completion_pending_insert.take();
                if lsp_accepted.is_some() {
                    self.completion_open = false;
                }

                if self.completion_open {
                    let has_items = !self.completion_filtered_items.is_empty();
                    if has_items {
                        let count = self.completion_filtered_items.len();
                        ui.input_mut(|inp| {
                            if inp.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                                self.completion_open = false;
                            } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                                // Clamp against the FILTERED count so selection never
                                // goes out of the visible list.
                                self.completion_sel =
                                    (self.completion_sel + 1).min(count.saturating_sub(1));
                            } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                                self.completion_sel = self.completion_sel.saturating_sub(1);
                            } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                                   || inp.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                            {
                                // Use the filtered list — guaranteed same items as shown.
                                let sel = self.completion_sel.min(count.saturating_sub(1));
                                if let Some(item) = self.completion_filtered_items.get(sel) {
                                    lsp_accepted = Some(item.insert_text.clone());
                                }
                                self.completion_open = false;
                            }
                        });
                    }
                }
                // Detect Ctrl+Space BEFORE the editor so egui doesn't pass it
                // to the TextEdit as a literal character.
                let ctrl_space_pressed = ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::CTRL, egui::Key::Space)
                });

                let editor_resp = CodeEditor::default()
                    .id_source(editor_id)
                    .with_rows(50)
                    .with_fontsize(13.0)
                    .with_theme(ColorTheme::GRUVBOX)
                    .with_numlines(true)
                    .show_with_completer(
                        ui,
                        &mut display_code,
                        &display_syntax,
                        &mut self.completer,
                    );

                // ── Write user edits back ────────────────────────────────────
                // display_code is a local clone; persist changes here.
                if let ProjectFileId::UserFile(i) = self.selected_file {
                    if let Some(entry) = self.user_src_files.get_mut(i) {
                        if display_code != entry.1 {
                            entry.1 = display_code.clone();
                            // Auto-save to workspace so LSP and build see the change
                            let workspace = std::env::temp_dir().join("embedded_ide_0_check");
                            let dest = workspace.join("src").join(&entry.0);
                            if let Some(parent) = dest.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::write(&dest, entry.1.as_bytes());
                        }
                    }
                } else if self.selected_file == ProjectFileId::MainRs
                    && display_code != self.generated_code
                {
                    self.generated_code = display_code.clone();
                }

                // ── LSP completion: post-editor apply + trigger + popup ───────
                let cursor_char_idx = editor_resp.state.cursor
                    .char_range()
                    .map(|r| r.primary.index);

                // Apply accepted completion: replace [word_start..cursor] with insert_text
                if let Some(insert_text) = lsp_accepted {
                    if let Some(cur_idx) = cursor_char_idx {
                        let word_start = lsp_word_start(&display_code, cur_idx);
                        let chars: Vec<char> = display_code.chars().collect();
                        let before: String = chars[..word_start].iter().collect();
                        let after:  String = chars[cur_idx..].iter().collect();
                        display_code = format!("{}{}{}", before, insert_text, after);
                        // Persist the change so the write-back below picks it up
                        // (the write-back already happened above; redo it for this file)
                        if let ProjectFileId::UserFile(i) = self.selected_file {
                            if let Some(entry) = self.user_src_files.get_mut(i) {
                                entry.1 = display_code.clone();
                                let workspace = std::env::temp_dir().join("embedded_ide_0_check");
                                let dest = workspace.join("src").join(&entry.0);
                                let _ = std::fs::write(&dest, entry.1.as_bytes());
                            }
                        } else if self.selected_file == ProjectFileId::MainRs {
                            self.generated_code = display_code.clone();
                        }
                    }
                }

                // Trigger detection
                // Completions are only meaningful for files that RA has open.
                // Currently RA only tracks src/main.rs; skip for other files.
                let lsp_file_tracked = matches!(
                    self.selected_file,
                    ProjectFileId::MainRs
                );
                {
                    let lsp_ready = lsp_file_tracked && matches!(
                        self.lsp_state.lock().unwrap().status,
                        lsp::LspStatus::Ready
                    );
                    if lsp_ready {
                        // Manual Ctrl+Space
                        if ctrl_space_pressed {
                            if let Some(idx) = cursor_char_idx {
                                let (line, col) = lsp_cursor_pos(&display_code, idx);
                                // Sync the latest editor text to RA BEFORE the
                                // completion request — the frame's did_change (sent
                                // at the top of update()) used last frame's code.
                                {
                                    let mut lsp = self.lsp_state.lock().unwrap();
                                    lsp.did_change(&display_code);
                                    lsp.request_completion(line, col, None);
                                }
                                self.completion_trigger_idx = idx;
                                self.completion_sel          = 0;
                                self.completion_open         = true;
                            }
                        }

                        // Auto-trigger on `.`  (method / field access)
                        let dot_trigger = editor_resp.response.changed()
                            && cursor_char_idx.map(|idx| {
                                let chars: Vec<char> = display_code.chars().collect();
                                idx > 0 && chars.get(idx - 1) == Some(&'.')
                            }).unwrap_or(false);
                        if dot_trigger && !ctrl_space_pressed {
                            if let Some(idx) = cursor_char_idx {
                                let (line, col) = lsp_cursor_pos(&display_code, idx);
                                {
                                    let mut lsp = self.lsp_state.lock().unwrap();
                                    lsp.did_change(&display_code);
                                    lsp.request_completion(line, col, Some('.'));
                                }
                                self.completion_trigger_idx = idx;
                                self.completion_sel          = 0;
                                self.completion_open         = true;
                            }
                        }

                        // Auto-trigger on `::` (Rust path separator — module / type paths)
                        // Fires when the user types the SECOND colon of `::`.
                        // Sending triggerCharacter=':' tells rust-analyzer this is a
                        // path-completion context so it returns module members, not
                        // arbitrary identifier completions.
                        let colon_trigger = !dot_trigger
                            && !ctrl_space_pressed
                            && editor_resp.response.changed()
                            && cursor_char_idx.map(|idx| {
                                let chars: Vec<char> = display_code.chars().collect();
                                idx >= 2
                                    && chars.get(idx - 1) == Some(&':')
                                    && chars.get(idx - 2) == Some(&':')
                            }).unwrap_or(false);
                        if colon_trigger {
                            if let Some(idx) = cursor_char_idx {
                                let (line, col) = lsp_cursor_pos(&display_code, idx);
                                {
                                    let mut lsp = self.lsp_state.lock().unwrap();
                                    lsp.did_change(&display_code);
                                    lsp.request_completion(line, col, Some(':'));
                                }
                                self.completion_trigger_idx = idx;
                                self.completion_sel          = 0;
                                self.completion_open         = true;
                            }
                        }
                    }

                    // Close popup if cursor moved back past the trigger point,
                    // or too far ahead (user navigated away from the trigger word).
                    if self.completion_open {
                        if let Some(idx) = cursor_char_idx {
                            let cursor  = idx as isize;
                            let trigger = self.completion_trigger_idx as isize;
                            let delta   = cursor - trigger;
                            // delta < 0  → user deleted back past trigger point
                            // delta > 80 → user moved far forward (switched context)
                            if delta < 0 || delta > 80 {
                                self.completion_open = false;
                            }
                        }
                    }
                }

                // ── LSP completion popup ───────────────────────────────────────
                if self.completion_open {
                    let all_items = self.lsp_state.lock().unwrap().completion_items.clone();

                    if !all_items.is_empty() {
                        // ── Live prefix filtering ────────────────────────────────
                        // Compute what the user has typed since the trigger point.
                        let prefix = cursor_char_idx
                            .map(|cur| lsp_completion_prefix(
                                &display_code,
                                self.completion_trigger_idx,
                                cur,
                            ))
                            .unwrap_or_default();

                        let filtered: Vec<lsp::CompletionItem> = if prefix.is_empty() {
                            all_items
                        } else {
                            let pl = prefix.to_lowercase();
                            all_items
                                .into_iter()
                                .filter(|it| it.label.to_lowercase().starts_with(&pl))
                                .collect()
                        };

                        // Persist filtered list so next frame's key handlers see
                        // exactly the same items the user sees right now.
                        self.completion_filtered_items = filtered.clone();

                        if filtered.is_empty() {
                            // Nothing matches the current prefix — hide the popup.
                            self.completion_open = false;
                        } else {
                            // Clamp selection into the visible filtered range.
                            self.completion_sel =
                                self.completion_sel.min(filtered.len() - 1);
                            let sel = self.completion_sel;

                            // ── Popup screen position ────────────────────────────
                            let popup_pos = if let Some(char_range) =
                                editor_resp.state.cursor.char_range()
                            {
                                let cursor_idx = char_range.primary.index;
                                let text_char_count =
                                    editor_resp.galley.job.text.chars().count();
                                let clamped = cursor_idx
                                    .min(text_char_count.saturating_sub(1));
                                let cursor_local = editor_resp.galley
                                    .pos_from_cursor(egui::text::CCursor::new(clamped));
                                let offset =
                                    egui::vec2(0.0, cursor_local.height() + 2.0);
                                editor_resp.response.rect.left_top()
                                    + cursor_local.min.to_vec2()
                                    + offset
                            } else {
                                editor_resp.response.rect.left_top()
                            };

                            // ── Render popup ─────────────────────────────────────
                            // `interactable` defaults to true → mouse clicks work.
                            egui::Area::new(egui::Id::new("lsp_completion_popup"))
                                .fixed_pos(popup_pos)
                                .order(egui::Order::Foreground)
                                .show(ui.ctx(), |ui| {
                                    egui::Frame::popup(&ui.ctx().global_style())
                                        .show(ui, |ui| {
                                        ui.style_mut().wrap_mode =
                                            Some(egui::TextWrapMode::Extend);
                                        ui.set_min_width(440.0);
                                        ui.set_max_width(440.0);

                                        egui::ScrollArea::vertical()
                                            .max_height(300.0)
                                            .auto_shrink([false, true])
                                            .show(ui, |ui| {
                                            for (i, item) in filtered.iter().enumerate() {
                                                let selected = i == sel;

                                                let fg = if selected {
                                                    egui::Color32::WHITE
                                                } else {
                                                    egui::Color32::from_rgb(200, 210, 230)
                                                };
                                                let sel_bg =
                                                    egui::Color32::from_rgb(40, 90, 160);
                                                let hover_bg =
                                                    egui::Color32::from_rgb(50, 60, 80);
                                                let detail_fg = if selected {
                                                    egui::Color32::from_rgb(160, 195, 255)
                                                } else {
                                                    egui::Color32::from_rgb(110, 130, 155)
                                                };

                                                // Allocate the full row width for hit-testing.
                                                let row_h = 19.0;
                                                let avail_w = ui.available_width();
                                                let (rect, row_resp) =
                                                    ui.allocate_exact_size(
                                                        egui::vec2(avail_w, row_h),
                                                        egui::Sense::click(),
                                                    );

                                                // Background (selected / hovered).
                                                if selected {
                                                    ui.painter().rect_filled(
                                                        rect, 2.0, sel_bg,
                                                    );
                                                } else if row_resp.hovered() {
                                                    ui.painter().rect_filled(
                                                        rect, 2.0, hover_bg,
                                                    );
                                                }

                                                let painter = ui.painter();
                                                let icon    = lsp_kind_icon(item.kind);
                                                let label   = format!("{} {}", icon, item.label);

                                                // Icon + label — left-aligned.
                                                painter.text(
                                                    rect.left_center()
                                                        + egui::vec2(4.0, 0.0),
                                                    egui::Align2::LEFT_CENTER,
                                                    &label,
                                                    egui::FontId::monospace(12.0),
                                                    fg,
                                                );

                                                // Detail (type signature) — right-aligned,
                                                // smaller and dimmer, truncated if needed.
                                                if !item.detail.is_empty() {
                                                    let det = {
                                                        let chars: Vec<char> =
                                                            item.detail.chars().collect();
                                                        if chars.len() > 38 {
                                                            format!("{}…",
                                                                chars[..35].iter().collect::<String>())
                                                        } else {
                                                            item.detail.clone()
                                                        }
                                                    };
                                                    painter.text(
                                                        rect.right_center()
                                                            - egui::vec2(4.0, 0.0),
                                                        egui::Align2::RIGHT_CENTER,
                                                        det,
                                                        egui::FontId::monospace(10.5),
                                                        detail_fg,
                                                    );
                                                }

                                                // Mouse click → deferred insert.
                                                if row_resp.clicked() {
                                                    self.completion_pending_insert =
                                                        Some(item.insert_text.clone());
                                                    self.completion_open = false;
                                                }

                                                // Scroll selected item into view.
                                                if selected {
                                                    row_resp.scroll_to_me(None);
                                                }

                                                // Hover tooltip: documentation first,
                                                // then full detail as fallback.
                                                if !item.documentation.is_empty() {
                                                    row_resp.on_hover_text(
                                                        egui::RichText::new(&item.documentation)
                                                            .size(11.5),
                                                    );
                                                } else if !item.detail.is_empty() {
                                                    row_resp.on_hover_text(
                                                        egui::RichText::new(&item.detail)
                                                            .monospace()
                                                            .size(11.0),
                                                    );
                                                }
                                            }
                                        }); // ScrollArea
                                    }); // Frame
                                }); // Area
                        }
                    }
                    // all_items is empty: either RA hasn't responded yet, or
                    // it responded with no completions / an error.
                    else if lsp_file_tracked {
                        let (resp_received, timed_out) = {
                            let lsp = self.lsp_state.lock().unwrap();
                            let received = lsp.completion_response_received;
                            let timeout  = lsp.completion_request_sent_at
                                .map(|t| t.elapsed().as_secs() > 6)
                                .unwrap_or(false);
                            (received, timeout)
                        };

                        if resp_received || timed_out {
                            // RA answered (empty) or request is stale — close popup.
                            self.completion_open = false;
                        } else {
                            // Still waiting — show a small spinner popup.
                            let popup_pos = cursor_char_idx.and_then(|_| {
                                editor_resp.state.cursor.char_range().map(|cr| {
                                    let clamped = cr.primary.index.min(
                                        editor_resp.galley.job.text.chars().count()
                                            .saturating_sub(1),
                                    );
                                    let local = editor_resp.galley
                                        .pos_from_cursor(egui::text::CCursor::new(clamped));
                                    editor_resp.response.rect.left_top()
                                        + local.min.to_vec2()
                                        + egui::vec2(0.0, local.height() + 2.0)
                                })
                            });
                            if let Some(pos) = popup_pos {
                                egui::Area::new(egui::Id::new("lsp_completion_loading"))
                                    .fixed_pos(pos)
                                    .order(egui::Order::Foreground)
                                    .show(ui.ctx(), |ui| {
                                        egui::Frame::popup(&ui.ctx().global_style())
                                            .show(ui, |ui| {
                                            ui.add_space(2.0);
                                            ui.horizontal(|ui| {
                                                ui.spinner();
                                                ui.label(
                                                    egui::RichText::new("  rust-analyzer…")
                                                        .size(11.5)
                                                        .color(
                                                            egui::Color32::from_rgb(160, 175, 200),
                                                        ),
                                                );
                                            });
                                            ui.add_space(2.0);
                                        });
                                    });
                                ui.ctx().request_repaint();
                            }
                        }
                    }
                }

                // ── RA error lane (right-edge overlay on editor rect) ─────────
                // Proportional tick marks showing where errors/warnings are in
                // the file — useful even when the lines are scrolled out of view.
                if self.selected_file == ProjectFileId::MainRs {
                    let lsp = self.lsp_state.lock().unwrap();
                    if let Some(diags) = lsp.diagnostics.get("src/main.rs") {
                        if !diags.is_empty() {
                            let total = display_code.lines().count().max(1) as f32;
                            let rect = editor_resp.galley.rect;
                            let lane = egui::Rect::from_min_max(
                                egui::pos2(rect.right() - 6.0, rect.top()),
                                egui::pos2(rect.right(), rect.bottom()),
                            );
                            ui.painter().rect_filled(
                                lane,
                                0.0,
                                egui::Color32::from_black_alpha(50),
                            );
                            for d in diags {
                                let t = (d.line.saturating_sub(1) as f32) / total;
                                let y = rect.top() + t * rect.height();
                                let color = match d.severity {
                                    lsp::DiagSeverity::Error => {
                                        egui::Color32::from_rgb(210, 60, 50)
                                    }
                                    lsp::DiagSeverity::Warning => {
                                        egui::Color32::from_rgb(190, 155, 30)
                                    }
                                    _ => egui::Color32::from_rgb(80, 130, 200),
                                };
                                ui.painter().rect_filled(
                                    egui::Rect::from_min_max(
                                        egui::pos2(lane.left(), y - 2.0),
                                        egui::pos2(lane.right(), y + 2.0),
                                    ),
                                    0.0,
                                    color,
                                );
                            }
                        }
                    }
                }

                // ── Inline diagnostic underlines + hover tooltips ──────────────
                // Draws wavy underlines directly on the text at the error/warning
                // spans reported by rust-analyzer, and shows a tooltip on hover.
                //
                // Coordinate system:
                //   galley.pos_from_cursor(CCursor(n))  →  Rect in galley-local space
                //   response.rect.left_top()            →  screen position of local (0,0)
                //   screen_pos = left_top + local.to_vec2()
                //
                // Scroll is handled automatically: pos_from_cursor returns positions
                // relative to the galley start; if those are outside the editor's
                // clip rect (scrolled out of view) the painter ignores them.
                if self.selected_file == ProjectFileId::MainRs {
                    let diags: Vec<lsp::LspDiagnostic> = {
                        let lsp = self.lsp_state.lock().unwrap();
                        lsp.diagnostics
                            .get("src/main.rs")
                            .cloned()
                            .unwrap_or_default()
                    };

                    if !diags.is_empty() {
                        // Painter clipped to the editor rect so underlines never
                        // bleed into the line-number gutter or the scroll bar.
                        let editor_rect = editor_resp.response.rect;
                        let painter     = ui.painter().with_clip_rect(editor_rect);
                        let origin      = editor_rect.left_top();
                        let total_chars = display_code.chars().count();

                        for (di, diag) in diags.iter().enumerate() {
                            // ── Convert LSP (line, col) → galley char index ───────
                            let start_ci = lsp_pos_to_char_idx(
                                &display_code, diag.line, diag.col,
                            ).min(total_chars);
                            let end_ci_raw = lsp_pos_to_char_idx(
                                &display_code, diag.end_line, diag.end_col,
                            ).min(total_chars);
                            // Guarantee a minimum visible span of 1 char.
                            let end_ci = if end_ci_raw <= start_ci {
                                (start_ci + 1).min(total_chars)
                            } else {
                                end_ci_raw
                            };

                            // ── Galley positions (galley-local Rect) ──────────────
                            let loc_s = editor_resp.galley
                                .pos_from_cursor(egui::text::CCursor::new(start_ci));
                            let loc_e = editor_resp.galley
                                .pos_from_cursor(egui::text::CCursor::new(end_ci));

                            // Screen-space coordinates
                            let sx = origin.x + loc_s.min.x;
                            let sy_top = origin.y + loc_s.min.y;
                            let sy_bot = origin.y + loc_s.max.y; // bottom of text line
                            let line_h = loc_s.height();

                            // For multi-line spans draw only the first line;
                            // end_x is either the real end (same line) or the
                            // right edge of the start line.
                            let same_line =
                                (loc_s.min.y - loc_e.min.y).abs() < line_h * 0.5;
                            let ex = if same_line {
                                origin.x + loc_e.min.x
                            } else {
                                // Extend underline to EOL
                                origin.x + editor_resp.galley.rect.width() - 8.0
                            };

                            // Skip if horizontally degenerate
                            if ex <= sx + 0.5 {
                                continue;
                            }

                            // ── Colors per severity ───────────────────────────────
                            let (ul_color, bg_color) = match diag.severity {
                                lsp::DiagSeverity::Error => (
                                    egui::Color32::from_rgb(220, 65, 55),
                                    egui::Color32::from_rgba_unmultiplied(210, 55, 45, 20),
                                ),
                                lsp::DiagSeverity::Warning => (
                                    egui::Color32::from_rgb(210, 165, 35),
                                    egui::Color32::from_rgba_unmultiplied(200, 160, 30, 14),
                                ),
                                lsp::DiagSeverity::Info => (
                                    egui::Color32::from_rgb(80, 140, 215),
                                    egui::Color32::TRANSPARENT,
                                ),
                                lsp::DiagSeverity::Hint => (
                                    egui::Color32::from_rgb(100, 170, 120),
                                    egui::Color32::TRANSPARENT,
                                ),
                            };

                            // ── Background highlight ──────────────────────────────
                            if bg_color != egui::Color32::TRANSPARENT {
                                painter.rect_filled(
                                    egui::Rect::from_min_max(
                                        egui::pos2(sx,  sy_top),
                                        egui::pos2(ex,  sy_bot),
                                    ),
                                    0.0,
                                    bg_color,
                                );
                            }

                            // ── Wavy underline ────────────────────────────────────
                            draw_wavy_underline(&painter, sx, ex, sy_bot, ul_color);

                            // ── Hover tooltip ─────────────────────────────────────
                            // Use ui.interact() — does not allocate layout space.
                            let hover_rect = egui::Rect::from_min_max(
                                egui::pos2(sx, sy_top),
                                egui::pos2(ex, sy_bot + 3.0),
                            );
                            let hover = ui.interact(
                                hover_rect,
                                egui::Id::new("inline_diag").with(di),
                                egui::Sense::hover(),
                            );
                            if hover.hovered() {
                                let icon = match diag.severity {
                                    lsp::DiagSeverity::Error   => "⛔",
                                    lsp::DiagSeverity::Warning => "⚠",
                                    lsp::DiagSeverity::Info    => "ℹ",
                                    lsp::DiagSeverity::Hint    => "·",
                                };
                                let msg   = format!("{icon}  {}", diag.message);
                                let code  = diag.code.clone();
                                #[allow(deprecated)]
                                egui::show_tooltip_at_pointer(
                                    ui.ctx(),
                                    ui.layer_id(),   // parent LayerId (required in 0.34)
                                    egui::Id::new("inline_diag_tip").with(di),
                                    |ui: &mut egui::Ui| {
                                        ui.set_max_width(380.0);
                                        ui.label(
                                            egui::RichText::new(&msg).size(12.0),
                                        );
                                        if let Some(c) = &code {
                                            ui.label(
                                                egui::RichText::new(format!("[{c}]"))
                                                    .size(10.5)
                                                    .color(egui::Color32::from_rgb(
                                                        140, 150, 170,
                                                    )),
                                            );
                                        }
                                    },
                                );
                            }
                        }
                    }
                }
            });

        // ── Panel 3: MCU Configurator ─────────────────────────────────────────
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("MCU Configurator");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let reset_btn = ui
                        .add(egui::Button::new(
                            egui::RichText::new(format!(
                                "{} Reset pins",
                                ph::ARROW_COUNTER_CLOCKWISE
                            ))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(220, 100, 80)),
                        ))
                        .on_hover_text("Clear all pin function selections");
                    if reset_btn.clicked() {
                        if let Some(mcu) = &mut self.mcu {
                            mcu.reset_all_pins();
                        }
                    }
                });
            });

            // Chip label — always read-only.
            // Selection is done exclusively via the "New Project" popup.
            ui.horizontal(|ui| {
                ui.label("Chip:");
                ui.label(
                    egui::RichText::new(self.selected_mcu_type.label())
                        .strong()
                        .color(egui::Color32::LIGHT_BLUE),
                );
                ui.label(
                    egui::RichText::new(format!("·  {}", self.selected_mcu_type.family()))
                        .color(egui::Color32::GRAY)
                        .size(11.0),
                );
            });

            ui.separator();

            // Tab bar
            ui.horizontal(|ui| {
                for tab in [
                    McuTab::Pins,
                    McuTab::Peripherals,
                    McuTab::Clock,
                    McuTab::System,
                ] {
                    let is_active = self.active_tab == tab;
                    let label = egui::RichText::new(tab.label())
                        .size(13.0)
                        .color(if is_active {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::from_rgb(160, 160, 170)
                        });
                    if ui.selectable_label(is_active, label).clicked() {
                        self.active_tab = tab;
                    }
                }
            });

            ui.separator();

            // Tab content
            match self.active_tab {
                McuTab::Pins => {
                    let pin_changed = egui::ScrollArea::both()
                        .show(ui, |ui| match &mut self.mcu {
                            Some(mcu) => mcu.draw(ui),
                            None => {
                                ui.centered_and_justified(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{}  {}  —  support coming soon",
                                            ph::GEAR,
                                            self.selected_mcu_type.label()
                                        ))
                                        .size(18.0)
                                        .color(egui::Color32::GRAY),
                                    );
                                });
                                None
                            }
                        })
                        .inner;

                    // Any pin change (configure OR deselect) triggers a full
                    // sync: files for unconfigured pins are removed, files for
                    // configured pins are created/updated, and mod.rs is rebuilt.
                    if pin_changed.is_some() {
                        if let Some(mcu) = &self.mcu {
                            let all_pins = mcu.all_pin_functions();
                            Self::sync_pin_files(
                                &mut self.user_src_files,
                                &mut self.user_src_folders,
                                &all_pins,
                            );
                        }
                    }
                }
                McuTab::Peripherals => show_peripherals_tab(ui, &self.mcu),
                McuTab::Clock => {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  Clock configuration — coming soon",
                                ph::CLOCK
                            ))
                            .size(16.0)
                            .color(egui::Color32::GRAY),
                        );
                    });
                }
                McuTab::System => {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  System configuration — coming soon",
                                ph::GEAR
                            ))
                            .size(16.0)
                            .color(egui::Color32::GRAY),
                        );
                    });
                }
            }
        });
    }
}

// ── Project tree ──────────────────────────────────────────────────────────────

fn show_project_tree(
    ui: &mut egui::Ui,
    pkg_name: &str,
    toolchain: &ToolchainKind,
    selected: &mut ProjectFileId,
    build_result: Option<&build::BuildResult>,
    lsp: Option<&lsp::LspState>,
    user_src_files: &mut Vec<(String, String)>,
    user_src_folders: &mut Vec<String>,
    new_src_name: &mut Option<String>,
    new_src_folder_name: &mut Option<String>,
    new_file_in_folder: &mut Option<(String, String)>,
    renaming_file: &mut Option<(usize, String)>,
    renaming_folder: &mut Option<(String, String)>,
    workspace_dir: &std::path::Path,
    save_needed: &mut bool,
) {
    let default_tree_folder_color = egui::Color32::from_rgb(100, 105, 115);

    ui.label(
        egui::RichText::new(format!("package: {pkg_name}"))
            .size(12.0)
            .strong()
            // .background_color(egui::Color32::LIGHT_GRAY)
            .color(egui::Color32::DARK_RED),
    );
    ui.add_space(2.0);

    // ── .cargo/ ───────────────────────────────────────────────────────────────
    egui::CollapsingHeader::new(
        egui::RichText::new(".cargo/")
            .size(11.5)
            .monospace()
            .color(default_tree_folder_color),
    )
    .default_open(true)
    .show(ui, |ui| {
        file_row(
            ui,
            8.0,
            "config.toml",
            ProjectFileId::CargoConfig,
            selected,
            build_result,
            lsp,
        );
    });

    // ── src/ ──────────────────────────────────────────────────────────────────
    let src_ch = egui::CollapsingHeader::new(
        egui::RichText::new("src/")
            .size(11.5)
            .monospace()
            .color(default_tree_folder_color),
    )
    .default_open(true)
    .show(ui, |ui| {
        file_row(
            ui,
            8.0,
            "main.rs",
            ProjectFileId::MainRs,
            selected,
            build_result,
            lsp,
        );

        // ── Build folder map: explicit folders ∪ implicit from file paths ─
        let mut folders: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for folder in user_src_folders.iter() {
            folders.entry(folder.clone()).or_default();
        }
        let mut direct: Vec<usize> = vec![];
        for (i, (path, _)) in user_src_files.iter().enumerate() {
            if let Some(slash) = path.find('/') {
                folders
                    .entry(path[..slash].to_string())
                    .or_default()
                    .push(i);
            } else {
                direct.push(i);
            }
        }

        let mut to_delete: Option<usize> = None;
        let mut folder_to_delete: Option<String> = None;
        // Signals: confirm/cancel for the folder-level file input
        let mut confirm_in_folder = false;
        let mut cancel_in_folder = false;
        // Which folder the user just clicked "+ New file" in
        let mut open_input_for_folder: Option<String> = None;
        // Set by folder context menu → open new-folder name input
        let mut ctx_open_new_folder = false;
        // Rename signals
        let mut do_rename_file: Option<usize> = None;
        let mut cancel_rename_file = false;
        let mut do_rename_folder = false;
        let mut cancel_rename_folder = false;

        // ── Direct files ──────────────────────────────────────────────────
        for &i in &direct {
            let name = user_src_files[i].0.clone();
            user_file_row(
                ui,
                8.0,
                &name,
                i,
                selected,
                &mut to_delete,
                renaming_file,
                &mut do_rename_file,
                &mut cancel_rename_file,
            );
        }

        // ── Folder groups ─────────────────────────────────────────────────
        for (folder_name, file_indices) in &folders {
            let is_renaming_this = renaming_folder
                .as_ref()
                .map(|(f, _)| f == folder_name)
                .unwrap_or(false);

            if is_renaming_this {
                // ── Inline folder rename input ─────────────────────────
                if let Some((_, new_name)) = renaming_folder.as_mut() {
                    let fid = egui::Id::new(("__rename_folder__", folder_name.as_str()));
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(ph::FOLDER)
                                .size(11.5)
                                .color(egui::Color32::from_rgb(200, 165, 70)),
                        );
                        let resp = ui.add(
                            egui::TextEdit::singleline(new_name)
                                .desired_width(ui.available_width()),
                        );
                        if ui.memory(|m| m.data.get_temp::<bool>(fid).unwrap_or(true)) {
                            resp.request_focus();
                            ui.memory_mut(|m| m.data.insert_temp(fid, false));
                        }
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        if enter {
                            do_rename_folder = true;
                        } else if esc || resp.lost_focus() {
                            cancel_rename_folder = true;
                        }
                    });
                }
            } else {
                let this_has_input = new_file_in_folder
                    .as_ref()
                    .map(|(f, _)| f == folder_name)
                    .unwrap_or(false);

                let ch = egui::CollapsingHeader::new(
                    egui::RichText::new(format!("{folder_name}/"))
                        .size(11.5)
                        .monospace()
                        .color(default_tree_folder_color),
                )
                .default_open(true)
                .show(ui, |ui| {
                    if file_indices.is_empty() && !this_has_input {
                        ui.label(
                            egui::RichText::new("  (empty)")
                                .size(10.0)
                                .color(egui::Color32::from_gray(95)),
                        );
                    }
                    for &i in file_indices {
                        let full = user_src_files[i].0.clone();
                        let fname = full.split('/').last().unwrap_or(&full).to_string();
                        user_file_row(
                            ui,
                            16.0,
                            &fname,
                            i,
                            selected,
                            &mut to_delete,
                            renaming_file,
                            &mut do_rename_file,
                            &mut cancel_rename_file,
                        );
                    }

                    // ── Inline file input for this folder ─────────────
                    if this_has_input {
                        if let Some((folder_key, name)) = new_file_in_folder.as_mut() {
                            let fid = egui::Id::new(("__folder_file_focus__", folder_key.as_str()));
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                let resp = ui.add(
                                    egui::TextEdit::singleline(name)
                                        .hint_text("filename.rs")
                                        .desired_width(ui.available_width()),
                                );
                                if ui.memory(|m| m.data.get_temp::<bool>(fid).unwrap_or(true)) {
                                    resp.request_focus();
                                    ui.memory_mut(|m| m.data.insert_temp(fid, false));
                                }
                                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                if enter {
                                    confirm_in_folder = true;
                                } else if esc || resp.lost_focus() {
                                    cancel_in_folder = true;
                                }
                            });
                        }
                    }
                });

                // Right-click context menu on the folder header
                let fname_ctx = folder_name.clone();
                ch.header_response.context_menu(|ui| {
                    if ui
                        .button(
                            egui::RichText::new(format!("{} New File", ph::FILE_PLUS)).size(11.5),
                        )
                        .clicked()
                    {
                        open_input_for_folder = Some(fname_ctx.clone());
                        ui.close();
                    }
                    if ui
                        .button(
                            egui::RichText::new(format!("{} New Folder", ph::FOLDER_PLUS))
                                .size(11.5),
                        )
                        .clicked()
                    {
                        ctx_open_new_folder = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button(
                            egui::RichText::new(format!("{} Rename", ph::PENCIL_SIMPLE)).size(11.5),
                        )
                        .clicked()
                    {
                        *renaming_folder = Some((fname_ctx.clone(), fname_ctx.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button(
                            egui::RichText::new(format!("{} Delete", ph::TRASH))
                                .size(11.5)
                                .color(egui::Color32::from_rgb(220, 80, 60)),
                        )
                        .clicked()
                    {
                        folder_to_delete = Some(fname_ctx);
                        ui.close();
                    }
                });
            }
        }

        // Apply folder-level file input results
        if confirm_in_folder {
            if let Some((folder, name)) = new_file_in_folder.take() {
                ui.memory_mut(|m| {
                    m.data.insert_temp::<bool>(
                        egui::Id::new(("__folder_file_focus__", folder.as_str())),
                        true,
                    )
                });
                let clean = name.trim().replace('\\', "/");
                let clean = clean.trim_start_matches('/').to_string();
                if !clean.is_empty() {
                    let full_path = format!("{folder}/{clean}");
                    if !user_src_files.iter().any(|(p, _)| p == &full_path) {
                        user_src_files.push((full_path, String::new()));
                        *selected = ProjectFileId::UserFile(user_src_files.len() - 1);
                        *save_needed = true;
                    }
                }
            }
        } else if cancel_in_folder {
            if let Some((folder, _)) = new_file_in_folder.as_ref() {
                ui.memory_mut(|m| {
                    m.data.insert_temp::<bool>(
                        egui::Id::new(("__folder_file_focus__", folder.as_str())),
                        true,
                    )
                });
            }
            *new_file_in_folder = None;
        }
        if let Some(folder) = open_input_for_folder {
            *new_file_in_folder = Some((folder, String::new()));
        }
        if ctx_open_new_folder {
            *new_src_folder_name = Some(String::new());
        }

        // ── Root-level new file input ─────────────────────────────────────
        const SRC_NAME_FID: &str = "__new_src_name_focus__";
        let mut confirm_file = false;
        let mut cancel_file = false;
        if let Some(name) = new_src_name {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text("e.g. utils.rs or drivers/spi.rs")
                        .desired_width(ui.available_width()),
                );
                let fid = egui::Id::new(SRC_NAME_FID);
                if ui.memory(|m| m.data.get_temp::<bool>(fid).unwrap_or(true)) {
                    resp.request_focus();
                    ui.memory_mut(|m| m.data.insert_temp(fid, false));
                }
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if enter {
                    confirm_file = true;
                } else if esc || resp.lost_focus() {
                    cancel_file = true;
                }
            });
        }
        if confirm_file {
            ui.memory_mut(|m| {
                m.data
                    .insert_temp::<bool>(egui::Id::new(SRC_NAME_FID), true)
            });
            if let Some(name) = new_src_name.take() {
                let clean = name.trim().replace('\\', "/");
                let clean = clean.trim_start_matches('/').to_string();
                if !clean.is_empty() && !user_src_files.iter().any(|(p, _)| p == &clean) {
                    user_src_files.push((clean, String::new()));
                    *selected = ProjectFileId::UserFile(user_src_files.len() - 1);
                    *save_needed = true;
                }
            }
        } else if cancel_file {
            ui.memory_mut(|m| {
                m.data
                    .insert_temp::<bool>(egui::Id::new(SRC_NAME_FID), true)
            });
            *new_src_name = None;
        }

        // ── Root-level new folder input ───────────────────────────────────
        const SRC_FOLDER_FID: &str = "__new_src_folder_focus__";
        let mut confirm_folder = false;
        let mut cancel_folder = false;
        if let Some(name) = new_src_folder_name {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text("e.g. drivers")
                        .desired_width(ui.available_width()),
                );
                let fid = egui::Id::new(SRC_FOLDER_FID);
                if ui.memory(|m| m.data.get_temp::<bool>(fid).unwrap_or(true)) {
                    resp.request_focus();
                    ui.memory_mut(|m| m.data.insert_temp(fid, false));
                }
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if enter {
                    confirm_folder = true;
                } else if esc || resp.lost_focus() {
                    cancel_folder = true;
                }
            });
        }
        if confirm_folder {
            ui.memory_mut(|m| {
                m.data
                    .insert_temp::<bool>(egui::Id::new(SRC_FOLDER_FID), true)
            });
            if let Some(name) = new_src_folder_name.take() {
                let clean: String = name
                    .trim()
                    .replace('\\', "/")
                    .split('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or("")
                    .to_string();
                if !clean.is_empty() && !user_src_folders.contains(&clean) {
                    let dest = workspace_dir.join("src").join(&clean);
                    let _ = std::fs::create_dir_all(&dest);
                    user_src_folders.push(clean);
                    *save_needed = true;
                }
            }
        } else if cancel_folder {
            ui.memory_mut(|m| {
                m.data
                    .insert_temp::<bool>(egui::Id::new(SRC_FOLDER_FID), true)
            });
            *new_src_folder_name = None;
        }

        // ── Handle file deletion ──────────────────────────────────────────
        if let Some(idx) = to_delete {
            if *selected == ProjectFileId::UserFile(idx) {
                *selected = ProjectFileId::MainRs;
            }
            if let ProjectFileId::UserFile(j) = selected {
                if *j > idx {
                    *j -= 1;
                }
            }
            let dest = workspace_dir.join("src").join(&user_src_files[idx].0);
            let _ = std::fs::remove_file(&dest);
            user_src_files.remove(idx);
            *save_needed = true;
        }

        // ── Handle folder deletion ────────────────────────────────────────
        if let Some(ref dname) = folder_to_delete {
            if let ProjectFileId::UserFile(_) = *selected {
                *selected = ProjectFileId::MainRs;
            }
            let prefix = format!("{dname}/");
            let to_rm: Vec<usize> = user_src_files
                .iter()
                .enumerate()
                .filter(|(_, (p, _))| p.starts_with(&prefix))
                .map(|(i, _)| i)
                .collect();
            for i in to_rm.into_iter().rev() {
                let dest = workspace_dir.join("src").join(&user_src_files[i].0);
                let _ = std::fs::remove_file(&dest);
                user_src_files.remove(i);
            }
            user_src_folders.retain(|f| f != dname);
            // Also close any active input for this folder
            if new_file_in_folder
                .as_ref()
                .map(|(f, _)| f == dname)
                .unwrap_or(false)
            {
                *new_file_in_folder = None;
            }
            let dest = workspace_dir.join("src").join(dname.as_str());
            let _ = std::fs::remove_dir_all(&dest);
            *save_needed = true;
        }

        // ── Apply file rename ─────────────────────────────────────────────
        if let Some(confirm_idx) = do_rename_file {
            if let Some((_, new_name)) = renaming_file.take() {
                ui.memory_mut(|m| {
                    m.data
                        .insert_temp::<bool>(egui::Id::new(("__rename_file__", confirm_idx)), true)
                });
                let old_path = user_src_files[confirm_idx].0.clone();
                let clean = new_name.trim().to_string();
                if !clean.is_empty() {
                    let new_path = if let Some(slash) = old_path.rfind('/') {
                        format!("{}/{clean}", &old_path[..slash])
                    } else {
                        clean
                    };
                    if new_path != old_path && !user_src_files.iter().any(|(p, _)| p == &new_path) {
                        let old_dest = workspace_dir.join("src").join(&old_path);
                        let new_dest = workspace_dir.join("src").join(&new_path);
                        let _ = std::fs::rename(&old_dest, &new_dest);
                        user_src_files[confirm_idx].0 = new_path;
                        *save_needed = true;
                    }
                }
            }
        } else if cancel_rename_file {
            if let Some((idx, _)) = renaming_file.as_ref() {
                ui.memory_mut(|m| {
                    m.data
                        .insert_temp::<bool>(egui::Id::new(("__rename_file__", *idx)), true)
                });
            }
            *renaming_file = None;
        }

        // ── Apply folder rename ───────────────────────────────────────────
        if do_rename_folder {
            if let Some((old_name, new_name)) = renaming_folder.take() {
                ui.memory_mut(|m| {
                    m.data.insert_temp::<bool>(
                        egui::Id::new(("__rename_folder__", old_name.as_str())),
                        true,
                    )
                });
                let clean = new_name.trim().to_string();
                if !clean.is_empty() && clean != old_name && !user_src_folders.contains(&clean) {
                    let old_dest = workspace_dir.join("src").join(&old_name);
                    let new_dest = workspace_dir.join("src").join(&clean);
                    let _ = std::fs::rename(&old_dest, &new_dest);
                    if let Some(f) = user_src_folders.iter_mut().find(|f| **f == old_name) {
                        *f = clean.clone();
                    }
                    let prefix = format!("{old_name}/");
                    let new_prefix = format!("{clean}/");
                    for (path, _) in user_src_files.iter_mut() {
                        if path.starts_with(&prefix) {
                            *path = format!("{new_prefix}{}", &path[prefix.len()..]);
                        }
                    }
                    *save_needed = true;
                }
            }
        } else if cancel_rename_folder {
            if let Some((old_name, _)) = renaming_folder.as_ref() {
                ui.memory_mut(|m| {
                    m.data.insert_temp::<bool>(
                        egui::Id::new(("__rename_folder__", old_name.as_str())),
                        true,
                    )
                });
            }
            *renaming_folder = None;
        }
    });

    // Right-click context menu on the src/ folder header
    let mut src_ctx_new_file = false;
    let mut src_ctx_new_folder = false;
    src_ch.header_response.context_menu(|ui| {
        if ui
            .button(egui::RichText::new(format!("{} New File", ph::FILE_PLUS)).size(11.5))
            .clicked()
        {
            src_ctx_new_file = true;
            ui.close();
        }
        if ui
            .button(egui::RichText::new(format!("{} New Folder", ph::FOLDER_PLUS)).size(11.5))
            .clicked()
        {
            src_ctx_new_folder = true;
            ui.close();
        }
    });
    if src_ctx_new_file {
        *new_src_name = Some(String::new());
    }
    if src_ctx_new_folder {
        *new_src_folder_name = Some(String::new());
    }

    // ── Root files ────────────────────────────────────────────────────────────
    ui.add_space(2.0);
    file_row(
        ui,
        4.0,
        ".gitignore",
        ProjectFileId::GitIgnore,
        selected,
        build_result,
        lsp,
    );
    // build.rs and memory.x only exist for RustEmbedded toolchain
    if *toolchain == ToolchainKind::RustEmbedded {
        file_row(
            ui,
            4.0,
            "build.rs",
            ProjectFileId::BuildRs,
            selected,
            build_result,
            lsp,
        );
    }
    file_row(
        ui,
        4.0,
        "Cargo.toml",
        ProjectFileId::CargoToml,
        selected,
        build_result,
        lsp,
    );
    if *toolchain == ToolchainKind::RustEmbedded {
        file_row(
            ui,
            4.0,
            "memory.x",
            ProjectFileId::MemoryX,
            selected,
            build_result,
            lsp,
        );
    }
}

// ── Single file row for fixed project files (with diagnostic indicators) ──────

fn file_row(
    ui: &mut egui::Ui,
    indent: f32,
    name: &str,
    id: ProjectFileId,
    selected: &mut ProjectFileId,
    build_result: Option<&build::BuildResult>,
    lsp: Option<&lsp::LspState>,
) {
    let dim = egui::Color32::from_rgb(140, 150, 165);
    let hi = egui::Color32::from_rgb(100, 180, 255);
    let normal = egui::Color32::from_rgb(200, 205, 215);

    ui.horizontal(|ui| {
        ui.add_space(indent);
        let is_sel = *selected == id;
        let color = if is_sel { hi } else { normal };
        let icon_color = if is_sel {
            egui::Color32::from_rgb(180, 210, 255)
        } else {
            egui::Color32::from_rgb(160, 170, 190)
        };
        ui.label(egui::RichText::new(ph::FILE).size(11.5).color(icon_color));
        let resp = ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .size(11.5)
                    .monospace()
                    .color(color),
            )
            .sense(egui::Sense::click()),
        );
        if resp.clicked() {
            *selected = id;
        }
        if resp.hovered() && !is_sel {
            let r = resp.rect;
            ui.painter().line_segment(
                [r.left_bottom(), r.right_bottom()],
                egui::Stroke::new(1.0, dim),
            );
        }
        if let Some(cargo_path) = id.cargo_path() {
            let cargo_err = build_result.map_or(false, |r| r.has_errors_in(cargo_path));
            let cargo_warn = build_result.map_or(false, |r| r.has_warnings_in(cargo_path));
            let lsp_err = lsp.map_or(false, |l| l.error_count_for(cargo_path) > 0);
            let lsp_warn = lsp.map_or(false, |l| l.warning_count_for(cargo_path) > 0);
            let has_err = cargo_err || lsp_err;
            let has_warn = cargo_warn || lsp_warn;
            let (dot, dot_color) = if has_err {
                (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
            } else if has_warn {
                (ph::WARNING, egui::Color32::from_rgb(220, 180, 50))
            } else {
                ("", egui::Color32::TRANSPARENT)
            };
            if !dot.is_empty() {
                ui.label(egui::RichText::new(dot).size(10.0).color(dot_color));
            }
        }
    });
}

// ── Single file row for user-created source files (with delete button) ────────

fn user_file_row(
    ui: &mut egui::Ui,
    indent: f32,
    name: &str,
    idx: usize,
    selected: &mut ProjectFileId,
    to_delete: &mut Option<usize>,
    renaming: &mut Option<(usize, String)>,
    do_rename: &mut Option<usize>,
    cancel_rename: &mut bool,
) {
    let hi = egui::Color32::from_rgb(100, 180, 255);
    let normal = egui::Color32::from_rgb(200, 205, 215);
    let id = ProjectFileId::UserFile(idx);
    let is_renaming = renaming.as_ref().map(|(i, _)| *i == idx).unwrap_or(false);

    // ── Inline rename mode ────────────────────────────────────────────────
    if is_renaming {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            ui.label(
                egui::RichText::new(ph::FILE)
                    .size(11.5)
                    .color(egui::Color32::from_rgb(180, 210, 255)),
            );
            if let Some((_, new_name)) = renaming.as_mut() {
                let fid = egui::Id::new(("__rename_file__", idx));
                let resp = ui
                    .add(egui::TextEdit::singleline(new_name).desired_width(ui.available_width()));
                if ui.memory(|m| m.data.get_temp::<bool>(fid).unwrap_or(true)) {
                    resp.request_focus();
                    ui.memory_mut(|m| m.data.insert_temp(fid, false));
                }
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if enter {
                    *do_rename = Some(idx);
                } else if esc || resp.lost_focus() {
                    *cancel_rename = true;
                }
            }
        });
        return;
    }

    // ── Normal display mode ───────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.add_space(indent);
        let is_sel = *selected == id;
        let color = if is_sel { hi } else { normal };
        let icon_color = if is_sel {
            egui::Color32::from_rgb(180, 210, 255)
        } else {
            egui::Color32::from_rgb(160, 170, 190)
        };
        ui.label(egui::RichText::new(ph::FILE).size(11.5).color(icon_color));
        let resp = ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .size(11.5)
                    .monospace()
                    .color(color),
            )
            .sense(egui::Sense::click()),
        );
        if resp.clicked() {
            *selected = id;
        }
        resp.context_menu(|ui| {
            if ui
                .button(egui::RichText::new(format!("{} Rename", ph::PENCIL_SIMPLE)).size(11.5))
                .clicked()
            {
                *renaming = Some((idx, name.to_string()));
                ui.close();
            }
            ui.separator();
            if ui
                .button(
                    egui::RichText::new(format!("{} Delete", ph::TRASH))
                        .size(11.5)
                        .color(egui::Color32::from_rgb(220, 80, 60)),
                )
                .clicked()
            {
                *to_delete = Some(idx);
                ui.close();
            }
        });
    });
}

// ── Peripherals tab ───────────────────────────────────────────────────────────

fn show_peripherals_tab(ui: &mut egui::Ui, mcu: &Option<Mcu>) {
    let Some(mcu) = mcu else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No chip selected.").color(egui::Color32::GRAY));
        });
        return;
    };

    let configured: Vec<_> = mcu
        .top_pins
        .iter()
        .chain(mcu.bottom_pins.iter())
        .chain(mcu.left_pins.iter())
        .chain(mcu.right_pins.iter())
        .filter(|p| !p.reserved && p.selected_function != PinFunction::Unset)
        .collect();

    if configured.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No pins configured yet.")
                    .size(14.0)
                    .color(egui::Color32::GRAY),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Switch to the Pins tab and assign functions to pins.")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(120, 120, 130)),
            );
        });
        return;
    }

    let mut gpio_in = vec![];
    let mut gpio_out = vec![];
    let mut adc = vec![];
    let mut timers = vec![];
    let mut usart = vec![];
    let mut spi = vec![];
    let mut i2c = vec![];
    let mut usb = vec![];
    let mut can = vec![];
    let mut swd = vec![];
    let mut other = vec![];

    for pin in &configured {
        match &pin.selected_function {
            PinFunction::GpioInput => gpio_in.push(*pin),
            PinFunction::GpioOutput => gpio_out.push(*pin),
            PinFunction::AdcChannel { .. } => adc.push(*pin),
            PinFunction::TimerPwm { .. } => timers.push(*pin),
            PinFunction::UsartTx(_)
            | PinFunction::UsartRx(_)
            | PinFunction::UsartCts(_)
            | PinFunction::UsartRts(_)
            | PinFunction::UsartCk(_) => usart.push(*pin),
            PinFunction::SpiNss(_)
            | PinFunction::SpiSck(_)
            | PinFunction::SpiMiso(_)
            | PinFunction::SpiMosi(_) => spi.push(*pin),
            PinFunction::I2cScl(_) | PinFunction::I2cSda(_) => i2c.push(*pin),
            PinFunction::UsbDm | PinFunction::UsbDp => usb.push(*pin),
            PinFunction::CanRx | PinFunction::CanTx => can.push(*pin),
            PinFunction::SwdIo | PinFunction::SwdClk => swd.push(*pin),
            _ => other.push(*pin),
        }
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);
        periph_section(
            ui,
            "GPIO Input",
            &gpio_in,
            egui::Color32::from_rgb(70, 160, 70),
        );
        periph_section(
            ui,
            "GPIO Output",
            &gpio_out,
            egui::Color32::from_rgb(200, 120, 50),
        );
        periph_section(ui, "ADC", &adc, egui::Color32::from_rgb(150, 70, 200));
        periph_section(
            ui,
            "Timers / PWM",
            &timers,
            egui::Color32::from_rgb(190, 170, 30),
        );
        periph_section(ui, "USART", &usart, egui::Color32::from_rgb(50, 110, 200));
        periph_section(ui, "SPI", &spi, egui::Color32::from_rgb(30, 170, 170));
        periph_section(ui, "I2C", &i2c, egui::Color32::from_rgb(60, 180, 100));
        periph_section(ui, "USB", &usb, egui::Color32::from_rgb(190, 50, 160));
        periph_section(ui, "CAN", &can, egui::Color32::from_rgb(200, 130, 20));
        periph_section(
            ui,
            "SWD / Debug",
            &swd,
            egui::Color32::from_rgb(190, 50, 50),
        );
        periph_section(ui, "Other", &other, egui::Color32::GRAY);
    });
}

fn periph_section(ui: &mut egui::Ui, title: &str, pins: &[&Pin], color: egui::Color32) {
    if pins.is_empty() {
        return;
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 16.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, color);
        ui.label(egui::RichText::new(title).size(13.0).strong().color(color));
    });

    let dim = egui::Color32::from_rgb(140, 140, 155);
    egui::Grid::new(format!("periph_grid_{title}"))
        .num_columns(3)
        .spacing([16.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Pin").size(11.0).color(dim));
            ui.label(egui::RichText::new("Name").size(11.0).color(dim));
            ui.label(egui::RichText::new("Function").size(11.0).color(dim));
            ui.end_row();

            for pin in pins {
                ui.label(
                    egui::RichText::new(format!("#{}", pin.number))
                        .size(11.0)
                        .monospace(),
                );
                ui.label(
                    egui::RichText::new(pin.name.as_str())
                        .size(11.0)
                        .monospace()
                        .color(egui::Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new(pin.selected_function.label())
                        .size(11.0)
                        .color(color),
                );
                ui.end_row();
            }
        });

    ui.add_space(2.0);
    ui.separator();
}

// ── Diagnostics panel (tabbed: Cargo Check | rust-analyzer) ──────────────────

fn show_diag_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    build_state: &Arc<Mutex<BuildState>>,
    lsp_state: &Arc<Mutex<lsp::LspState>>,
    dfu_state: &Arc<Mutex<DfuState>>,
    dfu_log: &Arc<Mutex<Vec<String>>>,
    dfu_programmers: &Arc<Mutex<Vec<dfu::ProgrammerInfo>>>,
    dfu_sel_programmer: &mut usize,
    dfu_flash_addr: &mut String,
    openocd_state: &Arc<Mutex<OpenOcdState>>,
    openocd_target_cfg: &mut String,
    espflash_state: &Arc<Mutex<EspFlashState>>,
    tools_state: &Arc<Mutex<required_tools::ToolsState>>,
    toolchain: &ToolchainKind,
    tab: &mut BuildPanelTab,
    cargo_sel: &mut Option<usize>,
    lsp_sel: &mut Option<usize>,
    selected_file: &mut ProjectFileId,
) {
    // ── Tab header ────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        // Cargo tab button
        {
            let st = build_state.lock().unwrap();
            let (badge, col) = match &*st {
                BuildState::Done(r) if r.error_count() > 0 => (
                    format!(" {} {}", r.error_count(), ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                ),
                BuildState::Done(r) if r.warning_count() > 0 => (
                    format!(" {} {}", r.warning_count(), ph::WARNING),
                    egui::Color32::from_rgb(210, 170, 40),
                ),
                BuildState::Done(r) if r.success => (
                    format!(" {}", ph::CHECK_CIRCLE),
                    egui::Color32::from_rgb(80, 200, 100),
                ),
                BuildState::Building => (" …".to_owned(), egui::Color32::GRAY),
                _ => (String::new(), egui::Color32::GRAY),
            };
            let label = format!("{} Cargo Check{badge}", ph::HAMMER);
            let active = *tab == BuildPanelTab::Cargo;
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Cargo;
            }
        }

        ui.separator();

        // RA tab button
        {
            let lsp = lsp_state.lock().unwrap();
            let (badge, col) = match &lsp.status {
                LspStatus::Starting | LspStatus::Indexing => {
                    (" …".to_owned(), egui::Color32::from_rgb(180, 180, 80))
                }
                LspStatus::Ready if lsp.total_errors() > 0 => (
                    format!(" {} {}", lsp.total_errors(), ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                ),
                LspStatus::Ready if lsp.total_warnings() > 0 => (
                    format!(" {} {}", lsp.total_warnings(), ph::WARNING),
                    egui::Color32::from_rgb(210, 170, 40),
                ),
                LspStatus::Ready => (
                    format!(" {}", ph::CHECK_CIRCLE),
                    egui::Color32::from_rgb(80, 200, 100),
                ),
                LspStatus::Failed(_) => (
                    format!(" {}", ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                ),
                _ => (String::new(), egui::Color32::DARK_GRAY),
            };
            let label = format!("rust-analyzer{badge}");
            let active = *tab == BuildPanelTab::RustAnalyzer;
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::RustAnalyzer;
            }
        }

        ui.separator();

        // Flash tab button — badge reflects whichever flash operation is active
        {
            let dfu = dfu_state.lock().unwrap();
            let ocd = openocd_state.lock().unwrap();
            let esp = espflash_state.lock().unwrap();
            let any_busy = dfu.is_busy() || ocd.is_busy() || esp.is_busy();
            let any_success = matches!(*dfu, DfuState::Success)
                || matches!(*ocd, OpenOcdState::Success)
                || matches!(*esp, EspFlashState::Success);
            let any_error = matches!(*dfu, DfuState::Error(_))
                || matches!(*ocd, OpenOcdState::Error(_))
                || matches!(*esp, EspFlashState::Error(_));
            let (badge, col) = if any_busy {
                if matches!(*dfu, DfuState::Flashing)
                    || matches!(*ocd, OpenOcdState::Flashing)
                    || matches!(*esp, EspFlashState::Flashing)
                {
                    (" …".to_owned(), egui::Color32::from_rgb(100, 180, 255))
                } else {
                    (" …".to_owned(), egui::Color32::from_rgb(220, 180, 60))
                }
            } else if any_success {
                (
                    format!(" {}", ph::CHECK_CIRCLE),
                    egui::Color32::from_rgb(80, 200, 100),
                )
            } else if any_error {
                (
                    format!(" {}", ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                )
            } else {
                (String::new(), egui::Color32::DARK_GRAY)
            };
            drop(esp);
            drop(ocd);
            drop(dfu);
            let label = format!("{} Flash{badge}", ph::LIGHTNING);
            let active = *tab == BuildPanelTab::Dfu;
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Dfu;
            }
        }

        ui.separator();

        // Required Tools tab button
        {
            let ts = tools_state.lock().unwrap();
            let missing = ts.missing_installable_count();
            let any_busy = ts.any_busy();
            drop(ts);
            let (badge, col) = if any_busy {
                (" …".to_owned(), egui::Color32::from_rgb(180, 180, 80))
            } else if missing > 0 {
                (
                    format!(" {} {}", missing, ph::WARNING),
                    egui::Color32::from_rgb(230, 160, 50),
                )
            } else {
                (String::new(), egui::Color32::DARK_GRAY)
            };
            let label = format!("{} Tools{badge}", ph::WRENCH);
            let active = *tab == BuildPanelTab::RequiredTools;
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::RequiredTools;
            }
        }
    });

    ui.separator();

    // ── Tab content ───────────────────────────────────────────────────────────
    match tab {
        BuildPanelTab::Cargo => {
            show_cargo_tab(ui, ctx, build_state, cargo_sel, selected_file);
        }
        BuildPanelTab::RustAnalyzer => {
            show_ra_tab(ui, lsp_state, lsp_sel, selected_file);
        }
        BuildPanelTab::Dfu => {
            show_dfu_tab(
                ui,
                dfu_state,
                dfu_log,
                dfu_programmers,
                dfu_sel_programmer,
                dfu_flash_addr,
                openocd_state,
                openocd_target_cfg,
                espflash_state,
                toolchain,
            );
        }
        BuildPanelTab::RequiredTools => {
            show_tools_tab(ui, tools_state, ctx);
        }
    }
}

// ── DFU Flash tab ─────────────────────────────────────────────────────────────

fn show_dfu_tab(
    ui: &mut egui::Ui,
    dfu_state: &Arc<Mutex<DfuState>>,
    dfu_log: &Arc<Mutex<Vec<String>>>,
    dfu_programmers: &Arc<Mutex<Vec<dfu::ProgrammerInfo>>>,
    dfu_sel_programmer: &mut usize,
    dfu_flash_addr: &mut String,
    openocd_state: &Arc<Mutex<OpenOcdState>>,
    openocd_target_cfg: &mut String,
    espflash_state: &Arc<Mutex<EspFlashState>>,
    toolchain: &ToolchainKind,
) {
    let state = dfu_state.lock().unwrap().clone();
    let ocd_state = openocd_state.lock().unwrap().clone();
    let esp_state = espflash_state.lock().unwrap().clone();
    let log = dfu_log.lock().unwrap().clone();
    let progs = dfu_programmers.lock().unwrap().clone();

    // Determine selected programmer kind for adaptive config UI
    let sel_kind = progs.get(*dfu_sel_programmer).map(|p| p.kind).unwrap_or("");
    let is_swd = matches!(sel_kind, "ST-Link" | "J-Link" | "CMSIS-DAP");
    let interface_cfg = openocd::interface_cfg_for_kind(sel_kind);

    // ── Programmer selector ComboBox ──────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Programmer:")
                .size(10.5)
                .color(egui::Color32::GRAY),
        );

        let combo_label = if progs.is_empty() {
            "— none detected —".to_string()
        } else {
            progs
                .get(*dfu_sel_programmer)
                .map(|p| p.combo_label())
                .unwrap_or_else(|| "— select —".to_string())
        };

        egui::ComboBox::from_id_salt("dfu_programmer_selector")
            .selected_text(egui::RichText::new(&combo_label).size(10.5).monospace())
            .width(ui.available_width() - 2.0)
            .show_ui(ui, |ui| {
                if progs.is_empty() {
                    ui.label(
                        egui::RichText::new("No programmer detected. Click 'Scan USB'.")
                            .size(10.5)
                            .color(egui::Color32::GRAY),
                    );
                }
                for (i, p) in progs.iter().enumerate() {
                    let kind_color = match p.kind {
                        "DFU Bootloader" => egui::Color32::from_rgb(100, 200, 255),
                        "ST-Link" => egui::Color32::from_rgb(100, 220, 120),
                        "J-Link" => egui::Color32::from_rgb(220, 180, 60),
                        "CMSIS-DAP" => egui::Color32::from_rgb(180, 140, 220),
                        "USB-Serial" => egui::Color32::from_rgb(200, 160, 100),
                        "ESP32" => egui::Color32::from_rgb(220, 120, 60),
                        _ => egui::Color32::GRAY,
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("[{}]", p.kind))
                                .size(10.0)
                                .monospace()
                                .color(kind_color),
                        );
                        ui.selectable_value(
                            dfu_sel_programmer,
                            i,
                            egui::RichText::new(format!("{}  [{}]", p.name, p.vid_pid))
                                .size(10.5)
                                .monospace(),
                        );
                    });
                }
            });
    });

    // Guidance text for selected programmer
    if let Some(p) = progs.get(*dfu_sel_programmer) {
        let guidance = p.guidance();
        if !guidance.is_empty() {
            let color = match p.kind {
                "DFU Bootloader" => egui::Color32::from_rgb(80, 200, 100),
                "ST-Link" | "J-Link" | "CMSIS-DAP" => egui::Color32::from_rgb(180, 180, 100),
                _ => egui::Color32::from_rgb(160, 160, 170),
            };
            for line in guidance.lines() {
                ui.label(egui::RichText::new(line).size(10.0).color(color).italics());
            }
        }
    }

    ui.separator();

    // ── Config row — adaptive: ESP32 / SWD (OpenOCD) / DFU ───────────────────
    ui.horizontal(|ui| {
        let build_done = log.iter().any(|l| l.contains("✔ Build OK"));

        // Helper: render a phase indicator icon + label
        let phase_widget = |ui: &mut egui::Ui, icon: &str, label: &str, color: egui::Color32| {
            ui.label(egui::RichText::new(icon).size(11.5).color(color));
            ui.label(egui::RichText::new(label).size(11.0).color(color));
        };

        if *toolchain == ToolchainKind::EspRust {
            // ── EspRust config ────────────────────────────────────────────────
            ui.label(
                egui::RichText::new("Tool: espflash")
                    .size(10.5)
                    .color(egui::Color32::from_rgb(220, 140, 60)),
            );
            ui.separator();

            // Phases: Build → Flash ESP
            let (b_icon, b_col) = match &esp_state {
                EspFlashState::Building => {
                    (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(220, 180, 60))
                }
                _ if build_done => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                EspFlashState::Error(_) if !build_done && !log.is_empty() => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, b_icon, "Build", b_col);
            ui.label(
                egui::RichText::new("→")
                    .size(11.0)
                    .color(egui::Color32::from_gray(70)),
            );

            let (f_icon, f_col) = match &esp_state {
                EspFlashState::Flashing => {
                    (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(220, 140, 60))
                }
                EspFlashState::Success => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                EspFlashState::Error(_) if build_done => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, f_icon, "Flash ESP32", f_col);
        } else if is_swd {
            // ── SWD / OpenOCD config ──────────────────────────────────────────
            ui.label(
                egui::RichText::new("Interface:")
                    .size(10.5)
                    .color(egui::Color32::GRAY),
            );
            ui.label(
                egui::RichText::new(interface_cfg)
                    .size(10.5)
                    .monospace()
                    .color(egui::Color32::from_rgb(120, 160, 200)),
            )
            .on_hover_text("OpenOCD interface config — auto-selected from programmer type");

            ui.separator();

            ui.label(
                egui::RichText::new("Target:")
                    .size(10.5)
                    .color(egui::Color32::GRAY),
            );
            ui.add(
                egui::TextEdit::singleline(openocd_target_cfg)
                    .desired_width(140.0)
                    .font(egui::TextStyle::Monospace),
            )
            .on_hover_text(
                "OpenOCD target config file.\n\
                 Examples: target/stm32f1x.cfg, target/stm32f4x.cfg\n\
                 Full list in your OpenOCD install under scripts/target/",
            );

            // Quick-select presets for common STM32 families
            for (label, cfg, tip) in [
                ("F1", "target/stm32f1x.cfg", "STM32F1 (F103, F100, F105, …)"),
                ("F4", "target/stm32f4x.cfg", "STM32F4 (F407, F411, F401, …)"),
                ("H7", "target/stm32h7x.cfg", "STM32H7 (H743, H750, …)"),
                ("L4", "target/stm32l4x.cfg", "STM32L4 (L432, L476, L496, …)"),
            ] {
                if ui
                    .add(egui::Button::new(egui::RichText::new(label).size(10.0)))
                    .on_hover_text(tip)
                    .clicked()
                {
                    *openocd_target_cfg = cfg.to_string();
                }
            }

            ui.separator();

            // SWD phases: Build → Flash (no objcopy — OpenOCD programs ELF directly)
            let (b_icon, b_col) = match &ocd_state {
                OpenOcdState::Building => (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(220, 180, 60)),
                _ if build_done => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                OpenOcdState::Error(_) if !build_done && !log.is_empty() => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, b_icon, "Build", b_col);
            ui.label(
                egui::RichText::new("→")
                    .size(11.0)
                    .color(egui::Color32::from_gray(70)),
            );

            let (f_icon, f_col) = match &ocd_state {
                OpenOcdState::Flashing => {
                    (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(100, 180, 255))
                }
                OpenOcdState::Success => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                OpenOcdState::Error(_) if build_done => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, f_icon, "Flash SWD", f_col);
        } else {
            // ── DFU config ────────────────────────────────────────────────────
            ui.label(
                egui::RichText::new("Flash addr:")
                    .size(10.5)
                    .color(egui::Color32::GRAY),
            );
            ui.add(
                egui::TextEdit::singleline(dfu_flash_addr)
                    .desired_width(82.0)
                    .font(egui::TextStyle::Monospace),
            )
            .on_hover_text(
                "Start address passed to dfu-util.\n\
                 0x08000000 — standard (no bootloader)\n\
                 0x08002000 — with 8 KB bootloader offset",
            );

            if ui
                .add(egui::Button::new(egui::RichText::new("Std").size(10.0)))
                .on_hover_text("0x08000000 — standard flash start (no bootloader)")
                .clicked()
            {
                *dfu_flash_addr = "0x08000000".to_string();
            }
            if ui
                .add(egui::Button::new(egui::RichText::new("+BL").size(10.0)))
                .on_hover_text("0x08002000 — 8 KB bootloader offset")
                .clicked()
            {
                *dfu_flash_addr = "0x08002000".to_string();
            }

            ui.separator();

            // DFU phases: Build → Objcopy → Flash
            let objcopy_done = log.iter().any(|l| l.contains("✔ firmware.bin ready"));

            let (b_icon, b_col) = match &state {
                DfuState::Building => (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(220, 180, 60)),
                _ if build_done => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                DfuState::Error(_) if !build_done && !log.is_empty() => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, b_icon, "Build", b_col);
            ui.label(
                egui::RichText::new("→")
                    .size(11.0)
                    .color(egui::Color32::from_gray(70)),
            );

            let (o_icon, o_col) = match &state {
                _ if objcopy_done => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                DfuState::Flashing | DfuState::Success => {
                    (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100))
                }
                DfuState::Error(_) if build_done && !objcopy_done => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                DfuState::Building if build_done => {
                    (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(220, 180, 60))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, o_icon, "Objcopy", o_col);
            ui.label(
                egui::RichText::new("→")
                    .size(11.0)
                    .color(egui::Color32::from_gray(70)),
            );

            let (f_icon, f_col) = match &state {
                DfuState::Flashing => (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(100, 180, 255)),
                DfuState::Success => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                DfuState::Error(_) if objcopy_done => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, f_icon, "Flash", f_col);
        }

        // Clear button — right-aligned, always visible, resets both states + log
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(
                    egui::RichText::new(format!("{} Clear", ph::X))
                        .size(10.0)
                        .color(egui::Color32::GRAY),
                ))
                .clicked()
            {
                dfu_log.lock().unwrap().clear();
                *dfu_state.lock().unwrap() = DfuState::Idle;
                *openocd_state.lock().unwrap() = OpenOcdState::Idle;
                *espflash_state.lock().unwrap() = EspFlashState::Idle;
            }
        });
    });

    // ── ESP32 diagnostic row (read-only chip identification) ─────────────────
    // Shown only for EspRust; sits between the config row and the log area.
    if *toolchain == ToolchainKind::EspRust {
        ui.horizontal(|ui| {
            let busy = esp_state.is_busy();
            let reading = matches!(esp_state, EspFlashState::ReadingInfo);

            let btn_color = if reading {
                egui::Color32::from_rgb(100, 180, 255)
            } else if busy {
                egui::Color32::GRAY
            } else {
                egui::Color32::from_rgb(100, 180, 255)
            };

            let btn_label = if reading {
                format!("{} Reading…", ph::CIRCLE_NOTCH)
            } else {
                format!("{} Read Chip Info", ph::INFO)
            };

            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(
                        egui::RichText::new(&btn_label).size(10.5).color(btn_color),
                    ),
                )
                .on_hover_text(
                    "Run `espflash board-info` — connects to the chip and reads:\n\
                     chip type, silicon revision, flash size, MAC address.\n\
                     \n\
                     Nothing is written to flash — safe to run at any time.\n\
                     Use this to verify the USB cable / COM port works before flashing.",
                )
                .clicked()
            {
                espflash::read_board_info(
                    Arc::clone(espflash_state),
                    Arc::clone(dfu_log),
                    ui.ctx().clone(),
                );
            }

            ui.label(
                egui::RichText::new("← verify connectivity without flashing")
                    .size(10.0)
                    .color(egui::Color32::from_gray(100))
                    .italics(),
            );
        });
    }

    ui.separator();

    // ── DFU Error / Success banners ───────────────────────────────────────────
    if let DfuState::Error(msg) = &state {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ph::X_CIRCLE)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(220, 80, 70)),
            );
            ui.label(
                egui::RichText::new(format!("DFU: {}", msg.lines().next().unwrap_or("Error")))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(220, 80, 70))
                    .strong(),
            );
        });
        ui.separator();
    }
    if matches!(state, DfuState::Success) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ph::CHECK_CIRCLE)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(80, 200, 100)),
            );
            ui.label(
                egui::RichText::new("Device programmed via DFU successfully!")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(80, 200, 100))
                    .strong(),
            );
        });
        ui.separator();
    }

    // ── OpenOCD Error / Success banners ───────────────────────────────────────
    if let OpenOcdState::Error(msg) = &ocd_state {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ph::X_CIRCLE)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(220, 80, 70)),
            );
            ui.label(
                egui::RichText::new(format!("SWD: {}", msg.lines().next().unwrap_or("Error")))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(220, 80, 70))
                    .strong(),
            );
        });
        ui.separator();
    }
    if matches!(ocd_state, OpenOcdState::Success) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ph::CHECK_CIRCLE)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(80, 200, 100)),
            );
            ui.label(
                egui::RichText::new("Device programmed via SWD successfully!")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(80, 200, 100))
                    .strong(),
            );
        });
        ui.separator();
    }

    // ── ESP32 flash Error / Success banners ───────────────────────────────────
    if let EspFlashState::Error(msg) = &esp_state {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ph::X_CIRCLE)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(220, 80, 70)),
            );
            ui.label(
                egui::RichText::new(format!("ESP: {}", msg.lines().next().unwrap_or("Error")))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(220, 80, 70))
                    .strong(),
            );
        });
        ui.separator();
    }
    if matches!(esp_state, EspFlashState::Success) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ph::CHECK_CIRCLE)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(80, 200, 100)),
            );
            ui.label(
                egui::RichText::new("ESP32 programmed successfully!")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(80, 200, 100))
                    .strong(),
            );
        });
        ui.separator();
    }

    // ── Scrollable log (shared between DFU and OpenOCD operations) ────────────
    egui::ScrollArea::vertical()
        .id_salt("dfu_log_scroll")
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if log.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "No output yet.\n\
                         • Flash USB    — DFU mode (STM32 with BOOT0 = 1)\n\
                         • Flash SWD    — ST-Link / J-Link via OpenOCD\n\
                         • Flash ESP32  — espflash (ESP32-C3 via USB-Serial)",
                    )
                    .size(11.0)
                    .color(egui::Color32::GRAY),
                );
                return;
            }

            for line in &log {
                // Colour-code lines by content prefix / keywords
                let color = if line.starts_with("✔") {
                    egui::Color32::from_rgb(80, 200, 100)
                } else if line.starts_with("▶") {
                    egui::Color32::from_rgb(100, 180, 255)
                } else if line.contains("error") && !line.contains("0 error") {
                    egui::Color32::from_rgb(220, 100, 80)
                } else if line.contains("warning") && !line.contains("0 warning") {
                    egui::Color32::from_rgb(210, 170, 40)
                } else {
                    egui::Color32::from_rgb(175, 180, 192)
                };
                ui.label(
                    egui::RichText::new(line.as_str())
                        .size(10.5)
                        .monospace()
                        .color(color),
                );
            }
        });
}

// ── Cargo Check tab ───────────────────────────────────────────────────────────

fn show_cargo_tab(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    build_state: &Arc<Mutex<BuildState>>,
    selected_diagnostic: &mut Option<usize>,
    selected_file: &mut ProjectFileId,
) {
    let state = build_state.lock().unwrap().clone();
    let workspace = std::env::temp_dir().join("embedded_ide_0_check");

    // ── Status bar ────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let (icon, text, color) = match &state {
            BuildState::Idle => return,
            BuildState::Building => (
                ph::HAMMER,
                "Building…".to_owned(),
                egui::Color32::from_rgb(180, 180, 180),
            ),
            BuildState::Failed(msg) => {
                let first = msg.lines().next().unwrap_or(msg);
                // Suppress the [DISK_FULL] prefix from the one-liner badge
                let first = first.strip_prefix("[DISK_FULL] ").unwrap_or(first);
                (
                    ph::X_CIRCLE,
                    format!("Build failed: {}", first),
                    egui::Color32::from_rgb(230, 90, 80),
                )
            }
            BuildState::Done(r) if r.error_count() > 0 => (
                ph::X_CIRCLE,
                format!(
                    "{} error{}{}",
                    r.error_count(),
                    if r.error_count() == 1 { "" } else { "s" },
                    if r.warning_count() > 0 {
                        format!(
                            ",  {} warning{}",
                            r.warning_count(),
                            if r.warning_count() == 1 { "" } else { "s" }
                        )
                    } else {
                        String::new()
                    }
                ),
                egui::Color32::from_rgb(230, 90, 80),
            ),
            BuildState::Done(r) if r.warning_count() > 0 => (
                ph::WARNING,
                format!(
                    "{} warning{}",
                    r.warning_count(),
                    if r.warning_count() == 1 { "" } else { "s" }
                ),
                egui::Color32::from_rgb(230, 190, 50),
            ),
            BuildState::Done(_) => (
                ph::CHECK_CIRCLE,
                "Build succeeded — no errors".to_owned(),
                egui::Color32::from_rgb(80, 200, 100),
            ),
        };

        ui.label(egui::RichText::new(icon).size(13.0).color(color));
        ui.label(egui::RichText::new(text).size(12.0).color(color).strong());

        // Right-side buttons: Clean target/ | Clear
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Clear result button
            if ui
                .add(egui::Button::new(
                    egui::RichText::new(format!("{} Clear", ph::X))
                        .size(10.0)
                        .color(egui::Color32::GRAY),
                ))
                .clicked()
            {
                *build_state.lock().unwrap() = BuildState::Idle;
                *selected_diagnostic = None;
            }

            ui.add_space(4.0);

            // "Clean target/" — deletes cached LLVM/cargo build artefacts to
            // recover disk space.  Shown always; especially helpful after a
            // disk-full build failure.
            let is_building = matches!(state, BuildState::Building);
            if ui
                .add_enabled(
                    !is_building,
                    egui::Button::new(
                        egui::RichText::new(format!("{} Clean target/", ph::TRASH))
                            .size(10.0)
                            .color(egui::Color32::from_rgb(200, 160, 80)),
                    ),
                )
                .on_hover_text(
                    "Run `cargo clean` — deletes the target/ directory to free disk space.\n\
                     Crates cached in ~/.cargo are NOT removed; only rebuilt files are re-compiled.",
                )
                .clicked()
            {
                build::start_clean(workspace.clone(), Arc::clone(build_state), ctx.clone());
                *selected_diagnostic = None;
            }
        });
    });

    let BuildState::Done(result) = &state else {
        // For Building/Failed we've shown what we can
        if let BuildState::Failed(msg) = &state {
            ui.separator();

            // ── Special disk-full banner ──────────────────────────────────────
            let is_disk_full = msg.starts_with("[DISK_FULL]");
            if is_disk_full {
                // Orange warning box with an inline "Clean target/" button
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(60, 45, 10))
                    .inner_margin(egui::Margin::same(8))
                    .corner_radius(egui::CornerRadius::same(4))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{} Disk full", ph::WARNING))
                                    .size(12.0)
                                    .color(egui::Color32::from_rgb(250, 190, 60))
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new(
                                    " — the build target/ directory ran out of space.",
                                )
                                .size(11.0)
                                .color(egui::Color32::from_rgb(230, 210, 140)),
                            );
                        });
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(
                                "ESP32 / RISC-V builds produce several GB of LLVM artefacts \
                                 on the first run.  Click the button to delete the target/ \
                                 folder and free that space — crates cached in ~/.cargo are \
                                 NOT removed so the next build only re-compiles changed files.",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_rgb(200, 190, 150)),
                        );
                        ui.add_space(6.0);
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new(format!(
                                    "{} Clean target/  (free space)",
                                    ph::TRASH
                                ))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(255, 210, 80)),
                            ))
                            .clicked()
                        {
                            build::start_clean(
                                workspace.clone(),
                                Arc::clone(build_state),
                                ctx.clone(),
                            );
                            *selected_diagnostic = None;
                        }
                    });
                ui.add_space(4.0);
            }

            // Full error text in a scroll area
            egui::ScrollArea::vertical()
                .id_salt("build_failed_scroll")
                .show(ui, |ui| {
                    // Strip the [DISK_FULL] marker before display
                    let display_msg = msg
                        .strip_prefix("[DISK_FULL] ")
                        .unwrap_or(msg.as_str());
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(display_msg)
                                .size(11.0)
                                .monospace()
                                .color(egui::Color32::from_rgb(230, 90, 80)),
                        )
                        .wrap(),
                    );
                });
        }
        return;
    };

    if result.diagnostics.is_empty() {
        return;
    }

    ui.separator();

    // ── Compact diagnostic list ───────────────────────────────────────────────
    let sel = *selected_diagnostic;

    // If something is selected, split the panel: list on top, detail below
    let list_height = if sel.is_some() {
        ui.available_height() * 0.45
    } else {
        ui.available_height()
    };

    egui::ScrollArea::vertical()
        .id_salt("build_diag_list")
        .max_height(list_height)
        .show(ui, |ui| {
            for (i, diag) in result.diagnostics.iter().enumerate() {
                let is_sel = sel == Some(i);

                let (level_icon, level_color) = if diag.is_error() {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                } else {
                    (ph::WARNING, egui::Color32::from_rgb(210, 170, 40))
                };

                let location = match (diag.file.as_deref(), diag.line) {
                    (Some(f), Some(l)) => format!("{f}:{l}"),
                    (Some(f), None) => f.to_owned(),
                    _ => String::new(),
                };

                let row_bg = if is_sel {
                    egui::Color32::from_rgba_premultiplied(60, 80, 110, 180)
                } else {
                    egui::Color32::TRANSPARENT
                };

                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 18.0),
                    egui::Sense::click(),
                );

                if ui.is_rect_visible(rect) {
                    let painter = ui.painter();
                    painter.rect_filled(rect, 2.0, row_bg);

                    // painter.text() returns the Rect of the rendered text,
                    // letting us advance x without needing &mut Fonts.
                    let cy = rect.center().y;
                    let mut x = rect.left() + 4.0;

                    // Level icon
                    let r = painter.text(
                        egui::pos2(x, cy),
                        egui::Align2::LEFT_CENTER,
                        level_icon,
                        egui::FontId::proportional(11.0),
                        level_color,
                    );
                    x = r.right() + 4.0;

                    // File:line location
                    if !location.is_empty() {
                        let r = painter.text(
                            egui::pos2(x, cy),
                            egui::Align2::LEFT_CENTER,
                            &location,
                            egui::FontId::monospace(10.5),
                            egui::Color32::from_rgb(120, 160, 200),
                        );
                        x = r.right() + 6.0;
                    }

                    // Error code [E0308]
                    if let Some(code) = &diag.code {
                        let r = painter.text(
                            egui::pos2(x, cy),
                            egui::Align2::LEFT_CENTER,
                            format!("[{code}]"),
                            egui::FontId::monospace(10.0),
                            egui::Color32::from_rgb(150, 130, 80),
                        );
                        x = r.right() + 6.0;
                    }

                    // Message text
                    let msg_color = if is_sel {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(210, 210, 220)
                    };
                    painter.text(
                        egui::pos2(x, cy),
                        egui::Align2::LEFT_CENTER,
                        &diag.message,
                        egui::FontId::proportional(11.0),
                        msg_color,
                    );
                }

                if resp.clicked() {
                    *selected_diagnostic = if is_sel { None } else { Some(i) };

                    // Navigate to the file if possible
                    if let Some(file) = &diag.file {
                        let target = match file.as_str() {
                            "src/main.rs" => Some(ProjectFileId::MainRs),
                            "build.rs" => Some(ProjectFileId::BuildRs),
                            "Cargo.toml" => Some(ProjectFileId::CargoToml),
                            _ => None,
                        };
                        if let Some(id) = target {
                            *selected_file = id;
                        }
                    }
                }
            }
        });

    // ── Detail view for selected diagnostic ───────────────────────────────────
    if let Some(idx) = sel {
        if let Some(diag) = result.diagnostics.get(idx) {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("build_diag_detail")
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&diag.rendered)
                                .size(11.0)
                                .monospace()
                                .color(egui::Color32::from_rgb(220, 215, 200)),
                        )
                        .wrap(),
                    );
                });
        }
    }
}

// ── rust-analyzer tab ─────────────────────────────────────────────────────────

fn show_ra_tab(
    ui: &mut egui::Ui,
    lsp_state: &Arc<Mutex<lsp::LspState>>,
    selected: &mut Option<usize>,
    selected_file: &mut ProjectFileId,
) {
    // Extract everything we need while holding the lock, then drop it
    // before we start drawing so there's no risk of a deadlock.
    let (status, total_err, total_warn, all_diags, failed_msg) = {
        let lsp = lsp_state.lock().unwrap();
        let failed_msg = if let lsp::LspStatus::Failed(ref m) = lsp.status {
            Some(m.clone())
        } else {
            None
        };
        // Flatten all diagnostics into (rel_path, LspDiagnostic) pairs
        let mut flat: Vec<(String, lsp::LspDiagnostic)> = lsp
            .diagnostics
            .iter()
            .flat_map(|(path, diags)| diags.iter().map(move |d| (path.clone(), d.clone())))
            .collect();
        // Errors first, then warnings
        flat.sort_by_key(|(_, d)| (d.severity != lsp::DiagSeverity::Error, d.line));
        (
            lsp.status.clone(),
            lsp.total_errors(),
            lsp.total_warnings(),
            flat,
            failed_msg,
        )
    };

    // ── Status bar ────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let (icon, text, color) = match &status {
            lsp::LspStatus::Stopped => (
                ph::PLUGS,
                "rust-analyzer not running".to_owned(),
                egui::Color32::DARK_GRAY,
            ),
            lsp::LspStatus::Starting => (
                ph::CIRCLE_NOTCH,
                "rust-analyzer starting…".to_owned(),
                egui::Color32::from_rgb(180, 180, 80),
            ),
            lsp::LspStatus::Indexing => (
                ph::CIRCLE_NOTCH,
                "Indexing project…".to_owned(),
                egui::Color32::from_rgb(180, 180, 80),
            ),
            lsp::LspStatus::Ready if total_err > 0 => (
                ph::X_CIRCLE,
                format!(
                    "{} error{}{}",
                    total_err,
                    if total_err == 1 { "" } else { "s" },
                    if total_warn > 0 {
                        format!(
                            ",  {} warning{}",
                            total_warn,
                            if total_warn == 1 { "" } else { "s" }
                        )
                    } else {
                        String::new()
                    }
                ),
                egui::Color32::from_rgb(230, 90, 80),
            ),
            lsp::LspStatus::Ready if total_warn > 0 => (
                ph::WARNING,
                format!(
                    "{} warning{}",
                    total_warn,
                    if total_warn == 1 { "" } else { "s" }
                ),
                egui::Color32::from_rgb(230, 190, 50),
            ),
            lsp::LspStatus::Ready => (
                ph::CHECK_CIRCLE,
                "No issues — rust-analyzer ready".to_owned(),
                egui::Color32::from_rgb(80, 200, 100),
            ),
            lsp::LspStatus::Failed(_) => (
                ph::X_CIRCLE,
                "rust-analyzer failed to start".to_owned(),
                egui::Color32::from_rgb(230, 90, 80),
            ),
        };

        ui.label(egui::RichText::new(icon).size(13.0).color(color));
        ui.label(egui::RichText::new(text).size(12.0).color(color).strong());
    });

    // Spinner repaint
    if matches!(status, lsp::LspStatus::Starting | lsp::LspStatus::Indexing) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    }

    // Failed detail
    if let Some(msg) = failed_msg {
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("ra_failed_scroll")
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&msg)
                            .size(11.0)
                            .monospace()
                            .color(egui::Color32::from_rgb(230, 90, 80)),
                    )
                    .wrap(),
                );
            });
        return;
    }

    if all_diags.is_empty() {
        return;
    }

    ui.separator();

    // ── Diagnostic list ───────────────────────────────────────────────────────
    let sel = *selected;
    let list_height = if sel.is_some() {
        ui.available_height() * 0.45
    } else {
        ui.available_height()
    };

    egui::ScrollArea::vertical()
        .id_salt("ra_diag_list")
        .max_height(list_height)
        .show(ui, |ui| {
            for (i, (path, diag)) in all_diags.iter().enumerate() {
                let is_sel = sel == Some(i);

                let (level_icon, level_color) = match diag.severity {
                    lsp::DiagSeverity::Error => {
                        (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                    }
                    lsp::DiagSeverity::Warning => {
                        (ph::WARNING, egui::Color32::from_rgb(210, 170, 40))
                    }
                    lsp::DiagSeverity::Info | lsp::DiagSeverity::Hint => {
                        (ph::INFO, egui::Color32::from_rgb(80, 140, 210))
                    }
                };

                let location = format!("{}:{}", path, diag.line);

                let row_bg = if is_sel {
                    egui::Color32::from_rgba_premultiplied(60, 80, 110, 180)
                } else {
                    egui::Color32::TRANSPARENT
                };

                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 18.0),
                    egui::Sense::click(),
                );

                if ui.is_rect_visible(rect) {
                    let painter = ui.painter();
                    painter.rect_filled(rect, 2.0, row_bg);

                    let cy = rect.center().y;
                    let mut x = rect.left() + 4.0;

                    // Severity icon
                    let r = painter.text(
                        egui::pos2(x, cy),
                        egui::Align2::LEFT_CENTER,
                        level_icon,
                        egui::FontId::proportional(11.0),
                        level_color,
                    );
                    x = r.right() + 4.0;

                    // file:line
                    let r = painter.text(
                        egui::pos2(x, cy),
                        egui::Align2::LEFT_CENTER,
                        &location,
                        egui::FontId::monospace(10.5),
                        egui::Color32::from_rgb(120, 160, 200),
                    );
                    x = r.right() + 6.0;

                    // Error code [E0308]
                    if let Some(code) = &diag.code {
                        let r = painter.text(
                            egui::pos2(x, cy),
                            egui::Align2::LEFT_CENTER,
                            format!("[{code}]"),
                            egui::FontId::monospace(10.0),
                            egui::Color32::from_rgb(150, 130, 80),
                        );
                        x = r.right() + 6.0;
                    }

                    // Message
                    let msg_color = if is_sel {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(210, 210, 220)
                    };
                    painter.text(
                        egui::pos2(x, cy),
                        egui::Align2::LEFT_CENTER,
                        &diag.message,
                        egui::FontId::proportional(11.0),
                        msg_color,
                    );
                }

                if resp.clicked() {
                    *selected = if is_sel { None } else { Some(i) };

                    // Navigate to file
                    let target = match path.as_str() {
                        "src/main.rs" => Some(ProjectFileId::MainRs),
                        "build.rs" => Some(ProjectFileId::BuildRs),
                        "Cargo.toml" => Some(ProjectFileId::CargoToml),
                        _ => None,
                    };
                    if let Some(id) = target {
                        *selected_file = id;
                    }
                }
            }
        });

    // ── Detail view ───────────────────────────────────────────────────────────
    if let Some(idx) = sel {
        if let Some((_, diag)) = all_diags.get(idx) {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("ra_diag_detail")
                .show(ui, |ui| {
                    // Header: severity + code + location
                    ui.horizontal(|ui| {
                        let (sev_icon, sev_col) = match diag.severity {
                            lsp::DiagSeverity::Error => {
                                (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                            }
                            lsp::DiagSeverity::Warning => {
                                (ph::WARNING, egui::Color32::from_rgb(210, 170, 40))
                            }
                            _ => (ph::INFO, egui::Color32::from_rgb(80, 140, 210)),
                        };
                        ui.label(egui::RichText::new(sev_icon).size(13.0).color(sev_col));
                        if let Some(code) = &diag.code {
                            ui.label(
                                egui::RichText::new(format!("[{code}]"))
                                    .size(11.0)
                                    .monospace()
                                    .color(egui::Color32::from_rgb(180, 155, 80)),
                            );
                        }
                        ui.label(
                            egui::RichText::new(format!("line {}  col {}", diag.line, diag.col))
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        );
                    });

                    // Full message body
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&diag.message)
                                .size(11.5)
                                .color(egui::Color32::from_rgb(220, 215, 200)),
                        )
                        .wrap(),
                    );
                });
        }
    }
}

// ── Required Tools tab ────────────────────────────────────────────────────────

// ── Dark IDE theme ────────────────────────────────────────────────────────────

/// Apply a consistent dark IDE theme to the entire application.
///
/// Palette (One Dark / VSCode Dark+):
///   bg0  #1e2127  — darkest  (extreme bg, gutter)
///   bg1  #21252b  — panel / window fill
///   bg2  #282c34  — widget normal fill
///   bg3  #2c313a  — widget hovered
///   bg4  #3e4451  — widget active / selected
///   fg0  #abb2bf  — primary text
///   fg1  #d0d7e0  — strong / heading text
///   acc  #528bff  — selection / focus accent
///   sep  #3a3f4b  — borders / separators
fn apply_dark_theme(ctx: &egui::Context) {
    use egui::Color32;

    // Pin theme to Dark so an OS appearance change can't flip it to light.
    ctx.set_theme(egui::Theme::Dark);

    // ── Start from egui's built-in dark base ─────────────────────────────────
    let mut vis = egui::Visuals::dark();

    // ── Palette ──────────────────────────────────────────────────────────────
    let bg0 = Color32::from_rgb(0x1e, 0x21, 0x27);
    let bg1 = Color32::from_rgb(0x21, 0x25, 0x2b);
    let bg2 = Color32::from_rgb(0x28, 0x2c, 0x34);
    let bg3 = Color32::from_rgb(0x2c, 0x31, 0x3a);
    let bg4 = Color32::from_rgb(0x3e, 0x44, 0x51);
    let sep = Color32::from_rgb(0x3a, 0x3f, 0x4b);
    let fg0 = Color32::from_rgb(0xab, 0xb2, 0xbf);
    let fg1 = Color32::from_rgb(0xd0, 0xd7, 0xe0);
    let acc    = Color32::from_rgb(0x52, 0x8b, 0xff);
    let acc_bg = Color32::from_rgba_unmultiplied(0x52, 0x8b, 0xff, 50);
    let cr4    = egui::CornerRadius::same(4); // CornerRadius uses u8

    // ── Backgrounds ──────────────────────────────────────────────────────────
    vis.window_fill      = bg1;
    vis.panel_fill       = bg1;
    vis.faint_bg_color   = bg0;
    vis.extreme_bg_color = bg0;
    vis.code_bg_color    = Color32::from_rgb(0x1a, 0x1d, 0x23);

    // ── Window chrome ────────────────────────────────────────────────────────
    vis.window_stroke         = egui::Stroke::new(1.0, sep);
    vis.window_corner_radius  = egui::CornerRadius::same(6);
    vis.menu_corner_radius    = egui::CornerRadius::same(5);
    vis.window_shadow = egui::Shadow {
        offset: [0, 4],
        blur:   14,
        spread: 0,
        color:  Color32::from_black_alpha(130),
    };
    vis.popup_shadow = egui::Shadow {
        offset: [0, 2],
        blur:   8,
        spread: 0,
        color:  Color32::from_black_alpha(100),
    };

    // ── Widgets: noninteractive ───────────────────────────────────────────────
    vis.widgets.noninteractive.bg_fill      = bg1;
    vis.widgets.noninteractive.weak_bg_fill = bg1;
    vis.widgets.noninteractive.bg_stroke    = egui::Stroke::new(1.0, sep);
    vis.widgets.noninteractive.fg_stroke    = egui::Stroke::new(1.0, fg0);
    vis.widgets.noninteractive.corner_radius = cr4;

    // ── Widgets: inactive ────────────────────────────────────────────────────
    vis.widgets.inactive.bg_fill      = bg2;
    vis.widgets.inactive.weak_bg_fill = bg2;
    vis.widgets.inactive.bg_stroke    = egui::Stroke::new(1.0, sep);
    vis.widgets.inactive.fg_stroke    = egui::Stroke::new(1.5, fg0);
    vis.widgets.inactive.corner_radius = cr4;

    // ── Widgets: hovered ─────────────────────────────────────────────────────
    vis.widgets.hovered.bg_fill      = bg3;
    vis.widgets.hovered.weak_bg_fill = bg3;
    vis.widgets.hovered.bg_stroke    = egui::Stroke::new(1.0, Color32::from_rgb(0x60, 0x6a, 0x80));
    vis.widgets.hovered.fg_stroke    = egui::Stroke::new(1.5, fg1);
    vis.widgets.hovered.corner_radius = cr4;

    // ── Widgets: active (pressed) ────────────────────────────────────────────
    vis.widgets.active.bg_fill      = bg4;
    vis.widgets.active.weak_bg_fill = bg4;
    vis.widgets.active.bg_stroke    = egui::Stroke::new(1.0, acc);
    vis.widgets.active.fg_stroke    = egui::Stroke::new(2.0, fg1);
    vis.widgets.active.corner_radius = cr4;

    // ── Widgets: open (combo-box open state) ─────────────────────────────────
    vis.widgets.open.bg_fill      = bg4;
    vis.widgets.open.weak_bg_fill = bg4;
    vis.widgets.open.bg_stroke    = egui::Stroke::new(1.0, acc);
    vis.widgets.open.fg_stroke    = egui::Stroke::new(1.5, fg1);
    vis.widgets.open.corner_radius = cr4;

    // ── Text / selection ─────────────────────────────────────────────────────
    vis.override_text_color = Some(fg0);
    vis.selection.bg_fill   = acc_bg;
    vis.selection.stroke    = egui::Stroke::new(1.0, acc);

    // ── Misc ─────────────────────────────────────────────────────────────────
    vis.hyperlink_color  = Color32::from_rgb(0x61, 0xaf, 0xef);
    vis.warn_fg_color    = Color32::from_rgb(0xe5, 0xc0, 0x7b);
    vis.error_fg_color   = Color32::from_rgb(0xe0, 0x6c, 0x75);

    ctx.set_visuals(vis);

    // ── Global style tweaks ──────────────────────────────────────────────────
    let mut style = (*ctx.style()).clone();

    style.spacing.item_spacing   = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    style.spacing.window_margin  = egui::Margin::same(8);
    style.spacing.indent         = 14.0;

    use egui::TextStyle;
    style.text_styles.insert(
        TextStyle::Heading,
        egui::FontId::new(14.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Body,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Button,
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        egui::FontId::new(13.0, egui::FontFamily::Monospace),
    );

    ctx.set_style(style);
}

// ── LSP completion helpers ────────────────────────────────────────────────────

/// Convert a character offset into a (line, UTF-16-column) pair for LSP.
///
/// LSP `Position.character` is a count of UTF-16 code units from the start
/// of the line, NOT a count of Unicode chars.  For the BMP (U+0000–U+FFFF,
/// which includes all ASCII + Romanian diacritics) each char = 1 unit, so the
/// result is the same.  Emoji and other non-BMP chars are 2 units each.
fn lsp_cursor_pos(text: &str, char_idx: usize) -> (u32, u32) {
    let mut line: u32 = 0;
    let mut col: u32  = 0;
    for (i, c) in text.chars().enumerate() {
        if i >= char_idx {
            break;
        }
        if c == '\n' {
            line += 1;
            col   = 0;
        } else {
            col += c.len_utf16() as u32;
        }
    }
    (line, col)
}

/// Extract the prefix typed between `trigger_idx` and `cursor_idx`.
///
/// Used for live filtering of the completion popup: after the user triggers
/// completion (via `.` or Ctrl+Space) any additional characters they type
/// narrow the visible list.  Returns an empty string when the cursor hasn't
/// moved past the trigger point yet.
fn lsp_completion_prefix(text: &str, trigger_idx: usize, cursor_idx: usize) -> String {
    if cursor_idx <= trigger_idx {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let start = trigger_idx.min(chars.len());
    let end   = cursor_idx.min(chars.len());
    // Only take identifier characters (letters, digits, _).  A dot or space
    // means the user has moved to a new expression — we'll rely on the delta
    // check in the trigger section to close the popup in that case.
    chars[start..end]
        .iter()
        .take_while(|&&c| c.is_alphanumeric() || c == '_')
        .collect()
}

/// Return the char-index of the first character of the identifier that ends at `end_idx`.
fn lsp_word_start(text: &str, end_idx: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let end = end_idx.min(chars.len());
    let mut i = end;
    while i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
        i -= 1;
    }
    i
}

// ── Inline diagnostics helpers ────────────────────────────────────────────────

/// Convert an LSP position (1-based line, 1-based column in UTF-16 units)
/// to a char index suitable for `galley.pos_from_cursor(CCursor::new(idx))`.
///
/// LSP columns are UTF-16 code-unit offsets from line start.  For ASCII and
/// BMP characters (including Romanian diacritics) each char is 1 unit.
fn lsp_pos_to_char_idx(text: &str, line_1: u32, col_1: u32) -> usize {
    let want_line = line_1.saturating_sub(1) as usize;
    let want_utf16_col = col_1.saturating_sub(1) as usize;
    let mut cur_line     = 0usize;
    let mut utf16_col    = 0usize;
    let mut char_idx     = 0usize;
    for c in text.chars() {
        if cur_line == want_line && utf16_col >= want_utf16_col {
            return char_idx;
        }
        if c == '\n' {
            if cur_line == want_line {
                // Column is past end of line — clamp to the newline position.
                return char_idx;
            }
            cur_line += 1;
            utf16_col = 0;
        } else {
            utf16_col += c.len_utf16();
        }
        char_idx += 1;
    }
    char_idx
}

/// Draw a wavy (zigzag) underline between `x_start` and `x_end` at height `y`.
///
/// Each segment alternates up/down by `AMP` pixels with a horizontal step of
/// `STEP` pixels, producing the classic "squiggly" error underline appearance.
fn draw_wavy_underline(
    painter: &egui::Painter,
    x_start: f32,
    x_end:   f32,
    y:       f32,
    color:   egui::Color32,
) {
    const STEP: f32 = 3.0;
    const AMP:  f32 = 1.5;
    if x_end <= x_start { return; }
    let stroke = egui::Stroke::new(1.2, color);
    let mut x  = x_start;
    let mut up = true;
    while x < x_end {
        let x2 = (x + STEP).min(x_end);
        let (y1, y2) = if up { (y, y + AMP) } else { (y + AMP, y) };
        painter.line_segment([egui::pos2(x, y1), egui::pos2(x2, y2)], stroke);
        x  = x2;
        up = !up;
    }
}

/// Map an LSP CompletionItemKind number to a short (3-char) icon string.
fn lsp_kind_icon(kind: u8) -> &'static str {
    match kind {
        2 | 3 => "fn ",   // Method / Function
        4      => "ctr",  // Constructor
        5      => "fld",  // Field
        6      => "var",  // Variable
        7      => "cls",  // Class
        8      => "int",  // Interface
        9      => "mod",  // Module
        13     => "enm",  // Enum
        14     => "kwd",  // Keyword
        20     => "enm",  // EnumMember
        21     => "con",  // Constant
        22     => "str",  // Struct
        25     => "typ",  // TypeParameter
        _      => "   ",
    }
}

fn show_tools_tab(
    ui: &mut egui::Ui,
    tools_state: &Arc<Mutex<required_tools::ToolsState>>,
    ctx: &egui::Context,
) {
    use required_tools::ToolStatus;

    // Snapshot all state before rendering (avoids holding the lock during draw)
    let (rows, any_busy, missing_count, log) = {
        let s = tools_state.lock().unwrap();
        (
            s.snapshot(),
            s.any_busy(),
            s.missing_installable_count(),
            s.log.clone(),
        )
    };

    // ── Toolbar ───────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !any_busy,
                egui::Button::new(
                    egui::RichText::new(format!("{} Check All", ph::MAGNIFYING_GLASS)).size(11.0),
                ),
            )
            .on_hover_text("Check every tool / target in the list")
            .clicked()
        {
            required_tools::start_check_all(Arc::clone(tools_state), ctx.clone());
        }

        let install_label = if missing_count > 0 {
            format!("Install Missing ({})", missing_count)
        } else {
            "Install Missing".to_string()
        };
        if ui
            .add_enabled(
                !any_busy && missing_count > 0,
                egui::Button::new(egui::RichText::new(install_label).size(11.0)),
            )
            .on_hover_text("Auto-install every missing tool that supports it")
            .clicked()
        {
            required_tools::start_install_missing(Arc::clone(tools_state), ctx.clone());
        }
    });

    ui.add_space(2.0);
    ui.separator();

    // ── Tools grid ────────────────────────────────────────────────────────────
    let available_h = ui.available_height();
    // Reserve ~30 % of the panel for the log area (min 60 px, max 110 px)
    let log_h = (available_h * 0.30).clamp(60.0, 110.0);
    let grid_h = (available_h - log_h - 20.0).max(40.0);

    egui::ScrollArea::vertical()
        .id_salt("tools_grid_scroll")
        .max_height(grid_h)
        .show(ui, |ui| {
            egui::Grid::new("tools_grid")
                .num_columns(5)
                .striped(true)
                .min_col_width(50.0)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    // Header row
                    let hdr = |text: &str| {
                        egui::RichText::new(text)
                            .strong()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(150, 160, 180))
                    };
                    ui.label(hdr("Tool"));
                    ui.label(hdr("For"));
                    ui.label(hdr("Status"));
                    ui.label(hdr("Version"));
                    ui.label(hdr("Action"));
                    ui.end_row();

                    for (idx, row) in rows.iter().enumerate() {
                        // ── Tool name + description tooltip ────────────────
                        ui.label(
                            egui::RichText::new(row.name)
                                .monospace()
                                .size(10.5),
                        )
                        .on_hover_text(row.description);

                        // ── Toolchain column ──────────────────────────────
                        let tc_label = match &row.toolchain {
                            None => "All",
                            Some(ToolchainKind::RustEmbedded) => "STM32",
                            Some(ToolchainKind::EspRust) => "ESP32-C3",
                            Some(ToolchainKind::SdccC) => "SDCC",
                        };
                        ui.label(
                            egui::RichText::new(tc_label)
                                .size(10.5)
                                .color(egui::Color32::GRAY),
                        );

                        // ── Status badge ───────────────────────────────────
                        ui.label(
                            egui::RichText::new(row.status.label())
                                .size(10.5)
                                .color(row.status.color()),
                        );

                        // ── Version string ─────────────────────────────────
                        let ver = match &row.status {
                            ToolStatus::Ok(v) => v.as_str(),
                            _ => "—",
                        };
                        ui.label(
                            egui::RichText::new(ver)
                                .monospace()
                                .size(10.0)
                                .color(egui::Color32::from_rgb(140, 150, 170)),
                        );

                        // ── Action buttons ─────────────────────────────────
                        let busy = row.status.is_busy();
                        ui.horizontal(|ui| {
                            // Check button — always shown, disabled while busy
                            if ui
                                .add_enabled(
                                    !busy,
                                    egui::Button::new(
                                        egui::RichText::new(ph::MAGNIFYING_GLASS).size(11.0),
                                    )
                                    .small(),
                                )
                                .on_hover_text("Re-check this tool")
                                .clicked()
                            {
                                required_tools::start_check(
                                    idx,
                                    Arc::clone(tools_state),
                                    ctx.clone(),
                                );
                            }

                            // Install button — only when missing/failed AND auto-installable
                            if row.can_auto_install
                                && matches!(
                                    row.status,
                                    ToolStatus::Missing | ToolStatus::Failed(_)
                                )
                            {
                                if ui
                                    .add_enabled(
                                        !busy,
                                        egui::Button::new(
                                            egui::RichText::new("Install").size(10.5),
                                        )
                                        .small(),
                                    )
                                    .on_hover_text("Auto-install this tool")
                                    .clicked()
                                {
                                    required_tools::start_install(
                                        idx,
                                        Arc::clone(tools_state),
                                        ctx.clone(),
                                    );
                                }
                            }

                            // Manual URL link — for tools without auto-install
                            if !row.can_auto_install
                                && matches!(
                                    row.status,
                                    ToolStatus::Missing
                                        | ToolStatus::Unknown
                                        | ToolStatus::Failed(_)
                                )
                            {
                                ui.hyperlink_to(
                                    egui::RichText::new("Get…")
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(100, 160, 220)),
                                    row.manual_url,
                                )
                                .on_hover_text(row.manual_url);
                            }
                        });

                        ui.end_row();
                    }
                });
        });

    // ── Log area ──────────────────────────────────────────────────────────────
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("tools_log_scroll")
        .stick_to_bottom(true)
        .max_height(log_h)
        .show(ui, |ui| {
            if log.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "Click \"Check All\" to verify your toolchain setup.\n\
                         Use \"Install\" buttons to auto-install missing components.",
                    )
                    .size(10.5)
                    .color(egui::Color32::GRAY),
                );
                return;
            }
            for line in &log {
                let color = if line.starts_with("  ✔") || line.starts_with("✔") {
                    egui::Color32::from_rgb(80, 200, 100)
                } else if line.starts_with("  ✘") {
                    egui::Color32::from_rgb(220, 80, 70)
                } else if line.starts_with("▶") {
                    egui::Color32::from_rgb(100, 180, 255)
                } else {
                    egui::Color32::from_rgb(175, 180, 192)
                };
                ui.label(
                    egui::RichText::new(line.as_str())
                        .monospace()
                        .size(10.0)
                        .color(color),
                );
            }
        });
}
