//! Draw the module diagram: fit-to-panel with zoom + pan, dashed containment
//! lines, solid dependency arrows, and clickable nodes (click → the caller
//! opens that node's file in the editor).

use super::calls::CallEdge;
use super::layout::{
    GraphLayout, HEADER_H, MARGIN, MAX_SYMBOL_ROWS, ROW_H, ROW_NAME_CHARS, recompute_bounds,
    shown_rows,
};
use super::parse::{ModuleGraph, SymKind};
use eframe::egui;

/// Session view state of the diagram (not persisted). The BASE zoom is
/// automatic — the whole diagram fits the panel with [`FIT_PAD`] padding,
/// recomputed every frame so panel/window resizes always track. `zoom` is a
/// user multiplier on top (Ctrl+± while hovering the diagram, Ctrl+0 resets)
/// — at 1.0 the diagram always fits; above it, scrollbars take over.
pub struct StructureView {
    /// User zoom multiplier over the auto-fit base (1.0 = pure auto-fit).
    pub zoom: f32,
    /// Draw the cross-module call edges (Phase 3) over the diagram.
    pub show_calls: bool,
    /// How many call-hops BELOW the focused module to draw: `Some(1)` = only
    /// its direct edges (default), `Some(n)` = the downstream tree n levels
    /// deep, `None` = "All" (the whole tree under the selected module).
    pub call_depth: Option<usize>,
}

impl Default for StructureView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            show_calls: true,
            call_depth: Some(1),
        }
    }
}

/// Padding kept around the diagram when it auto-fits the panel.
const FIT_PAD: f32 = 20.0;

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

