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
                // Drop a recent-project choice this prompt was gating, or it
                // would hijack the NEXT "Open Project…" and skip its picker.
                self.pending_open_dir = None;
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

    /// Open a project: the folder already chosen from "Open Recent", or the
    /// native picker. Cancelling the picker leaves the current project untouched.
    ///
    /// One entry point for both on purpose — this is what the unsaved-changes
    /// gate calls, directly and after a "Save and open…", so a recent project
    /// gets exactly the same protection as a picked one without duplicating
    /// that flow.
    pub(super) fn pick_and_open_project(&mut self, save_needed: &mut bool) {
        let chosen = self.pending_open_dir.take().or_else(|| {
            rfd::FileDialog::new()
                .set_title("Open Embedded IDE Project — pick the project root folder")
                .pick_folder()
        });
        if let Some(folder) = chosen {
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
    pub(super) fn import_stm32_pin_data(&mut self, paths: &[std::path::PathBuf]) {
        self.import_stm32_pin_data_from(paths, None);
    }

    /// The same import, optionally pulling each chip's CLOCK TREE as well.
    ///
    /// `clock_src` is the catalogue source the files came from. When it carries
    /// a CubeMX `db`, every chip gets its real tree instead of the family
    /// template — which for the families that have no template (H5, H7, U5, F3,
    /// C0, WB, WL, L0/L1/L5) is the difference between a Clock tab that works
    /// and one that says the chip has no clock.
    ///
    /// A clock that cannot be read is NOT fatal: the chip still imports with its
    /// pins, and the reason is collected for the status line. Losing a whole
    /// import over a missing RCC file would be a poor trade.
    pub(super) fn import_stm32_pin_data_from(
        &mut self,
        paths: &[std::path::PathBuf],
        clock_src: Option<&crate::panels::mcu_module::chip_sources::ChipSource>,
    ) {
        let mut clocks = 0usize;
        let mut clock_err: Option<String> = None;
        let mut unbound_ids: Vec<String> = Vec::new();
        let mut saved = 0usize;
        let mut skipped = 0usize;
        let mut last_id: Option<String> = None;
        let mut first_err: Option<String> = None;
        // Everything an imported chip arrives missing, per chip, in ONE verdict:
        // a HAL chip feature `embassy-stm32` does not publish, a clock tree no
        // RCC recipe can turn into code, or no DMA channels at all.
        //
        // One list rather than three counters because STM32WL30 had all three at
        // once and the import reported none of them — each was found separately,
        // in generated code, days apart. A chip that fails every check should say
        // so in one sentence, at the moment it is imported.
        //
        // The HAL half costs one index lookup per import, cached below; when the
        // index is unreadable (offline, or past the 4 s timeout this runs under
        // on the UI thread) the verdict is `Unverified`, which is its own line —
        // "we could not check" and "it is fine" are different answers.
        let mut gap_chips: Vec<String> = Vec::new();
        // The supporting-file caches, now owned by one struct so the routine
        // that uses them can live outside this method.
        let mut caches = ImportCaches::default();
        // Stays HERE, not in the extracted routine: it is one network lookup for
        // the whole run, and a function that can be called from a test has no
        // business reaching for the crates.io index.
        let mut embassy_features: Option<Option<Vec<String>>> = None;

        for path in paths {
            let xml = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    skipped += 1;
                    first_err.get_or_insert(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            let made = definitions_from_file(path, &xml, clock_src, &mut caches);
            unbound_ids.extend(made.unbound);
            if let Some(e) = made.clock_err {
                clock_err.get_or_insert(e);
            }
            clocks += made.clocks;
            for e in made.errors {
                skipped += 1;
                first_err.get_or_insert(e);
            }
            for def in made.defs {
                        let hal = match stm32_pin_data::embassy_feature_in(&def.project.hal_dep)
                        {
                            Some(feat) => {
                                let known = embassy_features.get_or_insert_with(|| {
                                    crate::app::editor_panel::cargo_complete::known_features(
                                        stm32_pin_data::EMBASSY_CRATE,
                                        stm32_pin_data::EMBASSY_VERSION,
                                    )
                                });
                                feature_verdict(feat, known.as_deref())
                            }
                            // No embassy feature in the HAL line at all (STM32F1
                            // uses `stm32f1xx-hal`): nothing to check, nothing
                            // missing.
                            None => FeatureVerdict::Present,
                        };
                        // `to_config` rather than the XML tree: an F1 chip carries
                        // `ClockDef::Stm32f1` and generates clock code even though
                        // `convert_xml` gave it nothing, and reporting that as a
                        // gap would be a false alarm on the family that works best.
                        let gaps = chip_gaps(
                            &hal,
                            crate::panels::mcu_module::codegen::rcc::generates_clock_code_for(
                                &def.family,
                                &def.clock.to_config(&def.clock_limits),
                            ),
                            uses_dma_def(&def.family)
                                .then(|| def.dma.as_ref().map_or(0, |d| d.channels.len())),
                        );
                        if !gaps.is_empty() {
                            gap_chips.push(format!("{} ({})", def.display_name, gaps.join(", ")));
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
            if clocks > 0 {
                msg.push_str(&format!("; {clocks} with their clock tree"));
            }
            if let Some(e) = &clock_err {
                // The chip is in; only its clock is not.
                msg.push_str(&format!(
                    "
{}  No clock tree imported: {e}",
                    ph::WARNING
                ));
            }
            if !unbound_ids.is_empty() {
                unbound_ids.sort();
                unbound_ids.dedup();
                msg.push_str(&format!(
                    "
{}  {} clock id(s) unbound ({}) — those values fall back to a                      default in the generated code.",
                    ph::WARNING,
                    unbound_ids.len(),
                    unbound_ids.join(", ")
                ));
            }
            if !gap_chips.is_empty() {
                // Named here, at import, because every alternative is worse: the
                // HAL gap surfaces as a project that will not resolve, the clock
                // gap as commented-out code in `main.rs`, and the DMA gap as a
                // `DMA_TX_TODO` placeholder — three separate mysteries, none of
                // which names the chip as the cause.
                let shown: Vec<&str> = gap_chips.iter().take(3).map(String::as_str).collect();
                msg.push_str(&format!(
                    "
{}  {} chip(s) imported with gaps — a project on one of these will                      not fully build: {}{}. HAL features checked against {} {}.",
                    ph::WARNING,
                    gap_chips.len(),
                    shown.join(" | "),
                    if gap_chips.len() > shown.len() {
                        " | …"
                    } else {
                        ""
                    },
                    stm32_pin_data::EMBASSY_CRATE,
                    stm32_pin_data::EMBASSY_VERSION,
                ));
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
                // The dialog outgrew the screen. It is anchored and NOT
                // resizable, so a window taller than the viewport simply has an
                // unreachable bottom - and expanding the Filters twisty adds
                // five sliders, a ten-cell grid and fifty chips at once.
                //
                // The ACTION ROW stays outside this: a Create button that
                // scrolls away is the same bug wearing a smaller hat.
                egui::ScrollArea::vertical()
                    .id_salt("new_project_body")
                    .max_height(ui.ctx().content_rect().height() * 0.70)
                    .show(ui, |ui| {
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
                                    ui.selectable_value(
                                        &mut self.pending_mcu_id,
                                        None,
                                        "— Empty —",
                                    );
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
                        });

                        // What this chip cannot do, BEFORE the button that acts
                        // on it. Creating a project replaces the current one, so
                        // learning it afterwards costs the project you had —
                        // the System tab tells you the same thing one
                        // irreversible click too late.
                        let gaps = self.pending_chip_gaps().to_vec();
                        if !gaps.is_empty() {
                            ui.add_space(3.0);
                            for g in &gaps {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "    {}  {g}",
                                        ph::ARROW_ELBOW_DOWN_RIGHT
                                    ))
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(230, 190, 90)),
                                );
                            }
                            ui.label(
                                egui::RichText::new(
                                    "    Not a block: the rest still generates; these parts come out as comments or TODOs.",
                                )
                                .size(10.5)
                                .color(egui::Color32::GRAY),
                            );
                        }

                        // ── Search the vendor data on this machine ────────────
                        // The row above picks from what the IDE already knows; this
                        // reaches the ~2800 parts ST ships data for, by part number
                        // rather than by hunting for the file that happens to hold it.
                        ui.add_space(6.0);
                        ui.separator();
                        self.show_chip_search(ui);
                        ui.separator();
                        // ── Imports ───────────────────────────────────────────────
                        // Collapsed by default: these four are how chip data GETS
                        // here, which is a rarer job than picking a chip that is
                        // already here. Keeping them on the Chip row made that row
                        // read as four actions with a dropdown attached.
                        ui.add_space(4.0);
                        egui::CollapsingHeader::new(
                            egui::RichText::new(format!("{} Imports", ph::PLUS))
                                .size(10.5)
                                .color(egui::Color32::GRAY),
                        )
                        .id_salt("new_project_imports")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                // ── Import MCU… (runtime .ron import) ──────────────
                                if ui
                                    .button(
                                        egui::RichText::new(format!("{} Import…", ph::PLUS))
                                            .size(12.0),
                                    )
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
                                                // Checked here too, though the line came from
                                                // someone else's file: whoever wrote it, a chip the
                                                // crate does not publish, whose tree makes no clock
                                                // code, or that has no DMA channels fails the same
                                                // way for the person importing it. "It is their
                                                // file" explains where it came from; it does not
                                                // help the user whose project will not build.
                                                let hal = match stm32_pin_data::embassy_feature_in(
                                                    &def.project.hal_dep,
                                                ) {
                                                    Some(feat) => feature_verdict(
                                                        feat,
                                                        crate::app::editor_panel::cargo_complete::known_features(
                                                            stm32_pin_data::EMBASSY_CRATE,
                                                            stm32_pin_data::EMBASSY_VERSION,
                                                        )
                                                        .as_deref(),
                                                    ),
                                                    None => FeatureVerdict::Present,
                                                };
                                                let gaps = chip_gaps(
                                                    &hal,
                                                    crate::panels::mcu_module::codegen::rcc::generates_clock_code_for(
                                                        &def.family,
                                                        &def.clock.to_config(&def.clock_limits),
                                                    ),
                                                    uses_dma_def(&def.family).then(|| {
                                                        def.dma.as_ref().map_or(0, |d| d.channels.len())
                                                    }),
                                                );
                                                let feat_note = if gaps.is_empty() {
                                                    String::new()
                                                } else {
                                                    format!(" — {}", gaps.join("; "))
                                                };
                                                registry::merge_def(&mut self.mcu_registry, def);
                                                self.pending_mcu_id = Some(id);
                                                let note = if codegen::family::backend_for(&fam)
                                                    .is_none()
                                                {
                                                    format!(" — no codegen backend for '{fam}'")
                                                } else {
                                                    feat_note
                                                };
                                                self.mcu_import_status = Some(format!(
                                                    "{}  Imported {name}{note}",
                                                    ph::CHECK
                                                ));
                                            }
                                            Err(e) => {
                                                self.mcu_import_status =
                                                    Some(format!("{}  {e}", ph::WARNING));
                                            }
                                        }
                                    }
                                }

                                // ── Import STM32 open-pin-data XML (bulk vendor data) ──
                                if ui
                                .button(
                                    egui::RichText::new(format!("{} STM32 XML…", ph::FILE_CODE))
                                        .size(12.0),
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
                                    .button(
                                        egui::RichText::new(format!("{} New MCU…", ph::WRENCH))
                                            .size(12.0),
                                    )
                                    .on_hover_text("Author a new chip definition in a form")
                                    .clicked()
                                {
                                    open_form_blank = true;
                                }
                                if let Some(id) = &self.pending_mcu_id {
                                    if self.mcu_registry.iter().any(|d| &d.id == id) {
                                        if ui
                                            .button(
                                                egui::RichText::new(format!(
                                                    "{} Edit…",
                                                    ph::PENCIL_SIMPLE
                                                ))
                                                .size(12.0),
                                            )
                                            .on_hover_text(
                                                "Edit / clone the selected chip's definition",
                                            )
                                            .clicked()
                                        {
                                            open_form_edit = Some(id.clone());
                                        }
                                    }
                                }
                            });
                            // ── Import-folder discoverability ──────────────────────────
                            // Show where user .ron definitions live + a one-click "Open".
                            if let Some(dir) = registry::user_mcus_dir() {
                                let path_str = dir.display().to_string();
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} Import folder:",
                                            ph::FOLDER
                                        ))
                                        .size(10.5)
                                        .color(egui::Color32::GRAY),
                                    );
                                    if ui
                                        .button(egui::RichText::new("Open").size(10.5))
                                        .on_hover_text(format!(
                                            "Open {path_str}\n(drop .ron files here)"
                                        ))
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
                        });

                        // Last import result (persists until the popup closes).
                        if let Some(msg) = &self.mcu_import_status {
                            let col = import_status_colour(msg);
                            ui.add_space(2.0);
                            ui.label(egui::RichText::new(msg).size(11.0).color(col));
                        }
                    });
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(
                            egui::RichText::new(format!("{} New Project", ph::NOTE_PENCIL))
                                .color(egui::Color32::from_rgb(220, 80, 60)),
                        )
                        .clicked()
                    {
                        // ── Adopt the chosen chip ─────────────────────────
                        // UNCONDITIONALLY, including "— Empty —" (`None`) and
                        // re-picking the chip that is already selected. Both used
                        // to be skipped — the first left the previous project's
                        // chip and code in place behind an "Empty" label, the
                        // second kept every configured pin and the user's own
                        // main.rs tail in a project that says it is new.
                        self.selected_mcu_id = self.pending_mcu_id.take().unwrap_or_default();
                        self.mcu = Self::build_mcu_for(&self.mcu_registry, &self.selected_mcu_id);
                        // Ask the index what this chip's HAL feature is, once,
                        // off-thread. Picking a chip is the last moment before
                        // a project exists on it — after this the answer only
                        // arrives as a manifest cargo cannot resolve, which
                        // kills rust-analyzer for the whole project and says
                        // nothing about the chip.
                        self.start_hal_check();
                        // A new chip starts in the default orientation — never
                        // inherit the previous chip's rotation (its package may
                        // differ). The fresh build already clears it; kept
                        // explicit so it can't regress.
                        if let Some(m) = &mut self.mcu {
                            m.rotated = false;
                        }
                        // Re-fit the Pins canvas to the new chip.
                        self.mcu_view_adjusted = false;
                        // No chip, no code — `init_frame` only regenerates while
                        // `mcu` is `Some`, so an empty project would otherwise
                        // keep showing the previous chip's main.rs forever.
                        self.generated_code = self
                            .mcu
                            .as_ref()
                            .map(|m| m.fresh_main_rs())
                            .unwrap_or_default();
                        // Re-arm the change detector: the fresh MCU must look
                        // new to `init_frame` even when the chip is unchanged.
                        self.mcu_state_hash = 0;
                        // System first — Runtime is the choice everything else
                        // is generated from, so it comes before pins. With no
                        // chip it is also where the picker lives.
                        self.active_tab = McuTab::System;
                        self.lsp_state.lock().unwrap().reset();
                        self.lsp_selected_diagnostic = None;
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
                        // New project = fresh deps → drop the stale workspace lock
                        // so the next check re-resolves (saves otherwise keep it).
                        self.reset_workspace_lock();
                        *save_project_needed = true;
                        // Both of these only make sense once there is a chip.
                        // `pins/mod.rs` is generated code, which an empty project
                        // must not have; and the overlay would announce a load
                        // with nothing to load (`write_project` is skipped
                        // without a build config, so the chain never starts).
                        if self.mcu.is_some() {
                            // Pre-populate the pins/ scaffold so the tree shows
                            // the folder immediately, before any pin is configured.
                            self.project_tree.init_pins_scaffold();
                            // Nothing is read from disk here, but everything after
                            // still runs (workspace rewrite, RA restart on a chip
                            // change, re-index, check) — same wait, same overlay.
                            self.begin_project_loading(super::loading_overlay::LoadKind::New);
                        }
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

/// What the crates.io index says about one chip's `embassy-stm32` feature.
///
/// Pulled out of the import loop so it can be TESTED. The distinction that
/// matters is the third variant: for a long time "we could not look it up" and
/// "it is fine" were the same branch, so a chip whose feature does not exist —
/// an STM32WL30, where embassy-stm32 publishes no `stm32wl3*` at all — arrived
/// looking verified whenever the index lookup timed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FeatureVerdict {
    /// The index lists it.
    Present,
    /// The index was read, and does not list it. The project will not resolve.
    Missing,
    /// The index could not be read — offline, or past the lookup timeout.
    Unverified,
}

