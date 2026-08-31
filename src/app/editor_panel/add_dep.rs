//! "Add dependency" — the Ctrl+Enter action for a `use` line whose crate is not
//! in Cargo.toml (`unresolved import`).
//!
//! rust-analyzer has no assist for this: it does not know about Cargo.toml. So
//! the row is ours, injected at the top of the code-action list, and choosing it
//! opens a second popup with the crates that could be meant — the same curated
//! list `Ctrl+Space` offers inside a manifest, led by the crate the identifier
//! literally names.
//!
//! Flow: Ctrl+Enter → `add_dep_candidate` decides whether the row appears → the
//! chooser lists candidates → a choice starts one sparse-index fetch (background
//! thread, exactly like the version completion) → the newest non-yanked version
//! becomes `<name> = "<major>"` under `[dependencies]` of the manifest that OWNS
//! this file, and a Save is requested so rust-analyzer sees it.

use super::cargo_complete::VersionFetch;
use crate::app::AppIde;
use eframe::egui;
use std::sync::{Arc, Mutex};

/// One row in the crate chooser.
#[derive(Clone)]
pub(crate) struct Candidate {
    pub name: String,
    pub detail: String,
}

/// Per-`AppIde` state for the crate chooser.
#[derive(Default)]
pub(crate) struct AddDepState {
    /// True while the chooser is visible.
    pub open: bool,
    /// Highlighted row.
    pub sel: usize,
    /// Rows shown in the last rendered frame.
    pub items: Vec<Candidate>,
    /// A choice deferred from a click / keypress, applied at frame top.
    pub choice: Option<usize>,
    pub pos: egui::Pos2,
    /// Project-root-relative path of the manifest the line goes into. Resolved
    /// when the chooser opens — a file inside an extracted library belongs to
    /// THAT crate's Cargo.toml, and writing the root one would leave the error
    /// exactly where it was.
    pub manifest: Option<String>,
    /// `(crate name, shared result)` of the sparse-index fetch in flight.
    pub fetch: Option<(String, Arc<Mutex<VersionFetch>>)>,
    /// One line under the list: what went wrong, or what is being fetched.
    pub note: Option<String>,
}

// ── Pure core ────────────────────────────────────────────────────────────────

/// Path roots that are never a dependency: the current crate and the implicit
/// sysroot crates.
const NOT_A_DEP: [&str; 6] = ["crate", "self", "super", "std", "core", "alloc"];

/// The crate a `use` / `extern crate` line imports from, when that could be a
/// Cargo dependency.
///
/// Returns the RUST identifier (`embedded_io_async`), not the Cargo key — the
/// two differ by `-`/`_` and the caller decides which it needs. `None` for a
/// line that imports nothing, imports from the current crate, or reaches for a
/// sysroot crate.
pub(crate) fn imported_crate(line: &str) -> Option<String> {
    let t = line.trim_start();
    // A commented-out `use` is not an import. The generated config files are
    // full of them — every `pins/configs/*.rs` ends with a commented usage
    // example — and offering to add a dependency for one would be nonsense.
    if t.starts_with("//") {
        return None;
    }
    // `pub use`, `pub(crate) use`, `pub(super) use`, …
    let t = t.strip_prefix("pub").map_or(t, |rest| {
        let rest = rest
            .strip_prefix('(')
            .map_or(rest, |r| r.split_once(')').map_or(r, |(_, after)| after));
        rest.trim_start()
    });
    let rest = t
        .strip_prefix("use ")
        .or_else(|| t.strip_prefix("extern crate "))?;
    // `use ::heapless::Vec;` — a leading `::` names an external crate too.
    let rest = rest.trim_start().trim_start_matches("::");
    let seg: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    // A brace group at the root (`use {a, b};`) leaves `seg` empty.
    if seg.is_empty() || NOT_A_DEP.contains(&seg.as_str()) {
        return None;
    }
    // A bare `use x;` / `use x::…` / `use x as y;` — anything else (an operator
    // where a path separator belongs) is not an import we understand.
    let after = &rest[seg.len()..];
    let ok = after.starts_with("::")
        || after.starts_with(';')
        || after.starts_with(" as ")
        || after.trim().is_empty();
    ok.then_some(seg)
}

