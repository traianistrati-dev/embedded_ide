//! Draw the module diagram: fit-to-panel with zoom + pan, dashed containment
//! lines, solid dependency arrows, and clickable nodes (click → the caller
//! opens that node's file in the editor).

use super::layout::GraphLayout;
use super::parse::ModuleGraph;
use eframe::egui;

/// Session view state of the diagram (not persisted).
pub struct StructureView {
    pub zoom: f32,
    pub pan: egui::Vec2,
}

impl Default for StructureView {
    fn default() -> Self {
        Self { zoom: 1.0, pan: egui::Vec2::ZERO }
    }
}

/// A click on a node: which file to open (`None` = main.rs).
pub struct NodeClick {
    pub file: Option<usize>,
}

const NODE_FILL: egui::Color32 = egui::Color32::from_rgb(46, 52, 66);
const ROOT_FILL: egui::Color32 = egui::Color32::from_rgb(60, 58, 44);
const NODE_STROKE: egui::Color32 = egui::Color32::from_rgb(96, 106, 128);
const HOVER_STROKE: egui::Color32 = egui::Color32::from_rgb(120, 170, 240);
const DEP_COLOR: egui::Color32 = egui::Color32::from_rgb(110, 145, 215);
const CONTAIN_COLOR: egui::Color32 = egui::Color32::from_rgb(105, 105, 115);

/// Render the diagram; returns `Some(NodeClick)` when a node was clicked.
pub fn show(
    ui: &mut egui::Ui,
    graph: &ModuleGraph,
    lay: &GraphLayout,
    view: &mut StructureView,
) -> Option<NodeClick> {
    // ── Toolbar ───────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} modules · {} dependencies · {} mod declarations",
                graph.nodes.len(),
                graph.deps.len(),
                graph.contains.len()
            ))
            .size(11.0)
            .color(egui::Color32::from_rgb(150, 150, 160)),
        );
        ui.separator();
        ui.label(egui::RichText::new("Zoom").size(11.0));
        ui.add(
            egui::Slider::new(&mut view.zoom, 0.25..=3.0)
                .show_value(false)
                .clamping(egui::SliderClamping::Always),
        );
        if ui
            .small_button("Fit")
            .on_hover_text("Reset zoom and pan to fit the whole diagram")
            .clicked()
        {
            view.zoom = 1.0;
            view.pan = egui::Vec2::ZERO;
        }
        ui.label(
            egui::RichText::new("· drag to pan · click a module to open its file")
                .size(10.5)
                .color(egui::Color32::from_rgb(120, 120, 130)),
        );
    });
    ui.add_space(2.0);

    // ── Canvas ────────────────────────────────────────────────────────────
    let avail = ui.available_size();
    if avail.x < 40.0 || avail.y < 40.0 || lay.pos.is_empty() {
        return None;
    }
    let (rect, bg_resp) = ui.allocate_exact_size(avail, egui::Sense::click_and_drag());
    if bg_resp.dragged() {
        view.pan += bg_resp.drag_delta();
    }

    // Fit the virtual bounds into the canvas, then apply the user zoom.
    let fit = (rect.width() / lay.width)
        .min(rect.height() / lay.height)
        .min(1.6);
    let scale = (fit * view.zoom).max(0.08);
    let content = egui::vec2(lay.width, lay.height) * scale;
    // Center when smaller than the canvas, top-left anchor when larger.
    let free = (rect.size() - content) * 0.5;
    let origin = rect.left_top()
        + egui::vec2(free.x.max(8.0), free.y.max(8.0))
        + view.pan;
    let to_screen =
        |x: f32, y: f32| -> egui::Pos2 { origin + egui::vec2(x, y) * scale };

    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(24, 26, 32));

    let stroke_w = (1.3 * scale).clamp(0.6, 2.4);

    // ── Containment edges (dashed, parent bottom → child top) ─────────────
    for &(p, c) in &graph.contains {
        let a = lay.pos[p];
        let b = lay.pos[c];
        let from = to_screen(a.center_x(), a.bottom());
        let to = to_screen(b.center_x(), b.y);
        painter.add(egui::Shape::dashed_line(
            &[from, to],
            egui::Stroke::new(stroke_w, CONTAIN_COLOR),
            4.0 * scale.max(0.5),
            4.0 * scale.max(0.5),
        ));
    }

    // ── Dependency edges (solid arrows, user bottom → used top) ───────────
    for &(u, v) in &graph.deps {
        let a = lay.pos[u];
        let b = lay.pos[v];
        // A back-edge (cycle) leaves from the top and enters the bottom.
        let downward = b.y > a.y;
        let from = if downward {
            to_screen(a.center_x(), a.bottom())
        } else {
            to_screen(a.center_x(), a.y)
        };
        let to = if downward {
            to_screen(b.center_x(), b.y)
        } else {
            to_screen(b.center_x(), b.bottom())
        };
        let stroke = egui::Stroke::new(stroke_w, DEP_COLOR);
        painter.line_segment([from, to], stroke);
        arrowhead(&painter, from, to, 7.0 * scale.clamp(0.5, 1.6), stroke);
    }

    // ── Nodes ─────────────────────────────────────────────────────────────
    let mut clicked: Option<NodeClick> = None;
    let name_font = egui::FontId::proportional((11.5 * scale).clamp(6.0, 24.0));
    let sub_font = egui::FontId::proportional((8.5 * scale).clamp(5.0, 18.0));
    for (i, node) in graph.nodes.iter().enumerate() {
        let p = lay.pos[i];
        let r = egui::Rect::from_min_size(
            to_screen(p.x, p.y),
            egui::vec2(p.w, p.h) * scale,
        );
        if !rect.intersects(r) {
            continue;
        }
        let resp = ui.interact(
            r.intersect(rect),
            ui.id().with(("structure_node", i)),
            egui::Sense::click(),
        );
        let fill = if i == 0 { ROOT_FILL } else { NODE_FILL };
        let stroke_c = if resp.hovered() { HOVER_STROKE } else { NODE_STROKE };
        painter.rect(
            r,
            4.0 * scale.clamp(0.5, 1.5),
            fill,
            egui::Stroke::new(if resp.hovered() { 1.6 } else { 1.0 }, stroke_c),
            egui::StrokeKind::Inside,
        );

        // Name (upper line) + a small fn/ty badge (lower line, hidden when tiny).
        let show_sub = scale > 0.45;
        let name_y = if show_sub { r.center().y - r.height() * 0.18 } else { r.center().y };
        painter.text(
            egui::pos2(r.center().x, name_y),
            egui::Align2::CENTER_CENTER,
            &node.name,
            name_font.clone(),
            egui::Color32::WHITE,
        );
        if show_sub {
            painter.text(
                egui::pos2(r.center().x, r.center().y + r.height() * 0.24),
                egui::Align2::CENTER_CENTER,
                format!("{} fn · {} ty", node.fn_count, node.ty_count),
                sub_font.clone(),
                egui::Color32::from_rgb(150, 158, 172),
            );
        }

        let full = if node.path.is_empty() { "crate root" } else { &node.path };
        resp.clone().on_hover_text(format!(
            "{full}\n{}\n{} fn · {} struct/enum/trait\nClick to open in the editor",
            node.file_rel, node.fn_count, node.ty_count
        ));
        if resp.clicked() {
            clicked = Some(NodeClick { file: node.file });
        }
    }

    clicked
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
