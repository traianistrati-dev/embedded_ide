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
                    egui::RichText::new(self.selected_label())
                        .strong()
                        .color(egui::Color32::LIGHT_BLUE),
                );
                ui.label(
                    egui::RichText::new(format!("·  {}", self.selected_family()))
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
                    McuTab::Structure,
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
                    // ── Virtual-module palette + list, in a scrollable strip
                    //    BELOW the chip. Add a module (auto-wires to compatible
                    //    pins), rename it, edit its config, or remove it. Add/
                    //    remove change pin functions, so re-sync pins/ after.
                    let mut modules_changed = false;
                    egui::TopBottomPanel::bottom("vmodules_panel")
                        .resizable(true)
                        .default_height(190.0)
                        .show_inside(ui, |ui| {
                            let Some(mcu) = &mut self.mcu else { return };
                            use crate::panels::mcu_module::mcu::gui::modules as mod_gui;
                            use crate::panels::mcu_module::modules::ModuleKind;

                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Virtual modules:")
                                        .size(12.0)
                                        .color(egui::Color32::from_rgb(150, 150, 160)),
                                );
                                for (kind, hover) in [
                                    (
                                        ModuleKind::GenericInterfaceUsart,
                                        "Add a virtual USART device and auto-wire it to a free USART TX/RX pin pair",
                                    ),
                                    (
                                        ModuleKind::GenericInterfaceSpi,
                                        "Add a virtual SPI device and auto-wire it to free SPI SCK/MOSI/MISO(/NSS) pins",
                                    ),
                                    (
                                        ModuleKind::GenericInterfaceI2c,
                                        "Add a virtual I2C device and auto-wire it to a free I2C SCL/SDA pin pair",
                                    ),
                                    (
                                        ModuleKind::GenericInterfaceCan,
                                        "Add a virtual CAN device and auto-wire it to the CAN RX/TX pins (needs the bxcan crate)",
                                    ),
                                    (
                                        ModuleKind::GenericInterfaceUsb,
                                        "Add a virtual USB device and auto-wire it to the USB D-/D+ pins (PA11/PA12)",
                                    ),
                                ] {
                                    if ui
                                        .button(format!("{} {}", ph::PLUS, kind.short()))
                                        .on_hover_text(hover)
                                        .clicked()
                                        && mcu.add_module(kind)
                                    {
                                        modules_changed = true;
                                    }
                                }
                            });

                            // Id of a module clicked on the canvas last frame →
                            // TOGGLE its list entry this frame (expand if closed,
                            // collapse if open), then it's user-controlled again.
                            let to_open = mcu.expand_module.take();

                            if !mcu.modules.is_empty() {
                                let pin_names: std::collections::HashMap<usize, String> = mcu
                                    .iter_all_pins()
                                    .map(|p| (p.number, p.name.clone()))
                                    .collect();
                                let mut remove_id: Option<String> = None;
                                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                                    for m in &mut mcu.modules {
                                        let title = mod_gui::module_title(m);
                                        let toggle = to_open.as_deref() == Some(m.id.as_str());
                                        // Drive the section via CollapsingState so a canvas
                                        // click can TOGGLE (not just force-open) the entry.
                                        let cs_id =
                                            ui.make_persistent_id(("vmod_hdr", m.id.as_str()));
                                        let mut state =
                                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                                ui.ctx(),
                                                cs_id,
                                                false,
                                            );
                                        if toggle {
                                            state.toggle(ui);
                                        }
                                        state
                                            .show_header(ui, |ui| {
                                                ui.label(egui::RichText::new(title).strong());
                                            })
                                            .body(|ui| {
                                                // Rename field — appended to the generated
                                                // variable name(s); also shown in the title.
                                                ui.horizontal(|ui| {
                                                    ui.label("Name:");
                                                    ui.add(
                                                        egui::TextEdit::singleline(
                                                            m.config.custom_label_mut(),
                                                        )
                                                        .hint_text("variable name")
                                                        .desired_width(160.0),
                                                    );
                                                });
                                                mod_gui::module_config_ui(ui, m, &pin_names);
                                                ui.add_space(4.0);
                                                if ui
                                                    .button(format!("{} Remove module", ph::TRASH))
                                                    .clicked()
                                                {
                                                    remove_id = Some(m.id.clone());
                                                }
                                            });
                                    }
                                });
                                if let Some(id) = remove_id {
                                    mcu.remove_module(&id);
                                    modules_changed = true;
                                }
                            }
                        });

                    if modules_changed {
                        if let Some(mcu) = &self.mcu {
                            let all_pins = mcu.all_pin_functions();
                            self.project_tree.sync_pin_files(&all_pins);
                        }
                    }

                    // Diagram fills the remaining (top) area.
                    // Computed before borrowing `self.mcu` mutably below.
                    let chip_label = self.selected_label();
                    let pin_changed = egui::ScrollArea::both()
                        .show(ui, |ui| match &mut self.mcu {
                            Some(mcu) => mcu.draw(ui),
                            None => {
                                ui.centered_and_justified(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{}  {}  —  support coming soon",
                                            ph::GEAR, chip_label
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
                McuTab::Peripherals => {
                    // Assigning a function here mutates the MCU just like the
                    // Pins tab, so re-sync the generated pins/ files on change.
                    let changed = show_peripherals_tab(ui, &mut self.mcu);
                    if changed.is_some() {
                        if let Some(mcu) = &self.mcu {
                            let all_pins = mcu.all_pin_functions();
                            self.project_tree.sync_pin_files(&all_pins);
                        }
                    }
                }
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
                // Module-relationship diagram — chip-agnostic (works with no
                // MCU selected), so it doesn't gate on `self.mcu`.
                McuTab::Structure => self.show_structure_tab(ui),
            }
        });
    }
}
