use crate::build::BuildState;
use crate::dfu::{self, DfuState};
use crate::espflash::EspFlashState;
use crate::lsp::{self, LspStatus};
use crate::openocd::OpenOcdState;
use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use crate::panels::mcu_module::mcu_def::{McuDefinition, ProjectDef};
use crate::panels::mcu_module::{project_gen, project_gen::ProjectFiles, registry};
use crate::project_tree::ProjectTreeState;
use crate::required_tools;
use eframe::egui;
use egui_code_editor::{Completer, Syntax};
use notify::Watcher as _;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

// ── Module structure ──────────────────────────────────────────────────────────
pub(crate) mod tabs;

pub(crate) mod helpers;
use helpers::apply_dark_theme;

mod chip_filter_ui;
mod chip_search_ui;
mod clock_import_dialog;
mod clone_project_dialog;
mod datasheet_import_dialog;
mod dialogs;
mod extract_crate_dialog;
mod mcu_form_dialog;

mod diag_panel;

mod project_panel;

mod tree_clipboard;

mod mcu_panel;

mod structure_tab;

mod editor_panel;

mod project_io;

mod loading_overlay;

mod startup_picker;

mod settings_menu;

// ── Project file selector ─────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug, Default)]
pub enum ProjectFileId {
    #[default]
    MainRs,
    CargoToml,
    CargoConfig,
    MemoryX,
    BuildRs,
    GitIgnore,
    /// Index into `AppIde::user_src_files`
    UserFile(usize),
}

impl ProjectFileId {
    fn label(self) -> &'static str {
        match self {
            Self::MainRs => "src/main.rs",
            Self::CargoToml => "Cargo.toml",
            Self::CargoConfig => ".cargo/config.toml",
            Self::MemoryX => "memory.x",
            Self::BuildRs => "build.rs",
            Self::GitIgnore => ".gitignore",
            Self::UserFile(_) => "src/???", // resolved at call site
        }
    }

    fn content<'a>(self, files: &'a ProjectFiles) -> &'a str {
        match self {
            Self::MainRs => &files.main_rs,
            Self::CargoToml => &files.cargo_toml,
            Self::CargoConfig => &files.cargo_config,
            Self::MemoryX => &files.memory_x,
            Self::BuildRs => &files.build_rs,
            Self::GitIgnore => &files.gitignore,
            Self::UserFile(_) => "", // handled separately before calling this
        }
    }

    /// `path` is the file's project-root-relative path, needed for `UserFile`:
    /// a library crate's `Cargo.toml` is a user file too, and highlighting it
    /// as Rust would render its `#` comments as ordinary text.
    /// Is this a Cargo manifest — the firmware's, or an extracted library's
    /// (which is an ordinary user file at `<crate>/Cargo.toml`)? `path` is the
    /// file's project-root-relative path, only consulted for `UserFile`.
    pub(crate) fn is_cargo_manifest(self, path: &str) -> bool {
        matches!(self, Self::CargoToml)
            || (matches!(self, Self::UserFile(_))
                && (path == "Cargo.toml" || path.ends_with("/Cargo.toml")))
    }

    fn syntax(self, path: &str) -> Syntax {
        match self {
            // TOML (Cargo.toml, .cargo/config.toml) and .gitignore use `#` line
            // comments — give them a syntax whose comment marker is `#` so those
            // lines render in the comment (gray) colour, matching `//` comments
            // in .rs files (same theme → same Comment token colour).
            Self::CargoToml | Self::CargoConfig | Self::GitIgnore => Syntax::simple("#"),
            Self::UserFile(_) if !path.ends_with(".rs") => Syntax::simple("#"),
            // main.rs / build.rs are Rust; memory.x uses C-style `/* */`, which
            // Rust highlighting already renders as comments.
            _ => Syntax::rust(),
        }
    }

    /// Path as reported by `rustc` in JSON diagnostics (relative to project root).
    /// Returns `None` for files that rustc never reports errors for.
    pub fn cargo_path(self) -> Option<&'static str> {
        match self {
            Self::MainRs => Some("src/main.rs"),
            Self::BuildRs => Some("build.rs"),
            Self::CargoToml => Some("Cargo.toml"),
            _ => None,
        }
    }

    /// Project-root-relative path of a FIXED project file. `None` for
    /// `UserFile`, whose path is not knowable from the id alone — it lives in
    /// `user_src_files` (see [`Self::rel_path`]).
    ///
    /// Unlike [`Self::cargo_path`] this covers EVERY fixed file, not just the
    /// three rustc reports diagnostics for.
    pub fn fixed_rel_path(self) -> Option<&'static str> {
        Some(match self {
            Self::MainRs => "src/main.rs",
            Self::CargoToml => "Cargo.toml",
            Self::CargoConfig => ".cargo/config.toml",
            Self::MemoryX => "memory.x",
            Self::BuildRs => "build.rs",
            Self::GitIgnore => ".gitignore",
            Self::UserFile(_) => return None,
        })
    }

    /// Project-root-relative path of ANY editor file. `None` only for a
    /// `UserFile` index that no longer exists (deleted since it was captured).
    pub fn rel_path(self, user_files: &[(String, String)]) -> Option<String> {
        match self {
            Self::UserFile(i) => user_files.get(i).map(|(p, _)| p.clone()),
            _ => self.fixed_rel_path().map(str::to_owned),
        }
    }
}

/// Translucent band colour for a clicked diagnostic's line in the editor, keyed
/// by severity — same alpha (26 ≈ 0.1) as the error red. Error → red,
/// Warning → yellow, Info / Hint → blue.
pub fn diag_highlight_color(sev: lsp::DiagSeverity) -> egui::Color32 {
    match sev {
        lsp::DiagSeverity::Error => egui::Color32::from_rgba_unmultiplied(255, 0, 100, 26),
        lsp::DiagSeverity::Warning => egui::Color32::from_rgba_unmultiplied(255, 210, 0, 26),
        lsp::DiagSeverity::Info | lsp::DiagSeverity::Hint => {
            egui::Color32::from_rgba_unmultiplied(60, 150, 255, 26)
        }
    }
}

/// Height of a bottom panel that has been collapsed to its bar: the button /
/// tab row, the frame's vertical margin, and the drag handle above it.
///
/// Only the panel under the editor needs it — that one is `exact_size`d and so
/// cannot shrink to its content. The Virtual-modules panel hugs its own content
/// instead: forcing it into this same box lined the outlines up but left dead
/// space under its buttons, which is what you actually see.
pub fn collapsed_panel_height(ui: &egui::Ui, handle_h: f32) -> f32 {
    const FRAME_V_MARGIN: f32 = 4.0; // `Frame::side_top_panel` is symmetric(8, 2)
    ui.spacing().interact_size.y + ui.spacing().item_spacing.y * 2.0 + FRAME_V_MARGIN + handle_h
}

/// An F12 pressed while rust-analyzer could not answer, kept until it can.
///
/// The position is captured HERE, at the keypress, because it only exists
/// inside the editor's render path (the caret lives in the `TextEdit`'s state
/// and the column is computed from the live buffer) — the frame loop that
/// waits for the analyzer has neither.
pub struct PendingGoto {
    /// The file the caret was in. A deferred jump that fired against a
    /// different file would be nonsense.
    pub file: ProjectFileId,
    /// Workspace-relative path, as the LSP request wants it.
    pub rel: String,
    /// 0-based line, and the column in UTF-16 units (LSP's own unit).
    pub line: u32,
    pub col: u32,
    /// Ctrl+F12 rather than F12.
    pub implementation: bool,
    /// Hash of the buffer the position was taken in. A cold analyzer start
    /// takes tens of seconds, in which one pin click regenerates main.rs
    /// wholesale — the same line and column would then point at a different
    /// symbol, and the jump would be a lie.
    pub text_hash: u64,
    /// When it was asked for, for the give-up deadline.
    pub since: std::time::Instant,
    /// The one-shot restart has been fired. Without this the wait would reset
    /// the analyzer it just spawned on the very next frame, for ever.
    pub restart_fired: bool,
}

/// How long to wait for a cold analyzer before giving up on a deferred jump.
/// Generous on purpose: a first index of an embedded project routinely takes
/// half a minute, and the alternative is telling the user it failed while it is
/// still working.
const GOTO_WAIT: std::time::Duration = std::time::Duration::from_secs(90);

/// Background of a diagnostic row in the bottom panel. Every one of those rows
/// navigates to the code it names, so hovering has to say so: the row lights up,
/// its `file:line` is underlined, and the cursor becomes a pointing hand (see
/// [`diag_row_link_hint`]). Without that the list reads as static text and the
/// click goes undiscovered.
pub fn diag_row_bg(selected: bool, hovered: bool) -> egui::Color32 {
    if selected {
        egui::Color32::from_rgba_premultiplied(60, 80, 110, 180)
    } else if hovered {
        egui::Color32::from_rgba_premultiplied(60, 80, 110, 70)
    } else {
        egui::Color32::TRANSPARENT
    }
}

/// The hover half of [`diag_row_bg`]: underline the `file:line` text (whose rect
/// `painter.text` just returned) and switch the cursor to the pointing hand.
pub fn diag_row_link_hint(
    painter: &egui::Painter,
    resp: &egui::Response,
    location: egui::Rect,
    color: egui::Color32,
) {
    if resp.hovered() && location.width() > 0.0 {
        painter.hline(
            location.x_range(),
            location.bottom() - 0.5,
            egui::Stroke::new(1.0_f32, color),
        );
        painter
            .ctx()
            .set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

/// The lines lit up by a jump from the Pins canvas: one for a pin click, one per
/// wired pin for a module click. All in the same file — the editor shows one, and
/// a band nobody can see is worse than a missing line.
struct PinHighlight {
    file: ProjectFileId,
    /// 1-based, sorted, deduplicated.
    lines: Vec<usize>,
    /// `Context::input().time` when the pulse started.
    start: f64,
}

/// How long the "here is your pin" band pulses before clearing itself — about
/// four pulses at [`PIN_PULSE_HZ`], long enough to catch the eye without leaving
/// a permanent stripe behind.
const PIN_PULSE_SECS: f64 = 2.5;
const PIN_PULSE_HZ: f64 = 1.5;
/// Peak alpha of that band. The band is painted OVER the text, so a genuinely
/// opaque yellow would hide the very line it points at — 80/255 (≈31 %) reads as
/// a clear wash while the code stays perfectly legible.
const PIN_PULSE_ALPHA: f32 = 80.0;

/// Lift persisted paths from `src/`-relative to PROJECT-ROOT-relative.
///
/// Paths in `user_src_files` used to be relative to `src/`; they are now
/// relative to the project root, so a library crate's files can live in the
/// same flat list. eframe persists this list across restarts, so state written
/// by an older build must be lifted on load — otherwise every file points at
/// the wrong place and `write_project` recreates them at the root.
///
/// Driven by an explicit `paths_root_relative` flag rather than by sniffing the
/// paths: a user folder literally named `src` would make any heuristic guess
/// wrong, and guessing wrong here silently relocates the user's whole project.
fn migrate_to_root_relative(files: Vec<(String, String)>, already: bool) -> Vec<(String, String)> {
    if already {
        return files;
    }
    files
        .into_iter()
        .map(|(p, c)| (format!("src/{p}"), c))
        .collect()
}

fn migrate_folders_to_root_relative(folders: Vec<String>, already: bool) -> Vec<String> {
    if already {
        return folders;
    }
    folders.into_iter().map(|f| format!("src/{f}")).collect()
}

/// Resolve a diagnostic's project-relative path (as reported by rustc /
/// rust-analyzer) to the editor file it should open. `user_files` names are
/// relative to the PROJECT ROOT (`src/pins.rs`, `mw_radar/src/lib.rs`), which
/// is exactly the form cargo reports spans in — so library diagnostics resolve
/// through the same path as the firmware's.
pub fn resolve_diag_file(path: &str, user_files: &[(String, String)]) -> Option<ProjectFileId> {
    match path {
        "src/main.rs" => Some(ProjectFileId::MainRs),
        "build.rs" => Some(ProjectFileId::BuildRs),
        "Cargo.toml" => Some(ProjectFileId::CargoToml),
        "memory.x" => Some(ProjectFileId::MemoryX),
        ".cargo/config.toml" => Some(ProjectFileId::CargoConfig),
        ".gitignore" => Some(ProjectFileId::GitIgnore),
        _ => user_files
            .iter()
            .position(|(name, _)| path == name)
            .map(ProjectFileId::UserFile),
    }
}

/// Byte ranges of main.rs's GENERATED blocks (`GEN_BEGIN` marker → end of the
/// `GEN_END` marker line). The code inside is owned by the MCU Configurator and
/// regenerated on config changes, so a Clippy auto-fix landing there must be
/// blocked (it would be reverted). A fix span `[s, e)` is "locked" when it
/// overlaps any returned range: `s < range.1 && e > range.0`.
pub fn generated_byte_ranges(code: &str) -> Vec<(usize, usize)> {
    use crate::panels::mcu_module::codegen::common::{GEN_BEGIN, GEN_END};
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(rel_b) = code[from..].find(GEN_BEGIN) {
        let b = from + rel_b;
        let after_begin = b + GEN_BEGIN.len();
        match code[after_begin..].find(GEN_END) {
            Some(rel_e) => {
                let end = after_begin + rel_e + GEN_END.len();
                ranges.push((b, end));
                from = end;
            }
            None => {
                // Unterminated block — lock to end of file.
                ranges.push((b, code.len()));
                break;
            }
        }
    }
    ranges
}

/// Splice `edits` into `buf` (one file's content), largest-offset first so that
/// earlier byte offsets stay valid as the text changes. An edit is skipped when
/// its span is reversed, out of bounds, off a UTF-8 char boundary, overlaps an
/// already-applied edit, or overlaps any `locked` byte range (main.rs's
/// GENERATED block). Returns how many edits were applied.
pub fn apply_edits_to_buffer(
    buf: &mut String,
    edits: &[&crate::build::SpanEdit],
    locked: &[(usize, usize)],
) -> usize {
    let mut sorted: Vec<&crate::build::SpanEdit> = edits.to_vec();
    sorted.sort_by(|a, b| b.start.cmp(&a.start));
    let mut applied = 0usize;
    let mut last_start = buf.len() + 1;
    for e in sorted {
        if e.start > e.end || e.end > buf.len() {
            continue;
        }
        if e.end > last_start {
            continue; // overlaps an already-applied (later) edit
        }
        if !buf.is_char_boundary(e.start) || !buf.is_char_boundary(e.end) {
            continue;
        }
        if locked.iter().any(|&(b, end)| e.start < end && e.end > b) {
            continue; // inside the GENERATED block — never edit
        }
        buf.replace_range(e.start..e.end, &e.replacement);
        last_start = e.start;
        applied += 1;
    }
    applied
}

// ── Tab bar ──────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug)]
enum McuTab {
    Pins,
    Peripherals,
    /// Pin-less peripherals (IWDG / WWDG): configured from a tab because they
    /// have no pin to click on the Pins canvas.
    Configuration,
    Clock,
    System,
    /// Module-relationship diagram of the project (parse-based, chip-agnostic).
    Structure,
    /// F12 "Go to definition" snippet (external / crate / std files). The tab
    /// appears only while `definition_view` is set.
    Definition,
    /// A second project file, opened READ-ONLY beside the editor so it can be
    /// consulted while typing. Appears only while `reference_file` is set.
    Reference,
}

impl McuTab {
    fn label(self) -> &'static str {
        match self {
            Self::Pins => "Pins",
            Self::Peripherals => "Peripherals",
            Self::Configuration => "Configuration",
            Self::Clock => "Clock",
            Self::System => "System",
            Self::Structure => "Structure",
            Self::Definition => "Definition",
            Self::Reference => "Reference",
        }
    }

    /// Tab-bar group: `false` = the chip-config "MCU" group (Pins /
    /// Peripherals / Clock / System), `true` = the chip-agnostic "Project"
    /// group (Structure / Definition).
    /// Structure / Definition — the tabs that live UNDER the "Project" group
    /// button.
    ///
    /// `Reference` is deliberately absent: it is its own top-level entry beside
    /// MCU and Project, because it holds an editor and has to line up with the
    /// main one. Nested a level deeper it sat below the code it is read against.
    fn is_project_group(self) -> bool {
        matches!(self, Self::Structure | Self::Definition)
    }
}

// ── Build panel tab ──────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug, Default)]
enum BuildPanelTab {
    #[default]
    RustAnalyzer,
    Cargo,
    Dfu,
    /// RTT / defmt live logs through the debug probe (probe-rs).
    Rtt,
    /// On-target debugger (probe-rs dap-server): breakpoints, step, variables.
    Debug,
    /// Built-in USART/UART serial console.
    Serial,
    /// `cargo clippy` improvement suggestions.
    Clippy,
    /// `cargo bloat` code-size breakdown (Flash per function / crate).
    Profile,
    /// Built-in command console (streaming host-shell runner — PowerShell on
    /// Windows, `$SHELL` elsewhere).
    Terminal,
    /// Per-action timing breakdown (Save / Build / Flash / Clippy).
    Activity,
    /// Git status / commit / push / pull in the project directory.
    Git,
    RequiredTools,
}

/// Which of the two code editors a keystroke / completion belongs to.
///
/// Only ONE completion popup can exist at a time — it belongs to one caret at
/// one moment — so the completion state stays a single set, tagged with its
/// owner rather than duplicated per editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum EditorSlot {
    /// The main editor (left panel).
    #[default]
    Main,
    /// The second editor in the "Reference" tab.
    Reference,
}

/// The source snippet shown in the F12 "Definition" tab (MCU Configurator,
/// next to Structure — moved from the bottom panel on 2026-07-10).
struct DefinitionView {
    /// Header line, e.g. `src/pins/utils/i2c1.rs  (line 42)`.
    header: String,
    /// The code snippet around the definition.
    code: String,
    /// 0-based index (within `code`'s lines) of the definition line, drawn
    /// coloured so it stands out from the rest.
    highlight: usize,
}

// ── Horizontal layout minimums ────────────────────────────────────────────────
// The window is three columns: [Editor][MCU Configurator][Project tree]. Only
// the MCU zone is a `CentralPanel` — it has no width of its own and takes
// whatever the other two leave, which is why it is the one that gets starved,
// and why every rule below is written to protect IT.

/// Narrowest useful code editor. Also the `Panel::left` min width.
pub(crate) const EDITOR_MIN_W: f32 = 220.0;
/// Narrowest useful MCU Configurator: the four-tab row plus a chip still big
/// enough to read its pin labels. Below this the zone is chrome and nothing
/// else — the pin canvas has no room left at all.
pub(crate) const MCU_MIN_W: f32 = 420.0;
/// Narrowest useful project tree — the panel's own default width.
pub(crate) const TREE_MIN_W: f32 = 200.0;
/// Below this the three columns cannot all hold their minimum at once. Derived,
/// not picked: change a minimum above and this follows.
///
/// It is a floor, not the whole test — a window can clear it and still be a
/// tall strip nobody wants three columns in. See [`room_for_three_columns`].
pub(crate) const NARROW_W: f32 = EDITOR_MIN_W + MCU_MIN_W + TREE_MIN_W;

/// Is there room for all three columns — editor, MCU zone and project tree?
///
/// Two gates, and BOTH must pass:
///
/// 1. **`content_w >= NARROW_W`** — the arithmetic floor. Below it the three
///    minimums simply do not add up, whatever the screen looks like.
/// 2. **The window is a landscape working area**: the display is wider than it
///    is tall AND the window takes more than half of it. Either half failing
///    means the user is working in a tall, narrow strip — a portrait monitor, or
///    a window docked to one side of a landscape one — where three columns are
///    arithmetically possible but miserable.
///
/// Gate 2 is an AND, not an OR. On an OR a landscape display would satisfy it by
/// itself and every side-docked window would be back to three columns, which is
/// the case this exists for.
///
/// `monitor` unknown (the backend does not report it) → gate 1 decides alone;
/// guessing an orientation would be worse than not applying the rule.
fn room_for_three_columns(content_w: f32, window_w: f32, monitor: Option<egui::Vec2>) -> bool {
    if content_w < NARROW_W {
        return false;
    }
    match monitor {
        Some(m) => m.x > m.y && window_w > m.x / 2.0,
        None => true,
    }
}

/// The decision behind [`AppIde::enforce_narrow_layout`], as a function of what
/// it actually depends on — separated from the app so the rule can be exercised
/// without building a whole `AppIde`.
///
/// In and out: `(mcu_collapsed, tree_collapsed, wide_layout)`.
fn narrow_layout_rule(
    room_for_three: bool,
    mcu_collapsed: bool,
    tree_collapsed: bool,
    wide: Option<(bool, bool)>,
) -> (bool, bool, Option<(bool, bool)>) {
    if room_for_three {
        // Room for everyone again: hand the user back the arrangement they had,
        // and forget it — from here on the live flags are theirs.
        return match wide {
            Some((mcu, tree)) => (mcu, tree, None),
            None => (mcu_collapsed, tree_collapsed, None),
        };
    }
    // Both open is the ONLY illegal combination. One open, or none, fits at any
    // width, and forcing anything there would take away a choice the window can
    // still honour.
    if mcu_collapsed || tree_collapsed {
        return (mcu_collapsed, tree_collapsed, wide);
    }
    // The tree wins. `wide.unwrap_or` and not an overwrite: only the FIRST
    // crossing records the wide layout — re-recording later would capture the
    // forced pair, leaving nothing to restore.
    (true, false, Some(wide.unwrap_or((false, false))))
}

#[cfg(test)]
mod narrow_layout_tests {
    use super::{NARROW_W, narrow_layout_rule, room_for_three_columns};
    use eframe::egui;

    const ROOM: bool = true;
    const CRAMPED: bool = false;

    // ── The layout rule ───────────────────────────────────────────────────────

    #[test]
    fn room_for_three_leaves_both_zones_alone() {
        assert_eq!(
            narrow_layout_rule(ROOM, false, false, None),
            (false, false, None)
        );
    }

    #[test]
    fn a_cramped_window_closes_the_mcu_zone_and_remembers() {
        // Tree wins; the pair that was open is kept for the way back.
        assert_eq!(
            narrow_layout_rule(CRAMPED, false, false, None),
            (true, false, Some((false, false)))
        );
    }

    #[test]
    fn a_cramped_window_leaves_a_legal_pair_alone() {
        // Only one open — nothing to enforce, and nothing to remember either.
        assert_eq!(
            narrow_layout_rule(CRAMPED, true, false, None),
            (true, false, None)
        );
        assert_eq!(
            narrow_layout_rule(CRAMPED, false, true, None),
            (false, true, None)
        );
        // Neither open (editor only) is legal too.
        assert_eq!(
            narrow_layout_rule(CRAMPED, true, true, None),
            (true, true, None)
        );
    }

    #[test]
    fn choosing_the_mcu_zone_while_cramped_does_not_overwrite_the_memory() {
        // The user swapped to the MCU zone; what they had while roomy must
        // survive, or widening would restore the forced layout instead.
        let remembered = Some((false, false));
        assert_eq!(
            narrow_layout_rule(CRAMPED, false, true, remembered),
            (false, true, remembered)
        );
    }

    #[test]
    fn regaining_room_restores_the_remembered_pair_and_forgets_it() {
        // Both come back even though the live flags say otherwise — including
        // after a swap made while cramped, which is as temporary as the cramped
        // layout itself.
        assert_eq!(
            narrow_layout_rule(ROOM, false, true, Some((false, false))),
            (false, false, None)
        );
    }

    // ── The verdict ───────────────────────────────────────────────────────────

    /// A 1920x1080 desktop monitor.
    const LANDSCAPE: Option<egui::Vec2> = Some(egui::Vec2::new(1920.0, 1080.0));
    /// The same panel rotated — 1080 wide is plenty of pixels, and that is the
    /// point: width alone would call this roomy.
    const PORTRAIT: Option<egui::Vec2> = Some(egui::Vec2::new(1080.0, 1920.0));

    #[test]
    fn the_floor_fits_all_three_minimums() {
        // Derived, not picked: it must be exactly what the three columns need,
        // or the rule fires at a width that would have been fine (or fails to
        // fire at one that isn't).
        assert_eq!(
            NARROW_W,
            super::EDITOR_MIN_W + super::MCU_MIN_W + super::TREE_MIN_W
        );
    }

    #[test]
    fn a_maximised_landscape_window_has_room() {
        assert!(room_for_three_columns(1900.0, 1920.0, LANDSCAPE));
    }

    #[test]
    fn a_window_docked_to_half_a_landscape_screen_does_not() {
        // The reported case. 960 px clears the arithmetic floor of 840, which is
        // exactly why the floor alone was not enough.
        assert!(!room_for_three_columns(940.0, 960.0, LANDSCAPE));
        // Exactly half is not MORE than half — the boundary belongs to the
        // two-panel side, so a window snapped to it is not treated as roomy.
        assert!(!room_for_three_columns(940.0, 1920.0 / 2.0, LANDSCAPE));
        // Past half, three columns are welcome again: the cutoff is a share of
        // the screen, not a comfort judgement.
        assert!(room_for_three_columns(1000.0, 1020.0, LANDSCAPE));
    }

