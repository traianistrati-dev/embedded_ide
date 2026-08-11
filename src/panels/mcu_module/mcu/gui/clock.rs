//! Clock tab rendering — dispatches to the per-family clock GUI.

use crate::panels::mcu_module::clock::{ClockConfig, gui as clock_gui};
use crate::panels::mcu_module::mcu::model::Mcu;
use eframe::egui;

impl Mcu {
    /// Render the "Clock" tab. The returned [`ClockTabOut`] says whether the
    /// configuration changed (the app regenerates `main.rs` from MCU state every
    /// frame in `init_frame`) and whether the user asked to write the edited tree
    /// back into the chip definition.
    ///
    /// `positions` are the project's dragged node positions (edit mode reads and
    /// writes them); they persist in `project_structure.config`, not in the
    /// clock config, so moving a box never regenerates code.
    pub fn draw_clock_tab(
        &mut self,
        ui: &mut egui::Ui,
        positions: &mut crate::panels::mcu_module::structure_config::ClockPositions,
        note: &mut String,
    ) -> clock_gui::ClockTabOut {
        // Destructure so the config borrows mutably while the chip's limits,
        // presets and family stay readable alongside it.
        let Mcu {
            clock,
            clock_limits,
            clock_presets,
            clock_defaults,
            family,
            name,
            ..
        } = self;
        match clock {
            ClockConfig::Graph(gc) => clock_gui::draw_graph_clock(
                ui,
                gc,
                clock_limits,
                clock_presets,
                clock_defaults.as_ref(),
                family,
                positions,
                note,
            ),
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
                clock_gui::ClockTabOut::default()
            }
        }
    }
}
