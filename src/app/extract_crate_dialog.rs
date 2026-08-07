//! "Extract to library crate" dialog + the apply step.
//!
//! The plan itself is pure ([`crate::project_tree::extract_crate`]); this file
//! collects the metadata, previews the plan live (it is cheap to recompute) and
//! performs the I/O once the user confirms.

use super::{AppIde, ProjectFileId};
use crate::project_tree::extract_crate::{self, CrateMeta, ExtractPlan};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Open dialog state: the manifest fields being edited, plus which job this is.
///
/// One dialog for both jobs because they collect exactly the same thing — a
/// [`CrateMeta`]. `folder` is what distinguishes them: `Some` extracts that
/// folder, `None` creates an empty library.
pub(crate) struct ExtractCrateDialog {
    /// Folder to extract (project-root-relative, e.g. `src/mw_radar`), or
    /// `None` for a brand-new empty library.
    pub folder: Option<String>,
    pub meta: CrateMeta,
    /// Set when applying failed (writing files, no project on disk, …).
    pub error: Option<String>,
}

impl ExtractCrateDialog {
    pub(crate) fn extract(folder: String) -> Self {
        // Default the crate name to the folder's own name.
        let name = folder.rsplit('/').next().unwrap_or(&folder).to_owned();
        Self {
            folder: Some(folder),
            meta: CrateMeta {
                name,
                ..Default::default()
            },
            error: None,
        }
    }

    pub(crate) fn new_library() -> Self {
        Self {
            folder: None,
            meta: CrateMeta::default(),
            error: None,
        }
    }
}

/// "Clone a library from git" dialog: a repo URL + a target folder name.
pub(crate) struct CloneLibraryDialog {
    pub url: String,
    pub dir: String,
    /// `true` while the folder name is auto-derived from the URL — retyping the
    /// URL keeps updating it until the user edits the folder field themselves.
    pub dir_auto: bool,
    /// `git submodule add` (tracked, self-contained) instead of a plain `git
    /// clone` (independent repo, gitignored).
    pub as_submodule: bool,
    /// Error from the last clone attempt (set by `diag_embed` on failure).
    pub error: Option<String>,
}

impl CloneLibraryDialog {
    pub(crate) fn new() -> Self {
        Self {
            url: String::new(),
            dir: String::new(),
            dir_auto: true,
            as_submodule: false,
            error: None,
        }
    }
}

