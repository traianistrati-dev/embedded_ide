// A GUI app in EVERY profile: Windows never allocates a console for it, so no
// black window pops up at startup — debug included. Debug used to keep the
// console just to see `println!`; `build::attach_parent_console()` (called first
// thing in `main`) buys that back without the window, by adopting the console we
// were launched from when there is one.
//
// The other half: every child process we spawn is given CREATE_NO_WINDOW (see
// `build::no_window`) so none of them flash a console either. Both halves are
// needed — the subsystem alone makes children flash WORSE, since they no longer
// have a parent console to inherit.
#![windows_subsystem = "windows"]

use eframe::egui;
pub mod activity;
pub mod app;
use app::AppIde;

pub mod build;
pub mod debugger;
pub mod dfu;
pub mod editor;
pub mod espflash;
pub mod failure_hint;
pub mod flamegraph;
pub mod git;
pub mod lsp;
pub mod msvc;
pub mod openocd;
pub mod panels;
pub mod probe;
pub mod probe_flash;
pub mod profile;
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
    // FIRST, before anything can print: adopt the console we were launched from
    // (if any). A GUI-subsystem binary has no standard handles until this runs.
    build::attach_parent_console();

    // Resolve the MSVC toolchain env off-thread so the first build doesn't pay
    // for the one-off `vcvars64.bat` capture (see `msvc`).
    msvc::warm_up();

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
