//! Draw a laid-out [`FlowLayout`] — the only part of the Flow tab that touches
//! egui.
//!
//! The view controls are deliberately the SAME as the Structure tab's, because
//! they sit next to each other in the Project group and a diagram that pans
//! differently from the diagram beside it is a diagram the user fights: auto-fit
//! as the base scale, mouse wheel and Ctrl+± on top, background drag to pan,
//! Ctrl+0 to re-centre.
//!
//! "All — whole file" is the exception, and on purpose: it is a list, not a
//! diagram, so there the wheel scrolls (see [`outline`]).

use super::layout::{Edge, EdgeKind, FlowLayout, Placed};
use super::parse::{Element, ElementKind, EntryKind, FileModel, Shape};
use eframe::egui;

/// Session view state for the Flow tab.
pub struct FlowView {
    /// User zoom over the auto-fit base (1.0 = the whole chart fits).
    pub zoom: f32,
    /// View offset from centred, in screen px.
    pub pan: egui::Vec2,
    /// The scale actually drawn last frame, so the toolbar can say so.
    pub last_scale: f32,
    /// Which function is charted, by its [`Chart::key`] — an index would
    /// silently point at a different function the moment the file is edited,
    /// and a bare name cannot tell two same-named functions apart.
    ///
    /// Kept while [`Self::all`] is on, so leaving the whole-file view goes back
    /// to the function that was open.
    ///
    /// [`Chart::key`]: super::parse::Chart::key
    pub selected: String,
    /// "All — whole file": every element of the file instead of one chart.
    /// Its own flag rather than a magic value of `selected`, which a function
    /// named `All` would collide with.
    pub all: bool,
}

impl Default for FlowView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            last_scale: 1.0,
            selected: String::new(),
            all: false,
        }
    }
}

/// `@flow_mode` bit: the whole-file view is on.
const MODE_ALL: u8 = 1;

impl FlowView {
    /// The persisted mode, as a bit set. Bits this build does not know are
    /// ignored on reading, so a file written by a newer build still opens here
    /// with everything it DOES know.
    pub fn mode_bits(&self) -> u8 {
        if self.all { MODE_ALL } else { 0 }
    }

    pub fn set_mode_bits(&mut self, bits: u8) {
        self.all = bits & MODE_ALL != 0;
    }

    /// Chart the function `key` - from the picker, a subroutine box, or a
    /// double click in the whole-file list. Always leaves the whole-file view,
    /// and starts the chart fitted and centred.
    pub fn open(&mut self, key: String) {
        self.all = false;
        self.selected = key;
        self.zoom = 1.0;
        self.pan = egui::Vec2::ZERO;
    }
}

/// What one frame of the chart reports back to the driver.
#[derive(Default)]
pub struct ShowResult {
    /// Jump the editor to this 1-based line of the charted file.
    pub goto_line: Option<usize>,
    /// Chart this function instead, by key — a subroutine box was opened, or a
    /// function was double-clicked in the whole-file list.
    pub open_chart: Option<String>,
}

/// The first row of the element picker.
pub const ALL_LABEL: &str = "All — whole file";

/// One row of the element picker below [`ALL_LABEL`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerRow {
    pub depth: usize,
    pub label: String,
    /// The chart it opens; `None` for a container heading (`impl Parser`),
    /// which is shown so its methods read as its members, not picked itself.
    pub key: Option<String>,
    /// An entry point - drawn bright, like before.
    pub entry: bool,
}

/// The picker's rows: every function with a body, each indented under the
/// containers that hold it.
///
/// Only functions are picked on their own: they are what has a chart. Structs,
/// consts, `use`s and macro calls are seen through "All" - listing every one of
/// them here would put a few hundred rows in a 200-pixel popup.
pub fn picker_rows(model: &FileModel) -> Vec<PickerRow> {
    let els = &model.elements;
    let mut holds = vec![false; els.len()];
    for e in els.iter().filter(|e| e.chart.is_some()) {
        let mut p = e.parent;
        while let Some(j) = p {
            holds[j] = true;
            p = els[j].parent;
        }
    }
    els.iter()
        .enumerate()
        .filter_map(|(i, e)| match (e.chart, e.kind) {
            (Some(_), ElementKind::Fn(k)) => Some(PickerRow {
                depth: e.depth,
                label: format!("{}  ·  {}", e.name, k.word()),
                key: Some(e.key.clone()),
                entry: k.is_entry(),
            }),
            _ if holds[i] => Some(PickerRow {
                depth: e.depth,
                label: format!("{} {}", e.kind.word(), e.name),
                key: None,
                entry: false,
            }),
            _ => None,
        })
        .collect()
}

