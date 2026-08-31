//! Code folding — the model (phase 1).
//!
//! Folding breaks the editor's load-bearing invariant: the `LayoutJob` text has
//! always been the source *exactly*, which is what makes egui's cursor and
//! selection mapping line up with buffer offsets. A folded view is a projection
//! of the buffer, so everything that maps a char index to a screen position has
//! to go through [`FoldMap`].
//!
//! Two decisions keep that projection cheap and total:
//!
//! * **Only whole lines are removed.** A fold hides the *interior* of a block —
//!   the header line with `{` and the line with the matching `}` both stay. So
//!   the display is the buffer minus some line ranges, nothing else.
//! * **No synthetic text is inserted.** A `{ … }` placeholder would be text that
//!   exists on screen but not in the buffer, and every offset past it would need
//!   a special case. The "N lines hidden" affordance is painted as an overlay
//!   instead, the same way the "N refs" pill is.
//!
//! Together those make [`FoldMap::to_buffer`] total and exact: every display
//! offset corresponds to a real buffer offset.

use eframe::egui;
use std::collections::BTreeSet;

/// What kind of thing a foldable region delimits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    /// A function BODY — a `fn` keyword opened its signature. "Collapse all"
    /// folds only these; folding every block at once shreds the file instead of
    /// summarising it.
    Fn,
    /// Any other `{ … }`: an `impl`, a `match`, a `struct`, a bare block.
    Block,
    /// A `/* … */` comment.
    Comment,
}

/// A foldable region, as 0-based line indices. `end > head` always — something
/// that opens and closes on one line has nothing to hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    /// The line carrying the opening `{` or `/*`.
    pub head: usize,
    /// The line carrying the matching `}` or `*/`.
    pub end: usize,
    pub kind: RegionKind,
}

impl Region {
    /// The lines this region hides when folded — the interior, so both the
    /// header and the closing line stay on screen. The same rule for a comment
    /// as for a block: one projection, no per-kind special case.
    pub fn hidden(&self) -> std::ops::RangeInclusive<usize> {
        (self.head + 1)..=(self.end - 1)
    }

    /// How many lines fold away.
    pub fn hidden_count(&self) -> usize {
        self.end - self.head - 1
    }
}

/// Every foldable region in `text`, ordered by header line. Braces inside
/// strings, char literals and comments are skipped — `format!("{}", x)` must not
/// throw the pairing off, the same rule [`brace_block`](super::brace_block) and
/// the formatter already follow.
pub fn regions(text: &str) -> Vec<Region> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut stack: Vec<(usize, bool)> = Vec::new(); // (header line, is a fn body)
    let mut line = 0usize;
    let mut i = 0usize;
    // A `fn` keyword has been seen and no `{ } ;` has closed the signature yet,
    // so the next `{` opens that function's body. `Fn`/`FnMut` are capitalised
    // and never match; `let f: fn() = g;` is cleared by its `;`.
    let mut pending_fn = false;

    while i < n {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                // Rust block comments NEST. Stopping at the first `*/` would
                // both mis-report this comment's last line and drop the scanner
                // back into "code" inside the outer comment, where it would
                // invent brace regions out of prose.
                let head = line;
                let mut depth = 0usize;
                let mut closed = false;
                while i < n {
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                        continue;
                    }
                    if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            closed = true;
                            break;
                        }
                        continue;
                    }
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                // An unterminated `/*` (mid-typing) folds nothing: emitting here
                // would swallow the whole rest of the file.
                if closed && line > head {
                    out.push(Region {
                        head,
                        end: line,
                        kind: RegionKind::Comment,
                    });
                }
                // `pending_fn` is deliberately NOT cleared: `fn f() /* n */ {`
                // is legal, and the body must still count as a function.
            }
            '"' => {
                i = skip_string(&chars, i, &mut line);
            }
            'r' | 'b' if raw_string_at(&chars, i).is_some() => {
                let (hashes, quote) = raw_string_at(&chars, i).unwrap();
                i = skip_raw_string(&chars, quote, hashes, &mut line);
            }
            '\'' => {
                i = match char_literal_end(&chars, i) {
                    Some(end) => end + 1,
                    None => i + 1, // a lifetime
                };
            }
            '{' => {
                stack.push((line, pending_fn));
                pending_fn = false;
                i += 1;
            }
            '}' => {
                if let Some((head, is_fn)) = stack.pop() {
                    if line > head {
                        out.push(Region {
                            head,
                            end: line,
                            kind: if is_fn {
                                RegionKind::Fn
                            } else {
                                RegionKind::Block
                            },
                        });
                    }
                }
                pending_fn = false;
                i += 1;
            }
            ';' => {
                pending_fn = false;
                i += 1;
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                // Whole word only, so `effn` / `fnord` don't count.
                if chars[start..i] == ['f', 'n'] {
                    pending_fn = true;
                }
            }
            _ => i += 1,
        }
    }

    out.sort_by_key(|r| (r.head, r.end));
    out
}

