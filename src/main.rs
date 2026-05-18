use eframe::egui;
pub mod app;
use app::AppIde;

pub mod panels;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_maximized(true),

        ..Default::default()
    };

    eframe::run_native(
        "Embedded IDE",
        options,
        Box::new(|cc| Box::new(AppIde::new(cc))),
    )
}
