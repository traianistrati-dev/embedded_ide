//! Shape rendering primitives for pins (rectangles and outlines).

use eframe::egui;

/// Draw filled rectangle with rounded corners.
pub fn draw_rect_filled(
    painter: &egui::Painter,
    rect: egui::Rect,
    rounding: f32,
    color: egui::Color32,
) {
    painter.rect_filled(rect, rounding, color);
}

/// Draw rectangle outline (stroke) with rounded corners.
pub fn draw_rect_stroke(
    painter: &egui::Painter,
    rect: egui::Rect,
    rounding: f32,
    stroke_color: egui::Color32,
) {
    painter.rect_stroke(
        rect,
        rounding,
        egui::Stroke::new(2.0, stroke_color),
        egui::StrokeKind::Middle,
    );
}
