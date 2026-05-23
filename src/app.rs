use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::mcu_catalog::McuType;
use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;
use crate::panels::mcu_module::pin_module::pin::Pin;
use crate::panels::mcu_module::pin_module::pin_function::PinFunction;
use eframe::egui;

// ── Tab bar ──────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug)]
enum McuTab {
    Pins,
    Peripherals,
    Clock,
    System,
}

impl McuTab {
    fn label(&self) -> &str {
        match self {
            McuTab::Pins => "Pins",
            McuTab::Peripherals => "Peripherals",
            McuTab::Clock => "Clock",
            McuTab::System => "System",
        }
    }
}

// ── App state ─────────────────────────────────────────────────────────────────

pub struct AppIde {
    selected_mcu_type: McuType,
    /// None when the selected chip is not yet implemented
    mcu: Option<Mcu>,
    /// Generated Rust HAL code — rebuilt each frame from pin state
    generated_code: String,
    /// Active tab in the MCU configurator
    active_tab: McuTab,
    /// Shown briefly after a successful copy
    copy_flash: u8,
}

impl AppIde {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mcu = create_stm32f103c8tx();
        let generated_code = mcu.generate_code();
        Self {
            selected_mcu_type: McuType::Stm32f103c8t6,
            generated_code,
            mcu: Some(mcu),
            active_tab: McuTab::Pins,
            copy_flash: 0,
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
        // Regenerate code from current pin state each frame
        if let Some(mcu) = &self.mcu {
            self.generated_code = mcu.generate_code();
        }

        // Tick the copy flash counter down
        if self.copy_flash > 0 {
            self.copy_flash -= 1;
        }

        let left_width = ui.available_width() * 0.5;

        // ── Left panel: Code Editor ──────────────────────────────────────────
        egui::Panel::left("code_editor")
            .resizable(true)
            .default_size(left_width)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Code Editor");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Copy button
                        let copy_label = if self.copy_flash > 0 {
                            "✔ Copied!"
                        } else {
                            "⧉ Copy"
                        };
                        let copy_btn = ui.add(egui::Button::new(
                            egui::RichText::new(copy_label).size(11.0),
                        ));
                        if copy_btn.clicked() {
                            ui.output_mut(|o| {
                                o.commands.push(egui::output::OutputCommand::CopyText(
                                    self.generated_code.clone(),
                                ));
                            });
                            self.copy_flash = 60; // ~1 second at 60fps
                        }

                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("auto-generated  •  read-only")
                                .size(10.0)
                                .color(egui::Color32::from_rgb(120, 140, 120)),
                        );
                    });
                });

                ui.separator();

                // Code display — regenerated every frame so edits are discarded
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.generated_code)
                                .desired_width(f32::INFINITY)
                                .desired_rows(50)
                                .code_editor(),
                        );
                    });
            });

        // ── Right panel: MCU Configurator ────────────────────────────────────
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("MCU Configurator");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Reset button
                    let reset_btn = ui
                        .add(egui::Button::new(
                            egui::RichText::new("↺ Reset pins")
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

                            ui.selectable_value(&mut self.selected_mcu_type, mcu_type, label);
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
                    self.active_tab = McuTab::Pins;
                }
            });

            ui.separator();

            // ── Tab bar ──────────────────────────────────────────────────────
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

            // ── Tab content ──────────────────────────────────────────────────
            match self.active_tab {
                McuTab::Pins => {
                    egui::ScrollArea::both().show(ui, |ui| match &mut self.mcu {
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
                    });
                }

                McuTab::Peripherals => {
                    show_peripherals_tab(ui, &self.mcu);
                }

                McuTab::Clock => {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("🕐  Clock configuration — coming soon")
                                .size(16.0)
                                .color(egui::Color32::GRAY),
                        );
                    });
                }

                McuTab::System => {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("⚙  System configuration — coming soon")
                                .size(16.0)
                                .color(egui::Color32::GRAY),
                        );
                    });
                }
            }
        });
    }
}

// ── Peripherals tab ───────────────────────────────────────────────────────────

