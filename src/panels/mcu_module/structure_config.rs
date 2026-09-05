//! The `project_structure.config` file — per-project DIAGRAM state: the
//! Structure tab's dragged node positions (`@structure_layout`) and view options
//! (`@structure_view`), the Clock tab's dragged node positions
//! (`@clock_layout`), and the Flow tab's charted function (`@flow_view`).
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

/// Clock-diagram node positions the user dragged, keyed by graph node id
/// (`"pllm"`, `"sysclk"`). Only nodes MOVED away from the automatic layout are
/// stored, so improving `auto_layout` still reaches every untouched node — and a
/// project nobody dragged in carries no section at all.
pub type ClockPositions = std::collections::BTreeMap<String, (f32, f32)>;

/// Clock-tab view options persisted per project: whether the FIELDS list is
/// shown beside the diagram.
///
/// A bare `bool` because that is all there is; if the tab grows another view
/// option this becomes a tuple, and an older file then simply fails to parse and
/// falls back to the default — acceptable for a gitignored view-state file.
pub type ClockViewPersist = bool;

/// A fresh project shows the diagram alone.
pub const CLOCK_VIEW_DEFAULT: ClockViewPersist = false;

/// Flow-tab reading position: `(project-root-relative file, function name)`.
///
/// The FILE is half of it deliberately. A bare function name would be restored
/// onto whatever file happened to be open at the time, where it either means
/// nothing or - worse - silently matches a same-named function in a different
/// file. Restored only when the paths agree; otherwise the tab falls back to
/// the file's own entry point.
pub type FlowViewPersist = (String, String);

pub(super) const LAYOUT_HEADER: &str = "@structure_layout";
pub(super) const VIEW_HEADER: &str = "@structure_view";
pub(super) const CLOCK_HEADER: &str = "@clock_layout";
pub(super) const CLOCK_VIEW_HEADER: &str = "@clock_view";
pub(super) const FLOW_HEADER: &str = "@flow_view";

