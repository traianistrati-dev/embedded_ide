//! `Ctrl+Up` / `Ctrl+Down` — move the selected lines up / down in the editor.

/// Move the lines spanned by the selection `[sel_lo, sel_hi]` (char indices into
/// `text`) one row `down` (or up when `down == false`), swapping them with the
/// adjacent line. Returns the new text and the char range to re-select (the
/// moved block, so repeated presses keep moving it). A no-op at the top/bottom
/// returns the input unchanged.
pub fn move_lines(text: &str, sel_lo: usize, sel_hi: usize, down: bool) -> (String, usize, usize) {
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

    // A trailing '\n' is structural, not a real (movable) empty line — keep it
    // pinned to the end and operate on the lines before it.
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
    let num = line_ranges.len();

    let lines: Vec<String> = line_ranges
        .iter()
        .map(|&(s, e)| chars[s..e].iter().collect())
        .collect();

    // Bail at the edges.
    if (down && last + 1 >= num) || (!down && first == 0) {
        return (text.to_owned(), sel_lo, sel_hi);
    }

    let mut new_lines: Vec<String> = Vec::with_capacity(num);
    let (new_first, new_last);
    if down {
        new_lines.extend_from_slice(&lines[..first]);
        new_lines.push(lines[last + 1].clone());
        new_lines.extend_from_slice(&lines[first..=last]);
        new_lines.extend_from_slice(&lines[last + 2..]);
        new_first = first + 1;
        new_last = last + 1;
    } else {
        new_lines.extend_from_slice(&lines[..first - 1]);
        new_lines.extend_from_slice(&lines[first..=last]);
        new_lines.push(lines[first - 1].clone());
        new_lines.extend_from_slice(&lines[last + 1..]);
        new_first = first - 1;
        new_last = last - 1;
    }

    let mut new_text = new_lines.join("\n");
    if had_trailing {
        new_text.push('\n');
    }

    let new_lo: usize = new_lines[..new_first]
        .iter()
        .map(|l| l.chars().count() + 1)
        .sum();
    let block_len: usize = new_lines[new_first..=new_last].join("\n").chars().count();
    (new_text, new_lo, new_lo + block_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_single_line_down() {
        let (out, _, _) = move_lines("a\nb\nc\n", 0, 0, true);
        assert_eq!(out, "b\na\nc\n");
    }

    #[test]
    fn moves_single_line_up() {
        // Cursor on line "b" (index 2..3).
        let (out, _, _) = move_lines("a\nb\nc\n", 2, 2, false);
        assert_eq!(out, "b\na\nc\n");
    }

    #[test]
    fn moves_a_multi_line_block_down() {
        let src = "1\n2\n3\n4\n";
        // Select lines "1" and "2".
        let (out, lo, hi) = move_lines(src, 0, 3, true);
        assert_eq!(out, "3\n1\n2\n4\n");
        // Re-selection follows the moved block ("1\n2" now on lines 2-3).
        assert_eq!(&out[lo..hi], "1\n2");
    }

    #[test]
    fn moves_a_multi_line_block_up() {
        let src = "1\n2\n3\n4\n";
        // Select lines "3" and "4" (chars 4..7).
        let (out, lo, hi) = move_lines(src, 4, 7, false);
        assert_eq!(out, "1\n3\n4\n2\n");
        assert_eq!(&out[lo..hi], "3\n4");
    }

    #[test]
    fn no_op_at_top() {
        let (out, lo, hi) = move_lines("a\nb\n", 0, 0, false);
        assert_eq!(out, "a\nb\n");
        assert_eq!((lo, hi), (0, 0));
    }

    #[test]
    fn no_op_at_bottom() {
        // Last real line is "b" (chars 2..3); cannot move it down.
        let (out, _, _) = move_lines("a\nb\n", 2, 3, true);
        assert_eq!(out, "a\nb\n");
    }

    #[test]
    fn preserves_no_trailing_newline() {
        let (out, _, _) = move_lines("a\nb", 0, 0, true);
        assert_eq!(out, "b\na");
    }

    #[test]
    fn move_then_move_back_restores() {
        let src = "x\ny\nz\n";
        let (down, _, _) = move_lines(src, 0, 0, true); // x down → "y\nx\nz\n"
        assert_eq!(down, "y\nx\nz\n");
        // x is now on line 2 (chars 2..3); move it back up.
        let (back, _, _) = move_lines(&down, 2, 3, false);
        assert_eq!(back, src);
    }
}
