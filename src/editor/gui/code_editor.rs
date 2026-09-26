//! Lifetime-aware Rust code editor.
//!
//! `egui_code_editor::CodeEditor` is great, but its lexer treats `'` purely as a
//! char-literal quote — so a Rust lifetime like `'a` opens a "string" that never
//! closes until the next `'`, painting the rest of the file in the string colour
//! (the classic green spill). The crate exposes no layouter hook, so we can't fix
//! it from the outside via its `show()`.
//!
//! This module re-implements `CodeEditor::show` / `show_with_completer` (the same
//! frame + numbered-lines column + nested scroll areas, and the SAME scroll-area
//! ids the editor panel's caret-follow code relies on) but feeds the underlying
//! `egui::TextEdit` a highlighter we control: lifetimes get their own blue span,
//! char literals keep the string colour, and nothing spills.
//!
//! Only Rust files use this path; TOML / .gitignore keep the stock `CodeEditor`.

use eframe::egui;
use egui::text_edit::TextEditOutput;
use egui_code_editor::{ColorTheme, Completer, Syntax, TokenType};

/// Rust lifetimes (`'a`, `'static`) — blue, per request (RGB 0,100,255).
const LIFETIME_COLOR: egui::Color32 = egui::Color32::from_rgb(0, 100, 255);

/// The keys the editor keeps while it has focus: Tab indents, the arrows move
/// the caret, and Escape stays in the editor.
///
/// egui's default hands Escape to its focus system, which drops the focused
/// widget before any code runs, and from egui 0.36 on an unfocused `TextEdit`
/// collapses its selection. So an Escape meant for a popup (completion, code
/// actions) also threw the selection away. In the code editor Escape means
/// "dismiss the popup" or "drop the extra carets", never "leave the editor".
pub(crate) const EDITOR_KEYS: egui::EventFilter = egui::EventFilter {
    tab: true,
    horizontal_arrows: true,
    vertical_arrows: true,
    escape: true,
};

/// Character cells reserved to the RIGHT of the line numbers for the fold
/// carets. The number column is the only place with room: the diff bars and the
/// breakpoint dot already fill everything between it and the code.
pub const FOLD_GUTTER_CHARS: usize = 2;

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A lexer position: a byte offset into the source and the char index it is.
///
/// The lexer steps it one char at a time, just as it used to step an index into
/// a `Vec<char>`, so every decision is the same. The byte offset lets a token
/// be sliced out of the source instead of collected into a `String`, and the
/// char index is what the marks are expressed in.
#[derive(Clone, Copy, Default)]
struct At {
    byte: usize,
    char: usize,
}

impl At {
    /// One char further on. `c` must be the char at this position.
    fn past(self, c: char) -> At {
        At {
            byte: self.byte + c.len_utf8(),
            char: self.char + 1,
        }
    }
}

/// The char at `at` (always a char boundary), `None` at the end.
fn char_at(text: &str, at: At) -> Option<char> {
    match text.as_bytes().get(at.byte) {
        None => None,
        Some(&b) if b.is_ascii() => Some(b as char),
        Some(_) => text[at.byte..].chars().next(),
    }
}

/// The char `k` chars after `at`: what `chars.get(p + k)` was.
fn char_after(text: &str, mut at: At, k: usize) -> Option<char> {
    for _ in 0..k {
        at = at.past(char_at(text, at)?);
    }
    char_at(text, at)
}

/// Past the char at `at`, or `at` itself at the end.
fn step(text: &str, at: At) -> At {
    char_at(text, at).map_or(at, |c| at.past(c))
}

/// If a raw string (`r"…"`, `r#"…"#`, `br#"…"#`) starts at `p`, return the
/// position just past its closing delimiter; otherwise `None`. Handles any
/// number of `#`.
fn raw_string_end(text: &str, p: At) -> Option<At> {
    let mut i = p;
    if char_at(text, i) == Some('b') {
        i = i.past('b');
    }
    if char_at(text, i) != Some('r') {
        return None;
    }
    i = i.past('r');
    let mut hashes = 0;
    while char_at(text, i) == Some('#') {
        i = i.past('#');
        hashes += 1;
    }
    if char_at(text, i) != Some('"') {
        return None; // `result`, `r#ident`, … — not a raw string
    }
    i = i.past('"'); // past the opening quote
    while let Some(c) = char_at(text, i) {
        if c == '"' {
            let mut j = i.past('"');
            let mut seen = 0;
            while seen < hashes && char_at(text, j) == Some('#') {
                j = j.past('#');
                seen += 1;
            }
            if seen == hashes {
                return Some(j);
            }
        }
        i = i.past(c);
    }
    Some(i) // unterminated → colour to EOF
}

/// Blend a colour 55% toward neutral gray, for tokens inside a "dead" (never-
/// referenced) span — same technique as a disabled UI control, so unused code
/// reads as visually de-emphasised without losing its shape/structure.
fn fade(color: egui::Color32) -> egui::Color32 {
    const GRAY: (u8, u8, u8) = (120, 120, 120);
    let mix = |a: u8, b: u8| ((a as u16 + b as u16 * 2) / 3) as u8;
    egui::Color32::from_rgb(
        mix(color.r(), GRAY.0),
        mix(color.g(), GRAY.1),
        mix(color.b(), GRAY.2),
    )
}

/// Char-index `[start, end)` spans the analyses want emphasised, on top of the
/// plain syntax colouring. A token belongs to a span when its START index falls
/// inside it (tokens never straddle a boundary in practice).
#[derive(Default, Clone, Copy)]
pub struct Marks<'a> {
    /// `true` while the editor shows a FOLDED projection of the buffer: the
    /// text on screen is missing whole lines, so the widget is made
    /// non-interactive.
    ///
    /// Implemented by handing the widget an IMMUTABLE buffer, not by
    /// `interactive(false)` — see the render site for why that difference cost
    /// three failed fixes. egui refuses every edit at the source, while the
    /// editor keeps focus, clicks, selection and scrolling.
    ///
    /// The caller still has work to do: a caret placed in the projection is an
    /// index into a SHORTER string, so it is translated back through
    /// `FoldMap::to_buffer` when a keystroke unfolds the file, and the widget's
    /// undo history is cleared at the same moment — egui snapshots whatever text
    /// it is shown, and a later Ctrl+Z would write the projection over the
    /// file.
    pub read_only: bool,
    /// De-emphasised (faded toward gray): never-referenced fn/struct/enum/const
    /// from the usages analysis, unused locals, unused generic parameters.
    pub dead: &'a [(usize, usize)],
    /// Underlined in their own colour, NOT faded: generic parameters an item
    /// declares without using, which an `impl` of it does use. They are live
    /// code — the underline links the declaration to the `impl` that needs it.
    pub underline: &'a [(usize, usize)],
}

/// How one token is marked, resolved from its start index.
#[derive(Default, Clone, Copy)]
struct TokenMark {
    dead: bool,
    underline: bool,
}

/// Whether a set of char ranges covers an index, asked for indices that never
/// decrease — the lexer asks at each token start, in order.
///
/// The ranges are walked once, sorted by start: an index is covered exactly
/// when some range that starts at or before it ends after it, i.e. when the
/// furthest end among the ranges started so far lies beyond it.
struct Coverage<'r> {
    ranges: std::borrow::Cow<'r, [(usize, usize)]>,
    next: usize,
    reach: usize,
}

impl<'r> Coverage<'r> {
    fn new(ranges: &'r [(usize, usize)]) -> Self {
        let ranges = if ranges.is_sorted_by_key(|r| r.0) {
            std::borrow::Cow::Borrowed(ranges)
        } else {
            let mut sorted = ranges.to_vec();
            sorted.sort_unstable_by_key(|r| r.0);
            std::borrow::Cow::Owned(sorted)
        };
        Self {
            ranges,
            next: 0,
            reach: 0,
        }
    }

