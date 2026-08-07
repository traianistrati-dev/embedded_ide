//! Modal dialogs for the project panel: New Project, New File, New Folder.
//!
//! These are inherent methods on [`AppIde`]; being a child module of `app`
//! lets them touch `AppIde`'s private fields directly.  Each takes the active
//! `ui` plus a `save_project_needed` flag that the caller acts on afterwards
//! (writing the whole project to disk when the tree changed).

use super::{AppIde, McuTab, ProjectFileId};
use crate::panels::mcu_module::{codegen, registry, stm32_pin_data};
use eframe::egui;
use egui_phosphor::regular as ph;

/// What the user picked in an [`unsaved_changes_modal`].
#[derive(PartialEq)]
pub(super) enum UnsavedChoice {
    /// Still open, or the save it started is still running.
    None,
    Save,
    Discard,
    Cancel,
}

/// The shared "N files changed since the last save" modal.
///
/// Two things put it up — closing the app and opening another project — and both
/// ask the same question, so they share one body: same list, same three buttons,
/// same order. Only the wording and what happens afterwards differ.
#[allow(clippy::too_many_arguments)]
pub(super) fn unsaved_changes_modal(
    ui: &egui::Ui,
    id: &str,
    unsaved: &[String],
    // A save started by THIS prompt is still running: the buttons are replaced
    // by a spinner so the action can't be started twice or interrupted mid-write.
    saving: bool,
    saving_note: &str,
    // Save writes the WHOLE project from the current build config; without one
    // (chip not supported for export) there is nothing to save to.
    can_save: bool,
    save_label: &str,
    discard_label: &str,
) -> UnsavedChoice {
    let mut choice = UnsavedChoice::None;
    egui::Window::new("Unsaved changes")
        .id(egui::Id::new(id))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!(
                    "{} file{} changed since the last save:",
                    unsaved.len(),
                    if unsaved.len() == 1 { "" } else { "s" }
                ))
                .size(12.0),
            );
            ui.add_space(4.0);
            // A long list would grow the modal past the window — cap it.
            const MAX_LISTED: usize = 8;
            for path in unsaved.iter().take(MAX_LISTED) {
                ui.label(
                    egui::RichText::new(format!("  {path}"))
                        .monospace()
                        .size(11.0),
                );
            }
            if unsaved.len() > MAX_LISTED {
                ui.label(
                    egui::RichText::new(format!("  … and {} more", unsaved.len() - MAX_LISTED))
                        .size(11.0)
                        .color(egui::Color32::GRAY),
                );
            }
            ui.add_space(10.0);

            if saving {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(saving_note);
                });
                return;
            }
            ui.horizontal(|ui| {
                let resp = ui
                    .add_enabled(
                        can_save,
                        egui::Button::new(
                            egui::RichText::new(format!("{} {save_label}", ph::EXPORT))
                                .color(egui::Color32::from_rgb(120, 200, 140)),
                        ),
                    )
                    .on_disabled_hover_text("This chip has no export configuration to save");
                if resp.clicked() {
                    choice = UnsavedChoice::Save;
                }
                if ui
                    .button(
                        egui::RichText::new(format!("{} {discard_label}", ph::TRASH))
                            .color(egui::Color32::from_rgb(230, 130, 90)),
                    )
                    .clicked()
                {
                    choice = UnsavedChoice::Discard;
                }
                if ui.button("Cancel").clicked() {
                    choice = UnsavedChoice::Cancel;
                }
            });
        });
    choice
}

