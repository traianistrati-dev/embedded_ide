//! Text-position and LSP coordinate helpers for the code editor.
//!
//! These are pure functions that translate between:
//! - char offsets (what egui's `CCursor` uses)
//! - LSP positions (1-based line, UTF-16 column)
//! - on-screen geometry (wavy underline rendering)
//!
//! Plus small lookup helpers for mapping the selected file to its LSP path
//! and fetching diagnostics resiliently across path-format mismatches.

use crate::app::ProjectFileId;
use crate::lsp;
use eframe::egui;
use std::sync::Arc;

// ── LSP completion helpers ────────────────────────────────────────────────────

/// Convert a character offset into a (line, UTF-16-column) pair for LSP.
///
/// LSP `Position.character` is a count of UTF-16 code units from the start
/// of the line, NOT a count of Unicode chars.  For the BMP (U+0000–U+FFFF,
/// which includes all ASCII + Romanian diacritics) each char = 1 unit, so the
/// result is the same.  Emoji and other non-BMP chars are 2 units each.
pub fn lsp_cursor_pos(text: &str, char_idx: usize) -> (u32, u32) {
    let mut line: u32 = 0;
    let mut col: u32 = 0;
    for (i, c) in text.chars().enumerate() {
        if i >= char_idx {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += c.len_utf16() as u32;
        }
    }
    (line, col)
}

/// Extract the prefix typed between `trigger_idx` and `cursor_idx`.
///
/// Used for live filtering of the completion popup: after the user triggers
/// completion (via `.` or Ctrl+Space) any additional characters they type
/// narrow the visible list.  Returns an empty string when the cursor hasn't
/// moved past the trigger point yet.
pub fn lsp_completion_prefix(text: &str, trigger_idx: usize, cursor_idx: usize) -> String {
    if cursor_idx <= trigger_idx {
        return String::new();
    }
    // Only take identifier characters (letters, digits, _).  A dot or space
    // means the user has moved to a new expression — we'll rely on the delta
    // check in the trigger section to close the popup in that case.
    //
    // Walked in place: both ends clamp to the end of the text, as slicing a
    // collected `Vec<char>` did, without copying the whole file every frame
    // the popup is open.
    text.chars()
        .skip(trigger_idx)
        .take(cursor_idx - trigger_idx)
        .take_while(|&c| is_ident_char(c))
        .collect()
}

/// Return the char-index of the first character of the identifier that ends at `end_idx`.
pub fn lsp_word_start(text: &str, end_idx: usize) -> usize {
    let (end, end_byte) = char_and_byte_at(text, end_idx);
    end - text[..end_byte]
        .chars()
        .rev()
        .take_while(|&c| is_ident_char(c))
        .count()
}

/// A char that continues an identifier: what the word helpers above and
/// `rename::identifier_at` stop at.
pub fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `(char_idx.min(chars), byte offset of that char)` for a text of `chars`
/// chars, walking only up to the index. Past the end both clamp to the end.
pub fn char_and_byte_at(text: &str, char_idx: usize) -> (usize, usize) {
    let mut chars = 0;
    for (byte, _) in text.char_indices() {
        if chars == char_idx {
            return (chars, byte);
        }
        chars += 1;
    }
    (chars, text.len())
}

// ── LSP / file helpers ────────────────────────────────────────────────────────

/// Return the LSP relative path for the currently selected file.
///
/// - `MainRs`       → `"src/main.rs"`
/// - `UserFile(i)`  → `"src/{user_src_files[i].0}"`  (e.g. `"src/pins.rs"`)
/// - Other files (Cargo.toml, memory.x, …) → `None` (not tracked by RA)
pub fn selected_file_rel_path(
    selected: &ProjectFileId,
    user_files: &[(String, String)],
) -> Option<String> {
    match selected {
        ProjectFileId::MainRs => Some("src/main.rs".to_owned()),
        ProjectFileId::UserFile(i) => user_files.get(*i).map(|(p, _)| p.clone()),
        _ => None,
    }
}