/// The header lines of every foldable FUNCTION body — what "collapse all"
/// (Ctrl+Shift+Q) acts on.
pub fn fn_heads(text: &str) -> BTreeSet<usize> {
    regions(text)
        .into_iter()
        .filter(|r| r.kind == RegionKind::Fn)
        .map(|r| r.head)
        .collect()
}

/// The new fold set for a "toggle collapse all": anything folded → clear
/// everything; nothing folded → fold every function.
///
/// Comparing against "is anything folded" rather than flipping each block is
/// what makes the shortcut predictable after you have folded a few by hand —
/// the first press always tidies up, the second always collapses.
pub fn toggle_all(text: &str, current: &BTreeSet<usize>) -> BTreeSet<usize> {
    if current.is_empty() {
        fn_heads(text)
    } else {
        BTreeSet::new()
    }
}

/// Index just past a plain `"…"` string opened at `i`, counting newlines.
fn skip_string(chars: &[char], mut i: usize, line: &mut usize) -> usize {
    i += 1; // opening quote
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '"' => return i + 1,
            '\n' => {
                *line += 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    i
}

/// Index just past a raw string whose opening quote is at `quote`.
fn skip_raw_string(chars: &[char], quote: usize, hashes: usize, line: &mut usize) -> usize {
    let mut i = quote + 1;
    while i < chars.len() {
        if chars[i] == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#')) {
            return i + 1 + hashes;
        }
        if chars[i] == '\n' {
            *line += 1;
        }
        i += 1;
    }
    i
}

/// `(hash count, opening-quote index)` when a raw/byte-raw string starts at `i`.
fn raw_string_at(chars: &[char], i: usize) -> Option<(usize, usize)> {
    if i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
        return None;
    }
    let mut j = i;
    if chars.get(j) == Some(&'b') {
        j += 1;
    }
    if chars.get(j) != Some(&'r') {
        return None;
    }
    j += 1;
    let first_hash = j;
    while chars.get(j) == Some(&'#') {
        j += 1;
    }
    (chars.get(j) == Some(&'"')).then_some((j - first_hash, j))
}

/// Closing `'` of a char literal starting at `i`, or `None` for a lifetime.
fn char_literal_end(chars: &[char], i: usize) -> Option<usize> {
    match chars.get(i + 1) {
        Some('\\') => (i + 2..(i + 14).min(chars.len())).find(|&j| chars[j] == '\''),
        Some(&c) if c != '\'' => (chars.get(i + 2) == Some(&'\'')).then_some(i + 2),
        _ => None,
    }
}