    #[test]
    fn a_portrait_display_never_has_room_however_wide_the_window() {
        // Maximised on a rotated monitor: 1080 px, twice the floor, and still
        // two panels — the user asked for this explicitly.
        assert!(!room_for_three_columns(1080.0, 1080.0, PORTRAIT));
    }

    #[test]
    fn the_arithmetic_floor_still_applies_on_a_landscape_screen() {
        // Fullscreen on a small landscape display: the orientation gate passes
        // and the width one must still refuse.
        assert!(!room_for_three_columns(
            800.0,
            800.0,
            Some(egui::Vec2::new(800.0, 600.0))
        ));
    }

    #[test]
    fn an_unknown_monitor_falls_back_to_the_width_floor() {
        // Guessing an orientation would be worse than not applying the rule.
        assert!(room_for_three_columns(NARROW_W, f32::INFINITY, None));
        assert!(!room_for_three_columns(NARROW_W - 1.0, f32::INFINITY, None));
    }
}

// ── Persisted project state ───────────────────────────────────────────────────
// Everything that must survive an application restart.
// Stored via eframe's platform storage (Registry on Windows, ~/.local on Linux).

const STORAGE_KEY: &str = "embedded_ide_project_v1";

/// Byte offset in `text` of the 0-based LSP position `(line, character)`.
/// Character is treated as a char count (correct for ASCII identifiers), clamped
/// to the line's end.
fn lsp_pos_to_byte(text: &str, line: u32, character: u32) -> usize {
    let mut offset = 0usize;
    for (li, l) in text.split_inclusive('\n').enumerate() {
        if li as u32 == line {
            let mut c = 0u32;
            for (b, _) in l.char_indices() {
                if c == character {
                    return offset + b;
                }
                c += 1;
            }
            return offset + l.trim_end_matches('\n').len(); // char beyond EOL
        }
        offset += l.len();
    }
    text.len()
}

/// If a definition's absolute path points to a file in the *current project*
/// (under the LSP workspace `…/embedded_ide_0_check/`), return its editor file
/// id — so F12 can open it editable in the main editor instead of the read-only
/// snippet tab. `None` for external files (crates / std), which keep the snippet.
fn project_file_for_def(abs_path: &str, user_files: &[(String, String)]) -> Option<ProjectFileId> {
    let ws = crate::workspace::dir();
    let ws_str = ws.to_string_lossy().replace('\\', "/");
    let p = abs_path.replace('\\', "/");
    let prefix = format!("{ws_str}/");
    // Case-insensitive prefix (Windows drive-letter / temp-dir case can differ).
    let head = p.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(&prefix) {
        return None;
    }
    resolve_diag_file(&p[prefix.len()..], user_files)
}

/// Read the file a definition points to and build the view shown in the
/// "Definition" tab. The WHOLE file is included (so the user can scroll above
/// and below the target); the tab scrolls the target near the top on open.
/// `None` if unreadable.
fn build_definition_view(loc: &lsp::DefinitionLoc) -> Option<DefinitionView> {
    let content = std::fs::read_to_string(&loc.path).ok()?;
    let line_count = content.lines().count();
    if line_count == 0 {
        return None;
    }
    let target = (loc.line as usize).min(line_count - 1);
    Some(DefinitionView {
        header: format!("{}  (line {})", short_path(&loc.path), loc.line + 1),
        code: content,     // full file
        highlight: target, // the def line's index in the file (0-based)
    })
}

/// Shorten a definition path for the tab header so it's clear which crate it
/// comes from: the crate dir (the segment just before `/src/`) + the `src/…`
/// tail, e.g. `stm32f1xx-hal-0.10.0/src/gpio.rs` instead of a bare `src/gpio.rs`.
/// Falls back to the bare file name when there's no `/src/`.
fn short_path(path: &str) -> String {
    let norm = path.replace('\\', "/");
    if let Some(i) = norm.rfind("/src/") {
        let crate_dir = norm[..i].rsplit('/').next().unwrap_or("");
        let tail = &norm[i + 1..]; // "src/…"
        if crate_dir.is_empty() {
            tail.to_string()
        } else {
            format!("{crate_dir}/{tail}")
        }
    } else {
        norm.rsplit('/').next().unwrap_or(&norm).to_string()
    }
}

/// Apply LSP text edits to `text`. Edits are non-overlapping; applying them
/// back-to-front (by start position) keeps earlier offsets valid.
fn apply_text_edits(text: &str, mut edits: Vec<lsp::RenameEdit>) -> String {
    edits.sort_by(|a, b| (b.start_line, b.start_char).cmp(&(a.start_line, a.start_char)));
    let mut s = text.to_owned();
    for e in edits {
        let start = lsp_pos_to_byte(&s, e.start_line, e.start_char);
        let end = lsp_pos_to_byte(&s, e.end_line, e.end_char);
        if start <= end && end <= s.len() {
            s.replace_range(start..end, &e.new_text);
        }
    }
    s
}

/// Guarantees the async save always reports SOMETHING.
///
/// `save_in_progress` is cleared only when the shared slot holds a result, so a
/// worker that dies without filling it (panic, or a poisoned mutex on the way)
/// left the status bar stuck on "Saving…" — and a visible spinner keeps the app
/// repainting, so it also burned CPU until restart. Dropping this guard writes
/// a failure if nothing else did.
struct SaveSlotGuard(Arc<Mutex<Option<Result<String, String>>>>);

impl Drop for SaveSlotGuard {
    fn drop(&mut self) {
        // NEVER `unwrap` here: a panic inside Drop while already unwinding
        // aborts the process. A poisoned lock still has to be reported.
        let mut slot = match self.0.lock() {
            Ok(s) => s,
            Err(poisoned) => poisoned.into_inner(),
        };
        if slot.is_none() {
            *slot = Some(Err(
                "the save worker stopped unexpectedly — see the Activity tab".to_string(),
            ));
        }
    }
}

/// Milestones of one user-perceived save (all `Instant`s; durations are
/// computed FROM THE CLICK, so they overlap rather than add up). Finished —
/// and logged as "Save (wall clock)" — when the flycheck the save triggered
/// ends (diagnostics fresh), or on a 120 s timeout.
struct SaveWall {
    started: std::time::Instant,
    worker_done: Option<std::time::Instant>,
    flush_done: Option<std::time::Instant>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct PersistedState {
    /// `(path_relative_to_project_root, content)` for every user-created file —
    /// `src/app.rs`, `mw_radar/src/lib.rs`. See `paths_root_relative`.
    user_src_files: Vec<(String, String)>,
    /// Explicitly-created empty folders, project-root-relative.
    user_src_folders: Vec<String>,
    /// `false` (the serde default, i.e. state written by an older build) means
    /// the two lists above are still `src/`-relative and get lifted on load.
    /// Always written as `true`.
    #[serde(default)]
    paths_root_relative: bool,
    /// Display name of the last opened/exported project folder.
    #[serde(default)]
    project_name: Option<String>,
    /// Full filesystem path of the last opened project root (UTF-8 string).
    /// On startup the IDE reopens this folder automatically. Written only when
    /// the folder EXISTS, and it is what licenses the two lists above — see
    /// [`AppIde::save`].
    #[serde(default)]
    project_dir: Option<String>,
    /// Editor-only layout (MCU + Project panels collapsed away).
    #[serde(default)]
    side_panels_collapsed: bool,
    /// Project tree hidden. Independent of `side_panels_collapsed`: either
    /// panel can be away without the other. Serde default `false` = shown, so
    /// older state opens exactly as it did.
    #[serde(default)]
    tree_collapsed: bool,
    /// Bottom diagnostics panel reduced to its tab bar.
    #[serde(default)]
    diag_collapsed: bool,
    /// Share of the project tree's height given to the main project, above the
    /// LIBRARIES section. `0.0` (the serde default) means "never set" and is
    /// replaced by the 60% default on load.
    #[serde(default)]
    tree_split_ratio: f32,
    /// User turned OFF the full-width yellow line background for git-changed
    /// lines in the editor (keeping only the gutter bars). Serde default `false`
    /// = shown, so existing/older state keeps the current look.
    #[serde(default)]
    hide_diff_line_bg: bool,
    /// User turned OFF "open the ESP Monitor after a successful flash". Stored
    /// inverted for the same reason as `hide_diff_line_bg`: the serde default
    /// `false` must mean the ON behaviour, so older state keeps it enabled.
    #[serde(default)]
    esp_monitor_no_auto: bool,
}

impl PersistedState {
    /// Enforce the rule that the persisted project is a POINTER, not a copy:
    /// the source buffers are kept only alongside the folder they came from.
    /// Returns that folder, or `None` after clearing everything that needs one.
    ///
    /// Everything that MAKES a project — the chip's pin configuration, main.rs,
    /// Cargo.toml — is read back by [`AppIde::load_project_from_dir`], which
    /// only runs for a folder that exists. The buffers alone are half a project:
    /// restored without their folder they landed on top of whatever chip started
    /// next, giving sources from one project and a chip from another — a
    /// combination that never existed. A project moved or deleted since the last
    /// save is the same case as one never saved at all.
    ///
    /// `selected_mcu_id` is deliberately outside the rule: it is one id, it
    /// cannot contradict anything, and reopening on the chip you were working
    /// with is the one part of an unsaved session worth keeping.
    ///
    /// BOTH ends call this — `save` before writing, `AppIde::new` after reading.
    /// The read side is not redundant: state written by an older build already
    /// carries the ghost, and cleaning it only on the way out would still let
    /// one bad restore through first.
    fn drop_homeless_files(&mut self) -> Option<std::path::PathBuf> {
        let home = self
            .project_dir
            .as_deref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists());
        if home.is_none() {
            self.project_dir = None;
            self.project_name = None;
            self.user_src_files.clear();
            self.user_src_folders.clear();
        }
        home
    }
}

#[cfg(test)]
mod persisted_state_tests {
    use super::PersistedState;

