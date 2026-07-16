//! Serial matrix view — the payload framed by Find start / Find end laid out
//! as a rows × cols grid of N-byte integers. Phase 3 of the Serial monitor.
//!
//! The motivating format: a radar frame of 4 B header + 1280 B data + 4 B
//! tail, where the data is 20 rows × 16 gate energies × u32 — the matrix
//! shows the 20×16 numbers live, one grid per received frame. Row count,
//! values per row and bytes per value are all user fields (min 1), so any
//! fixed-layout binary payload can be inspected the same way.
//!
//! The frame source is [`crate::serial::frame_ranges`] over the RX buffer —
//! the same delimiting the "Between" counter uses; the LAST complete payload
//! is decoded each frame (live), or a frozen snapshot while paused (the live
//! bytes may leave the RX ring while reading numbers).

use eframe::egui;
use egui_phosphor::regular as ph;

/// Values-cell cap actually laid out — a huge rows×cols setting must not
/// allocate hundreds of thousands of egui rects per frame.
const MAX_CELLS: usize = 4096;

/// State of the matrix view (owned by `SerialMonitor`).
pub struct MatrixView {
    /// `true` → the RX area shows the matrix (exclusive with the plotter).
    pub on: bool,
    /// Grid height, in value rows (min 1).
    pub rows: usize,
    /// Values per row (min 1).
    pub cols: usize,
    /// Bytes per value, 1..=8 (4 = u32).
    pub value_bytes: usize,
    /// Payload bytes IGNORED before the first value (min 0) — for frames
    /// whose data is preceded by status/length fields (e.g. 1 status byte +
    /// 2 distance bytes before the gate energies).
    pub skip_bytes: usize,
    /// Little-endian decode (big-endian when false).
    pub le: bool,
    /// Hex display (decimal when false).
    pub hex: bool,
    /// Heatmap cell tint (value vs the frame's max).
    pub heat: bool,
    /// Freeze the current frame (new payloads are ignored while set).
    pub paused: bool,
    /// The payload snapshot shown while paused.
    frozen: Option<Vec<u8>>,
}

impl Default for MatrixView {
    fn default() -> Self {
        Self {
            on: false,
            rows: 20,
            cols: 16,
            value_bytes: 4,
            skip_bytes: 0,
            le: true,
            hex: false,
            heat: true,
            paused: false,
            frozen: None,
        }
    }
}

/// Split `payload` into `value_bytes`-wide unsigned integers. A trailing
/// partial value is dropped; the width is clamped to 1..=8 (u64).
pub fn decode_values(payload: &[u8], value_bytes: usize, le: bool) -> Vec<u64> {
    let vb = value_bytes.clamp(1, 8);
    payload
        .chunks_exact(vb)
        .map(|c| {
            let mut v: u64 = 0;
            if le {
                for (i, &byte) in c.iter().enumerate() {
                    v |= (byte as u64) << (8 * i);
                }
            } else {
                for &byte in c {
                    v = (v << 8) | byte as u64;
                }
            }
            v
        })
        .collect()
}

/// Heatmap tint: 0 → cold dark blue, the frame's max → red. Transparent when
/// the whole frame is zeros (no information to colour).
fn heat_color(v: u64, max: u64) -> egui::Color32 {
    if max == 0 {
        return egui::Color32::TRANSPARENT;
    }
    let t = (v as f32 / max as f32).clamp(0.0, 1.0);
    egui::Color32::from_rgb(
        (30.0 + t * 175.0) as u8,
        (44.0 + t * 8.0) as u8,
        (92.0 - t * 52.0) as u8,
    )
}

