//! Clock tab rendering — dispatches to the per-family clock GUI.

use crate::panels::mcu_module::clock::{gui as clock_gui, ClockConfig};
use crate::panels::mcu_module::mcu::model::Mcu;
use eframe::egui;

impl Mcu {
    /// Render the "Clock" tab. Returns `true` if the configuration changed
    /// (the app regenerates `main.rs` from MCU state every frame in `init_frame`).
    pub fn draw_clock_tab(&mut self, ui: &mut egui::Ui) -> bool {
        // Destructure so the config borrows mutably while the chip's limits,
        // presets and family stay readable alongside it.
        let Mcu { clock, clock_limits, clock_presets, family, name, .. } = self;
        match clock {
            ClockConfig::Graph(gc) => {
                clock_gui::draw_graph_clock(ui, gc, clock_limits, clock_presets, family)
            }
            ClockConfig::None => {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Clock configuration for {name} is not modelled yet.",
                        ))
                        .size(14.0)
                        .color(egui::Color32::GRAY),
                    );
                });
                false
            }
        }
    }
}
