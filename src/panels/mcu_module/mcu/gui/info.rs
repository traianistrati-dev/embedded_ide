//! Information popup window — displays detailed specifications for pin functions.

use eframe::egui;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

/// Render the info popup window for a pin function.
/// Returns `true` if the window should stay open, `false` if closed.
pub fn draw_info_popup(
    func: &PinFunction,
    chip_rect: egui::Rect,
    ui: &mut egui::Ui,
) -> bool {
    let info = func.info();
    let mut open = true;

    let popup_pos = egui::pos2(chip_rect.center().x - 170.0, chip_rect.center().y - 100.0);

    egui::Window::new(format!("{}", func.label()))
        .open(&mut open)
        .resizable(true)
        .default_width(340.0)
        .default_pos(popup_pos)
        .show(ui.ctx(), |ui| {
            // Description
            ui.label(
                egui::RichText::new(&info.description)
                    .size(14.0)
                    .color(egui::Color32::from_rgb(20, 20, 20)),
            );

            if !info.specs.is_empty() {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // Specs grid
                egui::Grid::new("info_specs_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for (key, value) in &info.specs {
                            ui.label(
                                egui::RichText::new(key)
                                    .size(12.0)
                                    .color(egui::Color32::from_rgb(0, 50, 250)),
                            );
                            ui.label(
                                egui::RichText::new(value)
                                    .size(12.0)
                                    .color(egui::Color32::DARK_GRAY),
                            );
                            ui.end_row();
                        }
                    });
            }
        });

    open
}
