//! Cross-instance copy/paste for tree items — a file, a folder, or a whole
//! library crate, copied in one IDE window and pasted into another.
//!
//! # Why a staging directory rather than the OS clipboard
//!
//! egui can WRITE the clipboard (`Context::copy_text`) but has no read API at
//! all: the only way text ever comes back is `Event::Paste(String)`, which
//! egui-winit emits solely on the Ctrl+V keystroke. So a "Paste" menu entry
//! could not read the clipboard even if we put the payload there.
//!
//! On top of that, a library crate is hundreds of kilobytes. Putting that on
//! the system clipboard would replace whatever text the user was carrying with
//! an unreadable blob, on every single Copy.
//!
//! So the payload lives on disk under the app's config folder — which, unlike
//! [`crate::workspace::dir`], is deliberately NOT per-instance; being shared
//! between windows is the entire feature. The system clipboard gets only a
//! short [`TOKEN_PREFIX`] token, which is harmless if it lands in a text
//! editor, and which makes Ctrl+V work with the correct cross-window ordering.
//!
//! Because the payload is a self-contained directory, the window that copied
//! may be CLOSED by the time another one pastes.
//!
//! # Text only
//!
//! `user_src_files` holds `String` content: the tree model cannot represent a
//! binary file (`scan_src_dir` reads with `read_to_string().unwrap_or_default()`,
//! so a binary already becomes an empty string on load). A payload therefore
//! carries text only, and reports how many entries it had to skip.

use std::path::{Path, PathBuf};

/// Marker that identifies our token on the system clipboard. Anything pasted
/// into the tree that does not start with this is ordinary text and ignored.
pub const TOKEN_PREFIX: &str = "embedded-ide-clip:v1:";

/// How many payloads to keep in the staging directory. Pruned on every copy —
/// no background task, no daemon, and the newest is never at risk.
const KEEP_PAYLOADS: usize = 10;

/// What was copied. Decides what pasting it MEANS, not just where the bytes go.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum ClipKind {
    /// A single file.
    File,
    /// A folder and everything under it.
    Folder,
    /// A whole crate directory (its own `Cargo.toml`).
    Library,
}

impl ClipKind {
    /// Word used in the menu entry and in status messages.
    pub fn noun(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Folder => "folder",
            Self::Library => "library",
        }
    }
}

/// What a staged payload contains. Written as RON next to the files so a
/// payload directory is self-describing — you can open one in a file manager
/// and see what it is.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ClipManifest {
    /// Bumped if the on-disk shape ever changes; an unknown version is ignored
    /// rather than guessed at, so a newer IDE's payload can't confuse an older
    /// one.
    pub version: u32,
    pub kind: ClipKind,
    /// Base name of the copied item — `foo.rs`, `drivers`, `mw_radar`.
    pub name: String,
    /// Folder name of the project it came from, for the menu label. Purely
    /// informational: pasting never touches the source.
    pub source_project: String,
    /// Text files in the payload.
    pub file_count: usize,
    /// Entries left out because they were not text (see the module docs).
    pub skipped_binary: usize,
}

/// The current version of the on-disk payload layout.
const MANIFEST_VERSION: u32 = 1;

/// Pointer to the most recent payload, so the context-menu "Paste" entry has
/// something to read — it cannot consult the system clipboard.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ClipPointer {
    id: String,
}

/// "Copy this tree item" — raised by a context menu, applied by the app, which
/// is the only place that can read `user_src_files`.
#[derive(Clone, Debug)]
pub struct CopyRequest {
    pub kind: ClipKind,
    /// Project-root-relative path of the item to copy.
    pub path: String,
}

/// "Paste here" — raised by a context menu or by Ctrl+V.
#[derive(Clone, Debug)]
pub struct PasteRequest {
    /// Project-root-relative folder to paste into; `""` is the project root,
    /// which is where a library crate has to land.
    pub target_dir: String,
    /// Payload to paste. `None` means "whatever was staged last" — the menu
    /// entry's case, since a menu cannot read the system clipboard.
    pub id: Option<String>,
}

/// A staged payload, ready to paste.
#[derive(Clone, Debug)]
pub struct ClipPayload {
    pub id: String,
    pub manifest: ClipManifest,
    /// `(path relative to the item's own root, content)`. For a file payload
    /// the single entry's path IS the file name.
    pub files: Vec<(String, String)>,
}