    fn with_files(dir: Option<&str>) -> PersistedState {
        PersistedState {
            user_src_files: vec![("src/app.rs".to_owned(), "fn main() {}".to_owned())],
            user_src_folders: vec!["src/pins".to_owned()],
            project_name: Some("blinky".to_owned()),
            project_dir: dir.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn a_project_with_no_folder_keeps_nothing() {
        let mut s = with_files(None);
        assert_eq!(s.drop_homeless_files(), None);
        assert!(s.user_src_files.is_empty());
        assert!(s.user_src_folders.is_empty());
        assert_eq!(s.project_name, None);
    }

    #[test]
    fn a_folder_deleted_since_the_last_save_counts_as_none() {
        let mut s = with_files(Some("Z:/no/such/project/26-08-13"));
        assert_eq!(s.drop_homeless_files(), None);
        assert!(s.user_src_files.is_empty());
        // Cleared too, so the next start doesn't retry a path that is gone.
        assert_eq!(s.project_dir, None);
    }

    #[test]
    fn a_folder_that_still_exists_keeps_its_files() {
        let here = env!("CARGO_MANIFEST_DIR");
        let mut s = with_files(Some(here));
        assert_eq!(s.drop_homeless_files().as_deref(), Some(here.as_ref()));
        assert_eq!(s.user_src_files.len(), 1);
        assert_eq!(s.user_src_folders.len(), 1);
        assert_eq!(s.project_name.as_deref(), Some("blinky"));
    }
}

// ── App state ─────────────────────────────────────────────────────────────────

pub struct AppIde {
    /// All known MCU definitions (built-in + later: imported from a folder).
    mcu_registry: Vec<McuDefinition>,
    /// `id` of the currently selected MCU (key into `mcu_registry`).
    selected_mcu_id: String,
    /// None when the selected chip is not yet implemented
    mcu: Option<Mcu>,
    /// Generated Rust HAL code — rebuilt each frame from pin state
    generated_code: String,
    /// Hash of the MCU state (pins + clock + modules) used to detect changes
    /// and avoid unnecessary code regeneration.
    mcu_state_hash: u64,
    /// Editable project config files. Each holds the current content (a
    /// `<<< GENERATED >>>` block the IDE refreshes on chip change, plus whatever
    /// the user adds outside it). `memory_x`/`build_rs` are empty for ESP.
    cargo_toml: String,
    cargo_config: String,
    memory_x: String,
    build_rs: String,
    gitignore: String,
    /// Cached project files to avoid repeated cloning
    cached_project_files: Option<ProjectFiles>,
    /// Active tab in the MCU configurator
    active_tab: McuTab,
    /// Rename-project dialog: `Some(edit buffer)` while open (Tools menu).
    renaming_project: Option<String>,
    /// One-shot: focus the rename field on the dialog's first frame.
    renaming_project_focus: bool,
    /// View rect (scene coords) of the Pins canvas — chip + virtual modules.
    /// While `mcu_view_adjusted` is false it is refilled from last frame's
    /// content bounds each frame, so the canvas AUTO-FITS the panel (window /
    /// panel resizes, added modules). Once the user pans or zooms it holds the
    /// persisted view instead. See [`Self::mcu_view_adjusted`].
    mcu_scene_bounds: egui::Rect,
    /// Latches once the user pans/zooms the Pins canvas (drag, scroll,
    /// Ctrl+scroll, Ctrl+±): the view is then PERSISTED in `mcu_scene_bounds`
    /// instead of auto-fitting. Ctrl+0 (or a chip change) clears it → re-fit.
    mcu_view_adjusted: bool,
    /// Screen rect of the selected pin's function list inside the chip
    /// (`Rect::NOTHING` when no pin is selected). The list is painted inside the
    /// canvas' `Scene`, which cannot receive the wheel (we intercept it first),
    /// so the wheel handler uses this to scroll the list instead of zooming.
    /// Written each frame from `Mcu::draw`; transient.
    mcu_fn_list_rect: egui::Rect,
    /// How tall the Virtual-module panel needed to be last frame for its OPEN
    /// configs to fit: the right column's content, plus the toolbar above it.
    ///
    /// Read one frame late on purpose — a config's height is only known after
    /// it has been laid out once, and the bottom panel is built before that.
    /// Zero when nothing is open.
    vmod_needed_h: f32,
    /// Height the Virtual-modules LIST column needs to show every module at
    /// once. Measured while rendering, so it is applied one frame later — the
    /// caret button opens the panel to exactly this.
    vmod_list_h: f32,
    /// Height of the Virtual-module panel's BODY — the list and the configs,
    /// not the toolbar above them.
    ///
    /// Owned here rather than left to the panel, because an `egui` bottom panel
    /// shrinks to its content: its own resize handle can only ever cap the room
    /// the content may take, so dragging it does nothing to a body that sizes
    /// itself. This value is what the panel's drag handle actually moves.
    vmod_body_h: f32,
    /// Which modules were open last frame, as a cheap signature. The panel is
    /// resized when this CHANGES — once — so it makes room for a config you
    /// just opened and then leaves the splitter entirely to you.
    vmod_open_sig: u64,
    /// The Virtual-module panel is collapsed to its toolbar, giving the height
    /// back to the chip diagram. The bar stays visible; the caret reopens it.
    vmod_collapsed: bool,
    /// Which open module's DETAILS pane is showing, if any (its `id`).
    ///
    /// One at a time, and deliberately: the pane is a third column, so two of
    /// them would leave the configs a strip. Cleared on its own when that
    /// module is collapsed or removed — the pane is a view of a config that is
    /// on screen, not a window that outlives it.
    vmod_info_id: Option<String>,
    /// The header's "Reset pins" is ARMED and waiting for confirmation.
    ///
    /// It wipes every pin function on the chip — and with them the Virtual
    /// Modules, which `reconcile_modules` drops once their pins are gone — so it
    /// asks first, the same way removing one module does. Transient: a click
    /// anywhere else disarms it.
    reset_pins_confirm: bool,
    /// Cached module graph for the Structure tab: `(content hash, graph,
    /// layout)`. Rebuilt only when a file's content or the file list changes.
    structure_cache: Option<(
        u64,
        crate::panels::structure_map::parse::ModuleGraph,
        crate::panels::structure_map::layout::GraphLayout,
    )>,
    /// Zoom / pan state of the Structure diagram (session-only).
    structure_view: crate::panels::structure_map::gui::StructureView,
    /// The incremental cross-module call-graph pass (Phase 3) — serialized
    /// references lookups, one per top-level symbol; rebuilt on content change.
    structure_calls: Option<crate::panels::structure_map::calls::CallPass>,
    /// Node-level call-pair count the current layout was optimized with — when
    /// the finished pass yields a different set, the diagram is re-laid-out
    /// once so node ordering also minimizes call-edge crossings.
    structure_layout_calls: usize,
    /// Manually dragged Structure-diagram positions, keyed by the module's
    /// file. Applied over every automatic layout; persisted in the project's
    /// `project_structure.config` on Project Save.
    structure_overrides: crate::panels::mcu_module::structure_config::StructurePositions,
    /// The Clock tab's per-project state: dragged node positions, the last
    /// action's note, and whether the fields list is shown. Same deal as
    /// `structure_overrides` — persisted in `project_structure.config`, and
    /// deliberately NOT in `mcu.config`: none of it is configuration, so it must
    /// not show up in Git or regenerate `main.rs`.
    clock_ui: crate::panels::mcu_module::clock::gui::ClockUiState,
    /// Currently selected file in the project tree
    selected_file: ProjectFileId,
    /// Shown briefly after a successful copy
    copy_flash: u8,
    /// Master switch for the inline RA/cargo diagnostic overlay (squiggles +
    /// inline error text drawn over the code). Toggled from the editor toolbar;
    /// `true` by default. When off, the bottom-panel Cargo Check / rust-analyzer
    /// tabs still list everything — only the in-editor overlay is suppressed.
    inline_errors_enabled: bool,
    /// Show the export/save result message until this deadline. Time-based
    /// (not a frame countdown): frame cadence varies from 60+ FPS to the 4 FPS
    /// activity watchdog, so counting frames made the message's lifetime
    /// swing from ~3 s to ~45 s.
    export_status_until: Option<std::time::Instant>,
    /// `Instant` of the previous `ui()` frame — measures inter-frame gaps for
    /// the UI-stall detector (a gap while work was pending = a lost wake-up).
    last_frame_at: Option<std::time::Instant>,
    /// Whether the PREVIOUS frame had background activity (busy status bar).
    /// An inter-frame gap only matters when work was pending through it.
    was_busy_last_frame: bool,
    /// Wall-clock envelope of the current save AS THE USER PERCEIVES IT:
    /// click → project written → LSP flush done → diagnostics fresh. Logged
    /// to Activity as "Save (wall clock)" so the tab has an entry matching
    /// the felt duration, decomposed into milestones.
    save_wall: Option<SaveWall>,
    /// Last export result message
    export_msg: String,
    /// In-flight async project save: `None` until the worker finishes, then
    /// `Some(Ok(name))` / `Some(Err(msg))`. `Some(handle)` ⇒ a save is running
    /// (UI stays responsive; the header shows a "Saving…" spinner).
    #[allow(clippy::type_complexity)]
    save_in_progress: Option<Arc<Mutex<Option<Result<String, String>>>>>,
    /// Increments on every user Save. All the actions one Ctrl+S produces
    /// (project write → LSP flush → wall clock) carry this id, so the Activity
    /// tab shows ONE group per save instead of one per action.
    save_session: u64,
    /// Destination folder of the running save — becomes `project_dir` on success.
    save_dest: Option<std::path::PathBuf>,
    // ── Build ────────────────────────────────────────────────────────────────
    /// egui context stored for cross-thread repaint requests
    egui_ctx: egui::Context,
    /// Shared state written by the background build thread
    build_state: Arc<Mutex<BuildState>>,
    /// Index of the diagnostic currently expanded in the cargo build panel
    selected_diagnostic: Option<usize>,
    /// Shared state written by the background `cargo clippy` thread (reuses
    /// BuildState) + the expanded-suggestion index for the Clippy tab.
    clippy_state: Arc<Mutex<BuildState>>,
    clippy_sel: Option<usize>,
    /// Shared state of the Flash/RAM size measurement (Cargo tab's Size button:
    /// `cargo build --release` + ELF section parse — see `crate::size`).
    size_state: Arc<Mutex<crate::size::SizeState>>,
    /// `cargo bloat` code-size breakdown for the Profile tab (see
    /// `crate::profile`). `profile_by_crate` toggles per-crate vs per-function.
    profile_state: Arc<Mutex<crate::profile::ProfileState>>,
    profile_by_crate: bool,
    /// Profile-tab view: Static (cargo bloat) vs Runtime (flamegraph).
    profile_mode: crate::profile::ProfileMode,
    /// On-target flamegraph sampling state (Runtime mode; see `crate::flamegraph`).
    flame_state: Arc<Mutex<crate::flamegraph::FlameState>>,
    /// Was any flash pipeline busy last frame? Edge-detects "flash finished" to
    /// re-measure Flash/RAM automatically (see `poll_flash_finished_size`).
    flash_was_busy: bool,
    /// Which bottom tab was active last frame — edge-detects "this tab was just
    /// opened", which the Git tab uses to refresh its status automatically.
    last_build_tab: BuildPanelTab,
    /// Shared state for USB DFU detection and flashing
    dfu_state: Arc<Mutex<DfuState>>,
    /// Live output lines from the DFU flash operation (build + objcopy + dfu-util)
    dfu_log: Arc<Mutex<Vec<String>>>,
    /// Filtered list of detected USB programmers (ST-Link, J-Link, DFU, serial, …)
    // dfu_programmers: Arc<Mutex<Vec<dfu::ProgrammerInfo>>>,
    dfu_programmers: Arc<Mutex<HashMap<String, dfu::ProgrammerInfo>>>,
    /// Index of the programmer currently selected in the ComboBox
    // dfu_sel_programmer: usize,
    dfu_sel_programmer: String,
    /// Flash start address sent to dfu-util (editable; default = 0x08000000)
    dfu_flash_addr: String,
    /// Shared state for OpenOCD SWD flash operations
    openocd_state: Arc<Mutex<OpenOcdState>>,
    /// Status of a `cargo flash` (probe-rs) run — the Flash tab's probe-rs path,
    /// which shares `selected_probe` with the Debug / RTT / Runtime tabs.
    probe_flash_state: Arc<Mutex<crate::probe_flash::ProbeFlashState>>,
    /// Pid of the running `cargo flash`, so the Flash button can stop it.
    probe_flash_child: Arc<Mutex<Option<u32>>>,
    /// Target config file passed to OpenOCD (e.g. "target/stm32f1x.cfg")
    openocd_target_cfg: String,
    /// Shared state for ESP32 espflash operations
    espflash_state: Arc<Mutex<EspFlashState>>,
    /// Optional serial port override for espflash (e.g. "COM3", "/dev/ttyUSB0").
    /// Empty = auto-detect (espflash scans available ports automatically).
    espflash_port: String,
    /// Shared state for the Required Tools tab (check + install operations)
    tools_state: Arc<Mutex<required_tools::ToolsState>>,
    /// Dependency self-check: `false` until the one-shot startup scan has been
    /// kicked off (background thread — see `poll_dependency_check`).
    deps_checked: bool,
    /// The user closed the "missing dependencies" banner this session.
    deps_banner_dismissed: bool,
    /// RTT / defmt console (probe-rs pipeline + scrollback) — the RTT tab.
    rtt: crate::rtt::RttConsole,
    /// ESP device console (`espflash monitor` + scrollback) — the Monitor tab.
    /// Where `esp_println::println!` output shows up.
    esp_monitor: crate::esp_monitor::EspMonitor,
    /// Open the Monitor automatically after a successful ESP flash. Persisted —
    /// it is a workflow preference, not session state.
    esp_monitor_auto: bool,
    /// The port the last `espflash flash` actually used (its own override, or
    /// the one it auto-detected and logged). Written by the flash thread, read
    /// when the Monitor session starts so it follows the board just flashed.
    espflash_used_port: Arc<Mutex<String>>,
    /// On-target debug session (DAP client over probe-rs dap-server).
    debugger: crate::debugger::Debugger,
    /// Collapsed blocks per file: rel path → the 0-based line carrying the `{`
    /// of each folded block. View state only — never persisted, and cleared for
    /// a file the moment anything is typed into it (see
    /// [`fold`](crate::app::editor_panel::fold)).
    folds: std::collections::HashMap<String, std::collections::BTreeSet<usize>>,
    /// Set by a fold toggle: `(rel path, the block's header line, the screen y
    /// it had BEFORE the toggle)`. The next frame re-anchors the scroll offset
    /// so that line stays exactly where it was — folding 200 lines otherwise
    /// slides the whole page under the pointer.
    fold_anchor: Option<(String, usize, f32)>,
    /// Debug probes from the last `probe-rs list` scan — the shared selector on
    /// the RTT and Debug tabs (both drive probe-rs). Populated by `scan_probes`.
    probe_list: Vec<crate::probe::ProbeInfo>,
    /// The chosen `--probe VID:PID[:Serial]` selector for RTT + Debug sessions;
    /// `None` = auto-select (probe-rs picks the only attached probe). Shared by
    /// both tabs. Session-only (not persisted).
    selected_probe: Option<String>,
    /// Error from the last probe scan (e.g. probe-rs missing), shown in the tab.
    probe_scan_err: Option<String>,
    /// Result of a probe scan running on its own thread, waiting to be applied.
    /// `probe-rs list` shells out, and a busy or wedged probe makes it take far
    /// longer than the "well under a second" it usually does — long enough to
    /// freeze the UI if it ran inline, which is exactly what the automatic scan
    /// on entering the Flash tab would have done.
    probe_scan_inbox: Arc<Mutex<Option<Result<Vec<crate::probe::ProbeInfo>, String>>>>,
    /// A scan is in flight (drops overlapping requests).
    probe_scanning: bool,
    /// When the Flash tab last auto-scanned, so flipping between tabs doesn't
    /// spawn an enumeration per click.
    last_flash_autoscan: Option<std::time::Instant>,
    /// Source breakpoints per workspace-relative path (1-based lines), toggled
    /// from the editor's line-number gutter. Session-only (not persisted).
    breakpoints: std::collections::BTreeMap<String, std::collections::BTreeSet<u32>>,
    /// Code-completion engine — stores the trie, current prefix and popup state.
    /// Must live in the App (not a local) so state is preserved across frames.
    completer: Completer,
    /// True when the LSP completion popup is visible.
    completion_open: bool,
    /// Transient note shown at the cursor when a completion request came back
    /// EMPTY — a silent popup flash was undiagnosable. Carries the reason
    /// (e.g. "the file has no `mod …;` declaration") + when it appeared.
    completion_note: Option<(String, std::time::Instant)>,
    /// Index of the currently highlighted row in the completion popup.
    completion_sel: usize,
    /// Character-offset in the editor text where completion was triggered.
    /// Used to compute the live prefix for filtering and to close the popup
    /// when the cursor moves away.
    completion_trigger_idx: usize,
    /// Completion item deferred from a mouse-click on a popup row.
    /// Applied at the start of the next frame (before the editor renders);
    /// carries the whole item so snippet expansion sees `insert_is_snippet`.
    completion_pending_insert: Option<lsp::CompletionItem>,
    /// Filtered completion list from the last rendered frame.
    /// Key handlers (Tab / Enter / Arrow) use this so they always operate
    /// on the same slice the user sees, not the full unfiltered LSP list.
    completion_filtered_items: Vec<lsp::CompletionItem>,
    /// Which editor asked for the open completion — decides where the popup is
    /// anchored and, crucially, WHICH buffer an accept writes into.
    completion_owner: EditorSlot,
    /// Cargo.toml dependency-completion popup (crate names + live crates.io
    /// versions). Independent of rust-analyzer.
    cargo_complete: editor_panel::cargo_complete::CargoCompleteState,
    /// Primary caret char-index from the previous frame, used to scroll the
    /// editor so the caret stays in view when it moves off-screen (e.g.
    /// Shift+Up/Down selection past the visible area).
    last_caret_idx: Option<usize>,
    /// Pending "jump to this diagnostic": the target file and its 1-based line.
    /// Set when a row in the Cargo Check / rust-analyzer tab is clicked; applied
    /// once the editor is displaying that file (scrolls the line to row ~10).
    pending_scroll_to_line: Option<(ProjectFileId, usize)>,
    /// The file + 1-based line + band colour of the last-clicked diagnostic.
    /// Highlighted with a translucent band (colour keyed by severity, see
    /// `diag_highlight_color`) in the editor until another diagnostic is clicked.
    highlighted_error_line: Option<(ProjectFileId, usize, egui::Color32)>,
    /// The file + 1-based line of the last F12 go-to-definition that landed in a
    /// project file. Highlighted with a translucent yellow band (like the
    /// Definition tab) until the next F12.
    highlighted_def_line: Option<(ProjectFileId, usize)>,
    /// The pulsing "here is your pin" highlight, or `None` when none is running.
    highlighted_pin_lines: Option<PinHighlight>,
    /// Live "usages" analysis (fn/struct/enum/const/… fade-if-unused + a
    /// "references" popup) for whichever `.rs` file is currently displayed. See
    /// `editor_panel::usages`.
    usages: editor_panel::usages::UsagesState,
    /// Every `.rs` file's content (`"src/main.rs"` / `"src/{rel}"` → text) at
    /// the moment the last Cargo Check / Clippy run was kicked off — lets the
    /// "unused local variable" fade (which can only come from an on-demand
    /// compile, see `editor_panel::usages`) tell whether a diagnostic still
    /// matches the live text or has gone stale from a later edit.
    build_text_snapshot: HashMap<String, String>,
    /// Extra caret positions for Ctrl+Shift+Up/Down multi-cursor editing (char
    /// indices into the displayed file, in the order they were added — last
    /// added is popped first by Ctrl+Shift+Down). See `editor_panel::multi_cursor`.
    extra_cursors: Vec<editor_panel::multi_cursor::ExtraCaret>,
    /// Which file `extra_cursors` belongs to — cleared on a file switch so
    /// stale positions never leak into an unrelated file.
    extra_cursors_file: Option<ProjectFileId>,
    /// The primary caret's char index at the end of the previous frame — lets
    /// multi-cursor replay tell a Backspace (deletes BEFORE the cursor) apart
    /// from a Delete-key press (deletes AFTER it) — and, since it stores the
    /// whole `(anchor, head)` selection, typing OVER a selection apart from
    /// either, because then each caret replaces its OWN span.
    mc_prev_primary_sel: Option<(usize, usize)>,
    /// Did the code editor hold keyboard focus last frame? egui surrenders the
    /// focused widget on Escape before any of our code runs, so this is the
    /// only way to know whether the caret that just vanished was OURS — and
    /// therefore whether to take the focus back.
    editor_was_focused: bool,
    /// The SECOND (Reference) editor had keyboard focus last frame. Set where
    /// that editor renders; read by the main editor's keyboard-scope gate,
    /// which runs earlier in the frame — hence "last frame".
    reference_was_focused: bool,
    /// Ctrl+Space arrived while the Reference editor owned the keyboard.
    /// Consumed by that editor when it renders, later in the same frame.
    reference_ctrl_space: bool,
    // ── rust-analyzer LSP ────────────────────────────────────────────────────
    /// Shared LSP client state (updated from background threads)
    lsp_state: Arc<Mutex<lsp::LspState>>,
    /// Set by a Project Save (Ctrl+S / Save button / project reload). RA
    /// re-verifies (didChange + workspace disk write + didSave) ONLY when this is
    /// set — never while typing, so editing stays light. See `init_frame`.
    lsp_flush_requested: bool,
    /// True while a background LSP flush worker is running. Guards overlap: a
    /// save arriving mid-flush keeps `lsp_flush_requested` set and `init_frame`
    /// re-fires it once the worker clears this and wakes the UI.
    lsp_flush_in_flight: Arc<std::sync::atomic::AtomicBool>,
    /// One-shot guard for the post-load RA restart. On startup / project open, RA
    /// analyzes too early — the freshly-reset `Cargo.lock` is still re-resolving
    /// and late-written config files aren't indexed yet — so its first
    /// diagnostics can be stale false errors. Once RA is `Ready` and the workspace
    /// has been stable for `LSP_SETTLE`, we restart it ONCE so the status reflects
    /// the fully-resolved workspace. Reset to `false` on every project load.
    lsp_settle_recheck_done: bool,
    /// Stage of that one-shot: `false` = the cheap forced re-verify hasn't run
    /// yet, `true` = it did and diagnostics survived it, so the next settle
    /// escalates to a full restart. Reset with `lsp_settle_recheck_done`.
    lsp_settle_reverified: bool,
    /// When the RA workspace content last changed (codegen regen / project load) —
    /// the debounce baseline for the settle restart above.
    last_workspace_change: Option<std::time::Instant>,
    /// When the current RA session entered `Indexing`. Only used by the fallback
    /// that opens `src/main.rs` anyway if RA never reports indexing as finished
    /// (see the `Indexing` arm of the LSP lifecycle).
    lsp_indexing_since: Option<std::time::Instant>,
    // ── Rename symbol (Ctrl+R → textDocument/rename) ─────────────────────────
    /// While `true`, the rename input popup is shown.
    rename_active: bool,
    /// The new name being typed in the rename popup (pre-filled with the symbol).
    rename_input: String,
    /// The symbol's name BEFORE the rename, captured when the popup opens, so
    /// the applied edits can be audited for occurrences RA did not reach.
    rename_old_name: String,
    /// The name submitted in the rename popup, so leftovers can be offered the
    /// same target.
    rename_new_name: String,
    /// File + 0-based (line, char) where the rename was triggered.
    rename_rel: String,
    rename_line: u32,
    rename_char: u32,
    /// Screen position to anchor the rename popup at.
    rename_popup_pos: egui::Pos2,
    /// `true` after a rename request was sent, until RA's edits are applied.
    rename_in_flight: bool,
    // ── Code actions (Ctrl+Enter — RA assists / quick-fixes) ─────────────────
    /// `true` after a codeAction request, until the list arrives.
    code_action_in_flight: bool,
    /// `true` after a codeAction/resolve request, until its edits arrive.
    code_action_resolve_in_flight: bool,
    /// The code actions to choose from (popup shown when > 1).
    code_actions: Vec<lsp::CodeAction>,
    /// Whether the chooser popup is open.
    code_action_popup_open: bool,
    /// Highlighted row in the chooser popup.
    code_action_sel: usize,
    /// Screen anchor for the chooser popup (the cursor rect when triggered).
    code_action_popup_pos: egui::Pos2,
    /// Chooser selection deferred to next frame's `init_frame` (so the edit
    /// applies at frame TOP, avoiding the display_code write-back revert).
    code_action_choice: Option<usize>,
    // ── Inline type hints (inferred type on the cursor's `let` line) ──────────
    /// Master switch for the cursor-line inferred-type ghost hint + its Tab
    /// accept; toggled from the editor toolbar ("Types" button). `true` default.
    inlay_types_enabled: bool,
    /// The inferred-type hint to draw as ghost text after an untyped `let` on
    /// the cursor's line, if any (its `text_edits` insert the type on Tab).
    /// Cleared when the caret leaves an untyped `let`.
    inlay_hint: Option<lsp::InlayHint>,
    /// `(rel_path, 0-based line)` the last inlay request was fired for — so we
    /// re-request when the caret moves to a different `let` line, or after RA
    /// re-syncs (the request key is reset while the file is dirty).
    inlay_requested: Option<(String, u32)>,
    /// Set when Tab is pressed while the ghost hint shows; the type is inserted
    /// at frame TOP next `init_frame` (like code actions, to dodge the revert).
    inlay_accept_pending: bool,
    /// `true` when the in-flight rename came from a Clippy "Rename" button — once
    /// RA's edits land, clippy is re-run so its (now-stale) list refreshes.
    clippy_rename_pending: bool,
    /// Remaining renames to apply one-by-one (Clippy "Apply all" batches them, and
    /// a single "Rename" enqueues one). Each is fired only after the previous one's
    /// edits land; a queued entry is skipped if its position no longer matches its
    /// `old_name` (a prior edit shifted it — it'll resurface on the final re-run).
    clippy_rename_queue: std::collections::VecDeque<crate::build::RenameFix>,
    /// Request keyboard focus for the rename input on the frame it opens.
    rename_focus: bool,
    /// Show the full-width translucent-yellow line background for git-changed
    /// lines in the editor. When off, only the gutter bars (green/amber/red)
    /// remain — the band was distracting while editing. Persisted (inverted, as
    /// `hide_diff_line_bg`). Toggled from the editor toolbar.
    diff_line_bg: bool,
    // ── Find / Replace (Ctrl+F / Ctrl+H / Ctrl+Shift+F / Ctrl+Shift+H) ───────
    /// Search bar state: mode, query/replacement text, results, match cursor.
    find: editor_panel::find_replace::FindReplace,
    /// Code-editor font size in points, zoomed with Ctrl + `+`/`-`/`0`.
    editor_font_size: f32,
    /// Full-definition highlight set by a triple-click on a `{`/`}` — `(file,
    /// start, close)` inclusive char range, kept until the selection changes.
    full_block_selection: Option<(ProjectFileId, usize, usize)>,
    // ── Serial monitor (built-in USART/UART console) ─────────────────────────
    serial: crate::serial::SerialMonitor,
    // ── Terminal (built-in streaming command console) ────────────────────────
    terminal: crate::terminal::TerminalConsole,
    // ── Git (commit/push/pull in the project directory) ──────────────────────
    git: crate::git::GitConsole,
    /// Editor gutter diff (live in-memory text vs HEAD) + revert-hunk state.
    diff_gutter: editor_panel::diff_gutter::DiffGutter,
    // ── Activity log (per-Save/Build/Flash timing breakdown) ─────────────────
    activity: Arc<Mutex<crate::activity::ActivityLog>>,
    /// MRU file-switch history + active Ctrl+Tab cycling session
    /// (see `editor_panel::file_cycle`).
    file_cycle: editor_panel::file_cycle::FileCycle,
    /// `selected_file` as of the previous frame — the change detector that
    /// feeds `file_cycle` regardless of what caused the switch (tree click,
    /// F12, diagnostics nav, …).
    last_selected_file: ProjectFileId,
    /// Hash of the content last written to each file in the RA workspace
    /// (`src/<rel>` → content hash). Lets the LSP flush skip the per-file disk
    /// `fs::read` for unchanged files — those reads contended with
    /// rust-analyzer's flycheck reading the same files and grew over a session
    /// (observed as the "write files to RA workspace" phase climbing to 100ms+).
    /// The IDE is the only writer of that workspace, so the cache is
    /// authoritative. Shared with the flush WORKER thread (`spawn_lsp_flush`);
    /// `lsp_flush_in_flight` keeps two flushes from racing it.
    flushed_hashes: Arc<Mutex<std::collections::HashMap<String, u64>>>,
    // ── Go to definition (F12 → textDocument/definition) ─────────────────────
    /// `true` after an F12 request, until the definition arrives.
    definition_in_flight: bool,
    /// A Go-to-definition the user asked for while rust-analyzer could not
    /// answer it. Held until the analyzer is genuinely usable, then re-issued —
    /// see [`AppIde::poll_pending_goto`].
    pending_goto: Option<PendingGoto>,
    /// One-shot: scroll the Definition tab to the highlighted line on the first
    /// render after a new F12 snippet loads (then the user scrolls freely).
    def_scroll_pending: bool,
    /// The fetched definition snippet — its presence shows the "Definition" tab
    /// in the MCU Configurator (next to Structure).
    definition_view: Option<DefinitionView>,
    /// Project-relative path of a second file shown READ-ONLY in the
    /// "Reference" tab. Opened from the project tree; independent of
    /// `selected_file`, so the editor keeps whatever it had.
    ///
    /// A PATH, not a `user_src_files` index: indices shift when a file is
    /// deleted, which would silently point this at the WRONG file. A path
    /// either resolves or doesn't.
    reference_file: Option<String>,
    /// MCU tab to return to when the Definition tab closes / clears.
    definition_return_tab: McuTab,
    /// Last active tab of each tab-bar group (two-level navigation): clicking
    /// the "MCU" / "Project" group header returns to that group's last tab.
    mcu_group_last: McuTab,
    project_group_last: McuTab,
    /// Which tab is active in the bottom diagnostics panel
    build_tab: BuildPanelTab,
    /// Index of the RA diagnostic row that is expanded
    lsp_selected_diagnostic: Option<usize>,
    // ── Diagnostics panel layout ─────────────────────────────────────────────
    /// Height of the bottom diagnostics panel in pixels — persisted so the
    /// user's drag position is remembered across show/hide cycles.
    diag_panel_height: f32,
    // ── User source files ─────────────────────────────────────────────────────
    /// Project tree state (files and folders in src/)
    project_tree: ProjectTreeState,
    /// While `Some(s)`, a text-input for naming a new file is shown in the tree.
    new_src_name: Option<String>,
    /// While `Some(s)`, a text-input for naming a new folder is shown in the tree.
    new_src_folder_name: Option<String>,
    /// Parent folder path when creating a new file (e.g., "src" or "src/utils").
    /// If empty, creates in src/ root. For project-level files, would use "".
    new_file_parent_folder: Option<String>,
    /// Parent folder path when creating a new folder (e.g., "src" or "src/utils").
    new_folder_parent_folder: Option<String>,
    /// While `Some((folder, name))`, an inline file-name input is open inside that folder.
    new_file_in_folder: Option<(String, String)>,
    /// While `Some((idx, new_name))`, an inline rename input is shown for that user file.
    renaming_file: Option<(usize, String)>,
    /// While `Some((old_folder, new_name))`, an inline rename input is shown for that folder.
    renaming_folder: Option<(String, String)>,
    // ── Project management ────────────────────────────────────────────────────
    /// `true` while the "New Project" confirmation dialog is open.
    confirm_new_project: bool,
    /// Chip `id` staged inside the "New Project" popup.
    /// `None` = "Empty" (no chip change on confirm).
    pending_mcu_id: Option<String>,
    /// The selected chip's HAL-feature verdict, looked up OFF the UI thread.
    ///
    /// `(chip id, slot)`; `None` inside the slot means the answer has not landed
    /// yet. The lookup hits the crates.io index and takes up to four seconds, so
    /// doing it inline would freeze the app on every chip pick — and doing it in
    /// the `ui` function would do that on every repaint. Same shape as
    /// `ensure_version_fetch`, for the same reason.
    hal_check: Option<(
        String,
        std::sync::Arc<std::sync::Mutex<Option<dialogs::FeatureVerdict>>>,
    )>,
    /// `(chip id, its gaps)` for the chip staged in the New Project dialog.
    ///
    /// Cached on the id because working it out clones the chip's whole clock
    /// graph, and the dialog redraws every frame it is open.
    new_project_gaps: Option<(String, Vec<String>)>,
    /// The New Project chip-search field: its query, the catalogue of vendor
    /// chips on this machine, and the worker indexing it.
    chip_search: chip_search_ui::ChipSearchState,
    /// Last "Import MCU…" result message shown in the New Project popup
    /// (`✔ …` on success, `✗ …` on failure). Cleared when the popup closes.
    mcu_import_status: Option<String>,
    /// `Some` while the "New MCU definition" form is open (the editable chip
    /// authoring dialog — see `app::mcu_form_dialog`). Session-only.
    mcu_form: Option<crate::panels::mcu_module::mcu_form::McuForm>,
    /// Transient result of the New-MCU form's Import/Export clock buttons.
    /// `Ok` = a green confirmation, `Err` = a red parse/validation error.
    mcu_form_clock_note: Option<Result<String, String>>,
    /// New/Edit MCU definition window maximized vs normal size.
    mcu_form_maximized: bool,
    mcu_form_prev_maximized: bool,
    /// `false` until the form window has rendered once this open — forces its
    /// default size on the first frame so it never reopens huge.
    mcu_form_shown_once: bool,
    /// `Some` while the "Import from datasheet (AI)" sub-dialog of the MCU form
    /// is open (see `app::datasheet_import_dialog`). Session-only.
    datasheet_import: Option<datasheet_import_dialog::DatasheetImport>,
    /// "Extract clock tree from datasheet (AI)" sub-dialog (clock Layer 3).
    clock_import: Option<clock_import_dialog::ClockImport>,
    /// `Some((git path, is_untracked))` while the Git "discard file" confirm
    /// dialog is open (Phase A). Session-only.
    git_discard_confirm: Option<(String, bool)>,
    /// A whole-file discard the user CONFIRMED — applied at the top of the next
    /// editor render (so `display_code` refreshes; see `diag_embed`).
    pending_discard_file: Option<String>,
    /// `true` while the "Discard ALL changes" confirm dialog is open (Phase C).
    git_discard_all_confirm: bool,
    /// `(sha, path)` awaiting confirmation for History's "Restore this file".
    git_restore_confirm: Option<(String, String)>,
    /// Sha awaiting confirmation for History's "Restore ALL files".
    git_restore_all_confirm: Option<String>,
    /// Branch name awaiting confirmation for a header-picker switch (only used
    /// when there are unsaved editor changes the disk reload would discard).
    git_switch_confirm: Option<String>,
    /// Branch name awaiting confirmation for a picker `git branch -D` delete.
    git_delete_branch_confirm: Option<String>,
    /// Confirmed restore, applied at the next editor render so `display_code`
    /// refreshes with it (same deferral as `pending_discard_file`).
    pending_restore: Option<(String, String)>,
    /// Collapse the MCU Configurator (Pins / Clock / Structure …) so the editor
    /// takes the central slot (toggled from the editor toolbar). Persisted.
    side_panels_collapsed: bool,
    /// Hide the Project tree panel on the far right (toggled from the editor
    /// toolbar, next to the MCU toggle). Persisted across restarts.
    ///
    /// Orthogonal to `side_panels_collapsed`: the tree is a SIDE panel in every
    /// state, so which panel takes egui's central slot never depends on it.
    tree_collapsed: bool,
    /// The `(side_panels_collapsed, tree_collapsed)` pair from before the window
    /// stopped having room for three columns, put back when it regains it — see
    /// [`AppIde::enforce_narrow_layout`]. `None` while there IS room and the
    /// user's own arrangement is the live one.
    ///
    /// Transient by design: a layout the user never chose must not outlive the
    /// window size that forced it, which is also why `save` persists THIS pair
    /// in preference to the live flags.
    wide_layout: Option<(bool, bool)>,
    /// No room for three columns right now — see [`room_for_three_columns`],
    /// which weighs the display's orientation as well as the width. Recomputed
    /// every frame by [`AppIde::enforce_narrow_layout`]; the toolbar reads it so
    /// its two toggles behave — and read — as a radio pair rather than two
    /// independent checkboxes.
    layout_narrow: bool,

    /// The Peripherals tab's search box. Transient: a filter is a lens on the
    /// question you are asking now, not a property of the project, and one
    /// restored from disk would hide half the chip for no visible reason.
    peripheral_query: String,
    /// Width the project tree had last frame, `0.0` while it is collapsed. The
    /// editor's width cap needs it, and the editor is built BEFORE the tree —
    /// so it reads the previous frame's value. One frame of lag while dragging
    /// the tree's edge, which is not perceptible; the alternative is assuming
    /// [`TREE_MIN_W`] and letting a widened tree starve the MCU zone anyway.
    tree_width: f32,
    /// Bottom diagnostics panel reduced to its tab bar (toggled by the caret
    /// button right of "More"). The bar itself always stays visible — only the
    /// tab CONTENT is hidden. Persisted across restarts.
    diag_collapsed: bool,
    /// Share of the project tree's height for the main project vs LIBRARIES,
    /// dragged via the splitter between them. Persisted across restarts.
    tree_split_ratio: f32,
    /// Open "Extract to library crate" dialog, if any.
    extract_crate: Option<extract_crate_dialog::ExtractCrateDialog>,
    /// Open "Clone a library from git" dialog, if any.
    clone_library_dialog: Option<extract_crate_dialog::CloneLibraryDialog>,
    /// "Clone project" modal (duplicate the whole project + libraries to a new
    /// folder). See [`clone_project_dialog`].
    clone_project_dialog: Option<clone_project_dialog::CloneProjectDialog>,
    /// Open delete/rename confirmation for a library crate, if any.
    library_action: Option<extract_crate_dialog::LibraryActionDialog>,
    /// In-flight "Add to workspace" cargo-metadata pre-check (see
    /// `add_detached_lib_to_workspace`). `None` when idle.
    workspace_add: Option<project_io::WorkspaceAdd>,
    /// A pre-check that FAILED — shown as a modal so the user sees the cargo
    /// error instead of a silently-broken workspace. `(dir, error)`.
    workspace_add_error: Option<(String, String)>,
    /// A detected workspace-load failure (our own `cargo metadata` health check
    /// found the project won't load) — surfaced as a banner instead of a mystery
    /// stuck "Checking…". Holds the cargo error text.
    workspace_load_error: Option<String>,
    /// In-flight `cargo metadata` health check (project open / after a detach).
    /// `None` while idle; posts `Ok(())` / `Err(msg)` into the inner slot.
    workspace_health: Option<std::sync::Arc<std::sync::Mutex<Option<Result<(), String>>>>>,
    /// `true` while the "unsaved changes" prompt is up (close was cancelled).
    exit_prompt: bool,
    /// Set once the user has decided, so the close we send isn't intercepted
    /// again by our own handler.
    allow_close: bool,
    /// Close the window as soon as the in-flight Save finishes.
    close_after_save: bool,
    /// `true` while the same "unsaved changes" prompt is up for **Open Project**
    /// (the click was intercepted). Opening replaces everything in memory, so it
    /// is as destructive as closing.
    open_prompt: bool,
    /// Run the Open-Project folder picker as soon as the in-flight Save finishes.
    open_after_save: bool,
    /// Same gate in front of **New Project**, which clears every user file.
    new_prompt: bool,
    /// Open the New Project dialog as soon as the in-flight Save finishes.
    new_after_save: bool,
    /// A Save requested from somewhere other than the toolbar (the exit
    /// prompt); OR-ed into `save_clicked` for one frame.
    request_save: bool,
    /// The dependency fingerprint (`project_gen::deps_fingerprint`) at the last
    /// Save / project load. When a Save finds it changed — a library was
    /// added/edited/removed in Cargo.toml, by the user or the codegen — the IDE
    /// auto-runs `cargo check` so the new deps resolve + compile. `None` before
    /// any project is loaded (no auto-build on the very first Save).
    last_saved_deps: Option<String>,
    /// Display name of the last opened/exported project (shown in the panel heading).
    project_name: Option<String>,
    /// Full path to the last opened project root folder.
    /// Persisted so the IDE can reopen it automatically on next startup.
    project_dir: Option<std::path::PathBuf>,
    // ── Filesystem watcher ────────────────────────────────────────────────────
    /// Receives filesystem events from the background notify thread.
    /// Polled every frame; used to detect files created/removed by external tools.
    fs_rx: Option<std::sync::mpsc::Receiver<notify::Result<notify::Event>>>,
    /// Kept alive so the watcher thread lives as long as the app.
    _fs_watcher: Option<notify::RecommendedWatcher>,

    /// A project chosen from "Open Recent", waiting for the unsaved-changes
    /// gate. Consumed by `pick_and_open_project`, which opens the folder picker
    /// only when this is empty.
    pending_open_dir: Option<std::path::PathBuf>,

    /// `Some` while the startup picker is up (see [`startup_picker`]) — the
    /// window has nothing open and the user is choosing what it should be.
    startup_picker: Option<startup_picker::StartupPicker>,

    /// Set by code that runs OUTSIDE the frame's `save_project_needed` local
    /// (the startup picker) to ask for the same workspace rewrite. OR-ed into
    /// that flag for one frame.
    workspace_write_requested: bool,

    /// A `--project` argument that could not be used (missing folder, no
    /// `Cargo.toml`). Shown as a banner and dismissible: a bad argument must
    /// not stop the IDE from starting, but it must not be swallowed either —
    /// the window would otherwise open on a different project with no
    /// explanation.
    cli_project_error: Option<String>,

    /// The last title sent to the window, so the viewport command fires only
    /// when it actually changes — the title is recomputed every frame, and
    /// pushing it to the OS 60 times a second would be pure waste.
    window_title: String,

    // ── Project-folder claim ──────────────────────────────────────────────────
    /// This instance's claim on the open project's FOLDER, held for as long as
    /// that project is open (see [`crate::workspace::claim_project`]). Isolating
    /// the scratch workspace keeps two windows from corrupting each other's
    /// generated project; this covers the other half — the same project opened
    /// twice, where both windows save over the user's real files.
    project_lock: Option<crate::workspace::ProjectLock>,
    /// Which recent-list entry is one click away from being forgotten, and
    /// since when — see [`helpers::forget_button`]. One slot for BOTH offers
    /// (the startup picker and the Open Recent menu): they are the same list,
    /// and two independent arms could leave one of them loaded out of sight.
    recent_forget_confirm: helpers::forget_button::Armed,
    /// `Some(name)` while the open project is claimed by ANOTHER window: the
    /// project is loaded anyway (refusing would be worse than warning), with a
    /// banner up. Cleared as soon as a retry succeeds.
    project_lock_conflict: Option<String>,
    /// Throttle for the background re-claim — the banner must clear itself when
    /// the other window closes, without probing the filesystem every frame.
    project_lock_retry: Option<std::time::Instant>,

    // ── Project-switch overlay ────────────────────────────────────────────────
    /// `Some` while the full-window "loading the project" overlay is up (see
    /// [`loading_overlay`]). Armed by every project-change entry point; lifts
    /// itself once the load chain (workspace write → RA index → check) goes
    /// quiet.
    project_loading: Option<loading_overlay::ProjectLoading>,
}

impl AppIde {
    /// Build the app.
    ///
    /// `cli_project` is the folder named on the command line, which OUTRANKS the
    /// project this instance's storage remembers. Without it, which project a
    /// window opens is decided by launch order (the slot picks the storage, the
    /// storage remembers a project), so the second window reopens whatever the
    /// second window last had — surprising every time. `cli_error` carries an
    /// argument that could not be used, to be surfaced once the UI exists.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        cli_project: Option<std::path::PathBuf>,
        cli_error: Option<String>,
    ) -> Self {
        // Always start maximized. `with_maximized(true)` on the viewport plus
        // `forget_window_geometry` in `main` normally settle this before the
        // window is ever shown; this is the belt-and-braces for the case where
        // the storage file could not be parsed, and costs one frame at the old
        // size instead of a session at it.
        cc.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::Maximized(true));

        // ── Dark IDE theme ────────────────────────────────────────────────────
        apply_dark_theme(&cc.egui_ctx);

        // Load Phosphor icon font alongside egui's default fonts
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        cc.egui_ctx.set_fonts(fonts);

        // Mouse wheel: by default egui makes Shift+wheel scroll *horizontally*,
        // so holding Shift (e.g. while selecting) and scrolling did nothing
        // vertically. Drop Shift as the horizontal-scroll modifier so Shift+wheel
        // scrolls up/down like a plain wheel (used to reach off-screen text while
        // selecting). Horizontal scrolling stays available via the scrollbar.
        cc.egui_ctx.options_mut(|o| {
            o.input_options.horizontal_scroll_modifier = egui::Modifiers::NONE;
            // Repurpose Ctrl + `+`/`-`/`0` to zoom the CODE EDITOR text (handled in
            // the editor panel) instead of egui's global UI zoom.
            o.zoom_with_keyboard = false;
        });

        // Steady (non-blinking) text carets everywhere. The editor also paints
        // its primary caret itself (`paint_primary_caret`) because egui hides
        // the caret whenever `input.focused` is false — and that OS-window
        // focus flag goes stale on Windows when a `Focused(true)` event is
        // missed (app start, Alt+Tab), leaving typing functional but the caret
        // invisible. A steady caret keeps the two paints indistinguishable.
        cc.egui_ctx
            .global_style_mut(|s| s.visuals.text_cursor.blink = false);

        // ── Load persisted project state ─────────────────────────────────────
        let mut persisted: PersistedState = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        // Hygiene: earlier builds' fs watcher pushed directory-create events
        // into `user_src_files` as phantom ("folder", "") FILE entries, and
        // eframe persistence kept them across restarts — where they shadowed
        // the real folder node in the tree. Drop any "file" whose path is a
        // tracked folder.
        {
            let folders = persisted.user_src_folders.clone();
            persisted
                .user_src_files
                .retain(|(p, _)| !folders.contains(p));
        }

        // Drop source buffers that have no project folder to belong to (see
        // `drop_homeless_files`) — including state an older build wrote before
        // the rule existed. What's left is the folder to reopen, if any.
        let mut saved_project_dir = persisted.drop_homeless_files();

        // A project named on the command line wins over the remembered one. The
        // buffers restored above belong to THAT project, so they go too — the
        // load below rebuilds the tree from the chosen folder, and keeping them
        // would mix two projects' files in one window.
        if let Some(dir) = &cli_project {
            if saved_project_dir.as_deref() != Some(dir.as_path()) {
                persisted.user_src_files.clear();
                persisted.user_src_folders.clear();
            }
            saved_project_dir = Some(dir.clone());
        }

        // Load the MCU registry: bundled built-ins + any user `.ron` imports
        // from the per-user `mcus/` folder (Phase 5 — runtime import).
        let mcu_registry = registry::load_registry();
        // ── No chip until one is asked for ───────────────────────────────────
        // A restored project names its own (`load_project_from_dir` reads the
        // marker in main.rs); with no project there is nothing to infer one
        // from, and inventing one is not harmless — a selected chip means a
        // generated `main.rs`, a Cargo.toml and a memory.x, i.e. a whole
        // project's worth of code the user never asked for. Earlier builds
        // defaulted to the STM32F103 and then persisted the last chip; both
        // produced exactly that.
        //
        // The empty state is representable throughout: `selected_def()`,
        // `selected_build_cfg()` and `has_project()` all key off this id, and
        // every MCU tab has a no-chip branch (`show_no_mcu_notice`) that offers
        // the picker.
        let selected_mcu_id = String::new();
        let mcu: Option<Mcu> = None;
        let generated_code = String::new();
        let init_files = ProjectFiles::default();

        // ── Start filesystem watcher on the build workspace src/ dir ─────────
        // The watcher runs on a background thread and sends events through a
        // channel.  We poll the channel each frame (non-blocking).
        let workspace_src = crate::workspace::dir().join("src");
        let (fs_tx, fs_rx) = std::sync::mpsc::channel();
        let ctx_clone = cc.egui_ctx.clone();
        let mut watcher = notify::recommended_watcher(move |ev| {
            let _ = fs_tx.send(ev);
            ctx_clone.request_repaint(); // wake the UI thread on fs event
        });
        if let Ok(ref mut w) = watcher {
            // Watch even if the dir doesn't exist yet — we'll re-watch after
            // write_project creates it for the first time.
            if workspace_src.exists() {
                let _ = w.watch(&workspace_src, notify::RecursiveMode::Recursive);
            }
        }

        // ── USB DFU state — created before Self so we can start monitoring ──
        let dfu_state: Arc<Mutex<DfuState>> = Arc::new(Mutex::new(DfuState::Idle));
        let dfu_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let dfu_programmers: Arc<Mutex<HashMap<String, dfu::ProgrammerInfo>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let openocd_state: Arc<Mutex<OpenOcdState>> = Arc::new(Mutex::new(OpenOcdState::Idle));
        let probe_flash_state = Arc::new(Mutex::new(crate::probe_flash::ProbeFlashState::Idle));
        let espflash_state: Arc<Mutex<EspFlashState>> = Arc::new(Mutex::new(EspFlashState::Idle));

        // Scan immediately on startup (non-blocking — runs in background thread)
        dfu::detect_dfu(
            Arc::clone(&dfu_state),
            Arc::clone(&dfu_log),
            Arc::clone(&dfu_programmers),
            cc.egui_ctx.clone(),
        );
        // Start persistent USB hotplug monitor: re-scans every 2-4 s automatically
        dfu::start_usb_monitor(Arc::clone(&dfu_state), cc.egui_ctx.clone());

        let mut app = Self {
            mcu_registry,
            selected_mcu_id,
            generated_code,
            mcu_state_hash: 0,
            cargo_toml: init_files.cargo_toml,
            cargo_config: init_files.cargo_config,
            memory_x: init_files.memory_x,
            build_rs: init_files.build_rs,
            gitignore: init_files.gitignore,
            cached_project_files: None,
            mcu,
            active_tab: McuTab::Pins,
            renaming_project: None,
            renaming_project_focus: false,
            mcu_scene_bounds: egui::Rect::NOTHING,
            mcu_view_adjusted: false,
            mcu_fn_list_rect: egui::Rect::NOTHING,
            vmod_needed_h: 0.0,
            vmod_list_h: 0.0,
            vmod_body_h: 150.0,
            vmod_open_sig: 0,
            vmod_collapsed: false,
            vmod_info_id: None,
            reset_pins_confirm: false,
            structure_cache: None,
            structure_view: Default::default(),
            structure_calls: None,
            structure_layout_calls: 0,
            structure_overrides: Default::default(),
            clock_ui: Default::default(),
            selected_file: ProjectFileId::MainRs,
            copy_flash: 0,
            inline_errors_enabled: true,
            export_status_until: None,
            last_frame_at: None,
            was_busy_last_frame: false,
            save_wall: None,
            export_msg: String::new(),
            save_in_progress: None,
            save_session: 0,
            save_dest: None,
            egui_ctx: cc.egui_ctx.clone(),
            build_state: Arc::new(Mutex::new(BuildState::Idle)),
            clippy_state: Arc::new(Mutex::new(BuildState::Idle)),
            size_state: Arc::new(Mutex::new(crate::size::SizeState::Idle)),
            profile_state: Arc::new(Mutex::new(crate::profile::ProfileState::Idle)),
            profile_by_crate: false,
            profile_mode: crate::profile::ProfileMode::Static,
            flame_state: Arc::new(Mutex::new(crate::flamegraph::FlameState::Idle)),
            flash_was_busy: false,
            last_build_tab: BuildPanelTab::RustAnalyzer,
            clippy_sel: None,
            selected_diagnostic: None,
            dfu_state,
            dfu_log,
            dfu_programmers,
            dfu_sel_programmer: "".to_owned(),
            // dfu_sel_programmer: 0,
            dfu_flash_addr: "0x08000000".to_string(),
            openocd_state,
            probe_flash_state,
            probe_flash_child: Arc::new(Mutex::new(None)),
            openocd_target_cfg: "target/stm32f1x.cfg".to_string(),
            espflash_state,
            espflash_port: String::new(),
            tools_state: required_tools::make_tools_state(),
            deps_checked: false,
            deps_banner_dismissed: false,
            rtt: crate::rtt::RttConsole::default(),
            esp_monitor: crate::esp_monitor::EspMonitor::default(),
            // On by default: an ESP project is flashed to see what it prints.
            esp_monitor_auto: !persisted.esp_monitor_no_auto,
            espflash_used_port: Arc::new(Mutex::new(String::new())),
            debugger: crate::debugger::Debugger::default(),
            folds: std::collections::HashMap::new(),
            fold_anchor: None,
            probe_list: Vec::new(),
            selected_probe: None,
            probe_scan_err: None,
            probe_scan_inbox: Arc::new(Mutex::new(None)),
            probe_scanning: false,
            last_flash_autoscan: None,
            breakpoints: std::collections::BTreeMap::new(),
            // Completer: seeded with Rust keywords/types + learns words from code
            completer: Completer::new_with_syntax(&Syntax::rust())
                .with_auto_indent()
                .with_user_words(),
            completion_open: false,
            completion_note: None,
            completion_sel: 0,
            completion_trigger_idx: 0,
            completion_pending_insert: None,
            completion_filtered_items: Vec::new(),
            completion_owner: EditorSlot::Main,
            cargo_complete: editor_panel::cargo_complete::CargoCompleteState::default(),
            last_caret_idx: None,
            pending_scroll_to_line: None,
            highlighted_error_line: None,
            highlighted_def_line: None,
            highlighted_pin_lines: None,
            usages: editor_panel::usages::UsagesState::default(),
            build_text_snapshot: HashMap::new(),
            extra_cursors: Vec::new(),
            extra_cursors_file: None,
            mc_prev_primary_sel: None,
            editor_was_focused: false,
            reference_was_focused: false,
            reference_ctrl_space: false,
            lsp_state: Arc::new(Mutex::new(lsp::LspState::default())),
            lsp_flush_requested: false,
            lsp_settle_recheck_done: true,
            lsp_settle_reverified: false,
            last_workspace_change: None,
            lsp_indexing_since: None,
            lsp_flush_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            rename_active: false,
            rename_input: String::new(),
            rename_old_name: String::new(),
            rename_new_name: String::new(),
            rename_rel: String::new(),
            rename_line: 0,
            rename_char: 0,
            rename_popup_pos: egui::Pos2::ZERO,
            rename_in_flight: false,
            code_action_in_flight: false,
            code_action_resolve_in_flight: false,
            code_actions: Vec::new(),
            code_action_popup_open: false,
            code_action_sel: 0,
            code_action_popup_pos: egui::Pos2::ZERO,
            code_action_choice: None,
            inlay_types_enabled: true,
            inlay_hint: None,
            inlay_requested: None,
            inlay_accept_pending: false,
            clippy_rename_pending: false,
            clippy_rename_queue: std::collections::VecDeque::new(),
            rename_focus: false,
            find: editor_panel::find_replace::FindReplace::default(),
            editor_font_size: editor_panel::DEFAULT_EDITOR_FONT_SIZE,
            full_block_selection: None,
            serial: crate::serial::SerialMonitor::default(),
            terminal: crate::terminal::TerminalConsole::default(),
            git: crate::git::GitConsole::default(),
            diff_gutter: editor_panel::diff_gutter::DiffGutter::default(),
            activity: Arc::new(Mutex::new(crate::activity::ActivityLog::default())),
            flushed_hashes: Arc::new(Mutex::new(std::collections::HashMap::new())),
            file_cycle: editor_panel::file_cycle::FileCycle::default(),
            last_selected_file: ProjectFileId::MainRs,
            definition_in_flight: false,
            pending_goto: None,
            def_scroll_pending: false,
            definition_view: None,
            reference_file: None,
            definition_return_tab: McuTab::Pins,
            mcu_group_last: McuTab::Pins,
            project_group_last: McuTab::Structure,
            build_tab: BuildPanelTab::RustAnalyzer,
            lsp_selected_diagnostic: None,
            diag_panel_height: 180.0,
            project_tree: ProjectTreeState {
                // `.replace`: buffers persisted by older builds may carry CRLF
                // read straight from a Windows checkout; in-memory text must be
                // pure LF (see `scan_src_dir`) or the git gutter shows a
                // permanent phantom diff.
                user_src_files: migrate_to_root_relative(
                    persisted
                        .user_src_files
                        .into_iter()
                        .map(|(p, c)| (p, c.replace("\r\n", "\n")))
                        .collect(),
                    persisted.paths_root_relative,
                ),
                user_src_folders: migrate_folders_to_root_relative(
                    persisted.user_src_folders,
                    persisted.paths_root_relative,
                ),
            },
            new_src_name: None,
            new_src_folder_name: None,
            new_file_parent_folder: None,
            new_folder_parent_folder: None,
            new_file_in_folder: None,
            renaming_file: None,
            renaming_folder: None,
            confirm_new_project: false,
            pending_mcu_id: None,
            hal_check: None,
            new_project_gaps: None,
            chip_search: Default::default(),
            mcu_import_status: None,
            mcu_form: None,
            mcu_form_clock_note: None,
            mcu_form_maximized: false,
            mcu_form_prev_maximized: false,
            mcu_form_shown_once: false,
            datasheet_import: None,
            clock_import: None,
            git_discard_confirm: None,
            pending_discard_file: None,
            git_discard_all_confirm: false,
            git_restore_confirm: None,
            git_restore_all_confirm: None,
            git_switch_confirm: None,
            git_delete_branch_confirm: None,
            pending_restore: None,
            side_panels_collapsed: persisted.side_panels_collapsed,
            tree_collapsed: persisted.tree_collapsed,
            wide_layout: None,
            layout_narrow: false,
            peripheral_query: String::new(),
            tree_width: 0.0,
            diag_collapsed: persisted.diag_collapsed,
            diff_line_bg: !persisted.hide_diff_line_bg,
            // 0.0 = absent from an older build's state; clamp keeps a corrupt
            // value from collapsing one half to nothing.
            tree_split_ratio: if persisted.tree_split_ratio <= 0.0 {
                crate::project_tree::gui::DEFAULT_SPLIT_RATIO
            } else {
                persisted.tree_split_ratio.clamp(0.15, 0.85)
            },
            extract_crate: None,
            clone_library_dialog: None,
            clone_project_dialog: None,
            library_action: None,
            workspace_add: None,
            workspace_add_error: None,
            workspace_load_error: None,
            workspace_health: None,
            exit_prompt: false,
            allow_close: false,
            close_after_save: false,
            open_prompt: false,
            open_after_save: false,
            new_prompt: false,
            new_after_save: false,
            request_save: false,
            last_saved_deps: None,
            project_name: persisted.project_name,
            project_dir: saved_project_dir.clone(),
            fs_rx: Some(fs_rx),
            _fs_watcher: watcher.ok(),
            pending_open_dir: None,
            startup_picker: None,
            workspace_write_requested: false,
            cli_project_error: cli_error,
            // Empty, not the startup title: the first frame then always pushes
            // one title, so what the window shows can't drift from what this
            // app computes (a restored project renames it immediately anyway).
            window_title: String::new(),
            recent_forget_confirm: None,
            project_lock: None,
            project_lock_conflict: None,
            project_lock_retry: None,
            project_loading: None,
        };

        // ── Restore previously opened project on startup ──────────────────────
        // User source files, pin configuration and generated code are all
        // recovered from the folder, exactly as they were when the app was last
        // closed. `saved_project_dir` is already filtered to a folder that
        // exists — when it is `None` the state above was cleared to match, so
        // there is a clean empty project here rather than a half-restored one.
        // …unless the command line named another one, the preference says to
        // ask every time, or the remembered project is already open in another
        // window — see `crate::startup::decide` for the rule and why.
        match crate::startup::decide(
            cli_project,
            crate::startup::load_mode(),
            saved_project_dir,
            // Probing IS claiming: the claim is dropped immediately and retaken
            // by the load below. A window that grabs the folder in between is a
            // race no desktop user can hit, and the folder banner covers it.
            |dir| {
                matches!(
                    crate::workspace::claim_project(dir),
                    crate::workspace::ProjectClaim::Busy
                )
            },
        ) {
            crate::startup::StartupAction::Open(dir) => {
                app.load_project_from_dir(&dir);
                // Same overlay, restated for what this actually is — the load
                // above armed it as an "Open".
                app.begin_project_loading(loading_overlay::LoadKind::Restore);
            }
            action => {
                // Nothing is being opened, so the restored buffers have no
                // project to belong to — the `drop_homeless_files` rule, applied
                // to a project we chose NOT to reopen. Leaving them would show
                // one project's files in a window holding another.
                app.project_dir = None;
                app.project_name = None;
                app.project_tree.user_src_files.clear();
                app.project_tree.user_src_folders.clear();
                if let crate::startup::StartupAction::Ask { last } = action {
                    app.startup_picker = Some(startup_picker::StartupPicker::new(
                        last,
                        crate::startup::load_mode(),
                    ));
                }
            }
        }

        app
    }

