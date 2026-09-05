//! The floating error list pinned to the editor's top-right corner.
//!
//! The bottom diagnostics panel already lists everything, but it lists it for
//! the whole PROJECT and it is usually collapsed — so while you are reading one
//! file there is nothing on screen that says where its errors are. A squiggle
//! only helps once you have already scrolled to it.
//!
//! Anchored to the EDITOR's visible rect, not to the window: pinned to the
//! window it would float over the MCU panel or the project tree as soon as
//! either is resized.
//!
//! Errors only. "Errors" is what was asked for, and a six-row box fills up
//! instantly with `unused_variable` otherwise; warnings stay in the bottom
//! panel.

use crate::lsp::{DiagSeverity, LspDiagnostic};
use eframe::egui;

/// Characters of the message shown on a row.
///
/// Twenty, as requested — but a row also carries the rustc CODE when there is
/// one, because twenty characters routinely cut before the part that tells two
/// errors apart: `unresolved import 'embedded_hal_async'` and `unresolved
/// import 'embedded_io_async'` both truncate to `unresolved import '`. The code
/// costs five characters and is exactly the discriminator. The whole message is
/// on the tooltip.
pub const EXCERPT_CHARS: usize = 20;

/// Rows drawn before the rest collapse into "+N more".
pub const MAX_ROWS: usize = 6;

/// One diagnostic reduced to what a row needs.
///
/// The seam that lets one row builder serve both sources: rust-analyzer's
/// diagnostics and, when it has none, the last `cargo check` result.
#[derive(Clone, Debug)]
pub struct Entry {
    /// 1-based.
    pub line: u32,
    /// 1-based; only used to tell two errors on the same line apart.
    pub col: u32,
    pub code: Option<String>,
    pub message: String,
}

/// One drawn row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: u32,
    /// `Some("E0425")` for a numbered rustc error; `None` for anything else.
    pub code: Option<String>,
    /// First [`EXCERPT_CHARS`] characters of the message's first line.
    pub excerpt: String,
    /// The whole message, for the tooltip.
    pub full: String,
}

