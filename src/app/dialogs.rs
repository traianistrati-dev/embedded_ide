//! Modal dialogs for the project panel: New Project, New File, New Folder.
//!
//! These are inherent methods on [`AppIde`]; being a child module of `app`
//! lets them touch `AppIde`'s private fields directly.  Each takes the active
//! `ui` plus a `save_project_needed` flag that the caller acts on afterwards
//! (writing the whole project to disk when the tree changed).

use super::{AppIde, McuTab, ProjectFileId};
use crate::panels::mcu_module::{codegen, registry};
use eframe::egui;
use egui_phosphor::regular as ph;

impl AppIde {
    /// "New Project" confirmation modal — picks a chip and clears all user files.
    pub(super) fn show_new_project_dialog(
        &mut self,
        ui: &mut egui::Ui,
        save_project_needed: &mut bool,
    ) {
        if !self.confirm_new_project {
            return;
        }
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

                // ── Chip selector (driven by the MCU registry) ────────────
                // (id, display_name) snapshot so the closure doesn't borrow the
                // registry while mutating `pending_mcu_id`.
                let options: Vec<(String, String)> = self
                    .mcu_registry
                    .iter()
                    .map(|d| (d.id.clone(), d.display_name.clone()))
                    .collect();
                let selected_text = match &self.pending_mcu_id {
                    None => "— Empty —".to_string(),
                    Some(id) => options
                        .iter()
                        .find(|(oid, _)| oid == id)
                        .map(|(_, name)| name.clone())
                        .unwrap_or_else(|| id.clone()),
                };
                let family = self
                    .pending_mcu_id
                    .as_ref()
                    .and_then(|id| self.mcu_registry.iter().find(|d| &d.id == id))
                    .map(|d| d.cpu.clone());

                ui.horizontal(|ui| {
                    ui.label("Chip:");
                    egui::ComboBox::from_id_salt("new_project_chip_selector")
                        .selected_text(selected_text)
                        .show_ui(ui, |ui| {
                            // "Empty" — first entry, no chip selected
                            ui.selectable_value(&mut self.pending_mcu_id, None, "— Empty —");
                            for (id, name) in &options {
                                ui.selectable_value(
                                    &mut self.pending_mcu_id,
                                    Some(id.clone()),
                                    name,
                                );
                            }
                        });
                    // Architecture family hint
                    if let Some(fam) = family {
                        ui.label(
                            egui::RichText::new(fam)
                                .color(egui::Color32::GRAY)
                                .size(11.0),
                        );
                    }

                    // ── Import MCU… (runtime .ron import) ──────────────
                    if ui
                        .button(egui::RichText::new(format!("{} Import…", ph::PLUS)).size(12.0))
                        .on_hover_text("Import an MCU definition from a .ron file")
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("MCU definition", &["ron"])
                            .set_title("Import MCU definition (.ron)")
                            .pick_file()
                        {
                            match registry::import_file(&path) {
                                Ok(def) => {
                                    let id = def.id.clone();
                                    let name = def.display_name.clone();
                                    let fam = def.family.clone();
                                    registry::merge_def(&mut self.mcu_registry, def);
                                    self.pending_mcu_id = Some(id);
                                    let note = if codegen::family::backend_for(&fam).is_none() {
                                        format!(" — no codegen backend for '{fam}'")
                                    } else {
                                        String::new()
                                    };
                                    self.mcu_import_status =
                                        Some(format!("{}  Imported {name}{note}", ph::CHECK));
                                }
                                Err(e) => {
                                    self.mcu_import_status =
                                        Some(format!("{}  {e}", ph::WARNING));
                                }
                            }
                        }
                    }
                });