/// The Cargo key a Rust identifier most likely names: `embedded_io_async` →
/// `embedded-io-async`.
///
/// The mapping Cargo applies is `-` → `_`, which is not reversible: the
/// identifier could equally come from `embedded_io-async`. All-dashes is the
/// overwhelmingly common form and the one crates.io canonicalises towards, so
/// it leads the list — and it is also the ONLY route to a crate outside the
/// curated set, since there is no crates.io search client here, just the sparse
/// index, which needs an exact name.
pub(crate) fn dash_form(ident: &str) -> String {
    ident.replace('_', "-")
}

/// Is `ident` already satisfied by a dependency in `cargo_toml`?
///
/// Compares against the KEY of every dependency table — `[dependencies]`,
/// `[dev-dependencies]`, `[build-dependencies]` and the target-specific ones.
/// The key is what decides the identifier even for a renamed dependency
/// (`embedded-hal-0-2 = { package = "embedded-hal", … }` is `embedded_hal_0_2`
/// in code), so the key is exactly the right thing to compare.
///
/// All three TOML spellings count, because this is the ONLY thing standing
/// between a already-declared crate and a duplicate line:
/// - `foo = "1"` and `foo = { … }` — the key before `=`;
/// - `foo.workspace = true` / `foo.version = "1"` — the key before the FIRST
///   dot, which is still `foo`;
/// - `[dependencies.foo]` — a whole table per dependency, where the name is the
///   last segment of the section header.
pub(crate) fn has_dep(cargo_toml: &str, ident: &str) -> bool {
    let matches = |key: &str| !key.is_empty() && key.replace('-', "_") == ident;
    let mut in_deps = false;
    for line in cargo_toml.lines() {
        let t = line.trim();
        if let Some(section) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let section = section.trim().trim_matches('"');
            in_deps = section == "dependencies"
                || section == "dev-dependencies"
                || section == "build-dependencies"
                || section.ends_with(".dependencies");
            // `[dependencies.foo]`, `[target.'cfg(x)'.dev-dependencies.foo]`.
            if let Some((table, name)) = section.rsplit_once('.') {
                let table = table.rsplit('.').next().unwrap_or(table);
                if (table == "dependencies"
                    || table == "dev-dependencies"
                    || table == "build-dependencies")
                    && matches(name)
                {
                    return true;
                }
            }
            continue;
        }
        if !in_deps || t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((key, _)) = t.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        // `foo.workspace = true` still declares `foo`.
        let key = key.split_once('.').map_or(key, |(head, _)| head);
        if matches(key.trim().trim_matches('"')) {
            return true;
        }
    }
    false
}

/// The version requirement to write for a full version from the index:
/// `0.7.1` → `0.7`, `1.2.3` → `1`, `0.0.3` → `0.0.3`.
///
/// Keeps segments up to and including the first non-zero one — that is the
/// granularity Cargo's caret requirement actually pins, and the convention every
/// generated line in this project already follows. A pre-release or build
/// suffix is written whole: truncating `1.0.0-rc.1` to `1` would silently
/// resolve to something else.
pub(crate) fn short_version(v: &str) -> String {
    if v.contains('-') || v.contains('+') {
        return v.to_owned();
    }
    let parts: Vec<&str> = v.split('.').collect();
    let mut out: Vec<&str> = Vec::new();
    for p in &parts {
        out.push(p);
        if *p != "0" {
            break;
        }
    }
    out.join(".")
}

/// Insert `<name> = "<version>"` at the top of `[dependencies]`.
///
/// Idempotent — a manifest that already declares the crate comes back
/// unchanged. No [`DEP_MARKER`]: that marker means "the IDE put this here and
/// may take it away again on a feature toggle", and a dependency the user asked
/// for by name is theirs, not ours.
///
/// [`DEP_MARKER`]: crate::panels::mcu_module::project_gen::DEP_MARKER
pub(crate) fn insert_dep_line(cargo_toml: &str, name: &str, version: &str) -> String {
    if has_dep(cargo_toml, &name.replace('-', "_")) {
        return cargo_toml.to_owned();
    }
    let new_line = format!("{name} = \"{version}\"");
    let trailing_nl = cargo_toml.ends_with('\n');
    let mut out: Vec<String> = Vec::new();
    let mut inserted = false;
    for line in cargo_toml.lines() {
        out.push(line.to_string());
        if !inserted && line.trim() == "[dependencies]" {
            out.push(new_line.clone());
            inserted = true;
        }
    }
    if !inserted {
        // No table yet — open one at the end rather than guessing a position.
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push("[dependencies]".to_owned());
        out.push(new_line);
    }
    let mut s = out.join("\n");
    if trailing_nl {
        s.push('\n');
    }
    s
}