/// `known` is `None` when the index lookup failed, `Some(list)` when it worked.
/// What colour the import report is painted.
///
/// Three outcomes, not two, and the third is why this is a function rather than
/// a line inside the `ui` closure: the gap report is APPENDED to a message that
/// starts with the tick, so keying the colour on the first glyph alone painted
/// "2 chip(s) imported with gaps — a project on one of these will not fully
/// build" in success green. That is the one line the reader most needs to stop
/// at.
///
/// Extracted because the alternative was a rule nothing could check. Whether
/// egui paints it is still something only a person can see; WHICH colour it is
/// told to paint no longer is.
pub(super) fn import_status_colour(msg: &str) -> egui::Color32 {
    if !msg.starts_with(ph::CHECK) {
        // Nothing was imported at all.
        egui::Color32::from_rgb(220, 120, 90)
    } else if msg.contains(ph::WARNING) {
        // Imported, but something in it has to be read.
        egui::Color32::from_rgb(230, 190, 90)
    } else {
        egui::Color32::from_rgb(120, 200, 120)
    }
}

pub(super) fn feature_verdict(feat: &str, known: Option<&[String]>) -> FeatureVerdict {
    match known {
        None => FeatureVerdict::Unverified,
        Some(list) if list.iter().any(|f| f == feat) => FeatureVerdict::Present,
        Some(_) => FeatureVerdict::Missing,
    }
}

