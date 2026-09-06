//! "Move to folder…" — the menu-driven alternative to dragging a file row.
//!
//! Drag-and-drop can only reach a folder that is on screen, and it cannot say
//! what the move implies. This dialog picks the destination from a list and,
//! for a `.rs` file, performs the `mod`-declaration surgery that a bare move
//! leaves undone — see [`crate::project_tree::move_module`] for why the
//! declaration half is mechanical and the use-site half is not.

use super::AppIde;
use crate::project_tree::gui::{set_tree_notice, validate_move};
use crate::project_tree::move_module;
use eframe::egui;

/// The open dialog. `None` on `AppIde` means closed, like every other dialog
/// in this app.
pub(super) struct MoveFileDialog {
    /// Project-root-relative path of the file being moved.
    pub path: String,
    /// The chosen destination folder, project-root-relative.
    pub dest: String,
    /// Leave `pub use <new path>;` behind in the old parent.
    pub keep_old_paths: bool,
    pub error: Option<String>,
}

impl MoveFileDialog {
    pub fn new(path: String) -> Self {
        let dest = move_module::parent_of(&path).to_owned();
        Self {
            path,
            dest,
            // On by default: without it the move compiles only by luck. Every
            // `crate::<name>::…` path in the project keeps pointing at the old
            // location, and nothing in this IDE — or in rust-analyzer — can
            // rewrite them correctly.
            keep_old_paths: true,
            error: None,
        }
    }
}

impl AppIde {
    /// Folders a file at `path` may move into: every folder of ITS OWN crate,
    /// plus that crate's root.
    ///
    /// Restricted to one crate because a cross-crate move cannot be rescued by
    /// a re-export — the old crate would have to depend on the new one, which
    /// is the wrong way round or outright circular.
    fn move_destinations(&self, path: &str) -> Vec<String> {
        let root = crate_src_root(path);
        let mut out = vec![root.clone()];
        out.extend(
            self.project_tree
                .user_src_folders
                .iter()
                .filter(|f| f.starts_with(&format!("{root}/")))
                .cloned(),
        );
        out.sort();
        out.dedup();
        out
    }

