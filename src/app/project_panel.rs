//! "Project" panel — docked on the FAR RIGHT (the [Editor][MCU][Project]
//! layout) — header toolbar (Tools dropdown: Save / Open / New / Rename) plus the file
//! tree body (delegated to [`crate::project_tree`]).
//!
//! Rendering is an inherent method on [`AppIde`] so it can read/write the
//! many `self` fields the tree needs.  Button presses are returned to the
//! caller as [`ProjectPanelSignals`]; the caller acts on them after the panel
//! closure ends (opening folders, showing modals, exporting).

use super::AppIde;
use crate::panels::mcu_module::project_gen::ProjectFiles;
use crate::project_tree::gui::show_project_tree as show_project_tree_panel;
use eframe::egui;
use egui_phosphor::regular as ph;

/// Toolbar button presses collected inside the panel, acted on by the caller.
pub(super) struct ProjectPanelSignals {
    pub open_clicked: bool,
    pub new_clicked: bool,
    pub save_clicked: bool,
    /// Folder (relative to `src/`) the user asked to extract into its own crate.
    pub extract_folder: Option<String>,
    /// The LIBRARIES "+" button was clicked — create an empty library crate.
    pub new_library: bool,
    /// The LIBRARIES "clone from git" button was clicked.
    pub clone_library: bool,
    /// `(crate dir, is_rename)` when a library's pen / trash icon was clicked.
    pub library_action: Option<(String, bool)>,
    /// A DETACHED library the user asked to promote into the workspace.
    pub add_to_workspace: Option<String>,
    /// A member library the user asked to remove from the workspace (keep files).
    pub detach_from_workspace: Option<String>,
    /// `user_src_files` index to show READ-ONLY in the Reference tab.
    pub open_reference: Option<usize>,
}

