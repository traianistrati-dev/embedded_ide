//! Layout and positioning logic for pin rectangles.

use eframe::egui;

/// Calculate rectangle for pin on right side.
/// width and height are swapped for right orientation.
pub fn calc_rect_right(x: f32, y: f32, height: f32, width: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(height, width))
}

/// Calculate rectangle for pin on left side.
/// width and height are swapped for left orientation.
pub fn calc_rect_left(x: f32, y: f32, height: f32, width: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(height, width))
}

/// Calculate rectangle for pin on top side.
pub fn calc_rect_top(x: f32, y: f32, height: f32, width: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height))
}

/// Calculate rectangle for pin on bottom side.
pub fn calc_rect_bottom(x: f32, y: f32, height: f32, width: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height))
}

/// Calculate text position for horizontal (left/right side) pins.
pub fn text_position_horizontal(rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(rect.left() + 2.0, rect.center().y)
}

/// Calculate text position for vertical (top/bottom side) pins.
pub fn text_position_vertical(rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(
        rect.left() + (rect.width() / 3.4),
        rect.top() + rect.height() - 4.0,
    )
}