/// Return the diagnostics for `rel_path` regardless of how the key is stored.
///
/// On Windows, rust-analyzer may send file URIs with a lowercase drive letter
/// (`file:///c:/…`) while `path_to_uri` produces uppercase (`file:///C:/…`).
/// `uri_to_rel` does a case-sensitive prefix strip, so on a mismatch the full
/// URI is used as the key.  This helper tries several key formats so inline
/// diagnostics work even when the key is "wrong".
///
/// Borrowed, not cloned: the callers read it under the `LspState` lock every
/// frame and copy out only what they keep.
pub fn diags_for_file<'a>(
    map: &'a std::collections::HashMap<String, Vec<lsp::LspDiagnostic>>,
    rel_path: &str,
) -> &'a [lsp::LspDiagnostic] {
    // 1. Exact match (ideal case)
    if let Some(v) = map.get(rel_path) {
        return v;
    }
    // 2. Case-insensitive suffix match for Windows drive-letter mismatches.
    //    The key may be a full URI like "file:///c:/.../{rel_path}".
    //
    //    ONLY absolute/URI keys are eligible. Keys are project-root-relative
    //    now, and more than one crate lives under the root — a bare suffix
    //    match would serve `mw_radar/src/utils.rs`'s diagnostics for a lookup
    //    of `src/utils.rs`, painting a library's errors onto the firmware file
    //    with the same sub-path.
    let rel_lc = rel_path.to_lowercase();
    let suffix_slash = format!("/{rel_lc}");
    let suffix_bslash = format!("\\{}", rel_lc.replace('/', "\\"));
    for (k, v) in map {
        if !is_absolute_or_uri(k) {
            continue;
        }
        let k_lc = k.to_lowercase();
        if k_lc.ends_with(&suffix_slash) || k_lc.ends_with(&suffix_bslash) {
            return v;
        }
    }
    &[]
}

/// `true` for a full URI (`file:///…`) or an absolute filesystem path — the
/// only key shapes the suffix fallback above is meant for.
fn is_absolute_or_uri(key: &str) -> bool {
    key.contains("://")
        || key.starts_with('/')
        || key.starts_with('\\')
        // Windows drive letter: `C:\…` / `c:/…`
        || key.as_bytes().get(1) == Some(&b':')
}

// ── Inline diagnostics helpers ────────────────────────────────────────────────

/// Return the char index of the last non-newline character on `line_1` (1-based).
/// Used to position inline error messages at the end of the error line.
pub fn lsp_line_end_char_idx(text: &str, line_1: u32) -> usize {
    let want_line = line_1.saturating_sub(1) as usize;
    let mut cur_line = 0usize;
    let mut char_idx = 0usize;
    let mut line_end = 0usize;
    for c in text.chars() {
        if c == '\n' {
            if cur_line == want_line {
                return line_end;
            }
            cur_line += 1;
            line_end = char_idx + 1; // start of next line
        } else {
            line_end = char_idx + 1; // last non-newline on this line (so far)
        }
        char_idx += 1;
    }
    char_idx
}

/// Convert an LSP position (1-based line, 1-based column in UTF-16 units)
/// to a char index suitable for `galley.pos_from_cursor(CCursor::new(idx))`.
///
/// LSP columns are UTF-16 code-unit offsets from line start.  For ASCII and
/// BMP characters (including Romanian diacritics) each char is 1 unit.
pub fn lsp_pos_to_char_idx(text: &str, line_1: u32, col_1: u32) -> usize {
    let want_line = line_1.saturating_sub(1) as usize;
    let want_utf16_col = col_1.saturating_sub(1) as usize;
    let mut cur_line = 0usize;
    let mut utf16_col = 0usize;
    let mut char_idx = 0usize;
    for c in text.chars() {
        if cur_line == want_line && utf16_col >= want_utf16_col {
            return char_idx;
        }
        if c == '\n' {
            if cur_line == want_line {
                // Column is past end of line — clamp to the newline position.
                return char_idx;
            }
            cur_line += 1;
            utf16_col = 0;
        } else {
            utf16_col += c.len_utf16();
        }
        char_idx += 1;
    }
    char_idx
}

// ── Line index ────────────────────────────────────────────────────────────────