    /// `ranges.iter().any(|&(s, e)| at >= s && at < e)`, for a non-decreasing `at`.
    fn covers(&mut self, at: usize) -> bool {
        while let Some(&(s, e)) = self.ranges.get(self.next)
            && s <= at
        {
            self.reach = self.reach.max(e);
            self.next += 1;
        }
        at < self.reach
    }
}

/// The token classes the lexer tells apart, each drawn in one colour.
#[derive(Clone, Copy)]
enum Class {
    Whitespace,
    LineComment,
    BlockComment,
    /// Strings, byte strings and raw strings.
    Str,
    /// Char literals.
    Char,
    Lifetime,
    Numeric,
    Function,
    Keyword,
    Type,
    Special,
    Literal,
    Punctuation,
}

const CLASSES: usize = Class::Punctuation as usize + 1;

/// The `TextFormat` of each (class, mark) pair, made on first use and cloned for
/// every later token, instead of one colour lookup and format per token.
struct Formats<'t> {
    theme: &'t ColorTheme,
    font: egui::FontId,
    /// Indexed by class, then by `dead | underline << 1`.
    made: [[Option<egui::TextFormat>; 4]; CLASSES],
}

impl<'t> Formats<'t> {
    fn new(theme: &'t ColorTheme, fontsize: f32) -> Self {
        Self {
            theme,
            font: egui::FontId::monospace(fontsize),
            made: std::array::from_fn(|_| Default::default()),
        }
    }

    fn get(&mut self, class: Class, mark: TokenMark) -> egui::TextFormat {
        let (theme, font) = (self.theme, &self.font);
        let slot = &mut self.made[class as usize]
            [usize::from(mark.dead) | usize::from(mark.underline) << 1];
        slot.get_or_insert_with(|| {
            // `type_color` ignores the char a `TokenType` carries, so one colour
            // per class is the colour of every token in it (pinned by a test).
            let color = match class {
                Class::Whitespace => theme.type_color(TokenType::Whitespace(' ')),
                Class::LineComment => theme.type_color(TokenType::Comment(false)),
                Class::BlockComment => theme.type_color(TokenType::Comment(true)),
                Class::Str => theme.type_color(TokenType::Str('"')),
                Class::Char => theme.type_color(TokenType::Str('\'')),
                Class::Lifetime => LIFETIME_COLOR,
                Class::Numeric => theme.type_color(TokenType::Numeric(false)),
                Class::Function => theme.type_color(TokenType::Function),
                Class::Keyword => theme.type_color(TokenType::Keyword),
                Class::Type => theme.type_color(TokenType::Type),
                Class::Special => theme.type_color(TokenType::Special),
                Class::Literal => theme.type_color(TokenType::Literal),
                Class::Punctuation => theme.type_color(TokenType::Punctuation('.')),
            };
            let color = if mark.dead { fade(color) } else { color };
            let mut fmt = egui::TextFormat::simple(font.clone(), color);
            if mark.underline {
                // Drawn in the token's OWN colour, so it reads as a property of
                // the identifier rather than a foreign marker pasted over it.
                fmt.underline = egui::Stroke::new(1.0_f32, color);
            }
            fmt
        })
        .clone()
    }
}

/// Build a syntax-highlighted `LayoutJob` for Rust source. Mirrors the colours of
/// `egui_code_editor`'s built-in highlighter (keyword / type / special sets come
/// from the same `Syntax`, colours from the same `ColorTheme`) but is lifetime-
/// aware: a `'a` / `'static` lifetime is its own blue span instead of a runaway
/// char-literal string. Char literals (`'x'`, `'\n'`) keep the string colour.
///
/// `marks` carries the analysis overlays — see [`Marks`].
///
/// The job text equals the source exactly (every char appended in order), which
/// egui requires for correct cursor / selection mapping.
pub(crate) fn rust_layout_job(
    text: &str,
    theme: &ColorTheme,
    fontsize: f32,
    syntax: &Syntax,
    marks: Marks<'_>,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut formats = Formats::new(theme, fontsize);

    // A token is classified by its START index — it never straddles a mark
    // boundary in practice (fn/struct/etc. spans start and end at statement
    // boundaries, and a generic parameter's mark is exactly its identifier).
    let mut dead = Coverage::new(marks.dead);
    let mut underline = Coverage::new(marks.underline);

    let mut push = |job: &mut egui::text::LayoutJob, s: At, e: At, class: Class| {
        let mark = TokenMark {
            dead: dead.covers(s.char),
            underline: underline.covers(s.char),
        };
        if s.byte < e.byte {
            job.append(&text[s.byte..e.byte], 0.0, formats.get(class, mark));
        }
    };

    let mut p = At::default();
    while let Some(c) = char_at(text, p) {
        // ── Whitespace ──
        if c.is_whitespace() {
            let s = p;
            while let Some(c) = char_at(text, p)
                && c.is_whitespace()
            {
                p = p.past(c);
            }
            push(&mut job, s, p, Class::Whitespace);
            continue;
        }
        // ── Line comment `// …` ──
        if c == '/' && char_after(text, p, 1) == Some('/') {
            let s = p;
            while let Some(c) = char_at(text, p)
                && c != '\n'
            {
                p = p.past(c);
            }
            push(&mut job, s, p, Class::LineComment);
            continue;
        }
        // ── Block comment `/* … */` ──
        if c == '/' && char_after(text, p, 1) == Some('*') {
            let s = p;
            p = p.past('/').past('*');
            // The char before `p` is `*` exactly when the byte before it is:
            // the last byte of a multi-byte char is never ASCII.
            while let Some(c) = char_at(text, p)
                && !(text.as_bytes()[p.byte - 1] == b'*' && c == '/')
            {
                p = p.past(c);
            }
            p = step(text, p); // include the closing '/'
            push(&mut job, s, p, Class::BlockComment);
            continue;
        }
        // ── Raw string `r"…"` / `r#"…"#` / `br#"…"#` ──
        if let Some(end) = raw_string_end(text, p) {
            push(&mut job, p, end, Class::Str);
            p = end;
            continue;
        }
        // ── String `"…"` / byte string `b"…"` ──
        if c == '"' || (c == 'b' && char_after(text, p, 1) == Some('"')) {
            let s = p;
            if c == 'b' {
                p = p.past('b');
            }
            p = p.past('"'); // opening quote
            while let Some(c) = char_at(text, p) {
                match c {
                    '\\' => p = step(text, p.past('\\')), // skip escaped char
                    '"' => {
                        p = p.past('"');
                        break;
                    }
                    _ => p = p.past(c),
                }
            }
            push(&mut job, s, p, Class::Str);
            continue;
        }
        // ── Char literal vs lifetime (both start with `'`) ──
        if c == '\'' {
            // Escaped char literal: '\n' '\'' '\\' '\u{1F}' …
            if char_after(text, p, 1) == Some('\\') {
                let s = p;
                p = p.past('\'').past('\\');
                while let Some(c) = char_at(text, p)
                    && c != '\''
                {
                    p = p.past(c);
                }
                p = step(text, p);
                push(&mut job, s, p, Class::Char);
                continue;
            }
            // Lifetime: `'` + identifier, and NOT a single-char literal `'x'`
            // (those have a closing `'` two chars along).
            let next_is_ident = char_after(text, p, 1).is_some_and(is_ident_start);
            if next_is_ident && char_after(text, p, 2) != Some('\'') {
                let s = p;
                p = p.past('\'');
                while let Some(c) = char_at(text, p)
                    && is_ident(c)
                {
                    p = p.past(c);
                }
                push(&mut job, s, p, Class::Lifetime);
                continue;
            }
            // Plain char literal `'x'`.
            let s = p;
            p = p.past('\'');
            p = step(text, p); // the char
            if char_at(text, p) == Some('\'') {
                p = p.past('\''); // closing quote
            }
            push(&mut job, s, p, Class::Char);
            continue;
        }
        // ── Number ──
        if c.is_ascii_digit() {
            let s = p;
            p = p.past(c);
            // integer body (also hex/oct/bin digits, `_`, type suffix letters)
            while let Some(c) = char_at(text, p)
                && (c.is_alphanumeric() || c == '_')
            {
                p = p.past(c);
            }
            // float fraction `.123` — but never consume a `..` range operator.
            if char_at(text, p) == Some('.')
                && char_after(text, p, 1).is_some_and(|c| c.is_ascii_digit())
            {
                p = p.past('.');
                while let Some(c) = char_at(text, p)
                    && (c.is_alphanumeric() || c == '_')
                {
                    p = p.past(c);
                }
            }
            push(&mut job, s, p, Class::Numeric);
            continue;
        }
        // ── Identifier / keyword / type / special / function ──
        if is_ident_start(c) {
            let s = p;
            while let Some(c) = char_at(text, p)
                && is_ident(c)
            {
                p = p.past(c);
            }
            let word = &text[s.byte..p.byte];
            let class = if char_at(text, p) == Some('(') {
                Class::Function
            } else if syntax.is_keyword(word) {
                Class::Keyword
            } else if syntax.is_type(word) {
                Class::Type
            } else if syntax.is_special(word) {
                Class::Special
            } else {
                Class::Literal
            };
            push(&mut job, s, p, class);
            continue;
        }
        // ── Punctuation / anything else (one char) ──
        let s = p;
        p = p.past(c);
        push(&mut job, s, p, Class::Punctuation);
    }
    job
}

