//! RTT / defmt tab — live logs through the debug probe via probe-rs.
//! See [`crate::rtt::RttConsole`] for the pipeline.

use super::terminal_tab::render_scrollback;
use crate::app::helpers::help_panel;
use crate::rtt::{RttConsole, RttMode, RttPhase};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Memory key of this tab's help panel.
const HELP_ID: &str = "rtt";

pub fn show_rtt_tab(
    ui: &mut egui::Ui,
    rtt: &mut RttConsole,
    // Set to `Some(mode)` when Run / Attach is clicked; the caller writes the
    // project and starts the session (the `build_go` signal pattern).
    rtt_go: &mut Option<RttMode>,
    // A buildable chip config exists (same gate as Flash / Build).
    can_run: bool,
    // The probe-rs chip name the session will use (shown so a wrong chip is
    // obvious before attaching).
    chip: &str,
    // Shared probe selector (RTT + Debug): the scanned list, the chosen
    // `--probe` selector, a "scan" click signal, and the last scan error.
    probes: &[crate::probe::ProbeInfo],
    selected_probe: &mut Option<String>,
    probe_scan: &mut bool,
    probe_scan_err: Option<&str>,
    // Project chip's toolchain — gates which probes are selectable.
    toolchain: &crate::panels::mcu_module::mcu_catalog::ToolchainKind,
) {
    let phase = rtt.phase();
    let busy = rtt.is_busy();

    // ── Controls row ──────────────────────────────────────────────────────────
    // Wrapped for the same reason as the Debug tab's: an overflowing plain row
    // would keep re-widening the Code Editor side panel (it adopts its
    // content's rect as its width).
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                !busy && can_run,
                egui::Button::new(
                    egui::RichText::new(format!("{} Run (flash + RTT)", ph::PLAY))
                        .size(10.5)
                        .color(if !busy && can_run {
                            egui::Color32::from_rgb(100, 220, 100)
                        } else {
                            egui::Color32::GRAY
                        }),
                ),
            )
            .on_hover_text(
                "cargo build --release, then `probe-rs run`: flash, reset and \
                 stream RTT/defmt logs.\nNeeds probe-rs in PATH and a debug \
                 probe (ST-Link / J-Link / CMSIS-DAP / ESP32 USB-JTAG).",
            )
            .clicked()
        {
            *rtt_go = Some(RttMode::Run);
        }
        if ui
            .add_enabled(
                !busy && can_run,
                egui::Button::new(
                    egui::RichText::new(format!("{} Attach", ph::PLUGS_CONNECTED)).size(10.5),
                ),
            )
            .on_hover_text(
                "`probe-rs attach`: stream RTT from the firmware ALREADY running \
                 on the target — no flash, no reset.\nThe build only provides \
                 the symbols (RTT block address + defmt table), so it must match \
                 what is on the chip.",
            )
            .clicked()
        {
            *rtt_go = Some(RttMode::Attach);
        }
        ui.add_enabled_ui(busy, |ui| {
            if ui
                .button(
                    egui::RichText::new(format!("{} Stop", ph::STOP_CIRCLE))
                        .size(10.5)
                        .color(egui::Color32::from_rgb(230, 120, 110)),
                )
                .clicked()
            {
                rtt.stop();
            }
        });
        if ui
            .button(egui::RichText::new(format!("{} Clear", ph::BROOM)).size(10.5))
            .clicked()
        {
            rtt.clear();
        }

        ui.separator();
        help_panel::toggle_button(ui, HELP_ID);

        ui.separator();
        ui.label(
            egui::RichText::new("Chip:")
                .size(10.5)
                .color(egui::Color32::GRAY),
        );
        ui.label(
            egui::RichText::new(if chip.is_empty() { "—" } else { chip })
                .size(10.5)
                .monospace()
                .color(egui::Color32::from_rgb(120, 160, 200)),
        )
        .on_hover_text("probe-rs chip name — from the selected MCU definition");

        ui.separator();
        super::probe_selector_ui(
            ui,
            probes,
            selected_probe,
            probe_scan,
            probe_scan_err,
            toolchain,
        );

        // Phase status, right-aligned.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (text, color) = match &phase {
                RttPhase::Idle => ("—".to_owned(), egui::Color32::GRAY),
                RttPhase::Building => (
                    "building…".to_owned(),
                    egui::Color32::from_rgb(220, 180, 60),
                ),
                RttPhase::Streaming => (
                    format!("{} streaming", ph::BROADCAST),
                    egui::Color32::from_rgb(80, 200, 100),
                ),
                RttPhase::Error(e) => (
                    format!("{} {}", ph::X_CIRCLE, e.lines().next().unwrap_or("error")),
                    egui::Color32::from_rgb(230, 90, 80),
                ),
            };
            let label = ui.label(egui::RichText::new(text).size(10.5).color(color));
            if let RttPhase::Error(e) = &phase {
                label.on_hover_text(e);
            }
            if busy {
                crate::app::helpers::spinner::throttled_spinner(ui, 12.0);
            }
        });
    });

    // ── Help panel (toggled from the toolbar) ─────────────────────────────────
    help_panel::show_panel(
        ui,
        HELP_ID,
        &[
            (
                "Run (flash + RTT)",
                egui::Color32::from_rgb(100, 220, 100),
                "Builds the project with `cargo build --release`, flashes it to \
                 the chip through the debug probe, resets the target, then \
                 streams its RTT output live. This is the button to use after \
                 changing the code.",
            ),
            (
                "Attach",
                egui::Color32::from_rgb(200, 210, 230),
                "Streams RTT from the firmware ALREADY running on the chip: \
                 nothing is flashed and the target is not reset — use it to \
                 watch a device mid-run. The build still runs because probe-rs \
                 needs the ELF symbols (RTT block address, defmt table), so the \
                 sources must match what is on the chip; otherwise the logs come \
                 out garbled or no RTT block is found.",
            ),
            (
                "Stop",
                egui::Color32::from_rgb(230, 120, 110),
                "Kills probe-rs and releases the debug probe. Only one process \
                 can hold the probe, so stop the RTT session before starting a \
                 Debug session (and the other way round). The firmware keeps \
                 running on the chip.",
            ),
            (
                "Clear",
                egui::Color32::from_gray(170),
                "Empties the log view. It does not touch the target or an \
                 active session.",
            ),
        ],
        &[
            "Requires `probe-rs` in PATH (cargo install probe-rs-tools) and a \
             probe: ST-Link / J-Link / CMSIS-DAP, or the built-in USB-JTAG on \
             ESP32-C3. The Tools tab can install it.",
            "Firmware side: `rtt-target` with `rtt_init_print!()` + \
             `rprintln!(…)`, or `defmt` + `defmt-rtt` — probe-rs decodes defmt \
             frames automatically.",
            "RTT logs travel over the probe, so no USART pin is used and the \
             throughput is far higher than a serial console.",
            "Chip is taken from the selected MCU definition; a wrong chip name \
             makes probe-rs refuse to attach.",
            "Probe: with several probes attached, click Scan and pick one — it \
             passes `--probe VID:PID:Serial` so probe-rs targets that exact \
             probe. Auto lets probe-rs choose (fine with a single probe). The \
             choice is shared with the Debug tab.",
        ],
    );

    ui.separator();

    // ── Log scrollback ────────────────────────────────────────────────────────
    if rtt.state.lock().unwrap().lines.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "RTT — logs through the debug probe (no USART pin needed).\n\
                 • Run (flash + RTT)  — build --release, flash, reset, stream\n\
                 • Attach             — stream from the already-running firmware\n\
                 defmt output is decoded automatically; rtt_target strings pass through.\n\
                 Firmware side: add `rtt-target` (or `defmt` + `defmt-rtt`) and call \
                 `rtt_init_print!()` / `rprintln!(…)`.",
            )
            .size(11.0)
            .color(egui::Color32::GRAY),
        );
        return;
    }
    render_scrollback(ui, &rtt.state, "rtt_scroll", ui.available_height());
}
