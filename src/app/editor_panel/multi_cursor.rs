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
//! (primary/secondary) cursor. Extra carets are tracked ourselves as plain
//! char indices, and a text edit is captured by diffing this frame's text
//! against last frame's (a single contiguous insert/delete — the normal shape
//! of one keystroke or one paste) and replaying that same operation at each
//! extra position, back-to-front so an earlier replay never shifts a
//! not-yet-processed one out from under itself (the same technique already
//! used for applying several Clippy fixes at once — see `app::apply_edits_to_buffer`).
//!
//! PLAIN arrow keys move the whole set: egui moves the primary and each extra
//! caret is moved the same way (vertical moves keep each caret's OWN column, so
//! a staggered block stays staggered). MODIFIED arrows are excluded — Ctrl+Shift
//! adds a caret, Ctrl moves a line, and Shift extends a SELECTION, which an
//! extra caret cannot represent: it is a bare position, not a range.
//!
//! Scope: mouse clicks still re-anchor to a single caret, and no other
//! navigation (Home/End/PageUp/PageDown) is mirrored yet.

use crate::app::{AppIde, ProjectFileId};
use crate::editor::gui::text_pos::{lsp_cursor_pos, lsp_pos_to_char_idx};
use eframe::egui;
use egui::text_edit::TextEditOutput;

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
fn cursor_above(text: &str, primary_idx: usize, extra_cursors: &[usize]) -> Option<usize> {
    let (_, col0) = lsp_cursor_pos(text, primary_idx);
    let topmost_idx = extra_cursors
        .iter()
        .copied()
        .chain(std::iter::once(primary_idx))
        .min_by_key(|&idx| lsp_cursor_pos(text, idx).0)
        .unwrap_or(primary_idx);
    let (topmost_line0, _) = lsp_cursor_pos(text, topmost_idx);
    // Skip blank lines on the way up — nothing above means no room.
    let target_line0 = nearest_text_line(text, topmost_line0 as i32 - 1, -1)?;
    // `lsp_pos_to_char_idx` is 1-based.
    let new_idx = lsp_pos_to_char_idx(text, target_line0 + 1, col0 + 1);
    if new_idx == primary_idx || extra_cursors.contains(&new_idx) {
        None
    } else {
        Some(new_idx)
    }
}

/// Char index of a new caret one line below the bottommost of `primary_idx` +
/// `extra_cursors`, at `primary_idx`'s own column. `None` when the bottommost
/// caret is already on the file's last line, or the computed position already
/// has a caret. Mirror of [`cursor_above`].
fn cursor_below(text: &str, primary_idx: usize, extra_cursors: &[usize]) -> Option<usize> {
    let (_, col0) = lsp_cursor_pos(text, primary_idx);
    let bottommost_idx = extra_cursors
        .iter()
        .copied()
        .chain(std::iter::once(primary_idx))
        .max_by_key(|&idx| lsp_cursor_pos(text, idx).0)
        .unwrap_or(primary_idx);
    let (bottommost_line0, _) = lsp_cursor_pos(text, bottommost_idx);
    // Skip blank lines on the way down — nothing below means no room.
    let target_line0 = nearest_text_line(text, bottommost_line0 as i32 + 1, 1)?;
    // `lsp_pos_to_char_idx` is 1-based.
    let new_idx = lsp_pos_to_char_idx(text, target_line0 + 1, col0 + 1);
    if new_idx == primary_idx || extra_cursors.contains(&new_idx) {
        None
    } else {
        Some(new_idx)
    }
}

/// Ctrl+Shift+Up's full add/undo logic: if the most-recently-added extra
/// caret is BELOW the primary (added by a previous Ctrl+Shift+Down), pop it
/// (undo) instead of adding a new one above; otherwise add one above via
/// [`cursor_above`]. Returns the new `extra_cursors` list.
fn toggle_up(text: &str, primary_idx: usize, extra_cursors: &[usize]) -> Vec<usize> {
    let mut out = extra_cursors.to_vec();
    if let Some(&last) = out.last() {
        if last > primary_idx {
            out.pop();
            return out;
        }
    }
    if let Some(idx) = cursor_above(text, primary_idx, &out) {
        out.push(idx);
    }
    out
}