// ── Layout-job memo ───────────────────────────────────────────────────────────

/// How many layout jobs to keep memoized. Was ONE while a single editor was
/// ever visible — but with a second (Reference) editor open on a different
/// file, one slot THRASHES: each editor misses the other's key every frame, so
/// both files get fully re-tokenized at the repaint rate. That is precisely the
/// cost this memo exists to remove, so the capacity has to exceed the number of
/// simultaneously visible editors.
const LAYOUT_MEMO_SLOTS: usize = 4;

thread_local! {
    /// Small memo for [`rust_layout_job`]. egui's `TextEdit` calls the
    /// layouter EVERY frame, and while any spinner is on screen the app
    /// repaints continuously — so the whole file was re-tokenized at 60+ FPS
    /// for the entire duration of Saving/Checking/Flashing (the dominant
    /// per-frame CPU / energy cost). UI-thread only.
    ///
    /// Most-recently-used first; the tail is dropped when full, so switching
    /// files or zooming costs one recompute rather than evicting a live editor.
    static LAYOUT_MEMO: std::cell::RefCell<Vec<(u64, egui::text::LayoutJob)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// [`rust_layout_job`] memoized on (text, font size, dead ranges, theme name).
/// The `.rs` editor always uses the Rust `Syntax`, so it isn't part of the key.
/// The remaining per-frame work is one job clone (memcpy) + egui's own galley
/// cache lookup — no tokenization.
fn cached_rust_layout_job(
    text: &str,
    theme: &ColorTheme,
    fontsize: f32,
    syntax: &Syntax,
    marks: Marks<'_>,
) -> egui::text::LayoutJob {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    fontsize.to_bits().hash(&mut h);
    marks.dead.hash(&mut h);
    marks.underline.hash(&mut h);
    theme.name.hash(&mut h);
    let key = h.finish().max(1); // 0 is the "nothing memoized yet" sentinel

    LAYOUT_MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if let Some(pos) = m.iter().position(|(k, _)| *k == key) {
            // Promote to front so two alternating editors both stay resident.
            if pos != 0 {
                let hit = m.remove(pos);
                m.insert(0, hit);
            }
            return m[0].1.clone();
        }
        let job = rust_layout_job(text, theme, fontsize, syntax, marks);
        m.insert(0, (key, job.clone()));
        m.truncate(LAYOUT_MEMO_SLOTS);
        job
    })
}

/// The numbered-lines gutter, faithfully ported from `CodeEditor::numlines_show`
/// (we never use the shift / only-natural options, so they're dropped).
/// Which rendered rows are the HEADER of a collapsed block, from the line
/// numbers alone — no extra state needed. The rows are consecutive buffer lines
/// except across a fold, so a row whose number is not one less than the next
/// row's is exactly the line holding the `{` of a block whose body is hidden.
///
/// An empty `numbers` means nothing is folded (the editor is showing the whole
/// buffer and counts its own rows), so there are no gaps and no headers.
fn folded_header_rows(numbers: &[usize]) -> std::collections::BTreeSet<usize> {
    numbers
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[1] > w[0] + 1)
        .map(|(i, _)| i)
        .collect()
}

/// How many rows the number column counts for `text`: `text.lines().count()`,
/// plus one when the text is empty or ends in a newline, which is exactly one
/// more than its number of `'\n'`s.
fn row_count(text: &str) -> usize {
    let mut rows = 1;
    let mut rest = text;
    // Line by line through `find`, which reaches core's memchr, rather than a
    // per-byte loop that the dev build would leave unoptimised.
    while let Some(nl) = rest.find('\n') {
        rows += 1;
        rest = &rest[nl + 1..];
    }
    rows
}

/// The number column's text for `total` rows, or for the explicit `numbers`
/// while folded, and the width in digits of its widest number.
fn number_column_text(total: usize, rows: usize, numbers: &[usize]) -> (String, usize) {
    use std::fmt::Write;
    // The column is sized by the WIDEST number shown, which while folded is the
    // last buffer line, not the row count.
    let max_indent = numbers
        .last()
        .copied()
        .unwrap_or(total)
        .max(total)
        .to_string()
        .len();
    // Two trailing blanks widen the column past the numbers, reserving the strip
    // the fold carets are drawn in (`fold_ui`). Without it there is nowhere to
    // put them: the numbers run to `gp.x - 12` and the diff bars + breakpoint dot
    // own everything from there to the text.
    let gutter = FOLD_GUTTER_CHARS;
    let row_len = max_indent + gutter + 1;
    let mut out;
    if numbers.is_empty() {
        out = String::with_capacity(total * row_len);
        for n in 1..=total {
            if n > 1 {
                out.push('\n');
            }
            let _ = write!(out, "{n:>max_indent$}{:gutter$}", "");
        }
    } else {
        out = String::with_capacity(numbers.len().max(rows) * row_len);
        for (i, n) in numbers.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let _ = write!(out, "{n:>max_indent$}{:gutter$}", "");
        }
        // Trailing blanks keep the column as tall as `desired_rows`.
        for _ in numbers.len()..rows {
            out.push('\n');
            let _ = write!(out, "{:max_indent$}", "");
        }
    }
    (out, max_indent)
}

/// The number column's layout: one section, or while folded one per row, so a
/// collapsed block's header number can take the brace colour.
fn number_column_job(
    counter: &str,
    fontsize: f32,
    plain: egui::Color32,
    folded_fg: egui::Color32,
    folded_rows: &std::collections::BTreeSet<usize>,
) -> egui::text::LayoutJob {
    let font = egui::FontId::monospace(fontsize);
    if folded_rows.is_empty() {
        egui::text::LayoutJob::single_section(
            counter.to_string(),
            egui::TextFormat::simple(font, plain),
        )
    } else {
        let mut job = egui::text::LayoutJob::default();
        for (row, line) in counter.split('\n').enumerate() {
            if row > 0 {
                job.append("\n", 0.0, egui::TextFormat::simple(font.clone(), plain));
            }
            let fg = if folded_rows.contains(&row) {
                folded_fg
            } else {
                plain
            };
            job.append(line, 0.0, egui::TextFormat::simple(font.clone(), fg));
        }
        job
    }
}

