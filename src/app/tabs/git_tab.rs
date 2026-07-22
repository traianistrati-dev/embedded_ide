//! Bottom-panel "Git" tab — status, changed files, commit/push/pull, output.
//!
//! Buttons don't run git themselves: they set `op_out` (the `clippy_run`
//! signal pattern) and `AppIde::run_git_op` spawns the worker with the
//! project dir + the in-memory snapshot for the unsaved-changes warning.
//! Commits are STRICTLY what's on disk — the amber banner warns when the
//! editors hold unsaved edits a commit would miss.

use crate::git::{GitConsole, GitLine, GitOp, GitView};
use eframe::egui;
use egui_phosphor::regular as ph;

/// `true` for files the IDE generates / manages, which must NOT be discarded
/// through git (they're rebuilt from the pin/clock model, or belong to a
/// generated block). main.rs is better reverted hunk-by-hunk in the editor
/// gutter. Everything else (the user's own `src/` modules) is discardable.
pub(crate) fn is_ide_managed(path: &str) -> bool {
    matches!(
        path,
        "src/main.rs"
            | "Cargo.toml"
            | ".cargo/config.toml"
            | "memory.x"
            | "build.rs"
            | ".gitignore"
    ) || path == crate::panels::mcu_module::mcu_config::FILE_NAME
        || crate::project_tree::gui::generated_file_reason(path).is_some()
}