/// Mirror of [`toggle_up`] for Ctrl+Shift+Down: undoes the most recently
/// added extra caret if it's ABOVE the primary, else adds one below via
/// [`cursor_below`].
fn toggle_down(text: &str, primary_idx: usize, extra_cursors: &[usize]) -> Vec<usize> {
    let mut out = extra_cursors.to_vec();
    if let Some(&last) = out.last() {
        if last < primary_idx {
            out.pop();
            return out;
        }
    }
    if let Some(idx) = cursor_below(text, primary_idx, &out) {
        out.push(idx);
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
/// primary. Carets that land on the primary are dropped and duplicates merged
/// (two carets sharing a position are one caret) — **in place**, because
/// `toggle_up`/`toggle_down` treat the LAST entry as the most recently added
/// one when deciding whether a press undoes instead of adds.
fn move_extras(text: &str, extras: &[usize], dir: CaretMove, primary_idx: usize) -> Vec<usize> {
    let mut out: Vec<usize> = extras
        .iter()
        .map(|&idx| moved_caret(text, idx, dir))
        .collect();
    let mut kept: Vec<usize> = Vec::with_capacity(out.len());
    out.retain(|&c| {
        let keep = c != primary_idx && !kept.contains(&c);
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
    old_primary_idx: usize,
    new_primary_idx: usize,
    extra_cursors: &[usize],
) -> (String, Vec<usize>, isize) {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let Some((prefix, removed_len, inserted)) = diff_edit(&old_chars, &new_chars) else {
        return (new.to_owned(), extra_cursors.to_vec(), 0);
    };
    let edit_end_old = prefix + removed_len;
    let is_backspace_like = removed_len > 0 && old_primary_idx >= edit_end_old;
    let delta = inserted.chars().count() as isize - removed_len as isize;
    let ins_chars: Vec<char> = inserted.chars().collect();

    // Back-to-front (highest first): an edit only shifts char indices AT/AFTER
    // its own start, so processing the highest caret first never invalidates
    // a not-yet-processed (lower) one's already-computed position.
    let mut order: Vec<usize> = extra_cursors.to_vec();
    order.sort_unstable_by(|a, b| b.cmp(a));

    let mut buf: Vec<char> = new_chars;
    // `(old_idx, caret position recorded in the buffer state at ITS splice)`.
    // The position is not final yet — see the rank fix-up after the loop.
    let mut placed: Vec<(usize, usize)> = Vec::with_capacity(extra_cursors.len());
    let mut primary_shift: isize = 0;
    for old_idx in order {
        // A caret that landed inside the primary edit's own old span is
        // ambiguous to replay — drop it rather than risk corruption.
        if old_idx >= prefix && old_idx < edit_end_old {
            continue;
        }
        let shifted = if old_idx >= edit_end_old {
            (old_idx as isize + delta).max(0) as usize
        } else {
            old_idx
        };
        let (start, end, caret_after) = if removed_len > 0 && inserted.is_empty() {
            if is_backspace_like {
                let s = shifted.saturating_sub(removed_len);
                (s, shifted, s)
            } else {
                let e = (shifted + removed_len).min(buf.len());
                (shifted, e, shifted)
            }
        } else {
            (shifted, shifted, shifted + ins_chars.len())
        };
        let start = start.min(buf.len());
        let end = end.min(buf.len()).max(start);
        buf.splice(start..end, ins_chars.iter().copied());
        placed.push((old_idx, caret_after));
        if old_idx < old_primary_idx {
            primary_shift += delta;
        }
    }

    // Each caret's recorded position was correct for the buffer as it stood at
    // ITS OWN splice — but every caret processed afterwards sits LOWER and
    // shifts it by another `delta`. Sorted ascending, the n-th caret has
    // exactly n edits below it.
    //
    // Missing this is invisible with a single extra caret (nothing is below
    // it), which is why every test here used one and the bug shipped: with
    // three carets the middle drifted by `delta` and the top by `2 * delta`,
    // so the next keystroke landed at the wrong offset.
    placed.sort_unstable_by_key(|&(old_idx, _)| old_idx);
    let mut new_positions: Vec<usize> = placed
        .into_iter()
        .enumerate()
        .map(|(rank, (_, caret))| (caret as isize + delta * rank as isize).max(0) as usize)
        .collect();

    // Collide-check against the primary's FINAL position (after its own
    // shift), not its pre-shift one — both are now expressed in the same
    // (fully-edited) buffer's coordinate space.
    let final_primary_idx = (new_primary_idx as isize + primary_shift).max(0) as usize;
    new_positions.retain(|&c| c != final_primary_idx);
    new_positions.sort_unstable();
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
        // A plain (unmodified) arrow key pressed this frame: egui already moved
        // the primary, we mirror it onto the extras.
        caret_move: Option<CaretMove>,
    ) -> Option<isize> {
        if self.extra_cursors_file != Some(displayed_file) {
            self.extra_cursors.clear();
            self.extra_cursors_file = Some(displayed_file);
            self.mc_prev_primary_idx = None;
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
        if let Some(dir) = caret_move {
            if !self.extra_cursors.is_empty() {
                self.extra_cursors =
                    move_extras(text_before, &self.extra_cursors, dir, primary_idx);
            }
        }

        let mut shift = None;
        if !self.extra_cursors.is_empty() && text_before != display_code.as_str() {
            let old_primary_idx = self.mc_prev_primary_idx.unwrap_or(primary_idx);
            let (new_text, new_extras, primary_shift) = replay_edit(
                text_before,
                display_code,
                old_primary_idx,
                primary_idx,
                &self.extra_cursors,
            );
            *display_code = new_text;
            self.extra_cursors = new_extras;
            shift = Some(primary_shift);
        }

        self.mc_prev_primary_idx = Some(primary_idx);
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
        for &idx in &self.extra_cursors {
            let idx = idx.min(total_chars);
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
        CaretMove, cursor_above, cursor_below, move_extras, replay_edit, toggle_down, toggle_up,
    };

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
        let second = cursor_above(text, primary_idx, &[first]).unwrap(); // on "aaa"
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
        assert_eq!(&text[after_down[0]..after_down[0] + 1], "b"); // on "bbb"
    }

    #[test]
    fn toggle_up_extends_further_after_the_first_add() {
        let text = "aaa\nbbb\nccc\n";
        let primary_idx = text.find("ccc").unwrap();
        let after_1 = toggle_up(text, primary_idx, &[]);
        let after_2 = toggle_up(text, primary_idx, &after_1);
        assert_eq!(after_2.len(), 2, "a second Up press extends further, not undo");
        assert!(after_2.iter().any(|&idx| &text[idx..idx + 1] == "a"));
    }

    #[test]
    fn replay_insert_applies_same_char_at_each_extra_cursor() {
        // Two lines, cursors after the 'a' on each; typing 'X' at the primary.
        // old chars: a(0) \n(1) a(2) \n(3) — extra cursor at index 3 sits
        // right after the second 'a' (before its newline), mirroring the
        // primary's own old position (1, right after the first 'a').
        let old = "a\na\n";
        let new = "aX\na\n"; // typed X after the first 'a'
        let extra = vec![3];
        let (out, positions, shift) = replay_edit(old, &new, 1, 2, &extra);
        assert_eq!(out, "aX\naX\n");
        // new chars: a(0) X(1) \n(2) a(3) X(4) \n(5) — caret sits right after
        // the second X, i.e. index 5.
        assert_eq!(positions, vec![5]);
        // The extra cursor (old idx 3) is AFTER the primary (old idx 1), so it
        // doesn't push the primary's own position around.
        assert_eq!(shift, 0);
    }

    #[test]
    fn replay_backspace_deletes_before_each_extra_cursor() {
        let old = "ab\nab\n";
        // Backspace after the primary's 'b' (index 2, end of first line) → "a\nab\n"
        let new = "a\nab\n";
        let extra = vec![5]; // after the second "ab" (index of trailing \n)
        let (out, positions, shift) = replay_edit(old, &new, 2, 1, &extra);
        assert_eq!(out, "a\na\n");
        assert_eq!(positions, vec![3]); // shifted back by the one deleted char
        assert_eq!(shift, 0); // extra is after the primary here too
    }

    #[test]
    fn replay_delete_key_removes_after_each_extra_cursor() {
        // Delete-key at the START of "ab" removes 'a', cursor stays put.
        let old = "ab\nab\n";
        let new = "b\nab\n";
        let extra = vec![3]; // start of the second "ab" line
        // old_primary_idx == prefix (0) → NOT backspace-like → delete-forward.
        let (out, positions, shift) = replay_edit(old, &new, 0, 0, &extra);
        assert_eq!(out, "b\nb\n");
        assert_eq!(positions, vec![2]);
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
        let (_out, positions, _shift) = replay_edit(old, &new, 1, 5, &[3]);
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
        let extra = vec![0];
        let (out, positions, shift) = replay_edit(old, &new, 3, 4, &extra);
        // The extra's replay inserts 'X' right at the very start (its own old
        // position, 0, is unaffected by the primary's edit further along).
        assert_eq!(out, "Xa\nbX\n");
        assert_eq!(positions, vec![1]); // right after the inserted leading X
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
        assert_eq!(move_extras(text, &[1, 5], CaretMove::Right, 99), vec![2, 6]);
        assert_eq!(move_extras(text, &[1, 5], CaretMove::Left, 99), vec![0, 4]);
        // Clamped: index 0 cannot go left, the last index cannot go right.
        assert_eq!(move_extras(text, &[0], CaretMove::Left, 99), vec![0]);
        assert_eq!(move_extras(text, &[8], CaretMove::Right, 99), vec![8]);
    }

    /// Vertical movement keeps each caret's OWN column — using the primary's
    /// column instead would collapse a staggered set into one line shape.
    #[test]
    fn arrows_move_every_caret_vertically_keeping_its_own_column() {
        //  idx:  0 1 2  3   4 5 6  7   8 9 10 11
        //        a b c  NL  d e f  NL  g h i  NL
        let text = "abc\ndef\nghi\n";
        // Carets at column 1 of line 0 and column 2 of line 1.
        assert_eq!(move_extras(text, &[1, 6], CaretMove::Down, 99), vec![5, 10]);
        assert_eq!(move_extras(text, &[5, 10], CaretMove::Up, 99), vec![1, 6]);
        // No line with text left in that direction → the caret stays put. The
        // trailing empty line after the final newline is blank, so `ghi` is the
        // last line a caret can reach.
        assert_eq!(move_extras(text, &[1], CaretMove::Up, 99), vec![1]);
        assert_eq!(move_extras(text, &[9], CaretMove::Down, 99), vec![9]);
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
        let second = cursor_above(text, primary_idx, &[first]).expect("room above");
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
        assert_eq!(move_extras(text, &[2], CaretMove::Down, 99), vec![8]);
        // And back up again.
        assert_eq!(move_extras(text, &[8], CaretMove::Up, 99), vec![2]);
    }

    /// A caret that lands on the primary, or on another caret, is one caret.
    #[test]
    fn moving_onto_the_primary_or_a_sibling_merges() {
        let text = "abc\ndef\n";
        // Extra at 1 moves right onto the primary at 2 → dropped.
        assert_eq!(move_extras(text, &[1], CaretMove::Right, 2), Vec::<usize>::new());
        // Two carets landing on the same spot collapse to one.
        assert_eq!(move_extras(text, &[0, 0], CaretMove::Right, 99), vec![1]);
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
        let extra = vec![3, 7]; // ends of line 1 and line 2
        let (out, positions, shift) = replay_edit(old, new, 11, 12, &extra);

        assert_eq!(out, "aaaX\nbbbX\ncccX\n");
        //  idx:  0 1 2 3  4   5 6 7 8  9   10 11 12 13  14
        //        a a a X  NL  b b b X  NL   c  c  c  X  NL
        // Each caret belongs right after its own X: 4 and 9.
        assert_eq!(
            positions,
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
        let extra = vec![2, 5]; // ends of line 1 and line 2
        let (out, positions, shift) = replay_edit(old, new, 8, 7, &extra);

        assert_eq!(out, "a\na\na\n");
        //  idx:  0  1   2  3   4  5
        //        a  NL  a  NL  a  NL
        assert_eq!(
            positions,
            vec![1, 3],
            "the upper caret must account for the deletion replayed below it"
        );
        assert_eq!(shift, -2);
    }
}