/// The repo name from a git URL (`…/foo.git` / `git@host:user/foo.git` → `foo`).
fn repo_name_from_url(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    let last = u.rsplit(['/', ':']).next().unwrap_or("");
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

/// Confirmation for deleting or renaming an existing library crate.
pub(crate) struct LibraryActionDialog {
    /// The crate's directory, project-root-relative (`mw_radar`).
    pub dir: String,
    /// `Some(new_name)` renames; `None` deletes.
    pub rename_to: Option<String>,
    pub error: Option<String>,
}

impl AppIde {
    /// The "Clone a library from git" dialog. Starts the clone worker; the
    /// wiring (workspace member + gitignore + tree scan) happens in `diag_embed`
    /// when the worker's `clone_result` lands.
    pub(super) fn show_clone_library_dialog(&mut self, ui: &egui::Ui) {
        let Some(dlg) = &mut self.clone_library_dialog else {
            return;
        };
        let busy = self.git.state.lock().unwrap().busy == Some("clone");
        // Auto-fill the folder from the URL's repo name until the user edits it,
        // snake-cased so a hyphenated repo (`foo-bar`) lands as `foo_bar`.
        if dlg.dir_auto {
            dlg.dir = extract_crate::to_snake_case(&repo_name_from_url(&dlg.url));
        }
        let mut close = false;
        let mut start: Option<(String, String, bool)> = None;

        egui::Window::new("Clone a library from git")
            .id(egui::Id::new("clone_library_dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(520.0);
                egui::Grid::new("clone_lib_fields")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Repo URL").size(11.0));
                        ui.add(
                            egui::TextEdit::singleline(&mut dlg.url)
                                .desired_width(380.0)
                                .hint_text("https://github.com/user/repo.git"),
                        );
                        ui.end_row();
                        ui.label(egui::RichText::new("Folder").size(11.0));
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut dlg.dir)
                                    .desired_width(220.0)
                                    .hint_text("repo"),
                            )
                            .changed()
                        {
                            dlg.dir_auto = false;
                            // Snake-case what the user typed: spaces / `-` / other
                            // specials → `_`, so the folder is always valid.
                            dlg.dir = extract_crate::to_snake_case(&dlg.dir);
                        }
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.checkbox(&mut dlg.as_submodule, "Add as git submodule")
                    .on_hover_text(
                        "On: the project tracks it via .gitmodules + a pinned commit, so a \
                         fresh clone (+ `git submodule update --init`) includes it — but it \
                         needs the project to be a git repo, and updating the library is a \
                         two-step commit (in the submodule, then the pointer here).\n\
                         Off: an independent clone, gitignored by this project.",
                    );
                ui.add_space(4.0);
                if dlg.as_submodule {
                    ui.label(
                        egui::RichText::new(
                            "Added with `git submodule add` — the project TRACKS it (pinned \
                             commit), so a fresh clone can fetch it. Keeps its own git + remote.",
                        )
                        .size(10.5)
                        .color(egui::Color32::from_rgb(140, 190, 240)),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} Editing it is a two-step commit: in the submodule, then the \
                             updated pointer here.",
                            ph::WARNING,
                        ))
                        .size(10.5)
                        .color(egui::Color32::from_rgb(220, 180, 90)),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(
                            "Cloned as an INDEPENDENT repo (keeps its own git + remote), \
                             gitignored by this project.",
                        )
                        .size(10.5)
                        .color(egui::Color32::from_rgb(140, 190, 240)),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} A fresh clone of THIS project won't include it — you'd re-clone \
                             the library.",
                            ph::WARNING,
                        ))
                        .size(10.5)
                        .color(egui::Color32::from_rgb(220, 180, 90)),
                    );
                }
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(
                        "Cloned as a DETACHED library — not added to the workspace yet. Use \
                         \"Add to workspace\" in LIBRARIES when ready; it runs a cargo-metadata \
                         check first so an incompatible crate can't break rust-analyzer.",
                    )
                    .size(10.5)
                    .color(egui::Color32::from_gray(155)),
                );
                if let Some(e) = &dlg.error {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("{} {e}", ph::X_CIRCLE))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(230, 110, 90)),
                    );
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let can = !busy && !dlg.url.trim().is_empty() && !dlg.dir.trim().is_empty();
                    let label = if dlg.as_submodule {
                        "Add submodule"
                    } else {
                        "Clone"
                    };
                    if ui
                        .add_enabled(
                            can,
                            egui::Button::new(
                                egui::RichText::new(format!("{} {label}", ph::GIT_FORK))
                                    .color(egui::Color32::from_rgb(120, 200, 140)),
                            ),
                        )
                        .clicked()
                    {
                        start = Some((
                            dlg.url.trim().to_owned(),
                            dlg.dir.trim().to_owned(),
                            dlg.as_submodule,
                        ));
                    }
                    if busy {
                        crate::app::helpers::spinner::throttled_spinner(ui, 12.0);
                        ui.label(
                            egui::RichText::new("cloning…")
                                .size(11.0)
                                .color(egui::Color32::from_rgb(220, 180, 70)),
                        );
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                        close = true;
                    }
                });
            });

        if let Some((url, dir, as_submodule)) = start {
            if let Some(d) = &mut self.clone_library_dialog {
                d.error = None;
            }
            self.start_clone_library(url, dir, as_submodule);
        }
        if close {
            self.clone_library_dialog = None;
        }
    }

    /// Delete / rename confirmation for a library crate.
    pub(super) fn show_library_action_dialog(&mut self, ui: &egui::Ui) {
        let Some(dlg) = &mut self.library_action else {
            return;
        };
        let dir = dlg.dir.clone();
        // Snake-case the new name live (spaces / `-` / specials → `_`) so the
        // renamed folder + crate are always valid — same rule as the clone
        // dialog. Done before the plan is built so the preview stays in sync.
        if let Some(name) = &mut dlg.rename_to {
            *name = extract_crate::to_snake_case(name);
        }
        let is_rename = dlg.rename_to.is_some();
        let mut close = false;
        let mut confirmed = false;

        // Both plans are pure and cheap, so they are recomputed every frame and
        // the preview can never disagree with what the button will do.
        let del_plan = (!is_rename).then(|| {
            extract_crate::plan_delete_crate(
                &dir,
                &self.project_tree.user_src_files,
                &self.generated_code,
                &self.cargo_toml,
            )
        });
        let ren_plan = dlg.rename_to.as_ref().map(|n| {
            extract_crate::plan_rename_crate(
                &dir,
                n,
                &self.project_tree.user_src_files,
                &self.generated_code,
                &self.cargo_toml,
            )
        });

        egui::Window::new(if is_rename {
            format!("Rename library `{dir}`")
        } else {
            format!("Delete library `{dir}`?")
        })
        .id(egui::Id::new("library_action_dialog"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.set_width(460.0);
            if let Some(name) = &mut dlg.rename_to {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("New name").size(11.0));
                    ui.text_edit_singleline(name);
                });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "The folder, its Cargo.toml name, the workspace entry and every \
                         `use` of the old name are updated together.",
                    )
                    .size(10.5)
                    .color(egui::Color32::from_gray(160)),
                );
            } else if let Some(p) = &del_plan {
                ui.label(
                    egui::RichText::new(format!(
                        "{} {}/ and its {} file(s) are deleted from disk, and the \
                         workspace entry + path dependency are removed.",
                        ph::WARNING,
                        dir,
                        p.removed_files.len()
                    ))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(230, 160, 90)),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("This cannot be undone from the IDE.")
                        .size(10.5)
                        .color(egui::Color32::from_rgb(230, 130, 90)),
                );
                if !p.warnings.is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Still used here — the build will break:")
                            .size(10.5)
                            .color(egui::Color32::from_rgb(230, 180, 80)),
                    );
                    for w in &p.warnings {
                        ui.label(
                            egui::RichText::new(format!("{} {w}", ph::DOT))
                                .size(10.0)
                                .color(egui::Color32::from_rgb(200, 190, 150)),
                        );
                    }
                }
            }

            let err = dlg.error.clone().or_else(|| match &ren_plan {
                Some(Err(e)) => Some(e.clone()),
                _ => None,
            });
            if let Some(e) = &err {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("{} {e}", ph::X_CIRCLE))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(230, 90, 80)),
                );
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let (label, color) = if is_rename {
                    ("Rename", egui::Color32::from_rgb(120, 200, 140))
                } else {
                    ("Delete", egui::Color32::from_rgb(230, 110, 90))
                };
                let can = !matches!(&ren_plan, Some(Err(_)));
                if ui
                    .add_enabled(
                        can,
                        egui::Button::new(egui::RichText::new(label).color(color)),
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
            let outcome = match (del_plan, ren_plan) {
                (Some(p), _) => Some(self.apply_delete_crate(p)),
                (_, Some(Ok(p))) => Some(self.apply_rename_crate(p)),
                _ => None,
            };
            match outcome {
                Some(Ok(())) => close = true,
                Some(Err(e)) => {
                    if let Some(d) = &mut self.library_action {
                        d.error = Some(e);
                    }
                }
                None => {}
            }
        }
        if close {
            self.library_action = None;
        }
    }

    /// Modal shown when an "Add to workspace" cargo-metadata pre-check FAILED —
    /// the library would break the workspace (and rust-analyzer), so it was NOT
    /// added. Shows the cargo error so the user can fix the library's deps.
    pub(super) fn show_workspace_add_error_dialog(&mut self, ui: &egui::Ui) {
        let Some((dir, error)) = self.workspace_add_error.clone() else {
            return;
        };
        let mut close = false;
        egui::Window::new(format!("Can't add `{dir}` to the workspace"))
            .id(egui::Id::new("workspace_add_error_dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(560.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{} Adding it as a workspace member would break `cargo metadata` \
                         for the whole project — the same load rust-analyzer does — so it \
                         was NOT added. The library stays cloned (detached).",
                        ph::WARNING,
                    ))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(230, 160, 90)),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("cargo metadata:")
                        .size(10.5)
                        .color(egui::Color32::from_gray(150)),
                );
                egui::Frame::NONE
                    .fill(egui::Color32::from_gray(24))
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&error)
                                .size(10.5)
                                .monospace()
                                .color(egui::Color32::from_rgb(230, 120, 100)),
                        );
                    });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Common causes: the library needs a different version of a shared \
                         crate, it is its OWN workspace (has its own [workspace]), or a \
                         path/git dependency can't be resolved. Fix its Cargo.toml, then \
                         try Add to workspace again.",
                    )
                    .size(10.0)
                    .color(egui::Color32::from_gray(150)),
                );
                ui.add_space(10.0);
                if ui.button("OK").clicked() {
                    close = true;
                }
            });
        if close {
            self.workspace_add_error = None;
        }
    }

    fn apply_delete_crate(&mut self, plan: extract_crate::DeleteCratePlan) -> Result<(), String> {
        let root = self.require_project_dir()?;
        // A library cloned as a git submodule owns a `.gitmodules` entry + a
        // gitlink in the index; a plain `remove_dir_all` would leave those
        // dangling (stale entry → `git status` fails → the repo reads as "not a
        // repo"). Deinit + `git rm` cleans them and removes the tree too.
        if crate::git::is_submodule(&root, &plan.crate_dir) {
            crate::git::remove_submodule(&root, &plan.crate_dir);
        }
        // For a regular library this is THE removal; for a submodule `git rm`
        // already deleted the tree, so a now-missing dir is expected.
        let dir = root.join(&plan.crate_dir);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("Could not delete {}: {e}", plan.crate_dir))?;
        }
        self.project_tree
            .user_src_files
            .retain(|(p, _)| !plan.removed_files.contains(p));
        let sub = format!("{}/", plan.crate_dir);
        self.project_tree
            .user_src_folders
            .retain(|f| f != &plan.crate_dir && !f.starts_with(&sub));
        self.cargo_toml = plan.root_cargo_toml;
        self.reselect_after_tree_change();
        self.request_save = true;
        Ok(())
    }

    fn apply_rename_crate(&mut self, plan: extract_crate::RenameCratePlan) -> Result<(), String> {
        let root = self.require_project_dir()?;
        std::fs::rename(root.join(&plan.old_dir), root.join(&plan.new_dir))
            .map_err(|e| format!("Could not rename {}: {e}", plan.old_dir))?;
        for (old, new) in &plan.moved {
            if let Some(e) = self
                .project_tree
                .user_src_files
                .iter_mut()
                .find(|(p, _)| p == old)
            {
                e.0 = new.clone();
            }
        }
        let sub = format!("{}/", plan.old_dir);
        for f in &mut self.project_tree.user_src_folders {
            if *f == plan.old_dir {
                *f = plan.new_dir.clone();
            } else if let Some(rest) = f.strip_prefix(&sub) {
                *f = format!("{}/{rest}", plan.new_dir);
            }
        }
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
        self.cargo_toml = plan.root_cargo_toml;
        self.cached_project_files = None;
        self.request_save = true;
        Ok(())
    }

    /// Drop a selection that pointed into files just removed.
    fn reselect_after_tree_change(&mut self) {
        if let ProjectFileId::UserFile(i) = self.selected_file {
            if i >= self.project_tree.user_src_files.len() {
                self.selected_file = ProjectFileId::MainRs;
            }
        }
        self.cached_project_files = None;
    }

    pub(super) fn show_extract_crate_dialog(&mut self, ui: &egui::Ui) {
        let Some(dlg) = &mut self.extract_crate else {
            return;
        };
        // Recomputed every frame so the preview and the button always agree
        // with what is actually in the fields. Exactly one of the two plans is
        // built, decided by whether a source folder was given.
        let folder = dlg.folder.clone();
        let extract_plan = folder.as_ref().map(|f| {
            extract_crate::plan_extract(
                f,
                &self.project_tree.user_src_files,
                &self.generated_code,
                &self.cargo_toml,
                &dlg.meta,
            )
        });
        let new_plan = if folder.is_none() {
            Some(extract_crate::plan_new_crate(&dlg.meta, &self.cargo_toml))
        } else {
            None
        };
        let error_text = match (&extract_plan, &new_plan) {
            (Some(Err(e)), _) | (_, Some(Err(e))) => Some(e.clone()),
            _ => None,
        };
        let title = match &folder {
            Some(f) => format!("Extract `{f}/` to a library crate"),
            None => "New library crate".to_owned(),
        };
        let mut close = false;
        let mut confirmed = false;

        egui::Window::new(title)
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
                        if ui.text_edit_singleline(&mut m.name).changed() {
                            // Snake-case the crate/folder name (spaces / `-` /
                            // specials → `_`) — same rule as clone + rename.
                            m.name = extract_crate::to_snake_case(&m.name);
                        }
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

                if let Some(e) = &error_text {
                    ui.label(
                        egui::RichText::new(format!("{} {e}", ph::WARNING))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(230, 130, 90)),
                    );
                }
                if let Some(p) = new_plan.as_ref().and_then(|r| r.as_ref().ok()) {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} creates {}/Cargo.toml + {}/src/lib.rs, and adds it \
                             to the workspace",
                            ph::ARROW_RIGHT,
                            p.crate_dir,
                            p.crate_dir,
                        ))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(140, 190, 240)),
                    );
                }
                match extract_plan.as_ref() {
                    None | Some(Err(_)) => {}
                    Some(Ok(p)) => {
                        // No raw Unicode arrows/bullets anywhere in the UI: only
                        // the phosphor glyphs are in the loaded font, a literal
                        // `→` renders as a tofu square.
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {} file(s) moved to {}/src/,  {} file(s) rewritten",
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
                                            egui::RichText::new(format!("{} {w}", ph::DOT))
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
                    let can = error_text.is_none();
                    let label = if folder.is_some() {
                        "Extract"
                    } else {
                        "Create"
                    };
                    if ui
                        .add_enabled(
                            can,
                            egui::Button::new(
                                egui::RichText::new(format!("{} {label}", ph::PACKAGE))
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
            let outcome = match (extract_plan, new_plan) {
                (Some(Ok(p)), _) => Some(self.apply_extract_crate(p)),
                (_, Some(Ok(p))) => Some(self.apply_new_crate(p)),
                _ => None,
            };
            match outcome {
                Some(Ok(())) => close = true,
                Some(Err(e)) => {
                    if let Some(d) = &mut self.extract_crate {
                        d.error = Some(e);
                    }
                }
                None => {}
            }
        }
        if close {
            self.extract_crate = None;
        }
    }

    /// Create an empty library crate: write it, register it, wire the manifest.
    fn apply_new_crate(&mut self, plan: extract_crate::NewCratePlan) -> Result<(), String> {
        let root = self.require_project_dir()?;
        self.write_and_register(&root, &plan.new_files)?;
        for dir in [plan.crate_dir.clone(), format!("{}/src", plan.crate_dir)] {
            if !self.project_tree.user_src_folders.contains(&dir) {
                self.project_tree.user_src_folders.push(dir);
            }
        }
        self.cargo_toml = plan.root_cargo_toml;
        self.cached_project_files = None;
        // Open the new lib.rs so there is somewhere obvious to start typing.
        let lib = format!("{}/src/lib.rs", plan.crate_dir);
        if let Some(i) = self
            .project_tree
            .user_src_files
            .iter()
            .position(|(p, _)| *p == lib)
        {
            self.selected_file = ProjectFileId::UserFile(i);
        }
        self.request_save = true;
        Ok(())
    }

    /// The project directory, or the reason there isn't one.
    fn require_project_dir(&self) -> Result<std::path::PathBuf, String> {
        self.project_dir.clone().ok_or_else(|| {
            "Save the project first — the crate is written next to it on disk.".to_owned()
        })
    }

    /// Write `files` to disk AND take ownership of them in the tree.
    ///
    /// Both halves are mandatory: `write_project` prunes every `.rs` under the
    /// root that is not in `user_src_files`, so a crate known only to the disk
    /// is DELETED by the next save or build.
    fn write_and_register(
        &mut self,
        root: &std::path::Path,
        files: &[(String, String)],
    ) -> Result<(), String> {
        for (rel, content) in files {
            let dest = root.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
            }
            std::fs::write(&dest, content)
                .map_err(|e| format!("Could not write {}: {e}", dest.display()))?;
        }
        for (rel, content) in files {
            if let Some(e) = self
                .project_tree
                .user_src_files
                .iter_mut()
                .find(|(p, _)| p == rel)
            {
                e.1 = content.clone(); // overwriting an existing crate
            } else {
                self.project_tree
                    .user_src_files
                    .push((rel.clone(), content.clone()));
            }
        }
        Ok(())
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
        let root = self.require_project_dir()?;
        self.write_and_register(&root, &plan.new_files)?;
        for dir in [plan.crate_dir.clone(), format!("{}/src", plan.crate_dir)] {
            if !self.project_tree.user_src_folders.contains(&dir) {
                self.project_tree.user_src_folders.push(dir);
            }
        }

        // Remember what was selected: the indices below shift under it.
        let selected_path = match self.selected_file {
            ProjectFileId::UserFile(i) => self
                .project_tree
                .user_src_files
                .get(i)
                .map(|(p, _)| p.clone()),
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
