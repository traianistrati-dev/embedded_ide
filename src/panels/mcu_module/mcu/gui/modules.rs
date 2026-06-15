//! Render virtual modules (e.g. GI_USART) and their wires beside the chip on the
//! Pins canvas — a simplified schematic. Read-only (add/remove is in the Pins
//! tab toolbar; config is the Module panel).

use super::super::model::{Mcu, PIN_HEIGHT, PIN_SPACING, PIN_WIDTH};
use crate::panels::mcu_module::modules::{
    ModuleConfig, ModuleSignal, Parity, StopBits, VirtualModule,
};
use eframe::egui;
use std::collections::HashMap;

const BOX_W: f32 = 170.0;
const BOX_H: f32 = 76.0;
const BOX_GAP: f32 = 14.0;
/// Gap between the pin tips and a module box.
const PIN_GAP: f32 = 18.0;
/// Canvas margin reserved around the chip for modules (so boxes + wires sit
/// beyond the pins without overlapping the chip). Horizontal fits a box's width,
/// vertical its height.
pub const MARGIN_X: f32 = PIN_HEIGHT + PIN_GAP + BOX_W + 24.0;
pub const MARGIN_Y: f32 = PIN_HEIGHT + PIN_GAP + BOX_H + 24.0;

/// Which side of the chip a pin sits on.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Side {
    Right,
    Left,
    Top,
    Bottom,
}

/// Outer connection point of an MCU pin + the side it's on. `None` if the pin
/// isn't on this chip.
pub fn pin_anchor(mcu: &Mcu, chip_rect: egui::Rect, pin_num: usize) -> Option<egui::Pos2> {
    pin_anchor_side(mcu, chip_rect, pin_num).map(|(p, _)| p)
}

fn pin_anchor_side(mcu: &Mcu, chip_rect: egui::Rect, pin_num: usize) -> Option<(egui::Pos2, Side)> {
    let row_y = |i: usize| {
        chip_rect.top() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING) + PIN_WIDTH / 2.0
    };
    let col_x = |i: usize| {
        chip_rect.left() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING) + PIN_WIDTH / 2.0
    };
    if let Some(i) = mcu.right_pins.iter().position(|p| p.number == pin_num) {
        return Some((egui::pos2(chip_rect.right() + PIN_HEIGHT, row_y(i)), Side::Right));
    }
    if let Some(i) = mcu.left_pins.iter().position(|p| p.number == pin_num) {
        return Some((egui::pos2(chip_rect.left() - PIN_HEIGHT, row_y(i)), Side::Left));
    }
    if let Some(i) = mcu.top_pins.iter().position(|p| p.number == pin_num) {
        return Some((egui::pos2(col_x(i), chip_rect.top() - PIN_HEIGHT), Side::Top));
    }
    if let Some(i) = mcu.bottom_pins.iter().position(|p| p.number == pin_num) {
        return Some((egui::pos2(col_x(i), chip_rect.bottom() + PIN_HEIGHT), Side::Bottom));
    }
    None
}

fn signal_color(sig: ModuleSignal) -> egui::Color32 {
    match sig {
        ModuleSignal::Tx => egui::Color32::from_rgb(90, 170, 230), // blue
        ModuleSignal::Rx => egui::Color32::from_rgb(230, 160, 70), // orange
    }
}

/// The box that sits just beyond `side`, near the connected pins' centroid,
/// offset by `idx` so stacked modules on the same side don't overlap.
fn place_box(chip_rect: egui::Rect, side: Side, centroid: egui::Pos2, idx: usize) -> egui::Rect {
    let off = idx as f32;
    let min = match side {
        Side::Right => egui::pos2(
            chip_rect.right() + PIN_HEIGHT + PIN_GAP,
            centroid.y - BOX_H / 2.0 + off * (BOX_H + BOX_GAP),
        ),
        Side::Left => egui::pos2(
            chip_rect.left() - PIN_HEIGHT - PIN_GAP - BOX_W,
            centroid.y - BOX_H / 2.0 + off * (BOX_H + BOX_GAP),
        ),
        Side::Top => egui::pos2(
            centroid.x - BOX_W / 2.0 + off * (BOX_W + BOX_GAP),
            chip_rect.top() - PIN_HEIGHT - PIN_GAP - BOX_H,
        ),
        Side::Bottom => egui::pos2(
            centroid.x - BOX_W / 2.0 + off * (BOX_W + BOX_GAP),
            chip_rect.bottom() + PIN_HEIGHT + PIN_GAP,
        ),
    };
    egui::Rect::from_min_size(min, egui::vec2(BOX_W, BOX_H))
}