/// The line starts of one text, found in a single pass, so that a position
/// lookup walks one line instead of the whole prefix of the file.
///
/// The overlays convert positions every frame; the free functions above each
/// walk the text from offset 0, which made the inline diagnostics cost
/// O(diagnostics x file) per frame in the dev build.
///
/// It owns a copy of the text it describes. The column walk on a non-ASCII line
/// and [`LineIndex::line_str`] read that copy, and an index can never be paired
/// with some other text. Every method returns exactly what the free function it
/// names returns, clamps included (pinned by `line_index_tests`).
pub struct LineIndex {
    text: String,
    /// Char offset where each line starts: `0`, then one past every `'\n'`.
    line_start_chars: Vec<usize>,
    /// The same starts as byte offsets into `text`.
    line_start_bytes: Vec<usize>,
    total_chars: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_start_chars = vec![0];
        let mut line_start_bytes = vec![0];
        let (mut chars, mut bytes) = (0usize, 0usize);
        // Line by line through `find`, which reaches core's memchr, rather than
        // a per-byte loop that the dev build would leave unoptimised.
        let mut rest = text;
        while let Some(nl) = rest.find('\n') {
            chars += rest[..nl].chars().count() + 1;
            bytes += nl + 1;
            line_start_chars.push(chars);
            line_start_bytes.push(bytes);
            rest = &rest[nl + 1..];
        }
        chars += rest.chars().count();
        Self {
            text: text.to_owned(),
            line_start_chars,
            line_start_bytes,
            total_chars: chars,
        }
    }

    /// The text this index was built from.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// `text().chars().count()`.
    pub fn total_chars(&self) -> usize {
        self.total_chars
    }

    /// `((char start, char end), (byte start, byte end))` of 0-based `line`,
    /// without its `'\n'`. `None` past the last line.
    fn span(&self, line: usize) -> Option<((usize, usize), (usize, usize))> {
        let cs = *self.line_start_chars.get(line)?;
        let bs = self.line_start_bytes[line];
        Some(match self.line_start_chars.get(line + 1) {
            Some(&next) => ((cs, next - 1), (bs, self.line_start_bytes[line + 1] - 1)),
            None => ((cs, self.total_chars), (bs, self.text.len())),
        })
    }

    /// Exactly [`lsp_pos_to_char_idx`] on [`Self::text`]: a column past the end
    /// of the line lands on its `'\n'` (or EOF), a line past the end on the
    /// total char count.
    pub fn pos_to_char_idx(&self, line_1: u32, col_1: u32) -> usize {
        let want = col_1.saturating_sub(1) as usize;
        let Some(((cs, ce), (bs, be))) = self.span(line_1.saturating_sub(1) as usize) else {
            return self.total_chars;
        };
        if be - bs == ce - cs {
            // One byte per char means all ASCII: one UTF-16 unit per char.
            return cs + want.min(ce - cs);
        }
        // Checked BEFORE advancing, like the free function, so a column that
        // points into the middle of a surrogate pair lands on the next char.
        let mut utf16 = 0usize;
        for (k, c) in self.text[bs..be].chars().enumerate() {
            if utf16 >= want {
                return cs + k;
            }
            utf16 += c.len_utf16();
        }
        ce
    }

    /// Exactly [`lsp_line_end_char_idx`] on [`Self::text`]: the index of the
    /// line's `'\n'`, or the total char count on the last line and past it.
    pub fn line_end_char_idx(&self, line_1: u32) -> usize {
        match self
            .line_start_chars
            .get(line_1.saturating_sub(1) as usize + 1)
        {
            Some(&next) => next - 1,
            None => self.total_chars,
        }
    }

    /// Exactly `lsp_cursor_pos(text, char_idx).0`: the 0-based line holding
    /// char `char_idx`, and the last line for any index past the end.
    pub fn line_of_char(&self, char_idx: usize) -> usize {
        self.line_start_chars.partition_point(|&s| s <= char_idx) - 1
    }

    /// Char index where 0-based `line` starts: `0`, then one past each
    /// `'\n'`. `None` past the last line (a final `'\n'` opens an empty one).
    pub fn line_start_char(&self, line: usize) -> Option<usize> {
        self.line_start_chars.get(line).copied()
    }

    /// How many lines [`Self::line_start_char`] answers for: one more than the
    /// number of `'\n'`s.
    pub fn line_count(&self) -> usize {
        self.line_start_chars.len()
    }

    /// Exactly `text.lines().nth(line)`: no segment after a final `'\n'` (nor
    /// for an empty text), and a `'\r'` is dropped only before a `'\n'`.
    pub fn line_str(&self, line: usize) -> Option<&str> {
        let (_, (bs, be)) = self.span(line)?;
        let l = &self.text[bs..be];
        if line + 1 < self.line_start_bytes.len() {
            Some(l.strip_suffix('\r').unwrap_or(l))
        } else {
            (!l.is_empty()).then_some(l)
        }
    }

    /// Exactly `text.char_indices().nth(char_idx).map_or(text.len(), |(b, _)| b)`:
    /// the byte offset of char `char_idx`, and the text's length past the end.
    pub fn byte_of_char(&self, char_idx: usize) -> usize {
        if char_idx >= self.total_chars {
            return self.text.len();
        }
        let Some(((cs, ce), (bs, be))) = self.span(self.line_of_char(char_idx)) else {
            return self.text.len();
        };
        if be - bs == ce - cs {
            return bs + (char_idx - cs);
        }
        // `char_idx == ce` is the line's `'\n'`, which sits at byte `be`.
        self.text[bs..be]
            .char_indices()
            .nth(char_idx - cs)
            .map_or(be, |(b, _)| bs + b)
    }
}

