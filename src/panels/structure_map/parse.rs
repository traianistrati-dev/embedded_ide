//! Build the module graph by parsing source text (pure logic, tested).
//!
//! Nodes come from the file list (`foo/bar.rs` → module `foo::bar`,
//! `foo/mod.rs` → `foo`, main.rs → the crate root). Edges come from `mod x;`
//! declarations (containment) and from `use` / inline path chains resolved
//! against the known module paths (dependency). Resolution is deliberately
//! approximate — a plain-text scan, not name resolution — but it is exact for
//! the common shapes this IDE generates (`use crate::a::b`, `super::x`,
//! `pins::configs::usart1::init(...)`).

use std::collections::{HashMap, HashSet};

/// Kind of a top-level item shown inside a module node (Phase 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymKind {
    Fn,
    Struct,
    Enum,
    Trait,
}

/// One top-level item of a module — listed inside its node, UML-package style.
/// `line` is 1-based in the module's file (used for click-to-jump), exact for
/// the text the graph was parsed from (unlike LSP positions, which refer to
/// rust-analyzer's last-synced text and would mis-jump after unsaved edits).
#[derive(Clone, Debug)]
pub struct SymbolItem {
    pub name: String,
    pub kind: SymKind,
    pub line: usize,
    /// 1-based line of the item's closing brace (== `line` for `struct Foo;`).
    /// Together with `line` this is the item's SPAN, used to attribute a
    /// reference site to its enclosing top-level item.
    pub end_line: usize,
    /// 0-based column (chars) of the name on its line — the position a
    /// `textDocument/references` request must point at. Keyword/modifier
    /// prefixes are ASCII, so char count == UTF-16 units here.
    pub col: usize,
}

/// One module in the graph.
#[derive(Clone, Debug)]
pub struct ModuleNode {
    /// Full Rust path (`"mw_radar::utils"`); empty string for the crate root.
    pub path: String,
    /// Display name — the last path segment (`"utils"`), `"main"` for the root.
    pub name: String,
    /// Workspace-relative file (`"mw_radar/utils.rs"`); `"main.rs"` for the root.
    pub file_rel: String,
    /// Index into `user_src_files`; `None` = main.rs.
    pub file: Option<usize>,
    /// Number of `fn` items (incl. methods) — a size badge, not a precise count.
    pub fn_count: usize,
    /// Number of `struct` / `enum` / `trait` items.
    pub ty_count: usize,
    /// Top-level items (brace-depth-0 `fn` / `struct` / `enum` / `trait`), in
    /// file order. Methods inside `impl` blocks are counted in `fn_count` but
    /// not listed here (they'd bloat the node).
    pub symbols: Vec<SymbolItem>,
    /// Depth-0 `impl` block spans, each resolved to the ROW of the implemented
    /// type (`impl Parser { … }` / `impl Frame for HmmdFrame { … }` → the
    /// `Parser` / `HmmdFrame` row). Call sites inside methods attribute to
    /// that row — without this, modules that keep their logic in impl methods
    /// (most library code) produced NO outgoing call edges at all.
    pub impl_spans: Vec<(usize, usize, usize)>,
}

impl ModuleNode {
    /// The symbol row whose line SPAN contains 1-based `line` — brace-depth
    /// based, so it is immune to indentation style. (The first version used
    /// "column-0 line = item boundary", which real code broke: statements
    /// written at column 0 inside `fn main` — e.g. `radar.read_data(|rx| {` —
    /// became anonymous boundaries and every call site after them was
    /// attributed to nothing, so its call edge silently vanished.)
    /// Falls back to the enclosing `impl` block's type row (see `impl_spans`).
    pub fn enclosing_row(&self, line: usize) -> Option<usize> {
        self.symbols
            .iter()
            .position(|s| line >= s.line && line <= s.end_line)
            .or_else(|| {
                self.impl_spans
                    .iter()
                    .find(|&&(s, e, _)| line >= s && line <= e)
                    .map(|&(_, _, row)| row)
            })
    }
}

/// The whole module graph. Edge tuples are `(from, to)` node indices.
#[derive(Clone, Debug, Default)]
pub struct ModuleGraph {
    pub nodes: Vec<ModuleNode>,
    /// `user → used` module references (solid arrows).
    pub deps: Vec<(usize, usize)>,
    /// `parent → child` from `mod child;` declarations (dashed lines).
    pub contains: Vec<(usize, usize)>,
}