/// The colour an element's kind word is drawn in. Function / struct / enum /
/// trait match the Structure tab's glyphs, so the two Project tabs agree.
pub fn kind_color(kind: ElementKind) -> egui::Color32 {
    use egui::Color32 as C;
    match kind {
        ElementKind::Fn(EntryKind::Function) => C::from_rgb(130, 170, 240),
        // An entry point starts on its own - the gold the rest of the app uses
        // for "look here".
        ElementKind::Fn(_) => C::from_rgb(240, 200, 110),
        ElementKind::RequiredFn => C::from_rgb(105, 135, 190),
        ElementKind::Struct | ElementKind::Union => C::from_rgb(230, 160, 80),
        ElementKind::Enum => C::from_rgb(190, 130, 230),
        ElementKind::Trait | ElementKind::TraitAlias => C::from_rgb(120, 200, 140),
        ElementKind::Impl => C::from_rgb(110, 190, 200),
        ElementKind::Const | ElementKind::Static => C::from_rgb(215, 175, 125),
        ElementKind::TypeAlias => C::from_rgb(190, 190, 130),
        ElementKind::Mod | ElementKind::ModDecl => C::from_rgb(170, 174, 184),
        ElementKind::MacroRules | ElementKind::MacroCall => C::from_rgb(230, 130, 160),
        ElementKind::Use => C::from_rgb(140, 146, 158),
        ElementKind::ExternCrate | ElementKind::ForeignMod => C::from_rgb(160, 160, 205),
        ElementKind::CrateAttrs | ElementKind::Other => C::from_rgb(150, 150, 160),
    }
}

const BG: egui::Color32 = egui::Color32::from_rgb(24, 26, 32);
const TEXT: egui::Color32 = egui::Color32::from_rgb(228, 232, 240);
const DIM_TEXT: egui::Color32 = egui::Color32::from_rgb(132, 138, 150);
const LABEL: egui::Color32 = egui::Color32::from_rgb(186, 194, 210);
const BORDER: egui::Color32 = egui::Color32::from_rgb(96, 106, 128);
const HOVER: egui::Color32 = egui::Color32::from_rgb(250, 250, 250);
/// The `.await` pill — the executor's yield point, in the definition-highlight
/// gold the rest of the app already uses for "look here".
const AWAIT: egui::Color32 = egui::Color32::from_rgb(255, 214, 90);

/// Fill per shape. Muted, so the white box text stays readable on the dark
/// canvas — the same constraint the module diagram's package palette works
/// under.
fn fill(shape: Shape) -> egui::Color32 {
    match shape {
        Shape::Terminal => egui::Color32::from_rgb(60, 58, 44),
        Shape::Process => egui::Color32::from_rgb(46, 52, 66),
        Shape::Io => egui::Color32::from_rgb(38, 60, 56),
        Shape::Decision => egui::Color32::from_rgb(64, 54, 40),
        Shape::Subroutine => egui::Color32::from_rgb(56, 46, 68),
        Shape::Generated => egui::Color32::from_rgb(31, 33, 39),
    }
}

fn edge_color(kind: EdgeKind) -> egui::Color32 {
    match kind {
        EdgeKind::Flow => egui::Color32::from_rgb(150, 165, 195),
        EdgeKind::Back => egui::Color32::from_rgb(120, 170, 240),
        EdgeKind::Break | EdgeKind::Return => egui::Color32::from_rgb(226, 148, 96),
        EdgeKind::Continue => egui::Color32::from_rgb(140, 200, 140),
        EdgeKind::Try => egui::Color32::from_rgb(220, 96, 86),
    }
}

/// Padding kept around the chart when it auto-fits the panel.
const FIT_PAD: f32 = 20.0;
/// Below this the box text stops being readable, so the toolbar says what scale
/// the chart is at rather than leaving it looking broken.
const LEGIBLE_SCALE: f32 = 0.45;

