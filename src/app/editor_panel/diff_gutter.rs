//! Live git gutter marks in the code editor.
//!
//! Compares the LIVE in-memory editor text — including unsaved edits — against
//! the file's content at git HEAD (`git show HEAD:<path>`, fetched on a worker
//! and cached per (path, `GitState::op_gen`), so a commit/pull refreshes the
//! baseline). Marks are painted left of the text: green bar = added lines,
//! amber bar = modified, red wedge = lines deleted at that boundary. Hovering
//! a mark shows the HEAD version of those lines. Click-to-revert was REMOVED —
//! the bars sit under the breakpoint dot, so clicking to set a breakpoint
//! reverted the hunk; revert now lives in the Git tab's diff view (+ Ctrl+Z).

use crate::app::AppIde;
use crate::git::{BaselineFetch, DiffHunk, compute_hunks, fetch_baseline};
use eframe::egui;
use std::sync::{Arc, Mutex};

const ADDED: egui::Color32 = egui::Color32::from_rgb(110, 200, 120);
const MODIFIED: egui::Color32 = egui::Color32::from_rgb(220, 170, 60);
const DELETED: egui::Color32 = egui::Color32::from_rgb(230, 105, 95);
/// Translucent yellow line background for added / modified lines — spans the
/// whole editor width (line numbers included) to make changes stand out.
/// Premultiplied form of unmultiplied `(230, 205, 70, α=28)` (`from_rgba_
/// unmultiplied` isn't `const`).
const LINE_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(25, 22, 7, 28);

/// Per-app state: the baseline slot shared with the fetch worker + the hunks
/// computed for the current text (valid only while `computed_hash` matches).
pub(crate) struct DiffGutter {
    baseline: Arc<Mutex<BaselineFetch>>,
    /// Baseline text copied out on the last recompute — for hover previews and
    /// revert splices (one copy per recompute, not per frame).
    baseline_text: Option<String>,
    hunks: Vec<DiffHunk>,
    /// Char index of each line start in the text the hunks were computed for.
    line_starts: Vec<usize>,
    /// Hash of (text, baseline) the fields above were computed for; 0 = stale.
    computed_hash: u64,
}

impl Default for DiffGutter {
    fn default() -> Self {
        Self {
            baseline: Arc::new(Mutex::new(BaselineFetch::default())),
            baseline_text: None,
            hunks: Vec::new(),
            line_starts: Vec::new(),
            computed_hash: 0,
        }
    }
}

/// Char index of every line start (line 0 included).
fn line_starts(text: &str) -> Vec<usize> {
    let mut v = vec![0];
    for (i, c) in text.chars().enumerate() {
        if c == '\n' {
            v.push(i + 1);
        }
    }
    v
}

impl AppIde {
    /// Resolve which git repo owns a project-relative editor `path`, returning
    /// `(repo_dir, path_relative_to_that_repo)`. A workspace-member or detached
    /// library folder that carries its OWN `.git` (submodule / separate clone)
    /// is a distinct repo — its files aren't blobs in the project repo, so the
    /// baseline must be read from the library repo with the prefix stripped.
    /// Everything else stays on the project repo with the path unchanged.
    fn diff_repo_for(&self, root: &std::path::Path, path: String) -> (std::path::PathBuf, String) {
        let members = crate::panels::mcu_module::project_gen::workspace_members(&self.cargo_toml);
        let detached = crate::project_tree::extract_crate::detached_libs(
            &self.project_tree.user_src_files,
            &members,
        );
        for lib in members.iter().chain(detached.iter()) {
            let prefix = format!("{}/", lib.trim_end_matches('/'));
            if let Some(rest) = path.strip_prefix(&prefix) {
                let lib_dir = root.join(lib);
                // `.git` is a directory for a normal repo, a FILE for a
                // submodule / worktree — `exists()` catches both.
                if lib_dir.join(".git").exists() {
                    return (lib_dir, rest.to_string());
                }
                break; // in a member that shares the project repo → use as-is
            }
        }
        (root.to_path_buf(), path)
    }

    /// Keep the gutter data fresh for the displayed file: (re)fetch the HEAD
    /// baseline when the file or `op_gen` changed, and recompute the hunks
    /// when the text or baseline changed (memoized on their hashes — the diff
    /// itself only runs on an actual edit). Call post-editor, with the frame's
    /// final text, right before [`AppIde::paint_diff_gutter`].
    pub(super) fn tick_diff_gutter(&mut self, display_code: &str) {
        let (Some(root), Some(path)) = (
            self.project_dir.clone(),
            self.selected_file
                .rel_path(&self.project_tree.user_src_files),
        ) else {
            self.diff_gutter.hunks.clear();
            self.diff_gutter.computed_hash = 0;
            return;
        };

        // A library folder can be its OWN git repo (submodule / detached clone,
        // separate remote). The project repo doesn't track its blobs, so a
        // baseline `git show HEAD:<lib>/src/…` run there returns nothing and the
        // gutter stayed blank for library files. Redirect the fetch to the repo
        // that actually owns the file, with the path relative to IT.
        let (dir, path) = self.diff_repo_for(&root, path);

        let key = (path, self.git.state.lock().unwrap().op_gen);
        let (fresh, done, content_hash) = {
            let slot = self.diff_gutter.baseline.lock().unwrap();
            (slot.key == key, slot.done, slot.content_hash)
        };
        if !fresh {
            self.diff_gutter.hunks.clear();
            self.diff_gutter.computed_hash = 0;
            fetch_baseline(
                key,
                dir,
                Arc::clone(&self.diff_gutter.baseline),
                self.egui_ctx.clone(),
            );
            return;
        }
        if !done {
            return; // worker still loading — marks stay hidden
        }

        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        display_code.hash(&mut h);
        content_hash.hash(&mut h);
        let hash = h.finish().max(1);
        if hash == self.diff_gutter.computed_hash {
            return;
        }
        self.diff_gutter.computed_hash = hash;
        let baseline = self.diff_gutter.baseline.lock().unwrap().content.clone();
        match baseline {
            Some(old) => {
                self.diff_gutter.hunks = compute_hunks(&old, display_code);
                self.diff_gutter.line_starts = line_starts(display_code);
                self.diff_gutter.baseline_text = Some(old);
            }
            None => {
                // Untracked file / no repo / unborn HEAD → no marks (an
                // all-green untracked file would be noise).
                self.diff_gutter.hunks.clear();
                self.diff_gutter.baseline_text = None;
            }
        }
    }

