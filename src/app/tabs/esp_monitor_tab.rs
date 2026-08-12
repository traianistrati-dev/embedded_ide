//! ESP Monitor tab — the device's own output (`esp_println::println!`, panics)
//! streamed from `espflash monitor`. See [`crate::esp_monitor::EspMonitor`].

use super::terminal_tab::render_scrollback;
use crate::app::helpers::help_panel;
use crate::esp_monitor::{EspMonitor, MonitorPhase};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Memory key of this tab's help panel.
const HELP_ID: &str = "esp_monitor";

#[allow(clippy::too_many_arguments)]
pub fn show_esp_monitor_tab(
    ui: &mut egui::Ui,
    monitor: &mut EspMonitor,
    // Set on Start; the caller launches the session (the `build_go` pattern).
    monitor_go: &mut bool,
    // Auto-open after a successful ESP flash — the setting and the toggle.
    auto_open: bool,
    auto_open_set: &mut Option<bool>,
    // An ESP chip config exists (gates Start, same idea as Flash / RTT).
    can_run: bool,
    // The espflash chip name the session will use.
    chip: &str,
    // Port the Serial tab currently holds, if any — a serial port is exclusive,
    // so the monitor cannot attach to the same one.
    serial_port_held: Option<&str>,
    // Tools confirmed missing (see `super::tool_missing`).
    missing_tools: &[&'static str],
) {
    let phase = monitor.phase();
    let busy = monitor.is_busy();
    let no_espflash = super::tool_missing(missing_tools, "espflash");
    let can_run = can_run && !no_espflash;

    // ── Controls row ──────────────────────────────────────────────────────────
    // Wrapped for the same reason as the RTT tab's: an overflowing plain row
    // keeps re-widening the Code Editor side panel.
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                !busy && can_run,
                egui::Button::new(
                    egui::RichText::new(format!("{} Start", ph::PLAY))
                        .size(10.5)
                        .color(if !busy && can_run {
                            egui::Color32::from_rgb(100, 220, 100)
                        } else {
                            egui::Color32::GRAY
                        }),
                ),
            )
            .on_hover_text(
                "`espflash monitor`: attach to the board's serial port and stream \
                 what the firmware prints (esp-println / log).\nIt resets the \
                 target once attached, so the output starts at boot.",
            )
            .on_disabled_hover_text(if no_espflash {
                super::needs_tool_hint("espflash")
            } else {
                "A session is running, or no ESP chip config exists yet.".to_owned()
            })
            .clicked()
        {
            *monitor_go = true;
        }
        ui.add_enabled_ui(busy, |ui| {
            if ui
                .button(
                    egui::RichText::new(format!("{} Stop", ph::STOP_CIRCLE))
                        .size(10.5)
                        .color(egui::Color32::from_rgb(230, 120, 110)),
                )
                .on_hover_text("Kill espflash and release the serial port.")
                .clicked()
            {
                monitor.stop();
            }
        });
        if ui
            .button(egui::RichText::new(format!("{} Clear", ph::BROOM)).size(10.5))
            .clicked()
        {
            monitor.clear();
        }

        ui.separator();
        let mut auto = auto_open;
        if ui
            .checkbox(&mut auto, egui::RichText::new("After flash").size(10.5))
            .on_hover_text(
                "Open this monitor automatically when an ESP flash succeeds.\n\
                 The flash then leaves the chip in reset and the monitor resets \
                 it once attached, so the first println! of main is not missed.",
            )
            .changed()
        {
            *auto_open_set = Some(auto);
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
        .on_hover_text("espflash chip name — from the selected MCU definition");

        let port = monitor.port.lock().unwrap().clone();
        ui.label(
            egui::RichText::new("Port:")
                .size(10.5)
                .color(egui::Color32::GRAY),
        );
        ui.label(
            egui::RichText::new(if port.is_empty() { "auto" } else { &port })
                .size(10.5)
                .monospace()
                .color(egui::Color32::from_rgb(120, 160, 200)),
        )
        .on_hover_text(
            "Taken from the flash that just ran (espflash logs the port it chose), \
             or auto-detected when starting the monitor on its own.",
        );

        // Phase status, right-aligned.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (text, color) = match &phase {
                MonitorPhase::Idle => ("—".to_owned(), egui::Color32::GRAY),
                MonitorPhase::Starting => (
                    "attaching…".to_owned(),
                    egui::Color32::from_rgb(220, 180, 60),
                ),
                MonitorPhase::Streaming => (
                    format!("{} streaming", ph::BROADCAST),
                    egui::Color32::from_rgb(80, 200, 100),
                ),
                MonitorPhase::Error(e) => (
                    format!(
                        "{} {}",
                        ph::X_CIRCLE,
                        e.lines().next().unwrap_or("error")
                    ),
                    egui::Color32::from_rgb(230, 90, 80),
                ),
            };
            let label = ui.label(egui::RichText::new(text).size(10.5).color(color));
            if let MonitorPhase::Error(e) = &phase {
                label.on_hover_text(e);
            }
            if busy {
                crate::app::helpers::spinner::throttled_spinner(ui, 12.0);
            }
        });
    });

    // A serial port is opened exclusively: whoever has it first wins, and the
    // loser's error ("Access is denied" / "Device or resource busy") says
    // nothing about the real cause. Name it up front instead.
    if let Some(held) = serial_port_held {
        ui.label(
            egui::RichText::new(format!(
                "{}  The Serial tab is connected on {held} — disconnect it there, \
                 or this monitor cannot open the port.",
                ph::WARNING
            ))
            .size(10.5)
            .color(egui::Color32::from_rgb(210, 170, 90)),
        );
    }

    // ── Help panel (toggled from the toolbar) ─────────────────────────────────
    help_panel::show_panel(
        ui,
        HELP_ID,
        &[
            (
                "What this shows",
                egui::Color32::from_rgb(200, 210, 230),
                "Whatever the firmware writes with `esp_println::println!` (or the \
                 `log` macros through esp-println), plus panic messages and \
                 exception backtraces. It is the device talking, not the IDE.",
            ),
            (
                "Why not the Serial tab",
                egui::Color32::from_rgb(200, 210, 230),
                "It could read the same bytes, but it connects AFTER the chip was \
                 reset, so the first prints of main are already gone — on a \
                 USB Serial/JTAG board the port even disappears for a second \
                 while it re-enumerates. espflash attaches first and resets the \
                 target itself. It also decodes panic backtrace addresses into \
                 function names using the ELF.",
            ),
            (
                "Use the Serial tab for",
                egui::Color32::from_rgb(200, 210, 230),
                "Sending data TO the device, the live plotter and the frames \
                 view. Only one of the two can hold the port at a time.",
            ),
            (
                "Nothing appears",
                egui::Color32::from_rgb(210, 170, 90),
                "esp-println picks its output at RUNTIME on the ESP32-C3: USB \
                 Serial/JTAG when a USB host is attached, otherwise UART0 on \
                 GPIO20/21. If the board is reached through a UART bridge and \
                 GPIO21 (U0TXD) was assigned another function on the Pins \
                 canvas, the prints have nowhere to go.",
            ),
        ],
        &[
            "The monitor holds the port for as long as it runs — Stop it before \
             flashing again by hand, or before connecting the Serial tab.",
        ],
    );

    ui.separator();
    let out_h = (ui.available_height() - 4.0).max(60.0);
    render_scrollback(ui, &monitor.state, "esp_monitor_scroll", out_h);
}
