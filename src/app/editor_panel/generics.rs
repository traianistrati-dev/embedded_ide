//! Unused generic-parameter detection for the displayed `.rs` file.
//!
//! Companion to [`usages`](super::usages): that module fades never-referenced
//! *items* (fn/struct/enum/…) using rust-analyzer's `references`. Generic
//! parameters can't go through the same pipeline — RA's `documentSymbol` never
//! reports them — and asking `references` for each one would add a whole-crate
//! query per parameter to an already deliberately serialized queue.
//!
//! So this analysis is **purely syntactic and local**, which a generic parameter
//! allows: its scope is exactly its own item, and there is no way to use one
//! without writing its name inside that item. That makes the check cheap enough
//! to re-run on every edit (no debounce, no staleness window — the ranges are
//! derived from the very text being drawn).
//!
//! What counts as a *use* is the subtle part. A parameter's own declaration and
//! its `where` predicate are declaration syntax, not uses:
//!
//! ```ignore
//! fn f<T>(x: u8) where T: Display {}   // T is UNUSED: decl + where subject only
//! fn f<T, U: Into<T>>(u: U) {}         // T IS used — by U's bound
//! ```
//!
//! Everything else that mentions the name counts, so the analysis errs toward
//! *not* fading: a name collision (a module-level `T` used in the body) merely
//! loses us one fade, it never dims live code.

/// Char-index `[start, end)` ranges of generic parameters (and their `where`
/// predicates) that are declared but never used, ready to append to the
/// editor's dead-range list.
pub fn unused_generic_ranges(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mask = code_mask(&chars);
    let mut out = Vec::new();

    for open in item_generic_lists(&chars, &mask) {
        let Some(close) = match_angle(&chars, &mask, open) else {
            continue; // unbalanced / not really a generic list
        };
        let (where_start, body_start, item_end) = item_layout(&chars, &mask, close + 1);
        let params = split_params(&chars, &mask, open + 1, close);
        if params.is_empty() {
            continue;
        }
        let predicates = where_start
            .map(|w| where_predicates(&chars, &mask, w + "where".len(), body_start))
            .unwrap_or_default();

        for p in &params {
            let name: String = chars[p.name.0..p.name.1].iter().collect();
            // Declaration-side occurrences: the parameter's own name in `<…>`
            // and the subject of each of its `where` predicates.
            let mut skip: Vec<(usize, usize)> = vec![p.name];
            let mut own_preds: Vec<(usize, usize)> = Vec::new();
            for pred in &predicates {
                if chars[pred.subject.0..pred.subject.1]
                    .iter()
                    .collect::<String>()
                    == name
                {
                    skip.push(pred.subject);
                    own_preds.push(pred.span);
                }
            }
            let used = find_word(&chars, &mask, &name, open, item_end)
                .any(|at| !skip.iter().any(|&(s, _)| s == at));
            if !used {
                out.push(p.span);
                out.extend(own_preds);
            }
        }
    }

    out.sort_unstable();
    out
}

// ── Lexical mask ──────────────────────────────────────────────────────────────