/// How many views' number columns to keep, for the same reason as
/// [`LAYOUT_MEMO_SLOTS`].
const NUMBER_COLUMN_SLOTS: usize = 4;

/// One view's number column, rebuilt only when what it shows changes.
///
/// The column is drawn every frame, and building it cost a newline count over
/// the whole buffer plus a few allocations per line, while its text only
/// changes with the row count, `rows` or the fold numbers.
struct NumberColumn {
    /// The editor id the column belongs to.
    id: String,
    /// The text whose rows were counted last, and that count. Kept as a copy so
    /// an unchanged buffer costs one comparison.
    text: String,
    text_rows: usize,
    /// The `(total, rows)` and `numbers` the column was built for.
    built_for: Option<(usize, usize)>,
    numbers: Vec<usize>,
    counter: std::rc::Rc<str>,
    max_indent: usize,
    folded_rows: std::collections::BTreeSet<usize>,
    /// The layout of `counter`, keyed on (font size bits, plain, folded colour).
    job: Option<((u32, egui::Color32, egui::Color32), egui::text::LayoutJob)>,
}

thread_local! {
    /// Per-view [`NumberColumn`]s, most recently used first. UI-thread only.
    static NUMBER_COLUMNS: std::cell::RefCell<Vec<NumberColumn>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

impl NumberColumn {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            text: String::new(),
            text_rows: row_count(""),
            built_for: None,
            numbers: Vec::new(),
            counter: "".into(),
            max_indent: 0,
            folded_rows: Default::default(),
            job: None,
        }
    }

    fn update(
        &mut self,
        text: &str,
        rows: usize,
        numbers: &[usize],
        fontsize: f32,
        plain: egui::Color32,
        folded_fg: egui::Color32,
    ) -> (std::rc::Rc<str>, usize, egui::text::LayoutJob) {
        if self.text != text {
            self.text.clear();
            self.text.push_str(text);
            self.text_rows = row_count(text);
        }
        let total = self.text_rows.max(rows);
        if self.built_for != Some((total, rows)) || self.numbers != numbers {
            let (counter, max_indent) = number_column_text(total, rows, numbers);
            self.counter = counter.into();
            self.max_indent = max_indent;
            self.numbers.clear();
            self.numbers.extend_from_slice(numbers);
            self.folded_rows = folded_header_rows(numbers);
            self.built_for = Some((total, rows));
            self.job = None;
        }
        let key = (fontsize.to_bits(), plain, folded_fg);
        let job = match &self.job {
            Some((built, job)) if *built == key => job.clone(),
            _ => {
                let job =
                    number_column_job(&self.counter, fontsize, plain, folded_fg, &self.folded_rows);
                self.job = Some((key, job.clone()));
                job
            }
        };
        (std::rc::Rc::clone(&self.counter), self.max_indent, job)
    }
}

/// View `id`'s column text, its width in digits and its layout: what
/// [`number_column_text`] and [`number_column_job`] build for `text`, memoized.
fn number_column(
    id: &str,
    text: &str,
    rows: usize,
    numbers: &[usize],
    fontsize: f32,
    plain: egui::Color32,
    folded_fg: egui::Color32,
) -> (std::rc::Rc<str>, usize, egui::text::LayoutJob) {
    NUMBER_COLUMNS.with(|m| {
        let mut m = m.borrow_mut();
        match m.iter().position(|c| c.id == id) {
            Some(0) => {}
            Some(pos) => {
                let hit = m.remove(pos);
                m.insert(0, hit);
            }
            None => {
                m.insert(0, NumberColumn::new(id));
                m.truncate(NUMBER_COLUMN_SLOTS);
            }
        }
        m[0].update(text, rows, numbers, fontsize, plain, folded_fg)
    })
}

fn numlines_show(
    ui: &mut egui::Ui,
    text: &str,
    theme: &ColorTheme,
    fontsize: f32,
    rows: usize,
    id: &str,
    // Explicit 1-based numbers, one per rendered row, when the text on screen is
    // a FOLDED projection of the buffer — the rows are then 1, 2, 40, 41, … and
    // counting them would be a lie. Empty = the text is the whole buffer.
    numbers: &[usize],
) {
    use egui::TextBuffer;

    let plain = theme.type_color(TokenType::Comment(true));
    // The same orange the highlighter gives `{` and `}` — the number picks up
    // the colour of the braces whose contents it is standing in for.
    let folded_fg = theme.type_color(TokenType::Punctuation('{'));
    let (counter, max_indent, job) =
        number_column(id, text, rows, numbers, fontsize, plain, folded_fg);

    let width = (max_indent + FOLD_GUTTER_CHARS) as f32 * fontsize * 0.5;

    let mut prepared = Some(job);
    let mut layouter = |ui: &egui::Ui, buf: &dyn TextBuffer, _wrap: f32| {
        let job = match prepared.take() {
            Some(job) if job.text == buf.as_str() => job,
            _ => number_column_job(
                buf.as_str(),
                fontsize,
                plain,
                folded_fg,
                &folded_header_rows(numbers),
            ),
        };
        ui.fonts_mut(|f| f.layout_job(job))
    };
    // An immutable buffer, so the column is not copied into a `String` every
    // frame. The widget is non-interactive with its own frame, and egui only
    // reads `is_mutable()` for an interactive widget or its default frame, so
    // it draws the same.
    let mut view: &str = &counter;
    ui.add(
        egui::TextEdit::multiline(&mut view)
            .id_source(format!("{id}_numlines"))
            .font(egui::TextStyle::Monospace)
            .interactive(false)
            .frame(egui::Frame::NONE)
            .desired_rows(rows)
            .desired_width(width)
            .layouter(&mut layouter),
    );
}

/// Render the Rust code editor (theme frame + line numbers + nested scroll areas
/// + the lifetime-aware highlighter). The scroll-area `id_salt`s match the stock
/// `CodeEditor` (`{id}_outer_scroll` / `{id}_inner_scroll`) so the editor panel's
/// caret-follow / scroll-to-line code keeps working unchanged.
fn show_rust_editor(
    ui: &mut egui::Ui,
    text: &mut dyn egui::TextBuffer,
    theme: &ColorTheme,
    fontsize: f32,
    rows: usize,
    syntax: &Syntax,
    id: &str,
    marks: Marks<'_>,
    line_numbers: &[usize],
) -> TextEditOutput {
    let mut out: Option<TextEditOutput> = None;
    let code_editor = |ui: &mut egui::Ui| {
        egui::Frame::new().fill(theme.bg()).show(ui, |ui| {
            ui.horizontal_top(|h| {
                theme.modify_style(h, fontsize);
                numlines_show(h, text.as_str(), theme, fontsize, rows, id, line_numbers);
                egui::ScrollArea::horizontal()
                    .id_salt(format!("{id}_inner_scroll"))
                    .show(h, |ui| {
                        let mut layouter =
                            |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                                let job = cached_rust_layout_job(
                                    buf.as_str(),
                                    theme,
                                    fontsize,
                                    syntax,
                                    marks,
                                );
                                ui.fonts_mut(|f| f.layout_job(job))
                            };
                        // Read-only is expressed by handing the widget an
                        // IMMUTABLE buffer, never by `interactive(false)`.
                        //
                        // That looked like the obvious way to do it and is a
                        // trap: a non-interactive TextEdit gets `Sense::hover()`,
                        // which is not focusable, so egui surrenders its focus
                        // every frame AND it can no longer be clicked back into
                        // focus. Since event handling is gated on
                        // `interactive && has_focus`, losing focus that way is
                        // terminal — the editor stays dead even after it becomes
                        // mutable again, which is how folding a block made a
                        // whole file untypable.
                        //
                        // `&str` implements `TextBuffer` with
                        // `is_mutable() == false`: the widget keeps focus,
                        // clicks and selection, and refuses every edit at the
                        // source.
                        let output = if marks.read_only {
                            let frozen = text.as_str().to_owned();
                            let mut view: &str = &frozen;
                            egui::TextEdit::multiline(&mut view)
                                .id_source(id)
                                .event_filter(EDITOR_KEYS)
                                .desired_rows(rows)
                                .desired_width(f32::INFINITY)
                                .layouter(&mut layouter)
                                .show(ui)
                        } else {
                            egui::TextEdit::multiline(text)
                                .id_source(id)
                                .event_filter(EDITOR_KEYS)
                                .desired_rows(rows)
                                .desired_width(f32::INFINITY)
                                .layouter(&mut layouter)
                                .show(ui)
                        };
                        // The TextEdit's own `request_focus` (each pass of a
                        // drag-select, a fresh click) resets egui's focus
                        // filter to the default, which hands Escape back to
                        // egui: put the editor's keys back before the next
                        // pass reads them. It only applies to a widget that
                        // already had focus, so it takes it from nothing.
                        ui.memory_mut(|m| m.set_focus_lock_filter(output.response.id, EDITOR_KEYS));
                        out = Some(output);
                    });
            });
        });
    };
    egui::ScrollArea::vertical()
        .id_salt(format!("{id}_outer_scroll"))
        .show(ui, code_editor);
    out.expect("TextEditOutput should exist at this point")
}

