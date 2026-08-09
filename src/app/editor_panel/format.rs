//! `Shift+Alt+F` — re-indent code by brace/bracket nesting depth, and (for Rust
//! files) normalise token spacing.
//!
//! A lightweight formatter. Two passes over the same single-pass tokenizer:
//!
//! * **indent** — each line is trimmed and re-indented to the current `{ [ (`
//!   nesting depth (4 spaces per level), dedenting lines that start with a
//!   closing bracket. Blank lines are preserved.
//! * **re-space** (Rust only, see [`format_code_opts`]) — collapses runs of
//!   spaces to one and normalises `, ; :` and `( [ ] )` spacing.
//!
//! Brackets and tokens inside strings (including raw/byte strings), char
//! literals and `//` / `/* */` comments are never touched; those regions are
//! copied out verbatim, and a string or block comment left open carries over to
//! the next line, which is then emitted untouched.

const INDENT: &str = "    "; // 4 spaces / level

/// Re-indent `text`, returning the formatted text and a cursor position kept on
/// the same line index (at that line's start).
///
/// `respace` additionally normalises token spacing. Only pass `true` for Rust
/// source — the rules (`:` → `: `, `,` → `, `) are Rust syntax and would rewrite
/// unrelated files (linker scripts, `.ron`, Markdown tables…).
pub fn format_code(text: &str, cursor: usize, respace: bool) -> (String, usize) {
    let formatted = reindent_opts(text, respace);
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

fn reindent_opts(text: &str, respace: bool) -> String {
    let mut out = String::new();
    let mut depth: i32 = 0;
    let mut st = ScanState::default();
    let mut first = true;

    for line in text.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;

        // `continuing` = this line is the inside of a `/* … */` block comment or
        // of a multi-line string opened on a previous line.
        let continuing = st.open();
        let info = scan_line(line, &mut st, respace && !continuing);

        if continuing {
            // Keep continuation lines verbatim: re-indenting them would shift a
            // block comment's internal alignment, and re-spacing them would
            // rewrite the *contents* of a string literal.
            out.push_str(line);
        } else {
            let trimmed = if respace {
                info.text.trim()
            } else {
                line.trim()
            };
            if !trimmed.is_empty() {
                let line_depth = (depth - info.leading_closers as i32).max(0);
                for _ in 0..line_depth {
                    out.push_str(INDENT);
                }
                out.push_str(trimmed);
            }
            // blank line → nothing but the newline already pushed above
        }

        depth = (depth + info.net).max(0);
    }
    out
}

// ── Tokenizer ─────────────────────────────────────────────────────────────────

/// A string literal left open at the end of a line.
#[derive(Clone, Copy, PartialEq)]
enum StrKind {
    /// `"…"` (or a `'…'` char literal, which never spans lines).
    Quoted(char),
    /// `r#"…"#` / `br##"…"##` — closed by `"` followed by exactly N hashes.
    Raw(usize),
}

/// Lexer state that has to survive across lines.
#[derive(Clone, Copy, Default)]
struct ScanState {
    in_block_comment: bool,
    string: Option<StrKind>,
}

impl ScanState {
    /// Is a multi-line construct still open?
    fn open(&self) -> bool {
        self.in_block_comment || self.string.is_some()
    }
}

struct LineInfo {
    /// Closing brackets before any other token — they dedent their own line.
    leading_closers: usize,
    /// Net `{ [ (` minus `} ] )` on this line.
    net: i32,
    /// The re-spaced line (empty unless re-spacing was requested).
    text: String,
}

/// Tokens after which a `, ; :` needs no separating space — the space would
/// only pad a closing bracket (`foo(a, b,)`, `[u8; 4]`-style trailing commas).
const NO_SPACE_BEFORE: [char; 4] = [')', ']', '}', '>'];

/// The next non-space char at or after `i`.
fn next_non_space(chars: &[char], mut i: usize) -> Option<char> {
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    chars.get(i).copied()
}

