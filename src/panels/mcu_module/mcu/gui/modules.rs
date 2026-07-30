//! Render virtual modules (e.g. GI_USART) and their wires beside the chip on the
//! Pins canvas — a simplified schematic. Read-only (add/remove is in the Pins
//! tab toolbar; config is the Module panel).

use super::super::model::{Mcu, PIN_HEIGHT, PIN_SPACING, PIN_WIDTH};
use crate::panels::mcu_module::modules::model::hz_label;
use crate::panels::mcu_module::modules::{
    ApiStyle, AsyncBusMode, ModuleConfig, ModuleKind, ModuleSignal, Parity, StopBits, VirtualModule,
};
use crate::panels::mcu_module::codegen::sanitize_label;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
use eframe::egui;
use egui_phosphor::regular as ph;
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
        ModuleKind::GenericInterfaceCan => PinFunction::CanTx,
        ModuleKind::GenericInterfaceUsb => PinFunction::UsbDp,
    };
    f.color()
}

/// Place a box just beyond `side`, centred on `along` (the pins' centroid along
/// the side axis) but **nudged forward** past `cursor` so same-side boxes never
/// overlap. `cursor` tracks the trailing edge of the previously placed box on
/// this side and is advanced here. Call with boxes pre-sorted by `along`.
fn packed_rect(chip_rect: egui::Rect, side: Side, along: f32, cursor: &mut f32) -> egui::Rect {
    match side {
        Side::Right | Side::Left => {
            let half = BOX_H / 2.0;
            let mut cy = along;
            if cy - half < *cursor + BOX_GAP {
                cy = *cursor + BOX_GAP + half;
            }
            *cursor = cy + half;
            let x = if side == Side::Right {
                chip_rect.right() + PIN_HEIGHT + PIN_GAP
            } else {
                chip_rect.left() - PIN_HEIGHT - PIN_GAP - BOX_W
            };
            egui::Rect::from_min_size(egui::pos2(x, cy - half), egui::vec2(BOX_W, BOX_H))
        }
        Side::Top | Side::Bottom => {
            let half = BOX_W / 2.0;
            let mut cx = along;
            if cx - half < *cursor + BOX_GAP {
                cx = *cursor + BOX_GAP + half;
            }
            *cursor = cx + half;
            let y = if side == Side::Top {
                chip_rect.top() - PIN_HEIGHT - PIN_GAP - BOX_H
            } else {
                chip_rect.bottom() + PIN_HEIGHT + PIN_GAP
            };
            egui::Rect::from_min_size(egui::pos2(cx - half, y), egui::vec2(BOX_W, BOX_H))
        }
    }
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

/// The module name without the `GI_` prefix (e.g. `GI_I2C1` → `I2C1`).
pub fn module_base_name(m: &VirtualModule) -> &str {
    m.name.strip_prefix("GI_").unwrap_or(&m.name)
}

/// Display title for the module list/box: the base name plus the user's **raw**
/// label, verbatim — e.g. `GI_I2C1` + "128x32 display" → `I2C1 - 128x32 display`.
pub fn module_title(m: &VirtualModule) -> String {
    let base = module_base_name(m);
    let label = m.config.custom_label();
    if label.trim().is_empty() {
        base.to_owned()
    } else {
        format!("{base} - {label}")
    }
}

/// Live preview of the generated handle variable name(s), with the user's
/// label appended — the module analogue of the pin's `pc13_out_board_led`
/// caption. SPI/I2C have one handle; USART has two (`_txN` / `_rxN`).
fn handle_preview(m: &VirtualModule) -> String {
    let n = m.instance();
    let lbl = sanitize_label(m.config.custom_label());
    let sfx = if lbl.is_empty() {
        String::new()
    } else {
        format!("_{lbl}")
    };
    match m.kind {
        // USART: Native returns the split `(Tx, Rx)` handles → two names; Portable
        // (and the async embedded-io bridge) returns one value → `_serialN`.
        ModuleKind::GenericInterfaceUsart => {
            let native = matches!(&m.config, ModuleConfig::Usart(c) if c.api_style == ApiStyle::Native);
            if native {
                format!("_tx{n}{sfx}, _rx{n}{sfx}")
            } else {
                format!("_serial{n}{sfx}")
            }
        }
        ModuleKind::GenericInterfaceSpi => format!("_spi{n}{sfx}"),
        ModuleKind::GenericInterfaceI2c => format!("_i2c{n}{sfx}"),
        ModuleKind::GenericInterfaceCan => format!("_can{n}{sfx}"),
        ModuleKind::GenericInterfaceUsb => format!("usb_dev{sfx}, serial{sfx}"),
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
        module_base_name(m),
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
    // Live preview of the resulting variable name(s) above the rename field —
    // updates as the user types (same as the pin's `pc13_out_board_led`).
    // Clipped to the box so a long label can't overflow the border.
    painter.with_clip_rect(rect).text(
        egui::pos2(rect.left() + 10.0, rect.bottom() - 26.0),
        egui::Align2::LEFT_BOTTOM,
        handle_preview(m),
        egui::FontId::proportional(9.0),
        egui::Color32::from_rgb(140, 140, 150),
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
    // ── 1. Classify each module: connected (side + along-axis centroid) or
    //       floating (no wired pins). `conns` keeps the wire endpoints.
    struct Sided {
        idx: usize,
        conns: Vec<(ModuleSignal, egui::Pos2)>,
        side: Side,
        along: f32,
    }
    let mut sided: Vec<Sided> = Vec::new();
    let mut floating_idx: Vec<usize> = Vec::new();

    for (i, m) in mcu.modules.iter().enumerate() {
        let conns: Vec<(ModuleSignal, egui::Pos2, Side)> = m
            .connections
            .iter()
            .filter_map(|c| {
                pin_anchor_side(mcu, chip_rect, c.mcu_pin).map(|(p, s)| (c.signal, p, s))
            })
            .collect();
        if conns.is_empty() {
            floating_idx.push(i);
            continue;
        }
        let side = dominant_side(&conns);
        // Centroid along the side axis, from the pins actually on that side.
        let on_side: Vec<f32> = conns
            .iter()
            .filter(|(_, _, s)| *s == side)
            .map(|(_, p, _)| match side {
                Side::Top | Side::Bottom => p.x,
                Side::Left | Side::Right => p.y,
            })
            .collect();
        let along = on_side.iter().sum::<f32>() / on_side.len().max(1) as f32;
        let conns2 = conns.iter().map(|(sig, p, _)| (*sig, *p)).collect();
        sided.push(Sided { idx: i, conns: conns2, side, along });
    }

    // ── 2. Pack each side independently so same-side boxes never overlap. ──────
    // (module index, rect, conns, side, connected)
    let mut boxes: Vec<(usize, egui::Rect, Vec<(ModuleSignal, egui::Pos2)>, Side, bool)> =
        Vec::new();
    for target in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
        let mut group: Vec<&Sided> = sided.iter().filter(|e| e.side == target).collect();
        group.sort_by(|a, b| a.along.total_cmp(&b.along));
        let mut cursor = f32::MIN;
        for e in group {
            let rect = packed_rect(chip_rect, e.side, e.along, &mut cursor);
            boxes.push((e.idx, rect, e.conns.clone(), e.side, true));
        }
    }
    // Disconnected modules stack in the right margin.
    let mut fy = chip_rect.top();
    for i in floating_idx {
        let min = egui::pos2(chip_rect.right() + PIN_HEIGHT + PIN_GAP, fy);
        fy += BOX_H + BOX_GAP;
        boxes.push((i, egui::Rect::from_min_size(min, egui::vec2(BOX_W, BOX_H)), Vec::new(), Side::Right, false));
    }

    // ── 3. Draw boxes + wires; detect a header click to expand the list entry. ─
    let mut clicked_id: Option<String> = None;
    let mut field_pass: Vec<(usize, egui::Rect)> = Vec::new();
    for (i, rect, conns, side, connected) in &boxes {
        let m = &mcu.modules[*i];
        let inst = m.instance();
        draw_box(painter, *rect, m, *connected, module_color(m.kind, inst));

        for (sig, anchor) in conns {
            let color = signal_color(*sig, inst);
            let term = facing_terminal(*rect, *side, *anchor);
            painter.circle_filled(term, 3.5, color);
            painter.circle_filled(*anchor, 3.5, color);
            painter.line_segment([term, *anchor], egui::Stroke::new(1.6, color));
        }

        // Click the header area (above the rename field) to expand the list entry.
        let header_rect =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.bottom() - 30.0));
        let resp = ui.interact(header_rect, ui.id().with(("vmod_box", *i)), egui::Sense::click());
        if resp.hovered() {
            painter.rect_stroke(
                *rect,
                6.0,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(50)),
                egui::StrokeKind::Middle,
            );
        }
        if resp.clicked() {
            clicked_id = Some(m.id.clone());
        }
        field_pass.push((*i, *rect));
    }

    // ── 4. Rename fields (mutable pass) ───────────────────────────────────────
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

    if clicked_id.is_some() {
        mcu.expand_module = clicked_id;
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
    is_async: bool,
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

    // Portable (embedded-io/hal) vs native (concrete HAL) init — shown for the
    // bus modules (USART/SPI/I2C) that generate a `pins/configs/*.rs` init.
    let api_row = |ui: &mut egui::Ui, style: &mut ApiStyle| {
        ui.label("Init API");
        egui::ComboBox::from_id_salt("api_style")
            .selected_text(match style {
                ApiStyle::Portable => "Portable (embedded-io/hal)",
                ApiStyle::Native => "Native (HAL type)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(style, ApiStyle::Portable, "Portable (embedded-io/hal)")
                    .on_hover_text(
                        "init returns a STANDARD embedded-io / embedded-hal 1.0 value — \
                         portable driver code across HALs. Its `.0` still gives the raw \
                         HAL object back.",
                    );
                ui.selectable_value(style, ApiStyle::Native, "Native (HAL type)")
                    .on_hover_text(
                        "init returns the concrete stm32f1xx-hal type — no bridge, no extra \
                         trait crates, max HAL features.",
                    );
            });
        ui.end_row();
    };

    // Async runtime only (SPI/I2C): blocking embassy driver vs async-DMA.
    let async_row = |ui: &mut egui::Ui, mode: &mut AsyncBusMode| {
        ui.label("Async init");
        egui::ComboBox::from_id_salt("async_mode")
            .selected_text(match mode {
                AsyncBusMode::Blocking => "Blocking (embedded-hal 1.0)",
                AsyncBusMode::AsyncDma => "Async-DMA (embedded-hal-async)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(mode, AsyncBusMode::Blocking, "Blocking (embedded-hal 1.0)")
                    .on_hover_text(
                        "embassy new_blocking → a STANDARD blocking embedded-hal 1.0 bus. \
                         No DMA — compiles out of the box. Fine inside an async project.",
                    );
                ui.selectable_value(mode, AsyncBusMode::AsyncDma, "Async-DMA (embedded-hal-async)")
                    .on_hover_text(
                        "embassy DMA new → an .await-able embedded-hal-async bus. Needs DMA \
                         channels: main.rs gets a TODO line to fill with channels valid for \
                         this peripheral on your chip (won't compile until you do).",
                    );
            });
        ui.end_row();
    };

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
                    // Async USART is always the embedded-io-async BufferedUart
                    // bridge (no per-module choice) — hide the blocking selector.
                    if !is_async {
                        api_row(ui, &mut cfg.api_style);
                    }
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
                    if is_async {
                        async_row(ui, &mut cfg.async_mode);
                    } else {
                        api_row(ui, &mut cfg.api_style);
                    }
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
                    if is_async {
                        async_row(ui, &mut cfg.async_mode);
                    } else {
                        api_row(ui, &mut cfg.api_style);
                    }
                }
                ModuleConfig::Can(cfg) => {
                    ui.label("Bit rate");
                    egui::ComboBox::from_id_salt("canbr")
                        .selected_text(format!("{} kbit", cfg.bitrate / 1_000))
                        .show_ui(ui, |ui| {
                            for br in [125_000u32, 250_000, 500_000, 1_000_000] {
                                ui.selectable_value(
                                    &mut cfg.bitrate,
                                    br,
                                    format!("{} kbit", br / 1_000),
                                );
                            }
                        });
                    ui.end_row();
                }
                ModuleConfig::Usb(cfg) => {
                    ui.label("Product");
                    ui.add(
                        egui::TextEdit::singleline(&mut cfg.product)
                            .desired_width(140.0)
                            .hint_text("device name shown to host"),
                    );
                    ui.end_row();
                    ui.label("Vendor ID");
                    ui.add(egui::DragValue::new(&mut cfg.vid).hexadecimal(4, false, true));
                    ui.end_row();
                    ui.label("Product ID");
                    ui.add(egui::DragValue::new(&mut cfg.pid).hexadecimal(4, false, true));
                    ui.end_row();
                }
            }

            for (sig, pin) in &conn_rows {
                ui.label(format!("{sig} {} pin", ph::ARROW_RIGHT));
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