/// Rust editor + keyword auto-completer — the drop-in replacement for
/// `CodeEditor::show_with_completer` used for `.rs` files.
///
/// `suppress_keyword_completer` hides the crate's built-in keyword popup (and its
/// key handling) for this frame — set while our LSP completion popup is open so
/// the two don't stack on top of each other (the LSP popup wins).
#[allow(clippy::too_many_arguments)]
/// The Rust editor WITHOUT the keyword completer or dead-code fading — the
/// second (Reference) editor.
///
/// Same widget, highlighter and gutter as the main editor, so the two views
/// read alike. LSP completion IS wired for this editor (through
/// `handle_editor_completion`, tagged with `EditorSlot::Reference`); what is
/// missing here is the crate's built-in KEYWORD completer.
///
/// **Do not "unify" this with [`show_rust_with_completer`].** `AppIde` holds a
/// single `Completer`, and it stores `completer.text_edit_id` — two editors
/// going through that path would overwrite each other's id every frame and the
/// keyword popup would attach to whichever rendered last.
pub fn show_rust_editor_plain(
    ui: &mut egui::Ui,
    text: &mut dyn egui::TextBuffer,
    fontsize: f32,
    rows: usize,
    id: &str,
) -> TextEditOutput {
    show_rust_editor(
        ui,
        text,
        &ColorTheme::GRUVBOX,
        fontsize,
        rows,
        &Syntax::rust(),
        id,
        Marks::default(),
        &[],
    )
}

