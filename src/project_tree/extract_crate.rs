//! "Extract to library crate" — turn a folder of `src/` into a sibling Cargo
//! crate that can be published to crates.io.
//!
//! Everything here is pure: [`plan_extract`] takes the current file set and
//! returns an [`ExtractPlan`] describing every write, removal and rewrite. The
//! caller performs the I/O. That split is what makes the tricky parts (module
//! tree, reference rewriting, manifest patching) testable.
//!
//! Layout produced for a folder `mw_radar`:
//!
//! ```text
//! <root>/mw_radar/Cargo.toml        — publishable manifest
//! <root>/mw_radar/src/lib.rs        — from mw_radar/mod.rs, or generated
//! <root>/mw_radar/src/<rest>.rs     — the moved files
//! ```
//!
//! The root manifest gains `[workspace] members` + `[dependencies.<name>]`, and
//! the remaining sources get `crate::<folder>::` rewritten to `<crate>::`.

/// Publishable metadata for the new crate, collected in the dialog.
#[derive(Clone, Debug)]
pub struct CrateMeta {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub license: String,
    pub description: String,
    /// `#![no_std]` at the top of `lib.rs` — the default for embedded drivers.
    pub no_std: bool,
}

impl Default for CrateMeta {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: "0.1.0".to_owned(),
            edition: "2021".to_owned(),
            license: "MIT OR Apache-2.0".to_owned(),
            description: String::new(),
            no_std: true,
        }
    }
}

/// Everything the caller must apply, computed without touching the filesystem.
#[derive(Clone, Debug, Default)]
pub struct ExtractPlan {
    /// Directory of the new crate, relative to the project root.
    pub crate_dir: String,
    /// Files to create, as paths relative to the PROJECT ROOT.
    pub new_files: Vec<(String, String)>,
    /// Paths (relative to `src/`) to drop from `user_src_files`.
    pub removed: Vec<String>,
    /// Updated content for files that kept referring to the moved module, as
    /// `(path relative to src/, new content)`.
    pub rewritten: Vec<(String, String)>,
    /// New `main.rs`, when it referenced the moved module.
    pub rewritten_main: Option<String>,
    /// Root `Cargo.toml` with the workspace + path dependency added.
    pub root_cargo_toml: String,
    /// Things the user has to fix by hand — shown before confirming.
    pub warnings: Vec<String>,
}

/// The Rust identifier a crate is reachable under: crates.io allows `-`, `use`
/// does not.
pub fn crate_ident(name: &str) -> String {
    name.replace('-', "_")
}

