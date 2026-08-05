//! In/out arrow graphics for GPIO Input / Output / PWM pins on the Pins canvas.
//!
//! A lightweight cousin of the virtual-module schematic ([`super::modules`]):
//! every pin configured as **GPIO Output** or **PWM** gets an arrow pointing
//! away from the chip (a driven output), every **GPIO Input** an arrow pointing
//! into the chip. Each arrow carries a small text field to rename the pin; the
//! typed text is appended to the generated binding name, e.g. a `pc13` output
//! labelled "led" becomes `let pc13_out_led = …`. No add/remove buttons — the
//! arrows simply mirror the pin functions.

use super::super::model::{Mcu, PIN_HEIGHT, PIN_SPACING, PIN_WIDTH};
use super::modules::{nearest_edge, pin_anchor_dir};
use super::rotate::Rot;

/// Line from `from` to `to` with a FIXED-size arrowhead at `to`. egui's
/// `Painter::arrow` scales the head with the vector length, so a long connector
/// (to a far / dragged field) grows a huge head — this keeps it constant.
fn connector(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, color: egui::Color32) {
    let stroke = egui::Stroke::new(2.0, color);
    painter.line_segment([from, to], stroke);
    let v = to - from;
    if v.length() < 1.0 {
        return;
    }
    let dir = v.normalized();
    let head = 9.0;
    let rot = egui::emath::Rot2::from_angle(std::f32::consts::TAU / 12.0);
    painter.line_segment([to, to - head * (rot * dir)], stroke);
    painter.line_segment([to, to - head * (rot.inverse() * dir)], stroke);
}
use crate::panels::mcu_module::codegen::{pin_binding, sanitize_label};
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
use eframe::egui;

const ARROW_LEN: f32 = 24.0;
const GAP: f32 = 5.0;
const FIELD_W: f32 = 88.0;
const FIELD_H: f32 = 18.0;

/// Canvas margin (beyond the pin tips) needed to fit an arrow + rename field.
pub const MARGIN_X: f32 = PIN_HEIGHT + ARROW_LEN + GAP + FIELD_W + 14.0;
pub const MARGIN_Y: f32 = PIN_HEIGHT + ARROW_LEN + GAP + FIELD_H + 14.0;

/// `Some(true)` for an outbound pin (GPIO Output / PWM), `Some(false)` for an
/// inbound pin (GPIO Input), `None` for anything else.
fn io_outbound(func: &PinFunction) -> Option<bool> {
    match func {
        PinFunction::GpioOutput | PinFunction::TimerPwm { .. } => Some(true),
        PinFunction::GpioInput => Some(false),
        _ => None,
    }
}

/// Whether any non-reserved pin is GPIO In/Out/PWM (so the canvas should reserve
/// a margin for arrows).
pub fn has_io_pins(mcu: &Mcu) -> bool {
    mcu.iter_all_pins()
        .any(|p| !p.reserved && io_outbound(&p.selected_function).is_some())
}

