//! `Ctrl+X` — cut the line(s) where the cursor / selection is: copy them to the
//! clipboard (see `cut_text`) and remove them (see `delete_lines`).

/// The lines spanned by the selection `[sel_lo, sel_hi]` (char indices into
/// `text`), their index range `first..=last`, and whether `text` ended with a
/// structural trailing newline. Shared by `delete_lines` and `cut_text` so the
/// cut clipboard text always matches exactly what gets removed.
fn spanned(text: &str, sel_lo: usize, sel_hi: usize) -> (Vec<String>, usize, usize, bool) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let lo = sel_lo.min(n).min(sel_hi.min(n));
    let hi = sel_lo.min(n).max(sel_hi.min(n));
    // A non-empty selection ending at a line start shouldn't pull in that line.
    let hi_eff = if hi > lo && hi > 0 && chars[hi - 1] == '\n' {
        hi - 1
    } else {
        hi
    };

    // A trailing '\n' is structural; operate on the lines before it.
    let had_trailing = chars.last() == Some(&'\n');
    let body_len = if had_trailing { n - 1 } else { n };

    let mut line_ranges: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for i in 0..body_len {
        if chars[i] == '\n' {
            line_ranges.push((start, i));
            start = i + 1;
        }
    }
    line_ranges.push((start, body_len));

    let line_of = |idx: usize| -> usize {
        line_ranges
            .iter()
            .position(|&(s, e)| idx >= s && idx <= e)
            .unwrap_or(line_ranges.len() - 1)
    };
    let first = line_of(lo);
    let last = line_of(hi_eff).max(first);

    let lines: Vec<String> = line_ranges
        .iter()
        .map(|&(s, e)| chars[s..e].iter().collect())
        .collect();
    (lines, first, last, had_trailing)
}

/// Delete the lines spanned by the selection `[sel_lo, sel_hi]` (char indices
/// into `text`). With no selection it's just the cursor's line. Returns the new
/// text and the collapsed cursor position (start of the line that takes the
/// deleted line's place, or the end of the text when the last line was removed).
/// The returned range is collapsed (`lo == hi`) so it fits the editor's
/// line-operation pipeline.
pub fn delete_lines(text: &str, sel_lo: usize, sel_hi: usize) -> (String, usize, usize) {
    let (lines, first, last, had_trailing) = spanned(text, sel_lo, sel_hi);

    // Keep everything except [first..=last].
    let mut kept: Vec<String> = Vec::with_capacity(lines.len());
    kept.extend_from_slice(&lines[..first]);
    kept.extend_from_slice(&lines[last + 1..]);

    let mut new_text = kept.join("\n");
    if had_trailing && !kept.is_empty() {
        new_text.push('\n');
    }

    // Cursor → start of the line now at `first`, or end of text if we deleted the
    // last line(s).
    let cursor = if first < kept.len() {
        kept[..first].iter().map(|l| l.chars().count() + 1).sum()
    } else {
        new_text.chars().count()
    };
    (new_text, cursor, cursor)
}

/// The text `delete_lines` removes for the same selection — the full line(s)
/// with a trailing newline, so a later paste re-inserts them as whole lines.
/// Makes Ctrl+X a *cut* (to the clipboard) rather than a plain delete.
pub fn cut_text(text: &str, sel_lo: usize, sel_hi: usize) -> String {
    let (lines, first, last, _had_trailing) = spanned(text, sel_lo, sel_hi);
    let mut s = lines[first..=last].join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletes_middle_line() {
        let (out, lo, hi) = delete_lines("a\nb\nc\n", 2, 2); // cursor on "b"
        assert_eq!(out, "a\nc\n");
        assert_eq!((lo, hi), (2, 2)); // start of "c"
    }

    #[test]
    fn deletes_first_line() {
        let (out, lo, _) = delete_lines("a\nb\nc\n", 0, 0);
        assert_eq!(out, "b\nc\n");
        assert_eq!(lo, 0);
    }

    #[test]
    fn deletes_last_line_cursor_to_end() {
        let (out, lo, _) = delete_lines("a\nb\nc\n", 4, 4); // cursor on "c"
        assert_eq!(out, "a\nb\n");
        assert_eq!(lo, "a\nb\n".chars().count());
    }

    #[test]
    fn deletes_a_multi_line_selection() {
        // Select lines "b" and "c".
        let src = "a\nb\nc\nd\n";
        let lo = src.find("b").unwrap();
        let hi = src.find("c").unwrap() + 1;
        let (out, _, _) = delete_lines(src, lo, hi);
        assert_eq!(out, "a\nd\n");
    }

    #[test]
    fn deletes_only_line_leaves_empty() {
        assert_eq!(delete_lines("only", 2, 2).0, "");
        assert_eq!(delete_lines("only\n", 2, 2).0, "");
    }

    #[test]
    fn preserves_no_trailing_newline() {
        let (out, _, _) = delete_lines("a\nb", 0, 0); // delete "a"
        assert_eq!(out, "b");
    }

    #[test]
    fn cut_text_single_line_has_trailing_newline() {
        assert_eq!(cut_text("a\nb\nc\n", 2, 2), "b\n"); // cursor on "b"
        assert_eq!(cut_text("abc", 1, 1), "abc\n"); // no trailing newline in src
    }

    #[test]
    fn cut_text_multi_line_selection() {
        let src = "a\nb\nc\nd\n";
        let lo = src.find("b").unwrap();
        let hi = src.find("c").unwrap() + 1;
        assert_eq!(cut_text(src, lo, hi), "b\nc\n");
    }
}