pub fn show_rust_with_completer(
    ui: &mut egui::Ui,
    text: &mut dyn egui::TextBuffer,
    theme: &ColorTheme,
    fontsize: f32,
    rows: usize,
    syntax: &Syntax,
    id: &str,
    completer: &mut Completer,
    suppress_keyword_completer: bool,
    marks: Marks<'_>,
    line_numbers: &[usize],
) -> TextEditOutput {
    if !suppress_keyword_completer {
        completer.handle_input(ui.ctx());
    }
    let mut out = show_rust_editor(
        ui,
        text,
        theme,
        fontsize,
        rows,
        syntax,
        id,
        marks,
        line_numbers,
    );
    completer.text_edit_id = Some(out.response.id);
    if !suppress_keyword_completer {
        completer.show(syntax, theme, fontsize, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heads(numbers: &[usize]) -> Vec<usize> {
        folded_header_rows(numbers).into_iter().collect()
    }

    #[test]
    fn nothing_folded_colours_no_line_number() {
        // Empty = the editor is counting its own rows, no projection.
        assert!(heads(&[]).is_empty());
        assert!(heads(&[1, 2, 3, 4]).is_empty());
    }

    #[test]
    fn the_row_before_the_gap_is_the_folded_header() {
        // `fn a() {` on line 10 with lines 11..=39 hidden: row 0 shows 10,
        // row 1 shows 40 (the `}`). Row 0 is the header.
        assert_eq!(heads(&[10, 40, 41]), [0]);
    }

    #[test]
    fn two_folds_give_two_headers() {
        // 1, [2..4 hidden], 5, 6, [7..8 hidden], 9
        assert_eq!(heads(&[1, 5, 6, 9]), [0, 2]);
    }

    #[test]
    fn a_fold_hiding_a_single_line_still_counts() {
        assert_eq!(heads(&[3, 5]), [0]);
    }

    #[test]
    fn a_header_on_the_last_numbered_row_is_not_reported() {
        // There is always a visible `}` line after the hidden body, so this
        // cannot happen — but the window walk must not index past the end.
        assert!(heads(&[7]).is_empty());
    }

    /// The memo must return exactly what a direct call produces, and any input
    /// change (text, dead ranges, font size) must recompute — never serve the
    /// previous entry.
    #[test]
    fn cached_layout_job_matches_direct_and_tracks_inputs() {
        let theme = ColorTheme::GRUVBOX;
        let syn = Syntax::rust();
        let src = "fn main() { let x = 1; }";

        let direct = rust_layout_job(src, &theme, 13.0, &syn, Marks::default());
        let cached = cached_rust_layout_job(src, &theme, 13.0, &syn, Marks::default());
        let repeat = cached_rust_layout_job(src, &theme, 13.0, &syn, Marks::default()); // memo hit
        assert_eq!(direct, cached);
        assert_eq!(cached, repeat);

        // Changed inputs must not return the stale memo.
        let faded = cached_rust_layout_job(
            src,
            &theme,
            13.0,
            &syn,
            Marks {
                dead: &[(0, 5)],
                ..Default::default()
            },
        );
        assert_ne!(faded, cached, "dead range must change the job");
        let zoomed = cached_rust_layout_job(src, &theme, 15.0, &syn, Marks::default());
        assert_ne!(zoomed, cached, "font size must change the job");
        let edited = cached_rust_layout_job("fn main() {}", &theme, 13.0, &syn, Marks::default());
        assert_ne!(edited, cached, "text must change the job");
    }

    /// Two editors alternating on DIFFERENT files must both stay memoized.
    ///
    /// With the old single-slot memo each one evicted the other every frame, so
    /// both files were fully re-tokenized at the repaint rate — the exact cost
    /// the memo exists to remove. Guards the second (Reference) editor.
    #[test]
    fn two_alternating_files_both_stay_memoized() {
        let theme = ColorTheme::GRUVBOX;
        let syn = Syntax::rust();
        let a = "fn a() { let x = 1; }";
        let b = "fn b() { let y = 2; }";

        // Prime both, then alternate the way two visible editors would.
        let ja = cached_rust_layout_job(a, &theme, 13.0, &syn, Marks::default());
        let jb = cached_rust_layout_job(b, &theme, 13.0, &syn, Marks::default());
        for _ in 0..4 {
            assert_eq!(
                cached_rust_layout_job(a, &theme, 13.0, &syn, Marks::default()),
                ja
            );
            assert_eq!(
                cached_rust_layout_job(b, &theme, 13.0, &syn, Marks::default()),
                jb
            );
        }
        // Both must be resident simultaneously — not one evicting the other.
        let resident = LAYOUT_MEMO.with(|m| m.borrow().len());
        assert!(resident >= 2, "only {resident} entr(y/ies) memoized");
    }

    /// Collect (segment_text, is_lifetime_blue) from a highlighted job so tests
    /// can assert which spans were coloured as lifetimes without a real UI.
    fn spans(src: &str) -> Vec<(String, bool)> {
        let job = rust_layout_job(
            src,
            &ColorTheme::GRUVBOX,
            13.0,
            &Syntax::rust(),
            Marks::default(),
        );
        job.sections
            .iter()
            .map(|s| {
                let txt = job.text[s.byte_range.start.0..s.byte_range.end.0].to_string();
                (txt, s.format.color == LIFETIME_COLOR)
            })
            .collect()
    }

    /// The job text must reproduce the source exactly (egui relies on it).
    fn assert_roundtrip(src: &str) {
        let job = rust_layout_job(
            src,
            &ColorTheme::GRUVBOX,
            13.0,
            &Syntax::rust(),
            Marks::default(),
        );
        assert_eq!(job.text, src, "layout job text must equal source");
    }

    /// A dead range dims a token's colour without changing the job's text.
    #[test]
    fn dead_range_fades_color_without_changing_text() {
        let src = "fn dead() {}\nfn used() {}";
        let job = rust_layout_job(
            src,
            &ColorTheme::GRUVBOX,
            13.0,
            &Syntax::rust(),
            Marks {
                dead: &[(0, 12)],
                ..Default::default()
            },
        );
        assert_eq!(
            job.text, src,
            "dead ranges must not change the rendered text"
        );
        // The "dead" token (first `fn`, inside [0,12)) must not use the normal
        // keyword colour; the "used" token (after the dead range) must.
        let normal_kw = ColorTheme::GRUVBOX.type_color(TokenType::Keyword);
        let mut saw_dead_fn = false;
        let mut saw_live_fn = false;
        for s in &job.sections {
            let txt = &job.text[s.byte_range.start.0..s.byte_range.end.0];
            if txt == "fn" {
                if s.byte_range.start.0 < 12 {
                    assert_ne!(s.format.color, normal_kw, "dead `fn` must be faded");
                    saw_dead_fn = true;
                } else {
                    assert_eq!(s.format.color, normal_kw, "live `fn` keeps its colour");
                    saw_live_fn = true;
                }
            }
        }
        assert!(saw_dead_fn && saw_live_fn, "both `fn` tokens must be found");
    }

    #[test]
    fn lifetime_is_blue_char_literal_is_not() {
        let src = "fn f<'a>(x: &'a mut [u8]) -> &'a str { 'a' }";
        assert_roundtrip(src);
        let blue: Vec<String> = spans(src)
            .into_iter()
            .filter(|(_, b)| *b)
            .map(|(t, _)| t)
            .collect();
        // The three `'a` lifetimes are blue; the `'a'` char literal is not.
        assert_eq!(
            blue,
            vec!["'a", "'a", "'a"],
            "only lifetimes are blue: {blue:?}"
        );
    }

    #[test]
    fn static_lifetime_and_no_spill_after() {
        let src = "const N: &'static str = \"hi\";\nlet x = 5;";
        assert_roundtrip(src);
        let blue: Vec<String> = spans(src)
            .into_iter()
            .filter(|(_, b)| *b)
            .map(|(t, _)| t)
            .collect();
        assert_eq!(blue, vec!["'static"], "static lifetime blue, nothing else");
    }

    #[test]
    fn range_is_not_a_float() {
        // `0..16` must not be swallowed into one numeric token.
        let src = "for i in 0..16 {}";
        assert_roundtrip(src);
        let texts: Vec<String> = spans(src).into_iter().map(|(t, _)| t).collect();
        assert!(texts.iter().any(|t| t == "0"), "0 separate: {texts:?}");
        assert!(texts.iter().any(|t| t == "16"), "16 separate: {texts:?}");
        assert!(
            !texts.iter().any(|t| t.contains("0..16")),
            "no merged range"
        );
    }

    // ── Equivalence with the implementations these replaced ──────────────────

    /// The tokenizer and number column as they were before they were made
    /// cheaper, verbatim, so the new ones can be compared against them.
    mod old {
        use super::super::*;

        fn slice(chars: &[char], a: usize, b: usize) -> String {
            chars[a..b].iter().collect()
        }

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
                return None; // `result`, `r#ident`, … — not a raw string
            }
            i += 1; // past the opening quote
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
            Some(chars.len()) // unterminated → colour to EOF
        }

        pub(super) fn rust_layout_job(
            text: &str,
            theme: &ColorTheme,
            fontsize: f32,
            syntax: &Syntax,
            marks: Marks<'_>,
        ) -> egui::text::LayoutJob {
            let mut job = egui::text::LayoutJob::default();
            let font = egui::FontId::monospace(fontsize);
            let chars: Vec<char> = text.chars().collect();
            let n = chars.len();

            let in_dead = |at: usize| TokenMark {
                dead: marks.dead.iter().any(|&(s, e)| at >= s && at < e),
                underline: marks.underline.iter().any(|&(s, e)| at >= s && at < e),
            };

            let push = |job: &mut egui::text::LayoutJob,
                        s: &str,
                        color: egui::Color32,
                        mark: TokenMark| {
                if !s.is_empty() {
                    let color = if mark.dead { fade(color) } else { color };
                    let mut fmt = egui::TextFormat::simple(font.clone(), color);
                    if mark.underline {
                        fmt.underline = egui::Stroke::new(1.0_f32, color);
                    }
                    job.append(s, 0.0, fmt);
                }
            };
            let col = |ty: TokenType| theme.type_color(ty);

            let mut p = 0;
            while p < n {
                let c = chars[p];

                if c.is_whitespace() {
                    let s = p;
                    while p < n && chars[p].is_whitespace() {
                        p += 1;
                    }
                    push(
                        &mut job,
                        &slice(&chars, s, p),
                        col(TokenType::Whitespace(' ')),
                        in_dead(s),
                    );
                    continue;
                }
                if c == '/' && chars.get(p + 1) == Some(&'/') {
                    let s = p;
                    while p < n && chars[p] != '\n' {
                        p += 1;
                    }
                    push(
                        &mut job,
                        &slice(&chars, s, p),
                        col(TokenType::Comment(false)),
                        in_dead(s),
                    );
                    continue;
                }
                if c == '/' && chars.get(p + 1) == Some(&'*') {
                    let s = p;
                    p += 2;
                    while p < n && !(chars[p - 1] == '*' && chars[p] == '/') {
                        p += 1;
                    }
                    if p < n {
                        p += 1;
                    }
                    push(
                        &mut job,
                        &slice(&chars, s, p),
                        col(TokenType::Comment(true)),
                        in_dead(s),
                    );
                    continue;
                }
                if let Some(end) = raw_string_end(&chars, p) {
                    push(
                        &mut job,
                        &slice(&chars, p, end),
                        col(TokenType::Str('"')),
                        in_dead(p),
                    );
                    p = end;
                    continue;
                }
                if c == '"' || (c == 'b' && chars.get(p + 1) == Some(&'"')) {
                    let s = p;
                    if c == 'b' {
                        p += 1;
                    }
                    p += 1;
                    while p < n {
                        match chars[p] {
                            '\\' => p += 2,
                            '"' => {
                                p += 1;
                                break;
                            }
                            _ => p += 1,
                        }
                    }
                    push(
                        &mut job,
                        &slice(&chars, s, p.min(n)),
                        col(TokenType::Str('"')),
                        in_dead(s),
                    );
                    continue;
                }
                if c == '\'' {
                    if chars.get(p + 1) == Some(&'\\') {
                        let s = p;
                        p += 2;
                        while p < n && chars[p] != '\'' {
                            p += 1;
                        }
                        if p < n {
                            p += 1;
                        }
                        push(
                            &mut job,
                            &slice(&chars, s, p),
                            col(TokenType::Str('\'')),
                            in_dead(s),
                        );
                        continue;
                    }
                    let next_is_ident = chars.get(p + 1).is_some_and(|&c| is_ident_start(c));
                    if next_is_ident && chars.get(p + 2) != Some(&'\'') {
                        let s = p;
                        p += 1;
                        while p < n && is_ident(chars[p]) {
                            p += 1;
                        }
                        push(&mut job, &slice(&chars, s, p), LIFETIME_COLOR, in_dead(s));
                        continue;
                    }
                    let s = p;
                    p += 1;
                    if p < n {
                        p += 1;
                    }
                    if p < n && chars[p] == '\'' {
                        p += 1;
                    }
                    push(
                        &mut job,
                        &slice(&chars, s, p),
                        col(TokenType::Str('\'')),
                        in_dead(s),
                    );
                    continue;
                }
                if c.is_ascii_digit() {
                    let s = p;
                    p += 1;
                    while p < n && (chars[p].is_alphanumeric() || chars[p] == '_') {
                        p += 1;
                    }
                    if p < n
                        && chars[p] == '.'
                        && chars.get(p + 1).is_some_and(|c| c.is_ascii_digit())
                    {
                        p += 1;
                        while p < n && (chars[p].is_alphanumeric() || chars[p] == '_') {
                            p += 1;
                        }
                    }
                    push(
                        &mut job,
                        &slice(&chars, s, p),
                        col(TokenType::Numeric(false)),
                        in_dead(s),
                    );
                    continue;
                }
                if is_ident_start(c) {
                    let s = p;
                    while p < n && is_ident(chars[p]) {
                        p += 1;
                    }
                    let word = slice(&chars, s, p);
                    let ty = if chars.get(p) == Some(&'(') {
                        TokenType::Function
                    } else if syntax.is_keyword(&word) {
                        TokenType::Keyword
                    } else if syntax.is_type(&word) {
                        TokenType::Type
                    } else if syntax.is_special(&word) {
                        TokenType::Special
                    } else {
                        TokenType::Literal
                    };
                    push(&mut job, &word, col(ty), in_dead(s));
                    continue;
                }
                let s = p;
                p += 1;
                push(
                    &mut job,
                    &slice(&chars, s, p),
                    col(TokenType::Punctuation(c)),
                    in_dead(s),
                );
            }
            job
        }

        /// `numlines_show` up to the widget: the column text and its digit width.
        pub(super) fn numlines_text(text: &str, rows: usize, numbers: &[usize]) -> (String, usize) {
            let total = if text.ends_with('\n') || text.is_empty() {
                text.lines().count() + 1
            } else {
                text.lines().count()
            }
            .max(rows);
            let max_indent = numbers
                .last()
                .copied()
                .unwrap_or(total)
                .max(total)
                .to_string()
                .len();
            let pad = |n: usize| {
                let label = n.to_string();
                format!(
                    "{}{label}{}",
                    " ".repeat(max_indent.saturating_sub(label.len())),
                    " ".repeat(FOLD_GUTTER_CHARS),
                )
            };
            let counter = if numbers.is_empty() {
                (1..=total).map(pad).collect::<Vec<String>>().join("\n")
            } else {
                let mut v: Vec<String> = numbers.iter().map(|&n| pad(n)).collect();
                while v.len() < rows {
                    v.push(" ".repeat(max_indent));
                }
                v.join("\n")
            };
            (counter, max_indent)
        }

        /// `numlines_show`'s layouter.
        pub(super) fn numlines_job(
            buf: &str,
            fontsize: f32,
            plain: egui::Color32,
            folded_fg: egui::Color32,
            numbers: &[usize],
        ) -> egui::text::LayoutJob {
            let folded_rows = folded_header_rows(numbers);
            let font = egui::FontId::monospace(fontsize);
            if folded_rows.is_empty() {
                egui::text::LayoutJob::single_section(
                    buf.to_string(),
                    egui::TextFormat::simple(font, plain),
                )
            } else {
                let mut job = egui::text::LayoutJob::default();
                for (row, line) in buf.split('\n').enumerate() {
                    if row > 0 {
                        job.append("\n", 0.0, egui::TextFormat::simple(font.clone(), plain));
                    }
                    let fg = if folded_rows.contains(&row) {
                        folded_fg
                    } else {
                        plain
                    };
                    job.append(line, 0.0, egui::TextFormat::simple(font.clone(), fg));
                }
                job
            }
        }
    }

    /// Deterministic xorshift64 stream.
    fn rng(mut state: u64) -> impl FnMut() -> u64 {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    /// Snippets that reach each lexer branch at its edges: unterminated
    /// literals, escapes at EOF, multi-byte chars right after a quote or a
    /// backslash, `*/` shapes, and float-vs-range numbers.
    const LEXER_NAMED: &[&str] = &[
        "",
        "r#\"x\"#",
        "br##\"a\"#b\"##",
        "r#\"abc",
        "r\"",
        "r#",
        "rb\"x\"",
        "result r#ident",
        "\"abc\\\"def\"",
        "\"\\",
        "\"\\é",
        "\"\\é\"x",
        "b\"x\" b'x'",
        "'\\''",
        "'\\\\'",
        "'\\u{1F}'",
        "'\\",
        "'a' 'a 'ab' 'é' '😀' '😀 '' '",
        "'𝔸b 'é'",
        "/* a */ /*/ /**/ /* é*/ /*é/",
        "/* unterminated",
        "// comment é\nnext",
        "0..16 1.5f32 1. 0x1F_u8 1.é 7٣ ٣",
        "fn f<'a>(x: &'a str) -> &'static str { x }",
        "é( x٣ _ __ Self self::Vec<T>",
        "a\u{a0}b\u{3000}c\r\n\t d",
        "€😀{}();::",
    ];

    /// Named snippets, pseudo-random texts over every char a lexer branch
    /// tests for (multi-byte and astral ones too), and real sources.
    fn lexer_texts() -> Vec<String> {
        const ALPHABET: &[char] = &[
            'a', 'b', 'r', 'x', '_', 'Z', '0', '7', '.', '#', '"', '\'', '\\', '/', '*', '(', ')',
            '{', '}', ':', ' ', '\n', '\t', '\r', 'é', '😀', '𝔸', '٣', '\u{a0}', '\u{3000}', '€',
        ];
        let mut next = rng(0x2545_F491_4F6C_DD1D);
        let generated = (0..4000).map(|_| {
            let len = (next() % 40) as usize;
            (0..len)
                .map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize])
                .collect::<String>()
        });
        LEXER_NAMED
            .iter()
            .map(|s| (*s).to_owned())
            .chain(generated)
            .chain(
                [
                    include_str!("code_editor.rs"),
                    include_str!("text_pos.rs"),
                    include_str!("diagnostics_overlay.rs"),
                    include_str!("../../app/editor_panel/generics.rs"),
                    include_str!("../../app/editor_panel/fold.rs"),
                ]
                .map(str::to_owned),
            )
            .collect()
    }

    /// Mark lists for a text of `n` chars: none, unsorted and overlapping ones
    /// (inverted, empty and past-the-end ranges included), sorted ones, and one
    /// running to `usize::MAX`.
    fn mark_sets(n: usize, next: &mut impl FnMut() -> u64) -> Vec<Vec<(usize, usize)>> {
        let mut random = |count: u64| -> Vec<(usize, usize)> {
            (0..count)
                .map(|_| {
                    let s = (next() % (n as u64 + 3)) as usize;
                    let len = next() % 12;
                    // One in eight inverted.
                    if next().is_multiple_of(8) {
                        (s, s.saturating_sub(len as usize))
                    } else {
                        (s, s + len as usize)
                    }
                })
                .collect()
        };
        let unsorted = random(5);
        let mut sorted = random(4);
        sorted.sort();
        let wide = vec![(n / 3, usize::MAX), (0, n / 4)];
        let dense = random(40);
        vec![Vec::new(), unsorted, sorted, wide, dense]
    }

    /// Where two jobs first differ, for a readable failure on a large text.
    fn first_difference(got: &egui::text::LayoutJob, want: &egui::text::LayoutJob) -> String {
        if got.text != want.text {
            return "job text differs".to_owned();
        }
        let i = got
            .sections
            .iter()
            .zip(&want.sections)
            .position(|(g, w)| g != w)
            .unwrap_or(got.sections.len().min(want.sections.len()));
        format!(
            "section {i} of {}/{}: got {:?}, want {:?}",
            got.sections.len(),
            want.sections.len(),
            got.sections.get(i),
            want.sections.get(i),
        )
    }

    /// Same text, same sections, same formats as the char-vector tokenizer, for
    /// every text and every combination of fade and underline lists.
    #[test]
    fn tokenizer_matches_the_char_vector_implementation() {
        let theme = ColorTheme::GRUVBOX;
        let syntax = Syntax::rust();
        let mut next = rng(0x9E37_79B9_7F4A_7C15);
        let (mut faded, mut underlined, mut lifetimes, mut raw_strings) = (0, 0, 0, 0);
        let faded_literal = fade(theme.type_color(TokenType::Literal));
        for text in lexer_texts() {
            let sets = mark_sets(text.chars().count(), &mut next);
            for (k, dead) in sets.iter().enumerate() {
                let underline = &sets[(k + 2) % sets.len()];
                let marks = Marks {
                    read_only: false,
                    dead,
                    underline,
                };
                let got = rust_layout_job(&text, &theme, 13.0, &syntax, marks);
                let want = old::rust_layout_job(&text, &theme, 13.0, &syntax, marks);
                assert!(
                    got == want,
                    "{} (text of {} bytes, dead {dead:?}, underline {underline:?})",
                    first_difference(&got, &want),
                    text.len(),
                );
                for s in &got.sections {
                    let slice = &got.text[s.byte_range.start.0..s.byte_range.end.0];
                    faded += usize::from(s.format.color == faded_literal);
                    underlined += usize::from(s.format.underline.width > 0.0);
                    lifetimes += usize::from(s.format.color == LIFETIME_COLOR);
                    raw_strings += usize::from(slice.starts_with("r#\"") && slice.len() > 3);
                }
            }
        }
        // The inputs really did reach the marked and the rarer branches.
        assert!(faded > 100, "{faded}");
        assert!(underlined > 100, "{underlined}");
        assert!(lifetimes > 100, "{lifetimes}");
        assert!(raw_strings > 5, "{raw_strings}");
    }

    /// The format cache keys the punctuation colour on nothing but the class.
    #[test]
    fn punctuation_colour_ignores_the_char() {
        let theme = ColorTheme::GRUVBOX;
        let want = theme.type_color(TokenType::Punctuation('.'));
        for c in ['{', '}', '(', ';', '#', '\\', '€', '😀', '\u{3000}'] {
            assert_eq!(theme.type_color(TokenType::Punctuation(c)), want, "{c:?}");
        }
    }

    /// `covers` answers exactly what scanning every range answers, for any
    /// ranges and any non-decreasing sequence of indices, repeats included.
    #[test]
    fn coverage_matches_scanning_every_range() {
        let mut next = rng(0xA076_1D64_78BD_642F);
        for round in 0..2000 {
            let n = (next() % 60) as usize;
            let sets = mark_sets(n, &mut next);
            let ranges = &sets[1 + round % (sets.len() - 1)];
            let mut coverage = Coverage::new(ranges);
            let mut at = 0;
            while at <= n + 3 {
                let want = ranges.iter().any(|&(s, e)| at >= s && at < e);
                assert_eq!(coverage.covers(at), want, "{ranges:?} at {at}");
                at += (next() % 3) as usize; // 0 asks the same index again
            }
        }
    }

    /// `row_count` is the old `lines().count()` rule, on every shape of text.
    #[test]
    fn row_count_matches_the_lines_rule() {
        let texts = crate::editor::gui::text_pos::word_walk_tests::word_texts();
        for text in texts
            .iter()
            .map(String::as_str)
            .chain(LEXER_NAMED.iter().copied())
        {
            let old = if text.ends_with('\n') || text.is_empty() {
                text.lines().count() + 1
            } else {
                text.lines().count()
            };
            assert_eq!(row_count(text), old, "{text:?}");
        }
    }

    /// Fold number lists: consecutive, with gaps, out of order, wider than the
    /// row count, and longer or shorter than `rows`.
    fn number_lists() -> Vec<Vec<usize>> {
        vec![
            Vec::new(),
            vec![1, 2, 3],
            vec![10, 40, 41],
            vec![1, 5, 6, 9],
            vec![7],
            vec![5, 3],
            vec![12345, 12346, 20000],
            (1..=30).filter(|n| n % 7 != 0).collect(),
        ]
    }

    #[test]
    fn number_column_text_and_job_match_the_per_line_build() {
        let plain = egui::Color32::from_rgb(1, 2, 3);
        let orange = egui::Color32::from_rgb(250, 128, 25);
        for lines in [0usize, 1, 2, 9, 10, 11, 99, 100, 101, 999, 1000] {
            let text = "x\n".repeat(lines);
            for rows in [0, 1, 5, 10, 40, 150] {
                for numbers in number_lists() {
                    let (want, want_indent) = old::numlines_text(&text, rows, &numbers);
                    let total = row_count(&text).max(rows);
                    let (got, indent) = number_column_text(total, rows, &numbers);
                    assert_eq!(got, want, "lines {lines} rows {rows} numbers {numbers:?}");
                    assert_eq!(indent, want_indent);
                    for fontsize in [13.0, 17.5] {
                        assert_eq!(
                            number_column_job(
                                &got,
                                fontsize,
                                plain,
                                orange,
                                &folded_header_rows(&numbers)
                            ),
                            old::numlines_job(&want, fontsize, plain, orange, &numbers),
                        );
                    }
                }
            }
        }
    }

    /// The memo never serves a column built for other inputs: any change of
    /// text, rows, fold numbers, font size or colour shows up, and more views
    /// than slots only cost rebuilds.
    #[test]
    fn number_column_memo_tracks_every_input() {
        let plain = egui::Color32::from_rgb(1, 2, 3);
        let colours = [
            egui::Color32::from_rgb(250, 128, 25),
            egui::Color32::from_rgb(9, 9, 9),
        ];
        let texts: Vec<String> = ["", "a", "a\nb", "a\nb\n", "a\nb\nc"]
            .map(str::to_owned)
            .into_iter()
            .chain(["x\n".repeat(9), "x\n".repeat(120)])
            .collect();
        let ids = ["v0", "v1", "v2", "v3", "v4", "v5"];
        let lists = number_lists();
        let mut next = rng(0x5851_F42D_4C95_7F2D);
        let mut pick = |len: usize| (next() % len as u64) as usize;
        for _ in 0..3000 {
            let id = ids[pick(ids.len())];
            let text = &texts[pick(texts.len())];
            let rows = [0, 1, 3, 20][pick(4)];
            let numbers = &lists[pick(lists.len())];
            let fontsize = [13.0, 14.5][pick(2)];
            let fg = colours[pick(2)];
            let (counter, indent, job) =
                number_column(id, text, rows, numbers, fontsize, plain, fg);
            let (want, want_indent) = old::numlines_text(text, rows, numbers);
            assert_eq!(
                &*counter,
                want.as_str(),
                "{id} {text:?} rows {rows} {numbers:?}"
            );
            assert_eq!(indent, want_indent);
            assert_eq!(job, old::numlines_job(&want, fontsize, plain, fg, numbers));
        }
        let resident = NUMBER_COLUMNS.with(|m| m.borrow().len());
        assert_eq!(resident, NUMBER_COLUMN_SLOTS);
    }
}
