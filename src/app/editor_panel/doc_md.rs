//! Rustdoc markdown → styled lines for the completion detail panel.
//!
//! rust-analyzer sends completion `documentation` as markdown (confirmed on the
//! wire: every `documentation` object in an LSP trace is `kind: "markdown"`).
//! This splits it into lines tagged with how they should be drawn, so the panel
//! can show code examples in monospace instead of running them together with
//! the prose.
//!
//! Two rustdoc conventions drive the parsing, and both need the fence state to
//! get right — which is why `lsp.rs` now hands over the markdown intact rather
//! than pre-flattening it:
//!
//! * ` ```rust ` fences delimit code examples.  Their *contents* are code, and
//!   the fence lines themselves are markup that should never be displayed.
//! * Inside a fence, a line starting with `#` is a **hidden line**: compiled as
//!   part of the doctest but not shown in rendered docs.  Outside a fence the
//!   same `#` is a heading.  Without fence tracking the two are indistinguishable
//!   and `# use std::fmt;` renders as a heading titled "use std::fmt;".
//!
//! Pure text analysis; all drawing lives in `completion.rs`.

/// How one line of documentation should be drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocKind {
    /// `# Examples` → level 1..=6, marker stripped.  Drawn larger/brighter.
    Heading(u8),
    /// A line inside a ` ``` ` fence. Monospace.
    Code,
    /// A `//` comment inside a fence.  Monospace and dimmed; the `//` is kept,
    /// since it is what marks the line as a comment once the fence is gone.
    Comment,
    /// Ordinary prose.
    Body,
    /// Paragraph break — carries no text, only vertical space.
    Blank,
}

/// One parsed line: how to draw it, and the text to draw (markers removed).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DocLine {
    pub kind: DocKind,
    pub text: String,
}

/// Parse rustdoc markdown into styled lines.
///
/// Leading whitespace is preserved inside fences (nested code stays readable)
/// and trimmed outside them, where it is only markdown indentation.
pub fn parse_doc(md: &str) -> Vec<DocLine> {
    let mut out: Vec<DocLine> = Vec::new();
    let mut in_fence = false;

    for line in md.lines() {
        let trimmed = line.trim();

        // Fence delimiters are markup: toggle state, never emit.
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }

        if in_fence {
            // Mirrors rustdoc's own `map_line`, in its order — the cases are
            // not independent:
            //   `##…`  escaped literal `#`, shown with ONE `#` removed
            //   `# …`  hidden doctest line
            //   `#`    hidden doctest line
            //   else   shown — INCLUDING `#[attr]`, which rustdoc deliberately
            //          cannot hide (that is why `# #[attr]` is the idiom), so
            //          dropping attributes here would delete real example code
            let text = if trimmed.starts_with("##") {
                line.replacen("##", "#", 1)
            } else if trimmed.starts_with("# ") || trimmed == "#" {
                continue;
            } else {
                line.to_owned()
            };
            let text = text.trim_end().to_owned();
            let kind = if text.trim_start().starts_with("//") {
                DocKind::Comment
            } else {
                DocKind::Code
            };
            out.push(DocLine { kind, text });
            continue;
        }

        if trimmed.is_empty() {
            // Collapse runs of blank lines, and never lead with one.
            if !matches!(out.last().map(|l| l.kind), None | Some(DocKind::Blank)) {
                out.push(DocLine {
                    kind: DocKind::Blank,
                    text: String::new(),
                });
            }
            continue;
        }

        if let Some((level, text)) = heading(trimmed) {
            out.push(DocLine {
                kind: DocKind::Heading(level),
                text,
            });
            continue;
        }

        out.push(DocLine {
            kind: DocKind::Body,
            text: trimmed.to_owned(),
        });
    }

    // A trailing blank contributes nothing but padding.
    while matches!(out.last().map(|l| l.kind), Some(DocKind::Blank)) {
        out.pop();
    }
    out
}