#[cfg(test)]
mod feature_verdict_tests {
    use super::*;

    /// Reading the code was not enough to be sure of this — twice. So it is a
    /// test: a failed lookup must NOT be reported as a good feature.
    #[test]
    fn a_failed_lookup_is_not_a_pass() {
        let known: Vec<String> = vec!["stm32f411re".into(), "stm32g431cb".into()];
        assert_eq!(
            feature_verdict("stm32f411re", Some(&known)),
            FeatureVerdict::Present
        );
        assert_eq!(
            feature_verdict("stm32wl30kb", Some(&known)),
            FeatureVerdict::Missing,
            "the index was read and does not have it"
        );
        assert_eq!(
            feature_verdict("stm32wl30kb", None),
            FeatureVerdict::Unverified,
            "offline is not proof of anything"
        );
        // An empty list is still an ANSWER, and the answer is no.
        assert_eq!(feature_verdict("stm32f411re", Some(&[])), FeatureVerdict::Missing);
    }
}

    /// Every route that puts a chip definition into the registry must have
    /// decided about its HAL feature — or be listed here as knowingly exempt.
    ///
    /// Written after getting this wrong TWICE in one afternoon, in opposite
    /// directions: first "the check does not exist", then "chip search does not
    /// call it". Both were reasoning about which function delegates to which,
    /// and both were wrong. This answers the question by reading the source
    /// instead, and it fails the day a fourth route is added — which is when
    /// nobody will be thinking about the third.
    #[test]
    fn every_import_route_decides_about_the_hal_feature() {
        // fn name -> why it does not check. Anything not here must check.
        const EXEMPT: &[(&str, &str)] = &[
            (
                "show_mcu_form_dialog",
                "the user typed the dependency line themselves, in the form",
            ),
            (
                "save_clock_to_definition",
                "re-saves a chip already in the registry; the dep line is untouched",
            ),
            // `show_new_project_dialog` used to be exempt here, importing
            // someone else's `.ron` unchecked. It now checks like the rest —
            // an exemption is a decision, and that one did not survive being
            // written down and looked at.
        ];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
        let mut files = vec![root.join("dialogs.rs"), root.join("mcu_form_dialog.rs")];
        files.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app.rs"));

        let mut unchecked: Vec<String> = Vec::new();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let lines: Vec<&str> = text.lines().collect();
            // (line, name) of every `fn`, in order — the owner of a call is the
            // last one declared before it.
            let fns: Vec<(usize, String)> = lines
                .iter()
                .enumerate()
                .filter_map(|(i, l)| {
                    let rest = l.trim_start().split_once("fn ")?.1;
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    (!name.is_empty()).then_some((i, name))
                })
                .collect();

            for (i, line) in lines.iter().enumerate() {
                if !(line.contains("registry::merge_def(")
                    || line.contains("registry::save_definition("))
                {
                    continue;
                }
                let Some((start, name)) = fns.iter().rev().find(|(f, _)| *f < i) else {
                    continue;
                };
                if EXEMPT.iter().any(|(n, _)| n == name) {
                    continue;
                }
                let end = fns
                    .iter()
                    .find(|(f, _)| f > start)
                    .map_or(lines.len(), |(f, _)| *f);
                if !lines[*start..end].iter().any(|l| l.contains("feature_verdict")) {
                    unchecked.push(format!(
                        "{}:{}  fn {name} stores a definition without checking its HAL feature",
                        path.file_name().unwrap().to_string_lossy(),
                        i + 1
                    ));
                }
            }
        }
        assert!(
            unchecked.is_empty(),
            "a chip can reach the registry without its embassy feature being \
             checked.\nEither call `feature_verdict`, or add the function to \
             EXEMPT with the reason:\n{}",
            unchecked.join("\n")
        );
    }

