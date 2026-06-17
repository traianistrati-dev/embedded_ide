//! Render virtual modules (e.g. GI_USART) and their wires beside the chip on the
//! Pins canvas — a simplified schematic. Read-only (add/remove is in the Pins
//! tab toolbar; config is the Module panel).

use super::super::model::{Mcu, PIN_HEIGHT, PIN_SPACING, PIN_WIDTH};
use crate::panels::mcu_module::modules::model::hz_label;
use crate::panels::mcu_module::modules::{
    ModuleConfig, ModuleKind, ModuleSignal, Parity, StopBits, VirtualModule,
};
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
use eframe::egui;
use std::collections::HashMap;

const BOX_W: f32 = 170.0;
/// Tall enough for the name, the config summary, and the rename field at the
/// bottom.
const BOX_H: f32 = 98.0;
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

/// Outer connection point of an MCU pin + the unit vector pointing **away** from
/// the chip on that pin's side. `None` if the pin isn't on this chip. Used to
/// draw in/out arrows that extend straight out past the pin.
pub fn pin_anchor_dir(
    mcu: &Mcu,
    chip_rect: egui::Rect,
    pin_num: usize,
) -> Option<(egui::Pos2, egui::Vec2)> {
    pin_anchor_side(mcu, chip_rect, pin_num).map(|(p, s)| {
        let dir = match s {
            Side::Right => egui::vec2(1.0, 0.0),
            Side::Left => egui::vec2(-1.0, 0.0),
            Side::Top => egui::vec2(0.0, -1.0),
            Side::Bottom => egui::vec2(0.0, 1.0),
        };
        (p, dir)
    })
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

/// Wire/terminal colour for a module signal — the **same colour as the MCU pin**
/// it connects to (so the schematic matches the pin colours on the chip).
fn signal_color(sig: ModuleSignal, instance: u8) -> egui::Color32 {
    sig.pin_function(instance).color()
}

/// The module's representative colour = the peripheral's pin colour (USART/SPI/
/// I2C category), used for the box border + title so it matches its pins.
fn module_color(kind: ModuleKind, instance: u8) -> egui::Color32 {
    let f = match kind {
        ModuleKind::GenericInterfaceUsart => PinFunction::UsartTx(instance),
        ModuleKind::GenericInterfaceSpi => PinFunction::SpiSck(instance),
        ModuleKind::GenericInterfaceI2c => PinFunction::I2cScl(instance),
    };
    f.color()
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

/// The rename-field rect at the bottom of a module box (edited in a later pass).
fn label_field_rect(box_rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(box_rect.left() + 10.0, box_rect.bottom() - 24.0),
        egui::pos2(box_rect.right() - 10.0, box_rect.bottom() - 6.0),
    )
}

fn draw_box(
    painter: &egui::Painter,
    rect: egui::Rect,
    m: &VirtualModule,
    connected: bool,
    color: egui::Color32,
) {
    painter.rect_filled(rect, 6.0, egui::Color32::from_rgb(38, 42, 50));
    let stroke = if connected {
        egui::Stroke::new(1.4, color) // border matches the pin colour
    } else {
        egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 90, 90)) // disconnected
    };
    painter.rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Middle);

    let title_color = if connected {
        color
    } else {
        egui::Color32::from_rgb(175, 150, 150)
    };
    painter.text(
        rect.center_top() + egui::vec2(0.0, 13.0),
        egui::Align2::CENTER_CENTER,
        &m.name,
        egui::FontId::proportional(13.0),
        title_color,
    );
    let summary = if connected {
        m.config.summary()
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
    // Caption for the rename field below.
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.bottom() - 26.0),
        egui::Align2::LEFT_BOTTOM,
        "var name",
        egui::FontId::proportional(8.0),
        egui::Color32::from_rgb(120, 120, 130),
    );
}

