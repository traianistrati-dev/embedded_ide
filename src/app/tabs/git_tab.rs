//! Bottom-panel "Git" tab — status, changed files, commit/push/pull, output.
//!
//! Buttons don't run git themselves: they set `op_out` (the `clippy_run`
//! signal pattern) and `AppIde::run_git_op` spawns the worker with the
//! project dir + the in-memory snapshot for the unsaved-changes warning.
//! Commits are STRICTLY what's on disk — the amber banner warns when the
//! editors hold unsaved edits a commit would miss.

use crate::git::{GitConsole, GitLine, GitOp};
use eframe::egui;
use egui_phosphor::regular as ph;

pub fn show_git_tab(
    ui: &mut egui::Ui,
    git: &mut GitConsole,
    project_dir: Option<&std::path::Path>,
    op_out: &mut Option<GitOp>,
) {
    let Some(project_dir) = project_dir else {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!(
                "{}  Salvează proiectul întâi (Ctrl+S) — git rulează în folderul proiectului.",
                ph::GIT_BRANCH
            ))
            .size(12.0)
            .color(egui::Color32::from_gray(150)),
        );
        return;
    };

    // Snapshot the shared state for this frame (short lock).
    let (busy, loaded, is_repo, git_missing, status, unsaved, commit_ok) = {
        let mut st = git.state.lock().unwrap();
        let commit_ok = std::mem::take(&mut st.commit_succeeded);
        (
            st.busy,
            st.loaded,
            st.is_repo,
            st.git_missing,
            st.status.clone(),
            st.unsaved.clone(),
            commit_ok,
        )
    };
    if commit_ok {
        git.commit_msg.clear();
    }
    // First open → load the status once, automatically.
    if !loaded && busy.is_none() {
        *op_out = Some(GitOp::Refresh);
    }

    // ── Header row: branch / upstream / ahead-behind / refresh ───────────────
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        if let Some(op) = busy {
            crate::app::helpers::spinner::throttled_spinner(ui, 12.0);
            ui.label(
                egui::RichText::new(format!(" git {op}…"))
                    .size(11.5)
                    .color(egui::Color32::from_rgb(220, 180, 70)),
            );
            ui.separator();
        }
        if git_missing {
            ui.label(
                egui::RichText::new(format!(
                    "{} `git` nu e instalat — https://git-scm.com",
                    ph::X_CIRCLE
                ))
                .size(11.5)
                .color(egui::Color32::from_rgb(220, 90, 80)),
            );
            return;
        }
        if is_repo {
            let branch = status.branch.as_deref().unwrap_or("(detached)");
            ui.label(
                egui::RichText::new(format!("{} {branch}", ph::GIT_BRANCH))
                    .size(12.0)
                    .strong()
                    .color(egui::Color32::from_rgb(150, 195, 235)),
            );
            if let Some(up) = &status.upstream {
                ui.label(
                    egui::RichText::new(format!("→ {up}"))
                        .size(11.0)
                        .color(egui::Color32::from_gray(140)),
                );
                let ab = format!("↑{} ↓{}", status.ahead, status.behind);
                let col = if status.ahead > 0 || status.behind > 0 {
                    egui::Color32::from_rgb(220, 180, 70)
                } else {
                    egui::Color32::from_gray(120)
                };
                ui.label(egui::RichText::new(ab).size(11.0).color(col));
            }
            let n = status.changes.len();
            ui.label(
                egui::RichText::new(format!(
                    "· {n} {}",
                    if n == 1 { "modificare" } else { "modificări" }
                ))
                .size(11.0)
                .color(egui::Color32::from_gray(140)),
            );
        } else if loaded {
            ui.label(
                egui::RichText::new("nu e un repository git")
                    .size(11.5)
                    .color(egui::Color32::from_gray(150)),
            );
            if ui
                .add_enabled(busy.is_none(), egui::Button::new("git init"))
                .clicked()
            {
                *op_out = Some(GitOp::Init);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(busy.is_none(), egui::Button::new(format!("{} Refresh", ph::ARROWS_CLOCKWISE)))
                .clicked()
            {
                *op_out = Some(GitOp::Refresh);
            }
            ui.label(
                egui::RichText::new(project_dir.to_string_lossy())
                    .size(10.0)
                    .color(egui::Color32::from_gray(100)),
            );
        });
    });

    // ── Unsaved-changes warning (commit uses ONLY what's on disk) ────────────
    if !unsaved.is_empty() {
        let n = unsaved.len();
        let resp = ui.label(
            egui::RichText::new(format!(
                "{}  {n} fișier{} cu modificări NESALVATE — commit-ul include doar starea de pe disc. Salvează întâi (Ctrl+S).",
                ph::WARNING,
                if n == 1 { "" } else { "e" }
            ))
            .size(11.5)
            .color(egui::Color32::from_rgb(230, 180, 60)),
        );
        resp.on_hover_text(unsaved.join("\n"));
    }
    ui.separator();

    // ── Body: changes list (left) + output scrollback (right) ────────────────
    let footer_h = 30.0;
    let body_h = (ui.available_height() - footer_h).max(40.0);
    ui.horizontal_top(|ui| {
        let total = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(total * 0.38, body_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("git_changes")
                    .max_height(body_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if status.changes.is_empty() && is_repo {
                            ui.label(
                                egui::RichText::new("fără modificări")
                                    .size(11.0)
                                    .italics()
                                    .color(egui::Color32::from_gray(120)),
                            );
                        }
                        for c in &status.changes {
                            let col = match c.code.as_str() {
                                "??" => egui::Color32::from_gray(140),
                                s if s.contains('U') => egui::Color32::from_rgb(230, 90, 80),
                                s if s.starts_with('.') => egui::Color32::from_rgb(220, 160, 70),
                                _ => egui::Color32::from_rgb(120, 200, 130),
                            };
                            ui.label(
                                egui::RichText::new(format!("{:>2}  {}", c.code, c.path))
                                    .monospace()
                                    .size(11.0)
                                    .color(col),
                            );
                        }
                    });
            },
        );
        ui.separator();
        ui.vertical(|ui| {
            egui::ScrollArea::vertical()
                .id_salt("git_output")
                .max_height(body_h)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let lines = git.state.lock().unwrap().lines.clone();
                    for (kind, text) in &lines {
                        let col = match kind {
                            GitLine::Cmd => egui::Color32::from_rgb(150, 195, 235),
                            GitLine::Out => egui::Color32::from_gray(190),
                            GitLine::Err => egui::Color32::from_rgb(220, 150, 90),
                            GitLine::Notice => egui::Color32::from_gray(130),
                        };
                        ui.label(egui::RichText::new(text).monospace().size(10.5).color(col));
                    }
                });
        });
    });

    // ── Footer: commit message + actions ─────────────────────────────────────
    ui.horizontal(|ui| {
        let idle = busy.is_none() && is_repo && !git_missing;
        let msg_edit = egui::TextEdit::singleline(&mut git.commit_msg)
            .desired_width((ui.available_width() * 0.35).max(160.0))
            .hint_text("mesaj de commit…");
        ui.add_enabled(idle, msg_edit);
        let has_msg = !git.commit_msg.trim().is_empty();
        if ui
            .add_enabled(idle && has_msg, egui::Button::new(format!("{} Commit", ph::CHECK)))
            .clicked()
        {
            *op_out = Some(GitOp::Commit);
        }
        if ui
            .add_enabled(
                idle && has_msg,
                egui::Button::new(format!("{} Commit & Push", ph::ARROW_SQUARE_UP)),
            )
            .clicked()
        {
            *op_out = Some(GitOp::CommitPush);
        }
        ui.separator();
        if ui
            .add_enabled(idle, egui::Button::new(format!("{} Pull", ph::ARROW_DOWN)))
            .clicked()
        {
            *op_out = Some(GitOp::Pull);
        }
        if ui
            .add_enabled(idle, egui::Button::new(format!("{} Push", ph::ARROW_UP)))
            .clicked()
        {
            *op_out = Some(GitOp::Push);
        }
        if ui
            .add_enabled(idle, egui::Button::new(format!("{} Fetch", ph::ARROWS_DOWN_UP)))
            .clicked()
        {
            *op_out = Some(GitOp::Fetch);
        }
        if ui
            .add_enabled(idle, egui::Button::new(format!("{} Log", ph::LIST_DASHES)))
            .clicked()
        {
            *op_out = Some(GitOp::Log);
        }
    });
}
