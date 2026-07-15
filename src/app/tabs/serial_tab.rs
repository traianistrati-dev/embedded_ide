//! Serial monitor tab — a raw USART/UART console (Phase 1).
//!
//! Port + baud selectors, Connect/Disconnect, a live RX view (text / hex) and a
//! TX line. Reads/writes the host serial port the firmware's USART is wired to
//! (USB-UART bridge / on-board VCP), so data can be seen without an external
//! terminal. See [`crate::serial::SerialMonitor`].

use crate::serial::{
    SEARCH_HIT, SEARCH_HIT2, SerialMonitor, byte_color, gap_counts, hex_layout_job, hex_search_job,
    parse_hex_search, render_rx_text, seq_color, seq_counts, text_search_job,
};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Common baud rates offered in the dropdown.
const BAUDS: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

/// Height of the send-area resize handle / minimum send-area height.
const HANDLE_H: f32 = 6.0;
const MIN_TX: f32 = 26.0;

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
        // Plot view: parse numeric lines into live curves (Arduino Serial
        // Plotter style) — replaces the text/hex view while on.
        ui.checkbox(&mut serial.plot_on, "Plot")
            .on_hover_text(
                "Plot numeric lines as live curves.\n\
                 Formats:  temp:23.4 hum:56   ·   1.0 2.5 -3\n\
                 One line = one sample tick; log lines in between are ignored.",
            );
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
            ui.label("Row:");
            ui.add(
                egui::DragValue::new(&mut serial.row_bytes)
                    .range(1..=64)
                    .speed(0.2),
            )
            .on_hover_text("Bytes shown per line in the hex view.");
        });
        // Search fields. Field 1 works in BOTH views: hex mode highlights the
        // byte sequence in yellow; text mode tints whole LINES that START with
        // the typed text. Field 2 stays hex-only.
        ui.colored_label(SEARCH_HIT, "Find start:");
        ui.add(
            egui::TextEdit::singleline(&mut serial.search)
                .hint_text(if serial.hex { "hex e.g. 0D 0A" } else { "line prefix" })
                .desired_width(110.0),
        )
        .on_hover_text(
            "Hex view: highlight this hex sequence in yellow (rest greyed).\n\
             Text view: lines STARTING with this text turn yellow.",
        );

        // ── Payload size between the two markers ──────────────────────────
        // How many bytes sit BETWEEN Find1 and Find2 (both excluded) — the
        // payload length of each framed message. Hex mode only: that's where
        // both Find fields are byte sequences.
        if serial.hex {
            let a = parse_hex_search(&serial.search);
            let b = parse_hex_search(&serial.search2);
            if !a.is_empty() && !b.is_empty() {
                let gaps = {
                    let st = serial.state.lock().unwrap();
                    gap_counts(&st.rx, &a, &b)
                };
                ui.label(
                    egui::RichText::new("Between:")
                        .size(11.0)
                        .color(egui::Color32::GRAY),
                );
                let (text, color) = match (gaps.last(), gaps.iter().min(), gaps.iter().max()) {
                    (Some(&last), Some(&min), Some(&max)) => (
                        if min == max {
                            format!("{last} B")
                        } else {
                            // Sizes vary across frames — show the spread too.
                            format!("{last} B  ({min}..{max})")
                        },
                        egui::Color32::from_rgb(120, 210, 140),
                    ),
                    _ => ("—".to_owned(), egui::Color32::from_gray(120)),
                };
                ui.label(egui::RichText::new(text).size(11.0).monospace().color(color))
                    .on_hover_text(if gaps.is_empty() {
                        "Bytes between Find start and Find end, both markers excluded.\n\
                         No complete Find start … Find end pair in the buffer yet."
                            .to_owned()
                    } else {
                        format!(
                            "Bytes between Find start and Find end, both markers excluded \
                             (the payload of each framed message).\n\
                             {} frame(s) · last {} B · min {} B · max {} B",
                            gaps.len(),
                            gaps.last().copied().unwrap_or(0),
                            gaps.iter().min().copied().unwrap_or(0),
                            gaps.iter().max().copied().unwrap_or(0),
                        )
                    });
            }
        }

        ui.add_enabled_ui(serial.hex, |ui| {
            ui.colored_label(SEARCH_HIT2, "Find end:");
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
    let section_h = ui.available_height();
    // Keep the send area valid for the current panel height (≥ Send button, and
    // leaving ≥ 40px for the RX view).
    let max_tx = (section_h - HANDLE_H - 40.0).max(MIN_TX);
    serial.tx_height = serial.tx_height.clamp(MIN_TX, max_tx);
    let rx_height = (section_h - serial.tx_height - HANDLE_H).max(40.0);

    // ── Plot view (replaces the text/hex view while on; the send area below
    //    keeps working, so commands can be sent while plotting) ────────────────
    if serial.plot_on {
        {
            let st = serial.state.lock().unwrap();
            serial.plot.feed(&st.rx, st.rx_total);
        }
        crate::serial_plot::show_plot(ui, &mut serial.plot, rx_height);
    } else {
        show_rx_view(ui, serial, rx_height);
    }

    // ── Send area (drag handle + TX line) — shared by both views ────────────────
    show_tx_area(ui, serial, ctx, max_tx);
}

/// The classic RX view: coloured hex (+ unique-sequences legend) or decoded
/// text, with the Find highlights. Extracted unchanged so the Plot toggle can
/// swap it for the live plotter.
fn show_rx_view(ui: &mut egui::Ui, serial: &mut SerialMonitor, rx_height: f32) {
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
                hex_search_job(&st.rx, 12.0, &patterns, serial.row_bytes)
            } else {
                hex_layout_job(&st.rx, 12.0, seq_len, serial.row_bytes)
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
        let needle = serial.search.trim().to_owned();
        egui::ScrollArea::vertical()
            .id_salt("serial_rx_text")
            .stick_to_bottom(serial.autoscroll)
            .auto_shrink([false, false])
            .max_height(rx_height)
            .show(ui, |ui| {
                if needle.is_empty() {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text_display).monospace().size(12.0))
                            .selectable(true)
                            .wrap(),
                    );
                } else {
                    // Find-1 in text mode: whole lines STARTING with the
                    // needle turn yellow, the rest keep the default colour.
                    let job =
                        text_search_job(&text_display, &needle, 12.0, ui.visuals().text_color());
                    ui.add(egui::Label::new(job).selectable(true).wrap());
                }
            });
    }
}

