use super::pin::{PIN_FONT_SIZE, PIN_ROUNDING, Pin};
use eframe::egui;

impl Pin {
    pub fn draw_vertical_text(&self, painter: &egui::Painter, pos: egui::Pos2) {
        self.draw_vertical_text_colored(painter, pos, self.get_text_color());
    }

    fn draw_vertical_text_colored(
        &self,
        painter: &egui::Painter,
        pos: egui::Pos2,
        color: egui::Color32,
    ) {
        let galley = painter.layout_no_wrap(
            self.name.to_owned(),
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

    pub fn draw_horizontal_text(&self, painter: &egui::Painter, pos: egui::Pos2) {
        painter.text(
            pos,
            egui::Align2::LEFT_CENTER,
            self.name.as_str(),
            egui::FontId::monospace(PIN_FONT_SIZE),
            self.get_text_color(),
        );
    }

    fn draw_horizontal_text_colored(
        &self,
        painter: &egui::Painter,
        pos: egui::Pos2,
        color: egui::Color32,
    ) {
        painter.text(
            pos,
            egui::Align2::LEFT_CENTER,
            self.name.as_str(),
            egui::FontId::monospace(PIN_FONT_SIZE),
            color,
        );
    }

    /// Draws on the right side. Returns (rect, clicked).
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
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(height, width));
        painter.rect_filled(rect, PIN_ROUNDING, self.get_background_color());

        let clicked = ui.map_or(false, |ui| self.listen(painter, ui, rect, is_selected));

        let text_color = if is_selected { egui::Color32::YELLOW } else { self.get_text_color() };
        self.draw_horizontal_text_colored(painter, egui::pos2(rect.left() + 2.0, rect.center().y), text_color);

        if is_selected {
            painter.rect_stroke(
                rect,
                PIN_ROUNDING,
                egui::Stroke::new(2.0, egui::Color32::YELLOW),
                egui::StrokeKind::Middle,
            );
        }

        (rect, clicked)
    }

    /// Draws on the left side. Returns (rect, clicked).
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
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(height, width));
        painter.rect_filled(rect, PIN_ROUNDING, self.get_background_color());

        let clicked = ui.map_or(false, |ui| self.listen(painter, ui, rect, is_selected));

        let text_color = if is_selected { egui::Color32::YELLOW } else { self.get_text_color() };
        self.draw_horizontal_text_colored(painter, egui::pos2(rect.left() + 2.0, rect.center().y), text_color);

        if is_selected {
            painter.rect_stroke(
                rect,
                PIN_ROUNDING,
                egui::Stroke::new(2.0, egui::Color32::YELLOW),
                egui::StrokeKind::Middle,
            );
        }

        (rect, clicked)
    }

    /// Draws on the top side. Returns (rect, clicked).
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
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height));
        painter.rect_filled(rect, PIN_ROUNDING, self.get_background_color());

        let clicked = ui.map_or(false, |ui| self.listen(painter, ui, rect, is_selected));

        let text_color = if is_selected { egui::Color32::YELLOW } else { self.get_text_color() };
        self.draw_vertical_text_colored(painter, egui::pos2(x + (width / 3.4), y + height - 4.0), text_color);

        if is_selected {
            painter.rect_stroke(
                rect,
                PIN_ROUNDING,
                egui::Stroke::new(2.0, egui::Color32::YELLOW),
                egui::StrokeKind::Middle,
            );
        }

        (rect, clicked)
    }

    /// Draws on the bottom side. Returns (rect, clicked).
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
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height));
        painter.rect_filled(rect, PIN_ROUNDING, self.get_background_color());

        let clicked = ui.map_or(false, |ui| self.listen(painter, ui, rect, is_selected));

        let text_color = if is_selected { egui::Color32::YELLOW } else { self.get_text_color() };
        self.draw_vertical_text_colored(painter, egui::pos2(x + (width / 3.4), y + height - 4.0), text_color);

        if is_selected {
            painter.rect_stroke(
                rect,
                PIN_ROUNDING,
                egui::Stroke::new(2.0, egui::Color32::YELLOW),
                egui::StrokeKind::Middle,
            );
        }

        (rect, clicked)
    }
}
