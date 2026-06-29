//! Serial monitor tab — a raw USART/UART console (Phase 1).
//!
//! Port + baud selectors, Connect/Disconnect, a live RX view (text / hex) and a
//! TX line. Reads/writes the host serial port the firmware's USART is wired to
//! (USB-UART bridge / on-board VCP), so data can be seen without an external
//! terminal. See [`crate::serial::SerialMonitor`].

use crate::serial::{render_rx, SerialMonitor};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Common baud rates offered in the dropdown.
const BAUDS: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

pub fn show_serial_tab(ui: &mut egui::Ui, serial: &mut SerialMonitor, ctx: &egui::Context) {
    if serial.ports.is_empty() {
        serial.refresh_ports();
    }
    let connected = serial.is_connected();

    // ── Controls row ──────────────────────────────────────────────────────────
    ui.horizontal_wrapped(|ui| {
        ui.label("Port:");
        egui::ComboBox::from_id_salt("serial_port")
            .selected_text(if serial.port.is_empty() {
                "—".to_owned()
            } else {
                serial.port.clone()
            })
            .show_ui(ui, |ui| {
                for p in serial.ports.clone() {
                    ui.selectable_value(&mut serial.port, p.clone(), p);
                }
            });
        if ui
            .button(ph::ARROWS_CLOCKWISE)
            .on_hover_text("Refresh ports")
            .clicked()
        {
            serial.refresh_ports();
        }

        ui.add_space(8.0);
        ui.label("Baud:");
        egui::ComboBox::from_id_salt("serial_baud")
            .selected_text(serial.baud.to_string())
            .show_ui(ui, |ui| {
                for b in BAUDS {
                    ui.selectable_value(&mut serial.baud, b, b.to_string());
                }
            });

        ui.add_space(8.0);
        if connected {
            if ui
                .button(format!("{} Disconnect", ph::PLUGS))
                .clicked()
            {
                serial.disconnect();
            }
        } else {
            let can = !serial.port.is_empty();
            if ui
                .add_enabled(can, egui::Button::new(format!("{} Connect", ph::PLUGS_CONNECTED)))
                .clicked()
            {
                serial.connect(ctx);
            }
        }

        ui.separator();
        ui.checkbox(&mut serial.hex, "Hex");
        ui.checkbox(&mut serial.autoscroll, "Autoscroll");
        if ui.button(format!("{} Clear", ph::BROOM)).clicked() {
            serial.clear_rx();
        }
    });

    if let Some(err) = serial.state.lock().unwrap().error.clone() {
        ui.colored_label(
            egui::Color32::from_rgb(220, 90, 80),
            format!("{} {err}", ph::WARNING),
        );
    }
    ui.separator();

    // ── RX view ───────────────────────────────────────────────────────────────
    // Build the display string while holding the lock (avoids cloning the whole
    // buffer); `render_rx` returns only the tail so layout stays cheap.
    let display = {
        let st = serial.state.lock().unwrap();
        render_rx(&st.rx, serial.hex)
    };
    let tx_row_h = ui.spacing().interact_size.y + ui.spacing().item_spacing.y * 2.0;
    let rx_height = (ui.available_height() - tx_row_h).max(40.0);
    egui::ScrollArea::vertical()
        .stick_to_bottom(serial.autoscroll)
        .auto_shrink([false, false])
        .max_height(rx_height)
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(display).monospace().size(12.0))
                    .selectable(true)
                    .wrap(),
            );
        });

    // ── TX line ───────────────────────────────────────────────────────────────
    ui.separator();
    ui.horizontal(|ui| {
        let resp = ui.add_enabled(
            connected,
            egui::TextEdit::singleline(&mut serial.tx_input)
                .hint_text("text to send")
                .desired_width(ui.available_width() - 150.0),
        );
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let send_clicked = ui
            .add_enabled(connected, egui::Button::new(format!("{} Send", ph::PAPER_PLANE_RIGHT)))
            .clicked();
        ui.checkbox(&mut serial.append_crlf, "CR+LF");

        if connected && (send_clicked || enter) && !serial.tx_input.is_empty() {
            let mut bytes = serial.tx_input.clone().into_bytes();
            if serial.append_crlf {
                bytes.extend_from_slice(b"\r\n");
            }
            serial.send(&bytes);
            serial.tx_input.clear();
            resp.request_focus(); // keep typing without re-clicking the field
        }
    });
}