pub fn show_git_tab(
    ui: &mut egui::Ui,
    git: &mut GitConsole,
    project_dir: Option<&std::path::Path>,
    op_out: &mut Option<GitOp>,
    // Set to `(git path, 1-based line)` when the user clicks an added (green)
    // row in the diff view — the caller opens that file in the editor and
    // scrolls to the line.
    open_file: &mut Option<(String, usize)>,
    // Set to `(git path, hunk row index)` when the user clicks a hunk's revert
    // button — the caller reverses just that hunk (Phase B).
    revert_hunk: &mut Option<(String, usize)>,
    // History view (read-only): set when a commit is selected / one of its
    // files is clicked. The caller spawns the `git diff-tree` / `git show`.
    commit_load: &mut Option<String>,
    commit_file_load: &mut Option<(String, String)>,
    // `(sha, path)` when "Restore this file" is clicked; the caller confirms
    // first, then rewrites just that file.
    restore_from_commit: &mut Option<(String, String)>,
    // Sha when "Restore ALL files" is clicked; the caller confirms first.
    restore_all_from_commit: &mut Option<String>,
    // Set to `(git path, is_untracked)` when the user clicks a file's discard
    // button — the caller confirms, then restores it to HEAD (tracked) or
    // deletes it (untracked) (Phase A).
    discard_out: &mut Option<(String, bool)>,
    // Set true when the user clicks "Discard all" — the caller confirms, then
    // resets every file to HEAD + deletes untracked files (Phase C).
    discard_all_out: &mut bool,
    // Workspace-member crate names (extracted libraries), for the Library view.
    libraries: &[String],
    // Set to `(library, remote url, branch)` when "Push to its repository" is
    // clicked; the caller spawns `git subtree push`.
    lib_push_out: &mut Option<(String, String, String)>,
) {
    let Some(project_dir) = project_dir else {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!(
                "{}  Save the project first (Ctrl+S) — git runs in the project folder.",
                ph::GIT_BRANCH
            ))
            .size(12.0)
            .color(egui::Color32::from_gray(150)),
        );
        return;
    };

    // Snapshot the shared state for this frame (short lock).
    let (busy, loaded, is_repo, git_missing, status, unsaved, commit_ok, diff, remote_url, log, commit_files, commit_files_sha) = {
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
            st.log.clone(),
            st.commit_files.clone(),
            st.commit_files_sha.clone(),
        )
    };
    if commit_ok {
        git.commit_msg.clear();
    }
    // First open → load the status once, automatically.
    if !loaded && busy.is_none() {
        *op_out = Some(GitOp::Refresh);
    }
    let log_is_empty = log.is_empty();
    // Set when the view switches — the two halves share `state.diff`.
    let mut state_diff_clear = false;
    // Set when a commit / a commit's file is picked; loaded after the borrows end.
    let mut load_commit: Option<String> = None;
    let mut load_commit_file: Option<(String, String)> = None;
    // "Restore this file" in the History diff header.
    let mut restore_req: Option<(String, String)> = None;
    // "Restore all files" for the selected commit.
    let mut restore_all_req: Option<String> = None;

    // ── Changes | History switch ─────────────────────────────────────────────
    // History is strictly READ-ONLY (log / diff-tree / show); nothing in it can
    // touch the worktree.
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        for (v, label) in [
            (GitView::Changes, "Changes"),
            (GitView::History, "History"),
            (GitView::Library, "Library"),
        ] {
            if ui
                .selectable_label(git.view == v, egui::RichText::new(label).size(11.0))
                .clicked()
                && git.view != v
            {
                git.view = v;
                // The two views share `state.diff`; carrying one's diff into the
                // other would show a working-tree diff under a commit header.
                state_diff_clear = true;
                if v == GitView::History && log_is_empty {
                    *op_out = Some(GitOp::Log);
                }
            }
        }
        if git.view == GitView::History {
            ui.separator();
            if ui
                .add_enabled(
                    busy.is_none(),
                    egui::Button::new(egui::RichText::new(format!("{} Reload", ph::ARROWS_CLOCKWISE)).size(11.0)),
                )
                .clicked()
            {
                *op_out = Some(GitOp::Log);
            }
        }
    });
    ui.separator();

    // ── Library view: publish a workspace member to its own repository ───────
    // `git subtree push` grafts the library's history onto a separate remote
    // while the parent repo keeps the real files — so cloning the project still
    // works and the cargo workspace member stays valid. Push-only by design.
    if git.view == GitView::Library {
        show_library_panel(ui, git, libraries, &status, busy.is_some(), is_repo, project_dir, lib_push_out);
        return;
    }

    // ── Header row: branch / upstream / ahead-behind / refresh ───────────────
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        if let Some(op) = busy {
            crate::app::helpers::spinner::throttled_spinner(ui, 12.0);
            ui.label(
                egui::RichText::new(format!(" git {op} {}", ph::DOTS_THREE))
                    .size(11.5)
                    .color(egui::Color32::from_rgb(220, 180, 70)),
            );
            ui.separator();
        }
        if git_missing {
            ui.label(
                egui::RichText::new(format!(
                    "{} `git` is not installed — https://git-scm.com",
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
                    egui::RichText::new(format!("{} {up}", ph::ARROW_RIGHT))
                        .size(11.0)
                        .color(egui::Color32::from_gray(140)),
                );
                let ab = format!(
                    "{}{} {}{}",
                    ph::ARROW_UP,
                    status.ahead,
                    ph::ARROW_DOWN,
                    status.behind
                );
                let col = if status.ahead > 0 || status.behind > 0 {
                    egui::Color32::from_rgb(220, 180, 70)
                } else {
                    egui::Color32::from_gray(120)
                };
                ui.label(egui::RichText::new(ab).size(11.0).color(col));
            } else if remote_url.is_some() {
                // Remote configured but no upstream yet — the first Push will
                // create it (`push -u origin HEAD`). The URL itself is shown
                // by the repo row below, for every state.
                ui.label(
                    egui::RichText::new("(first Push sets the upstream)")
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
                        "git remote add origin <url> — create the (empty) repo on GitHub first, \
                         then paste its URL here. Authentication on the first push is handled by \
                         Git Credential Manager (a browser window).",
                    )
                    .clicked()
                {
                    *op_out = Some(GitOp::SetRemote);
                }
            }
            let n = status.changes.len();
            ui.label(
                egui::RichText::new(format!(
                    "{} {n} {}",
                    ph::DOT,
                    if n == 1 { "change" } else { "changes" }
                ))
                .size(11.0)
                .color(egui::Color32::from_gray(140)),
            );
        } else if loaded {
            ui.label(
                egui::RichText::new("not a git repository")
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
                .add_enabled(
                    busy.is_none(),
                    egui::Button::new(format!("{} Refresh", ph::ARROWS_CLOCKWISE)),
                )
                .clicked()
            {
                *op_out = Some(GitOp::Refresh);
            }
            ui.label(
                egui::RichText::new(project_dir.to_string_lossy())
                    .size(10.0)
                    .color(egui::Color32::from_gray(100)),
            )
            .on_hover_text("Local folder — git runs here");
        });
    });

    // ── Repository row: WHERE this project pushes ───────────────────────────
    // Shown in every state (the old header only revealed the URL in the brief
    // window between `remote add` and the first push, so day to day you could
    // not tell which repository you were pushing to — `origin/main` names the
    // remote, not the address).
    if is_repo {
        if let Some(raw) = &remote_url {
            let info = crate::git::parse_remote_url(raw);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("{} repo", ph::GIT_FORK))
                        .size(10.5)
                        .color(egui::Color32::from_gray(130)),
                );
                // The NAME is what identifies the repository; the scheme,
                // credentials and `.git` suffix are noise. `safe_url` in the
                // tooltip is credential-masked — the raw string never renders.
                match &info.web_url {
                    Some(web) => {
                        ui.hyperlink_to(
                            egui::RichText::new(&info.name).size(11.5).strong(),
                            web,
                        )
                        .on_hover_text(format!("{}\n\nOpens in your browser", info.safe_url));
                    }
                    None => {
                        // A local path has nothing a browser can open.
                        ui.label(egui::RichText::new(&info.name).size(11.5).strong())
                            .on_hover_text(&info.safe_url);
                    }
                }
                if !info.host.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("on {}", info.host))
                            .size(10.0)
                            .color(egui::Color32::from_gray(125)),
                    );
                }
                if ui
                    .add_enabled(
                        busy.is_none(),
                        egui::Button::new(egui::RichText::new(format!("{} Change", ph::PENCIL_SIMPLE)).size(10.5)),
                    )
                    .on_hover_text("Point this project at a different repository")
                    .clicked()
                {
                    // Pre-fill with the CURRENT url so a small edit (a typo, a
                    // rename) doesn't mean retyping the whole address.
                    git.remote_url_draft = raw.clone();
                    git.changing_remote = true;
                }
            });

            if git.changing_remote {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("New URL:").size(11.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut git.remote_url_draft)
                            .desired_width(330.0)
                            .hint_text("https://github.com/user/repo.git"),
                    );
                    let changed = git.remote_url_draft.trim() != raw.trim();
                    if ui
                        .add_enabled(
                            busy.is_none() && changed,
                            egui::Button::new(egui::RichText::new("Save").size(11.0)),
                        )
                        .on_disabled_hover_text("Edit the URL first")
                        .clicked()
                    {
                        match crate::git::validate_remote_url(&git.remote_url_draft) {
                            Ok(()) => {
                                *op_out = Some(GitOp::ChangeRemote);
                                git.changing_remote = false;
                                git.remote_note = None;
                            }
                            Err(e) => git.remote_note = Some(e),
                        }
                    }
                    if ui.button(egui::RichText::new("Cancel").size(11.0)).clicked() {
                        git.changing_remote = false;
                        git.remote_note = None;
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "Only re-points where this project pushes — your files and history \
                             are untouched. The upstream is cleared, so the next Push re-creates it.",
                        )
                        .size(10.0)
                        .color(egui::Color32::from_gray(140))
                        .italics(),
                    );
                });
                if let Some(note) = &git.remote_note {
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(note)
                                .size(10.5)
                                .color(egui::Color32::from_rgb(220, 120, 100)),
                        );
                    });
                }
            }
        }
    }

    // ── Unsaved-changes warning (commit uses ONLY what's on disk) ────────────
    if !unsaved.is_empty() {
        let n = unsaved.len();
        let resp = ui.label(
            egui::RichText::new(format!(
                "{}  {n} file{} with UNSAVED changes — the commit only includes what's on disk. Save first (Ctrl+S).",
                ph::WARNING,
                if n == 1 { "" } else { "s" }
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
            ui.add_space(5.0);
            // ui.separator();
            let idle = busy.is_none() && is_repo && !git_missing;
            let remoted = idle && remote_url.is_some();
            let has_msg = !git.commit_msg.trim().is_empty();
            let can_commit = idle && has_msg && any_checked;
            ui.horizontal(|ui| {
                // Conventional-commit prefix dropdown — picking a type
                // prepends it to the message (best-practice guidance in the
                // per-item tooltips, shown after a 1 s hover).
                ui.scope(|ui| {
                    ui.style_mut().interaction.tooltip_delay = 1.0;
                    egui::ComboBox::from_id_salt("commit_prefix")
                        .selected_text(format!("type {}", ph::CARET_DOWN))
                        .width(76.0)
                        .show_ui(ui, |ui| {
                            for (prefix, desc) in crate::git::COMMIT_TYPES {
                                if ui
                                    .selectable_label(false, *prefix)
                                    .on_hover_text(*desc)
                                    .clicked()
                                {
                                    git.commit_msg =
                                        crate::git::apply_commit_prefix(&git.commit_msg, prefix);
                                }
                            }
                        })
                        .response
                        .on_hover_text("Conventional-commit type — prepended to the message.");
                });
                ui.add_enabled(
                    idle,
                    egui::TextEdit::singleline(&mut git.commit_msg)
                        .desired_width(f32::INFINITY)
                        .hint_text("commit message..."),
                );
            });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        can_commit,
                        egui::Button::new(format!("{} Commit", ph::CHECK)),
                    )
                    .on_disabled_hover_text("write a message and check at least one changed file")
                    .clicked()
                {
                    *op_out = Some(GitOp::Commit);
                }
                if ui
                    .add_enabled(
                        can_commit && remoted,
                        egui::Button::new(format!("{} Commit & Push", ph::ARROW_SQUARE_UP)),
                    )
                    .on_disabled_hover_text(
                        "needs a message, checked files, and a configured remote",
                    )
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
                        "needs a configured remote and at least one local commit (Commit first)",
                    )
                    .clicked()
                {
                    *op_out = Some(GitOp::Push);
                }
                if ui
                    .add_enabled(
                        remoted,
                        egui::Button::new(format!("{} Pull", ph::ARROW_DOWN)),
                    )
                    .clicked()
                {
                    *op_out = Some(GitOp::Pull);
                }
                if ui
                    .add_enabled(
                        remoted,
                        egui::Button::new(format!("{} Fetch", ph::ARROWS_DOWN_UP)),
                    )
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
                // Discard ALL changes (Phase C) — destructive; needs a commit to
                // reset to and at least one change. The caller confirms first.
                ui.separator();
                if ui
                    .add_enabled(
                        idle && status.has_commits && !status.changes.is_empty(),
                        egui::Button::new(
                            egui::RichText::new(format!("{} Discard all", ph::TRASH))
                                .color(egui::Color32::from_rgb(220, 120, 100)),
                        ),
                    )
                    .on_hover_text(
                        "Reset every tracked file to HEAD and delete untracked files (asks first)",
                    )
                    .on_disabled_hover_text("needs a commit to reset to and at least one change")
                    .clicked()
                {
                    *discard_all_out = true;
                }
            });
        });

    // ── History body: commits (left) + files + diff (right) ──────────────────
    if git.view == GitView::History {
        let body_h = ui.available_height().max(40.0);
        ui.horizontal_top(|ui| {
            let total = ui.available_width();
            ui.allocate_ui_with_layout(
                egui::vec2(total * 0.38, body_h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("git_history")
                        .max_height(body_h)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if log.is_empty() {
                                ui.label(
                                    egui::RichText::new(if busy.is_some() {
                                        "loading history…"
                                    } else {
                                        "no commits yet"
                                    })
                                    .size(11.0)
                                    .italics()
                                    .color(egui::Color32::from_gray(120)),
                                );
                            }
                            for c in &log {
                                let sel = git.selected_commit.as_deref() == Some(c.sha.as_str());
                                let resp = ui.add(
                                    egui::Button::selectable(
                                        sel,
                                        egui::RichText::new(format!(
                                            "{}  {}",
                                            c.short, c.subject
                                        ))
                                        .size(11.0)
                                        .monospace(),
                                    )
                                    .truncate(),
                                );
                                if resp
                                    .on_hover_text(format!(
                                        "{}
{} · {}{}",
                                        c.sha,
                                        c.author,
                                        c.date,
                                        if c.refs.is_empty() {
                                            String::new()
                                        } else {
                                            format!("
{}", c.refs)
                                        }
                                    ))
                                    .clicked()
                                {
                                    git.selected_commit = Some(c.sha.clone());
                                    load_commit = Some(c.sha.clone());
                                }
                                // Second line: author + date, dimmed.
                                ui.label(
                                    egui::RichText::new(format!("      {} · {}", c.date, c.author))
                                        .size(9.5)
                                        .color(egui::Color32::from_gray(115)),
                                );
                            }
                        });
                },
            );
            ui.separator();
            ui.vertical(|ui| {
                // Files of the selected commit — clicking one loads its diff.
                let showing = git.selected_commit.as_deref().unwrap_or("");
                if !showing.is_empty() && commit_files_sha == showing {
                    ui.horizontal_wrapped(|ui| {
                        for f in &commit_files {
                            let col = match f.status {
                                'A' => egui::Color32::from_rgb(120, 200, 130),
                                'D' => egui::Color32::from_rgb(230, 110, 95),
                                'R' => egui::Color32::from_rgb(160, 170, 230),
                                _ => egui::Color32::from_rgb(220, 180, 90),
                            };
                            let open = diff.as_ref().is_some_and(|d| d.path == f.path);
                            if ui
                                .add(egui::Button::selectable(
                                    open,
                                    egui::RichText::new(format!("{} {}", f.status, f.path))
                                        .size(10.5)
                                        .monospace()
                                        .color(col),
                                ))
                                .clicked()
                            {
                                load_commit_file = Some((showing.to_owned(), f.path.clone()));
                            }
                        }
                    });
                    ui.separator();
                }
                if !showing.is_empty() {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                busy.is_none(),
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{} Restore ALL files from this commit",
                                        ph::ARROW_COUNTER_CLOCKWISE
                                    ))
                                    .size(10.5)
                                    .color(egui::Color32::from_rgb(230, 160, 70)),
                                )
                                .small(),
                            )
                            .on_hover_text(
                                "Make the tracked files match this commit. The branch does                                  not move — it becomes one uncommitted change (asks first).",
                            )
                            .clicked()
                        {
                            restore_all_req = Some(showing.to_owned());
                        }
                    });
                    ui.separator();
                }
                render_commit_diff(ui, diff.as_ref(), showing, &mut restore_req, body_h);
            });
        });
        // Applied after the borrows end.
        if state_diff_clear {
            git.state.lock().unwrap().diff = None;
        }
        *commit_load = load_commit;
        *commit_file_load = load_commit_file;
        *restore_from_commit = restore_req;
        *restore_all_from_commit = restore_all_req;
        return;
    }

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
                                egui::RichText::new("no changes")
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
                                    .on_hover_text("checked = included in the commit")
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
                                        "click: show its changes (disk vs HEAD) on the right",
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
                                // Discard button (user files only): ↺ restore to
                                // HEAD for a tracked file, 🗑 delete for an
                                // untracked one. The caller confirms first.
                                if !is_ide_managed(&c.path) && busy.is_none() {
                                    let untracked = c.code == "??";
                                    let (icon, hint) = if untracked {
                                        (ph::TRASH, "Delete this untracked file (not recoverable)")
                                    } else {
                                        (
                                            ph::ARROW_COUNTER_CLOCKWISE,
                                            "Discard changes — restore this file to HEAD",
                                        )
                                    };
                                    if ui
                                        .add(
                                            egui::Button::new(egui::RichText::new(icon).size(10.0))
                                                .small(),
                                        )
                                        .on_hover_text(hint)
                                        .clicked()
                                    {
                                        *discard_out = Some((c.path.clone(), untracked));
                                    }
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
                            .button(egui::RichText::new(format!("{} close", ph::X)).size(10.5))
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
                        // The user's own modules can be reverted hunk-by-hunk;
                        // IDE-generated files can't (see `is_ide_managed`).
                        let discardable = !is_ide_managed(&diff_path);
                        for (i, row) in d.rows.iter().enumerate() {
                            // Hunk header: a small "revert this hunk" button
                            // (discardable files only) + the header text.
                            if let crate::git::DiffRow::Hunk(h) = row {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    if discardable
                                        && ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(ph::ARROW_COUNTER_CLOCKWISE)
                                                        .size(10.0),
                                                )
                                                .small(),
                                            )
                                            .on_hover_text("Revert this hunk (restore it to HEAD)")
                                            .clicked()
                                    {
                                        *revert_hunk = Some((diff_path.clone(), i));
                                    }
                                    ui.label(
                                        egui::RichText::new(h)
                                            .monospace()
                                            .size(10.5)
                                            .color(egui::Color32::from_rgb(110, 145, 200)),
                                    );
                                });
                                continue;
                            }
                            // Added (green) rows are clickable → jump to that
                            // line in the editor; the rest are static.
                            let (text, col, jump_line) = match row {
                                crate::git::DiffRow::Hunk(h) => {
                                    (h.clone(), egui::Color32::from_rgb(110, 145, 200), None)
                                }
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
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    if resp
                                        .on_hover_text("click: open the file at this line")
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
                render_git_output(ui, git, "git_output", body_h);
            }
        });
    });
}

