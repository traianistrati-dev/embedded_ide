//! `let` binding type-annotation on completion accept.
//!
//! When a function completion is accepted right after `let my_value = `, the
//! statement is completed to `let my_value: Option<u32> = get_param_value(…);`
//! — the binding gets the function's return type (parsed from rust-analyzer's
//! `detail` signature) and the line is closed with `;`.  Anywhere else the
//! accept inserts just the call, unchanged.
//!
//! Pure text analysis only; the insertion itself happens in `completion.rs`.

/// True for LSP completion kinds that insert a *call* whose return type can
/// annotate a `let` binding: 2 = Method, 3 = Function, 4 = Constructor.
pub fn is_callable_kind(kind: u8) -> bool {
    matches!(kind, 2 | 3 | 4)
}

/// If the text on `word_start`'s line, before `word_start`, is a `let` binding
/// awaiting its value — `let x = `, `let mut x = ` — return the char index
/// right after the binding name, where `: Type` should be inserted.
///
/// Between `=` and `word_start` a partially-typed access path is allowed —
/// `mw_radar::utils::` or `config.parser.` (plain identifiers joined by `::` or
/// `.`) — because the completion replaces only the last segment and the
/// finished call's return type is still the type of the whole `let` value.
///
/// Returns `None` for anything else: already-annotated bindings (`let x: T =`),
/// destructuring (`let (a, b) =`), compound expressions (`let x = 1 + `,
/// `let x = foo(`), or plain non-`let` positions.
pub fn let_context(chars: &[char], word_start: usize) -> Option<usize> {
    let line_start = chars[..word_start]
        .iter()
        .rposition(|&c| c == '\n')
        .map(|p| p + 1)
        .unwrap_or(0);

    let mut i = line_start;
    let skip_ws = |i: &mut usize| {
        while *i < word_start && chars[*i].is_whitespace() {
            *i += 1;
        }
    };

    skip_ws(&mut i);
    if !keyword_at(chars, i, word_start, "let") {
        return None;
    }
    i += 3;
    skip_ws(&mut i);

    if keyword_at(chars, i, word_start, "mut") {
        i += 3;
        skip_ws(&mut i);
    }

    // Binding name: a single plain identifier.
    let id_start = i;
    if i < word_start && (chars[i] == '_' || chars[i].is_ascii_alphabetic()) {
        i += 1;
        while i < word_start && (chars[i] == '_' || chars[i].is_ascii_alphanumeric()) {
            i += 1;
        }
    }
    if i == id_start {
        return None;
    }
    let annotation_at = i;

    // Exactly `=` (not `==`), then whitespace.
    skip_ws(&mut i);
    if i >= word_start || chars[i] != '=' {
        return None;
    }
    i += 1;
    if i < word_start && chars[i] == '=' {
        return None;
    }
    skip_ws(&mut i);

    // Then either nothing, or a path prefix: `(ident ("::" | "."))*` ending
    // exactly at the insertion point (`let mm = mw_radar::utils::` + accept).
    loop {
        if i == word_start {
            return Some(annotation_at);
        }
        // One plain identifier segment…
        let seg_start = i;
        if chars[i] == '_' || chars[i].is_ascii_alphabetic() {
            i += 1;
            while i < word_start && (chars[i] == '_' || chars[i].is_ascii_alphanumeric()) {
                i += 1;
            }
        }
        if i == seg_start {
            return None;
        }
        // …joined by `::` or `.`.
        if i + 1 < word_start && chars[i] == ':' && chars[i + 1] == ':' {
            i += 2;
        } else if i < word_start && chars[i] == '.' {
            i += 1;
        } else {
            return None;
        }
    }
}

/// `kw` present at `i` (within `end`) and followed by whitespace.
fn keyword_at(chars: &[char], i: usize, end: usize, kw: &str) -> bool {
    let k: Vec<char> = kw.chars().collect();
    i + k.len() < end
        && chars[i..i + k.len()] == k[..]
        && chars[i + k.len()].is_whitespace()
}

