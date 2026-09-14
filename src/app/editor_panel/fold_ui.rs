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
        // The galley was laid out BEFORE this frame's edit was adopted, so an
        // index taken from the current projection can sit one character past
        // its end — and `pos_from_cursor` has a `debug_assert!` on exactly
        // that. One frame of a slightly stale arrow position beats a panic on
        // the keystroke that adds a line.
        let galley_len = galley.text().chars().count();
        let y_of = |disp_line: usize| -> Option<(f32, f32)> {
            let ci = (*starts.get(disp_line)?).min(galley_len);
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
        // `(header line, its screen y)` — the y is what the next frame re-anchors
        // the scroll offset on.
        let mut toggle: Option<(usize, f32)> = None;

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
                toggle = Some((head, top));
            }

            // Folded: say how much is hidden, at the end of the header line.
            if is_folded {
                let eol = starts
                    .get(disp_line + 1)
                    .map(|&s| s - 1)
                    .unwrap_or(shown.chars().count())
                    .min(galley_len);
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
                    toggle = Some((head, top));
                }
                badge.on_hover_text("Click to expand this block");
            }
        }

        if let Some((head, y)) = toggle {
            let set = self.folds.entry(rel.to_owned()).or_default();
            if !set.remove(&head) {
                set.insert(head);
            }
            if set.is_empty() {
                self.folds.remove(rel);
            }
            // Pin the block's header where it is. Hiding (or restoring) a couple
            // of hundred lines changes what sits at every pixel below it, and
            // the page slides out from under the pointer unless we correct it.
            self.ed.fold_anchor = Some((rel.to_owned(), head, y));
        }
    }

    /// Ctrl+Shift+Q / "toggle collapse all", with the view kept in place.
    ///
    /// A gutter click pins the clicked header; there is no clicked line here,
    /// so the CARET's line is pinned when it is on screen, else the first
    /// visible row — an off-screen caret must not pull the view to itself,
    /// which is the very jump this feature is being fixed for. A line about
    /// to be hidden pins its block's header instead, at the header's own y
    /// so nothing above it moves — or at the top edge when the header is
    /// scrolled out above: pinned off-screen, the header would have taken the
    /// user's whole block with it and the view would land on the functions
    /// after it.
    pub(super) fn toggle_fold_all(
        &mut self,
        editor_resp: &egui::text_edit::TextEditOutput,
        clip: egui::Rect,
        display_code: &str,
        map: &FoldMap,
        rel: &str,
    ) {
        let current = self.folds.get(rel).cloned().unwrap_or_default();
        let next = super::fold::toggle_all(display_code, &current);
        if next == current {
            return;
        }
        let next_map = FoldMap::new(display_code, &next);

        // Rows are uniform (monospace, no wrapping), which is what lets a row
        // be turned into a y without walking the galley.
        let gp = editor_resp.galley_pos;
        let row_h = editor_resp
            .galley
            .pos_from_cursor(egui::text::CCursor::new(0))
            .height()
            .max(1.0);
        let galley_len = editor_resp.galley.text().chars().count();
        // The caret is in buffer space by now (converted after the render).
        let caret_row = editor_resp.state.cursor.char_range().map(|r| {
            let disp = map.to_display_clamped(r.primary.index).min(galley_len);
            map.display()
                .chars()
                .take(disp)
                .filter(|&c| c == '\n')
                .count()
        });
        let first_row = ((clip.top() - gp.y) / row_h).ceil().max(0.0) as usize;
        let last_row = ((clip.bottom() - gp.y) / row_h).floor() as usize;
        let row = anchor_row(caret_row, first_row..last_row);

        let mut line = map.buffer_line_of_row(row);
        let mut y = gp.y + row as f32 * row_h;
        if next_map.display_line_of(line).is_none() {
            // About to be hidden: the header stands in for it, where it is now
            // — but never above the viewport.
            line = next_map.buffer_line_of_row(next_map.display_row_of(line));
            y = (gp.y + map.display_row_of(line) as f32 * row_h).max(clip.top());
        }

        if next.is_empty() {
            self.folds.remove(rel);
        } else {
            self.folds.insert(rel.to_owned(), next);
        }
        self.ed.fold_anchor = Some((rel.to_owned(), line, y));
    }
}