/// `## Examples` → `(2, "Examples")`.  Requires a space after the run of `#`,
/// per CommonMark, so `#[derive(Debug)]` in prose is not a heading.
fn heading(trimmed: &str) -> Option<(u8, String)> {
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    let text = rest.strip_prefix(' ')?.trim();
    if text.is_empty() {
        return None;
    }
    // Trailing `###` (closed ATX heading) is markup too.
    Some((
        hashes as u8,
        text.trim_end_matches('#').trim_end().to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(md: &str) -> Vec<DocKind> {
        parse_doc(md).into_iter().map(|l| l.kind).collect()
    }

    fn texts(md: &str) -> Vec<String> {
        parse_doc(md).into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn plain_prose_is_body() {
        assert_eq!(
            kinds("Writes 9-bit words to the UART."),
            vec![DocKind::Body]
        );
    }

    #[test]
    fn heading_marker_is_stripped() {
        let lines = parse_doc("# Examples");
        assert_eq!(lines[0].kind, DocKind::Heading(1));
        assert_eq!(lines[0].text, "Examples");
    }

    #[test]
    fn heading_level_is_the_hash_count() {
        assert_eq!(kinds("## Panics"), vec![DocKind::Heading(2)]);
        assert_eq!(kinds("###### Deep"), vec![DocKind::Heading(6)]);
        // 7+ hashes is not a heading in CommonMark.
        assert_eq!(kinds("####### Nope"), vec![DocKind::Body]);
    }

    #[test]
    fn hash_without_space_is_not_a_heading() {
        // Would otherwise swallow attributes mentioned in prose.
        assert_eq!(kinds("#[derive(Debug)]"), vec![DocKind::Body]);
        assert_eq!(kinds("#42 is the answer"), vec![DocKind::Body]);
    }

    #[test]
    fn closed_atx_heading_drops_trailing_hashes() {
        assert_eq!(texts("## Errors ##"), vec!["Errors".to_owned()]);
    }

    #[test]
    fn fence_lines_are_never_emitted() {
        let md = "Intro\n```rust\nlet x = 5;\n```";
        assert_eq!(kinds(md), vec![DocKind::Body, DocKind::Code]);
        assert_eq!(texts(md), vec!["Intro".to_owned(), "let x = 5;".to_owned()]);
    }

    #[test]
    fn comments_inside_a_fence_are_tagged_and_keep_their_slashes() {
        let md = "```\n// set the baud rate\nlet x = 5;\n```";
        assert_eq!(kinds(md), vec![DocKind::Comment, DocKind::Code]);
        assert_eq!(texts(md)[0], "// set the baud rate");
    }

    #[test]
    fn comment_outside_a_fence_stays_prose() {
        // `///` is stripped by rustdoc, so a bare `//` in prose is just text.
        assert_eq!(kinds("// not code"), vec![DocKind::Body]);
    }

    #[test]
    fn hidden_doctest_lines_are_dropped_not_read_as_headings() {
        // The collision this whole module exists for: inside a fence `# foo`
        // is a hidden line, and flattening the fence first turned it into a
        // heading titled "use std::fmt;".
        let md = "```rust\n# use std::fmt;\nlet x = 5;\n#\n```";
        assert_eq!(kinds(md), vec![DocKind::Code]);
        assert_eq!(texts(md), vec!["let x = 5;".to_owned()]);
    }

    #[test]
    fn attributes_are_kept_rustdoc_cannot_hide_them() {
        // rustdoc hides `# text`, never `#[attr]` — hence the `# #[attr]`
        // idiom. Dropping these would delete real lines from the example.
        let md = "```\n#[allow(unused)]\nlet x = 5;\n```";
        assert_eq!(kinds(md), vec![DocKind::Code, DocKind::Code]);
        assert_eq!(texts(md)[0], "#[allow(unused)]");
    }

    #[test]
    fn hidden_attribute_uses_the_hash_space_idiom() {
        let md = "```\n# #[allow(unused)]\nlet x = 5;\n```";
        assert_eq!(texts(md), vec!["let x = 5;".to_owned()]);
    }

    #[test]
    fn escaped_hash_in_a_fence_keeps_one_hash() {
        // `##` is rustdoc's escape for a line that really starts with `#`.
        let md = "```\n## not hidden\n```";
        assert_eq!(kinds(md), vec![DocKind::Code]);
        assert_eq!(texts(md), vec!["# not hidden".to_owned()]);
    }

    #[test]
    fn indentation_is_kept_in_code_and_dropped_in_prose() {
        let md = "```\nif x {\n    foo();\n}\n```";
        assert_eq!(texts(md)[1], "    foo();");
        assert_eq!(texts("   indented prose")[0], "indented prose");
    }

    #[test]
    fn blank_runs_collapse_and_never_bookend() {
        let md = "\n\nA\n\n\n\nB\n\n";
        assert_eq!(
            kinds(md),
            vec![DocKind::Body, DocKind::Blank, DocKind::Body]
        );
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(parse_doc("").is_empty());
        assert!(parse_doc("\n\n \n").is_empty());
    }

    #[test]
    fn realistic_rustdoc_block() {
        let md = "\
Writes 9-bit words to the UART/USART.

# Examples

```rust
# use stm32f1xx_hal::serial::Serial;
// send a word
tx.write_u16(0x1FF).unwrap();
```

# Errors

Returns `Err` when the peripheral is busy.";
        let lines = parse_doc(md);
        let got: Vec<(DocKind, &str)> = lines.iter().map(|l| (l.kind, l.text.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (DocKind::Body, "Writes 9-bit words to the UART/USART."),
                (DocKind::Blank, ""),
                (DocKind::Heading(1), "Examples"),
                (DocKind::Blank, ""),
                (DocKind::Comment, "// send a word"),
                (DocKind::Code, "tx.write_u16(0x1FF).unwrap();"),
                (DocKind::Blank, ""),
                (DocKind::Heading(1), "Errors"),
                (DocKind::Blank, ""),
                (DocKind::Body, "Returns `Err` when the peripheral is busy."),
            ]
        );
    }
}
