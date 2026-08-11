//! Serial monitor tab — a raw USART/UART console (Phase 1).
//!
//! Port + baud selectors, Connect/Disconnect, a live RX view (text / hex) and a
//! TX line. Reads/writes the host serial port the firmware's USART is wired to
//! (USB-UART bridge / on-board VCP), so data can be seen without an external
//! terminal. See [`crate::serial::SerialMonitor`].

use crate::serial::{
    SEARCH_HIT, SEARCH_HIT2, SerialMonitor, byte_color, frame_ranges, gap_counts, hex_layout_job,
    hex_search_job, parse_hex_search, render_rx_text, seq_color, seq_counts, text_search_job,
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
            let can = !serial.port.is_empty() && (!serial.bridge || !serial.bridge_port.is_empty());
            if ui
                .add_enabled(can, egui::Button::new(format!("{} Connect", ph::PLUGS_CONNECTED)))
                .on_disabled_hover_text(if serial.bridge {
                    "Bridge needs BOTH a device port and a virtual-pair port"
                } else {
                    "Pick a port first"
                })
                .clicked()
            {
                if serial.bridge {
                    serial.connect_bridge(ctx);
                } else {
                    serial.connect(ctx);
                }
            }
        }

        ui.separator();
        // Bridge (MITM): relay a port another application already holds, instead
        // of opening it. Locked while connected — the wiring can't be re-pointed
        // under a live relay.
        ui.add_enabled_ui(!connected, |ui| {
            ui.checkbox(&mut serial.bridge, "Bridge")
                .on_hover_text(
                    "Man-in-the-middle a port another application is using.\n\
                     The app talks to a virtual port, the IDE relays every byte \
                     to the real device and logs both directions.",
                )
                .on_disabled_hover_text("Disconnect first to change the wiring");
        });
        // The explainer stays reachable whether or not Bridge is on — it is
        // what you read to decide whether you need Bridge at all.
        ui.toggle_value(&mut serial.info_on, format!("{} Info", ph::INFO))
            .on_hover_text("How Bridge (MITM) wiring works, with this session's ports");

        ui.separator();
        // Plot view: parse numeric lines into live curves (Arduino Serial
        // Plotter style) — replaces the text/hex view while on.
        if ui
            .checkbox(&mut serial.plot_on, "Plot")
            .on_hover_text(
                "Plot numeric lines as live curves.\n\
                 Formats:  temp:23.4 hum:56   ·   1.0 2.5 -3\n\
                 One line = one sample tick; log lines in between are ignored.",
            )
            .clicked()
            && serial.plot_on
        {
            serial.matrix.on = false; // one special view at a time
        }
        // Matrix view: the newest Find start…Find end payload as a rows×cols
        // grid of N-byte integers (e.g. 1280 B = 20×16×u32 radar frame).
        if ui
            .checkbox(&mut serial.matrix.on, "Matrix")
            .on_hover_text(
                "Show the newest payload between `Find start` and `Find end` \
                 as a 2D matrix of N-byte values.\n\
                 Example: 1280 B payload = 20 rows × 16 values × 4 bytes (u32).",
            )
            .clicked()
            && serial.matrix.on
        {
            serial.plot_on = false;
        }
        ui.checkbox(&mut serial.hex, "Hex");
        // Timestamped view: both directions as blocks, with the gap between
        // them — the only way to read a send→receive latency here.
        ui.checkbox(&mut serial.stamps, "Time")
            .on_hover_text(
                "Show what was SENT and what was RECEIVED as timestamped blocks:\n\
                 >> what this console sent   ·   << what the device answered\n\
                 The `(+N ms)` on a reply is the time since the previous block — the \
                 send→receive latency.\n\n\
                 The clock is when the IDE wrote/read the bytes, not when they hit the \
                 wire: good to milliseconds, not better. Blocks are split by the idle \
                 gap set in Bridge mode.",
            );
        // Number of bytes per repeating sequence to colour (hex mode).
        ui.add_enabled_ui(serial.hex, |ui| {
            ui.label("Seq:");
            ui.add(
                egui::DragValue::new(&mut serial.seq_len)
                    .range(1..=16)
                    .speed(0.1),
            )
            .on_hover_text("Bytes per repeating sequence: each group of N bytes\nis coloured as a unit (same sequence -> same colour).");
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
    // ── Bridge wiring row (only while Bridge is on) ─────────────────────────
    if serial.bridge {
        show_bridge_row(ui, serial, connected);
    }
    ui.separator();

    // ── RX view (fills the space left above the resizable send area) ────────────
    let section_h = ui.available_height();
    // Keep the send area valid for the current panel height (≥ Send button, and
    // leaving ≥ 40px for the RX view).
    let max_tx = (section_h - HANDLE_H - 40.0).max(MIN_TX);
    serial.tx_height = serial.tx_height.clamp(MIN_TX, max_tx);
    let rx_height = (section_h - serial.tx_height - HANDLE_H).max(40.0);

    // ── Plot / Matrix view (replaces the text/hex view while on; the send
    //    area below keeps working, so commands can be sent meanwhile) ─────────
    if serial.info_on {
        // Outranks every other view: it was asked for explicitly, and it is
        // read while nothing is connected.
        let (app_side, ide_side) = match &serial.pair {
            Some(p) => (p.app_side.clone(), p.ide_side.clone()),
            None => (String::new(), serial.bridge_port.clone()),
        };
        super::serial_info::show_bridge_info(
            ui,
            section_h,
            &serial.port,
            &app_side,
            &ide_side,
            !cfg!(windows),
        );
        return;
    }
    if serial.matrix.on {
        // Newest complete Find-start…Find-end payload + how many the buffer
        // holds (the counter makes a live stream visibly tick).
        let (payload, frames_total) = {
            let a = parse_hex_search(&serial.search);
            let b = parse_hex_search(&serial.search2);
            if a.is_empty() || b.is_empty() {
                (None, 0)
            } else {
                let st = serial.state.lock().unwrap();
                let ranges = frame_ranges(&st.rx, &a, &b);
                (
                    ranges.last().map(|&(s, e)| st.rx[s..e].to_vec()),
                    ranges.len(),
                )
            }
        };
        crate::serial_matrix::show_matrix(
            ui,
            &mut serial.matrix,
            payload.as_deref(),
            frames_total,
            rx_height,
        );
    } else if serial.bridge {
        // The bridge log takes the WHOLE section: in relay mode the IDE is not
        // a participant, so there is nothing to send and no send area to leave
        // room for. Injecting bytes is a deliberate non-feature — it would
        // corrupt a conversation the user came here to observe.
        show_bridge_log(ui, serial, section_h);
        return;
    } else if serial.plot_on {
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

/// The Bridge wiring row: which real device, which end of the virtual pair, and
/// — the part everyone gets stuck on — what the OTHER application must open.
fn show_bridge_row(ui: &mut egui::Ui, serial: &mut SerialMonitor, connected: bool) {
    use crate::serial_bridge::{PairProvider, provider, setup_hint};
    ui.add_enabled_ui(!connected, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Pair:").strong());
            match provider() {
                // Unix: the IDE can make the pair itself, so it does.
                PairProvider::Socat => {
                    if ui
                        .button(format!("{} Create pair", ph::PLUS))
                        .on_hover_text("Run socat to create two linked PTYs")
                        .clicked()
                    {
                        serial.create_pair();
                    }
                    if !serial.bridge_port.is_empty() {
                        ui.label(
                            egui::RichText::new(&serial.bridge_port)
                                .monospace()
                                .size(11.0),
                        );
                    }
                }
                // Windows: the pair is a driver resource the user made earlier,
                // but the IDE can LOOK IT UP — asking someone to remember which
                // two COM numbers are mates is the part that goes wrong.
                PairProvider::Com0com => {
                    let pairs = serial.com0com_pairs.clone();
                    let label = match &serial.pair {
                        Some(p) => format!("{} <-> {}", p.ide_side, p.app_side),
                        None => "—".to_owned(),
                    };
                    egui::ComboBox::from_id_salt("bridge_pair_port")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            if pairs.is_empty() {
                                ui.label(
                                    egui::RichText::new("no com0com pair detected")
                                        .size(10.5)
                                        .italics(),
                                );
                            }
                            for (a, b) in &pairs {
                                // The IDE takes B, the other app gets A — an
                                // arbitrary but STABLE split; Swap flips it.
                                if ui.selectable_label(false, format!("{a} <-> {b}")).clicked() {
                                    serial.bridge_port = b.clone();
                                    serial.pair =
                                        Some(crate::serial_bridge::VirtualPair::existing(
                                            b.clone(),
                                            a.clone(),
                                        ));
                                }
                            }
                        });
                    if serial.pair.is_some()
                        && ui
                            .button(ph::ARROWS_LEFT_RIGHT)
                            .on_hover_text("Swap which end of the pair the IDE holds")
                            .clicked()
                    {
                        if let Some(p) = serial.pair.take() {
                            let swapped = crate::serial_bridge::VirtualPair::existing(
                                p.app_side.clone(),
                                p.ide_side.clone(),
                            );
                            serial.bridge_port = swapped.ide_side.clone();
                            serial.pair = Some(swapped);
                        }
                    }
                }
            }
        });
    });
    let hint = setup_hint(serial.pair.as_ref());
    ui.label(
        egui::RichText::new(hint)
            .size(10.5)
            .color(egui::Color32::from_rgb(150, 160, 180)),
    );
}

