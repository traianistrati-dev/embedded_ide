//! Data-driven clock diagram renderer — CubeMX-style vector schematic.
//!
//! Everything is driven by a [`ClockLayout`] (blocks, outputs, tags, wires)
//! plus [`Widget`]s editing [`ClockGraph`] node states:
//! - Orthogonal wiring routed in lanes so paths never cross blocks.
//! - Mux selectors drawn as **trapezoids with radio buttons** so the active
//!   input is obvious — like CubeMX.
//! - Frequency tags/boxes turn red past their datasheet limit.
//!
//! The figure lives in a fixed virtual space and scales to the panel width.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, UiBuilder, Vec2};

use super::super::graph::layout::{ClockLayout, ValueSrc, Widget};
use super::super::graph::model::{ClockGraph, NodeState};
use super::super::graph::validate::ceiling_for;
use super::super::model::ClockLimits;

// ── Virtual canvas ────────────────────────────────────────────────────────────
const VW: f32 = 1000.0;
const VH: f32 = 790.0;

// ── Palette ───────────────────────────────────────────────────────────────────
const BG: Color32 = Color32::from_rgb(26, 28, 34);
const BOX_FILL: Color32 = Color32::from_rgb(42, 46, 56);
const OUT_FILL: Color32 = Color32::from_rgb(34, 40, 50);
const STROKE_C: Color32 = Color32::from_rgb(140, 150, 165);
const WIRE_C: Color32 = Color32::from_rgb(110, 122, 140);
const LABEL_C: Color32 = Color32::from_rgb(208, 215, 228);
const DIM_C: Color32 = Color32::from_rgb(150, 158, 172);
const MUX_FILL: Color32 = Color32::from_rgb(50, 56, 70);
const MUX_STROKE: Color32 = Color32::from_rgb(95, 140, 215);
const FREQ_OK: Color32 = Color32::from_rgb(120, 205, 140);
const FREQ_BAD: Color32 = Color32::from_rgb(235, 95, 85);

#[derive(Clone, Copy)]
pub(crate) struct Tf {
    origin: Pos2,
    scale: f32,
}
impl Tf {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.origin + Vec2::new(x, y) * self.scale
    }
    fn r(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.p(x, y), Vec2::new(w, h) * self.scale)
    }
    fn fs(&self, pt: f32) -> f32 {
        (pt * self.scale).max(6.0)
    }
}