/// Render the toolbar and the chart. `status` is a short note from the driver
/// (a syntax error, an empty file); an empty string means all is well.
pub fn show(
    ui: &mut egui::Ui,
    model: &FileModel,
    lay: &FlowLayout,
    view: &mut FlowView,
    status: &str,
) -> ShowResult {
    let charts = &model.charts;
    let mut result = ShowResult::default();

    // ── Toolbar ───────────────────────────────────────────────────────────
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Element").size(11.0).color(DIM_TEXT));
        let current = if view.all {
            ALL_LABEL.to_string()
        } else {
            charts
                .iter()
                .find(|c| c.key == view.selected)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "—".to_string())
        };
        egui::ComboBox::from_id_salt("flow_chart_pick")
            .selected_text(egui::RichText::new(current).size(11.5))
            .width(260.0)
            .height(420.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        view.all,
                        egui::RichText::new(ALL_LABEL).size(11.5).color(TEXT),
                    )
                    .on_hover_text("Every element of the file, in source order")
                    .clicked()
                {
                    view.all = true;
                }
                ui.separator();
                for row in picker_rows(model) {
                    ui.horizontal(|ui| {
                        ui.add_space(row.depth as f32 * 14.0);
                        match &row.key {
                            None => {
                                ui.label(
                                    egui::RichText::new(&row.label).size(11.0).color(DIM_TEXT),
                                );
                            }
                            Some(key) => {
                                // Entry points lead with what starts them; an
                                // `#[interrupt]` in the same list as a helper
                                // `fn` is the difference between "hardware calls
                                // this" and "someone calls this".
                                let text = egui::RichText::new(&row.label)
                                    .size(11.5)
                                    .color(if row.entry { TEXT } else { DIM_TEXT });
                                let on = !view.all && view.selected == *key;
                                if ui.selectable_label(on, text).clicked() {
                                    view.open(key.clone());
                                }
                            }
                        }
                    });
                }
            });

        ui.add_space(10.0);
        let counts = if view.all {
            format!(
                "{} · {}",
                plural(model.elements.len(), "element"),
                plural(charts.len(), "function")
            )
        } else {
            format!("{} boxes · {} edges", lay.boxes.len(), lay.edges.len())
        };
        ui.label(egui::RichText::new(counts).size(11.0).color(DIM_TEXT));
        if !view.all && view.last_scale < LEGIBLE_SCALE {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!(
                    "at {:.0}% — zoom in to read the boxes",
                    view.last_scale * 100.0
                ))
                .size(11.0)
                .color(egui::Color32::from_rgb(220, 180, 90)),
            );
        }
        if !status.is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(status)
                    .size(11.0)
                    .color(egui::Color32::from_rgb(226, 148, 96)),
            );
        }
    });

    // ── The whole file ────────────────────────────────────────────────────
    if view.all {
        ui.label(
            egui::RichText::new(
                "click a row = go to its line · double-click a function = open its flowchart",
            )
            .size(10.5)
            .color(egui::Color32::from_rgb(120, 120, 130)),
        );
        ui.add_space(2.0);
        let (goto, open) = outline(ui, model, &view.selected);
        result.goto_line = goto;
        result.open_chart = open;
        return result;
    }

    // ── Legend + hints ────────────────────────────────────────────────────
    ui.horizontal_wrapped(|ui| {
        for (shape, name) in [
            (Shape::Process, "statements"),
            (Shape::Io, "in / out"),
            (Shape::Decision, "decision"),
            (Shape::Subroutine, "call — click to open"),
            (Shape::Generated, "generated"),
        ] {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(13.0, 9.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, fill(shape));
            ui.painter().rect_stroke(
                rect,
                2.0,
                egui::Stroke::new(1.0_f32, BORDER),
                egui::StrokeKind::Inside,
            );
            ui.label(egui::RichText::new(name).size(10.5).color(DIM_TEXT));
            ui.add_space(6.0);
        }
    });
    ui.label(
        egui::RichText::new(
            "Ctrl+± / mouse wheel zoom, Ctrl+0 reset · drag the background = pan · click a box = go to its line",
        )
        .size(10.5)
        .color(egui::Color32::from_rgb(120, 120, 130)),
    );
    ui.add_space(2.0);

    // ── Canvas ────────────────────────────────────────────────────────────
    let avail = ui.available_size();
    if avail.x < 3.0 * FIT_PAD || avail.y < 3.0 * FIT_PAD || lay.boxes.is_empty() {
        // Still claim the space, so the panel does not jump around while the
        // file has nothing to draw.
        ui.allocate_exact_size(avail.max(egui::Vec2::ZERO), egui::Sense::hover());
        return result;
    }

    if ui.rect_contains_pointer(ui.available_rect_before_wrap()) {
        ui.input_mut(|i| {
            let cmd = egui::Modifiers::COMMAND;
            if i.consume_key(cmd, egui::Key::Num0) {
                view.zoom = 1.0;
                view.pan = egui::Vec2::ZERO;
            } else if i.consume_key(cmd, egui::Key::Plus) || i.consume_key(cmd, egui::Key::Equals) {
                view.zoom = (view.zoom * 1.15).min(4.0);
            } else if i.consume_key(cmd, egui::Key::Minus) {
                view.zoom = (view.zoom / 1.15).max(0.3);
            }
            let scroll = i.smooth_scroll_delta.y;
            if scroll != 0.0 {
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                view.zoom = (view.zoom * (scroll * 0.002).exp()).clamp(0.3, 4.0);
            }
        });
    }

    let base = ((avail.x - 2.0 * FIT_PAD) / lay.width.max(1.0_f32))
        .min((avail.y - 2.0 * FIT_PAD) / lay.height.max(1.0_f32))
        .clamp(0.05, 2.5);
    let scale = (base * view.zoom).clamp(0.05, 5.0);
    view.last_scale = scale;
    let content = egui::vec2(lay.width, lay.height) * scale;

    let (rect, bg) = ui.allocate_exact_size(avail, egui::Sense::click_and_drag());
    if bg.dragged() {
        view.pan += bg.drag_delta();
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    let free = (rect.size() - content) * 0.5;
    let rel = egui::vec2(
        clamp_rel(free.x + view.pan.x, rect.width(), content.x),
        clamp_rel(free.y + view.pan.y, rect.height(), content.y),
    );
    view.pan = rel - free;
    let origin = rect.left_top() + rel;
    let to_screen = |x: f32, y: f32| -> egui::Pos2 { origin + egui::vec2(x, y) * scale };

    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, BG);

    // Edges first, so a box always covers the tail of its own arrow.
    let stroke_w = (1.4 * scale).clamp(0.7, 2.6);
    for e in &lay.edges {
        draw_edge(&painter, e, &to_screen, stroke_w, scale);
    }

    // Which box is under the pointer (boxes are drawn after edges, so the
    // hit-test uses the same rectangles the user sees).
    // `rect_contains_pointer`, not `rect.contains`: it also asks whether the
    // chart is the topmost thing there, so a box under the New Project chip
    // list or a window does not light up and pop its tooltip over it.
    let pointer = ui
        .ctx()
        .pointer_latest_pos()
        .filter(|_| ui.rect_contains_pointer(rect));
    let mut hovered: Option<usize> = None;
    for (i, b) in lay.boxes.iter().enumerate() {
        let r = box_rect(b, &to_screen, scale);
        if pointer.is_some_and(|p| r.contains(p)) {
            hovered = Some(i);
        }
    }

    for (i, b) in lay.boxes.iter().enumerate() {
        draw_box(&painter, b, &to_screen, scale, hovered == Some(i));
    }

    // A click lands on whatever the pointer is over. Opening a subroutine also
    // jumps the editor to the CALL, so the two views never disagree about what
    // is being looked at.
    if let Some(i) = hovered {
        let b = &lay.boxes[i];
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if bg.clicked() {
            // Always land on the box's own line — for a subroutine that is the
            // CALL, so the editor and the chart never disagree about what is
            // being looked at.
            result.goto_line = Some(b.node.line);
            if b.node.shape == Shape::Subroutine
                && let Some(key) = &b.node.goto_key
                && charts.iter().any(|c| c.key == *key)
            {
                result.open_chart = Some(key.clone());
            }
        }
        // The box text may be elided at this scale; the tooltip never is.
        let mut tip = b.node.text.clone();
        for d in &b.node.detail {
            tip.push('\n');
            tip.push_str(d);
        }
        if b.node.hidden > 0 {
            tip.push_str(&format!("\n+{} more", b.node.hidden));
        }
        tip.push_str(&format!("\n\nline {}", b.node.line));
        if b.node.awaits {
            tip.push_str("  ·  yields to the executor (.await)");
        }
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            egui::Id::new("flow_box_tip"),
            egui::PopupAnchor::Pointer,
        )
        .gap(12.0)
        .show(|ui| {
            ui.label(egui::RichText::new(tip).size(11.0).monospace());
        });
    }

    result
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Height of one outline row.
const ROW_H: f32 = 18.0;
/// Indent per level of nesting (a method under its `impl`).
const INDENT: f32 = 16.0;
/// Width of the kind-word column.
const KIND_W: f32 = 58.0;
/// Width kept free on the right for the line number.
const LINE_W: f32 = 44.0;