/// The relayed traffic, newest at the bottom: `>>` app→device, `<<` device→app.
fn show_bridge_log(ui: &mut egui::Ui, serial: &mut SerialMonitor, height: f32) {
    use crate::serial::{DIR_APP, DIR_SENSOR, bridge_log_job};
    ui.horizontal(|ui| {
        ui.colored_label(DIR_APP, ">> app -> device");
        ui.add_space(10.0);
        ui.colored_label(DIR_SENSOR, "<< device -> app");
        ui.add_space(10.0);
        if ui.button("Clear").clicked() {
            serial.state.lock().unwrap().log.clear();
        }
        ui.add_space(10.0);
        ui.checkbox(&mut serial.stamps, "Time").on_hover_text(
            "Prefix each block with its wall clock and the gap since the previous 
             one. The time is when the IDE READ the bytes, not when they hit the 
             wire - good to milliseconds, not better.",
        );
        // The block boundary is a guess about the protocol, so it has to be
        // adjustable while watching the traffic.
        let mut gap = serial.block_gap_ms();
        ui.label("Gap:");
        let resp = ui.add(
            egui::DragValue::new(&mut gap)
                .range(1..=2000)
                .speed(1.0)
                .suffix(" ms"),
        );
        if resp.changed() {
            serial.set_block_gap_ms(gap);
        }
        resp.on_hover_text(
            "Silence that ends a block. Bytes arriving closer than this join the 
             block in progress - a frame delivered in several reads stays one 
             block. Raise it if frames get split, lower it if they run together.",
        );
        // Say so when the view is filtered — an empty log because a filter is
        // on looks exactly like an empty log because nothing is happening.
        if !serial.search.is_empty() || !serial.search2.is_empty() {
            ui.add_space(10.0);
            ui.colored_label(
                SEARCH_HIT,
                format!(
                    "{} filtered to bursts containing Find start / Find end",
                    ph::FUNNEL
                ),
            );
        }
    });
    // The Find fields mean the same thing here as in the RX view, read in the
    // mode you are in: hex mode parses them as byte sequences, text mode takes
    // the typed characters as-is. Same field, no second concept to learn.
    let (a, b) = if serial.hex {
        (
            parse_hex_search(&serial.search),
            parse_hex_search(&serial.search2),
        )
    } else {
        (
            serial.search.as_bytes().to_vec(),
            serial.search2.as_bytes().to_vec(),
        )
    };
    let job = {
        let st = serial.state.lock().unwrap();
        // Bridge: Find FILTERS — the point there is to pull one frame out of
        // someone else's conversation.
        bridge_log_job(&st.log, serial.hex, 12.0, &a, &b, serial.stamps, st.epoch, true)
    };
    egui::ScrollArea::both()
        .id_salt("bridge_log")
        .max_height(height - 24.0)
        .stick_to_bottom(serial.autoscroll)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(job);
        });
}