/// Build the plan for extracting `folder` (a path relative to `src/`, no
/// trailing slash) into a crate described by `meta`.
///
/// `Err` when the request cannot work at all — an empty folder, a bad name, or
/// a directory that already exists in the manifest.
pub fn plan_extract(
    folder: &str,
    user_files: &[(String, String)],
    main_rs: &str,
    root_cargo_toml: &str,
    meta: &CrateMeta,
) -> Result<ExtractPlan, String> {
    let name = meta.name.trim();
    if name.is_empty() {
        return Err("Crate name is required.".to_owned());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Crate name may only contain letters, digits, `-` and `_`.".to_owned());
    }
    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Err("Crate name cannot start with a digit.".to_owned());
    }

    let prefix = format!("{folder}/");
    let moved: Vec<&(String, String)> = user_files
        .iter()
        .filter(|(p, _)| p.starts_with(&prefix))
        .collect();
    if moved.is_empty() {
        return Err(format!("`{folder}/` has no files to extract."));
    }

    let crate_dir = name.to_owned();
    let ident = crate_ident(name);
    // The module path the folder had inside the app crate.
    let folder_ident = folder.rsplit('/').next().unwrap_or(folder).replace('-', "_");

    let mut plan = ExtractPlan {
        crate_dir: crate_dir.clone(),
        ..Default::default()
    };

    // ── Move the files ───────────────────────────────────────────────────────
    // `<folder>/mod.rs` becomes the crate root; everything else keeps its
    // relative position under the new `src/`.
    let mut has_mod_rs = false;
    let mut top_level_mods: Vec<String> = Vec::new();
    for (path, content) in &moved {
        let rel = &path[prefix.len()..];
        let dest_rel = if rel == "mod.rs" {
            has_mod_rs = true;
            "lib.rs".to_owned()
        } else {
            rel.to_owned()
        };
        if let Some(m) = top_level_module_of(rel) {
            if !top_level_mods.contains(&m) {
                top_level_mods.push(m);
            }
        }
        plan.new_files
            .push((format!("{crate_dir}/src/{dest_rel}"), (*content).clone()));
        plan.removed.push((*path).clone());

        // References the moved code cannot keep: they pointed at the app crate.
        for (line_no, line) in content.lines().enumerate() {
            if line.contains("crate::") && !line.trim_start().starts_with("//") {
                plan.warnings.push(format!(
                    "{path}:{} refers to `crate::…` — inside the library that now \
                     means the library itself, not the firmware. Pass what it \
                     needs in as a parameter (an `embedded-hal` trait) instead.",
                    line_no + 1
                ));
            }
        }
    }
    if has_mod_rs {
        plan.warnings.push(format!(
            "`{folder}/mod.rs` became `{crate_dir}/src/lib.rs`. Any `super::…` \
             in it pointed at the firmware crate and no longer resolves."
        ));
    } else {
        // No mod.rs: synthesize a crate root that re-exports the modules.
        top_level_mods.sort();
        let mut lib = String::new();
        if meta.no_std {
            lib.push_str("#![no_std]\n\n");
        }
        lib.push_str(&format!(
            "//! `{name}` — extracted from the firmware project.\n\n"
        ));
        for m in &top_level_mods {
            lib.push_str(&format!("pub mod {m};\n"));
        }
        plan.new_files
            .push((format!("{crate_dir}/src/lib.rs"), lib));
    }

    plan.new_files.push((
        format!("{crate_dir}/Cargo.toml"),
        member_cargo_toml(meta),
    ));

    // ── Rewrite what stays behind ────────────────────────────────────────────
    // `crate::mw_radar::Foo` → `mw_radar::Foo`, and the `mod mw_radar;`
    // declaration disappears with the folder.
    for (path, content) in user_files {
        if path.starts_with(&prefix) {
            continue;
        }
        let new = rewrite_refs(content, &folder_ident, &ident);
        if new != *content {
            plan.rewritten.push((path.clone(), new));
        }
    }
    let new_main = rewrite_refs(main_rs, &folder_ident, &ident);
    if new_main != main_rs {
        plan.rewritten_main = Some(new_main);
    }

    // ── Patch the root manifest ──────────────────────────────────────────────
    plan.root_cargo_toml = patch_root_manifest(root_cargo_toml, &crate_dir, name);

    if meta.description.trim().is_empty() {
        plan.warnings.push(
            "No description — crates.io rejects a publish without one. You can \
             fill it in later in the new Cargo.toml."
                .to_owned(),
        );
    }
    Ok(plan)
}

/// The top-level module name a path inside the folder contributes: `foo.rs` →
/// `foo`, `bar/baz.rs` → `bar`. `mod.rs` contributes nothing.
fn top_level_module_of(rel: &str) -> Option<String> {
    match rel.split_once('/') {
        Some((dir, _)) => Some(dir.replace('-', "_")),
        None => {
            let stem = rel.strip_suffix(".rs")?;
            if stem == "mod" {
                None
            } else {
                Some(stem.replace('-', "_"))
            }
        }
    }
}

/// Replace `crate::<folder>` with `<crate_ident>` and drop the now-dangling
/// `mod <folder>;` declaration.
fn rewrite_refs(src: &str, folder_ident: &str, crate_ident: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let t = line.trim();
        // `mod mw_radar;` / `pub mod mw_radar;` — the module is gone.
        if t == format!("mod {folder_ident};") || t == format!("pub mod {folder_ident};") {
            continue;
        }
        out.push_str(&line.replace(
            &format!("crate::{folder_ident}"),
            crate_ident,
        ));
        out.push('\n');
    }
    // Keep the original's trailing-newline shape.
    if !src.ends_with('\n') {
        out.pop();
    }
    out
}

/// A publishable manifest for the new crate.
fn member_cargo_toml(meta: &CrateMeta) -> String {
    format!(
        "[package]\n\
         name        = \"{name}\"\n\
         version     = \"{version}\"\n\
         edition     = \"{edition}\"\n\
         license     = \"{license}\"\n\
         description = \"{desc}\"\n\
         # Fill these in before `cargo publish`:\n\
         # repository  = \"https://github.com/…\"\n\
         # readme      = \"README.md\"\n\
         # keywords    = [\"embedded\", \"no-std\"]\n\
         # categories  = [\"embedded\", \"no-std\"]\n\
         \n\
         [dependencies]\n\
         # Depend on TRAITS, not on a concrete HAL, so this crate stays usable\n\
         # on any chip (and is worth publishing):\n\
         # embedded-hal = \"1.0\"\n",
        name = meta.name.trim(),
        version = meta.version.trim(),
        edition = meta.edition.trim(),
        license = meta.license.trim(),
        desc = meta.description.trim().replace('"', "'"),
    )
}