/// Shared static-diagram renderer. Scales `lay` to the viewport, paints the
/// background + wires + blocks/outputs/tags/labels/mux-titles, and returns the
/// canvas `Rect` + transform. `resolve` supplies each box/tag's frequency —
/// `value_of` for the F103 typed path, `value_from_graph` for imported graph
/// clocks. No interactivity (callers add their own).
pub(crate) fn draw_static_diagram<R: Fn(&ValueSrc) -> u32>(
    ui: &mut egui::Ui,
    lay: &ClockLayout,
    l: &ClockLimits,
    avail_w: f32,
    avail_h: f32,
    zoom: f32,
    resolve: R,
) -> (Rect, Tf) {
    // Fit the whole diagram into the viewport (both dimensions), then apply zoom.
    let fit = (avail_w / VW).min(avail_h / VH);
    let scale = (fit * zoom).max(0.12);
    let size = Vec2::new(VW * scale, VH * scale);

    let (rect, _resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    let tf = Tf { origin: rect.min, scale };

    let clip = rect.intersect(ui.clip_rect());
    let painter = ui.painter().with_clip_rect(clip);
    painter.rect_filled(rect, 4.0, BG);

    draw_wires(&painter, &tf, lay);
    draw_static(&painter, &tf, lay, l, &resolve);
    (rect, tf)
}

// ──────────────────────────────────────────────────────────────────────────────
// Wires (drawn first, under the blocks)
// ──────────────────────────────────────────────────────────────────────────────

fn draw_wires(p: &egui::Painter, tf: &Tf, lay: &ClockLayout) {
    // CubeMX style: muxes show their sources as labelled stubs (drawn inside
    // `mux_radios`), so the routed wires are short, local links only. The
    // polyline geometry is data ([`ClockLayout::wires`]).
    for poly in &lay.wires {
        wire(p, tf, poly);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Static blocks + labels + frequency tags — all driven by `ClockLayout` data
// ──────────────────────────────────────────────────────────────────────────────

fn draw_static<R: Fn(&ValueSrc) -> u32>(
    p: &egui::Painter,
    tf: &Tf,
    lay: &ClockLayout,
    l: &ClockLimits,
    resolve: &R,
) {
    for b in &lay.blocks {
        block(p, tf, b.x, b.y, b.w, b.h, &b.label);
    }

    for o in &lay.outputs {
        let hz = resolve(&o.src);
        let limit = o.limit.and_then(|k| ceiling_for(k, l));
        out_box(p, tf, o.x, o.y, o.w, o.h, &o.label, hz, limit);
    }

    for t in &lay.tags {
        let hz = resolve(&t.src);
        let bad = t.limit.and_then(|k| ceiling_for(k, l)).map_or(false, |lim| over(hz, lim));
        tag(p, tf, t.x, t.y, &mhz(hz), bad, &t.name);
    }

    for lb in &lay.labels_above {
        label_above(p, tf, lb.x, lb.y, &lb.text);
    }

    for mt in &lay.mux_titles {
        p.text(
            tf.p(mt.x, mt.y),
            Align2::CENTER_BOTTOM,
            &mt.text,
            FontId::proportional(tf.fs(9.0)),
            DIM_C,
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Generic interactive overlay for graph clocks
// ──────────────────────────────────────────────────────────────────────────────

/// Draw each [`Widget`] dropdown from the layout and write the picked
/// [`NodeState`] back into `graph`. Returns `true` when a state changed.
pub(crate) fn interactive_graph(
    ui: &mut egui::Ui,
    tf: &Tf,
    graph: &mut ClockGraph,
    widgets: &[Widget],
) -> bool {
    let mut pending: Option<(String, NodeState)> = None;
    for w in widgets {
        match w {
            Widget::Combo { node, x, y, w: cw, options } => {
                let Some(cur) = graph.node(node).map(|n| n.state.clone()) else {
                    continue;
                };
                if let Some(state) = graph_combo(ui, tf, *x, *y, *cw, node, &cur, options) {
                    pending = Some((node.clone(), state));
                }
            }
            // Trapezoid mux — reuses the proven `mux_radios` primitive 1:1.
            Widget::MuxRadios { node, x, y, w: mw, h, flip, inputs } => {
                let Some(cur) = graph.node(node).map(|n| n.state.clone()) else {
                    continue;
                };
                let selected = inputs.iter().position(|(_, _, st)| st == &cur);
                let labels: Vec<(&str, f32)> =
                    inputs.iter().map(|(l, dy, _)| (l.as_str(), *dy)).collect();
                let mut picked: Option<usize> = None;
                mux_radios(ui, tf, *x, *y, *mw, *h, &labels, selected, *flip, |i| {
                    picked = Some(i);
                });
                if let Some((_, _, st)) = picked.and_then(|i| inputs.get(i)) {
                    pending = Some((node.clone(), st.clone()));
                }
            }
            Widget::DragMhz { node, x, y, w: dw, min_mhz, max_mhz } => {
                let Some(NodeState::Source { enabled, hz }) =
                    graph.node(node).map(|n| n.state.clone())
                else {
                    continue;
                };
                let rect = tf.r(*x, *y, *dw, 22.0);
                let mut mhz = hz as f64 / 1e6;
                let mut dragged = false;
                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    if ui
                        .add(
                            egui::DragValue::new(&mut mhz)
                                .range(*min_mhz as f64..=*max_mhz as f64)
                                .speed(0.1)
                                .suffix(" MHz"),
                        )
                        .changed()
                    {
                        dragged = true;
                    }
                });
                if dragged {
                    pending = Some((
                        node.clone(),
                        NodeState::Source { enabled, hz: (mhz * 1e6).round() as u32 },
                    ));
                }
            }
        }
    }
    if let Some((id, state)) = pending {
        if let Some(n) = graph.node_mut(&id) {
            if n.state != state {
                n.state = state;
                return true;
            }
        }
    }
    false
}

/// A dropdown over `(label, state)` options bound to a graph node.
fn graph_combo(
    ui: &mut egui::Ui,
    tf: &Tf,
    x: f32,
    y: f32,
    w: f32,
    id: &str,
    current: &NodeState,
    options: &[(String, NodeState)],
) -> Option<NodeState> {
    let rect = tf.r(x, y, w, 26.0);
    let cur_label = options
        .iter()
        .find(|(_, s)| s == current)
        .map(|(l, _)| l.as_str())
        .unwrap_or("?");
    let mut picked = None;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt(("graph_combo", id))
            .selected_text(egui::RichText::new(cur_label).size(tf.fs(9.0)))
            .width(rect.width())
            .show_ui(ui, |ui| {
                for (label, state) in options {
                    if ui.selectable_label(state == current, label).clicked() {
                        picked = Some(state.clone());
                    }
                }
            });
    });
    picked
}

/// Draw a vertical trapezoid mux with one radio button per input. Returns
/// `true` and runs `on_pick` if the user selects a new input.
///
/// `flip = false`: inputs on the left, output on the right (the usual case).
/// `flip = true`:  mirrored — inputs on the right, output on the left (MCO).
fn mux_radios(
    ui: &mut egui::Ui,
    tf: &Tf,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    inputs: &[(&str, f32)], // (label, dy from y where the input enters)
    selected: Option<usize>,
    flip: bool,
    mut on_pick: impl FnMut(usize),
) -> bool {
    let painter = ui.painter().clone();
    // Small, bounded taper → nearly rectangular mux so the radio buttons always
    // sit comfortably INSIDE the trapezoid (CubeMX style).
    let taper = (h * 0.16).clamp(8.0, 12.0);
    let poly = if flip {
        // Tall on the right (inputs), short on the left (output).
        vec![
            tf.p(x + w, y),
            tf.p(x + w, y + h),
            tf.p(x, y + h - taper),
            tf.p(x, y + taper),
        ]
    } else {
        // Tall on the left (inputs), short on the right (output).
        vec![
            tf.p(x, y),
            tf.p(x, y + h),
            tf.p(x + w, y + h - taper),
            tf.p(x + w, y + taper),
        ]
    };
    painter.add(Shape::convex_polygon(poly, MUX_FILL, Stroke::new(1.2, MUX_STROKE)));

    let mut changed = false;
    for (i, (label, dy)) in inputs.iter().enumerate() {
        let cy = y + dy;
        let stroke = Stroke::new(1.2, WIRE_C);
        // Radio sits just inside the wide edge (left for normal, right for flip);
        // the input stub stops at the mux edge so the wire meets the radio.
        let (stub_a, stub_b, lbl_x, lbl_align, radio_cx) = if flip {
            (x + w + 24.0, x + w, x + w + 26.0, Align2::LEFT_CENTER, x + w - 11.0)
        } else {
            (x - 24.0, x, x - 26.0, Align2::RIGHT_CENTER, x + 11.0)
        };
        painter.line_segment([tf.p(stub_a, cy), tf.p(stub_b, cy)], stroke);
        painter.text(tf.p(lbl_x, cy), lbl_align, *label, FontId::proportional(tf.fs(8.0)), DIM_C);

        let center = tf.p(radio_cx, cy);
        let rsz = 12.0 * tf.scale;
        let rrect = Rect::from_center_size(center, Vec2::splat(rsz));
        let is_sel = selected == Some(i);
        if ui.put(rrect, egui::RadioButton::new(is_sel, "")).clicked() {
            on_pick(i);
            changed = true;
        }
    }
    changed
}

// ──────────────────────────────────────────────────────────────────────────────
// Primitive painters
// ──────────────────────────────────────────────────────────────────────────────

fn block(p: &egui::Painter, tf: &Tf, x: f32, y: f32, w: f32, h: f32, title: &str) {
    let r = tf.r(x, y, w, h);
    p.rect(r, 3.0, BOX_FILL, Stroke::new(1.2, STROKE_C), egui::StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, title, FontId::proportional(tf.fs(9.0)), LABEL_C);
}

fn out_box(p: &egui::Painter, tf: &Tf, x: f32, y: f32, w: f32, h: f32, label: &str, hz: u32, limit: Option<u32>) {
    let r = tf.r(x, y, w, h);
    p.rect(r, 3.0, OUT_FILL, Stroke::new(1.0, STROKE_C), egui::StrokeKind::Inside);
    let bad = limit.map(|l| hz > l).unwrap_or(false);
    let col = if bad { FREQ_BAD } else { FREQ_OK };
    // value (left) + label (right)
    p.text(
        tf.p(x + 6.0, y + h / 2.0),
        Align2::LEFT_CENTER,
        mhz(hz),
        FontId::monospace(tf.fs(9.5)),
        col,
    );
    p.text(
        tf.p(x + w - 6.0, y + h / 2.0),
        Align2::RIGHT_CENTER,
        label,
        FontId::proportional(tf.fs(8.5)),
        DIM_C,
    );
}

fn label_above(p: &egui::Painter, tf: &Tf, x: f32, y: f32, text: &str) {
    p.text(tf.p(x, y), Align2::LEFT_BOTTOM, text, FontId::proportional(tf.fs(8.5)), DIM_C);
}

fn wire(p: &egui::Painter, tf: &Tf, pts: &[(f32, f32)]) {
    let stroke = Stroke::new(1.3, WIRE_C);
    for w in pts.windows(2) {
        p.line_segment([tf.p(w[0].0, w[0].1), tf.p(w[1].0, w[1].1)], stroke);
    }
    if pts.len() >= 2 {
        let a = pts[pts.len() - 2];
        let b = pts[pts.len() - 1];
        arrowhead(p, tf, a, b);
    }
}

fn arrowhead(p: &egui::Painter, tf: &Tf, from: (f32, f32), to: (f32, f32)) {
    let a = tf.p(from.0, from.1);
    let b = tf.p(to.0, to.1);
    let dir = (b - a).normalized();
    if !dir.is_finite() {
        return;
    }
    let n = Vec2::new(-dir.y, dir.x);
    let s = 5.0 * tf.scale.max(0.5);
    let p1 = b - dir * s + n * (s * 0.5);
    let p2 = b - dir * s - n * (s * 0.5);
    p.add(Shape::convex_polygon(vec![b, p1, p2], WIRE_C, Stroke::NONE));
}

fn tag(p: &egui::Painter, tf: &Tf, x: f32, y: f32, value: &str, bad: bool, name: &str) {
    let color = if bad { FREQ_BAD } else { FREQ_OK };
    p.text(
        tf.p(x, y),
        Align2::LEFT_CENTER,
        format!("{name} {value}"),
        FontId::monospace(tf.fs(9.0)),
        color,
    );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn mhz(hz: u32) -> String {
    if hz == 0 {
        return "—".to_string();
    }
    if hz < 1_000_000 {
        let khz = hz as f64 / 1000.0;
        if (khz.fract()).abs() < 1e-6 {
            return format!("{} kHz", khz as u32);
        }
        return format!("{khz:.3} kHz");
    }
    let m = hz as f64 / 1e6;
    if (m.fract()).abs() < 1e-6 {
        format!("{} MHz", m as u32)
    } else {
        format!("{m:.2} MHz")
    }
}
fn over(hz: u32, limit: u32) -> bool {
    hz > limit
}