/// `false` for every char that is inside a comment, a string (plain, byte or
/// raw) or a char literal — i.e. everything that must not be read as code.
/// Lifetimes stay `true`: `'a` is code, `'a'` is not.
fn code_mask(chars: &[char]) -> Vec<bool> {
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

// ── Item / generic-list parsing ───────────────────────────────────────────────

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

/// End index of the identifier starting at `i`.
fn ident_end(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && is_ident_char(chars[i]) {
        i += 1;
    }
    i
}

/// Skip whitespace and anything masked out (comments), from `i`.
fn skip_trivia(chars: &[char], mask: &[bool], mut i: usize) -> usize {
    while i < chars.len() && (!mask[i] || chars[i].is_whitespace()) {
        i += 1;
    }
    i
}

/// Items that can carry a generic list. `impl` is handled separately — its `<`
/// follows the keyword directly, with no name in between.
const ITEM_KEYWORDS: [&str; 6] = ["fn", "struct", "enum", "union", "trait", "type"];

/// Indices of every `<` that opens an *item's* generic parameter list. Anything
/// else that looks like `<` (comparisons, turbofish, type arguments) is skipped
/// because it isn't preceded by an item keyword + name.
fn item_generic_lists(chars: &[char], mask: &[bool]) -> Vec<usize> {
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if !mask[i] || !is_ident_start(chars[i]) || (i > 0 && is_ident_char(chars[i - 1])) {
            i += 1;
            continue;
        }
        let end = ident_end(chars, i);
        let word: String = chars[i..end].iter().collect();
        i = end;

        let open = if word == "impl" {
            let j = skip_trivia(chars, mask, end);
            (chars.get(j) == Some(&'<')).then_some(j)
        } else if ITEM_KEYWORDS.contains(&word.as_str()) {
            let name = skip_trivia(chars, mask, end);
            if name < n && is_ident_start(chars[name]) {
                let j = skip_trivia(chars, mask, ident_end(chars, name));
                (chars.get(j) == Some(&'<')).then_some(j)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(o) = open {
            out.push(o);
        }
    }
    out
}

/// Index of the `>` closing the list opened at `open`, or `None` if the text
/// doesn't look like a generic list after all. `->` / `=>` are not closers, and
/// nesting inside `( ) [ ] { }` is tracked so `Fn(A) -> B` and `T = [u8; 4]`
/// parse. A `;` or `{` at bracket depth 0 means we mis-detected the `<` (it was
/// a comparison) — bail rather than run away over the rest of the file.
fn match_angle(chars: &[char], mask: &[bool], open: usize) -> Option<usize> {
    let mut angle = 0i32;
    let mut bracket = 0i32;
    let mut i = open;
    while i < chars.len() {
        if !mask[i] {
            i += 1;
            continue;
        }
        match chars[i] {
            '<' if bracket == 0 => angle += 1,
            '>' if bracket == 0 => {
                let arrow = i > 0 && matches!(chars[i - 1], '-' | '=');
                if !arrow {
                    angle -= 1;
                    if angle == 0 {
                        return Some(i);
                    }
                }
            }
            '(' | '[' | '{' => bracket += 1,
            ')' | ']' | '}' => {
                bracket -= 1;
                if bracket < 0 {
                    return None;
                }
            }
            ';' if bracket == 0 => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// `(where_keyword, body_start, item_end)` for an item whose generic list ends
/// just before `from`. `body_start` is the `{` or `;` that follows the header;
/// `item_end` is one past the item's last char.
fn item_layout(chars: &[char], mask: &[bool], from: usize) -> (Option<usize>, usize, usize) {
    let n = chars.len();
    let mut where_kw = None;
    let mut bracket = 0i32;
    let mut i = from;
    while i < n {
        if !mask[i] {
            i += 1;
            continue;
        }
        let c = chars[i];
        if bracket == 0 && is_ident_start(c) && (i == 0 || !is_ident_char(chars[i - 1])) {
            let end = ident_end(chars, i);
            if where_kw.is_none() && chars[i..end].iter().collect::<String>() == "where" {
                where_kw = Some(i);
            }
            i = end;
            continue;
        }
        match c {
            '(' | '[' => bracket += 1,
            ')' | ']' => bracket -= 1,
            '{' if bracket == 0 => return (where_kw, i, block_end(chars, mask, i)),
            ';' if bracket == 0 => return (where_kw, i, i + 1),
            _ => {}
        }
        i += 1;
    }
    (where_kw, n, n)
}

/// One past the `}` matching the `{` at `open`.
fn block_end(chars: &[char], mask: &[bool], open: usize) -> usize {
    let mut depth = 0i32;
    let mut i = open;
    while i < chars.len() {
        if mask[i] {
            match chars[i] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    chars.len()
}

// ── Parameters and where predicates ───────────────────────────────────────────

/// One declared generic parameter.
struct Param {
    /// The parameter's name, `'a` included for lifetimes.
    name: (usize, usize),
    /// The whole declaration — bounds and default included — so fading it dims
    /// `C: Display + Clone` as a unit.
    span: (usize, usize),
}

/// One `where` predicate: its subject (`T` in `T: Display`) and its full span.
struct Predicate {
    subject: (usize, usize),
    span: (usize, usize),
}

/// Split `[start, end)` at commas that are at nesting depth 0.
fn split_top_level(chars: &[char], mask: &[bool], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut seg = start;
    let mut i = start;
    while i < end {
        if !mask[i] {
            i += 1;
            continue;
        }
        match chars[i] {
            '<' | '(' | '[' | '{' => depth += 1,
            '>' | ')' | ']' | '}' => {
                // `->` / `=>` are not brackets.
                if !(chars[i] == '>' && i > 0 && matches!(chars[i - 1], '-' | '=')) {
                    depth -= 1;
                }
            }
            ',' if depth == 0 => {
                out.push((seg, i));
                seg = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push((seg, end));
    out
}

/// Trim whitespace / comments off both ends of `[s, e)`.
fn trim_span(chars: &[char], mask: &[bool], s: usize, e: usize) -> (usize, usize) {
    let mut a = skip_trivia(chars, mask, s).min(e);
    let mut b = e;
    while b > a && (!mask[b - 1] || chars[b - 1].is_whitespace()) {
        b -= 1;
    }
    if a > b {
        a = b;
    }
    (a, b)
}

/// The declared parameters between `<` and `>`.
fn split_params(chars: &[char], mask: &[bool], start: usize, end: usize) -> Vec<Param> {
    let mut out = Vec::new();
    for (s, e) in split_top_level(chars, mask, start, end) {
        let (s, e) = trim_span(chars, mask, s, e);
        if s >= e {
            continue; // empty (trailing comma)
        }
        // `'a` lifetime, `const N: usize`, or a plain type parameter.
        let name = if chars[s] == '\'' {
            (s, ident_end(chars, s + 1))
        } else {
            let first_end = ident_end(chars, s);
            if chars[s..first_end].iter().collect::<String>() == "const" {
                let n = skip_trivia(chars, mask, first_end);
                (n, ident_end(chars, n))
            } else {
                (s, first_end)
            }
        };
        if name.0 >= name.1 {
            continue;
        }
        out.push(Param { name, span: (s, e) });
    }
    out
}

/// The predicates of a `where` clause spanning `[start, end)`.
fn where_predicates(chars: &[char], mask: &[bool], start: usize, end: usize) -> Vec<Predicate> {
    let mut out = Vec::new();
    for (s, e) in split_top_level(chars, mask, start, end.min(chars.len())) {
        let (s, e) = trim_span(chars, mask, s, e);
        if s >= e {
            continue;
        }
        // A higher-ranked `for<'x>` binder introduces its own lifetimes — step
        // over it so the subject is the type that follows.
        let mut subj = s;
        if chars[subj..ident_end(chars, subj)]
            .iter()
            .collect::<String>()
            == "for"
        {
            let j = skip_trivia(chars, mask, ident_end(chars, subj));
            if chars.get(j) == Some(&'<')
                && let Some(close) = match_angle(chars, mask, j)
            {
                subj = skip_trivia(chars, mask, close + 1);
            }
        }
        let subject = if chars.get(subj) == Some(&'\'') {
            (subj, ident_end(chars, subj + 1))
        } else {
            (subj, ident_end(chars, subj))
        };
        // Only a *bare* parameter name is declaration syntax. `Vec<T>: Clone`
        // mentions `T` for real, so its subject is left to count as a use.
        let after = skip_trivia(chars, mask, subject.1);
        if subject.0 < subject.1 && chars.get(after) == Some(&':') {
            out.push(Predicate {
                subject,
                span: (s, e),
            });
        }
    }
    out
}

/// Positions of `name` occurring as a whole word (identifier or lifetime) in
/// `[from, to)`, skipping strings and comments.
fn find_word<'a>(
    chars: &'a [char],
    mask: &'a [bool],
    name: &'a str,
    from: usize,
    to: usize,
) -> impl Iterator<Item = usize> + 'a {
    let pat: Vec<char> = name.chars().collect();
    let to = to.min(chars.len());
    (from..to).filter(move |&i| {
        if !mask[i] || i + pat.len() > to {
            return false;
        }
        if chars[i..i + pat.len()] != pat[..] {
            return false;
        }
        // A lifetime's own `'` is the left boundary; an identifier needs one.
        let left_ok = pat[0] == '\'' || i == 0 || !is_ident_char(chars[i - 1]);
        let right_ok = chars.get(i + pat.len()).is_none_or(|&c| !is_ident_char(c));
        left_ok && right_ok
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The source slices reported as unused, in order.
    fn unused(src: &str) -> Vec<String> {
        let chars: Vec<char> = src.chars().collect();
        unused_generic_ranges(src)
            .into_iter()
            .map(|(s, e)| chars[s..e].iter().collect())
            .collect()
    }

    #[test]
    fn flags_only_the_unused_type_param() {
        assert_eq!(unused("fn my_func<A, B, C>(a: A, b: B) {}"), ["C"]);
    }

    #[test]
    fn all_params_used_flags_nothing() {
        assert!(unused("fn f<A, B>(a: A) -> B { todo!() }").is_empty());
        assert!(unused("impl<T> Foo<T> {}").is_empty());
        assert!(unused("struct S<T> { x: PhantomData<T> }").is_empty());
    }

    #[test]
    fn param_used_only_by_another_bound_is_used() {
        assert!(unused("fn f<T: Into<U>, U>(x: T) -> U { x.into() }").is_empty());
        // …and the reverse: T appears only inside U's bound.
        assert!(unused("fn f<T, U: Into<T>>(u: U) {}").is_empty());
    }

    #[test]
    fn phantom_less_struct_param_is_flagged() {
        // `struct S<T>;` is E0392 in rustc — we still fade it.
        assert_eq!(unused("struct S<T>;"), ["T"]);
        assert_eq!(unused("struct S<T>(u8);"), ["T"]);
    }

    #[test]
    fn bounds_are_faded_with_the_param() {
        assert_eq!(
            unused("fn f<C: Display + Clone>(x: u8) {}"),
            ["C: Display + Clone"]
        );
    }

    #[test]
    fn where_clause_is_declaration_not_use() {
        // `where T: Display` must NOT rescue T — and the predicate fades too.
        assert_eq!(
            unused("fn f<T>(x: u8) where T: Display {}"),
            ["T", "T: Display"]
        );
    }

    #[test]
    fn where_clause_bound_side_counts_as_use() {
        assert!(unused("fn f<T, U>(u: U) where U: Into<T> {}").is_empty());
    }

    #[test]
    fn where_on_a_type_expression_is_a_use() {
        // `Vec<T>: Clone` genuinely mentions T.
        assert!(unused("fn f<T>(x: u8) where Vec<T>: Clone {}").is_empty());
    }

    #[test]
    fn hrtb_where_predicate_subject_is_found() {
        assert_eq!(
            unused("fn f<T>(x: u8) where for<'x> T: Fn(&'x u8) {}"),
            ["T", "for<'x> T: Fn(&'x u8)"]
        );
    }

    #[test]
    fn lifetimes_are_analysed_too() {
        assert_eq!(unused("fn f<'a>(x: &str) {}"), ["'a"]);
        assert!(unused("fn f<'a>(x: &'a str) {}").is_empty());
        assert!(unused("struct S<'a> { x: &'a u8 }").is_empty());
        // `'ab` must not be matched by a search for `'a`.
        assert_eq!(unused("fn f<'a>(x: &'ab str) {}"), ["'a"]);
    }

    #[test]
    fn lifetime_bounds_count_as_uses() {
        assert!(unused("struct S<'a, 'b: 'a> { x: &'b u8 }").is_empty());
    }

    #[test]
    fn const_generics() {
        assert!(unused("fn f<const N: usize>() -> [u8; N] { [0; N] }").is_empty());
        assert_eq!(unused("fn f<const N: usize>() {}"), ["const N: usize"]);
    }

    #[test]
    fn turbofish_and_comparisons_are_not_declarations() {
        assert!(unused("let v = Vec::<u8>::new();").is_empty());
        assert!(unused("if a < b && c > d { g(); }").is_empty());
        assert!(unused("let t = (a < b, c > d);").is_empty());
    }

    #[test]
    fn comments_and_strings_do_not_count_as_uses() {
        assert_eq!(unused("fn f<T>(x: u8) { /* T */ let s = \"T\"; }"), ["T"]);
        assert_eq!(unused("/// Takes a T.\nfn f<T>(x: u8) {}"), ["T"]);
        assert_eq!(unused("fn f<T>(x: u8) { let s = r#\"T\"#; }"), ["T"]);
    }

    #[test]
    fn char_literal_is_not_a_lifetime() {
        // The `'a'` literal must not be read as a use of `'a`.
        assert_eq!(unused("fn f<'a>(x: u8) { let c = 'a'; }"), ["'a"]);
    }

    #[test]
    fn nested_items_are_independent() {
        let src = "impl<T> Foo<T> {\n    fn bar<U>(&self) {}\n}";
        assert_eq!(unused(src), ["U"]);
    }

    #[test]
    fn fn_trait_bound_with_arrow_parses() {
        assert!(unused("fn f<T: Fn(u8) -> u8>(t: T) {}").is_empty());
        assert_eq!(
            unused("fn f<T: Fn(u8) -> u8>(x: u8) {}"),
            ["T: Fn(u8) -> u8"]
        );
    }

    #[test]
    fn default_type_param_with_array() {
        assert_eq!(unused("struct S<T = [u8; 4]>(u8);"), ["T = [u8; 4]"]);
    }

    #[test]
    fn trait_method_declaration_without_body() {
        let src = "trait Tr {\n    fn f<T>(&self, x: u8);\n}";
        assert_eq!(unused(src), ["T"]);
    }

    #[test]
    fn type_alias_and_enum() {
        assert_eq!(unused("type Alias<T> = u8;"), ["T"]);
        assert!(unused("type Alias<T> = Vec<T>;").is_empty());
        assert_eq!(unused("enum E<T> { A, B }"), ["T"]);
        assert!(unused("enum E<T> { A(T), B }").is_empty());
    }

    #[test]
    fn multiple_items_report_each() {
        let src = "fn a<T>(x: u8) {}\nfn b<U>(u: U) {}\nfn c<V>(x: u8) {}";
        assert_eq!(unused(src), ["T", "V"]);
    }

    #[test]
    fn incomplete_generic_list_is_ignored() {
        // Mid-typing: no closing `>` yet — must not panic or fade anything.
        assert!(unused("fn f<T").is_empty());
        assert!(unused("fn f<T,").is_empty());
    }
}