    /// Build the runtime `Mcu` for `id` from the registry, if present.
    fn build_mcu_for(registry: &[McuDefinition], id: &str) -> Option<Mcu> {
        registry.iter().find(|d| d.id == id).map(|d| d.build_mcu())
    }

    /// Write the Clock tab's edited tree back into the chip's `.ron` definition.
    ///
    /// This is what makes a STRUCTURAL clock edit (a node added, deleted, rewired)
    /// outlive the session: node states round-trip through `mcu.config`, but the
    /// TOPOLOGY lives in the definition, so it has to be saved there.
    /// [`registry::save_definition`] writes `<user mcus>/<id>.ron`, which the
    /// loader merges over the built-in of the same id — so editing a bundled
    /// chip's clock creates a personal override rather than needing a writable
    /// install.
    ///
    /// The result goes to `clock_note`, which the Clock tab shows next frame —
    /// the answer appears where the button was.
    fn save_clock_to_definition(&mut self) {
        use crate::panels::mcu_module::clock::ClockConfig;
        use crate::panels::mcu_module::mcu_def::ClockDef;

        let Some(mcu) = &self.mcu else { return };
        let ClockConfig::Graph(gc) = &mcu.clock else {
            return;
        };
        let Some(def) = self.selected_def() else {
            self.clock_ui.note = "No chip selected.".to_owned();
            return;
        };
        let mut def = def.clone();
        def.clock = ClockDef::Graph(gc.clone());

        self.clock_ui.note = match registry::save_definition(&def) {
            Ok(path) => {
                // Keep the live registry in step, so reopening the project (or
                // the chip picker) sees the edited tree without a restart.
                registry::merge_def(&mut self.mcu_registry, def);
                // What was just saved IS the chip's factory clock now, so the
                // Reset button must aim at it — otherwise Reset would revert to
                // the tree this one replaced.
                if let Some(mcu) = &mut self.mcu {
                    mcu.capture_clock_defaults();
                }
                format!("Clock tree saved to {}", path.display())
            }
            Err(e) => format!("Could not save the chip definition: {e}"),
        };
    }

    /// The currently-selected MCU definition (key = `selected_mcu_id`).
    fn selected_def(&self) -> Option<&McuDefinition> {
        self.mcu_registry
            .iter()
            .find(|d| d.id == self.selected_mcu_id)
    }

    /// True when a real chip is selected (replaces `project_config().is_some()`).
    fn has_project(&self) -> bool {
        self.selected_def().is_some()
    }

