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
/// Per-line content ranges `[start, end)` in char indices (the `\n` excluded),
/// plus the first and last line the selection touches.
///
/// Shared by both toggles so they can never disagree about which lines count as
/// selected — in particular the rule that a selection ending exactly at a line
/// start must not pull in the next line.
fn selected_lines(
    chars: &[char],
    sel_lo: usize,
    sel_hi: usize,
) -> (Vec<(usize, usize)>, usize, usize) {
    let n = chars.len();
    let lo = sel_lo.min(n).min(sel_hi.min(n));
    let hi = sel_lo.min(n).max(sel_hi.min(n));
    let hi_eff = if hi > lo && hi > 0 && chars[hi - 1] == '\n' {
        hi - 1
    } else {
        hi
    };

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
    (line_ranges, first, last)
}

pub fn toggle_line_comments(
    text: &str,
    sel_lo: usize,
    sel_hi: usize,
    prefix: &str,
) -> (String, usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let (line_ranges, first, last) = selected_lines(&chars, sel_lo, sel_hi);

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

/// Toggle ONE `/* … */` around the lines the selection touches (`Ctrl+Shift+/`).
///
/// `/* ` goes after the first selected line's indentation and ` */` at the end
/// of the last one, so the line count never changes and the indentation of the
/// block is preserved — a `/*` on a line of its own would shift every line
/// below and re-indent nothing.
///
/// Pressing it again on the same block removes the pair. Blank lines at the
/// edges of the selection are skipped, so selecting a whole function including
/// its trailing empty line still wraps the code rather than the blank.
///
/// Rust block comments NEST, so wrapping a range that already contains
/// `/* … */` is valid and does the obvious thing. The one shape this cannot
/// survive is a `*/` inside a string literal in the range: Rust lexes comments
/// before strings, so that sequence closes the block early — the same trap the
/// language has for a hand-written block comment.
pub fn toggle_block_comment(text: &str, sel_lo: usize, sel_hi: usize) -> (String, usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let (line_ranges, first, last) = selected_lines(&chars, sel_lo, sel_hi);
    let content_of = |(s, e): (usize, usize)| -> String { chars[s..e].iter().collect() };

    // The block is delimited by the first and last NON-BLANK lines: putting the
    // markers on blank lines would look like they wrapped nothing.
    let non_blank: Vec<usize> = (first..=last)
        .filter(|&li| !content_of(line_ranges[li]).trim().is_empty())
        .collect();
    let (Some(&open_li), Some(&close_li)) = (non_blank.first(), non_blank.last()) else {
        // Nothing but blank lines — leave the text alone.
        return (text.to_owned(), sel_lo, sel_hi);
    };

    let open_content = content_of(line_ranges[open_li]);
    let close_content = content_of(line_ranges[close_li]);
    let wrapped =
        open_content.trim_start().starts_with("/*") && close_content.trim_end().ends_with("*/");

    let mut out: Vec<String> = Vec::with_capacity(line_ranges.len());
    for (li, &range) in line_ranges.iter().enumerate() {
        let mut content = content_of(range);
        if li == open_li {
            let indent_len = content.len() - content.trim_start().len();
            let (indent, rest) = content.split_at(indent_len);
            content = if wrapped {
                let rest = rest.strip_prefix("/*").unwrap_or(rest);
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                format!("{indent}{rest}")
            } else {
                format!("{indent}/* {rest}")
            };
        }
        if li == close_li {
            content = if wrapped {
                let c = content.strip_suffix("*/").unwrap_or(&content);
                c.strip_suffix(' ').unwrap_or(c).to_owned()
            } else {
                format!("{content} */")
            };
        }
        out.push(content);
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

    // ── Ctrl+Shift+/ — one `/* … */` around the selected lines ──────────────

    #[test]
    fn wraps_a_single_line_keeping_its_indentation() {
        let (out, _, _) = toggle_block_comment("    let x = 1;", 4, 4);
        assert_eq!(out, "    /* let x = 1; */");
    }

    #[test]
    fn unwraps_the_same_single_line() {
        let (out, _, _) = toggle_block_comment("    /* let x = 1; */", 4, 4);
        assert_eq!(out, "    let x = 1;");
    }

    #[test]
    fn wraps_a_multi_line_selection_and_toggles_back() {
        let src = "fn f() {\n    a;\n    b;\n}\n";
        let lo = src.find("    a;").unwrap();
        let hi = src.find("    b;").unwrap() + "    b;".len();
        let (out, _, _) = toggle_block_comment(src, lo, hi);
        assert_eq!(out, "fn f() {\n    /* a;\n    b; */\n}\n");
        // One pair only — not one per line.
        assert_eq!(out.matches("/*").count(), 1);
        assert_eq!(out.matches("*/").count(), 1);
        let lo2 = out.find("    /* a;").unwrap();
        let hi2 = out.find("    b; */").unwrap() + "    b; */".len();
        let (back, _, _) = toggle_block_comment(&out, lo2, hi2);
        assert_eq!(back, src);
    }

    #[test]
    fn markers_skip_blank_lines_at_the_edges() {
        // Selecting the whole thing must wrap the CODE, not the empty lines.
        let src = "\na;\nb;\n\n";
        let (out, _, _) = toggle_block_comment(src, 0, src.len());
        assert_eq!(out, "\n/* a;\nb; */\n\n");
    }

    #[test]
    fn a_selection_of_only_blank_lines_changes_nothing() {
        let src = "a;\n\n\nb;\n";
        let lo = src.find("\n\n").unwrap() + 1;
        let (out, _, _) = toggle_block_comment(src, lo, lo + 2);
        assert_eq!(out, src);
    }

    #[test]
    fn nested_block_comments_are_allowed() {
        // Rust nests them, so wrapping a range that already holds one is fine.
        let src = "a;\n/* inner */\nb;\n";
        let (out, _, _) = toggle_block_comment(src, 0, src.len());
        assert_eq!(out, "/* a;\n/* inner */\nb; */\n");
    }

    #[test]
    fn a_selection_ending_at_a_line_start_does_not_pull_the_next_line() {
        let src = "a;\nb;\nc;\n";
        // Selection covers "a;\n" exactly.
        let (out, _, _) = toggle_block_comment(src, 0, 3);
        assert_eq!(out, "/* a; */\nb;\nc;\n");
    }

    #[test]
    fn unwrap_only_when_both_ends_are_markers() {
        // A `/*` on the first line but no `*/` on the last → this is not a
        // wrapped block, so the toggle WRAPS instead of stripping.
        let src = "/* a;\nb;\n";
        let (out, _, _) = toggle_block_comment(src, 0, src.len());
        assert_eq!(out, "/* /* a;\nb; */\n");
    }

    #[test]
    fn block_reselect_range_covers_the_block() {
        let src = "a\nb\n";
        let (out, lo, hi) = toggle_block_comment(src, 0, 3);
        assert_eq!(out, "/* a\nb */\n");
        assert_eq!(lo, 0);
        assert_eq!(hi, "/* a\nb */".chars().count());
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
