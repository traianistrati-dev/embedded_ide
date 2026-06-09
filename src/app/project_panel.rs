//! Left "Project" panel — header toolbar (Save / Open / New) plus the file
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
}

impl AppIde {
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

                        // Save button
                        let can_save = project_files.is_some();
                        let save_label = if can_save { "Save" } else { "Save (N/A)" };
                        if btn(
                            ui,
                            ph::EXPORT,
                            save_label,
                            "Export/Save project to disk (Ctrl+S)",
                        ) {
                            save_project_clicked = true;
                        }
                        ui.add_space(2.0);

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

                // Owned (project params, toolchain) so no `self` borrow is held
                // across the `&mut self` arguments below.
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
                            save_project_needed,
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
        }
    }
}