/// A terminal point on the box edge that faces the chip, aligned to `anchor`
/// (clamped to the edge), so the wire to the pin is short and doesn't cross the
/// chip.
fn facing_terminal(box_rect: egui::Rect, side: Side, anchor: egui::Pos2) -> egui::Pos2 {
    match side {
        Side::Right => egui::pos2(
            box_rect.left(),
            anchor.y.clamp(box_rect.top() + 8.0, box_rect.bottom() - 8.0),
        ),
        Side::Left => egui::pos2(
            box_rect.right(),
            anchor.y.clamp(box_rect.top() + 8.0, box_rect.bottom() - 8.0),
        ),
        Side::Top => egui::pos2(
            anchor.x.clamp(box_rect.left() + 8.0, box_rect.right() - 8.0),
            box_rect.bottom(),
        ),
        Side::Bottom => egui::pos2(
            anchor.x.clamp(box_rect.left() + 8.0, box_rect.right() - 8.0),
            box_rect.top(),
        ),
    }
}

fn draw_box(painter: &egui::Painter, rect: egui::Rect, m: &VirtualModule, connected: bool) {
    painter.rect_filled(rect, 6.0, egui::Color32::from_rgb(38, 52, 44));
    let stroke = if connected {
        egui::Stroke::new(1.2, egui::Color32::from_rgb(90, 160, 110))
    } else {
        egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 90, 90)) // disconnected
    };
    painter.rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Middle);

    painter.text(
        rect.center_top() + egui::vec2(0.0, 13.0),
        egui::Align2::CENTER_CENTER,
        &m.name,
        egui::FontId::proportional(13.0),
        egui::Color32::WHITE,
    );
    let ModuleConfig::Usart(cfg) = &m.config;
    let summary = if connected {
        format!("USART{}  ·  {} baud", cfg.instance, cfg.baud_rate)
    } else {
        "disconnected".to_owned()
    };
    painter.text(
        rect.center_top() + egui::vec2(0.0, 30.0),
        egui::Align2::CENTER_CENTER,
        summary,
        egui::FontId::proportional(10.0),
        egui::Color32::from_rgb(150, 150, 160),
    );
}

/// Draw each module beside the chip, on the side of the pins it connects to,
/// with short direct wires (no crossing the chip). Fully-disconnected modules
/// float in the right margin.
pub fn draw_modules(mcu: &Mcu, painter: &egui::Painter, chip_rect: egui::Rect) {
    let mut per_side: HashMap<Side, usize> = HashMap::new();
    let mut floating = 0usize;

    for m in &mcu.modules {
        // Connected pins → their anchors + sides.
        let conns: Vec<(ModuleSignal, egui::Pos2, Side)> = m
            .connections
            .iter()
            .filter_map(|c| {
                pin_anchor_side(mcu, chip_rect, c.mcu_pin).map(|(p, s)| (c.signal, p, s))
            })
            .collect();

        if conns.is_empty() {
            // Disconnected — float it in the right margin.
            let min = egui::pos2(
                chip_rect.right() + PIN_HEIGHT + PIN_GAP,
                chip_rect.top() + floating as f32 * (BOX_H + BOX_GAP),
            );
            floating += 1;
            draw_box(painter, egui::Rect::from_min_size(min, egui::vec2(BOX_W, BOX_H)), m, false);
            continue;
        }

        // Dominant side = where most of its pins are.
        let side = dominant_side(&conns);
        let centroid = {
            let on_side: Vec<egui::Pos2> =
                conns.iter().filter(|(_, _, s)| *s == side).map(|(_, p, _)| *p).collect();
            let n = on_side.len().max(1) as f32;
            let sum = on_side.iter().fold(egui::Vec2::ZERO, |a, p| a + p.to_vec2());
            (sum / n).to_pos2()
        };
        let idx = per_side.entry(side).or_insert(0);
        let box_rect = place_box(chip_rect, side, centroid, *idx);
        *idx += 1;

        draw_box(painter, box_rect, m, true);

        for (sig, anchor, _) in &conns {
            let color = signal_color(*sig);
            let term = facing_terminal(box_rect, side, *anchor);
            painter.circle_filled(term, 3.5, color);
            painter.circle_filled(*anchor, 3.5, color);
            painter.line_segment([term, *anchor], egui::Stroke::new(1.6, color));
        }
    }
}

