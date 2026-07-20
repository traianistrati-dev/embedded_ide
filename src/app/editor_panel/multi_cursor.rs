//! Ctrl+Shift+Up/Down multi-cursor editing.
//!
//! Ctrl+Shift+Up adds a caret one line above the topmost caret in the set
//! (primary + extras); Ctrl+Shift+Down adds one below the bottommost. Each key
//! first checks whether the MOST RECENTLY added extra caret sits on the
//! OPPOSITE side of the primary from the direction just pressed — if so, that
//! press undoes it instead of adding a new one, so repeated opposite-direction
//! presses fully unwind before that side starts growing (e.g. one
//! Ctrl+Shift+Up then one Ctrl+Shift+Down removes exactly the caret Up just
//! added, rather than adding a second caret below). Escape clears every extra
//! caret, back to just the primary. While extra carets exist, typing /
//! Backspace / Delete / paste at the primary caret is replayed identically at
//! each of them.
//!
//! `egui::TextEdit` has no native multi-cursor support — there is exactly one
//! (primary/secondary) cursor. Extra carets are tracked ourselves as
//! [`ExtraCaret`] (`anchor` + `head`, so each carries its own SELECTION), and a
//! text edit is captured by diffing this frame's text against last frame's (a
//! single contiguous insert/delete — the normal shape of one keystroke or one
//! paste).
//!
//! **The replay is per-caret, not literal.** Each caret replaces ITS OWN
//! selection with the inserted text; only a real Backspace/Delete keypress —
//! recognised by the primary having had NO selection — deletes characters a
//! caret does not own. Spans therefore differ in length between carets, so the
//! replay walks them in ASCENDING order carrying a running shift: every edit
//! already applied sits below the next one and moves it.
//!
//! Arrow keys move the whole set: egui moves the primary and each extra follows
//! (vertical moves keep each caret's OWN column, so a staggered block stays
//! staggered; blank lines are skipped, because a caret there would snap to
//! column 0). Shift+arrow extends each caret's own selection instead of
//! collapsing it. Ctrl-modified arrows are excluded — those are add-caret
//! (Ctrl+Shift+Up/Down) and move-line (Ctrl+Up/Down).
//!
//! Scope: mouse clicks still re-anchor to a single caret, no other navigation
//! (Home/End/PageUp/PageDown) is mirrored, and copy/paste acts on the primary
//! selection only.

use crate::app::{AppIde, ProjectFileId};
use crate::editor::gui::text_pos::{lsp_cursor_pos, lsp_pos_to_char_idx};
use eframe::egui;
use egui::text_edit::TextEditOutput;

/// One extra caret, with its selection.
///
/// `head` is where the caret sits and where arrow keys move it; `anchor` is
/// where its selection started. They are equal when there is no selection — the
/// common case — and `anchor` may be after `head` (selecting leftwards).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ExtraCaret {
    pub anchor: usize,
    pub head: usize,
}

impl ExtraCaret {
    pub(crate) fn at(pos: usize) -> Self {
        Self {
            anchor: pos,
            head: pos,
        }
    }
    /// The selected span as `(start, end)` with `start <= end`.
    fn range(self) -> (usize, usize) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }
    fn has_selection(self) -> bool {
        self.anchor != self.head
    }
}

/// `true` when 0-based `line0` of `text` holds nothing but whitespace. The
/// final line after a trailing newline counts as blank (`str::lines` does not
/// yield it, while the caret coordinate helpers do address it).
fn line_is_blank(text: &str, line0: u32) -> bool {
    text.lines()
        .nth(line0 as usize)
        .map_or(true, |l| l.trim().is_empty())
}

/// Nearest line with text starting at `from` and walking in `dir` (`-1` up,
/// `+1` down); `None` when the edge is reached without finding one.
///
/// Carets are only ever placed on lines that have text: on a blank line
/// `lsp_pos_to_char_idx` clamps to column 0, so the caret jumps to the far left
/// and visibly breaks out of the column the rest of the set shares.
fn nearest_text_line(text: &str, from: i32, dir: i32) -> Option<u32> {
    let last_line0 = text.chars().filter(|&c| c == '\n').count() as i32;
    let mut line = from;
    while (0..=last_line0).contains(&line) {
        if !line_is_blank(text, line as u32) {
            return Some(line as u32);
        }
        line += dir;
    }
    None
}

/// Char index of a new caret one line above the topmost of `primary_idx` +
/// `extra_cursors`, at `primary_idx`'s own column. `None` when the topmost
/// caret is already on the file's first line, or the computed position
/// already has a caret.
fn cursor_above(text: &str, primary_idx: usize, extra_cursors: &[ExtraCaret]) -> Option<usize> {
    let (_, col0) = lsp_cursor_pos(text, primary_idx);
    let topmost_idx = extra_cursors
        .iter()
        .map(|c| c.head)
        .chain(std::iter::once(primary_idx))
        .min_by_key(|&idx| lsp_cursor_pos(text, idx).0)
        .unwrap_or(primary_idx);
    let (topmost_line0, _) = lsp_cursor_pos(text, topmost_idx);
    // Skip blank lines on the way up — nothing above means no room.
    let target_line0 = nearest_text_line(text, topmost_line0 as i32 - 1, -1)?;
    // `lsp_pos_to_char_idx` is 1-based.
    let new_idx = lsp_pos_to_char_idx(text, target_line0 + 1, col0 + 1);
    if new_idx == primary_idx || extra_cursors.iter().any(|c| c.head == new_idx) {
        None
    } else {
        Some(new_idx)
    }
}

