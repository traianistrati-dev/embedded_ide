//! Brace-block selection: **Ctrl+[ / Ctrl+]** selects the innermost `{ … }`
//! block around the caret (braces included) and copies it to the clipboard.
//! (Refactored 2026-07-05 off the old implicit trigger — selecting a `{`/`}`
//! character — which fired, and hijacked Ctrl+C, on any accidental brace
//! selection.) Triple-click full-definition selection lives here too.
//!
//! Brace matching skips braces inside strings, char literals, and `//` / `/* */`
//! comments — so the `{}` in `format!("{}", x)` never throws the pairing off.

use crate::app::{AppIde, ProjectFileId};
use eframe::egui;
use std::collections::HashMap;

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// If a raw string (`r"…"`, `r#"…"#`, `br#"…"#`) starts at `p`, the index just
/// past its closing delimiter; otherwise `None`.
fn raw_string_end(chars: &[char], p: usize) -> Option<usize> {
    let mut i = p;
    if chars.get(i) == Some(&'b') {
        i += 1;
    }
    if chars.get(i) != Some(&'r') {
        return None;
    }
    i += 1;
    let hash_start = i;
    while chars.get(i) == Some(&'#') {
        i += 1;
    }
    let hashes = i - hash_start;
    if chars.get(i) != Some(&'"') {
        return None;
    }
    i += 1;
    while i < chars.len() {
        if chars[i] == '"' {
            let mut j = i + 1;
            let mut seen = 0;
            while seen < hashes && chars.get(j) == Some(&'#') {
                j += 1;
                seen += 1;
            }
            if seen == hashes {
                return Some(j);
            }
        }
        i += 1;
    }
    Some(chars.len())
}

/// If a comment / string / raw string / char literal / lifetime starts at `i`,
/// return the index just past it; otherwise `None`. Lets brace scanning skip
/// over non-code so braces inside `"{}"`, `// }`, `'{'`, etc. never count.
fn skip_token(chars: &[char], i: usize) -> Option<usize> {
    let n = chars.len();
    let c = chars[i];
    // Line comment.
    if c == '/' && chars.get(i + 1) == Some(&'/') {
        let mut j = i;
        while j < n && chars[j] != '\n' {
            j += 1;
        }
        return Some(j);
    }
    // Block comment.
    if c == '/' && chars.get(i + 1) == Some(&'*') {
        let mut j = i + 2;
        while j < n && !(chars[j - 1] == '*' && chars[j] == '/') {
            j += 1;
        }
        if j < n {
            j += 1;
        }
        return Some(j);
    }
    // Raw string.
    if let Some(end) = raw_string_end(chars, i) {
        return Some(end);
    }
    // String / byte string.
    if c == '"' || (c == 'b' && chars.get(i + 1) == Some(&'"')) {
        let mut j = if c == 'b' { i + 1 } else { i };
        j += 1;
        while j < n {
            match chars[j] {
                '\\' => j += 2,
                '"' => {
                    j += 1;
                    break;
                }
                _ => j += 1,
            }
        }
        return Some(j);
    }
    // Char literal vs lifetime.
    if c == '\'' {
        if chars.get(i + 1) == Some(&'\\') {
            let mut j = i + 2;
            while j < n && chars[j] != '\'' {
                j += 1;
            }
            if j < n {
                j += 1;
            }
            return Some(j);
        }
        let next_ident = chars
            .get(i + 1)
            .is_some_and(|&c| c.is_alphabetic() || c == '_');
        if next_ident && chars.get(i + 2) != Some(&'\'') {
            let mut j = i + 1;
            while j < n && is_ident(chars[j]) {
                j += 1;
            }
            return Some(j); // lifetime — no braces inside
        }
        let mut j = i + 1;
        if j < n {
            j += 1; // the char
        }
        if j < n && chars[j] == '\'' {
            j += 1; // closing quote
        }
        return Some(j);
    }
    None
}

/// Map every matched `{`/`}` to its partner (both directions), ignoring braces
/// in strings / char literals / comments. Unbalanced braces are simply omitted.
fn brace_pairs(chars: &[char]) -> HashMap<usize, usize> {
    let n = chars.len();
    let mut stack: Vec<usize> = Vec::new();
    let mut pairs = HashMap::new();
    let mut i = 0;
    while i < n {
        if let Some(j) = skip_token(chars, i) {
            i = j;
            continue;
        }
        if chars[i] == '{' {
            stack.push(i);
        } else if chars[i] == '}' {
            if let Some(open) = stack.pop() {
                pairs.insert(open, i);
                pairs.insert(i, open);
            }
        }
        i += 1;
    }
    pairs
}