/// The resizable send area: drag handle, TX text box (hex-coloured in hex
/// mode), Send + CR+LF + line-gap pacing. Shared by the RX and Plot views.
fn show_tx_area(ui: &mut egui::Ui, serial: &mut SerialMonitor, ctx: &egui::Context, max_tx: f32) {
    let hex = serial.hex;
    let connected = serial.is_connected();

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
        // Per-line pause (ms) for multi-line command sequences that need the
        // device to settle before the next one. 0 = send back-to-back.
        ui.add(
            egui::DragValue::new(&mut serial.line_delay_ms)
                .range(0..=60_000)
                .speed(10.0)
                .suffix(" ms"),
        )
        .on_hover_text("Pause between each line when sending a multi-line block");
        ui.label("line gap");
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
        // Encode each non-empty line, then queue them so they go out one at a
        // time with the configured `line_gap` pause — non-blocking (paced by
        // `pump_tx_queue` below), so the UI stays responsive during the sequence.
        let lines: Vec<Vec<u8>> = serial
            .tx_input
            .clone()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|line| {
                let mut bytes = hex_string_to_bytes(line).unwrap_or_default();
                if serial.append_crlf {
                    bytes.extend_from_slice(b"\r\n");
                }
                bytes
            })
            .collect();
        serial.queue_lines(lines);
    }

    // Pace the pending TX queue (if any); schedule a repaint when the next line
    // is due so the pause elapses even while the app is otherwise idle.
    if let Some(due_in) = serial.pump_tx_queue() {
        ctx.request_repaint_after(due_in);
    }
}

fn hex_string_to_bytes(s: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    s.split_whitespace()
        .map(|x| u8::from_str_radix(x, 16))
        .collect()
}
