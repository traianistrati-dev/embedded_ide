//! Serial monitor tab — a raw USART/UART console (Phase 1).
//!
//! Port + baud selectors, Connect/Disconnect, a live RX view (text / hex) and a
//! TX line. Reads/writes the host serial port the firmware's USART is wired to
//! (USB-UART bridge / on-board VCP), so data can be seen without an external
//! terminal. See [`crate::serial::SerialMonitor`].

use crate::serial::{
    SEARCH_HIT, SEARCH_HIT2, SerialMonitor, byte_color, hex_layout_job, hex_search_job,
    parse_hex_search, render_rx_text, seq_color, seq_counts,
};
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
        // Number of bytes per repeating sequence to colour (hex mode).
        ui.add_enabled_ui(serial.hex, |ui| {
            ui.label("Seq:");
            ui.add(
                egui::DragValue::new(&mut serial.seq_len)
                    .range(1..=16)
                    .speed(0.1),
            )
            .on_hover_text("Bytes per repeating sequence: each group of N bytes\nis coloured as a unit (same sequence → same colour).");
        });
        // Two hex search fields → highlight matches in yellow / blue, grey rest.
        ui.add_enabled_ui(serial.hex, |ui| {
            ui.colored_label(SEARCH_HIT, "Find:");
            ui.add(
                egui::TextEdit::singleline(&mut serial.search)
                    .hint_text("hex e.g. 0D 0A")
                    .desired_width(110.0),
            )
            .on_hover_text("Highlight this hex sequence in yellow (rest greyed).");
            ui.colored_label(SEARCH_HIT2, "Find:");
            ui.add(
                egui::TextEdit::singleline(&mut serial.search2)
                    .hint_text("hex e.g. 4F 4E")
                    .desired_width(110.0),
            )
            .on_hover_text("Highlight this hex sequence in blue (rest greyed).");
        });
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

    // ── RX view (fills the space left above the resizable send area) ────────────
    const HANDLE_H: f32 = 6.0;
    const MIN_TX: f32 = 26.0;
    let section_h = ui.available_height();
    // Keep the send area valid for the current panel height (≥ Send button, and
    // leaving ≥ 40px for the RX view).
    let max_tx = (section_h - HANDLE_H - 40.0).max(MIN_TX);
    serial.tx_height = serial.tx_height.clamp(MIN_TX, max_tx);
    let rx_height = (section_h - serial.tx_height - HANDLE_H).max(40.0);

    // Build the display under one lock. Search mode → yellow/grey highlight (no
    // legend); hex mode → per-sequence colours + unique-sequences legend; text
    // mode → plain decoded text.
    let hex = serial.hex;
    let seq_len = serial.seq_len.max(1);
    let search_a = parse_hex_search(&serial.search);
    let search_b = parse_hex_search(&serial.search2);
    let searching = hex && (!search_a.is_empty() || !search_b.is_empty());
    let mut patterns: Vec<(&[u8], egui::Color32)> = Vec::new();
    if !search_a.is_empty() {
        patterns.push((&search_a, SEARCH_HIT));
    }
    if !search_b.is_empty() {
        patterns.push((&search_b, SEARCH_HIT2));
    }
    let (hex_job, text_display, counts) = {
        let st = serial.state.lock().unwrap();
        if hex {
            // Search highlight (yellow/blue) when a Find field is filled, else
            // the per-sequence colouring. The unique-sequence legend is always
            // computed so it stays visible even while searching.
            let job = if searching {
                hex_search_job(&st.rx, 12.0, &patterns)
            } else {
                hex_layout_job(&st.rx, 12.0, seq_len)
            };
            (Some(job), String::new(), seq_counts(&st.rx, seq_len))
        } else {
            (None, render_rx_text(&st.rx), Vec::new())
        }
    };

    if let Some(job) = hex_job {
        // Coloured hex on the left, unique-sequences legend on the right with a
        // draggable vertical divider. All bounded to `rx_height` so the resize
        // handle + send area below stay visible.
        const DIV_W: f32 = 6.0;
        let max_legend = (ui.available_width() - 160.0).max(80.0);
        serial.legend_w = serial.legend_w.clamp(80.0, max_legend);
        let legend_w = serial.legend_w;
        let hex_w = (ui.available_width() - legend_w - DIV_W - 8.0).max(120.0);
        ui.horizontal_top(|ui| {
            egui::ScrollArea::both()
                .id_salt("serial_rx_hex")
                .stick_to_bottom(serial.autoscroll)
                .auto_shrink([false, false])
                .max_height(rx_height)
                .max_width(hex_w)
                .show(ui, |ui| {
                    ui.add(egui::Label::new(job).selectable(true));
                });

            // Draggable vertical divider — resize the legend width.
            let (div_rect, _) =
                ui.allocate_exact_size(egui::vec2(DIV_W, rx_height), egui::Sense::hover());
            let div = ui.interact(
                div_rect,
                ui.id().with("serial_legend_resize"),
                egui::Sense::drag(),
            );
            let div_color = if div.hovered() || div.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                egui::Color32::from_rgb(100, 140, 200)
            } else {
                egui::Color32::from_gray(70)
            };
            let cx = div_rect.center().x;
            ui.painter()
                .vline(cx, div_rect.y_range(), egui::Stroke::new(1.5, div_color));
            for dy in [-6.0_f32, 0.0, 6.0] {
                ui.painter().circle_filled(
                    egui::pos2(cx, div_rect.center().y + dy),
                    1.5,
                    div_color,
                );
            }
            if div.dragged() {
                // Drag left → legend grows; right → shrinks.
                serial.legend_w = (serial.legend_w - div.drag_delta().x).clamp(80.0, max_legend);
            }

            egui::ScrollArea::both()
                .id_salt("serial_legend")
                .auto_shrink([false, false])
                .max_height(rx_height)
                .max_width(legend_w)
                .show(ui, |ui| {
                    // Force a vertical list — the scroll area inherits the parent
                    // `horizontal_top` layout, which would otherwise flow the rows
                    // left-to-right.
                    ui.vertical(|ui| {
                        let title = if seq_len == 1 {
                            "Unique bytes".to_owned()
                        } else {
                            format!("Unique {seq_len}-byte seq")
                        };
                        ui.label(
                            egui::RichText::new(title)
                                .size(10.0)
                                .color(egui::Color32::GRAY),
                        );
                        for (seq, count) in counts.iter().take(96) {
                            ui.horizontal(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(11.0, 11.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(rect, 2.0, seq_color(seq));
                                let hex: String = seq
                                    .iter()
                                    .map(|b| format!("{b:02X}"))
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                let ascii: String = seq
                                    .iter()
                                    .map(|&b| {
                                        if (0x20..0x7f).contains(&b) {
                                            b as char
                                        } else {
                                            '·'
                                        }
                                    })
                                    .collect();
                                ui.label(
                                    egui::RichText::new(format!("{hex}  {ascii} ×{count}"))
                                        .monospace()
                                        .size(11.0),
                                );
                            });
                        }
                    });
                });
        });
    } else {
        egui::ScrollArea::vertical()
            .id_salt("serial_rx_text")
            .stick_to_bottom(serial.autoscroll)
            .auto_shrink([false, false])
            .max_height(rx_height)
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(text_display).monospace().size(12.0))
                        .selectable(true)
                        .wrap(),
                );
            });
    }

    // ── Drag handle — resize the send area up / down ────────────────────────────
    let (handle_rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HANDLE_H),
        egui::Sense::hover(),
    );
    let drag = ui.interact(
        handle_rect,
        ui.id().with("serial_tx_resize"),
        egui::Sense::drag(),
    );
    let line_color = if drag.hovered() || drag.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        egui::Color32::from_rgb(100, 140, 200)
    } else {
        egui::Color32::from_gray(70)
    };
    let mid_y = handle_rect.center().y;
    ui.painter().hline(
        handle_rect.x_range(),
        mid_y,
        egui::Stroke::new(1.5, line_color),
    );
    for dx in [-6.0_f32, 0.0, 6.0] {
        ui.painter().circle_filled(
            egui::pos2(handle_rect.center().x + dx, mid_y),
            1.5,
            line_color,
        );
    }
    if drag.dragged() {
        // Dragging up (negative delta) grows the send area.
        serial.tx_height = (serial.tx_height - drag.drag_delta().y).clamp(MIN_TX, max_tx);
    }

    // ── Send area — Send + CR+LF pinned right (always visible); the text box
    //    fills the rest and is as tall as the (resizable) send area. ────────────
    let mut do_send = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.checkbox(&mut serial.append_crlf, "CR+LF");
        if ui
            .add_enabled(
                connected,
                egui::Button::new(format!("{} Send", ph::PAPER_PLANE_RIGHT)),
            )
            .clicked()
        {
            do_send = true;
        }
        // In hex mode, colour the typed text per byte (same scheme as the RX
        // view) so repeated chars match the legend colours.
        let mut tx_layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
            let font = egui::FontId::monospace(13.0);
            let mut job = egui::text::LayoutJob::default();
            let s = buf.as_str();
            if hex {
                for c in s.chars() {
                    let col = if c.is_ascii() {
                        byte_color(c as u8)
                    } else {
                        egui::Color32::LIGHT_GRAY
                    };
                    job.append(
                        &c.to_string(),
                        0.0,
                        egui::text::TextFormat::simple(font.clone(), col),
                    );
                }
            } else {
                job.append(
                    s,
                    0.0,
                    egui::text::TextFormat::simple(font.clone(), egui::Color32::from_gray(220)),
                );
            }
            job.wrap.max_width = wrap;
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let resp = ui.add_sized(
            [ui.available_width(), serial.tx_height],
            egui::TextEdit::multiline(&mut serial.tx_input)
                .hint_text("text to send (Ctrl+Enter)")
                .interactive(connected)
                .layouter(&mut tx_layouter),
        );
        // Ctrl+Enter sends; plain Enter inserts a newline (multi-line composing).
        if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command)
        {
            do_send = true;
        }
    });

    if do_send && connected && !serial.tx_input.trim_end_matches(['\r', '\n']).is_empty() {
        for line in serial.tx_input.clone().lines() {
            let line = line.trim();

            if line.is_empty() {
                continue;
            }
            let mut bytes = hex_string_to_bytes(line).unwrap_or_default();
            if serial.append_crlf {
                bytes.extend_from_slice(b"\r\n");
            }
            serial.send(&bytes);
            // serial.tx_input.clear();
        }
    }
}

fn hex_string_to_bytes(s: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    s.split_whitespace()
        .map(|x| u8::from_str_radix(x, 16))
        .collect()
}
