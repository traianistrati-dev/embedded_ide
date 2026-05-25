use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::mcu_catalog::McuType;
use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;
use crate::panels::mcu_module::pin_module::pin::Pin;
use crate::panels::mcu_module::pin_module::pin_function::PinFunction;
use crate::panels::mcu_module::project_gen::{self, ProjectFiles};
use eframe::egui;
use egui_code_editor::{CodeEditor, ColorTheme, Syntax};

// ── Project file selector ─────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug, Default)]
enum ProjectFileId {
    #[default]
    MainRs,
    CargoToml,
    CargoConfig,
    MemoryX,
    BuildRs,
    GitIgnore,
}

impl ProjectFileId {
    fn label(self) -> &'static str {
        match self {
            Self::MainRs => "src/main.rs",
            Self::CargoToml => "Cargo.toml",
            Self::CargoConfig => ".cargo/config.toml",
            Self::MemoryX => "memory.x",
            Self::BuildRs => "build.rs",
            Self::GitIgnore => ".gitignore",
        }
    }

    fn content<'a>(self, files: &'a ProjectFiles) -> &'a str {
        match self {
            Self::MainRs => &files.main_rs,
            Self::CargoToml => &files.cargo_toml,
            Self::CargoConfig => &files.cargo_config,
            Self::MemoryX => &files.memory_x,
            Self::BuildRs => &files.build_rs,
            Self::GitIgnore => &files.gitignore,
        }
    }

    fn syntax(self) -> Syntax {
        // All .rs files get full Rust highlighting.
        // TOML/memory.x use the same for now (no other built-in syntax available).
        Syntax::rust()
    }
}

// ── Tab bar ──────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Debug)]
enum McuTab {
    Pins,
    Peripherals,
    Clock,
    System,
}

impl McuTab {
    fn label(self) -> &'static str {
        match self {
            Self::Pins => "Pins",
            Self::Peripherals => "Peripherals",
            Self::Clock => "Clock",
            Self::System => "System",
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
    /// Currently selected file in the project tree
    selected_file: ProjectFileId,
    /// Shown briefly after a successful copy
    copy_flash: u8,
    /// >0: show export status message countdown
    export_flash: u8,
    /// Last export result message
    export_msg: String,
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
            selected_file: ProjectFileId::MainRs,
            copy_flash: 0,
            export_flash: 0,
            export_msg: String::new(),
        }
    }

    fn init_mcu(mcu_type: &McuType) -> Option<Mcu> {
        match mcu_type {
            McuType::Stm32f103c8t6 => Some(create_stm32f103c8tx()),
            _ => None,
        }
    }
}

impl eframe::App for AppIde {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // ── Rebuild generated code each frame ────────────────────────────────
        if let Some(mcu) = &self.mcu {
            self.generated_code = mcu.generate_code();
        }

        // Tick flash counters down
        if self.copy_flash > 0 {
            self.copy_flash -= 1;
        }
        if self.export_flash > 0 {
            self.export_flash -= 1;
        }

        // ── Build project files snapshot ─────────────────────────────────────
        // Cheap (pure string fmt); used by both tree panel and code editor.
        let project_files: Option<ProjectFiles> = self
            .selected_mcu_type
            .project_config()
            .map(|cfg| project_gen::build_project_files(&cfg, &self.generated_code));

        // Content to display in the editor (cloned so CodeEditor gets &mut String)
        let mut display_code: String = match &project_files {
            Some(files) => self.selected_file.content(files).to_owned(),
            None => self.generated_code.clone(),
        };
        let display_syntax = self.selected_file.syntax();

