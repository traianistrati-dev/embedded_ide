//! MCU chip layout calculations — geometry and positioning.

use crate::panels::mcu_module::mcu::model::{PIN_HEIGHT, PIN_SPACING, PIN_WIDTH};
use eframe::egui;

/// Smallest chip-body span, in "pin units", along an edge that carries few (or
/// no) pins. Without it a dual-in-line layout (all pins on left+right, so
/// `top_count == 0`) collapses to a sliver and the two pin rows look stuck
/// together with no body between them. A floor here gives every chip a visible
/// middle without inventing phantom pins.
pub const MIN_BODY_PINS: usize = 3;

/// The same floor for the WIDTH of a two-sided (DIP) chip — three times
/// [`MIN_BODY_PINS`].
///
/// A quad package gets its body width from the top row; a two-sided one has no
/// top row, so the width was whatever [`MIN_BODY_PINS`] happened to be — about
/// 105 px. That body is not decoration: the selected pin's FUNCTION LIST is
/// drawn inside it, and at that width neither the header ("Pin 2 · PC14") nor a
/// row like "TIM17 CH1 (PWM)" fitted — both were cut off mid-word. The list has
/// nowhere else to go (see [`super::panel`]), so the body has to be wide enough
/// to hold it.
pub const MIN_BODY_PINS_DIP: usize = MIN_BODY_PINS * 3;

/// Calculate chip dimensions and canvas size based on pin counts.
/// `top_pad` is the blank slots a BOARD keeps at each end of its top row, so
/// the body has to be that much wider — see `geometry::BOARD_EDGE_PAD`.
pub fn calculate_layout(
    top_count: usize,
    left_count: usize,
    top_pad: usize,
) -> (f32, f32, f32, f32) {
    // No top row → nothing but the floor decides the width, and for a chip whose
    // body has to carry the function list that floor is the wider one.
    let width_floor = if top_count == 0 {
        MIN_BODY_PINS_DIP
    } else {
        MIN_BODY_PINS
    };
    let body_w = (top_count + 2 * top_pad).max(width_floor) as f32;
    let body_h = left_count.max(MIN_BODY_PINS) as f32;
    let mcu_width = (body_w * (PIN_WIDTH + PIN_SPACING)) + PIN_SPACING * 2.0;
    let mcu_height = (body_h * (PIN_WIDTH + PIN_SPACING)) + PIN_SPACING * 2.0;
    let canvas_w = mcu_width + PIN_HEIGHT * 2.0 + 20.0;
    let canvas_h = mcu_height + PIN_HEIGHT * 2.0 + 20.0;
    (mcu_width, mcu_height, canvas_w, canvas_h)
}

/// Body + canvas for a BALL-GRID package, whose size comes from the grid it has
/// to contain rather than from pin counts along the edges. The canvas keeps the
/// same margin the edge-pin layout leaves, so the two look consistent side by
/// side in the chip selector.
pub fn calculate_grid_layout(body: egui::Vec2) -> (f32, f32, f32, f32) {
    (body.x, body.y, body.x + 20.0, body.y + 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-sided chip is three times wider than the bare floor, because its
    /// body is where the pin-function list lives.
    #[test]
    fn a_dip_body_is_wide_enough_for_the_function_list() {
        let (dip_w, _, _, _) = calculate_layout(0, 20, 0);
        let (min_w, _, _, _) = calculate_layout(3, 20, 0);
        assert!(
            (dip_w / min_w - 3.0).abs() < 0.15,
            "3x the old width: {dip_w} vs {min_w}"
        );
        // Wide enough for the widest thing drawn in there: a function row plus
        // its info button, inside the list's own margins.
        assert!(dip_w > 280.0, "{dip_w}");
    }

    /// A quad package is untouched: its width still comes from its own top row,
    /// so nothing about the existing chips moves.
    #[test]
    fn a_quad_package_keeps_its_pin_derived_width() {
        let (w12, _, _, _) = calculate_layout(12, 12, 0);
        let (w3, _, _, _) = calculate_layout(3, 12, 0);
        assert!(w12 > w3, "12 top pins are wider than 3");
        // The floor still applies below it, exactly as before.
        assert_eq!(calculate_layout(1, 12, 0).0, w3);
    }
}

/// Calculate pin Y position for pins on left/right sides.
pub fn pin_y_position(i: usize) -> f32 {
    PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING)
}

/// Calculate pin X/Y for pins on top/bottom sides.
pub fn pin_x_position(i: usize) -> f32 {
    PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING)
}

/// Calculate scrollable area bounds for function list.
pub fn function_panel_bounds(chip_rect: egui::Rect, sep_y: f32) -> (f32, f32, f32) {
    let content_top = sep_y + 12.0;
    let content_bottom = chip_rect.bottom() - 8.0;
    let available_h = (content_bottom - content_top).max(0.0);
    (content_top, content_bottom, available_h)
}

/// Calculate button dimensions and scrollbar parameters.
pub fn button_layout(
    chip_rect: egui::Rect,
    func_count: usize,
    btn_h: f32,
) -> (f32, f32, f32, f32, f32) {
    let item_h = btn_h + 6.0;
    let total_h = func_count as f32 * item_h;
    let content_top = chip_rect.top() + 50.0; // approx header height
    let content_bottom = chip_rect.bottom() - 8.0;
    let available_h = (content_bottom - content_top).max(0.0);
    let max_scroll = (total_h - available_h).max(0.0);
    (total_h, item_h, available_h, max_scroll, content_top)
}
