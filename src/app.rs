use eframe::egui;
use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::mcu_catalog::McuType;
use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;

pub struct AppIde {
    code: String,
    selected_mcu_type: McuType,
    /// None when the selected chip is not yet implemented
    mcu: Option<Mcu>,
}

impl AppIde {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            code: String::from("// main.rs\n"),
            selected_mcu_type: McuType::Stm32f103c8t6,
            mcu: Some(create_stm32f103c8tx()),
        }
    }

    /// Creates the MCU instance for the given chip type.
    /// Returns None if the chip is not yet supported.
    fn init_mcu(mcu_type: &McuType) -> Option<Mcu> {
        match mcu_type {
            McuType::Stm32f103c8t6 => Some(create_stm32f103c8tx()),
            _ => None,
        }
    }
}

impl eframe::App for AppIde {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let left_width = ui.available_width() * 0.5;

        // ── Left panel: Code Editor ──────────────────────────────────────────
        egui::SidePanel::left("code_editor")
            .resizable(true)
            .default_width(left_width)
            .show_inside(ui, |ui| {
                ui.heading("Code Editor");

                ui.add(
                    egui::TextEdit::multiline(&mut self.code)
                        .desired_width(f32::INFINITY)
                        .desired_rows(40),
                );
            });

        // ── Right panel: MCU Configurator ────────────────────────────────────
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.heading("MCU Configurator");

            // ── Chip selector ────────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label("Chip:");

                let prev_type = self.selected_mcu_type.clone();

                egui::ComboBox::from_id_salt("mcu_type_selector")
                    .selected_text(self.selected_mcu_type.label())
                    .show_ui(ui, |ui| {
                        for mcu_type in McuType::all() {
                            let label = if mcu_type.is_supported() {
                                mcu_type.label().to_string()
                            } else {
                                format!("{} — coming soon", mcu_type.label())
                            };

                            ui.selectable_value(
                                &mut self.selected_mcu_type,
                                mcu_type,
                                label,
                            );
                        }
                    });

                // Show CPU architecture family next to the dropdown
                ui.label(
                    egui::RichText::new(self.selected_mcu_type.family())
                        .color(egui::Color32::GRAY)
                        .size(11.0),
                );

                // Re-initialize the MCU when the selection changes
                if prev_type != self.selected_mcu_type {
                    self.mcu = Self::init_mcu(&self.selected_mcu_type);
                }
            });

            ui.separator();

            // ── Tab bar ──────────────────────────────────────────────────────
            ui.horizontal(|ui| {
                let _pins = ui.selectable_label(true, "Pins");
                let _clock = ui.selectable_label(false, "Clock");
                let _peripherals = ui.selectable_label(false, "Peripherals");
                let _system = ui.selectable_label(false, "System");
            });

            ui.separator();

            // ── MCU canvas ───────────────────────────────────────────────────
            egui::ScrollArea::both().show(ui, |ui| {
                match &mut self.mcu {
                    Some(mcu) => mcu.draw(ui),
                    None => {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "⚙  {}  —  support coming soon",
                                    self.selected_mcu_type.label()
                                ))
                                .size(18.0)
                                .color(egui::Color32::GRAY),
                            );
                        });
                    }
                }
            });
        });
    }
}
