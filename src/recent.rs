//! The list of recently opened projects.
//!
//! Deliberately GLOBAL — `<config>/recent.ron`, not the eframe app state. That
//! state is per instance slot (see [`crate::workspace`]), so a per-slot history
//! would give the second window a different list than the first: the same
//! split that makes "open a second window" reopen a project you didn't ask for.
//! One list, shared by every window, is the whole point.
//!
//! The chip id travels with each entry so a picker can say STM32F103 or ESP32-C3
//! next to the name — which is what tells two projects apart at a glance.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How many projects to remember. Long enough to cover a work week, short
/// enough that the list stays scannable.
const MAX_ENTRIES: usize = 15;

/// File name under [`crate::panels::mcu_module::registry::user_config_dir`].
const FILE_NAME: &str = "recent.ron";

/// One remembered project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentProject {
    /// Absolute path of the project root.
    pub path: String,
    /// Folder leaf at the time it was opened — shown as the name.
    pub name: String,
    /// Chip id (`stm32f103`, `esp32c3`), when one was detected. Older entries
    /// and projects whose chip couldn't be identified have `None`.
    #[serde(default)]
    pub mcu_id: Option<String>,
    /// Unix seconds of the last open. Sort key, and what a picker shows.
    #[serde(default)]
    pub opened_at: u64,
}

/// Read the list, newest first, dropping entries whose folder is gone.
///
/// Never fails: a missing, unreadable or corrupt file is an empty history, not
/// an error to put in front of the user.
pub fn load() -> Vec<RecentProject> {
    let Some(path) = file_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let list: Vec<RecentProject> = ron::from_str(&text).unwrap_or_default();
    prune_missing(list, |p| Path::new(p).is_dir())
}

/// Record `dir` as the most recently opened project and persist the list.
///
/// Read-modify-write: two instances opening projects at the same moment can
/// lose one entry, which is the right trade for a history — a lock would make
/// every project open wait on another window.
pub fn record(dir: &Path, mcu_id: Option<&str>) {
    let Some(path) = file_path() else {
        return;
    };
    let list = promote(load(), dir, mcu_id, now_secs());
    let Ok(text) = ron::ser::to_string_pretty(&list, ron::ser::PrettyConfig::default()) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Via a temp file + rename: a crash mid-write would otherwise truncate the
    // history into unparseable RON.
    let tmp = path.with_extension("ron.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Move `dir` to the front of `list` (adding it if new), refresh its metadata,
/// and cap the length. Pure — the whole ordering contract lives here.
///
/// Matching is by path, case-insensitively on Windows, so reopening the same
/// folder spelled differently updates the entry instead of duplicating it.
fn promote(
    mut list: Vec<RecentProject>,
    dir: &Path,
    mcu_id: Option<&str>,
    opened_at: u64,
) -> Vec<RecentProject> {
    let path = normalize(dir);
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    list.retain(|e| !same_path(&e.path, &path));
    list.insert(
        0,
        RecentProject {
            path,
            name,
            // Keep a previously known chip when this open couldn't tell (a
            // project whose main.rs has no marker yet).
            mcu_id: mcu_id.map(str::to_owned),
            opened_at,
        },
    );
    list.truncate(MAX_ENTRIES);
    list
}

/// Drop entries whose folder no longer exists. `exists` is a parameter so the
/// rule is testable without touching the filesystem.
fn prune_missing(list: Vec<RecentProject>, exists: impl Fn(&str) -> bool) -> Vec<RecentProject> {
    list.into_iter().filter(|e| exists(&e.path)).collect()
}

/// Canonical string form of a project path — absolute where possible, forward
/// slashes, so the same folder always compares equal.
fn normalize(dir: &Path) -> String {
    let abs = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    // `canonicalize` returns a `\\?\` extended path on Windows; that prefix is
    // an implementation detail nobody wants to read in a menu.
    let s = abs.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).to_owned()
}

/// Does `dir` name the same project as the stored path `stored`?
///
/// Public because the menu needs it to hide the project this window already
/// has open, and that comparison must follow the same rules the list itself
/// uses — normalized separators, case-insensitive on Windows.
pub fn is_same_path(dir: &Path, stored: &str) -> bool {
    same_path(&normalize(dir), stored)
}

/// Are these the same project path? Case-insensitive on Windows only.
fn same_path(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn file_path() -> Option<PathBuf> {
    crate::panels::mcu_module::registry::user_config_dir().map(|d| d.join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> RecentProject {
        RecentProject {
            path: path.to_owned(),
            name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            mcu_id: None,
            opened_at: 1,
        }
    }

    #[test]
    fn reopening_moves_to_the_front_without_duplicating() {
        let list = vec![entry("/a/one"), entry("/a/two"), entry("/a/three")];
        let out = promote(list, Path::new("/a/three"), Some("stm32f103"), 99);
        assert_eq!(out.len(), 3, "no duplicate for a path already listed");
        assert!(out[0].path.ends_with("/a/three"));
        assert_eq!(out[0].mcu_id.as_deref(), Some("stm32f103"));
        assert_eq!(out[0].opened_at, 99);
        assert!(out[1].path.ends_with("/a/one"), "the rest keep their order");
    }

    #[test]
    fn the_list_is_capped() {
        let mut list: Vec<RecentProject> =
            (0..MAX_ENTRIES).map(|i| entry(&format!("/p/{i}"))).collect();
        list = promote(list, Path::new("/p/new"), None, 5);
        assert_eq!(list.len(), MAX_ENTRIES);
        assert!(list[0].path.ends_with("/p/new"));
        assert!(
            !list.iter().any(|e| e.path.ends_with(&format!("/p/{}", MAX_ENTRIES - 1))),
            "the oldest entry falls off the end"
        );
    }

    /// A folder deleted or moved since it was opened must not sit in the menu
    /// as a dead row that errors when clicked.
    #[test]
    fn missing_folders_are_dropped_on_load() {
        let list = vec![entry("/gone"), entry("/here")];
        let out = prune_missing(list, |p| p == "/here");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "/here");
    }

    #[test]
    fn the_name_comes_from_the_folder_leaf() {
        let out = promote(Vec::new(), Path::new("/projects/STM32F103_radar"), None, 0);
        assert_eq!(out[0].name, "STM32F103_radar");
    }

    /// The RON round-trip is what a future build reads; `#[serde(default)]` on
    /// the newer fields is what lets an older file still parse.
    #[test]
    fn entries_survive_a_round_trip_and_older_files_still_parse() {
        let list = vec![RecentProject {
            path: "/a/b".into(),
            name: "b".into(),
            mcu_id: Some("esp32c3".into()),
            opened_at: 1234,
        }];
        let text = ron::ser::to_string_pretty(&list, ron::ser::PrettyConfig::default()).unwrap();
        assert_eq!(ron::from_str::<Vec<RecentProject>>(&text).unwrap(), list);

        let older = r#"[( path: "/a/b", name: "b" )]"#;
        let parsed: Vec<RecentProject> = ron::from_str(older).expect("older shape still parses");
        assert_eq!(parsed[0].mcu_id, None);
        assert_eq!(parsed[0].opened_at, 0);
    }
}
