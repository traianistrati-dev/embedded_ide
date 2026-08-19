use super::super::logic::pin::{PIN_ROUNDING, Pin};
use eframe::egui;

/// Helper function to handle pin interaction and return click status.
/// Draws hover/selection feedback and returns whether the pin was clicked.
pub fn listen_on_rect(
    pin: &Pin,
    painter: &egui::Painter,
    ui: &egui::Ui,
    rect: egui::Rect,
    is_selected: bool,
) -> bool {
    // Reserved pins used to return here, unclickable. They still cannot be
    // RECONFIGURED - that is what reserved means - but they can be selected,
    // so the in-chip panel can explain what the pin is for. A power rail you
    // cannot even ask about is a worse answer than one that says "Ground".

    let response = ui.interact(rect, ui.id().with(&pin.number), egui::Sense::click());

    let color = if is_selected {
        egui::Color32::from_rgb(60, 60, 80)
    } else if response.hovered() {
        egui::Color32::DARK_GRAY
    } else {
        pin.get_background_color()
    };

    painter.rect_filled(rect, PIN_ROUNDING, color);

    response.clicked()
}

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
        listen_on_rect(self, painter, ui, rect, is_selected)
    }
}
