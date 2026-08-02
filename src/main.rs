// Release builds are a GUI app: no attached console window. (Debug keeps the
// console so `println!` logging stays visible.) Every child process we spawn
// is given CREATE_NO_WINDOW (see `build::no_window`) so none of them flash a
// console either — without both halves, spawning cargo / rust-analyzer / rustup
// popped ghost windows that stole focus and made the app appear to flicker.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
pub mod activity;
pub mod app;
use app::AppIde;

pub mod build;
pub mod debugger;
pub mod dfu;
pub mod editor;
pub mod espflash;
pub mod git;
pub mod lsp;
pub mod openocd;
pub mod panels;
pub mod probe;
pub mod project_tree;
pub mod required_tools;
pub mod reveal;
pub mod rtt;
pub mod serial;
pub mod serial_matrix;
pub mod serial_plot;
pub mod size;
pub mod terminal;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_maximized(true)
            // Window + taskbar icon while the app runs. The PNG is baked into
            // the exe at compile time — replace assets/icon.png (any size,
            // 256×256 recommended) and rebuild to change it.
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .expect("assets/icon.png must be a valid PNG"),
            ),
        ..Default::default()
    };

    eframe::run_native(
        "Embedded IDE",
        options,
        Box::new(|cc| Ok(Box::new(AppIde::new(cc)))),
    )
}
