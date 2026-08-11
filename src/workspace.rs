//! The per-instance scratch workspace.
//!
//! Every build, check, clippy, flash, size and rust-analyzer session runs
//! against a throw-away COPY of the project under the temp dir — never the
//! user's own folder. That directory used to be one hardcoded path, which made
//! a second IDE window a data hazard rather than a second window: both
//! processes wrote their whole project into it on every save, so `main.rs`,
//! `Cargo.toml`, `memory.x` and the target triple in `.cargo/config.toml` were
//! whatever the last instance saved. On top of that, `write_project`'s
//! foreign-crate prune deleted the other project's libraries, the notify
//! watcher pulled the other project's files into this one's tree (and from
//! there into the real project folder on the next save), and the stale-RA sweep
//! — keyed by this very directory — shot the other instance's analyzer.
//!
//! So each process now claims its own SLOT: `embedded_ide_0_check`,
//! `embedded_ide_0_check_2`, `embedded_ide_0_check_3`, … The first instance
//! keeps the original path byte-for-byte, so the usual single-window case is
//! unchanged and its warm `target/` survives this change.
//!
//! Claiming is an OS-level exclusive lock on a file inside the slot, held open
//! for the life of the process. That is deliberate: the kernel drops the handle
//! when the process dies *however* it dies, so a crash can never leave a slot
//! permanently reserved — no pid files, no liveness probes, no stale-lock
//! recovery path to get wrong.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// How many slots to try before giving up and going pid-unique. Well past any
/// plausible number of IDE windows; the cap only exists so a pathological
/// environment can't spin.
const MAX_SLOTS: u32 = 16;

/// The claimed slot's directory + its 1-based number, resolved once by [`init`].
static WORKSPACE: OnceLock<(PathBuf, u32)> = OnceLock::new();

/// The open lock file. Never read — kept alive so the OS keeps the slot
/// reserved for as long as this process runs.
static LOCK: OnceLock<std::fs::File> = OnceLock::new();

