//! High-level pin rendering orchestration.
//! Coordinates text, shapes, layout, and interaction for pin visualization.

pub mod layout;
pub mod shapes;
pub mod text;

use super::super::logic::pin::Pin;
use super::listeners;
use eframe::egui;

/// A selected pin is called out in WHITE — text and border alike — so it reads
/// as the focused item from across the diagram.
const SELECTION_COLOR: egui::Color32 = egui::Color32::WHITE;
/// Its name is drawn 10 % larger than an idle pin's, and its border twice as
/// thick, on top of the colour.
pub const SELECTED_TEXT_SCALE: f32 = 1.1;
const STROKE_W: f32 = 2.0;
const SELECTED_STROKE_W: f32 = STROKE_W * 2.0;

/// `(text colour, font size, border width)` for a pin in either state.
fn selection_style(pin: &Pin, is_selected: bool) -> (egui::Color32, f32, f32) {
    if is_selected {
        (
            SELECTION_COLOR,
            super::super::logic::pin::PIN_FONT_SIZE * SELECTED_TEXT_SCALE,
            SELECTED_STROKE_W,
        )
    } else {
        (
            pin.get_text_color(),
            super::super::logic::pin::PIN_FONT_SIZE,
            STROKE_W,
        )
    }
}

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

        let (text_color, font_size, stroke_w) = selection_style(self, is_selected);
        text::draw_horizontal_text_colored(
            self,
            painter,
            layout::text_position_horizontal(rect),
            text_color,
            font_size,
        );

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR, stroke_w);
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

        let (text_color, font_size, stroke_w) = selection_style(self, is_selected);
        text::draw_horizontal_text_colored(
            self,
            painter,
            layout::text_position_horizontal(rect),
            text_color,
            font_size,
        );

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR, stroke_w);
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

        let (text_color, font_size, stroke_w) = selection_style(self, is_selected);
        text::draw_vertical_text_colored(
            self,
            painter,
            layout::text_position_vertical(rect),
            text_color,
            font_size,
        );

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR, stroke_w);
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

        let (text_color, font_size, stroke_w) = selection_style(self, is_selected);
        text::draw_vertical_text_colored(
            self,
            painter,
            layout::text_position_vertical(rect),
            text_color,
            font_size,
        );

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR, stroke_w);
        }

        (rect, clicked)
    }
}
