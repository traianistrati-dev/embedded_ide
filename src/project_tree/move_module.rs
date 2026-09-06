//! Working out what a file move does to the `mod` declarations around it.
//!
//! Moving `src/radar.rs` into `src/drivers/` does not just move bytes: the
//! `mod radar;` line that CREATES the module lives in the old parent and has to
//! be taken out and put into the new one. Nothing else in the tree does this, so
//! before this module a move left `mod radar;` pointing at a file that was gone.
//!
//! # What is mechanical here and what is not
//!
//! The edit at the DECLARATION site is mechanical — it needs `mod`-item text,
//! not name resolution. Every edit at a USE site (`crate::radar::X` becoming
//! `crate::drivers::radar::X`) is semantic: deciding whether a given `radar::`
//! token even means this module needs the module tree, and `#[path]` / `#[cfg]`
//! make that tree configuration-dependent. rust-analyzer cannot help — its
//! `willRenameFiles` is same-directory-only (a guard unchanged since 2020) and
//! no assist takes a destination directory.
//!
//! So this module does the mechanical half exactly, and offers a RE-EXPORT for
//! the other half: `pub(crate) use crate::drivers::radar;` left behind in the
//! old parent keeps `crate::radar::X`, `super::radar::X` and a bare `radar::X`
//! compiling with no edits at all. One line instead of a refactor nobody can do
//! correctly.
//!
//! # Three rules learned from rustc, each the hard way
//!
//! **Edits fold, they do not stack.** Two steps can target the same file — the
//! old parent losing the declaration and, when the destination folder is new,
//! that same parent gaining `mod drivers;`. Emitting both as whole-file
//! replacements built from the ORIGINAL text makes the second silently discard
//! the first, leaving `mod radar;` behind for an E0583. Everything here edits a
//! working set keyed by path, read out as one edit per file at the end.
//!
//! **`pub use` cannot re-export something less than `pub`** (E0365). The
//! re-export is therefore `pub(crate) use`, which is what every in-crate path
//! needs anyway.
//!
//! **The visibility keyword does not carry over, and must never narrow.** A bare
//! `mod radar;` at a crate root is already reachable crate-wide, because the
//! root's descendants are the whole crate; the same keyword inside `drivers/`
//! reaches only `drivers/`. So a move DOWN widens to `pub(crate)`. But an
//! original `pub mod` must stay `pub`, or a library module stops being
//! reachable from outside the crate. Never narrower than either the original or
//! what the new depth requires.

use std::collections::BTreeMap;

/// One file edit the move needs, as a whole-content replacement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModEdit {
    /// Project-root-relative path of the file to write.
    pub path: String,
    pub new_content: String,
    /// True when the file does not exist yet and has to be created.
    pub create: bool,
}

/// Everything a move implies beyond renaming the file itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModPlan {
    pub edits: Vec<ModEdit>,
    /// Human-readable notes for the tree notice — what was done, and what the
    /// user still has to look at.
    pub notes: Vec<String>,
}

/// How visible a `mod` item is. Ordered: `Private < Crate < Public`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Vis {
    Private,
    Crate,
    Public,
}

impl Vis {
    fn keyword(self) -> &'static str {
        match self {
            Self::Private => "",
            Self::Crate => "pub(crate) ",
            Self::Public => "pub ",
        }
    }
}

// ── Lexical scanning ─────────────────────────────────────────────────────────
// A `mod` item is recognised from LINE text, but only on lines that are really
// top-level code. Without this, a `mod x;` inside an inline `mod y { … }`, a
// block comment or a raw string all read as declarations, and a new one gets
// spliced into the middle of them.

