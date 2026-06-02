//! MCU chip layout calculations — geometry and positioning.

use eframe::egui;
use crate::panels::mcu_module::mcu::model::{PIN_HEIGHT, PIN_WIDTH, PIN_SPACING};

/// Calculate chip dimensions and canvas size based on pin counts.
pub fn calculate_layout(top_count: usize, left_count: usize) -> (f32, f32, f32, f32) {
    let mcu_width = (top_count as f32 * (PIN_WIDTH + PIN_SPACING)) + PIN_SPACING * 2.0;
    let mcu_height = (left_count as f32 * (PIN_WIDTH + PIN_SPACING)) + PIN_SPACING * 2.0;
    let canvas_w = mcu_width + PIN_HEIGHT * 2.0 + 20.0;
    let canvas_h = mcu_height + PIN_HEIGHT * 2.0 + 20.0;
    (mcu_width, mcu_height, canvas_w, canvas_h)
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
pub fn function_panel_bounds(
    chip_rect: egui::Rect,
    sep_y: f32,
) -> (f32, f32, f32) {
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
    let content_top = chip_rect.top() + 50.0;  // approx header height
    let content_bottom = chip_rect.bottom() - 8.0;
    let available_h = (content_bottom - content_top).max(0.0);
    let max_scroll = (total_h - available_h).max(0.0);
    (total_h, item_h, available_h, max_scroll, content_top)
}
