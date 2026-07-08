//! Chip body and pin rendering — draws the MCU chip and its pins on 4 sides.

use eframe::egui;
use crate::panels::mcu_module::mcu::model::{Mcu, PIN_HEIGHT, PIN_WIDTH, PIN_SPACING};
use super::layout;

// ── Pin-number label (drawn INSIDE the chip body, white) ────────────────────
/// Inset from the chip edge for the number.
const NUM_MARGIN: f32 = 4.0;
const NUM_COLOR: egui::Color32 = egui::Color32::WHITE;
/// `FontId` isn't const (needs a runtime `f32` size), so this is a small fn.
fn num_font() -> egui::FontId {
    egui::FontId::monospace(11.0)
}

/// Draw the chip body (gray rectangle).
pub fn draw_chip_body(painter: &egui::Painter, chip_rect: egui::Rect) {
    painter.rect_filled(chip_rect, 4.0, egui::Color32::from_rgb(45, 45, 55));
}

/// Render all pins on all 4 sides and detect clicks.
/// Returns `Some(pin_number)` if a pin was clicked.
pub fn render_pins_and_detect_clicks(
    mcu: &Mcu,
    painter: &egui::Painter,
    chip_rect: egui::Rect,
    ui: &mut egui::Ui,
) -> Option<usize> {
    let mut clicked_pin: Option<usize> = None;
    let selected = mcu.selected_pin;

    // RIGHT
    for (i, pin) in mcu.right_pins.iter().enumerate() {
        let y = chip_rect.top() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let x = chip_rect.right();
        let (_, hit) = pin.draw_right(
            &painter,
            x,
            y,
            PIN_HEIGHT,
            PIN_WIDTH,
            Some(ui),
            selected == Some(pin.number),
        );
        // Pin number inside the chip, right-aligned just inside the right edge.
        painter.text(
            egui::pos2(chip_rect.right() - NUM_MARGIN, y + PIN_WIDTH / 2.0),
            egui::Align2::RIGHT_CENTER,
            pin.number,
            num_font(),
            NUM_COLOR,
        );
        if hit {
            clicked_pin = Some(pin.number);
        }
    }

    // LEFT
    for (i, pin) in mcu.left_pins.iter().enumerate() {
        let y = chip_rect.top() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let x = chip_rect.left() - PIN_HEIGHT;
        let (_, hit) = pin.draw_left(
            &painter,
            x,
            y,
            PIN_HEIGHT,
            PIN_WIDTH,
            Some(ui),
            selected == Some(pin.number),
        );
        // Pin number inside the chip, left-aligned just inside the left edge.
        painter.text(
            egui::pos2(chip_rect.left() + NUM_MARGIN, y + PIN_WIDTH / 2.0),
            egui::Align2::LEFT_CENTER,
            pin.number,
            num_font(),
            NUM_COLOR,
        );
        if hit {
            clicked_pin = Some(pin.number);
        }
    }

    // TOP
    for (i, pin) in mcu.top_pins.iter().enumerate() {
        let x = chip_rect.left() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let y = chip_rect.top() - PIN_HEIGHT;
        let (_, hit) = pin.draw_top(
            &painter,
            x,
            y,
            PIN_HEIGHT,
            PIN_WIDTH,
            Some(ui),
            selected == Some(pin.number),
        );
        // Pin number inside the chip, just below the top edge.
        painter.text(
            egui::pos2(x + PIN_WIDTH / 2.0, chip_rect.top() + NUM_MARGIN),
            egui::Align2::CENTER_TOP,
            pin.number,
            num_font(),
            NUM_COLOR,
        );
        if hit {
            clicked_pin = Some(pin.number);
        }
    }

    // BOTTOM
    for (i, pin) in mcu.bottom_pins.iter().enumerate() {
        let x = chip_rect.left() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let y = chip_rect.bottom();
        let (_, hit) = pin.draw_bottom(
            &painter,
            x,
            y,
            PIN_HEIGHT,
            PIN_WIDTH,
            Some(ui),
            selected == Some(pin.number),
        );
        // Pin number inside the chip, just above the bottom edge.
        painter.text(
            egui::pos2(x + PIN_WIDTH / 2.0, chip_rect.bottom() - NUM_MARGIN),
            egui::Align2::CENTER_BOTTOM,
            pin.number,
            num_font(),
            NUM_COLOR,
        );
        if hit {
            clicked_pin = Some(pin.number);
        }
    }

    clicked_pin
}
