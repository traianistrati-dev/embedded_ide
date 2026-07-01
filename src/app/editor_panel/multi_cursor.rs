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
//! Scope: only TEXT EDITS are replayed. Cursor MOVEMENT (arrow keys, mouse
//! clicks) is intentionally NOT synced across carets — each Ctrl+Shift+Up/Down
//! press re-anchors deliberately, which keeps this a text-diff replay instead
//! of a full parallel-input-handling rewrite.

use crate::app::{AppIde, ProjectFileId};
use crate::editor::gui::text_pos::{lsp_cursor_pos, lsp_pos_to_char_idx};
use eframe::egui;
use egui::text_edit::TextEditOutput;

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
    if topmost_line0 == 0 {
        return None; // already at the first line
    }
    // `lsp_pos_to_char_idx` takes a 1-based line; passing `topmost_line0`
    // (the CURRENT 0-based line number) targets the 0-based line right above.
    let new_idx = lsp_pos_to_char_idx(text, topmost_line0, col0 + 1);
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
    // Highest valid 0-based line number: one line per '\n', plus the final
    // (possibly-empty) line after the last one.
    let last_line0 = text.chars().filter(|&c| c == '\n').count() as u32;
    if bottommost_line0 >= last_line0 {
        return None; // already at (or past) the last line
    }
    // `lsp_pos_to_char_idx` takes a 1-based line; passing `bottommost_line0 + 2`
    // (CURRENT 0-based line, +1 for 1-based, +1 for "next line") targets the
    // 0-based line right below.
    let new_idx = lsp_pos_to_char_idx(text, bottommost_line0 + 2, col0 + 1);
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
    let mut new_positions = Vec::with_capacity(extra_cursors.len());
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
        new_positions.push(caret_after);
        if old_idx < old_primary_idx {
            primary_shift += delta;
        }
    }

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
    use super::{cursor_above, cursor_below, replay_edit, toggle_down, toggle_up};

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
}