/// The matched `{ … }` block `(open, close)` (inclusive char indices) for a
/// caret at `caret`: a brace directly at (or just before) the caret wins;
/// otherwise the innermost matched pair CONTAINING the caret. `None` when the
/// caret sits outside every block.
fn block_at_caret(chars: &[char], caret: usize) -> Option<(usize, usize)> {
    let pairs = brace_pairs(chars);
    // A brace the caret touches wins — matches the old "on the brace" feel.
    for idx in [caret, caret.wrapping_sub(1)] {
        if idx < chars.len() && matches!(chars[idx], '{' | '}') {
            if let Some(&other) = pairs.get(&idx) {
                return Some((idx.min(other), idx.max(other)));
            }
        }
    }
    // Otherwise: the innermost enclosing pair = the one opening latest before
    // the caret. (`pairs` maps both directions; `o < c` walks each pair once.)
    pairs
        .iter()
        .filter(|&(&o, &c)| o < c && o < caret && caret <= c)
        .max_by_key(|&(&o, _)| o)
        .map(|(&o, &c)| (o, c))
}

/// `&str` wrapper around [`block_at_caret`] for tests.
#[cfg(test)]
pub(super) fn brace_block_at(text: &str, caret: usize) -> Option<(usize, usize)> {
    block_at_caret(&text.chars().collect::<Vec<_>>(), caret)
}

// ── Full-definition selection (fn / struct / enum / if / while / match / …) ────

/// Index of the next body-opening `{` at/after `from`, scanning over non-code.
/// `None` if a `;` or `}` (statement / block end) comes first — i.e. there's no
/// block (a `;`-terminated item like `struct S;` or `fn f();`).
fn next_block_open(chars: &[char], from: usize) -> Option<usize> {
    let n = chars.len();
    let mut i = from;
    while i < n {
        if let Some(j) = skip_token(chars, i) {
            i = j;
            continue;
        }
        match chars[i] {
            '{' => return Some(i),
            ';' | '}' => return None,
            _ => i += 1,
        }
    }
    None
}

/// Start of the header that precedes the body `{` at `open`: scan back to the
/// previous statement / block boundary (`;` `{` `}`) and skip forward over the
/// whitespace, so the result is the first non-blank char of the construct
/// (its keyword / attribute / doc comment). Clamps to the start of file.
fn header_start(chars: &[char], open: usize) -> usize {
    let mut h = open;
    while h > 0 {
        match chars[h - 1] {
            ';' | '{' | '}' => break,
            _ => h -= 1,
        }
    }
    while h < open && chars[h].is_whitespace() {
        h += 1;
    }
    h
}

/// The whole definition `(start, close)` (inclusive char indices) whose body
/// brace is `brace_idx` — used for a triple-click on a `{`/`}`.
fn full_def_from_brace(chars: &[char], brace_idx: usize) -> Option<(usize, usize)> {
    let other = *brace_pairs(chars).get(&brace_idx)?;
    let open = brace_idx.min(other);
    let close = brace_idx.max(other);
    Some((header_start(chars, open), close))
}

/// True when `[lo, hi)` is exactly one identifier word. `false` (not a panic) for
/// an out-of-bounds range — defends against a stale cursor outliving an edit that
/// shrank the text.
fn is_word_selection(chars: &[char], lo: usize, hi: usize) -> bool {
    lo < hi
        && hi <= chars.len()
        && chars[lo..hi].iter().all(|&c| is_ident(c))
        && !chars[lo].is_ascii_digit()
        && (lo == 0 || !is_ident(chars[lo - 1]))
        && chars.get(hi).is_none_or(|&c| !is_ident(c))
}

/// True when the word starting at `lo` is the NAME in a block-bodied definition
/// (`fn`/`struct`/`enum`/`trait`/`impl`/`mod`/`union` immediately before it).
fn preceded_by_def_keyword(chars: &[char], lo: usize) -> bool {
    let mut i = lo;
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    let end = i;
    while i > 0 && is_ident(chars[i - 1]) {
        i -= 1;
    }
    let kw: String = chars[i..end].iter().collect();
    matches!(
        kw.as_str(),
        "fn" | "struct" | "enum" | "trait" | "impl" | "mod" | "union"
    )
}

/// The whole definition `(start, close)` when `[lo, hi)` selects a definition
/// NAME (the `foo` in `fn foo(..) { .. }`). `None` when it isn't a definition
/// name or has no `{ }` body.
fn full_def_from_name(chars: &[char], lo: usize, hi: usize) -> Option<(usize, usize)> {
    if !is_word_selection(chars, lo, hi) || !preceded_by_def_keyword(chars, lo) {
        return None;
    }
    let open = next_block_open(chars, hi)?;
    let other = *brace_pairs(chars).get(&open)?;
    let close = open.max(other);
    Some((header_start(chars, open), close))
}

