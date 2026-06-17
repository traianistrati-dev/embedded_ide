//! `Ctrl+/` line-comment toggling for the code editor.

/// Toggle line comments on the lines spanned by the selection `[sel_lo, sel_hi]`
/// (char indices into `text`), using `prefix` (`"//"` for Rust, `"#"` for TOML /
/// .gitignore).
///
/// VS Code-style: if **every** non-blank line in the range is already commented,
/// they are un-commented; otherwise each non-blank line gets `prefix` inserted
/// after its indentation. Blank lines are left untouched. Returns the new text
/// plus the char range to re-select — the whole affected block, so pressing
/// `Ctrl+/` again keeps toggling the same lines.
pub fn toggle_line_comments(
    text: &str,
    sel_lo: usize,
    sel_hi: usize,
    prefix: &str,
) -> (String, usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let lo = sel_lo.min(n).min(sel_hi.min(n));
    let hi = sel_lo.min(n).max(sel_hi.min(n));

    // A non-empty selection ending exactly at a line start shouldn't pull in the
    // next line.
    let hi_eff = if hi > lo && hi > 0 && chars[hi - 1] == '\n' { hi - 1 } else { hi };

    // Per-line content ranges [start, end) in char indices ('\n' excluded).
    let mut line_ranges: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            line_ranges.push((start, i));
            start = i + 1;
        }
    }
    line_ranges.push((start, n));

    let line_of = |idx: usize| -> usize {
        line_ranges
            .iter()
            .position(|&(s, e)| idx >= s && idx <= e)
            .unwrap_or(line_ranges.len() - 1)
    };
    let first = line_of(lo);
    let last = line_of(hi_eff).max(first);

    let content_of = |(s, e): (usize, usize)| -> String { chars[s..e].iter().collect() };

    // Un-comment only when every non-blank line in the range is already commented.
    let mut any_nonblank = false;
    let mut all_commented = true;
    for &range in &line_ranges[first..=last] {
        let c = content_of(range);
        if c.trim().is_empty() {
            continue;
        }
        any_nonblank = true;
        if !c.trim_start().starts_with(prefix) {
            all_commented = false;
        }
    }
    let uncomment = any_nonblank && all_commented;

    let mut out: Vec<String> = Vec::with_capacity(line_ranges.len());
    for (li, &range) in line_ranges.iter().enumerate() {
        let content = content_of(range);
        if li < first || li > last || content.trim().is_empty() {
            out.push(content);
            continue;
        }
        let indent_len = content.len() - content.trim_start().len();
        let (indent, rest) = content.split_at(indent_len);
        if uncomment {
            let after = &rest[prefix.len()..];
            let after = after.strip_prefix(' ').unwrap_or(after);
            out.push(format!("{indent}{after}"));
        } else {
            out.push(format!("{indent}{prefix} {rest}"));
        }
    }

    let new_text = out.join("\n");
    let new_lo = out[..first].iter().map(|l| l.chars().count() + 1).sum();
    let block_len: usize = out[first..=last].join("\n").chars().count();
    (new_text, new_lo, new_lo + block_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_a_single_line() {
        let (out, _, _) = toggle_line_comments("    let x = 1;", 4, 4, "//");
        assert_eq!(out, "    // let x = 1;");
    }

    #[test]
    fn uncomments_a_single_line() {
        let (out, _, _) = toggle_line_comments("    // let x = 1;", 4, 4, "//");
        assert_eq!(out, "    let x = 1;");
    }

    #[test]
    fn toggles_a_multi_line_block_and_keeps_structure() {
        let src = "fn f() {\n    a;\n    b;\n}\n";
        // Selection spanning lines 2-3 (the two indented statements).
        let lo = src.find("    a;").unwrap();
        let hi = src.find("    b;").unwrap() + "    b;".len();
        let (out, _, _) = toggle_line_comments(src, lo, hi, "//");
        assert_eq!(out, "fn f() {\n    // a;\n    // b;\n}\n");
        // Toggling again un-comments back to the original.
        let lo2 = out.find("    // a;").unwrap();
        let hi2 = out.find("    // b;").unwrap() + "    // b;".len();
        let (back, _, _) = toggle_line_comments(&out, lo2, hi2, "//");
        assert_eq!(back, src);
    }

    #[test]
    fn mixed_block_comments_all_when_not_all_commented() {
        // One line commented, one not → toggle COMMENTS both (not uncomment).
        let src = "// a\nb\n";
        let (out, _, _) = toggle_line_comments(src, 0, src.len(), "//");
        assert_eq!(out, "// // a\n// b\n");
    }

    #[test]
    fn toml_uses_hash_prefix() {
        let (out, _, _) = toggle_line_comments("name = \"x\"", 0, 0, "#");
        assert_eq!(out, "# name = \"x\"");
        let (back, _, _) = toggle_line_comments("# name = \"x\"", 0, 0, "#");
        assert_eq!(back, "name = \"x\"");
    }

    #[test]
    fn blank_lines_are_left_untouched() {
        let src = "a\n\nb";
        let (out, _, _) = toggle_line_comments(src, 0, src.len(), "//");
        assert_eq!(out, "// a\n\n// b");
    }

    #[test]
    fn reselect_range_covers_the_block() {
        let src = "a\nb\n";
        let (out, lo, hi) = toggle_line_comments(src, 0, 3, "//");
        // Re-selection spans exactly the two commented lines.
        assert_eq!(&out[..], "// a\n// b\n");
        assert_eq!(lo, 0);
        assert_eq!(hi, "// a\n// b".chars().count());
    }
}