/// Claim a workspace slot. Call ONCE from `main`, before anything spawns a
/// thread or touches the workspace — every later reader gets the same answer.
pub fn init() {
    let base = std::env::temp_dir();
    for slot in 1..=MAX_SLOTS {
        let dir = base.join(slot_name(slot));
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        if let Some(file) = try_lock(&dir.join(".instance.lock")) {
            let _ = LOCK.set(file);
            let _ = WORKSPACE.set((dir, slot));
            return;
        }
    }
    // Every slot is taken (or the temp dir is unwritable). A pid-unique
    // directory is worse — a cold `target/` every launch — but it is still
    // correct, and correctness is the point of this module.
    let dir = base.join(format!("embedded_ide_0_check_p{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let _ = WORKSPACE.set((dir, 0));
}

/// Directory name for `slot`. Slot 1 is the historical path — unsuffixed, so an
/// existing install keeps its build cache.
fn slot_name(slot: u32) -> String {
    if slot <= 1 {
        "embedded_ide_0_check".to_owned()
    } else {
        format!("embedded_ide_0_check_{slot}")
    }
}

/// Absolute path of THIS instance's scratch workspace.
///
/// Falls back to claiming a slot on the spot if [`init`] never ran (a test
/// binary, say) — the return value must be stable for the whole process, so it
/// can never be left undefined.
pub fn dir() -> PathBuf {
    if WORKSPACE.get().is_none() {
        init();
    }
    WORKSPACE
        .get()
        .map(|(d, _)| d.clone())
        .unwrap_or_else(|| std::env::temp_dir().join("embedded_ide_0_check"))
}

/// This instance's slot number (1 = the first/only window, 0 = pid fallback).
pub fn slot() -> u32 {
    if WORKSPACE.get().is_none() {
        init();
    }
    WORKSPACE.get().map(|(_, s)| *s).unwrap_or(1)
}

/// Suffix that makes a per-instance name out of a shared one — empty for the
/// first instance, so its files keep the names they have always had.
///
/// For everything OUTSIDE the workspace directory that would otherwise be
/// shared between windows: the rust-analyzer log, the serial-bridge PTY links,
/// the eframe storage name.
pub fn suffix() -> String {
    match slot() {
        0 => format!("_p{}", std::process::id()),
        1 => String::new(),
        s => format!("_{s}"),
    }
}

// ── Project-folder lock ──────────────────────────────────────────────────────
// Isolating the scratch workspace stops two windows from corrupting each
// other's GENERATED project. It does nothing about the remaining hazard: the
// same project folder opened in both, where each `write_project` targets the
// user's real files and the last save silently wins.
//
// The claim uses the same OS-lock trick as the slots, but the lock file lives
// in the app's config dir rather than in the project. A file inside the project
// would show up in the tree, in `git status`, and in the stale-file prune — a
// permanent footprint for a transient fact.

/// A held claim on a project folder. Dropping it releases the folder.
pub struct ProjectLock {
    /// Kept open for the life of the claim; the OS releases it on exit.
    _file: std::fs::File,
}

/// The result of trying to claim a project folder.
pub enum ProjectClaim {
    /// This process now owns the folder.
    Acquired(ProjectLock),
    /// Another live IDE window owns it.
    Busy,
    /// Locking isn't possible here (no config dir, unwritable). Treated as
    /// "fine" by callers: a missing lock must never produce a false warning.
    Unavailable,
}

/// Try to claim `project_dir` for this instance.
///
/// The caller must drop its previous claim FIRST — reopening the same project
/// would otherwise report itself as busy.
pub fn claim_project(project_dir: &Path) -> ProjectClaim {
    let Some(path) = project_lock_path(project_dir) else {
        return ProjectClaim::Unavailable;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return ProjectClaim::Unavailable;
        }
    }
    match try_lock(&path) {
        Some(file) => ProjectClaim::Acquired(ProjectLock { _file: file }),
        None => ProjectClaim::Busy,
    }
}

/// `<config>/locks/<leaf>-<hash>.lock` for a project directory.
///
/// The hash is what identifies the folder (the leaf name is only there to make
/// the directory readable when something goes wrong). Case-insensitive on
/// Windows, where `C:\Proj` and `c:\proj` are the same folder and must not be
/// claimable twice.
fn project_lock_path(project_dir: &Path) -> Option<PathBuf> {
    use std::hash::{Hash as _, Hasher as _};
    // Canonicalize so `.`, `..` and a trailing separator can't disguise the
    // same folder as a different one. A path that can't be canonicalized (it
    // was just deleted) falls back to its literal form.
    let canonical =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let mut key = canonical.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        key = key.to_lowercase();
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    let leaf: String = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_owned())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(32)
        .collect();
    Some(
        crate::panels::mcu_module::registry::user_config_dir()?
            .join("locks")
            .join(format!("{leaf}-{:016x}.lock", hasher.finish())),
    )
}

/// Open `path` with an exclusive, non-blocking lock. `None` = another live
/// process holds it.
#[cfg(windows)]
fn try_lock(path: &Path) -> Option<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    // share_mode(0): no other process may open this file at all, which is
    // exactly the claim we want. Windows releases it when the process exits,
    // crash included.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .share_mode(0)
        .open(path)
        .ok()
}

#[cfg(unix)]
fn try_lock(path: &Path) -> Option<std::fs::File> {
    use std::os::unix::io::AsRawFd as _;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .ok()?;
    // LOCK_NB so a taken slot fails instantly instead of blocking startup.
    // The lock is released with the fd when the process exits.
    let ok = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    ok.then_some(file)
}

