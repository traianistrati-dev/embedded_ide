//! Breakpoint gutter — red dots in the editor's line-number column.
//!
//! Clicking left of a line number toggles a breakpoint on that line (a hollow
//! ghost dot previews the spot on hover). Breakpoints live in
//! `AppIde::breakpoints` (rel path → 1-based lines); every toggle also syncs
//! the file's set into a live debug session (`Debugger::sync_breakpoints`),
//! so they can be placed before OR during debugging.

use crate::app::{AppIde, ProjectFileId};
use eframe::egui;

/// Breakpoint dot colour (the classic red).
const BP_FILL: egui::Color32 = egui::Color32::from_rgb(220, 70, 60);
const BP_RADIUS: f32 = 4.0;

/// The workspace-relative path breakpoints are keyed by — only Rust sources
/// can hold one (a breakpoint in Cargo.toml means nothing to the debugger).
fn bp_path_of(id: ProjectFileId, user_files: &[(String, String)]) -> Option<String> {
    match id {
        ProjectFileId::MainRs => Some("src/main.rs".to_owned()),
        ProjectFileId::UserFile(i) => {
            let name = &user_files.get(i)?.0;
            name.ends_with(".rs").then(|| name.clone())
        }
        _ => None,
    }
}

impl AppIde {
    /// Paint the dots + handle gutter clicks. Call with the editor's output,
    /// right after the diff gutter (same coordinate machinery).
    pub(super) fn paint_breakpoint_gutter(
        &mut self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        clip: egui::Rect,
        display_code: &str,
    ) {
        let Some(rel) = bp_path_of(self.selected_file, &self.project_tree.user_src_files)
        else {
            return;
        };
        let galley = &editor_resp.galley;
        let gp = editor_resp.galley_pos;

        // The clickable strip: from the panel edge to just left of the diff
        // marks (which sit at `gp.x - 10 .. gp.x - 1`) — i.e. the line-number
        // column. The dot is centred in it.
        let strip_r = gp.x - 12.0;
        if strip_r - clip.left() < 10.0 {
            return; // no visible number column (degenerate layout)
        }
        let dot_x = clip.left() + 8.0;
        let strip = egui::Rect::from_min_max(
            egui::pos2(clip.left(), clip.top()),
            egui::pos2(strip_r, clip.bottom()),
        );

        // Char index of every line start — line → y via the galley.
        let starts: Vec<usize> = {
            let mut v = vec![0usize];
            for (i, c) in display_code.chars().enumerate() {
                if c == '\n' {
                    v.push(i + 1);
                }
            }
            v
        };
        let y_range_of = |line0: usize| -> Option<(f32, f32)> {
            let ci = *starts.get(line0)?;
            let loc = galley.pos_from_cursor(egui::text::CCursor::new(ci));
            Some((gp.y + loc.min.y, gp.y + loc.max.y))
        };

        let painter = ui.painter().with_clip_rect(clip);

        // ── Existing breakpoints ──────────────────────────────────────────────
        if let Some(set) = self.breakpoints.get(&rel) {
            for &line in set {
                let Some((top, bot)) = y_range_of(line.saturating_sub(1) as usize) else {
                    continue; // line beyond the current text — keep, don't draw
                };
                let cy = (top + bot) * 0.5;
                if cy < clip.top() || cy > clip.bottom() {
                    continue;
                }
                painter.circle_filled(egui::pos2(dot_x, cy), BP_RADIUS, BP_FILL);
            }
        }

        // ── Hover ghost + click toggle (one interact for the whole strip) ────
        let resp = ui.interact(
            strip,
            egui::Id::new("bp_gutter").with(&rel),
            egui::Sense::click(),
        );
        let hovered_line: Option<u32> = resp.hover_pos().and_then(|pos| {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            // Binary search the line whose row contains pos.y (rows are
            // y-sorted; O(log n) keeps hover cheap on big files).
            let (mut lo, mut hi) = (0usize, starts.len().saturating_sub(1));
            while lo < hi {
                let mid = (lo + hi + 1) / 2;
                match y_range_of(mid) {
                    Some((top, _)) if top <= pos.y => lo = mid,
                    _ => hi = mid - 1,
                }
            }
            let (_, bot) = y_range_of(lo)?;
            (pos.y <= bot + 2.0).then_some(lo as u32 + 1)
        });

        if let Some(line) = hovered_line {
            let already = self
                .breakpoints
                .get(&rel)
                .is_some_and(|s| s.contains(&line));
            if !already {
                if let Some((top, bot)) = y_range_of(line as usize - 1) {
                    painter.circle_stroke(
                        egui::pos2(dot_x, (top + bot) * 0.5),
                        BP_RADIUS,
                        egui::Stroke::new(1.2, BP_FILL.gamma_multiply(0.6)),
                    );
                }
            }
            resp.clone().on_hover_text(if already {
                "Remove breakpoint"
            } else {
                "Set breakpoint"
            });
            if resp.clicked() {
                let set = self.breakpoints.entry(rel.clone()).or_default();
                if !set.remove(&line) {
                    set.insert(line);
                }
                if set.is_empty() {
                    self.breakpoints.remove(&rel);
                }
                // Live session (if any) gets the file's new set immediately.
                let lines: Vec<u32> = self
                    .breakpoints
                    .get(&rel)
                    .map(|s| s.iter().copied().collect())
                    .unwrap_or_default();
                self.debugger.sync_breakpoints(&rel, &lines);
            }
        }
    }
}