/// The whole definition `(start, close)` when the char range `[lo, hi)` — a
/// triple-click's whole-line selection — CONTAINS a definition header: the
/// first word preceded by `fn`/`struct`/`enum`/… whose construct has a `{ }`
/// body. Covers headers whose opening `{` sits on a LATER line (multi-line
/// signatures), which the brace-on-line path can't reach.
fn full_def_from_line(chars: &[char], lo: usize, hi: usize) -> Option<(usize, usize)> {
    let hi = hi.min(chars.len());
    let mut i = lo.min(hi);
    while i < hi {
        if is_ident(chars[i]) && (i == 0 || !is_ident(chars[i - 1])) {
            let mut j = i;
            while j < chars.len() && is_ident(chars[j]) {
                j += 1;
            }
            // `full_def_from_name` itself rejects words not preceded by a def
            // keyword, so just probe every word on the line.
            if let Some(def) = full_def_from_name(chars, i, j) {
                return Some(def);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    None
}

/// `&str` test wrapper: full definition for a NAME selection `[lo, hi)`.
#[cfg(test)]
pub(super) fn full_def_name(text: &str, lo: usize, hi: usize) -> Option<(usize, usize)> {
    full_def_from_name(&text.chars().collect::<Vec<_>>(), lo, hi)
}

/// `&str` test wrapper: full definition for a line selection `[lo, hi)`.
#[cfg(test)]
pub(super) fn full_def_line(text: &str, lo: usize, hi: usize) -> Option<(usize, usize)> {
    full_def_from_line(&text.chars().collect::<Vec<_>>(), lo, hi)
}

/// `&str` test wrapper: full definition for a brace at `brace_idx`.
#[cfg(test)]
pub(super) fn full_def_brace(text: &str, brace_idx: usize) -> Option<(usize, usize)> {
    full_def_from_brace(&text.chars().collect::<Vec<_>>(), brace_idx)
}

/// Paint one same-row segment `[s, e)` of the block band.
fn paint_segment(
    painter: &egui::Painter,
    galley: &egui::text::Galley,
    gp: egui::Pos2,
    clip: egui::Rect,
    color: egui::Color32,
    s: usize,
    e: usize,
) {
    let loc_s = galley.pos_from_cursor(egui::text::CCursor::new(s));
    let loc_e = galley.pos_from_cursor(egui::text::CCursor::new(e));
    let y_top = gp.y + loc_s.min.y;
    let y_bot = gp.y + loc_s.max.y;
    if y_bot < clip.top() || y_top > clip.bottom() {
        return;
    }
    let x_l = gp.x + loc_s.min.x;
    // Keep a thin sliver for empty lines so the block stays visually continuous.
    let x_r = (gp.x + loc_e.min.x).max(x_l + 3.0);
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(x_l, y_top), egui::pos2(x_r, y_bot)),
        0.0,
        color,
    );
}

/// Paint a translucent band over the inclusive char range `[open, close]`,
/// one rectangle per line (split at `\n`). `pub(super)` so sibling overlay
/// modules (e.g. `usages`) can reuse the same multi-line band technique.
pub(super) fn paint_band(
    painter: &egui::Painter,
    galley: &egui::text::Galley,
    gp: egui::Pos2,
    clip: egui::Rect,
    chars: &[char],
    open: usize,
    close: usize,
    color: egui::Color32,
) {
    let end = (close + 1).min(chars.len());
    let mut seg_start = open;
    let mut i = open;
    while i < end {
        if chars[i] == '\n' {
            paint_segment(painter, galley, gp, clip, color, seg_start, i);
            seg_start = i + 1;
        }
        i += 1;
    }
    paint_segment(painter, galley, gp, clip, color, seg_start, end);
}

impl AppIde {
    /// Ctrl+[ / Ctrl+]: select the innermost `{ … }` block around the caret
    /// (braces included) and copy it to the clipboard. The selection is stored
    /// for next frame (the widget already rendered), the copy happens now.
    pub(super) fn select_brace_block(
        &self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        display_code: &str,
    ) {
        let Some(range) = editor_resp.state.cursor.char_range() else {
            return;
        };
        let chars: Vec<char> = display_code.chars().collect();
        let caret = range.primary.index.min(chars.len());
        let Some((open, close)) = block_at_caret(&chars, caret) else {
            return;
        };

        let end = (close + 1).min(chars.len());
        let mut st = editor_resp.state.clone();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(open),
            egui::text::CCursor::new(end),
        )));
        st.store(ui.ctx(), editor_resp.response.id);
        ui.ctx()
            .copy_text(chars[open..end].iter().collect::<String>());
    }

    /// Highlight the WHOLE definition (fn / struct / enum / if / while / match /
    /// …) with a translucent white band (RGBA 255,255,255,50) and copy it on
    /// Ctrl+C. Triggers ONLY on a TRIPLE-click — on a `{`/`}`, on any line
    /// holding a brace of the construct, or on the definition's header line
    /// (its `fn`/`struct`/… name). Double-click stays plain word-select.
    /// Returns `true` when it owns this frame (so the caller skips the
    /// brace-block highlight).
    pub(super) fn highlight_full_definition(
        &mut self,
        editor_resp: &egui::text_edit::TextEditOutput,
        display_code: &str,
        displayed_file: ProjectFileId,
        clip: egui::Rect,
        ui: &egui::Ui,
        copy_requested: bool,
    ) -> bool {
        let chars: Vec<char> = display_code.chars().collect();
        // Clamp to the current text: a stale cursor (recorded before an edit that
        // just shrank the file — e.g. a Clippy "Fix" mid-frame) would otherwise
        // index past the end and panic in `is_word_selection`'s `chars[lo..hi]`.
        let (lo, hi) = match editor_resp.state.cursor.char_range() {
            Some(r) => (
                r.primary.index.min(r.secondary.index).min(chars.len()),
                r.primary.index.max(r.secondary.index).min(chars.len()),
            ),
            None => (0, 0),
        };
        let white = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 50);
        let paint = |start: usize, close: usize| {
            let painter = ui.painter().with_clip_rect(clip);
            paint_band(
                &painter,
                &editor_resp.galley,
                editor_resp.galley_pos,
                clip,
                &chars,
                start,
                close,
                white,
            );
        };
        let copy = |start: usize, close: usize| {
            if copy_requested && close < chars.len() {
                ui.ctx()
                    .copy_text(chars[start..=close].iter().collect::<String>());
            }
        };

        // 1. Triple-click → select the whole construct. egui set the selection
        //    to the clicked line; find a (matched) brace on it, or — for headers
        //    whose `{` sits on a later line (multi-line signatures) — a
        //    definition name on it. (Moved off double-click by request:
        //    double-click keeps egui's plain word-select + the cyan
        //    word-occurrence highlight; triple-click selects the definition.)
        if editor_resp.response.triple_clicked() {
            let pairs = brace_pairs(&chars);
            let def = (lo..hi)
                .find(|i| pairs.contains_key(i))
                .and_then(|b| full_def_from_brace(&chars, b))
                .or_else(|| full_def_from_line(&chars, lo, hi));
            if let Some((start, close)) = def {
                self.ed.full_block_selection = Some((displayed_file, start, close));
                // Make the whole construct the editor selection (so it's
                // visibly selected and Ctrl+C copies it).
                let mut st = editor_resp.state.clone();
                let end = (close + 1).min(chars.len());
                st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(start),
                    egui::text::CCursor::new(end),
                )));
                st.store(ui.ctx(), editor_resp.response.id);
                paint(start, close);
                return true;
            }
        }

        // 2. A persisted triple-click selection — keep highlighting while the
        //    editor selection still matches it; clear once the user moves on.
        if let Some((f, start, close)) = self.ed.full_block_selection {
            if f == displayed_file {
                if lo == start && hi == (close + 1).min(chars.len()) {
                    copy(start, close);
                    paint(start, close);
                    return true;
                }
                self.ed.full_block_selection = None; // selection diverged
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::{brace_block_at, full_def_brace, full_def_line, full_def_name};

    #[test]
    fn triple_click_header_line_selects_whole_definition() {
        // Triple-click selects the whole first line — even without a `{` on it
        // (multi-line signature), the definition resolves via its name.
        let src = "pub fn foo(\n    x: u8,\n) {\n    body();\n}";
        let line_end = src.find('\n').unwrap() + 1;
        let close = src.rfind('}').unwrap();
        assert_eq!(full_def_line(src, 0, line_end), Some((0, close)));
    }

    #[test]
    fn triple_click_plain_line_selects_nothing() {
        // A body line with no definition keyword resolves to no definition.
        let src = "fn f() {\n    let x = 1;\n}";
        let lo = src.find("    let").unwrap();
        let hi = src.find("1;").unwrap() + 2;
        assert_eq!(full_def_line(src, lo, hi), None);
        // A bodyless item (`struct S;`) doesn't resolve either.
        assert_eq!(full_def_line("struct S;", 0, 9), None);
    }

    #[test]
    fn full_def_from_function_name() {
        let src = "fn foo() {\n    a;\n}";
        let name = src.find("foo").unwrap();
        let close = src.rfind('}').unwrap();
        // Double-click / Shift-select `foo` → whole `fn foo() { … }`.
        assert_eq!(full_def_name(src, name, name + 3), Some((0, close)));
    }

    #[test]
    fn full_def_from_struct_name_includes_attrs() {
        // The header scan reaches back to the previous boundary (the `;`), so a
        // leading attribute / doc line is part of the definition.
        let src = "let x = 1;\n#[derive(Clone)]\nstruct S {\n    a: u8,\n}";
        let start = src.find("#[derive").unwrap();
        let name = src.find("S {").unwrap();
        let close = src.rfind('}').unwrap();
        assert_eq!(full_def_name(src, name, name + 1), Some((start, close)));
    }

    #[test]
    fn name_without_def_keyword_or_block_is_none() {
        // `Point` in a struct literal is not a definition name.
        let src = "let p = Point { x: 1 };";
        let pt = src.find("Point").unwrap();
        assert_eq!(full_def_name(src, pt, pt + 5), None);
        // `foo` of a bodyless decl (`fn foo();`) has no block.
        let src2 = "trait T { fn foo(); }";
        let foo = src2.find("foo").unwrap();
        assert_eq!(full_def_name(src2, foo, foo + 3), None);
    }

    #[test]
    fn triple_click_brace_spans_whole_construct() {
        // A triple-click on either brace of an `if { … }` selects the whole `if`.
        let src = "if x > 0 {\n    a;\n}";
        let open = src.find('{').unwrap();
        let close = src.rfind('}').unwrap();
        assert_eq!(full_def_brace(src, open), Some((0, close)));
        assert_eq!(full_def_brace(src, close), Some((0, close)));
    }

    #[test]
    fn triple_click_inner_brace_spans_inner_construct() {
        let src = "fn f() {\n    while y {\n        z;\n    }\n}";
        let inner_open = src.find("{\n        z").unwrap();
        let inner_close = src[inner_open..].find('}').unwrap() + inner_open;
        let while_kw = src.find("while").unwrap();
        assert_eq!(
            full_def_brace(src, inner_open),
            Some((while_kw, inner_close))
        );
    }

    #[test]
    fn caret_on_a_brace_spans_its_block() {
        let src = "fn f() { a; }";
        let open = src.find('{').unwrap(); // char idx == byte idx (ASCII)
        let close = src.find('}').unwrap();
        // Caret right before the `{`, right after it, and on the `}`.
        assert_eq!(brace_block_at(src, open), Some((open, close)));
        assert_eq!(brace_block_at(src, open + 1), Some((open, close)));
        assert_eq!(brace_block_at(src, close), Some((open, close)));
    }

    #[test]
    fn caret_inside_nested_block_picks_innermost() {
        let src = "a { b { c } d }";
        let inner_open = src.find("{ c").unwrap();
        let inner_close = src.find('}').unwrap();
        let outer_open = src.find('{').unwrap();
        let outer_close = src.rfind('}').unwrap();
        // Caret on `c` → the inner pair.
        let on_c = src.find('c').unwrap();
        assert_eq!(brace_block_at(src, on_c), Some((inner_open, inner_close)));
        // Caret on `d` (between the blocks) → the outer pair.
        let on_d = src.find('d').unwrap();
        assert_eq!(brace_block_at(src, on_d), Some((outer_open, outer_close)));
    }

    #[test]
    fn braces_in_strings_and_comments_are_ignored() {
        // The `{` in the format string and the `}` in the comment must not pair.
        let src = "{ println!(\"{}\", x); // }\n}";
        let close = src.rfind('}').unwrap();
        // Caret on the real opening brace…
        assert_eq!(brace_block_at(src, 0), Some((0, close)));
        // …and on code inside — same (only) real pair, not the string's `{}`.
        let on_x = src.find(", x").unwrap();
        assert_eq!(brace_block_at(src, on_x), Some((0, close)));
    }

    #[test]
    fn caret_outside_any_block_returns_none() {
        let src = "fn f() { a; }";
        assert_eq!(brace_block_at(src, 0), None); // on `fn`, before any block
        assert_eq!(brace_block_at("no braces at all", 5), None);
        assert_eq!(brace_block_at("", 0), None);
    }
}