/// A view's [`LineIndex`], rebuilt only when it is asked about a text other
/// than the one it holds.
///
/// Keyed on text EQUALITY rather than built once at a fixed point of the frame:
/// the editor text changes mid-frame (completion accept, multi-cursor replay,
/// line ops), so whoever asks passes the exact text they draw against and gets
/// an index of that text. A hit costs one `memcmp`. Handed out as an `Arc` so the
/// caller can keep it while calling `&mut self` methods.
#[derive(Default)]
pub struct LineIndexCache(Option<Arc<LineIndex>>);

impl LineIndexCache {
    pub fn get(&mut self, text: &str) -> Arc<LineIndex> {
        if let Some(index) = &self.0
            && index.text() == text
        {
            return Arc::clone(index);
        }
        let index = Arc::new(LineIndex::new(text));
        self.0 = Some(Arc::clone(&index));
        index
    }
}

// ── Galley rows ───────────────────────────────────────────────────────────────

/// A laid-out galley's rows as char ranges, so a char index finds its row by
/// binary search instead of the linear walk in `Galley::layout_from_cursor`.
///
/// The occurrence highlights called `pos_from_cursor` twice for every match in
/// the file, on screen or not, which is O(matches x rows) per frame. Every
/// lookup here reads the galley alone, as the galley's own methods do, so it
/// stays exact when the galley is an edit behind the text an index came from.
///
/// The row table is built on first use: creating one costs nothing, so a
/// caller can make it before it knows whether anything will be drawn.
pub struct GalleyRows<'g> {
    galley: &'g egui::Galley,
    /// Char index just past each row's last glyph (before its `'\n'`, if any).
    ends: std::cell::OnceCell<Vec<usize>>,
}

impl<'g> GalleyRows<'g> {
    pub fn new(galley: &'g egui::Galley) -> Self {
        Self {
            galley,
            ends: std::cell::OnceCell::new(),
        }
    }

    /// The galley these rows describe.
    pub fn galley(&self) -> &'g egui::Galley {
        self.galley
    }

    /// Non-decreasing: a row starts where the previous one ended, plus its `'\n'`.
    fn ends(&self) -> &[usize] {
        self.ends.get_or_init(|| {
            let mut start = 0;
            self.galley
                .rows
                .iter()
                .map(|row| {
                    let end = start + row.char_count_excluding_newline();
                    start += row.char_count_including_newline();
                    end
                })
                .collect()
        })
    }

    /// Exactly `galley.layout_from_cursor(CCursor::new(idx))`: the first row
    /// whose end reaches `idx` (so an index on a wrap boundary stays on the
    /// upper row), and past the last row the end of the last row.
    pub fn layout_cursor(&self, idx: usize) -> egui::epaint::text::cursor::LayoutCursor {
        use egui::epaint::text::cursor::LayoutCursor;
        let ends = self.ends();
        let row = ends.partition_point(|&end| end < idx);
        if let Some(placed) = self.galley.rows.get(row) {
            let start = ends[row] - placed.char_count_excluding_newline();
            return LayoutCursor {
                row,
                column: idx - start,
            };
        }
        match self.galley.rows.last() {
            Some(last) => LayoutCursor {
                row: row - 1,
                column: last.char_count_including_newline(),
            },
            None => LayoutCursor::default(),
        }
    }

    /// Exactly `galley.pos_from_cursor(CCursor::new(idx))`.
    pub fn pos(&self, idx: usize) -> egui::Rect {
        self.galley.pos_from_layout_cursor(&self.layout_cursor(idx))
    }

    /// The char indices whose [`Self::pos`] can meet the band `top..=bottom`
    /// once moved down by `y_offset`: from the first row that meets it to the
    /// last one. A row in between that misses the band is still covered, so the
    /// caller keeps its own per-hit test. The end is `usize::MAX` when the last
    /// row meets the band, because every index past the galley lands there.
    /// `None` when no row meets it.
    pub fn chars_meeting_band(
        &self,
        y_offset: f32,
        top: f32,
        bottom: f32,
    ) -> Option<std::ops::RangeInclusive<usize>> {
        // The sums the highlights test per hit (`gp.y + pos.max.y >= top`),
        // since a cursor rect spans exactly its row's `min_y..max_y`.
        let meets = |row: &egui::epaint::text::PlacedRow| {
            y_offset + row.max_y() >= top && y_offset + row.min_y() <= bottom
        };
        let rows = &self.galley.rows;
        let first = rows.iter().position(meets)?;
        let last = rows.iter().rposition(meets)?;
        let ends = self.ends();
        let start = if first == 0 { 0 } else { ends[first - 1] + 1 };
        let end = if last + 1 == rows.len() {
            usize::MAX
        } else {
            ends[last]
        };
        Some(start..=end)
    }
}

