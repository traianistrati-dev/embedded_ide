//! Modal dialogs for the project panel: New Project, New File, New Folder.
//!
//! These are inherent methods on [`AppIde`]; being a child module of `app`
//! lets them touch `AppIde`'s private fields directly.  Each takes the active
//! `ui` plus a `save_project_needed` flag that the caller acts on afterwards
//! (writing the whole project to disk when the tree changed).

use super::{AppIde, McuTab, ProjectFileId};
use crate::panels::mcu_module::mcu_catalog::McuType;
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

                // ── Chip selector ─────────────────────────────────────────
                ui.horizontal(|ui| {
                    ui.label("Chip:");
                    let selected_text = match &self.pending_mcu_type {
                        None => "— Empty —".to_string(),
                        Some(t) => t.label().to_string(),
                    };
                    egui::ComboBox::from_id_salt("new_project_chip_selector")
                        .selected_text(selected_text)
                        .show_ui(ui, |ui| {
                            // "Empty" — first entry, no chip selected
                            ui.selectable_value(&mut self.pending_mcu_type, None, "— Empty —");
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
                        self.project_tree.user_src_files.clear();
                        self.project_tree.user_src_folders.clear();
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
                        // Pre-populate the pins/ scaffold so the tree shows
                        // the folder immediately, before any pin is configured.
                        self.project_tree.init_pins_scaffold();
                        *save_project_needed = true;
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
