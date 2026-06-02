//! Text rendering primitives for pins (vertical and horizontal).

use super::super::super::logic::pin::{PIN_FONT_SIZE, Pin};
use eframe::egui;

/// Render pin name vertically (rotated -90°) with custom color.
pub fn draw_vertical_text_colored(
    pin: &Pin,
    painter: &egui::Painter,
    pos: egui::Pos2,
    color: egui::Color32,
) {
    let galley = painter.layout_no_wrap(
        pin.name.to_owned(),
        egui::FontId::monospace(PIN_FONT_SIZE),
        color,
    );

    let text_shape = egui::epaint::TextShape {
        pos,
        galley,
        underline: egui::epaint::Stroke::NONE,
        override_text_color: Some(color),
        angle: -std::f32::consts::FRAC_PI_2,
        fallback_color: color,
        opacity_factor: 1.0,
    };

    painter.add(egui::Shape::Text(text_shape));
}

/// Render pin name horizontally with custom color.
pub fn draw_horizontal_text_colored(
    pin: &Pin,
    painter: &egui::Painter,
    pos: egui::Pos2,
    color: egui::Color32,
) {
    painter.text(
        pos,
        egui::Align2::LEFT_CENTER,
        pin.name.as_str(),
        egui::FontId::monospace(PIN_FONT_SIZE),
        color,
    );
}
