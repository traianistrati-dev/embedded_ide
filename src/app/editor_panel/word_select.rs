//! Rust-aware word selection: double-click and Ctrl(+Shift)+Left/Right.
//!
//! egui segments words with Unicode UAX#29, where `:` between letters is a
//! *MidLetter* — so `radar_delay_gate_val:Option` (unspaced `let name:Type`)
//! counts as ONE word. Double-clicking the name selected the whole thing, and
//! Ctrl+Right jumped clean over the `:` (egui special-cases only `.`, for
//! `www.example.com`; `:` and `'` have the same problem). Code wants plain
//! identifier runs, so both paths are overridden here:
//!
//! - double-click → the `[A-Za-z0-9_]*` run around the clicked position;
//! - Ctrl+Left / Ctrl+Right (+Shift to select) → class-based word movement
//!   ([`next_word_boundary`] / [`prev_word_boundary`]), where identifiers,
//!   punctuation and spaces are separate classes — so a jump stops AT the `:`.

use crate::app::AppIde;
use eframe::egui;

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Char class for word-wise movement. Runs of one class are consumed whole, so
/// `:` (Punct) ends the identifier before it instead of joining the two.
#[derive(Clone, Copy, PartialEq)]
enum Class {
    Ident,
    Punct,
    /// Space / tab — skipped before the run being consumed.
    Space,
    /// Hard stop: a word jump never silently crosses lines.
    Newline,
}

fn class_of(c: char) -> Class {
    if c == '\n' {
        Class::Newline
    } else if is_ident(c) {
        Class::Ident
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Punct
    }
}

/// Caret target one word to the RIGHT of `i` (Ctrl+Right): skip spaces, then
/// consume the whole run under the caret. `name:Type` therefore stops at the
/// `:`, and `::`/`>>` are consumed as single punctuation runs.
pub(super) fn next_word_boundary(chars: &[char], i: usize) -> usize {
    let n = chars.len();
    let mut j = i.min(n);
    if j >= n {
        return n;
    }
    // Sitting on the line break: step over exactly it.
    if chars[j] == '\n' {
        return j + 1;
    }
    while j < n && class_of(chars[j]) == Class::Space {
        j += 1;
    }
    if j >= n || chars[j] == '\n' {
        return j; // stop at the line end rather than jumping into the next line
    }
    let class = class_of(chars[j]);
    while j < n && class_of(chars[j]) == class {
        j += 1;
    }
    j
}

/// Caret target one word to the LEFT of `i` (Ctrl+Left) — mirror of
/// [`next_word_boundary`], looking at the char before the caret.
pub(super) fn prev_word_boundary(chars: &[char], i: usize) -> usize {
    let mut j = i.min(chars.len());
    if j == 0 {
        return 0;
    }
    if chars[j - 1] == '\n' {
        return j - 1;
    }
    while j > 0 && class_of(chars[j - 1]) == Class::Space {
        j -= 1;
    }
    if j == 0 || chars[j - 1] == '\n' {
        return j;
    }
    let class = class_of(chars[j - 1]);
    while j > 0 && class_of(chars[j - 1]) == class {
        j -= 1;
    }
    j
}

/// The identifier run `[start, end)` around char index `idx`: the char at the
/// index, or — clicking just past a word's last char — the one before it.
/// `None` when neither side is an identifier char (punctuation / whitespace) —
/// the caller then leaves egui's own selection alone.
pub(super) fn ident_run_at(chars: &[char], idx: usize) -> Option<(usize, usize)> {
    let anchor = if idx < chars.len() && is_ident(chars[idx]) {
        idx
    } else if idx > 0 && chars.get(idx - 1).is_some_and(|&c| is_ident(c)) {
        idx - 1
    } else {
        return None;
    };
    let mut start = anchor;
    while start > 0 && is_ident(chars[start - 1]) {
        start -= 1;
    }
    let mut end = anchor + 1;
    while end < chars.len() && is_ident(chars[end]) {
        end += 1;
    }
    Some((start, end))
}

