//! Rust-aware double-click word selection.
//!
//! egui's own double-click select uses Unicode UAX#29 word segmentation, where
//! `:` between letters is a *MidLetter* — so `radar_delay_gate_val:Option`
//! (unspaced `let name:Type`) counts as ONE word and double-clicking the name
//! selects `radar_delay_gate_val:Option`. (egui special-cases only `.` for
//! `www.example.com`; `:` and `'` have the same problem.) For code we want the
//! plain identifier run — this overrides egui's selection right after the
//! double-click with `[A-Za-z0-9_]*` around the clicked position.

use crate::app::AppIde;
use eframe::egui;

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
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
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::ident_run_at;

    fn run(text: &str, idx: usize) -> Option<(usize, usize)> {
        ident_run_at(&text.chars().collect::<Vec<_>>(), idx)
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
