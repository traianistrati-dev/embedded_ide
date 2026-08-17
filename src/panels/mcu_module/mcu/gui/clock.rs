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
    /// `state` is the project's Clock-tab state — dragged node positions, the
    /// last action's note, and whether the fields list is shown. The positions
    /// and the view preference persist in `project_structure.config`, not in the
    /// clock config, so moving a box never regenerates code.
    pub fn draw_clock_tab(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut clock_gui::ClockUiState,
    ) -> clock_gui::ClockTabOut {
        // Destructure so the config borrows mutably while the chip's limits,
        // presets and family stay readable alongside it.
        let Mcu {
            clock,
            clock_limits,
            clock_presets,
            clock_defaults,
            clock_manual,
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
                state,
                clock_manual,
            ),
            ClockConfig::None => {
                // Not a dead end any more: the chip has no tree YET, and every
                // way of giving it one is offered here. A tree made this way is
                // the chip's, so it only outlives the session once "Save to
                // chip" writes it into the definition.
                match clock_gui::draw_no_clock(ui, name, family, clock_limits, state) {
                    Some(new_clock) => {
                        if let ClockConfig::Graph(gc) = &new_clock {
                            *clock_defaults = Some(gc.graph.clone());
                        }
                        *clock = new_clock;
                        clock_gui::ClockTabOut {
                            changed: true,
                            save_to_definition: false,
                        }
                    }
                    None => clock_gui::ClockTabOut::default(),
                }
            }
        }
    }
}