/// The single replaced range between two versions of a text: `(start, end,
/// replacement)` in CHAR indices, where `start..end` indexes `before`. `None`
/// when they are equal.
///
/// This is what lets a fold survive editing. The editor is handed a projection
/// and hands one back; adopting that text wholesale would write the projection —
/// the file minus the hidden lines — into the buffer. Adopting its DELTA instead
/// keeps the hidden lines: the range is translated through
/// [`FoldMap::to_buffer`] and spliced into the real text.
///
/// Common prefix + common suffix, which describes any single edit exactly — and
/// one frame delivers at most one. It also handles an undo the same way, because
/// a restored earlier projection is just another delta.
pub fn text_delta(before: &str, after: &str) -> Option<(usize, usize, String)> {
    if before == after {
        return None;
    }
    let b: Vec<char> = before.chars().collect();
    let a: Vec<char> = after.chars().collect();
    let mut head = 0;
    while head < b.len() && head < a.len() && b[head] == a[head] {
        head += 1;
    }
    // The two tails must not run back past the shared head, or an insertion of
    // text that repeats its surroundings would report a negative-length range.
    let mut tail = 0;
    while tail < b.len() - head
        && tail < a.len() - head
        && b[b.len() - 1 - tail] == a[a.len() - 1 - tail]
    {
        tail += 1;
    }
    Some((
        head,
        b.len() - tail,
        a[head..a.len() - tail].iter().collect(),
    ))
}

/// Move fold heads that sit BELOW an edit, so they keep pointing at their own
/// block after the line count changed.
///
/// `at_line` is the 0-based buffer line the edit started on, `removed` / `added`
/// the line counts it replaced and inserted. A head inside the replaced span is
/// dropped: its block may not exist any more, and a stale head that lands on
/// another block's brace would fold something the user never asked to hide.
pub fn shift_heads(
    heads: &BTreeSet<usize>,
    at_line: usize,
    removed: usize,
    added: usize,
) -> BTreeSet<usize> {
    heads
        .iter()
        .filter_map(|&h| {
            if h <= at_line {
                Some(h)
            } else if h <= at_line + removed {
                None // inside what the edit replaced
            } else {
                Some((h + added).checked_sub(removed).unwrap_or(h))
            }
        })
        .collect()
}

/// Is there input this frame that could MODIFY the buffer? Folds are cleared on
/// one of these before the editor renders, so the keystroke lands on the full
/// text and the editor's write-back is always the whole file.
///
/// Deliberately narrow: navigation (arrows, Home/End, Ctrl+C, scrolling) and the
/// mouse must not disturb a fold — that is the whole point of folding.
pub fn edit_pending(ui: &egui::Ui) -> bool {
    use eframe::egui::{Event, Key};
    ui.input(|i| {
        i.events.iter().any(|e| match e {
            // Typing, deleting and pasting are NOT here: since phase 3 the
            // editor edits the projection and only its delta is adopted, so
            // those all land in the right place with the fold intact.
            //
            // Cut and Copy still are. They put the SELECTION on the clipboard,
            // and a selection spanning a folded block would hand over text with
            // the hidden lines deleted — pasting that back is a way to lose
            // them for real.
            Event::Cut | Event::Copy => true,
            Event::Key {
                key, pressed: true, ..
            } => {
                // The line ops (comment, move, duplicate, cut, format) rewrite
                // the buffer from their own caret arithmetic rather than
                // through the editor, so they need the full text.
                i.modifiers.command
                    && matches!(
                        key,
                        Key::D
                            | Key::X
                            | Key::F
                            | Key::V
                            | Key::Slash
                            | Key::ArrowUp
                            | Key::ArrowDown
                    )
            }
            _ => false,
        })
    })
}

/// One line that survives into the display text.
#[derive(Clone, Copy)]
struct VisLine {
    /// Char index of the line's first char in the BUFFER.
    buf_start: usize,
    /// Char index of the line's first char in the DISPLAY text.
    disp_start: usize,
    /// Line length in chars, newline excluded.
    len: usize,
    /// 0-based line index in the buffer.
    buf_line: usize,
}

/// The buffer → display projection for one set of folded blocks, plus the index
/// mapping every overlay needs to keep painting in the right place.
pub struct FoldMap {
    display: String,
    vis: Vec<VisLine>,
    /// 1-based buffer line number of each visible line — what the gutter shows.
    numbers: Vec<usize>,
    identity: bool,
}