/// The per-file caches the importer carries across a bulk run.
///
/// 98 GPIO AF tables serve 2240 chips and a DMA table serves a whole family, so
/// a family import reads each supporting file once instead of once per part.
#[derive(Default)]
pub(super) struct ImportCaches {
    af: std::collections::HashMap<String, Option<std::sync::Arc<stm32_pin_data::GpioAf>>>,
    irq: std::collections::HashMap<String, Vec<String>>,
    dma: std::collections::HashMap<String, Option<crate::panels::mcu_module::mcu_def::DmaDef>>,
}

/// What one vendor file yields.
#[derive(Default)]
pub(super) struct FileImport {
    pub defs: Vec<crate::panels::mcu_module::mcu_def::McuDefinition>,
    /// How many of `defs` carry the vendor's own clock tree.
    pub clocks: usize,
    pub unbound: Vec<String>,
    pub clock_err: Option<String>,
    /// One line per chip that could not be made, ready for the status line.
    pub errors: Vec<String>,
}

/// Everything the importer knows how to build from ONE vendor XML.
///
/// Extracted from the dialog method so it can be called without an `AppIde` —
/// which is to say, so it can be TESTED and scripted. The GUI method keeps what
/// is genuinely its own: the counters, the status line, the registry, and the
/// HAL-feature lookup that needs the network.
pub(super) fn definitions_from_file(
    path: &std::path::Path,
    xml: &str,
    clock_src: Option<&crate::panels::mcu_module::chip_sources::ChipSource>,
    caches: &mut ImportCaches,
) -> FileImport {
    let mut out = FileImport::default();

    // Once per file: a range file's variants are the same silicon and therefore
    // the same clock tree.
    let mut clock: Option<crate::panels::mcu_module::clock::graph::GraphClock> = None;
    if let Some(db) = clock_src.and_then(|s| s.db.as_deref()) {
        let family = stm32_pin_data::convert_xml(xml)
            .ok()
            .and_then(|c| c.first().map(|c| c.form.family.clone()))
            .unwrap_or_default();
        match crate::panels::mcu_module::clock::graph::cubemx::graph_for_chip_xml(db, xml, &family)
        {
            Ok((gc, missing)) => {
                out.unbound.extend(missing);
                clock = Some(gc);
            }
            Err(e) => {
                out.clock_err.get_or_insert(e);
            }
        }
    }

    // The AF indices live in a sibling file, `<mcu dir>/IP/GPIO-<ver>_Modes.xml`.
    let af = stm32_pin_data::gpio_ip_version(xml).and_then(|ver| {
        if let Some(t) = caches.af.get(&ver) {
            return t.as_ref().map(std::sync::Arc::clone);
        }
        let file = path
            .parent()
            .map(|d| d.join("IP").join(stm32_pin_data::gpio_ip_file_name(&ver)));
        let table = file
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|text| std::sync::Arc::new(stm32_pin_data::GpioAf::parse(&text)));
        caches.af.insert(ver, table.clone());
        table
    });

    // The DMA channels come from two more files in that same `IP/` folder. Only
    // the STM32Cube database ships them — importing from the public
    // open-pin-data repo leaves this `None`, and codegen falls back to the
    // hand-written family tables.
    let irq_vectors =
        crate::panels::mcu_module::codegen::nvic::vectors_for(xml, path.parent(), &mut caches.irq);
    let dma = crate::panels::mcu_module::codegen::dma_data::dma_def_for(
        xml,
        path.parent(),
        &mut caches.dma,
    );

    let chips = match stm32_pin_data::convert_xml_with_af(xml, af.as_deref()) {
        Ok(chips) => chips,
        Err(e) => {
            out.errors.push(format!("{}: {e}", path.display()));
            return out;
        }
    };
    for chip in chips {
        let errs = chip.form.errors();
        if !errs.is_empty() {
            out.errors
                .push(format!("{}: {}", chip.form.display_name, errs[0]));
            continue;
        }
        let mut def = chip.form.to_definition();
        def.dma = dma.clone();
        def.irq_vectors = irq_vectors.clone();
        def.usart_ip = stm32_pin_data::usart_ip_version(xml);
        def.sdmmc_ip = stm32_pin_data::sdmmc_ip_version(xml);
        // F4's real ceiling is per-chip (F401 84 … F429 180) — override the
        // form's F411-class default.
        if def.family == "stm32f4" {
            def.clock_limits = stm32_pin_data::f4_limits_for_chip(&def.id);
        }
        // F2 shares the F4 clock TREE but none of its ceilings: 120 MHz HCLK
        // with APB /4 and /2, per embassy's own `#[cfg(stm32f2)] mod max` —
        // which is `rcc_assert!`, so exceeding it panics at boot on a debug
        // build. Imports before this carried F4's 100/50/100.
        if def.family == "stm32f2" {
            def.clock_limits = crate::panels::mcu_module::clock::graph::stm32f2_limits();
        }
        // The vendor's own tree replaces the family template. `convert_xml` can
        // only offer what the IDE ships for the family — `ClockChoice::None`
        // for most of them — so this is where a chip stops arriving clock-less.
        if let Some(gc) = &clock {
            def.clock = crate::panels::mcu_module::mcu_def::ClockDef::Graph(gc.clone());
            out.clocks += 1;
        }
        out.defs.push(def);
    }
    out
}