/// Lays `(text, wrap width)` pairs out as monospace galleys in one headless
/// frame, for tests that pin geometry against a real galley.
#[cfg(test)]
pub(crate) fn test_galleys(specs: &[(&str, f32)]) -> Vec<Arc<egui::Galley>> {
    let ctx = egui::Context::default();
    let mut out = Vec::new();
    let _ = ctx.run_ui(Default::default(), |ui| {
        out = specs
            .iter()
            .map(|&(text, wrap)| {
                let job = egui::text::LayoutJob::simple(
                    text.to_owned(),
                    egui::FontId::monospace(12.0),
                    egui::Color32::WHITE,
                    wrap,
                );
                ui.fonts_mut(|f| f.layout_job(job))
            })
            .collect();
    });
    out
}

/// Draw a wavy (zigzag) underline between `x_start` and `x_end` at height `y`.
///
/// Each segment alternates up/down by `AMP` pixels with a horizontal step of
/// `STEP` pixels, producing the classic "squiggly" error underline appearance.
pub fn draw_wavy_underline(
    painter: &egui::Painter,
    x_start: f32,
    x_end: f32,
    y: f32,
    color: egui::Color32,
) {
    const STEP: f32 = 3.0;
    const AMP: f32 = 1.5;
    if x_end <= x_start {
        return;
    }
    let stroke = egui::Stroke::new(1.2_f32, color);
    let mut x = x_start;
    let mut up = true;
    while x < x_end {
        let x2 = (x + STEP).min(x_end);
        let (y1, y2) = if up { (y, y + AMP) } else { (y + AMP, y) };
        painter.line_segment([egui::pos2(x, y1), egui::pos2(x2, y2)], stroke);
        x = x2;
        up = !up;
    }
}

/// Map an LSP CompletionItemKind number to a short (3-char) icon string.
pub fn lsp_kind_icon(kind: u8) -> &'static str {
    match kind {
        2 | 3 => "fn ", // Method / Function
        4 => "ctr",     // Constructor
        5 => "fld",     // Field
        6 => "var",     // Variable
        7 => "cls",     // Class
        8 => "int",     // Interface
        9 => "mod",     // Module
        13 => "enm",    // Enum
        14 => "kwd",    // Keyword
        20 => "enm",    // EnumMember
        21 => "con",    // Constant
        22 => "str",    // Struct
        25 => "typ",    // TypeParameter
        _ => "   ",
    }
}

#[cfg(test)]
mod diags_for_file_tests {
    use super::*;
    use std::collections::HashMap;

    fn diag() -> lsp::LspDiagnostic {
        lsp::LspDiagnostic {
            severity: lsp::DiagSeverity::Error,
            message: "boom".into(),
            line: 1,
            col: 1,
            end_line: 1,
            end_col: 2,
            code: None,
            source: "rustc".into(),
        }
    }

    /// The whole point of the suffix fallback: a full URI key still resolves.
    #[test]
    fn uri_keys_still_match_by_suffix() {
        let mut map = HashMap::new();
        map.insert("file:///c:/tmp/ws/src/utils.rs".to_string(), vec![diag()]);
        assert_eq!(diags_for_file(&map, "src/utils.rs").len(), 1);
    }

    /// Regression: with more than one crate under the project root, a RELATIVE
    /// key must never satisfy a suffix match — `mw_radar/src/utils.rs` would
    /// otherwise paint the library's errors onto the firmware's `src/utils.rs`.
    #[test]
    fn another_crates_relative_key_does_not_match() {
        let mut map = HashMap::new();
        map.insert("mw_radar/src/utils.rs".to_string(), vec![diag()]);
        assert!(
            diags_for_file(&map, "src/utils.rs").is_empty(),
            "a different crate's file must not supply diagnostics"
        );
        // …while its own exact lookup still works.
        assert_eq!(diags_for_file(&map, "mw_radar/src/utils.rs").len(), 1);
    }

    #[test]
    fn exact_match_wins() {
        let mut map = HashMap::new();
        map.insert("src/utils.rs".to_string(), vec![diag()]);
        assert_eq!(diags_for_file(&map, "src/utils.rs").len(), 1);
        assert!(diags_for_file(&map, "src/other.rs").is_empty());
    }

    /// The lookup hands out the stored diagnostics themselves, not a copy.
    #[test]
    fn the_result_borrows_the_stored_list() {
        let mut map = HashMap::new();
        map.insert("src/utils.rs".to_string(), vec![diag(), diag()]);
        let got = diags_for_file(&map, "src/utils.rs");
        assert!(std::ptr::eq(got, map["src/utils.rs"].as_slice()));
    }
}

#[cfg(test)]
mod line_index_tests {
    use super::*;

