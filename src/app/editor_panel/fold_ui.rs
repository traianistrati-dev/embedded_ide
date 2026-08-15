//! Code folding — the gutter UI (phase 2).
//!
//! One caret per foldable block in the line-number column, plus a badge on a
//! folded header saying how many lines are hidden. The model (which blocks
//! exist, and the buffer ↔ display projection) lives in [`fold`](super::fold).

use super::fold::{FoldMap, Region, regions};
use crate::app::AppIde;
use eframe::egui;
use egui_phosphor::regular as ph;

/// Caret colours — dim by default so a column of them doesn't compete with the
/// code, brighter when folded (that block is hiding something) and brightest
/// under the pointer.
const ARROW_IDLE: egui::Color32 = egui::Color32::from_gray(110);
const ARROW_FOLDED: egui::Color32 = egui::Color32::from_rgb(190, 165, 105);
const ARROW_HOT: egui::Color32 = egui::Color32::from_gray(225);
/// The "N lines" badge on a folded header line.
const BADGE_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(58, 58, 40, 220);
const BADGE_FG: egui::Color32 = egui::Color32::from_rgb(205, 195, 140);

impl AppIde {
    /// Paint the fold carets and the hidden-lines badges, and handle their
    /// clicks. `display_code` is the BUFFER (regions are buffer lines); `map`
    /// translates those to the rows actually on screen.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_fold_gutter(
        &mut self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        clip: egui::Rect,
        display_code: &str,
        map: &FoldMap,
        rel: &str,
        font_size: f32,
    ) {
        let galley = &editor_resp.galley;
        let gp = editor_resp.galley_pos;
        // Same guard the breakpoint gutter uses: a degenerate layout has no
        // number column to draw into.
        if gp.x - 12.0 - clip.left() < 10.0 {
            return;
        }

        // Display-line starts, so a region's header can be located on screen.
        let shown = map.display();
        let starts: Vec<usize> = {
            let mut v = vec![0usize];
            for (i, c) in shown.chars().enumerate() {
                if c == '\n' {
                    v.push(i + 1);
                }
            }
            v
        };
        let y_of = |disp_line: usize| -> Option<(f32, f32)> {
            let ci = *starts.get(disp_line)?;
            let loc = galley.pos_from_cursor(egui::text::CCursor::new(ci));
            Some((gp.y + loc.min.y, gp.y + loc.max.y))
        };

        // The caret goes in the blank cells `numlines_show` reserves at the end
        // of the number column: everything from `gp.x - 12` rightwards belongs
        // to the diff bars and the breakpoint dot, and anything further left
        // would sit on top of the digits.
        // `+ 5` pulls it clear of the line number: centred in the reserved cells
        // it still read as glued to the digits.
        let cell = font_size * 0.5;
        let arrow_x =
            gp.x - 12.0 - cell * crate::editor::gui::code_editor::FOLD_GUTTER_CHARS as f32
                + cell * 0.5
                + 5.0;
        let folded_now = self.folds.get(rel).cloned().unwrap_or_default();
        let painter = ui.painter().with_clip_rect(clip);
        let mut toggle: Option<usize> = None;

        for region in regions(display_code) {
            // Every block gets a caret — `is_fn` only narrows "collapse all".
            let Region { head, end, .. } = region;
            if end <= head + 1 {
                continue; // nothing to hide
            }
            let Some(disp_line) = map.display_line_of(head) else {
                continue; // the header itself is inside another fold
            };
            let Some((top, bot)) = y_of(disp_line) else {
                continue;
            };
            if bot < clip.top() || top > clip.bottom() {
                continue; // off-screen
            }
            let is_folded = folded_now.contains(&head);
            let cy = (top + bot) * 0.5;
            let hit = egui::Rect::from_center_size(
                egui::pos2(arrow_x, cy),
                egui::vec2(14.0, (bot - top).max(10.0)),
            );
            let resp = ui.interact(
                hit,
                egui::Id::new("fold_caret").with(rel).with(head),
                egui::Sense::click(),
            );
            let hot = resp.hovered();
            if hot {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            // EVERY foldable block shows its caret. Drawing them only on hover
            // made the feature invisible — you cannot hover what you don't know
            // is there — so they are always painted, just dim until pointed at.
            let color = if hot {
                ARROW_HOT
            } else if is_folded {
                ARROW_FOLDED
            } else {
                ARROW_IDLE
            };
            // Phosphor glyphs, not raw Unicode carets: the bundled font has no
            // arrows and would render them as empty boxes (there is a guard test
            // for exactly this).
            painter.text(
                egui::pos2(arrow_x, cy),
                egui::Align2::CENTER_CENTER,
                if is_folded {
                    ph::CARET_RIGHT
                } else {
                    ph::CARET_DOWN
                },
                egui::FontId::proportional((bot - top) * 0.72),
                color,
            );
            if resp.clicked() {
                toggle = Some(head);
            }

            // Folded: say how much is hidden, at the end of the header line.
            if is_folded {
                let eol = starts
                    .get(disp_line + 1)
                    .map(|&s| s - 1)
                    .unwrap_or(shown.chars().count());
                let loc = galley.pos_from_cursor(egui::text::CCursor::new(eol));
                let label = format!("... {} lines", region.hidden_count());
                let font = egui::FontId::proportional(10.0);
                let g = painter.layout_no_wrap(label.clone(), font.clone(), BADGE_FG);
                let rect = egui::Rect::from_min_size(
                    egui::pos2(gp.x + loc.min.x + 10.0, top),
                    egui::vec2(g.size().x + 8.0, bot - top),
                );
                painter.rect_filled(rect, 3.0, BADGE_BG);
                painter.text(
                    rect.left_center() + egui::vec2(4.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &label,
                    font,
                    BADGE_FG,
                );
                let badge = ui.interact(
                    rect,
                    egui::Id::new("fold_badge").with(rel).with(head),
                    egui::Sense::click(),
                );
                if badge.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if badge.clicked() {
                    toggle = Some(head);
                }
                badge.on_hover_text("Click to expand this block");
            }
        }

        if let Some(head) = toggle {
            let set = self.folds.entry(rel.to_owned()).or_default();
            if !set.remove(&head) {
                set.insert(head);
            }
            if set.is_empty() {
                self.folds.remove(rel);
            }
        }
    }
}