    /// Paint the gutter marks (green/amber bars, red deletion wedge) + the
    /// hover HEAD preview. Read-only: click-to-revert was removed (it collided
    /// with the breakpoint dot); revert lives in the Git tab.
    pub(super) fn paint_diff_gutter(
        &mut self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        clip: egui::Rect,
        display_code: &str,
    ) {
        if self.diff_gutter.hunks.is_empty() || self.diff_gutter.computed_hash == 0 {
            return;
        }
        let galley = &editor_resp.galley;
        let gp = editor_resp.galley_pos;
        let total_chars = display_code.chars().count();
        let painter = ui.painter().with_clip_rect(clip);

        let starts = &self.diff_gutter.line_starts;
        let ci_of = |line: usize| {
            starts
                .get(line)
                .copied()
                .unwrap_or(total_chars)
                .min(total_chars)
        };
        let y_of = |ci: usize| {
            let loc = galley.pos_from_cursor(egui::text::CCursor::new(ci));
            (gp.y + loc.min.y, gp.y + loc.max.y)
        };

        for (i, hk) in self.diff_gutter.hunks.iter().enumerate() {
            let (y_top, y_bot, color) = if hk.new_len == 0 {
                // Deletion marker: a wedge at the boundary line's top edge.
                let (top, _) = y_of(ci_of(hk.new_start));
                (top - 4.0, top + 4.0, DELETED)
            } else {
                let (top, _) = y_of(ci_of(hk.new_start));
                let (_, bot) = y_of(ci_of(hk.new_start + hk.new_len - 1));
                (top, bot, if hk.old_len == 0 { ADDED } else { MODIFIED })
            };
            if y_bot < clip.top() || y_top > clip.bottom() {
                continue;
            }
            // Added / modified lines (new_len > 0): a translucent yellow band
            // across the FULL width — line-number gutter included — so the
            // changed lines are obvious. Deletion markers have no line to fill.
            // Painted first, so the coloured bar stays on top. The user can turn
            // the band off (it can distract / mis-align while editing) — the
            // gutter bars below always remain.
            if hk.new_len > 0 && self.diff_line_bg {
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(clip.left(), y_top),
                        egui::pos2(clip.right(), y_bot),
                    ),
                    0.0,
                    LINE_BG,
                );
            }
            let x = gp.x - 7.0;
            if hk.new_len == 0 {
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(x, y_top),
                        egui::pos2(x, y_bot),
                        egui::pos2(x + 5.0, (y_top + y_bot) * 0.5),
                    ],
                    color,
                    egui::Stroke::NONE,
                ));
            } else {
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(x, y_top), egui::pos2(x + 3.0, y_bot)),
                    1.0,
                    color,
                );
            }

            // Hover-only: preview the HEAD version. Click-to-revert was REMOVED —
            // the bar sits under the breakpoint dot (gp.x-6), so clicking to set a
            // breakpoint reverted the hunk instead. Revert now lives in the Git
            // tab's diff view (+ Ctrl+Z). Being hover-only, this no longer fights
            // the breakpoint gutter's click strip (painted after → owns clicks in
            // this overlapping area).
            let hit =
                egui::Rect::from_min_max(egui::pos2(x - 3.0, y_top), egui::pos2(x + 6.0, y_bot));
            let resp = ui.interact(
                hit,
                egui::Id::new("diff_gutter")
                    .with(self.selected_file_key())
                    .with(i),
                egui::Sense::hover(),
            );
            let baseline = self.diff_gutter.baseline_text.as_deref().unwrap_or("");
            resp.on_hover_ui(|ui| {
                ui.set_max_width(520.0);
                let head = if hk.old_len == 0 {
                    "linii noi (nu există în HEAD)".to_owned()
                } else {
                    format!("în HEAD ({} lin.):", hk.old_len)
                };
                ui.label(
                    egui::RichText::new(head)
                        .size(10.5)
                        .color(egui::Color32::from_gray(150)),
                );
                for l in baseline.lines().skip(hk.old_start).take(hk.old_len.min(12)) {
                    ui.label(
                        egui::RichText::new(l)
                            .monospace()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(230, 130, 110)),
                    );
                }
                if hk.old_len > 12 {
                    ui.label(egui::RichText::new("…").size(10.5));
                }
                ui.label(
                    egui::RichText::new("revert: Git tab -> diff, or Ctrl+Z")
                        .size(10.0)
                        .italics()
                        .color(egui::Color32::from_gray(130)),
                );
            });
        }
    }

    /// A stable per-file discriminant for widget ids.
    fn selected_file_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{:?}", self.selected_file).hash(&mut h);
        h.finish()
    }
}