    /// The shapes that decide a clamp: empty, trailing newline, CRLF, a lone
    /// `'\r'`, multi-byte BMP chars and astral chars (two UTF-16 units).
    const NAMED: &[&str] = &[
        "",
        "\n",
        "\n\n",
        "a",
        "a\n",
        "ab\ncd",
        "ab\ncd\n",
        "\r\n",
        "a\r\nb\r\n",
        "a\r",
        "\r",
        "ăîș\nțâ",
        "x😀y\n😀",
        "😀",
        "€uro\n\n  // c\n",
        "fn main() {\n    let x = 1;\n}\n",
    ];

    const ALPHABET: &[char] = &[
        'a', 'Z', ' ', '\t', '/', '\n', '\n', '\r', 'ă', '€', '😀', '𝄞',
    ];

    /// Deterministic pseudo-random texts over [`ALPHABET`] (xorshift64).
    fn generated() -> Vec<String> {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        (0..400)
            .map(|_| {
                let len = (next() % 48) as usize;
                (0..len)
                    .map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize])
                    .collect()
            })
            .collect()
    }

    pub(crate) fn texts() -> Vec<String> {
        NAMED
            .iter()
            .map(|s| (*s).to_owned())
            .chain(generated())
            .collect()
    }

    /// `(line count, widest line in UTF-16 units)`.
    fn extent(text: &str) -> (u32, u32) {
        let lines = text.split('\n').count() as u32;
        let widest = text
            .split('\n')
            .map(|l| l.encode_utf16().count())
            .max()
            .unwrap_or(0) as u32;
        (lines, widest)
    }

    #[test]
    fn the_generated_texts_reach_the_hard_cases() {
        let all = generated();
        assert!(all.iter().any(|t| t.contains('😀') && t.contains('\n')));
        assert!(all.iter().any(|t| t.contains("\r\n")));
        assert!(all.iter().any(|t| t.ends_with('\n')));
        assert!(all.iter().any(|t| t.ends_with('\r')));
        assert!(all.iter().any(|t| t.is_empty()));
    }

    #[test]
    fn pos_to_char_idx_matches_the_walk_from_offset_zero() {
        for text in texts() {
            let index = LineIndex::new(&text);
            let (lines, widest) = extent(&text);
            for line in 0..=lines + 2 {
                for col in (0..=widest + 3).chain([u32::MAX]) {
                    assert_eq!(
                        index.pos_to_char_idx(line, col),
                        lsp_pos_to_char_idx(&text, line, col),
                        "{text:?} line {line} col {col}"
                    );
                }
            }
            for col in [0, 1, 2, u32::MAX] {
                assert_eq!(
                    index.pos_to_char_idx(u32::MAX, col),
                    lsp_pos_to_char_idx(&text, u32::MAX, col),
                    "{text:?} line MAX col {col}"
                );
            }
        }
    }

    #[test]
    fn line_end_char_idx_matches_the_walk_from_offset_zero() {
        for text in texts() {
            let index = LineIndex::new(&text);
            let (lines, _) = extent(&text);
            for line in (0..=lines + 2).chain([u32::MAX]) {
                assert_eq!(
                    index.line_end_char_idx(line),
                    lsp_line_end_char_idx(&text, line),
                    "{text:?} line {line}"
                );
            }
        }
    }

    #[test]
    fn total_chars_and_line_of_char_match() {
        for text in texts() {
            let index = LineIndex::new(&text);
            let total = text.chars().count();
            assert_eq!(index.total_chars(), total, "{text:?}");
            assert_eq!(index.text(), text);
            for i in (0..=total + 3).chain([usize::MAX]) {
                assert_eq!(
                    index.line_of_char(i),
                    lsp_cursor_pos(&text, i).0 as usize,
                    "{text:?} char {i}"
                );
            }
        }
    }

    #[test]
    fn line_start_char_matches_a_scan_for_newlines() {
        for text in texts() {
            let index = LineIndex::new(&text);
            let mut starts = vec![0];
            for (i, c) in text.chars().enumerate() {
                if c == '\n' {
                    starts.push(i + 1);
                }
            }
            for line in (0..starts.len() + 3).chain([usize::MAX]) {
                assert_eq!(
                    index.line_start_char(line),
                    starts.get(line).copied(),
                    "{text:?} line {line}"
                );
            }
        }
    }

    #[test]
    fn line_count_is_the_number_of_line_starts() {
        for text in texts() {
            let index = LineIndex::new(&text);
            let starts = 1 + text.chars().filter(|&c| c == '\n').count();
            assert_eq!(index.line_count(), starts, "{text:?}");
            assert_eq!(index.line_start_char(starts), None, "{text:?}");
        }
    }

    #[test]
    fn line_str_matches_lines_nth() {
        for text in texts() {
            let index = LineIndex::new(&text);
            let (lines, _) = extent(&text);
            for n in (0..lines as usize + 3).chain([usize::MAX]) {
                assert_eq!(index.line_str(n), text.lines().nth(n), "{text:?} line {n}");
            }
        }
    }

    #[test]
    fn byte_of_char_matches_char_indices() {
        for text in texts() {
            let index = LineIndex::new(&text);
            let total = text.chars().count();
            for i in (0..=total + 3).chain([usize::MAX]) {
                assert_eq!(
                    index.byte_of_char(i),
                    text.char_indices().nth(i).map_or(text.len(), |(b, _)| b),
                    "{text:?} char {i}"
                );
            }
        }
    }

    #[test]
    fn the_cache_rebuilds_only_for_a_different_text() {
        let mut cache = LineIndexCache::default();
        let first = cache.get("ab\ncd");
        let same = cache.get(&String::from("ab\ncd"));
        assert!(Arc::ptr_eq(&first, &same), "an equal text reuses the index");
        let changed = cache.get("ab\ncd\n");
        assert!(!Arc::ptr_eq(&first, &changed), "a different text rebuilds");
        assert_eq!(changed.text(), "ab\ncd\n");
        assert_eq!(changed.line_end_char_idx(2), 5);
        // The index handed out earlier still describes ITS text.
        assert_eq!(first.line_end_char_idx(2), 5);
        assert_eq!(first.total_chars(), 5);
    }
}

