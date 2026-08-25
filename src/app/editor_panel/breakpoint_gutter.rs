//! Breakpoint gutter — red dots in the editor's line-number column.
//!
//! Clicking left of a line number toggles a breakpoint on that line (a hollow
//! ghost dot previews the spot on hover). Breakpoints live in
//! `AppIde::breakpoints` (rel path → 1-based lines); every toggle also syncs
//! the file's set into a live debug session (`Debugger::sync_breakpoints`),
//! so they can be placed before OR during debugging.

use crate::app::{AppIde, ProjectFileId};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Breakpoint dot colour (the classic red).
const BP_FILL: egui::Color32 = egui::Color32::from_rgb(220, 70, 60);
/// Dot diameter as a share of the line's height — it reads as "this whole line",
/// which a small fixed dot did not. Under 1.0 so it keeps clear of the rows
/// above and below and stays mostly inside the gutter.
const BP_DOT_SHARE: f32 = 0.80;
/// The hover ghost's radius stays small: it previews the spot, it doesn't claim
/// the line yet.
const BP_GHOST_RADIUS: f32 = 4.0;
/// A breakpoint line is underlined instead of tinted: one rule along the bottom
/// of the row, in a light red. An edge leaves the syntax colours alone, which a
/// band never can. Was a dark red at 50 % alpha, which all but vanished against
/// the editor background — this one is opaque and reads at a glance while still
/// sitting a step below the dot's saturation, so the dot stays the anchor.
const BP_EDGE: egui::Color32 = egui::Color32::from_rgb(235, 120, 110);
const BP_EDGE_W: f32 = 1.5;
/// The row under the pointer while it is over the line-number column: black at
/// 10 % across the full editor width. It covers the number column too, so it has
/// to stay light — at 90 % the band blacked out the very line number the pointer
/// was aiming at. Black (0,0,0) is its own premultiplied form, so this stays
/// const (`from_rgba_unmultiplied` is not).
const HOVER_ROW_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(0, 0, 0, 26);

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
        let Some(rel) = bp_path_of(self.selected_file, &self.project_tree.user_src_files) else {
            return;
        };
        let galley = &editor_resp.galley;
        let gp = editor_resp.galley_pos;

        // Guard: a visible line-number column must exist (its right edge is
        // ~`gp.x - 12`); a degenerate layout has none.
        let num_col_r = gp.x - 12.0;
        if num_col_r - clip.left() < 10.0 {
            return;
        }
        // The dot sits to the RIGHT of the line number, on the gutter/code
        // divider (where the diff marks live at `gp.x - 7`).
        let dot_x = gp.x - 6.0;
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

        // Clickable strip: the number column PLUS the dot itself, so clicking
        // the red dot toggles the breakpoint — not just the digits. The dot is
        // line-height wide now and pokes a couple of pixels past the gutter, so
        // the strip follows it instead of a fixed `gp.x - 3`. The diff bars
        // underneath are hover-only (their click-to-revert was removed), so the
        // overlap is safe.
        let dot_r = y_range_of(0)
            .map(|(t, b)| (b - t) * 0.5 * BP_DOT_SHARE)
            .unwrap_or(BP_GHOST_RADIUS);
        let strip = egui::Rect::from_min_max(
            egui::pos2(clip.left(), clip.top()),
            egui::pos2(dot_x + dot_r, clip.bottom()),
        );

        let painter = ui.painter().with_clip_rect(clip);

        // probe-rs's verdict per line for THIS file — what the Debug tab's
        // Breakpoints pane shows. Empty outside a session: nothing has been
        // asked yet, which is not the same as "it won't work".
        let bp_status = self
            .debugger
            .state
            .lock()
            .unwrap()
            .bp_status
            .get(&rel)
            .cloned()
            .unwrap_or_default();
        let mut warn_spots: Vec<(u32, egui::Rect)> = Vec::new();

        // ── Existing breakpoints ──────────────────────────────────────────────
        // A line-height dot in the gutter, and the line underlined rather than
        // tinted — so the code keeps its own colours.
        if let Some(set) = self.breakpoints.get(&rel) {
            for &line in set {
                let Some((top, bot)) = y_range_of(line.saturating_sub(1) as usize) else {
                    continue; // line beyond the current text — keep, don't draw
                };
                let cy = (top + bot) * 0.5;
                if cy < clip.top() || cy > clip.bottom() {
                    continue;
                }
                let r = (bot - top) * 0.5 * BP_DOT_SHARE;
                // The rule starts clear of the dot and runs to the editor's
                // right edge; half a stroke inside the row so it isn't clipped.
                painter.hline(
                    (dot_x + r + 1.0)..=clip.right(),
                    bot - BP_EDGE_W * 0.5,
                    egui::Stroke::new(BP_EDGE_W, BP_EDGE),
                );
                painter.circle_filled(egui::pos2(dot_x, cy), r, BP_FILL);

                // A breakpoint the target REFUSED to arm gets the same warning
                // triangle as the Debug tab's list. It goes just right of the
                // dot — which means the line's first character cell, because
                // the dot already fills the gutter (its right edge lands on
                // `gp.x`). Breakpoint lines are inside a function and therefore
                // indented, so in practice it sits on blank space.
                if bp_status.get(&line).is_some_and(|s| !s.verified) {
                    let size = (bot - top) * 0.62;
                    let glyph = painter.text(
                        egui::pos2(gp.x + 1.0, cy),
                        egui::Align2::LEFT_CENTER,
                        ph::WARNING,
                        egui::FontId::proportional(size),
                        crate::app::tabs::debug_tab::UNARMED_AMBER,
                    );
                    warn_spots.push((line, glyph));
                }
            }
        }

        // A warning sign nobody can explain is worse than none: the same text
        // the Breakpoints pane shows, on hover. Hover-only — a click-sensing
        // rect out here would fight the editor for the caret.
        for (line, rect) in warn_spots {
            ui.interact(
                rect,
                egui::Id::new("bp_unarmed").with(&rel).with(line),
                egui::Sense::hover(),
            )
            .on_hover_ui(|ui| {
                ui.set_max_width(clip.width() * 0.4);
                ui.label(crate::app::tabs::debug_tab::bp_hover(
                    &format!("{rel}:{line}"),
                    false,
                    bp_status.get(&line),
                ));
            });
        }

        // ── Hover ghost + click toggle (one interact for the whole strip) ────
        let resp = ui.interact(
            strip,
            egui::Id::new("bp_gutter").with(&rel),
            egui::Sense::click(),
        );
        // The fold carets sit inside this strip and are registered AFTER it, so
        // they take the primary click; this only ever acts on the secondary one.
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
            // Pick the row out, full width, while the pointer is in the number
            // column — the gutter is far from the code and it was easy to lose
            // track of which line you were about to put a breakpoint on.
            if let Some((top, bot)) = y_range_of(line as usize - 1) {
                let row = egui::Rect::from_min_max(
                    egui::pos2(clip.left(), top),
                    egui::pos2(clip.right(), bot),
                );
                // The band goes OVER the text (everything here is painted after
                // the editor), which is exactly why it stays this light: at 10 %
                // the code and the line number read straight through it, so
                // nothing has to be re-drawn on top of it.
                painter.rect_filled(row, 0.0, HOVER_ROW_BG);
            }
            let already = self
                .breakpoints
                .get(&rel)
                .is_some_and(|s| s.contains(&line));
            if !already {
                if let Some((top, bot)) = y_range_of(line as usize - 1) {
                    painter.circle_stroke(
                        egui::pos2(dot_x, (top + bot) * 0.5),
                        BP_GHOST_RADIUS,
                        egui::Stroke::new(1.2_f32, BP_FILL.gamma_multiply(0.6)),
                    );
                }
            }
            resp.clone().on_hover_text(if already {
                "Right-click to remove this breakpoint"
            } else {
                "Right-click to set a breakpoint"
            });
            // SECONDARY click: the left button belongs to the fold carets, which
            // live in this same strip (the number column). One gutter, two
            // actions — a left click that silently set a breakpoint made the
            // fold arrows unusable.
            if resp.secondary_clicked() {
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