/// The classic RX view: coloured hex (+ unique-sequences legend) or decoded
/// text, with the Find highlights. Extracted unchanged so the Plot toggle can
/// swap it for the live plotter.
fn show_rx_view(ui: &mut egui::Ui, serial: &mut SerialMonitor, rx_height: f32) {
    // ── Timed view ────────────────────────────────────────────────────────────
    // With "Time" on, the console shows the same block log the Bridge does —
    // BOTH directions, each stamped, with the gap since the previous block. That
    // gap on a `<<` line right after a `>>` line IS the send→receive latency,
    // which the raw byte stream cannot express: it has no notion of when
    // anything arrived, or of who said it.
    if serial.stamps {
        let job = {
            let st = serial.state.lock().unwrap();
            crate::serial::bridge_log_job(
                &st.log,
                serial.hex,
                12.0,
                &parse_hex_search(&serial.search),
                &parse_hex_search(&serial.search2),
                true,
                st.epoch,
                // Find HIGHLIGHTS here, it does not filter — same as the plain
                // hex view. Keeping only matching blocks would hide the reply
                // whose latency you are reading, and would blank the pane
                // entirely while a pattern matches nothing yet.
                false,
            )
        };
        egui::ScrollArea::both()
            .id_salt("serial_timed_log")
            .stick_to_bottom(serial.autoscroll)
            .auto_shrink([false, false])
            .max_height(rx_height)
            .show(ui, |ui| {
                ui.add(egui::Label::new(job).selectable(true));
            });
        return;
    }

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