impl ModuleGraph {
    /// The FOCUS SET for the call-edge display, as a membership mask. A plain
    /// module focuses itself alone; a PACKAGE ROOT (a `mod.rs` file, e.g.
    /// selecting `mw_radar/mod.rs`) focuses the whole package subtree — the
    /// root plus every `pkg::…` descendant — so the diagram shows all interior
    /// links of the package at once, plus its outside connections.
    pub fn focus_set(&self, focus: usize) -> Vec<bool> {
        let mut set = vec![false; self.nodes.len()];
        let Some(node) = self.nodes.get(focus) else {
            return set;
        };
        set[focus] = true;
        if node.file_rel.ends_with("/mod.rs") && !node.path.is_empty() {
            let prefix = format!("{}::", node.path);
            for (i, n) in self.nodes.iter().enumerate() {
                if n.path.starts_with(&prefix) {
                    set[i] = true;
                }
            }
        }
        set
    }
}

/// `"foo/bar.rs"` → `"foo::bar"`, `"foo/mod.rs"` → `"foo"`, `"utils.rs"` → `"utils"`.
pub fn module_path_of(rel: &str) -> String {
    let no_ext = rel.strip_suffix(".rs").unwrap_or(rel);
    let no_mod = no_ext.strip_suffix("/mod").unwrap_or(no_ext);
    no_mod.replace('/', "::")
}

/// Build the graph from main.rs plus the user source files
/// (`(rel_path, content)`, as in `project_tree.user_src_files`).
pub fn build_graph(main_rs: &str, user_files: &[(String, String)]) -> ModuleGraph {
    // ── Nodes ─────────────────────────────────────────────────────────────
    let mut nodes = vec![make_node(String::new(), "main", "main.rs", None, main_rs)];
    for (i, (rel, content)) in user_files.iter().enumerate() {
        if !rel.ends_with(".rs") {
            continue; // defensive: only Rust files become modules
        }
        let path = module_path_of(rel);
        if path.is_empty() {
            continue; // a bare "mod.rs" at src/ root — not a module
        }
        let name = path.rsplit("::").next().unwrap_or(&path).to_owned();
        nodes.push(make_node(path, &name, rel, Some(i), content));
    }

    // Path → node index (for edge resolution).
    let by_path: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.path.clone(), i))
        .collect();

    // ── Edges ─────────────────────────────────────────────────────────────
    let mut deps: HashSet<(usize, usize)> = HashSet::new();
    let mut contains: HashSet<(usize, usize)> = HashSet::new();
    let texts: Vec<(usize, &str)> = std::iter::once((0usize, main_rs))
        .chain(nodes.iter().skip(1).map(|n| {
            let idx = by_path[&n.path];
            (idx, user_files[n.file.unwrap()].1.as_str())
        }))
        .collect();

    for (idx, text) in texts {
        let cur_path = nodes[idx].path.clone();
        for raw_line in text.lines() {
            // Naive comment strip — may also cut a "://" inside a string, which
            // only loses potential edges on that line (acceptable noise).
            let line = raw_line.split("//").next().unwrap_or("");

            // `mod child;` → containment edge.
            if let Some(name) = mod_decl(line) {
                let child_path = if cur_path.is_empty() {
                    name.clone()
                } else {
                    format!("{cur_path}::{name}")
                };
                if let Some(&child) = by_path.get(&child_path) {
                    if child != idx {
                        contains.insert((idx, child));
                    }
                }
            }

            // Path chains (`use crate::a::b`, `super::x::y`, `a::b::c(...)`).
            for chain in scan_chains(line) {
                if let Some(target) = resolve(&chain, &cur_path, &by_path) {
                    if target != idx {
                        deps.insert((idx, target));
                    }
                }
            }
        }
    }

    // A dep edge that duplicates a containment edge adds only clutter — the
    // dashed containment line already links the pair.
    let deps: Vec<(usize, usize)> = {
        let mut v: Vec<_> = deps
            .into_iter()
            .filter(|e| !contains.contains(e))
            .collect();
        v.sort_unstable();
        v
    };
    let contains: Vec<(usize, usize)> = {
        let mut v: Vec<_> = contains.into_iter().collect();
        v.sort_unstable();
        v
    };

    ModuleGraph { nodes, deps, contains }
}

