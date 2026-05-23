use super::pin::{PIN_ROUNDING, Pin};
use eframe::egui;

impl Pin {
    pub fn listen(&self, painter: &egui::Painter, ui: &egui::Ui, rect: egui::Rect) {
        if self.reserved {
            return;
        }

        let response = ui.interact(rect, ui.id().with(&self.number), egui::Sense::click());

        let color = if response.hovered() {
            egui::Color32::DARK_GRAY
        } else {
            self.get_backgroung_collor()
        };

        painter.rect_filled(rect, PIN_ROUNDING, color);
    }
}