/// Char index of a new caret one line below the bottommost of `primary_idx` +
/// `extra_cursors`, at `primary_idx`'s own column. `None` when the bottommost
/// caret is already on the file's last line, or the computed position already
/// has a caret. Mirror of [`cursor_above`].
fn cursor_below(text: &str, primary_idx: usize, extra_cursors: &[ExtraCaret]) -> Option<usize> {
    let (_, col0) = lsp_cursor_pos(text, primary_idx);
    let bottommost_idx = extra_cursors
        .iter()
        .map(|c| c.head)
        .chain(std::iter::once(primary_idx))
        .max_by_key(|&idx| lsp_cursor_pos(text, idx).0)
        .unwrap_or(primary_idx);
    let (bottommost_line0, _) = lsp_cursor_pos(text, bottommost_idx);
    // Skip blank lines on the way down — nothing below means no room.
    let target_line0 = nearest_text_line(text, bottommost_line0 as i32 + 1, 1)?;
    // `lsp_pos_to_char_idx` is 1-based.
    let new_idx = lsp_pos_to_char_idx(text, target_line0 + 1, col0 + 1);
    if new_idx == primary_idx || extra_cursors.iter().any(|c| c.head == new_idx) {
        None
    } else {
        Some(new_idx)
    }
}

/// Ctrl+Shift+Up's full add/undo logic: if the most-recently-added extra
/// caret is BELOW the primary (added by a previous Ctrl+Shift+Down), pop it
/// (undo) instead of adding a new one above; otherwise add one above via
/// [`cursor_above`]. Returns the new `extra_cursors` list.
fn toggle_up(text: &str, primary_idx: usize, extra_cursors: &[ExtraCaret]) -> Vec<ExtraCaret> {
    let mut out = extra_cursors.to_vec();
    if let Some(&last) = out.last() {
        if last.head > primary_idx {
            out.pop();
            return out;
        }
    }
    if let Some(idx) = cursor_above(text, primary_idx, &out) {
        out.push(ExtraCaret::at(idx));
    }
    out
}

/// Mirror of [`toggle_up`] for Ctrl+Shift+Down: undoes the most recently
/// added extra caret if it's ABOVE the primary, else adds one below via
/// [`cursor_below`].
fn toggle_down(text: &str, primary_idx: usize, extra_cursors: &[ExtraCaret]) -> Vec<ExtraCaret> {
    let mut out = extra_cursors.to_vec();
    if let Some(&last) = out.last() {
        if last.head < primary_idx {
            out.pop();
            return out;
        }
    }
    if let Some(idx) = cursor_below(text, primary_idx, &out) {
        out.push(ExtraCaret::at(idx));
    }
    out
}

/// A plain arrow-key caret movement, mirrored from the primary onto the extras.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum CaretMove {
    Left,
    Right,
    Up,
    Down,
}

/// Where one caret lands after `dir`. Clamped at the buffer edges: a caret on
/// the first line stays put on `Up`, one at the end stays put on `Right`.
/// Vertical moves keep the caret's OWN column, so a block of carets stays a
/// block instead of collapsing onto the primary's column.
fn moved_caret(text: &str, idx: usize, dir: CaretMove) -> usize {
    match dir {
        CaretMove::Left => idx.saturating_sub(1),
        CaretMove::Right => (idx + 1).min(text.chars().count()),
        CaretMove::Up | CaretMove::Down => {
            let (line0, col0) = lsp_cursor_pos(text, idx);
            // Step over blank lines rather than landing on one: a caret there
            // snaps to column 0 and breaks the set's shared column. Stay put
            // when there is no line with text left in that direction.
            let step = if dir == CaretMove::Up { -1 } else { 1 };
            let Some(target_line0) = nearest_text_line(text, line0 as i32 + step, step) else {
                return idx;
            };
            // `lsp_pos_to_char_idx` is 1-based in both axes.
            lsp_pos_to_char_idx(text, target_line0 + 1, col0 + 1)
        }
    }
}

