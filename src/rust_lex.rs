//! The one lexical mask over Rust source: which characters are CODE.
//!
//! Hand-written scanners that hunt for a `;`, a `{` or a brace pair keep getting
//! the same thing wrong — the character they look for also appears inside
//! comments and string literals. This crate has paid for that four times: the
//! inferred-type hint gave up on any `let` with a comment above it, the
//! triple-click definition selection began in the middle of a doc comment, and
//! the Structure diagram silently dropped a top-level `fn` when a block comment
//! contained a brace.
//!
//! `fold::regions` and `format`'s lexer keep their own scanners — they return
//! regions and drive a formatter rather than answering "is this char code?" —
//! but everything that only needs the question answered should ask here.

/// `text` with every comment and string-literal character replaced by a space.
///
/// Line-based scanners want this rather than the raw mask: blanking is 1:1, so
/// line numbers, line lengths and column offsets all survive, and a caller can
/// keep splitting into lines exactly as before.
///
/// Line breaks are preserved even inside a block comment, which the mask itself
/// marks as non-code. Blanking those would merge every line of a block comment
/// into one and shift every line number after it.
pub fn blank_non_code(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mask = code_mask(&chars);
    chars
        .iter()
        .zip(&mask)
        .map(|(&c, &is_code)| {
            if is_code || c == '\n' || c == '\r' {
                c
            } else {
                ' '
            }
        })
        .collect()
}

/// `true` for a character that can appear inside an identifier.
///
/// A private three-token copy rather than an import: this module exists to
/// serve the editor, not to depend on it.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `false` for every char that is inside a comment, a string (plain, byte or
/// raw) or a char literal — i.e. everything that must not be read as code.
/// Lifetimes stay `true`: `'a` is code, `'a'` is not.
pub fn code_mask(chars: &[char]) -> Vec<bool> {
    let n = chars.len();
    let mut mask = vec![true; n];
    let mut i = 0;
    while i < n {
        let c = chars[i];

        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < n && chars[i] != '\n' {
                mask[i] = false;
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            // Rust block comments nest.
            let mut depth = 0usize;
            while i < n {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    mask[i] = false;
                    mask[i + 1] = false;
                    i += 2;
                    continue;
                }
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    mask[i] = false;
                    mask[i + 1] = false;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                mask[i] = false;
                i += 1;
            }
            continue;
        }
        if let Some((hashes, quote)) = raw_string_at(chars, i) {
            for m in mask.iter_mut().take(quote + 1).skip(i) {
                *m = false;
            }
            i = quote + 1;
            // Ends at `"` followed by exactly `hashes` hashes.
            while i < n {
                let closes =
                    chars[i] == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#'));
                mask[i] = false;
                if closes {
                    for k in 1..=hashes {
                        if i + k < n {
                            mask[i + k] = false;
                        }
                    }
                    i += 1 + hashes;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if c == '"' {
            mask[i] = false;
            i += 1;
            while i < n {
                if chars[i] == '\\' {
                    mask[i] = false;
                    if i + 1 < n {
                        mask[i + 1] = false;
                    }
                    i += 2;
                    continue;
                }
                let end = chars[i] == '"';
                mask[i] = false;
                i += 1;
                if end {
                    break;
                }
            }
            continue;
        }
        if c == '\'' {
            if let Some(end) = char_literal_end(chars, i) {
                for m in mask.iter_mut().take(end + 1).skip(i) {
                    *m = false;
                }
                i = end + 1;
            } else {
                i += 1; // a lifetime — real code
            }
            continue;
        }
        i += 1;
    }
    mask
}

/// End index (the closing `'`) of the char literal starting at `i`, or `None`
/// when the quote opens a lifetime instead. Escapes are scanned within a short
/// window, which covers `'\n'` and `'\u{1F600}'` alike.
fn char_literal_end(chars: &[char], i: usize) -> Option<usize> {
    match chars.get(i + 1) {
        Some('\\') => (i + 2..(i + 14).min(chars.len())).find(|&j| chars[j] == '\''),
        Some(&c) if c != '\'' => (chars.get(i + 2) == Some(&'\'')).then_some(i + 2),
        _ => None,
    }
}

/// Hash count and opening-quote index of a raw/byte-raw string starting at `i`
/// (`r"`, `r#"`, `br##"`, …). Mirrors the same check in
/// [`format`](super::format), kept separate because that one is line-based.
fn raw_string_at(chars: &[char], i: usize) -> Option<(usize, usize)> {
    if !matches!(chars.get(i), Some('r') | Some('b')) {
        return None;
    }
    if i > 0 && is_ident_char(chars[i - 1]) {
        return None; // tail of an identifier, not a literal prefix
    }
    let mut j = i;
    if chars[j] == 'b' {
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