fn make_node(
    path: String,
    name: &str,
    file_rel: &str,
    file: Option<usize>,
    content: &str,
) -> ModuleNode {
    let (fn_count, ty_count, symbols, impl_spans) = scan_items(content);
    ModuleNode {
        path,
        name: name.to_owned(),
        file_rel: file_rel.to_owned(),
        file,
        fn_count,
        ty_count,
        symbols,
        impl_spans,
    }
}

/// Count `fn` and `struct`/`enum`/`trait` item lines (badge-grade accuracy)
/// and collect the TOP-LEVEL ones with their line spans.
///
/// "Top-level" = the declaration appears at **brace depth 0**, and the item's
/// span runs until the depth returns to 0 (its closing brace) — never by
/// indentation, which real code violates (column-0 statements inside `fn
/// main`). Items inside `impl`/`mod` blocks sit at depth ≥ 1, so the badge
/// counts them but the symbol list skips them. Braces inside string literals
/// can skew the depth — badge-grade accuracy, and they usually balance out.
fn scan_items(
    text: &str,
) -> (usize, usize, Vec<SymbolItem>, Vec<(usize, usize, usize)>) {
    let mut fns = 0;
    let mut tys = 0;
    let mut symbols: Vec<SymbolItem> = Vec::new();
    // Depth-0 impl blocks: (start, end, implemented-type NAME) — resolved to
    // symbol rows after the scan (the type may be declared below its impl).
    let mut pending_impls: Vec<(usize, usize, Option<String>)> = Vec::new();
    let mut depth: i32 = 0;
    // The depth-0 block the scan is currently inside — a symbol item or an
    // `impl` — and whether its opening `{` has been seen yet (multi-line
    // signatures put it lines later; `struct Foo;` never opens one and closes
    // at the `;`).
    let mut open_sym: Option<usize> = None;
    let mut open_impl: Option<(usize, Option<String>, usize)> = None;
    let mut entered_body = false;
    for (li, raw) in text.lines().enumerate() {
        // Naive comment strip — same trade-off as the edge scan above.
        let line = raw.split("//").next().unwrap_or("");
        let t = strip_modifiers(line.trim_start());
        let (kind, rest) = if let Some(r) = t.strip_prefix("fn ") {
            fns += 1;
            (Some(SymKind::Fn), r)
        } else if let Some(r) = t.strip_prefix("struct ") {
            tys += 1;
            (Some(SymKind::Struct), r)
        } else if let Some(r) = t.strip_prefix("enum ") {
            tys += 1;
            (Some(SymKind::Enum), r)
        } else if let Some(r) = t.strip_prefix("trait ") {
            tys += 1;
            (Some(SymKind::Trait), r)
        } else {
            (None, t)
        };
        if depth == 0 && open_sym.is_none() && open_impl.is_none() {
            if let Some(kind) = kind {
                let name: String = rest
                    .chars()
                    .take_while(|&c| c.is_alphanumeric() || c == '_')
                    .collect();
                if !name.is_empty() {
                    // Column of the name = chars before `rest` on the raw line
                    // (prefixes are ASCII, so chars == UTF-16 units).
                    let col = line.chars().count() - rest.chars().count();
                    open_sym = Some(symbols.len());
                    symbols.push(SymbolItem {
                        name,
                        kind,
                        line: li + 1,
                        end_line: li + 1,
                        col,
                    });
                }
            } else if let Some(r) = t.strip_prefix("impl") {
                // Word boundary: `impl` / `impl<…>` / `impl Foo`, not `implxyz`.
                if r.starts_with(|c: char| !c.is_alphanumeric() && c != '_') || r.is_empty() {
                    open_impl = Some((li + 1, impl_target_name(r), li + 1));
                }
            }
        }
        for c in line.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth = (depth - 1).max(0),
                _ => {}
            }
        }
        if open_sym.is_some() || open_impl.is_some() {
            if let Some(row) = open_sym {
                symbols[row].end_line = li + 1;
            }
            if let Some((_, _, end)) = &mut open_impl {
                *end = li + 1;
            }
            if !entered_body && line.contains('{') {
                entered_body = true;
            }
            if entered_body {
                if depth == 0 {
                    // The block's closing brace was reached on this line.
                    if let Some((s, name, e)) = open_impl.take() {
                        pending_impls.push((s, e, name));
                    }
                    open_sym = None;
                    entered_body = false;
                }
            } else if depth == 0 && line.contains(';') {
                open_sym = None; // bodyless item: `struct Foo;` / `struct P(u8);`
                open_impl = None;
            }
        }
    }
    // An unclosed impl at EOF (mid-typing) still gets its span so far.
    if let Some((s, name, e)) = open_impl.take() {
        pending_impls.push((s, e, name));
    }
    // Resolve impl targets to symbol rows (unresolved → dropped: the type
    // lives elsewhere, e.g. `impl Display for ExternalType`).
    let impl_spans: Vec<(usize, usize, usize)> = pending_impls
        .into_iter()
        .filter_map(|(s, e, name)| {
            let name = name?;
            let row = symbols.iter().position(|sym| sym.name == name)?;
            Some((s, e, row))
        })
        .collect();
    (fns, tys, symbols, impl_spans)
}