/// Apply `dir` to every extra caret, mirroring what egui just did to the
/// primary.
///
/// `extend` is Shift being held: the head moves and the anchor stays, growing
/// each caret's own selection. Without it the selection collapses — a plain
/// arrow is a move, not a select.
///
/// Carets that land on the primary are dropped and duplicates merged (two
/// carets in the same state are one caret) — **in place**, because
/// `toggle_up`/`toggle_down` treat the LAST entry as the most recently added
/// one when deciding whether a press undoes instead of adds.
fn move_extras(
    text: &str,
    extras: &[ExtraCaret],
    dir: CaretMove,
    extend: bool,
    primary_idx: usize,
) -> Vec<ExtraCaret> {
    let mut out: Vec<ExtraCaret> = extras
        .iter()
        .map(|&c| {
            let head = moved_caret(text, c.head, dir);
            ExtraCaret {
                anchor: if extend { c.anchor } else { head },
                head,
            }
        })
        .collect();
    let mut kept: Vec<ExtraCaret> = Vec::with_capacity(out.len());
    out.retain(|&c| {
        // A caret only collides with the primary when neither has a selection —
        // two overlapping selections are still two distinct edit targets.
        let on_primary = !c.has_selection() && c.head == primary_idx;
        let keep = !on_primary && !kept.contains(&c);
        if keep {
            kept.push(c);
        }
        keep
    });
    out
}

/// The single contiguous edit between `old` and `new`: `(start, removed_len,
/// inserted)` in `old`'s char-index space. `None` when the texts are equal.
fn diff_edit(old: &[char], new: &[char]) -> Option<(usize, usize, String)> {
    if old == new {
        return None;
    }
    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    let mut suffix = 0;
    while suffix < max_suffix && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix] {
        suffix += 1;
    }
    let removed_len = old.len() - prefix - suffix;
    let inserted: String = new[prefix..new.len() - suffix].iter().collect();
    Some((prefix, removed_len, inserted))
}

/// Replay the edit between `old` and `new` at every position in
/// `extra_cursors` (given in `old`'s coordinate space), returning `(new text,
/// each extra caret's new position — deduplicated, never colliding with
/// `new_primary_idx` —, the primary caret's OWN required shift)`.
///
/// The shift matters because `Ctrl+Shift+Up` only ever adds carets ABOVE the
/// primary: replaying an insert/delete at one of them changes the buffer's
/// length BEFORE the primary's own position, so the primary's effective index
/// moves too even though its own edit already landed correctly — the caller
/// must apply this shift to egui's stored cursor state or the caret drifts
/// out of place the next time something is typed above it.
///
/// `old_primary_idx` (the primary caret's position last frame) disambiguates
/// Backspace (deletes BEFORE the old cursor) from Delete (deletes AFTER it) —
/// both leave the primary caret at the same spot afterward, so the edit's
/// shape alone can't tell them apart. Pure / UI-free so it's directly
/// unit-testable.
fn replay_edit(
    old: &str,
    new: &str,
    old_primary: (usize, usize),
    new_primary_idx: usize,
    extra_cursors: &[ExtraCaret],
) -> (String, Vec<ExtraCaret>, isize) {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let Some((prefix, removed_len, inserted)) = diff_edit(&old_chars, &new_chars) else {
        return (new.to_owned(), extra_cursors.to_vec(), 0);
    };
    let edit_end_old = prefix + removed_len;
    let (old_primary_anchor, old_primary_head) = old_primary;
    let primary_had_selection = old_primary_anchor != old_primary_head;
    // Only a real Backspace/Delete keypress (no selection) deletes characters
    // the caret does not own. When the primary REPLACED a selection, each extra
    // replaces its OWN selection instead — same keystroke, different spans.
    let is_backspace_like =
        !primary_had_selection && removed_len > 0 && old_primary_head >= edit_end_old;
    let is_plain_deletion = !primary_had_selection && removed_len > 0 && inserted.is_empty();
    let ins_chars: Vec<char> = inserted.chars().collect();
    let ins_len = ins_chars.len();

    // The span each caret replaces, in OLD coordinates.
    let span_of = |c: ExtraCaret| -> (usize, usize) {
        if c.has_selection() {
            c.range()
        } else if is_plain_deletion {
            if is_backspace_like {
                (c.head.saturating_sub(removed_len), c.head)
            } else {
                (c.head, c.head + removed_len)
            }
        } else {
            (c.head, c.head) // pure insert
        }
    };

    // Ascending, carrying a running shift: every edit already applied sits
    // BELOW the next one and moves it. (Descending order plus a per-caret
    // correction was the previous shape; it only worked while every caret
    // replaced a span of the SAME length, which selections break.)
    let mut order: Vec<ExtraCaret> = extra_cursors.to_vec();
    order.sort_unstable_by_key(|c| c.range().0);

    let mut buf: Vec<char> = new_chars;
    let mut placed: Vec<ExtraCaret> = Vec::with_capacity(extra_cursors.len());
    let mut primary_shift: isize = 0;
    let mut cum: isize = 0;
    for c in order {
        let (s_old, e_old) = span_of(c);
        // A span overlapping the primary's own edit is ambiguous to replay —
        // drop that caret rather than risk corrupting the text.
        let overlaps = s_old < edit_end_old && e_old > prefix;
        let inside = s_old >= prefix && s_old < edit_end_old;
        if overlaps || inside {
            continue;
        }
        // OLD → NEW coordinates: only positions after the primary's edit move.
        let to_new = |p: usize| -> isize {
            if p >= edit_end_old {
                p as isize + (ins_len as isize - removed_len as isize)
            } else {
                p as isize
            }
        };
        let start = (to_new(s_old) + cum).max(0) as usize;
        let end = (to_new(e_old) + cum).max(0) as usize;
        let start = start.min(buf.len());
        let end = end.min(buf.len()).max(start);

        buf.splice(start..end, ins_chars.iter().copied());
        placed.push(ExtraCaret::at(start + ins_len));

        let own_delta = ins_len as isize - (end - start) as isize;
        cum += own_delta;
        if s_old < old_primary_head {
            primary_shift += own_delta;
        }
    }

    let mut new_positions = placed;

    // Collide-check against the primary's FINAL position (after its own
    // shift), not its pre-shift one — both are now expressed in the same
    // (fully-edited) buffer's coordinate space.
    let final_primary_idx = (new_primary_idx as isize + primary_shift).max(0) as usize;
    new_positions.retain(|c| c.head != final_primary_idx);
    new_positions.sort_unstable_by_key(|c| c.head);
    new_positions.dedup();
    (buf.into_iter().collect(), new_positions, primary_shift)
}