/// `<config>/clipboard` — shared by every IDE window on this machine.
///
/// `None` when no config dir resolves (rare); every caller then degrades to
/// "copy/paste unavailable" rather than falling back to a per-instance path,
/// which would silently stop working across windows — the one thing this
/// feature exists to do.
pub fn staging_root() -> Option<PathBuf> {
    crate::panels::mcu_module::registry::user_config_dir().map(|d| d.join("clipboard"))
}

/// Token to put on the system clipboard for a payload id.
pub fn token_for(id: &str) -> String {
    format!("{TOKEN_PREFIX}{id}")
}

/// The payload id inside a pasted string, or `None` if this is ordinary text.
///
/// Trimmed first: a clipboard round-trip through some apps adds a trailing
/// newline, and egui-winit already rewrites CRLF to LF on the way in.
pub fn id_from_token(pasted: &str) -> Option<&str> {
    let t = pasted.trim();
    let id = t.strip_prefix(TOKEN_PREFIX)?;
    // A token is one line; anything after a newline means this is a longer
    // document that merely starts with the marker.
    (!id.is_empty() && !id.contains('\n')).then_some(id)
}

/// Stage `files` as a new payload and return its id.
///
/// `files` is `(path relative to the item's root, content)` — exactly what
/// [`ClipPayload::files`] gives back. `name` is the item's base name.
pub fn stage(
    kind: ClipKind,
    name: &str,
    source_project: &str,
    files: &[(String, String)],
    skipped_binary: usize,
) -> Result<String, String> {
    let root = staging_root().ok_or("No config folder — copy/paste is unavailable here.")?;
    let id = new_id();
    let dir = root.join(&id);
    std::fs::create_dir_all(dir.join("files")).map_err(|e| format!("{e}"))?;

    for (rel, content) in files {
        let full = dir.join("files").join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{e}"))?;
        }
        std::fs::write(&full, content).map_err(|e| format!("{e}"))?;
    }

    let manifest = ClipManifest {
        version: MANIFEST_VERSION,
        kind,
        name: name.to_owned(),
        source_project: source_project.to_owned(),
        file_count: files.len(),
        skipped_binary,
    };
    let text = ron::ser::to_string_pretty(&manifest, ron::ser::PrettyConfig::default())
        .map_err(|e| format!("{e}"))?;
    std::fs::write(dir.join("manifest.ron"), text).map_err(|e| format!("{e}"))?;

    // Only now does the payload become visible to other windows: the pointer
    // is written LAST, so a reader can never find an id whose files are still
    // being written.
    write_pointer(&root, &id)?;
    prune(&root, KEEP_PAYLOADS);
    Ok(id)
}

/// Just the manifest of a payload — enough to LABEL a menu entry.
///
/// Separate from [`load`] on purpose: a context menu re-renders every frame
/// while it is open, and `load` reads every file in the payload. Naming a
/// pasted library must not mean re-reading its whole source sixty times a
/// second.
pub fn load_manifest(id: &str) -> Option<ClipManifest> {
    let path = staging_root()?.join(id).join("manifest.ron");
    let manifest: ClipManifest = ron::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    // An unknown version is ignored rather than guessed at, so a newer IDE's
    // payload can't confuse an older one.
    (manifest.version == MANIFEST_VERSION).then_some(manifest)
}

