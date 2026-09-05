//! Draw a laid-out [`FlowLayout`] — the only part of the Flow tab that touches
//! egui.
//!
//! The view controls are deliberately the SAME as the Structure tab's, because
//! they sit next to each other in the Project group and a diagram that pans
//! differently from the diagram beside it is a diagram the user fights: auto-fit
//! as the base scale, mouse wheel and Ctrl+± on top, background drag to pan,
//! Ctrl+0 to re-centre.

use super::layout::{Edge, EdgeKind, FlowLayout, Placed};
use super::parse::{Chart, Shape};
use eframe::egui;

/// Session view state for the Flow tab.
pub struct FlowView {
    /// User zoom over the auto-fit base (1.0 = the whole chart fits).
    pub zoom: f32,
    /// View offset from centred, in screen px.
    pub pan: egui::Vec2,
    /// The scale actually drawn last frame, so the toolbar can say so.
    pub last_scale: f32,
    /// Which function is charted, BY NAME — an index would silently point at a
    /// different function the moment the file is edited.
    pub selected: String,
}

impl Default for FlowView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            last_scale: 1.0,
            selected: String::new(),
        }
    }
}

/// What one frame of the chart reports back to the driver.
#[derive(Default)]
pub struct ShowResult {
    /// Jump the editor to this 1-based line of the charted file.
    pub goto_line: Option<usize>,
    /// A subroutine box was opened — chart this function instead.
    pub open_chart: Option<String>,
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
    charts: &[Chart],
    lay: &FlowLayout,
    view: &mut FlowView,
    status: &str,
) -> ShowResult {
    let mut result = ShowResult::default();

    // ── Toolbar ───────────────────────────────────────────────────────────
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Function").size(11.0).color(DIM_TEXT));
        let current = if view.selected.is_empty() {
            "—".to_string()
        } else {
            view.selected.clone()
        };
        egui::ComboBox::from_id_salt("flow_chart_pick")
            .selected_text(egui::RichText::new(current).size(11.5))
            .width(240.0)
            .show_ui(ui, |ui| {
                for c in charts {
                    // Entry points lead with what starts them; an `#[interrupt]`
                    // in the same list as a helper `fn` is the difference
                    // between "hardware calls this" and "someone calls this".
                    let text = egui::RichText::new(format!("{}  ·  {}", c.name, c.kind.word()))
                        .size(11.5)
                        .color(if c.kind.is_entry() { TEXT } else { DIM_TEXT });
                    if ui.selectable_label(view.selected == c.name, text).clicked() {
                        view.selected = c.name.clone();
                        view.pan = egui::Vec2::ZERO;
                        view.zoom = 1.0;
                    }
                }
            });

        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(format!(
                "{} boxes · {} edges",
                lay.boxes.len(),
                lay.edges.len()
            ))
            .size(11.0)
            .color(DIM_TEXT),
        );
        if view.last_scale < LEGIBLE_SCALE {
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
            "Ctrl+± / mouse wheel zoom, Ctrl+0 reset · drag the background = pan · \
             click a box = go to its line",
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
    let pointer = ui.ctx().pointer_latest_pos().filter(|p| rect.contains(*p));
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
        if b.node.shape == Shape::Subroutine && b.node.goto_line.is_some() {
            let target = b.node.goto_line.unwrap();
            if let Some(c) = charts.iter().find(|c| c.line == target) {
                if bg.clicked() {
                    result.open_chart = Some(c.name.clone());
                    result.goto_line = Some(b.node.line);
                }
            } else if bg.clicked() {
                result.goto_line = Some(b.node.line);
            }
        } else if bg.clicked() {
            result.goto_line = Some(b.node.line);
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