/// Draw an in/out arrow + rename field for every GPIO In/Out/PWM pin. The text
/// field edits the pin's `custom_label` in place (regenerated into the binding
/// name every frame by `update_main_rs`).
pub fn draw_io_arrows(
    mcu: &mut Mcu,
    painter: &egui::Painter,
    local_chip: egui::Rect,
    rot: Rot,
    ui: &mut egui::Ui,
) {
    // Snapshot geometry first (immutable borrow), then edit labels (mutable).
    struct Item {
        num: usize,
        anchor: egui::Pos2,
        dir: egui::Vec2,
        outbound: bool,
        color: egui::Color32,
        preview_base: String,
    }
    let mut items: Vec<Item> = Vec::new();
    for p in mcu.iter_all_pins() {
        if p.reserved {
            continue;
        }
        let Some(outbound) = io_outbound(&p.selected_function) else {
            continue;
        };
        let Some((anchor, dir)) = pin_anchor_dir(mcu, local_chip, rot, p.number) else {
            continue;
        };
        items.push(Item {
            num: p.number,
            anchor,
            dir,
            outbound,
            color: p.selected_function.color(),
            // Base binding the label is appended to, e.g. "pc13_out" / "gpio2_out".
            preview_base: pin_binding(&p.name.to_ascii_lowercase(), &p.selected_function, ""),
        });
    }

    let chip_center = local_chip.center();
    // In the diamond the pins point DIAGONALLY, so auto-placing each field along
    // its pin pushes it far up the diagonal AND lets neighbours overlap. Group
    // the auto (non-dragged) io pins into CONTIGUOUS runs along each edge: a run
    // of ≥2 adjacent pins becomes a vertical column beside the chip (fields at a
    // fixed x → their labels line up), while a lone io pin keeps the simple
    // beside-the-pin placement.
    let pitch = PIN_WIDTH + PIN_SPACING;
    let row = FIELD_H + 16.0 + 8.0; // field + preview strip + gap
    let mut packed: std::collections::HashMap<usize, egui::Pos2> = std::collections::HashMap::new();
    {
        // Bucket diagonal, non-dragged io pins by edge (sign of the outward dir).
        let mut by_side: std::collections::BTreeMap<(i32, i32), Vec<&Item>> =
            std::collections::BTreeMap::new();
        for it in &items {
            if mcu.io_pin_pos.contains_key(&it.num) {
                continue;
            }
            if it.dir.x.abs() > 0.3 && it.dir.y.abs() > 0.3 {
                let key = (it.dir.x.signum() as i32, it.dir.y.signum() as i32);
                by_side.entry(key).or_default().push(it);
            }
        }
        for ((sx, _), mut group) in by_side {
            // Sort along the edge (tangent ⟂ outward), then split into runs where
            // a gap > 1.5·pitch means a non-io pin sits between two io pins.
            let tangent = egui::vec2(-group[0].dir.y, group[0].dir.x);
            let proj = |it: &&Item| it.anchor.to_vec2().dot(tangent);
            group.sort_by(|a, b| proj(a).total_cmp(&proj(b)));
            let mut runs: Vec<Vec<&Item>> = Vec::new();
            let mut cur: Vec<&Item> = Vec::new();
            let mut prev = f32::NAN;
            for it in group {
                let t = proj(&it);
                if prev.is_finite() && (t - prev).abs() > 1.5 * pitch {
                    runs.push(std::mem::take(&mut cur));
                }
                cur.push(it);
                prev = t;
            }
            if !cur.is_empty() {
                runs.push(cur);
            }
            for mut run in runs {
                if run.len() < 2 {
                    continue; // a lone io pin stays with the simple placement
                }
                // Column just outside the run (outward side = sign of dir.x).
                let col_x = if sx >= 0 {
                    run.iter().map(|it| it.anchor.x).fold(f32::MIN, f32::max)
                        + ARROW_LEN
                        + GAP
                        + FIELD_W / 2.0
                } else {
                    run.iter().map(|it| it.anchor.x).fold(f32::MAX, f32::min)
                        - ARROW_LEN
                        - GAP
                        - FIELD_W / 2.0
                };
                run.sort_by(|a, b| a.anchor.y.total_cmp(&b.anchor.y));
                let base_y = run[0].anchor.y;
                for (i, it) in run.iter().enumerate() {
                    packed.insert(it.num, egui::pos2(col_x, base_y + i as f32 * row));
                }
            }
        }
    }

    // Drag / right-click-reset are collected and applied AFTER the loop (it
    // borrows `mcu.find_pin_mut`).
    let mut drag_updates: Vec<(usize, (f32, f32))> = Vec::new();
    let mut reset_updates: Vec<usize> = Vec::new();

    for it in items {
        // Field centre: the user's dragged offset, else the packed diamond
        // column, else (axis-aligned default / 90°) straight beyond the arrow.
        let manual = mcu.io_pin_pos.get(&it.num).copied();
        let packed_center = packed.get(&it.num).copied();
        let field_center = if let Some((ox, oy)) = manual {
            chip_center + egui::vec2(ox, oy)
        } else if let Some(c) = packed_center {
            c
        } else {
            let along =
                ARROW_LEN + GAP + (it.dir.x.abs() * FIELD_W + it.dir.y.abs() * FIELD_H) / 2.0;
            it.anchor + it.dir * along
        };
        let field_rect = egui::Rect::from_center_size(field_center, egui::vec2(FIELD_W, FIELD_H));

        // Connector pin ↔ field, with a FIXED-size arrowhead (see `connector`).
        // Straight axis-aligned auto = a short arrow along the pin direction;
        // dragged OR packed-column = a line to the nearest field edge (re-routes
        // as it moves), arrowhead pointing the data direction (out of / into pin).
        let routed = manual.is_some() || packed_center.is_some();
        painter.circle_filled(it.anchor, 2.5, it.color);
        let (from, to) = if routed {
            let edge = nearest_edge(field_rect, it.anchor);
            if it.outbound { (it.anchor, edge) } else { (edge, it.anchor) }
        } else if it.outbound {
            (it.anchor, it.anchor + it.dir * ARROW_LEN)
        } else {
            (it.anchor + it.dir * ARROW_LEN, it.anchor)
        };
        connector(painter, from, to, it.color);

        // Preview of the resulting binding name — faint above the field, and the
        // DRAG HANDLE (the field itself is a text box, so drag from its label).
        let preview = {
            let label = mcu
                .find_pin(it.num)
                .map(|p| p.custom_label.clone())
                .unwrap_or_default();
            let suffix = sanitize_label(&label);
            if suffix.is_empty() {
                it.preview_base.clone()
            } else {
                format!("{}_{}", it.preview_base, suffix)
            }
        };
        let handle_rect = egui::Rect::from_min_max(
            egui::pos2(field_rect.left(), field_rect.top() - 16.0),
            egui::pos2(field_rect.right(), field_rect.top()),
        );
        let resp = ui
            .interact(
                handle_rect,
                ui.id().with(("io_drag", it.num)),
                egui::Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab);
        if resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            let off = (field_center + resp.drag_delta()) - chip_center;
            drag_updates.push((it.num, (off.x, off.y)));
        }
        if manual.is_some() {
            resp.context_menu(|ui| {
                if ui.button("Reset field position").clicked() {
                    reset_updates.push(it.num);
                    ui.close();
                }
            });
        }
        let pcol = if resp.hovered() {
            egui::Color32::from_rgb(205, 205, 215)
        } else {
            egui::Color32::from_rgb(140, 140, 150)
        };
        // Left-aligned with the field's left edge, so a column's labels line up.
        painter.text(
            field_rect.left_top() - egui::vec2(0.0, 2.0),
            egui::Align2::LEFT_BOTTOM,
            preview,
            egui::FontId::proportional(9.0),
            pcol,
        );

        if let Some(pin) = mcu.find_pin_mut(it.num) {
            ui.push_id(("io_label", it.num), |ui| {
                ui.put(
                    field_rect,
                    egui::TextEdit::singleline(&mut pin.custom_label)
                        .hint_text("name")
                        .font(egui::FontId::proportional(10.0)),
                );
            });
        }
    }
    // Apply drag / reset now the per-item `mcu` borrows have ended.
    for (num, off) in drag_updates {
        mcu.io_pin_pos.insert(num, off);
    }
    for num in reset_updates {
        mcu.io_pin_pos.remove(&num);
    }
}

/// Half-extents (from the chip centre) needed to keep every DRAGGED in/out field
/// on-canvas — combined with the module extent by `Mcu::draw` so the Scene's
/// auto-fit doesn't clip a field pulled far from the chip.
pub fn dragged_half_extent(mcu: &Mcu) -> egui::Vec2 {
    let mut hx = 0.0_f32;
    let mut hy = 0.0_f32;
    for (x, y) in mcu.io_pin_pos.values() {
        hx = hx.max(x.abs() + FIELD_W / 2.0);
        hy = hy.max(y.abs() + FIELD_H / 2.0 + 16.0);
    }
    egui::vec2(hx, hy)
}