    /// Drive a Go-to-definition that was asked for while rust-analyzer could not
    /// answer: start the analyzer once, wait for it to be genuinely usable, then
    /// re-issue the request. The normal result handler takes it from there, so
    /// the jump (or the Definition tab) happens on its own.
    ///
    /// Every branch here exists because of a way this can go wrong:
    ///
    /// * the restart is **one-shot**. Anything shaped as "not ready yet, restart"
    ///   would kill the analyzer it spawned last frame — for ever, and each
    ///   round costs a synchronous `taskkill` and a project rewrite.
    /// * the fire waits for `indexed`, not `Ready`. `Ready` flips on the first
    ///   `$/progress` end of any rust-prefixed token (it can be "Fetching
    ///   metadata"), and the F12 path's `did_change` AUTO-OPENS the document —
    ///   opening it that early is exactly the detached-file analysis that
    ///   produces phantom type errors.
    /// * a cold start takes tens of seconds, in which the user may configure a
    ///   pin and have main.rs regenerated under the captured position. The text
    ///   hash catches that.
    /// * with no chip selected there is nothing to start, and the wait would
    ///   never end — say so instead.
    fn poll_pending_goto(&mut self) {
        let Some(p) = &self.pending_goto else { return };

        // Give up quietly rather than leave a spinner running for ever.
        if p.since.elapsed() > GOTO_WAIT {
            self.pending_goto = None;
            self.set_status_msg(format!(
                "{} Go to definition: the analyzer did not finish loading",
                egui_phosphor::regular::X_CIRCLE
            ));
            return;
        }

        // The position was taken in a buffer that has since changed — the same
        // line and column now mean something else.
        let current = self.file_text_hash(p.file);
        if current.is_some_and(|h| h != p.text_hash) {
            self.pending_goto = None;
            self.set_status_msg(format!(
                "{} Go to definition: the file changed while the analyzer was loading",
                egui_phosphor::regular::X_CIRCLE
            ));
            return;
        }

        let (status, indexed) = {
            let lsp = self.lsp_state.lock().unwrap();
            (lsp.status.clone(), lsp.indexed)
        };

        match status {
            // Usable at last: re-issue exactly what was asked for.
            crate::lsp::LspStatus::Ready if indexed => {
                let p = self.pending_goto.take().expect("checked above");
                let sent = {
                    let mut lsp = self.lsp_state.lock().unwrap();
                    if p.implementation {
                        lsp.request_implementation(&p.rel, p.line, p.col)
                    } else {
                        lsp.request_definition(&p.rel, p.line, p.col)
                    }
                };
                // Only wait for an answer that can actually come.
                self.definition_in_flight = sent;
                if !sent {
                    self.set_status_msg(format!(
                        "{} Go to definition: the analyzer is not reachable",
                        egui_phosphor::regular::X_CIRCLE
                    ));
                }
                self.egui_ctx.request_repaint();
            }
            // Dead. Start it — once.
            crate::lsp::LspStatus::Stopped | crate::lsp::LspStatus::Failed(_) => {
                if !self.has_project() {
                    // `has_project` is "a chip is selected": without one the
                    // Stopped arm never starts anything, so the wait could not
                    // end. Better a sentence than a spinner for ever.
                    self.pending_goto = None;
                    self.set_status_msg(format!(
                        "{} Go to definition needs the analyzer, which needs a chip selected",
                        egui_phosphor::regular::X_CIRCLE
                    ));
                    return;
                }
                if let crate::lsp::LspStatus::Failed(why) = &status {
                    // `reset()` wipes `load_log`, the only record of WHY the
                    // last session died — keep the reason before it goes.
                    let why = why.clone();
                    self.lsp_state.lock().unwrap().push_load_log(format!(
                        "• restarting after a failure, on Go to definition: {why}"
                    ));
                }
                let fired = self.pending_goto.as_ref().is_some_and(|p| p.restart_fired);
                if fired {
                    // Already restarted once and it is dead again — do not spin.
                    self.pending_goto = None;
                    let why = match &status {
                        crate::lsp::LspStatus::Failed(w) => w.clone(),
                        _ => "it did not start".to_owned(),
                    };
                    self.set_status_msg(format!(
                        "{} Analyzer: {why}",
                        egui_phosphor::regular::X_CIRCLE
                    ));
                    return;
                }
                if let Some(p) = &mut self.pending_goto {
                    p.restart_fired = true;
                }
                self.restart_lsp();
                self.egui_ctx.request_repaint();
            }
            // Starting / Indexing / Ready-but-not-indexed: just wait.
            _ => {}
        }
    }

    /// Hash of a file's CURRENT in-memory text, for staleness checks.
    fn file_text_hash(&self, id: ProjectFileId) -> Option<u64> {
        let text = match id {
            ProjectFileId::MainRs => &self.generated_code,
            ProjectFileId::UserFile(i) => &self.project_tree.user_src_files.get(i)?.1,
            _ => return None,
        };
        Some(Self::content_hash(text))
    }