/// Controls row + grid, `height` px tall in total. `live` is the newest
/// complete payload between Find start / Find end (`None` = no complete frame
/// yet, or the Find fields are empty); `frames_total` is how many complete
/// frames the RX buffer currently holds (shown so a live stream is obvious).
pub fn show_matrix(
    ui: &mut egui::Ui,
    m: &mut MatrixView,
    live: Option<&[u8]>,
    frames_total: usize,
    height: f32,
) {
    // ── Controls row ──────────────────────────────────────────────────────────
    ui.horizontal_wrapped(|ui| {
        let pause_label = if m.paused {
            format!("{} Resume", ph::PLAY)
        } else {
            format!("{} Pause", ph::PAUSE)
        };
        if ui
            .selectable_label(m.paused, egui::RichText::new(pause_label).size(10.5))
            .on_hover_text("Freeze the shown frame — new payloads are ignored while paused")
            .clicked()
        {
            m.paused = !m.paused;
            m.frozen = if m.paused { live.map(|p| p.to_vec()) } else { None };
        }
        ui.separator();
        ui.label(egui::RichText::new("Rows:").size(10.5).color(egui::Color32::GRAY));
        ui.add(egui::DragValue::new(&mut m.rows).range(1..=512).speed(0.2))
            .on_hover_text("Matrix height, in value rows (min 1)");
        ui.label(egui::RichText::new("Row values:").size(10.5).color(egui::Color32::GRAY));
        ui.add(egui::DragValue::new(&mut m.cols).range(1..=128).speed(0.2))
            .on_hover_text("Values per row (min 1)");
        ui.label(egui::RichText::new("Value bytes:").size(10.5).color(egui::Color32::GRAY));
        ui.add(egui::DragValue::new(&mut m.value_bytes).range(1..=8).speed(0.1))
            .on_hover_text("Bytes per value: 1 = u8, 2 = u16, 4 = u32, 8 = u64");
        ui.label(egui::RichText::new("Ignore first:").size(10.5).color(egui::Color32::GRAY));
        ui.add(egui::DragValue::new(&mut m.skip_bytes).range(0..=65535).speed(0.2))
            .on_hover_text(
                "Payload bytes skipped before the first matrix value (min 0) — \
                 for status/length fields that precede the data.",
            );
        ui.separator();
        if ui
            .selectable_label(m.le, egui::RichText::new(if m.le { "LE" } else { "BE" }).size(10.5))
            .on_hover_text("Byte order of each value: LE = little-endian (STM32/ESP32), BE = big-endian. Click to flip.")
            .clicked()
        {
            m.le = !m.le;
        }
        ui.checkbox(&mut m.hex, egui::RichText::new("Hex").size(10.5))
            .on_hover_text("Show values in hexadecimal");
        ui.checkbox(&mut m.heat, egui::RichText::new("Heat").size(10.5))
            .on_hover_text("Tint each cell by its value (blue = 0 … red = the frame's max)");

        // Status, right-aligned: which frame is shown + its size.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (text, color) = match (m.paused, live) {
                (true, _) => (
                    format!("{} frozen", ph::SNOWFLAKE),
                    egui::Color32::from_rgb(120, 180, 240),
                ),
                (false, Some(p)) => (
                    format!("frame #{frames_total} · {} B", p.len()),
                    egui::Color32::from_rgb(120, 210, 140),
                ),
                (false, None) => ("—".to_owned(), egui::Color32::GRAY),
            };
            ui.label(egui::RichText::new(text).size(10.5).monospace().color(color));
        });
    });

    // ── Payload resolution (frozen wins while paused) ─────────────────────────
    let payload: Option<&[u8]> = if m.paused {
        // A pause hit before any frame arrived freezes the first one to come.
        if m.frozen.is_none() {
            m.frozen = live.map(|p| p.to_vec());
        }
        m.frozen.as_deref()
    } else {
        live
    };
    let Some(payload) = payload else {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "No complete frame yet.\n\
                 The matrix shows the newest payload BETWEEN `Find start` and \
                 `Find end` (both markers excluded) — fill both fields (hex) \
                 and wait for one full frame.\n\
                 Example: start FD FC FB FA · end 04 03 02 01 · 1280 B payload \
                 = 20 rows × 16 values × 4 bytes.",
            )
            .size(11.0)
            .color(egui::Color32::GRAY),
        );
        return;
    };

    // ── Decode + grid ─────────────────────────────────────────────────────────
    // "Ignore first" trims the leading status/length fields; hover offsets
    // stay ABSOLUTE in the payload (skip included) so they match the Hex view.
    let skip = m.skip_bytes.min(payload.len());
    let payload = &payload[skip..];
    let values = decode_values(payload, m.value_bytes, m.le);
    let vb = m.value_bytes.clamp(1, 8);
    let want = m.rows * m.cols;
    let shown = want.min(values.len()).min(MAX_CELLS);
    let max = values[..shown].iter().copied().max().unwrap_or(0);

    // Cell metrics from the widest text the grid will show.
    let font = egui::FontId::monospace(11.0);
    let fmt = |v: u64| -> String {
        if m.hex {
            format!("{v:0width$X}", width = vb * 2)
        } else {
            v.to_string()
        }
    };
    let widest = (0..shown)
        .map(|i| fmt(values[i]).len())
        .max()
        .unwrap_or(1)
        .max(if m.hex { vb * 2 } else { 3 });
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let cell = egui::vec2(char_w * widest as f32 + 10.0, 17.0);
    let idx_w = char_w * 5.0;

    // Leftover info: payload bytes beyond the grid / grid cells beyond payload.
    let leftover = payload.len() as isize - (want * vb) as isize;

    egui::ScrollArea::both()
        .id_salt("serial_matrix")
        .auto_shrink([false, false])
        .max_height(height - 26.0)
        .show(ui, |ui| {
            for row in 0..m.rows {
                ui.horizontal(|ui| {
                    // Row index, dim, with the row's byte offset on hover.
                    let (r, resp) =
                        ui.allocate_exact_size(egui::vec2(idx_w, cell.y), egui::Sense::hover());
                    ui.painter().text(
                        r.right_center(),
                        egui::Align2::RIGHT_CENTER,
                        format!("{row:>3} "),
                        font.clone(),
                        egui::Color32::from_gray(110),
                    );
                    let row_off = skip + row * m.cols * vb;
                    resp.on_hover_text(format!(
                        "row {row} — payload offset {row_off} (0x{row_off:X})"
                    ));
                    for col in 0..m.cols {
                        let i = row * m.cols + col;
                        let (rect, resp) = ui.allocate_exact_size(cell, egui::Sense::hover());
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        if i >= shown {
                            // Beyond the payload (or the cell cap) — greyed.
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "—",
                                font.clone(),
                                egui::Color32::from_gray(70),
                            );
                            continue;
                        }
                        let v = values[i];
                        if m.heat {
                            ui.painter().rect_filled(
                                rect.shrink(1.0),
                                2.0,
                                heat_color(v, max),
                            );
                        }
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            fmt(v),
                            font.clone(),
                            egui::Color32::from_gray(215),
                        );
                        let off = i * vb;
                        let raw: String = payload[off..off + vb]
                            .iter()
                            .map(|b| format!("{b:02X}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let abs = skip + off;
                        resp.on_hover_text(format!(
                            "[{row}][{col}] = {v}  (0x{v:X})\nbytes {raw} @ offset {abs} (0x{abs:X})"
                        ));
                    }
                });
            }
            if leftover > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "+{leftover} payload byte(s) beyond the {}×{} grid",
                        m.rows, m.cols
                    ))
                    .size(10.0)
                    .color(egui::Color32::from_rgb(200, 160, 80)),
                );
            } else if leftover < 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "payload {} B short of the {}×{}×{} grid",
                        -leftover, m.rows, m.cols, vb
                    ))
                    .size(10.0)
                    .color(egui::Color32::from_gray(120)),
                );
            }
            if want > MAX_CELLS {
                ui.label(
                    egui::RichText::new(format!("showing the first {MAX_CELLS} cells"))
                        .size(10.0)
                        .color(egui::Color32::from_rgb(200, 160, 80)),
                );
            }
        });
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The motivating example: 1280 payload bytes as u32 LE → 320 values,
    /// i.e. exactly a 20 × 16 grid.
    #[test]
    fn radar_payload_decodes_to_20_by_16_u32() {
        let payload: Vec<u8> = (0..1280u32).map(|i| (i % 256) as u8).collect();
        let values = decode_values(&payload, 4, true);
        assert_eq!(values.len(), 320); // = 20 rows × 16 values
        // First value: bytes 00 01 02 03 LE → 0x03020100.
        assert_eq!(values[0], 0x0302_0100);
        // Same bytes BE → 0x00010203.
        assert_eq!(decode_values(&payload, 4, false)[0], 0x0001_0203);
    }

    #[test]
    fn decode_widths_and_partial_tail() {
        // 1-byte values are the bytes themselves.
        assert_eq!(decode_values(&[7, 8, 9], 1, true), vec![7, 8, 9]);
        // u16 LE / BE.
        assert_eq!(decode_values(&[0x34, 0x12], 2, true), vec![0x1234]);
        assert_eq!(decode_values(&[0x34, 0x12], 2, false), vec![0x3412]);
        // A trailing partial value is dropped (5 bytes at width 2 → 2 values).
        assert_eq!(decode_values(&[1, 0, 2, 0, 3], 2, true).len(), 2);
        // u64 max width; wider requests clamp to 8 instead of panicking.
        assert_eq!(decode_values(&[0xFF; 8], 8, true), vec![u64::MAX]);
        assert_eq!(decode_values(&[0xFF; 16], 99, true), vec![u64::MAX; 2]);
        // Zero width clamps to 1.
        assert_eq!(decode_values(&[5], 0, true), vec![5]);
    }

    /// Heat: zero-max frames stay untinted; the gradient's ends are stable.
    #[test]
    fn heat_gradient_endpoints() {
        assert_eq!(heat_color(3, 0), eframe::egui::Color32::TRANSPARENT);
        let cold = heat_color(0, 100);
        let hot = heat_color(100, 100);
        assert!(cold.b() > cold.r(), "cold end is blue");
        assert!(hot.r() > hot.b(), "hot end is red");
    }
}
