//! UI tabs for diagnostics and MCU configuration panels.

pub mod mcu_tab;
pub mod cargo_tab;
pub mod ra_tab;
pub mod clippy_tab;
pub mod debug_tab;
pub mod dfu_tab;
pub mod rtt_tab;
pub mod serial_tab;
pub mod terminal_tab;
pub mod activity_tab;
pub mod tools_tab;
pub mod git_tab;

// Re-export all tab functions for convenience
pub use mcu_tab::show_peripherals_tab;
pub use cargo_tab::show_cargo_tab;
pub use clippy_tab::show_clippy_tab;
pub use ra_tab::show_ra_tab;
pub use debug_tab::show_debug_tab;
pub use dfu_tab::show_dfu_tab;
pub use rtt_tab::show_rtt_tab;
pub use serial_tab::show_serial_tab;
pub use terminal_tab::show_terminal_tab;
pub use activity_tab::show_activity_tab;
pub use tools_tab::show_tools_tab;
pub use git_tab::show_git_tab;

use eframe::egui;
use egui_phosphor::regular as ph;

/// The shared probe picker rendered on both the RTT and Debug tabs (both drive
/// probe-rs). `Scan` re-runs `probe-rs list`; the ComboBox pins the session to
/// one probe via `--probe VID:PID[:Serial]`, or "Auto" to let probe-rs choose.
/// Meant to sit inside a `horizontal_wrapped` toolbar row.
pub(crate) fn probe_selector_ui(
    ui: &mut egui::Ui,
    probes: &[crate::probe::ProbeInfo],
    selected: &mut Option<String>,
    // Set true when the user clicks Scan; the caller runs `scan_probes`.
    scan_go: &mut bool,
    scan_err: Option<&str>,
) {
    ui.label(
        egui::RichText::new("Probe:")
            .size(10.5)
            .color(egui::Color32::GRAY),
    );

    if ui
        .button(egui::RichText::new(format!("{} Scan", ph::MAGNIFYING_GLASS)).size(10.5))
        .on_hover_text("Enumerate connected debug probes (`probe-rs list`).")
        .clicked()
    {
        *scan_go = true;
    }

    // The label for the currently selected probe (or Auto).
    let current = selected
        .as_ref()
        .and_then(|sel| probes.iter().find(|p| &p.selector == sel))
        .map(|p| p.combo_label())
        .or_else(|| {
            // Selected but not in the list (not scanned yet / unplugged): show
            // the raw selector so the choice is still visible.
            selected.as_ref().map(|s| format!("· {s}"))
        })
        .unwrap_or_else(|| "Auto (first found)".to_owned());

    egui::ComboBox::from_id_salt("probe_selector")
        .selected_text(egui::RichText::new(current).size(10.5).monospace())
        .width(320.0)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(selected.is_none(), "Auto (first found)")
                .clicked()
            {
                *selected = None;
            }
            for p in probes {
                let is_sel = selected.as_deref() == Some(p.selector.as_str());
                if ui.selectable_label(is_sel, p.combo_label()).clicked() {
                    *selected = Some(p.selector.clone());
                }
            }
            if probes.is_empty() {
                ui.label(
                    egui::RichText::new("No probes — click Scan.")
                        .size(10.5)
                        .italics()
                        .color(egui::Color32::from_gray(140)),
                );
            }
        })
        .response
        .on_hover_text(
            "Which debug probe to use when several are attached. Auto lets \
             probe-rs pick the only one — ambiguous with more than one.",
        );

    if let Some(err) = scan_err {
        ui.label(
            egui::RichText::new(format!("{} {err}", ph::WARNING))
                .size(10.5)
                .color(egui::Color32::from_rgb(210, 150, 90)),
        );
    }
}
