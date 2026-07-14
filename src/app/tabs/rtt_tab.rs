//! RTT / defmt tab — live logs through the debug probe via probe-rs.
//! See [`crate::rtt::RttConsole`] for the pipeline.

use super::terminal_tab::render_scrollback;
use crate::rtt::{RttConsole, RttMode, RttPhase};
use eframe::egui;
use egui_phosphor::regular as ph;

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
) {
    let phase = rtt.phase();
    let busy = rtt.is_busy();

    // ── Controls row ──────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
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