/// Crates that could be what `ident` meant, best first.
///
/// The literal dash form leads — it is what the identifier actually names — and
/// the curated embedded list follows, deduplicated against it. That list is the
/// same one `Ctrl+Space` shows inside a Cargo.toml.
pub(crate) fn candidates(ident: &str) -> Vec<Candidate> {
    let exact = dash_form(ident);
    let curated = super::cargo_complete::crate_matches(&exact);
    let detail = curated
        .iter()
        .find(|(n, _)| *n == exact)
        .map(|(_, d)| (*d).to_string())
        .unwrap_or_else(|| "from crates.io".to_owned());
    let mut out = vec![Candidate {
        name: exact.clone(),
        detail,
    }];
    out.extend(
        curated
            .into_iter()
            .filter(|(n, _)| *n != exact)
            .map(|(name, detail)| Candidate {
                name: name.to_owned(),
                detail: detail.to_owned(),
            }),
    );
    out
}

/// Is `ident` a module of this project rather than a crate?
///
/// The first segment of a `use` resolves to either an external crate or an item
/// at the crate root, so a `mod <ident>;` anywhere in the sources means the path
/// is local and no dependency is missing. Without this, widening the diagnostic
/// list above would start offering "Add dependency: pins" for
/// `use pins::configs::Missing;`.
///
/// Scanning ALL sources rather than just the crate root over-approximates — a
/// module of a sibling library counts too — and that is the safe direction:
/// over-approximating means declining to offer the row, never adding a
/// dependency that should not be there.
pub(crate) fn is_local_module(sources: &[&str], ident: &str) -> bool {
    sources.iter().any(|src| {
        src.lines().any(|l| {
            let t = l.trim_start();
            // `pub mod x;`, `pub(crate) mod x;`, `mod x;` — and `mod x {` too.
            let t = t.strip_prefix("pub").map_or(t, |rest| {
                let rest = rest
                    .strip_prefix('(')
                    .map_or(rest, |r| r.split_once(')').map_or(r, |(_, a)| a));
                rest.trim_start()
            });
            t.strip_prefix("mod ").is_some_and(|rest| {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                name == ident
            })
        })
    })
}

// ── Wiring ───────────────────────────────────────────────────────────────────

impl AppIde {
    /// The crate the "Add dependency" row would offer, or `None` for no row.
    ///
    /// The test is STRUCTURAL, and deliberately does not look at the
    /// diagnostics. It did once, and that is exactly how it broke: a missing
    /// `embedded_hal_async` was reported as "cannot find module or crate"
    /// rather than "unresolved import", the row did not appear — and with it
    /// missing, rust-analyzer's lone "replace with `embedded_io_async`"
    /// typo-fix became the only action and applied ITSELF, silently rewriting
    /// the identifier. Matching messages is whack-a-mole with RA's wording, and
    /// losing is not a missing row, it is the editor changing code by itself.
    ///
    /// So ask the question directly instead. The first segment of a `use` can
    /// only resolve to an extern crate, a sysroot crate, `crate`/`self`/`super`,
    /// or an item of the current module — so if it is none of those, the import
    /// cannot resolve and the dependency really is missing. The three checks
    /// below are that list, and each one closes a real case: `use std::fmt`
    /// (sysroot), `use embedded_io_async::Write` when it is already declared
    /// (manifest), `use pins::configs::Missing` (a module of this crate).
    ///
    /// Erring toward NOT offering is the safe direction — a missing row costs a
    /// trip to Cargo.toml, a wrong one is only ever an offer nobody has to take.
    pub(super) fn add_dep_candidate(
        &self,
        display_code: &str,
        cursor_char_idx: usize,
    ) -> Option<String> {
        let (line0, _) =
            crate::editor::gui::text_pos::lsp_cursor_pos(display_code, cursor_char_idx);
        let line_text = display_code.lines().nth(line0 as usize)?;
        let ident = imported_crate(line_text)?;
        let manifest = self.manifest_for_current_file();
        let toml = self.manifest_text(&manifest)?;
        if has_dep(toml, &ident) {
            return None;
        }
        let mut sources: Vec<&str> = vec![self.generated_code.as_str()];
        sources.extend(
            self.project_tree
                .user_src_files
                .iter()
                .filter(|(p, _)| p.ends_with(".rs"))
                .map(|(_, c)| c.as_str()),
        );
        (!is_local_module(&sources, &ident)).then_some(ident)
    }