#[cfg(test)]
pub(crate) mod galley_rows_tests {
    use super::*;
    use egui::text::CCursor;

    /// Every text of `line_index_tests`, laid out unwrapped (as the editor does)
    /// and at a width of a few chars, so rows also end without a `'\n'`.
    pub(crate) fn galleys() -> Vec<(String, Arc<egui::Galley>)> {
        let texts = super::line_index_tests::texts();
        let specs: Vec<(&str, f32)> = texts
            .iter()
            .flat_map(|t| [(t.as_str(), f32::INFINITY), (t.as_str(), 30.0)])
            .collect();
        let laid = test_galleys(&specs);
        specs
            .iter()
            .map(|(t, _)| (*t).to_owned())
            .zip(laid)
            .collect()
    }

    #[test]
    fn the_galleys_reach_wrapped_rows() {
        assert!(galleys().iter().any(|(_, g)| {
            let n = g.rows.len();
            g.rows
                .iter()
                .take(n.saturating_sub(1))
                .any(|r| !r.ends_with_newline)
        }));
    }

    #[test]
    fn layout_cursor_and_pos_match_the_galley_walk() {
        for (text, galley) in galleys() {
            let rows = GalleyRows::new(&galley);
            for idx in 0..=text.chars().count() + 3 {
                let want = galley.layout_from_cursor(CCursor::new(idx));
                assert_eq!(rows.layout_cursor(idx), want, "{text:?} idx {idx}");
                let want = galley.pos_from_cursor(CCursor::new(idx));
                assert_eq!(rows.pos(idx), want, "{text:?} idx {idx}");
            }
        }
    }

    #[test]
    fn a_galley_without_rows_matches_too() {
        let mut galley = (*test_galleys(&[("ab\ncd", f32::INFINITY)])[0]).clone();
        galley.rows.clear();
        let rows = GalleyRows::new(&galley);
        for idx in 0..4 {
            let want = galley.layout_from_cursor(CCursor::new(idx));
            assert_eq!(rows.layout_cursor(idx), want);
            assert_eq!(rows.pos(idx), galley.pos_from_cursor(CCursor::new(idx)));
        }
        assert_eq!(rows.chars_meeting_band(0.0, -1.0e6, 1.0e6), None);
    }

    /// Every index whose cursor rect meets the band is inside the span, and the
    /// span is `None` exactly when no row meets it.
    #[test]
    fn chars_meeting_band_covers_every_index_that_meets_it() {
        for (text, galley) in galleys() {
            let rows = GalleyRows::new(&galley);
            let rects: Vec<egui::Rect> = (0..=text.chars().count() + 3)
                .map(|idx| galley.pos_from_cursor(CCursor::new(idx)))
                .collect();
            let h = galley.rect.height();
            for y_offset in [0.0, -13.5, 20.25] {
                for top in [-40.0, -1.0, 0.0, 7.0, h * 0.5, h - 1.0, h, h + 30.0] {
                    for height in [0.0, 5.0, 14.0, 60.0, 1.0e6] {
                        let bottom = top + height;
                        let span = rows.chars_meeting_band(y_offset, top, bottom);
                        for (idx, r) in rects.iter().enumerate() {
                            if y_offset + r.max.y >= top && y_offset + r.min.y <= bottom {
                                assert!(
                                    span.as_ref().is_some_and(|s| s.contains(&idx)),
                                    "{text:?} idx {idx} band {top}..={bottom} span {span:?}"
                                );
                            }
                        }
                        let any_row = galley.rows.iter().any(|row| {
                            y_offset + row.max_y() >= top && y_offset + row.min_y() <= bottom
                        });
                        assert_eq!(span.is_some(), any_row, "{text:?} band {top}..={bottom}");
                    }
                }
            }
        }
    }

