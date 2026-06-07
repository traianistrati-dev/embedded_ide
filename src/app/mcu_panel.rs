//! Central "MCU Configurator" panel — chip label, tab bar (Pins / Peripherals
//! / Clock / System) and the active tab's content.
//!
//! Inherent method on [`AppIde`]; the Pins tab draws the chip and, on any pin
//! change, re-syncs the generated `pins/` files.

use super::tabs::show_peripherals_tab;
use super::{AppIde, McuTab};
use eframe::egui;
use egui_phosphor::regular as ph;

impl AppIde {
    /// Render the central MCU configurator panel.
    pub(super) fn show_mcu_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("MCU Configurator");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let reset_btn = ui
                        .add(egui::Button::new(
                            egui::RichText::new(format!(
                                "{} Reset pins",
                                ph::ARROW_COUNTER_CLOCKWISE
                            ))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(220, 100, 80)),
                        ))
                        .on_hover_text("Clear all pin function selections");
                    if reset_btn.clicked() {
                        if let Some(mcu) = &mut self.mcu {
                            mcu.reset_all_pins();
                        }
                    }
                });
            });

            // Chip label — always read-only.
            // Selection is done exclusively via the "New Project" popup.
            ui.horizontal(|ui| {
                ui.label("Chip:");
                ui.label(
                    egui::RichText::new(self.selected_mcu_type.label())
                        .strong()
                        .color(egui::Color32::LIGHT_BLUE),
                );
                ui.label(
                    egui::RichText::new(format!("·  {}", self.selected_mcu_type.family()))
                        .color(egui::Color32::GRAY)
                        .size(11.0),
                );
            });

            ui.separator();

            // Tab bar
            ui.horizontal(|ui| {
                for tab in [
                    McuTab::Pins,
                    McuTab::Peripherals,
                    McuTab::Clock,
                    McuTab::System,
                ] {
                    let is_active = self.active_tab == tab;
                    let label = egui::RichText::new(tab.label())
                        .size(13.0)
                        .color(if is_active {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::from_rgb(160, 160, 170)
                        });
                    if ui.selectable_label(is_active, label).clicked() {
                        self.active_tab = tab;
                    }
                }
            });

            ui.separator();

            // Tab content
            match self.active_tab {
                McuTab::Pins => {
                    let pin_changed = egui::ScrollArea::both()
                        .show(ui, |ui| match &mut self.mcu {
                            Some(mcu) => mcu.draw(ui),
                            None => {
                                ui.centered_and_justified(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{}  {}  —  support coming soon",
                                            ph::GEAR,
                                            self.selected_mcu_type.label()
                                        ))
                                        .size(18.0)
                                        .color(egui::Color32::GRAY),
                                    );
                                });
                                None
                            }
                        })
                        .inner;

                    // Any pin change (configure OR deselect) triggers a full
                    // sync: files for unconfigured pins are removed, files for
                    // configured pins are created/updated, and mod.rs is rebuilt.
                    if pin_changed.is_some() {
                        if let Some(mcu) = &self.mcu {
                            let all_pins = mcu.all_pin_functions();
                            self.project_tree.sync_pin_files(&all_pins);
                        }
                    }
                }
                McuTab::Peripherals => show_peripherals_tab(ui, &self.mcu),
                McuTab::Clock => match &mut self.mcu {
                    Some(mcu) => {
                        // The Clock tab owns its layout (fixed 3-zone footer +
                        // scrollable diagram), so no outer ScrollArea here.
                        // Mutating mcu.clock is enough — `init_frame`
                        // regenerates main.rs from MCU state each frame.
                        let _changed = mcu.draw_clock_tab(ui);
                    }
                    None => {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  Clock configuration — coming soon",
                                    ph::CLOCK
                                ))
                                .size(16.0)
                                .color(egui::Color32::GRAY),
                            );
                        });
                    }
                },
                McuTab::System => {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  System configuration — coming soon",
                                ph::GEAR
                            ))
                            .size(16.0)
                            .color(egui::Color32::GRAY),
                        );
                    });
                }
            }
        });
    }
}
