//! The chooser shown when a go-to request resolves to MORE THAN ONE target.
//!
//! `textDocument/definition` answers with a single location for a Rust symbol,
//! so the F12 pipeline was built around one `DefinitionLoc` and the array LSP
//! sends was collapsed with `.first()`. Ctrl+F12 arrived later and reused that
//! pipeline — but `textDocument/implementation` is multi-valued by definition:
//! a trait implemented by two types answers with two locations. The editor
//! therefore navigated to whichever impl rust-analyzer happened to list first,
//! from every caret position, for ever.
//!
//! **Picking a better element from the array would not have fixed it.**
//! rust-analyzer resolves the caret to the TRAIT ITEM and answers for all of
//! its impls, so `Foo::parse()` and `Bar::parse()` produce the byte-identical
//! array — writing a different type on the line cannot change the answer. The
//! caret's type is a fact only the editor holds. So: keep the whole array, show
//! it, and float the entry the caret actually names to the top.

use crate::app::EditorSlot;
use eframe::egui;

/// Weights for [`match_score`]. An `impl … for T` header naming what the caret
/// names is the strongest statement available — it is the impl's identity.
const CONTEXT_HIT: usize = 2;
/// A file whose NAME the caret wrote (`report_debug_mode::…` → `report_debug_mode.rs`)
/// is a real signal, just a weaker one: modules are named loosely.
const STEM_HIT: usize = 1;
/// The target's own line. Usually identical across the impls of one trait
/// method (`fn parse(&mut self)`), so it mostly adds a constant — it earns its
/// keep only when several impls share a file.
const SIGNATURE_HIT: usize = 1;

/// A generic `impl` may spread `where` clauses over several lines before the
/// `{`, so the header is rebuilt by walking back this far to the keyword.
const MAX_HEADER_LINES: usize = 6;

/// Identifiers that say nothing about identity. `for` in `impl T for U` would
/// otherwise score against `for` in a loop on the caret's line.
const NOISE: &[&str] = &[
    "let", "mut", "impl", "for", "fn", "pub", "self", "Self", "crate", "super", "as", "ref", "dyn",
    "move", "const", "static", "async", "await", "unsafe", "where", "use", "return", "if", "else",
    "match", "loop", "while", "in", "type", "struct", "enum", "trait", "mod", "new",
];

/// One navigable target, carrying enough text to tell it from its siblings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImplTarget {
    /// Absolute path, as rust-analyzer reported it.
    pub path: String,
    /// 0-based, LSP's own numbering — the navigation layer adds the 1.
    pub line: u32,
    pub character: u32,
    /// The target's own source line, trimmed.
    pub signature: String,
    /// The `impl …` header it sits under, when it sits under one. This is what
    /// makes two rows of `fn parse(&mut self)` distinguishable at a glance.
    pub context: Option<String>,
}

impl ImplTarget {
    /// What the row shows first: the impl it belongs to, or failing that its
    /// own line. Never empty — a blank row is unclickable in practice.
    pub(crate) fn headline(&self) -> &str {
        match self.context.as_deref() {
            Some(c) if !c.is_empty() => c,
            _ if !self.signature.is_empty() => &self.signature,
            _ => "(unnamed target)",
        }
    }
}

/// The open chooser. Lives on `AppIde` rather than `EditorState` because the
/// answer it displays arrives at frame top, before either view has drawn, and
/// is routed by `lsp_asker.definition` — the same slot recorded here.
pub(crate) struct ImplPicker {
    pub targets: Vec<ImplTarget>,
    pub sel: usize,
    pub pos: egui::Pos2,
    pub slot: EditorSlot,
    /// "2 implementations" / "2 definitions" — which question was asked.
    pub title: String,
}

/// Every identifier in `s`, noise and bare numbers removed.
pub(super) fn idents(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()) && !NOISE.contains(w))
        .map(str::to_owned)
        .collect()
}

/// `…/report_debug_mode.rs` → `report_debug_mode`.
pub(super) fn file_stem(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next()?;
    Some(name.strip_suffix(".rs").unwrap_or(name))
}

/// Does `line` open an `impl` item? `implementation_of` must not match.
fn starts_impl(line: &str) -> bool {
    let t = line.trim_start();
    t.strip_prefix("impl")
        .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
}