/// The git console scrollback. Shared by the Changes body and the Library view
/// — every view that can RUN something must be able to show what it printed.
fn render_git_output(ui: &mut egui::Ui, git: &GitConsole, id: &str, max_h: f32) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(max_h)
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

/// Read-only diff view for the History tab.
///
/// Deliberately NOT the Changes pane's renderer: that one carries per-hunk
/// revert buttons, and those reverse-patch the CURRENT worktree — pressing one
/// under a commit header would look like "undo this part of that commit" and do
/// something entirely different. Keeping History on its own renderer makes that
/// impossible by construction rather than by remembering to pass a flag.
fn render_commit_diff(
    ui: &mut egui::Ui,
    diff: Option<&crate::git::FileDiff>,
    sha: &str,
    restore_req: &mut Option<(String, String)>,
    body_h: f32,
) {
    let Some(d) = diff else {
        ui.label(
            egui::RichText::new("select a commit, then one of its files")
                .size(11.0)
                .italics()
                .color(egui::Color32::from_gray(120)),
        );
        return;
    };
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
                .color(egui::Color32::from_rgb(120, 200, 130)),
        );
        ui.label(
            egui::RichText::new(format!("-{}", d.removed))
                .size(11.0)
                .color(egui::Color32::from_rgb(230, 110, 95)),
        );
        // The ONE write History offers. Scoped to this file, HEAD untouched —
        // the result is a normal uncommitted change you can inspect or discard.
        if !sha.is_empty()
            && ui
                .add(
                    egui::Button::new(
                        egui::RichText::new(format!("{} Restore this file", ph::ARROW_COUNTER_CLOCKWISE))
                            .size(10.5),
                    )
                    .small(),
                )
                .on_hover_text("Bring this file back to its content at this commit (asks first)")
                .clicked()
        {
            *restore_req = Some((sha.to_owned(), d.path.clone()));
        }
    });
    egui::ScrollArea::vertical()
        .id_salt("git_commit_diff")
        .max_height(body_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &d.rows {
                use crate::git::DiffRow;
                let (text, color) = match row {
                    DiffRow::Hunk(h) => (h.clone(), egui::Color32::from_rgb(140, 150, 190)),
                    DiffRow::Ctx(_, n, t) => (
                        format!("{n:>5}   {t}"),
                        egui::Color32::from_gray(150),
                    ),
                    DiffRow::Del(o, t) => (
                        format!("{o:>5} - {t}"),
                        egui::Color32::from_rgb(230, 130, 120),
                    ),
                    DiffRow::Add(n, t) => (
                        format!("{n:>5} + {t}"),
                        egui::Color32::from_rgb(130, 205, 140),
                    ),
                };
                ui.label(egui::RichText::new(text).size(10.5).monospace().color(color));
            }
        });
}

