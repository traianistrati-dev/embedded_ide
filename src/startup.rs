//! What a freshly launched window should open.
//!
//! Three inputs decide it — the command line, a stored preference, and whether
//! the remembered project is already open in another window — and the rule that
//! combines them is the substance of this feature, so it lives here as a pure
//! function with tests rather than inside `AppIde::new`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "startup.ron";

/// How a window decides what to open when nothing was named on the command line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupMode {
    /// Reopen the project this instance had last, without asking.
    ReopenLast,
    /// Show the picker on every start.
    ///
    /// The default: with several projects and several windows, which one a
    /// window opens should be a choice, not a consequence of launch order. The
    /// picker offers the last project as its primary action bound to Enter, so
    /// a single-window session still costs one keypress, not a hunt.
    #[default]
    AlwaysAsk,
}

/// The project this instance had last, and whether it can be reopened.
#[derive(Clone, Debug, PartialEq)]
pub enum LastProject {
    /// Nothing remembered (a fresh install, or the folder is gone).
    None,
    /// Remembered and free — the picker offers it as "Continue with…".
    Available(PathBuf),
    /// Remembered but another window has it. Named in the picker as the reason
    /// this window didn't just reopen it, rather than silently missing.
    OpenElsewhere(PathBuf),
}

/// What the app should do at startup.
#[derive(Clone, Debug, PartialEq)]
pub enum StartupAction {
    /// Open this folder straight away.
    Open(PathBuf),
    /// Show the picker, told what became of the last project.
    Ask { last: LastProject },
    /// Start with no project at all.
    Empty,
}

/// Decide what to open.
///
/// Precedence, and why:
/// 1. **the command line** — an explicit instruction, and the one thing that
///    can't be a mistake; it opens even if another window has that project
///    (the folder-claim banner then says so).
/// 2. **`AlwaysAsk`** — the user asked to be asked.
/// 3. **the remembered project**, unless another window already has it. That
///    last check is what stops the reported behaviour: two windows racing to
///    reopen their own last project, one of them landing on a folder the other
///    is already writing to.
///
/// `is_open_elsewhere` is injected so the rule can be tested without spawning
/// a second process.
pub fn decide(
    cli: Option<PathBuf>,
    mode: StartupMode,
    remembered: Option<PathBuf>,
    is_open_elsewhere: impl Fn(&Path) -> bool,
) -> StartupAction {
    if let Some(dir) = cli {
        return StartupAction::Open(dir);
    }
    let last = match remembered {
        None => LastProject::None,
        Some(dir) if is_open_elsewhere(&dir) => LastProject::OpenElsewhere(dir),
        Some(dir) => LastProject::Available(dir),
    };
    match (mode, &last) {
        (StartupMode::AlwaysAsk, _) => StartupAction::Ask { last },
        (StartupMode::ReopenLast, LastProject::Available(dir)) => StartupAction::Open(dir.clone()),
        // Nothing to reopen, and the user didn't ask to be asked.
        (StartupMode::ReopenLast, LastProject::None) => StartupAction::Empty,
        // Taken by another window: asking is the only honest move.
        (StartupMode::ReopenLast, LastProject::OpenElsewhere(_)) => StartupAction::Ask { last },
    }
}

/// Read the stored preference. Anything unreadable is the default — a startup
/// setting is not worth an error dialog.
pub fn load_mode() -> StartupMode {
    let Some(path) = file_path() else {
        return StartupMode::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| ron::from_str(&t).ok())
        .unwrap_or_default()
}

/// Persist the preference. Best-effort, for the same reason.
pub fn save_mode(mode: StartupMode) {
    let Some(path) = file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = ron::ser::to_string_pretty(&mode, ron::ser::PrettyConfig::default()) {
        let _ = std::fs::write(path, text);
    }
}

fn file_path() -> Option<PathBuf> {
    crate::panels::mcu_module::registry::user_config_dir().map(|d| d.join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing_open(_: &Path) -> bool {
        false
    }
    fn everything_open(_: &Path) -> bool {
        true
    }
    fn dir(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    #[test]
    fn the_command_line_wins_over_everything() {
        // Even over "always ask": naming a folder IS the answer to the question.
        assert_eq!(
            decide(
                dir("/cli"),
                StartupMode::AlwaysAsk,
                dir("/last"),
                everything_open
            ),
            StartupAction::Open(PathBuf::from("/cli"))
        );
        // And even over another window holding it — the banner covers that.
        assert_eq!(
            decide(dir("/cli"), StartupMode::ReopenLast, None, everything_open),
            StartupAction::Open(PathBuf::from("/cli"))
        );
    }

    #[test]
    fn reopen_last_skips_the_picker_entirely() {
        assert_eq!(
            decide(None, StartupMode::ReopenLast, dir("/last"), nothing_open),
            StartupAction::Open(PathBuf::from("/last"))
        );
        assert_eq!(
            decide(None, StartupMode::ReopenLast, None, nothing_open),
            StartupAction::Empty,
            "nothing remembered, nothing asked for: start clean"
        );
    }

    /// The whole point of the second window: it must not silently reopen a
    /// project the first window is already writing to — in EITHER mode.
    #[test]
    fn a_project_open_elsewhere_asks_instead_of_reopening() {
        assert_eq!(
            decide(
                None,
                StartupMode::ReopenLast,
                dir("/shared"),
                everything_open
            ),
            StartupAction::Ask {
                last: LastProject::OpenElsewhere(PathBuf::from("/shared"))
            }
        );
    }

    /// Asking still carries the last project, so the picker can offer it as the
    /// one-keypress way through.
    #[test]
    fn always_ask_offers_the_last_project_as_the_default_action() {
        assert_eq!(
            decide(None, StartupMode::AlwaysAsk, dir("/last"), nothing_open),
            StartupAction::Ask {
                last: LastProject::Available(PathBuf::from("/last"))
            }
        );
        assert_eq!(
            decide(None, StartupMode::AlwaysAsk, None, nothing_open),
            StartupAction::Ask {
                last: LastProject::None
            },
            "a first run has nothing to continue — the list is the whole screen"
        );
    }

    #[test]
    fn the_preference_round_trips_and_defaults_to_asking() {
        assert_eq!(
            StartupMode::default(),
            StartupMode::AlwaysAsk,
            "which project a window opens is a choice, not a consequence of launch order"
        );
        let text =
            ron::ser::to_string_pretty(&StartupMode::ReopenLast, ron::ser::PrettyConfig::default())
                .unwrap();
        assert_eq!(
            ron::from_str::<StartupMode>(&text).unwrap(),
            StartupMode::ReopenLast
        );
        assert!(
            ron::from_str::<StartupMode>("garbage").is_err(),
            "callers fall back to the default on this"
        );
    }
}