    /// Show a short-lived message in the status bar (the same channel the
    /// export / save results use).
    fn set_status_msg(&mut self, msg: String) {
        self.export_msg = msg;
        self.export_status_until =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(6));
        self.egui_ctx.request_repaint();
    }

    /// Display name of the selected chip (empty if none).
    fn selected_label(&self) -> String {
        self.selected_def()
            .map(|d| d.display_name.clone())
            .unwrap_or_default()
    }

    /// The grey facts that follow the chip name in the header: core, package,
    /// package pin count, datasheet maximum frequency — values only, no field
    /// labels.
    ///
    /// Each is dropped when it is not KNOWN rather than filled with a
    /// plausible default: `max_mhz` is absent for roughly a third of the
    /// vendor database (the whole C0 series states no frequency), and the
    /// two built-in chips carry neither package nor frequency.
    fn chip_facts(&self) -> Vec<String> {
        let Some(def) = self.selected_def() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if !def.cpu.trim().is_empty() {
            out.push(def.cpu.trim().to_owned());
        }
        if !def.package.trim().is_empty() {
            out.push(def.package.trim().to_owned());
        }
        // Counted off the LIVE chip, not the package name: `iter_all_pins` is
        // layout-blind, so a ball grid answers as readily as a QFP, and the
        // power/reset pads count as the package pins they are.
        if let Some(n) = self.mcu.as_ref().map(|m| m.iter_all_pins().count()) {
            if n > 0 {
                out.push(format!("{n} pins"));
            }
        }
        if let Some(mhz) = def.max_mhz {
            out.push(format!("{mhz} MHz"));
        }
        out
    }

    /// Toolchain of the selected chip (None if no chip selected).
    fn selected_toolchain(&self) -> Option<ToolchainKind> {
        self.selected_def().map(|d| d.toolchain.clone())
    }

    /// The selected chip's Rust target triple, for narrowing which tools the
    /// banner asks for — see [`RequiredTool::only_for_target`].
    fn selected_target(&self) -> Option<String> {
        self.selected_def().map(|d| d.project.target.clone())
    }

    /// Owned `(project params, toolchain)` for project generation — cloned so no
    /// borrow of `self` is held across the subsequent `self` mutations.
    /// The selected chip's build config, AS IT APPLIES TO THE RUNTIME.
    ///
    /// `for_async` is what swaps a family's HAL crate line, and it used to be
    /// called from nowhere but the cross-compile harnesses — so a Pico set to
    /// Async got `main.rs` full of `embassy_rp::` against a Cargo.toml that
    /// named `rp2040-hal` and no embassy at all (E0433 on the first Save),
    /// while every emitted-project test stayed green because each one called
    /// `for_async` by hand. Applied HERE rather than at the two writers, so a
    /// third consumer cannot be added without it.
    ///
    /// `Mcu::is_async` is the same predicate codegen picks its backend with -
    /// it already folds in `async_supported`, so a family that falls back to
    /// Blocking gets the blocking manifest to match.
    fn selected_build_cfg(&self) -> Option<(ProjectDef, ToolchainKind)> {
        let mcu = self.mcu.as_ref();
        self.selected_def().map(|d| {
            (
                crate::panels::mcu_module::mcu_def::build_cfg(d, mcu),
                d.toolchain.clone(),
            )
        })
    }
    /// The live project files: generated `main.rs` plus the five editable config
    /// The live project files: generated `main.rs` plus the five editable config
    /// files (with their current user edits). This — not a fresh regeneration —
    /// is what the editor shows and what `write_project` persists.
    fn current_project_files(&self) -> ProjectFiles {
        // Return cached version if available and up-to-date
        if let Some(ref cached) = self.cached_project_files {
            // Check if cache is still valid by comparing with current state
            if cached.main_rs == self.generated_code
                && cached.cargo_toml == self.cargo_toml
                && cached.cargo_config == self.cargo_config
                && cached.memory_x == self.memory_x
                && cached.build_rs == self.build_rs
                && cached.gitignore == self.gitignore
            {
                return cached.clone();
            }
        }

        // Create new ProjectFiles and cache it
        let files = ProjectFiles {
            main_rs: self.generated_code.clone(),
            cargo_toml: self.cargo_toml.clone(),
            cargo_config: self.cargo_config.clone(),
            memory_x: self.memory_x.clone(),
            build_rs: self.build_rs.clone(),
            gitignore: self.gitignore.clone(),
            // Derived, never edited: it says which compiler the chip needs, and
            // that is not the user's to change per project.
            rust_toolchain: self
                .selected_target()
                .map(|t| project_gen::rust_toolchain_for(&t))
                .unwrap_or_default(),
        };
        files
    }

    /// Update the cached project files (call this after modifying any config file)
    fn invalidate_project_files_cache(&mut self) {
        self.cached_project_files = None;
    }

    /// Apply clippy's machine-applicable [`SpanEdit`](crate::build::SpanEdit)s to
    /// the in-memory source. Edits are grouped per file and applied back-to-front
    /// (so earlier byte offsets stay valid), skipping any that overlap an
    /// already-applied edit or land off a UTF-8 char boundary. `src/main.rs`
    /// targets the generated buffer; `src/<rel>` targets the matching user source
    /// file. Returns how many edits were applied.
    ///
    /// Edits inside main.rs's GENERATED block are **never** applied: that code is
    /// owned by the MCU Configurator and regenerated on the next config change, so
    /// a hand-applied fix there would be silently reverted (and editing it while
    /// the editor shows it risks a cursor/offset mismatch). The Clippy tab also
    /// disables the "Fix" button for those rows — this is the matching guard.
    fn apply_source_edits(&mut self, edits: &[crate::build::SpanEdit]) -> usize {
        use std::collections::HashMap;
        let gen_ranges = generated_byte_ranges(&self.generated_code);
        let mut by_file: HashMap<&str, Vec<&crate::build::SpanEdit>> = HashMap::new();
        for e in edits {
            by_file.entry(e.file.as_str()).or_default().push(e);
        }
        let mut applied = 0usize;
        for (file, group) in by_file {
            // Paths are project-root-relative, exactly as cargo reports them.
            let is_main = file == "src/main.rs";
            let target: Option<&mut String> = if is_main {
                Some(&mut self.generated_code)
            } else {
                self.project_tree
                    .user_src_files
                    .iter_mut()
                    .find(|(p, _)| p == file)
                    .map(|(_, c)| c)
            };
            let Some(buf) = target else { continue };
            // main.rs's GENERATED block is off-limits; user files have no lock.
            let locked: &[(usize, usize)] = if is_main { &gen_ranges } else { &[] };
            applied += apply_edits_to_buffer(buf, &group, locked);
        }
        if applied > 0 {
            self.invalidate_project_files_cache();
        }
        applied
    }

    /// The `mcu.config` text: the live MCU's sections (virtual modules + clock)
    /// and nothing else. Written alongside the project by `write_project`;
    /// empty when there is nothing to persist.
    ///
    /// The Structure diagram's positions and view options are NOT here — they
    /// changed on every node drag and kept this file permanently modified in
    /// Git. See [`Self::structure_config_text`].
    fn mcu_config_text(&self) -> String {
        self.mcu
            .as_ref()
            .map(|m| m.mcu_config_text())
            .unwrap_or_default()
    }

    /// The `project_structure.config` text: dragged node positions + view
    /// options of the Structure tab. Empty when there is nothing to persist, so
    /// an untouched project carries no such file at all.
    fn structure_config_text(&self) -> String {
        let v = &self.structure_view;
        crate::panels::mcu_module::structure_config::serialize(
            &self.structure_overrides,
            &(
                v.show_calls,
                v.call_depth,
                v.path_style.to_u8(),
                v.show_externals,
            ),
            &self.clock_ui.positions,
            &self.clock_ui.fields,
        )
    }

    /// Regenerate every editable config file fresh from the selected chip,
    /// discarding prior edits. Used when starting a New Project / switching chip
    /// (a clean slate, like clearing `user_src_files`). Toolchains that don't use
    /// a file (e.g. `memory.x`/`build.rs` on ESP) get an empty string.
    fn reset_config_files(&mut self) {
        // No chip, no files. This used to be an early return, which left the
        // PREVIOUS chip's Cargo.toml and memory.x in a project that had just
        // been emptied — every one of them derived from a target that is no
        // longer selected.
        let Some((cfg, tc)) = self.selected_build_cfg() else {
            self.cargo_toml.clear();
            self.cargo_config.clear();
            self.memory_x.clear();
            self.build_rs.clear();
            self.gitignore.clear();
            self.last_saved_deps = None;
            self.invalidate_project_files_cache();
            return;
        };
        let f = project_gen::build_project_files(&cfg, &tc, &self.generated_code);
        self.cargo_toml = f.cargo_toml;
        self.cargo_config = f.cargo_config;
        self.memory_x = f.memory_x;
        self.build_rs = f.build_rs;
        self.gitignore = f.gitignore;
        // Baseline the deps so a New Project auto-builds on the first Save
        // after the user's config adds libraries (USART/SPI/embassy/…).
        self.last_saved_deps = Some(project_gen::deps_fingerprint(&self.cargo_toml));
        self.invalidate_project_files_cache();
    }

    /// File + 1-based line of the code that defines pin `pin_num`'s variable, or
    /// `None` when nothing has been generated for it yet (an unconfigured pin, or
    /// a project that has never been generated) — that stays silent rather than
    /// scrolling somewhere arbitrary.
    ///
    /// Targets, in order:
    /// 1. the pin's own `let <binding> = …` in main.rs's GEN block — the actual
    ///    definition, and what every backend emits for a configured pin;
    /// 2. the first GEN-block line that mentions the pin (ESP hands bus pins
    ///    straight to their driver, e.g. `.with_rx(peripherals.GPIO20)`, so that
    ///    call IS the definition site);
    /// 3. the peripheral / Custom-module config file under `src/pins/configs/`
    ///    that names the binding.
    fn locate_pin(&self, pin_num: usize) -> Option<(ProjectFileId, usize)> {
        use crate::panels::mcu_module::codegen::common;
        let (name, binding) = self.mcu.as_ref().and_then(|m| {
            m.find_pin(pin_num).map(|p| {
                (
                    p.name.clone(),
                    common::pin_binding(
                        &p.name.to_ascii_lowercase(),
                        &p.selected_function,
                        &p.custom_label,
                    ),
                )
            })
        })?;

        let main_hit = common::find_pin_binding_line(&self.generated_code, &name)
            .map(|(line, _)| line)
            .or_else(|| common::find_pin_mention_line(&self.generated_code, &name));
        if let Some(line) = main_hit {
            return Some((ProjectFileId::MainRs, line));
        }

        self.project_tree
            .user_src_files
            .iter()
            .enumerate()
            .find_map(|(i, (path, content))| {
                if !path.contains("pins/configs/") {
                    return None;
                }
                content
                    .lines()
                    .position(|l| l.contains(&binding))
                    .map(|li| (ProjectFileId::UserFile(i), li + 1))
            })
    }

    /// Open the code for a whole GROUP of pins — one pin for a pin click, a
    /// module's whole wiring for a module click — scroll to the first of them and
    /// pulse every one of their lines.
    ///
    /// Pins that resolve to a DIFFERENT file than the first hit are dropped: the
    /// editor shows one file, and a band the user can't see is worse than one
    /// missing line. In practice every pin of a module binds in main.rs anyway.
    fn goto_pins_in_code(&mut self, pins: &[usize], now: f64) {
        let hits: Vec<(ProjectFileId, usize)> =
            pins.iter().filter_map(|n| self.locate_pin(*n)).collect();
        let Some(&(file, _)) = hits.first() else {
            return;
        };
        let mut lines: Vec<usize> = hits
            .iter()
            .filter(|(f, _)| *f == file)
            .map(|(_, l)| *l)
            .collect();
        lines.sort_unstable();
        lines.dedup();

        self.selected_file = file;
        self.pending_scroll_to_line = Some((file, lines[0]));
        self.highlighted_pin_lines = Some(PinHighlight {
            file,
            lines,
            start: now,
        });
    }

    // ── Frame initialization (frame state, LSP, MCU synchronization) ───────────
    /// Calculate a hash of the MCU state (pins + clock + modules) for change detection
    fn calculate_mcu_state_hash(&self, mcu: &Mcu) -> u64 {
        let mut hasher = DefaultHasher::new();

        // Hash all pins
        for pin in mcu.iter_all_pins() {
            pin.selected_function.hash(&mut hasher);
            pin.custom_label.hash(&mut hasher);
            // The interrupt edge is CODE, not a view preference: arming a pin
            // adds a whole #[task] on the RTIC runtime, so it must regenerate.
            pin.irq.hash(&mut hasher);
            // Same reasoning for the GPIO drive/pull mode: it selects the
            // `into_*` / `Pull::*` the binding is generated with.
            pin.io_mode.hash(&mut hasher);
        }

        // Hash clock config
        format!("{:?}", mcu.clock).hash(&mut hasher);

        // Hash the runtime — flipping Blocking⇄Async re-targets the backend
        // (async entry) and the embassy deps, so it must trigger regeneration.
        mcu.runtime.as_token().hash(&mut hasher);
        // Hash the GPIO api — Portable⇄Native flips the io.rs bridge + bindings.
        format!("{:?}", mcu.gpio_api).hash(&mut hasher);
        // Strict-lints toggle: flipping it adds/removes the Cargo.toml
        // `[lints.clippy]` block AND the `#[allow]` exemptions injected into the
        // generated main.rs / config files, so it must trigger regeneration.
        mcu.strict_lints.hash(&mut hasher);
        // Debug-friendly build toggle: rewrites `[profile.release]` in the
        // Cargo.toml, so the same regeneration pass has to run.
        mcu.debug_build.hash(&mut hasher);

        // Watchdogs: codegen input, so a change here must regenerate. Without
        // this the Configuration tab would edit a value that never reaches the
        // generated project until something else happened to bump the hash.
        format!("{:?}", mcu.watchdog).hash(&mut hasher);

        // Hash modules
        for module in &mcu.modules {
            module.id.hash(&mut hasher);
            module.kind.hash(&mut hasher);
            module.name.hash(&mut hasher);
            format!("{:?}{:?}", module.pos.0, module.pos.1).hash(&mut hasher);
            // Parameters (baud, mode, …) and pin wiring feed `config_files()`
            // — they must bump the hash, or the hash-gated regeneration in
            // `init_frame` would miss a module edit and keep emitting the old
            // configs/*.rs content.
            format!("{:?}{:?}", module.config, module.connections).hash(&mut hasher);
        }

        hasher.finish()
    }

    fn init_frame(&mut self, ui: &mut egui::Ui) {
        // ── Poll filesystem watcher events ────────────────────────────────────
        self.poll_fs_events();

        // ── Re-take a project folder another window was holding ───────────────
        // No-op unless a conflict is up, and throttled to one probe every 2 s.
        self.retry_project_claim();

        // ── Window title = the open project ───────────────────────────────────
        // After the claim retry, so a conflict that just cleared drops the
        // instance marker in the same frame.
        self.refresh_window_title(ui);

        // ── MRU file history (Ctrl+Tab switching) ─────────────────────────────
        // One central change detector: whatever switched `selected_file` (tree
        // click, F12, diagnostics nav, …), promote the new file to the front of
        // the MRU — except while a Ctrl+Tab cycling session drives the switches
        // itself (the promotion happens on commit instead).
        use editor_panel::file_cycle::HistEntry;
        if self.file_cycle.is_empty() {
            // Seed with the file shown at startup.
            if let Some(e) =
                HistEntry::from_id(self.selected_file, &self.project_tree.user_src_files)
            {
                self.file_cycle.note_open(e);
            }
        }
        if self.selected_file != self.last_selected_file {
            if !self.file_cycle.is_cycling() {
                if let Some(e) =
                    HistEntry::from_id(self.selected_file, &self.project_tree.user_src_files)
                {
                    self.file_cycle.note_open(e);
                }
            }
            self.last_selected_file = self.selected_file;
        }

        // ── Update generated code when the MCU state changes ─────────────────
        // ONE hash gates BOTH main.rs regeneration AND the peripheral-config /
        // pin-file sync below. The sync used to run UNCONDITIONALLY every
        // frame — full codegen of every configs/*.rs plus Cargo.toml dep
        // checks, just to no-op compare — which, under spinner-driven
        // continuous repaint, was a big share of the per-frame CPU cost.
        // The Native runtime binds every GPIO raw, so the GPIO In/Out choice is
        // not live there — snap the stored value to Native BEFORE hashing, so the
        // (locked) selector shows what the build does and the now-unused
        // `embedded-hal` is dropped in the same pass. Idempotent.
        if let Some(m) = &mut self.mcu {
            m.normalize_gpio_api();
        }
        let mut mcu_changed = false;
        if let Some(mcu) = &self.mcu {
            let current_hash = self.calculate_mcu_state_hash(mcu);
            if current_hash != self.mcu_state_hash {
                // Store the hash even when main.rs comes out identical —
                // otherwise this branch (and `update_main_rs`) re-runs every
                // frame until the NEXT state change.
                self.mcu_state_hash = current_hash;
                mcu_changed = true;
                let updated = mcu.update_main_rs(&self.generated_code);
                if updated != self.generated_code {
                    self.generated_code = updated;
                    self.cached_project_files = None; // Invalidate cache
                }
            }
        }

        // Regenerate the per-peripheral init modules (src/pins/configs/) and the
        // pins/ module files from the current pin + Virtual-Module config. Both
        // are no-ops when nothing changed (splice preserves user edits). Configs
        // first so `sync_pin_files` can declare `pub mod configs;` in pins/mod.rs.
        let regen = if mcu_changed {
            self.mcu
                .as_ref()
                .map(|m| (m.all_pin_functions(), m.config_files()))
        } else {
            None
        };
        if let Some((all_pins, config_files)) = regen {
            use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
            // Peripheral config files pull in standard-trait crates (embedded-io
            // for USART, embedded-hal 1.0 `SpiBus`/`I2c` for SPI/I2C, bxcan for
            // CAN, nb for the blocking bridges). Keyed on the config files AND —
            // for the trait crates — on each module's API style: a NATIVE-style
            // USART/SPI/I2C module emits a config file but needs NO trait crate.
            let has_cfg = |p: &str| config_files.iter().any(|(name, _)| name.starts_with(p));
            // On the async (embassy) path the blocking F1 trait-bridge deps
            // (embedded-io / embedded-hal-0-2 / nb / bxcan / usb-device) DON'T
            // apply — the async USART config pulls embedded-io-async + static_cell
            // via `ensure_async_deps` instead. Async is only offered on non-F1
            // STM32, where those bridges aren't generated anyway.
            let is_async = self.mcu.as_ref().is_some_and(|m| m.is_async());
            // Native runtime → the bus peripherals expose CONCRETE HAL types
            // (all `ApiStyle::Native`), so NONE of the portable trait-bridge
            // crates are pulled — same as async, for a different reason.
            let is_native = self.mcu.as_ref().is_some_and(|m| m.is_native());
            // ESP config modules speak esp-hal DIRECTLY (`Uart<'d, Blocking>`,
            // `I2c<'d, Blocking>`) — no portable bridge, so none of the trait
            // crates below apply. Without this an ESP project with a USART
            // module collected `embedded-io` / `nb` / `embedded-hal`, which
            // nothing in its generated code ever mentions.
            let is_esp = self
                .selected_build_cfg()
                .is_some_and(|(_, tc)| tc == ToolchainKind::EspRust);
            let needs_can = !is_async && !is_esp && has_cfg("can");
            // Trait crates only for PORTABLE modules on the Blocking path.
            let (mut needs_usart, mut needs_spi, mut needs_i2c) = (false, false, false);
            // Async SPI/I2C deps: `embedded-hal` 1.0 for any bus, plus
            // `embedded-hal-async` when a module is in async-DMA mode.
            let (mut needs_eh, mut needs_eh_async) = (false, false);
            // ANY USART on the blocking/native path needs `nb`: a Portable USART
            // via its `nb::block!` bridge, a Native USART because the concrete
            // stm32f1xx-hal `Tx`/`Rx` are nb-based (`nb::block!(rx.read())`).
            // (Async USART uses embedded-io-async → no nb.)
            let mut any_usart_blocking = false;
            if let Some(m) = &self.mcu.as_ref().filter(|_| !is_esp) {
                use crate::panels::mcu_module::modules::{ApiStyle, AsyncBusMode, ModuleConfig};
                for md in &m.modules {
                    if is_async {
                        let mode = match &md.config {
                            ModuleConfig::Spi(c) => Some(c.async_mode),
                            ModuleConfig::I2c(c) => Some(c.async_mode),
                            _ => None,
                        };
                        if let Some(mode) = mode {
                            needs_eh = true;
                            needs_eh_async |= mode == AsyncBusMode::AsyncDma;
                        }
                    } else {
                        // Blocking OR Native path.
                        if matches!(&md.config, ModuleConfig::Usart(_)) {
                            any_usart_blocking = true;
                        }
                        if !is_native {
                            // Blocking (mixed): trait crates for the Portable modules.
                            match &md.config {
                                ModuleConfig::Usart(c) if c.api_style == ApiStyle::Portable => {
                                    needs_usart = true
                                }
                                ModuleConfig::Spi(c) if c.api_style == ApiStyle::Portable => {
                                    needs_spi = true
                                }
                                ModuleConfig::I2c(c) if c.api_style == ApiStyle::Portable => {
                                    needs_i2c = true
                                }
                                _ => {}
                            }
                        }
                        // is_native → all Native → no portable trait crates
                        // (but still `nb` via `any_usart_blocking` above).
                    }
                }
            }
            // `nb`: CAN (bxcan), the Portable SPI FullDuplex bridge, and ANY
            // USART on the non-async path. Kept separate so a Native USART (which
            // sets no other trait crate) still keeps `nb` — and a user's pinned
            // `nb = "1.1.0"` survives Save instead of being stripped.
            let needs_nb = needs_can || needs_spi || any_usart_blocking;
            // GPIO in/out pins are wrapped in the `pins::configs::io` eh-1.0
            // bridge, emitted as `io.rs` (always portable).
            let needs_gpio = !is_async && has_cfg("io");
            // USB CDC init needs `usb-device`/`usbd-serial` + the `stm32-usbd`
            // HAL feature, keyed on whether the USB D-/D+ pins are configured.
            // BOTH pads, matching what `gen_parts` will actually emit: one pad
            // alone generates no USB init, so the crates would be unused.
            let usb_pad = |want: PinFunction| all_pins.iter().any(|(_, _, f)| *f == want);
            let is_esp_family = self
                .mcu
                .as_ref()
                .is_some_and(|m| crate::panels::mcu_module::codegen::family::is_esp(&m.family));
            let both_usb_pads = usb_pad(PinFunction::UsbDm) && usb_pad(PinFunction::UsbDp);
            // The STM32F1 `usb-device` 0.2 stack. It used to fire on an ESP too:
            // both pads wired on a blocking project added two crates nothing
            // referenced, at versions the ESP bus cannot even use.
            let needs_usb = !is_async && !is_esp_family && both_usb_pads;
            // …and the ESP's own stack, which only the OTG role wants.
            let needs_esp_otg = is_esp_family
                && both_usb_pads
                && self.mcu.as_ref().is_some_and(|m| {
                    crate::panels::mcu_module::codegen_esp::has_usb_otg(&m.family)
                        && crate::panels::mcu_module::modules::usb_configs(&m.modules)
                            .get(&1)
                            .is_some_and(|c| c.role.is_otg())
                });
            // The user's own sources — a dependency referenced by THIS code is
            // never stripped, whatever the feature flags say (a Runtime switch
            // used to silently delete a hand-added `embedded-hal`). See
            // `project_gen::is_crate_referenced`.
            //
            // `src/pins/**` is excluded on purpose: those files are generated
            // from the model and rewritten on the next sync, so a bridge the IDE
            // is about to stop emitting must not keep its crate alive (that is
            // what left `embedded-hal` behind after switching to the Native
            // runtime).
            let sources: Vec<&str> = std::iter::once(self.generated_code.as_str())
                .chain(
                    self.project_tree
                        .user_src_files
                        .iter()
                        .filter(|(path, _)| !path.starts_with("src/pins/"))
                        .map(|(_, body)| body.as_str()),
                )
                .collect();
            let new_toml = project_gen::ensure_peripheral_deps(
                &self.cargo_toml,
                needs_can,
                needs_usart,
                needs_spi,
                needs_i2c,
                needs_gpio,
                needs_nb,
                &sources,
            );
            let new_toml = project_gen::ensure_usb_deps(&new_toml, needs_usb, &sources);
            let new_toml = project_gen::ensure_esp_usb_deps(&new_toml, needs_esp_otg, &sources);
            // Async runtime (embassy-executor + embassy-time + the HAL time
            // driver), plus — when the respective config files were emitted —
            // embedded-io-async + static_cell (USART) and embedded-hal /
            // embedded-hal-async (SPI/I2C). Keyed on the project Runtime being
            // Async on an embassy-capable STM32 family.
            //
            // BOTH `ensure_peripheral_deps` (blocking eh1 = SPI/I2C/GPIO) and this
            // manage `embedded-hal` 1.0 — so the need passed here MUST include the
            // blocking case, or the blocking GPIO (io.rs) / SPI / I2C `embedded-hal`
            // that `ensure_peripheral_deps` just added would be stripped right back
            // out here (and a user's manual `embedded-hal` line with it).
            let needs_eh_total = needs_eh || needs_spi || needs_i2c || needs_gpio;
            // Which async stack: embassy-stm32, or esp-rtos on the ESP32-C3.
            // Same crate names, different versions — see `AsyncFlavor`. The chip
            // feature (`esp32c3`) comes from the same `ProjectDef` the base
            // Cargo.toml template uses.
            let esp_chip = self
                .selected_build_cfg()
                .map(|(p, _)| p.probe_chip)
                .unwrap_or_default();
            // One decision, shared with the harness - see `async_flavor_for`.
            // Choosing here as well is how a Pico on Async came to be saved with
            // the STM32 executor line while every test stayed green.
            let async_flavor = project_gen::async_flavor_for(
                self.mcu.as_ref().map_or("", |m| m.family.as_str()),
                &esp_chip,
            );
            let new_toml = project_gen::ensure_async_deps(
                &new_toml,
                is_async,
                async_flavor,
                // `has_cfg` matches a config-FILE prefix, and the async RP
                // backend writes no config files at all - so a Pico with a
                // `BufferedUart` in main.rs would ask for `static_cell` against
                // a manifest that never got the line. A Pico W hides it, since
                // the radio adds that crate for its own reasons.
                is_async && has_cfg("usart")
                    || self
                        .mcu
                        .as_ref()
                        .is_some_and(crate::panels::mcu_module::codegen::rp::needs_async_usart),
                needs_eh_total,
                needs_eh_async,
                &sources,
            );
            // `embassy_stm32::exti` is behind a Cargo feature, so an input the
            // user armed with an edge needs the FEATURE as well as the code.
            // Read off the generated main.rs rather than off the pins: a pin
            // refused for a line clash generates no `exti` use, and asking for a
            // feature nothing imports would be a lie in the manifest.
            let new_toml = project_gen::ensure_exti_feature(
                &new_toml,
                self.generated_code.contains("embassy_stm32::exti"),
            );
            // The CYW43 radio, on a Pico W / Pico 2 W whose WL_LED is driven.
            // Gated on the pin rather than on the board, because a W board with
            // the LED untouched should not carry a wifi stack it never calls.
            let needs_radio = self.mcu.as_ref().is_some_and(|m| {
                m.iter_all_pins().any(|p| {
                    p.name == "WL_LED"
                        && p.selected_function
                            == crate::panels::mcu_module::pins::PinFunction::GpioOutput
                })
            });
            let new_toml = project_gen::ensure_cyw43_deps(&new_toml, needs_radio, &sources);
            // Cortex-M0 async: `static_cell` needs CAS the core does not have.
            let async_target = self
                .selected_build_cfg()
                .map(|(p, _)| p.target)
                .unwrap_or_default();
            let new_toml =
                project_gen::ensure_m0_atomics(&new_toml, is_async, &async_target, &sources);
            // RTIC runtime: the framework + its SysTick monotonic. The backend
            // feature follows the chip's Rust target, which only `ProjectDef`
            // knows — hence reading it here rather than in the codegen backend.
            let is_rtic = self.mcu.as_ref().is_some_and(|m| m.is_rtic());
            let rtic_target = self
                .selected_build_cfg()
                .map(|(p, _)| p.target)
                .unwrap_or_default();
            let new_toml =
                project_gen::ensure_rtic_deps(&new_toml, is_rtic, &rtic_target, &sources);
            // Strict-lints `[lints.clippy]` block (MCU System toggle).
            let strict = self.mcu.as_ref().is_some_and(|m| m.strict_lints);
            let new_toml = project_gen::ensure_strict_lints(&new_toml, strict);
            // Debug-friendly `[profile.release]` (Debug tab toggle).
            let debug_build = self.mcu.as_ref().is_some_and(|m| m.debug_build);
            let new_toml = project_gen::ensure_debug_build(&new_toml, debug_build);
            if new_toml != self.cargo_toml {
                self.cargo_toml = new_toml;
                self.invalidate_project_files_cache();
            }
            // On a Runtime / Init-API Apply the config templates change wholesale
            // (blocking ⇄ async ⇄ native init) — force a FULL rewrite so the old
            // `init()` in the editable region is replaced, not just the consts.
            let force_configs = self.mcu.as_ref().is_some_and(|m| m.config_regen_forced);
            // Older revisions of each Custom module's file stay on disk.
            let keep: Vec<String> = self
                .mcu
                .as_ref()
                .map(|m| {
                    m.modules
                        .iter()
                        .filter(|md| md.kind.is_custom())
                        .map(crate::panels::mcu_module::mcu::gui::modules::custom_file_prefix)
                        .collect()
                })
                .unwrap_or_default();
            self.project_tree
                .sync_config_files(&config_files, force_configs, &keep);
            self.project_tree.sync_pin_files(&all_pins);
            if force_configs {
                if let Some(m) = &mut self.mcu {
                    m.config_regen_forced = false;
                }
            }
        }
        // Any regen rewrites main.rs / config files / deps in the RA workspace —
        // push back the settle baseline so the post-load restart waits for the
        // codegen to stabilise before it fires.
        if mcu_changed {
            self.last_workspace_change = Some(std::time::Instant::now());
        }

        // ── "Show me this pin in the code" ────────────────────────────────────
        // Deliberately AFTER the regen above: a click that also assigns the pin's
        // function leaves main.rs one frame stale, so the request is queued on the
        // MCU and resolved here, against freshly generated sources.
        // A module click resolves to the pins it wires, so both requests end up
        // as the same "light these pins up" call.
        let goto_pins: Vec<usize> = match &mut self.mcu {
            Some(m) => {
                let pin = m.pin_goto.take();
                let module = m.module_goto.take();
                match (pin, module) {
                    (Some(n), _) => vec![n],
                    (None, Some(id)) => m
                        .modules
                        .iter()
                        .find(|md| md.id == id)
                        .map(|md| {
                            let mut v: Vec<usize> =
                                md.connections.iter().map(|c| c.mcu_pin).collect();
                            v.sort_unstable();
                            v.dedup();
                            v
                        })
                        .unwrap_or_default(),
                    (None, None) => Vec::new(),
                }
            }
            None => Vec::new(),
        };
        if !goto_pins.is_empty() {
            let now = ui.input(|i| i.time);
            self.goto_pins_in_code(&goto_pins, now);
        }

        // Tick flash counters down
        if self.copy_flash > 0 {
            self.copy_flash -= 1;
        }
        if self
            .export_status_until
            .is_some_and(|t| t <= std::time::Instant::now())
        {
            self.export_status_until = None;
        }

        // ── LSP lifecycle ─────────────────────────────────────────────────────
        let lsp_status = self.lsp_state.lock().unwrap().status.clone();
        match lsp_status {
            LspStatus::Stopped => {
                if self.has_project() {
                    let build_dir = crate::workspace::dir();
                    if self.selected_build_cfg().is_some() {
                        if project_gen::write_project(
                            &build_dir,
                            &self.current_project_files(),
                            &self.project_tree.user_src_files,
                            &self.mcu_config_text(),
                            &self.structure_config_text(),
                        )
                        .is_ok()
                        {
                            self.lsp_indexing_since = None; // fresh session
                            lsp::start(
                                &build_dir,
                                Arc::clone(&self.lsp_state),
                                self.egui_ctx.clone(),
                            );
                        }
                    }
                }
            }
            LspStatus::Indexing => {
                // Deliberately NOT opening the document here. `Indexing` is set
                // the moment the `initialize` handshake completes — long before
                // RA has fetched metadata and built the crate graph — and RA
                // answers a `didOpen` immediately, analysing the file as a
                // DETACHED one: no sysroot, so no `core`. Without `core` the
                // `Unsize` lang item is missing, `&mut [u8; 32]` no longer
                // coerces to `&mut [u8]`, and the length const can't be
                // evaluated — the exact "expected &mut [u8], found &mut [u8; _]"
                // false error that used to greet every startup. It then stuck,
                // because RA re-verifies only on Save. `Ready` means the
                // workspace finished loading (indexing `$/progress` end), so the
                // document is opened in the arm below instead.
                self.open_main_rs_when_indexed();
            }
            LspStatus::Ready => {
                // Same gate as during Indexing: `Ready` alone is not proof the
                // crate graph is built (it flips on the first `$/progress end`
                // of any rust-prefixed token, e.g. "Fetching metadata").
                self.open_main_rs_when_indexed();
                // RA re-verifies ONLY on an explicit Project Save (Ctrl+S / Save
                // button / project reload) — never while typing, so editing stays
                // light. The Save re-syncs every file to RA and re-runs its checks
                // (didChange + workspace disk write + didSave). Completions still
                // sync their own text on demand (see completion.rs), so they work
                // on the current text between saves.
                // Runs on a WORKER thread so the save frame stays short (the
                // synchronous flush froze the UI — and the status spinner —
                // for the whole disk-write + didChange span). While a flush is
                // in flight the request stays set; the worker wakes the UI
                // when done and the queued flush fires on that frame.
                if self.lsp_flush_requested
                    && !self
                        .lsp_flush_in_flight
                        .load(std::sync::atomic::Ordering::Acquire)
                {
                    self.lsp_flush_requested = false;
                    self.spawn_lsp_flush();
                }

                // Post-load clean-up of a possibly-stale first analysis (the
                // freshly-reset Cargo.lock is still re-resolving; late config
                // files weren't indexed). It sticks because RA only re-checks on
                // Save, so once RA is Ready, the workspace has been stable for
                // `LSP_SETTLE` and there ARE diagnostics, force a re-analysis.
                //
                // Two stages, cheapest first: a forced re-verify of the open
                // documents (a version-bumped no-op `didChange` — what your own
                // "type a line and save" does by hand) and, only if diagnostics
                // survive that, the full restart. Restarting first was the old
                // behaviour and could RE-CREATE the very errors it was meant to
                // clear: the relaunched RA went through the same too-early open.
                const LSP_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);
                if !self.lsp_settle_recheck_done && self.has_project() {
                    let settled = self
                        .last_workspace_change
                        .is_some_and(|t| t.elapsed() > LSP_SETTLE);
                    // Wait for any in-flight check too: RA's first `cargo check`
                    // re-resolves the reset Cargo.lock — acting before it
                    // finishes would just make RA re-resolve again.
                    let (has_diags, checking) = {
                        let lsp = self.lsp_state.lock().unwrap();
                        (!lsp.diagnostics.is_empty(), lsp.checking)
                    };
                    if settled && !checking {
                        // A clean load needs nothing.
                        if !has_diags {
                            self.lsp_settle_recheck_done = true;
                        } else if !self.lsp_settle_reverified {
                            self.lsp_settle_reverified = true;
                            self.reverify_open_documents();
                            // Give RA a fresh settle window to re-publish before
                            // the stage above is judged.
                            self.last_workspace_change = Some(std::time::Instant::now());
                        } else {
                            self.lsp_settle_recheck_done = true;
                            self.restart_lsp();
                        }
                    }
                }
            }
            _ => {}
        }

        // ── Code actions (Ctrl+Enter) — list / choice / resolve, applied at
        //    frame top so edits survive the editor's end-of-frame write-back. ─
        self.poll_code_actions();

        // ── Inline type hint — receive the cursor-line result and apply a Tab
        //    accept at frame top (same write-back-revert dodge as above). ──────
        self.poll_inlay_hint();

        // ── Apply a completed rename (textDocument/rename) across files ───────
        if self.rename_in_flight {
            let result = self.lsp_state.lock().unwrap().take_rename_result();
            if let Some(edits) = result {
                self.rename_in_flight = false;
                if !edits.is_empty() {
                    self.apply_rename_edits(edits);
                    // RA's reference search does not reach every position (a
                    // const-generic argument body is the known case), and it
                    // reports success regardless — so the stale name would
                    // otherwise surface much later as a compile error. Audit
                    // and show what is left; never edit it automatically.
                    let old = std::mem::take(&mut self.rename_old_name);
                    let new = std::mem::take(&mut self.rename_new_name);
                    self.report_rename_leftovers(&old, &new);
                }
                // Continue an "Apply all" / queued batch: fire the next rename. When
                // the queue is drained, re-run clippy once so the list reflects all
                // the renamed code (clippy_rename_pending was set when the batch
                // started).
                if !self.start_next_queued_rename() && self.clippy_rename_pending {
                    self.clippy_rename_pending = false;
                    self.start_clippy_run();
                }
                self.egui_ctx.request_repaint();
            }
        }

        self.poll_pending_goto();

        // ── Handle a completed F12 go-to-definition ──────────────────────────
        // A definition in the CURRENT project opens editable in the main editor
        // (navigate + scroll the line into view). A definition in another file
        // (crate / std) is shown read-only in the Definition tab snippet.
        if self.definition_in_flight {
            let result = self.lsp_state.lock().unwrap().take_definition_result();
            if let Some(loc) = result {
                self.definition_in_flight = false;
                if let Some(loc) = loc {
                    if let Some(id) =
                        project_file_for_def(&loc.path, &self.project_tree.user_src_files)
                    {
                        // Editable: open the file, scroll to the definition, and
                        // mark the def line with a yellow band (like the Def tab).
                        self.selected_file = id;
                        self.pending_scroll_to_line = Some((id, loc.line as usize + 1));
                        self.highlighted_def_line = Some((id, loc.line as usize + 1));
                        // Not an error — clear the snippet; the MCU tab bar
                        // auto-leaves the (now empty) Definition tab.
                        self.definition_view = None;
                    } else if let Some(view) = build_definition_view(&loc) {
                        // External file → read-only snippet in the Definition
                        // tab (MCU Configurator), scrolled to the target line.
                        if self.active_tab != McuTab::Definition {
                            self.definition_return_tab = self.active_tab;
                        }
                        self.definition_view = Some(view);
                        self.active_tab = McuTab::Definition;
                        self.def_scroll_pending = true;
                        // That tab lives in the middle zone, so a collapsed
                        // layout would swallow the snippet — F12 would look
                        // like it did nothing. Open the zone back up.
                        self.side_panels_collapsed = false;
                    }
                } else {
                    // Answered, but with nothing. Silence here read as "the key
                    // did nothing", and a just-started analyzer answers this way
                    // more often than a warm one — an empty result and a
                    // JSON-RPC error arrive in the same shape.
                    self.set_status_msg(format!(
                        "{} No definition found for the symbol at the caret",
                        egui_phosphor::regular::X_CIRCLE
                    ));
                }
                self.egui_ctx.request_repaint();
            }
        }

        // ── Commit finished RA flycheck spans to the Activity log ─────────────
        // The post-save "Checking…" wall time lives inside rust-analyzer, so no
        // in-app recorder can wrap it; RA's `$/progress` begin/end timestamps
        // are collected in `finished_checks` and logged here as their own
        // entry — THIS is the seconds-long tail a Save actually costs. The
        // queue span separates a clogged RA (long wait before cargo starts)
        // from a genuinely slow `cargo check` (long run).
        let spans = {
            let mut lsp = self.lsp_state.lock().unwrap();
            std::mem::take(&mut lsp.finished_checks)
        };
        for (queued, ran) in spans {
            let mut rec = crate::activity::Recorder::new("Check (RA flycheck)");
            rec.add(
                "waiting in rust-analyzer's queue (didSave -> check start)",
                queued,
            );
            rec.add("cargo check run", ran);
            self.activity
                .lock()
                .unwrap()
                .push(rec.finish_with_total(queued + ran));
        }

        self.tick_save_wall();
    }

    /// Advance the "Save (wall clock)" envelope: note when the LSP flush
    /// finishes, and once the flycheck the save triggered is done (or nothing
    /// triggered one, or 120 s passed) log the whole user-perceived span to
    /// Activity — the entry that should match what the save FELT like,
    /// decomposed into milestones.
    fn tick_save_wall(&mut self) {
        let Some(mut w) = self.save_wall.take() else {
            return;
        };
        let now = std::time::Instant::now();

        // The LSP flush is done once its request was consumed and no worker is
        // in flight. (`did_save` — which sets `flycheck_pending` — happens
        // BEFORE the worker clears the in-flight flag, so the completion check
        // below can't race past a just-triggered check.)
        if w.flush_done.is_none()
            && !self.lsp_flush_requested
            && !self
                .lsp_flush_in_flight
                .load(std::sync::atomic::Ordering::Acquire)
        {
            w.flush_done = Some(now);
        }

        let (checking, pending) = {
            let lsp = self.lsp_state.lock().unwrap();
            (lsp.checking, lsp.flycheck_pending())
        };
        let done = w.flush_done.is_some() && !checking && !pending;
        let timed_out = now - w.started > std::time::Duration::from_secs(120);
        if !(done || timed_out) {
            self.save_wall = Some(w); // still running — keep the clock
            return;
        }

        let mut rec =
            crate::activity::Recorder::new("Save (wall clock)").in_session(self.save_session);
        if let Some(t) = w.worker_done {
            rec.add("click -> project written to disk", t - w.started);
        }
        if let Some(t) = w.flush_done {
            rec.add("click -> LSP flush finished", t - w.started);
        }
        if timed_out {
            rec.mark("timed out waiting for the flycheck (rust-analyzer not Ready?)");
        } else {
            rec.add(
                "click -> inline diagnostics fresh (flycheck done)",
                now - w.started,
            );
        }
        rec.mark("milestones measured FROM THE CLICK — they overlap, not add up");
        self.activity
            .lock()
            .unwrap()
            .push(rec.finish_with_total(now - w.started));
    }

    /// Apply rename edits returned by rust-analyzer to the in-memory files
    /// (main.rs + user source files). Per file, edits are applied back-to-front
    /// so earlier positions stay valid; the debounce then re-syncs RA.
    fn apply_rename_edits(&mut self, edits: Vec<lsp::RenameEdit>) {
        use std::collections::HashMap;
        let mut by_file: HashMap<String, Vec<lsp::RenameEdit>> = HashMap::new();
        for e in edits {
            by_file.entry(e.rel_path.clone()).or_default().push(e);
        }
        for (rel, es) in by_file {
            if rel == "src/main.rs" {
                self.generated_code = apply_text_edits(&self.generated_code, es);
            } else {
                if let Some(entry) = self
                    .project_tree
                    .user_src_files
                    .iter_mut()
                    .find(|(p, _)| *p == rel)
                {
                    entry.1 = apply_text_edits(&entry.1, es);
                }
            }
        }
    }

    /// Single point where rust-analyzer is asked to re-verify: writes main.rs +
    /// every user source file to the LSP workspace (so cargo-check/flycheck sees
    /// them), pushes `didChange` (RA's in-memory analysis) for the files that
    /// actually changed and `didSave` (RA's flycheck) when anything did.
    /// Called **only on a Project Save** — never while typing.
    ///
    /// Delete the temp check-workspace's `Cargo.lock` so the next `cargo check`
    /// re-resolves dependencies. Called ONLY when the chip/toolchain changes or a
    /// project is opened (the deps differ then); saves keep the lock so checks
    /// stay fast. The user's own project `Cargo.lock` is left untouched.
    fn reset_workspace_lock(&self) {
        let workspace = crate::workspace::dir();
        let _ = std::fs::remove_file(workspace.join("Cargo.lock"));
    }

    /// Content hash used by [`AppIde::flushed_hashes`] (SipHash via `DefaultHasher`).
    fn content_hash(content: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut h);
        h.finish()
    }

    /// Write one file into the RA workspace, skipping disk I/O when possible.
    /// `cache` maps `rel` → the hash of the content we last wrote there. Since
    /// the IDE is the only writer of this workspace, the cache is authoritative:
    /// a hash match means the disk already has this content (no read, no write);
    /// a known-stale entry means we can write directly WITHOUT a compare read;
    /// only a brand-new path falls back to a disk read (to avoid a needless
    /// mtime bump if it already matches, e.g. a reopened project).
    /// Returns `true` if it actually wrote to disk (for the Activity diagnostic).
    fn write_workspace_file(
        cache: &mut std::collections::HashMap<String, u64>,
        workspace: &std::path::Path,
        rel: &str,
        content: &str,
    ) -> bool {
        let hash = Self::content_hash(content);
        match cache.get(rel) {
            Some(&h) if h == hash => return false, // unchanged since last flush
            Some(_) => {}                          // known-stale → write directly
            None => {
                let dest = workspace.join(rel);
                if std::fs::read(&dest).is_ok_and(|d| d == content.as_bytes()) {
                    cache.insert(rel.to_string(), hash);
                    return false;
                }
            }
        }
        let dest = workspace.join(rel);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&dest, content.as_bytes()).is_ok() {
            cache.insert(rel.to_string(), hash);
        }
        true
    }

    /// Hand `src/main.rs` to rust-analyzer — but only once RA has reported its
    /// indexing pass finished, i.e. the crate graph and sysroot are loaded.
    ///
    /// Opening earlier is what produced the "expected `&mut [u8]`, found
    /// `&mut [u8; _]`" errors that greeted every startup: RA answers a `didOpen`
    /// immediately, and with no sysroot loaded the file is analysed detached —
    /// without `core` there is no `Unsize` lang item, so `&mut [u8; 32]` no
    /// longer coerces to `&mut [u8]` and the length const can't be evaluated
    /// (hence the `_`). Those false errors then stuck, because RA re-verifies
    /// only on Save.
    ///
    /// The timeout is a safety valve for an RA that never reports indexing as
    /// finished: without it the document would never be opened at all. A
    /// possibly-early analysis beats none, and the settle re-verify cleans up.
    fn open_main_rs_when_indexed(&mut self) {
        const OPEN_FALLBACK: std::time::Duration = std::time::Duration::from_secs(20);
        let since = *self
            .lsp_indexing_since
            .get_or_insert_with(std::time::Instant::now);
        let mut lsp = self.lsp_state.lock().unwrap();
        if lsp.is_file_open("src/main.rs") {
            return;
        }
        if lsp.indexed || since.elapsed() > OPEN_FALLBACK {
            lsp.did_open("src/main.rs", &self.generated_code.clone());
        }
    }

    /// Make rust-analyzer re-run its analysis of the documents it already has
    /// open, with no edit: a version-bumped `didChange` carrying the identical
    /// text (`force`), plus one `didSave` so the flycheck re-runs too.
    ///
    /// This is the programmatic form of what clears a stale first analysis by
    /// hand — type a character and save. Note the normal Save flush deliberately
    /// uses `force = false` (it must not re-analyse the whole project on every
    /// save), which is exactly why it can NOT clear a diagnostic computed from
    /// unchanged text; hence this separate, targeted path. Only already-open
    /// documents are touched — `did_change` would otherwise auto-open every user
    /// file and flood RA.
    fn reverify_open_documents(&mut self) {
        let mut docs: Vec<(String, String)> = vec![(
            crate::project_tree::logic::src_path("main.rs"),
            self.generated_code.clone(),
        )];
        docs.extend(
            self.project_tree
                .user_src_files
                .iter()
                .map(|(rel, content)| (rel.clone(), content.clone())),
        );
        let mut lsp = self.lsp_state.lock().unwrap();
        let mut resent = false;
        for (rel, text) in &docs {
            if lsp.is_file_open(rel) {
                lsp.did_change(rel, text, true);
                resent = true;
            }
        }
        // One save is enough — the flycheck it triggers covers the workspace.
        if resent {
            lsp.did_save(&crate::project_tree::logic::src_path("main.rs"));
        }
    }

    fn spawn_lsp_flush(&mut self) {
        // The flag is set here and cleared by a `FlagGuard` inside the worker
        // (see below) — NOT by a store on the worker's last line, which a panic
        // would skip, hanging the status bar at "Saving…" forever.

        // Same-frame snapshot of every file, so what reaches disk + RA is
        // exactly what the user saved. Every path here is PROJECT-ROOT-relative
        // — the shape `write_workspace_file` writes and `did_change` sends to
        // rust-analyzer. It used to be `src/`-relative with the prefix added by
        // those two; main.rs was the one hardcoded entry, so when the prefixes
        // moved out it silently became `<root>/main.rs`: cargo still compiled
        // the real `src/main.rs`, but RA analysed a stray copy keyed `main.rs`,
        // so nothing that looks diagnostics up by `src/main.rs` — inline
        // squiggles, the Structure error badge, the RA tab's jump-to-error —
        // found anything.
        let files: Vec<(String, String)> = std::iter::once((
            crate::project_tree::logic::src_path("main.rs"),
            self.generated_code.clone(),
        ))
        .chain(
            self.project_tree
                .user_src_files
                .iter()
                .map(|(rel, content)| (rel.clone(), content.clone())),
        )
        .collect();

        let hashes = Arc::clone(&self.flushed_hashes);
        let lsp_state = Arc::clone(&self.lsp_state);
        let activity = Arc::clone(&self.activity);
        let in_flight = Arc::clone(&self.lsp_flush_in_flight);
        let ctx = self.egui_ctx.clone();
        // The flush is part of the Save that requested it.
        let session = self.save_session;

        std::thread::spawn(move || {
            // Clears `lsp_flush_in_flight` on EVERY exit path, unwinding
            // included. Declared first so it outlives everything below.
            let _flag = crate::activity::FlagGuard::set(in_flight);
            let mut rec = crate::activity::Recorder::new("Save (LSP flush)").in_session(session);
            let workspace = crate::workspace::dir();

            // ── Disk writes (hash-cached) ─────────────────────────────────
            // Diagnostic: count how many files actually hit disk vs were
            // cache-skipped, and which single file was slowest — so the
            // Activity tab shows whether the cost is writes, reads, or one
            // specific file (contention).
            let t_write = std::time::Instant::now();
            let mut wrote = 0usize;
            let mut slowest = (String::new(), std::time::Duration::ZERO);
            {
                let mut cache = hashes.lock().unwrap();
                for (rel, content) in &files {
                    let t = std::time::Instant::now();
                    if Self::write_workspace_file(&mut cache, &workspace, rel, content) {
                        wrote += 1;
                    }
                    let d = t.elapsed();
                    if d > slowest.1 {
                        slowest = (rel.clone(), d);
                    }
                }
            }
            let total = files.len();
            rec.add("write files to RA workspace", t_write.elapsed());
            rec.mark(format!(
                "wrote {wrote}/{total} to disk · slowest: {} {}",
                if slowest.0.is_empty() {
                    "—"
                } else {
                    &slowest.0
                },
                crate::activity::fmt_dur(slowest.1),
            ));

            // ── Sync text to RA ───────────────────────────────────────────
            // Only files whose text differs from what RA already holds are
            // re-sent (`force = false`). The old forced re-send of EVERY
            // document bumped every doc version on every save, making RA
            // re-analyse the whole project each time. One SHORT lock per file
            // (not one big lock around the loop): the UI thread takes this
            // mutex every frame — holding it across the whole loop would just
            // move the freeze from "our frame" into its lock wait.
            let t_sync = std::time::Instant::now();
            let mut synced = 0usize;
            for (rel, content) in &files {
                let workspace_rel = rel;
                if lsp_state
                    .lock()
                    .unwrap()
                    .did_change(&workspace_rel, content, false)
                {
                    synced += 1;
                }
            }
            rec.add("did_change (sync text to RA)", t_sync.elapsed());
            rec.mark(format!(
                "re-synced {synced}/{total} files (unchanged skipped)"
            ));

            // Trigger RA's `checkOnSave` flycheck (cargo check) so real compiler
            // errors — E0425 "cannot find value", type mismatches, unused vars, … —
            // refresh inline against the just-flushed text. RA's native pass alone
            // does NOT publish these for nested user files, so this is what makes
            // inline errors work at all. It runs asynchronously in RA; its REAL
            // duration (queue + run) lands in the Activity log as its own
            // "Check (RA flycheck)" entry when RA reports it done (that's the
            // seconds-long tail a Save actually costs — see `finished_checks`).
            // Skipped entirely when nothing changed: an idle Ctrl+S must not
            // re-run a whole cargo check on identical code.
            if synced > 0 || wrote > 0 {
                rec.phase("did_save (trigger RA flycheck)", || {
                    lsp_state.lock().unwrap().did_save("src/main.rs");
                });
            } else {
                rec.mark("nothing changed since last flush — flycheck not re-triggered");
            }

            activity.lock().unwrap().push(rec.finish());
            // `_flag` clears the in-flight flag as it drops, just below.
            // Wake `init_frame`: a save made during this flush left
            // `lsp_flush_requested` set and must fire now.
            ctx.request_repaint();
        });
    }

    /// Keep the MCU zone and the project tree from being open together in a
    /// window with no room for both beside the editor — see
    /// [`room_for_three_columns`] for what counts as room.
    ///
    /// The MCU Configurator is the `CentralPanel` — it has no width of its own
    /// and gets whatever the two side panels leave. With all three open in a
    /// cramped window that remainder collapses to a strip of chrome, and the pin
    /// canvas drawn in it spills over the tree. Portrait displays and a window
    /// docked to half a landscape screen both land here.
    ///
    /// **The tree wins.** It is how you navigate the project; the MCU
    /// Configurator is somewhere you go deliberately, and the toolbar button
    /// that brings it back is on screen the whole time.
    ///
    /// The pair the user had while wide is remembered and put back verbatim
    /// when the window grows again — including any change they made while
    /// narrow, which is treated as temporary, exactly like the narrow layout
    /// itself. Dragging a window narrow and back must not quietly rearrange a
    /// layout the user chose.
    fn enforce_narrow_layout(&mut self, ui: &egui::Ui) {
        // The window's OUTER width against the monitor: "more than half the
        // screen" is about the window the user dragged, decorations included,
        // not about the content area left after our own chrome. Both are in
        // points, so they compare directly. No outer rect (some backends don't
        // report one) → treat the window as unbounded and let the display
        // orientation and the width floor decide.
        let (window_w, monitor) = ui.ctx().input(|i| {
            let vp = i.viewport();
            (
                vp.outer_rect.map_or(f32::INFINITY, |r| r.width()),
                vp.monitor_size,
            )
        });
        let room = room_for_three_columns(ui.available_width(), window_w, monitor);
        self.layout_narrow = !room;
        let (mcu, tree, wide) = narrow_layout_rule(
            room,
            self.side_panels_collapsed,
            self.tree_collapsed,
            self.wide_layout,
        );
        self.side_panels_collapsed = mcu;
        self.tree_collapsed = tree;
        self.wide_layout = wide;
    }
}