/// `true` for a numbered rustc compiler code (`E0308`) as opposed to a lint
/// name (`unused_variables`).
///
/// Only numbered codes go on a row: a lint name is longer than the excerpt it
/// would be sharing the row with.
fn is_numbered(code: &str) -> bool {
    match code.strip_prefix('E') {
        Some(d) => !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// The first `max` characters, with an ellipsis when something was cut.
///
/// Counts CHARACTERS, not bytes — a byte slice would panic mid-`…` on a message
/// quoting a non-ASCII identifier.
fn excerpt(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().nth(max).is_some() {
        out.push('…');
    }
    out
}

/// Build the rows: errors only, deduplicated, in reading order.
///
/// NOT capped. The cap is a display choice made in [`show`], and stepping with
/// F8 has to reach the seventh error too — which is precisely the one that does
/// not fit on screen.
pub fn rows_for(entries: &[Entry]) -> Vec<Row> {
    let mut seen: Vec<(u32, u32, String)> = Vec::new();
    let mut rows: Vec<(u32, u32, Row)> = Vec::new();
    for e in entries {
        // The first line only: rust-analyzer messages are routinely multi-line
        // ("mismatched types\nexpected `u8`, found …") and the rest belongs on
        // the tooltip.
        let head = e
            .message
            .lines()
            .next()
            .unwrap_or("")
            .trim_end()
            .to_string();
        // The SAME error can arrive twice — natively from rust-analyzer and
        // again through flycheck from rustc. Two identical rows would eat a
        // third of a six-row box.
        let key = (e.line, e.col, head.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        rows.push((
            e.line,
            e.col,
            Row {
                line: e.line,
                code: e
                    .code
                    .as_deref()
                    .filter(|c| is_numbered(c))
                    .map(String::from),
                excerpt: excerpt(&head, EXCERPT_CHARS),
                full: e.message.clone(),
            },
        ));
    }
    // Reading order, not publication order — rust-analyzer emits in whatever
    // order its analysis finished.
    rows.sort_by_key(|(l, c, _)| (*l, *c));
    rows.into_iter().map(|(_, _, r)| r).collect()
}

/// The key that steps through the errors; Shift makes it step backwards.
///
/// Named, and guarded by a test, for the reason Ctrl+Alt+Insert cost an
/// afternoon: `egui-winit` rewrites some keys into clipboard events before egui
/// ever sees an `Event::Key`, and a shortcut killed that way looks exactly like
/// a feature that was never built.
pub const ERROR_STEP_KEY: egui::Key = egui::Key::F8;

/// The line F8 (`forward`) / Shift+F8 should land on, from a caret on
/// `caret_line`. Wraps at both ends; `None` when there is nothing to step to.
///
/// Strictly past the caret in either direction, so pressing F8 twice moves
/// twice: landing on an error puts the caret on its line, and a `>=` test would
/// hand back the same line for ever.
pub fn step(rows: &[Row], caret_line: u32, forward: bool) -> Option<u32> {
    if forward {
        rows.iter()
            .find(|r| r.line > caret_line)
            .or_else(|| rows.first())
    } else {
        rows.iter()
            .rev()
            .find(|r| r.line < caret_line)
            .or_else(|| rows.last())
    }
    .map(|r| r.line)
}

/// Every error of `rel` that rust-analyzer is publishing.
pub fn entries_from_lsp(diags: &[LspDiagnostic]) -> Vec<Entry> {
    diags
        .iter()
        .filter(|d| d.severity == DiagSeverity::Error)
        .map(|d| Entry {
            line: d.line,
            col: d.col,
            code: d.code.clone(),
            message: d.message.clone(),
        })
        .collect()
}

/// Every error of one file from the last `cargo check`.
///
/// A rustc diagnostic can name a file with no primary span (a link error, say);
/// it has nothing to jump to, so it is not a row.
pub fn entries_from_cargo(diags: &[&crate::build::Diagnostic]) -> Vec<Entry> {
    diags
        .iter()
        .filter(|d| d.is_error())
        .filter_map(|d| {
            Some(Entry {
                line: d.line?,
                col: d.col.unwrap_or(1),
                code: d.code.clone(),
                message: d.message.clone(),
            })
        })
        .collect()
}

/// Where the rows came from, and whether they can still be trusted to point at
/// the right line.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// rust-analyzer holds the current text and has re-published since.
    Live,
    /// rust-analyzer's last publish predates the edits on screen: the lines
    /// have moved under it.
    Stale,
    /// From the last completed `cargo check` — a snapshot by definition.
    Cargo,
}

impl Freshness {
    fn note(self) -> &'static str {
        match self {
            Self::Live => "",
            Self::Stale => " · stale",
            Self::Cargo => " · cargo",
        }
    }
}

const WIDTH: f32 = 272.0;
/// Clear of the editor's own vertical scrollbar, so the two never fight for a
/// click.
const SCROLLBAR_GAP: f32 = 16.0;
const PAD: f32 = 6.0;

const FILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(26, 20, 22, 232);
const STROKE: egui::Color32 = egui::Color32::from_rgb(150, 62, 62);
const HEAD: egui::Color32 = egui::Color32::from_rgb(236, 130, 120);
const LINE_NO: egui::Color32 = egui::Color32::from_rgb(150, 156, 168);
const CODE: egui::Color32 = egui::Color32::from_rgb(226, 120, 110);
const MSG: egui::Color32 = egui::Color32::from_rgb(224, 226, 232);
const DIM: egui::Color32 = egui::Color32::from_rgb(128, 132, 142);

/// Draw the list in the top-right of `editor_clip`. Returns the 1-based line of
/// a clicked row.
///
/// Drawn at [`egui::Order::Middle`], BELOW the completion / code-action /
/// extract-function popups — those live at `Foreground` and open in the same
/// corner often enough that the list would otherwise cover the one the user is
/// actually typing into.
/// `salt` distinguishes the two editors.
///
/// Both views run this same code, and an `Area` is keyed by its id: one shared
/// id would make the Reference editor's list and the main editor's list the
/// SAME area, so whichever drew last would move the other one on top of itself.
pub fn show(
    ui: &egui::Ui,
    salt: &str,
    editor_clip: egui::Rect,
    rows: &[Row],
    fresh: Freshness,
) -> Option<u32> {
    if rows.is_empty() {
        return None;
    }
    let mut clicked = None;
    let total = rows.len();
    let extra = total.saturating_sub(MAX_ROWS);
    let rows = &rows[..total.min(MAX_ROWS)];
    // Anchored by its RIGHT edge, with a RIGHT_TOP pivot, rather than by a
    // left-edge position computed from WIDTH. The frame's real width is the
    // content plus its margins plus its stroke, so a left-edge anchor has to
    // predict all three — and predicted it two pixels short, which put the
    // first row under the editor's scrollbar. A pivot has nothing to predict.
    let anchor = egui::pos2(editor_clip.right() - SCROLLBAR_GAP, editor_clip.top() + PAD);
    egui::Area::new(egui::Id::new(("editor_error_list", salt)))
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(anchor)
        .order(egui::Order::Middle)
        .constrain_to(editor_clip)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(FILL)
                .stroke(egui::Stroke::new(1.0_f32, STROKE))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(7, 5))
                .show(ui, |ui| {
                    ui.set_width(WIDTH - 14.0);
                    // The keyboard twins are only discoverable if something
                    // says so, and the header is the one row that is always
                    // there.
                    ui.label(
                        egui::RichText::new(format!(
                            "{total} error{}{}",
                            if total == 1 { "" } else { "s" },
                            fresh.note()
                        ))
                        .size(10.5)
                        .strong()
                        .color(if fresh == Freshness::Live { HEAD } else { DIM }),
                    )
                    .on_hover_text("F8 / Shift+F8 step to the next / previous error");
                    for r in rows {
                        if row_button(ui, r, fresh).clicked() {
                            clicked = Some(r.line);
                        }
                    }
                    if extra > 0 {
                        ui.label(
                            egui::RichText::new(format!("+{extra} more"))
                                .size(10.0)
                                .color(DIM),
                        );
                    }
                });
        });
    clicked
}

