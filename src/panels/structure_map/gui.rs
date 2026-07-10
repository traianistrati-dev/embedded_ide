//! Draw the module diagram: fit-to-panel with zoom + pan, dashed containment
//! lines, solid dependency arrows, and clickable nodes (click → the caller
//! opens that node's file in the editor).

use super::calls::CallEdge;
use super::layout::{
    recompute_bounds, shown_rows, GraphLayout, HEADER_H, MARGIN, MAX_SYMBOL_ROWS, ROW_H,
    ROW_NAME_CHARS,
};
use super::parse::{ModuleGraph, SymKind};
use eframe::egui;

/// Session view state of the diagram (not persisted).
pub struct StructureView {
    pub zoom: f32,
    pub pan: egui::Vec2,
    /// Draw the cross-module call edges (Phase 3) over the diagram.
    pub show_calls: bool,
}

impl Default for StructureView {
    fn default() -> Self {
        Self { zoom: 1.0, pan: egui::Vec2::ZERO, show_calls: true }
    }
}

/// A click on a node: which file to open (`None` = main.rs) and, when a
/// symbol row was clicked, the 1-based line to jump to.
pub struct NodeClick {
    pub file: Option<usize>,
    pub line: Option<usize>,
}

/// Everything one frame of the diagram can report back to the driver.
#[derive(Default)]
pub struct ShowResult {
    /// A node / symbol-row click (open the file, optionally jump to a line).
    pub click: Option<NodeClick>,
    /// A header drag ended on this node — persist its position override.
    pub moved: Option<usize>,
    /// The "Auto layout" button was clicked — clear overrides and re-lay-out.
    pub reset_layout: bool,
}

const NODE_FILL: egui::Color32 = egui::Color32::from_rgb(46, 52, 66);
const ROOT_FILL: egui::Color32 = egui::Color32::from_rgb(60, 58, 44);
const NODE_STROKE: egui::Color32 = egui::Color32::from_rgb(96, 106, 128);
const HOVER_STROKE: egui::Color32 = egui::Color32::from_rgb(120, 170, 240);
const DEP_COLOR: egui::Color32 = egui::Color32::from_rgb(110, 145, 215);
const CONTAIN_COLOR: egui::Color32 = egui::Color32::from_rgb(105, 105, 115);
const CALL_COLOR: egui::Color32 = egui::Color32::from_rgba_premultiplied(200, 150, 50, 160);