/// If the STATEMENT enclosing `cursor` is a `let [mut] <ident> = …` binding
/// WITHOUT an explicit type (no `:` before the `=`), return the char index of
/// `<ident>`.
///
/// rust-analyzer's "Add explicit type" assist is position-sensitive — it's
/// offered only when the cursor is on the `let` pattern, NOT on the initializer
/// expression. Ctrl+Enter therefore re-targets its code-action request to this
/// binding position, so the type-add works from ANYWHERE in the statement —
/// including the continuation lines of a multi-line method chain
/// (`let clocks = rcc.cfgr.sysclk(..)\n .freeze(..);`).
///
/// The statement is found by scanning back to the previous `;` / `{` / `}`
/// (or file start). `None` for already-typed / destructured / non-`let`
/// statements. (Limitation: a `;`/`{`/`}` inside a string/char/comment on the
/// initializer would cut the scan short — rare in the builder chains this
/// targets.)
pub fn let_binding_pos(chars: &[char], cursor: usize) -> Option<usize> {
    let n = chars.len();
    let cursor = cursor.min(n);
    // Start of the enclosing statement: just after the nearest preceding
    // statement / block boundary.
    let mut stmt_start = cursor;
    while stmt_start > 0 && !matches!(chars[stmt_start - 1], ';' | '{' | '}') {
        stmt_start -= 1;
    }

    let mut i = stmt_start;
    let skip_ws = |i: &mut usize| {
        while *i < n && chars[*i].is_whitespace() {
            *i += 1;
        }
    };
    skip_ws(&mut i);
    if !keyword_at(chars, i, n, "let") {
        return None;
    }
    i += 3;
    skip_ws(&mut i);
    if keyword_at(chars, i, n, "mut") {
        i += 3;
        skip_ws(&mut i);
    }
    // Single plain-identifier binding.
    let id_start = i;
    if i < n && (chars[i] == '_' || chars[i].is_ascii_alphabetic()) {
        i += 1;
        while i < n && (chars[i] == '_' || chars[i].is_ascii_alphanumeric()) {
            i += 1;
        }
    }
    if i == id_start {
        return None; // destructuring / no binding name
    }
    // After the name: whitespace, then `=` (not `:` = already typed, not `==`).
    skip_ws(&mut i);
    match chars.get(i) {
        Some('=') if chars.get(i + 1) != Some(&'=') => Some(id_start),
        _ => None,
    }
}