/// "All — whole file": every element of the file, one row each, in source
/// order, members indented under their container.
///
/// A LIST, not boxes on the zoomable canvas: it is an enumeration, and on the
/// canvas it would inherit the auto-fit that shrinks a long file to unreadable
/// and the wheel-zoom that fights reading down a page. Here the wheel scrolls,
/// the text stays at its native size, and only the visible rows are drawn
/// (`show_rows`), so a ten-thousand-line file costs what a screenful does.
///
/// Returns `(line to jump to, chart to open)`. A single click only jumps - the
/// list stays, so it can be read top to bottom while the editor follows; a
/// double click on a function opens its flowchart.
fn outline(ui: &mut egui::Ui, model: &FileModel, current: &str) -> (Option<usize>, Option<String>) {
    let mut goto = None;
    let mut open = None;
    if model.elements.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("This file has no items.")
                .size(12.0)
                .color(DIM_TEXT),
        );
        return (goto, open);
    }
    let bg = ui.available_rect_before_wrap();
    ui.painter().rect_filled(bg, 0.0, BG);
    egui::ScrollArea::vertical()
        .id_salt("flow_outline")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_H, model.elements.len(), |ui, range| {
            for e in &model.elements[range] {
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), ROW_H),
                    egui::Sense::click(),
                );
                let painter = ui.painter();
                if resp.hovered() {
                    painter.rect_filled(
                        rect,
                        2.0,
                        egui::Color32::from_rgba_unmultiplied(120, 150, 210, 38),
                    );
                } else if e.chart.is_some() && e.key == current {
                    // The function the chart view would go back to.
                    painter.rect_filled(
                        rect,
                        2.0,
                        egui::Color32::from_rgba_unmultiplied(255, 214, 90, 22),
                    );
                }
                let dim = |c: egui::Color32| {
                    if e.generated {
                        c.gamma_multiply(0.55)
                    } else {
                        c
                    }
                };
                let x = rect.left() + 6.0 + e.depth as f32 * INDENT;
                let mid = rect.center().y;
                let kind = row_galley(
                    ui,
                    e.kind.word(),
                    10.5,
                    dim(kind_color(e.kind)),
                    KIND_W - 6.0,
                );
                painter.galley(egui::pos2(x, mid - kind.size().y / 2.0), kind, TEXT);
                let text_x = x + KIND_W;
                let room = rect.right() - LINE_W - text_x;
                let sig = row_galley(ui, &e.signature, 11.5, dim(TEXT), room);
                painter.galley(egui::pos2(text_x, mid - sig.size().y / 2.0), sig, TEXT);
                painter.text(
                    egui::pos2(rect.right() - 6.0, mid),
                    egui::Align2::RIGHT_CENTER,
                    e.ident_line.to_string(),
                    egui::FontId::monospace(10.0),
                    DIM_TEXT,
                );
                let resp = resp.on_hover_text(outline_tip(e));
                if resp.clicked() {
                    goto = Some(e.ident_line);
                }
                if resp.double_clicked() && e.chart.is_some() {
                    open = Some(e.key.clone());
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        });
    (goto, open)
}