    /// The span is the band's rows, not the whole text: that is the point.
    #[test]
    fn chars_meeting_band_stops_at_the_rows_in_the_band() {
        let galley = &test_galleys(&[("ab\ncd\nef", f32::INFINITY)])[0];
        let rows = GalleyRows::new(galley);
        let middle = &galley.rows[1];
        let span = rows.chars_meeting_band(0.0, middle.min_y() + 1.0, middle.max_y() - 1.0);
        assert_eq!(span, Some(3..=5));
        let last = &galley.rows[2];
        let span = rows.chars_meeting_band(0.0, last.min_y() + 1.0, last.max_y() + 100.0);
        assert_eq!(span, Some(6..=usize::MAX));
        let span = rows.chars_meeting_band(0.0, -100.0, galley.rows[0].max_y() - 1.0);
        assert_eq!(span, Some(0..=2));
    }
}

#[cfg(test)]
pub(crate) mod word_walk_tests {
    use super::*;

    /// `line_index_tests` texts, plus pseudo-random ones built from identifier
    /// chars and the separators the word helpers stop at, including an astral
    /// letter and a non-ASCII digit that DO continue an identifier.
    pub(crate) fn word_texts() -> Vec<String> {
        const ALPHABET: &[char] = &[
            'a', 'Z', '_', '7', ' ', '.', ':', '\n', '\r', 'ă', '€', '😀', '𝔸', '٣',
        ];
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let generated = (0..300).map(|_| {
            let len = (next() % 40) as usize;
            (0..len)
                .map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize])
                .collect::<String>()
        });
        super::line_index_tests::texts()
            .into_iter()
            .chain(generated)
            .collect()
    }

    /// Char indices worth asking about: every one, a few past the end, and
    /// `usize::MAX`.
    pub(crate) fn indices(text: &str) -> impl Iterator<Item = usize> {
        (0..=text.chars().count() + 3).chain([usize::MAX])
    }

    /// The implementations these replaced, verbatim.
    fn old_completion_prefix(text: &str, trigger_idx: usize, cursor_idx: usize) -> String {
        if cursor_idx <= trigger_idx {
            return String::new();
        }
        let chars: Vec<char> = text.chars().collect();
        let start = trigger_idx.min(chars.len());
        let end = cursor_idx.min(chars.len());
        chars[start..end]
            .iter()
            .take_while(|&&c| c.is_alphanumeric() || c == '_')
            .collect()
    }

    fn old_word_start(text: &str, end_idx: usize) -> usize {
        let chars: Vec<char> = text.chars().collect();
        let end = end_idx.min(chars.len());
        let mut i = end;
        while i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
            i -= 1;
        }
        i
    }

    #[test]
    fn the_texts_reach_multi_byte_identifier_chars() {
        let texts = word_texts();
        assert!(texts.iter().any(|t| t.contains("𝔸") && t.contains('٣')));
        assert!(is_ident_char('𝔸') && is_ident_char('٣') && is_ident_char('ă'));
        assert!(!is_ident_char('€') && !is_ident_char('😀'));
    }

    #[test]
    fn char_and_byte_at_matches_char_indices() {
        for text in word_texts() {
            let total = text.chars().count();
            for i in indices(&text) {
                assert_eq!(
                    char_and_byte_at(&text, i),
                    (
                        i.min(total),
                        text.char_indices().nth(i).map_or(text.len(), |(b, _)| b)
                    ),
                    "{text:?} char {i}"
                );
            }
        }
    }

    #[test]
    fn lsp_word_start_matches_the_collected_walk() {
        for text in word_texts() {
            for i in indices(&text) {
                assert_eq!(
                    lsp_word_start(&text, i),
                    old_word_start(&text, i),
                    "{text:?} end {i}"
                );
            }
        }
    }

    #[test]
    fn lsp_completion_prefix_matches_the_collected_slice() {
        for text in word_texts() {
            for trigger in indices(&text) {
                for cursor in indices(&text) {
                    assert_eq!(
                        lsp_completion_prefix(&text, trigger, cursor),
                        old_completion_prefix(&text, trigger, cursor),
                        "{text:?} trigger {trigger} cursor {cursor}"
                    );
                }
            }
        }
    }
}
