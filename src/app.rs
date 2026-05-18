use eframe::egui;

pub struct AppIde {
    code: String,
}

impl AppIde {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            code: String::from("// main.rs"),
        }
    }
}

impl eframe::App for AppIde {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let screen_width = ctx.available_rect().width();
        let left_width = screen_width * 0.5;

        egui::SidePanel::left("code_editor")
            .resizable(true)
            .default_width(left_width)
            .show(ctx, |ui| {
                ui.heading("Code Editor");

                ui.add(
                    egui::TextEdit::multiline(&mut self.code)
                        .desired_width(f32::INFINITY)
                        .desired_rows(40),
                );
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("MCU Configurator");

            ui.horizontal(|ui| {
                let pins = ui.selectable_label(true, "Pins");
                let clock = ui.selectable_label(false, "Clock");
                let periferals = ui.selectable_label(false, "Peripherals");
                let system = ui.selectable_label(false, "System");
            });

            ui.separator();

            ui.label("MCU Canvas");

            crate::panels::mcu::mock_mcu::draw_mock_mcu_stm32f103c8tx(ui);
        });
    }
}

//use eframe::egui;
