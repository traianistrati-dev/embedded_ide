use crate::build::{self, BuildState};
use crate::lsp::{self, LspStatus};
use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::mcu_catalog::McuType;
use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;
use crate::panels::mcu_module::pin_module::pin::Pin;
use crate::panels::mcu_module::pin_module::pin_function::PinFunction;
use crate::panels::mcu_module::project_gen::{self, ProjectFiles};
use eframe::egui;
use egui_code_editor::{CodeEditor, ColorTheme, Syntax};
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
    /// Stored as a plain String (not PathBuf) to avoid any platform
    /// path-separator issues with eframe's JSON storage on Windows.
    #[serde(default)]
    project_name: Option<String>,
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
    /// Display name of the last opened/exported project (shown in the panel heading).
    project_name: Option<String>,
    // ── Filesystem watcher ────────────────────────────────────────────────────
    /// Receives filesystem events from the background notify thread.
    /// Polled every frame; used to detect files created/removed by external tools.
    fs_rx: Option<std::sync::mpsc::Receiver<notify::Result<notify::Event>>>,
    /// Kept alive so the watcher thread lives as long as the app.
    _fs_watcher: Option<notify::RecommendedWatcher>,
}

impl AppIde {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
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

        Self {
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
            project_name: persisted.project_name,
            fs_rx: Some(fs_rx),
            _fs_watcher: watcher.ok(),
        }
    }

    fn init_mcu(mcu_type: &McuType) -> Option<Mcu> {
        match mcu_type {
            McuType::Stm32f103c8t6 => Some(create_stm32f103c8tx()),
            _ => None,
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
    /// Called whenever a pin receives a non-Unset function.
    /// Ensures `pins/` folder, `pins/mod.rs`, and `pins/pin{N}_{name}.rs` exist,
    /// and keeps the `pub mod` declaration in `mod.rs` up to date.
    fn ensure_pin_files(
        files:   &mut Vec<(String, String)>,
        folders: &mut Vec<String>,
        pin_num:  usize,
        pin_name: &str,
        func:    &PinFunction,
    ) {
        let slug      = format!("pin{}_{}", pin_num, pin_name.to_lowercase());
        let folder    = "pins".to_string();
        let mod_path  = "pins/mod.rs".to_string();
        let file_path = format!("pins/{slug}.rs");
        let mod_decl  = format!("pub mod {slug};");

        // 1. Ensure pins/ folder is registered
        if !folders.contains(&folder) {
            folders.push(folder);
        }

        // 2. Ensure pins/mod.rs exists (empty is fine — we append below)
        if !files.iter().any(|(p, _)| p == &mod_path) {
            files.push((mod_path.clone(), String::new()));
        }

        // 3. Ensure the per-pin source file exists (only created, never overwritten)
        if !files.iter().any(|(p, _)| p == &file_path) {
            let content = format!(
                "// Pin {pin_num} — {pin_name}\n\
                 // Function: {func_label}\n\
                 \n\
                 // TODO: implement pin {pin_num} ({pin_name}) logic here\n",
                pin_num    = pin_num,
                pin_name   = pin_name,
                func_label = func.label(),
            );
            files.push((file_path, content));
        }

        // 4. Add `pub mod` declaration to pins/mod.rs if not already present
        if let Some((_, mod_content)) = files.iter_mut().find(|(p, _)| p == &mod_path) {
            if !mod_content.contains(&mod_decl) {
                if !mod_content.is_empty() && !mod_content.ends_with('\n') {
                    mod_content.push('\n');
                }
                mod_content.push_str(&mod_decl);
                mod_content.push('\n');
            }
        }
    }

    /// Creates the initial `pins/` scaffold (folder + empty mod.rs).
    /// Called once when "New Project" is confirmed so the tree is
    /// pre-populated before any pin is configured.
    fn init_pins_scaffold(
        files:   &mut Vec<(String, String)>,
        folders: &mut Vec<String>,
    ) {
        let folder   = "pins".to_string();
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

                // Show project name under the heading when one is loaded
                if let Some(name) = &self.project_name {
                    ui.label(
                        egui::RichText::new(format!("  {}", name))
                            .size(10.5)
                            .color(egui::Color32::from_rgb(140, 160, 180))
                            .italics(),
                    );
                }
                ui.separator();

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

        // "New Project" → ask for confirmation
        if new_project_clicked {
            self.confirm_new_project = true;
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
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.add_space(4.0);
                    ui.label("This will clear all user files and folders.");
                    ui.label(
                        egui::RichText::new("The action cannot be undone.")
                            .color(egui::Color32::from_rgb(220, 160, 60)),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                egui::RichText::new(format!("{} New Project", ph::NOTE_PENCIL))
                                    .color(egui::Color32::from_rgb(220, 80, 60)),
                            )
                            .clicked()
                        {
                            self.user_src_files.clear();
                            self.user_src_folders.clear();
                            self.selected_file = ProjectFileId::MainRs;
                            self.project_name = None;
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
                    let show_panel = cargo_has || lsp_active;

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
                                    &self.build_state,
                                    &self.lsp_state,
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

                let editor_resp = CodeEditor::default()
                    .id_source(editor_id)
                    .with_rows(50)
                    .with_fontsize(13.0)
                    .with_theme(ColorTheme::GRUVBOX)
                    .with_numlines(true)
                    .show(ui, &mut display_code, &display_syntax);

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

            // Chip selector
            ui.horizontal(|ui| {
                ui.label("Chip:");
                let prev_type = self.selected_mcu_type.clone();

                egui::ComboBox::from_id_salt("mcu_type_selector")
                    .selected_text(self.selected_mcu_type.label())
                    .show_ui(ui, |ui| {
                        for mcu_type in McuType::all() {
                            let label = if mcu_type.is_supported() {
                                mcu_type.label().to_string()
                            } else {
                                format!("{} — coming soon", mcu_type.label())
                            };
                            ui.selectable_value(&mut self.selected_mcu_type, mcu_type, label);
                        }
                    });

                ui.label(
                    egui::RichText::new(self.selected_mcu_type.family())
                        .color(egui::Color32::GRAY)
                        .size(11.0),
                );

                if prev_type != self.selected_mcu_type {
                    self.mcu = Self::init_mcu(&self.selected_mcu_type);
                    // Build a fresh file with new stubs — user had no edits yet
                    // for this MCU type, and pin layout is completely different.
                    self.generated_code = self
                        .mcu
                        .as_ref()
                        .map(|m| m.fresh_main_rs())
                        .unwrap_or_default();
                    self.active_tab = McuTab::Pins;
                    self.selected_file = ProjectFileId::MainRs;
                    // Stop old RA session; it will auto-restart on the next frame
                    // for supported chips (generation counter prevents stale updates).
                    self.lsp_state.lock().unwrap().reset();
                    self.lsp_selected_diagnostic = None;
                }
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

                    // When a pin receives a real function, auto-create
                    // pins/<slug>.rs and register it in pins/mod.rs.
                    if let Some((num, name, func)) = pin_changed {
                        if func != PinFunction::Unset {
                            Self::ensure_pin_files(
                                &mut self.user_src_files,
                                &mut self.user_src_folders,
                                num,
                                &name,
                                &func,
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
    let normal = egui::Color32::from_rgb(100, 105, 115);

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
            .color(normal),
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
            .color(normal),
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
                        .color(normal),
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
    file_row(
        ui,
        4.0,
        "build.rs",
        ProjectFileId::BuildRs,
        selected,
        build_result,
        lsp,
    );
    file_row(
        ui,
        4.0,
        "Cargo.toml",
        ProjectFileId::CargoToml,
        selected,
        build_result,
        lsp,
    );
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
    build_state: &Arc<Mutex<BuildState>>,
    lsp_state: &Arc<Mutex<lsp::LspState>>,
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
    });

    ui.separator();

    // ── Tab content ───────────────────────────────────────────────────────────
    match tab {
        BuildPanelTab::Cargo => {
            show_cargo_tab(ui, build_state, cargo_sel, selected_file);
        }
        BuildPanelTab::RustAnalyzer => {
            show_ra_tab(ui, lsp_state, lsp_sel, selected_file);
        }
    }
}

// ── Cargo Check tab ───────────────────────────────────────────────────────────

fn show_cargo_tab(
    ui: &mut egui::Ui,
    build_state: &Arc<Mutex<BuildState>>,
    selected_diagnostic: &mut Option<usize>,
    selected_file: &mut ProjectFileId,
) {
    let state = build_state.lock().unwrap().clone();

    // ── Status bar ────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let (icon, text, color) = match &state {
            BuildState::Idle => return,
            BuildState::Building => (
                ph::HAMMER,
                "Building…".to_owned(),
                egui::Color32::from_rgb(180, 180, 180),
            ),
            BuildState::Failed(msg) => (
                ph::X_CIRCLE,
                format!("Build failed: {}", msg.lines().next().unwrap_or(msg)),
                egui::Color32::from_rgb(230, 90, 80),
            ),
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

        // Clear button
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
        });
    });

    let BuildState::Done(result) = &state else {
        // For Building/Failed we've shown what we can
        if let BuildState::Failed(msg) = &state {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("build_failed_scroll")
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(msg)
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