    /// Project-root-relative path of the Cargo.toml that owns the open file.
    /// A file inside a workspace member or a detached library belongs to that
    /// crate's manifest; everything else to the root one.
    pub(super) fn manifest_for_current_file(&self) -> String {
        let Some(rel) = crate::editor::gui::text_pos::selected_file_rel_path(
            &self.selected_file,
            &self.project_tree.user_src_files,
        ) else {
            return "Cargo.toml".to_owned();
        };
        let members = crate::panels::mcu_module::project_gen::workspace_members(&self.cargo_toml);
        let detached = crate::project_tree::extract_crate::detached_libs(
            &self.project_tree.user_src_files,
            &members,
        );
        for lib in members.iter().chain(detached.iter()) {
            let prefix = format!("{}/", lib.trim_end_matches('/'));
            if rel.starts_with(&prefix) {
                return format!("{prefix}Cargo.toml");
            }
        }
        "Cargo.toml".to_owned()
    }

    fn manifest_text(&self, path: &str) -> Option<&str> {
        if path == "Cargo.toml" {
            return Some(&self.cargo_toml);
        }
        self.project_tree
            .user_src_files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, c)| c.as_str())
    }

    /// Open the crate chooser for the identifier the code-action row offered.
    pub(super) fn open_add_dep_chooser(&mut self, ident: &str, pos: egui::Pos2) {
        self.ed.add_dep.items = candidates(ident);
        self.ed.add_dep.sel = 0;
        self.ed.add_dep.choice = None;
        self.ed.add_dep.note = None;
        self.ed.add_dep.fetch = None;
        self.ed.add_dep.manifest = Some(self.manifest_for_current_file());
        self.ed.add_dep.pos = pos;
        self.ed.add_dep.open = true;
    }

    /// Frame-top step: act on a chooser pick, then on a finished fetch.
    /// Called from `poll_code_actions`, so the manifest write lands at frame top
    /// like every other applied edit.
    pub(super) fn poll_add_dep(&mut self) {
        if let Some(i) = self.ed.add_dep.choice.take() {
            if let Some(c) = self.ed.add_dep.items.get(i).cloned() {
                let shared = Arc::new(Mutex::new(VersionFetch::Loading));
                self.ed.add_dep.fetch = Some((c.name.clone(), shared.clone()));
                self.ed.add_dep.note = Some(format!("Fetching {}…", c.name));
                let name = c.name;
                std::thread::spawn(move || {
                    let result = match super::cargo_complete::fetch_versions(&name) {
                        Ok(d) if d.versions.is_empty() => {
                            VersionFetch::Error("no published versions".into())
                        }
                        Ok(d) => VersionFetch::Done(d),
                        Err(e) => VersionFetch::Error(e),
                    };
                    *shared.lock().unwrap() = result;
                });
            }
        }

        let done = match &self.ed.add_dep.fetch {
            Some((name, shared)) => match &*shared.lock().unwrap() {
                VersionFetch::Loading => None,
                VersionFetch::Done(d) => match d.versions.first() {
                    Some(v) => Some(Ok((name.clone(), v.clone()))),
                    None => Some(Err(format!("{name}: no published versions"))),
                },
                VersionFetch::Error(e) => Some(Err(format!("{name}: {e}"))),
            },
            None => None,
        };
        let Some(done) = done else { return };
        self.ed.add_dep.fetch = None;
        match done {
            Ok((name, version)) => {
                let version = short_version(&version);
                self.write_dep_line(&name, &version);
                self.ed.add_dep.open = false;
                self.ed.add_dep.note = None;
            }
            Err(e) => self.ed.add_dep.note = Some(e),
        }
    }

    /// Splice the dependency into its manifest and ask for a Save.
    ///
    /// The Save is not a nicety: rust-analyzer only sees a manifest change when
    /// the project is written to disk, so without it the dependency is added and
    /// the red squiggle stays exactly where it was — which reads as "it did not
    /// work". The save also runs `cargo check` by itself when dependencies moved.
    fn write_dep_line(&mut self, name: &str, version: &str) {
        let Some(path) = self.ed.add_dep.manifest.clone() else {
            return;
        };
        let Some(old) = self.manifest_text(&path).map(str::to_owned) else {
            self.set_status_msg(format!("Cargo.toml not found: {path}"));
            return;
        };
        let new = insert_dep_line(&old, name, version);
        if new == old {
            self.set_status_msg(format!("{name} is already a dependency"));
            return;
        }
        if path == "Cargo.toml" {
            self.cargo_toml = new;
        } else if let Some(slot) = self
            .project_tree
            .user_src_files
            .iter_mut()
            .find(|(p, _)| *p == path)
        {
            slot.1 = new;
        }
        self.invalidate_project_files_cache();
        self.request_save = true;
        self.set_status_msg(format!("Added {name} = \"{version}\" to {path}"));
    }

    /// Draw the crate chooser. Keyboard nav is consumed BEFORE the editor
    /// renders (see `editor_panel/mod.rs`), same as the code-action popup; this
    /// only renders and handles clicks.
    pub(super) fn show_add_dep_popup(&mut self, ui: &mut egui::Ui) {
        if !self.ed.add_dep.open {
            return;
        }
        let sel = self.ed.add_dep.sel;
        let fetching = self.ed.add_dep.fetch.is_some();
        let mut chosen: Option<usize> = None;
        egui::Area::new(egui::Id::new("add_dep_popup"))
            .fixed_pos(self.ed.add_dep.pos)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.set_min_width(300.0);
                    ui.set_max_width(520.0);
                    ui.label(
                        egui::RichText::new("Add dependency")
                            .size(11.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                    if !fetching {
                        for (i, c) in self.ed.add_dep.items.iter().enumerate() {
                            let selected = i == sel;
                            let row = ui.horizontal(|ui| {
                                let r = ui.selectable_label(
                                    selected,
                                    egui::RichText::new(&c.name).size(12.0),
                                );
                                ui.label(
                                    egui::RichText::new(&c.detail)
                                        .size(10.0)
                                        .color(ui.visuals().weak_text_color()),
                                );
                                r
                            });
                            if selected {
                                row.response.scroll_to_me(None);
                            }
                            if row.inner.clicked() {
                                chosen = Some(i);
                            }
                        }
                    }
                    if let Some(note) = &self.ed.add_dep.note {
                        ui.label(
                            egui::RichText::new(note)
                                .size(11.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                    }
                });
            });
        if let Some(i) = chosen {
            self.ed.add_dep.choice = Some(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── imported_crate ───────────────────────────────────────────────────────

    fn imp(line: &str) -> Option<String> {
        imported_crate(line)
    }

    #[test]
    fn a_plain_use_names_its_first_segment() {
        assert_eq!(
            imp("use embedded_io_async::Write;").as_deref(),
            Some("embedded_io_async")
        );
        assert_eq!(imp("    use heapless::Vec;").as_deref(), Some("heapless"));
        assert_eq!(imp("use heapless;").as_deref(), Some("heapless"));
    }

    #[test]
    fn the_shapes_around_the_path_do_not_hide_it() {
        // The exact line from the report.
        assert_eq!(
            imp("use embedded_io_async::Write as _;").as_deref(),
            Some("embedded_io_async")
        );
        assert_eq!(
            imp("use embedded_io_async::{Read, Write};").as_deref(),
            Some("embedded_io_async")
        );
        assert_eq!(imp("pub use heapless::Vec;").as_deref(), Some("heapless"));
        assert_eq!(
            imp("pub(crate) use heapless::Vec;").as_deref(),
            Some("heapless")
        );
        assert_eq!(imp("use ::heapless::Vec;").as_deref(), Some("heapless"));
        assert_eq!(imp("extern crate heapless;").as_deref(), Some("heapless"));
    }

    /// The generated `pins/configs/*.rs` all END with a commented usage example
    /// — offering to add a dependency for one would be nonsense.
    #[test]
    fn a_commented_use_imports_nothing() {
        assert_eq!(imp("//     use embedded_io_async::Write;"), None);
        assert_eq!(imp("// use heapless::Vec;"), None);
    }

    #[test]
    fn the_current_crate_and_the_sysroot_are_not_dependencies() {
        for line in [
            "use crate::pins::configs;",
            "use self::inner::X;",
            "use super::sibling::Y;",
            "use std::fmt::Write;",
            "use core::mem;",
            "use alloc::vec::Vec;",
        ] {
            assert_eq!(imp(line), None, "{line}");
        }
    }

    #[test]
    fn a_line_that_is_not_an_import_is_ignored() {
        for line in [
            "let use_count = 3;",
            "fn main() {}",
            "    // nothing here",
            "use {a, b};",
            "",
        ] {
            assert_eq!(imp(line), None, "{line}");
        }
    }

    // ── is_local_module ──────────────────────────────────────────────────────

    /// `pins` is a module of this very crate, so `use pins::configs::Missing;`
    /// must not offer to add a crate called `pins` — the one false positive the
    /// structural test would otherwise let through.
    #[test]
    fn a_declared_module_is_not_a_missing_crate() {
        let src = "#![no_std]\npub mod pins;\nmod util;\n";
        assert!(is_local_module(&[src], "pins"));
        assert!(is_local_module(&[src], "util"));
        assert!(!is_local_module(&[src], "embedded_hal_async"));
    }

    #[test]
    fn every_visibility_of_a_mod_declaration_is_seen() {
        for line in [
            "mod x;",
            "pub mod x;",
            "pub(crate) mod x;",
            "    pub(super) mod x;",
            "mod x {",
        ] {
            assert!(is_local_module(&[line], "x"), "{line}");
        }
    }

    /// `mod` inside a longer word, and a module whose name merely starts the
    /// same, must not count.
    #[test]
    fn a_near_miss_is_not_a_module() {
        assert!(!is_local_module(&["mod pins_extra;"], "pins"));
        assert!(!is_local_module(&["let modem = 1;"], "modem"));
        assert!(!is_local_module(&["use pins::x;"], "pins"));
    }

    // ── has_dep ──────────────────────────────────────────────────────────────

    const MANIFEST: &str = "[package]\n\
        name = \"fw\"\n\
        \n\
        [dependencies]\n\
        cortex-m = \"0.7\"\n\
        embedded-hal-0-2 = { package = \"embedded-hal\", version = \"0.2.7\" }\n\
        \n\
        [dev-dependencies]\n\
        proptest = \"1\"\n\
        \n\
        [profile.release]\n\
        opt-level = \"z\"\n";

    #[test]
    fn a_declared_crate_is_found_under_its_rust_identifier() {
        assert!(has_dep(MANIFEST, "cortex_m"));
        assert!(has_dep(MANIFEST, "proptest"));
    }

    /// A renamed dependency is reached by its KEY in code, not by the crate it
    /// wraps — `embedded-hal-0-2 = { package = "embedded-hal" }` is
    /// `embedded_hal_0_2::…`, and `embedded_hal` is still missing.
    #[test]
    fn a_renamed_dependency_answers_for_its_key_only() {
        assert!(has_dep(MANIFEST, "embedded_hal_0_2"));
        assert!(!has_dep(MANIFEST, "embedded_hal"));
    }

    #[test]
    fn a_key_outside_a_dependency_table_does_not_count() {
        // `opt-level` lives in [profile.release], `name` in [package].
        assert!(!has_dep(MANIFEST, "opt_level"));
        assert!(!has_dep(MANIFEST, "name"));
    }

    #[test]
    fn a_missing_crate_is_missing() {
        assert!(!has_dep(MANIFEST, "embedded_io_async"));
    }

    #[test]
    fn a_target_specific_table_counts_too() {
        let m = "[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n";
        assert!(has_dep(m, "libc"));
    }

    /// Without the diagnostics as a second opinion, this IS the check that
    /// stops a duplicate line — so every TOML spelling of a dependency has to
    /// register, not just `name = "…"`.
    #[test]
    fn a_dotted_key_still_declares_its_crate() {
        let m = "[dependencies]\ndefmt.workspace = true\nheapless.version = \"0.8\"\n";
        assert!(has_dep(m, "defmt"));
        assert!(has_dep(m, "heapless"));
    }

    #[test]
    fn a_table_per_dependency_declares_it_too() {
        let m = "[dependencies.embedded-hal-async]\nversion = \"1.0\"\n";
        assert!(has_dep(m, "embedded_hal_async"));
        let t = "[target.'cfg(unix)'.dev-dependencies.libc]\nversion = \"0.2\"\n";
        assert!(has_dep(t, "libc"));
    }

    /// `[profile.release]` is not a dependency table, whatever it contains.
    #[test]
    fn a_section_that_merely_looks_dotted_is_not_a_dependency() {
        assert!(!has_dep(
            "[profile.release]\nopt-level = \"z\"\n",
            "release"
        ));
        assert!(!has_dep("[package.metadata]\nfoo = 1\n", "metadata"));
    }

    // ── short_version ────────────────────────────────────────────────────────

    #[test]
    fn the_requirement_stops_at_the_first_non_zero_segment() {
        assert_eq!(short_version("0.7.1"), "0.7");
        assert_eq!(short_version("1.2.3"), "1");
        assert_eq!(short_version("0.0.3"), "0.0.3");
        assert_eq!(short_version("2.0.0"), "2");
        assert_eq!(short_version("0.10.0"), "0.10");
    }

    /// Truncating `1.0.0-rc.1` to `1` would resolve to a different release.
    #[test]
    fn a_pre_release_is_written_whole() {
        assert_eq!(short_version("1.0.0-rc.1"), "1.0.0-rc.1");
        assert_eq!(short_version("0.1.0-alpha"), "0.1.0-alpha");
    }

    // ── insert_dep_line ──────────────────────────────────────────────────────

    #[test]
    fn the_line_goes_under_the_dependencies_header() {
        let out = insert_dep_line(MANIFEST, "embedded-io-async", "0.7");
        let deps = out.find("[dependencies]").expect("table");
        let added = out.find("embedded-io-async").expect("added");
        let dev = out.find("[dev-dependencies]").expect("dev table");
        assert!(deps < added && added < dev, "{out}");
        assert!(out.contains("embedded-io-async = \"0.7\""), "{out}");
    }

    /// `# <embedded-ide>` means "the IDE may take this away again on a feature
    /// toggle". A dependency the user asked for by name is theirs.
    #[test]
    fn the_added_line_is_not_marked_as_ide_owned() {
        let out = insert_dep_line(MANIFEST, "embedded-io-async", "0.7");
        let line = out
            .lines()
            .find(|l| l.starts_with("embedded-io-async"))
            .expect("the line");
        assert!(!line.contains("<embedded-ide>"), "{line}");
    }

    #[test]
    fn adding_twice_changes_nothing_the_second_time() {
        let once = insert_dep_line(MANIFEST, "embedded-io-async", "0.7");
        let twice = insert_dep_line(&once, "embedded-io-async", "0.9");
        assert_eq!(once, twice);
    }

    #[test]
    fn a_manifest_without_the_table_gets_one() {
        let m = "[package]\nname = \"fw\"\n";
        let out = insert_dep_line(m, "heapless", "0.8");
        assert!(out.contains("[dependencies]\nheapless = \"0.8\""), "{out}");
        assert!(out.starts_with("[package]"), "{out}");
    }

    #[test]
    fn the_trailing_newline_is_preserved_either_way() {
        assert!(insert_dep_line(MANIFEST, "heapless", "0.8").ends_with('\n'));
        let no_nl = MANIFEST.trim_end();
        assert!(!insert_dep_line(no_nl, "heapless", "0.8").ends_with('\n'));
    }

    // ── candidates ───────────────────────────────────────────────────────────

    #[test]
    fn the_literal_dash_form_leads_the_list() {
        let c = candidates("embedded_io_async");
        assert_eq!(c[0].name, "embedded-io-async");
        // …and it carries the curated description, not the generic one.
        assert!(c[0].detail.contains("Async"), "{}", c[0].detail);
    }

    #[test]
    fn the_list_never_repeats_the_leader() {
        let c = candidates("embedded_io_async");
        let n = c.iter().filter(|x| x.name == "embedded-io-async").count();
        assert_eq!(n, 1, "{:?}", c.iter().map(|x| &x.name).collect::<Vec<_>>());
    }

    /// A crate outside the curated set is still offered — the dash form is the
    /// only route to it, since there is no crates.io search here.
    #[test]
    fn an_unknown_crate_is_still_offered() {
        let c = candidates("some_private_thing");
        assert_eq!(c[0].name, "some-private-thing");
        assert_eq!(c[0].detail, "from crates.io");
    }

    #[test]
    fn related_curated_crates_follow_the_leader() {
        let c = candidates("embedded_io");
        assert_eq!(c[0].name, "embedded-io");
        assert!(
            c.iter().any(|x| x.name == "embedded-io-async"),
            "{:?}",
            c.iter().map(|x| &x.name).collect::<Vec<_>>()
        );
    }
}