/// Draw each module beside the chip, on the side of the pins it connects to,
/// with short direct wires (no crossing the chip). Fully-disconnected modules
/// float in the right margin. Each box carries a rename field whose text is
/// appended to the module's generated handle variable (e.g. `_spi1_imu`).
pub fn draw_modules(
    mcu: &mut Mcu,
    painter: &egui::Painter,
    chip_rect: egui::Rect,
    ui: &mut egui::Ui,
) {
    let mut per_side: HashMap<Side, usize> = HashMap::new();
    let mut floating = 0usize;
    // (module index, box rect) collected for the mutable rename-field pass below.
    let mut field_pass: Vec<(usize, egui::Rect)> = Vec::new();

    for (i, m) in mcu.modules.iter().enumerate() {
        let inst = m.instance();
        let mcolor = module_color(m.kind, inst);

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
            let rect = egui::Rect::from_min_size(min, egui::vec2(BOX_W, BOX_H));
            draw_box(painter, rect, m, false, mcolor);
            field_pass.push((i, rect));
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

        draw_box(painter, box_rect, m, true, mcolor);

        for (sig, anchor, _) in &conns {
            let color = signal_color(*sig, inst);
            let term = facing_terminal(box_rect, side, *anchor);
            painter.circle_filled(term, 3.5, color);
            painter.circle_filled(*anchor, 3.5, color);
            painter.line_segment([term, *anchor], egui::Stroke::new(1.6, color));
        }
        field_pass.push((i, box_rect));
    }

    // ── Rename fields (mutable pass) ──────────────────────────────────────────
    // The typed text is appended to the module's generated variable name(s);
    // regenerated every frame by `update_main_rs`, so it updates as you type.
    for (i, box_rect) in field_pass {
        let field_rect = label_field_rect(box_rect);
        let label = mcu.modules[i].config.custom_label_mut();
        ui.push_id(("module_label", i), |ui| {
            ui.put(
                field_rect,
                egui::TextEdit::singleline(label)
                    .hint_text("name")
                    .font(egui::FontId::proportional(10.0)),
            );
        });
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
    // Connection rows (generic over kind), computed before borrowing config.
    let conn_rows: Vec<(&'static str, String)> = m
        .connections
        .iter()
        .map(|c| {
            let pin = pin_names
                .get(&c.mcu_pin)
                .cloned()
                .unwrap_or_else(|| format!("pin{}", c.mcu_pin));
            (c.signal.label(), pin)
        })
        .collect();

    egui::Grid::new("module_cfg")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            match &mut m.config {
                ModuleConfig::Usart(cfg) => {
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
                }
                ModuleConfig::Spi(cfg) => {
                    ui.label("SPI mode");
                    egui::ComboBox::from_id_salt("spimode")
                        .selected_text(format!("Mode {}", cfg.mode))
                        .show_ui(ui, |ui| {
                            for md in 0u8..=3 {
                                ui.selectable_value(&mut cfg.mode, md, format!("Mode {md}"));
                            }
                        });
                    ui.end_row();
                    ui.label("Clock");
                    egui::ComboBox::from_id_salt("spiclk")
                        .selected_text(hz_label(cfg.clock_hz))
                        .show_ui(ui, |ui| {
                            for hz in [125_000u32, 250_000, 500_000, 1_000_000, 2_000_000, 4_000_000, 8_000_000] {
                                ui.selectable_value(&mut cfg.clock_hz, hz, hz_label(hz));
                            }
                        });
                    ui.end_row();
                }
                ModuleConfig::I2c(cfg) => {
                    ui.label("Clock");
                    egui::ComboBox::from_id_salt("i2cclk")
                        .selected_text(hz_label(cfg.clock_hz))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut cfg.clock_hz, 100_000, "100 kHz");
                            ui.selectable_value(&mut cfg.clock_hz, 400_000, "400 kHz");
                        });
                    ui.end_row();
                    ui.label("Address (7-bit)");
                    ui.add(egui::DragValue::new(&mut cfg.address).range(0..=127).hexadecimal(2, false, true));
                    ui.end_row();
                }
            }

            for (sig, pin) in &conn_rows {
                ui.label(format!("{sig} → pin"));
                ui.label(pin);
                ui.end_row();
            }
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
        egui::TextEdit::multiline(m.config.rx_model_mut())
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .code_editor()
            .hint_text("data the device sends — e.g. struct Reading { temp: f32, .. }"),
    );
    label(ui, "TX data model");
    ui.add(
        egui::TextEdit::multiline(m.config.tx_model_mut())
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .code_editor()
            .hint_text("data you send to the device — e.g. command frames"),
    );
}
