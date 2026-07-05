//! LSP snippet expansion (RFC: LSP `InsertTextFormat::Snippet`).
//!
//! rust-analyzer sends function/method completions as snippets when the client
//! advertises `snippetSupport`, e.g. `frequencies(${1:c})$0`.  The editor's
//! TextEdit knows nothing about snippets, so [`expand`] flattens one into plain
//! text plus the char-range of the **first tab stop** — the caller inserts the
//! text and selects that range, landing the caret on the first parameter:
//! `functia_selectata(a, b, c)` with `a` selected.
//!
//! Supported constructs (everything RA emits): `$1` / `$0` bare tab stops,
//! `${1}`, `${1:placeholder}` (nesting allowed), `${1|choice,…|}` (first choice
//! taken), and the `\$` / `\}` / `\\` escapes. Unknown `${VARIABLE}` forms keep
//! their inner text.

/// Flatten `snippet` to plain text. Returns the text and, when the snippet has
/// tab stops, the char range `(start, end)` to select: the lowest-numbered
/// stop > 0 (first occurrence on ties), falling back to `$0`'s caret position.
pub fn expand(snippet: &str) -> (String, Option<(usize, usize)>) {
    let chars: Vec<char> = snippet.chars().collect();
    let mut out: Vec<char> = Vec::new();
    let mut stops: Vec<(u32, usize, usize)> = Vec::new(); // (tabstop, start, end)
    let mut i = 0;
    parse_into(&chars, &mut i, &mut out, &mut stops, false);

    let sel = stops
        .iter()
        .filter(|s| s.0 > 0)
        .min_by_key(|s| s.0)
        .or_else(|| stops.iter().find(|s| s.0 == 0))
        .map(|&(_, s, e)| (s, e));
    (out.into_iter().collect(), sel)
}

/// Recursive-descent walk. Appends plain text to `out`, records tab stops.
/// With `stop_at_brace` the walk ends at (and consumes) the next unescaped `}`
/// — used for `${n:…}` placeholder bodies, which may nest further snippets.
fn parse_into(
    chars: &[char],
    i: &mut usize,
    out: &mut Vec<char>,
    stops: &mut Vec<(u32, usize, usize)>,
    stop_at_brace: bool,
) {
    while *i < chars.len() {
        let c = chars[*i];
        match c {
            '\\' if *i + 1 < chars.len() && matches!(chars[*i + 1], '$' | '}' | '\\') => {
                out.push(chars[*i + 1]);
                *i += 2;
            }
            '}' if stop_at_brace => {
                *i += 1;
                return;
            }
            '$' => {
                *i += 1;
                if chars.get(*i) == Some(&'{') {
                    *i += 1;
                    let n = read_number(chars, i);
                    match (n, chars.get(*i)) {
                        // ${n:placeholder} — body may nest more tab stops.
                        (Some(n), Some(':')) => {
                            *i += 1;
                            let start = out.len();
                            parse_into(chars, i, out, stops, true);
                            stops.push((n, start, out.len()));
                        }
                        // ${n|a,b,c|} — take the first choice.
                        (Some(n), Some('|')) => {
                            *i += 1;
                            let start = out.len();
                            while *i < chars.len() && chars[*i] != ',' && chars[*i] != '|' {
                                out.push(chars[*i]);
                                *i += 1;
                            }
                            while *i < chars.len() && chars[*i] != '}' {
                                *i += 1;
                            }
                            if *i < chars.len() {
                                *i += 1; // consume '}'
                            }
                            stops.push((n, start, out.len()));
                        }
                        // ${n}
                        (Some(n), _) => {
                            if chars.get(*i) == Some(&'}') {
                                *i += 1;
                            }
                            stops.push((n, out.len(), out.len()));
                        }
                        // ${VARIABLE} or malformed — keep the inner text verbatim.
                        (None, _) => {
                            while *i < chars.len() && chars[*i] != '}' {
                                out.push(chars[*i]);
                                *i += 1;
                            }
                            if *i < chars.len() {
                                *i += 1;
                            }
                        }
                    }
                } else {
                    // $n bare tab stop; a lone '$' stays literal.
                    match read_number(chars, i) {
                        Some(n) => stops.push((n, out.len(), out.len())),
                        None => out.push('$'),
                    }
                }
            }
            _ => {
                out.push(c);
                *i += 1;
            }
        }
    }
}

/// Consume a run of ASCII digits at `i`; `None` (and no advance) if absent.
fn read_number(chars: &[char], i: &mut usize) -> Option<u32> {
    let mut n: u32 = 0;
    let mut any = false;
    while let Some(d) = chars.get(*i).and_then(|c| c.to_digit(10)) {
        n = n.saturating_mul(10).saturating_add(d);
        any = true;
        *i += 1;
    }
    any.then_some(n)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::expand;

    /// The headline case: RA's `fill_arguments` call snippet.
    #[test]
    fn function_call_selects_first_arg() {
        let (text, sel) = expand("frequencies(${1:c})$0");
        assert_eq!(text, "frequencies(c)");
        assert_eq!(sel, Some((12, 13))); // selects `c`
    }

    #[test]
    fn multiple_args_pick_lowest_stop() {
        let (text, sel) = expand("draw(${1:ui}, ${2:c}, ${3:limits})$0");
        assert_eq!(text, "draw(ui, c, limits)");
        assert_eq!(sel, Some((5, 7))); // selects `ui`
    }

    /// Stops out of textual order: `$1` still wins over `$2`.
    #[test]
    fn lowest_stop_wins_regardless_of_position() {
        let (text, sel) = expand("m!(${2:b}, ${1:a})");
        assert_eq!(text, "m!(b, a)");
        assert_eq!(sel, Some((6, 7))); // selects `a`
    }

    /// No parameters: caret lands on `$0` (after the parens).
    #[test]
    fn no_args_caret_at_final_stop() {
        let (text, sel) = expand("init()$0");
        assert_eq!(text, "init()");
        assert_eq!(sel, Some((6, 6)));
    }

    /// Plain text (no snippet syntax) — no selection, text unchanged.
    #[test]
    fn plain_text_passes_through() {
        let (text, sel) = expand("plain_name");
        assert_eq!(text, "plain_name");
        assert_eq!(sel, None);
    }

    #[test]
    fn nested_placeholder_flattens() {
        let (text, sel) = expand("Some(${1:Ok(${2:v})})");
        assert_eq!(text, "Some(Ok(v))");
        assert_eq!(sel, Some((5, 10))); // outer `$1` selects `Ok(v)`
    }

    #[test]
    fn bare_and_braced_stops() {
        let (text, sel) = expand("let ${1} = $2;$0");
        assert_eq!(text, "let  = ;");
        assert_eq!(sel, Some((4, 4)));
    }

    #[test]
    fn choices_take_first_option() {
        let (text, sel) = expand("derive(${1|Debug,Clone,Copy|})");
        assert_eq!(text, "derive(Debug)");
        assert_eq!(sel, Some((7, 12)));
    }

    #[test]
    fn escapes_stay_literal() {
        let (text, sel) = expand(r"cost: \$${1:5} \\ ok\}");
        assert_eq!(text, r"cost: $5 \ ok}");
        assert_eq!(sel, Some((7, 8)));
    }

    /// Unknown `${VAR}` forms keep their inner text, no selection.
    #[test]
    fn variables_keep_inner_text() {
        let (text, sel) = expand("${TM_FILENAME}");
        assert_eq!(text, "TM_FILENAME");
        assert_eq!(sel, None);
    }
}
