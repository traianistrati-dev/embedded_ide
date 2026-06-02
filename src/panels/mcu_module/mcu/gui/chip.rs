//! Chip body and pin rendering — draws the MCU chip and its pins on 4 sides.

use eframe::egui;
use crate::panels::mcu_module::mcu::model::{Mcu, PIN_HEIGHT, PIN_WIDTH, PIN_SPACING};
use super::layout;

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
        if hit {
            clicked_pin = Some(pin.number);
        }
    }

    clicked_pin
}