#[cfg(not(any(windows, unix)))]
fn try_lock(_path: &Path) -> Option<std::fs::File> {
    None // unknown platform: fall through to the pid-unique directory
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_one_keeps_the_historical_path() {
        assert_eq!(slot_name(1), "embedded_ide_0_check");
        assert_eq!(slot_name(2), "embedded_ide_0_check_2");
    }

    /// A held lock must refuse the next claim — that is the whole mechanism.
    /// Same-process locking counts: Windows share_mode and unix `flock` are
    /// both per-open-file-description, not per-process-and-forgiving.
    #[test]
    fn a_locked_slot_cannot_be_claimed_twice() {
        let dir = std::env::temp_dir().join(format!("eide_slot_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".instance.lock");
        let held = try_lock(&path).expect("first claim must succeed");
        assert!(
            try_lock(&path).is_none(),
            "a slot already claimed must not be handed out again"
        );
        drop(held);
        assert!(
            try_lock(&path).is_some(),
            "releasing the lock must free the slot"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The isolation only holds if EVERY reader goes through [`dir`]. There
    /// were 22 hand-rolled `temp_dir().join("embedded_ide_0_check")` sites
    /// before this module existed, and one re-introduced anywhere silently
    /// pins that feature back onto the first instance's workspace — a bug that
    /// shows up as cross-window corruption, not as a compile error. So the
    /// literal is banned outside this file, in the spirit of `glyph_guard`.
    #[test]
    fn nothing_rebuilds_the_workspace_path_by_hand() {
        fn scan(dir: &Path, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    scan(&p, out);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs")
                    // This module owns the name (and its own fallback).
                    || p.file_name().and_then(|x| x.to_str()) == Some("workspace.rs")
                {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                for (i, line) in text.lines().enumerate() {
                    let t = line.trim_start();
                    // Comments, doc-comments and test fixtures may SAY the
                    // name; only code that joins it onto the temp dir is a bug.
                    if t.starts_with("//") {
                        continue;
                    }
                    if line.contains("temp_dir()") && line.contains("embedded_ide_0_check") {
                        out.push(format!("{}:{}  {}", p.display(), i + 1, t));
                    }
                }
            }
        }
        let mut bad = Vec::new();
        scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut bad);
        assert!(
            bad.is_empty(),
            "the scratch workspace is PER INSTANCE - call \
             crate::workspace::dir() instead of rebuilding the path, or a \
             second IDE window will write into this one's workspace:\n{}",
            bad.join("\n")
        );
    }

    /// The same folder must map to ONE lock file however it is spelled —
    /// otherwise two windows both "claim" it and the warning never fires.
    #[test]
    fn project_lock_path_identifies_the_folder_not_the_spelling() {
        let base = std::env::temp_dir().join(format!("eide_lockpath_{}", std::process::id()));
        let proj = base.join("Proj");
        std::fs::create_dir_all(&proj).unwrap();

        let direct = project_lock_path(&proj);
        let indirect = project_lock_path(&base.join("Proj").join("."));
        assert!(
            direct.is_some(),
            "a config dir resolves in this environment"
        );
        assert_eq!(direct, indirect, "`.` must not look like another project");

        // A different folder must NOT collide.
        let other = base.join("Other");
        std::fs::create_dir_all(&other).unwrap();
        assert_ne!(direct, project_lock_path(&other));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// End to end: the second claim on a folder is refused, and releasing the
    /// first hands it back — the behaviour the warning banner reads.
    #[test]
    fn a_project_can_only_be_claimed_by_one_instance() {
        let dir = std::env::temp_dir().join(format!("eide_claim_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let first = claim_project(&dir);
        if matches!(first, ProjectClaim::Unavailable) {
            return; // no config dir here — locking is opt-out, not a failure
        }
        assert!(matches!(first, ProjectClaim::Acquired(_)));
        assert!(
            matches!(claim_project(&dir), ProjectClaim::Busy),
            "a project already open elsewhere must report Busy"
        );
        drop(first);
        assert!(
            matches!(claim_project(&dir), ProjectClaim::Acquired(_)),
            "closing the other window must free the project"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The names the OTHER shared files are built from.
    #[test]
    fn suffix_is_empty_only_for_the_first_instance() {
        // `suffix` reads the process-wide slot, so assert on the mapping it
        // uses rather than on whichever slot this test binary happened to get.
        let name = |slot: u32| match slot {
            0 => "_p<pid>".to_owned(),
            1 => String::new(),
            s => format!("_{s}"),
        };
        assert_eq!(name(1), "");
        assert_eq!(name(3), "_3");
    }
}