/// One line of monospace text, cut with an ellipsis at `max_w` rather than
/// wrapped - a row is one line high.
fn row_galley(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    color: egui::Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(egui::FontId::monospace(size), color),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(8.0));
    ui.painter().layout_job(job)
}

/// The hover text of an outline row: everything the row had no room for.
pub fn outline_tip(e: &Element) -> String {
    let mut lines: Vec<String> = match e.kind {
        ElementKind::Use => e.detail.iter().map(|t| format!("use {t};")).collect(),
        _ => e.detail.clone(),
    };
    lines.push(String::new());
    lines.push(if e.start_line == e.end_line {
        format!("line {}", e.start_line)
    } else {
        format!("lines {}–{}", e.start_line, e.end_line)
    });
    if e.generated {
        lines.push("generated by the IDE".to_string());
    }
    if e.test_code {
        lines.push("test code".to_string());
    }
    if e.chart.is_some() {
        lines.push("double-click to open its flowchart".to_string());
    }
    lines.join("\n")
}

/// One panning axis, clamped so the chart can never be dragged out of sight.
/// Same shape as the Structure tab's — see the note there on why the bounds are
/// ORDERED rather than branched on a fits/overflows test.
fn clamp_rel(rel: f32, avail: f32, content: f32) -> f32 {
    let inside = FIT_PAD;
    let overflow = avail - content - FIT_PAD;
    let (lo, hi) = if inside <= overflow {
        (inside, overflow)
    } else {
        (overflow, inside)
    };
    rel.max(lo).min(hi)
}

fn box_rect(b: &Placed, to_screen: &impl Fn(f32, f32) -> egui::Pos2, scale: f32) -> egui::Rect {
    egui::Rect::from_min_size(to_screen(b.x, b.y), egui::vec2(b.w, b.h) * scale)
}