/// Does a raw-string literal start at `i` (`r"`, `r#"`, `br##"`, …)? Returns the
/// hash count and the index of the opening quote.
fn raw_string_at(chars: &[char], i: usize) -> Option<(usize, usize)> {
    // Must be a token start, or it's just the tail of an identifier (`for"`…).
    if i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
        return None;
    }
    let mut j = i;
    if chars.get(j) == Some(&'b') {
        j += 1; // byte string: `br"…"`
    }
    if chars.get(j) != Some(&'r') {
        return None;
    }
    j += 1;
    let start_hashes = j;
    while chars.get(j) == Some(&'#') {
        j += 1;
    }
    if chars.get(j) == Some(&'"') {
        Some((j - start_hashes, j))
    } else {
        None
    }
}

/// Scan one line: count brackets, and — when `respace` — rebuild it with
/// normalised spacing. `st` carries the block-comment / open-string state in and
/// out.
// `sep!` resets both space flags on every expansion; at the last token of a line
// nothing reads them back, which is the shape `unused_assignments` flags.
#[allow(unused_assignments)]
fn scan_line(line: &str, st: &mut ScanState, respace: bool) -> LineInfo {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut leading = true; // only whitespace + closing brackets seen so far
    let mut leading_closers = 0usize;
    let mut net: i32 = 0;

    let mut out = String::new();
    // Whitespace has been seen and collapses to at most one space, emitted only
    // once the next token proves it needs separating.
    let mut pending_space = false;
    // Set by `(` / `[`: the next token hugs the bracket.
    let mut suppress_space = false;

    // Emit the pending separator (if any) before a token.
    macro_rules! sep {
        () => {
            if respace {
                if pending_space && !suppress_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                suppress_space = false;
            }
        };
    }
    macro_rules! emit {
        ($c:expr) => {
            if respace {
                out.push($c);
            }
        };
    }

    while i < n {
        let c = chars[i];

        // ── Inside a block comment / string: copy verbatim to its terminator ──
        if st.in_block_comment {
            if c == '*' && chars.get(i + 1) == Some(&'/') {
                st.in_block_comment = false;
                emit!('*');
                emit!('/');
                i += 2;
            } else {
                emit!(c);
                i += 1;
            }
            continue;
        }

        if let Some(kind) = st.string {
            match kind {
                StrKind::Quoted(q) => {
                    if c == '\\' {
                        emit!(c);
                        if let Some(&e) = chars.get(i + 1) {
                            emit!(e);
                        }
                        i += 2; // skip escaped char
                    } else {
                        emit!(c);
                        if c == q {
                            st.string = None;
                        }
                        i += 1;
                    }
                }
                StrKind::Raw(hashes) => {
                    // No escapes in a raw string — it ends at `"` + N hashes.
                    let closes = c == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#'));
                    emit!(c);
                    if closes {
                        for k in 1..=hashes {
                            if let Some(&h) = chars.get(i + k) {
                                emit!(h);
                            }
                        }
                        st.string = None;
                        i += 1 + hashes;
                    } else {
                        i += 1;
                    }
                }
            }
            continue;
        }

        // ── Code ─────────────────────────────────────────────────────────────
        if c.is_whitespace() {
            if !out.is_empty() {
                pending_space = true;
            }
            i += 1;
            continue; // stay in the leading region
        }

        // Line comment: the rest of the line is copied as-is.
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            sep!();
            if respace {
                out.extend(&chars[i..]);
            }
            break;
        }

        if c == '/' && chars.get(i + 1) == Some(&'*') {
            sep!();
            emit!('/');
            emit!('*');
            st.in_block_comment = true;
            leading = false;
            i += 2;
            continue;
        }

        // Raw / byte-raw string — checked before the plain-token branch so the
        // leading `r` / `br` isn't mistaken for an identifier.
        if (c == 'r' || c == 'b')
            && let Some((hashes, quote)) = raw_string_at(&chars, i)
        {
            sep!();
            if respace {
                out.extend(&chars[i..=quote]);
            }
            st.string = Some(StrKind::Raw(hashes));
            leading = false;
            i = quote + 1;
            continue;
        }

        match c {
            '"' => {
                sep!();
                emit!(c);
                st.string = Some(StrKind::Quoted('"'));
                leading = false;
                i += 1;
            }
            '\'' => {
                // Distinguish a char literal (`'a'`, `'\n'`) from a lifetime
                // (`'a`): only the former enters string mode.
                let esc_lit = chars.get(i + 1) == Some(&'\\') && chars.get(i + 3) == Some(&'\'');
                let plain_lit = chars.get(i + 1).is_some_and(|x| *x != '\'' && *x != '\\')
                    && chars.get(i + 2) == Some(&'\'');
                sep!();
                emit!(c);
                if esc_lit || plain_lit {
                    st.string = Some(StrKind::Quoted('\''));
                }
                leading = false;
                i += 1;
            }
            '{' | '[' | '(' => {
                net += 1;
                leading = false;
                sep!();
                emit!(c);
                // `[ u8` / `f( a` → the operand hugs the bracket. `{` keeps its
                // spacing (`Foo { x: 1 }`).
                suppress_space = c != '{';
                pending_space = false;
                i += 1;
            }
            '}' | ']' | ')' => {
                net -= 1;
                if leading {
                    leading_closers += 1;
                }
                // `4 ]` / `a )` → close up. `}` keeps its spacing.
                if c != '}' {
                    pending_space = false;
                }
                sep!();
                emit!(c);
                i += 1;
            }
            ':' if chars.get(i + 1) == Some(&':') => {
                // Path separator / turbofish — an ordinary token, never split.
                leading = false;
                sep!();
                emit!(':');
                emit!(':');
                i += 2;
            }
            ',' | ';' | ':' => {
                leading = false;
                pending_space = false; // no space *before*
                sep!();
                emit!(c);
                // …and exactly one after, unless a closing bracket or the end of
                // the line follows.
                pending_space = next_non_space(&chars, i + 1)
                    .is_some_and(|next| !NO_SPACE_BEFORE.contains(&next));
                suppress_space = false;
                i += 1;
            }
            _ => {
                leading = false;
                sep!();
                emit!(c);
                i += 1;
            }
        }
    }

    LineInfo {
        leading_closers,
        net,
        text: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(s: &str) -> String {
        reindent_opts(s, false)
    }

    /// Re-space + re-indent, as a Rust file gets it.
    fn rs(s: &str) -> String {
        reindent_opts(s, true)
    }

    /// One line through the re-spacer only (no indentation).
    fn sp(s: &str) -> String {
        rs(s)
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
        let expected = "fn f() {\n    let s = \"a { b }\";\n    // a } brace } here\n    g();\n}";
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
        let (out, new_cursor) = format_code(src, cursor, true);
        // New cursor is at the start of line 1 in the formatted text.
        let line1_start = out.find("    let").unwrap();
        assert_eq!(new_cursor, line1_start);
    }

    // ── Re-spacing ───────────────────────────────────────────────────────────

    #[test]
    fn collapses_multiple_spaces() {
        assert_eq!(sp("let    x   =  1;"), "let x = 1;");
    }

    #[test]
    fn fn_params_get_one_space_after_colon_and_comma() {
        assert_eq!(
            sp("fn func(a:u32 ,b : u32) -> (u32, u32)"),
            "fn func(a: u32, b: u32) -> (u32, u32)"
        );
    }

    #[test]
    fn array_type_spacing() {
        assert_eq!(sp("let b: [ u8;4 ] = [0;4];"), "let b: [u8; 4] = [0; 4];");
    }

    #[test]
    fn generics_get_space_after_comma() {
        assert_eq!(
            sp("struct S<'a, DECODER,PAYLOAD_LEN>;"),
            "struct S<'a, DECODER, PAYLOAD_LEN>;"
        );
    }

    #[test]
    fn const_generics_get_space_after_colon() {
        assert_eq!(
            sp("struct S<'a, DECODER,const PAYLOAD_LEN:usize,const RESERVED_LEN:usize>;"),
            "struct S<'a, DECODER, const PAYLOAD_LEN: usize, const RESERVED_LEN: usize>;"
        );
    }

    #[test]
    fn path_separator_is_never_split() {
        assert_eq!(
            sp("let v = alloc::vec::Vec::<u8>::new();"),
            "let v = alloc::vec::Vec::<u8>::new();"
        );
        assert_eq!(sp("x.parse::<u32>()"), "x.parse::<u32>()");
    }

    #[test]
    fn comparison_and_shifts_untouched() {
        assert_eq!(sp("if a < b && c >> 2 > d {"), "if a < b && c >> 2 > d {");
        assert_eq!(sp("let v: Vec<Vec<u8>> = x;"), "let v: Vec<Vec<u8>> = x;");
    }

    #[test]
    fn trailing_comma_and_semicolon_keep_no_space() {
        assert_eq!(sp("foo(a, b,);"), "foo(a, b,);");
        assert_eq!(sp("let x = [1, 2,];"), "let x = [1, 2,];");
    }

    #[test]
    fn braces_keep_their_spacing() {
        assert_eq!(
            sp("let f = Foo { x: 1, y: 2 };"),
            "let f = Foo { x: 1, y: 2 };"
        );
    }

    #[test]
    fn closures_and_match_arms() {
        assert_eq!(sp("let f = |a,b| a + b;"), "let f = |a, b| a + b;");
        assert_eq!(sp("Some(x) => x,"), "Some(x) => x,");
    }

    #[test]
    fn string_contents_are_untouched() {
        assert_eq!(sp(r#"let s = "a,b:c   d";"#), r#"let s = "a,b:c   d";"#);
        assert_eq!(
            sp(r#"println!("{:?}   {}", a,b);"#),
            r#"println!("{:?}   {}", a, b);"#
        );
    }

    #[test]
    fn comment_contents_are_untouched() {
        assert_eq!(sp("let x = 1; //  a,b:c   d"), "let x = 1; //  a,b:c   d");
        assert_eq!(sp("/*  a,b:c  */ let x = 1;"), "/*  a,b:c  */ let x = 1;");
    }

    #[test]
    fn doc_comments_are_untouched() {
        let src = "/// `Foo::bar(a,b)`   keeps   its spacing\nfn f() {}";
        assert_eq!(rs(src), src);
    }

    #[test]
    fn multi_line_string_is_not_rewritten() {
        // The second line lives inside the string literal: verbatim, no
        // re-indent and no re-spacing of its `,`/`:`.
        let src = "fn f() {\nlet s = \"line1,x\nline2,y:z   w\";\n}";
        let expected = "fn f() {\n    let s = \"line1,x\nline2,y:z   w\";\n}";
        assert_eq!(rs(src), expected);
    }

    #[test]
    fn raw_string_is_not_rewritten() {
        let src = "let s = r#\"a,b:c   \"quoted\"\"#;";
        assert_eq!(sp(src), src);
        // …and one spanning lines keeps its second line verbatim.
        let multi = "fn f() {\nlet s = r#\"a,b\nc:d   e\"#;\n}";
        let expected = "fn f() {\n    let s = r#\"a,b\nc:d   e\"#;\n}";
        assert_eq!(rs(multi), expected);
    }

    #[test]
    fn char_literals_and_lifetimes() {
        assert_eq!(sp("let c = ',';"), "let c = ',';");
        assert_eq!(sp("let c = ':';"), "let c = ':';");
        assert_eq!(
            sp("impl<'a> Foo<'a> where 'a: 'static {"),
            "impl<'a> Foo<'a> where 'a: 'static {"
        );
        assert_eq!(sp("'outer: loop {"), "'outer: loop {");
    }

    #[test]
    fn indexing_and_ranges() {
        assert_eq!(sp("let x = arr[ i ];"), "let x = arr[i];");
        assert_eq!(sp("let s = &buf[0..=4];"), "let s = &buf[0..=4];");
    }

    #[test]
    fn attributes_get_comma_spacing() {
        assert_eq!(sp("#[derive(Debug,Clone)]"), "#[derive(Debug, Clone)]");
    }

    #[test]
    fn respacing_is_idempotent() {
        let src =
            "fn func(a:u32 ,b : u32) {\nlet b: [ u8;4 ] = [0;4];\nlet s = \"a,b\";\n// c,d\n}";
        let once = rs(src);
        assert_eq!(rs(&once), once, "second format must be a no-op");
    }

    #[test]
    fn respace_off_leaves_spacing_alone() {
        // The non-Rust path must still only re-indent.
        let src = "MEMORY\n{\nFLASH : ORIGIN = 0x08000000, LENGTH = 64K\n}";
        let expected = "MEMORY\n{\n    FLASH : ORIGIN = 0x08000000, LENGTH = 64K\n}";
        assert_eq!(fmt(src), expected);
    }
}