fn dominant_side(conns: &[(ModuleSignal, egui::Pos2, Side)]) -> Side {
    let mut count: HashMap<Side, usize> = HashMap::new();
    for (_, _, s) in conns {
        *count.entry(*s).or_insert(0) += 1;
    }
    // Most common; ties resolved by the first connection's side.
    conns
        .iter()
        .map(|(_, _, s)| *s)
        .max_by_key(|s| count[s])
        .unwrap_or(Side::Right)
}

fn parity_label(p: Parity) -> &'static str {
    match p {
        Parity::None => "None",
        Parity::Even => "Even",
        Parity::Odd => "Odd",
    }
}

fn stop_label(s: StopBits) -> &'static str {
    match s {
        StopBits::One => "1",
        StopBits::Two => "2",
    }
}

/// Configuration panel for one module: USART comm settings, the wired TX/RX
/// pins, and the user's RX/TX data model. `pin_names` maps pin number → name.
pub fn module_config_ui(
    ui: &mut egui::Ui,
    m: &mut VirtualModule,
    pin_names: &HashMap<usize, String>,
) {
    let tx = m.pin_for(ModuleSignal::Tx);
    let rx = m.pin_for(ModuleSignal::Rx);
    let name_of = |p: Option<usize>| {
        p.and_then(|n| pin_names.get(&n).cloned())
            .unwrap_or_else(|| "—".to_owned())
    };
    let ModuleConfig::Usart(cfg) = &mut m.config;

    egui::Grid::new("module_cfg")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Baud rate");
            egui::ComboBox::from_id_salt("baud")
                .selected_text(cfg.baud_rate.to_string())
                .show_ui(ui, |ui| {
                    for b in [9600u32, 19200, 38400, 57600, 115200, 230400, 460800, 921600] {
                        ui.selectable_value(&mut cfg.baud_rate, b, b.to_string());
                    }
                });
            ui.end_row();

            ui.label("Data bits");
            egui::ComboBox::from_id_salt("databits")
                .selected_text(cfg.data_bits.to_string())
                .show_ui(ui, |ui| {
                    for d in [8u8, 9] {
                        ui.selectable_value(&mut cfg.data_bits, d, d.to_string());
                    }
                });
            ui.end_row();

            ui.label("Parity");
            egui::ComboBox::from_id_salt("parity")
                .selected_text(parity_label(cfg.parity))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.parity, Parity::None, "None");
                    ui.selectable_value(&mut cfg.parity, Parity::Even, "Even");
                    ui.selectable_value(&mut cfg.parity, Parity::Odd, "Odd");
                });
            ui.end_row();

            ui.label("Stop bits");
            egui::ComboBox::from_id_salt("stop")
                .selected_text(stop_label(cfg.stop_bits))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut cfg.stop_bits, StopBits::One, "1");
                    ui.selectable_value(&mut cfg.stop_bits, StopBits::Two, "2");
                });
            ui.end_row();

            ui.label("TX → pin");
            ui.label(name_of(tx));
            ui.end_row();
            ui.label("RX → pin");
            ui.label(name_of(rx));
            ui.end_row();
        });

    ui.add_space(4.0);
    let label = |ui: &mut egui::Ui, t: &str| {
        ui.label(
            egui::RichText::new(t)
                .size(11.0)
                .color(egui::Color32::from_rgb(150, 150, 160)),
        );
    };
    label(ui, "RX data model");
    ui.add(
        egui::TextEdit::multiline(&mut cfg.rx_model)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .code_editor()
            .hint_text("data the device sends — e.g. struct Reading { temp: f32, .. }"),
    );
    label(ui, "TX data model");
    ui.add(
        egui::TextEdit::multiline(&mut cfg.tx_model)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .code_editor()
            .hint_text("data you send to the device — e.g. command frames"),
    );
}
