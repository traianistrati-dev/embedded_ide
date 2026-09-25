//! Keep the RON the IDE writes byte-identical across `ron` versions.
//!
//! ron 0.12 spells a unit variant named `None` (`Parity::None`,
//! `UsartFlow::None`, `ClockDef::None`, ...) as the raw identifier `r#None`;
//! ron 0.8 wrote a bare `None`. Both versions read either spelling back as the
//! same variant, but every file already on disk holds the bare one. Written
//! raw, an untouched project whose mcu.config has a USART module compared as
//! "unsaved" the moment it opened, and its first save put a `None` ->
//! `r#None` diff into Git. `Option::None` is unaffected: it was and stays
//! `None`.

/// `text` with every `r#None` identifier outside a string or char literal
/// written as a bare `None`.
///
/// A lexer rather than a text replace: a module's label or a note may hold the
/// characters `r#None` too, and those must survive untouched.
pub fn bare_none(text: &str) -> String {
    const RAW: &[u8] = b"r#None";
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut copied) = (0, 0);
    while i < b.len() {
        match b[i] {
            b'"' => i = skip_quoted(b, i, b'"'),
            b'\'' => i = skip_quoted(b, i, b'\''),
            b'r' if raw_string_hashes(b, i).is_some() => {
                let hashes = raw_string_hashes(b, i).unwrap_or(0);
                i = skip_raw_string(b, i, hashes);
            }
            b'r' if b[i..].starts_with(RAW)
                && !(i > 0 && is_ident(b[i - 1]))
                && !b.get(i + RAW.len()).copied().is_some_and(is_ident) =>
            {
                out.push_str(&text[copied..i]);
                out.push_str("None");
                i += RAW.len();
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Index just past the literal opened by `quote` at `start`, honouring `\`
/// escapes. An unterminated literal runs to the end of the text.
fn skip_quoted(b: &[u8], start: usize, quote: u8) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    b.len()
}

/// `Some(hashes)` when a raw string (`r"`, `r#"`, `r##"`, ...) opens at `i`.
/// `r#None` is not one: a raw string needs a quote after its hashes.
fn raw_string_hashes(b: &[u8], i: usize) -> Option<usize> {
    if i > 0 && is_ident(b[i - 1]) {
        return None;
    }
    let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
    (b.get(i + 1 + hashes) == Some(&b'"')).then_some(hashes)
}

/// Index just past the raw string that opens at `start` with `hashes` hashes.
fn skip_raw_string(b: &[u8], start: usize, hashes: usize) -> usize {
    let mut i = start + 2 + hashes;
    while i < b.len() {
        if b[i] == b'"'
            && b[i + 1..]
                .iter()
                .take(hashes)
                .filter(|&&c| c == b'#')
                .count()
                == hashes
        {
            return i + 1 + hashes;
        }
        i += 1;
    }
    b.len()
}

#[cfg(test)]
mod tests {
    use super::bare_none;

    #[test]
    fn a_raw_none_variant_is_written_bare() {
        assert_eq!(bare_none("parity: r#None,\n"), "parity: None,\n");
        assert_eq!(
            bare_none("(a:r#None,b:Some(r#None))"),
            "(a:None,b:Some(None))"
        );
        assert_eq!(bare_none("[r#None, r#None]"), "[None, None]");
    }

    #[test]
    fn strings_and_chars_are_left_alone() {
        let s = "label: \"x: r#None,\",\nnote: \"say \\\"r#None\\\"\",\nc: '\"',\nflow: r#None,";
        assert_eq!(
            bare_none(s),
            "label: \"x: r#None,\",\nnote: \"say \\\"r#None\\\"\",\nc: '\"',\nflow: None,"
        );
        assert_eq!(
            bare_none("x: r#\"r#None\"#, y: r#None"),
            "x: r#\"r#None\"#, y: None"
        );
        assert_eq!(bare_none("x: r\"r#None\""), "x: r\"r#None\"");
    }

    #[test]
    fn only_the_whole_identifier_is_rewritten() {
        assert_eq!(
            bare_none("a: r#Nonesuch, b: xr#None"),
            "a: r#Nonesuch, b: xr#None"
        );
        assert_eq!(bare_none("a: r#Some"), "a: r#Some");
    }

    #[test]
    fn the_bare_spelling_reads_back_as_the_variant() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        enum Parity {
            None,
            Even,
        }
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct Cfg {
            parity: Parity,
            maybe: Option<Parity>,
            label: String,
        }
        let cfg = Cfg {
            parity: Parity::None,
            maybe: Some(Parity::None),
            label: "r#None".into(),
        };
        let raw = ron::ser::to_string_pretty(&cfg, ron::ser::PrettyConfig::new()).unwrap();
        let bare = bare_none(&raw);
        assert!(
            !bare.contains(": r#None") && bare.contains("\"r#None\""),
            "{bare}"
        );
        assert_eq!(ron::from_str::<Cfg>(&bare).unwrap(), cfg);
    }
}
