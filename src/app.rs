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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let screen_width = ctx.content_rect().width();
        let left_width = screen_width * 0.5;

        egui::Panel::left("code_editor")
            .resizable(true)
            .default_size(left_width)
            .show_inside(ui, |ui| {
                ui.heading("Code Editor");

                ui.add(
                    egui::TextEdit::multiline(&mut self.code)
                        .desired_width(f32::INFINITY)
                        .desired_rows(40),
                );
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.heading("MCU Configurator");

            ui.horizontal(|ui| {
                let _pins = ui.selectable_label(true, "Pins");
                let _clock = ui.selectable_label(false, "Clock");
                let _periferals = ui.selectable_label(false, "Peripherals");
                let _system = ui.selectable_label(false, "System");
            });

            ui.separator();

            ui.label("MCU Canvas");

            crate::panels::mcu_module::mock_mcu::draw_mock_mcu_stm32f103c8tx(ui);
        });
    }
}

//use eframe::egui;