impl AppIde {
    /// Handle Ctrl+Shift+Up / Down (see the module docs for the add-vs-undo
    /// rule), then replay this frame's text edit (if any) at every extra
    /// caret — mutating `display_code` in place when a replay happens.
    /// Returns `Some(primary_shift)` when multi-cursor claimed this frame's
    /// edit (the caller applies the shift to egui's cursor state and skips
    /// its own line-op shortcuts, which assume a single cursor/selection).
    pub(super) fn handle_multi_cursor(
        &mut self,
        display_code: &mut String,
        text_before: &str,
        editor_resp: &TextEditOutput,
        displayed_file: ProjectFileId,
        up_pressed: bool,
        down_pressed: bool,
        escape_pressed: bool,
        // An arrow key pressed this frame: egui already moved the primary, we
        // mirror it onto the extras. `extend` = Shift held, i.e. grow each
        // caret's own selection instead of collapsing it.
        caret_move: Option<(CaretMove, bool)>,
    ) -> Option<isize> {
        if self.extra_cursors_file != Some(displayed_file) {
            self.extra_cursors.clear();
            self.extra_cursors_file = Some(displayed_file);
            self.mc_prev_primary_sel = None;
        }
        if escape_pressed {
            self.extra_cursors.clear();
        }

        let primary_idx = editor_resp
            .state
            .cursor
            .char_range()
            .map(|r| r.primary.index)
            .unwrap_or(0)
            .min(text_before.chars().count());

        if up_pressed {
            self.extra_cursors = toggle_up(text_before, primary_idx, &self.extra_cursors);
        }
        if down_pressed {
            self.extra_cursors = toggle_down(text_before, primary_idx, &self.extra_cursors);
        }

        // An arrow key moves the WHOLE cursor set, not just the primary —
        // otherwise the extras are left behind the moment you navigate. egui
        // has already moved the primary (this runs after the editor), so
        // `primary_idx` is its post-move position and the extras follow.
        if let Some((dir, extend)) = caret_move {
            if !self.extra_cursors.is_empty() {
                self.extra_cursors =
                    move_extras(text_before, &self.extra_cursors, dir, extend, primary_idx);
            }
        }

        let mut shift = None;
        if !self.extra_cursors.is_empty() && text_before != display_code.as_str() {
            // The primary's PREVIOUS selection, not just its index: it tells a
            // real Backspace/Delete keypress (which deletes text the caret does
            // not own, and must be replayed literally) apart from typing OVER a
            // selection (where every caret replaces its OWN span instead).
            let old_primary = self
                .mc_prev_primary_sel
                .unwrap_or((primary_idx, primary_idx));
            let (new_text, new_extras, primary_shift) = replay_edit(
                text_before,
                display_code,
                old_primary,
                primary_idx,
                &self.extra_cursors,
            );
            *display_code = new_text;
            self.extra_cursors = new_extras;
            shift = Some(primary_shift);
        }

        self.mc_prev_primary_sel = editor_resp
            .state
            .cursor
            .char_range()
            .map(|r| (r.secondary.index, r.primary.index));
        shift
    }