/// For each line of `src`, whether it begins at brace depth 0 and outside any
/// comment or string.
fn top_level_flags(src: &str) -> Vec<bool> {
    let bytes = src.as_bytes();
    let mut flags = Vec::new();
    let mut depth: i32 = 0;
    let mut block_comment: i32 = 0; // Rust block comments nest
    let mut in_str = false;
    let mut raw_hashes: Option<usize> = None; // inside r#".."#
    let mut i = 0;
    let mut line_start = true;

    while i <= bytes.len() {
        if line_start {
            flags.push(depth == 0 && block_comment == 0 && !in_str && raw_hashes.is_none());
            line_start = false;
        }
        if i == bytes.len() {
            break;
        }
        let c = bytes[i];
        if c == b'\n' {
            line_start = true;
            i += 1;
            continue;
        }
        if let Some(hashes) = raw_hashes {
            if c == b'"' && src[i + 1..].bytes().take(hashes).all(|b| b == b'#') {
                raw_hashes = None;
                i += 1 + hashes;
                continue;
            }
            i += 1;
            continue;
        }
        if in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if block_comment > 0 {
            if src[i..].starts_with("/*") {
                block_comment += 1;
                i += 2;
                continue;
            }
            if src[i..].starts_with("*/") {
                block_comment -= 1;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if src[i..].starts_with("//") {
            match src[i..].find('\n') {
                Some(n) => i += n,
                None => break,
            }
            continue;
        }
        if src[i..].starts_with("/*") {
            block_comment = 1;
            i += 2;
            continue;
        }
        if c == b'r' && (src[i + 1..].starts_with('"') || src[i + 1..].starts_with('#')) {
            let hashes = src[i + 1..].bytes().take_while(|b| *b == b'#').count();
            if src[i + 1 + hashes..].starts_with('"') {
                raw_hashes = Some(hashes);
                i += 2 + hashes;
                continue;
            }
        }
        if c == b'"' {
            in_str = true;
            i += 1;
            continue;
        }
        if c == b'\'' {
            // A char literal, or a lifetime. Only `'x'` / `'\x'` shapes are
            // literals; a lifetime holds no braces, so treating it as ordinary
            // text is safe.
            let rest = &src[i + 1..];
            let lit_len = if let Some(escaped) = rest.strip_prefix('\\') {
                escaped.find('\'').map(|n| n + 2)
            } else {
                rest.char_indices()
                    .nth(1)
                    .and_then(|(off, _)| rest[off..].starts_with('\'').then_some(off + 1))
            };
            if let Some(n) = lit_len {
                i += 1 + n;
                continue;
            }
        }
        match c {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    flags
}

/// A line with any trailing `//` comment removed. Only called on lines already
/// known to be top-level code, so a `//` inside a string cannot reach here.
fn code_of(line: &str) -> &str {
    match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// Parse a top-level line as a `mod <name>;` item, returning its visibility.
///
/// Accepts every visibility form and a trailing comment — `mod radar; // sensor`
/// used to be rejected outright, which meant the move produced no edits at all
/// and left the declaration dangling.
fn parse_mod_decl(line: &str) -> Option<(Vis, &str)> {
    let t = code_of(line).trim();
    let rest = t.strip_suffix(';')?.trim_end();
    let (vis, rest) = match rest.strip_prefix("pub") {
        Some(after) if after.is_empty() || after.starts_with(['(', ' ', '\t']) => {
            let after = after.trim_start();
            match after.strip_prefix('(') {
                Some(inner) => {
                    let close = inner.find(')')?;
                    let scope = inner[..close].trim();
                    // `pub(crate)` / `pub(in …)` / `pub(super)` are all
                    // crate-limited for our purpose; `pub(self)` is private.
                    let vis = if scope == "self" {
                        Vis::Private
                    } else {
                        Vis::Crate
                    };
                    (vis, inner[close + 1..].trim_start())
                }
                None => (Vis::Public, after),
            }
        }
        _ => (Vis::Private, rest),
    };
    let name = rest.strip_prefix("mod")?;
    // `modfoo;` must not parse as `mod foo;`.
    if !name.starts_with([' ', '\t']) {
        return None;
    }
    Some((vis, name.trim()))
}

/// Byte range of the `mod <name>;` item in `src`, INCLUDING the attribute lines
/// directly above it, plus its visibility.
///
/// The attributes come along because `#[cfg(feature = "x")]` belongs to the item
/// it precedes: deleting only the `mod` line would re-target the attribute onto
/// whatever follows, silently gating an unrelated item.
fn find_mod_decl(src: &str, name: &str) -> Option<(std::ops::Range<usize>, Vis)> {
    let flags = top_level_flags(src);
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    let mut starts = Vec::with_capacity(lines.len());
    let mut off = 0usize;
    for line in &lines {
        starts.push(off);
        off += line.len();
    }

    for (i, line) in lines.iter().enumerate() {
        if !flags.get(i).copied().unwrap_or(false) {
            continue;
        }
        let Some((vis, found)) = parse_mod_decl(line) else {
            continue;
        };
        if found != name {
            continue;
        }
        let mut first = i;
        while first > 0 {
            let prev = lines[first - 1].trim();
            let attached = prev.starts_with("#[")
                || prev.starts_with("#![")
                || prev.starts_with("///")
                || prev.starts_with("//!");
            if attached && flags.get(first - 1).copied().unwrap_or(false) {
                first -= 1;
            } else {
                break;
            }
        }
        return Some((starts[first]..starts[i] + lines[i].len(), vis));
    }
    None
}

/// Remove the `mod <name>;` item (with its attributes) from `content`.
/// `None` when there is nothing to remove.
pub fn remove_mod_decl(content: &str, name: &str) -> Option<(String, Vis)> {
    let (range, vis) = find_mod_decl(content, name)?;
    let mut out = String::with_capacity(content.len());
    out.push_str(&content[..range.start]);
    out.push_str(&content[range.end..]);
    Some((out, vis))
}

/// Insert `decl` next to the other top-level `mod` items, or at the end.
///
/// Only TOP-LEVEL lines count as anchors — a purely line-based version would
/// happily splice a declaration inside an inline `mod x { … }`, a block comment
/// or a raw string.
pub fn insert_mod_decl(content: &str, decl: &str) -> String {
    let flags = top_level_flags(content);
    let mut last_end = None;
    let mut off = 0usize;
    for (i, line) in content.split_inclusive('\n').enumerate() {
        if flags.get(i).copied().unwrap_or(false) && parse_mod_decl(line).is_some() {
            last_end = Some(off + line.len());
        }
        off += line.len();
    }
    match last_end {
        Some(at) => {
            let mut out = String::with_capacity(content.len() + decl.len());
            out.push_str(&content[..at]);
            out.push_str(decl);
            out.push_str(&content[at..]);
            out
        }
        None => {
            let mut out = content.to_owned();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(decl);
            out
        }
    }
}

// ── Paths ────────────────────────────────────────────────────────────────────

/// The parent folder of a project-root-relative path (`""` for a root file).
pub fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// The module name a `.rs` file declares.
pub fn module_name(path: &str) -> &str {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    leaf.strip_suffix(".rs").unwrap_or(leaf)
}

/// Is `dir` the `src` of some crate?
fn is_crate_src(dir: &str) -> bool {
    dir == "src" || dir.ends_with("/src")
}

/// The module file that declares the children of `dir`, and whether it exists.
///
/// Three shapes, not interchangeable:
/// * `dir/mod.rs` — the classic form.
/// * `dir.rs` next to `dir/` — the 2018 form; the declaration goes THERE.
/// * a crate root — `main.rs` or `lib.rs`.
///
/// Creating `dir/mod.rs` while `dir.rs` exists is E0761, so the sibling form is
/// checked first. At a LIBRARY root the default is `lib.rs`: only the firmware
/// crate at `src/` has a generated `main.rs`, and defaulting a library to a
/// `main.rs` that does not exist wrote the declaration into a phantom file.
pub fn module_file_of(dir: &str, exists: impl Fn(&str) -> bool) -> (String, bool) {
    if is_crate_src(dir) {
        for leaf in ["lib.rs", "main.rs"] {
            let cand = format!("{dir}/{leaf}");
            if exists(&cand) {
                return (cand, true);
            }
        }
        // Nothing listed: the firmware crate's `src/main.rs` is GENERATED and
        // never appears as a user file, so it is there even when unlisted.
        return if dir == "src" {
            (format!("{dir}/main.rs"), true)
        } else {
            (format!("{dir}/lib.rs"), false)
        };
    }
    let sibling = format!("{dir}.rs");
    if exists(&sibling) {
        return (sibling, true);
    }
    let mod_rs = format!("{dir}/mod.rs");
    let there = exists(&mod_rs);
    (mod_rs, there)
}

/// Path of `dir` below its crate root (`""` at the root).
fn below_crate_root(dir: &str) -> &str {
    if is_crate_src(dir) {
        return "";
    }
    if let Some(rest) = dir.strip_prefix("src/") {
        return rest;
    }
    dir.split_once("/src/").map(|(_, r)| r).unwrap_or("")
}

/// `crate::drivers::radar` for a module `radar` in folder `src/drivers`.
fn module_path_expr(dir: &str, name: &str) -> String {
    let rel = below_crate_root(dir);
    if rel.is_empty() {
        format!("crate::{name}")
    } else {
        format!("crate::{}::{name}", rel.replace('/', "::"))
    }
}

/// The declaration line for `name` landing in `dest_dir`.
///
/// Never narrower than `was`: an original `pub mod` keeps `pub`, or a library
/// module silently stops being reachable from outside its crate.
fn destination_decl(name: &str, dest_dir: &str, was: Vis) -> String {
    let needed = if below_crate_root(dest_dir).is_empty() {
        Vis::Private
    } else {
        Vis::Crate
    };
    format!("{}mod {name};\n", was.max(needed).keyword())
}

// ── Planning ─────────────────────────────────────────────────────────────────

/// Plan the declaration surgery for moving `old_path` to `new_path`.
///
/// `content_of` reads a project-root-relative file; `exists` answers whether one
/// is present. `keep_old_paths` adds the re-export that keeps every existing
/// `crate::<old parent>::<name>` path compiling.
///
/// `None` when there is no declaration work: a non-`.rs` file, or a move that
/// stays in the same folder (that is a rename, a different feature).
pub fn plan_move(
    old_path: &str,
    new_path: &str,
    keep_old_paths: bool,
    exists: impl Fn(&str) -> bool,
    content_of: impl Fn(&str) -> Option<String>,
) -> Option<ModPlan> {
    if !old_path.ends_with(".rs") {
        return None;
    }
    let name = module_name(old_path);
    let old_dir = parent_of(old_path);
    let new_dir = parent_of(new_path);
    if old_dir == new_dir {
        return None;
    }

    let mut plan = ModPlan::default();
    // The working set: every step reads through it, so two steps touching one
    // file compose instead of the second discarding the first.
    let mut edited: BTreeMap<String, String> = BTreeMap::new();
    let mut created: Vec<String> = Vec::new();
    macro_rules! get {
        ($p:expr) => {{
            let p: &str = $p;
            edited.get(p).cloned().or_else(|| content_of(p))
        }};
    }
    macro_rules! here {
        () => {
            |p: &str| exists(p) || created.iter().any(|c| c == p)
        };
    }

    // ── The old parent loses the declaration ────────────────────────────────
    let (old_mod_file, _) = module_file_of(old_dir, here!());
    let Some(old_content) = get!(&old_mod_file) else {
        return Some(plan);
    };
    let Some((after_removal, was)) = remove_mod_decl(&old_content, name) else {
        plan.notes.push(format!(
            "no `mod {name};` found in `{old_mod_file}` - the file was not in the module tree"
        ));
        return Some(plan);
    };
    edited.insert(old_mod_file.clone(), after_removal);

    // ── The destination folder chain ────────────────────────────────────────
    // EVERY ancestor between the crate root and the destination must be a
    // module, not just the last one. Following only one level left a two-level
    // destination orphaned: the innermost file existed and nothing declared the
    // folder above it (E0433).
    let mut missing = Vec::new();
    let mut dir = new_dir.to_owned();
    while !is_crate_src(&dir) && !dir.is_empty() {
        let (file, there) = module_file_of(&dir, here!());
        if there {
            break;
        }
        missing.push((dir.clone(), file));
        dir = parent_of(&dir).to_owned();
    }
    // Outermost first, so each new folder is declared in a parent that exists.
    for (folder_dir, folder_file) in missing.into_iter().rev() {
        created.push(folder_file.clone());
        edited.insert(folder_file.clone(), String::new());
        plan.notes.push(format!(
            "created `{folder_file}` for the destination folder"
        ));

        let folder_name = folder_dir.rsplit('/').next().unwrap_or(&folder_dir);
        let parent_dir = parent_of(&folder_dir);
        let (parent_file, _) = module_file_of(parent_dir, here!());
        if let Some(parent_content) = get!(&parent_file)
            && find_mod_decl(&parent_content, folder_name).is_none()
        {
            let decl = destination_decl(folder_name, parent_dir, Vis::Private);
            edited.insert(parent_file, insert_mod_decl(&parent_content, &decl));
            plan.notes
                .push(format!("declared the new folder as `mod {folder_name};`"));
        }
    }

    // ── The new parent gains the declaration ────────────────────────────────
    let (new_mod_file, _) = module_file_of(new_dir, here!());
    let base = get!(&new_mod_file).unwrap_or_default();
    let decl = destination_decl(name, new_dir, was);
    edited.insert(new_mod_file, insert_mod_decl(&base, &decl));

    // ── The re-export that saves the use sites ──────────────────────────────
    // `pub(crate) use`, never `pub use`: re-exporting an item that is less than
    // `pub` is E0365, and the destination declaration is at most `pub(crate)`
    // unless the original was already `pub`.
    if keep_old_paths {
        let target = module_path_expr(new_dir, name);
        let vis = if was == Vis::Public {
            "pub"
        } else {
            "pub(crate)"
        };
        let line = format!("{vis} use {target};\n");
        let cur = get!(&old_mod_file).unwrap_or_default();
        edited.insert(old_mod_file.clone(), insert_mod_decl(&cur, &line));
        plan.notes.push(format!(
            "left `{vis} use {target};` behind, so existing `{name}::` paths keep working"
        ));
    } else {
        plan.notes.push(format!(
            "every `{name}::` path now needs the new module path - they are NOT rewritten"
        ));
    }

    plan.edits = edited
        .into_iter()
        .map(|(path, new_content)| ModEdit {
            create: created.contains(&path),
            path,
            new_content,
        })
        .collect();
    Some(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(p, c)| ((*p).to_owned(), (*c).to_owned()))
            .collect()
    }

    fn run(have: &[(String, String)], old: &str, new: &str, keep: bool) -> ModPlan {
        plan_move(
            old,
            new,
            keep,
            |p| have.iter().any(|(hp, _)| hp == p),
            |p| have.iter().find(|(hp, _)| hp == p).map(|(_, c)| c.clone()),
        )
        .unwrap()
    }

    fn content(plan: &ModPlan, path: &str) -> String {
        plan.edits
            .iter()
            .find(|e| e.path == path)
            .unwrap_or_else(|| panic!("no edit for {path}: {:?}", plan.edits))
            .new_content
            .clone()
    }

    #[test]
    fn a_mod_line_is_parsed_with_its_visibility() {
        assert_eq!(parse_mod_decl("mod radar;"), Some((Vis::Private, "radar")));
        assert_eq!(
            parse_mod_decl("  pub mod radar;"),
            Some((Vis::Public, "radar"))
        );
        assert_eq!(
            parse_mod_decl("pub(crate) mod radar;"),
            Some((Vis::Crate, "radar"))
        );
        assert_eq!(
            parse_mod_decl("pub(in crate::a) mod radar;\n"),
            Some((Vis::Crate, "radar"))
        );
        // A trailing comment used to make the whole item unrecognisable, so the
        // move produced no edits and left the declaration dangling.
        assert_eq!(
            parse_mod_decl("mod radar; // the sensor"),
            Some((Vis::Private, "radar"))
        );
        assert_eq!(parse_mod_decl("mod radar ;"), Some((Vis::Private, "radar")));
    }

    #[test]
    fn things_that_merely_mention_the_name_are_not_declarations() {
        for line in [
            "use crate::radar::Frame;",
            "// mod radar;",
            "mod radar {",
            "let radar = 3;",
            "modradar;",
            "pub use radar::Frame;",
        ] {
            assert!(
                parse_mod_decl(line).is_none_or(|(_, n)| n != "radar"),
                "{line:?}"
            );
        }
    }

    /// A line-based scan treats all of these as top-level code. They are not.
    #[test]
    fn only_real_top_level_lines_count() {
        let src = "mod real;\nmod inner {\n    mod nested;\n}\n/*\nmod commented;\n*/\nconst S: &str = r#\"\nmod in_a_raw_string;\n\"#;\n";
        assert!(find_mod_decl(src, "real").is_some());
        assert!(
            find_mod_decl(src, "nested").is_none(),
            "inside an inline mod"
        );
        assert!(
            find_mod_decl(src, "commented").is_none(),
            "inside a block comment"
        );
        assert!(
            find_mod_decl(src, "in_a_raw_string").is_none(),
            "inside a raw string"
        );
    }

    /// `#[cfg]` belongs to the item under it. Removing only the `mod` line
    /// re-targets the attribute onto whatever follows.
    #[test]
    fn attributes_travel_with_their_declaration() {
        let src = "#[cfg(feature = \"x\")]\nmod radar;\nmod other;\n";
        let (after, vis) = remove_mod_decl(src, "radar").unwrap();
        assert_eq!(after, "mod other;\n");
        assert_eq!(vis, Vis::Private);
    }

    #[test]
    fn a_new_declaration_joins_the_other_top_level_mods() {
        let src = "mod a;\nmod b;\n\nfn main() {}\n";
        assert_eq!(
            insert_mod_decl(src, "mod radar;\n"),
            "mod a;\nmod b;\nmod radar;\n\nfn main() {}\n"
        );
        // A `mod` inside an inline block is not an anchor.
        let nested = "mod outer {\n    mod inner;\n}\n";
        assert_eq!(
            insert_mod_decl(nested, "mod r;\n"),
            "mod outer {\n    mod inner;\n}\nmod r;\n"
        );
        assert_eq!(insert_mod_decl("", "mod r;\n"), "mod r;\n");
    }

    #[test]
    fn the_sibling_module_file_wins_over_mod_rs() {
        assert_eq!(
            module_file_of("src/drivers", |p| p == "src/drivers.rs"),
            ("src/drivers.rs".to_owned(), true)
        );
        assert_eq!(
            module_file_of("src/drivers", |_| false),
            ("src/drivers/mod.rs".to_owned(), false)
        );
    }

    /// A library root is `lib.rs`. Defaulting it to a `main.rs` that does not
    /// exist wrote the declaration into a phantom file.
    #[test]
    fn a_library_root_is_lib_rs() {
        assert_eq!(
            module_file_of("mylib/src", |p| p == "mylib/src/lib.rs"),
            ("mylib/src/lib.rs".to_owned(), true)
        );
        assert_eq!(
            module_file_of("mylib/src", |_| false),
            ("mylib/src/lib.rs".to_owned(), false)
        );
        // The firmware crate's main.rs is generated, so it is there unlisted.
        assert_eq!(
            module_file_of("src", |_| false),
            ("src/main.rs".to_owned(), true)
        );
    }

    /// Widen when the depth demands it, but NEVER narrow an existing `pub`.
    #[test]
    fn visibility_never_narrows() {
        assert_eq!(
            destination_decl("radar", "src", Vis::Private),
            "mod radar;\n"
        );
        assert_eq!(
            destination_decl("radar", "src/drivers", Vis::Private),
            "pub(crate) mod radar;\n"
        );
        assert_eq!(
            destination_decl("radar", "src/hal/drivers", Vis::Crate),
            "pub(crate) mod radar;\n"
        );
        assert_eq!(
            destination_decl("radar", "src/drivers", Vis::Public),
            "pub mod radar;\n"
        );
        assert_eq!(
            destination_decl("radar", "src", Vis::Public),
            "pub mod radar;\n"
        );
    }

    #[test]
    fn module_paths_are_absolute_from_the_crate_root() {
        assert_eq!(module_path_expr("src", "radar"), "crate::radar");
        assert_eq!(
            module_path_expr("src/drivers", "radar"),
            "crate::drivers::radar"
        );
        assert_eq!(
            module_path_expr("src/hal/drivers", "radar"),
            "crate::hal::drivers::radar"
        );
        assert_eq!(
            module_path_expr("mylib/src/io", "radar"),
            "crate::io::radar"
        );
        assert_eq!(module_path_expr("mylib/src", "radar"), "crate::radar");
    }

    #[test]
    fn a_move_down_moves_the_declaration_and_leaves_a_re_export() {
        let have = files(&[
            ("src/main.rs", "mod radar;\nmod drivers;\n\nfn main() {}\n"),
            ("src/drivers/mod.rs", "mod uart;\n"),
        ]);
        let plan = run(&have, "src/radar.rs", "src/drivers/radar.rs", true);
        assert_eq!(
            content(&plan, "src/main.rs"),
            "mod drivers;\npub(crate) use crate::drivers::radar;\n\nfn main() {}\n"
        );
        assert_eq!(
            content(&plan, "src/drivers/mod.rs"),
            "mod uart;\npub(crate) mod radar;\n"
        );
    }

    /// THE regression: the destination folder is new AND sits under the source's
    /// own parent, so both steps target `src/main.rs`. Emitting them as
    /// independent whole-file replacements made the second discard the first —
    /// `mod radar;` survived while the file was gone (E0583).
    #[test]
    fn two_steps_on_one_file_compose_instead_of_overwriting() {
        let have = files(&[("src/main.rs", "mod pins;\nmod radar;\n\nfn main() {}\n")]);
        let plan = run(&have, "src/radar.rs", "src/drivers/radar.rs", true);

        let mains = plan
            .edits
            .iter()
            .filter(|e| e.path == "src/main.rs")
            .count();
        assert_eq!(mains, 1, "one folded edit, not two: {:?}", plan.edits);

        let main = content(&plan, "src/main.rs");
        assert!(
            !main.contains("mod radar;"),
            "the stale declaration must be gone: {main}"
        );
        assert!(main.contains("mod drivers;"), "{main}");
        assert!(
            main.contains("pub(crate) use crate::drivers::radar;"),
            "the re-export must survive: {main}"
        );
        assert_eq!(
            content(&plan, "src/drivers/mod.rs"),
            "pub(crate) mod radar;\n"
        );
    }

    /// Every ancestor has to become a module, not just the innermost one.
    #[test]
    fn a_two_level_destination_declares_the_whole_chain() {
        let have = files(&[("src/main.rs", "mod radar;\n")]);
        let plan = run(&have, "src/radar.rs", "src/hal/drivers/radar.rs", false);

        assert_eq!(
            content(&plan, "src/hal/drivers/mod.rs"),
            "pub(crate) mod radar;\n"
        );
        let hal = content(&plan, "src/hal/mod.rs");
        assert!(hal.contains("mod drivers;"), "{hal}");
        let main = content(&plan, "src/main.rs");
        assert!(main.contains("mod hal;"), "{main}");
        assert!(!main.contains("mod radar;"), "{main}");
    }

    #[test]
    fn without_the_re_export_the_notes_say_so() {
        let have = files(&[("src/main.rs", "mod radar;\n")]);
        let plan = run(&have, "src/radar.rs", "src/drivers/radar.rs", false);
        assert!(!content(&plan, "src/main.rs").contains("use crate::"));
        assert!(plan.notes.iter().any(|n| n.contains("NOT rewritten")));
    }

    /// A `pub mod` in a library keeps `pub`, and its re-export is `pub` too —
    /// `pub(crate) use` would silently drop the reach the `pub` was for.
    #[test]
    fn a_public_library_module_keeps_its_reach() {
        let have = files(&[
            ("mylib/src/lib.rs", "pub mod radar;\n"),
            ("mylib/src/io/mod.rs", ""),
        ]);
        let plan = run(&have, "mylib/src/radar.rs", "mylib/src/io/radar.rs", true);
        assert_eq!(content(&plan, "mylib/src/io/mod.rs"), "pub mod radar;\n");
        assert_eq!(
            content(&plan, "mylib/src/lib.rs"),
            "pub use crate::io::radar;\n"
        );
    }

    #[test]
    fn an_unlinked_file_needs_no_surgery() {
        let have = files(&[("src/main.rs", "fn main() {}\n")]);
        let plan = run(&have, "src/orphan.rs", "src/drivers/orphan.rs", true);
        assert!(plan.edits.is_empty());
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("not in the module tree"))
        );
    }

    #[test]
    fn a_non_rust_file_and_a_same_folder_move_need_no_plan() {
        assert!(plan_move("src/notes.md", "src/doc/notes.md", true, |_| true, |_| None).is_none());
        assert!(plan_move("src/a.rs", "src/b.rs", true, |_| true, |_| None).is_none());
    }
}