fn show_peripherals_tab(ui: &mut egui::Ui, mcu: &Option<Mcu>) {
    let Some(mcu) = mcu else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No chip selected.").color(egui::Color32::GRAY));
        });
        return;
    };

    // Collect all configured pins
    let configured: Vec<_> = mcu
        .top_pins
        .iter()
        .chain(mcu.bottom_pins.iter())
        .chain(mcu.left_pins.iter())
        .chain(mcu.right_pins.iter())
        .filter(|p| !p.reserved && p.selected_function != PinFunction::Unset)
        .collect();

    if configured.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No pins configured yet.")
                    .size(14.0)
                    .color(egui::Color32::GRAY),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Switch to the Pins tab and assign functions to pins.")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(120, 120, 130)),
            );
        });
        return;
    }

    // Group by peripheral category
    let mut gpio_in: Vec<_> = vec![];
    let mut gpio_out: Vec<_> = vec![];
    let mut adc: Vec<_> = vec![];
    let mut timers: Vec<_> = vec![];
    let mut usart: Vec<_> = vec![];
    let mut spi: Vec<_> = vec![];
    let mut i2c: Vec<_> = vec![];
    let mut usb: Vec<_> = vec![];
    let mut can: Vec<_> = vec![];
    let mut swd: Vec<_> = vec![];
    let mut other: Vec<_> = vec![];

    for pin in &configured {
        match &pin.selected_function {
            PinFunction::GpioInput => gpio_in.push(*pin),
            PinFunction::GpioOutput => gpio_out.push(*pin),
            PinFunction::AdcChannel { .. } => adc.push(*pin),
            PinFunction::TimerPwm { .. } => timers.push(*pin),
            PinFunction::UsartTx(_)
            | PinFunction::UsartRx(_)
            | PinFunction::UsartCts(_)
            | PinFunction::UsartRts(_)
            | PinFunction::UsartCk(_) => usart.push(*pin),
            PinFunction::SpiNss(_)
            | PinFunction::SpiSck(_)
            | PinFunction::SpiMiso(_)
            | PinFunction::SpiMosi(_) => spi.push(*pin),
            PinFunction::I2cScl(_) | PinFunction::I2cSda(_) => i2c.push(*pin),
            PinFunction::UsbDm | PinFunction::UsbDp => usb.push(*pin),
            PinFunction::CanRx | PinFunction::CanTx => can.push(*pin),
            PinFunction::SwdIo | PinFunction::SwdClk => swd.push(*pin),
            _ => other.push(*pin),
        }
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);

        periph_section(
            ui,
            "GPIO Input",
            &gpio_in,
            egui::Color32::from_rgb(70, 160, 70),
        );
        periph_section(
            ui,
            "GPIO Output",
            &gpio_out,
            egui::Color32::from_rgb(200, 120, 50),
        );
        periph_section(ui, "ADC", &adc, egui::Color32::from_rgb(150, 70, 200));
        periph_section(
            ui,
            "Timers / PWM",
            &timers,
            egui::Color32::from_rgb(190, 170, 30),
        );
        periph_section(ui, "USART", &usart, egui::Color32::from_rgb(50, 110, 200));
        periph_section(ui, "SPI", &spi, egui::Color32::from_rgb(30, 170, 170));
        periph_section(ui, "I2C", &i2c, egui::Color32::from_rgb(60, 180, 100));
        periph_section(ui, "USB", &usb, egui::Color32::from_rgb(190, 50, 160));
        periph_section(ui, "CAN", &can, egui::Color32::from_rgb(200, 130, 20));
        periph_section(
            ui,
            "SWD / Debug",
            &swd,
            egui::Color32::from_rgb(190, 50, 50),
        );
        periph_section(ui, "Other", &other, egui::Color32::GRAY);
    });
}

fn periph_section(ui: &mut egui::Ui, title: &str, pins: &[&Pin], color: egui::Color32) {
    if pins.is_empty() {
        return;
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 16.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, color);
        ui.label(egui::RichText::new(title).size(13.0).strong().color(color));
    });

    let dim = egui::Color32::from_rgb(140, 140, 155);
    egui::Grid::new(format!("periph_grid_{title}"))
        .num_columns(3)
        .spacing([16.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Pin").size(11.0).color(dim));
            ui.label(egui::RichText::new("Name").size(11.0).color(dim));
            ui.label(egui::RichText::new("Function").size(11.0).color(dim));
            ui.end_row();

            for pin in pins {
                ui.label(
                    egui::RichText::new(format!("#{}", pin.number))
                        .size(11.0)
                        .monospace(),
                );
                ui.label(
                    egui::RichText::new(pin.name.as_str())
                        .size(11.0)
                        .monospace()
                        .color(egui::Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new(pin.selected_function.label())
                        .size(11.0)
                        .color(color),
                );
                ui.end_row();
            }
        });

    ui.add_space(2.0);
    ui.separator();
}
