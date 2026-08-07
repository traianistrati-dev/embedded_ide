//! `Ctrl+D` — duplicate the line(s) where the cursor / selection is.

/// Duplicate the lines spanned by the selection `[sel_lo, sel_hi]` (char indices
/// into `text`), inserting a copy directly below. With no selection it's just
/// the cursor's line. The returned selection is the original one shifted onto
/// the new copy, so the caret keeps its column and repeated Ctrl+D keeps
/// duplicating downward. Fits the editor's line-operation pipeline.
pub fn duplicate_lines(text: &str, sel_lo: usize, sel_hi: usize) -> (String, usize, usize) {
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

    // A trailing '\n' is structural; operate on the lines before it and re-add.
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

    let block = &lines[first..=last];
    // Char length of the duplicated block joined with '\n'.
    let block_len: usize = block.iter().map(|l| l.chars().count()).sum::<usize>() + (last - first);

    // lines[..=last] + (copy of the block) + lines[last+1..]
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + (last - first + 1));
    out.extend_from_slice(&lines[..=last]);
    out.extend_from_slice(block);
    out.extend_from_slice(&lines[last + 1..]);

    let mut new_text = out.join("\n");
    if had_trailing {
        new_text.push('\n');
    }

    // Shift the original selection onto the inserted copy: the block's chars plus
    // the '\n' separating the original block from its duplicate.
    let shift = block_len + 1;
    (new_text, lo + shift, hi + shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_middle_line() {
        // cursor on "b" (index 2)
        let (out, lo, hi) = duplicate_lines("a\nb\nc\n", 2, 2);
        assert_eq!(out, "a\nb\nb\nc\n");
        // caret moved onto the duplicate (start of the 2nd "b")
        assert_eq!((lo, hi), (4, 4));
    }

    #[test]
    fn duplicates_first_line() {
        let (out, lo, _) = duplicate_lines("a\nb\nc\n", 0, 0);
        assert_eq!(out, "a\na\nb\nc\n");
        assert_eq!(lo, 2); // start of the duplicated "a"
    }

    #[test]
    fn duplicates_last_line() {
        let (out, _, _) = duplicate_lines("a\nb\nc\n", 4, 4); // cursor on "c"
        assert_eq!(out, "a\nb\nc\nc\n");
    }

    #[test]
    fn keeps_caret_column_on_duplicate() {
        // cursor at column 1 of "def"
        let (out, lo, hi) = duplicate_lines("abc\ndef\n", 5, 5);
        assert_eq!(out, "abc\ndef\ndef\n");
        // index 9 = 'e' of the 2nd "def"
        assert_eq!((lo, hi), (9, 9));
    }

    #[test]
    fn duplicates_multi_line_selection() {
        let src = "a\nb\nc\nd\n";
        let lo = src.find('b').unwrap(); // 2
        let hi = src.find('c').unwrap() + 1; // 5 (covers b and c)
        let (out, _, _) = duplicate_lines(src, lo, hi);
        assert_eq!(out, "a\nb\nc\nb\nc\nd\n");
    }

    #[test]
    fn preserves_no_trailing_newline() {
        let (out, _, _) = duplicate_lines("abc", 1, 1);
        assert_eq!(out, "abc\nabc");
    }

    #[test]
    fn duplicates_only_line() {
        let (out, lo, _) = duplicate_lines("only\n", 2, 2);
        assert_eq!(out, "only\nonly\n");
        assert_eq!(lo, 7); // 2 + ("only".len() + 1)
    }
}
