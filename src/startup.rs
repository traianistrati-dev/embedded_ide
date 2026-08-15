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
    /// Reopen the project this instance had last — the behaviour that has
    /// always been, and the right default for a single window.
    #[default]
    ReopenLast,
    /// Always show the picker. For someone who keeps several projects and
    /// several windows, and wants to say which is which every time.
    AlwaysAsk,
}

/// What the app should do at startup.
#[derive(Clone, Debug, PartialEq)]
pub enum StartupAction {
    /// Open this folder straight away.
    Open(PathBuf),
    /// Show the picker. `blocked` is the project that WOULD have reopened but
    /// is already open in another window — worth saying, since otherwise the
    /// picker looks like it forgot the last project.
    Ask { blocked: Option<PathBuf> },
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
    if mode == StartupMode::AlwaysAsk {
        return StartupAction::Ask { blocked: None };
    }
    match remembered {
        Some(dir) if is_open_elsewhere(&dir) => StartupAction::Ask { blocked: Some(dir) },
        Some(dir) => StartupAction::Open(dir),
        None => StartupAction::Empty,
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
    fn reopen_last_is_the_default_path() {
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
    /// project the first window is already writing to.
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
                blocked: Some(PathBuf::from("/shared"))
            }
        );
    }

    #[test]
    fn always_ask_asks_even_with_a_free_project_remembered() {
        assert_eq!(
            decide(None, StartupMode::AlwaysAsk, dir("/last"), nothing_open),
            StartupAction::Ask { blocked: None }
        );
    }

    #[test]
    fn the_preference_round_trips_and_defaults_to_reopen_last() {
        assert_eq!(StartupMode::default(), StartupMode::ReopenLast);
        let text =
            ron::ser::to_string_pretty(&StartupMode::AlwaysAsk, ron::ser::PrettyConfig::default())
                .unwrap();
        assert_eq!(
            ron::from_str::<StartupMode>(&text).unwrap(),
            StartupMode::AlwaysAsk
        );
        assert!(
            ron::from_str::<StartupMode>("garbage").is_err(),
            "callers fall back to the default on this"
        );
    }
}