/// Render the diagram; the [`ShowResult`] carries clicks, a finished node drag
/// and the Auto-layout request. `lay` is mutable: dragging a node's HEADER
/// moves it live (edges/ports/detours recompute from `lay.pos` every frame).
/// `calls` are the Phase-3 cross-module call edges (drawn when `view.show_calls`);
/// `calls_status` is a short toolbar note ("analyzing 12/47…" / save hint / "").
pub fn show(
    ui: &mut egui::Ui,
    graph: &ModuleGraph,
    lay: &mut GraphLayout,
    view: &mut StructureView,
    calls: &[CallEdge],
    calls_status: &str,
) -> ShowResult {
    let mut result = ShowResult::default();
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
        if ui
            .small_button("Auto layout")
            .on_hover_text(
                "Discard the manually dragged positions and re-run the \
                 automatic arrangement",
            )
            .clicked()
        {
            result.reset_layout = true;
        }
        ui.separator();
        ui.checkbox(&mut view.show_calls, egui::RichText::new("Calls").size(11.0))
            .on_hover_text(
                "Show cross-module call edges (amber): which fn/struct is used \
                 by which item of another module. Computed via rust-analyzer, \
                 one symbol at a time, only while the project is saved/in sync.",
            );
        if !calls_status.is_empty() {
            ui.label(
                egui::RichText::new(calls_status)
                    .size(10.5)
                    .color(egui::Color32::from_rgb(200, 160, 70)),
            );
        }
        ui.label(
            egui::RichText::new(
                "· drag background = pan · drag a module's header = move it · \
                 click = open",
            )
            .size(10.5)
            .color(egui::Color32::from_rgb(120, 120, 130)),
        );
    });
    ui.add_space(2.0);

    // ── Canvas ────────────────────────────────────────────────────────────
    let avail = ui.available_size();
    if avail.x < 40.0 || avail.y < 40.0 || lay.pos.is_empty() {
        return result;
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

    // ── Module edges (containment dashed + dependency solid), routed ──────
    // Two anti-overlap measures: (1) PORT SPREADING — each edge endpoint gets
    // its own x along the node's top/bottom edge, ordered by where the other
    // end sits, so edges never converge on one point; (2) OBSTACLE DETOUR — an
    // edge whose straight segment would cut through another node box becomes a
    // lateral bezier around the blockers instead.
    {
        // (u, v, is_dep): deps drawn solid + arrowhead, containment dashed.
        let module_edges: Vec<(usize, usize, bool)> = graph
            .contains
            .iter()
            .map(|&(u, v)| (u, v, false))
            .chain(graph.deps.iter().map(|&(u, v)| (u, v, true)))
            .collect();

        // Ports in VIRTUAL coords. Endpoint id = 2·edge (source) / 2·edge+1
        // (target). A downward edge exits u's bottom and enters v's top; a
        // back edge (cycle) exits the top and enters the bottom.
        let mut port_x = vec![0.0f32; module_edges.len() * 2];
        let mut groups: std::collections::HashMap<(usize, bool), Vec<(usize, f32)>> =
            std::collections::HashMap::new();
        for (ei, &(u, v, _)) in module_edges.iter().enumerate() {
            let downward = lay.pos[v].y > lay.pos[u].y;
            groups
                .entry((u, !downward))
                .or_default()
                .push((ei * 2, lay.pos[v].center_x()));
            groups
                .entry((v, downward))
                .or_default()
                .push((ei * 2 + 1, lay.pos[u].center_x()));
        }
        for ((node, _is_top), mut ends) in groups {
            ends.sort_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.0.cmp(&b.0))
            });
            let p = lay.pos[node];
            let count = ends.len() as f32;
            for (k, (endpoint, _)) in ends.into_iter().enumerate() {
                let frac = (k as f32 + 1.0) / (count + 1.0);
                port_x[endpoint] = p.x + p.w * (0.12 + 0.76 * frac);
            }
        }

        let dash = 4.0 * scale.max(0.5);
        for (ei, &(u, v, is_dep)) in module_edges.iter().enumerate() {
            let (a, b) = (lay.pos[u], lay.pos[v]);
            let downward = b.y > a.y;
            let from_v = (port_x[ei * 2], if downward { a.bottom() } else { a.y });
            let to_v = (port_x[ei * 2 + 1], if downward { b.y } else { b.bottom() });

            // Straight segment blocked by a non-endpoint node box? → detour x
            // just past the blockers, on whichever side deviates less.
            let mut blocked = false;
            let (mut left_cand, mut right_cand) = (f32::MAX, f32::MIN);
            for (k, q) in lay.pos.iter().enumerate() {
                if k == u || k == v {
                    continue;
                }
                if crate::panels::structure_map::layout::seg_hits_rect(
                    from_v, to_v, q.x, q.y, q.w, q.h,
                ) {
                    blocked = true;
                    left_cand = left_cand.min(q.x);
                    right_cand = right_cand.max(q.x + q.w);
                }
            }
            let stroke = egui::Stroke::new(
                stroke_w,
                if is_dep { DEP_COLOR } else { CONTAIN_COLOR },
            );
            let from = to_screen(from_v.0, from_v.1);
            let to = to_screen(to_v.0, to_v.1);
            if !blocked {
                if is_dep {
                    painter.line_segment([from, to], stroke);
                    arrowhead(&painter, from, to, 7.0 * scale.clamp(0.5, 1.6), stroke);
                } else {
                    painter.add(egui::Shape::dashed_line(&[from, to], stroke, dash, dash));
                }
            } else {
                const PAD: f32 = 14.0;
                let mid = (from_v.0 + to_v.0) / 2.0;
                let (l, r) = (left_cand - PAD, right_cand + PAD);
                let detour_x = if (mid - l).abs() <= (r - mid).abs() { l } else { r };
                let c1 = to_screen(detour_x, from_v.1 + 0.30 * (to_v.1 - from_v.1));
                let c2 = to_screen(detour_x, from_v.1 + 0.70 * (to_v.1 - from_v.1));
                let bez = egui::epaint::CubicBezierShape::from_points_stroke(
                    [from, c1, c2, to],
                    false,
                    egui::Color32::TRANSPARENT,
                    stroke,
                );
                if is_dep {
                    painter.add(bez);
                    arrowhead(&painter, c2, to, 7.0 * scale.clamp(0.5, 1.6), stroke);
                } else {
                    // Dashed beziers aren't a primitive — flatten to a polyline.
                    let pts = bez.flatten(Some(2.0));
                    painter.add(egui::Shape::dashed_line(&pts, stroke, dash, dash));
                }
            }
        }
    }

    // ── Nodes ─────────────────────────────────────────────────────────────
    let name_font = egui::FontId::proportional((11.5 * scale).clamp(6.0, 24.0));
    let sub_font = egui::FontId::proportional((8.5 * scale).clamp(5.0, 18.0));
    let row_font = egui::FontId::monospace((8.0 * scale).clamp(5.0, 16.0));
    // Symbol rows are unreadable below this scale — draw compact nodes instead.
    let show_detail = scale > 0.45;
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

        // ── Header drag: move the node ─────────────────────────────────────
        // The header band is the drag handle (the whole box when compact) —
        // registered after the node's click response so the pointer prefers
        // it there; symbol rows below keep their click-to-jump untouched.
        // Deltas are divided by `scale` (screen → virtual). Edges recompute
        // from `lay.pos` next frame, so they follow with a one-frame lag —
        // the usual immediate-mode drag behaviour.
        let pre_rows = if show_detail { shown_rows(node.symbols.len()) } else { 0 };
        let handle_h = if pre_rows > 0 { HEADER_H * scale } else { r.height() };
        let drag_resp = ui
            .interact(
                egui::Rect::from_min_size(r.min, egui::vec2(r.width(), handle_h))
                    .intersect(rect),
                ui.id().with(("structure_drag", i)),
                egui::Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab);
        if drag_resp.dragged() {
            let d = drag_resp.drag_delta() / scale;
            lay.pos[i].x = (lay.pos[i].x + d.x).max(MARGIN);
            lay.pos[i].y = (lay.pos[i].y + d.y).max(MARGIN);
        }
        if drag_resp.drag_stopped() {
            recompute_bounds(lay);
            result.moved = Some(i);
        }
        // Re-read the (possibly just-moved) position for drawing.
        let p = lay.pos[i];
        let r = egui::Rect::from_min_size(
            to_screen(p.x, p.y),
            egui::vec2(p.w, p.h) * scale,
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

        // ── Header band: name + fn/ty badge ───────────────────────────────
        // Rows below need the detail scale; a row-less (or zoomed-out) node
        // centers the header content in the whole box instead.
        let rows = if show_detail { shown_rows(node.symbols.len()) } else { 0 };
        let header = if rows > 0 {
            egui::Rect::from_min_size(r.min, egui::vec2(r.width(), HEADER_H * scale))
        } else {
            r
        };
        let name_y = if show_detail {
            header.center().y - header.height() * 0.18
        } else {
            header.center().y
        };
        painter.text(
            egui::pos2(header.center().x, name_y),
            egui::Align2::CENTER_CENTER,
            &node.name,
            name_font.clone(),
            egui::Color32::WHITE,
        );
        if show_detail {
            painter.text(
                egui::pos2(header.center().x, header.center().y + header.height() * 0.24),
                egui::Align2::CENTER_CENTER,
                format!("{} fn · {} ty", node.fn_count, node.ty_count),
                sub_font.clone(),
                egui::Color32::from_rgb(150, 158, 172),
            );
        }

        // ── Symbol rows (top-level fn/struct/enum/trait; click → jump) ────
        let mut row_clicked = false;
        if rows > 0 {
            painter.line_segment(
                [
                    egui::pos2(r.left() + 4.0 * scale, header.bottom()),
                    egui::pos2(r.right() - 4.0 * scale, header.bottom()),
                ],
                egui::Stroke::new(0.8, NODE_STROKE),
            );
            let row_h = ROW_H * scale;
            for j in 0..rows {
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(r.left(), header.bottom() + j as f32 * row_h),
                    egui::vec2(r.width(), row_h),
                );
                // Trailing "+K more" row when truncated (not clickable).
                if node.symbols.len() > MAX_SYMBOL_ROWS && j == rows - 1 {
                    painter.text(
                        egui::pos2(row_rect.left() + 6.0 * scale, row_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        format!("+{} more", node.symbols.len() - MAX_SYMBOL_ROWS),
                        row_font.clone(),
                        egui::Color32::from_rgb(130, 135, 150),
                    );
                    continue;
                }
                let sym = &node.symbols[j];
                let row_resp = ui.interact(
                    row_rect.intersect(rect),
                    ui.id().with(("structure_row", i, j)),
                    egui::Sense::click(),
                );
                if row_resp.hovered() {
                    painter.rect_filled(
                        row_rect,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(120, 170, 240, 26),
                    );
                }
                let (glyph, g_color) = kind_glyph(sym.kind);
                painter.text(
                    egui::pos2(row_rect.left() + 6.0 * scale, row_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    glyph,
                    row_font.clone(),
                    g_color,
                );
                let name: String = if sym.name.chars().count() > ROW_NAME_CHARS {
                    let mut s: String =
                        sym.name.chars().take(ROW_NAME_CHARS - 1).collect();
                    s.push('…');
                    s
                } else {
                    sym.name.clone()
                };
                painter.text(
                    egui::pos2(row_rect.left() + 16.0 * scale, row_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    row_font.clone(),
                    egui::Color32::from_rgb(200, 205, 215),
                );
                row_resp.clone().on_hover_text(format!(
                    "{} {} — line {}\nClick to jump to it",
                    kind_word(sym.kind),
                    sym.name,
                    sym.line
                ));
                if row_resp.clicked() {
                    result.click = Some(NodeClick { file: node.file, line: Some(sym.line) });
                    row_clicked = true;
                }
            }
        }

        let full = if node.path.is_empty() { "crate root" } else { &node.path };
        resp.clone().on_hover_text(format!(
            "{full}\n{}\n{} fn · {} struct/enum/trait\nClick to open in the editor",
            node.file_rel, node.fn_count, node.ty_count
        ));
        // The header's drag-sense response wins the pointer there, so a plain
        // click on the header surfaces as `drag_resp.clicked()`.
        if (resp.clicked() || drag_resp.clicked()) && !row_clicked {
            result.click = Some(NodeClick { file: node.file, line: None });
        }
    }

    // ── Call edges (Phase 3): caller row → callee row, amber beziers ──────
    // Drawn OVER the nodes (they attach to row side-edges and travel between
    // nodes) with a translucent stroke so the text stays readable.
    if view.show_calls && !calls.is_empty() {
        let anchor = |ni: usize, row: usize, right_side: bool| -> egui::Pos2 {
            let p = lay.pos[ni];
            let visible = graph.nodes[ni].symbols.len().min(MAX_SYMBOL_ROWS);
            // Row center when rows are drawn; node center when zoomed out or
            // the row sits past the "+K more" cut.
            let y = if show_detail && row < visible {
                p.y + HEADER_H + (row as f32 + 0.5) * ROW_H
            } else {
                p.y + p.h / 2.0
            };
            let x = if right_side { p.x + p.w } else { p.x };
            to_screen(x, y)
        };
        let stroke = egui::Stroke::new((1.1 * scale).clamp(0.5, 2.0), CALL_COLOR);
        for e in calls {
            let going_right =
                lay.pos[e.to_node].center_x() >= lay.pos[e.from_node].center_x();
            let from = anchor(e.from_node, e.from_row, going_right);
            let to = anchor(e.to_node, e.to_row, !going_right);
            let dx = ((to.x - from.x).abs() * 0.4)
                .clamp(20.0 * scale, 120.0 * scale)
                * if going_right { 1.0 } else { -1.0 };
            let c1 = from + egui::vec2(dx, 0.0);
            let c2 = to + egui::vec2(-dx, 0.0);
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [from, c1, c2, to],
                false,
                egui::Color32::TRANSPARENT,
                stroke,
            ));
            arrowhead(&painter, c2, to, 6.0 * scale.clamp(0.5, 1.5), stroke);
        }
    }

    result
}

/// Glyph letter + colour for a symbol kind (drawn before the name in a row).
fn kind_glyph(kind: SymKind) -> (&'static str, egui::Color32) {
    match kind {
        SymKind::Fn => ("f", egui::Color32::from_rgb(130, 170, 240)),
        SymKind::Struct => ("S", egui::Color32::from_rgb(230, 160, 80)),
        SymKind::Enum => ("E", egui::Color32::from_rgb(190, 130, 230)),
        SymKind::Trait => ("T", egui::Color32::from_rgb(120, 200, 140)),
    }
}

fn kind_word(kind: SymKind) -> &'static str {
    match kind {
        SymKind::Fn => "fn",
        SymKind::Struct => "struct",
        SymKind::Enum => "enum",
        SymKind::Trait => "trait",
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
