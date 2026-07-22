//! Open a project path in the OS file manager.
//!
//! Used by the project tree's "Show in Explorer" actions. A folder opens
//! directly; a file opens its parent folder with the file selected.
//!
//! The argument building is pure and tested; only [`open`] touches the OS.

use std::path::{Path, PathBuf};

/// What the file manager should be asked to do.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    /// Open this directory.
    Dir(PathBuf),
    /// Open the parent directory with this entry highlighted.
    Select(PathBuf),
}

/// Decide what to ask the file manager for, given a path that may not exist.
///
/// A tree entry is only on disk after a Save, so `path` can legitimately be
/// missing. Falling back to the nearest existing ancestor is what makes the
/// action useful anyway — it lands you where the file WILL be, instead of
/// failing with nothing to show.
pub fn target_for(path: &Path, exists: bool, is_dir: bool) -> Option<Target> {
    if exists {
        return Some(if is_dir {
            Target::Dir(path.to_path_buf())
        } else {
            Target::Select(path.to_path_buf())
        });
    }
    // Not on disk yet: walk up to the first ancestor that is.
    let mut p = path.parent();
    while let Some(dir) = p {
        if dir.as_os_str().is_empty() {
            break;
        }
        if dir.is_dir() {
            return Some(Target::Dir(dir.to_path_buf()));
        }
        p = dir.parent();
    }
    None
}

/// The platform command + arguments for `target`. Pure, so the argument shape
/// is unit-testable without launching anything.
///
/// Windows uses `explorer /select,<path>` — note there is **no space** after
/// the comma and the whole thing is ONE argument; passing it as two makes
/// Explorer open Documents instead. macOS uses `open -R`. Linux has no
/// portable "select this file" verb, so it opens the containing folder.
pub fn command_for(target: &Target) -> (&'static str, Vec<String>) {
    match target {
        Target::Dir(d) => {
            let p = d.to_string_lossy().to_string();
            if cfg!(target_os = "windows") {
                ("explorer", vec![p])
            } else if cfg!(target_os = "macos") {
                ("open", vec![p])
            } else {
                ("xdg-open", vec![p])
            }
        }
        Target::Select(f) => {
            let p = f.to_string_lossy().to_string();
            if cfg!(target_os = "windows") {
                ("explorer", vec![format!("/select,{p}")])
            } else if cfg!(target_os = "macos") {
                ("open", vec!["-R".to_string(), p])
            } else {
                // No select verb — show the folder that contains it.
                let dir = f
                    .parent()
                    .map(|d| d.to_string_lossy().to_string())
                    .unwrap_or(p);
                ("xdg-open", vec![dir])
            }
        }
    }
}

/// Launch the file manager for `path`.
///
/// **The exit code is deliberately ignored.** `explorer.exe` returns 1 even
/// when it succeeded (verified on Windows 10 for both the folder and the
/// `/select` form), so checking it would report a failure on every successful
/// click. Only a failure to SPAWN is a real error.
pub fn open(path: &Path) -> Result<(), String> {
    let target = target_for(path, path.exists(), path.is_dir())
        .ok_or_else(|| format!("{} is not on disk yet", path.display()))?;
    let (program, args) = command_for(&target);
    let mut cmd = std::process::Command::new(program);
    crate::build::no_window(&mut cmd).args(&args);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open the file manager ({program}): {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_dir_opens_itself_file_gets_selected() {
        let d = Path::new("C:/proj/src");
        assert_eq!(
            target_for(d, true, true),
            Some(Target::Dir(d.to_path_buf()))
        );
        let f = Path::new("C:/proj/src/main.rs");
        assert_eq!(
            target_for(f, true, false),
            Some(Target::Select(f.to_path_buf()))
        );
    }

    #[test]
    fn a_file_not_yet_saved_falls_back_to_an_existing_ancestor() {
        // Tree entries live in memory until a Save, so this is the normal case
        // for a freshly created file — it must still open somewhere useful.
        let dir = std::env::temp_dir();
        let missing = dir.join("eide_reveal_does_not_exist_123/deeper/x.rs");
        assert_eq!(
            target_for(&missing, false, false),
            Some(Target::Dir(dir.clone()))
        );
    }

    #[test]
    fn windows_select_is_one_argument_with_no_space() {
        // `explorer /select, <path>` as two args (or with a space) opens
        // Documents instead of selecting the file.
        let t = Target::Select(PathBuf::from("C:/proj/src/main.rs"));
        let (prog, args) = command_for(&t);
        if cfg!(target_os = "windows") {
            assert_eq!(prog, "explorer");
            assert_eq!(args.len(), 1);
            assert_eq!(args[0], "/select,C:/proj/src/main.rs");
            assert!(!args[0].contains(", "));
        }
    }

    #[test]
    fn windows_dir_is_passed_bare() {
        let t = Target::Dir(PathBuf::from("C:/proj/src"));
        let (prog, args) = command_for(&t);
        if cfg!(target_os = "windows") {
            assert_eq!(prog, "explorer");
            assert_eq!(args, vec!["C:/proj/src".to_string()]);
        }
    }
}