impl AppIde {
    /// On a double-click, replace egui's UAX#29 word selection with the plain
    /// identifier run under the pointer (stored for next frame — the widget
    /// already rendered with egui's selection this frame).
    pub(super) fn fix_double_click_selection(
        &self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        display_code: &str,
    ) {
        if !editor_resp.response.double_clicked() {
            return;
        }
        let Some(pos) = ui.input(|i| i.pointer.interact_pos()) else {
            return;
        };
        let ccursor = editor_resp
            .galley
            .cursor_from_pos(pos - editor_resp.galley_pos);
        let chars: Vec<char> = display_code.chars().collect();
        let Some((start, end)) = ident_run_at(&chars, ccursor.index.min(chars.len())) else {
            return; // not on an identifier — keep egui's selection
        };
        let mut st = editor_resp.state.clone();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(start),
            egui::text::CCursor::new(end),
        )));
        st.store(ui.ctx(), editor_resp.response.id);
    }

    /// Apply a Ctrl(+Shift)+Left/Right word move that was consumed before the
    /// editor (so egui's UAX#29 jump never ran). `extend` keeps the anchor —
    /// that's the Shift variant, which grows/shrinks the selection.
    ///
    /// Like the double-click fix, the new cursor is stored for NEXT frame: the
    /// widget already rendered with the old one. The key press repaints anyway,
    /// so the move lands immediately.
    pub(super) fn apply_word_move(
        &self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        display_code: &str,
        right: bool,
        extend: bool,
    ) {
        let Some(range) = editor_resp.state.cursor.char_range() else {
            return; // editor not focused / no caret yet
        };
        let chars: Vec<char> = display_code.chars().collect();
        let cur = range.primary.index.min(chars.len());
        let target = if right {
            next_word_boundary(&chars, cur)
        } else {
            prev_word_boundary(&chars, cur)
        };
        let primary = egui::text::CCursor::new(target);
        // `CCursorRange::two(min, max)` assigns by position, but here the moved
        // end must stay `primary` (Shift+arrow grows FROM the anchor either
        // way), so the range is built field-wise.
        let new = if extend {
            egui::text::CCursorRange {
                primary,
                secondary: range.secondary,
                h_pos: None,
            }
        } else {
            egui::text::CCursorRange::one(primary)
        };
        let mut st = editor_resp.state.clone();
        st.cursor.set_char_range(Some(new));
        st.store(ui.ctx(), editor_resp.response.id);
        ui.ctx().request_repaint();
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{ident_run_at, next_word_boundary, prev_word_boundary};

    fn run(text: &str, idx: usize) -> Option<(usize, usize)> {
        ident_run_at(&text.chars().collect::<Vec<_>>(), idx)
    }

    /// Walk `steps` word jumps from `from` and return the selected substring —
    /// what Ctrl+Shift+Right/Left would highlight after that many presses.
    fn walk(text: &str, from: usize, steps: usize, right: bool) -> &str {
        let chars: Vec<char> = text.chars().collect();
        let mut at = from;
        for _ in 0..steps {
            at = if right {
                next_word_boundary(&chars, at)
            } else {
                prev_word_boundary(&chars, at)
            };
        }
        let (a, b) = if right { (from, at) } else { (at, from) };
        &text[a..b] // ASCII fixtures: char index == byte index
    }

    /// The reported case: `:` must END the word, so a jump never swallows both
    /// the binding name and its type.
    #[test]
    fn word_jump_stops_at_the_type_colon() {
        let src = "radar_delay_gate_val:Option< u32 >";
        // Left → right: the first press selects only the name.
        assert_eq!(walk(src, 0, 1, true), "radar_delay_gate_val");
        // The next press takes the `:` alone, then `Option`.
        assert_eq!(walk(src, 0, 2, true), "radar_delay_gate_val:");
        assert_eq!(walk(src, 0, 3, true), "radar_delay_gate_val:Option");
        // Right → left from the end: `>`, ` u32`, ` <`, `Option` — stopping at
        // the colon, so the type comes out whole and the name is untouched.
        let end = src.len();
        assert_eq!(walk(src, end, 1, false), ">");
        assert_eq!(walk(src, end, 2, false), "u32 >");
        assert_eq!(walk(src, end, 3, false), "< u32 >");
        assert_eq!(walk(src, end, 4, false), "Option< u32 >");
        // Only the 5th press reaches the `:` — never together with the name.
        assert_eq!(walk(src, end, 5, false), ":Option< u32 >");
    }

    /// Punctuation runs (`::`, `>>`) are consumed as one word, identifiers stop
    /// at any separator.
    #[test]
    fn punctuation_runs_and_paths() {
        let src = "pins::configs::usart1";
        assert_eq!(walk(src, 0, 1, true), "pins");
        assert_eq!(walk(src, 0, 2, true), "pins::");
        assert_eq!(walk(src, 0, 3, true), "pins::configs");
        let src = "Vec<Vec<u32>>";
        let end = src.len();
        assert_eq!(walk(src, end, 1, false), ">>");
        assert_eq!(walk(src, end, 2, false), "u32>>");
    }

    /// Spaces are skipped before the run; a line break is a hard stop, so a
    /// jump never silently crosses into the next line.
    #[test]
    fn spaces_are_skipped_and_newlines_stop_the_jump() {
        let src = "let  x = 1\nlet y";
        assert_eq!(walk(src, 0, 1, true), "let");
        assert_eq!(walk(src, 0, 2, true), "let  x"); // double space skipped
        // From just before the newline: the jump stops ON it, then steps over.
        let nl = src.find('\n').unwrap();
        assert_eq!(walk(src, nl, 1, true), "\n");
        assert_eq!(walk(src, nl, 2, true), "\nlet");
        // Backwards from the start of line 2 lands at the line end, not inside
        // line 1's last word.
        assert_eq!(walk(src, nl + 1, 1, false), "\n");
    }

    #[test]
    fn word_jump_clamps_at_the_buffer_edges() {
        let chars: Vec<char> = "ab".chars().collect();
        assert_eq!(next_word_boundary(&chars, 2), 2);
        assert_eq!(next_word_boundary(&chars, 9), 2); // out-of-range index
        assert_eq!(prev_word_boundary(&chars, 0), 0);
        assert_eq!(prev_word_boundary(&[], 0), 0);
    }

    /// The reported case: unspaced `let name:Type` — clicking the name selects
    /// ONLY the name, clicking the type selects only the type.
    #[test]
    fn unspaced_type_annotation_selects_name_only() {
        let src = "let radar_delay_gate_val:Option<u32>";
        let name_start = 4;
        let name_end = src.find(':').unwrap();
        // Click in the middle of the name.
        assert_eq!(run(src, 10), Some((name_start, name_end)));
        // Click inside `Option` → just `Option`.
        let opt_start = name_end + 1;
        let opt_end = src.find('<').unwrap();
        assert_eq!(run(src, opt_start + 2), Some((opt_start, opt_end)));
    }

    #[test]
    fn click_just_past_last_char_still_hits_the_word() {
        let src = "foo bar";
        assert_eq!(run(src, 3), Some((0, 3))); // caret right after `foo`
    }

    #[test]
    fn word_boundaries_at_line_edges() {
        assert_eq!(run("foo", 0), Some((0, 3)));
        assert_eq!(run("foo", 3), Some((0, 3)));
    }

    #[test]
    fn punctuation_and_whitespace_yield_none() {
        let src = "a + b";
        assert_eq!(run(src, 2), None); // on `+`
        // On the space after `a` — the char before is an identifier, so the
        // click still resolves to `a` (matches editors' forgiving behaviour).
        assert_eq!(run(src, 1), Some((0, 1)));
        assert_eq!(run("  ", 1), None);
        assert_eq!(run("", 0), None);
    }

    #[test]
    fn underscores_and_digits_are_part_of_the_word() {
        let src = "count_2ab = 7";
        assert_eq!(run(src, 5), Some((0, 9)));
    }
}