const ROOT_FILL: egui::Color32 = egui::Color32::from_rgb(60, 58, 44);
/// Per-PACKAGE fills: every node of one top-level module subtree (same first
/// path segment — `pins::configs::usart1` → "pins") shares a colour, and each
/// package gets a different one, for visual separation. Muted dark hues so the
/// white node text stays readable on the dark canvas; assigned in
/// first-encounter node order, cycling when there are more packages.
const PKG_PALETTE: [egui::Color32; 8] = [
    egui::Color32::from_rgb(46, 52, 66), // slate blue
    egui::Color32::from_rgb(38, 60, 56), // teal
    egui::Color32::from_rgb(58, 46, 68), // violet
    egui::Color32::from_rgb(64, 54, 40), // amber brown
    egui::Color32::from_rgb(42, 60, 46), // green
    egui::Color32::from_rgb(64, 46, 52), // maroon
    egui::Color32::from_rgb(38, 56, 68), // steel cyan
    egui::Color32::from_rgb(56, 50, 58), // plum grey
];
const NODE_STROKE: egui::Color32 = egui::Color32::from_rgb(96, 106, 128);
/// Blinking border of nodes whose file carries error diagnostics.
const ERROR_STROKE: egui::Color32 = egui::Color32::from_rgb(230, 70, 60);
// const HOVER_STROKE: egui::Color32 = egui::Color32::from_rgb(120, 170, 240);
const HOVER_STROKE: egui::Color32 = egui::Color32::from_rgb(250, 250, 250);
/// Module dependency edges (solid, straight): LIGHT GRAY — the old blue now
/// belongs to fn-call edges (see below), so the module-level wiring recedes
/// into the background while stays brighter than the dashed containment.
const DEP_COLOR: egui::Color32 = egui::Color32::from_rgb(80, 80, 80);
const CONTAIN_COLOR: egui::Color32 = egui::Color32::from_rgb(105, 105, 115);

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
    // Focused module (index): only call edges touching it are drawn — showing
    // every collected edge at once was unreadable. Driven by the currently
    // selected FILE (main.rs by default), so clicking a node (which opens its
    // file) also moves the focus.
    focus_node: usize,
    // Per-node error flag (diagnostics in that module's file): the node's
    // border blinks red at 3× width so broken files stand out.
    node_errors: &[bool],
    // Total reference sites per symbol `(node, row)` — drawn right-aligned in
    // the row (single edges aggregate many sites; the count shows the total).
    ref_counts: &std::collections::HashMap<(usize, usize), usize>,
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
        ui.checkbox(
            &mut view.show_calls,
            egui::RichText::new("Calls").size(11.0),
        )
        .on_hover_text(
            "Show cross-module call edges, coloured by the TARGET's kind \
                 (like the row glyphs): blue = fn, orange = struct, purple = \
                 enum, green = trait. Computed via rust-analyzer, one symbol \
                 at a time, only while the project is saved/in sync.",
        );
        // Whose calls are shown (follows the selected file; main by default)
        // and how many hops DEEP below it the display drills.
        if view.show_calls {
            if let Some(node) = graph.nodes.get(focus_node) {
                // A package root (mod.rs) focuses its whole subtree.
                let label = if node.file_rel.ends_with("/mod.rs") {
                    format!("of {}::*", node.name)
                } else {
                    format!("of {}", node.name)
                };
                ui.label(
                    egui::RichText::new(label)
                        .size(10.5)
                        .color(egui::Color32::from_rgb(150, 158, 172)),
                )
                .on_hover_text(
                    "The call edges shown start from the selected file's module \
                     — selecting a package's mod.rs focuses the WHOLE package \
                     (all interior links + its outside connections). Select \
                     another file (tree / editor / node click) to move the focus.",
                );
            }
            egui::ComboBox::from_id_salt("structure_call_depth")
                .width(52.0)
                .selected_text(match view.call_depth {
                    Some(n) => n.to_string(),
                    None => "All".to_owned(),
                })
                .show_ui(ui, |ui| {
                    for n in 1..=10usize {
                        ui.selectable_value(&mut view.call_depth, Some(n), n.to_string());
                    }
                    ui.selectable_value(&mut view.call_depth, None, "All");
                })
                .response
                .on_hover_text(
                    "Depth of the displayed call tree below the focused module: \
                     1 = its direct edges only; N = follow callees N levels \
                     down; All = the whole tree under the selected module.",
                );
        }
        if !calls_status.is_empty() {
            ui.label(
                egui::RichText::new(calls_status)
                    .size(10.5)
                    .color(egui::Color32::from_rgb(200, 160, 70)),
            );
        }
        ui.label(
            egui::RichText::new(
                "· auto-fits the panel · Ctrl+± zoom, Ctrl+0 reset · drag a \
                 module's header = move it · click = open",
            )
            .size(10.5)
            .color(egui::Color32::from_rgb(120, 120, 130)),
        );
    });
    ui.add_space(2.0);

    // ── Canvas ────────────────────────────────────────────────────────────
    let avail = ui.available_size();
    if avail.x < 3.0 * FIT_PAD || avail.y < 3.0 * FIT_PAD || lay.pos.is_empty() {
        return result;
    }

    // Ctrl+± / Ctrl+0 zoom, consumed ONLY while the pointer hovers the diagram
    // area (the editor keeps its own Ctrl+± when hovered — hover routes them).
    if ui.rect_contains_pointer(ui.available_rect_before_wrap()) {
        ui.input_mut(|i| {
            let cmd = egui::Modifiers::COMMAND;
            if i.consume_key(cmd, egui::Key::Num0) {
                view.zoom = 1.0;
            } else if i.consume_key(cmd, egui::Key::Plus) || i.consume_key(cmd, egui::Key::Equals) {
                view.zoom = (view.zoom * 1.15).min(4.0);
            } else if i.consume_key(cmd, egui::Key::Minus) {
                view.zoom = (view.zoom / 1.15).max(0.3);
            }
        });
    }

    // AUTO-ZOOM base: the WHOLE diagram fits the panel with FIT_PAD padding,
    // recomputed every frame — panel/window resizes always rescale. The user
    // multiplier stacks on top; past 1.0 the scroll area takes over.
    let base = ((avail.x - 2.0 * FIT_PAD) / lay.width)
        .min((avail.y - 2.0 * FIT_PAD) / lay.height)
        .clamp(0.05, 2.5);
    let scale = (base * view.zoom).clamp(0.05, 5.0);
    let content = egui::vec2(lay.width, lay.height) * scale;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            show_canvas(
                ui,
                graph,
                lay,
                view,
                calls,
                scale,
                content,
                focus_node,
                node_errors,
                ref_counts,
                &mut result,
            );
        });

    result
}

