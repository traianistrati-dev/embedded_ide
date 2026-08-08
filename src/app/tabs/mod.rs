//! UI tabs for diagnostics and MCU configuration panels.

pub mod activity_tab;
pub mod cargo_tab;
pub mod clippy_tab;
pub mod debug_tab;
pub mod dfu_tab;
pub mod git_tab;
pub mod mcu_tab;
pub mod profile_tab;
pub mod ra_tab;
pub mod rtt_tab;
pub mod serial_tab;
pub mod terminal_tab;
pub mod tools_tab;

// ── Feature gating on missing tools ──────────────────────────────────────────

/// `true` when `tool` is in the CONFIRMED-unavailable list (see
/// [`crate::required_tools::ToolsState::unavailable`]) — i.e. a button that
/// shells out to it should be greyed out. An empty list (the normal case, and
/// also the state before the startup check finishes) never gates anything.
pub(crate) fn tool_missing(missing_tools: &[&'static str], tool: &str) -> bool {
    missing_tools.iter().any(|m| *m == tool)
}

/// Standard hover text for a button disabled because its tool is absent — one
/// wording everywhere, always naming the fix.
pub(crate) fn needs_tool_hint(tool: &str) -> String {
    format!("Needs `{tool}` — it is missing. Install it from the Tools tab.")
}

// Re-export all tab functions for convenience
pub use activity_tab::show_activity_tab;
pub use cargo_tab::show_cargo_tab;
pub use clippy_tab::show_clippy_tab;
pub use debug_tab::show_debug_tab;
pub use dfu_tab::show_dfu_tab;
pub use git_tab::show_git_tab;
pub use mcu_tab::show_peripherals_tab;
pub use profile_tab::show_profile_tab;
pub use ra_tab::show_ra_tab;
pub use rtt_tab::show_rtt_tab;
pub use serial_tab::show_serial_tab;
pub use terminal_tab::show_terminal_tab;
pub use tools_tab::show_tools_tab;

use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use eframe::egui;
use egui_phosphor::regular as ph;

/// Whether a probe of `kind` (as `probe-rs list` reports it — "ST-LINK",
/// "EspJtag", "JLink", "CMSIS-DAP", …) can drive the project chip's toolchain,
/// the same gate the Flash tab applies to programmers. ARM chips use SWD probes
/// (ST-Link / J-Link / CMSIS-DAP); ESP chips use the built-in USB-JTAG (or a
/// J-Link in JTAG mode). SDCC / 8051 isn't a probe-rs target at all.
pub(crate) fn probe_compatible(kind: &str, toolchain: &ToolchainKind) -> bool {
    let k = kind.to_ascii_lowercase();
    let is_jlink = k.contains("jlink") || k.contains("j-link");
    let is_arm_swd =
        k.contains("st-link") || k.contains("stlink") || k.contains("cmsis") || is_jlink;
    let is_esp_jtag = k.contains("esp") || k.contains("jtag");
    match toolchain {
        ToolchainKind::RustEmbedded => is_arm_swd,
        ToolchainKind::EspRust => is_esp_jtag || is_jlink,
        ToolchainKind::SdccC => false,
    }
}

/// The shared probe picker rendered on both the RTT and Debug tabs (both drive
/// probe-rs). `Scan` re-runs `probe-rs list`; the ComboBox pins the session to
/// one probe via `--probe VID:PID[:Serial]`, or "Auto" to let probe-rs choose.
/// Probes incompatible with the project chip's toolchain are shown greyed and
/// can't be selected (like the Flash tab's programmer list). Meant to sit
/// inside a `horizontal_wrapped` toolbar row.
pub(crate) fn probe_selector_ui(
    ui: &mut egui::Ui,
    probes: &[crate::probe::ProbeInfo],
    selected: &mut Option<String>,
    // Set true when the user clicks Scan; the caller runs `scan_probes`.
    scan_go: &mut bool,
    scan_err: Option<&str>,
    toolchain: &ToolchainKind,
) {
    probe_selector_ui_with(
        ui,
        "Probe:",
        probes,
        selected,
        scan_go,
        scan_err,
        toolchain,
        |_| {},
    );
}

/// [`probe_selector_ui`] with a caller-chosen label and room for one more button
/// between Scan and the list — the Flash tab puts its "Flash (probe-rs)" there,
/// so its Probe row reads like the Programmer row above it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn probe_selector_ui_with(
    ui: &mut egui::Ui,
    label: &str,
    probes: &[crate::probe::ProbeInfo],
    selected: &mut Option<String>,
    scan_go: &mut bool,
    scan_err: Option<&str>,
    toolchain: &ToolchainKind,
    after_scan: impl FnOnce(&mut egui::Ui),
) {
    if !label.is_empty() {
        ui.label(
            egui::RichText::new(label)
                .size(10.5)
                .color(egui::Color32::GRAY),
        );
    }

    if ui
        .button(egui::RichText::new(format!("{} Scan", ph::MAGNIFYING_GLASS)).size(10.5))
        .on_hover_text("Enumerate connected debug probes (`probe-rs list`).")
        .clicked()
    {
        *scan_go = true;
    }
    after_scan(ui);

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
        // Never wider than what is left: a widget that out-demands its region
        // re-widens the Code Editor side panel every frame (see `debug_tab`'s
        // pane note). 320 stays the look when there IS room.
        .width(ui.available_width().min(320.0).max(0.0))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(selected.is_none(), "Auto (first found)")
                .clicked()
            {
                *selected = None;
            }
            for p in probes {
                let is_sel = selected.as_deref() == Some(p.selector.as_str());
                let compatible = probe_compatible(&p.kind, toolchain);
                // Incompatible probes are greyed + disabled, like the Flash
                // tab's programmer list.
                let color = if compatible {
                    egui::Color32::from_gray(210)
                } else {
                    egui::Color32::from_gray(90)
                };
                let resp = ui.add_enabled(
                    compatible,
                    egui::Button::selectable(
                        is_sel,
                        egui::RichText::new(p.combo_label()).size(10.5).color(color),
                    ),
                );
                if resp.clicked() && compatible {
                    *selected = Some(p.selector.clone());
                }
                if !compatible {
                    resp.on_hover_text("Not compatible with this chip's toolchain.");
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
        // A tagged failure (a probe-rs crash) carries a whole explanation — the
        // toolbar shows its first line, the full text sits on the hover.
        let plain = crate::failure_hint::strip(err);
        ui.label(
            egui::RichText::new(format!(
                "{} {}",
                ph::WARNING,
                plain.lines().next().unwrap_or(plain)
            ))
            .size(10.5)
            .color(egui::Color32::from_rgb(210, 150, 90)),
        )
        .on_hover_text(plain);
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolchainKind, probe_compatible};

    #[test]
    fn arm_chips_accept_swd_probes_not_esp_jtag() {
        let arm = ToolchainKind::RustEmbedded;
        // Exact strings `probe-rs list` prints for these probes.
        assert!(probe_compatible("ST-LINK", &arm));
        assert!(probe_compatible("JLink", &arm));
        assert!(probe_compatible("CMSIS-DAP", &arm));
        // The ESP built-in USB-JTAG can't debug an ARM chip.
        assert!(!probe_compatible("EspJtag", &arm));
    }

    #[test]
    fn esp_chips_accept_jtag_not_stlink() {
        let esp = ToolchainKind::EspRust;
        assert!(probe_compatible("EspJtag", &esp));
        assert!(probe_compatible("JLink", &esp)); // J-Link JTAG works on ESP too
        assert!(!probe_compatible("ST-LINK", &esp));
        assert!(!probe_compatible("CMSIS-DAP", &esp));
    }

    #[test]
    fn sdcc_has_no_probe_rs_target() {
        let sdcc = ToolchainKind::SdccC;
        assert!(!probe_compatible("ST-LINK", &sdcc));
        assert!(!probe_compatible("EspJtag", &sdcc));
    }
}
