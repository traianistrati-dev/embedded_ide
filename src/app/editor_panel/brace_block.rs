//! Brace-block selection: when the user selects a `{` or `}`, highlight the
//! whole `{ … }` block (braces included) with a dark band so it's clearly marked
//! as selected, and copy the block on Ctrl+C.
//!
//! Brace matching skips braces inside strings, char literals, and `//` / `/* */`
//! comments — so the `{}` in `format!("{}", x)` never throws the pairing off.

use crate::app::AppIde;
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

/// Map every matched `{`/`}` to its partner (both directions), ignoring braces
/// in strings / char literals / comments. Unbalanced braces are simply omitted.
fn brace_pairs(chars: &[char]) -> HashMap<usize, usize> {
    let n = chars.len();
    let mut stack: Vec<usize> = Vec::new();
    let mut pairs = HashMap::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        // Line comment.
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // Block comment.
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < n && !(chars[i - 1] == '*' && chars[i] == '/') {
                i += 1;
            }
            if i < n {
                i += 1;
            }
            continue;
        }
        // Raw string.
        if let Some(end) = raw_string_end(chars, i) {
            i = end;
            continue;
        }
        // String / byte string.
        if c == '"' || (c == 'b' && chars.get(i + 1) == Some(&'"')) {
            if c == 'b' {
                i += 1;
            }
            i += 1;
            while i < n {
                match chars[i] {
                    '\\' => i += 2,
                    '"' => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            continue;
        }
        // Char literal vs lifetime.
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\\') {
                i += 2;
                while i < n && chars[i] != '\'' {
                    i += 1;
                }
                if i < n {
                    i += 1;
                }
                continue;
            }
            let next_ident = chars
                .get(i + 1)
                .is_some_and(|&c| c.is_alphabetic() || c == '_');
            if next_ident && chars.get(i + 2) != Some(&'\'') {
                i += 1;
                while i < n && is_ident(chars[i]) {
                    i += 1;
                }
                continue; // lifetime — no braces inside
            }
            i += 1;
            if i < n {
                i += 1;
            }
            if i < n && chars[i] == '\'' {
                i += 1;
            }
            continue;
        }

        if c == '{' {
            stack.push(i);
        } else if c == '}' {
            if let Some(open) = stack.pop() {
                pairs.insert(open, i);
                pairs.insert(i, open);
            }
        }
        i += 1;
    }
    pairs
}

/// The matched block `(open, close)` (inclusive char indices) when the selection
/// `[lo, hi)` is *on* a brace — i.e. its first or last char is `{`/`}`. `None`
/// for an empty selection or one not touching a brace.
fn block_for(chars: &[char], lo: usize, hi: usize) -> Option<(usize, usize)> {
    if lo == hi {
        return None; // only on a real selection
    }
    let is_brace = |c: char| c == '{' || c == '}';
    let idx = if chars.get(lo).is_some_and(|&c| is_brace(c)) {
        lo
    } else if hi > 0 && chars.get(hi - 1).is_some_and(|&c| is_brace(c)) {
        hi - 1
    } else {
        return None;
    };
    let other = *brace_pairs(chars).get(&idx)?;
    Some((idx.min(other), idx.max(other)))
}

/// `&str` wrapper around [`block_for`] for tests.
pub(super) fn brace_block(text: &str, lo: usize, hi: usize) -> Option<(usize, usize)> {
    block_for(&text.chars().collect::<Vec<_>>(), lo, hi)
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

impl AppIde {
    /// When a brace is selected, darken its whole `{ … }` block (braces included)
    /// and, on Ctrl+C, copy the block text (overriding the native selection copy).
    /// Painted after the editor, like the find / word-occurrence overlays.
    pub(super) fn highlight_brace_block(
        &self,
        editor_resp: &egui::text_edit::TextEditOutput,
        display_code: &str,
        clip: egui::Rect,
        ui: &egui::Ui,
        copy_requested: bool,
    ) {
        let Some(range) = editor_resp.state.cursor.char_range() else {
            return;
        };
        let lo = range.primary.index.min(range.secondary.index);
        let hi = range.primary.index.max(range.secondary.index);
        let chars: Vec<char> = display_code.chars().collect();
        let Some((open, close)) = block_for(&chars, lo, hi) else {
            return;
        };

        // Ctrl+C copies the whole block (not just the selected brace).
        if copy_requested && close < chars.len() {
            ui.ctx()
                .copy_text(chars[open..=close].iter().collect::<String>());
        }

        // Dark band over [open, close]. Semi-transparent so the code stays
        // readable through it (the overlay is drawn on top of the text).
        let color = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 50);
        let gp = editor_resp.galley_pos;
        let galley = &editor_resp.galley;
        let painter = ui.painter().with_clip_rect(clip);

        let end = (close + 1).min(chars.len());
        let mut seg_start = open;
        let mut i = open;
        while i < end {
            if chars[i] == '\n' {
                paint_segment(&painter, galley, gp, clip, color, seg_start, i);
                seg_start = i + 1;
            }
            i += 1;
        }
        paint_segment(&painter, galley, gp, clip, color, seg_start, end);
    }
}

#[cfg(test)]
mod tests {
    use super::brace_block;

    #[test]
    fn selecting_open_brace_spans_block() {
        let src = "fn f() { a; }";
        let open = src.find('{').unwrap(); // char idx == byte idx (ASCII)
        let close = src.find('}').unwrap();
        // Selection = exactly the `{`.
        assert_eq!(brace_block(src, open, open + 1), Some((open, close)));
        // Selection = exactly the `}`.
        assert_eq!(brace_block(src, close, close + 1), Some((open, close)));
    }

    #[test]
    fn nested_blocks_match_innermost_selected_brace() {
        let src = "a { b { c } d }";
        let inner_open = src.find("{ c").unwrap();
        let inner_close = src.find('}').unwrap();
        assert_eq!(
            brace_block(src, inner_open, inner_open + 1),
            Some((inner_open, inner_close))
        );
        let outer_open = src.find('{').unwrap();
        let outer_close = src.rfind('}').unwrap();
        assert_eq!(
            brace_block(src, outer_open, outer_open + 1),
            Some((outer_open, outer_close))
        );
    }

    #[test]
    fn braces_in_strings_and_comments_are_ignored() {
        // The `{` in the format string and the `}` in the comment must not pair.
        let src = "{ println!(\"{}\", x); // }\n}";
        let open = 0;
        let close = src.rfind('}').unwrap();
        assert_eq!(brace_block(src, open, open + 1), Some((open, close)));
    }

    #[test]
    fn no_selection_or_non_brace_returns_none() {
        let src = "fn f() { a; }";
        let open = src.find('{').unwrap();
        assert_eq!(brace_block(src, open, open), None); // empty selection
        assert_eq!(brace_block(src, 0, 2), None); // "fn" — no brace
    }
}