                // Last import result (persists until the popup closes).
                if let Some(msg) = &self.mcu_import_status {
                    let ok = msg.starts_with(ph::CHECK);
                    let col = if ok {
                        egui::Color32::from_rgb(120, 200, 120)
                    } else {
                        egui::Color32::from_rgb(220, 120, 90)
                    };
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(msg).size(11.0).color(col));
                }

                // ── Import-folder discoverability ──────────────────────────
                // Show where user .ron definitions live + a one-click "Open".
                if let Some(dir) = registry::user_mcus_dir() {
                    let path_str = dir.display().to_string();
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{} Import folder:", ph::FOLDER))
                                .size(10.5)
                                .color(egui::Color32::GRAY),
                        );
                        if ui
                            .button(egui::RichText::new("Open").size(10.5))
                            .on_hover_text(format!("Open {path_str}\n(drop .ron files here)"))
                            .clicked()
                        {
                            registry::open_user_mcus_dir();
                        }
                    });
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(path_str)
                                .size(9.5)
                                .monospace()
                                .color(egui::Color32::from_gray(120)),
                        )
                        .truncate(),
                    );
                }

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
                        if let Some(new_id) = self.pending_mcu_id.take() {
                            if new_id != self.selected_mcu_id {
                                self.selected_mcu_id = new_id;
                                self.mcu =
                                    Self::build_mcu_for(&self.mcu_registry, &self.selected_mcu_id);
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
                        self.project_tree.user_src_files.clear();
                        self.project_tree.user_src_folders.clear();
                        // Fresh config files (Cargo.toml, memory.x, …) for the
                        // selected chip — a clean slate for the new project.
                        self.reset_config_files();
                        self.selected_file = ProjectFileId::MainRs;
                        self.project_name = None;
                        self.project_dir = None;
                        self.renaming_file = None;
                        self.renaming_folder = None;
                        self.new_src_name = None;
                        self.new_src_folder_name = None;
                        self.new_file_parent_folder = None;
                        self.new_folder_parent_folder = None;
                        self.new_file_in_folder = None;
                        self.confirm_new_project = false;
                        self.mcu_import_status = None;
                        // Pre-populate the pins/ scaffold so the tree shows
                        // the folder immediately, before any pin is configured.
                        self.project_tree.init_pins_scaffold();
                        // New project = fresh deps → drop the stale workspace lock
                        // so the next check re-resolves (saves otherwise keep it).
                        self.reset_workspace_lock();
                        *save_project_needed = true;
                    }
                    ui.add_space(8.0);
                    if ui.button("Cancel").clicked() {
                        self.confirm_new_project = false;
                        self.pending_mcu_id = None;
                        self.mcu_import_status = None;
                    }
                });
                ui.add_space(4.0);
            });
    }

    /// "New File" dialog — creates a user source file under the chosen folder.
    pub(super) fn show_new_file_dialog(
        &mut self,
        ui: &mut egui::Ui,
        save_project_needed: &mut bool,
    ) {
        if let Some(ref mut new_name) = self.new_src_name {
            let mut should_close = false;
            let parent_folder = self
                .new_file_parent_folder
                .clone()
                .unwrap_or_else(|| String::new());
            egui::Window::new("New File")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.add_space(4.0);
                    let display_folder = if parent_folder.is_empty() {
                        "src"
                    } else {
                        &parent_folder
                    };
                    ui.label(format!("Create file in: {display_folder}/"));
                    ui.label("Enter filename:");
                    let response = ui.text_edit_singleline(new_name);
                    if ui.memory(|m| {
                        m.data
                            .get_temp::<bool>(egui::Id::new("__new_file__"))
                            .unwrap_or(true)
                    }) {
                        response.request_focus();
                        ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("__new_file__"), false));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
                            let clean = new_name.trim().to_string();
                            let full_path = if parent_folder.is_empty() {
                                clean.clone()
                            } else {
                                format!("{parent_folder}/{clean}")
                            };
                            if !clean.is_empty()
                                && !self
                                    .project_tree
                                    .user_src_files
                                    .iter()
                                    .any(|(p, _)| p == &full_path)
                            {
                                // Use the actual project directory if available, otherwise use temp workspace
                                let base_dir = if let Some(project_dir) = &self.project_dir {
                                    project_dir.join("src")
                                } else {
                                    std::env::temp_dir().join("embedded_ide_0_check")
                                };
                                let file_path = if parent_folder.is_empty() {
                                    base_dir.join(&clean)
                                } else {
                                    base_dir.join(&parent_folder).join(&clean)
                                };
                                if let Some(parent) = file_path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                let _ = std::fs::write(&file_path, "// New file\n");
                                self.project_tree
                                    .user_src_files
                                    .push((full_path, "// New file\n".to_string()));
                                self.selected_file = ProjectFileId::UserFile(
                                    self.project_tree.user_src_files.len() - 1,
                                );
                                *save_project_needed = true;
                            }
                            should_close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            should_close = true;
                        }
                    });
                    ui.add_space(4.0);
                });
            if should_close {
                self.new_src_name = None;
                self.new_file_parent_folder = None;
            }
        }
    }

    /// "New Folder" dialog — creates an empty folder under the chosen parent.
    pub(super) fn show_new_folder_dialog(
        &mut self,
        ui: &mut egui::Ui,
        save_project_needed: &mut bool,
    ) {
        if let Some(ref mut new_name) = self.new_src_folder_name {
            let mut should_close = false;
            let parent_folder = self
                .new_folder_parent_folder
                .clone()
                .unwrap_or_else(|| String::new());
            egui::Window::new("New Folder")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.add_space(4.0);
                    let display_folder = if parent_folder.is_empty() {
                        "src"
                    } else {
                        &parent_folder
                    };
                    ui.label(format!("Create folder in: {display_folder}/"));
                    ui.label("Enter folder name:");
                    let response = ui.text_edit_singleline(new_name);
                    if ui.memory(|m| {
                        m.data
                            .get_temp::<bool>(egui::Id::new("__new_folder__"))
                            .unwrap_or(true)
                    }) {
                        response.request_focus();
                        ui.memory_mut(|m| {
                            m.data.insert_temp(egui::Id::new("__new_folder__"), false)
                        });
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
                            let clean = new_name.trim().to_string();
                            let full_path = if parent_folder.is_empty() {
                                clean.clone()
                            } else {
                                format!("{parent_folder}/{clean}")
                            };
                            if !clean.is_empty()
                                && !self.project_tree.user_src_folders.contains(&full_path)
                            {
                                self.project_tree.user_src_folders.push(full_path.clone());
                                // Use the actual project directory if available, otherwise use temp workspace
                                let base_dir = if let Some(project_dir) = &self.project_dir {
                                    project_dir.join("src")
                                } else {
                                    std::env::temp_dir().join("embedded_ide_0_check")
                                };
                                let folder_path = if parent_folder.is_empty() {
                                    base_dir.join(&clean)
                                } else {
                                    base_dir.join(&parent_folder).join(&clean)
                                };
                                let _ = std::fs::create_dir_all(&folder_path);
                                *save_project_needed = true;
                            }
                            should_close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            should_close = true;
                        }
                    });
                    ui.add_space(4.0);
                });
            if should_close {
                self.new_src_folder_name = None;
                self.new_folder_parent_folder = None;
            }
        }
    }
}