    pub(super) fn show_move_file_dialog(&mut self, ui: &egui::Ui) {
        // Read what the list needs from `self` BEFORE borrowing the dialog
        // mutably — `move_destinations` wants `&self`.
        let Some(path) = self.move_file_dialog.as_ref().map(|d| d.path.clone()) else {
            return;
        };
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let is_rust = path.ends_with(".rs");
        let from = move_module::parent_of(&path).to_owned();
        let destinations = self.move_destinations(&path);
        let dlg = self.move_file_dialog.as_mut().expect("checked above");

        let mut close = false;
        let mut confirmed = false;
        egui::Window::new(format!("Move {name}"))
            .id(egui::Id::new("move_file_dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(460.0);
                ui.label(
                    egui::RichText::new(format!("From  {from}/"))
                        .size(11.0)
                        .monospace()
                        .color(egui::Color32::from_gray(150)),
                );
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Into").size(11.5));
                    egui::ComboBox::from_id_salt("move_file_dest")
                        .selected_text(egui::RichText::new(&dlg.dest).size(11.0).monospace())
                        .width(340.0)
                        .show_ui(ui, |ui| {
                            for d in &destinations {
                                ui.selectable_value(
                                    &mut dlg.dest,
                                    d.clone(),
                                    egui::RichText::new(d).size(11.0).monospace(),
                                );
                            }
                        });
                });

                if is_rust {
                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut dlg.keep_old_paths,
                        egui::RichText::new("Keep existing paths working").size(11.5),
                    )
                    .on_hover_text(
                        "Leaves `pub use <new path>;` in the old parent module, so every \
                         existing `crate::…` path still resolves.\nWithout it the module \
                         moves and every use site has to be fixed by hand — neither this \
                         IDE nor rust-analyzer can rewrite them safely.",
                    );
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(
                            "The `mod` declaration moves with the file. Use sites are never \
                             rewritten.",
                        )
                        .size(10.0)
                        .color(egui::Color32::from_gray(140)),
                    );
                }

                if let Some(e) = &dlg.error {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(e)
                            .size(10.5)
                            .color(egui::Color32::from_rgb(230, 130, 115)),
                    );
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let changed = dlg.dest != from;
                    if ui
                        .add_enabled(
                            changed,
                            egui::Button::new(egui::RichText::new("Move").size(11.5)),
                        )
                        .on_disabled_hover_text("Pick a different folder")
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui
                        .button(egui::RichText::new("Cancel").size(11.5))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });

        if confirmed {
            let (path, dest, keep) = {
                let d = self.move_file_dialog.as_ref().expect("still open");
                (d.path.clone(), d.dest.clone(), d.keep_old_paths)
            };
            match self.apply_move_file(ui.ctx(), &path, &dest, keep) {
                Ok(()) => close = true,
                Err(e) => {
                    if let Some(d) = &mut self.move_file_dialog {
                        d.error = Some(e);
                    }
                }
            }
        }
        if close {
            self.move_file_dialog = None;
        }
    }

    /// Move `path` into `dest_dir`, carrying its `mod` declaration.
    ///
    /// Order: plan, then move the file, then apply the declaration edits. A
    /// failed move must not leave the declarations rewritten around a file that
    /// never went anywhere.
    fn apply_move_file(
        &mut self,
        ctx: &egui::Context,
        path: &str,
        dest_dir: &str,
        keep_old_paths: bool,
    ) -> Result<(), String> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let new_path = if dest_dir.is_empty() {
            name.to_owned()
        } else {
            format!("{dest_dir}/{name}")
        };
        validate_move(path, &new_path, |cand| {
            self.project_tree
                .user_src_files
                .iter()
                .any(|(p, _)| p.eq_ignore_ascii_case(cand))
        })?;
        // The rename path already learned these two. `#[path]` decides which
        // file a module loads from, so moving the file leaves the attribute
        // pointing at nothing; and a `radar.rs` that owns `radar/` has its
        // submodules in that folder, which would have to move as well.
        if self.path_attr_targets(path) {
            return Err(format!(
                "`{name}` is loaded through a `#[path = \"...\"]` attribute - move it by hand."
            ));
        }
        if self.owns_module_dir(path) {
            let stem = name.trim_end_matches(".rs");
            return Err(format!(
                "`{name}` owns a `{stem}/` folder of submodules - move both by hand for now."
            ));
        }

        let files = &self.project_tree.user_src_files;
        let generated = self.generated_code.clone();
        let read = |p: &str| -> Option<String> {
            if p == "src/main.rs" {
                return Some(generated.clone());
            }
            files.iter().find(|(fp, _)| fp == p).map(|(_, c)| c.clone())
        };
        let have = |p: &str| p == "src/main.rs" || files.iter().any(|(fp, _)| fp == p);
        let plan = move_module::plan_move(path, &new_path, keep_old_paths, have, read);

        // ── The file itself ─────────────────────────────────────────────────
        let root = self
            .project_dir
            .clone()
            .unwrap_or_else(crate::workspace::dir);
        let dest_abs = root.join(&new_path);
        if let Some(parent) = dest_abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{e}"))?;
        }
        std::fs::rename(root.join(path), &dest_abs).map_err(|e| format!("{e}"))?;
        let Some(idx) = self
            .project_tree
            .user_src_files
            .iter()
            .position(|(p, _)| p == path)
        else {
            return Err(format!("`{path}` is no longer in the project."));
        };
        self.project_tree.user_src_files[idx].0 = new_path.clone();

        // ── The declarations ────────────────────────────────────────────────
        let mut notes = Vec::new();
        let mut unwritten = Vec::new();
        if let Some(plan) = plan {
            for edit in plan.edits {
                if edit.path == "src/main.rs" {
                    // main.rs is codegen-owned, but its `mod` block lives in the
                    // hand-editable region and `write_project` reuses this
                    // buffer verbatim, so replacing it is the same thing the
                    // editor does when you type there.
                    self.generated_code = edit.new_content;
                    continue;
                }
                match self
                    .project_tree
                    .user_src_files
                    .iter_mut()
                    .find(|(p, _)| *p == edit.path)
                {
                    Some(entry) => entry.1 = edit.new_content,
                    None if edit.create => {
                        // A brand-new module file for a folder that was not one
                        // yet. Its folder must be tracked too, or the tree shows
                        // the file as a phantom top-level entry.
                        let dir = move_module::parent_of(&edit.path).to_owned();
                        if !dir.is_empty() && !self.project_tree.user_src_folders.contains(&dir) {
                            self.project_tree.user_src_folders.push(dir);
                        }
                        self.project_tree
                            .user_src_files
                            .push((edit.path, edit.new_content));
                    }
                    // The plan named a file the tree does not hold. Silently
                    // skipping it reported a clean move over a project whose
                    // declarations are now half-updated — say so instead.
                    None => unwritten.push(edit.path),
                }
            }
            notes = plan.notes;
        }

        self.workspace_write_requested = true;
        if !unwritten.is_empty() {
            notes.push(format!(
                "could NOT update {} - check the `mod` lines there by hand",
                unwritten.join(", ")
            ));
        }
        let detail = if notes.is_empty() {
            String::new()
        } else {
            format!(" - {}", notes.join("; "))
        };
        set_tree_notice(ctx, format!("Moved `{name}` to `{dest_dir}/`{detail}."));
        Ok(())
    }
}

/// The `src` folder of the crate a project-root-relative path belongs to.
fn crate_src_root(path: &str) -> String {
    match path.split_once('/') {
        Some(("src", _)) => "src".to_owned(),
        Some((lib, _)) => format!("{lib}/src"),
        None => "src".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_belongs_to_its_own_crates_src() {
        assert_eq!(crate_src_root("src/radar.rs"), "src");
        assert_eq!(crate_src_root("src/drivers/radar.rs"), "src");
        assert_eq!(crate_src_root("mylib/src/radar.rs"), "mylib/src");
        // A loose file at the project root belongs to the firmware crate.
        assert_eq!(crate_src_root("notes.md"), "src");
    }
}

// ─────────────────────────── TEMPORARY REFUTATION PROBE ──────────────────────