/// Extract the return type from a rust-analyzer signature `detail`, e.g.
/// `fn get_param_value<const N: usize>(tx: &mut T, …) -> Option<u32>` →
/// `Some("Option<u32>")`.
///
/// Bracket-aware: only the first `->` OUTSIDE any `()`/`[]`/`{}`/`<>` counts,
/// so parameter types like `f: fn(A) -> B` don't split, while a return type of
/// `impl Fn(A) -> B` survives whole. `None` when the function returns `()`.
pub fn return_type(detail: &str) -> Option<String> {
    let chars: Vec<char> = detail.chars().collect();
    let mut depth: i32 = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '-' && chars.get(i + 1) == Some(&'>') {
            if depth == 0 {
                let ret: String = chars[i + 2..].iter().collect();
                let ret = ret.trim();
                return (!ret.is_empty()).then(|| ret.to_owned());
            }
            // Nested arrow: consume both chars so its '>' can't close a bracket.
            i += 2;
            continue;
        }
        match chars[i] {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{is_callable_kind, let_binding_pos, let_context, return_type};

    fn binding_pos(text: &str, cursor: usize) -> Option<usize> {
        let_binding_pos(&text.chars().collect::<Vec<_>>(), cursor)
    }

    #[test]
    fn let_binding_pos_targets_the_name_from_anywhere_on_the_line() {
        // The reported case: cursor anywhere on `let mut cp = cortex_m::…`
        // resolves to `cp`'s position (index 8) so RA offers "Add explicit type".
        let src = "let mut cp = cortex_m::Peripherals::take().unwrap();";
        let cp = src.find("cp").unwrap();
        // Cursor on the RHS call (where RA would NOT offer the assist directly).
        let on_call = src.find("take").unwrap();
        assert_eq!(binding_pos(src, on_call), Some(cp));
        // Cursor on the binding name itself.
        assert_eq!(binding_pos(src, cp), Some(cp));
        // `let x = ` on an indented line.
        assert_eq!(binding_pos("    let x = 1;", 10), Some(8));
    }

    #[test]
    fn let_binding_pos_spans_a_multiline_method_chain() {
        // The reported case: cursor on the `.freeze(...)` continuation line of a
        // multi-line `let` still resolves to the binding name so RA offers
        // "Add explicit type".
        let src = "let clocks2 = rcc.cfgr.sysclk(8.MHz())\n\
                   .pclk1(4.MHz())\n\
                   .freeze(&mut flash.acr);";
        let clocks2 = src.find("clocks2").unwrap();
        let on_freeze = src.find("freeze").unwrap();
        let on_pclk1 = src.find("pclk1").unwrap();
        assert_eq!(let_binding_pos(&src.chars().collect::<Vec<_>>(), on_freeze), Some(clocks2));
        assert_eq!(let_binding_pos(&src.chars().collect::<Vec<_>>(), on_pclk1), Some(clocks2));
        // A statement BEFORE this one terminates the backward scan.
        let two = "let a = 1;\nlet b = obj.foo()\n    .bar();";
        let b = two.find('b').unwrap();
        let on_bar = two.find("bar").unwrap();
        assert_eq!(let_binding_pos(&two.chars().collect::<Vec<_>>(), on_bar), Some(b));
    }

    #[test]
    fn let_binding_pos_rejects_typed_destructured_and_non_let() {
        assert_eq!(binding_pos("let x: u32 = 1;", 4), None); // already typed
        assert_eq!(binding_pos("let (a, b) = f();", 5), None); // destructuring
        assert_eq!(binding_pos("x = 1;", 0), None); // not a let
        assert_eq!(binding_pos("let y == z;", 4), None); // comparison, no `=` binding
        // Only the cursor's own line is considered.
        let two = "let a: u8 = 1;\nlet b = 2;";
        let on_b = two.find("b = 2").unwrap();
        assert_eq!(binding_pos(two, on_b), Some(two.find("b = 2").unwrap()));
    }

    fn ctx(text: &str) -> Option<usize> {
        // The insertion point is the end of the text (as when the user triggers
        // completion right after `= `).
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let_context(&chars, len)
    }

    #[test]
    fn plain_let_binding_matches() {
        // `let my_value = ` → annotate right after `my_value` (index 12).
        assert_eq!(ctx("let my_value = "), Some(12));
    }

    #[test]
    fn indented_and_mut_bindings_match() {
        assert_eq!(ctx("    let x = "), Some(9));
        assert_eq!(ctx("let mut count = "), Some(13));
    }

    #[test]
    fn previous_lines_are_ignored() {
        let text = "let a = 1;\n    let b = ";
        assert_eq!(ctx(text), Some(20)); // right after `b` on the second line
    }

    #[test]
    fn no_space_after_equals_still_matches() {
        assert_eq!(ctx("let x ="), Some(5));
    }

    #[test]
    fn qualified_path_and_method_chain_prefixes_match() {
        // The completion replaces only the last path segment, so the finished
        // call still types the whole `let` value.
        assert_eq!(ctx("let mm = mw_radar::utils::"), Some(6)); // after `mm`
        assert_eq!(ctx("let x = config.parser."), Some(5));
        assert_eq!(ctx("let x = self."), Some(5));
    }

    #[test]
    fn annotated_destructured_or_compound_do_not_match() {
        assert_eq!(ctx("let x: u32 = "), None); // already annotated
        assert_eq!(ctx("let (a, b) = "), None); // destructuring
        assert_eq!(ctx("x = "), None); // plain assignment
        assert_eq!(ctx("let x == "), None); // comparison typo
        assert_eq!(ctx("violet x = "), None); // `let` must be a whole word
        assert_eq!(ctx("let x = 1 + "), None); // compound expression
        assert_eq!(ctx("let x = foo("), None); // inside a call's arguments
        assert_eq!(ctx("let x = &"), None); // reference — type would be &Ret
        assert_eq!(ctx("let x = std:"), None); // half-typed `::`
        assert_eq!(ctx("let x = 0.."), None); // range, not a member access
    }

    #[test]
    fn return_type_from_plain_signature() {
        assert_eq!(
            return_type("fn frequencies(c: &Stm32f1Clock) -> ClockFrequencies"),
            Some("ClockFrequencies".to_owned())
        );
    }

    #[test]
    fn return_type_skips_nested_fn_arrows() {
        // The user's exact case: const generics + a generic parser param.
        let detail = "fn get_param_value<const PAYLOAD_LEN: usize, const HAS_CMD_ID: bool, \
                      const RESERVED_LEN: usize>(tx: &mut UsartTxType, rx: &mut UsartRxType, \
                      param_id: ParameterID, parser: &mut Parser<PAYLOAD_LEN, HAS_CMD_ID, \
                      RESERVED_LEN>) -> Option<u32>";
        assert_eq!(return_type(detail), Some("Option<u32>".to_owned()));
        // A fn-typed parameter's arrow is nested — not the return arrow.
        assert_eq!(
            return_type("fn apply(f: fn(u32) -> bool) -> usize"),
            Some("usize".to_owned())
        );
    }

    #[test]
    fn return_type_keeps_impl_fn_whole() {
        assert_eq!(
            return_type("fn make() -> impl Fn(u32) -> bool"),
            Some("impl Fn(u32) -> bool".to_owned())
        );
    }

    #[test]
    fn unit_functions_have_no_return_type() {
        assert_eq!(return_type("fn init(tx: &mut UsartTxType)"), None);
        assert_eq!(return_type(""), None);
    }

    #[test]
    fn callable_kinds() {
        assert!(is_callable_kind(2)); // Method
        assert!(is_callable_kind(3)); // Function
        assert!(is_callable_kind(4)); // Constructor
        assert!(!is_callable_kind(5)); // Field
        assert!(!is_callable_kind(6)); // Variable
    }
}