/// Full file text. Empty when there is nothing to persist (no dragged positions
/// in either diagram AND a default view), so the caller can skip writing (and
/// delete a stale file).
pub fn serialize(
    positions: &StructurePositions,
    view: &StructureViewPersist,
    clock: &ClockPositions,
    clock_view: &ClockViewPersist,
    flow: &FlowViewPersist,
) -> String {
    let mut out = String::new();
    let section = |out: &mut String, header: &str, body: String| {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{header}\n{body}\n"));
    };
    let pretty = |p: &std::collections::BTreeMap<String, (f32, f32)>| {
        ron::ser::to_string_pretty(p, ron::ser::PrettyConfig::new())
            .unwrap_or_else(|_| ron::to_string(p).unwrap_or_else(|_| "{}".into()))
    };
    if !positions.is_empty() {
        section(&mut out, LAYOUT_HEADER, pretty(positions));
    }
    if *view != default_view() {
        section(
            &mut out,
            VIEW_HEADER,
            ron::to_string(view).unwrap_or_default(),
        );
    }
    if !clock.is_empty() {
        section(&mut out, CLOCK_HEADER, pretty(clock));
    }
    if *clock_view != CLOCK_VIEW_DEFAULT {
        section(
            &mut out,
            CLOCK_VIEW_HEADER,
            ron::to_string(clock_view).unwrap_or_default(),
        );
    }
    if !flow.0.is_empty() && !flow.1.is_empty() {
        section(
            &mut out,
            FLOW_HEADER,
            ron::to_string(flow).unwrap_or_default(),
        );
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

/// Parse the `@clock_view` section (absent/garbled → the default).
pub fn parse_clock_view(text: &str) -> ClockViewPersist {
    mcu_config::section_body(text, CLOCK_VIEW_HEADER)
        .and_then(|body| ron::from_str::<ClockViewPersist>(body.trim()).ok())
        .unwrap_or(CLOCK_VIEW_DEFAULT)
}

/// Parse the `@flow_view` section (absent/garbled -> `None`, caller keeps its
/// defaults).
pub fn parse_flow_view(text: &str) -> Option<FlowViewPersist> {
    mcu_config::section_body(text, FLOW_HEADER)
        .and_then(|body| ron::from_str::<FlowViewPersist>(body.trim()).ok())
}

/// Parse the `@clock_layout` section (absent/garbled → empty map).
pub fn parse_clock(text: &str) -> ClockPositions {
    mcu_config::section_body(text, CLOCK_HEADER)
        .and_then(|body| ron::from_str::<ClockPositions>(body.trim()).ok())
        .unwrap_or_default()
}

/// Read the Structure state for the project at `root`.
///
/// Falls back to `mcu.config` when this file is absent: that is where the state
/// lived before the split, and a project saved back then must not lose its
/// layout. The next save writes the new file and drops the old sections.
pub fn load(
    root: &Path,
) -> (
    StructurePositions,
    Option<StructureViewPersist>,
    ClockPositions,
    ClockViewPersist,
    Option<FlowViewPersist>,
) {
    let text = match std::fs::read_to_string(root.join(FILE_NAME)) {
        Ok(t) => t,
        Err(_) => std::fs::read_to_string(root.join(mcu_config::FILE_NAME)).unwrap_or_default(),
    };
    (
        parse_layout(&text),
        parse_view(&text),
        parse_clock(&text),
        parse_clock_view(&text),
        parse_flow_view(&text),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No Flow-tab reading position — what most of these cases care about.
    fn no_flow() -> FlowViewPersist {
        FlowViewPersist::default()
    }

    fn positions() -> StructurePositions {
        let mut p = StructurePositions::new();
        p.insert("main.rs".into(), (14.0, 14.0));
        p.insert("mw_radar/utils.rs".into(), (321.5, 208.0));
        p
    }

    #[test]
    fn round_trips_positions_and_view() {
        let view: StructureViewPersist = (true, Some(3), 1, true);
        let flow: FlowViewPersist = ("src/mw_radar/parse.rs".into(), "HmmdFrame::feed".into());
        let text = serialize(
            &positions(),
            &view,
            &ClockPositions::new(),
            &CLOCK_VIEW_DEFAULT,
            &flow,
        );
        assert_eq!(parse_layout(&text), positions());
        assert_eq!(parse_view(&text), Some(view));
        assert_eq!(parse_flow_view(&text), Some(flow));
    }

    /// A default project must not produce this file at all — otherwise the
    /// split would just move the Git noise instead of removing it.
    #[test]
    fn nothing_to_persist_yields_no_file() {
        assert_eq!(
            serialize(
                &StructurePositions::new(),
                &default_view(),
                &ClockPositions::new(),
                &CLOCK_VIEW_DEFAULT,
                &FlowViewPersist::default()
            ),
            "",
            "empty layout + default view must write nothing"
        );
    }

    #[test]
    fn positions_alone_are_enough_to_write() {
        let text = serialize(
            &positions(),
            &default_view(),
            &ClockPositions::new(),
            &CLOCK_VIEW_DEFAULT,
            &no_flow(),
        );
        assert!(text.contains(LAYOUT_HEADER));
        assert!(!text.contains(VIEW_HEADER), "default view stays implicit");
    }

    /// Projects saved while this state lived in mcu.config must keep it.
    #[test]
    fn falls_back_to_mcu_config_when_the_new_file_is_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let legacy = format!(
            "@modules\n[]\n\n{}\n",
            serialize(
                &positions(),
                &(true, Some(2), 0, false),
                &ClockPositions::new(),
                &CLOCK_VIEW_DEFAULT,
                &no_flow()
            )
        );
        std::fs::write(dir.path().join(mcu_config::FILE_NAME), legacy).unwrap();

        let (pos, view, _clock, _cv, _flow) = load(dir.path());
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
            serialize(
                &stale,
                &default_view(),
                &ClockPositions::new(),
                &CLOCK_VIEW_DEFAULT,
                &no_flow(),
            ),
        )
        .unwrap();
        std::fs::write(
            dir.path().join(FILE_NAME),
            serialize(
                &positions(),
                &default_view(),
                &ClockPositions::new(),
                &CLOCK_VIEW_DEFAULT,
                &no_flow(),
            ),
        )
        .unwrap();

        assert_eq!(load(dir.path()).0, positions());
    }

    /// The Clock diagram's dragged boxes ride in the same file, in their own
    /// section — and stay absent when nobody dragged anything.
    #[test]
    fn clock_positions_round_trip_alongside_the_structure_ones() {
        let mut clock = ClockPositions::new();
        clock.insert("pllm".into(), (240.0, 118.0));
        clock.insert("sysclk".into(), (612.5, 46.0));

        let text = serialize(
            &positions(),
            &default_view(),
            &clock,
            &CLOCK_VIEW_DEFAULT,
            &no_flow(),
        );
        assert_eq!(parse_clock(&text), clock);
        assert_eq!(parse_layout(&text), positions(), "sections stay separate");

        let without = serialize(
            &positions(),
            &default_view(),
            &ClockPositions::new(),
            &CLOCK_VIEW_DEFAULT,
            &no_flow(),
        );
        assert!(
            !without.contains(CLOCK_HEADER),
            "an undragged clock diagram writes no section"
        );
        assert!(parse_clock(&without).is_empty());
    }

    /// Clock positions alone are reason enough to write the file.
    #[test]
    fn clock_positions_alone_are_enough_to_write() {
        let mut clock = ClockPositions::new();
        clock.insert("hclk".into(), (700.0, 300.0));
        let text = serialize(
            &StructurePositions::new(),
            &default_view(),
            &clock,
            &CLOCK_VIEW_DEFAULT,
            &no_flow(),
        );
        assert!(text.contains(CLOCK_HEADER));
        assert!(!text.contains(LAYOUT_HEADER));
    }

    /// The Clock tab's fields-view preference is per project, and absent from a
    /// file that never turned it on.
    #[test]
    fn the_clock_view_preference_round_trips() {
        let empty = StructurePositions::new();
        let none = ClockPositions::new();

        let on = serialize(&empty, &default_view(), &none, &true, &no_flow());
        assert!(on.contains(CLOCK_VIEW_HEADER));
        assert!(parse_clock_view(&on));

        let off = serialize(
            &empty,
            &default_view(),
            &none,
            &CLOCK_VIEW_DEFAULT,
            &no_flow(),
        );
        assert_eq!(off, "", "the default writes nothing at all");
        assert_eq!(parse_clock_view(&off), CLOCK_VIEW_DEFAULT);

        // A file written before the section existed keeps working.
        assert_eq!(
            parse_clock_view(
                "@modules
[]
"
            ),
            CLOCK_VIEW_DEFAULT
        );
    }

    #[test]
    fn missing_files_are_not_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let (pos, view, clock, _cv, _flow) = load(dir.path());
        assert!(pos.is_empty());
        assert!(clock.is_empty());
        assert!(view.is_none());
    }
}
