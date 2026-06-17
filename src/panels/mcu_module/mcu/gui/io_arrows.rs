//! In/out arrow graphics for GPIO Input / Output / PWM pins on the Pins canvas.
//!
//! A lightweight cousin of the virtual-module schematic ([`super::modules`]):
//! every pin configured as **GPIO Output** or **PWM** gets an arrow pointing
//! away from the chip (a driven output), every **GPIO Input** an arrow pointing
//! into the chip. Each arrow carries a small text field to rename the pin; the
//! typed text is appended to the generated binding name, e.g. a `pc13` output
//! labelled "led" becomes `let pc13_out_led = …`. No add/remove buttons — the
//! arrows simply mirror the pin functions.

use super::super::model::{Mcu, PIN_HEIGHT};
use super::modules::pin_anchor_dir;
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
    chip_rect: egui::Rect,
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
        let Some((anchor, dir)) = pin_anchor_dir(mcu, chip_rect, p.number) else {
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

    for it in items {
        // Arrow: outbound points away from the chip, inbound points into it.
        let outer = it.anchor + it.dir * ARROW_LEN;
        let stroke = egui::Stroke::new(2.0, it.color);
        if it.outbound {
            painter.arrow(it.anchor, it.dir * ARROW_LEN, stroke);
        } else {
            painter.arrow(outer, -it.dir * ARROW_LEN, stroke);
        }
        painter.circle_filled(it.anchor, 2.5, it.color);

        // Rename field, just beyond the arrow's outer end.
        let along =
            ARROW_LEN + GAP + (it.dir.x.abs() * FIELD_W + it.dir.y.abs() * FIELD_H) / 2.0;
        let center = it.anchor + it.dir * along;
        let field_rect = egui::Rect::from_center_size(center, egui::vec2(FIELD_W, FIELD_H));

        // Preview of the resulting binding name, faint above the field.
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
        painter.text(
            field_rect.center_top() - egui::vec2(0.0, 2.0),
            egui::Align2::CENTER_BOTTOM,
            preview,
            egui::FontId::proportional(9.0),
            egui::Color32::from_rgb(140, 140, 150),
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
}