/// Does this family's codegen actually allocate from the definition's DMA table?
///
/// Only the embassy backends do. STM32F1 takes its channel from the HAL, which
/// fixes it in the TYPE, and ESP32 gets its own from esp-hal — so an empty
/// `DmaDef` there means nothing at all, and reporting it as a gap is a false
/// alarm on the two families most likely to be installed.
///
/// The same two names `generates_clock_code` special-cases, for the same
/// underlying reason: they are the families that do not go through embassy.
pub(super) fn uses_dma_def(family: &str) -> bool {
    !matches!(family, "stm32f1" | "esp32c3")
}

/// Everything about a chip that will not work, in one list.
///
/// Three independent gaps, and the STM32WL30 that started this had all three at
/// once: `embassy-stm32` publishes no `stm32wl3*` feature, its RCC is a
/// different architecture so no clock reaches `main.rs`, and its single global
/// `DMA_IRQn` yielded no channels. The import reported none of them, so they
/// were found one at a time, in generated code, over two days.
///
/// Pure and primitive-taking on purpose: the callers already hold these three
/// facts, and a function that took a definition would need a network lookup to
/// be testable.
pub(super) fn chip_gaps(
    hal: &FeatureVerdict,
    clock_generates: bool,
    dma_channels: Option<usize>,
) -> Vec<String> {
    let mut out = Vec::new();
    match hal {
        FeatureVerdict::Missing => out.push("no HAL support (the crate publishes no such chip feature)".into()),
        FeatureVerdict::Unverified => out.push("HAL feature unverified (offline?)".into()),
        FeatureVerdict::Present => {}
    }
    if !clock_generates {
        out.push("no clock code (its tree cannot be turned into an RCC config)".into());
    }
    // `None` is not zero: it means the question does not apply here.
    if dma_channels == Some(0) {
        out.push("no DMA channels found for it".into());
    }
    out
}