/// The scaled diagram body, drawn inside the vertical scroll area.
#[allow(clippy::too_many_arguments)]
fn show_canvas(
    ui: &mut egui::Ui,
    graph: &ModuleGraph,
    lay: &mut GraphLayout,
    view: &mut StructureView,
    calls: &[CallEdge],
    scale: f32,
    content: egui::Vec2,
    focus_node: usize,
    node_errors: &[bool],
    ref_counts: &std::collections::HashMap<(usize, usize), usize>,
    result: &mut ShowResult,
) {
    // Fill at least the viewport (no background gap); when zoomed past the
    // fit, grow by the content + padding and let the scroll area take over.
    let size = egui::vec2(
        (content.x + 2.0 * FIT_PAD).max(ui.available_width()),
        (content.y + 2.0 * FIT_PAD).max(ui.available_height()),
    );
    let (rect, _bg_resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    // Center the diagram; `free` is ≥ FIT_PAD by construction of `size`.
    let free = (rect.size() - content) * 0.5;
    let origin = rect.left_top() + egui::vec2(free.x.max(FIT_PAD), free.y.max(FIT_PAD));
    let to_screen = |x: f32, y: f32| -> egui::Pos2 { origin + egui::vec2(x, y) * scale };

    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(24, 26, 32));

    let stroke_w = (1.3 * scale).clamp(0.6, 2.4);

    // Screen polylines of every edge already drawn — the call-edge router
    // below scores its candidate routes against them (crossing penalties).
    let mut edge_polys: Vec<Vec<egui::Pos2>> = Vec::new();

    // ── Module edges (containment dashed + dependency solid), routed ──────
    // CENTER anchoring (user fix): every dep/containment edge runs from node
    // CENTER to node CENTER — the convergence point hides BEHIND the node box
    // (edges draw before nodes), so nothing splays along the borders anymore;
    // only the arrowhead stays visible, clipped to the target's boundary.
    // OBSTACLE DETOUR kept: an edge whose straight segment would cut through
    // another node box becomes a lateral bezier around the blockers.
    {
        // (u, v, is_dep): deps drawn solid + arrowhead, containment dashed.
        let module_edges: Vec<(usize, usize, bool)> = graph
            .contains
            .iter()
            .map(|&(u, v)| (u, v, false))
            .chain(graph.deps.iter().map(|&(u, v)| (u, v, true)))
            .collect();

        let dash = 4.0 * scale.max(0.5);
        for &(u, v, is_dep) in &module_edges {
            let (a, b) = (lay.pos[u], lay.pos[v]);
            let from_v = (a.center_x(), a.y + a.h / 2.0);
            let to_v = (b.center_x(), b.y + b.h / 2.0);

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
            let stroke =
                egui::Stroke::new(stroke_w, if is_dep { DEP_COLOR } else { CONTAIN_COLOR });
            let from = to_screen(from_v.0, from_v.1);
            let to = to_screen(to_v.0, to_v.1);
            // Screen rect of the target — the arrowhead is drawn where the
            // edge crosses its boundary (the tail hides behind the boxes).
            let target_rect = egui::Rect::from_min_size(
                to_screen(b.x, b.y),
                egui::vec2(b.w, b.h) * scale,
            );
            let pts: Vec<egui::Pos2> = if !blocked {
                if is_dep {
                    painter.line_segment([from, to], stroke);
                } else {
                    painter.add(egui::Shape::dashed_line(&[from, to], stroke, dash, dash));
                }
                vec![from, to]
            } else {
                const PAD: f32 = 14.0;
                let mid = (from_v.0 + to_v.0) / 2.0;
                let (l, r) = (left_cand - PAD, right_cand + PAD);
                let detour_x = if (mid - l).abs() <= (r - mid).abs() {
                    l
                } else {
                    r
                };
                let c1 = to_screen(detour_x, from_v.1 + 0.30 * (to_v.1 - from_v.1));
                let c2 = to_screen(detour_x, from_v.1 + 0.70 * (to_v.1 - from_v.1));
                let bez = egui::epaint::CubicBezierShape::from_points_stroke(
                    [from, c1, c2, to],
                    false,
                    egui::Color32::TRANSPARENT,
                    stroke,
                );
                let pts = bez.flatten(Some(2.0));
                if is_dep {
                    painter.add(bez);
                } else {
                    // Dashed beziers aren't a primitive — flatten to a polyline.
                    painter.add(egui::Shape::dashed_line(&pts, stroke, dash, dash));
                }
                pts
            };
            if is_dep {
                if let Some((prev, tip)) = boundary_tip(&pts, &target_rect) {
                    arrowhead(&painter, prev, tip, 7.0 * scale.clamp(0.5, 1.6), stroke);
                }
            }
            edge_polys.push(pts);
        }
    }

    // ── Nodes ─────────────────────────────────────────────────────────────
    let name_font = egui::FontId::proportional((11.5 * scale).clamp(6.0, 24.0));
    let sub_font = egui::FontId::proportional((8.5 * scale).clamp(5.0, 18.0));
    let row_font = egui::FontId::monospace((8.0 * scale).clamp(5.0, 16.0));
    // Symbol rows are unreadable below this scale — draw compact nodes instead.
    let show_detail = scale > 0.45;
    // One fill per PACKAGE (top-level module subtree): same colour for a
    // package and all its children, different across packages. main keeps its
    // olive root fill.
    let fills: Vec<egui::Color32> = {
        let mut by_pkg: std::collections::HashMap<&str, egui::Color32> =
            std::collections::HashMap::new();
        let mut next = 0usize;
        graph
            .nodes
            .iter()
            .map(|n| {
                if n.path.is_empty() {
                    ROOT_FILL
                } else {
                    let key = n.path.split("::").next().unwrap_or("");
                    *by_pkg.entry(key).or_insert_with(|| {
                        let c = PKG_PALETTE[next % PKG_PALETTE.len()];
                        next += 1;
                        c
                    })
                }
            })
            .collect()
    };
    for (i, node) in graph.nodes.iter().enumerate() {
        let p = lay.pos[i];
        let r = egui::Rect::from_min_size(to_screen(p.x, p.y), egui::vec2(p.w, p.h) * scale);
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
        let pre_rows = if show_detail {
            shown_rows(node.symbols.len())
        } else {
            0
        };
        let handle_h = if pre_rows > 0 {
            HEADER_H * scale
        } else {
            r.height()
        };
        let drag_resp = ui
            .interact(
                egui::Rect::from_min_size(r.min, egui::vec2(r.width(), handle_h)).intersect(rect),
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
        let r = egui::Rect::from_min_size(to_screen(p.x, p.y), egui::vec2(p.w, p.h) * scale);

        // Package roots (a `mod.rs` file) stand out: pill corners, a border
        // twice as thick as regular nodes, and a bold name.
        let is_pkg_root = node.file_rel.ends_with("/mod.rs");
        let fill = fills[i];
        // Border style, by priority:
        //   1. ERROR — the file has diagnostics: BLINKING red at 3× width
        //      (driven by wall time; repaint scheduled below);
        //   2. SELECTED (the focus node) — the hover style, held while the
        //      node stays selected, so it's obvious whose edges are shown;
        //   3. hover; 4. normal.
        let has_error = node_errors.get(i).copied().unwrap_or(false);
        let blink_on = has_error && (ui.ctx().input(|inp| inp.time) * 2.0) as i64 % 2 == 0;
        if has_error {
            // Keep frames coming so the blink actually blinks.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        let selected = i == focus_node;
        let (stroke_c, base_w) = if blink_on {
            (ERROR_STROKE, 3.0)
        } else if selected || resp.hovered() {
            (HOVER_STROKE, 1.6)
        } else {
            (NODE_STROKE, 1.0)
        };
        painter.rect(
            r,
            if is_pkg_root {
                20.0 * scale.clamp(0.5, 1.5)
            } else {
                4.0 * scale.clamp(0.5, 1.5)
            },
            fill,
            egui::Stroke::new(if is_pkg_root { base_w * 2.0 } else { base_w }, stroke_c),
            egui::StrokeKind::Inside,
        );

        // ── Header band: name + fn/ty badge ───────────────────────────────
        // Rows below need the detail scale; a row-less (or zoomed-out) node
        // centers the header content in the whole box instead.
        let rows = if show_detail {
            shown_rows(node.symbols.len())
        } else {
            0
        };
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
        let name_pos = egui::pos2(header.center().x, name_y);
        painter.text(
            name_pos,
            egui::Align2::CENTER_CENTER,
            &node.name,
            name_font.clone(),
            egui::Color32::WHITE,
        );
        if is_pkg_root {
            // Poor-man's bold: egui ships no bold face, so overdraw the name
            // with a sub-pixel offset to thicken the glyphs.
            painter.text(
                name_pos + egui::vec2((0.6 * scale).clamp(0.3, 0.9), 0.0),
                egui::Align2::CENTER_CENTER,
                &node.name,
                name_font.clone(),
                egui::Color32::WHITE,
            );
        }
        if show_detail {
            painter.text(
                egui::pos2(
                    header.center().x,
                    header.center().y + header.height() * 0.24,
                ),
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
                    let mut s: String = sym.name.chars().take(ROW_NAME_CHARS - 1).collect();
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
                // Total reference sites (right-aligned, kind-coloured) — many
                // sites aggregate into one drawn edge, so the count keeps the
                // full picture (matches the editor's "N refs" pill).
                let refs = ref_counts.get(&(i, j)).copied();
                if let Some(cnt) = refs {
                    painter.text(
                        egui::pos2(row_rect.right() - 5.0 * scale, row_rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        cnt.to_string(),
                        row_font.clone(),
                        g_color,
                    );
                }
                row_resp.clone().on_hover_text(format!(
                    "{} {} — line {}{}\nClick to jump to it",
                    kind_word(sym.kind),
                    sym.name,
                    sym.line,
                    refs.map(|c| format!(" — {c} reference site(s)"))
                        .unwrap_or_default()
                ));
                if row_resp.clicked() {
                    result.click = Some(NodeClick {
                        file: node.file,
                        line: Some(sym.line),
                    });
                    row_clicked = true;
                }
            }
        }

        let full = if node.path.is_empty() {
            "crate root"
        } else {
            &node.path
        };
        resp.clone().on_hover_text(format!(
            "{full}\n{}\n{} fn · {} struct/enum/trait\nClick to open in the editor",
            node.file_rel, node.fn_count, node.ty_count
        ));
        // The header's drag-sense response wins the pointer there, so a plain
        // click on the header surfaces as `drag_resp.clicked()`.
        if (resp.clicked() || drag_resp.clicked()) && !row_clicked {
            result.click = Some(NodeClick {
                file: node.file,
                line: None,
            });
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
        // Downstream reach from the FOCUS SET (a single module, or a whole
        // package when its mod.rs is selected): multi-source BFS over the
        // node-level call graph. An edge is drawn when its SOURCE lies within
        // `call_depth` hops of the set (level 1 = the members' own edges —
        // which for a package means ALL its interior links plus the first
        // exterior level), plus any edge ENTERING the set (who uses it).
        // "All" = unbounded — the whole tree under the selection.
        let depth_limit = view.call_depth.unwrap_or(usize::MAX);
        let in_set = graph.focus_set(focus_node);
        let dist: Vec<usize> = {
            let n = graph.nodes.len();
            let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
            for e in calls {
                adj[e.from_node].push(e.to_node);
            }
            let mut dist = vec![usize::MAX; n];
            let mut q = std::collections::VecDeque::new();
            for (i, &member) in in_set.iter().enumerate() {
                if member {
                    dist[i] = 0;
                    q.push_back(i);
                }
            }
            while let Some(u) = q.pop_front() {
                if dist[u] >= depth_limit {
                    continue; // deep enough — don't expand further
                }
                for &v in &adj[u] {
                    if dist[v] == usize::MAX {
                        dist[v] = dist[u] + 1;
                        q.push_back(v);
                    }
                }
            }
            dist
        };
        // ── Candidate ROUTER (user fix: "choose the shortest path without
        //    crossing other paths") ─────────────────────────────────────────
        // For every visible edge THREE routes are scored — the direct
        // facing-sides bezier, and the left / right outer-flank arcs — by
        // length plus penalties for crossing anything already drawn (module
        // edges + previously routed calls) or cutting through a node box; the
        // cheapest wins. Short edges route first so they claim the direct
        // lanes and longer ones bend around them.
        let mut visible: Vec<&CallEdge> = calls
            .iter()
            .filter(|e| dist[e.from_node] < depth_limit || in_set[e.to_node])
            .collect();
        let center_dist2 = |e: &CallEdge| -> f32 {
            let (a, b) = (lay.pos[e.from_node], lay.pos[e.to_node]);
            let dx = a.center_x() - b.center_x();
            let dy = (a.y + a.h / 2.0) - (b.y + b.h / 2.0);
            dx * dx + dy * dy
        };
        visible.sort_by(|x, y| {
            center_dist2(x)
                .partial_cmp(&center_dist2(y))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let node_rects: Vec<egui::Rect> = lay
            .pos
            .iter()
            .map(|p| egui::Rect::from_min_size(to_screen(p.x, p.y), egui::vec2(p.w, p.h) * scale))
            .collect();
        use crate::panels::structure_map::layout::{seg_hits_rect, segments_cross};
        // Everything in `edge_polys` up to this point is MODULE wiring (gray
        // deps/containment) — background lines, cheap to cross. Entries pushed
        // later are routed CALL edges — crossing those costs real money.
        let module_polys_len = edge_polys.len();
        // Candidate cost: length in px, +60 per crossed module line (they're
        // background — a short direct route beats dodging them to the far
        // side, the reported bug), +400 per crossed CALL edge, +900 per
        // non-endpoint node box it cuts through.
        let poly_cost =
            |pts: &[egui::Pos2], skip_a: usize, skip_b: usize, polys: &[Vec<egui::Pos2>]| -> f32 {
                let mut cost = 0.0;
                for w in pts.windows(2) {
                    cost += w[0].distance(w[1]);
                }
                for (oi, other) in polys.iter().enumerate() {
                    let penalty = if oi < module_polys_len { 60.0 } else { 400.0 };
                    'pair: for w1 in pts.windows(2) {
                        for w2 in other.windows(2) {
                            if segments_cross(
                                (w1[0].x, w1[0].y),
                                (w1[1].x, w1[1].y),
                                (w2[0].x, w2[0].y),
                                (w2[1].x, w2[1].y),
                            ) {
                                cost += penalty;
                                break 'pair;
                            }
                        }
                    }
                }
                for (i, r) in node_rects.iter().enumerate() {
                    if i == skip_a || i == skip_b {
                        continue;
                    }
                    for w in pts.windows(2) {
                        if seg_hits_rect(
                            (w[0].x, w[0].y),
                            (w[1].x, w[1].y),
                            r.left(),
                            r.top(),
                            r.width(),
                            r.height(),
                        ) {
                            cost += 900.0;
                            break;
                        }
                    }
                }
                cost
            };
        for e in visible {
            let (a, b) = (lay.pos[e.from_node], lay.pos[e.to_node]);
            // Edge colour = the TARGET symbol's kind colour (the same palette
            // as the row glyphs): fn blue, Struct orange, Enum purple, Trait
            // green — the arrow tells at a glance WHAT is being used.
            let kind = graph.nodes[e.to_node]
                .symbols
                .get(e.to_row)
                .map(|s| s.kind)
                .unwrap_or(SymKind::Fn);
            let (_, kc) = kind_glyph(kind);
            let stroke = egui::Stroke::new(
                (1.1 * scale).clamp(0.5, 2.0),
                egui::Color32::from_rgba_unmultiplied(kc.r(), kc.g(), kc.b(), 200),
            );
            let toward_right = b.center_x() >= a.center_x();
            // Stagger keeps parallel calls between the same nodes off one arc.
            let swing = 26.0 + ((e.from_row + e.to_row) % 4) as f32 * 8.0;
            // SOURCE anchor (user fix): a call edge never latches onto the
            // CALLER's symbol-row zone (the f/S/E/T list) — it leaves through
            // the node's top/bottom edge beside the header's left/right
            // corner. Only the TARGET end still points at the used symbol's
            // row. Exit side follows the target's vertical direction; a small
            // per-row stagger keeps several departures off one corner point.
            let via_top = (b.y + b.h / 2.0) < (a.y + a.h / 2.0);
            let src = |right: bool| -> egui::Pos2 {
                let inset = (10.0 + (e.from_row % 4) as f32 * 7.0).min(a.w * 0.45);
                let x = if right { a.x + a.w - inset } else { a.x + inset };
                let y = if via_top { a.y } else { a.y + a.h };
                to_screen(x, y)
            };
            // Candidate 1: FACING sides — the short direct route (vertical
            // takeoff from the corner, horizontal landing at the row).
            let f_from = src(toward_right);
            let f_to = anchor(e.to_node, e.to_row, !toward_right);
            let dyf = ((f_to.y - f_from.y).abs() * 0.4).clamp(16.0 * scale, 100.0 * scale)
                * if via_top { -1.0 } else { 1.0 };
            let dxf = ((f_to.x - f_from.x).abs() * 0.4).clamp(20.0 * scale, 120.0 * scale)
                * if toward_right { 1.0 } else { -1.0 };
            let cand_facing = [
                f_from,
                f_from + egui::vec2(0.0, dyf),
                f_to - egui::vec2(dxf, 0.0),
                f_to,
            ];
            // Candidates 2 + 3: LEFT / RIGHT outer-flank arcs past the boxes.
            let mk_flank = |right: bool| {
                let from = src(right);
                let to = anchor(e.to_node, e.to_row, right);
                let outer_x = if right {
                    (a.x + a.w).max(b.x + b.w) + swing
                } else {
                    a.x.min(b.x) - swing
                };
                let cx = to_screen(outer_x, 0.0).x;
                [
                    from,
                    egui::pos2(cx, from.y + 0.25 * (to.y - from.y)),
                    egui::pos2(cx, from.y + 0.75 * (to.y - from.y)),
                    to,
                ]
            };
            // Candidate shapes: 3 beziers + an ORTHOGONAL 90°-bend route
            // (vertical drop from the corner, right-angle turn, horizontal run
            // into the row) — added only when the source corner sits clearly
            // beside the target's x-range, so the horizontal leg is real.
            enum Route {
                Bez([egui::Pos2; 4]),
                Poly(Vec<egui::Pos2>),
            }
            // SIDE exit (user fix): the source may also leave through the
            // node's LEFT/RIGHT edge — but only at HEADER height, never beside
            // the symbol-row zone. Horizontal takeoff, straight into the
            // target row: the shortest shape when the nodes sit side by side.
            let src_side = |right: bool| -> egui::Pos2 {
                let y = a.y + (HEADER_H * 0.5).min(a.h * 0.5);
                let x = if right { a.x + a.w } else { a.x };
                to_screen(x, y)
            };
            let s_from = src_side(toward_right);
            let dxs = ((f_to.x - s_from.x).abs() * 0.4).clamp(20.0 * scale, 120.0 * scale)
                * if toward_right { 1.0 } else { -1.0 };
            let cand_side = [
                s_from,
                s_from + egui::vec2(dxs, 0.0),
                f_to - egui::vec2(dxs, 0.0),
                f_to,
            ];
            let mut cands = vec![
                Route::Bez(cand_facing),
                Route::Bez(cand_side),
                Route::Bez(mk_flank(false)),
                Route::Bez(mk_flank(true)),
            ];
            {
                let (bl, br) = (to_screen(b.x, 0.0).x, to_screen(b.x + b.w, 0.0).x);
                if f_from.x < bl - 8.0 || f_from.x > br + 8.0 {
                    cands.push(Route::Poly(vec![
                        f_from,
                        egui::pos2(f_from.x, f_to.y),
                        f_to,
                    ]));
                }
            }
            let mut best: Option<(Route, Vec<egui::Pos2>, f32)> = None;
            for cand in cands {
                let pts = match &cand {
                    Route::Bez(b4) => egui::epaint::CubicBezierShape::from_points_stroke(
                        *b4,
                        false,
                        egui::Color32::TRANSPARENT,
                        stroke,
                    )
                    .flatten(Some(4.0)),
                    Route::Poly(p) => p.clone(),
                };
                let cost = poly_cost(&pts, e.from_node, e.to_node, &edge_polys);
                if best.as_ref().is_none_or(|(_, _, c)| cost < *c) {
                    best = Some((cand, pts, cost));
                }
            }
            let (cand, pts, _) = best.unwrap();
            match &cand {
                Route::Bez(b4) => {
                    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                        *b4,
                        false,
                        egui::Color32::TRANSPARENT,
                        stroke,
                    ));
                }
                Route::Poly(p) => {
                    painter.add(egui::Shape::line(p.clone(), stroke));
                }
            }
            // Arrowhead along the final segment, whatever the shape.
            if pts.len() >= 2 {
                arrowhead(
                    &painter,
                    pts[pts.len() - 2],
                    pts[pts.len() - 1],
                    6.0 * scale.clamp(0.5, 1.5),
                    stroke,
                );
            }
            edge_polys.push(pts);
        }
    }
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

/// Where a polyline (running INTO `target`'s interior) crosses the target's
/// boundary: returns `(point just outside, boundary tip)` for the arrowhead —
/// walked from the END of `pts` (which sits at the node's hidden center) back
/// to the first point outside, then bisected onto the border. `None` when the
/// whole polyline is inside (overlapping nodes) — no arrow then.
fn boundary_tip(pts: &[egui::Pos2], target: &egui::Rect) -> Option<(egui::Pos2, egui::Pos2)> {
    for i in (0..pts.len().saturating_sub(1)).rev() {
        if !target.contains(pts[i]) {
            let (mut lo, mut hi) = (pts[i], pts[i + 1]);
            for _ in 0..8 {
                let mid = lo + (hi - lo) * 0.5;
                if target.contains(mid) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            return Some((pts[i], lo));
        }
    }
    None
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