/// The display row a "toggle collapse all" pins: the caret's when it is
/// inside `visible` (rows `start..end`, `end` exclusive), else the first
/// visible row.
fn anchor_row(caret_row: Option<usize>, visible: std::ops::Range<usize>) -> usize {
    match caret_row {
        Some(r) if visible.contains(&r) => r,
        _ => visible.start,
    }
}

/// What becomes of a pending "toggle collapse all" on this frame.
#[derive(Debug, PartialEq)]
pub(super) enum Request {
    Fire,
    /// Same file, but the frame also changed its text: apply next frame.
    Wait,
    /// Raised on another file: a request never carries over to one the user
    /// did not make it on.
    Drop,
}

/// `requested_for`: the file the request was raised on, if any; `rel`: the
/// file this frame shows; `text_changed`: the buffer differs from what the
/// fold guard recorded last frame.
pub(super) fn fold_all_request(
    requested_for: Option<String>,
    rel: &str,
    text_changed: bool,
) -> Option<Request> {
    let for_rel = requested_for?;
    Some(if for_rel != rel {
        Request::Drop
    } else if text_changed {
        Request::Wait
    } else {
        Request::Fire
    })
}

/// The fold guard's fingerprint of a buffer.
pub(super) fn text_sig(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl AppIde {
    /// Put the line a fold toggle pinned back where it was on screen. Returns
    /// whether an anchor for this file was consumed — the caller then skips
    /// caret-follow for the frame, since the fold, not the user, moved the
    /// caret.
    ///
    /// A toggle records the line's y; this runs the NEXT frame, once the galley
    /// reflects the new fold state, and shifts the editor's outer scroll offset
    /// by the difference. Folding 200 lines changes what sits at every pixel
    /// below the header — without this the page slides out from under the
    /// pointer and you lose the block you were looking at.
    ///
    /// The delta is added to `drawn_offset` — the offset the galley was laid
    /// out at, read before the render — not to the stored one: when the content
    /// shrank, egui's `ScrollArea::end` has already clamped and stored a smaller
    /// offset by the time this runs, and adding the delta to THAT undershot all
    /// the way to the top of the file on a collapse-all deep in a large file.
    ///
    /// What it cannot undo: egui clamps the offset when the content shrinks
    /// below the viewport's reach, and there is no scrolling past the end, so
    /// folding the LAST block while its header sits at the top of the view
    /// still lets the header drop a few rows.
    ///
    /// Same one-frame lag as `apply_pending_scroll`: the correction lands after
    /// this frame was laid out, so a repaint is requested for it to show.
    pub(super) fn apply_fold_anchor(
        &mut self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        editor_id: &str,
        map: &FoldMap,
        rel: &str,
        drawn_offset: f32,
    ) -> bool {
        let Some((anchor_rel, line, old_y)) = self.ed.fold_anchor.clone() else {
            return false;
        };
        if anchor_rel != rel {
            return false; // the view moved to another file first
        }
        self.ed.fold_anchor = None;

        // Total: a line that ended up inside another fold pins that fold's
        // header, the row now standing in for it.
        let disp_line = map.display_row_of(line);
        // Char index of that display line's first character.
        let mut ci = 0usize;
        let mut seen = 0usize;
        if disp_line > 0 {
            for (i, c) in map.display().chars().enumerate() {
                if c == '\n' {
                    seen += 1;
                    if seen == disp_line {
                        ci = i + 1;
                        break;
                    }
                }
            }
        }
        let ci = ci.min(editor_resp.galley.text().chars().count());
        let loc = editor_resp
            .galley
            .pos_from_cursor(egui::text::CCursor::new(ci));
        let new_y = editor_resp.galley_pos.y + loc.min.y;
        let delta = new_y - old_y;
        if delta.abs() < 0.5 {
            return true;
        }
        let scroll_id = ui
            .id()
            .with(egui::Id::new(format!("{editor_id}_outer_scroll")));
        if let Some(mut state) = egui::containers::scroll_area::State::load(ui.ctx(), scroll_id) {
            state.offset.y = (drawn_offset + delta).max(0.0);
            state.store(ui.ctx(), scroll_id);
            ui.ctx().request_repaint();
        }
        true
    }
}

impl AppIde {
    /// Two safety checks, run once per frame AFTER the editor and the fold
    /// gutter — by which point every path that can change the fold set for this
    /// frame has already run.
    ///
    /// **A fold toggle clears the editor's undo history.** `TextEdit` snapshots
    /// the text it is shown into its own undo stack, and while folded that text
    /// is a projection with whole lines missing. One later Ctrl+Z would write
    /// that snapshot back over the file with `replace_with` — every folded
    /// block's body deleted at once. The widget is read-only while folded so
    /// nothing new is captured, but the history is keyed to the file for the
    /// whole app run, so anything already in it is dropped on the transition.
    ///
    /// **A file changed from outside drops its folds.** The set is keyed by LINE
    /// NUMBER and nothing else invalidates it: a codegen regeneration, a Clippy
    /// fix or a git restore moves the lines, and a stale head can then land on a
    /// different block's opening brace — folding something the user never asked
    /// to hide. (A change made by TYPING cannot reach here: it unfolds the file
    /// before the editor renders.)
    /// `own_edit`: this frame's text change came from the editor itself, through
    /// the folded delta path. Without that distinction the "the file changed
    /// from outside, drop the folds" rule fires on every keystroke — which is
    /// exactly the block re-expanding as soon as anything is typed.
    pub(super) fn guard_folds(
        &mut self,
        rel: &str,
        text: &str,
        editor_widget_id: egui::Id,
        ctx: &egui::Context,
        own_edit: bool,
    ) {
        use std::hash::{Hash, Hasher};
        let folds_sig = |me: &Self| -> u64 {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            if let Some(set) = me.folds.get(rel) {
                for line in set {
                    line.hash(&mut h);
                }
            }
            h.finish()
        };
        let text_sig = text_sig(text);

        if let Some((prev_folds, prev_text)) = self.fold_guard.get(rel).copied() {
            if prev_text != text_sig && !own_edit && self.folds.contains_key(rel) {
                self.folds.remove(rel);
            }
            // `own_edit` covers the undoer too: an edit made THROUGH the fold
            // leaves the history holding projections of the same structure, one
            // edit behind, which Ctrl+Z can still walk back correctly. The one
            // case that cannot — an edit that deleted a fold header — clears the
            // history at the point it happens.
            if prev_folds != folds_sig(self) && !own_edit {
                // `TextEditState` shares its undoer through an `Arc`, so clearing
                // the loaded copy is enough — no `store` needed.
                if let Some(mut state) = egui::TextEdit::load_state(ctx, editor_widget_id) {
                    state.clear_undoer();
                }
            }
        }
        self.fold_guard
            .insert(rel.to_owned(), (folds_sig(self), text_sig));
    }
}

#[cfg(test)]
mod tests {
    use super::{Request, anchor_row, fold_all_request};

    /// A request raised on one file must never fold another: via the menu on
    /// a library's `Cargo.toml` it used to sit armed until the next Rust file
    /// opened, and collapse every function there unasked.
    #[test]
    fn a_request_from_another_file_is_dropped_not_carried_over() {
        assert_eq!(
            fold_all_request(Some("mylib/Cargo.toml".into()), "mylib/src/lib.rs", false),
            Some(Request::Drop)
        );
        assert_eq!(fold_all_request(None, "src/main.rs", false), None);
    }

    /// A keystroke coalesced into the shortcut's frame would make the fold
    /// guard read the new folds as a change from outside and drop them.
    #[test]
    fn a_request_on_a_frame_that_edited_waits_one_frame() {
        assert_eq!(
            fold_all_request(Some("src/main.rs".into()), "src/main.rs", true),
            Some(Request::Wait)
        );
        assert_eq!(
            fold_all_request(Some("src/main.rs".into()), "src/main.rs", false),
            Some(Request::Fire)
        );
    }

    /// The reported jump, on the toggle-all path: a caret the user scrolled
    /// away from must not pull the view back to itself.
    #[test]
    fn an_off_screen_caret_does_not_choose_the_anchor() {
        assert_eq!(anchor_row(Some(328), 100..140), 100);
        assert_eq!(anchor_row(Some(3), 100..140), 100);
        assert_eq!(anchor_row(None, 100..140), 100);
    }

    #[test]
    fn a_visible_caret_is_the_anchor() {
        assert_eq!(anchor_row(Some(120), 100..140), 120);
        assert_eq!(anchor_row(Some(100), 100..140), 100);
        // `end` is exclusive: a row cut off at the bottom is not "on screen".
        assert_eq!(anchor_row(Some(140), 100..140), 100);
    }
}