impl AppIde {
    /// Start the off-thread HAL-feature lookup for the selected chip.
    ///
    /// Does nothing when the chip's HAL line carries no `embassy-stm32` feature
    /// (STM32F1 uses `stm32f1xx-hal`, ESP32 its own): there is nothing to look
    /// up, and an empty slot would read as "still loading" forever.
    pub(super) fn start_hal_check(&mut self) {
        self.hal_check = None;
        let Some(mcu) = &self.mcu else { return };
        let Some(def) = self.mcu_registry.iter().find(|d| d.id == mcu.id) else {
            return;
        };
        let Some(feat) = stm32_pin_data::embassy_feature_in(&def.project.hal_dep) else {
            return;
        };
        let feat = feat.to_owned();
        let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
        self.hal_check = Some((mcu.id.clone(), slot.clone()));
        std::thread::spawn(move || {
            let known = crate::app::editor_panel::cargo_complete::known_features(
                stm32_pin_data::EMBASSY_CRATE,
                stm32_pin_data::EMBASSY_VERSION,
            );
            *slot.lock().unwrap() = Some(feature_verdict(&feat, known.as_deref()));
        });
    }

    /// What the chip staged in the New Project dialog is missing.
    ///
    /// Only the two free halves — the HAL feature would mean a network lookup
    /// per dropdown flick. The System tab asks that one, once, after the chip
    /// is adopted.
    pub(super) fn pending_chip_gaps(&mut self) -> &[String] {
        let id = self.pending_mcu_id.clone().unwrap_or_default();
        if self.new_project_gaps.as_ref().map(|(k, _)| k.as_str()) != Some(id.as_str()) {
            let gaps = self
                .mcu_registry
                .iter()
                .find(|d| d.id == id)
                .map(|d| {
                    local_chip_gaps(
                        codegen::rcc::generates_clock_code_for(
                            &d.family,
                            &d.clock.to_config(&d.clock_limits),
                        ),
                        uses_dma_def(&d.family)
                            .then(|| d.dma.as_ref().map_or(0, |x| x.channels.len())),
                    )
                })
                .unwrap_or_default();
            self.new_project_gaps = Some((id, gaps));
        }
        self.new_project_gaps
            .as_ref()
            .map(|(_, g)| g.as_slice())
            .unwrap_or(&[])
    }

    /// The verdict, if it has landed AND belongs to the chip on screen.
    ///
    /// The chip check is not paranoia: pick a chip, pick another before the
    /// lookup returns, and without it the second chip would wear the first
    /// one's answer.
    pub(super) fn hal_verdict_now(&self) -> Option<FeatureVerdict> {
        let (id, slot) = self.hal_check.as_ref()?;
        verdict_for(&self.mcu.as_ref()?.id, id, *slot.lock().ok()?)
    }
}

/// A verdict is only shown when it belongs to the chip currently on screen.
///
/// Not paranoia — a real race: pick a chip, pick another before the index
/// answers, and the second chip wears the first one's verdict. A wrong verdict
/// is worse than none, because it is exactly as confident.
pub(super) fn verdict_for(
    on_screen: &str,
    was_checked: &str,
    landed: Option<FeatureVerdict>,
) -> Option<FeatureVerdict> {
    (on_screen == was_checked).then_some(landed).flatten()
}

/// The half of [`chip_gaps`] that costs nothing to ask.
///
/// The HAL feature lives in the crates.io index, and the caller for this one is
/// a `ui` function that runs on every repaint — a lookup there would be a network
/// stall per frame. So this asks only the two questions answerable from the
/// chip's own data, and **does not claim anything about the HAL feature**: the
/// import path checks that one, once, and says so in its report.
///
/// It exists because the import report is not where most chips are met. One can
/// arrive from a shared `.ron`, from the recent-projects list, or in a project
/// someone else made — and then nothing had ever told the user why their clock
/// is a commented skeleton and their DMA a `TODO`.
pub(super) fn local_chip_gaps(clock_generates: bool, dma_channels: Option<usize>) -> Vec<String> {
    chip_gaps(&FeatureVerdict::Present, clock_generates, dma_channels)
}