impl FoldMap {
    /// Project `text`, hiding the interior of every region whose header line is
    /// in `folded`. A header with no matching region (the code changed under an
    /// old fold) is ignored, so a stale fold degrades to "not folded".
    pub fn new(text: &str, folded: &BTreeSet<usize>) -> Self {
        let hidden = hidden_lines(text, folded);
        if hidden.is_empty() {
            return Self::identity(text);
        }
        let mut vis = Vec::new();
        let mut display = String::new();
        let mut numbers = Vec::new();
        let mut buf_start = 0usize;
        for (buf_line, line) in text.split('\n').enumerate() {
            let len = line.chars().count();
            if !hidden.contains(&buf_line) {
                if !display.is_empty() {
                    display.push('\n');
                }
                vis.push(VisLine {
                    buf_start,
                    disp_start: display.chars().count(),
                    len,
                    buf_line,
                });
                numbers.push(buf_line + 1);
                display.push_str(line);
            }
            buf_start += len + 1; // + the '\n'
        }
        Self {
            display,
            vis,
            numbers,
            identity: false,
        }
    }

    /// The no-fold projection: the buffer itself.
    pub fn identity(text: &str) -> Self {
        Self {
            display: text.to_owned(),
            vis: Vec::new(),
            numbers: Vec::new(),
            identity: true,
        }
    }

    /// `true` when nothing is folded — callers take their normal path.
    pub fn is_identity(&self) -> bool {
        self.identity
    }

    /// The text to hand the editor.
    pub fn display(&self) -> &str {
        &self.display
    }

    /// 1-based buffer line numbers of the visible lines, for the gutter. Empty
    /// on the identity map (the gutter counts by itself).
    pub fn line_numbers(&self) -> &[usize] {
        &self.numbers
    }

    /// Display char index for a buffer char index, or `None` when it is inside a
    /// folded block.
    pub fn to_display(&self, buf_idx: usize) -> Option<usize> {
        if self.identity {
            return Some(buf_idx);
        }
        let v = self.line_at(buf_idx)?;
        let col = buf_idx - v.buf_start;
        (col <= v.len).then_some(v.disp_start + col)
    }

    /// Like [`to_display`](Self::to_display), but total: a buffer index that
    /// is HIDDEN lands at the end of the last visible line before it — the
    /// header line of the block it is inside.
    ///
    /// The caret is stored in BUFFER space between frames (see the two
    /// conversion points around the editor render in `editor_panel`), and it
    /// can legitimately point into a folded block: F12, a search hit or a
    /// diagnostic all place it there. It has to become *some* real display
    /// position before the editor is shown, and the header is the honest one.
    pub fn to_display_clamped(&self, buf_idx: usize) -> usize {
        if self.identity {
            return buf_idx;
        }
        match self.line_at(buf_idx) {
            Some(v) => v.disp_start + (buf_idx - v.buf_start).min(v.len),
            None => 0,
        }
    }

    /// Buffer char index for a display char index. Total: every display offset
    /// is a real buffer offset (no synthetic text exists).
    ///
    /// The inverse of [`to_display`](Self::to_display). Used when a keystroke
    /// unfolds the file: the caret the user placed while folded is an index into
    /// the projection, and it has to mean the same place in the buffer before
    /// the keystroke is applied.
    pub fn to_buffer(&self, disp_idx: usize) -> usize {
        if self.identity {
            return disp_idx;
        }
        let Some(v) = self
            .vis
            .iter()
            .rev()
            .find(|v| v.disp_start <= disp_idx)
            .copied()
        else {
            return 0;
        };
        v.buf_start + (disp_idx - v.disp_start).min(v.len)
    }

    /// Display line (0-based) showing this buffer line, or `None` when hidden.
    pub fn display_line_of(&self, buf_line: usize) -> Option<usize> {
        if self.identity {
            return Some(buf_line);
        }
        self.vis.iter().position(|v| v.buf_line == buf_line)
    }

    /// Translate analysis ranges into display space, dropping those that fall in
    /// a folded block. The fade / underline marks go through this.
    pub fn map_ranges(&self, ranges: &[(usize, usize)]) -> Vec<(usize, usize)> {
        if self.identity {
            return ranges.to_vec();
        }
        ranges
            .iter()
            .filter_map(|&(s, e)| Some((self.to_display(s)?, self.to_display(e)?)))
            .collect()
    }

