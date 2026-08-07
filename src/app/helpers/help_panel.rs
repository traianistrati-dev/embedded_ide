//! Collapsible help panel for the probe tabs (RTT / Debug).
//!
//! A toolbar "Help" toggle plus the panel it opens: one row per command
//! (what it does, when to use it) and a notes block below. The open flag lives
//! in egui memory keyed by the caller's `id`, so the tab render functions stay
//! stateless and nothing leaks into the domain structs.
//!
//! Usage: `toggle_button(ui, "rtt")` inside the toolbar row, then
//! `show_panel(ui, "rtt", &rows, &notes)` right after it.

use eframe::egui;
use egui_phosphor::regular as ph;

/// One documented command: `(name, name colour, what it does)`. The colour
/// matches the command's own button, so the row is easy to map to the toolbar.
pub type HelpRow<'a> = (&'a str, egui::Color32, &'a str);

fn id_of(id: &str) -> egui::Id {
    egui::Id::new("help_panel").with(id)
}

fn is_open(ui: &egui::Ui, id: &str) -> bool {
    ui.data(|d| d.get_temp::<bool>(id_of(id)).unwrap_or(false))
}

/// The toolbar toggle button.
pub fn toggle_button(ui: &mut egui::Ui, id: &str) {
    let open = is_open(ui, id);
    if ui
        .selectable_label(
            open,
            egui::RichText::new(format!("{} Help", ph::INFO))
                .size(10.5)
                .color(if open {
                    egui::Color32::from_rgb(120, 170, 240)
                } else {
                    egui::Color32::from_gray(150)
                }),
        )
        .on_hover_text("What does each command do?")
        .clicked()
    {
        ui.data_mut(|d| d.insert_temp(id_of(id), !open));
    }
}

/// The panel body — a no-op while the toggle is off.
pub fn show_panel(ui: &mut egui::Ui, id: &str, rows: &[HelpRow], notes: &[&str]) {
    if !is_open(ui, id) {
        return;
    }
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(28, 32, 40))
        .inner_margin(egui::Margin::same(8))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            egui::Grid::new(id_of(id).with("grid"))
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for (name, color, text) in rows {
                        ui.label(egui::RichText::new(*name).size(11.0).strong().color(*color));
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(*text)
                                    .size(11.0)
                                    .color(egui::Color32::from_gray(195)),
                            )
                            .wrap(),
                        );
                        ui.end_row();
                    }
                });
            if !notes.is_empty() {
                ui.add_space(6.0);
                for note in notes {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{} {note}", ph::DOT))
                                .size(10.5)
                                .color(egui::Color32::from_gray(140)),
                        )
                        .wrap(),
                    );
                }
            }
        });
    ui.add_space(2.0);
}
