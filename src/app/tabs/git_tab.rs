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
    // Set to `(git path, 1-based line)` when the user clicks an added (green)
    // row in the diff view — the caller opens that file in the editor and
    // scrolls to the line.
    open_file: &mut Option<(String, usize)>,
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
    let (busy, loaded, is_repo, git_missing, status, unsaved, commit_ok, diff, remote_url) = {
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
            st.diff.clone(),
            st.remote_url.clone(),
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
            } else if let Some(url) = &remote_url {
                // Remote configured but no upstream yet — the first Push will
                // create it (`push -u origin HEAD`).
                ui.label(
                    egui::RichText::new(format!("remote: {url} (primul Push setează upstream-ul)"))
                        .size(10.5)
                        .color(egui::Color32::from_gray(130)),
                );
            } else {
                // No remote at all: inline "Set remote" field — paste the
                // GitHub URL (https or ssh) of an empty repo here.
                ui.add(
                    egui::TextEdit::singleline(&mut git.remote_url_draft)
                        .desired_width(260.0)
                        .hint_text("https://github.com/user/repo.git"),
                );
                let has_url = !git.remote_url_draft.trim().is_empty();
                if ui
                    .add_enabled(
                        busy.is_none() && has_url,
                        egui::Button::new(format!("{} Set remote", ph::PLUG)),
                    )
                    .on_hover_text(
                        "git remote add origin <url> — creează întâi repo-ul (gol) pe GitHub, \
                         apoi lipește URL-ul aici. Autentificarea la primul push o face \
                         Git Credential Manager (fereastră de browser).",
                    )
                    .clicked()
                {
                    *op_out = Some(GitOp::SetRemote);
                }
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
    // Drop stale exclusions (files no longer changed) so the checkbox set
    // always mirrors the visible list.
    git.excluded
        .retain(|p| status.changes.iter().any(|c| &c.path == p));
    let any_checked = status
        .changes
        .iter()
        .any(|c| !git.excluded.contains(&c.path));

    // ── Actions — a BOTTOM panel, declared before the body so it claims its
    //    space first (fixed at the bottom, below the file list; never clipped
    //    by a short panel). ────────────────────────────────────────────────
    egui::Panel::bottom("git_actions")
        .exact_size(58.0)
        .show_inside(ui, |ui| {
            ui.add_space(2.0);
            ui.separator();
            let idle = busy.is_none() && is_repo && !git_missing;
            let remoted = idle && remote_url.is_some();
            let has_msg = !git.commit_msg.trim().is_empty();
            let can_commit = idle && has_msg && any_checked;
            ui.add_enabled(
                idle,
                egui::TextEdit::singleline(&mut git.commit_msg)
                    .desired_width(f32::INFINITY)
                    .hint_text("mesaj de commit…"),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_commit, egui::Button::new(format!("{} Commit", ph::CHECK)))
                    .on_disabled_hover_text("scrie un mesaj și bifează cel puțin un fișier modificat")
                    .clicked()
                {
                    *op_out = Some(GitOp::Commit);
                }
                if ui
                    .add_enabled(
                        can_commit && remoted,
                        egui::Button::new(format!("{} Commit & Push", ph::ARROW_SQUARE_UP)),
                    )
                    .on_disabled_hover_text("cere mesaj, fișiere bifate și un remote setat")
                    .clicked()
                {
                    *op_out = Some(GitOp::CommitPush);
                }
                ui.separator();
                if ui
                    .add_enabled(
                        remoted && status.has_commits,
                        egui::Button::new(format!("{} Push", ph::ARROW_UP)),
                    )
                    .on_disabled_hover_text(
                        "cere un remote setat și cel puțin un commit local (fă întâi Commit)",
                    )
                    .clicked()
                {
                    *op_out = Some(GitOp::Push);
                }
                if ui
                    .add_enabled(remoted, egui::Button::new(format!("{} Pull", ph::ARROW_DOWN)))
                    .clicked()
                {
                    *op_out = Some(GitOp::Pull);
                }
                if ui
                    .add_enabled(remoted, egui::Button::new(format!("{} Fetch", ph::ARROWS_DOWN_UP)))
                    .clicked()
                {
                    *op_out = Some(GitOp::Fetch);
                }
                if ui
                    .add_enabled(
                        idle && status.has_commits,
                        egui::Button::new(format!("{} Log", ph::LIST_DASHES)),
                    )
                    .clicked()
                {
                    *op_out = Some(GitOp::Log);
                }
            });
        });

    // ── Body: changes list (left) + output scrollback (right) ────────────────
    let body_h = ui.available_height().max(40.0);
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
                            // Selected = its diff is open in the right pane.
                            let open = diff.as_ref().is_some_and(|d| d.path == c.path);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                // Checked = included in the next commit.
                                let mut on = !git.excluded.contains(&c.path);
                                if ui
                                    .checkbox(&mut on, "")
                                    .on_hover_text("bifat = intră în commit")
                                    .changed()
                                {
                                    if on {
                                        git.excluded.remove(&c.path);
                                    } else {
                                        git.excluded.insert(c.path.clone());
                                    }
                                }
                                // File name — CLICK OPENS ITS DIFF in the right
                                // pane (highlighted while open).
                                let resp = ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!("{:>2}  {}", c.code, c.path))
                                            .monospace()
                                            .size(11.0)
                                            .color(col)
                                            .background_color(if open {
                                                egui::Color32::from_rgb(45, 55, 70)
                                            } else {
                                                egui::Color32::TRANSPARENT
                                            }),
                                    )
                                    .sense(egui::Sense::click())
                                    .selectable(false),
                                );
                                if resp.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if resp
                                    .on_hover_text(
                                        "click: arată modificările (disc vs HEAD) în dreapta",
                                    )
                                    .clicked()
                                    && busy.is_none()
                                {
                                    crate::git::run_diff(
                                        c.path.clone(),
                                        c.code == "??",
                                        project_dir.to_path_buf(),
                                        std::sync::Arc::clone(&git.state),
                                        ui.ctx().clone(),
                                    );
                                }
                            });
                        }
                    });
            },
        );
        ui.separator();
        ui.vertical(|ui| {
            if let Some(d) = &diff {
                // ── Diff view (replaces the output while open) ────────────
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} {}", ph::GIT_DIFF, d.path))
                            .size(11.5)
                            .strong()
                            .color(egui::Color32::from_rgb(150, 195, 235)),
                    );
                    ui.label(
                        egui::RichText::new(format!("+{}", d.added))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(110, 200, 120)),
                    );
                    ui.label(
                        egui::RichText::new(format!("-{}", d.removed))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(230, 105, 95)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(egui::RichText::new(format!("{} închide", ph::X)).size(10.5))
                            .clicked()
                        {
                            git.state.lock().unwrap().diff = None;
                        }
                    });
                });
                egui::ScrollArea::both()
                    .id_salt("git_diff")
                    .max_height(body_h - 24.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let diff_path = d.path.clone();
                        for row in &d.rows {
                            // Added (green) rows are clickable → jump to that
                            // line in the editor; the rest are static.
                            let (text, col, jump_line) = match row {
                                crate::git::DiffRow::Hunk(h) => (
                                    h.clone(),
                                    egui::Color32::from_rgb(110, 145, 200),
                                    None,
                                ),
                                crate::git::DiffRow::Ctx(o, n, t) => (
                                    format!("{o:>4} {n:>4}   {t}"),
                                    egui::Color32::from_gray(150),
                                    None,
                                ),
                                crate::git::DiffRow::Del(o, t) => (
                                    format!("{o:>4}      - {t}"),
                                    egui::Color32::from_rgb(230, 105, 95),
                                    None,
                                ),
                                crate::git::DiffRow::Add(n, t) => (
                                    format!("     {n:>4} + {t}"),
                                    egui::Color32::from_rgb(110, 200, 120),
                                    Some(*n as usize),
                                ),
                            };
                            let rich = egui::RichText::new(text).monospace().size(10.5).color(col);
                            match jump_line {
                                Some(line) => {
                                    let resp = ui.add(
                                        egui::Label::new(rich)
                                            .sense(egui::Sense::click())
                                            .selectable(false),
                                    );
                                    if resp.hovered() {
                                        ui.ctx()
                                            .set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    if resp
                                        .on_hover_text("click: deschide fișierul la această linie")
                                        .clicked()
                                    {
                                        *open_file = Some((diff_path.clone(), line));
                                    }
                                }
                                None => {
                                    ui.label(rich);
                                }
                            }
                        }
                    });
            } else {
                // ── Output scrollback ─────────────────────────────────────
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
            }
        });
    });

}