        // ── Panel 1: Project Tree ─────────────────────────────────────────────
        egui::Panel::left("project_tree")
            .resizable(true)
            .default_size(200.0)
            .show_inside(ui, |ui| {
                ui.heading("Project");
                ui.separator();

                match (&project_files, self.selected_mcu_type.project_config()) {
                    (Some(_), Some(cfg)) => {
                        show_project_tree(ui, cfg.pkg_name, &mut self.selected_file);
                    }
                    _ => {
                        ui.add_space(12.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new("Export not available\nfor this chip yet.")
                                    .size(11.0)
                                    .color(egui::Color32::GRAY),
                            );
                        });
                    }
                }
            });

        // ── Panel 2: Code Editor ──────────────────────────────────────────────
        let editor_width = ui.available_width() * 0.5;
        egui::Panel::left("code_editor")
            .resizable(true)
            .default_size(editor_width)
            .show_inside(ui, |ui| {
                // Header row
                ui.horizontal(|ui| {
                    ui.heading("Code Editor");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Copy button — copies the currently displayed file
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
                                    display_code.clone(),
                                ));
                            });
                            self.copy_flash = 60;
                        }

                        ui.add_space(4.0);

                        // Export Project button
                        let can_export = project_files.is_some();
                        let export_label: &str = if self.export_flash > 0 {
                            &self.export_msg
                        } else if can_export {
                            "⬇ Export Project"
                        } else {
                            "⬇ Export (N/A)"
                        };

                        let export_color =
                            if self.export_flash > 0 && self.export_msg.starts_with('✔') {
                                egui::Color32::from_rgb(100, 220, 100)
                            } else if self.export_flash > 0 {
                                egui::Color32::from_rgb(230, 100, 80)
                            } else {
                                egui::Color32::WHITE
                            };

                        let export_btn = ui.add_enabled(
                            can_export && self.export_flash == 0,
                            egui::Button::new(
                                egui::RichText::new(export_label)
                                    .size(11.0)
                                    .color(export_color),
                            ),
                        );

                        if export_btn.clicked() {
                            if let Some(config) = self.selected_mcu_type.project_config() {
                                if let Some(dest) = rfd::FileDialog::new()
                                    .set_title("Choose folder for the exported project")
                                    .pick_folder()
                                {
                                    let code = self.generated_code.clone();
                                    match project_gen::write_project(&dest, &config, &code) {
                                        Ok(()) => {
                                            self.export_msg = format!(
                                                "✔  {}",
                                                dest.file_name()
                                                    .and_then(|n| n.to_str())
                                                    .unwrap_or("exported")
                                            );
                                            self.export_flash = 180;
                                        }
                                        Err(e) => {
                                            self.export_msg = format!("✗  {e}");
                                            self.export_flash = 180;
                                        }
                                    }
                                }
                            }
                        }

                        export_btn.on_hover_text(
                            "Exports a complete Cargo project:\n\
                             Cargo.toml · .cargo/config.toml · memory.x · build.rs · src/main.rs",
                        );

                        ui.add_space(8.0);
                        // Show which file is open
                        ui.label(
                            egui::RichText::new(self.selected_file.label())
                                .size(10.0)
                                .color(egui::Color32::from_rgb(120, 160, 200)),
                        );
                    });
                });

                ui.separator();

                CodeEditor::default()
                    .id_source("hal_code_editor")
                    .with_rows(50)
                    .with_fontsize(13.0)
                    .with_theme(ColorTheme::GRUVBOX)
                    .with_numlines(true)
                    .show(ui, &mut display_code, &display_syntax);
            });

        // ── Panel 3: MCU Configurator ─────────────────────────────────────────
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("MCU Configurator");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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

            // Chip selector
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

                ui.label(
                    egui::RichText::new(self.selected_mcu_type.family())
                        .color(egui::Color32::GRAY)
                        .size(11.0),
                );

                if prev_type != self.selected_mcu_type {
                    self.mcu = Self::init_mcu(&self.selected_mcu_type);
                    self.active_tab = McuTab::Pins;
                    self.selected_file = ProjectFileId::MainRs;
                }
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
                McuTab::Peripherals => show_peripherals_tab(ui, &self.mcu),
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

// ── Project tree ──────────────────────────────────────────────────────────────

fn show_project_tree(ui: &mut egui::Ui, pkg_name: &str, selected: &mut ProjectFileId) {
    let dim = egui::Color32::from_rgb(140, 150, 165);
    let hi = egui::Color32::from_rgb(100, 180, 255);
    let normal = egui::Color32::from_rgb(200, 205, 215);

    // Root folder label (non-clickable)
    ui.label(
        egui::RichText::new(format!("{pkg_name}-project/"))
            .size(12.0)
            .strong()
            .color(egui::Color32::WHITE),
    );

    ui.add_space(2.0);

    // Helper: single file row
    let mut file_row = |ui: &mut egui::Ui, indent: f32, name: &str, id: ProjectFileId| {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            let is_sel = *selected == id;
            let color = if is_sel { hi } else { normal };
            let resp = ui.add(
                egui::Label::new(
                    egui::RichText::new(name)
                        .size(11.5)
                        .monospace()
                        .color(color),
                )
                .sense(egui::Sense::click()),
            );
            if resp.clicked() {
                *selected = id;
            }
            if resp.hovered() && !is_sel {
                // Underline on hover via a thin rect under the text
                let r = resp.rect;
                ui.painter().line_segment(
                    [r.left_bottom(), r.right_bottom()],
                    egui::Stroke::new(1.0, dim),
                );
            }
        });
    };

    // ── .cargo/ ───────────────────────────────────────────────────────────────
    egui::CollapsingHeader::new(
        egui::RichText::new(".cargo/")
            .size(11.5)
            .monospace()
            .color(normal),
    )
    .default_open(true)
    .show(ui, |ui| {
        file_row(ui, 8.0, "config.toml", ProjectFileId::CargoConfig);
    });

    // ── src/ ──────────────────────────────────────────────────────────────────
    egui::CollapsingHeader::new(
        egui::RichText::new("src/")
            .size(11.5)
            .monospace()
            .color(normal),
    )
    .default_open(true)
    .show(ui, |ui| {
        file_row(ui, 8.0, "main.rs", ProjectFileId::MainRs);
    });

    // ── Root files ────────────────────────────────────────────────────────────
    ui.add_space(2.0);
    file_row(ui, 4.0, ".gitignore", ProjectFileId::GitIgnore);
    file_row(ui, 4.0, "build.rs", ProjectFileId::BuildRs);
    file_row(ui, 4.0, "Cargo.toml", ProjectFileId::CargoToml);
    file_row(ui, 4.0, "memory.x", ProjectFileId::MemoryX);
}

// ── Peripherals tab ───────────────────────────────────────────────────────────

fn show_peripherals_tab(ui: &mut egui::Ui, mcu: &Option<Mcu>) {
    let Some(mcu) = mcu else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No chip selected.").color(egui::Color32::GRAY));
        });
        return;
    };

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

    let mut gpio_in = vec![];
    let mut gpio_out = vec![];
    let mut adc = vec![];
    let mut timers = vec![];
    let mut usart = vec![];
    let mut spi = vec![];
    let mut i2c = vec![];
    let mut usb = vec![];
    let mut can = vec![];
    let mut swd = vec![];
    let mut other = vec![];

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