/// Load a payload by id, or `None` if it is gone / unreadable / a version this
/// build does not know.
pub fn load(id: &str) -> Option<ClipPayload> {
    let manifest = load_manifest(id)?;
    let files_root = staging_root()?.join(id).join("files");
    let mut files = Vec::new();
    collect(&files_root, &files_root, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Some(ClipPayload {
        id: id.to_owned(),
        manifest,
        files,
    })
}

/// Id of the most recently staged payload.
pub fn latest_id() -> Option<String> {
    let root = staging_root()?;
    let pointer: ClipPointer =
        ron::from_str(&std::fs::read_to_string(root.join("latest.ron")).ok()?).ok()?;
    Some(pointer.id)
}

/// The most recently staged payload — what the context-menu "Paste" entry
/// offers, since it cannot read the system clipboard.
pub fn latest() -> Option<ClipPayload> {
    load(&latest_id()?)
}

/// Paste `payload` into `target_dir` (project-root-relative, `""` for the
/// project root), returning the files to add as `(root-relative path, content)`.
///
/// `exists` answers whether a root-relative path is already taken — the caller
/// answers from `user_src_files` plus `user_src_folders`. The whole item is
/// renamed as ONE unit when its base name collides, so a pasted folder keeps
/// its internal structure intact instead of having individual files renamed
/// out from under their `mod` declarations.
pub fn paste_paths(
    payload: &ClipPayload,
    target_dir: &str,
    exists: impl Fn(&str) -> bool,
) -> Vec<(String, String)> {
    let base = free_name(&payload.manifest.name, |cand| {
        exists(&join(target_dir, cand))
    });
    payload
        .files
        .iter()
        .map(|(rel, content)| {
            // Every payload path starts with the item's own name; swapping that
            // first segment is what applies the rename to the whole tree.
            let rest = rel
                .strip_prefix(&payload.manifest.name)
                .map(|r| r.trim_start_matches('/'))
                .unwrap_or(rel.as_str());
            let renamed = if rest.is_empty() {
                base.clone()
            } else {
                format!("{base}/{rest}")
            };
            (join(target_dir, &renamed), content.clone())
        })
        .collect()
}

/// `dir/name`, or just `name` at the project root.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{}/{name}", dir.trim_end_matches('/'))
    }
}

/// `name` if free, else the first `<stem>_<n>` that is — the same `_1`, `_2`
/// convention the tree's Duplicate already uses, so a paste and a duplicate
/// produce names that look alike. Works for both `foo.rs` and a bare folder.
pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    // Split on the LAST dot; a leading dot is part of the name (".gitignore").
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|n| format!("{stem}_{n}{ext}"))
        .find(|cand| !taken(cand))
        .expect("an unbounded counter always reaches a free name")
}

/// Recursively read `dir` into `(path relative to `root`, content)` pairs.
/// Non-text files are skipped — the tree model has nowhere to put them.
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            // LF-normalized like every other in-memory buffer (see the
            // phantom-gutter rule in `logic::scan_src_dir`).
            if let Ok(content) = std::fs::read_to_string(&path) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                out.push((rel, content.replace("\r\n", "\n")));
            }
        }
    }
}

/// `<millis>-<pid>`: unique across windows without a uuid dependency, and it
/// sorts chronologically, which is what [`prune`] relies on.
fn new_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{millis:013}-{}", std::process::id())
}

fn write_pointer(root: &Path, id: &str) -> Result<(), String> {
    let text =
        ron::ser::to_string(&ClipPointer { id: id.to_owned() }).map_err(|e| format!("{e}"))?;
    // Write-then-rename: another window polling `latest.ron` must never read a
    // half-written file.
    let tmp = root.join("latest.ron.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{e}"))?;
    std::fs::rename(&tmp, root.join("latest.ron")).map_err(|e| format!("{e}"))
}