impl eframe::App for AppIde {
    // ── Persistence: called by eframe on app exit (and periodically) ──────────
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // While the window is too narrow to hold both right-hand zones, the live
        // flags are a FORCED layout, not a chosen one (see
        // `enforce_narrow_layout`) — persist what the user actually had, so the
        // next start on a bigger screen opens the way they left it.
        let (side_panels_collapsed, tree_collapsed) = self
            .wide_layout
            .unwrap_or((self.side_panels_collapsed, self.tree_collapsed));
        let mut state = PersistedState {
            user_src_files: self.project_tree.user_src_files.clone(),
            user_src_folders: self.project_tree.user_src_folders.clone(),
            paths_root_relative: true,
            project_name: self.project_name.clone(),
            project_dir: self
                .project_dir
                .as_ref()
                .and_then(|p| p.to_str())
                .map(String::from),
            side_panels_collapsed,
            tree_collapsed,
            diag_collapsed: self.diag_collapsed,
            tree_split_ratio: self.tree_split_ratio,
            hide_diff_line_bg: !self.diff_line_bg,
            esp_monitor_no_auto: !self.esp_monitor_auto,
        };
        // Never write buffers that have no folder to be restored onto (see
        // `drop_homeless_files`). This is also what makes "Close without saving"
        // true: eframe calls `save` on the way out — and every 30s — so the
        // files the user chose to discard used to be written anyway, leaving no
        // way to be rid of them.
        state.drop_homeless_files();
        eframe::set_value(storage, STORAGE_KEY, &state);
    }

    // ── App exit: terminate rust-analyzer ─────────────────────────────────────
    // Nothing else kills the RA child when the app closes (dropping a
    // `std::process::Child` only detaches it), so every app restart used to
    // leave an orphaned rust-analyzer + proc-macro server behind — each still
    // watching and re-analyzing the workspace on every file write, compounding
    // the "everything gets slower" degradation across restarts.
    fn on_exit(&mut self) {
        // Persist the Structure diagram's layout. It is deliberately NOT part
        // of the unsaved-changes snapshot any more (dragging a node must not
        // make the project look modified), so nothing else would write it when
        // a drag is the only thing that happened. Written even on "Close
        // without saving": this is gitignored view state, not project content.
        if let Some(root) = &self.project_dir {
            let text = self.structure_config_text();
            let path = root.join(crate::panels::mcu_module::structure_config::FILE_NAME);
            if text.trim().is_empty() {
                let _ = std::fs::remove_file(&path);
            } else {
                let _ = std::fs::write(&path, text);
            }
        }
        self.lsp_state.lock().unwrap().kill_child();
        // Orphaned probe-rs processes would keep the debug probe locked for
        // the next app start — kill them synchronously.
        self.rtt.stop();
        self.debugger.kill_now();
        // Same for espflash: an orphan keeps the serial port open, and the next
        // flash then fails to claim it ("Access is denied").
        self.esp_monitor.stop();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // ── Closing with unsaved work? Ask before losing it ───────────────────
        // Cancel the close and put a prompt up; `allow_close` marks the close
        // WE send after the user decided, so we don't intercept ourselves.
        if ui.ctx().input(|i| i.viewport().close_requested()) && !self.allow_close {
            if self.unsaved_files().is_empty() {
                self.allow_close = true; // nothing to lose — let it go
            } else {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.exit_prompt = true;
            }
        }

        // A flash that just finished re-measures Flash/RAM (Flash tab row).
        // Here, not in the diag panel: that panel can be hidden, and the edge
        // would be missed.
        self.poll_flash_finished_size();

        // ── UI-stall detector ─────────────────────────────────────────────────
        // A gap between frames while background work was pending means the
        // event loop slept through finished work (the "one-minute save"
        // symptom). The activity watchdog caps this at ~250 ms — an entry
        // showing up here on a current build points at a wake-up path we
        // haven't covered yet.
        let now_frame = std::time::Instant::now();
        if let (Some(prev), true) = (self.last_frame_at, self.was_busy_last_frame) {
            let gap = now_frame - prev;
            if gap >= std::time::Duration::from_secs(1) {
                let mut rec = crate::activity::Recorder::new("UI stall (frames stopped)");
                rec.add("gap between frames while work was pending", gap);
                rec.mark("event loop slept through pending work — lost wake-up?");
                self.activity
                    .lock()
                    .unwrap()
                    .push(rec.finish_with_total(gap));
            }
        }
        self.last_frame_at = Some(now_frame);

        // Initialize frame state (polling, LSP, MCU updates)
        self.init_frame(ui);

        // Detect Ctrl+S for Save/Export project
        let ctrl_s_pressed = ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S));

        // ── Bottom status bar ─────────────────────────────────────────────────
        // A persistent full-width strip for long-running activity (Saving /
        // Building / Flashing / Checking) and the last save result — with room
        // for future statuses. Declared before the side panels so it claims the
        // bottom edge across the whole window.
        let status = self.activity_status();
        // Watchdog: while ANYTHING is running, keep frames coming from the UI
        // thread itself. Cross-thread `request_repaint()` wake-ups (save
        // worker, LSP reader) travel as winit user events stamped with the
        // render-pass number they were issued at, and eframe DROPS them as
        // "outdated" when two or more passes ran in between (run.rs: "Got
        // outdated UserEvent::RequestRepaint"). A dropped LAST wake-up left
        // the event loop asleep with finished work unprocessed until a window
        // event — observed as a "one-minute save" that completed instantly
        // when the window was moved/minimized (Activity meanwhile showed the
        // true, small durations). A UI-thread `request_repaint_after` goes
        // through the per-frame repaint-delay path, which can't be dropped.
        if status.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        // Feed the UI-stall detector at the top of the next frame.
        self.was_busy_last_frame = status.is_some();
        egui::Panel::bottom("status_bar")
            .exact_size(24.0)
            .show_inside(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(8.0);
                    if let Some((spinner, text, color)) = &status {
                        if *spinner {
                            // Throttled (~10 FPS): egui's Spinner forces a
                            // repaint EVERY frame for its whole lifetime —
                            // i.e. the entire Saving/Checking/Flashing span.
                            helpers::spinner::throttled_spinner(ui, 13.0);
                            ui.add_space(5.0);
                        }
                        ui.label(egui::RichText::new(text).size(11.0).color(*color));
                    }
                });
            });

        // ── Missing-dependency banner (startup self-check) ────────────────────
        // The IDE shells out to rustup / rustc / probe-rs / the MSVC linker …;
        // when one is absent the failure used to surface only as a cryptic error
        // deep in a build log. Check once at startup (background thread) and say
        // plainly what is missing and what it costs. Only BLOCKING problems get a
        // banner — a missing cargo-bloat just greys out one tab.
        if !self.deps_checked {
            self.deps_checked = true;
            required_tools::start_check_all(Arc::clone(&self.tools_state), ui.ctx().clone());
        }
        if !self.deps_banner_dismissed {
            let tc = self.selected_toolchain();
            // The triple decides WHICH esp tools matter: `rustup target add`
            // for the RISC-V parts, Espressif's whole rustc fork for Xtensa.
            let target = self.selected_target();
            // One lock for everything the banner draws: the problems, the
            // specific finding under each (which names the thing `impact` can
            // only call `<NAME>`), and whether the install hint applies at all.
            let (problems, details, any_missing) = {
                let s = self.tools_state.lock().unwrap();
                let p: Vec<_> = s
                    .problems_for(tc.as_ref(), target.as_deref())
                    .into_iter()
                    .filter(|(_, sev, _)| *sev == crate::required_tools::Severity::Blocking)
                    .collect();
                let d: Vec<Option<String>> = p.iter().map(|(n, _, _)| s.status_detail(n)).collect();
                let m = s.any_blocking_missing_for(tc.as_ref(), target.as_deref());
                (p, d, m)
            };
            if !problems.is_empty() {
                let mut open_tools = false;
                let mut dismiss = false;
                egui::Panel::top("missing_deps_banner").show_inside(ui, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(58, 40, 20))
                        .inner_margin(7.0)
                        .show(ui, |ui| {
                            let names: Vec<&str> = problems.iter().map(|(n, _, _)| *n).collect();
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    // NOT "Missing required dependencies": the
                                    // banner also carries problems that are the
                                    // opposite of missing — a stray
                                    // `CARGO_FEATURE_*` in the environment is
                                    // reported because it is THERE. The bullets
                                    // below say what each one actually is.
                                    egui::RichText::new(format!(
                                        "{} Build environment problems: {}",
                                        egui_phosphor::regular::WARNING,
                                        names.join(", ")
                                    ))
                                    .size(11.5)
                                    .strong()
                                    .color(egui::Color32::from_rgb(245, 200, 130)),
                                );
                            });
                            for ((name, _, impact), detail) in problems.iter().zip(&details) {
                                ui.label(
                                    egui::RichText::new(format!("• {name} — {impact}"))
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(225, 195, 155)),
                                );
                                // What the check actually found, indented under
                                // its entry. `impact` is written before anything
                                // is probed, so it can only say `<NAME>`; this
                                // line is where the name lands — without it the
                                // banner tells you to delete a variable and
                                // never says which one.
                                if let Some(detail) = detail {
                                    // A phosphor icon, never a raw `↳`: the
                                    // bundled fonts have no arrow glyphs, so
                                    // that renders as an empty box. Cost me a
                                    // screenshot from the user to notice.
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "     {} {detail}",
                                            egui_phosphor::regular::ARROW_ELBOW_DOWN_RIGHT
                                        ))
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(245, 200, 130)),
                                    );
                                }
                            }
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .button(
                                        egui::RichText::new(format!(
                                            "{} Open Tools",
                                            egui_phosphor::regular::WRENCH
                                        ))
                                        .size(10.5),
                                    )
                                    .on_hover_text("Check / install the missing dependencies")
                                    .clicked()
                                {
                                    open_tools = true;
                                }
                                if ui.button("Dismiss").clicked() {
                                    dismiss = true;
                                }
                                // Only where it is true. This is advice about
                                // INSTALLING something, and re-checking is the
                                // action it recommends — neither applies to a
                                // problem like a stray `CARGO_FEATURE_*`, where
                                // nothing was installed and a re-check reads the
                                // same inherited environment. Showing it there
                                // pointed at the one action that cannot work.
                                if any_missing {
                                    ui.label(
                                        egui::RichText::new(
                                            "(installed it just now? re-check in Tools — a tool \
                                             installed after the IDE started isn't on its PATH)",
                                        )
                                        .size(9.5)
                                        .italics()
                                        .color(egui::Color32::from_gray(160)),
                                    );
                                }
                            });
                        });
                });
                if open_tools {
                    self.build_tab = BuildPanelTab::RequiredTools;
                    self.deps_banner_dismissed = true;
                }
                if dismiss {
                    self.deps_banner_dismissed = true;
                }
            }
        }

        // ── Bad `--project` argument banner ───────────────────────────────────
        // The window opened on something else than the caller asked for; say so
        // rather than letting a typo in a shortcut look like the IDE ignoring it.
        if let Some(err) = self.cli_project_error.clone() {
            let mut dismiss = false;
            egui::Panel::top("cli_project_error_banner").show_inside(ui, |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(58, 40, 20))
                    .inner_margin(7.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  Couldn't open the project from the command line: {err}",
                                    egui_phosphor::regular::WARNING
                                ))
                                .size(11.5)
                                .color(egui::Color32::from_rgb(245, 200, 130)),
                            );
                            if ui.button("Dismiss").clicked() {
                                dismiss = true;
                            }
                        });
                    });
            });
            if dismiss {
                self.cli_project_error = None;
            }
        }

        // ── Project-open-elsewhere banner ─────────────────────────────────────
        // The project's folder is claimed by another IDE window. Both windows
        // write the WHOLE project on save, so whoever saves last wins — silently,
        // and against the user's real files rather than a scratch copy. Nothing
        // is blocked (see `claim_open_project`); the risk is simply stated, and
        // the banner clears itself the moment the other window lets go.
        if let Some(name) = self.project_lock_conflict.clone() {
            egui::Panel::top("project_open_elsewhere_banner").show_inside(ui, |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(58, 40, 20))
                    .inner_margin(7.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  \"{name}\" is already open in another Embedded IDE \
                                     window.",
                                    egui_phosphor::regular::WARNING
                                ))
                                .size(11.5)
                                .strong()
                                .color(egui::Color32::from_rgb(245, 200, 130)),
                            );
                        });
                        ui.label(
                            egui::RichText::new(
                                "Both windows save the whole project into the same folder, so \
                                 the last save overwrites the other's work. Close it in one of \
                                 them — this notice clears by itself.",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_rgb(225, 195, 155)),
                        );
                    });
            });
        }

        // ── Workspace-load-failure banner (Part 3 safety net) ─────────────────
        // When `cargo metadata` fails, rust-analyzer never loads: no inline
        // errors, no Structure edges, a stuck "Checking…". Surface that plainly
        // (instead of the silent dead analyzer) with one-click recovery — detach
        // the offending library from the workspace.
        if let Some(err) = self.workspace_load_error.clone() {
            let members =
                crate::panels::mcu_module::project_gen::workspace_members(&self.cargo_toml);
            let mut detach: Option<String> = None;
            let mut dismiss = false;
            egui::Panel::top("workspace_load_error_banner").show_inside(ui, |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(58, 26, 26))
                    .inner_margin(7.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} rust-analyzer can't load this project — `cargo \
                                     metadata` failed. Inline errors, Structure edges and \
                                     completions stay off until it's fixed.",
                                    egui_phosphor::regular::WARNING,
                                ))
                                .size(11.5)
                                .strong()
                                .color(egui::Color32::from_rgb(240, 165, 150)),
                            );
                        });
                        ui.label(
                            egui::RichText::new(&err)
                                .size(10.5)
                                .monospace()
                                .color(egui::Color32::from_rgb(235, 140, 120)),
                        );
                        ui.horizontal_wrapped(|ui| {
                            if !members.is_empty() {
                                ui.label(
                                    egui::RichText::new("Detach a library to recover:")
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(220, 190, 150)),
                                );
                                for m in &members {
                                    if ui
                                        .button(
                                            egui::RichText::new(format!(
                                                "{} {m}",
                                                egui_phosphor::regular::LINK_BREAK
                                            ))
                                            .size(10.5),
                                        )
                                        .on_hover_text(format!(
                                            "Remove `{m}` from the workspace (keeps its files)"
                                        ))
                                        .clicked()
                                    {
                                        detach = Some(m.clone());
                                    }
                                }
                            }
                            if ui.button("Dismiss").clicked() {
                                dismiss = true;
                            }
                        });
                    });
            });
            if let Some(m) = detach {
                self.detach_lib_from_workspace(&m);
            }
            if dismiss {
                self.workspace_load_error = None;
            }
        }

        // Build project files snapshot (used by both tree and editor panels)
        // Snapshot from the live editable state (main.rs + the five editable
        // config files), so the tree/editor show current edits — not a fresh
        // regeneration that would discard them.
        let project_files: Option<ProjectFiles> = self
            .selected_build_cfg()
            .map(|_| self.current_project_files());

        // Narrow window? The MCU zone and the tree become a choice, not two
        // independent toggles. Must run BEFORE any panel is built — it decides
        // which ones exist this frame.
        self.enforce_narrow_layout(ui);

        // ── Panel 1: Code Editor (leftmost) ─────
        // The layout is [Editor][MCU][Project]: the editor docks left, the
        // project tree docks on the FAR RIGHT, the MCU Configurator (central)
        // takes the middle. Running the WHOLE editor panel (display_code
        // compute → render → write-back) before the tree keeps the old
        // write-back hazard away: a tree click changes `selected_file` only
        // after this frame's write-back finished, so stale text can never land
        // in the new file (the click's content shows next frame).
        // Collapsed, the editor becomes the CENTRAL panel, and egui requires the
        // central panel to come after every side panel — so it's rendered at the
        // MCU slot further down instead. Safe: the end-of-frame write-back
        // persists to the captured `displayed_file`, not to the live
        // `selected_file`, so a tree click earlier in the frame can't misfile
        // the text (see `show_editor_panel`).
        if !self.side_panels_collapsed {
            self.show_editor_panel(ui, &project_files);
        }

        // ── Panel 2: Project Tree (docked far right) ─────
        // `save_project_needed` is set when the tree mutates files/folders, so
        // the whole project gets rewritten to the workspace dir afterwards.
        // ALWAYS rendered — collapsing only hides the middle (MCU) zone.
        let mut save_project_needed = false;
        let signals =
            self.show_project_panel(ui, &project_files, ctrl_s_pressed, &mut save_project_needed);
        let mut open_project_clicked = signals.open_clicked;
        let new_project_clicked = signals.new_clicked;
        let save_project_clicked = signals.save_clicked;
        // Cross-instance tree clipboard. Copy stages the item and puts a token
        // on the system clipboard; paste reads a staged payload back.
        if let Some(req) = signals.clip_copy {
            self.apply_clip_copy(&ui.ctx().clone(), req);
        }
        if let Some(req) = signals.clip_paste {
            self.apply_clip_paste(&ui.ctx().clone(), req, &mut save_project_needed);
        }
        // "Extract to library crate…" on a tree folder → open the dialog.
        if let Some(folder) = signals.extract_folder {
            self.extract_crate = Some(extract_crate_dialog::ExtractCrateDialog::extract(folder));
        }
        // LIBRARIES "+" → the same dialog, in create-an-empty-library mode.
        if signals.new_library {
            self.extract_crate = Some(extract_crate_dialog::ExtractCrateDialog::new_library());
        }
        // LIBRARIES "clone from git" → the clone dialog.
        if signals.clone_library {
            self.clone_library_dialog = Some(extract_crate_dialog::CloneLibraryDialog::new());
        }
        // Project-header "Clone project" → the duplicate-to-a-new-folder dialog.
        if signals.clone_project {
            if let Some(dir) = self.project_dir.clone() {
                self.clone_project_dialog =
                    Some(clone_project_dialog::CloneProjectDialog::new(&dir));
            }
        }
        // A library's pen / trash icon → the confirmation dialog.
        if let Some((dir, is_rename)) = signals.library_action {
            self.library_action = Some(extract_crate_dialog::LibraryActionDialog {
                rename_to: is_rename.then(|| dir.clone()),
                dir,
                error: None,
            });
        }
        // "Add to workspace" on a detached library → run the cargo-metadata
        // pre-check; the member is applied only if it passes.
        if let Some(dir) = signals.add_to_workspace {
            self.add_detached_lib_to_workspace(dir);
        }
        // "Detach" a member library (remove from workspace, keep files).
        if let Some(dir) = signals.detach_from_workspace {
            self.detach_lib_from_workspace(&dir);
        }
        // Consume a finished "Add to workspace" pre-check (applies the member on
        // success, or surfaces the cargo error).
        self.poll_workspace_add();
        // Consume a finished workspace health check (sets/clears the load-error
        // banner).
        self.poll_workspace_health();
        // "Open beside editor" → show the file READ-ONLY in the Reference tab.
        // The editor keeps whatever it had open, which is the whole point.
        if let Some(path) = signals.open_reference {
            self.reference_file = Some(path);
            self.active_tab = McuTab::Reference;
            // The tab lives in the middle zone — reopen it if it was collapsed
            // away, or the file would silently go nowhere.
            self.side_panels_collapsed = false;
        }

        // ── Handle toolbar button clicks ──────────────────────────────────────

        // "New Project" → its own confirmation (chip picker) says the user files
        // are cleared, but never said WHAT would be lost — so the unsaved-changes
        // gate comes first, exactly as for Open Project.
        if new_project_clicked && self.save_in_progress.is_none() {
            if self.unsaved_files().is_empty() {
                self.begin_new_project();
            } else {
                self.new_prompt = true;
            }
        }

        // "Open Project" → folder picker, then load files. Loading REPLACES
        // everything in memory, so unsaved work is warned about first with the
        // same modal the window close uses.
        // "Open Recent" → the folder is already known, so it skips the picker
        // (see `pick_and_open_project`) but takes the SAME unsaved gate: it is
        // just as destructive as any other open.
        if let Some(dir) = signals.open_recent {
            if self.save_in_progress.is_none() {
                self.pending_open_dir = Some(dir);
                open_project_clicked = true;
            }
        }

        if open_project_clicked && self.save_in_progress.is_none() {
            if self.unsaved_files().is_empty() {
                self.pick_and_open_project(&mut save_project_needed);
            } else {
                self.open_prompt = true;
            }
        }

        // "Save Project" → write to the project's folder.
        //   • Existing project (opened/already saved → `project_dir` is set):
        //     save straight to that path, no dialog.
        //   • New project (`project_dir` is None): ask once where to save, then
        //     remember the chosen folder so later saves go there directly.
        // Ignore a fresh Save while one is still running (button left enabled,
        // but the click is a no-op until the worker finishes).
        // `request_save` lets the exit prompt drive the same save path as the
        // toolbar button (taken so it fires exactly once).
        // `take` first: `||` short-circuits, and leaving the flag set would fire
        // a second save on the next frame.
        let save_requested = std::mem::take(&mut self.request_save) || save_project_clicked;
        // Auto-build after this Save when a library changed in Cargo.toml.
        let mut auto_build_after_save = false;
        let mut auto_build_release = false;
        if save_requested && self.save_in_progress.is_none() && self.selected_build_cfg().is_some()
        {
            // A saved project goes back to its own folder. A NEW one asks for a
            // PARENT and gets a folder of its own underneath, named after the
            // chip — `STM32F217ZGTx`, then `_1`, `_2`, … if that is taken. Saving
            // straight into the picked folder used to scatter `Cargo.toml`,
            // `src/`, `memory.x` into it, and a second project chosen there would
            // have written over the first.
            let dest: Option<std::path::PathBuf> = match &self.project_dir {
                Some(dir) => Some(dir.clone()),
                None => {
                    let chip = self.selected_label();
                    rfd::FileDialog::new()
                        .set_title(format!(
                            "Choose where to create \"{}\" — a folder is made for it",
                            project_io::folder_name_for_chip(&chip)
                        ))
                        // A hint for the dialogs that show it; the folder is
                        // created from the parent either way.
                        .set_file_name(project_io::folder_name_for_chip(&chip))
                        .pick_folder()
                        .map(|parent| project_io::new_project_dir(&parent, &chip, |p| p.exists()))
                }
            };
            // Create it up front: the save worker writes files, and a missing
            // parent would fail per-file with a message about a path the user
            // never typed.
            if let Some(d) = &dest {
                if let Err(e) = std::fs::create_dir_all(d) {
                    self.export_msg = format!(
                        "{}  couldn't create {}: {e}",
                        egui_phosphor::regular::X_CIRCLE,
                        d.display()
                    );
                    self.export_status_until =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(4));
                }
            }
            if let Some(dest) = dest {
                // If a dependency was added/edited/removed since the last Save
                // (by the user editing Cargo.toml, or the codegen), run the
                // auto-build afterwards so the new deps resolve + compile — unless
                // the project's `auto_build` preference is Off. Skipped on the
                // very first Save (no baseline yet).
                use crate::panels::mcu_module::mcu::AutoBuild;
                let mode = self.mcu.as_ref().map(|m| m.auto_build).unwrap_or_default();
                let cur_deps = project_gen::deps_fingerprint(&self.cargo_toml);
                let deps_changed = self
                    .last_saved_deps
                    .as_deref()
                    .is_some_and(|prev| prev != cur_deps);
                self.last_saved_deps = Some(cur_deps);
                auto_build_after_save = deps_changed && mode != AutoBuild::Off;
                auto_build_release = mode == AutoBuild::Release;

                // One id for every action this Save spawns.
                self.save_session += 1;
                let session = self.save_session;
                // Run the disk write on a worker thread so the UI stays responsive
                // (the header shows a "Saving…" spinner until it completes).
                let files = self.current_project_files();
                let user_files = self.project_tree.user_src_files.clone();
                let mcu_cfg = self.mcu_config_text();
                let structure_cfg = self.structure_config_text();
                let shared: Arc<Mutex<Option<Result<String, String>>>> = Arc::new(Mutex::new(None));
                let out = Arc::clone(&shared);
                let ctx = self.egui_ctx.clone();
                let dest_thread = dest.clone();
                let activity = Arc::clone(&self.activity);
                std::thread::spawn(move || {
                    // Reports a failure if this thread dies before setting the
                    // result — otherwise the UI hangs on "Saving…".
                    let _slot = SaveSlotGuard(Arc::clone(&out));
                    let mut rec =
                        crate::activity::Recorder::new("Save (project)").in_session(session);
                    let res = rec
                        .phase("write_project", || {
                            project_gen::write_project(
                                &dest_thread,
                                &files,
                                &user_files,
                                &mcu_cfg,
                                &structure_cfg,
                            )
                        })
                        .map(|()| {
                            dest_thread
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("saved")
                                .to_string()
                        })
                        .map_err(|e| e.to_string());
                    activity.lock().unwrap().push(rec.finish());
                    *out.lock().unwrap() = Some(res);
                    ctx.request_repaint();
                });
                self.save_in_progress = Some(shared);
                self.save_dest = Some(dest);
                // Start the user-perceived save clock (see `SaveWall`).
                self.save_wall = Some(SaveWall {
                    started: std::time::Instant::now(),
                    worker_done: None,
                    flush_done: None,
                });
            }
        }
        // A library changed in Cargo.toml this Save → kick off the auto-build
        // (`cargo check`, or `cargo build --release` when the preference is
        // Release). Writes the check workspace + runs it; result in the Cargo
        // Check tab. Runs alongside the async disk-save, independent of it.
        if auto_build_after_save {
            self.start_build(auto_build_release);
        }

        // Apply a finished async save (set the result message / project home).
        let save_finished = self
            .save_in_progress
            .as_ref()
            .and_then(|s| s.lock().unwrap().take());
        if let Some(res) = save_finished {
            self.save_in_progress = None;
            if let Some(w) = &mut self.save_wall {
                w.worker_done.get_or_insert(std::time::Instant::now());
            }
            match res {
                Ok(name) => {
                    self.export_msg = format!("{}  {name}", egui_phosphor::regular::CHECK_CIRCLE);
                    self.export_status_until =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                    self.project_name = Some(name);
                    // A new project now has a home — later saves go here.
                    let had_dir = self.project_dir.clone();
                    self.project_dir = self.save_dest.take();
                    // First save into a folder → claim it. Re-claiming on every
                    // save would be wasted work (and would briefly release a
                    // folder we already hold), so only when it changed.
                    // The manifest on disk just changed, and it is what
                    // `cargo metadata` reads. Without this, fixing a broken
                    // dependency by hand left the "rust-analyzer can't load this
                    // project" banner up until the project was reopened — which
                    // reads exactly like the edit having done nothing.
                    if self.workspace_load_error.is_some() {
                        self.recheck_workspace_health();
                    }
                    if self.project_dir != had_dir {
                        self.claim_open_project();
                        // A project only becomes openable-by-path at its first
                        // Save, so this is where a NEW one enters the history.
                        if let Some(dir) = self.project_dir.clone() {
                            crate::recent::record(&dir, Some(&self.selected_mcu_id));
                        }
                    }
                    // "Save and close": the files are on disk now, so finish
                    // the close the prompt put on hold.
                    if self.close_after_save {
                        self.close_after_save = false;
                        self.exit_prompt = false;
                        self.allow_close = true;
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    // "Save and open…" / "Save and continue…": same deal — the
                    // work is safely on disk, so the project it belongs to can
                    // now be replaced or cleared.
                    if self.open_after_save {
                        self.open_after_save = false;
                        self.open_prompt = false;
                        self.pick_and_open_project(&mut save_project_needed);
                    }
                    if self.new_after_save {
                        self.new_after_save = false;
                        self.new_prompt = false;
                        self.begin_new_project();
                    }
                }
                Err(e) => {
                    self.export_msg = format!("{}  {e}", egui_phosphor::regular::X_CIRCLE);
                    self.export_status_until =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                    self.save_dest = None;
                    // Don't close (or open another project) on a failed save —
                    // leave the prompt up so the user sees the error and can
                    // still discard deliberately.
                    self.close_after_save = false;
                    self.open_after_save = false;
                    self.new_after_save = false;
                }
            }
        } else if save_requested && self.save_in_progress.is_none() {
            // The guard above needs a build config, i.e. a chip. Without one
            // there is genuinely nothing to write — but Save is an explicit
            // action and must not vanish silently.
            self.export_msg = format!(
                "{}  Nothing to save yet — select a chip first (System tab)",
                egui_phosphor::regular::WARNING
            );
            self.export_status_until =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(4));
        }

        // ── Modal dialog: New Project ─────────────────────────────
        // (New File / New Folder are now inline inputs rendered in the project
        // tree at the target folder — see `project_tree::gui::inline_new_item`.)
        self.show_new_project_dialog(ui, &mut save_project_needed);
        self.show_rename_project_dialog(ui);
        self.show_mcu_form_dialog(ui);
        self.show_git_discard_dialog(ui);
        self.show_git_restore_dialog(ui);
        self.show_git_restore_all_dialog(ui);
        self.show_git_switch_dialog(ui);
        self.show_git_delete_branch_dialog(ui);
        self.show_extract_crate_dialog(ui);
        self.show_clone_library_dialog(ui);
        self.show_clone_project_dialog(ui);
        self.show_library_action_dialog(ui);
        self.show_workspace_add_error_dialog(ui);
        self.show_exit_prompt(ui);
        self.show_open_project_prompt(ui, &mut save_project_needed);
        self.show_new_project_prompt(ui);

        // Write the entire project to the workspace directory when the file
        // tree changed (file added, deleted, or project opened/cleared).
        // This ensures Cargo.toml and all other required files are in sync.
        // The startup picker runs after this point in the frame, so its load
        // asks for the rewrite through a field instead of the local.
        save_project_needed |= std::mem::take(&mut self.workspace_write_requested);
        if save_project_needed {
            if self.selected_build_cfg().is_some() {
                let workspace = crate::workspace::dir();
                let _ = project_gen::write_project(
                    &workspace,
                    &self.current_project_files(),
                    &self.project_tree.user_src_files,
                    &self.mcu_config_text(),
                    &self.structure_config_text(),
                );
            }
        }

        // A Project Save (or a tree change that rewrote the workspace) flushes
        // pending edits to rust-analyzer next frame — the ONLY moment RA
        // re-verifies (no typing-time evaluation, so editing stays light).
        if save_project_clicked || save_project_needed {
            self.lsp_flush_requested = true;
        }

        // ── Panel 3: the central slot ────────────────────────────────────────
        // Normally the MCU Configurator (Pins / Clock / Structure / …). When
        // collapsed that whole middle zone is hidden and the EDITOR takes the
        // central slot instead, so it fills everything the Project tree leaves.
        // Safe to skip the MCU panel: main.rs regeneration and the pin/config
        // sync run in `init_frame` off `mcu_state_hash`, not from this panel.
        if self.side_panels_collapsed {
            self.show_editor_panel(ui, &project_files);
        } else {
            self.show_mcu_panel(ui);
        }

        // ── Startup picker ───────────────────────────────────────────────────
        // Before the loading overlay: the two never coexist (nothing loads while
        // the user is still choosing), and a project picked here arms the
        // overlay for the frames that follow.
        self.show_startup_picker(ui);

        // ── Project-switch overlay ───────────────────────────────────────────
        // Dead last: it covers the WHOLE window (panels, banners and dialogs
        // alike) while a project change is still loading, and its lift decision
        // needs this frame's final busy state.
        self.show_project_loading_overlay(ui);
    }
}