/// Add `[workspace] members` and the path dependency to the root manifest.
///
/// Both go in the user tail, AFTER the generated block, so a chip change won't
/// wipe them. The dependency has to be written as `[dependencies.<name>]`: the
/// generated block already opened a `[dependencies]` table, and a second one is
/// a TOML redefinition error that cargo rejects outright. Idempotent — running
/// the extraction twice must not duplicate either section.
fn patch_root_manifest(existing: &str, crate_dir: &str, name: &str) -> String {
    let mut out = existing.trim_end().to_owned();

    let members = crate::panels::mcu_module::project_gen::workspace_members(existing);
    if !members.iter().any(|m| m == crate_dir) {
        if members.is_empty() {
            out.push_str(&format!(
                "\n\n[workspace]\nmembers = [\"{crate_dir}\"]\n"
            ));
        } else {
            // A members list exists — extend it in place.
            out = extend_members_list(&out, crate_dir);
        }
    }
    if !out.contains(&format!("[dependencies.{name}]")) {
        out.push_str(&format!(
            "\n[dependencies.{name}]\npath = \"{crate_dir}\"\n"
        ));
    }
    out
}

/// Insert `crate_dir` into an existing `members = [...]` array.
fn extend_members_list(src: &str, crate_dir: &str) -> String {
    let mut out = String::with_capacity(src.len() + crate_dir.len() + 8);
    let mut done = false;
    for line in src.lines() {
        if !done {
            if let Some(close) = line.rfind(']') {
                if line.trim_start().starts_with("members") || line.trim() == "]" {
                    let (head, tail) = line.split_at(close);
                    let sep = if head.trim_end().ends_with('[') { "" } else { ", " };
                    out.push_str(&format!("{head}{sep}\"{crate_dir}\"{tail}\n"));
                    done = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> CrateMeta {
        CrateMeta {
            name: "mw_radar".to_owned(),
            description: "USART radar driver".to_owned(),
            ..Default::default()
        }
    }

    fn files() -> Vec<(String, String)> {
        vec![
            (
                "mw_radar/frame.rs".to_owned(),
                "pub struct Frame;\n".to_owned(),
            ),
            (
                "mw_radar/parser.rs".to_owned(),
                "pub fn parse() {}\n".to_owned(),
            ),
            (
                "app.rs".to_owned(),
                "mod mw_radar;\nuse crate::mw_radar::frame::Frame;\n".to_owned(),
            ),
        ]
    }

    #[test]
    fn moves_files_under_the_new_crate_src() {
        let p = plan_extract("mw_radar", &files(), "", "", &meta()).unwrap();
        let paths: Vec<&str> = p.new_files.iter().map(|(a, _)| a.as_str()).collect();
        assert!(paths.contains(&"mw_radar/src/frame.rs"));
        assert!(paths.contains(&"mw_radar/src/parser.rs"));
        assert!(paths.contains(&"mw_radar/Cargo.toml"));
        assert_eq!(p.removed.len(), 2, "only the folder's files are removed");
    }

    /// Without a `mod.rs` we must synthesize a crate root, or nothing compiles.
    #[test]
    fn generates_lib_rs_declaring_every_module() {
        let p = plan_extract("mw_radar", &files(), "", "", &meta()).unwrap();
        let lib = p
            .new_files
            .iter()
            .find(|(a, _)| a == "mw_radar/src/lib.rs")
            .map(|(_, c)| c.clone())
            .expect("lib.rs generated");
        assert!(lib.contains("#![no_std]"), "no_std requested:\n{lib}");
        assert!(lib.contains("pub mod frame;"), "{lib}");
        assert!(lib.contains("pub mod parser;"), "{lib}");
    }

    /// An existing `mod.rs` IS the crate root — don't generate a second one.
    #[test]
    fn existing_mod_rs_becomes_lib_rs() {
        let mut f = files();
        f.push((
            "mw_radar/mod.rs".to_owned(),
            "pub mod frame;\npub mod parser;\n".to_owned(),
        ));
        let p = plan_extract("mw_radar", &f, "", "", &meta()).unwrap();
        let libs: Vec<_> = p
            .new_files
            .iter()
            .filter(|(a, _)| a == "mw_radar/src/lib.rs")
            .collect();
        assert_eq!(libs.len(), 1, "exactly one crate root");
        assert_eq!(libs[0].1, "pub mod frame;\npub mod parser;\n");
        assert!(
            p.warnings.iter().any(|w| w.contains("super::")),
            "must warn that super:: breaks: {:?}",
            p.warnings
        );
    }

    #[test]
    fn rewrites_references_and_drops_the_mod_declaration() {
        let p = plan_extract("mw_radar", &files(), "", "", &meta()).unwrap();
        let (_, app) = p
            .rewritten
            .iter()
            .find(|(a, _)| a == "app.rs")
            .expect("app.rs rewritten");
        assert_eq!(app, "use mw_radar::frame::Frame;\n");
    }

    #[test]
    fn rewrites_main_rs_too() {
        let main = "mod mw_radar;\nfn main() { crate::mw_radar::parser::parse(); }\n";
        let p = plan_extract("mw_radar", &files(), main, "", &meta()).unwrap();
        assert_eq!(
            p.rewritten_main.unwrap(),
            "fn main() { mw_radar::parser::parse(); }\n"
        );
    }

    /// Code moving into the library that still says `crate::` is the one thing
    /// the user MUST fix by hand — it has to be reported, not silently moved.
    #[test]
    fn warns_about_crate_references_inside_the_moved_code() {
        let f = vec![(
            "mw_radar/frame.rs".to_owned(),
            "use crate::pins::configs::usart1;\n".to_owned(),
        )];
        let p = plan_extract("mw_radar", &f, "", "", &meta()).unwrap();
        assert!(
            p.warnings.iter().any(|w| w.contains("frame.rs:1")),
            "{:?}",
            p.warnings
        );
    }

    /// A second `[dependencies]` table would be a TOML redefinition error, so
    /// the dependency must be added as `[dependencies.<name>]`.
    #[test]
    fn root_manifest_uses_a_dependency_subtable() {
        let root = "[package]\nname = \"p\"\n\n[dependencies]\ncortex-m = \"0.7\"\n";
        let p = plan_extract("mw_radar", &files(), "", root, &meta()).unwrap();
        assert!(p.root_cargo_toml.contains("[dependencies.mw_radar]"));
        assert!(p.root_cargo_toml.contains("path = \"mw_radar\""));
        assert_eq!(
            p.root_cargo_toml.matches("[dependencies]").count(),
            1,
            "must not open a second [dependencies] table:\n{}",
            p.root_cargo_toml
        );
        assert!(p.root_cargo_toml.contains("[workspace]"));
    }

    /// Running it twice must not duplicate the workspace or the dependency.
    #[test]
    fn patching_the_root_manifest_is_idempotent() {
        let root = "[package]\nname = \"p\"\n";
        let once = plan_extract("mw_radar", &files(), "", root, &meta())
            .unwrap()
            .root_cargo_toml;
        let twice = plan_extract("mw_radar", &files(), "", &once, &meta())
            .unwrap()
            .root_cargo_toml;
        assert_eq!(once.trim_end(), twice.trim_end());
    }

    #[test]
    fn extends_an_existing_members_list() {
        let root = "[workspace]\nmembers = [\"other\"]\n";
        let p = plan_extract("mw_radar", &files(), "", root, &meta()).unwrap();
        assert!(
            p.root_cargo_toml.contains("members = [\"other\", \"mw_radar\"]"),
            "{}",
            p.root_cargo_toml
        );
        assert_eq!(p.root_cargo_toml.matches("[workspace]").count(), 1);
    }

    #[test]
    fn rejects_bad_names_and_empty_folders() {
        let bad = CrateMeta {
            name: "mw radar".to_owned(),
            ..meta()
        };
        assert!(plan_extract("mw_radar", &files(), "", "", &bad).is_err());
        assert!(plan_extract("nope", &files(), "", "", &meta()).is_err());
    }

    /// crates.io names may contain `-`; `use` paths may not.
    #[test]
    fn hyphenated_crate_names_map_to_underscore_paths() {
        let m = CrateMeta {
            name: "mw-radar".to_owned(),
            ..meta()
        };
        let p = plan_extract("mw_radar", &files(), "", "", &m).unwrap();
        let (_, app) = p.rewritten.iter().find(|(a, _)| a == "app.rs").unwrap();
        assert_eq!(app, "use mw_radar::frame::Frame;\n");
        assert!(p.new_files.iter().any(|(a, _)| a == "mw-radar/Cargo.toml"));
    }
}