fn draw_edge(
    painter: &egui::Painter,
    e: &Edge,
    to_screen: &impl Fn(f32, f32) -> egui::Pos2,
    w: f32,
    scale: f32,
) {
    let color = edge_color(e.kind);
    let stroke = egui::Stroke::new(w, color);
    let pts: Vec<egui::Pos2> = e.pts.iter().map(|&(x, y)| to_screen(x, y)).collect();
    if pts.len() < 2 {
        return;
    }
    for seg in pts.windows(2) {
        painter.line_segment([seg[0], seg[1]], stroke);
    }
    if e.arrow {
        let n = pts.len();
        arrowhead(
            painter,
            pts[n - 2],
            pts[n - 1],
            (8.0 * scale).max(3.5),
            stroke,
        );
    }
    if !e.label.is_empty() {
        // Beside the FIRST segment: that is where the reader's eye is when it
        // leaves the diamond, and it is the only place a label cannot be
        // confused with the neighbouring arm's.
        let a = pts[0];
        let b = pts[1];
        let mid = a + (b - a) * 0.5;
        let horizontal = (b.x - a.x).abs() > (b.y - a.y).abs();
        let off = if horizontal {
            egui::vec2(0.0, -8.0 * scale.max(0.6))
        } else {
            egui::vec2(11.0 * scale.max(0.6), 0.0)
        };
        painter.text(
            mid + off,
            egui::Align2::CENTER_CENTER,
            &e.label,
            egui::FontId::proportional((10.0 * scale).clamp(5.0, 15.0)),
            LABEL,
        );
    }
}

fn draw_box(
    painter: &egui::Painter,
    b: &Placed,
    to_screen: &impl Fn(f32, f32) -> egui::Pos2,
    scale: f32,
    hovered: bool,
) {
    let r = box_rect(b, to_screen, scale);
    let bg = fill(b.node.shape);
    let stroke = egui::Stroke::new(
        if hovered { 2.0 } else { 1.2 } * scale.clamp(0.6, 2.0),
        if hovered { HOVER } else { BORDER },
    );
    match b.node.shape {
        Shape::Terminal => {
            let rad = r.height() * 0.5;
            painter.rect_filled(r, rad, bg);
            painter.rect_stroke(r, rad, stroke, egui::StrokeKind::Inside);
        }
        Shape::Decision => {
            let c = r.center();
            let pts = vec![
                egui::pos2(c.x, r.top()),
                egui::pos2(r.right(), c.y),
                egui::pos2(c.x, r.bottom()),
                egui::pos2(r.left(), c.y),
            ];
            painter.add(egui::Shape::convex_polygon(pts, bg, stroke));
        }
        Shape::Io => {
            let s = 12.0 * scale;
            let pts = vec![
                egui::pos2(r.left() + s, r.top()),
                egui::pos2(r.right(), r.top()),
                egui::pos2(r.right() - s, r.bottom()),
                egui::pos2(r.left(), r.bottom()),
            ];
            painter.add(egui::Shape::convex_polygon(pts, bg, stroke));
        }
        Shape::Subroutine => {
            painter.rect_filled(r, 2.0, bg);
            painter.rect_stroke(r, 2.0, stroke, egui::StrokeKind::Inside);
            // The two side bars that make it a "predefined process".
            let inset = 7.0 * scale;
            for x in [r.left() + inset, r.right() - inset] {
                painter.line_segment(
                    [egui::pos2(x, r.top()), egui::pos2(x, r.bottom())],
                    egui::Stroke::new(stroke.width * 0.8, BORDER),
                );
            }
        }
        Shape::Generated => {
            painter.rect_filled(r, 2.0, bg);
            let dash = 4.0 * scale.max(0.4);
            for (a, b2) in [
                (r.left_top(), r.right_top()),
                (r.right_top(), r.right_bottom()),
                (r.right_bottom(), r.left_bottom()),
                (r.left_bottom(), r.left_top()),
            ] {
                painter.add(egui::Shape::dashed_line(
                    &[a, b2],
                    egui::Stroke::new(stroke.width, if hovered { HOVER } else { DIM_TEXT }),
                    dash,
                    dash,
                ));
            }
        }
        _ => {
            painter.rect_filled(r, 3.0, bg);
            painter.rect_stroke(r, 3.0, stroke, egui::StrokeKind::Inside);
        }
    }

    // ── Text ──────────────────────────────────────────────────────────────
    let font = egui::FontId::monospace((10.0 * scale).clamp(4.0, 20.0));
    if font.size < 4.5 {
        return; // unreadable anyway; drawing it would only smear the box
    }
    let color = if b.node.shape == Shape::Generated {
        DIM_TEXT
    } else {
        TEXT
    };
    let mut lines: Vec<String> = std::iter::once(b.node.text.clone())
        .chain(b.node.detail.iter().cloned())
        .collect();
    if b.node.hidden > 0 {
        lines.push(format!("+{} more", b.node.hidden));
    }
    let inner = painter.with_clip_rect(r.shrink(2.0 * scale));
    let line_h = font.size * 1.32;
    let total = line_h * lines.len() as f32;
    let mut y = r.center().y - total * 0.5 + line_h * 0.5;
    for l in &lines {
        inner.text(
            egui::pos2(r.center().x, y),
            egui::Align2::CENTER_CENTER,
            l,
            font.clone(),
            color,
        );
        y += line_h;
    }

    // ── Markers ───────────────────────────────────────────────────────────
    // `.await` is the whole reason an async chart is worth reading: it is where
    // the executor may hand the CPU to another task.
    if b.node.awaits {
        let d = (5.0 * scale).clamp(1.5, 7.0);
        painter.circle_filled(egui::pos2(r.right() - d * 1.6, r.top() + d * 1.6), d, AWAIT);
    }
    if b.node.try_exit {
        painter.text(
            egui::pos2(r.right() - 4.0 * scale, r.bottom() - 3.0 * scale),
            egui::Align2::RIGHT_BOTTOM,
            "?",
            egui::FontId::proportional((11.0 * scale).clamp(5.0, 16.0)),
            edge_color(EdgeKind::Try),
        );
    }
}