impl AppIde {
    /// "Unsaved changes" modal shown when the window close was intercepted
    /// (see `AppIde::ui`). Save and close / Close without saving / Cancel.
    ///
    /// Saving is asynchronous, so "Save and close" only *starts* the save
    /// (`request_save`) and arms `close_after_save`; the window is closed where
    /// the save worker's result is applied. The prompt stays up meanwhile so
    /// the app can't be closed twice or exited mid-write.
    pub(super) fn show_exit_prompt(&mut self, ui: &egui::Ui) {
        if !self.exit_prompt {
            return;
        }
        let unsaved = self.unsaved_files();
        // Saved from another route (Ctrl+S) while the prompt was up — nothing
        // left to warn about, so just go.
        if unsaved.is_empty() && self.save_in_progress.is_none() {
            self.exit_prompt = false;
            self.allow_close = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        match unsaved_changes_modal(
            ui,
            "exit_unsaved_confirm",
            &unsaved,
            self.save_in_progress.is_some(),
            "Saving — the app closes when it's done…",
            self.selected_build_cfg().is_some(),
            "Save and close",
            "Close without saving",
        ) {
            UnsavedChoice::Save => {
                self.request_save = true;
                self.close_after_save = true;
                // The save trigger sits EARLIER in the frame than this dialog,
                // so it runs next frame — make sure there is one.
                ui.ctx().request_repaint();
            }
            UnsavedChoice::Discard => {
                self.exit_prompt = false;
                self.allow_close = true;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            UnsavedChoice::Cancel => {
                self.exit_prompt = false;
                self.close_after_save = false;
            }
            UnsavedChoice::None => {}
        }
    }

    /// The same modal for **Tools → Open Project**: opening another project
    /// replaces everything in memory, so unsaved work would be gone with no
    /// warning at all — the one destructive path that had none.
    ///
    /// `Save and open` starts the async save and arms `open_after_save`; the
    /// folder picker runs where the save worker's result is applied, so the
    /// files are on disk before the current project is dropped.
    pub(super) fn show_open_project_prompt(&mut self, ui: &egui::Ui, save_needed: &mut bool) {
        if !self.open_prompt {
            return;
        }
        let unsaved = self.unsaved_files();
        // Saved from another route (Ctrl+S) while the prompt was up.
        if unsaved.is_empty() && self.save_in_progress.is_none() {
            self.open_prompt = false;
            self.pick_and_open_project(save_needed);
            return;
        }

        match unsaved_changes_modal(
            ui,
            "open_unsaved_confirm",
            &unsaved,
            self.save_in_progress.is_some(),
            "Saving — the folder picker opens when it's done…",
            self.selected_build_cfg().is_some(),
            "Save and open…",
            "Open without saving",
        ) {
            UnsavedChoice::Save => {
                self.request_save = true;
                self.open_after_save = true;
                ui.ctx().request_repaint();
            }
            UnsavedChoice::Discard => {
                self.open_prompt = false;
                self.pick_and_open_project(save_needed);
            }
            UnsavedChoice::Cancel => {
                self.open_prompt = false;
                self.open_after_save = false;
            }
            UnsavedChoice::None => {}
        }
    }

    /// And once more for **Tools → New Project**, which clears every user file
    /// and folder. Its own confirmation says so, but it never said WHAT would be
    /// lost — this gate names the files and offers to save them first.
    ///
    /// Unlike the other two, "continue" only opens the New Project dialog; the
    /// clearing happens when the user confirms there.
    pub(super) fn show_new_project_prompt(&mut self, ui: &egui::Ui) {
        if !self.new_prompt {
            return;
        }
        let unsaved = self.unsaved_files();
        // Saved from another route (Ctrl+S) while the prompt was up.
        if unsaved.is_empty() && self.save_in_progress.is_none() {
            self.new_prompt = false;
            self.begin_new_project();
            // The New Project dialog is rendered EARLIER in the frame than this
            // gate, so it first appears next frame — guarantee there is one.
            ui.ctx().request_repaint();
            return;
        }

        match unsaved_changes_modal(
            ui,
            "new_project_unsaved_confirm",
            &unsaved,
            self.save_in_progress.is_some(),
            "Saving — New Project continues when it's done…",
            self.selected_build_cfg().is_some(),
            "Save and continue…",
            "Continue without saving",
        ) {
            UnsavedChoice::Save => {
                self.request_save = true;
                self.new_after_save = true;
                ui.ctx().request_repaint();
            }
            UnsavedChoice::Discard => {
                self.new_prompt = false;
                self.begin_new_project();
                ui.ctx().request_repaint();
            }
            UnsavedChoice::Cancel => {
                self.new_prompt = false;
                self.new_after_save = false;
            }
            UnsavedChoice::None => {}
        }
    }

    /// Open the New Project dialog (chip picker + its own "cannot be undone"
    /// confirmation), defaulting the chip selection to Empty.
    pub(super) fn begin_new_project(&mut self) {
        self.confirm_new_project = true;
        self.pending_mcu_id = None;
    }

    /// Native folder picker → load that project. Cancelling the picker leaves
    /// the current project untouched.
    pub(super) fn pick_and_open_project(&mut self, save_needed: &mut bool) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Open Embedded IDE Project — pick the project root folder")
            .pick_folder()
        {
            self.load_project_from_dir(&folder);
            *save_needed = true;
        }
    }

    /// Confirmation for History's "Restore ALL files" — the whole tracked
    /// worktree back to a commit.
    ///
    /// States the three rules plainly rather than listing files: the rules are
    /// what makes the operation safe, and a file list would bury them.
    pub(super) fn show_git_restore_all_dialog(&mut self, ui: &egui::Ui) {
        let Some(sha) = self.git_restore_all_confirm.clone() else {
            return;
        };
        let short = &sha[..sha.len().min(7)];
        let unsaved = self.git.state.lock().unwrap().unsaved.clone();
        let mut keep = true;
        let mut confirmed = false;

        egui::Window::new("Restore ALL files from this commit?")
            .id(egui::Id::new("git_restore_all_confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(500.0);
                ui.add_space(2.0);
                for (icon, text, color) in [
                    (
                        ph::ARROW_COUNTER_CLOCKWISE,
                        format!("Tracked files become their version at {short}."),
                        egui::Color32::from_rgb(220, 210, 190),
                    ),
                    (
                        ph::TRASH,
                        "Files added AFTER it are removed.".to_owned(),
                        egui::Color32::from_rgb(230, 160, 90),
                    ),
                    (
                        ph::CHECK_CIRCLE,
                        "Untracked files are left alone.".to_owned(),
                        egui::Color32::from_rgb(150, 200, 160),
                    ),
                ] {
                    ui.label(
                        egui::RichText::new(format!("{icon}  {text}"))
                            .size(11.5)
                            .color(color),
                    );
                }
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "The branch does NOT move. All of this becomes one uncommitted \
                         change — review it in Changes, then commit it or undo the whole \
                         thing with \"Discard all\".",
                    )
                    .size(10.5)
                    .color(egui::Color32::from_gray(165)),
                );
                if !unsaved.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {} file(s) have unsaved editor changes — the project is \
                             reloaded from disk afterwards, so those are LOST.",
                            ph::WARNING,
                            unsaved.len()
                        ))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(235, 130, 90)),
                    );
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            egui::RichText::new(format!(
                                "{} Restore all",
                                ph::ARROW_COUNTER_CLOCKWISE
                            ))
                            .color(egui::Color32::from_rgb(230, 160, 70)),
                        )
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });

        if confirmed {
            if let Some(dir) = self.git_dir() {
                crate::git::run_restore_tree(
                    sha,
                    dir,
                    std::sync::Arc::clone(&self.git.state),
                    self.egui_ctx.clone(),
                );
            }
            keep = false;
        }
        if !keep {
            self.git_restore_all_confirm = None;
        }
    }

    /// Confirmation for switching branches WHILE there are unsaved editor
    /// changes: the switch reloads the project from disk, so those edits are
    /// lost. No dialog is shown when nothing is unsaved — the switch runs
    /// straight away (see `diag_embed`).
    pub(super) fn show_git_switch_dialog(&mut self, ui: &egui::Ui) {
        let Some(branch) = self.git_switch_confirm.clone() else {
            return;
        };
        let unsaved = self.git.state.lock().unwrap().unsaved.clone();
        let mut keep = true;
        let mut confirmed = false;

        egui::Window::new("Switch branch with unsaved changes?")
            .id(egui::Id::new("git_switch_confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(480.0);
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Checking out \"{branch}\" reloads every file from disk."
                    ))
                    .size(11.5)
                    .color(egui::Color32::from_rgb(220, 210, 190)),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{} {} file(s) have unsaved editor changes — they will be LOST. \
                         Save (Ctrl+S) or Discard them first to keep them.",
                        ph::WARNING,
                        unsaved.len()
                    ))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(230, 160, 90)),
                );
                if !unsaved.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(unsaved.join(" · "))
                            .size(10.0)
                            .color(egui::Color32::from_gray(150)),
                    );
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            egui::RichText::new(format!("{} Switch anyway", ph::GIT_BRANCH))
                                .color(egui::Color32::from_rgb(230, 160, 70)),
                        )
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });

        if confirmed {
            self.git.switch_target = Some(branch);
            self.run_git_op(crate::git::GitOp::SwitchBranch);
            keep = false;
        }
        if !keep {
            self.git_switch_confirm = None;
        }
    }

    /// Confirmation for deleting a local branch from the header picker
    /// (`git branch -D` — force, so it also drops unmerged commits).
    pub(super) fn show_git_delete_branch_dialog(&mut self, ui: &egui::Ui) {
        let Some(branch) = self.git_delete_branch_confirm.clone() else {
            return;
        };
        let mut keep = true;
        let mut confirmed = false;

        egui::Window::new("Delete this branch?")
            .id(egui::Id::new("git_delete_branch_confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(440.0);
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(format!("Delete the local branch \"{branch}\"."))
                        .size(11.5)
                        .color(egui::Color32::from_rgb(220, 210, 190)),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{} Force delete (git branch -D): any commits ONLY on this branch are \
                         lost. It does NOT touch the remote — a branch already deleted there is \
                         just removed locally.",
                        ph::WARNING,
                    ))
                    .size(10.5)
                    .color(egui::Color32::from_rgb(230, 160, 90)),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            egui::RichText::new(format!("{} Delete branch", ph::TRASH))
                                .color(egui::Color32::from_rgb(230, 120, 100)),
                        )
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });

        if confirmed {
            self.git.switch_target = Some(branch);
            self.run_git_op(crate::git::GitOp::DeleteBranch);
            keep = false;
        }
        if !keep {
            self.git_delete_branch_confirm = None;
        }
    }

    /// Confirmation for History's "Restore this file": overwrite one file with
    /// its content at a commit.
    pub(super) fn show_git_restore_dialog(&mut self, ui: &egui::Ui) {
        let Some((sha, path)) = self.git_restore_confirm.clone() else {
            return;
        };
        let short = &sha[..sha.len().min(7)];
        let mut keep = true;
        let mut confirmed = false;
        egui::Window::new("Restore file from history?")
            .id(egui::Id::new("git_restore_confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.add_space(2.0);
                ui.label(egui::RichText::new(&path).monospace().strong());
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Its current content is replaced by the version at {short}. \
                         Uncommitted changes to THIS file are lost."
                    ))
                    .size(12.0),
                );
                ui.add_space(4.0);
                // The reassuring half — this is why the operation is safe.
                ui.label(
                    egui::RichText::new(
                        "Nothing else moves: the branch stays where it is, and the result \
                         is an ordinary uncommitted change you can review or discard.",
                    )
                    .size(10.5)
                    .color(egui::Color32::from_gray(160)),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            egui::RichText::new(format!("{} Restore", ph::ARROW_COUNTER_CLOCKWISE))
                                .color(egui::Color32::from_rgb(230, 160, 70)),
                        )
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
        if confirmed {
            self.pending_restore = Some((sha, path));
            keep = false;
        }
        if !keep {
            self.git_restore_confirm = None;
        }
    }

    /// Confirmation modal for discarding a whole file's changes (Phase A). On
    /// confirm it queues `pending_discard_file` (applied at the next editor
    /// render, so the open editor's `display_code` refreshes). No-op closed.
    pub(super) fn show_git_discard_dialog(&mut self, ui: &egui::Ui) {
        let Some((path, untracked)) = self.git_discard_confirm.clone() else {
            return;
        };
        let mut keep = true;
        let mut confirmed = false;
        egui::Window::new(if untracked {
            "Delete untracked file?"
        } else {
            "Discard changes?"
        })
        .id(egui::Id::new("git_discard_confirm"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(&path).monospace().strong());
            ui.add_space(6.0);
            if untracked {
                ui.label(
                    egui::RichText::new(
                        "This file isn't tracked by git — deleting it is PERMANENT and cannot be undone.",
                    )
                    .size(12.0)
                    .color(egui::Color32::from_rgb(230, 130, 90)),
                );
            } else {
                ui.label(
                    egui::RichText::new(
                        "Restore this file to its last committed version. Uncommitted changes to it will be lost.",
                    )
                    .size(12.0),
                );
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let (label, col) = if untracked {
                    (format!("{} Delete", ph::TRASH), egui::Color32::from_rgb(230, 110, 90))
                } else {
                    (
                        format!("{} Discard", ph::ARROW_COUNTER_CLOCKWISE),
                        egui::Color32::from_rgb(230, 160, 70),
                    )
                };
                if ui.button(egui::RichText::new(label).color(col)).clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    keep = false;
                }
            });
        });

        if confirmed {
            self.pending_discard_file = Some(path);
            keep = false;
        }
        if !keep {
            self.git_discard_confirm = None;
        }

        // ── Discard ALL changes (Phase C) — the strongest confirm ────────────
        if self.git_discard_all_confirm {
            let mut keep_all = true;
            let mut confirmed_all = false;
            egui::Window::new("Discard ALL changes?")
                .id(egui::Id::new("git_discard_all_confirm"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(
                            "Reset EVERY tracked file to the last commit and DELETE all untracked files.",
                        )
                        .size(12.0)
                        .color(egui::Color32::from_rgb(230, 130, 90)),
                    );
                    ui.label(
                        egui::RichText::new("This cannot be undone.")
                            .size(12.0)
                            .strong()
                            .color(egui::Color32::from_rgb(230, 110, 90)),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                egui::RichText::new(format!("{} Discard everything", ph::WARNING))
                                    .color(egui::Color32::from_rgb(230, 100, 85)),
                            )
                            .clicked()
                        {
                            confirmed_all = true;
                        }
                        if ui.button("Cancel").clicked() {
                            keep_all = false;
                        }
                    });
                });
            if confirmed_all {
                self.git_discard_all_confirm = false;
                self.apply_discard_all();
            } else if !keep_all {
                self.git_discard_all_confirm = false;
            }
        }
    }

    /// Bulk-convert STM32 open-pin-data XML file(s) into `.ron` definitions in
    /// the user `mcus/` folder (Phase 3). A range file (`STM32F103C(8-B)Tx`)
    /// expands into several chips; only variants whose form validates are
    /// saved. Result summary lands in `mcu_import_status`.
    fn import_stm32_pin_data(&mut self, paths: &[std::path::PathBuf]) {
        let mut saved = 0usize;
        let mut skipped = 0usize;
        let mut last_id: Option<String> = None;
        let mut first_err: Option<String> = None;

        for path in paths {
            let xml = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    skipped += 1;
                    first_err.get_or_insert(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            match stm32_pin_data::convert_xml(&xml) {
                Ok(chips) => {
                    for chip in chips {
                        let errs = chip.form.errors();
                        if !errs.is_empty() {
                            skipped += 1;
                            first_err
                                .get_or_insert(format!("{}: {}", chip.form.display_name, errs[0]));
                            continue;
                        }
                        let mut def = chip.form.to_definition();
                        // F4's real ceiling is per-chip (F401 84 … F429 180) —
                        // override the form's F411-class default.
                        if def.family == "stm32f4" {
                            def.clock_limits = stm32_pin_data::f4_limits_for_chip(&def.id);
                        }
                        match registry::save_definition(&def) {
                            Ok(_) => {
                                last_id = Some(def.id.clone());
                                registry::merge_def(&mut self.mcu_registry, def);
                                saved += 1;
                            }
                            Err(e) => {
                                skipped += 1;
                                first_err.get_or_insert(e);
                            }
                        }
                    }
                }
                Err(e) => {
                    skipped += 1;
                    first_err.get_or_insert(format!("{}: {e}", path.display()));
                }
            }
        }

        // Select the last chip added, so it's ready in the chip list.
        if let Some(id) = last_id {
            self.pending_mcu_id = Some(id);
        }
        self.mcu_import_status = Some(if saved > 0 {
            let mut msg = format!(
                "{}  Imported {saved} chip(s) from {} file(s)",
                ph::CHECK,
                paths.len()
            );
            if skipped > 0 {
                msg.push_str(&format!("; {skipped} skipped"));
                if let Some(e) = first_err {
                    msg.push_str(&format!(" ({e})"));
                }
            }
            msg
        } else {
            format!(
                "{}  No chips imported{}",
                ph::WARNING,
                first_err.map(|e| format!(": {e}")).unwrap_or_default()
            )
        });
    }

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
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
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
                    self.export_msg = format!("{}  {e}", egui_phosphor::regular::X_CIRCLE);
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
        // Deferred form-open requests (set inside the window closure, which
        // already borrows `self`; acted on after it returns).
        let mut open_form_blank = false;
        let mut open_form_edit: Option<String> = None;
        egui::Window::new("New Project")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_TOP, [20.0, 10.0])
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
                                    self.mcu_import_status = Some(format!("{}  {e}", ph::WARNING));
                                }
                            }
                        }
                    }

                    // ── Import STM32 open-pin-data XML (bulk vendor data) ──
                    if ui
                        .button(
                            egui::RichText::new(format!("{} STM32 XML…", ph::FILE_CODE)).size(12.0),
                        )
                        .on_hover_text(
                            "Bulk-import chips from STMicroelectronics STM32_open_pin_data \
                             XML (mcu/*.xml). One file may add several flash variants.",
                        )
                        .clicked()
                    {
                        if let Some(paths) = rfd::FileDialog::new()
                            .add_filter("STM32 pin-data XML", &["xml"])
                            .set_title("Import STM32 open-pin-data XML")
                            .pick_files()
                        {
                            self.import_stm32_pin_data(&paths);
                        }
                    }

                    // ── New / Edit MCU definition (visual form) ────────────
                    // Deferred to after the window closure — `open_mcu_form`
                    // borrows `self`, already borrowed here.
                    if ui
                        .button(egui::RichText::new(format!("{} New MCU…", ph::WRENCH)).size(12.0))
                        .on_hover_text("Author a new chip definition in a form")
                        .clicked()
                    {
                        open_form_blank = true;
                    }
                    if let Some(id) = &self.pending_mcu_id {
                        if self.mcu_registry.iter().any(|d| &d.id == id) {
                            if ui
                                .button(
                                    egui::RichText::new(format!("{} Edit…", ph::PENCIL_SIMPLE))
                                        .size(12.0),
                                )
                                .on_hover_text("Edit / clone the selected chip's definition")
                                .clicked()
                            {
                                open_form_edit = Some(id.clone());
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
                                // A new chip starts in the default orientation —
                                // never inherit the previous chip's rotation (its
                                // package may differ). The fresh build already
                                // clears it; kept explicit so it can't regress.
                                if let Some(m) = &mut self.mcu {
                                    m.rotated = false;
                                }
                                // Re-fit the Pins canvas to the new chip.
                                self.mcu_view_adjusted = false;
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
                        // A new project starts with the automatic Structure
                        // layout and default view options.
                        self.structure_overrides.clear();
                        self.structure_cache = None;
                        self.structure_view = Default::default();
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

        // ── Act on deferred form-open requests ─────────────────────────────
        if open_form_blank {
            self.open_mcu_form(None);
        } else if let Some(id) = open_form_edit {
            if let Some(def) = self.mcu_registry.iter().find(|d| d.id == id) {
                let seed = crate::panels::mcu_module::mcu_form::McuForm::from_definition(def);
                self.open_mcu_form(Some(seed));
            }
        }
    }
}
