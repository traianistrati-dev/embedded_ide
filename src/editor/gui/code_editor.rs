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

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn slice(chars: &[char], a: usize, b: usize) -> String {
    chars[a..b].iter().collect()
}

/// If a raw string (`r"…"`, `r#"…"#`, `br#"…"#`) starts at `p`, return the index
/// just past its closing delimiter; otherwise `None`. Handles any number of `#`.
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

/// Build a syntax-highlighted `LayoutJob` for Rust source. Mirrors the colours of
/// `egui_code_editor`'s built-in highlighter (keyword / type / special sets come
/// from the same `Syntax`, colours from the same `ColorTheme`) but is lifetime-
/// aware: a `'a` / `'static` lifetime is its own blue span instead of a runaway
/// char-literal string. Char literals (`'x'`, `'\n'`) keep the string colour.
///
/// `dead_ranges` are char-index `[start, end)` spans (e.g. a never-referenced
/// fn/struct/enum/const, from `AppIde`'s usages analysis) whose tokens are
/// rendered in a de-emphasised (faded toward gray) colour instead of their
/// normal syntax colour.
///
/// The job text equals the source exactly (every char appended in order), which
/// egui requires for correct cursor / selection mapping.
pub(crate) fn rust_layout_job(
    text: &str,
    theme: &ColorTheme,
    fontsize: f32,
    syntax: &Syntax,
    dead_ranges: &[(usize, usize)],
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::monospace(fontsize);
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();

    // A token starting at char index `at` is "dead" when it falls inside any of
    // `dead_ranges` — checked once per token (using its start index) since a
    // token never straddles a dead-range boundary in practice (fn/struct/etc.
    // spans start/end at statement boundaries).
    let in_dead = |at: usize| dead_ranges.iter().any(|&(s, e)| at >= s && at < e);

    let push = |job: &mut egui::text::LayoutJob, s: &str, color: egui::Color32, dead: bool| {
        if !s.is_empty() {
            let color = if dead { fade(color) } else { color };
            job.append(s, 0.0, egui::TextFormat::simple(font.clone(), color));
        }
    };
    let col = |ty: TokenType| theme.type_color(ty);

    let mut p = 0;
    while p < n {
        let c = chars[p];

        // ── Whitespace ──
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
        // ── Line comment `// …` ──
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
        // ── Block comment `/* … */` ──
        if c == '/' && chars.get(p + 1) == Some(&'*') {
            let s = p;
            p += 2;
            while p < n && !(chars[p - 1] == '*' && chars[p] == '/') {
                p += 1;
            }
            if p < n {
                p += 1; // include the closing '/'
            }
            push(
                &mut job,
                &slice(&chars, s, p),
                col(TokenType::Comment(true)),
                in_dead(s),
            );
            continue;
        }
        // ── Raw string `r"…"` / `r#"…"#` / `br#"…"#` ──
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
        // ── String `"…"` / byte string `b"…"` ──
        if c == '"' || (c == 'b' && chars.get(p + 1) == Some(&'"')) {
            let s = p;
            if c == 'b' {
                p += 1;
            }
            p += 1; // opening quote
            while p < n {
                match chars[p] {
                    '\\' => p += 2, // skip escaped char
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
        // ── Char literal vs lifetime (both start with `'`) ──
        if c == '\'' {
            // Escaped char literal: '\n' '\'' '\\' '\u{1F}' …
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
            // Lifetime: `'` + identifier, and NOT a single-char literal `'x'`
            // (those have a closing `'` two chars along).
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
            // Plain char literal `'x'`.
            let s = p;
            p += 1;
            if p < n {
                p += 1; // the char
            }
            if p < n && chars[p] == '\'' {
                p += 1; // closing quote
            }
            push(
                &mut job,
                &slice(&chars, s, p),
                col(TokenType::Str('\'')),
                in_dead(s),
            );
            continue;
        }
        // ── Number ──
        if c.is_ascii_digit() {
            let s = p;
            p += 1;
            // integer body (also hex/oct/bin digits, `_`, type suffix letters)
            while p < n && (chars[p].is_alphanumeric() || chars[p] == '_') {
                p += 1;
            }
            // float fraction `.123` — but never consume a `..` range operator.
            if p < n && chars[p] == '.' && chars.get(p + 1).is_some_and(|c| c.is_ascii_digit()) {
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
        // ── Identifier / keyword / type / special / function ──
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
        // ── Punctuation / anything else (one char) ──
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
    dead_ranges: &[(usize, usize)],
) -> egui::text::LayoutJob {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    fontsize.to_bits().hash(&mut h);
    dead_ranges.hash(&mut h);
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
        let job = rust_layout_job(text, theme, fontsize, syntax, dead_ranges);
        m.insert(0, (key, job.clone()));
        m.truncate(LAYOUT_MEMO_SLOTS);
        job
    })
}

/// The numbered-lines gutter, faithfully ported from `CodeEditor::numlines_show`
/// (we never use the shift / only-natural options, so they're dropped).
fn numlines_show(
    ui: &mut egui::Ui,
    text: &str,
    theme: &ColorTheme,
    fontsize: f32,
    rows: usize,
    id: &str,
) {
    use egui::TextBuffer;

    let total = if text.ends_with('\n') || text.is_empty() {
        text.lines().count() + 1
    } else {
        text.lines().count()
    }
    .max(rows);
    let max_indent = total.to_string().len();
    let mut counter = (1..=total)
        .map(|i| {
            let label = i.to_string();
            format!(
                "{}{label}",
                " ".repeat(max_indent.saturating_sub(label.len()))
            )
        })
        .collect::<Vec<String>>()
        .join("\n");

    let width = max_indent as f32 * fontsize * 0.5;
    let mut layouter = |ui: &egui::Ui, buf: &dyn TextBuffer, _wrap: f32| {
        let job = egui::text::LayoutJob::single_section(
            buf.as_str().to_string(),
            egui::TextFormat::simple(
                egui::FontId::monospace(fontsize),
                theme.type_color(TokenType::Comment(true)),
            ),
        );
        ui.fonts_mut(|f| f.layout_job(job))
    };
    ui.add(
        egui::TextEdit::multiline(&mut counter)
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
    dead_ranges: &[(usize, usize)],
) -> TextEditOutput {
    let mut out: Option<TextEditOutput> = None;
    let code_editor = |ui: &mut egui::Ui| {
        egui::Frame::new().fill(theme.bg()).show(ui, |ui| {
            ui.horizontal_top(|h| {
                theme.modify_style(h, fontsize);
                numlines_show(h, text.as_str(), theme, fontsize, rows, id);
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
                                    dead_ranges,
                                );
                                ui.fonts_mut(|f| f.layout_job(job))
                            };
                        let output = egui::TextEdit::multiline(text)
                            .id_source(id)
                            .lock_focus(true)
                            .desired_rows(rows)
                            .desired_width(f32::INFINITY)
                            .layouter(&mut layouter)
                            .show(ui);
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
    dead_ranges: &[(usize, usize)],
) -> TextEditOutput {
    if !suppress_keyword_completer {
        completer.handle_input(ui.ctx());
    }
    let mut out = show_rust_editor(ui, text, theme, fontsize, rows, syntax, id, dead_ranges);
    completer.text_edit_id = Some(out.response.id);
    if !suppress_keyword_completer {
        completer.show(syntax, theme, fontsize, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The memo must return exactly what a direct call produces, and any input
    /// change (text, dead ranges, font size) must recompute — never serve the
    /// previous entry.
    #[test]
    fn cached_layout_job_matches_direct_and_tracks_inputs() {
        let theme = ColorTheme::GRUVBOX;
        let syn = Syntax::rust();
        let src = "fn main() { let x = 1; }";

        let direct = rust_layout_job(src, &theme, 13.0, &syn, &[]);
        let cached = cached_rust_layout_job(src, &theme, 13.0, &syn, &[]);
        let repeat = cached_rust_layout_job(src, &theme, 13.0, &syn, &[]); // memo hit
        assert_eq!(direct, cached);
        assert_eq!(cached, repeat);

        // Changed inputs must not return the stale memo.
        let faded = cached_rust_layout_job(src, &theme, 13.0, &syn, &[(0, 5)]);
        assert_ne!(faded, cached, "dead range must change the job");
        let zoomed = cached_rust_layout_job(src, &theme, 15.0, &syn, &[]);
        assert_ne!(zoomed, cached, "font size must change the job");
        let edited = cached_rust_layout_job("fn main() {}", &theme, 13.0, &syn, &[]);
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
        let ja = cached_rust_layout_job(a, &theme, 13.0, &syn, &[]);
        let jb = cached_rust_layout_job(b, &theme, 13.0, &syn, &[]);
        for _ in 0..4 {
            assert_eq!(cached_rust_layout_job(a, &theme, 13.0, &syn, &[]), ja);
            assert_eq!(cached_rust_layout_job(b, &theme, 13.0, &syn, &[]), jb);
        }
        // Both must be resident simultaneously — not one evicting the other.
        let resident = LAYOUT_MEMO.with(|m| m.borrow().len());
        assert!(resident >= 2, "only {resident} entr(y/ies) memoized");
    }

    /// Collect (segment_text, is_lifetime_blue) from a highlighted job so tests
    /// can assert which spans were coloured as lifetimes without a real UI.
    fn spans(src: &str) -> Vec<(String, bool)> {
        let job = rust_layout_job(src, &ColorTheme::GRUVBOX, 13.0, &Syntax::rust(), &[]);
        job.sections
            .iter()
            .map(|s| {
                let txt = job.text[s.byte_range.clone()].to_string();
                (txt, s.format.color == LIFETIME_COLOR)
            })
            .collect()
    }

    /// The job text must reproduce the source exactly (egui relies on it).
    fn assert_roundtrip(src: &str) {
        let job = rust_layout_job(src, &ColorTheme::GRUVBOX, 13.0, &Syntax::rust(), &[]);
        assert_eq!(job.text, src, "layout job text must equal source");
    }

    /// A dead range dims a token's colour without changing the job's text.
    #[test]
    fn dead_range_fades_color_without_changing_text() {
        let src = "fn dead() {}\nfn used() {}";
        let job = rust_layout_job(src, &ColorTheme::GRUVBOX, 13.0, &Syntax::rust(), &[(0, 12)]);
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
            let txt = &job.text[s.byte_range.clone()];
            if txt == "fn" {
                if s.byte_range.start < 12 {
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
}