/// How well `t` matches the code the caret was sitting on.
///
/// The signal is deliberately narrow: identifiers the user WROTE on that line,
/// found again in the target's file name, its `impl` header, or its own
/// signature. Writing `report_debug_mode::HmmdRdmapFrame::new_parser()` names
/// both a module and a type, and only one candidate impl answers to either.
pub(super) fn match_score(t: &ImplTarget, caret: &[String]) -> usize {
    let mut score = 0;
    if file_stem(&t.path).is_some_and(|stem| caret.iter().any(|i| i == stem)) {
        score += STEM_HIT;
    }
    if let Some(ctx) = &t.context {
        for id in idents(ctx) {
            if caret.contains(&id) {
                score += CONTEXT_HIT;
            }
        }
    }
    for id in idents(&t.signature) {
        if caret.contains(&id) {
            score += SIGNATURE_HIT;
        }
    }
    score
}

/// Float the targets the caret's line names to the top, keeping rust-analyzer's
/// own order among equals.
///
/// A PERMUTATION, never a filter: a low score means "probably not the one you
/// meant", which is not the same as "not a real implementation". Hiding one
/// would recreate the bug this module exists to fix, just with better aim.
pub(crate) fn rank(targets: &mut [ImplTarget], caret_line: &str) {
    let caret = idents(caret_line);
    // `sort_by_key` is stable, so equal scores keep the order RA sent.
    targets.sort_by_key(|t| std::cmp::Reverse(match_score(t, &caret)));
}

/// Drop exact repeats, keeping the first. rust-analyzer can name the same
/// location twice when a trait item is reachable by two paths.
pub(crate) fn dedupe(targets: &mut Vec<ImplTarget>) {
    let mut seen = std::collections::HashSet::new();
    targets.retain(|t| seen.insert((t.path.clone(), t.line, t.character)));
}

/// The `impl …` header the line at `target` sits inside, joined to one line.
///
/// Built on [`fold::regions`](super::fold::regions) rather than counting braces
/// here: that scanner already skips strings, char literals and comments, and
/// this codebase's standing rule is to reuse one of the correct scanners rather
/// than add an eighth.
pub(super) fn enclosing_impl(content: &str, target: usize) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut heads: Vec<usize> = super::fold::regions(content)
        .iter()
        .filter(|r| r.head <= target && target <= r.end)
        .map(|r| r.head)
        .collect();
    heads.sort_unstable();
    // Innermost first: `impl Trait for T { fn f() { … } }` must report the impl,
    // and an inner block must not shadow it.
    heads
        .into_iter()
        .rev()
        .find_map(|head| impl_header_at(&lines, head))
}

/// The `impl` header whose `{` lands on `head`, rebuilt from the keyword — or
/// `None` when what opens at `head` is not an impl.
///
/// The walk back must not leave the item. A first version simply looked upward
/// for the nearest line starting with `impl`, and from `fn go(&self) {` that is
/// the ENCLOSING impl one line above — so every method reported
/// `impl Parse for B { fn go(&self)`, welding two items into one caption. The
/// boundary is a line that ends a statement or opens a block (`{`, `}`, `;`) or
/// a blank line; only doc comments, attributes and `where` clauses lie between
/// an `impl` keyword and its brace.
fn impl_header_at(lines: &[&str], head: usize) -> Option<String> {
    let floor = head.saturating_sub(MAX_HEADER_LINES - 1);
    let mut start = head;
    while start > floor {
        let above = lines.get(start - 1)?.trim();
        if above.is_empty() || above.ends_with(['{', '}', ';']) {
            break;
        }
        start -= 1;
    }
    // Within the continuation span, the header begins at the keyword — a doc
    // comment or `#[…]` above it is part of the span but not of the caption.
    let kw = (start..=head).find(|&i| lines.get(i).is_some_and(|l| starts_impl(l)))?;
    let joined = lines[kw..=head]
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ");
    Some(joined.trim_end_matches('{').trim().to_owned())
}

/// Read each location's file once and decorate it with the line it points at
/// and the `impl` header above it.
///
/// An unreadable file still yields a target: the path and line are rust-analyzer's
/// answer and stay navigable — only the description is missing.
pub(crate) fn build_targets(locs: Vec<crate::lsp::DefinitionLoc>) -> Vec<ImplTarget> {
    let mut cache: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    locs.into_iter()
        .map(|l| {
            let content = cache
                .entry(l.path.clone())
                .or_insert_with(|| std::fs::read_to_string(&l.path).ok());
            let (signature, context) = match content {
                Some(text) => {
                    let idx = l.line as usize;
                    let sig = text
                        .lines()
                        .nth(idx)
                        .map(|s| s.trim().to_owned())
                        .unwrap_or_default();
                    (sig, enclosing_impl(text, idx))
                }
                None => (String::new(), None),
            };
            ImplTarget {
                path: l.path,
                line: l.line,
                character: l.character,
                signature,
                context,
            }
        })
        .collect()
}

