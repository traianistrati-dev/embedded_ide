use super::pin::{PIN_ROUNDING, Pin};
use eframe::egui;

impl Pin {
    /// Handles hover and click interaction for a pin.
    /// Returns `true` if the pin was clicked this frame.
    pub fn listen(
        &self,
        painter: &egui::Painter,
        ui: &egui::Ui,
        rect: egui::Rect,
        is_selected: bool,
    ) -> bool {
        if self.reserved {
            return false;
        }

        let response = ui.interact(rect, ui.id().with(&self.number), egui::Sense::click());

        let color = if is_selected {
            egui::Color32::from_rgb(60, 60, 80)
        } else if response.hovered() {
            egui::Color32::DARK_GRAY
        } else {
            self.get_background_color()
        };

        painter.rect_filled(rect, PIN_ROUNDING, color);

        response.clicked()
    }
}
