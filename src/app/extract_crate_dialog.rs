//! "Extract to library crate" dialog + the apply step.
//!
//! The plan itself is pure ([`crate::project_tree::extract_crate`]); this file
//! collects the metadata, previews the plan live (it is cheap to recompute) and
//! performs the I/O once the user confirms.

use super::{AppIde, ProjectFileId};
use crate::project_tree::extract_crate::{self, CrateMeta, ExtractPlan};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Open dialog state: which folder, and the manifest fields being edited.
pub(crate) struct ExtractCrateDialog {
    /// Folder path relative to the project root (e.g. `src/mw_radar`).
    pub folder: String,
    pub meta: CrateMeta,
    /// Set when applying failed (writing files, no project on disk, …).
    pub error: Option<String>,
}

impl ExtractCrateDialog {
    pub(crate) fn new(folder: String) -> Self {
        // Default the crate name to the folder's own name.
        let name = folder.rsplit('/').next().unwrap_or(&folder).to_owned();
        Self {
            folder,
            meta: CrateMeta {
                name,
                ..Default::default()
            },
            error: None,
        }
    }
}

impl AppIde {
    pub(super) fn show_extract_crate_dialog(&mut self, ui: &egui::Ui) {
        let Some(dlg) = &mut self.extract_crate else {
            return;
        };
        // Recomputed every frame so the preview and the Extract button always
        // agree with what is actually in the fields.
        let plan = extract_crate::plan_extract(
            &dlg.folder,
            &self.project_tree.user_src_files,
            &self.generated_code,
            &self.cargo_toml,
            &dlg.meta,
        );
        let folder = dlg.folder.clone();
        let mut close = false;
        let mut confirmed = false;

        egui::Window::new(format!("Extract `{folder}/` to a library crate"))
            .id(egui::Id::new("extract_crate_dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(520.0);
                let m = &mut dlg.meta;
                egui::Grid::new("extract_crate_fields")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Crate name").size(11.0));
                        ui.text_edit_singleline(&mut m.name);
                        ui.end_row();
                        ui.label(egui::RichText::new("Version").size(11.0));
                        ui.text_edit_singleline(&mut m.version);
                        ui.end_row();
                        ui.label(egui::RichText::new("Edition").size(11.0));
                        ui.text_edit_singleline(&mut m.edition);
                        ui.end_row();
                        ui.label(egui::RichText::new("License").size(11.0));
                        ui.text_edit_singleline(&mut m.license);
                        ui.end_row();
                        ui.label(egui::RichText::new("Description").size(11.0));
                        ui.text_edit_singleline(&mut m.description);
                        ui.end_row();
                    });
                ui.add_space(2.0);
                ui.checkbox(&mut m.no_std, "#![no_std] (embedded library)")
                    .on_hover_text("Adds `#![no_std]` to the generated lib.rs.");
                ui.add_space(8.0);

                match &plan {
                    Err(e) => {
                        ui.label(
                            egui::RichText::new(format!("{} {e}", ph::WARNING))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(230, 130, 90)),
                        );
                    }
                    Ok(p) => {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {} file(s) → {}/src/,  {} file(s) rewritten",
                                ph::ARROW_RIGHT,
                                p.removed.len(),
                                p.crate_dir,
                                p.rewritten.len() + p.rewritten_main.is_some() as usize,
                            ))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(140, 190, 240)),
                        );
                        ui.add_space(4.0);
                        if !p.warnings.is_empty() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} You will have to fix these by hand:",
                                    ph::WARNING
                                ))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(230, 180, 80)),
                            );
                            egui::ScrollArea::vertical()
                                .id_salt("extract_warnings")
                                .max_height(140.0)
                                .show(ui, |ui| {
                                    for w in &p.warnings {
                                        ui.label(
                                            egui::RichText::new(format!("• {w}"))
                                                .size(10.5)
                                                .color(egui::Color32::from_rgb(200, 190, 150)),
                                        );
                                    }
                                });
                        }
                    }
                }

                if let Some(err) = &dlg.error {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("{} {err}", ph::X_CIRCLE))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(230, 90, 80)),
                    );
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let can = plan.is_ok();
                    if ui
                        .add_enabled(
                            can,
                            egui::Button::new(
                                egui::RichText::new(format!("{} Extract", ph::PACKAGE))
                                    .color(egui::Color32::from_rgb(120, 200, 140)),
                            ),
                        )
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });

        if confirmed {
            if let Ok(p) = plan {
                match self.apply_extract_crate(p) {
                    Ok(()) => close = true,
                    Err(e) => {
                        if let Some(d) = &mut self.extract_crate {
                            d.error = Some(e);
                        }
                    }
                }
            }
        }
        if close {
            self.extract_crate = None;
        }
    }

    /// Perform the plan: register the new crate, drop the moved files, apply
    /// the rewrites, patch the root manifest.
    ///
    /// The new files go into `user_src_files` — paths there are relative to the
    /// PROJECT ROOT, so a library crate fits in the same list. This is not
    /// optional bookkeeping: `write_project` prunes every `.rs` under the root
    /// that is not in that list, so a crate known only to the disk would be
    /// DELETED by the next save or build.
    fn apply_extract_crate(&mut self, plan: ExtractPlan) -> Result<(), String> {
        let Some(root) = self.project_dir.clone() else {
            return Err(
                "Save the project first — the new crate is written next to it on disk."
                    .to_owned(),
            );
        };

        for (rel, content) in &plan.new_files {
            let dest = root.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
            }
            std::fs::write(&dest, content)
                .map_err(|e| format!("Could not write {}: {e}", dest.display()))?;
        }

        // Take ownership of the new crate in the tree model.
        for (rel, content) in &plan.new_files {
            if let Some(e) = self
                .project_tree
                .user_src_files
                .iter_mut()
                .find(|(p, _)| p == rel)
            {
                e.1 = content.clone(); // re-extraction over an existing crate
            } else {
                self.project_tree
                    .user_src_files
                    .push((rel.clone(), content.clone()));
            }
        }
        for dir in [plan.crate_dir.clone(), format!("{}/src", plan.crate_dir)] {
            if !self.project_tree.user_src_folders.contains(&dir) {
                self.project_tree.user_src_folders.push(dir);
            }
        }

        // Remember what was selected: the indices below shift under it.
        let selected_path = match self.selected_file {
            ProjectFileId::UserFile(i) => {
                self.project_tree.user_src_files.get(i).map(|(p, _)| p.clone())
            }
            _ => None,
        };

        for (path, content) in &plan.rewritten {
            if let Some(e) = self
                .project_tree
                .user_src_files
                .iter_mut()
                .find(|(p, _)| p == path)
            {
                e.1 = content.clone();
            }
        }
        if let Some(main) = plan.rewritten_main {
            self.generated_code = main;
        }

        // Drop the moved files (and delete them from the project on disk — the
        // next save would prune them anyway, but leaving them there means a
        // stale copy compiles in the meantime).
        for path in &plan.removed {
            let _ = std::fs::remove_file(root.join(path));
        }
        self.project_tree
            .user_src_files
            .retain(|(p, _)| !plan.removed.contains(p));
        // The folder is empty now — and its subfolders with it.
        let folder = plan.source_folder.clone();
        if !folder.is_empty() {
            let sub = format!("{folder}/");
            self.project_tree
                .user_src_folders
                .retain(|f| f != &folder && !f.starts_with(&sub));
            let _ = std::fs::remove_dir_all(root.join(&folder));
        }

        self.cargo_toml = plan.root_cargo_toml;
        self.cached_project_files = None;

        // Re-point the selection: the moved file is gone, others shifted.
        self.selected_file = selected_path
            .and_then(|p| {
                self.project_tree
                    .user_src_files
                    .iter()
                    .position(|(q, _)| *q == p)
            })
            .map_or(ProjectFileId::MainRs, ProjectFileId::UserFile);

        // The manifest changed and files moved — get it all on disk.
        self.request_save = true;
        Ok(())
    }
}
