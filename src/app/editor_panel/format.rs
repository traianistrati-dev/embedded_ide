//! `Ctrl+Shift+F` — re-indent code by brace/bracket nesting depth.
//!
//! A lightweight formatter: it trims each line and re-indents it to the current
//! `{ [ (` nesting depth (4 spaces per level), dedenting lines that start with a
//! closing bracket. Brackets inside strings, char literals, and `//` / `/* */`
//! comments are ignored, and blank lines are preserved. It only fixes
//! indentation — it does not rewrap or re-space tokens.

const INDENT: &str = "    "; // 4 spaces / level

/// Re-indent `text`, returning the formatted text and a cursor position kept on
/// the same line index (at that line's start).
pub fn format_code(text: &str, cursor: usize) -> (String, usize) {
    let formatted = reindent(text);
    let cur = cursor.min(text.chars().count());
    let line_idx = text.chars().take(cur).filter(|&c| c == '\n').count();
    (formatted.clone(), nth_line_start(&formatted, line_idx))
}

/// Char index of the start of line `line_idx` (0-based), clamped to the end.
fn nth_line_start(text: &str, line_idx: usize) -> usize {
    if line_idx == 0 {
        return 0;
    }
    let mut seen = 0;
    let mut count = 0;
    for c in text.chars() {
        count += 1;
        if c == '\n' {
            seen += 1;
            if seen == line_idx {
                return count;
            }
        }
    }
    text.chars().count()
}

fn reindent(text: &str) -> String {
    let mut out = String::new();
    let mut depth: i32 = 0;
    let mut in_block_comment = false;
    let mut first = true;

    for line in text.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;

        // `continuing` = this line is the inside of a `/* … */` block opened on a
        // previous line.
        let continuing = in_block_comment;
        let (leading_closers, net) = analyze_line(line, &mut in_block_comment);

        if continuing {
            // Keep block-comment continuation lines verbatim (preserve the user's
            // internal alignment); don't trim or re-indent them.
            out.push_str(line);
        } else {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let line_depth = (depth - leading_closers as i32).max(0);
                for _ in 0..line_depth {
                    out.push_str(INDENT);
                }
                out.push_str(trimmed);
            }
            // blank line → nothing but the newline already pushed above
        }

        depth = (depth + net).max(0);
    }
    out
}

/// For one line, returns `(leading_closing_brackets, net_bracket_delta)`,
/// ignoring brackets in strings / char literals / comments. `in_block_comment`
/// carries the `/* … */` state across lines.
fn analyze_line(line: &str, in_block_comment: &mut bool) -> (usize, i32) {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut in_string: Option<char> = None; // " or ' (char literal)
    let mut leading = true; // only whitespace + closing brackets seen so far
    let mut leading_closers = 0usize;
    let mut net: i32 = 0;

    while i < n {
        let c = chars[i];

        if *in_block_comment {
            if c == '*' && chars.get(i + 1) == Some(&'/') {
                *in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        if let Some(q) = in_string {
            if c == '\\' {
                i += 2; // skip escaped char
            } else {
                if c == q {
                    in_string = None;
                }
                i += 1;
            }
            continue;
        }

        match c {
            '/' if chars.get(i + 1) == Some(&'/') => break, // line comment → ignore rest
            '/' if chars.get(i + 1) == Some(&'*') => {
                *in_block_comment = true;
                leading = false;
                i += 2;
            }
            '"' => {
                in_string = Some('"');
                leading = false;
                i += 1;
            }
            '\'' => {
                // Distinguish a char literal (`'a'`, `'\n'`) from a lifetime
                // (`'a`): only the former enters string mode.
                let esc_lit =
                    chars.get(i + 1) == Some(&'\\') && chars.get(i + 3) == Some(&'\'');
                let plain_lit = chars.get(i + 1).map_or(false, |x| *x != '\'' && *x != '\\')
                    && chars.get(i + 2) == Some(&'\'');
                if esc_lit || plain_lit {
                    in_string = Some('\'');
                }
                leading = false;
                i += 1;
            }
            '{' | '[' | '(' => {
                net += 1;
                leading = false;
                i += 1;
            }
            '}' | ']' | ')' => {
                net -= 1;
                if leading {
                    leading_closers += 1;
                }
                i += 1;
            }
            c if c.is_whitespace() => i += 1, // stay in the leading region
            _ => {
                leading = false;
                i += 1;
            }
        }
    }

    (leading_closers, net)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(s: &str) -> String {
        reindent(s)
    }

    #[test]
    fn indents_nested_blocks() {
        let src = "fn f() {\nlet x = {\nlet y = 1;\n}\n}\n";
        let expected = "fn f() {\n    let x = {\n        let y = 1;\n    }\n}\n";
        assert_eq!(fmt(src), expected);
    }

    #[test]
    fn fixes_wrong_indentation() {
        let src = "fn f(){\n   let a = 1;\n       let b = 2;\n}";
        let expected = "fn f(){\n    let a = 1;\n    let b = 2;\n}";
        assert_eq!(fmt(src), expected);
    }

    #[test]
    fn dedents_closing_brackets() {
        let src = "if x {\nfoo();\n} else {\nbar();\n}";
        let expected = "if x {\n    foo();\n} else {\n    bar();\n}";
        assert_eq!(fmt(src), expected);
    }

    #[test]
    fn preserves_blank_lines() {
        let src = "fn f() {\na;\n\nb;\n}";
        let expected = "fn f() {\n    a;\n\n    b;\n}";
        assert_eq!(fmt(src), expected);
    }

    #[test]
    fn ignores_braces_in_strings_and_comments() {
        let src = "fn f() {\nlet s = \"a { b }\";\n// a } brace } here\ng();\n}";
        let expected =
            "fn f() {\n    let s = \"a { b }\";\n    // a } brace } here\n    g();\n}";
        assert_eq!(fmt(src), expected);
    }

    #[test]
    fn lifetime_is_not_a_string() {
        // The `'a` lifetime must not swallow the following braces.
        let src = "impl<'a> T<'a> {\nfn g(&'a self) {\nx;\n}\n}";
        let expected = "impl<'a> T<'a> {\n    fn g(&'a self) {\n        x;\n    }\n}";
        assert_eq!(fmt(src), expected);
    }

    #[test]
    fn multi_line_block_comment() {
        let src = "fn f() {\n/* line1\nline2 */\na;\n}";
        let expected = "fn f() {\n    /* line1\nline2 */\n    a;\n}";
        assert_eq!(fmt(src), expected);
    }

    #[test]
    fn cursor_stays_on_same_line() {
        let src = "fn f() {\nlet x = 1;\n}";
        // Cursor on line 1 ("let x = 1;").
        let cursor = src.find("let").unwrap();
        let (out, new_cursor) = format_code(src, cursor);
        // New cursor is at the start of line 1 in the formatted text.
        let line1_start = out.find("    let").unwrap();
        assert_eq!(new_cursor, line1_start);
    }
}