/// One clickable row: `  42 · E0425 cannot find value…`.
fn row_button(ui: &mut egui::Ui, r: &Row, fresh: Freshness) -> egui::Response {
    // A LayoutJob rather than three labels: the row must be ONE click target,
    // and the line number has to stay column-aligned with the rows above it.
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::monospace(10.5);
    // Stale rows still point somewhere useful, they just point at where the
    // compiler last looked — so they dim rather than disappear.
    let dimmed = fresh != Freshness::Live;
    let fmt = |color: egui::Color32| egui::TextFormat {
        font_id: font.clone(),
        color: if dimmed { DIM } else { color },
        ..Default::default()
    };
    job.append(&format!("{:>5}", r.line), 0.0, fmt(LINE_NO));
    job.append(" ", 0.0, fmt(LINE_NO));
    if let Some(code) = &r.code {
        job.append(code, 0.0, fmt(CODE));
        job.append(" ", 0.0, fmt(CODE));
    }
    job.append(&r.excerpt, 0.0, fmt(MSG));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;

    let resp = ui.add(
        egui::Button::new(job)
            .frame(false)
            .min_size(egui::vec2(WIDTH - 16.0, 0.0)),
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // The excerpt is twenty characters; the tooltip is the whole thing, which
    // is the only place a multi-line rustc message is readable.
    resp.on_hover_text(format!("line {}\n{}", r.line, r.full))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(line: u32, col: u32, code: Option<&str>, msg: &str) -> Entry {
        Entry {
            line,
            col,
            code: code.map(String::from),
            message: msg.to_string(),
        }
    }

    #[test]
    fn rows_are_ordered_by_position_not_publication_order() {
        let rows = rows_for(&[
            e(90, 1, None, "late"),
            e(12, 1, None, "early"),
            e(12, 30, None, "same line, later column"),
        ]);
        assert_eq!(
            rows.iter().map(|r| r.line).collect::<Vec<_>>(),
            [12, 12, 90]
        );
        assert_eq!(rows[0].excerpt, "early");
    }

    /// The same error arrives twice — natively from rust-analyzer and again
    /// through flycheck from rustc. In a six-row box, two identical rows cost a
    /// third of the list.
    #[test]
    fn the_same_error_from_two_sources_is_one_row() {
        let rows = rows_for(&[
            e(7, 5, Some("E0425"), "cannot find value `x` in this scope"),
            e(7, 5, Some("E0425"), "cannot find value `x` in this scope"),
        ]);
        assert_eq!(rows.len(), 1);
    }

    /// Two DIFFERENT errors on the same line must both survive — deduping on
    /// the line alone would swallow one.
    #[test]
    fn two_different_errors_on_one_line_stay_two_rows() {
        let rows = rows_for(&[
            e(7, 5, None, "cannot find value `x`"),
            e(7, 22, None, "cannot find value `y`"),
        ]);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn the_excerpt_is_twenty_characters_plus_an_ellipsis() {
        let rows = rows_for(&[e(1, 1, None, "cannot find value `x` in this scope")]);
        assert_eq!(rows[0].excerpt.chars().count(), EXCERPT_CHARS + 1);
        assert!(rows[0].excerpt.ends_with('…'));
        assert_eq!(rows[0].excerpt, "cannot find value `x…");
    }

    #[test]
    fn a_short_message_is_not_padded_or_elided() {
        let rows = rows_for(&[e(1, 1, None, "mismatched types")]);
        assert_eq!(rows[0].excerpt, "mismatched types");
    }

    /// A byte-based cut would panic in the middle of a multi-byte character.
    #[test]
    fn a_non_ascii_message_survives_the_cut() {
        let rows = rows_for(&[e(1, 1, None, "unresolved import `crate::măsurători`")]);
        assert_eq!(rows[0].excerpt.chars().count(), EXCERPT_CHARS + 1);
    }

    /// Only the first line reaches the row; the rest is tooltip material.
    #[test]
    fn a_multi_line_message_contributes_only_its_first_line() {
        let rows = rows_for(&[e(
            3,
            1,
            None,
            "mismatched types\nexpected `u8`, found `u16`",
        )]);
        assert_eq!(rows[0].excerpt, "mismatched types");
        assert!(rows[0].full.contains("expected `u8`"));
    }

    /// A numbered code goes on the row (five characters, and it is what tells
    /// two truncated messages apart); a lint NAME does not — it is longer than
    /// the excerpt it would share the row with.
    #[test]
    fn only_a_numbered_rustc_code_reaches_the_row() {
        let rows = rows_for(&[
            e(1, 1, Some("E0308"), "mismatched types"),
            e(2, 1, Some("unused_variables"), "unused variable: `x`"),
            e(3, 1, None, "no code at all"),
        ]);
        assert_eq!(rows[0].code.as_deref(), Some("E0308"));
        assert_eq!(rows[1].code, None);
        assert_eq!(rows[2].code, None);
    }

    /// The cap is a DISPLAY choice, applied inside [`show`]. The builder hands
    /// back everything so F8 can still reach the errors that do not fit in the
    /// box — those are exactly the ones you cannot see.
    #[test]
    fn the_builder_returns_every_error_not_just_the_visible_ones() {
        let many: Vec<Entry> = (1..=10).map(|i| e(i, 1, None, "boom")).collect();
        let rows = rows_for(&many);
        assert_eq!(rows.len(), 10);
        assert!(rows.len() > MAX_ROWS, "the point of the case");
    }

    /// F8 walks past the visible six and reaches the tenth.
    #[test]
    fn stepping_reaches_an_error_the_box_does_not_show() {
        let many: Vec<Entry> = (1..=10).map(|i| e(i * 10, 1, None, "boom")).collect();
        let rows = rows_for(&many);
        assert_eq!(
            step(&rows, 95, true),
            Some(100),
            "the tenth is off the list"
        );
    }

    #[test]
    fn f8_walks_forward_and_wraps_at_the_end() {
        let rows = rows_for(&[e(12, 1, None, "a"), e(40, 1, None, "b")]);
        assert_eq!(step(&rows, 1, true), Some(12));
        assert_eq!(
            step(&rows, 12, true),
            Some(40),
            "past the caret, not onto it"
        );
        assert_eq!(step(&rows, 40, true), Some(12), "wraps to the first");
        assert_eq!(step(&rows, 999, true), Some(12));
    }

    #[test]
    fn shift_f8_walks_backward_and_wraps_at_the_start() {
        let rows = rows_for(&[e(12, 1, None, "a"), e(40, 1, None, "b")]);
        assert_eq!(step(&rows, 999, false), Some(40));
        assert_eq!(step(&rows, 40, false), Some(12));
        assert_eq!(step(&rows, 12, false), Some(40), "wraps to the last");
        assert_eq!(step(&rows, 1, false), Some(40));
    }

    /// Two errors on ONE line collapse to one stop. Scrolling cannot tell them
    /// apart anyway, and a `>=` test would have parked F8 on that line for ever.
    #[test]
    fn two_errors_on_one_line_are_a_single_stop() {
        let rows = rows_for(&[
            e(12, 1, None, "a"),
            e(12, 30, None, "b"),
            e(40, 1, None, "c"),
        ]);
        assert_eq!(rows.len(), 3, "all three are listed");
        assert_eq!(step(&rows, 12, true), Some(40), "but F8 leaves the line");
    }

    #[test]
    fn a_single_error_is_its_own_next_and_previous() {
        let rows = rows_for(&[e(7, 1, None, "lonely")]);
        assert_eq!(step(&rows, 7, true), Some(7));
        assert_eq!(step(&rows, 7, false), Some(7));
    }

    /// Warnings never reach the list — that is what the bottom panel is for.
    #[test]
    fn only_errors_are_taken_from_the_lsp_map() {
        let mk = |sev: DiagSeverity, msg: &str| LspDiagnostic {
            severity: sev,
            message: msg.to_string(),
            line: 1,
            col: 1,
            end_line: 1,
            end_col: 2,
            code: None,
            source: "rust-analyzer".to_string(),
        };
        let diags = [
            mk(DiagSeverity::Error, "boom"),
            mk(DiagSeverity::Warning, "meh"),
            mk(DiagSeverity::Hint, "psst"),
            mk(DiagSeverity::Info, "fyi"),
        ];
        let entries = entries_from_lsp(&diags);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "boom");
    }

    /// A rustc diagnostic can name a file with no line (a link error). It
    /// badges the file but has nowhere to jump, so it must not become a row
    /// that goes nowhere.
    #[test]
    fn a_cargo_error_without_a_line_is_not_a_row() {
        let mk = |line: Option<u32>, level: &str| crate::build::Diagnostic {
            level: level.to_string(),
            message: "boom".to_string(),
            rendered: String::new(),
            file: Some("src/main.rs".to_string()),
            line,
            col: Some(4),
            code: None,
            fixes: Vec::new(),
            rename: None,
        };
        let a = mk(Some(12), "error");
        let b = mk(None, "error");
        let c = mk(Some(30), "warning");
        let entries = entries_from_cargo(&[&a, &b, &c]);
        assert_eq!(entries.len(), 1, "only the one with a line, and no warning");
        assert_eq!(entries[0].line, 12);
    }

    /// Lay the list out in a headless frame and report where it landed.
    ///
    /// Two passes: an `Area` reports its rect only once it has been laid out,
    /// so the first frame is what produces the answer the second one reads.
    fn placed(clip: egui::Rect, rows: &[Row]) -> Option<egui::Rect> {
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let _ = ctx.run_ui(Default::default(), |ui| {
                show(ui, "test", clip, rows, Freshness::Live);
            });
        }
        ctx.memory(|m| m.area_rect(egui::Id::new(("editor_error_list", "test"))))
    }

    fn sample_rows() -> Vec<Row> {
        rows_for(&[
            e(12, 1, Some("E0425"), "cannot find value `x` in this scope"),
            e(140, 4, Some("E0308"), "mismatched types"),
        ])
    }

    /// "Top right" is of the EDITOR, not of the window — and clear of the
    /// editor's own scrollbar, or the two would fight for every click on the
    /// first row.
    #[test]
    fn the_list_lands_in_the_editors_top_right_corner() {
        let clip = egui::Rect::from_min_size(egui::pos2(120.0, 60.0), egui::vec2(700.0, 500.0));
        let rows = sample_rows();
        let r = placed(clip, &rows).expect("the list was laid out");
        assert!(
            r.right() <= clip.right() - SCROLLBAR_GAP + 0.5,
            "the list at {:?} overlaps the scrollbar lane of {clip:?}",
            r
        );
        assert!(
            r.left() > clip.center().x,
            "top-RIGHT: {} should be past the middle of {clip:?}",
            r.left()
        );
        assert!(r.top() >= clip.top(), "and pinned to the top");
        assert!(
            r.bottom() < clip.bottom(),
            "a two-row list must not reach the bottom of the editor"
        );
    }

    /// Both editors run this code. Sharing one area id would make the two
    /// lists the SAME area — the second one drawn would drag the first on top
    /// of itself, in the wrong editor.
    #[test]
    fn the_two_editors_get_two_separate_panels() {
        let ctx = egui::Context::default();
        let left = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 400.0));
        let right = egui::Rect::from_min_size(egui::pos2(500.0, 0.0), egui::vec2(400.0, 400.0));
        let rows = sample_rows();
        for _ in 0..2 {
            let _ = ctx.run_ui(Default::default(), |ui| {
                show(ui, "main", left, &rows, Freshness::Live);
                show(ui, "ref", right, &rows, Freshness::Live);
            });
        }
        let a = ctx
            .memory(|m| m.area_rect(egui::Id::new(("editor_error_list", "main"))))
            .expect("the main editor's list");
        let b = ctx
            .memory(|m| m.area_rect(egui::Id::new(("editor_error_list", "ref"))))
            .expect("the reference editor's list");
        assert!(
            a.right() <= left.right() && b.left() >= right.left(),
            "each list must stay in its own editor: {a:?} in {left:?}, {b:?} in {right:?}"
        );
    }

    /// Zero errors, zero pixels: nothing is drawn over the code when the file
    /// is clean.
    #[test]
    fn a_clean_file_draws_no_panel_at_all() {
        let clip = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(700.0, 500.0));
        assert!(placed(clip, &[]).is_none());
    }

    /// A narrow editor must not push the list off its left edge — the anchor is
    /// derived from the right edge, so a pane narrower than the list is the case
    /// that would send it out of view.
    #[test]
    fn a_narrow_editor_keeps_the_list_on_screen() {
        let clip = egui::Rect::from_min_size(egui::pos2(300.0, 40.0), egui::vec2(200.0, 400.0));
        let rows = sample_rows();
        let r = placed(clip, &rows).expect("laid out");
        assert!(
            r.right() <= clip.right() + 0.5 && r.top() >= clip.top() - 0.5,
            "the list at {r:?} escaped the {clip:?} editor"
        );
    }

    #[test]
    fn an_empty_file_produces_no_rows_at_all() {
        let rows = rows_for(&[]);
        assert!(rows.is_empty());
    }
}