/// The Library view: publish one workspace-member crate to its own repository.
///
/// Only `git subtree push` is offered. The reverse direction (`subtree pull`)
/// is deliberately absent — it can conflict and merge foreign history back into
/// the project, which is a different decision than "put my library on GitHub".
#[allow(clippy::too_many_arguments)]
fn show_library_panel(
    ui: &mut egui::Ui,
    git: &mut GitConsole,
    libraries: &[String],
    status: &crate::git::GitStatus,
    busy: bool,
    is_repo: bool,
    project_dir: &std::path::Path,
    lib_push_out: &mut Option<(String, String, String)>,
) {
    ui.add_space(6.0);
    if !is_repo {
        ui.label(
            egui::RichText::new(format!(
                "{} The project is not a git repository yet — initialise it in the Changes \
                 view first. A library's history is pushed OUT of the project repo, so that \
                 repo has to exist.",
                ph::WARNING
            ))
            .size(11.0)
            .color(egui::Color32::from_rgb(220, 170, 90)),
        );
        return;
    }
    if libraries.is_empty() {
        ui.label(
            egui::RichText::new(
                "No library crates in this project. Use \"Extract to library crate\" on a \
                 folder in the project tree first.",
            )
            .size(11.0)
            .color(egui::Color32::from_gray(150)),
        );
        return;
    }

    // Selecting a library re-reads ITS stored URL: the value lives in the
    // parent repo's .git/config, not in IDE state, so it survives restarts and
    // is visible to plain `git remote -v`.
    let mut just_selected = None;
    if git.lib_selected.is_none() {
        just_selected = libraries.first().cloned();
    }
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Library:").size(11.0));
        let current = git.lib_selected.clone().unwrap_or_default();
        egui::ComboBox::from_id_salt("git_lib_pick")
            .selected_text(egui::RichText::new(&current).size(11.0))
            .width(180.0)
            .show_ui(ui, |ui| {
                for l in libraries {
                    if ui
                        .selectable_label(current == *l, egui::RichText::new(l).size(11.0))
                        .clicked()
                        && current != *l
                    {
                        just_selected = Some(l.clone());
                    }
                }
            });
    });
    if let Some(lib) = just_selected {
        git.lib_remote_draft =
            crate::git::library_remote_url(project_dir, &lib).unwrap_or_default();
        git.lib_selected = Some(lib);
        git.lib_note = None;
    }
    let Some(lib) = git.lib_selected.clone() else {
        return;
    };

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Repository URL:").size(11.0));
        ui.add(
            egui::TextEdit::singleline(&mut git.lib_remote_draft)
                .desired_width(330.0)
                .hint_text("https://github.com/you/mw_radar.git"),
        );
    });
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Branch:").size(11.0));
        ui.add(
            egui::TextEdit::singleline(&mut git.lib_branch_draft)
                .desired_width(120.0)
                .hint_text("main"),
        );
        ui.label(
            egui::RichText::new(format!(
                "stored as remote \"{}\"",
                crate::git::library_remote_name(&lib)
            ))
            .size(10.0)
            .color(egui::Color32::from_gray(140))
            .italics(),
        );
    });

    // `subtree push` publishes COMMITTED history only. Uncommitted work in the
    // library would be silently left out — which looks identical to a push that
    // did nothing — so say it before the click, not after.
    let dirty = crate::git::uncommitted_under_prefix(status, &lib);
    if !dirty.is_empty() {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!(
                "{} {} uncommitted file(s) in {lib}/ will NOT be pushed — commit them in the \
                 Changes view first:",
                ph::WARNING,
                dirty.len()
            ))
            .size(11.0)
            .color(egui::Color32::from_rgb(230, 180, 90)),
        );
        for p in dirty.iter().take(6) {
            ui.label(
                egui::RichText::new(format!("    {p}"))
                    .size(10.0)
                    .monospace()
                    .color(egui::Color32::from_gray(150)),
            );
        }
        if dirty.len() > 6 {
            ui.label(
                egui::RichText::new(format!("    … and {} more", dirty.len() - 6))
                    .size(10.0)
                    .color(egui::Color32::from_gray(140)),
            );
        }
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        let can_push = !busy && status.has_commits;
        if ui
            .add_enabled(
                can_push,
                egui::Button::new(
                    egui::RichText::new(format!("{} Push {lib} to its repository", ph::UPLOAD_SIMPLE))
                        .size(11.5),
                ),
            )
            .on_disabled_hover_text(if status.has_commits {
                "A git operation is already running."
            } else {
                "The project repo has no commits yet — commit once in the Changes view first."
            })
            .clicked()
        {
            match crate::git::validate_remote_url(&git.lib_remote_draft) {
                Ok(()) => {
                    git.lib_note = None;
                    *lib_push_out = Some((
                        lib.clone(),
                        git.lib_remote_draft.trim().to_string(),
                        git.lib_branch_draft.trim().to_string(),
                    ));
                }
                Err(e) => git.lib_note = Some(e),
            }
        }
        ui.label(
            egui::RichText::new("push only — the project keeps the files")
                .size(10.0)
                .color(egui::Color32::from_gray(140))
                .italics(),
        );
    });

    if let Some(note) = &git.lib_note {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(note)
                .size(11.0)
                .color(egui::Color32::from_rgb(220, 120, 100)),
        );
    }

    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(format!(
            "Pushes the history of {lib}/ to a separate repository. The project repo keeps the \
             real files, so cloning it still works and the cargo workspace member stays valid — \
             nothing here changes your working tree.",
        ))
        .size(10.0)
        .color(egui::Color32::from_gray(145)),
    );

    // ── Output ──────────────────────────────────────────────────────────────
    // This view runs a command, so it must show what that command printed.
    // Without it a push looked like it did nothing: the work happened on the
    // worker, the notice landed in the scrollback, and this view rendered
    // none of it.
    ui.add_space(4.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        if busy {
            crate::app::helpers::spinner::throttled_spinner(ui, 12.0);
            ui.label(
                egui::RichText::new(format!(
                    " pushing {lib} {} splitting the history can take a while on a large repo",
                    ph::DOTS_THREE
                ))
                .size(11.0)
                .color(egui::Color32::from_rgb(220, 180, 70)),
            );
        } else {
            ui.label(
                egui::RichText::new("Output")
                    .size(10.5)
                    .color(egui::Color32::from_gray(130)),
            );
        }
    });
    let out_h = (ui.available_height() - 4.0).max(60.0);
    render_git_output(ui, git, "git_library_output", out_h);
}