/// Keep the `keep` newest payload directories, delete the rest. Ids sort
/// chronologically, so this is a plain sort — no timestamps to read back.
fn prune(root: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    if dirs.len() <= keep {
        return;
    }
    dirs.sort();
    let doomed = dirs.len() - keep;
    for dir in dirs.into_iter().take(doomed) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(name: &str, kind: ClipKind, files: &[(&str, &str)]) -> ClipPayload {
        ClipPayload {
            id: "test".into(),
            manifest: ClipManifest {
                version: MANIFEST_VERSION,
                kind,
                name: name.into(),
                source_project: "other".into(),
                file_count: files.len(),
                skipped_binary: 0,
            },
            files: files
                .iter()
                .map(|(p, c)| ((*p).to_owned(), (*c).to_owned()))
                .collect(),
        }
    }

    /// Ordinary text must never be mistaken for a payload — Ctrl+V in the tree
    /// with a normal clipboard has to be a no-op, not a paste.
    #[test]
    fn only_our_own_token_is_recognised() {
        assert_eq!(id_from_token(&token_for("123-4")), Some("123-4"));
        assert_eq!(id_from_token("  \n embedded-ide-clip:v1:9 \n "), Some("9"));
        assert_eq!(id_from_token("fn main() {}"), None);
        assert_eq!(id_from_token(TOKEN_PREFIX), None, "empty id is not a token");
        // A document that merely opens with the marker is not a token.
        assert_eq!(id_from_token("embedded-ide-clip:v1:9\nmore text"), None);
    }

    /// A folder pasted where its name is free keeps every internal path.
    #[test]
    fn a_folder_lands_under_the_target_with_its_structure() {
        let p = payload(
            "drivers",
            ClipKind::Folder,
            &[("drivers/mod.rs", "a"), ("drivers/spi/bus.rs", "b")],
        );
        let out = paste_paths(&p, "src", |_| false);
        assert_eq!(
            out.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            ["src/drivers/mod.rs", "src/drivers/spi/bus.rs"]
        );
    }

    /// The rename applies to the item as ONE unit: every path moves under the
    /// new base, so `mod` declarations inside still resolve to their siblings.
    #[test]
    fn a_name_clash_renames_the_whole_item_not_each_file() {
        let p = payload(
            "drivers",
            ClipKind::Folder,
            &[("drivers/mod.rs", "a"), ("drivers/spi/bus.rs", "b")],
        );
        let out = paste_paths(&p, "src", |path| path == "src/drivers");
        assert_eq!(
            out.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            ["src/drivers_1/mod.rs", "src/drivers_1/spi/bus.rs"]
        );
    }

    /// A single file keeps its extension when renamed around a clash.
    #[test]
    fn a_file_clash_keeps_the_extension() {
        let p = payload("bus.rs", ClipKind::File, &[("bus.rs", "x")]);
        let out = paste_paths(&p, "src/drivers", |path| path == "src/drivers/bus.rs");
        assert_eq!(
            out,
            vec![("src/drivers/bus_1.rs".to_owned(), "x".to_owned())]
        );
    }

    /// A library pastes at the PROJECT ROOT, next to src/ — that is where a
    /// crate directory has to live to be a workspace member.
    #[test]
    fn a_library_lands_at_the_project_root() {
        let p = payload(
            "mw_radar",
            ClipKind::Library,
            &[
                ("mw_radar/Cargo.toml", "[package]"),
                ("mw_radar/src/lib.rs", ""),
            ],
        );
        let out = paste_paths(&p, "", |_| false);
        assert_eq!(
            out.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            ["mw_radar/Cargo.toml", "mw_radar/src/lib.rs"]
        );
    }

    /// Counting keeps going on the same base rather than growing suffixes.
    #[test]
    fn free_name_walks_past_every_taken_candidate() {
        let taken = ["a.rs", "a_1.rs", "a_2.rs"];
        assert_eq!(
            free_name("a.rs", |c| taken.contains(&c)),
            "a_3.rs".to_owned()
        );
        assert_eq!(free_name("a.rs", |_| false), "a.rs".to_owned());
        // A dotfile's leading dot is part of the name, not an extension.
        assert_eq!(
            free_name(".gitignore", |c| c == ".gitignore"),
            ".gitignore_1".to_owned()
        );
    }

    /// Round-trip through the real staging directory: stage, then read back
    /// through the pointer the way another window would.
    #[test]
    fn a_staged_payload_reads_back_through_the_pointer() {
        let Some(root) = staging_root() else {
            return; // no config dir in this environment — the feature opts out
        };
        let files = vec![
            ("t_lib/Cargo.toml".to_owned(), "[package]".to_owned()),
            ("t_lib/src/lib.rs".to_owned(), "pub fn f() {}".to_owned()),
        ];
        let id = stage(ClipKind::Library, "t_lib", "proj", &files, 2).expect("staging works");

        let back = load(&id).expect("the payload reads back");
        assert_eq!(back.manifest.kind, ClipKind::Library);
        assert_eq!(back.manifest.name, "t_lib");
        assert_eq!(back.manifest.skipped_binary, 2);
        assert_eq!(back.files, files, "contents survive the round trip");

        // `latest` is what the menu entry reads — it must point at this copy.
        assert_eq!(latest().map(|p| p.id), Some(id.clone()));

        let _ = std::fs::remove_dir_all(root.join(&id));
    }

    /// The staging directory must not grow without bound.
    #[test]
    fn prune_keeps_only_the_newest() {
        let base = std::env::temp_dir().join(format!("eide_clip_prune_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for name in ["0000000000001-1", "0000000000002-1", "0000000000003-1"] {
            std::fs::create_dir_all(base.join(name)).unwrap();
        }
        std::fs::write(base.join("latest.ron"), "x").unwrap();

        prune(&base, 2);

        assert!(!base.join("0000000000001-1").exists(), "oldest pruned");
        assert!(base.join("0000000000003-1").exists(), "newest kept");
        assert!(
            base.join("latest.ron").exists(),
            "the pointer is not a payload"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