/// The NAME of the type an `impl` line implements — `rest` is everything after
/// the `impl` keyword. Handles generics and trait impls by scanning at
/// angle-bracket depth 0: `<const N: usize> Parser<N> {` → `Parser`,
/// ` Frame for HmmdFrame {` → `HmmdFrame`, ` fmt::Display for Config` →
/// `Config` (last path segment; `impl Trait for Type` targets the TYPE).
fn impl_target_name(rest: &str) -> Option<String> {
    // Flatten to the angle-depth-0 text, stopping at the body brace.
    let mut flat = String::new();
    let mut depth = 0i32;
    for c in rest.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = (depth - 1).max(0),
            '{' if depth == 0 => break,
            _ if depth == 0 => flat.push(c),
            _ => {}
        }
    }
    let flat = flat.split(" where").next().unwrap_or(&flat).trim();
    // `impl Trait for Type` → the part after the LAST ` for `; else the whole.
    let target = flat.rsplit(" for ").next().unwrap_or(flat).trim();
    // Drop reference/mut sugar (`&mut Foo`), then the first token's last
    // path segment, identifier chars only.
    let target = target.trim_start_matches('&').trim_start();
    let target = target.strip_prefix("mut ").unwrap_or(target).trim_start();
    let token = target.split_whitespace().next()?;
    let seg = token.rsplit("::").next().unwrap_or(token);
    let name: String = seg
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Strip leading visibility / item modifiers so `pub async fn` matches `fn `.
fn strip_modifiers(mut s: &str) -> &str {
    loop {
        let before = s;
        for p in [
            "pub(crate) ",
            "pub(super) ",
            "pub(in crate) ",
            "pub ",
            "async ",
            "unsafe ",
            "const ",
            "extern \"C\" ",
        ] {
            if let Some(rest) = s.strip_prefix(p) {
                s = rest;
            }
        }
        if s == before {
            return s;
        }
    }
}

/// Detect a file-module declaration: `[pub …] mod name;` → `Some(name)`.
/// Inline `mod name { … }` blocks are skipped (they aren't separate files).
fn mod_decl(line: &str) -> Option<String> {
    let t = strip_modifiers(line.trim_start());
    let rest = t.strip_prefix("mod ")?;
    let name = rest.trim().strip_suffix(';')?.trim();
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .then(|| name.to_owned())
}