    /// The visible line whose buffer span contains `buf_idx`.
    fn line_at(&self, buf_idx: usize) -> Option<VisLine> {
        let pos = self.vis.partition_point(|v| v.buf_start <= buf_idx);
        (pos > 0).then(|| self.vis[pos - 1])
    }
}

/// Every buffer line hidden by the folds in `folded`. Nested folds simply
/// overlap — the union is what matters.
fn hidden_lines(text: &str, folded: &BTreeSet<usize>) -> BTreeSet<usize> {
    if folded.is_empty() {
        return BTreeSet::new();
    }
    let mut hidden = BTreeSet::new();
    for r in regions(text) {
        if folded.contains(&r.head) {
            hidden.extend(r.hidden());
        }
    }
    hidden
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heads(src: &str) -> Vec<(usize, usize)> {
        regions(src).into_iter().map(|r| (r.head, r.end)).collect()
    }

    fn folded_of(src: &str, lines: &[usize]) -> FoldMap {
        FoldMap::new(src, &lines.iter().copied().collect())
    }

    const FN_SRC: &str = "fn a() {\n    let x = 1;\n    let y = 2;\n}\nfn b() {}\n";

    #[test]
    fn finds_a_multi_line_block() {
        assert_eq!(heads(FN_SRC), [(0, 3)]);
    }

    #[test]
    fn single_line_block_is_not_foldable() {
        // `fn b() {}` opens and closes on line 4 — nothing to hide.
        assert!(!heads(FN_SRC).contains(&(4, 4)));
    }

    #[test]
    fn nested_blocks_are_all_regions() {
        let src = "impl S {\n    fn f() {\n        x;\n    }\n}\n";
        assert_eq!(heads(src), [(0, 4), (1, 3)]);
    }

    #[test]
    fn braces_in_strings_and_comments_are_skipped() {
        let src = "fn f() {\n    let s = \"a { b\";\n    // }\n    /* { */\n}\n";
        assert_eq!(heads(src), [(0, 4)]);
    }

    #[test]
    fn raw_string_braces_are_skipped() {
        let src = "fn f() {\n    let s = r#\"a { b \"# ;\n}\n";
        assert_eq!(heads(src), [(0, 2)]);
    }

    #[test]
    fn char_literal_brace_is_skipped_but_lifetime_is_not() {
        let src = "impl<'a> S<'a> {\n    fn f() { let c = '{'; }\n}\n";
        assert_eq!(heads(src), [(0, 2)]);
    }

    #[test]
    fn unclosed_block_yields_no_region() {
        // Mid-typing: must not panic, must not invent a region.
        assert!(heads("fn f() {\n    x;\n").is_empty());
    }

    // ── Editing through a fold (phase 3) ─────────────────────────────────────

    fn delta(before: &str, after: &str) -> Option<(usize, usize, String)> {
        text_delta(before, after)
    }

    #[test]
    fn no_change_is_no_delta() {
        assert_eq!(delta("abc", "abc"), None);
    }

    #[test]
    fn insertion_reports_an_empty_range() {
        // "ab" -> "aXb": nothing removed, "X" inserted at 1.
        assert_eq!(delta("ab", "aXb"), Some((1, 1, "X".to_owned())));
    }

    #[test]
    fn deletion_reports_an_empty_replacement() {
        assert_eq!(delta("aXb", "ab"), Some((1, 2, String::new())));
    }

    #[test]
    fn replacement_reports_both() {
        assert_eq!(delta("aXb", "aYYb"), Some((1, 2, "YY".to_owned())));
    }

    #[test]
    fn repeated_text_around_an_insertion_stays_a_valid_range() {
        // The naive prefix+suffix walk can run the tail back past the head here.
        let d = delta("aa", "aaa").expect("a change");
        assert!(d.0 <= d.1, "range start {} past end {}", d.0, d.1);
        // …and applying it reproduces the new text.
        let mut out: String = "aa".chars().take(d.0).collect();
        out.push_str(&d.2);
        out.extend("aa".chars().skip(d.1));
        assert_eq!(out, "aaa");
    }

    #[test]
    fn a_delta_round_trips_through_the_fold_map() {
        // Type "X" at the start of the line after a folded block, and check the
        // BUFFER gets it in the right place with the hidden lines intact.
        let src = "fn a() {
    x;
    y;
}
after
";
        let m = folded_of(src, &[0]);
        assert_eq!(
            m.display(),
            "fn a() {
}
after
"
        );
        let edited = "fn a() {
}
Xafter
";
        let (s, e, ins) = delta(m.display(), edited).expect("a change");
        let (bs, be) = (m.to_buffer(s), m.to_buffer(e));
        let chars: Vec<char> = src.chars().collect();
        let mut out: String = chars[..bs].iter().collect();
        out.push_str(&ins);
        out.extend(&chars[be..]);
        assert_eq!(
            out,
            "fn a() {
    x;
    y;
}
Xafter
"
        );
    }

    #[test]
    fn shift_heads_moves_only_what_is_below() {
        let heads: BTreeSet<usize> = [2, 10, 20].into_iter().collect();
        // Two lines inserted at line 5.
        let out = shift_heads(&heads, 5, 0, 2);
        assert_eq!(out.into_iter().collect::<Vec<_>>(), [2, 12, 22]);
    }

    #[test]
    fn shift_heads_drops_a_head_inside_the_edit() {
        let heads: BTreeSet<usize> = [2, 7, 20].into_iter().collect();
        // Lines 5..=9 replaced by one line: the head at 7 is gone.
        let out = shift_heads(&heads, 5, 4, 1);
        assert_eq!(out.into_iter().collect::<Vec<_>>(), [2, 17]);
    }

    /// The rule the editor uses to refuse a delta: translated into the buffer,
    /// a range that GREW is one that swallowed hidden lines.
    fn crosses(m: &FoldMap, ds: usize, de: usize) -> bool {
        m.to_buffer(de) - m.to_buffer(ds) != de - ds
    }

    #[test]
    fn an_edit_on_a_visible_line_does_not_cross_a_fold() {
        let src = "fn a() {\n    x;\n}\nafter\n";
        let m = folded_of(src, &[0]);
        // The whole "after" line, in display space.
        let start = m.display().find("after").expect("visible");
        assert!(!crosses(&m, start, start + 5));
    }

    #[test]
    fn deleting_the_newline_under_a_fold_head_is_caught_as_crossing() {
        // Caret at the end of the header line, Delete pressed: in the
        // projection it joins two adjacent lines, in the buffer it would eat
        // the whole hidden body.
        let src = "fn a() {\n    x;\n    y;\n}\nafter\n";
        let m = folded_of(src, &[0]);
        let nl = m.display().find('\n').expect("a newline");
        assert!(crosses(&m, nl, nl + 1));
    }

    #[test]
    fn a_caret_inside_a_folded_block_lands_on_its_header() {
        let src = "fn a() {\n    x;\n    y;\n}\nafter\n";
        let m = folded_of(src, &[0]);
        let hidden = src.find("y;").expect("hidden");
        // End of the header line — the last visible position before it.
        assert_eq!(m.to_display_clamped(hidden), "fn a() {".len());
        // A visible index still round-trips exactly.
        let after = src.find("after").expect("visible");
        assert_eq!(m.to_buffer(m.to_display_clamped(after)), after);
    }

    #[test]
    fn shift_heads_is_a_no_op_without_a_line_change() {
        let heads: BTreeSet<usize> = [2, 10].into_iter().collect();
        assert_eq!(shift_heads(&heads, 5, 0, 0), heads);
    }

    // ── Block comments ───────────────────────────────────────────────────────

    fn kinds(src: &str) -> Vec<(usize, usize, RegionKind)> {
        regions(src)
            .into_iter()
            .map(|r| (r.head, r.end, r.kind))
            .collect()
    }

    #[test]
    fn multi_line_block_comment_is_foldable() {
        let src = "/* header\n * text\n */\nfn f() {}\n";
        assert_eq!(kinds(src), [(0, 2, RegionKind::Comment)]);
    }

    #[test]
    fn single_line_block_comment_is_not_foldable() {
        assert!(kinds("let x = 1; /* note */\n").is_empty());
    }

    #[test]
    fn unterminated_block_comment_folds_nothing() {
        // Mid-typing: emitting here would swallow the rest of the file.
        assert!(kinds("/* open\nstill open\n").is_empty());
    }

    #[test]
    fn nested_block_comments_report_the_outer_end() {
        // Stopping at the first `*/` would end the region on line 1 AND leave
        // the scanner reading the tail as code.
        let src = "/* outer\n/* inner */\nstill comment {\n*/\nfn f() {}\n";
        assert_eq!(kinds(src), [(0, 3, RegionKind::Comment)]);
    }

    #[test]
    fn a_comment_after_code_on_the_same_line_still_folds() {
        let src = "let x = 1; /* why\n   this is here */\n";
        assert_eq!(kinds(src), [(0, 1, RegionKind::Comment)]);
    }

    #[test]
    fn a_comment_between_signature_and_brace_keeps_the_fn_kind() {
        let src = "fn f() /* note\n   spanning */ {\n    x;\n}\n";
        let k = kinds(src);
        assert_eq!(k[0], (0, 1, RegionKind::Comment));
        assert_eq!(k[1], (1, 3, RegionKind::Fn));
    }

    #[test]
    fn a_comment_inside_a_function_is_its_own_region() {
        let src = "fn f() {\n    /* a\n       b */\n    x;\n}\n";
        assert_eq!(
            kinds(src),
            [(0, 4, RegionKind::Fn), (1, 2, RegionKind::Comment)]
        );
    }

    #[test]
    fn collapse_all_still_ignores_comments() {
        let src = "/* a\n   b */\nfn f() {\n    x;\n}\n";
        assert_eq!(fn_heads(src).into_iter().collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn folding_a_comment_hides_its_interior() {
        let src = "/* a\n   b\n   c */\nfn f() {}\n";
        let m = folded_of(src, &[0]);
        assert_eq!(m.display(), "/* a\n   c */\nfn f() {}\n");
        assert_eq!(m.line_numbers(), [1, 3, 4, 5]);
    }

    // ── Function bodies / collapse all ───────────────────────────────────────

    fn fns(src: &str) -> Vec<usize> {
        fn_heads(src).into_iter().collect()
    }

    #[test]
    fn only_function_bodies_are_collapse_all_targets() {
        let src = "struct S {\n    x: u8,\n}\n\
                   impl S {\n    fn f(&self) {\n        if x {\n            y;\n        }\n    }\n}\n";
        // The struct body, the impl body and the `if` block are foldable by
        // hand, but only `fn f`'s body is a collapse-all target.
        assert_eq!(fns(src), [4]);
    }

    #[test]
    fn closures_and_match_blocks_are_not_functions() {
        let src = "fn f() {\n    let g = |x| {\n        x\n    };\n    match x {\n        _ => {}\n    }\n}\n";
        assert_eq!(fns(src), [0]);
    }

    #[test]
    fn fn_pointer_type_does_not_mark_the_next_block() {
        // The `;` ends the signature, so the following `impl` block is not a fn.
        let src = "type Cb = fn(u8);\nimpl S {\n    x;\n}\n";
        assert!(fns(src).is_empty());
    }

    #[test]
    fn fn_trait_bound_is_not_the_fn_keyword() {
        // `Fn` is capitalised — only the lowercase keyword counts.
        let src = "impl<T: Fn(u8)> S<T> {\n    y;\n}\n";
        assert!(fns(src).is_empty());
    }

    #[test]
    fn word_boundary_on_the_keyword() {
        let src = "struct fnord {\n    x: u8,\n}\n";
        assert!(fns(src).is_empty());
    }

    #[test]
    fn multi_line_signature_still_marks_the_body() {
        let src = "fn f(\n    a: u8,\n    b: u8,\n) -> u8 {\n    a\n}\n";
        assert_eq!(fns(src), [3]);
    }

    #[test]
    fn toggle_all_collapses_then_clears() {
        let src = "fn a() {\n    x;\n}\nfn b() {\n    y;\n}\n";
        let none = BTreeSet::new();
        let all = toggle_all(src, &none);
        assert_eq!(all.iter().copied().collect::<Vec<_>>(), [0, 3]);
        // Anything folded — even a single hand-folded block — clears.
        assert!(toggle_all(src, &all).is_empty());
        let one: BTreeSet<usize> = [3].into_iter().collect();
        assert!(toggle_all(src, &one).is_empty());
    }

    #[test]
    fn toggle_all_on_a_file_without_functions_stays_empty() {
        let src = "struct S {\n    x: u8,\n}\n";
        assert!(toggle_all(src, &BTreeSet::new()).is_empty());
    }

    // ── Projection ───────────────────────────────────────────────────────────

    #[test]
    fn no_folds_is_the_identity_map() {
        let m = folded_of(FN_SRC, &[]);
        assert!(m.is_identity());
        assert_eq!(m.display(), FN_SRC);
        assert_eq!(m.to_buffer(7), 7);
        assert_eq!(m.to_display(7), Some(7));
    }

    #[test]
    fn folding_hides_the_interior_only() {
        let m = folded_of(FN_SRC, &[0]);
        assert_eq!(m.display(), "fn a() {\n}\nfn b() {}\n");
        // Header, closing brace, and the lines after it survive.
        assert_eq!(m.line_numbers(), [1, 4, 5, 6]);
    }

    #[test]
    fn hidden_positions_have_no_display_index() {
        let m = folded_of(FN_SRC, &[0]);
        let inside = FN_SRC.find("let x").unwrap();
        assert_eq!(m.to_display(inside), None);
    }

    #[test]
    fn round_trip_on_every_visible_offset() {
        let m = folded_of(FN_SRC, &[0]);
        for d in 0..m.display().chars().count() {
            let b = m.to_buffer(d);
            assert_eq!(
                m.to_display(b),
                Some(d),
                "display {d} -> buffer {b} -> back failed"
            );
        }
    }

    #[test]
    fn buffer_offsets_after_a_fold_shift_back() {
        let m = folded_of(FN_SRC, &[0]);
        let b = FN_SRC.find("fn b").unwrap();
        let d = m.to_display(b).expect("visible");
        let disp: String = m.display().chars().skip(d).take(4).collect();
        assert_eq!(disp, "fn b");
    }

    #[test]
    fn nested_folds_union_their_lines() {
        let src = "impl S {\n    fn f() {\n        x;\n    }\n}\nfn g() {}\n";
        let m = folded_of(src, &[0, 1]);
        assert_eq!(m.display(), "impl S {\n}\nfn g() {}\n");
        assert_eq!(m.line_numbers(), [1, 5, 6, 7]);
    }

    #[test]
    fn stale_fold_line_is_ignored() {
        // Line 2 has no block of its own: the code changed under the fold.
        let m = folded_of(FN_SRC, &[2]);
        assert!(m.is_identity());
        assert_eq!(m.display(), FN_SRC);
    }

    #[test]
    fn display_line_of_maps_the_gutter() {
        let m = folded_of(FN_SRC, &[0]);
        assert_eq!(m.display_line_of(0), Some(0)); // header
        assert_eq!(m.display_line_of(1), None); // hidden
        assert_eq!(m.display_line_of(3), Some(1)); // the `}`
    }

    #[test]
    fn map_ranges_drops_hidden_and_shifts_visible() {
        let m = folded_of(FN_SRC, &[0]);
        let hidden = FN_SRC.find("let x").unwrap();
        let visible = FN_SRC.find("fn b").unwrap();
        let out = m.map_ranges(&[(hidden, hidden + 3), (visible, visible + 4)]);
        assert_eq!(out.len(), 1, "the hidden range must be dropped");
        let (s, e) = out[0];
        let got: String = m.display().chars().skip(s).take(e - s).collect();
        assert_eq!(got, "fn b");
    }

    #[test]
    fn hidden_count_is_the_interior() {
        let r = regions(FN_SRC)[0];
        assert_eq!(r.hidden_count(), 2);
        assert_eq!(r.hidden().collect::<Vec<_>>(), [1, 2]);
    }
}
