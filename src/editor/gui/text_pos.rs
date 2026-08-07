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
    let chars: Vec<char> = text.chars().collect();
    let start = trigger_idx.min(chars.len());
    let end = cursor_idx.min(chars.len());
    // Only take identifier characters (letters, digits, _).  A dot or space
    // means the user has moved to a new expression — we'll rely on the delta
    // check in the trigger section to close the popup in that case.
    chars[start..end]
        .iter()
        .take_while(|&&c| c.is_alphanumeric() || c == '_')
        .collect()
}

/// Return the char-index of the first character of the identifier that ends at `end_idx`.
pub fn lsp_word_start(text: &str, end_idx: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let end = end_idx.min(chars.len());
    let mut i = end;
    while i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
        i -= 1;
    }
    i
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
pub fn diags_for_file(
    map: &std::collections::HashMap<String, Vec<lsp::LspDiagnostic>>,
    rel_path: &str,
) -> Vec<lsp::LspDiagnostic> {
    // 1. Exact match (ideal case)
    if let Some(v) = map.get(rel_path) {
        return v.clone();
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
            return v.clone();
        }
    }
    Vec::new()
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
    let stroke = egui::Stroke::new(1.2, color);
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
}