impl crate::app::AppIde {
    /// Draw the go-to chooser, when one is open and belongs to this view.
    ///
    /// Keyboard nav (Up/Down/Enter/Esc) is consumed BEFORE the editor renders
    /// (see `editor_panel/mod.rs`), exactly as the code-action popup does —
    /// otherwise the editor would take Enter first and split the line.
    pub(super) fn show_impl_picker(&mut self, ui: &mut egui::Ui) {
        let Some(picker) = self.impl_picker.as_ref() else {
            return;
        };
        if picker.slot != self.ed_slot {
            return;
        }
        let sel = picker.sel;
        let pos = picker.pos;
        let title = picker.title.clone();
        let rows: Vec<(String, String)> = picker
            .targets
            .iter()
            .map(|t| {
                (
                    t.headline().to_owned(),
                    format!(
                        "{}:{}",
                        file_stem(&t.path).unwrap_or("?"),
                        t.line as usize + 1
                    ),
                )
            })
            .collect();

        let mut chosen: Option<usize> = None;
        egui::Area::new(egui::Id::new("impl_picker_popup"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.set_min_width(280.0);
                    ui.set_max_width(560.0);
                    ui.label(
                        egui::RichText::new(&title)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 180, 200)),
                    );
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for (i, (head, where_)) in rows.iter().enumerate() {
                                let selected = i == sel;
                                let row = ui.selectable_label(
                                    selected,
                                    egui::RichText::new(format!("{head}   {where_}")).size(12.0),
                                );
                                if selected {
                                    row.scroll_to_me(None);
                                }
                                if row.clicked() {
                                    chosen = Some(i);
                                }
                            }
                        });
                });
            });

        if let Some(i) = chosen {
            self.take_impl_picker_choice(i);
        }
    }

    /// Navigate to row `i` and close the chooser.
    pub(super) fn take_impl_picker_choice(&mut self, i: usize) {
        let Some(picker) = self.impl_picker.take() else {
            return;
        };
        let slot = picker.slot;
        if let Some(t) = picker.targets.into_iter().nth(i) {
            self.goto_definition_target(&t, slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(path: &str, line: u32, sig: &str, ctx: Option<&str>) -> ImplTarget {
        ImplTarget {
            path: path.to_owned(),
            line,
            character: 0,
            signature: sig.to_owned(),
            context: ctx.map(str::to_owned),
        }
    }

    /// The reported bug, end to end through the pure half: two impls of one
    /// trait method, and the caret writes the module AND the type of one of
    /// them. That one must come first.
    #[test]
    fn the_impl_the_caret_names_is_offered_first() {
        let caret = "let mut parser = hmmd_mmwave_sensor_async::report_debug_mode::HmmdRdmapFrame::new_parser();";
        let mut targets = vec![
            target(
                "/p/report_normal_mode.rs",
                40,
                "fn new_parser() -> Self {",
                Some("impl RadarFrame for HmmdFrame"),
            ),
            target(
                "/p/report_debug_mode.rs",
                55,
                "fn new_parser() -> Self {",
                Some("impl RadarFrame for HmmdRdmapFrame"),
            ),
        ];
        rank(&mut targets, caret);
        assert_eq!(
            file_stem(&targets[0].path),
            Some("report_debug_mode"),
            "the impl for the type written at the caret must lead"
        );
    }

    /// The half that is NOT about aim: nothing may be hidden. A ranking that
    /// dropped the low scorer would be the original bug wearing a better hat.
    #[test]
    fn ranking_is_a_permutation_and_never_a_filter() {
        let caret = "let x = Alpha::go();";
        let mut targets = vec![
            target("/p/a.rs", 1, "fn go()", Some("impl T for Alpha")),
            target("/p/b.rs", 2, "fn go()", Some("impl T for Beta")),
            target("/p/c.rs", 3, "fn go()", Some("impl T for Gamma")),
        ];
        let before: std::collections::HashSet<_> = targets.iter().map(|t| t.path.clone()).collect();
        rank(&mut targets, caret);
        let after: std::collections::HashSet<_> = targets.iter().map(|t| t.path.clone()).collect();
        assert_eq!(targets.len(), 3, "no target may be dropped");
        assert_eq!(before, after, "no target may be invented or lost");
    }

    /// With nothing to go on, rust-analyzer's own order is what we have — and
    /// inventing an order (alphabetical, by path) would claim a judgement we
    /// have not made.
    #[test]
    fn with_no_match_the_analyzer_order_is_kept() {
        let mut targets = vec![
            target("/p/zeta.rs", 1, "fn go()", Some("impl T for Zeta")),
            target("/p/alpha.rs", 2, "fn go()", Some("impl T for Alpha")),
        ];
        rank(&mut targets, "let x = something_else();");
        assert_eq!(file_stem(&targets[0].path), Some("zeta"));
        assert_eq!(file_stem(&targets[1].path), Some("alpha"));
    }

    /// The type name outranks the module name, because two modules can be named
    /// loosely while `impl … for T` states the impl's identity.
    #[test]
    fn the_type_outweighs_the_module_when_they_disagree() {
        let mut targets = vec![
            // Module matches, type does not.
            target(
                "/p/report_debug_mode.rs",
                1,
                "fn go()",
                Some("impl T for Other"),
            ),
            // Type matches, module does not.
            target("/p/elsewhere.rs", 2, "fn go()", Some("impl T for Wanted")),
        ];
        rank(&mut targets, "report_debug_mode::Wanted::go();");
        assert_eq!(
            file_stem(&targets[0].path),
            Some("elsewhere"),
            "the impl FOR the named type wins over a same-named file"
        );
    }

    /// Keywords carry no identity. Without the noise list every impl scores on
    /// `impl` and `for`, and the ranking flattens into RA's order again.
    #[test]
    fn keywords_do_not_score() {
        let caret = "for x in y { impl_like_name(); }";
        let t = target("/p/a.rs", 1, "fn go()", Some("impl T for U"));
        assert_eq!(
            match_score(&t, &idents(caret)),
            0,
            "`impl` and `for` must not be identity"
        );
    }

    /// `implementation_of` is not an `impl` item.
    #[test]
    fn an_identifier_starting_with_impl_is_not_an_impl_block() {
        assert!(starts_impl("impl Foo {"));
        assert!(starts_impl("    impl<T> Foo<T> {"));
        assert!(!starts_impl("implementation_of(x);"));
        assert!(!starts_impl("let implicit = 3;"));
    }

    #[test]
    fn the_enclosing_impl_of_a_method_is_its_impl_block() {
        let src = "\
struct A;
struct B;

impl Parse for A {
    fn go(&self) {
        let _ = 1;
    }
}

impl Parse for B {
    fn go(&self) {
        let _ = 2;
    }
}
";
        // `fn go` of the SECOND impl.
        let line = src.lines().position(|l| l.contains("let _ = 2")).unwrap();
        assert_eq!(
            enclosing_impl(src, line).as_deref(),
            Some("impl Parse for B"),
            "the innermost containing impl, not the first one in the file"
        );
    }

    /// A brace inside a string must not pair, or every header after it shifts.
    /// This is exactly why the fold scanner is reused instead of a local one.
    #[test]
    fn a_brace_in_a_string_does_not_move_the_header() {
        let src = "\
impl Parse for A {
    fn go(&self) {
        let s = \"}{\";
        let _ = s;
    }
}
";
        let line = src.lines().position(|l| l.contains("let _ = s")).unwrap();
        assert_eq!(
            enclosing_impl(src, line).as_deref(),
            Some("impl Parse for A")
        );
    }

    /// A `where` clause pushes the `{` off the `impl` line; the header is still
    /// the whole statement, not the bare brace.
    #[test]
    fn a_multi_line_impl_header_is_rebuilt() {
        let src = "\
impl<T> Parse for Wrapper<T>
where
    T: Clone,
{
    fn go(&self) {
        let _ = 1;
    }
}
";
        let line = src.lines().position(|l| l.contains("let _ = 1")).unwrap();
        assert_eq!(
            enclosing_impl(src, line).as_deref(),
            Some("impl<T> Parse for Wrapper<T> where T: Clone,"),
            "a bare `{{` is not a usable description of the impl"
        );
    }

    #[test]
    fn identical_locations_collapse_to_one_row() {
        let mut targets = vec![
            target("/p/a.rs", 7, "fn go()", None),
            target("/p/a.rs", 7, "fn go()", None),
            target("/p/a.rs", 9, "fn go()", None),
        ];
        dedupe(&mut targets);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].line, 7);
        assert_eq!(targets[1].line, 9);
    }

    /// A row with nothing to say must still be clickable.
    #[test]
    fn an_undescribable_target_still_has_a_headline() {
        let t = target("/p/a.rs", 3, "", None);
        assert!(!t.headline().is_empty());
    }

    #[test]
    fn a_windows_path_yields_its_stem() {
        assert_eq!(
            file_stem(r"C:\work\src\report_debug_mode.rs"),
            Some("report_debug_mode")
        );
    }
}