/// Extract every `ident(::ident)+` chain from a (comment-stripped) line.
/// Chains stop at any non-path character (`(`, `<`, `{`, whitespace, …), so
/// `pins::configs::usart1::init(&mut afio)` yields
/// `[pins, configs, usart1, init]`.
fn scan_chains(line: &str) -> Vec<Vec<String>> {
    let b: Vec<char> = line.chars().collect();
    let is_start = |c: char| c.is_alphabetic() || c == '_';
    let is_cont = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if is_start(b[i]) && (i == 0 || (!is_cont(b[i - 1]) && b[i - 1] != ':')) {
            let mut segs: Vec<String> = Vec::new();
            loop {
                let s = i;
                while i < b.len() && is_cont(b[i]) {
                    i += 1;
                }
                segs.push(b[s..i].iter().collect());
                if i + 2 < b.len() && b[i] == ':' && b[i + 1] == ':' && is_start(b[i + 2]) {
                    i += 2;
                    continue;
                }
                break;
            }
            if segs.len() >= 2 {
                out.push(segs);
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Resolve a path chain to a module node — the LONGEST known module path the
/// chain's prefix reaches. `cur_path` is the module the chain appears in.
///
/// Rules (kept intentionally simple):
///   * `crate::rest…`  → absolute; must consume ≥ 1 chain segment.
///   * `super::…::rest` → pop one path segment per `super`; if `rest` reaches a
///     deeper known module use it, otherwise the parent module itself is the
///     target (a `use super::ITEM` IS a dependency on the parent).
///   * `self::rest`    → relative to `cur_path`; must go deeper than it.
///   * bare `a::b…`    → try relative to `cur_path` first (must go deeper),
///     then as a crate-absolute path (covers `pins::configs::usart1::init(…)`
///     written in main.rs).
fn resolve(
    chain: &[String],
    cur_path: &str,
    by_path: &HashMap<String, usize>,
) -> Option<usize> {
    let cur_segs: Vec<&str> = if cur_path.is_empty() {
        Vec::new()
    } else {
        cur_path.split("::").collect()
    };

    match chain[0].as_str() {
        "crate" => longest_match(&[], &chain[1..], 0, by_path),
        "super" => {
            let mut supers = 0;
            while supers < chain.len() && chain[supers] == "super" {
                supers += 1;
            }
            if supers > cur_segs.len() {
                return None; // walked above the crate root
            }
            let base = &cur_segs[..cur_segs.len() - supers];
            let rest = &chain[supers..];
            // Deeper module under the parent, else the parent module itself.
            longest_match(base, rest, base.len(), by_path)
                .or_else(|| by_path.get(&base.join("::")).copied())
        }
        "self" => longest_match(&cur_segs, &chain[1..], cur_segs.len(), by_path),
        _ => longest_match(&cur_segs, chain, cur_segs.len(), by_path)
            .or_else(|| longest_match(&[], chain, 0, by_path)),
    }
}

/// Find the longest known module path among `base ++ chain[..k]` prefixes,
/// requiring the match to consume more than `min_len` total segments (so a
/// relative lookup can't "resolve" to the module it already sits in).
fn longest_match(
    base: &[&str],
    chain: &[String],
    min_len: usize,
    by_path: &HashMap<String, usize>,
) -> Option<usize> {
    let mut segs: Vec<&str> = base.to_vec();
    segs.extend(chain.iter().map(String::as_str));
    for len in (min_len + 1..=segs.len()).rev() {
        let candidate = segs[..len].join("::");
        if let Some(&idx) = by_path.get(&candidate) {
            return Some(idx);
        }
    }
    None
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_path_conversion() {
        assert_eq!(module_path_of("foo/bar.rs"), "foo::bar");
        assert_eq!(module_path_of("foo/mod.rs"), "foo");
        assert_eq!(module_path_of("utils.rs"), "utils");
        assert_eq!(module_path_of("a/b/c.rs"), "a::b::c");
    }

    fn sample() -> (String, Vec<(String, String)>) {
        let main_rs = "\
mod pins;
mod mw_radar;
use crate::mw_radar::read_report::HmmdFrame;
fn main() {
    pins::configs::usart1::init();
}
";
        let files = vec![
            ("pins/mod.rs".into(), "pub mod configs;\npub fn setup() {}\n".into()),
            ("pins/configs/mod.rs".into(), "pub mod usart1;\n".into()),
            ("pins/configs/usart1.rs".into(), "pub fn init() {}\n".into()),
            (
                "mw_radar/mod.rs".into(),
                "pub mod read_report;\npub mod utils;\npub struct Parser;\n".into(),
            ),
            (
                "mw_radar/read_report.rs".into(),
                "use super::utils::checksum;\npub struct HmmdFrame;\n".into(),
            ),
            ("mw_radar/utils.rs".into(), "pub fn checksum() -> u8 { 0 }\n".into()),
        ];
        (main_rs.to_owned(), files)
    }

    fn idx(g: &ModuleGraph, path: &str) -> usize {
        g.nodes.iter().position(|n| n.path == path).unwrap()
    }

    #[test]
    fn builds_nodes_and_containment() {
        let (main_rs, files) = sample();
        let g = build_graph(&main_rs, &files);
        assert_eq!(g.nodes.len(), 7); // main + 6 files
        assert_eq!(g.nodes[0].name, "main");
        let main = 0;
        let pins = idx(&g, "pins");
        let configs = idx(&g, "pins::configs");
        let usart1 = idx(&g, "pins::configs::usart1");
        let mw = idx(&g, "mw_radar");
        let rr = idx(&g, "mw_radar::read_report");
        let ut = idx(&g, "mw_radar::utils");
        for e in [
            (main, pins),
            (main, mw),
            (pins, configs),
            (configs, usart1),
            (mw, rr),
            (mw, ut),
        ] {
            assert!(g.contains.contains(&e), "missing containment {e:?}");
        }
    }

    #[test]
    fn resolves_dependency_edges() {
        let (main_rs, files) = sample();
        let g = build_graph(&main_rs, &files);
        let main = 0;
        let usart1 = idx(&g, "pins::configs::usart1");
        let rr = idx(&g, "mw_radar::read_report");
        let ut = idx(&g, "mw_radar::utils");
        // use crate::mw_radar::read_report::HmmdFrame → main → read_report
        assert!(g.deps.contains(&(main, rr)));
        // inline pins::configs::usart1::init() → main → usart1 (deepest match)
        assert!(g.deps.contains(&(main, usart1)));
        // use super::utils::checksum → read_report → utils
        assert!(g.deps.contains(&(rr, ut)));
        // no self-edges, no dep duplicating a containment edge
        assert!(g.deps.iter().all(|(a, b)| a != b));
        assert!(g.deps.iter().all(|e| !g.contains.contains(e)));
    }

    #[test]
    fn use_super_item_depends_on_parent_module() {
        let main_rs = "mod a;\n".to_owned();
        let files = vec![
            ("a/mod.rs".into(), "pub mod b;\npub const K: u8 = 1;\n".into()),
            ("a/b.rs".into(), "use super::K;\n".into()),
        ];
        let g = build_graph(&main_rs, &files);
        let a = idx(&g, "a");
        let b = idx(&g, "a::b");
        assert!(g.deps.contains(&(b, a)), "super::ITEM should point at parent");
    }

    #[test]
    fn ignores_external_crates_and_counts_items() {
        let main_rs = "use cortex_m::asm;\nfn main() { cortex_m::asm::nop(); }\n";
        let g = build_graph(main_rs, &[]);
        assert!(g.deps.is_empty(), "external crate paths must not create edges");
        assert_eq!(g.nodes[0].fn_count, 1);
    }

    #[test]
    fn extracts_top_level_symbols_with_lines() {
        let text = "\
pub struct Parser<const N: usize> {
    len: usize,
}
pub enum State { Idle, Busy }
impl Parser<8> {
    pub fn feed(&mut self, b: u8) {}
}
pub fn checksum(data: &[u8]) -> u8 { 0 }
trait Frame {}
";
        let g = build_graph("mod a;\n", &[("a.rs".into(), text.into())]);
        let a = &g.nodes[1];
        let names: Vec<(&str, SymKind, usize)> = a
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Parser", SymKind::Struct, 1),
                ("State", SymKind::Enum, 4),
                ("checksum", SymKind::Fn, 8),
                ("Frame", SymKind::Trait, 9),
            ],
            "top-level items only — the indented `fn feed` method is excluded"
        );
        // …but the badge still counts the method.
        assert_eq!(a.fn_count, 2);
        assert_eq!(a.ty_count, 3);
    }

    /// Selecting a package's `mod.rs` focuses the whole subtree; a plain file
    /// focuses only itself.
    #[test]
    fn focus_set_expands_package_roots() {
        let (main_rs, files) = sample(); // pins/{mod,configs/{mod,usart1}} + mw_radar/…
        let g = build_graph(&main_rs, &files);
        let idx = |p: &str| g.nodes.iter().position(|n| n.path == p).unwrap();

        // pins/mod.rs → pins + configs + usart1, NOT mw_radar or main.
        let set = g.focus_set(idx("pins"));
        assert!(set[idx("pins")] && set[idx("pins::configs")] && set[idx("pins::configs::usart1")]);
        assert!(!set[idx("mw_radar")] && !set[0]);

        // A leaf file focuses itself alone.
        let set = g.focus_set(idx("pins::configs::usart1"));
        assert_eq!(set.iter().filter(|&&b| b).count(), 1);
        assert!(set[idx("pins::configs::usart1")]);
    }

    /// Call sites inside `impl` methods must attribute to the implemented
    /// TYPE's row — modules keeping their logic in impl methods (most library
    /// code) previously produced no outgoing call edges at all.
    #[test]
    fn impl_method_sites_attribute_to_the_type_row() {
        let text = "\
pub struct Parser {
    len: usize,
}
impl Parser {
    pub fn feed(&mut self, b: u8) {
        helper(b);
    }
}
pub struct HmmdFrame;
impl Frame for HmmdFrame {
    fn decode(&self) {
        helper(0);
    }
}
impl core::fmt::Display for External {
    fn fmt(&self) {}
}
";
        let g = build_graph(text, &[]);
        let node = &g.nodes[0];
        let parser = node.symbols.iter().position(|s| s.name == "Parser").unwrap();
        let frame = node
            .symbols
            .iter()
            .position(|s| s.name == "HmmdFrame")
            .unwrap();
        // Site inside `impl Parser` (line 6) → the Parser row.
        assert_eq!(node.enclosing_row(6), Some(parser));
        // Site inside `impl Frame for HmmdFrame` (line 12) → the TYPE's row.
        assert_eq!(node.enclosing_row(12), Some(frame));
        // Impl on an external type resolves to no local row → dropped.
        assert_eq!(node.enclosing_row(16), None);
    }

    #[test]
    fn impl_target_name_shapes() {
        assert_eq!(impl_target_name(" Parser {").as_deref(), Some("Parser"));
        assert_eq!(
            impl_target_name("<const N: usize> Parser<N> {").as_deref(),
            Some("Parser")
        );
        assert_eq!(
            impl_target_name(" Frame for HmmdFrame {").as_deref(),
            Some("HmmdFrame")
        );
        assert_eq!(
            impl_target_name(" fmt::Display for Config {").as_deref(),
            Some("Config")
        );
        assert_eq!(
            impl_target_name("<'a> Iterator for &mut Cursor<'a> where Self: Sized {").as_deref(),
            Some("Cursor")
        );
    }

    /// Regression (user report): statements written at COLUMN 0 inside `fn
    /// main` plus call sites inside a closure. The old column-0 anchor rule
    /// treated `let x = 1;` / `radar.read_data(|rx| {` as anonymous item
    /// boundaries, so every site after them mapped to no symbol and its call
    /// edge vanished. Brace-depth spans are immune to indentation.
    #[test]
    fn spans_survive_column0_statements_and_closures() {
        let text = "\
#[entry]
fn main() {
let x = 1;
radar.read_data(|rx| {
    helper();
});
}
pub fn other(
    a: u8,
) -> u8 {
    a
}
";
        let g = build_graph(text, &[]);
        let node = &g.nodes[0];
        let names: Vec<(&str, usize, usize)> = node
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.line, s.end_line))
            .collect();
        assert_eq!(
            names,
            vec![("main", 2, 7), ("other", 8, 12)],
            "col-0 statements must not end main's span; multi-line signature spans to its brace"
        );
        assert_eq!(node.enclosing_row(5), Some(0), "closure site belongs to main");
        assert_eq!(node.enclosing_row(3), Some(0), "col-0 statement belongs to main");
        assert_eq!(node.enclosing_row(11), Some(1), "body of the multi-line-sig fn");
        assert_eq!(node.enclosing_row(1), None, "the attr line is outside any item");
    }

    #[test]
    fn cyclic_uses_do_not_break_the_graph() {
        let main_rs = "mod a;\nmod b;\n".to_owned();
        let files = vec![
            ("a.rs".into(), "use crate::b::f;\n".into()),
            ("b.rs".into(), "use crate::a::g;\n".into()),
        ];
        let g = build_graph(&main_rs, &files);
        let a = idx(&g, "a");
        let b = idx(&g, "b");
        assert!(g.deps.contains(&(a, b)) && g.deps.contains(&(b, a)));
    }
}
