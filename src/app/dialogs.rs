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
    /// The "Rename Project" dialog (opened from the Project panel's Tools
    /// menu). Renames the project FOLDER on disk — see
    /// [`AppIde::rename_project`] for what does (and deliberately does not)
    /// change. Result lands in the status bar via `export_msg`.
    pub(super) fn show_rename_project_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(mut name) = self.renaming_project.clone() else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Rename Project")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0])
            .show(ui.ctx(), |ui| {
                ui.add_space(4.0);
                ui.label("New folder name:");
                let resp = ui.text_edit_singleline(&mut name);
                if self.renaming_project_focus {
                    resp.request_focus();
                    self.renaming_project_focus = false;
                }
                // Live validation feedback (the same check rename enforces).
                let err = super::project_io::valid_project_name(&name).err();
                if let Some(e) = &err {
                    ui.label(
                        egui::RichText::new(e)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(230, 120, 90)),
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let enter =
                        resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui
                        .add_enabled(err.is_none(), egui::Button::new("Rename"))
                        .clicked()
                        || (enter && err.is_none())
                    {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        cancel = true;
                    }
                });
                ui.add_space(2.0);
            });

        if confirm {
            match self.rename_project(&name) {
                Ok(()) => {
                    self.export_msg = format!(
                        "{}  renamed to {}",
                        egui_phosphor::regular::CHECK_CIRCLE,
                        name.trim()
                    );
                }
                Err(e) => {
                    self.export_msg =
                        format!("{}  {e}", egui_phosphor::regular::X_CIRCLE);
                }
            }
            self.export_status_until =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(4));
            self.renaming_project = None;
        } else if cancel {
            self.renaming_project = None;
        } else {
            self.renaming_project = Some(name); // keep edits
        }
    }

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
                        // A new project starts with the automatic Structure layout.
                        self.structure_overrides.clear();
                        self.structure_cache = None;
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
}
