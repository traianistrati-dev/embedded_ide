//! `Ctrl+U` — toggle the selection between UPPER and lower case.
//!
//! The target of each caret is its selection, or — with nothing selected — the
//! identifier under it, so `SETTINGS_MODES` needs no selecting first. One
//! direction is chosen for every target together: any letter that the rule
//! below would change to uppercase means UPPER, otherwise lower. A second press
//! therefore undoes the first, which is what the key is for. Mixed case is lossy
//! (`CamelCase` → `CAMELCASE` → `camelcase`) by design, and Ctrl+Z recovers it —
//! except when the press had to unfold the file, because unfolding resets the
//! undo history (a limitation every line op shares).
//!
//! **A letter changes only when its mapping is one-to-one AND round-trips.**
//! - One-to-one: `'ß'.to_uppercase()` is `"SS"` and `'İ'.to_lowercase()` is two
//!   chars. Left alone, the text never changes length, so every char index the
//!   editor holds — the selection, the extra carets, the fold headers, the
//!   completion anchor — stays valid without a single one being shifted.
//! - Round-trips: MICRO SIGN `U+00B5` uppercases to Greek capital mu, which
//!   lowercases to Greek small mu `U+03BC` — a different codepoint that looks
//!   identical. Two presses would swap it silently inside a comment, or a string
//!   an LCD font table reads. Such letters (MICRO SIGN, OHM SIGN `U+2126`,
//!   KELVIN SIGN `U+212A`, `ſ`, `ı`, final `ς`) are left alone too.

use super::word_select::ident_run_at;

/// Toggle the case of every caret's target in `text`.
///
/// `carets` are `(anchor, head)` char indices in either order; out-of-range ones
/// are clamped. `None` when nothing would change (no target, no letters, or
/// letters without a case), so the caller can skip the edit — and the unfold
/// that an edit on a folded file costs.
pub(super) fn toggle_case(text: &str, carets: &[(usize, usize)]) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let spans = targets(&chars, carets);
    let in_spans = || spans.iter().flat_map(|&(s, e)| chars[s..e].iter().copied());
    // Tested on the one-to-one mapping actually applied: `char::is_lowercase`
    // is true for `ª` and `º`, which have no uppercase form, and a target
    // holding one would stay on the UPPER branch forever, changing nothing.
    let upper = in_spans().any(|c| to_upper(c) != c);
    let convert: fn(char) -> char = if upper { to_upper } else { to_lower };

    let mut out = chars.clone();
    for &(s, e) in &spans {
        for c in &mut out[s..e] {
            *c = convert(*c);
        }
    }
    (out != chars).then(|| out.into_iter().collect())
}

/// Each caret's span `[start, end)`. Overlaps need no merging: the conversion
/// has one fixed direction rather than flipping each char, so a char two carets
/// share comes out the same either way.
fn targets(chars: &[char], carets: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let n = chars.len();
    carets
        .iter()
        .filter_map(|&(a, b)| {
            let (lo, hi) = (a.min(b).min(n), a.max(b).min(n));
            if lo < hi {
                Some((lo, hi))
            } else {
                ident_run_at(chars, lo)
            }
        })
        .collect()
}

fn to_upper(c: char) -> char {
    let m = one_to_one(c.to_uppercase(), c);
    if one_to_one(m.to_lowercase(), m) == c {
        m
    } else {
        c
    }
}

fn to_lower(c: char) -> char {
    let m = one_to_one(c.to_lowercase(), c);
    if one_to_one(m.to_uppercase(), m) == c {
        m
    } else {
        c
    }
}

/// The mapping's single char, or `c` itself when it expands to several.
fn one_to_one(mut mapped: impl Iterator<Item = char>, c: char) -> char {
    match (mapped.next(), mapped.next()) {
        (Some(m), None) => m,
        _ => c,
    }
}

#[cfg(test)]
mod tests {
    use super::toggle_case;

    #[test]
    fn a_snake_case_selection_round_trips() {
        let src = "let settings_modes_options = 1;";
        let (lo, hi) = (4, 26);
        let once = toggle_case(src, &[(lo, hi)]).unwrap();
        assert_eq!(once, "let SETTINGS_MODES_OPTIONS = 1;");
        assert_eq!(toggle_case(&once, &[(lo, hi)]).unwrap(), src);
    }

    #[test]
    fn a_backwards_selection_is_the_same_selection() {
        assert_eq!(toggle_case("abc def", &[(3, 0)]).unwrap(), "ABC def");
    }

    #[test]
    fn mixed_case_goes_up_first() {
        assert_eq!(toggle_case("CamelCase", &[(0, 9)]).unwrap(), "CAMELCASE");
        assert_eq!(toggle_case("CAMELCASE", &[(0, 9)]).unwrap(), "camelcase");
    }