#[cfg(test)]
mod real_import_tests {
    use super::*;

    /// Import STM32WL30KBVx for real, through the SAME routine the dialog uses.
    ///
    /// Not a mock and not a parallel implementation — that is the whole point of
    /// `definitions_from_file` existing: until it was pulled out of the dialog
    /// method, nothing about importing a chip could be run without a window.
    ///
    /// Writes to the user's chip registry, so it is `#[ignore]`d:
    ///
    /// ```text
    /// cargo test --bin embedded_ide_0 import_wl30_for_real -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "writes a definition into the user's chip registry"]
    fn import_wl30_for_real() {
        use crate::panels::mcu_module::chip_sources;

        let Some(src) = chip_sources::all_sources()
            .into_iter()
            .find(|s| s.has_clock())
        else {
            println!("no source with clock trees - nothing imported");
            return;
        };
        let path = src.chips.join("STM32WL30KBVx.xml");
        if !path.is_file() {
            println!("{} is not in {}", path.display(), src.chips.display());
            return;
        }
        let xml = std::fs::read_to_string(&path).expect("read the vendor file");

        let mut caches = ImportCaches::default();
        let made = definitions_from_file(&path, &xml, Some(&src), &mut caches);
        println!(
            "from {}: {} definition(s), {} with a clock tree, {} unbound id(s), errors={:?}",
            src.chips.display(),
            made.defs.len(),
            made.clocks,
            made.unbound.len(),
            made.errors
        );
        assert!(made.errors.is_empty(), "{:?}", made.errors);
        assert_eq!(made.defs.len(), 1, "one part in this file");

        let def = &made.defs[0];
        let channels = def.dma.as_ref().map_or(0, |d| d.channels.len());
        println!(
            "  {} family={} dma_channels={} irq_vectors={} clock={}",
            def.display_name,
            def.family,
            channels,
            def.irq_vectors.len(),
            matches!(
                def.clock,
                crate::panels::mcu_module::mcu_def::ClockDef::Graph(_)
            )
        );
        // The reason the old stored definition was wrong: it predated the
        // `parse_value` fix and carried no channels at all.
        assert!(channels > 0, "the DMA channels must survive the import");

        match registry::save_definition(def) {
            Ok(p) => println!("  saved: {p:?}"),
            Err(e) => panic!("could not save: {e}"),
        }
    }
}

#[cfg(test)]
mod chip_gaps_tests {
    use super::*;

    #[test]
    fn a_chip_with_everything_reports_nothing() {
        assert!(chip_gaps(&FeatureVerdict::Present, true, Some(8)).is_empty());
    }

    /// The WL30 case, which is why this exists: three gaps, and the import used
    /// to mention none of them.
    #[test]
    fn all_three_gaps_are_reported_together() {
        let g = chip_gaps(&FeatureVerdict::Missing, false, Some(0));
        assert_eq!(g.len(), 3, "{g:?}");
        assert!(g[0].contains("no HAL support"), "{g:?}");
        assert!(g[1].contains("no clock code"), "{g:?}");
        assert!(g[2].contains("no DMA"), "{g:?}");
    }

    #[test]
    fn a_verdict_never_leaks_onto_another_chip() {
        let v = Some(FeatureVerdict::Missing);
        assert_eq!(verdict_for("stm32wl30kb", "stm32wl30kb", v), v, "same chip");
        assert_eq!(
            verdict_for("stm32g071cb", "stm32wl30kb", v),
            None,
            "a verdict for another chip must not be shown"
        );
        // Still looking: no answer either way.
        assert_eq!(verdict_for("stm32wl30kb", "stm32wl30kb", None), None);
    }

    /// A family that does not allocate from `DmaDef` must never be told it has
    /// no DMA.
    ///
    /// STM32F1 takes its channel from the HAL (fixed in the TYPE) and ESP32
    /// from esp-hal, so their definitions carry no channel list and never
    /// needed one. Counting that as a gap put a false warning on the two
    /// families most likely to be installed — which is where it was found.
    #[test]
    fn only_embassy_families_are_asked_about_dma() {
        assert!(!uses_dma_def("stm32f1"), "F1 fixes the channel in the type");
        assert!(!uses_dma_def("esp32c3"), "esp-hal brings its own");
        assert!(uses_dma_def("stm32g0"), "embassy allocates from the table");
        assert!(uses_dma_def("stm32wl3"));

        // `None` is the shape those families pass, and it must stay silent.
        assert!(local_chip_gaps(true, None).is_empty(), "no DMA question to answer");
        assert_eq!(local_chip_gaps(false, None).len(), 1, "clock gap still reported");
    }

    /// The local variant must never say anything about the HAL feature — it did
    /// not ask. It passes `Present` to reuse one list-builder, and a reader
    /// could easily mistake that for an answer; this is what stops it becoming
    /// one.
    #[test]
    fn the_local_variant_is_silent_about_the_hal() {
        for (clock, dma) in [(true, Some(8)), (false, Some(0)), (true, Some(0)), (false, Some(8))] {
            for line in local_chip_gaps(clock, dma) {
                assert!(!line.contains("HAL"), "it did not check the HAL: {line}");
            }
        }
        // It still reports the two it DID ask about.
        assert_eq!(local_chip_gaps(false, Some(0)).len(), 2);
        assert!(local_chip_gaps(true, Some(8)).is_empty());
    }

    /// "Could not check" is its own line, never silence and never a clean bill.
    #[test]
    fn an_unverified_feature_is_still_said_out_loud() {
        let g = chip_gaps(&FeatureVerdict::Unverified, true, Some(4));
        assert_eq!(g.len(), 1);
        assert!(g[0].contains("unverified"), "{g:?}");
    }