    /// Paint a thin caret at each extra cursor position, after the editor
    /// renders, so the user can see every active insertion point.
    pub(super) fn paint_extra_cursors(
        &self,
        ui: &egui::Ui,
        galley_pos: egui::Pos2,
        clip: egui::Rect,
        galley: &egui::text::Galley,
        display_code: &str,
    ) {
        if self.extra_cursors.is_empty() {
            return;
        }
        let total_chars = display_code.chars().count();
        let painter = ui.painter().with_clip_rect(clip);
        let color = egui::Color32::from_rgb(255, 200, 60);
        // Translucent, so the text stays readable underneath — same idea as the
        // diagnostic bands.
        let sel_fill = egui::Color32::from_rgba_unmultiplied(255, 200, 60, 48);

        for &caret in &self.extra_cursors {
            // ── Selection band ────────────────────────────────────────────
            // Painted per ROW: a selection spanning several lines is not one
            // rectangle, and `pos_from_cursor` only gives a single glyph's box.
            if caret.has_selection() {
                let (s, e) = caret.range();
                let (s, e) = (s.min(total_chars), e.min(total_chars));
                let mut row_start = s;
                while row_start < e {
                    let a = galley.pos_from_cursor(egui::text::CCursor::new(row_start));
                    // Walk to the last char of this visual row within [s, e).
                    let mut row_end = row_start;
                    while row_end + 1 <= e {
                        let b = galley.pos_from_cursor(egui::text::CCursor::new(row_end + 1));
                        if (b.min.y - a.min.y).abs() > 0.5 {
                            break; // next row starts here
                        }
                        row_end += 1;
                    }
                    let b = galley.pos_from_cursor(egui::text::CCursor::new(row_end));
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(galley_pos.x + a.min.x, galley_pos.y + a.min.y),
                        egui::pos2(galley_pos.x + b.max.x, galley_pos.y + a.max.y),
                    );
                    if rect.bottom() >= clip.top() && rect.top() <= clip.bottom() {
                        painter.rect_filled(rect, 1.0, sel_fill);
                    }
                    if row_end == row_start {
                        break; // no progress possible — don't spin
                    }
                    row_start = row_end;
                }
            }

            // ── The caret itself ──────────────────────────────────────────
            let idx = caret.head.min(total_chars);
            let loc = galley.pos_from_cursor(egui::text::CCursor::new(idx));
            let x = galley_pos.x + loc.min.x;
            let y_top = galley_pos.y + loc.min.y;
            let y_bot = galley_pos.y + loc.max.y;
            if y_bot < clip.top() || y_top > clip.bottom() {
                continue;
            }
            painter.line_segment(
                [egui::pos2(x, y_top), egui::pos2(x, y_bot)],
                egui::Stroke::new(1.5, color),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CaretMove, ExtraCaret, cursor_above, cursor_below, move_extras, replay_edit, toggle_down,
        toggle_up,
    };

    /// A caret with no selection, at `pos`.
    fn c(pos: usize) -> ExtraCaret {
        ExtraCaret::at(pos)
    }
    /// A caret selecting `anchor..head`.
    fn sel(anchor: usize, head: usize) -> ExtraCaret {
        ExtraCaret { anchor, head }
    }
    /// Just the head positions, for terser assertions.
    fn heads(v: &[ExtraCaret]) -> Vec<usize> {
        v.iter().map(|c| c.head).collect()
    }

    #[test]
    fn cursor_above_same_column_one_line_up() {
        let text = "aaa\nbbb\nccc\n";
        // primary at 'c' of "ccc" (line 2, col 1) → new caret at "bbb"'s col 1.
        let primary_idx = text.find("ccc").unwrap() + 1;
        let idx = cursor_above(text, primary_idx, &[]).expect("room above");
        assert_eq!(&text[idx..idx + 1], "b");
    }

    #[test]
    fn cursor_above_none_at_first_line() {
        let text = "only one line";
        assert_eq!(cursor_above(text, 3, &[]), None);
    }

    #[test]
    fn cursor_above_extends_past_existing_extra_cursor() {
        let text = "aaa\nbbb\nccc\n";
        let primary_idx = text.find("ccc").unwrap();
        let first = cursor_above(text, primary_idx, &[]).unwrap(); // on "bbb"
        let second = cursor_above(text, primary_idx, &[c(first)]).unwrap(); // on "aaa"
        assert_eq!(&text[second..second + 1], "a");
    }

    #[test]
    fn cursor_below_same_column_one_line_down() {
        let text = "aaa\nbbb\nccc\n";
        // primary at the second 'a' of "aaa" (line 0, col 1).
        let primary_idx = 1;
        let idx = cursor_below(text, primary_idx, &[]).expect("room below");
        assert_eq!(&text[idx..idx + 1], "b");
    }

    #[test]
    fn cursor_below_none_at_last_line() {
        let text = "only one line";
        assert_eq!(cursor_below(text, 3, &[]), None);
    }

    #[test]
    fn toggle_down_undoes_the_last_up_added_cursor() {
        // Regression guard for the confirmed-working simple case: one
        // Ctrl+Shift+Up then one Ctrl+Shift+Down must remove exactly the
        // caret Up just added — NOT add a second one below.
        let text = "aaa\nbbb\nccc\n";
        let primary_idx = text.find("ccc").unwrap();
        let after_up = toggle_up(text, primary_idx, &[]);
        assert_eq!(after_up.len(), 1, "Up should have added one caret");
        let after_down = toggle_down(text, primary_idx, &after_up);
        assert!(after_down.is_empty(), "Down must undo it, not add a second one");
    }

    #[test]
    fn toggle_up_undoes_the_last_down_added_cursor() {
        // Mirror of the above: Down-then-Up fully unwinds too.
        let text = "aaa\nbbb\nccc\n";
        let primary_idx = 0; // "aaa" start
        let after_down = toggle_down(text, primary_idx, &[]);
        assert_eq!(after_down.len(), 1, "Down should have added one caret");
        let after_up = toggle_up(text, primary_idx, &after_down);
        assert!(after_up.is_empty(), "Up must undo it, not add one above");
    }

    #[test]
    fn toggle_down_adds_below_once_nothing_left_to_undo() {
        // The new capability: Down can ALSO add (not just remove) once the
        // opposite side is empty.
        let text = "aaa\nbbb\nccc\n";
        let primary_idx = 0;
        let after_down = toggle_down(text, primary_idx, &[]);
        assert_eq!(after_down.len(), 1);
        assert_eq!(&text[after_down[0].head..after_down[0].head + 1], "b"); // on "bbb"
    }

    #[test]
    fn toggle_up_extends_further_after_the_first_add() {
        let text = "aaa\nbbb\nccc\n";
        let primary_idx = text.find("ccc").unwrap();
        let after_1 = toggle_up(text, primary_idx, &[]);
        let after_2 = toggle_up(text, primary_idx, &after_1);
        assert_eq!(after_2.len(), 2, "a second Up press extends further, not undo");
        assert!(after_2.iter().any(|c| &text[c.head..c.head + 1] == "a"));
    }

    #[test]
    fn replay_insert_applies_same_char_at_each_extra_cursor() {
        // Two lines, cursors after the 'a' on each; typing 'X' at the primary.
        // old chars: a(0) \n(1) a(2) \n(3) — extra cursor at index 3 sits
        // right after the second 'a' (before its newline), mirroring the
        // primary's own old position (1, right after the first 'a').
        let old = "a\na\n";
        let new = "aX\na\n"; // typed X after the first 'a'
        let extra = vec![c(3)];
        let (out, positions, shift) = replay_edit(old, new, (1, 1), 2, &extra);
        assert_eq!(out, "aX\naX\n");
        // new chars: a(0) X(1) \n(2) a(3) X(4) \n(5) — caret sits right after
        // the second X, i.e. index 5.
        assert_eq!(heads(&positions), vec![5]);
        // The extra cursor (old idx 3) is AFTER the primary (old idx 1), so it
        // doesn't push the primary's own position around.
        assert_eq!(shift, 0);
    }

    #[test]
    fn replay_backspace_deletes_before_each_extra_cursor() {
        let old = "ab\nab\n";
        // Backspace after the primary's 'b' (index 2, end of first line) → "a\nab\n"
        let new = "a\nab\n";
        let extra = vec![c(5)]; // after the second "ab" (index of trailing \n)
        let (out, positions, shift) = replay_edit(old, new, (2, 2), 1, &extra);
        assert_eq!(out, "a\na\n");
        assert_eq!(heads(&positions), vec![3]); // shifted back by the one deleted char
        assert_eq!(shift, 0); // extra is after the primary here too
    }

    #[test]
    fn replay_delete_key_removes_after_each_extra_cursor() {
        // Delete-key at the START of "ab" removes 'a', cursor stays put.
        let old = "ab\nab\n";
        let new = "b\nab\n";
        let extra = vec![c(3)]; // start of the second "ab" line
        // old_primary_idx == prefix (0) → NOT backspace-like → delete-forward.
        let (out, positions, shift) = replay_edit(old, new, (0, 0), 0, &extra);
        assert_eq!(out, "b\nb\n");
        assert_eq!(heads(&positions), vec![2]);
        assert_eq!(shift, 0);
    }

    #[test]
    fn replay_drops_extra_cursor_colliding_with_new_primary() {
        let old = "a\na\n";
        let new = "aX\na\n";
        // Same replay as `replay_insert_applies_same_char_at_each_extra_cursor`
        // (extra cursor at 3 → lands at 5 after the insert) — but this time
        // claim the primary ALSO ended up at 5 (contrived), which must drop
        // the extra caret rather than leave a duplicate on top of the primary.
        let (_out, positions, _shift) = replay_edit(old, new, (1, 1), 5, &[c(3)]);
        assert!(positions.is_empty());
    }

    #[test]
    fn replay_shifts_primary_when_extra_cursor_is_above_it() {
        // The real usage shape: Ctrl+Shift+Up only ever adds carets ABOVE the
        // primary. Extra at line 1 (old idx 0, on "a"), primary at line 2
        // typing 'X' right after 'b' (old idx 3, in "a\nb\n" — chars a(0)
        // \n(1) b(2) \n(3), so 3 = right after 'b').
        let old = "a\nb\n";
        let new = "a\nbX\n"; // primary's own edit already applied by egui
        let extra = vec![c(0)];
        let (out, positions, shift) = replay_edit(old, new, (3, 3), 4, &extra);
        // The extra's replay inserts 'X' right at the very start (its own old
        // position, 0, is unaffected by the primary's edit further along).
        assert_eq!(out, "Xa\nbX\n");
        assert_eq!(heads(&positions), vec![1]); // right after the inserted leading X
        // The primary's OWN effective position must shift forward by the one
        // char inserted before it, or its caret would drift out of place.
        assert_eq!(shift, 1);
    }

    /// Horizontal movement applies to every caret, clamped at the buffer ends.
    #[test]
    fn arrows_move_every_caret_horizontally() {
        //  idx:  0 1 2  3   4 5 6  7
        //        a b c  NL  d e f  NL
        let text = "abc\ndef\n";
        // Primary parked out of the way so nothing is dropped as a collision.
        assert_eq!(heads(&move_extras(text, &[c(1), c(5)], CaretMove::Right, false, 99)), vec![2, 6]);
        assert_eq!(heads(&move_extras(text, &[c(1), c(5)], CaretMove::Left, false, 99)), vec![0, 4]);
        // Clamped: index 0 cannot go left, the last index cannot go right.
        assert_eq!(heads(&move_extras(text, &[c(0)], CaretMove::Left, false, 99)), vec![0]);
        assert_eq!(heads(&move_extras(text, &[c(8)], CaretMove::Right, false, 99)), vec![8]);
    }

    /// Vertical movement keeps each caret's OWN column — using the primary's
    /// column instead would collapse a staggered set into one line shape.
    #[test]
    fn arrows_move_every_caret_vertically_keeping_its_own_column() {
        //  idx:  0 1 2  3   4 5 6  7   8 9 10 11
        //        a b c  NL  d e f  NL  g h i  NL
        let text = "abc\ndef\nghi\n";
        // Carets at column 1 of line 0 and column 2 of line 1.
        assert_eq!(heads(&move_extras(text, &[c(1), c(6)], CaretMove::Down, false, 99)), vec![5, 10]);
        assert_eq!(heads(&move_extras(text, &[c(5), c(10)], CaretMove::Up, false, 99)), vec![1, 6]);
        // No line with text left in that direction → the caret stays put. The
        // trailing empty line after the final newline is blank, so `ghi` is the
        // last line a caret can reach.
        assert_eq!(heads(&move_extras(text, &[c(1)], CaretMove::Up, false, 99)), vec![1]);
        assert_eq!(heads(&move_extras(text, &[c(9)], CaretMove::Down, false, 99)), vec![9]);
    }

    /// A caret must never land on a blank line: `lsp_pos_to_char_idx` clamps to
    /// column 0 there, so it would jump to the far left and break the column
    /// the rest of the set shares (reported with a screenshot).
    #[test]
    fn blank_lines_are_skipped_when_adding_a_caret() {
        //  line: 0      1  2      3  4
        let text = "aaaa\n\nbbbb\n\ncccc\n";
        let primary_idx = text.find("cccc").unwrap() + 2; // line 4, col 2

        // Up from line 4 skips the blank line 3 and lands on `bbbb`.
        let first = cursor_above(text, primary_idx, &[]).expect("room above");
        assert_eq!(&text[first..first + 2], "bb");
        // Again: skips blank line 1 and lands on `aaaa`.
        let second = cursor_above(text, primary_idx, &[c(first)]).expect("room above");
        assert_eq!(&text[second..second + 2], "aa");
        // Nothing but blanks above line 0 → no room.
        assert_eq!(cursor_above(text, second, &[]), None);
    }

    #[test]
    fn blank_lines_are_skipped_when_moving_carets() {
        //  idx:  0 1 2 3  4   5   6 7 8 9  10
        //        a a a a  NL  NL  b b b b  NL
        let text = "aaaa\n\nbbbb\n";
        // Down from line 0 col 2 skips blank line 1 → line 2 col 2 (index 8).
        assert_eq!(heads(&move_extras(text, &[c(2)], CaretMove::Down, false, 99)), vec![8]);
        // And back up again.
        assert_eq!(heads(&move_extras(text, &[c(8)], CaretMove::Up, false, 99)), vec![2]);
    }

    /// A caret that lands on the primary, or on another caret, is one caret.
    #[test]
    fn moving_onto_the_primary_or_a_sibling_merges() {
        let text = "abc\ndef\n";
        // Extra at 1 moves right onto the primary at 2 → dropped.
        assert!(move_extras(text, &[c(1)], CaretMove::Right, false, 2).is_empty());
        // Two carets landing on the same spot collapse to one.
        assert_eq!(heads(&move_extras(text, &[c(0), c(0)], CaretMove::Right, false, 99)), vec![1]);
    }

    /// Shift+arrow grows each caret's OWN selection; the anchor stays put.
    #[test]
    fn shift_arrows_extend_each_selection() {
        let text = "abc\ndef\n";
        let out = move_extras(text, &[c(0), c(4)], CaretMove::Right, true, 99);
        assert_eq!(out, vec![sel(0, 1), sel(4, 5)]);
        // Extending again keeps the same anchors.
        let out = move_extras(text, &out, CaretMove::Right, true, 99);
        assert_eq!(out, vec![sel(0, 2), sel(4, 6)]);
        // A PLAIN arrow collapses: a move is not a select.
        let out = move_extras(text, &out, CaretMove::Right, false, 99);
        assert_eq!(out, vec![c(3), c(7)]);
    }

    /// Typing with selections replaces EACH caret's own span — the spans have
    /// different lengths, which is exactly what the old "replay the identical
    /// operation everywhere" model could not express.
    #[test]
    fn typing_replaces_each_caret_s_own_selection() {
        //  idx:  0 1 2 3  4   5 6 7  8
        //        a a a a  NL  b b b  NL
        let old = "aaaa\nbbb\n";
        // Primary selected `bbb` (5..8) and typed X — egui already applied it.
        let new = "aaaa\nX\n";
        // The extra selected just `aa` (1..3): a DIFFERENT length.
        let extra = vec![sel(1, 3)];
        let (out, positions, shift) = replay_edit(old, new, (5, 8), 6, &extra);

        assert_eq!(out, "aXa\nX\n", "each selection replaced by the typed text");
        assert_eq!(heads(&positions), vec![2], "caret lands after its own X");
        // The extra is above the primary and shortened the buffer by one char
        // (2 selected -> 1 typed), so the primary must shift back by one.
        assert_eq!(shift, -1);
    }

    /// Backspace with selections deletes each span — same path, empty insert.
    #[test]
    fn backspace_deletes_each_caret_s_own_selection() {
        let old = "aaaa\nbbb\n";
        let new = "aaaa\n\n"; // primary deleted its `bbb` selection
        let extra = vec![sel(1, 3)];
        let (out, positions, shift) = replay_edit(old, new, (5, 8), 5, &extra);
        assert_eq!(out, "aa\n\n");
        assert_eq!(heads(&positions), vec![1]);
        assert_eq!(shift, -2);
    }

    /// Mixed: one caret has a selection, another does not. The selected one
    /// replaces its span, the bare one just inserts — no phantom deletion.
    #[test]
    fn a_caret_without_a_selection_only_inserts() {
        //  idx:  0 1 2 3  4   5 6 7  8   9 10 11 12
        //        a a a a  NL  b b b  NL  c  c  c  NL
        let old = "aaaa\nbbb\nccc\n";
        // Primary selected `ccc` (9..12) and typed X.
        let new = "aaaa\nbbb\nX\n";
        let extra = vec![sel(1, 3), c(6)]; // one selection, one bare caret
        let (out, positions, _shift) = replay_edit(old, new, (9, 12), 10, &extra);
        assert_eq!(
            out, "aXa\nbXbb\nX\n",
            "the bare caret inserts without deleting anything"
        );
        assert_eq!(heads(&positions), vec![2, 6]);
    }

    /// THE regression this module shipped with. Every other test here uses a
    /// SINGLE extra caret — and with one caret nothing sits below it to shift
    /// it, so the bug was invisible. With three, each caret is displaced by
    /// every edit replayed BELOW it: the middle drifted by one char and the top
    /// by two, so the next keystroke landed in the wrong column.
    #[test]
    fn replay_insert_keeps_every_caret_aligned_with_three_cursors() {
        //  idx:  0 1 2  3   4 5 6  7   8 9 10 11
        //        a a a  NL  b b b  NL  c c c  NL
        let old = "aaa\nbbb\nccc\n";
        // Primary sits at the end of the LAST line (11); typing X there.
        let new = "aaa\nbbb\ncccX\n";
        let extra = vec![c(3), c(7)]; // ends of line 1 and line 2
        let (out, positions, shift) = replay_edit(old, new, (11, 11), 12, &extra);

        assert_eq!(out, "aaaX\nbbbX\ncccX\n");
        //  idx:  0 1 2 3  4   5 6 7 8  9   10 11 12 13  14
        //        a a a X  NL  b b b X  NL   c  c  c  X  NL
        // Each caret belongs right after its own X: 4 and 9.
        assert_eq!(
            heads(&positions),
            vec![4, 9],
            "the upper caret must account for the insert replayed below it"
        );
        // Both extras are above the primary, so it moves by their two chars.
        assert_eq!(shift, 2);
    }

    /// Same for a deletion — there the drift is negative.
    #[test]
    fn replay_backspace_keeps_every_caret_aligned_with_three_cursors() {
        //  idx:  0 1  2   3 4  5   6 7  8
        //        a b  NL  a b  NL  a b  NL
        let old = "ab\nab\nab\n";
        // Backspace at the end of the LAST line (primary 8 -> 7).
        let new = "ab\nab\na\n";
        let extra = vec![c(2), c(5)]; // ends of line 1 and line 2
        let (out, positions, shift) = replay_edit(old, new, (8, 8), 7, &extra);

        assert_eq!(out, "a\na\na\n");
        //  idx:  0  1   2  3   4  5
        //        a  NL  a  NL  a  NL
        assert_eq!(
            heads(&positions),
            vec![1, 3],
            "the upper caret must account for the deletion replayed below it"
        );
        assert_eq!(shift, -2);
    }
}