/// Two short lines forming an arrowhead at `to`, pointing away from `from`.
fn arrowhead(
    painter: &egui::Painter,
    from: egui::Pos2,
    to: egui::Pos2,
    len: f32,
    stroke: egui::Stroke,
) {
    let dir = (to - from).normalized();
    if !dir.x.is_finite() || !dir.y.is_finite() {
        return;
    }
    let left = egui::vec2(
        dir.x * (-0.866) - dir.y * (-0.5),
        dir.x * (-0.5) + dir.y * (-0.866),
    );
    let right = egui::vec2(
        dir.x * (-0.866) - dir.y * 0.5,
        dir.x * 0.5 + dir.y * (-0.866),
    );
    painter.line_segment([to, to + left * len], stroke);
    painter.line_segment([to, to + right * len], stroke);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same clamp the Structure tab needs: at the auto-fit scale the two bounds
    /// can cross by a rounding hair, and a naive `clamp` then panics with
    /// "min > max". Ordering them is what makes it total.
    #[test]
    fn clamp_rel_survives_bounds_that_cross() {
        let v = clamp_rel(10.0, 500.0, 500.00003);
        assert!(v.is_finite());
    }

    #[test]
    fn clamp_rel_pins_a_fitting_chart_inside_the_pad() {
        assert_eq!(clamp_rel(-999.0, 500.0, 100.0), FIT_PAD);
    }

    /// An overflowing chart may be dragged, but never past its own far edge.
    #[test]
    fn clamp_rel_stops_an_overflowing_chart_at_its_edge() {
        let v = clamp_rel(999.0, 500.0, 900.0);
        assert_eq!(v, FIT_PAD);
        let v = clamp_rel(-999.0, 500.0, 900.0);
        assert_eq!(v, 500.0 - 900.0 - FIT_PAD);
    }

    /// Every shape must have a fill that is actually distinguishable — two
    /// shapes sharing a colour would make the legend a lie.
    #[test]
    fn every_shape_has_its_own_fill() {
        let all = [
            Shape::Terminal,
            Shape::Process,
            Shape::Io,
            Shape::Decision,
            Shape::Subroutine,
            Shape::Generated,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(fill(*a), fill(*b), "{a:?} and {b:?} share a fill");
            }
        }
    }

    const FILE: &str = "use core::fmt;\n\
                        const LIMIT: u32 = 10;\n\
                        struct Frame { a: u8 }\n\
                        impl Frame {\n    fn feed(&mut self) {}\n}\n\
                        #[entry]\nfn main() -> ! { loop {} }\n\
                        fn helper() {}\n";

    fn model() -> FileModel {
        crate::panels::flow_map::parse::parse_file(FILE).unwrap()
    }

    /// The picker lists functions, each under its container, and nothing that
    /// has no chart to open.
    #[test]
    fn the_picker_lists_functions_under_their_containers() {
        let rows = picker_rows(&model());
        let got: Vec<(usize, &str, Option<&str>, bool)> = rows
            .iter()
            .map(|r| (r.depth, r.label.as_str(), r.key.as_deref(), r.entry))
            .collect();
        assert_eq!(
            got,
            [
                (0, "impl Frame", None, false),
                (1, "feed  ·  fn", Some("Frame::feed"), false),
                (0, "main  ·  entry", Some("main"), true),
                (0, "helper  ·  fn", Some("helper"), false),
            ]
        );
    }

    /// The mode survives a save as a bit, and bits a newer build added are
    /// ignored rather than turning the view off.
    #[test]
    fn the_mode_bits_round_trip_and_ignore_what_they_do_not_know() {
        let mut v = FlowView::default();
        assert_eq!(v.mode_bits(), 0, "the default writes nothing");
        v.all = true;
        let bits = v.mode_bits();
        let mut back = FlowView::default();
        back.set_mode_bits(bits);
        assert!(back.all);
        back.set_mode_bits(bits | 0b1000_0000);
        assert!(back.all, "an unknown bit leaves the known one alone");
        back.set_mode_bits(0b1000_0000);
        assert!(!back.all);
    }

    /// Opening a function always lands on its chart, fitted - including from
    /// the whole-file view, which it leaves.
    #[test]
    fn opening_a_chart_leaves_the_whole_file_view() {
        let mut v = FlowView {
            all: true,
            zoom: 3.0,
            pan: egui::vec2(40.0, -12.0),
            ..Default::default()
        };
        v.open("Frame::feed".to_string());
        assert!(!v.all);
        assert_eq!(v.selected, "Frame::feed");
        assert_eq!((v.zoom, v.pan), (1.0, egui::Vec2::ZERO));
    }

    #[test]
    fn a_use_row_tooltip_reads_as_code() {
        let m = crate::panels::flow_map::parse::parse_file("use a::b;\nuse c::d;\n").unwrap();
        let tip = outline_tip(&m.elements[0]);
        assert!(tip.starts_with("use a::b;\nuse c::d;\n"), "{tip}");
        assert!(tip.contains("lines 1–2"), "{tip}");
    }

    /// Every string one frame of the tab paints, plus what it reported.
    fn frame(
        ctx: &egui::Context,
        m: &FileModel,
        view: &mut FlowView,
        events: Vec<egui::Event>,
        time: f64,
    ) -> (Vec<String>, ShowResult) {
        fn walk(s: &egui::Shape, out: &mut Vec<String>) {
            match s {
                egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 800.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        };
        let empty = FlowLayout::default();
        let mut result = ShowResult::default();
        let shapes = ctx
            .run_ui(input, |ui| {
                result = show(ui, m, &empty, view, "");
            })
            .shapes;
        let mut out = Vec::new();
        for s in &shapes {
            walk(&s.shape, &mut out);
        }
        (out, result)
    }

    /// "All — whole file" paints one row per element - kind word and
    /// declaration - and names itself in the picker.
    #[test]
    fn the_whole_file_view_lists_every_element() {
        let m = model();
        let mut view = FlowView {
            all: true,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let (texts, _) = frame(&ctx, &m, &mut view, Vec::new(), 0.0);
        assert!(texts.iter().any(|t| t == ALL_LABEL), "{texts:?}");
        for e in &m.elements {
            assert!(
                texts.contains(&e.signature),
                "row for {} missing: {texts:?}",
                e.key
            );
            assert!(texts.iter().any(|t| t == e.kind.word()), "{texts:?}");
        }
        // It is a list: none of the chart view's legend.
        assert!(!texts.iter().any(|t| t == "decision"), "{texts:?}");
    }

    /// A click on a row jumps the editor and keeps the list; a double click on
    /// a function opens its chart.
    #[test]
    fn a_click_jumps_and_a_double_click_opens() {
        let m = model();
        let mut view = FlowView {
            all: true,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let (_, first) = frame(&ctx, &m, &mut view, Vec::new(), 0.0);
        assert!(first.goto_line.is_none());

        // Where the `helper` row lands depends on the toolbar above the list,
        // so find it the way a user would: click down the list until the
        // click jumps to `helper`'s line.
        let helper = m.elements.iter().position(|e| e.key == "helper").unwrap();
        let mut hit = None;
        for y in (60..600).step_by(3) {
            let pos = egui::pos2(300.0, y as f32);
            let (_, r) = frame(
                &ctx,
                &m,
                &mut view,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                1.0 + y as f64,
            );
            if r.goto_line == Some(m.elements[helper].ident_line) {
                hit = Some(pos);
                break;
            }
        }
        let pos = hit.expect("clicking the helper row jumps to its line");
        assert!(view.all, "a single click keeps the list");

        // Two clicks in quick succession on the same row: a double click.
        let click = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let t = 5000.0;
        frame(&ctx, &m, &mut view, vec![egui::Event::PointerMoved(pos)], t);
        frame(
            &ctx,
            &m,
            &mut view,
            vec![click(true), click(false)],
            t + 0.05,
        );
        let (_, r) = frame(
            &ctx,
            &m,
            &mut view,
            vec![click(true), click(false)],
            t + 0.10,
        );
        assert_eq!(r.open_chart.as_deref(), Some("helper"));
    }

    #[test]
    fn every_edge_kind_has_its_own_colour_except_the_two_that_mean_the_same() {
        // Break and Return are both "leave through the right-hand lane", and
        // sharing one colour is deliberate — they are the same gesture.
        assert_eq!(edge_color(EdgeKind::Break), edge_color(EdgeKind::Return));
        for k in [
            EdgeKind::Flow,
            EdgeKind::Back,
            EdgeKind::Continue,
            EdgeKind::Try,
        ] {
            assert_ne!(edge_color(k), edge_color(EdgeKind::Break));
        }
    }
}