    /// The two halves that can be checked WITHOUT the network, run against the
    /// real vendor database: STM32WL30 must show both gaps, and a control chip
    /// neither.
    ///
    /// This is the case that motivated the whole preflight. Its clock tree is a
    /// different architecture (`PLL64RC` / `ROOTClkSource` / `SYSCLKDIV`, no
    /// `sw` or `ahb` node) so `generic_recipe` declines it, and it has no family
    /// recipe either — the generated `main.rs` kept a commented skeleton. The
    /// DMA half is here as a REGRESSION guard the other way round: WL30 had 0
    /// channels until `parse_value` started reading the vendor's own range, and
    /// a test asserting "WL30 has no channels" would now be asserting the bug.
    ///
    /// Ignored because it needs the database:
    ///
    /// ```text
    /// cargo test --bin embedded_ide_0 wl30_is_the_chip_this_preflight_exists_for -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs the STM32Cube database"]
    fn wl30_is_the_chip_this_preflight_exists_for() {
        use crate::panels::mcu_module::chip_sources;
        use crate::panels::mcu_module::clock::graph::cubemx::graph_for_chip_xml;
        use crate::panels::mcu_module::clock::model::ClockConfig;
        use crate::panels::mcu_module::codegen::rcc::generates_clock_code_for;
        use crate::panels::mcu_module::stm32_pin_data::convert_xml;

        let Some(src) = chip_sources::all_sources()
            .into_iter()
            .find(|s| s.has_clock())
        else {
            println!("no CubeMX installation — nothing checked");
            return;
        };
        let db = src.db.as_deref().unwrap();
        let files: Vec<std::path::PathBuf> = std::fs::read_dir(&src.chips)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        let mut cache = std::collections::HashMap::new();

        // (prefix, expected clock code, expected "has DMA channels")
        for (prefix, want_clock, want_dma) in
            [("STM32WL30", false, true), ("STM32G071", true, true)]
        {
            let Some(file) = files.iter().find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(prefix) && n.ends_with(".xml"))
            }) else {
                // NOT a skip. The database is present (the matrix gates this
                // case on that), so a part missing from it means the search is
                // wrong, or the installation is too old to answer the question
                // this test is named after. Passing quietly would report a
                // verdict nobody checked.
                panic!("{prefix} is not in the database at {}", src.chips.display());
            };
            let xml = std::fs::read_to_string(file).unwrap();
            let family = convert_xml(&xml).unwrap()[0].form.family.clone();

            let clock = match graph_for_chip_xml(db, &xml, &family) {
                Ok((gc, _)) => ClockConfig::Graph(gc),
                // No tree at all is itself a clock gap, not a test failure.
                Err(e) => {
                    println!("  no tree: {e}");
                    ClockConfig::None
                }
            };
            let has_clock = generates_clock_code_for(&family, &clock);
            let channels = dma_def_channels(&xml, file, &mut cache);
            println!("{prefix} ({family}): clock={has_clock} dma_channels={channels}");

            assert_eq!(has_clock, want_clock, "{prefix} clock verdict");
            assert_eq!(channels > 0, want_dma, "{prefix} DMA verdict ({channels})");

            // And the verdict the import actually prints, built from those two.
            let gaps = chip_gaps(&FeatureVerdict::Present, has_clock, Some(channels));
            println!("  gaps: {gaps:?}");
            assert_eq!(gaps.is_empty(), want_clock && want_dma);
        }
    }

    /// `dma_def_for` wants the chip directory, not the file.
    fn dma_def_channels(
        xml: &str,
        file: &std::path::Path,
        cache: &mut std::collections::HashMap<
            String,
            Option<crate::panels::mcu_module::mcu_def::DmaDef>,
        >,
    ) -> usize {
        crate::panels::mcu_module::codegen::dma_data::dma_def_for(xml, file.parent(), cache)
            .map_or(0, |d| d.channels.len())
    }
}

#[cfg(test)]
mod import_status_colour_tests {
    use super::*;

    const RED: egui::Color32 = egui::Color32::from_rgb(220, 120, 90);
    const AMBER: egui::Color32 = egui::Color32::from_rgb(230, 190, 90);
    const GREEN: egui::Color32 = egui::Color32::from_rgb(120, 200, 120);

    #[test]
    fn a_clean_import_is_green() {
        assert_eq!(
            import_status_colour(&format!("{}  Imported 3 chip(s) from 1 file(s)", ph::CHECK)),
            GREEN
        );
    }

    /// THE case this exists for: the gap report is appended to a message that
    /// already starts with the tick, so it used to come out in success green.
    #[test]
    fn an_import_carrying_gaps_is_amber_not_green() {
        let msg = format!(
            "{}  Imported 3 chip(s) from 1 file(s)
{}  2 chip(s) imported with gaps",
            ph::CHECK,
            ph::WARNING
        );
        assert_eq!(import_status_colour(&msg), AMBER, "a warning may not read as success");
    }

    #[test]
    fn importing_nothing_is_red() {
        assert_eq!(
            import_status_colour(&format!("{}  No chips imported", ph::WARNING)),
            RED
        );
    }

    /// The three are distinct: a test that passed while two of them were the
    /// same colour would be checking nothing.
    #[test]
    fn the_three_outcomes_do_not_share_a_colour() {
        assert_ne!(RED, AMBER);
        assert_ne!(AMBER, GREEN);
        assert_ne!(RED, GREEN);
    }
}