    #[test]
    fn no_letters_changes_nothing() {
        assert_eq!(toggle_case("x = 123_456;", &[(4, 11)]), None);
    }

    #[test]
    fn a_bare_caret_toggles_the_identifier_under_it() {
        let src = "call(max_speed);";
        // Inside, at the start, and just past the end of `max_speed`.
        for caret in [7, 5, 14] {
            assert_eq!(
                toggle_case(src, &[(caret, caret)]).unwrap(),
                "call(MAX_SPEED);",
                "caret at {caret}"
            );
        }
    }

    #[test]
    fn a_bare_caret_on_punctuation_or_space_does_nothing() {
        assert_eq!(toggle_case("a + b", &[(2, 2)]), None);
        assert_eq!(toggle_case("", &[(0, 0)]), None);
    }

    #[test]
    fn only_the_target_is_touched() {
        let src = "foo bar baz";
        assert_eq!(toggle_case(src, &[(4, 7)]).unwrap(), "foo BAR baz");
    }

    #[test]
    fn a_multi_line_selection_keeps_its_line_structure() {
        let src = "fn a() {\n    b();\n}\n";
        let out = toggle_case(src, &[(0, src.chars().count())]).unwrap();
        assert_eq!(out, "FN A() {\n    B();\n}\n");
    }

    #[test]
    fn romanian_letters_toggle() {
        assert_eq!(toggle_case("țară și", &[(0, 7)]).unwrap(), "ȚARĂ ȘI");
        assert_eq!(toggle_case("ȚARĂ ȘI", &[(0, 7)]).unwrap(), "țară și");
    }

    /// The length invariant everything else relies on: letters whose case
    /// mapping expands are left alone, the rest still toggle.
    #[test]
    fn expanding_letters_are_kept_and_the_length_never_changes() {
        let src = "straße";
        let out = toggle_case(src, &[(0, 6)]).unwrap();
        assert_eq!(out, "STRAßE");
        assert_eq!(out.chars().count(), src.chars().count());

        let dotted = "İSTANBUL";
        let out = toggle_case(dotted, &[(0, 8)]).unwrap();
        assert_eq!(out, "İstanbul");
        assert_eq!(out.chars().count(), dotted.chars().count());
    }

    /// Look-alike letters would come back as a DIFFERENT codepoint after two
    /// presses (MICRO SIGN -> Greek capital mu -> Greek small mu), invisibly.
    /// They stay; their neighbours toggle.
    #[test]
    fn letters_that_do_not_round_trip_are_kept() {
        // MICRO SIGN and OHM SIGN spelled out: the Greek letters look the same.
        let src = "delay_\u{b5}s \u{2126}";
        let up = toggle_case(src, &[(0, 10)]).unwrap();
        assert_eq!(up, "DELAY_\u{b5}S \u{2126}");
        assert_eq!(toggle_case(&up, &[(0, 10)]).unwrap(), src);
        assert_eq!(toggle_case("ſ ı ς", &[(0, 5)]), None);
    }

    /// `ª` reports `is_lowercase()` but has no uppercase form. A rule built on
    /// `is_lowercase` would pick UPPER forever and never change `ªBC`.
    #[test]
    fn a_caseless_lowercase_letter_does_not_pin_the_direction() {
        assert_eq!(toggle_case("ªBC", &[(0, 3)]).unwrap(), "ªbc");
        assert_eq!(toggle_case("ß", &[(0, 1)]), None);
    }

    #[test]
    fn every_caret_goes_the_same_way() {
        // One target is already upper, the other is not: both end up UPPER,
        // rather than each caret flipping on its own.
        let src = "ABC def";
        assert_eq!(toggle_case(src, &[(0, 3), (4, 7)]).unwrap(), "ABC DEF");
        assert_eq!(
            toggle_case("ABC DEF", &[(0, 3), (4, 7)]).unwrap(),
            "abc def"
        );
    }

    #[test]
    fn carets_mix_selections_and_bare_positions() {
        let src = "one two three";
        // A selection on `one`, a bare caret inside `three`.
        assert_eq!(
            toggle_case(src, &[(0, 3), (10, 10)]).unwrap(),
            "ONE two THREE"
        );
    }

    /// A per-char FLIP would undo itself where two targets overlap.
    #[test]
    fn overlapping_targets_do_not_cancel_out() {
        assert_eq!(toggle_case("abcdef", &[(0, 4), (2, 6)]).unwrap(), "ABCDEF");
    }

    #[test]
    fn stale_indices_are_clamped_not_a_panic() {
        assert_eq!(toggle_case("abc", &[(1, 99)]).unwrap(), "aBC");
        assert_eq!(toggle_case("abc", &[(99, 99)]).unwrap(), "ABC");
    }
}