impl AppIde {
    /// The current long-running activity for the bottom status bar:
    /// `(show_spinner, label, colour)`, or `None` when idle. Priority: save >
    /// build > flash > rust-analyzer; otherwise the last save result (✓ / ✗)
    /// while it's still flashing. Rendered in the bottom bar (see `app::ui`).
    pub(super) fn activity_status(&self) -> Option<(bool, String, egui::Color32)> {
        let amber = egui::Color32::from_rgb(220, 180, 70);
        let blue = egui::Color32::from_rgb(100, 170, 240);

        // The background LSP flush is part of the user's "save" — same label,
        // so the busy chain (save worker → flush → check) has no status gap.
        if self.save_in_progress.is_some()
            || self
                .lsp_flush_in_flight
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return Some((true, "Saving…".to_owned(), amber));
        }
        if matches!(
            *self.build_state.lock().unwrap(),
            crate::build::BuildState::Building
        ) {
            return Some((true, "Building…".to_owned(), blue));
        }
        if matches!(
            *self.clippy_state.lock().unwrap(),
            crate::build::BuildState::Building
        ) {
            return Some((true, "Running clippy…".to_owned(), amber));
        }
        let dfu_busy = self.dfu_state.lock().unwrap().is_busy();
        let ocd_busy = self.openocd_state.lock().unwrap().is_busy();
        let esp_busy = self.espflash_state.lock().unwrap().is_busy();
        if dfu_busy || ocd_busy || esp_busy {
            return Some((true, "Flashing…".to_owned(), blue));
        }
        if let Some(op) = self.git.state.lock().unwrap().busy {
            return Some((true, format!("Git: {op}…"), amber));
        }
        {
            let lsp = self.lsp_state.lock().unwrap();
            match lsp.status {
                crate::lsp::LspStatus::Starting | crate::lsp::LspStatus::Indexing => {
                    return Some((true, "Indexing…".to_owned(), amber));
                }
                crate::lsp::LspStatus::Ready if lsp.checking || lsp.flycheck_pending() => {
                    // Live elapsed seconds — makes the post-save flycheck tail
                    // (the "save takes 20s" perception) visible and measurable.
                    // `flycheck_pending` covers the QUEUE phase (didSave →
                    // cargo start), previously a status GAP with no spinner —
                    // and therefore no scheduled repaint to keep frames coming.
                    let label = match lsp.checking_elapsed_secs() {
                        Some(s) if s >= 1 => format!("Checking… {s}s"),
                        _ => "Checking…".to_owned(),
                    };
                    return Some((true, label, amber));
                }
                _ => {}
            }
        }
        if self
            .export_status_until
            .is_some_and(|t| std::time::Instant::now() < t)
            && !self.export_msg.is_empty()
        {
            let ok = !self.export_msg.starts_with(ph::X_CIRCLE);
            let color = if ok {
                egui::Color32::from_rgb(90, 200, 120)
            } else {
                egui::Color32::from_rgb(220, 90, 80)
            };
            return Some((false, self.export_msg.clone(), color));
        }
        None
    }

    /// Render the left Project panel and return which toolbar buttons were hit.
    ///
    /// `ctrl_s_pressed` seeds `save_clicked` so the Ctrl+S shortcut behaves
    /// exactly like clicking Save.  `save_project_needed` is set when the tree
    /// body mutates files/folders and the workspace must be rewritten.
    pub(super) fn show_project_panel(
        &mut self,
        ui: &mut egui::Ui,
        project_files: &Option<ProjectFiles>,
        ctrl_s_pressed: bool,
        save_project_needed: &mut bool,
    ) -> ProjectPanelSignals {
        let mut open_project_clicked = false;
        let mut new_project_clicked = false;
        let mut save_project_clicked = ctrl_s_pressed; // Ctrl+S triggers save
        let mut extract_folder: Option<String> = None;
        let mut new_library = false;
        let mut clone_library = false;
        let mut library_action: Option<(String, bool)> = None;
        let mut add_to_workspace: Option<String> = None;
        let mut detach_from_workspace: Option<String> = None;
        let mut open_reference: Option<usize> = None;

        egui::Panel::right("project_tree")
            .resizable(true)
            .default_size(200.0)
            .show_inside(ui, |ui| {
                // ── Panel header row ──────────────────────────────────────────
                ui.horizontal(|ui| {
                    ui.heading("Project");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // New / Open / Save grouped in one "Tools" dropdown
                        // (2026-07-10 refactor — the three separate buttons
                        // crowded the header).
                        ui.menu_button(
                            egui::RichText::new(format!("{} Tools", ph::WRENCH)).size(11.0),
                            |ui| {
                                if ui
                                    .button(format!("{} New Project", ph::NOTE_PENCIL))
                                    .on_hover_text("Start a new empty project")
                                    .clicked()
                                {
                                    new_project_clicked = true;
                                    ui.close();
                                }
                                if ui
                                    .button(format!("{} Open Project…", ph::FOLDER_OPEN))
                                    .on_hover_text("Open an existing project folder")
                                    .clicked()
                                {
                                    open_project_clicked = true;
                                    ui.close();
                                }
                                let can_save = project_files.is_some();
                                if ui
                                    .add_enabled(
                                        can_save,
                                        egui::Button::new(format!(
                                            "{} Save Project",
                                            ph::EXPORT
                                        )),
                                    )
                                    .on_hover_text("Export/Save project to disk (Ctrl+S)")
                                    .clicked()
                                {
                                    save_project_clicked = true;
                                    ui.close();
                                }
                                // Rename needs a folder on disk (a project gets
                                // its name at the first Save) and no save
                                // worker writing into the old path meanwhile.
                                let can_rename = self.project_dir.is_some()
                                    && self.save_in_progress.is_none();
                                if ui
                                    .add_enabled(
                                        can_rename,
                                        egui::Button::new(format!(
                                            "{} Rename Project…",
                                            ph::PENCIL_SIMPLE
                                        )),
                                    )
                                    .on_hover_text(
                                        "Rename the project folder on disk \
                                         (the Cargo package name is unaffected)",
                                    )
                                    .clicked()
                                {
                                    self.renaming_project =
                                        Some(self.project_name.clone().unwrap_or_default());
                                    self.renaming_project_focus = true;
                                    ui.close();
                                }
                            },
                        );
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

                // Owned (project params, toolchain) so no `self` borrow is held
                // across the `&mut self` arguments below. The manifest is the
                // authority on which directories are library crates — a stray
                // top-level folder must not be presented as one.
                let lib_crates =
                    crate::panels::mcu_module::project_gen::workspace_members(&self.cargo_toml);
                // Cloned libraries not (yet) promoted into the workspace — shown
                // in their own LIBRARIES subsection with an "Add to workspace"
                // action (guarded by a cargo-metadata pre-check).
                let detached = crate::project_tree::extract_crate::detached_libs(
                    &self.project_tree.user_src_files,
                    &lib_crates,
                );
                // Which detached lib has a pre-check running (spinner in the row).
                let ws_add_pending = self
                    .workspace_add
                    .as_ref()
                    .map(|w| w.dir.clone());
                let build_cfg = self.selected_build_cfg();
                match (project_files, build_cfg) {
                    (Some(_), Some((project, toolchain))) => {
                        let build_guard = self.build_state.lock().unwrap();
                        let build_result = build_guard.result().cloned();
                        drop(build_guard);
                        let lsp_guard = self.lsp_state.lock().unwrap();
                        // Use actual project directory if available, otherwise use temp workspace
                        let workspace_dir = if let Some(project_dir) = &self.project_dir {
                            project_dir.clone()
                        } else {
                            std::env::temp_dir().join("embedded_ide_0_check")
                        };
                        show_project_tree_panel(
                            ui,
                            &project.pkg_name,
                            &toolchain,
                            &mut self.selected_file,
                            build_result.as_ref(),
                            Some(&*lsp_guard),
                            &mut self.project_tree.user_src_files,
                            &mut self.project_tree.user_src_folders,
                            &mut self.new_src_name,
                            &mut self.new_src_folder_name,
                            &mut self.new_file_parent_folder,
                            &mut self.new_folder_parent_folder,
                            &mut self.new_file_in_folder,
                            &mut self.renaming_file,
                            &mut self.renaming_folder,
                            &workspace_dir,
                            self.project_dir.as_deref(),
                            save_project_needed,
                            &mut extract_folder,
                            &lib_crates,
                            &detached,
                            ws_add_pending.as_deref(),
                            &mut self.tree_split_ratio,
                            &mut new_library,
                            &mut clone_library,
                            &mut library_action,
                            &mut add_to_workspace,
                            &mut detach_from_workspace,
                            &mut open_reference,
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

        ProjectPanelSignals {
            open_clicked: open_project_clicked,
            new_clicked: new_project_clicked,
            save_clicked: save_project_clicked,
            extract_folder,
            new_library,
            clone_library,
            library_action,
            add_to_workspace,
            detach_from_workspace,
            open_reference,
        }
    }
}