#[cfg(test)]
mod path_migration_tests {
    use super::{migrate_folders_to_root_relative, migrate_to_root_relative};

    fn old() -> Vec<(String, String)> {
        vec![
            ("app.rs".into(), "a".into()),
            ("pins/mod.rs".into(), "b".into()),
        ]
    }

    /// State from an older build (flag absent → `false`) gets lifted.
    #[test]
    fn old_state_is_prefixed_with_src() {
        let got = migrate_to_root_relative(old(), false);
        assert_eq!(got[0].0, "src/app.rs");
        assert_eq!(got[1].0, "src/pins/mod.rs");
        assert_eq!(got[0].1, "a", "content untouched");
    }

    /// Already-migrated state must pass through byte-identical — running the
    /// migration twice would produce `src/src/…` and lose the whole project.
    #[test]
    fn migrated_state_is_left_alone() {
        let already = vec![
            ("src/app.rs".to_string(), "a".to_string()),
            ("mw_radar/src/lib.rs".to_string(), "c".to_string()),
        ];
        assert_eq!(migrate_to_root_relative(already.clone(), true), already);
    }

    /// A user folder literally named `src` is why this is flag-driven and not
    /// a path heuristic: sniffing would call this already-migrated.
    #[test]
    fn a_user_folder_named_src_still_migrates() {
        let tricky = vec![("src/deep.rs".to_string(), "x".to_string())];
        assert_eq!(
            migrate_to_root_relative(tricky, false)[0].0,
            "src/src/deep.rs",
            "the flag decides, not the shape of the path"
        );
    }

    #[test]
    fn folders_migrate_the_same_way() {
        assert_eq!(
            migrate_folders_to_root_relative(vec!["pins".into()], false),
            vec!["src/pins".to_string()]
        );
        assert_eq!(
            migrate_folders_to_root_relative(vec!["src/pins".into()], true),
            vec!["src/pins".to_string()]
        );
    }
}

#[cfg(test)]
mod flush_cache_tests {
    use super::AppIde;
    use std::collections::HashMap;

    #[test]
    fn write_workspace_file_caches_and_writes() {
        let dir = std::env::temp_dir().join(format!("eide_flush_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cache: HashMap<String, u64> = HashMap::new();

        // 1. First write for a new path → file created, hash cached.
        AppIde::write_workspace_file(&mut cache, &dir, "src/foo/bar.rs", "hello");
        let dest = dir.join("src").join("foo").join("bar.rs");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello");
        assert!(cache.contains_key("src/foo/bar.rs"));

        // 2. Same content again → cache hit; even if we corrupt the file on disk,
        //    the cached hash means we skip writing (proving no disk touch).
        std::fs::write(&dest, "CORRUPTED").unwrap();
        AppIde::write_workspace_file(&mut cache, &dir, "src/foo/bar.rs", "hello");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "CORRUPTED"); // untouched

        // 3. Changed content (cache has a stale entry) → written directly.
        AppIde::write_workspace_file(&mut cache, &dir, "src/foo/bar.rs", "world");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "world");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod rename_apply_tests {
    use super::{apply_text_edits, lsp_pos_to_byte};
    use crate::lsp::RenameEdit;

    fn edit(sl: u32, sc: u32, el: u32, ec: u32, t: &str) -> RenameEdit {
        RenameEdit {
            rel_path: "src/main.rs".into(),
            start_line: sl,
            start_char: sc,
            end_line: el,
            end_char: ec,
            new_text: t.into(),
        }
    }

    #[test]
    fn applies_multiple_edits_same_line() {
        let text = "let foo = foo + 1;";
        let edits = vec![edit(0, 4, 0, 7, "bar"), edit(0, 10, 0, 13, "bar")];
        assert_eq!(apply_text_edits(text, edits), "let bar = bar + 1;");
    }

    #[test]
    fn applies_edits_across_lines_with_length_change() {
        let text = "fn foo() {}\nfoo();\n";
        let edits = vec![edit(0, 3, 0, 6, "longer"), edit(1, 0, 1, 3, "longer")];
        assert_eq!(apply_text_edits(text, edits), "fn longer() {}\nlonger();\n");
    }

    #[test]
    fn pos_to_byte_maps_line_and_char() {
        let text = "ab\ncd";
        assert_eq!(lsp_pos_to_byte(text, 0, 0), 0);
        assert_eq!(lsp_pos_to_byte(text, 0, 2), 2);
        assert_eq!(lsp_pos_to_byte(text, 1, 0), 3);
        assert_eq!(lsp_pos_to_byte(text, 1, 2), 5);
    }

    #[test]
    fn short_path_keeps_crate_dir_and_src_tail_or_filename() {
        use super::short_path;
        // The crate dir (segment before `/src/`) is kept so the crate is clear.
        assert_eq!(
            short_path("/home/u/.cargo/registry/src/index-abc/stm32f1xx-hal-0.10.0/src/gpio.rs"),
            "stm32f1xx-hal-0.10.0/src/gpio.rs"
        );
        assert_eq!(
            short_path("C:/x/proj/src/pins/utils/i2c1.rs"),
            "proj/src/pins/utils/i2c1.rs"
        );
        assert_eq!(short_path(r"C:\x\proj\src\main.rs"), "proj/src/main.rs");
        // No `/src/` → bare file name.
        assert_eq!(short_path("/home/u/.cargo/esp-hal/lib.rs"), "lib.rs");
    }

    #[test]
    fn generated_byte_ranges_finds_blocks_and_locks_fixes() {
        use super::generated_byte_ranges;
        use crate::panels::mcu_module::codegen::common::{GEN_BEGIN, GEN_END};

        let code = format!("use foo;\n{GEN_BEGIN}\nlet x = 1;\n{GEN_END}\nloop {{}}\n");
        let ranges = generated_byte_ranges(&code);
        assert_eq!(ranges.len(), 1, "one generated block");
        let (b, e) = ranges[0];
        // The block spans from GEN_BEGIN to the end of the GEN_END marker.
        assert_eq!(&code[b..b + GEN_BEGIN.len()], GEN_BEGIN);
        assert_eq!(&code[e - GEN_END.len()..e], GEN_END);

        // A fix on `let x = 1;` (inside the block) is locked; one on `use foo;`
        // (before it) and `loop {}` (after it) is not.
        let inside = code.find("let x").unwrap();
        let before = code.find("use foo").unwrap();
        let after = code.find("loop").unwrap();
        let overlaps = |s: usize, end: usize| ranges.iter().any(|&(b, e)| s < e && end > b);
        assert!(overlaps(inside, inside + 5));
        assert!(!overlaps(before, before + 3));
        assert!(!overlaps(after, after + 4));
    }

    #[test]
    fn generated_byte_ranges_handles_none_and_multiple() {
        use super::generated_byte_ranges;
        use crate::panels::mcu_module::codegen::common::{GEN_BEGIN, GEN_END};

        assert!(generated_byte_ranges("no markers here").is_empty());
        let two = format!("{GEN_BEGIN}\na\n{GEN_END}\nmid\n{GEN_BEGIN}\nb\n{GEN_END}\n");
        assert_eq!(generated_byte_ranges(&two).len(), 2);
    }

    fn span_edit(start: usize, end: usize, repl: &str) -> crate::build::SpanEdit {
        crate::build::SpanEdit {
            file: "src/main.rs".into(),
            start,
            end,
            replacement: repl.into(),
        }
    }

    #[test]
    fn apply_edits_to_buffer_applies_all_unlocked_back_to_front() {
        use super::apply_edits_to_buffer;
        // Two unused imports on consecutive lines (Apply-all collects both).
        let mut buf = "use a;\nuse b;\nfn main() {}".to_string();
        let edits = [span_edit(0, 7, ""), span_edit(7, 14, "")]; // remove each "use …;\n"
        let refs: Vec<&_> = edits.iter().collect();
        let n = apply_edits_to_buffer(&mut buf, &refs, &[]);
        assert_eq!(n, 2);
        assert_eq!(buf, "fn main() {}");
    }

    #[test]
    fn apply_edits_to_buffer_skips_locked_applies_rest() {
        use super::apply_edits_to_buffer;
        // "AAAA BBBB CCCC": remove AAAA (locked) + BBBB (free). Only BBBB goes.
        let mut buf = "AAAA BBBB CCCC".to_string();
        let edits = [span_edit(0, 4, ""), span_edit(5, 9, "")];
        let refs: Vec<&_> = edits.iter().collect();
        let n = apply_edits_to_buffer(&mut buf, &refs, &[(0, 4)]);
        assert_eq!(n, 1, "the locked AAAA edit is skipped");
        assert_eq!(buf, "AAAA  CCCC");
    }

    #[test]
    fn apply_edits_to_buffer_dedups_overlapping() {
        use super::apply_edits_to_buffer;
        let mut buf = "hello world".to_string();
        // Two edits over the same span (duplicate suggestion) → applied once.
        let edits = [span_edit(0, 5, "hi"), span_edit(0, 5, "hi")];
        let refs: Vec<&_> = edits.iter().collect();
        let n = apply_edits_to_buffer(&mut buf, &refs, &[]);
        assert_eq!(n, 1);
        assert_eq!(buf, "hi world");
    }
}
