//! High-level pin rendering orchestration.
//! Coordinates text, shapes, layout, and interaction for pin visualization.

pub mod layout;
pub mod shapes;
pub mod text;

use super::super::logic::pin::Pin;
use super::listeners;
use eframe::egui;

const SELECTION_COLOR: egui::Color32 = egui::Color32::YELLOW;

impl Pin {
    /// Draws pin on the right side. Returns (rect, clicked).
    pub fn draw_right(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        height: f32,
        width: f32,
        ui: Option<&egui::Ui>,
        is_selected: bool,
    ) -> (egui::Rect, bool) {
        let rect = layout::calc_rect_right(x, y, height, width);
        shapes::draw_rect_filled(painter, rect, 0.0, self.get_background_color());

        let clicked = ui.map_or(false, |ui| listeners::listen_on_rect(self, painter, ui, rect, is_selected));

        let text_color = if is_selected { SELECTION_COLOR } else { self.get_text_color() };
        text::draw_horizontal_text_colored(self, painter, layout::text_position_horizontal(rect), text_color);

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR);
        }

        (rect, clicked)
    }

    /// Draws pin on the left side. Returns (rect, clicked).
    pub fn draw_left(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        height: f32,
        width: f32,
        ui: Option<&egui::Ui>,
        is_selected: bool,
    ) -> (egui::Rect, bool) {
        let rect = layout::calc_rect_left(x, y, height, width);
        shapes::draw_rect_filled(painter, rect, 0.0, self.get_background_color());

        let clicked = ui.map_or(false, |ui| listeners::listen_on_rect(self, painter, ui, rect, is_selected));

        let text_color = if is_selected { SELECTION_COLOR } else { self.get_text_color() };
        text::draw_horizontal_text_colored(self, painter, layout::text_position_horizontal(rect), text_color);

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR);
        }

        (rect, clicked)
    }

    /// Draws pin on the top side. Returns (rect, clicked).
    pub fn draw_top(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        height: f32,
        width: f32,
        ui: Option<&egui::Ui>,
        is_selected: bool,
    ) -> (egui::Rect, bool) {
        let rect = layout::calc_rect_top(x, y, height, width);
        shapes::draw_rect_filled(painter, rect, 0.0, self.get_background_color());

        let clicked = ui.map_or(false, |ui| listeners::listen_on_rect(self, painter, ui, rect, is_selected));

        let text_color = if is_selected { SELECTION_COLOR } else { self.get_text_color() };
        text::draw_vertical_text_colored(self, painter, layout::text_position_vertical(rect), text_color);

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR);
        }

        (rect, clicked)
    }

    /// Draws pin on the bottom side. Returns (rect, clicked).
    pub fn draw_bottom(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        height: f32,
        width: f32,
        ui: Option<&egui::Ui>,
        is_selected: bool,
    ) -> (egui::Rect, bool) {
        let rect = layout::calc_rect_bottom(x, y, height, width);
        shapes::draw_rect_filled(painter, rect, 0.0, self.get_background_color());

        let clicked = ui.map_or(false, |ui| listeners::listen_on_rect(self, painter, ui, rect, is_selected));

        let text_color = if is_selected { SELECTION_COLOR } else { self.get_text_color() };
        text::draw_vertical_text_colored(self, painter, layout::text_position_vertical(rect), text_color);

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR);
        }

        (rect, clicked)
    }
}
