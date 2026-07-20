//! The `project_structure.config` file — the Structure tab's per-project UI
//! state: dragged node positions (`@structure_layout`) and view options
//! (`@structure_view`).
//!
//! **Why it is its own file.** This state used to live in `mcu.config`. It
//! changes on every node drag and every view toggle, so `mcu.config` — which
//! holds real, reviewable configuration (virtual modules, clock tree) — showed
//! up as modified in Git constantly. Splitting them keeps `mcu.config` quiet;
//! this file is generated into `.gitignore` and is deliberately left out of the
//! Git/unsaved-changes snapshot, since it is view state, not project content.
//!
//! Same `@section` layout as [`super::mcu_config`], and reading falls back to
//! `mcu.config` so projects saved before the split keep their layout.

use super::mcu_config;
use std::path::Path;

/// File name written at the project root.
pub const FILE_NAME: &str = "project_structure.config";

/// Manually dragged node positions of the Structure diagram, keyed by the
/// module's workspace-relative file (`"mw_radar/utils.rs"`, `"main.rs"`) —
/// stable across graph rebuilds, unlike node indices. BTreeMap so the
/// serialized section is deterministic (the mtime-stable project writes rely
/// on unchanged content staying byte-identical).
pub type StructurePositions = std::collections::BTreeMap<String, (f32, f32)>;

/// Structure-tab view options persisted per project:
/// `(show_calls, call_depth, path_style as u8, show_externals)`.
pub type StructureViewPersist = (bool, Option<usize>, u8, bool);

pub(super) const LAYOUT_HEADER: &str = "@structure_layout";
pub(super) const VIEW_HEADER: &str = "@structure_view";

/// Full file text. Empty when there are no dragged positions AND the view is
/// at its defaults, so the caller can skip writing (and delete a stale file).
pub fn serialize(positions: &StructurePositions, view: &StructureViewPersist) -> String {
    let mut out = String::new();
    if !positions.is_empty() {
        let body = ron::ser::to_string_pretty(positions, ron::ser::PrettyConfig::new())
            .unwrap_or_else(|_| ron::to_string(positions).unwrap_or_else(|_| "{}".into()));
        out.push_str(&format!("{LAYOUT_HEADER}\n{body}\n"));
    }
    if *view != default_view() {
        let body = ron::to_string(view).unwrap_or_default();
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{VIEW_HEADER}\n{body}\n"));
    }
    out
}

/// The view options a fresh project starts with — matches
/// `structure_map::gui::StructureView::default()`. Serializing only on a
/// change is what keeps a default project from carrying this file at all.
fn default_view() -> StructureViewPersist {
    let d = crate::panels::structure_map::gui::StructureView::default();
    (
        d.show_calls,
        d.call_depth,
        d.path_style.to_u8(),
        d.show_externals,
    )
}

/// Parse the `@structure_layout` section (absent/garbled → empty map).
pub fn parse_layout(text: &str) -> StructurePositions {
    mcu_config::section_body(text, LAYOUT_HEADER)
        .and_then(|body| ron::from_str::<StructurePositions>(body.trim()).ok())
        .unwrap_or_default()
}

/// Parse the `@structure_view` section (absent/garbled → `None`, caller keeps
/// its defaults).
pub fn parse_view(text: &str) -> Option<StructureViewPersist> {
    mcu_config::section_body(text, VIEW_HEADER)
        .and_then(|body| ron::from_str::<StructureViewPersist>(body.trim()).ok())
}

/// Read the Structure state for the project at `root`.
///
/// Falls back to `mcu.config` when this file is absent: that is where the state
/// lived before the split, and a project saved back then must not lose its
/// layout. The next save writes the new file and drops the old sections.
pub fn load(root: &Path) -> (StructurePositions, Option<StructureViewPersist>) {
    let text = match std::fs::read_to_string(root.join(FILE_NAME)) {
        Ok(t) => t,
        Err(_) => std::fs::read_to_string(root.join(mcu_config::FILE_NAME)).unwrap_or_default(),
    };
    (parse_layout(&text), parse_view(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions() -> StructurePositions {
        let mut p = StructurePositions::new();
        p.insert("main.rs".into(), (14.0, 14.0));
        p.insert("mw_radar/utils.rs".into(), (321.5, 208.0));
        p
    }

    #[test]
    fn round_trips_positions_and_view() {
        let view: StructureViewPersist = (true, Some(3), 1, true);
        let text = serialize(&positions(), &view);
        assert_eq!(parse_layout(&text), positions());
        assert_eq!(parse_view(&text), Some(view));
    }

    /// A default project must not produce this file at all — otherwise the
    /// split would just move the Git noise instead of removing it.
    #[test]
    fn nothing_to_persist_yields_no_file() {
        assert_eq!(
            serialize(&StructurePositions::new(), &default_view()),
            "",
            "empty layout + default view must write nothing"
        );
    }

    #[test]
    fn positions_alone_are_enough_to_write() {
        let text = serialize(&positions(), &default_view());
        assert!(text.contains(LAYOUT_HEADER));
        assert!(!text.contains(VIEW_HEADER), "default view stays implicit");
    }

    /// Projects saved while this state lived in mcu.config must keep it.
    #[test]
    fn falls_back_to_mcu_config_when_the_new_file_is_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let legacy = format!(
            "@modules\n[]\n\n{}\n",
            serialize(&positions(), &(true, Some(2), 0, false))
        );
        std::fs::write(dir.path().join(mcu_config::FILE_NAME), legacy).unwrap();

        let (pos, view) = load(dir.path());
        assert_eq!(pos, positions(), "layout recovered from mcu.config");
        assert_eq!(view, Some((true, Some(2), 0, false)));
    }

    /// Once the new file exists it wins, even if mcu.config still has stale
    /// sections from before the split.
    #[test]
    fn new_file_takes_precedence_over_legacy_sections() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut stale = StructurePositions::new();
        stale.insert("main.rs".into(), (999.0, 999.0));
        std::fs::write(
            dir.path().join(mcu_config::FILE_NAME),
            serialize(&stale, &default_view()),
        )
        .unwrap();
        std::fs::write(
            dir.path().join(FILE_NAME),
            serialize(&positions(), &default_view()),
        )
        .unwrap();

        assert_eq!(load(dir.path()).0, positions());
    }

    #[test]
    fn missing_files_are_not_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let (pos, view) = load(dir.path());
        assert!(pos.is_empty());
        assert!(view.is_none());
    }
}
