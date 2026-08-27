//! Render virtual modules (e.g. _USART) and their wires beside the chip on the
//! Pins canvas — a simplified schematic. Read-only (add/remove is in the Pins
//! tab toolbar; config is the Module panel).

use super::super::model::{Mcu, PIN_HEIGHT};
use super::rotate::Rot;
use crate::panels::mcu_module::codegen;
use crate::panels::mcu_module::codegen::dma_map;
use crate::panels::mcu_module::codegen::embassy_async::is_advanced_timer;
use crate::panels::mcu_module::codegen::sanitize_label;
use crate::panels::mcu_module::modules::model::BlockingDma;
use crate::panels::mcu_module::modules::model::hz_label;
use crate::panels::mcu_module::modules::{
    ApiStyle, AsyncBusMode, BREAK_FILTERS, BreakPolarity, CanMode, HspiMode, I2sClockPolarity,
    I2sDirection, I2sFormat, I2sMode, I2sStandard, LcdCamMode, ModuleConfig, ModuleKind,
    ModuleSignal, OspiMemoryType, OspiMode, Parity, ParlIoBitOrder, ParlIoDirection, ParlIoWidth,
    PcntCtrlMode, PcntEdgeMode, PwmCounting, PwmMode, PwmOutput, PwmPolarity, QSPI_MEMORY_SIZES,
    QspiAddressSize, RmtDirection, SaiDataSize, SaiMode, SaiStereoMono, SaiTxRx, SpiBitOrder,
    SpiRole, StopBits, TouchScan, TouchThreshold, UsartDirection, UsartFlow, UsartMode,
    UsartModuleConfig, UsbRole, VirtualModule, XspiMemoryType, XspiMode,
};
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
use eframe::egui;
use egui_phosphor::regular as ph;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const BOX_W: f32 = 170.0;
/// Tall enough for the name, the config summary, and the rename field at the
/// bottom.
const BOX_H: f32 = 98.0;
const BOX_GAP: f32 = 14.0;
/// Height of one pin row inside a Custom module's box (its rename field).
const CUSTOM_ROW_H: f32 = 21.0;

/// Height of a module's box. A Custom module grows by one row per pin, because
/// its pins' rename fields live INSIDE the box, grouped under the module name,
/// instead of floating separately beside the chip.
fn box_h(m: &VirtualModule) -> f32 {
    if m.kind.is_custom() {
        let n = match &m.config {
            ModuleConfig::Custom(c) => c.pins.len(),
            _ => 0,
        };
        BOX_H + n as f32 * CUSTOM_ROW_H
    } else {
        BOX_H
    }
}

/// Every pin whose NAME belongs to a virtual module, so `io_arrows` must not
/// also float a rename field for it beside the chip.
///
/// Two different modules land a pin here, for two different reasons:
///
/// * a **Custom** module draws its pins' rename fields INSIDE its own box,
///   grouped under the module name;
/// * a **Timer** module owns the whole PWM handle (`_pwm1`), and its channel
///   pads are handed straight to that handle's `init`. On embassy they never
///   become a binding at all (`consumed` in the async backend), and on the F1
///   they name a variable `pwm_hz` moves out on the next line — so a field
///   beside the chip would edit a name the user cannot use either way, next to
///   the module's own field, which is the one that counts.
pub fn module_owned_pins(mcu: &Mcu) -> std::collections::HashSet<usize> {
    let mut owned: std::collections::HashSet<usize> = mcu
        .modules
        .iter()
        .filter(|m| m.kind.is_custom())
        .flat_map(|m| match &m.config {
            ModuleConfig::Custom(c) => c.pins.clone(),
            _ => Vec::new(),
        })
        .collect();
    owned.extend(
        mcu.modules
            .iter()
            .filter(|m| m.kind == ModuleKind::GenericInterfaceTimer)
            .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin)),
    );
    owned
}

/// Module path stem of a Custom module's generated file — `custom_<name>` at
/// revision 0, `custom_<name>_<n>` after the n-th **Update**. Every Update lands
/// in a FRESH file; only the current stem is declared in `configs/mod.rs` and
/// called from main.rs, so older revisions sit on disk as history without ever
/// producing a duplicate struct in the build.
pub fn custom_file_stem(m: &VirtualModule) -> String {
    let var = custom_var_name(m);
    match &m.config {
        ModuleConfig::Custom(c) if c.revision > 0 => format!("custom_{var}_{}", c.revision),
        _ => format!("custom_{var}"),
    }
}

/// The prefix every revision of a Custom module's file shares — used to keep the
/// older ones when the project tree prunes config files.
pub fn custom_file_prefix(m: &VirtualModule) -> String {
    format!("custom_{}", custom_var_name(m))
}

/// Fingerprint of a Custom module's pin list — each pin's number plus its
/// `function|label`. Compared against `applied_sig` to decide whether **Update**
/// has anything to do: renaming a pin or flipping it In→Out changes the
/// generated field names just as much as adding one does.
pub fn custom_pins_sig(pins: &[usize], pin_sigs: &HashMap<usize, String>) -> String {
    pins.iter()
        .map(|n| {
            let s = pin_sigs.get(n).map(String::as_str).unwrap_or("?");
            format!("{n}:{s}")
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Width reserved for a Custom module's field labels ("Name:", "Struct"), so the
/// boxes beside them start on one vertical line. Wide enough for the longest of
/// them in bold.
const CUSTOM_LABEL_W: f32 = 52.0;
/// Width of those fields — the same for Name and Struct, so they also END on one
/// line.
pub const CUSTOM_FIELD_W: f32 = 160.0;

/// The left column of a Custom module's `label + field` row: bold, left-aligned,
/// fixed width. `Name:` lives in the module list (`mcu_panel`) and `Struct` in
/// the config grid here — two different containers, so only a shared width keeps
/// their fields aligned.
pub fn custom_field_label(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(CUSTOM_LABEL_W, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(egui::RichText::new(text).strong());
        },
    );
}

/// Rect of the rename field for the `i`-th pin row inside a Custom box.
fn custom_pin_row(box_rect: egui::Rect, i: usize) -> egui::Rect {
    let top = box_rect.top() + 44.0 + i as f32 * CUSTOM_ROW_H;
    egui::Rect::from_min_max(
        egui::pos2(box_rect.left() + 52.0, top),
        egui::pos2(box_rect.right() - 8.0, top + CUSTOM_ROW_H - 4.0),
    )
}
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

/// Half-extents (from the chip centre) needed to keep every DRAGGED module box
/// fully on-canvas — so `Mcu::draw` can grow the painter and the Scene's
/// auto-fit won't clip a box pulled far from the chip. `(0,0)` when nothing is
/// manually placed. A module `pos` is the box top-left offset from the centre.
pub fn dragged_half_extent(mcu: &Mcu) -> egui::Vec2 {
    let mut hx = 0.0_f32;
    let mut hy = 0.0_f32;
    for m in &mcu.modules {
        if m.pos != (0.0, 0.0) {
            hx = hx.max(m.pos.0.abs()).max((m.pos.0 + BOX_W).abs());
            hy = hy.max(m.pos.1.abs()).max((m.pos.1 + box_h(m)).abs());
        }
    }
    egui::vec2(hx, hy)
}

/// Outer connection point of an MCU pin (rotation applied). `None` if the pin
/// isn't on this chip. `chip_rect` is the LOCAL (un-rotated) body; `rot` is the
/// diagram rotation.
pub fn pin_anchor(
    mcu: &Mcu,
    chip_rect: egui::Rect,
    rot: Rot,
    pin_num: usize,
) -> Option<egui::Pos2> {
    pin_anchor_side(mcu, chip_rect, rot, pin_num).map(|(p, _)| p)
}

/// Outer connection point + the unit vector pointing **away** from the chip
/// (rotation applied — a true 45° direction on a diamond). Used to draw in/out
/// arrows straight out past the pin.
pub fn pin_anchor_dir(
    mcu: &Mcu,
    chip_rect: egui::Rect,
    rot: Rot,
    pin_num: usize,
) -> Option<(egui::Pos2, egui::Vec2)> {
    pin_anchor_local(mcu, chip_rect, pin_num)
        .map(|(p, out)| (rot.apply(p), rot.vec(out).normalized()))
}

/// Outer connection point + its **screen** side — the rotated outward direction
/// snapped to the nearest axis, used to place module boxes on a chip edge.
fn pin_anchor_side(
    mcu: &Mcu,
    chip_rect: egui::Rect,
    rot: Rot,
    pin_num: usize,
) -> Option<(egui::Pos2, Side)> {
    pin_anchor_local(mcu, chip_rect, pin_num)
        .map(|(p, out)| (rot.apply(p), side_from_outward(rot.vec(out))))
}

/// LOCAL (un-rotated) outer point + unit outward vector of a pin. `None` if the
/// pin isn't on this chip.
fn pin_anchor_local(
    mcu: &Mcu,
    chip_rect: egui::Rect,
    pin_num: usize,
) -> Option<(egui::Pos2, egui::Vec2)> {
    // The same placement the chip is drawn from — a wire that computed its own
    // would land beside the pin the moment either formula moved.
    super::geometry::pin_geom(mcu, chip_rect, pin_num).map(|g| (g.anchor(), g.outward))
}

/// A fixed-size arrowhead at `to`, pointing along `from → to`. Used on custom
/// module wires to show which way the data flows (the line itself is drawn by
/// the caller). Fixed size — a long wire must not grow a huge head.
fn arrow_head(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, color: egui::Color32) {
    let v = to - from;
    if v.length() < 1.0 {
        return;
    }
    let dir = v.normalized();
    let rot = egui::emath::Rot2::from_angle(std::f32::consts::TAU / 12.0);
    let stroke = egui::Stroke::new(1.6_f32, color);
    painter.line_segment([to, to - 7.0 * (rot * dir)], stroke);
    painter.line_segment([to, to - 7.0 * (rot.inverse() * dir)], stroke);
}

/// Snap a (rotated) outward vector to the nearest chip side.
fn side_from_outward(v: egui::Vec2) -> Side {
    if v.x.abs() >= v.y.abs() {
        if v.x >= 0.0 { Side::Right } else { Side::Left }
    } else if v.y >= 0.0 {
        Side::Bottom
    } else {
        Side::Top
    }
}

/// Wire/terminal colour for a module signal — the **same colour as the MCU pin**
/// it connects to (so the schematic matches the pin colours on the chip).
fn signal_color(sig: ModuleSignal, instance: u8) -> egui::Color32 {
    sig.pin_function(instance).color()
}

/// The module's representative colour = the peripheral's pin colour (USART/SPI/
/// I2C category — instance-independent), used for the box border + title so it
/// The RMT block's source clock, in Hz.
///
/// Not from the Clock tab: esp-hal takes the rate as an argument to `Rmt::new`
/// and every example passes the same one per chip — 32 MHz on the ESP32-H2,
/// 80 MHz everywhere else. It is here so the module can show what one tick
/// actually lasts, which is the number a pulse train is written in.
pub fn rmt_source_hz(family: &str) -> u32 {
    if family == "esp32h2" {
        32_000_000
    } else {
        80_000_000
    }
}

/// matches its pins, plus the list-entry name + the "already added" palette
/// buttons.
pub fn module_color(kind: ModuleKind, instance: u8) -> egui::Color32 {
    let f = match kind {
        ModuleKind::GenericInterfaceUsart => PinFunction::UsartTx(instance),
        ModuleKind::GenericInterfaceLpuart => PinFunction::LpuartTx(instance),
        ModuleKind::GenericInterfaceSpi => PinFunction::SpiSck(instance),
        ModuleKind::GenericInterfaceI2c => PinFunction::I2cScl(instance),
        ModuleKind::GenericInterfaceI2s => PinFunction::I2sCk(instance),
        ModuleKind::GenericInterfaceRmt => PinFunction::RmtChannel(instance),
        ModuleKind::GenericInterfacePcnt => PinFunction::PcntEdge {
            unit: instance,
            channel: 0,
        },
        ModuleKind::GenericInterfaceParlIo => PinFunction::ParlData { lane: 0 },
        ModuleKind::GenericInterfaceParlIoRx => PinFunction::ParlRxData { lane: 0 },
        ModuleKind::GenericInterfaceLcdCam => PinFunction::LcdCamData { lane: 0 },
        ModuleKind::GenericInterfaceCamera => PinFunction::CamData { lane: 0 },
        ModuleKind::GenericInterfaceTouch => PinFunction::TouchPad(0),
        ModuleKind::GenericInterfaceMcpwm => PinFunction::McpwmA {
            unit: instance,
            operator: 0,
        },
        ModuleKind::GenericInterfaceDac => PinFunction::DacOut {
            dac: instance,
            channel: 1,
        },
        ModuleKind::GenericInterfaceSai => PinFunction::SaiSck {
            sai: instance,
            block: 1,
        },
        ModuleKind::GenericInterfaceSdmmc => PinFunction::SdmmcCk { unit: instance },
        ModuleKind::GenericInterfaceQspi => PinFunction::QspiClk,
        ModuleKind::GenericInterfaceOspi => PinFunction::OspiClk { port: instance },
        ModuleKind::GenericInterfaceXspi => PinFunction::XspiClk { port: instance },
        ModuleKind::GenericInterfaceHspi => PinFunction::HspiClk { unit: instance },
        ModuleKind::GenericInterfaceTimer => PinFunction::TimerPwm {
            timer: instance,
            channel: 1,
        },
        ModuleKind::GenericInterfaceCan => PinFunction::CanTx,
        ModuleKind::GenericInterfaceUsb => PinFunction::UsbDp,
        // A custom module has no peripheral colour of its own — a neutral slate
        // reads as "user-authored", matching its square corners.
        ModuleKind::Custom => return egui::Color32::from_rgb(150, 160, 180),
    };
    f.color()
}

/// Place a box just beyond `side`, centred on `along` (the pins' centroid along
/// the side axis) but **nudged forward** past `cursor` so same-side boxes never
/// overlap. `cursor` tracks the trailing edge of the previously placed box on
/// this side and is advanced here. Call with boxes pre-sorted by `along`.
fn packed_rect(
    chip_rect: egui::Rect,
    side: Side,
    along: f32,
    cursor: &mut f32,
    h: f32,
) -> egui::Rect {
    match side {
        Side::Right | Side::Left => {
            let half = h / 2.0;
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
            egui::Rect::from_min_size(egui::pos2(x, cy - half), egui::vec2(BOX_W, h))
        }
        Side::Top | Side::Bottom => {
            let half = BOX_W / 2.0;
            let mut cx = along;
            if cx - half < *cursor + BOX_GAP {
                cx = *cursor + BOX_GAP + half;
            }
            *cursor = cx + half;
            let y = if side == Side::Top {
                chip_rect.top() - PIN_HEIGHT - PIN_GAP - h
            } else {
                chip_rect.bottom() + PIN_HEIGHT + PIN_GAP
            };
            egui::Rect::from_min_size(egui::pos2(cx - half, y), egui::vec2(BOX_W, h))
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
            anchor
                .y
                .clamp(box_rect.top() + 8.0, box_rect.bottom() - 8.0),
        ),
        Side::Left => egui::pos2(
            box_rect.right(),
            anchor
                .y
                .clamp(box_rect.top() + 8.0, box_rect.bottom() - 8.0),
        ),
        Side::Top => egui::pos2(
            anchor
                .x
                .clamp(box_rect.left() + 8.0, box_rect.right() - 8.0),
            box_rect.bottom(),
        ),
        Side::Bottom => egui::pos2(
            anchor
                .x
                .clamp(box_rect.left() + 8.0, box_rect.right() - 8.0),
            box_rect.top(),
        ),
    }
}

/// Nearest point on a box's edge to `target` — the wire terminal for a
/// user-dragged (manually placed) box, whose original `side` no longer implies
/// which edge faces the chip. Clamps `target` into the rect: an anchor outside
/// the box lands on its boundary.
pub fn nearest_edge(rect: egui::Rect, target: egui::Pos2) -> egui::Pos2 {
    egui::pos2(
        target.x.clamp(rect.left(), rect.right()),
        target.y.clamp(rect.top(), rect.bottom()),
    )
}

/// The module name without the `_` prefix (e.g. `_I2C1` → `I2C1`).
pub fn module_base_name(m: &VirtualModule) -> &str {
    m.name.strip_prefix("_").unwrap_or(&m.name)
}

/// Display title for the module list/box: the base name plus the user's **raw**
/// label, verbatim — e.g. `_I2C1` + "128x32 display" → `I2C1 - 128x32 display`.
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
fn handle_preview(m: &VirtualModule, native_forced: bool) -> String {
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
        // `native_forced` is the project-level Native runtime (all peripherals
        // Native regardless of the per-module `api_style`).
        ModuleKind::GenericInterfaceUsart => {
            let native = native_forced
                || matches!(&m.config, ModuleConfig::Usart(c) if c.api_style == ApiStyle::Native);
            if native {
                format!("_tx{n}{sfx}, _rx{n}{sfx}")
            } else {
                format!("_serial{n}{sfx}")
            }
        }
        // `_lpserial`, NOT `_serial`: a chip with both USART1 and LPUART1 would
        // otherwise generate the same binding name twice.
        ModuleKind::GenericInterfaceLpuart => format!("_lpserial{n}{sfx}"),
        ModuleKind::GenericInterfaceSpi => format!("_spi{n}{sfx}"),
        ModuleKind::GenericInterfaceI2c => format!("_i2c{n}{sfx}"),
        ModuleKind::GenericInterfaceI2s => format!("_i2s{n}{sfx}"),
        ModuleKind::GenericInterfaceRmt => format!("_rmt{n}{sfx}"),
        ModuleKind::GenericInterfacePcnt => format!("_pcnt{n}{sfx}"),
        // The port is one driver, whatever the bus is wide.
        ModuleKind::GenericInterfaceParlIo => format!("_parl{sfx}"),
        ModuleKind::GenericInterfaceParlIoRx => format!("_parl_rx{sfx}"),
        ModuleKind::GenericInterfaceLcdCam => format!("_lcd{sfx}"),
        ModuleKind::GenericInterfaceCamera => format!("_cam{sfx}"),
        ModuleKind::GenericInterfaceTouch => format!("_touch{sfx}"),
        // One handle per OUTPUT: each is its own `PwmPin`, and they are set
        // independently. The list is built from what is wired, so an
        // operator with only A gives one name.
        ModuleKind::GenericInterfaceMcpwm => {
            let mut names: Vec<String> = m
                .connections
                .iter()
                .filter_map(|c| match c.signal {
                    ModuleSignal::McpwmA0 => Some(format!("_mcpwm{n}_op0a{sfx}")),
                    ModuleSignal::McpwmB0 => Some(format!("_mcpwm{n}_op0b{sfx}")),
                    ModuleSignal::McpwmA1 => Some(format!("_mcpwm{n}_op1a{sfx}")),
                    ModuleSignal::McpwmB1 => Some(format!("_mcpwm{n}_op1b{sfx}")),
                    ModuleSignal::McpwmA2 => Some(format!("_mcpwm{n}_op2a{sfx}")),
                    ModuleSignal::McpwmB2 => Some(format!("_mcpwm{n}_op2b{sfx}")),
                    _ => None,
                })
                .collect();
            names.sort();
            names.join(", ")
        }
        ModuleKind::GenericInterfaceDac => format!("_dac{n}{sfx}"),
        // One handle per SUB-BLOCK: they are independent streams.
        ModuleKind::GenericInterfaceSai => format!("_sai{n}a{sfx}, _sai{n}b{sfx}"),
        ModuleKind::GenericInterfaceSdmmc => format!("_sd{n}{sfx}"),
        // Single-instance peripheral, so no number in the handle.
        ModuleKind::GenericInterfaceQspi => format!("_qspi{sfx}"),
        ModuleKind::GenericInterfaceOspi => format!("_ospi{n}{sfx}"),
        ModuleKind::GenericInterfaceXspi => format!("_xspi{n}{sfx}"),
        ModuleKind::GenericInterfaceHspi => format!("_hspi{n}{sfx}"),
        ModuleKind::GenericInterfaceTimer => format!("_pwm{n}{sfx}"),
        ModuleKind::GenericInterfaceCan => format!("_can{n}{sfx}"),
        ModuleKind::GenericInterfaceUsb => format!("usb_dev{sfx}, serial{sfx}"),
        // The generated struct is named after the module, so the preview shows
        // the handle the user will bind: `let my_mod = MyMod::new(…)`.
        ModuleKind::Custom => {
            let name = custom_struct_name(m);
            format!("{}: {name}", custom_var_name(m))
        }
    }
}

/// Sanitised snake_case identifier for a custom module's variable, derived from
/// its name (empty / invalid → `custom{instance}`).
pub fn custom_var_name(m: &VirtualModule) -> String {
    let lbl = sanitize_label(m.config.custom_label());
    if lbl.is_empty() {
        format!("custom{}", m.instance())
    } else {
        lbl
    }
}

/// `CamelCase` struct name implied by a module label — the hint shown in the
/// Struct field while the user hasn't typed one.
pub fn derived_struct_name(label: &str, instance: u8) -> String {
    let lbl = sanitize_label(label);
    let base = if lbl.is_empty() {
        format!("custom{instance}")
    } else {
        lbl
    };
    base.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Struct name for a custom module: the user's explicit choice when set,
/// otherwise `CamelCase` of the module name (`temp sensor` → `TempSensor`).
pub fn custom_struct_name(m: &VirtualModule) -> String {
    // An explicit name wins, so renaming the module never renames the struct
    // (and the user's own `impl` blocks keep compiling).
    if let ModuleConfig::Custom(c) = &m.config {
        let explicit = sanitize_label(&c.struct_name);
        if !explicit.is_empty() {
            let mut s = String::new();
            for w in explicit.split('_').filter(|w| !w.is_empty()) {
                let mut ch = w.chars();
                if let Some(f) = ch.next() {
                    s.push_str(&f.to_uppercase().collect::<String>());
                    s.push_str(ch.as_str());
                }
            }
            return s;
        }
    }
    let var = custom_var_name(m);
    var.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
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
    native_forced: bool,
    // `Some(t)` (t in 0..1) while this module is pending deletion — its box
    // background pulses red so it's obvious on the diagram which one is being
    // removed. `None` = normal.
    removing_blink: Option<f32>,
    // The box the user clicked last: white title and a border twice as thick,
    // matching how a selected pin is called out on the chip. EVERY text in the
    // box grows by `SELECTED_TEXT_SCALE`, not just the title.
    selected: bool,
) {
    // Background: normal dark, or a red pulse (dark ↔ red by `t`) while the
    // remove-confirm for this module is open.
    let fill = match removing_blink {
        Some(t) => {
            let lerp = |a: u8, c: u8| (a as f32 + (c as f32 - a as f32) * t).round() as u8;
            egui::Color32::from_rgb(lerp(38, 190), lerp(42, 45), lerp(50, 45))
        }
        None => egui::Color32::from_rgb(38, 42, 50),
    };
    // SQUARE corners mark a user-authored (Custom) module; the peripheral ones
    // stay rounded, so the two kinds are told apart at a glance.
    let radius = if m.kind.is_custom() { 0.0 } else { 6.0 };
    painter.rect_filled(rect, radius, fill);
    // A pending removal outranks the selection: the box is about to disappear,
    // which is the more urgent thing to say about it.
    let stroke = if removing_blink.is_some() {
        egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(235, 70, 70)) // pending removal
    } else if selected {
        egui::Stroke::new(2.8_f32, egui::Color32::WHITE) // 2× the connected border
    } else if connected {
        egui::Stroke::new(1.4_f32, color) // border matches the pin colour
    } else {
        egui::Stroke::new(1.2_f32, egui::Color32::from_rgb(120, 90, 90)) // disconnected
    };
    painter.rect_stroke(rect, radius, stroke, egui::StrokeKind::Middle);

    const TITLE_SIZE: f32 = 13.0;
    let scale = text_scale(selected);
    let title_color = if selected {
        egui::Color32::WHITE
    } else if connected {
        color
    } else {
        egui::Color32::from_rgb(175, 150, 150)
    };
    let title_size = TITLE_SIZE * scale;
    painter.text(
        rect.center_top() + egui::vec2(0.0, 13.0),
        egui::Align2::CENTER_CENTER,
        module_base_name(m),
        egui::FontId::proportional(title_size),
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
        egui::FontId::proportional(10.0 * scale),
        egui::Color32::from_rgb(150, 150, 160),
    );
    // Live preview of the resulting variable name(s) above the rename field —
    // updates as the user types (same as the pin's `pc13_out_board_led`).
    // Clipped to the box so a long label can't overflow the border.
    painter.with_clip_rect(rect).text(
        egui::pos2(rect.left() + 10.0, rect.bottom() - 26.0),
        egui::Align2::LEFT_BOTTOM,
        handle_preview(m, native_forced),
        egui::FontId::proportional(9.0 * scale),
        egui::Color32::from_rgb(140, 140, 150),
    );
}

/// Font multiplier for a module box's texts: 1 normally, `SELECTED_TEXT_SCALE`
/// while the box is selected. Shared by the box painter and the (separate)
/// mutable pass that puts the rename fields, so the two never drift apart.
fn text_scale(selected: bool) -> f32 {
    if selected {
        crate::panels::mcu_module::pins::gui::draw::SELECTED_TEXT_SCALE
    } else {
        1.0
    }
}

/// Draw each module beside the chip, on the side of the pins it connects to,
/// with short direct wires (no crossing the chip). Fully-disconnected modules
/// float in the right margin. Each box carries a rename field whose text is
/// appended to the module's generated handle variable (e.g. `_spi1_imu`).
pub fn draw_modules(
    mcu: &mut Mcu,
    painter: &egui::Painter,
    local_chip: egui::Rect,
    display_chip: egui::Rect,
    rot: Rot,
    ui: &mut egui::Ui,
) {
    // Boxes are placed around the DISPLAY rect (what the user sees); pin anchors
    // are computed on the LOCAL rect then rotated. For an un-rotated chip the two
    // are identical and `rot` is the identity.
    let chip_rect = display_chip;
    // ── 1. Classify each module: connected (side + along-axis centroid) or
    //       floating (no wired pins). `conns` keeps the wire endpoints.
    struct Sided {
        idx: usize,
        conns: Vec<(ModuleSignal, egui::Pos2, usize)>,
        side: Side,
        along: f32,
    }
    let chip_center = chip_rect.center();
    let mut sided: Vec<Sided> = Vec::new();
    let mut floating_idx: Vec<usize> = Vec::new();
    // Modules the user has dragged to a manual position (`pos != (0,0)`, stored
    // as an offset from the chip centre) are placed there directly, skipping the
    // auto-packing. Their wire conns are kept so the wires still track the pins.
    let mut manual_mods: Vec<(usize, Vec<(ModuleSignal, egui::Pos2, usize)>)> = Vec::new();

    for (i, m) in mcu.modules.iter().enumerate() {
        let conns: Vec<(ModuleSignal, egui::Pos2, Side, usize)> = m
            .connections
            .iter()
            .filter_map(|c| {
                pin_anchor_side(mcu, local_chip, rot, c.mcu_pin)
                    .map(|(p, s)| (c.signal, p, s, c.mcu_pin))
            })
            .collect();
        if m.pos != (0.0, 0.0) {
            let conns2 = conns.iter().map(|(sig, p, _, n)| (*sig, *p, *n)).collect();
            manual_mods.push((i, conns2));
            continue;
        }
        if conns.is_empty() {
            floating_idx.push(i);
            continue;
        }
        let side = dominant_side(&conns);
        // Centroid along the side axis, from the pins actually on that side.
        let on_side: Vec<f32> = conns
            .iter()
            .filter(|(_, _, s, _)| *s == side)
            .map(|(_, p, _, _)| match side {
                Side::Top | Side::Bottom => p.x,
                Side::Left | Side::Right => p.y,
            })
            .collect();
        let along = on_side.iter().sum::<f32>() / on_side.len().max(1) as f32;
        let conns2 = conns.iter().map(|(sig, p, _, n)| (*sig, *p, *n)).collect();
        sided.push(Sided {
            idx: i,
            conns: conns2,
            side,
            along,
        });
    }

    // ── 2. Pack each side independently so same-side boxes never overlap. ──────
    // (module index, rect, conns, side, connected, manual)
    let mut boxes: Vec<(
        usize,
        egui::Rect,
        Vec<(ModuleSignal, egui::Pos2, usize)>,
        Side,
        bool,
        bool,
    )> = Vec::new();
    for target in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
        let mut group: Vec<&Sided> = sided.iter().filter(|e| e.side == target).collect();
        group.sort_by(|a, b| a.along.total_cmp(&b.along));
        let mut cursor = f32::MIN;
        for e in group {
            let rect = packed_rect(
                chip_rect,
                e.side,
                e.along,
                &mut cursor,
                box_h(&mcu.modules[e.idx]),
            );
            boxes.push((e.idx, rect, e.conns.clone(), e.side, true, false));
        }
    }
    // Disconnected modules stack in the right margin.
    let mut fy = chip_rect.top();
    for i in floating_idx {
        let min = egui::pos2(chip_rect.right() + PIN_HEIGHT + PIN_GAP, fy);
        let h = box_h(&mcu.modules[i]);
        fy += h + BOX_GAP;
        boxes.push((
            i,
            egui::Rect::from_min_size(min, egui::vec2(BOX_W, h)),
            Vec::new(),
            Side::Right,
            false,
            false,
        ));
    }
    // Manually-dragged boxes: placed at chip centre + stored offset.
    for (i, conns) in manual_mods {
        let p = mcu.modules[i].pos;
        let rect = egui::Rect::from_min_size(
            chip_center + egui::vec2(p.0, p.1),
            egui::vec2(BOX_W, box_h(&mcu.modules[i])),
        );
        let connected = !conns.is_empty();
        boxes.push((i, rect, conns, Side::Right, connected, true));
    }

    // ── 3. Draw boxes + wires; detect a header click to expand the list entry. ─
    // Native runtime → the handle preview shows the split (Tx, Rx) for USART.
    let native_forced = mcu.is_native();
    // While a remove-confirm is open, pulse the target module's box red so it's
    // obvious on the diagram which module is about to be deleted. Computed once
    // per frame; only the matching box uses it. Repaint keeps the pulse alive.
    let removing_id = mcu.module_remove_confirm.clone();
    let blink = if removing_id.is_some() {
        ui.ctx().request_repaint();
        let phase = (ui.input(|i| i.time) * std::f64::consts::TAU * 1.8).sin();
        0.5 + 0.5 * phase as f32
    } else {
        0.0
    };
    let mut clicked_id: Option<String> = None;
    // Applied after the loop (can't mutate `mcu.modules` while it's borrowed by
    // the box `m`): dragged offsets, and right-click "reset to auto" requests.
    let mut drag_updates: Vec<(usize, (f32, f32))> = Vec::new();
    let mut reset_updates: Vec<usize> = Vec::new();
    let mut field_pass: Vec<(usize, egui::Rect)> = Vec::new();
    for (i, rect, conns, side, connected, manual) in &boxes {
        let m = &mcu.modules[*i];
        let inst = m.instance();
        let removing = removing_id.as_deref() == Some(m.id.as_str());
        draw_box(
            painter,
            *rect,
            m,
            *connected,
            module_color(m.kind, inst),
            native_forced,
            removing.then_some(blink),
            mcu.selected_module.as_deref() == Some(m.id.as_str()),
        );

        for (sig, anchor, anchor_pin) in conns {
            let color = signal_color(*sig, inst);
            // A dragged box's stored side no longer implies an edge, so aim the
            // wire at the box edge nearest the pin; auto boxes keep their side.
            let term = if *manual {
                nearest_edge(*rect, *anchor)
            } else {
                facing_terminal(*rect, *side, *anchor)
            };
            painter.circle_filled(term, 3.5, color);
            painter.circle_filled(*anchor, 3.5, color);
            painter.line_segment([term, *anchor], egui::Stroke::new(1.6_f32, color));
            // Custom modules show the DATA DIRECTION: an MCU input is driven by
            // the device (module → pin), an output is driven by the MCU
            // (pin → module). Peripheral buses are bidirectional, so no head.
            if m.kind.is_custom() {
                let dir = mcu
                    .find_pin(*anchor_pin)
                    .map(|p| p.selected_function.clone());
                let (from, to) = match dir {
                    Some(PinFunction::GpioInput) => (term, *anchor),
                    Some(PinFunction::GpioOutput) | Some(PinFunction::TimerPwm { .. }) => {
                        (*anchor, term)
                    }
                    _ => (term, term), // unknown direction → plain wire
                };
                if from != to {
                    arrow_head(painter, from, to, color);
                }
            }
        }

        // Drag the header area (above the rename field) to MOVE the box; a plain
        // click still expands the list entry. Right-click a moved box to reset.
        let header_rect =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.bottom() - 30.0));
        let resp = ui
            .interact(
                header_rect,
                ui.id().with(("vmod_box", *i)),
                egui::Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab);
        if resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            // drag_delta is already in scene coords (the Scene layer transform is
            // applied by egui), so it's correct at any zoom.
            let new_min = rect.min + resp.drag_delta();
            let off = new_min - chip_center;
            drag_updates.push((*i, (off.x, off.y)));
        }
        if resp.hovered() || resp.dragged() {
            painter.rect_stroke(
                *rect,
                6.0,
                egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(50)),
                egui::StrokeKind::Middle,
            );
        }
        if resp.clicked() {
            clicked_id = Some(m.id.clone());
        }
        if *manual {
            resp.context_menu(|ui| {
                if ui.button("Reset to auto position").clicked() {
                    reset_updates.push(*i);
                    ui.close();
                }
            });
        }
        field_pass.push((*i, *rect));
    }
    // Apply drag / reset now that the box borrow of `mcu.modules` has ended.
    for (i, off) in drag_updates {
        mcu.modules[i].pos = off;
    }
    for i in reset_updates {
        mcu.modules[i].pos = (0.0, 0.0);
    }

    // ── 4. Rename fields (mutable pass) ───────────────────────────────────────
    // The typed text is appended to the module's generated variable name(s);
    // regenerated every frame by `update_main_rs`, so it updates as you type.
    for (i, box_rect) in field_pass {
        // Same selection state the box was painted with, so its fields grow with
        // the rest of it. Read BEFORE the mutable `find_pin_mut` borrows below.
        let selected = mcu.selected_module.as_deref() == Some(mcu.modules[i].id.as_str());
        let scale = text_scale(selected);
        let selected_pin = mcu.selected_pin;
        // A Custom module groups its PINS' rename fields inside the box, under
        // the module name — instead of each pin floating separately beside the
        // chip (io_arrows skips them, see `module_owned_pins`).
        if mcu.modules[i].kind.is_custom() {
            let pins: Vec<usize> = match &mcu.modules[i].config {
                ModuleConfig::Custom(c) => c.pins.clone(),
                _ => Vec::new(),
            };
            for (row, num) in pins.iter().enumerate() {
                let r = custom_pin_row(box_rect, row);
                let name = mcu
                    .find_pin(*num)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| format!("pin{num}"));
                // A pin selected on the chip is called out HERE too — its row
                // (pin name + rename field) gets the same white border a lone
                // pin's field group gets out in the margin.
                let pin_sel = selected_pin == Some(*num);
                let row_scale = scale.max(text_scale(pin_sel));
                painter.text(
                    egui::pos2(box_rect.left() + 8.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::monospace(9.5 * row_scale),
                    if pin_sel {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(170, 175, 190)
                    },
                );
                if let Some(pin) = mcu.find_pin_mut(*num) {
                    ui.push_id(("custom_pin_label", i, num), |ui| {
                        ui.put(
                            r,
                            egui::TextEdit::singleline(&mut pin.custom_label)
                                .hint_text("name")
                                .font(egui::FontId::proportional(9.5 * row_scale)),
                        );
                    });
                }
                if pin_sel {
                    painter.rect_stroke(
                        egui::Rect::from_min_max(
                            egui::pos2(box_rect.left() + 4.0, r.top() - 2.0),
                            egui::pos2(r.right() + 2.0, r.bottom() + 2.0),
                        ),
                        4.0,
                        egui::Stroke::new(2.8_f32, egui::Color32::WHITE),
                        egui::StrokeKind::Middle,
                    );
                }
            }
        }
        let field_rect = label_field_rect(box_rect);
        let label = mcu.modules[i].config.custom_label_mut();
        ui.push_id(("module_label", i), |ui| {
            ui.put(
                field_rect,
                egui::TextEdit::singleline(label)
                    .hint_text("name")
                    .font(egui::FontId::proportional(10.0 * scale)),
            );
        });
    }

    if let Some(id) = clicked_id {
        // One click does three things: select the box (click again to deselect,
        // like a pin), expand the module's entry in the list below, and light up
        // every line its pins bind in the code.
        mcu.selected_module = if mcu.selected_module.as_deref() == Some(id.as_str()) {
            None
        } else {
            Some(id.clone())
        };
        mcu.expand_module = Some(id.clone());
        mcu.module_goto = Some(id);
    }
}

fn dominant_side(conns: &[(ModuleSignal, egui::Pos2, Side, usize)]) -> Side {
    let mut count: HashMap<Side, usize> = HashMap::new();
    for (_, _, s, _) in conns {
        *count.entry(*s).or_insert(0) += 1;
    }
    // Most common; ties resolved by the first connection's side.
    conns
        .iter()
        .map(|(_, _, s, _)| *s)
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
/// The channels of `timer` this chip still has a free pad for: every channel
/// any pin can serve, minus the ones already wired to the module.
///
/// Derived, never a fixed "CH2/3/4": TIM14 has one channel and TIM9 two, and a
/// package bonds only some of the pads a die carries — the truth is in the
/// pins' own function lists, which is also where the codegen reads the channel
/// set from (`pwm_wires`).
fn free_pwm_channels(
    timer: u8,
    wired: &BTreeSet<String>,
    pin_funcs: &HashMap<usize, Vec<PinFunction>>,
) -> Vec<String> {
    let on_chip: BTreeSet<String> = pin_funcs
        .values()
        .flatten()
        .filter_map(|f| match f {
            PinFunction::TimerPwm { timer: t, channel } if *t == timer => {
                Some(format!("CH{channel}"))
            }
            // A complementary pad is offered the same way — it is one more
            // signal to assign on the canvas — but only where embassy can
            // actually drive it.
            PinFunction::TimerPwmN { timer: t, channel }
                if *t == timer && is_advanced_timer(*t) =>
            {
                Some(format!("CH{channel}N"))
            }
            // Break lives on the same driver, so it is offered under the same
            // rule: only where embassy can actually reach it.
            PinFunction::TimerBreak { timer: t, input } if *t == timer && is_advanced_timer(*t) => {
                Some(if *input == 1 {
                    "BKIN".to_owned()
                } else {
                    format!("BKIN{input}")
                })
            }
            _ => None,
        })
        .collect();
    on_chip.difference(wired).cloned().collect()
}

pub fn module_config_ui(
    ui: &mut egui::Ui,
    m: &mut VirtualModule,
    pin_names: &HashMap<usize, String>,
    // Per-pin `function|label` fingerprint — a Custom module compares it with
    // what its generated code was built from (see `has_pending_pins`).
    pin_sigs: &HashMap<usize, String>,
    // Pins a Custom module may not take: reserved, assigned, or owned elsewhere.
    pin_blocked: &std::collections::HashSet<usize>,
    // Editable per-pin labels — a Custom module's rows mirror the name field
    // inside its box on the canvas; the caller writes the changes back.
    pin_labels: &mut HashMap<usize, String>,
    // Each pin's CURRENT function — a Custom module needs it to tell a configured
    // pin from one still waiting for In / Out / ADC / PWM.
    pin_funcs_current: &HashMap<usize, PinFunction>,
    // Functions selectable per pin, behind the pin-name buttons.
    pin_funcs: &HashMap<usize, Vec<PinFunction>>,
    // Set when a function is picked from a pin button; the caller applies it.
    pin_fn_choice: &mut Option<(usize, PinFunction)>,
    // EMBASSY async. The ESP has an async runtime too, but none of its rows
    // turn on it: `with_dma`, `UartTx::new` and `.with_cts()` are all on the
    // blocking drivers, so what decides there is the FAMILY (`esp` below).
    is_async: bool,
    is_native: bool,
    // The chip's family key — the blocking DMA transport is stm32f1xx-hal's,
    // so only the F1 backend can emit it.
    family: &str,
    // The STAGED `(api_style, async_mode)` for this module — the api/async row
    // edit THIS, not `m.config`, so nothing regenerates until "Apply".
    pending: &mut (ApiStyle, AsyncBusMode),
    // The chip's DMA facts, for the channel picker. Cloned by the caller rather
    // than borrowed, because `m` is a mutable borrow out of the same `Mcu`.
    dma: Option<&crate::panels::mcu_module::mcu_def::DmaDef>,
    // Whether this chip's USART has the swap / invert bits. Passed in rather
    // than derived here, because it is a fact about the CHIP and this function
    // only sees one module.
    line_extras: bool,
) {
    // Read what we need off `m` BEFORE `m.config` is borrowed mutably below.
    let is_custom = m.kind.is_custom();
    // Which DMA request table the serial arm below asks. Read from the KIND here,
    // because inside the match the two share one binding (`UsartModuleConfig`).
    let uart_bus = if m.kind == ModuleKind::GenericInterfaceLpuart {
        dma_map::Bus::Lpuart
    } else {
        dma_map::Bus::Usart
    };
    let m_id = m.id.clone();
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
    // Whether an SPI module has a receive line at all. Read from its own wiring
    // for the same reason as `wired_flow` below.
    let has_miso = m.connections.iter().any(|c| c.signal == ModuleSignal::Miso);
    // Which pad of each PAIR this module actually has. On the F1 both are
    // required — see `f1_half_bus_note`.
    let has_sig = |want: ModuleSignal| m.connections.iter().any(|c| c.signal == want);
    let wired_serial = (
        has_sig(ModuleSignal::Tx) || has_sig(ModuleSignal::LpTx),
        has_sig(ModuleSignal::Rx) || has_sig(ModuleSignal::LpRx),
    );
    let wired_i2c = (has_sig(ModuleSignal::Scl), has_sig(ModuleSignal::Sda));
    let wired_can = (has_sig(ModuleSignal::CanTx), has_sig(ModuleSignal::CanRx));
    let wired_usb = (has_sig(ModuleSignal::UsbDm), has_sig(ModuleSignal::UsbDp));
    // Which flow-control pads this module actually has, read from its own
    // wiring — the flow selector warns against THIS, not against a guess.
    let wired_flow = (
        m.connections
            .iter()
            .any(|c| matches!(c.signal, ModuleSignal::Cts | ModuleSignal::LpCts)),
        m.connections
            .iter()
            .any(|c| matches!(c.signal, ModuleSignal::Rts | ModuleSignal::LpRts)),
    );

    // Several rows turn on the FAMILY rather than the runtime: the ESP's UART
    // direction and flow exist on both its runtimes, and its Init API exists on
    // neither.
    let esp = crate::panels::mcu_module::codegen::family::is_esp(family);

    // Portable (embedded-io/hal) vs native (concrete HAL) init — shown for the
    // bus modules (USART/SPI/I2C) that generate a `pins/configs/*.rs` init.
    //
    // On an ESP it is shown LOCKED. The choice is an STM32 one: the bridge it
    // selects is `embedded-io`/`embedded-hal` over a stm32f1xx-hal type, and no
    // ESP emitter reads `api_style` at all — `codegen_esp_configs` always
    // returns the esp-hal driver. Left editable it was a control that changed
    // nothing, which is worse than one that is visibly fixed.
    let api_row = |ui: &mut egui::Ui, style: &mut ApiStyle| {
        if esp {
            ui.label("Init API");
            let resp = ui.add_enabled_ui(false, |ui| {
                egui::ComboBox::from_id_salt("api_style_esp")
                    .selected_text("Native (esp-hal type)")
                    .show_ui(ui, |_ui| {});
            });
            resp.response.on_hover_text(
                "esp-hal only. `init` hands back the concrete esp-hal driver - `Uart`, \
                 `Spi`, `I2c` - because that is what its own traits are implemented on. \
                 The Portable bridge is an STM32F1 choice.",
            );
            ui.end_row();
            return;
        }
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

    // Native RUNTIME forces every peripheral to the concrete HAL — show the Init
    // API row DISABLED + locked on Native (instead of hiding it), so it's clear
    // the choice is fixed here, not missing.
    let api_row_locked = |ui: &mut egui::Ui| {
        ui.label("Init API");
        let resp = ui.add_enabled_ui(false, |ui| {
            egui::ComboBox::from_id_salt("api_style_locked")
                .selected_text("Native (HAL type)")
                .show_ui(ui, |_ui| {});
        });
        resp.response.on_hover_text(
            "Locked by the Native runtime — every peripheral uses the concrete HAL type. \
             Switch the Runtime (System tab) to choose per module.",
        );
        ui.end_row();
    };

    // Blocking runtime on the F1: poll each byte, or hand the bus to DMA.
    //
    // A checkbox rather than a channel picker, because there is nothing to pick
    // — stm32f1xx-hal fixes the channel per peripheral in its types. The label
    // names them anyway, so the choice is not a black box.
    //
    // `rx_ok` is false for an SPI with no MISO: there is no receive line, so
    // the two receiving choices are dropped and a stored one is narrowed on
    // sight — the same clamp codegen applies, done where the user can see it.
    let transport_row = |ui: &mut egui::Ui, on: &mut BlockingDma, chans: &str, rx_ok: bool| {
        if !rx_ok {
            *on = on.without_rx();
        }
        ui.label("Transport");
        egui::ComboBox::from_id_salt("blocking_dma")
            .selected_text(on.label())
            .show_ui(ui, |ui| {
                for v in BlockingDma::ALL.into_iter().filter(|v| rx_ok || !v.rx()) {
                    ui.selectable_value(on, v, v.label())
                        .on_hover_text(match v {
                            BlockingDma::Off => "The CPU moves every byte, both ways.".to_owned(),
                            BlockingDma::Both => format!(
                                "Both directions on DMA ({chans}), and both channels reserved."
                            ),
                            // The half-and-half cases are the interesting ones,
                            // so each says what it buys.
                            BlockingDma::Tx => format!(
                                "TX on DMA, RX polled. Frees the RX channel; `init` returns a \
                                 DMA transmitter and the HAL's ordinary `nb` receiver ({chans})."
                            ),
                            BlockingDma::Rx => format!(
                                "RX on DMA, TX written by the CPU - the usual choice when \
                                 receiving must not drop bytes but sending is short bursts. \
                                 Frees the TX channel ({chans})."
                            ),
                        });
                }
            });
        ui.end_row();
    };

    // With DMA on, the Init API choice has no object: the handles are the HAL's
    // own DMA types either way. Shown locked rather than hidden, same as under
    // the Native runtime.
    let api_row_locked_dma = |ui: &mut egui::Ui| {
        ui.label("Init API");
        let resp = ui.add_enabled_ui(false, |ui| {
            egui::ComboBox::from_id_salt("api_style_locked_dma")
                .selected_text("DMA handles (HAL type)")
                .show_ui(ui, |_ui| {});
        });
        resp.response.on_hover_text(
            "Locked by the DMA transport - `init` returns TxDma/RxDma (USART) or \
             SpiRxTxDma (SPI), which are stm32f1xx-hal's own types. There is no portable \
             bus to bridge them to; turn DMA off to choose again.",
        );
        ui.end_row();
    };

    // Async runtime only (SPI/I2C): blocking driver vs async-DMA.
    //
    // The wording is deliberately HAL-neutral. It named embassy's constructors
    // outright, on a row an Espressif chip sees too — where the driver is
    // esp-hal's and the DMA is the GDMA. Whatever the chip, the choice is the
    // same one: a standard blocking bus, or an .await-able one on DMA.
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
                        "A STANDARD blocking embedded-hal 1.0 bus. \
                         No DMA — compiles out of the box. Fine inside an async project.",
                    );
                ui.selectable_value(
                    mode,
                    AsyncBusMode::AsyncDma,
                    "Async-DMA (embedded-hal-async)",
                )
                .on_hover_text(
                    "An .await-able bus whose bytes are moved by DMA. On an STM32 the 
                         channels are allocated for you (or pinned below); on an 
                         ESP32 it is the one GDMA channel esp-hal takes, passed 
                         from main.rs. Either way the Configuration tab's DMA card 
                         shows what was taken.",
                );
            });
        ui.end_row();
    };

    // Async runtime only (USART): interrupt ring buffer vs DMA. A separate
    // enum from `AsyncBusMode` because "blocking" is not one of the options —
    // an async USART is never blocking, only differently non-blocking.
    // The ESP's own transport row. Same CHOICE as embassy's `async_row` and a
    // different mechanism behind it, so it says the mechanism: esp-hal takes a
    // channel in `with_dma`, and the driver that comes back is a different type
    // with different methods.
    let esp_dma_row = |ui: &mut egui::Ui, mode: &mut AsyncBusMode| {
        ui.label("Transfers");
        egui::ComboBox::from_id_salt("esp_dma")
            .selected_text(match mode {
                AsyncBusMode::Blocking => "CPU (blocking)",
                AsyncBusMode::AsyncDma => "DMA",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(mode, AsyncBusMode::Blocking, "CPU (blocking)")
                    .on_hover_text(
                        "The CPU moves every byte. Simplest, and fine for short \
                         transfers.",
                    );
                ui.selectable_value(mode, AsyncBusMode::AsyncDma, "DMA")
                    .on_hover_text(
                        "esp-hal's `with_dma`: the peripheral moves the bytes itself. \
                         Takes one of the chip's channels - the Configuration tab \
                         shows which one it got.",
                    );
            });
        ui.end_row();
    };

    // The ESP channel picker. `dma_plan` reserves a hand-pinned channel BEFORE
    // it allocates the rest, so naming one here takes it out of the pool rather
    // than fighting over it.
    //
    // The list is the same one the allocator would draw from: every channel on
    // a pooled DMA, and only the ones the request table names on a bolted one
    // (the original ESP32 and the S2, whose `DMA_SPI2` serves SPI2 alone).
    let esp_dma_channel_row = |ui: &mut egui::Ui,
                               dma: Option<&crate::panels::mcu_module::mcu_def::DmaDef>,
                               request: &str,
                               chan: &mut String| {
        ui.label("DMA channel");
        // The allocator's own list, not a second one built to match it.
        let options = crate::panels::mcu_module::codegen_esp::dma_candidates(dma, request);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(format!("esp_dma_ch_{request}"))
                .selected_text(if chan.is_empty() {
                    "Automatic".to_owned()
                } else {
                    chan.clone()
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(chan, String::new(), "Automatic");
                    for c in &options {
                        ui.selectable_value(chan, c.clone(), c);
                    }
                });
            if options.is_empty() {
                ui.label(
                    egui::RichText::new("no channel data for this chip")
                        .size(10.5)
                        .color(egui::Color32::from_rgb(220, 160, 70)),
                );
            }
        })
        .response
        .on_hover_text(
            "Automatic lets the generator pick a free one. Naming a channel pins \
             it: it is reserved before anything else is handed out, which is how \
             two buses are kept off each other.",
        );
        ui.end_row();
    };

    let usart_mode_row = |ui: &mut egui::Ui, mode: &mut UsartMode| {
        ui.label("Async transport");
        egui::ComboBox::from_id_salt("usart_mode")
            .selected_text(match mode {
                UsartMode::Buffered => "Buffered (interrupt)",
                UsartMode::Dma => "DMA (ring buffer)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(mode, UsartMode::Buffered, "Buffered (interrupt)")
                    .on_hover_text(
                        "embassy BufferedUart -> embedded-io-async Read + Write, one interrupt                          per byte into a software ring buffer. Needs no DMA channel, so it                          compiles out of the box.",
                    );
                ui.selectable_value(mode, UsartMode::Dma, "DMA (ring buffer)")
                    .on_hover_text(
                        "UartTx + RingBufferedUartRx -> the same embedded-io-async traits, but the \
                         peripheral talks to DMA directly and RX keeps filling a circular buffer \
                         between your reads, so bytes are not dropped in the gaps. Takes TWO \
                         channels on a bidirectional UART - embassy's constructor requires both. \
                         To spend one, set Data Direction to RX only or TX only; to keep both \
                         directions but send from the CPU, the generated file shows \
                         `blocking_write` on the same handle.",
                    );
            });
        ui.end_row();
    };

    // What the BLOCKING stm32f1xx-hal cannot do, said out loud.
    //
    // Direction, flow control, half duplex and swap/invert are all async-only
    // rows above. On the F1 they are not "not implemented yet": `Pins<USART>` is
    // implemented ONLY for the (TX alternate, RX input) pair, `serial.rs` has no
    // hdsel / rtse / ctse at all, and the SWAP / TXINV / RXINV bits do not exist
    // in F1 silicon. Leaving the rows out silently makes a user who just used
    // them on a G0 project hunt for them; this says why.
    let f1_serial_note = |ui: &mut egui::Ui| {
        ui.label("");
        ui.label(
            egui::RichText::new(
                "stm32f1xx-hal has no flow control, half duplex or one-way UART                  (its Serial takes the TX+RX pair), and the F1 USART has no                  swap/invert bits",
            )
            .size(10.5)
            .color(egui::Color32::from_gray(140)),
        );
        ui.end_row();
    };

    // Half a USART — or half an I2C — generates NOTHING on the F1: both HALs
    // take the PAIR (`serial::Pins`, `i2c::Pins`) and have no placeholder for
    // the pad that is missing. main.rs says so where the init would have gone;
    // this says so before the user gets there, since a peripheral quietly
    // absent from main.rs is easy to miss.
    //
    // `pads` is (first, second) in the order the pair is named, `wired` says
    // which of them the module actually has, and `why` is the reason clause —
    // three of these are a HAL `Pins` bound, the USB one is not.
    let f1_half_bus_note =
        |ui: &mut egui::Ui, bus: &str, pads: (&str, &str), why: &str, wired: (bool, bool)| {
            let missing = match wired {
                (true, false) => pads.1,
                (false, true) => pads.0,
                _ => return,
            };
            ui.label("");
            ui.label(
                egui::RichText::new(format!(
                    "{missing} is not wired, so this {bus} is not initialised at all — \
                 {why}. Assign the {missing} pad on the canvas."
                ))
                .size(10.5)
                .color(egui::Color32::from_rgb(220, 160, 70)),
            );
            ui.end_row();
        };

    // Data direction + hardware flow control. Both lists come from
    // `UsartDirection::options_for` / `UsartFlow::options_for`, i.e. from what
    // THIS FAMILY's backend has a CONSTRUCTOR for — never from what the silicon
    // could in principle do. The three backends differ a lot: embassy builds
    // every shape, the ESP builds a single direction and RTS/CTS, and the F1
    // builds one TX+RX port and nothing else.
    // `wired` says which of CTS/RTS the module actually has a pin for, so a
    // choice that needs one it hasn't got is called out instead of silently
    // generating code that won't build.
    let direction_row = |ui: &mut egui::Ui, cfg: &mut UsartModuleConfig| {
        let opts = UsartDirection::options_for(cfg.mode, family);
        ui.label("Data direction");
        if opts.len() == 1 {
            // Locked rather than hidden: the reason is the useful part.
            ui.add_enabled_ui(false, |ui| {
                egui::ComboBox::from_id_salt("usart_dir_locked")
                    .selected_text(UsartDirection::TxRx.label())
                    .show_ui(ui, |_ui| {});
            })
            .response
            .on_hover_text(
                "embassy has no buffered TX-only / RX-only: BufferedUartTx and                  BufferedUartRx come only from splitting a BufferedUart, so both                  pins are used either way. Switch the transport to DMA for a                  one-way UART that frees the other pin.",
            );
        } else {
            egui::ComboBox::from_id_salt("usart_dir")
                .selected_text(cfg.direction.label())
                .show_ui(ui, |ui| {
                    for d in opts {
                        ui.selectable_value(&mut cfg.direction, *d, d.label());
                    }
                });
        }
        ui.end_row();
        // Line-level extras. On an older USART the register bits do not exist —
        // and neither do embassy's `Config` fields — so the row says that
        // instead of offering a switch that cannot be generated.
        ui.label("Line");
        ui.vertical(|ui| {
            if line_extras {
                ui.checkbox(&mut cfg.swap_rx_tx, "Swap RX/TX pads")
                    .on_hover_text(
                        "The peripheral crosses the two itself — for a cable or a                          board that is wired the other way round, with no rework.",
                    );
                ui.checkbox(&mut cfg.invert_tx, "Invert TX")
                    .on_hover_text("Idle low instead of idle high, for an inverting transceiver.");
                ui.checkbox(&mut cfg.invert_rx, "Invert RX");
            } else {
                ui.label(
                    egui::RichText::new("this USART has no swap / invert bits")
                        .size(10.5)
                        .italics()
                        .color(egui::Color32::from_gray(140)),
                );
            }
        });
        ui.end_row();
        // Readback is a half-duplex-only argument, so it appears only there.
        if cfg.direction.is_half_duplex() {
            ui.label("Read back own TX");
            ui.checkbox(&mut cfg.half_duplex_readback, "")
                .on_hover_text(
                    "One wire carries both directions, so everything this node                      sends is also on its receiver. OFF (the default) disables the                      receiver while transmitting, which is what a bus with other                      talkers wants; ON keeps the echo, which is how you verify a                      driver that can be shouted down.",
                );
            ui.end_row();
        }
    };

    let flow_row = |ui: &mut egui::Ui, cfg: &mut UsartModuleConfig, wired: (bool, bool)| {
        ui.label("Hardware flow control");
        ui.vertical(|ui| {
            egui::ComboBox::from_id_salt("usart_flow")
                .selected_text(cfg.flow.label())
                .show_ui(ui, |ui| {
                    for f in UsartFlow::options_for(cfg.mode, cfg.direction, family) {
                        ui.selectable_value(&mut cfg.flow, *f, f.label());
                    }
                });
            let (has_cts, has_rts) = wired;
            let missing = match (
                cfg.flow.needs_cts() && !has_cts,
                cfg.flow.needs_rts() && !has_rts,
            ) {
                (true, true) => "CTS and RTS pins",
                (true, false) => "a CTS pin",
                (false, true) => "an RTS pin",
                (false, false) => "",
            };
            if !missing.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "{} needs {missing} — assign it on the canvas",
                        cfg.flow.label()
                    ))
                    .size(10.5)
                    .color(egui::Color32::from_rgb(220, 160, 70)),
                );
            }
        });
        ui.end_row();
    };

    // Which DMA channels this peripheral gets. Automatic is right almost
    // always — the row exists for the board that needs a SPECIFIC channel (to
    // leave a high-priority one free, to match an existing driver, to dodge an
    // erratum), which is not something the IDE can infer.
    // ONE direction's picker. Split out of `dma_row` because a peripheral can
    // legitimately use a single channel — an I2S transmits or receives, never
    // both — and offering the other one would invite a choice that moves no
    // bytes.
    let dma_one = |ui: &mut egui::Ui,
                   bus: dma_map::Bus,
                   inst: u8,
                   dir: dma_map::Dir,
                   label: &str,
                   chosen: &mut String| {
        {
            let options = dma_map::channels_for(dma, bus, inst, dir);
            ui.label(label);
            if options.is_empty() {
                // No vendor data for this chip: the IDE cannot say which
                // channels are valid, so it offers none rather than a free-text
                // box that invites a name which moves the wrong bytes.
                ui.add_enabled_ui(false, |ui| {
                    egui::ComboBox::from_id_salt(format!("dma_{label}_none"))
                        .selected_text("Automatic")
                        .show_ui(ui, |_ui| {});
                })
                .response
                .on_hover_text(
                    "This chip carries no DMA channel data - re-import it from the                      STM32Cube database to choose a channel by hand.",
                );
                ui.end_row();
                return;
            }
            egui::ComboBox::from_id_salt(format!("dma_{label}"))
                .selected_text(if chosen.is_empty() {
                    "Automatic".to_owned()
                } else {
                    chosen.clone()
                })
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(chosen.is_empty(), "Automatic")
                        .on_hover_text("The IDE takes the first channel this peripheral can use and nothing else has taken.")
                        .clicked()
                    {
                        chosen.clear();
                    }
                    for c in &options {
                        if ui.selectable_label(chosen == c, c).clicked() {
                            *chosen = c.clone();
                        }
                    }
                });
            ui.end_row();
        }
    };
    let dma_row =
        |ui: &mut egui::Ui, bus: dma_map::Bus, inst: u8, tx: &mut String, rx: &mut String| {
            dma_one(ui, bus, inst, dma_map::Dir::Tx, "DMA TX", tx);
            dma_one(ui, bus, inst, dma_map::Dir::Rx, "DMA RX", rx);
        };

    egui::Grid::new("module_cfg")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            match &mut m.config {
                ModuleConfig::Touch(cfg) => {
                    ui.label("Scan");
                    egui::ComboBox::from_id_salt("touchscan")
                        .selected_text(cfg.scan.label())
                        .show_ui(ui, |ui| {
                            for v in TouchScan::ALL {
                                ui.selectable_value(&mut cfg.scan, v, v.label())
                                    .on_hover_text(v.hint());
                            }
                        })
                        .response
                        .on_hover_text(
                            "The two are different TYPES in esp-hal, with different \
                             methods - not a speed setting.",
                        );
                    ui.end_row();

                    ui.label("Touched when");
                    egui::ComboBox::from_id_salt("touchthr")
                        .selected_text(cfg.threshold_mode.label())
                        .show_ui(ui, |ui| {
                            for v in TouchThreshold::ALL {
                                ui.selectable_value(&mut cfg.threshold_mode, v, v.label());
                            }
                        })
                        .response
                        .on_hover_text(
                            "A finger adds capacitance, so the pad charges slower and the \
                             count usually FALLS. Below-threshold is the normal choice.",
                        );
                    ui.end_row();

                    ui.label("Threshold");
                    ui.add(egui::DragValue::new(&mut cfg.threshold).range(1..=65535))
                        .on_hover_text(
                            "There is no right number here: read your own pad untouched, \
                             then take a margin off it. One value for every pad - the \
                             generated file takes it per pad if you need to differ.",
                        );
                    ui.end_row();

                    ui.label("Measurement")
                        .on_hover_text("Cycles of the 8 MHz touch clock, per measurement.");
                    ui.add(
                        egui::DragValue::new(&mut cfg.measurement_duration).range(1..=0x7fff),
                    );
                    ui.end_row();

                    // The sleep timer only exists in continuous mode; showing it
                    // in one-shot would be a control that changes nothing.
                    if cfg.scan.is_continuous() {
                        ui.label("Sleep cycles")
                            .on_hover_text("Idle time between background measurements.");
                        ui.add(egui::DragValue::new(&mut cfg.sleep_cycles).range(1..=0xffff));
                        ui.end_row();
                    }
                }
                ModuleConfig::LcdCam(cfg) => {
                    // The camera is the OTHER half and its own module, so this
                    // one chooses only between the two display shapes.
                    let camera = cfg.mode.is_camera();
                    if !camera {
                        ui.label("Mode");
                        egui::ComboBox::from_id_salt("lcd_mode")
                            .selected_text(cfg.mode.label())
                            .show_ui(ui, |ui| {
                                for v in [LcdCamMode::I8080, LcdCamMode::Dpi] {
                                    ui.selectable_value(&mut cfg.mode, v, v.label())
                                        .on_hover_text(v.hint());
                                }
                            })
                            .response
                            .on_hover_text(
                                "The display half is one or the other, never both - they \
                                 are two drivers over the same pads. The camera half runs \
                                 alongside either, as its own module.",
                            );
                        ui.end_row();
                    }

                    ui.label("Bus width");
                    egui::ComboBox::from_id_salt("lcd_width")
                        .selected_text(format!("{}-bit", cfg.width))
                        .show_ui(ui, |ui| {
                            for w in [8u8, 16] {
                                ui.selectable_value(&mut cfg.width, w, format!("{w}-bit"));
                            }
                        })
                        .response
                        .on_hover_text(
                            "How many data pads the driver binds. Assign D0..D15 on the \
                             canvas to match - only D0 and D1 are wired for you.",
                        );
                    ui.end_row();

                    // A camera in SLAVE mode is clocked by the sensor, so the
                    // number below changes nothing. Said, rather than hidden.
                    let slave_cam = cfg.mode.is_camera() && !cfg.master_clock;
                    ui.label("Pixel clock");
                    ui.horizontal(|ui| {
                        ui.add_enabled(
                            !slave_cam,
                            egui::DragValue::new(&mut cfg.clock_hz)
                                .range(100_000..=80_000_000)
                                .speed(100_000.0)
                                .custom_formatter(|v, _| hz_label(v as u32))
                                .suffix(""),
                        );
                        if slave_cam {
                            ui.label(
                                egui::RichText::new("the sensor supplies it")
                                    .size(11.0)
                                    .color(egui::Color32::GRAY),
                            );
                        }
                    });
                    ui.end_row();

                    if cfg.mode.is_camera() {
                        ui.label("Master clock");
                        ui.checkbox(&mut cfg.master_clock, "this chip clocks the sensor")
                            .on_hover_text(
                                "On, the MCLK pad drives the camera and the frequency above \
                                 is what it gets. Off is slave mode: the sensor is clocked \
                                 from elsewhere and this chip only reads.",
                            );
                        ui.end_row();
                    }

                    if cfg.mode == LcdCamMode::Dpi {
                        // An RGB panel has no controller: every one of these
                        // numbers comes off its datasheet, and a wrong one is a
                        // rolling picture rather than a build error.
                        ui.label("Active area");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut cfg.h_active).range(1..=4095));
                            ui.label("x");
                            ui.add(egui::DragValue::new(&mut cfg.v_active).range(1..=4095));
                            ui.label(
                                egui::RichText::new("px").size(11.0).color(egui::Color32::GRAY),
                            );
                        });
                        ui.end_row();

                        ui.label("Total")
                            .on_hover_text("Active area plus blanking - from the panel datasheet.");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut cfg.h_total).range(1..=4095));
                            ui.label("x");
                            ui.add(egui::DragValue::new(&mut cfg.v_total).range(1..=4095));
                        });
                        ui.end_row();

                        ui.label("Front porch");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut cfg.h_front_porch).range(0..=1023));
                            ui.label("x");
                            ui.add(egui::DragValue::new(&mut cfg.v_front_porch).range(0..=1023));
                        });
                        ui.end_row();

                        ui.label("Sync width");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut cfg.hsync_width).range(1..=1023));
                            ui.label("x");
                            ui.add(egui::DragValue::new(&mut cfg.vsync_width).range(1..=1023));
                        });
                        ui.end_row();
                    }

                    ui.label("Transfers");
                    ui.label(
                        egui::RichText::new("DMA, always")
                            .size(11.0)
                            .color(egui::Color32::GRAY),
                    )
                    .on_hover_text(
                        "Every LCD_CAM driver takes a channel in its only constructor: \
                         there is no CPU path. The Configuration tab shows which channel \
                         this one got.",
                    );
                    ui.end_row();
                }
                ModuleConfig::ParlIo(cfg) => {
                    ui.label("Direction");
                    egui::ComboBox::from_id_salt("parl_dir")
                        .selected_text(cfg.direction.label())
                        .show_ui(ui, |ui| {
                            for v in ParlIoDirection::ALL {
                                ui.selectable_value(&mut cfg.direction, v, v.label());
                            }
                        })
                        .response
                        .on_hover_text(
                            "The port moves data one way at a time. This also decides whether \
                             the data pads and the clock are outputs or inputs.",
                        );
                    ui.end_row();

                    ui.label("Bus width");
                    let widths = ParlIoWidth::options(family);
                    egui::ComboBox::from_id_salt("parl_width")
                        .selected_text(cfg.width.label())
                        .show_ui(ui, |ui| {
                            for v in widths.iter().copied() {
                                ui.selectable_value(&mut cfg.width, v, v.label());
                            }
                        })
                        .response
                        .on_hover_text(if widths.len() == 5 {
                            "How many data lines move together. Assign D0..D15 on the canvas \
                             to match - only D0 and the clock are wired for you."
                        } else {
                            "How many data lines move together. Sixteen is missing because \
                             esp-hal builds it only for the first generation of this \
                             peripheral, which this chip does not have."
                        });
                    ui.end_row();

                    ui.label("Clock");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut cfg.freq_hz)
                                .range(1_000..=40_000_000)
                                .suffix(" Hz")
                                .speed(1000.0),
                        );
                        for (lbl, hz) in [("1M", 1_000_000u32), ("10M", 10_000_000)] {
                            if ui.small_button(lbl).clicked() {
                                cfg.freq_hz = hz;
                            }
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "{} MB/s",
                                (u64::from(cfg.freq_hz) * u64::from(cfg.width.lanes()) / 8)
                                    as f64
                                    / 1_000_000.0
                            ))
                            .size(11.0)
                            .color(egui::Color32::GRAY),
                        );
                    })
                    .response
                    .on_hover_text(
                        "One transfer of the whole bus per clock, so the throughput is the \
                         clock times the width.",
                    );
                    ui.end_row();

                    ui.label("Bit order");
                    egui::ComboBox::from_id_salt("parl_order")
                        .selected_text(cfg.bit_order.label())
                        .show_ui(ui, |ui| {
                            for v in ParlIoBitOrder::ALL {
                                ui.selectable_value(&mut cfg.bit_order, v, v.label());
                            }
                        });
                    ui.end_row();

                    ui.label("DMA buffer");
                    ui.add(
                        egui::DragValue::new(&mut cfg.buffer_bytes)
                            .range(64..=32_768)
                            .suffix(" bytes"),
                    )
                    .on_hover_text(
                        "The block the DMA moves in one go. A parallel port has no non-DMA \
                         form - its constructor takes a channel.",
                    );
                    ui.end_row();
                }
                ModuleConfig::Mcpwm(cfg) => {
                    // One duty row per WIRED output: the six are not all in use,
                    // and a row for a pad nobody assigned sets nothing.
                    let mut wired: Vec<(u8, bool, &'static str)> = m
                        .connections
                        .iter()
                        .filter_map(|c| match c.signal {
                            ModuleSignal::McpwmA0 => Some((0u8, false, "OP0 A")),
                            ModuleSignal::McpwmB0 => Some((0, true, "OP0 B")),
                            ModuleSignal::McpwmA1 => Some((1, false, "OP1 A")),
                            ModuleSignal::McpwmB1 => Some((1, true, "OP1 B")),
                            ModuleSignal::McpwmA2 => Some((2, false, "OP2 A")),
                            ModuleSignal::McpwmB2 => Some((2, true, "OP2 B")),
                            _ => None,
                        })
                        .collect();
                    wired.sort();
                    let mut ops: Vec<u8> = wired.iter().map(|(op, _, _)| *op).collect();
                    ops.dedup();

                    // The unit has three timers and any operator can be on any
                    // of them, so an operator with a pad wired picks its own.
                    // With one operator there is nothing to choose, and the row
                    // would be a control that changes nothing.
                    if ops.len() > 1 {
                        for op in &ops {
                            ui.label(format!("OP{op} timer"));
                            let mut t = cfg.timer_of(*op);
                            egui::ComboBox::from_id_salt(format!("mcpwmtim{op}"))
                                .selected_text(format!("Timer {t}"))
                                .show_ui(ui, |ui| {
                                    for v in 0u8..3 {
                                        ui.selectable_value(&mut t, v, format!("Timer {v}"));
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "Operators on the SAME timer share its frequency and \
                                     period; on different timers they are independent. Two \
                                     motors at two speeds is what the three are for.",
                                );
                            cfg.op_timer[usize::from(*op).min(2)] = t;
                            ui.end_row();
                        }
                    }

                    // A frequency and a resolution per timer IN USE. An unused
                    // timer is never started, so showing its settings would be
                    // two more controls that change nothing.
                    let used = cfg.timers_used(&ops);
                    let one = used.len() == 1;
                    for t in &used {
                        let tag = if one {
                            String::new()
                        } else {
                            format!(" T{t}")
                        };
                        let (freq, period) = cfg.timer_mut(*t);
                        ui.label(format!("Frequency{tag}"));
                        let mut set: Option<u32> = None;
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(freq)
                                    .range(1..=1_000_000)
                                    .suffix(" Hz")
                                    .speed(100.0),
                            );
                            for (lbl, hz) in [("1k", 1_000u32), ("20k", 20_000), ("50k", 50_000)] {
                                if ui.small_button(lbl).clicked() {
                                    set = Some(hz);
                                }
                            }
                        })
                        .response
                        .on_hover_text(
                            "Shared by every operator on THIS timer. 20 kHz and up is above \
                             hearing, which is where a motor drive wants to be.",
                        );
                        if let Some(hz) = set {
                            *freq = hz;
                        }
                        ui.end_row();

                        ui.label(format!("Resolution{tag}"));
                        let steps = u32::from(*period) + 1;
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(period)
                                    .range(1..=65_534)
                                    .prefix("period "),
                            );
                            ui.label(
                                egui::RichText::new(format!("{steps} duty steps"))
                                    .size(11.0)
                                    .color(egui::Color32::GRAY),
                            );
                        })
                        .response
                        .on_hover_text(
                            "The timer's counter top. Duty can only land on one of these \
                             steps, and period x frequency is bounded by the peripheral \
                             clock - so a finer resolution costs frequency.",
                        );
                        ui.end_row();
                    }

                    for (op, is_b, lbl) in wired {
                        ui.label(format!("Duty {lbl}"));
                        let mut pct = f32::from(cfg.duty_x100_of(op, is_b)) / 100.0;
                        let steps = u32::from(cfg.timer_period(cfg.timer_of(op))) + 1;
                        if ui
                            .add(
                                egui::DragValue::new(&mut pct)
                                    .range(0.0..=100.0)
                                    .suffix(" %")
                                    .speed(0.5),
                            )
                            .on_hover_text(format!(
                                "Timestamp {} of {steps}, on timer {}",
                                cfg.timestamp_of(op, is_b),
                                cfg.timer_of(op),
                            ))
                            .changed()
                        {
                            cfg.duty_x100
                                .insert((op, is_b), (pct * 100.0).round() as u16);
                        }
                        ui.end_row();
                    }
                }
                ModuleConfig::Pcnt(cfg) => {
                    // A unit has two channels and each is wired on its own, so
                    // the rules are shown per WIRED channel. Channel 1 with no
                    // edge pad is not a channel — it gets no rows at all.
                    let wired: Vec<(u8, bool)> = [
                        (0u8, ModuleSignal::PcntEdgeSig, ModuleSignal::PcntCtrlSig),
                        (1, ModuleSignal::PcntEdgeSig1, ModuleSignal::PcntCtrlSig1),
                    ]
                    .into_iter()
                    .filter(|(_, edge, _)| m.connections.iter().any(|c| c.signal == *edge))
                    .map(|(ch, _, ctrl)| {
                        (ch, m.connections.iter().any(|c| c.signal == ctrl))
                    })
                    .collect();
                    // With one channel there is nothing to disambiguate, so the
                    // rows keep the names they always had.
                    let one = wired.len() <= 1;

                    for (ch, has_ctrl) in &wired {
                        let tag = if one {
                            String::new()
                        } else {
                            format!(" ch{ch}")
                        };
                        let mut k = cfg.channel(*ch);

                        ui.label(format!("Counts{tag}"));
                        ui.horizontal(|ui| {
                            for (lbl, mode, id) in [
                                ("Rising", &mut k.pos_edge, format!("pcnt_pos{ch}")),
                                ("Falling", &mut k.neg_edge, format!("pcnt_neg{ch}")),
                            ] {
                                ui.label(egui::RichText::new(lbl).size(11.0));
                                egui::ComboBox::from_id_salt(id)
                                    .width(96.0)
                                    .selected_text(mode.label())
                                    .show_ui(ui, |ui| {
                                        for v in PcntEdgeMode::ALL {
                                            ui.selectable_value(mode, v, v.label());
                                        }
                                    });
                            }
                        })
                        .response
                        .on_hover_text(
                            "What each edge does to the counter. Counting BOTH edges \
                             doubles the resolution and halves the range. Both channels \
                             add into the SAME counter.",
                        );
                        ui.end_row();

                        ui.label(format!("Control input{tag}"));
                        ui.add_enabled_ui(*has_ctrl, |ui| {
                            ui.horizontal(|ui| {
                                for (lbl, mode, id) in [
                                    ("Low", &mut k.ctrl_low, format!("pcnt_cl{ch}")),
                                    ("High", &mut k.ctrl_high, format!("pcnt_ch{ch}")),
                                ] {
                                    ui.label(egui::RichText::new(lbl).size(11.0));
                                    egui::ComboBox::from_id_salt(id)
                                        .width(104.0)
                                        .selected_text(mode.label())
                                        .show_ui(ui, |ui| {
                                            for v in PcntCtrlMode::ALL {
                                                ui.selectable_value(mode, v, v.label());
                                            }
                                        });
                                }
                            });
                        })
                        .response
                        .on_hover_text(if *has_ctrl {
                            "What the control input's level does. Set one side to Reverse \
                             and the unit follows an encoder's direction on its own."
                        } else {
                            "No control pad on this channel. Assign a PCNT CTRL pin on the \
                             canvas - it is what turns the counter into an encoder."
                        });
                        ui.end_row();

                        cfg.set_channel(*ch, k);
                    }

                    // The second channel is the other half of a quadrature
                    // encoder, and nothing else says so.
                    if one {
                        ui.label("Second channel");
                        ui.label(
                            egui::RichText::new("assign PCNT EDGE1 on the canvas")
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        )
                        .on_hover_text(
                            "The unit's other channel, with its own edge and control pads \
                             and its own rules, counting into the same counter. Two \
                             channels with opposite rules is a quadrature encoder.",
                        );
                        ui.end_row();
                    }

                    ui.label("Limits");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut cfg.low_limit)
                                .range(-32_767..=0)
                                .prefix("low "),
                        );
                        ui.add(
                            egui::DragValue::new(&mut cfg.high_limit)
                                .range(0..=32_767)
                                .prefix("high "),
                        );
                    })
                    .response
                    .on_hover_text(
                        "The counter is signed 16-bit. Reaching a limit CLEARS it and \
                         raises an event - which is how a count wider than 16 bits is \
                         accumulated.",
                    );
                    ui.end_row();

                    ui.label("Glitch filter");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut cfg.filter)
                                .range(0..=1023)
                                .suffix(" APB clocks"),
                        );
                        if cfg.filter == 0 {
                            ui.label(
                                egui::RichText::new("off")
                                    .size(11.0)
                                    .color(egui::Color32::GRAY),
                            );
                        }
                    })
                    .response
                    .on_hover_text(
                        "Pulses shorter than this are ignored - the difference between \
                         counting a contact bounce once and counting it eight times. The \
                         hardware caps it at 1023.",
                    );
                    ui.end_row();
                }
                ModuleConfig::Rmt(cfg) => {
                    ui.label("Direction");
                    let dirs = RmtDirection::options(family, cfg.instance);
                    let locked = dirs.len() == 1;
                    ui.add_enabled_ui(!locked, |ui| {
                        egui::ComboBox::from_id_salt("rmt_dir")
                            .selected_text(cfg.direction.label())
                            .show_ui(ui, |ui| {
                                for v in dirs.iter().copied() {
                                    ui.selectable_value(&mut cfg.direction, v, v.label());
                                }
                            });
                    })
                    .response
                    .on_hover_text(if locked {
                        "Fixed in silicon on this chip: the low RMT channels transmit and the \
                         high ones receive. Pick a different channel to change direction."
                    } else {
                        "This chip's RMT channels can each do either."
                    });
                    ui.end_row();

                    ui.label("Clock divider");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut cfg.clk_divider)
                                .range(1..=255)
                                .prefix("/"),
                        );
                        // The tick is what every duration in a pulse train is
                        // counted in, so showing it is more useful than the
                        // divider that produced it.
                        let src = rmt_source_hz(family);
                        let tick_ns = 1_000_000_000f64 / (src as f64 / cfg.clk_divider as f64);
                        ui.label(
                            egui::RichText::new(format!("1 tick = {tick_ns:.0} ns"))
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        );
                    })
                    .response
                    .on_hover_text(
                        "Divides the RMT source clock. A smaller divider gives finer \
                         resolution and a shorter longest pulse; the two trade off.",
                    );
                    ui.end_row();

                    if cfg.direction.is_tx() {
                        ui.label("Idle level");
                        egui::ComboBox::from_id_salt("rmt_idle")
                            .selected_text(if cfg.idle_high { "High" } else { "Low" })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut cfg.idle_high, false, "Low");
                                ui.selectable_value(&mut cfg.idle_high, true, "High");
                            })
                            .response
                            .on_hover_text("Where the pad rests between trains.");
                        ui.end_row();
                    } else {
                        ui.label("Idle threshold");
                        ui.add(
                            egui::DragValue::new(&mut cfg.idle_threshold)
                                .range(1..=65535)
                                .suffix(" ticks"),
                        )
                        .on_hover_text(
                            "How long the line must rest before the frame counts as over. \
                             Too short and one train is read as several.",
                        );
                        ui.end_row();
                    }

                    ui.label("Carrier");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut cfg.carrier, "");
                        ui.add_enabled_ui(cfg.carrier, |ui| {
                            ui.add(
                                egui::DragValue::new(&mut cfg.carrier_hz)
                                    .range(1_000..=1_000_000)
                                    .suffix(" Hz")
                                    .speed(100.0),
                            );
                            if ui.small_button("38k").clicked() {
                                cfg.carrier_hz = 38_000;
                            }
                        });
                    })
                    .response
                    .on_hover_text(
                        "Modulate the pulses onto a carrier — 38 kHz is what an IR receiver \
                         demodulates. Off for WS2812 and 1-Wire, which want the edges raw.",
                    );
                    ui.end_row();
                }
                // LPUART reuses the USART settings struct, so it reuses this
                // whole arm — only the DMA request table differs (`uart_bus`).
                ModuleConfig::Usart(cfg) | ModuleConfig::Lpuart(cfg) => {
                    ui.label("Baud rate");
                    egui::ComboBox::from_id_salt("baud")
                        .selected_text(cfg.baud_rate.to_string())
                        .show_ui(ui, |ui| {
                            for b in [9600u32, 19200, 38400, 57600, 115200, 230400, 460800, 921600]
                            {
                                ui.selectable_value(&mut cfg.baud_rate, b, b.to_string());
                            }
                        });
                    ui.end_row();
                    // Right after baud rate, because the two are read together:
                    // the buffer is only meaningful as "how many bytes at this
                    // speed". Async only - the blocking paths have no buffer -
                    // and hidden on a TX-only DMA link, which has none either.
                    let tx_only_dma =
                        cfg.mode == UsartMode::Dma && cfg.direction == UsartDirection::TxOnly;
                    if is_async && !tx_only_dma {
                        let dma = cfg.mode == UsartMode::Dma;
                        ui.label(if dma { "RX DMA buffer" } else { "RX/TX buffer" });
                        ui.horizontal(|ui| {
                            // A drag value, not a combo: this is a number the
                            // user sizes against their own read cadence, and a
                            // list of powers of two would only pretend to know
                            // it. Clamped to what the generated code can hold.
                            ui.add(
                                egui::DragValue::new(&mut cfg.buf_len)
                                    .speed(16.0)
                                    .range(16..=65_536)
                                    .suffix(" B"),
                            );
                            let ms = cfg.buf_len as f32 * 10.0 / cfg.baud_rate.max(1) as f32
                                * 1000.0;
                            ui.label(
                                egui::RichText::new(format!("~{ms:.0} ms at {} baud", cfg.baud_rate))
                                    .size(10.5)
                                    .color(egui::Color32::from_gray(130)),
                            );
                        })
                        .response
                        .on_hover_text(if dma {
                            "The circular buffer the DMA controller fills on its own. Reception                              never stops, so this only has to cover the longest GAP between your                              reads - overrun it and the OLDEST bytes are dropped, silently."
                        } else {
                            "Size of both software ring buffers, TX and RX. The CPU copies byte                              by byte on each interrupt, so this has to cover what arrives between                              your reads."
                        });
                        ui.end_row();
                    }
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
                    // Blocking → editable Portable|Native; Native runtime → shown
                    // locked on Native; async USART → hidden (always the
                    // embedded-io-async BufferedUart bridge, no choice).
                    if esp {
                        api_row(ui, &mut pending.0);
                        // Direction and flow control are NOT async concepts on
                        // an ESP — `UartTx::new` and `.with_cts()` are there on
                        // either runtime — so they show whatever the runtime is.
                        // They were unreachable while the ESP shared embassy's
                        // async gate.
                        direction_row(ui, cfg);
                        flow_row(ui, cfg, wired_flow);
                        if !UsartDirection::options_for(cfg.mode, family)
                            .contains(&cfg.direction)
                        {
                            cfg.direction = UsartDirection::TxRx;
                        }
                        if !UsartFlow::options_for(cfg.mode, cfg.direction, family)
                            .contains(&cfg.flow)
                        {
                            cfg.flow = UsartFlow::None;
                        }
                        // No transport row: esp-hal's UART DMA is UHCI, a
                        // different driver this generator does not write yet.
                        // Said rather than left blank — the SPI beside it has
                        // the row, and the difference is not obvious.
                        ui.label("Transfers");
                        ui.label(
                            egui::RichText::new("CPU (blocking)")
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        )
                        .on_hover_text(
                            "esp-hal moves UART bytes over DMA through UHCI, a driver of \
                             its own that this generator does not write yet. The SPI \
                             module's DMA is unrelated and does work.",
                        );
                        ui.end_row();
                    } else if is_native {
                        api_row_locked(ui);
                    } else if is_async {
                        // The API style is fixed on async (embedded-io-async
                        // either way); what IS a choice is the transport.
                        usart_mode_row(ui, &mut cfg.mode);
                        // The transport decides which directions exist, and the
                        // pair decides which flow options do — so this order is
                        // load-bearing, not cosmetic.
                        direction_row(ui, cfg);
                        flow_row(ui, cfg, wired_flow);
                        // A direction the new transport cannot build, or a flow
                        // option the new pair cannot, would otherwise sit there
                        // as a stale choice that generates nothing.
                        if !UsartDirection::options_for(cfg.mode, family)
                            .contains(&cfg.direction)
                        {
                            cfg.direction = UsartDirection::TxRx;
                        }
                        if !UsartFlow::options_for(cfg.mode, cfg.direction, family)
                            .contains(&cfg.flow)
                        {
                            cfg.flow = UsartFlow::None;
                        }
                        if cfg.mode == UsartMode::Dma {
                            let inst = cfg.instance;
                            dma_row(ui, uart_bus, inst, &mut cfg.dma_tx, &mut cfg.dma_rx);
                        }
                    } else if let Some(chans) =
                        codegen::stm32::blocking_dma_channels(family, uart_bus, cfg.instance)
                    {
                        f1_serial_note(ui);
                        f1_half_bus_note(
                            ui,
                            "USART",
                            ("TX", "RX"),
                            "stm32f1xx-hal builds a Serial only from the TX+RX pair",
                            wired_serial,
                        );
                        transport_row(ui, &mut cfg.blocking_dma, &chans, true);
                        if cfg.blocking_dma.any() {
                            api_row_locked_dma(ui);
                        } else {
                            api_row(ui, &mut pending.0);
                        }
                    } else {
                        api_row(ui, &mut pending.0);
                    }
                }
                ModuleConfig::Spi(cfg) => {
                    let roles = SpiRole::options(family);
                    if roles.len() > 1 {
                        ui.label("Role");
                        egui::ComboBox::from_id_salt("spirole")
                            .selected_text(cfg.role.label())
                            .show_ui(ui, |ui| {
                                for v in roles.iter().copied() {
                                    ui.selectable_value(&mut cfg.role, v, v.label());
                                }
                            })
                            .response
                            .on_hover_text(
                                "One peripheral, either end of the bus. As a SLAVE this chip \
                                 does not drive the clock or the chip select - the other side \
                                 does, and nothing moves until it asserts CS.",
                            );
                        ui.end_row();
                    }
                    let slave = cfg.role.is_slave();
                    // The ESP32's slave takes modes 1 and 3 only, and its own
                    // driver says so. Offering 0 and 2 there would be two
                    // settings that configure the peripheral wrong.
                    let modes = cfg.role.modes(family);
                    if !modes.contains(&cfg.mode) {
                        cfg.mode = modes[0];
                    }
                    ui.label("SPI mode");
                    egui::ComboBox::from_id_salt("spimode")
                        .selected_text(format!("Mode {}", cfg.mode))
                        .show_ui(ui, |ui| {
                            for md in modes.iter().copied() {
                                ui.selectable_value(&mut cfg.mode, md, format!("Mode {md}"));
                            }
                        })
                        .response
                        .on_hover_text(if modes.len() < 4 {
                            "This chip's SPI slave supports modes 1 and 3 only."
                        } else if slave {
                            "Must match what the master drives."
                        } else {
                            "CPOL/CPHA. The device on the other end states which it wants."
                        });
                    ui.end_row();
                    // Async only: the STM32F1 HAL's SPI takes no bit-order
                    // argument, so the field would be a control that does
                    // nothing there.
                    if is_async {
                        ui.label("Bit order");
                        egui::ComboBox::from_id_salt("spibitorder")
                            .selected_text(match cfg.bit_order {
                                SpiBitOrder::MsbFirst => "MSB first",
                                SpiBitOrder::LsbFirst => "LSB first",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut cfg.bit_order,
                                    SpiBitOrder::MsbFirst,
                                    "MSB first",
                                )
                                .on_hover_text(
                                    "What nearly every device expects, and embassy's default.",
                                );
                                ui.selectable_value(
                                    &mut cfg.bit_order,
                                    SpiBitOrder::LsbFirst,
                                    "LSB first",
                                )
                                .on_hover_text(
                                    "Some sensors and shift registers. Getting this wrong gives                                      bit-reversed data rather than silence, which is why it is                                      worth setting deliberately.",
                                );
                            });
                        ui.end_row();
                    }
                    // A slave has no clock of its own: the master supplies it.
                    // Shown as a line rather than hidden, or the row would just
                    // vanish and leave the reader wondering.
                    if slave {
                        ui.label("Clock");
                        ui.label(
                            egui::RichText::new("driven by the master")
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        );
                        ui.end_row();
                        ui.label("Transfers");
                        ui.label(
                            egui::RichText::new("DMA, always")
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        )
                        .on_hover_text(
                            "esp-hal's slave driver has no CPU path and no blocking transfer: \
                             the master decides when bytes move, so a channel is taken \
                             whatever the runtime. The Configuration tab shows which.",
                        );
                        ui.end_row();
                        return;
                    }
                    ui.label("Clock");
                    egui::ComboBox::from_id_salt("spiclk")
                        .selected_text(hz_label(cfg.clock_hz))
                        .show_ui(ui, |ui| {
                            for hz in [
                                125_000u32, 250_000, 500_000, 1_000_000, 2_000_000, 4_000_000,
                                8_000_000,
                            ] {
                                ui.selectable_value(&mut cfg.clock_hz, hz, hz_label(hz));
                            }
                        });
                    ui.end_row();
                    if esp {
                        api_row(ui, &mut pending.0);
                        // Not a runtime choice: `with_dma` is on
                        // `impl Spi<'d, Blocking>` and hands back a
                        // `SpiDma<'d, Blocking>`, so a blocking project puts a
                        // master on DMA exactly as an async one does.
                        esp_dma_row(ui, &mut pending.1);
                        if pending.1 == AsyncBusMode::AsyncDma {
                            // ONE channel per bus on an ESP: `with_dma` drives
                            // both directions from it, unlike embassy's pair.
                            let req = format!("SPI{}", cfg.instance);
                            esp_dma_channel_row(ui, dma, &req, &mut cfg.dma_tx);
                        }
                    } else if is_async {
                        async_row(ui, &mut pending.1);
                        if pending.1 == AsyncBusMode::AsyncDma {
                            let inst = cfg.instance;
                            dma_row(
                                ui,
                                dma_map::Bus::Spi,
                                inst,
                                &mut cfg.dma_tx,
                                &mut cfg.dma_rx,
                            );
                        }
                    } else if is_native {
                        api_row_locked(ui);
                    } else if let Some(chans) = codegen::stm32::blocking_dma_channels(
                        family,
                        dma_map::Bus::Spi,
                        cfg.instance,
                    ) {
                        transport_row(ui, &mut cfg.blocking_dma, &chans, has_miso);
                        if cfg.blocking_dma.any() {
                            api_row_locked_dma(ui);
                        } else {
                            api_row(ui, &mut pending.0);
                        }
                    } else {
                        api_row(ui, &mut pending.0);
                    }
                }
                // One TIMER: the frequency it shares, then a duty slider per
                // channel actually wired — the channel list comes from the
                // module's own connections, so it mirrors the canvas.
                ModuleConfig::Timer(cfg) => {
                    ui.label("Frequency");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut cfg.freq_hz)
                                .range(1..=1_000_000)
                                .suffix(" Hz")
                                .speed(10.0),
                        );
                        for (label, hz) in [("50 Hz", 50u32), ("1 kHz", 1_000), ("20 kHz", 20_000)]
                        {
                            if ui.small_button(label).clicked() {
                                cfg.freq_hz = hz;
                            }
                        }
                    });
                    ui.end_row();
                    // The counter belongs to the TIMER, like the frequency —
                    // one counter, one shape. embassy takes it in
                    // `SimplePwm::new`; the note below says why the other
                    // runtimes do not show it.
                    if is_async {
                        ui.label("Counter");
                        egui::ComboBox::from_id_salt("pwm_counting")
                            .selected_text(cfg.counting.label())
                            .show_ui(ui, |ui| {
                                for m in PwmCounting::ALL {
                                    ui.selectable_value(&mut cfg.counting, m, m.label());
                                }
                            })
                            .response
                            .on_hover_text(
                                "Center-aligned is what motor drive wants: the pulse sits in the                                  middle of the period, so several channels do not all switch at                                  the same instant. The three centred modes differ only in when                                  the compare interrupt fires.",
                            );
                        ui.end_row();
                    }
                    // The pads wired to this module, grouped by CHANNEL: the
                    // duty is the channel's, and a channel can own two pads —
                    // CHx and its complementary CHxN. Both come from the
                    // signals' own labels, so these rows and the hint below
                    // cannot disagree.
                    let wired: BTreeSet<String> =
                        conn_rows.iter().map(|(sig, _)| (*sig).to_owned()).collect();
                    let mut pads: BTreeMap<u8, Vec<String>> = BTreeMap::new();
                    for (sig, pin) in &conn_rows {
                        let digits = sig.strip_prefix("CH").map(|r| r.trim_end_matches('N'));
                        if let Some(ch) = digits.and_then(|d| d.parse::<u8>().ok()) {
                            pads.entry(ch).or_default().push(format!("{sig} {pin}"));
                        }
                    }
                    if pads.is_empty() {
                        ui.label("Channels");
                        ui.label(
                            egui::RichText::new("none wired yet")
                                .size(11.0)
                                .italics()
                                .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                    for (ch, pins) in &pads {
                        let (ch, sig) = (*ch, format!("CH{ch}"));
                        let pin = pins.join(", ");
                        ui.label(format!("{sig} duty  ({pin})"));
                        // Percent with two decimals, stored as hundredths: a
                        // servo's 1.5 ms of a 20 ms frame is 7.5 %, and whole
                        // percent could not say it. Dragging lands on
                        // hundredths too, so the handle and the typed value
                        // agree.
                        let mut pct = cfg.duty_percent_of(ch);
                        if ui
                            .add(
                                egui::Slider::new(&mut pct, 0.0..=100.0)
                                    .suffix(" %")
                                    .fixed_decimals(2)
                                    .step_by(0.01),
                            )
                            .changed()
                        {
                            cfg.set_duty_x100(ch, (pct * 100.0).round().max(0.0) as u16);
                        }
                        ui.end_row();

                        if is_async {
                            // Edited on a COPY and written back only on a real
                            // change, so opening the panel does not fill the
                            // config with rows of defaults.
                            let before = cfg.channel_of(ch);
                            let mut shape = before;
                            ui.label(format!("{sig} output"));
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt(("pwm_drive", ch))
                                    .width(88.0)
                                    .selected_text(shape.output.label())
                                    .show_ui(ui, |ui| {
                                        for v in PwmOutput::ALL {
                                            ui.selectable_value(&mut shape.output, v, v.label());
                                        }
                                    });
                                egui::ComboBox::from_id_salt(("pwm_pol", ch))
                                    .width(88.0)
                                    .selected_text(shape.polarity.label())
                                    .show_ui(ui, |ui| {
                                        for v in PwmPolarity::ALL {
                                            ui.selectable_value(&mut shape.polarity, v, v.label());
                                        }
                                    })
                                    .response
                                    .on_hover_text(
                                        "Active low inverts the pin: 100 % duty then HOLDS IT                                          LOW, which is what a current-sinking driver stage wants.",
                                    );
                                egui::ComboBox::from_id_salt(("pwm_mode", ch))
                                    .width(96.0)
                                    .selected_text(shape.mode.label())
                                    .show_ui(ui, |ui| {
                                        for v in PwmMode::ALL {
                                            ui.selectable_value(&mut shape.mode, v, v.label());
                                        }
                                    })
                                    .response
                                    .on_hover_text(
                                        "Mode 2 reverses the comparison — a second route to the                                          same inversion the polarity offers. CubeMX exposes both.",
                                    );
                            });
                            if shape != before {
                                cfg.set_channel(ch, shape);
                            }
                            ui.end_row();
                        }
                    }
                    // Channels are not added from this panel: the codegen
                    // derives them from the PIN functions, so the only way to
                    // gain one is to assign a pad on the canvas. Name the ones
                    // actually left — the old line hardcoded "CH2/3/4", which
                    // pointed at channels that could already be taken, never
                    // mentioned a freed CH1, offered channels the timer does
                    // not have, and kept advertising the action after all of
                    // them were wired.
                    // Dead time only matters once a pair exists, and only the
                    // async backend can emit it — see the note further down.
                    // `starts_with("CH")` matters: BKIN ends in an N too, and it
                    // is a fault input, not half of a pair.
                    let has_comp = wired
                        .iter()
                        .any(|s| s.starts_with("CH") && s.ends_with('N'));
                    if has_comp && is_async {
                        if is_advanced_timer(cfg.instance) {
                            ui.label("Dead time");
                            ui.add(
                                egui::DragValue::new(&mut cfg.dead_time)
                                    .range(0..=u16::MAX)
                                    .suffix(" ticks"),
                            )
                            .on_hover_text(
                                "Ticks on the same scale as the duty compare value; embassy                                  encodes them into the timer's CKD + DTG fields. 0 means the two                                  pads switch at the same instant — fine for independent loads,                                  fatal for a half-bridge.",
                            );
                            ui.end_row();
                        } else {
                            ui.label("");
                            ui.label(
                                egui::RichText::new(format!(
                                    "TIM{} has complementary pads wired, but embassy drives them                                      through `ComplementaryPwm`, which covers the advanced-control                                      timers (TIM1/8/20) only — they will not be initialised",
                                    cfg.instance
                                ))
                                .size(10.5)
                                .color(egui::Color32::from_rgb(200, 140, 60)),
                            );
                            ui.end_row();
                        }
                    }
                    // ── Break inputs ────────────────────────────────────
                    // A fault line the timer watches by itself: when it asserts,
                    // every output goes off in hardware. Only shown for a pad
                    // that is actually wired — break is not something to switch
                    // on in the abstract.
                    let breaks: Vec<u8> = wired
                        .iter()
                        .filter_map(|s| match s.as_str() {
                            "BKIN" => Some(1u8),
                            "BKIN2" => Some(2),
                            _ => None,
                        })
                        .collect();
                    if !breaks.is_empty() && is_async && is_advanced_timer(cfg.instance) {
                        for i in &breaks {
                            let before = cfg.break_of(*i);
                            let mut b = before;
                            let name = if *i == 1 { "BKIN" } else { "BKIN2" };
                            ui.label(format!("{name} fault"));
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt(("brk_pol", i))
                                    .width(96.0)
                                    .selected_text(b.polarity.label())
                                    .show_ui(ui, |ui| {
                                        for v in BreakPolarity::ALL {
                                            ui.selectable_value(&mut b.polarity, v, v.label());
                                        }
                                    })
                                    .response
                                    .on_hover_text(
                                        "Which level on the pad means fault. Active low is the \
                                         usual wiring: the line is released when all is well, so \
                                         a broken wire also reads as a fault.",
                                    );
                                egui::ComboBox::from_id_salt(("brk_filt", i))
                                    .width(110.0)
                                    .selected_text(b.filter_label())
                                    .show_ui(ui, |ui| {
                                        for (code, (_, label)) in BREAK_FILTERS.iter().enumerate() {
                                            ui.selectable_value(&mut b.filter, code as u8, *label);
                                        }
                                    })
                                    .response
                                    .on_hover_text(
                                        "How many consecutive samples must agree before the fault \
                                         is believed, and how fast they are taken. No filter \
                                         reacts fastest and trusts every glitch.",
                                    );
                            });
                            if b != before {
                                cfg.set_break(*i, b);
                            }
                            ui.end_row();
                        }
                        ui.label("After a fault");
                        ui.checkbox(
                            &mut cfg.auto_output_enable,
                            "outputs come back on their own",
                        )
                        .on_hover_text(
                            "Off (the reset state, and the safer one): the outputs stay dark \
                             until software turns them back on, so a fault cannot be ridden out \
                             unnoticed. On: they resume at the next update event once the line \
                             releases.",
                        );
                        ui.end_row();
                    }
                    if family.starts_with("esp") {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(
                                "esp-hal's LEDC takes duty in WHOLE percent — a fraction is                                  rounded up in the generated file",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                    let free = free_pwm_channels(cfg.instance, &wired, pin_funcs);
                    if !free.is_empty() {
                        let list = free.join(" / ");
                        ui.label("");
                        ui.label(
                            egui::RichText::new(format!(
                                "add a channel by assigning TIM{} {list} on the canvas",
                                cfg.instance
                            ))
                            .size(10.5)
                            .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                    // Say why the output controls are absent instead of just
                    // dropping them: the reason differs per backend, and the
                    // second one is worth knowing before wiring a pad.
                    // The ESP is the exception: its LEDC driver is the same on
                    // both runtimes, so there is nothing missing to explain.
                    if !is_async && !family.starts_with("esp") {
                        ui.label("");
                        let why = if family == "stm32f1" {
                            "counter mode, drive, polarity and PWM mode need the Async runtime                              — stm32f1xx-hal's `pwm_hz` cannot set them"
                        } else {
                            "this runtime emits no PWM code at all — only Async generates it                              (System tab)"
                        };
                        ui.label(
                            egui::RichText::new(why)
                                .size(10.5)
                                .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                }
                // The HSPI. The smallest panel of the four external-memory
                // controllers, because the driver is: two widths, and the octal
                // one needs its strobe.
                ModuleConfig::Hspi(cfg) => {
                    let lanes = conn_rows
                        .iter()
                        .filter(|(sig, _)| sig.starts_with("IO"))
                        .count() as u8;
                    let dqs0 = conn_rows.iter().any(|(sig, _)| *sig == "DQS0");

                    ui.label("Mode");
                    let fits: Vec<HspiMode> = HspiMode::ALL
                        .into_iter()
                        .filter(|m| m.lanes() == lanes)
                        .collect();
                    if fits.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{lanes} data line(s) wired — embassy's HSPI builds 2 or 8, and \
                                 nothing between"
                            ))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(200, 140, 60)),
                        );
                    } else {
                        egui::ComboBox::from_id_salt("hspi_mode")
                            .selected_text(cfg.mode.label())
                            .show_ui(ui, |ui| {
                                for m in &fits {
                                    ui.selectable_value(&mut cfg.mode, *m, m.label());
                                }
                            });
                        if !fits.contains(&cfg.mode) {
                            cfg.mode = fits[0];
                        }
                    }
                    ui.end_row();

                    if cfg.mode == HspiMode::Octal && !dqs0 {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(
                                "the octal call REQUIRES DQS0 — wire it, or nothing is generated",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_rgb(200, 140, 60)),
                        );
                        ui.end_row();
                    }

                    ui.label("Device");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("hspi_size")
                            .width(84.0)
                            .selected_text(cfg.size_label())
                            .show_ui(ui, |ui| {
                                for (i, name) in QSPI_MEMORY_SIZES.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut cfg.device_size,
                                        i as u8,
                                        name.trim_start_matches('_'),
                                    );
                                }
                            });
                        egui::ComboBox::from_id_salt("hspi_type")
                            .width(130.0)
                            .selected_text(cfg.memory_type.label())
                            .show_ui(ui, |ui| {
                                for v in OspiMemoryType::ALL {
                                    ui.selectable_value(&mut cfg.memory_type, v, v.label());
                                }
                            });
                        ui.add(
                            egui::DragValue::new(&mut cfg.prescaler)
                                .range(0..=255)
                                .prefix("clk / "),
                        );
                    });
                    ui.end_row();

                    ui.label("");
                    ui.label(
                        egui::RichText::new(if is_async {
                            "the silicon carries IO0-IO15 and a second strobe, but embassy's \
                             driver stops at eight lines — the wider pads have no constructor"
                        } else {
                            "only the Async runtime emits HSPI code — the blocking backends \
                             generate GPIO and watchdogs only"
                        })
                        .size(10.5)
                        .color(egui::Color32::from_gray(140)),
                    );
                    ui.end_row();
                }
                // One XSPI port — the OCTOSPI panel one step wider, with the
                // strobes derived from the wiring rather than asked for.
                ModuleConfig::Xspi(cfg) => {
                    let lanes = conn_rows
                        .iter()
                        .filter(|(sig, _)| sig.starts_with("IO"))
                        .count() as u8;
                    let dqs0 = conn_rows.iter().any(|(sig, _)| *sig == "DQS0");
                    let dqs1 = conn_rows.iter().any(|(sig, _)| *sig == "DQS1");

                    ui.label("Mode");
                    let fits: Vec<XspiMode> = XspiMode::ALL
                        .into_iter()
                        .filter(|m| m.lanes() == lanes)
                        .collect();
                    if fits.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{lanes} data line(s) wired — the controller takes 2, 4, 8 or 16"
                            ))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(200, 140, 60)),
                        );
                    } else {
                        egui::ComboBox::from_id_salt("xspi_mode")
                            .selected_text(cfg.mode.label())
                            .show_ui(ui, |ui| {
                                for m in &fits {
                                    ui.selectable_value(&mut cfg.mode, *m, m.label());
                                }
                            })
                            .response
                            .on_hover_text(
                                "Only the modes your wiring can carry. Single and dual share two \
                                 pads, octal and dual-quad share eight — the pins cannot tell \
                                 them apart, so this asks.",
                            );
                        if !fits.contains(&cfg.mode) {
                            cfg.mode = fits[0];
                        }
                    }
                    ui.end_row();

                    ui.label("Device");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("xspi_size")
                            .width(84.0)
                            .selected_text(cfg.size_label())
                            .show_ui(ui, |ui| {
                                for (i, name) in QSPI_MEMORY_SIZES.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut cfg.device_size,
                                        i as u8,
                                        name.trim_start_matches('_'),
                                    );
                                }
                            });
                        egui::ComboBox::from_id_salt("xspi_type")
                            .width(140.0)
                            .selected_text(cfg.memory_type.label())
                            .show_ui(ui, |ui| {
                                for v in XspiMemoryType::ALL {
                                    ui.selectable_value(&mut cfg.memory_type, v, v.label());
                                }
                            });
                        ui.add(
                            egui::DragValue::new(&mut cfg.prescaler)
                                .range(0..=255)
                                .prefix("clk / "),
                        );
                    });
                    ui.end_row();

                    if dqs0 {
                        ui.label("Strobe");
                        let text = if !cfg.mode.takes_dqs() {
                            "wired, but only the octal and hexadeca modes read it — it will be \
                             left out of the call"
                                .to_owned()
                        } else if dqs1 && cfg.mode == XspiMode::Hexa {
                            "DQS0 + DQS1 — the dual-strobe hexadeca call".to_owned()
                        } else if dqs1 {
                            "DQS1 needs the hexadeca mode; only DQS0 will be used".to_owned()
                        } else {
                            "DQS0".to_owned()
                        };
                        let warn = !cfg.mode.takes_dqs() || (dqs1 && cfg.mode != XspiMode::Hexa);
                        ui.label(
                            egui::RichText::new(text).size(10.5).color(if warn {
                                egui::Color32::from_rgb(200, 140, 60)
                            } else {
                                egui::Color32::from_gray(160)
                            }),
                        );
                        ui.end_row();
                    }
                    if !is_async {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(
                                "only the Async runtime emits XSPI code — the blocking backends \
                                 generate GPIO and watchdogs only",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                }
                // One OCTOSPI port. The width narrows the mode but does not
                // decide it: single and dual share two pads, octal and dual-quad
                // share eight — so the mode is asked for, and only the modes the
                // wiring can carry are offered.
                ModuleConfig::Ospi(cfg) => {
                    let lanes = conn_rows
                        .iter()
                        .filter(|(sig, _)| sig.starts_with("IO"))
                        .count() as u8;
                    let dqs = conn_rows.iter().any(|(sig, _)| *sig == "DQS");

                    ui.label("Mode");
                    let fits: Vec<OspiMode> = OspiMode::ALL
                        .into_iter()
                        .filter(|m| m.lanes() == lanes)
                        .collect();
                    if fits.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{lanes} data line(s) wired — the controller takes 2, 4 or 8"
                            ))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(200, 140, 60)),
                        );
                    } else {
                        egui::ComboBox::from_id_salt("ospi_mode")
                            .selected_text(cfg.mode.label())
                            .show_ui(ui, |ui| {
                                for m in &fits {
                                    ui.selectable_value(&mut cfg.mode, *m, m.label());
                                }
                            })
                            .response
                            .on_hover_text(
                                "Only the modes your wiring can carry. Single and dual use the \
                                 same two pads, octal and dual-quad the same eight — the pins \
                                 cannot tell them apart, so this asks.",
                            );
                        // Keep the setting inside what is wired: a mode the pads
                        // cannot carry would be refused at generation anyway.
                        if !fits.contains(&cfg.mode) {
                            cfg.mode = fits[0];
                        }
                    }
                    ui.end_row();

                    ui.label("Device");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("ospi_size")
                            .width(84.0)
                            .selected_text(cfg.size_label())
                            .show_ui(ui, |ui| {
                                for (i, name) in QSPI_MEMORY_SIZES.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut cfg.device_size,
                                        i as u8,
                                        name.trim_start_matches('_'),
                                    );
                                }
                            });
                        egui::ComboBox::from_id_salt("ospi_type")
                            .width(130.0)
                            .selected_text(cfg.memory_type.label())
                            .show_ui(ui, |ui| {
                                for v in OspiMemoryType::ALL {
                                    ui.selectable_value(&mut cfg.memory_type, v, v.label());
                                }
                            })
                            .response
                            .on_hover_text(
                                "The device family changes how commands are framed. Standard \
                                 covers ordinary NOR flash; HyperBus is a different protocol \
                                 altogether.",
                            );
                        ui.add(
                            egui::DragValue::new(&mut cfg.prescaler)
                                .range(0..=255)
                                .prefix("clk / "),
                        );
                    });
                    ui.end_row();

                    if dqs && cfg.mode != OspiMode::Octal {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(
                                "DQS is wired but only the octal mode reads it — the pad will \
                                 be left out of the call",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_rgb(200, 140, 60)),
                        );
                        ui.end_row();
                    }
                    if !is_async {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(
                                "only the Async runtime emits OCTOSPI code — the blocking \
                                 backends generate GPIO and watchdogs only",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                }
                // The external-flash controller. Which BANKS are wired is
                // which constructor embassy gets, so the panel reports the shape
                // and asks only for what the flash chip dictates.
                ModuleConfig::Qspi(cfg) => {
                    let bank = |b: u8| {
                        let tag = format!("BK{b} ");
                        let ios = conn_rows
                            .iter()
                            .filter(|(sig, _)| sig.starts_with(&tag) && sig.contains("IO"))
                            .count();
                        let ncs = conn_rows.iter().any(|(sig, _)| *sig == format!("BK{b} NCS"));
                        (ios, ncs)
                    };
                    let (io1, ncs1) = bank(1);
                    let (io2, ncs2) = bank(2);
                    let ok1 = io1 == 4 && ncs1;
                    let ok2 = io2 == 4 && ncs2;
                    let clk = conn_rows.iter().any(|(sig, _)| *sig == "CLK");

                    ui.label("Wiring");
                    let (text, colour) = match (clk, ok1, ok2) {
                        (true, true, true) => (
                            "both banks — dual flash, 8 lines wide".to_owned(),
                            egui::Color32::from_gray(200),
                        ),
                        (true, true, false) => {
                            ("bank 1".to_owned(), egui::Color32::from_gray(200))
                        }
                        (true, false, true) => {
                            ("bank 2".to_owned(), egui::Color32::from_gray(200))
                        }
                        (false, _, _) => (
                            "no CLK wired".to_owned(),
                            egui::Color32::from_rgb(200, 140, 60),
                        ),
                        _ => (
                            format!(
                                "incomplete — a bank needs NCS and all four IO ({io1}/4 on BK1, \
                                 {io2}/4 on BK2)"
                            ),
                            egui::Color32::from_rgb(200, 140, 60),
                        ),
                    };
                    ui.label(egui::RichText::new(text).size(11.0).color(colour));
                    ui.end_row();

                    ui.label("Flash size");
                    egui::ComboBox::from_id_salt("qspi_size")
                        .selected_text(cfg.memory_size_label())
                        .show_ui(ui, |ui| {
                            for (i, name) in QSPI_MEMORY_SIZES.iter().enumerate() {
                                ui.selectable_value(
                                    &mut cfg.memory_size,
                                    i as u8,
                                    name.trim_start_matches('_'),
                                );
                            }
                        })
                        .response
                        .on_hover_text(
                            "The size of the chip on the board. The controller needs it to know \
                             where the memory-mapped window ends.",
                        );
                    ui.end_row();

                    ui.label("Address");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("qspi_addr")
                            .width(88.0)
                            .selected_text(cfg.address_size.label())
                            .show_ui(ui, |ui| {
                                for v in QspiAddressSize::ALL {
                                    ui.selectable_value(&mut cfg.address_size, v, v.label());
                                }
                            })
                            .response
                            .on_hover_text(
                                "How many address bytes the chip expects. 24 bit covers up to \
                                 16 MiB; bigger flash needs 32.",
                            );
                        ui.add(
                            egui::DragValue::new(&mut cfg.prescaler)
                                .range(0..=255)
                                .prefix("clk / "),
                        )
                        .on_hover_text(
                            "The bus runs at kernel clock / (prescaler + 1). 0 is the fastest \
                             the chip can do and often too fast for the flash.",
                        );
                    });
                    ui.end_row();

                    if !is_async {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(
                                "only the Async runtime emits QUADSPI code — the blocking \
                                 backends generate GPIO and watchdogs only",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                }
                // The SD-card controller. The bus WIDTH is not a setting: how
                // many data lanes are wired is the width, and each width is a
                // different embassy constructor — so the panel reports it
                // instead of asking.
                ModuleConfig::Sdmmc(cfg) => {
                    let lanes: Vec<u8> = conn_rows
                        .iter()
                        .filter_map(|(sig, _)| sig.strip_prefix("D")?.parse::<u8>().ok())
                        .collect();
                    let width = match lanes.len() {
                        1 => Some(1u8),
                        4 => Some(4),
                        8 => Some(8),
                        _ => None,
                    };
                    ui.label("Bus width");
                    match width {
                        Some(w) => {
                            ui.label(
                                egui::RichText::new(format!("{w}-bit  ({} lanes wired)", lanes.len()))
                                    .size(11.0),
                            );
                        }
                        None => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} lanes wired — needs 1, 4 or 8",
                                    lanes.len()
                                ))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(200, 140, 60)),
                            );
                        }
                    }
                    ui.end_row();

                    ui.label("Data timeout");
                    ui.add(
                        egui::DragValue::new(&mut cfg.data_timeout)
                            .range(1_000..=100_000_000)
                            .speed(10_000.0),
                    )
                    .on_hover_text(
                        "In CARD bus clock periods, not microseconds — embassy's `Config` \
                         counts them. The default 5 000 000 is a few seconds on a slow card.",
                    );
                    ui.end_row();

                    let free: Vec<String> = (0..8u8)
                        .filter(|l| !lanes.contains(l))
                        .map(|l| format!("D{l}"))
                        .collect();
                    if !free.is_empty() {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(format!(
                                "widen the bus by assigning {} on the canvas",
                                free.join(" / ")
                            ))
                            .size(10.5)
                            .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                    if !is_async {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(
                                "only the Async runtime emits SD-card code — the blocking \
                                 backends generate GPIO and watchdogs only",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                }
                // One SAI unit, two independent sub-blocks. Each one that has
                // its three clock/data pads wired gets its own rows — the module
                // is the unit because `split_subblocks` happens once.
                ModuleConfig::Sai(cfg) => {
                    let wired: Vec<u8> = [1u8, 2]
                        .into_iter()
                        .filter(|b| {
                            let tag = if *b == 1 { "A " } else { "B " };
                            conn_rows.iter().any(|(sig, _)| sig.starts_with(tag))
                        })
                        .collect();
                    if wired.is_empty() {
                        ui.label("Sub-blocks");
                        ui.label(
                            egui::RichText::new("none wired yet")
                                .size(11.0)
                                .italics()
                                .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                    for b in &wired {
                        let letter = if *b == 1 { "A" } else { "B" };
                        let before = cfg.block_of(*b);
                        let mut blk = before;
                        ui.label(format!("{letter} stream"));
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt(("sai_dir", b))
                                .width(84.0)
                                .selected_text(blk.tx_rx.label())
                                .show_ui(ui, |ui| {
                                    for v in SaiTxRx::ALL {
                                        ui.selectable_value(&mut blk.tx_rx, v, v.label());
                                    }
                                });
                            egui::ComboBox::from_id_salt(("sai_mode", b))
                                .width(120.0)
                                .selected_text(blk.mode.label())
                                .show_ui(ui, |ui| {
                                    for v in SaiMode::ALL {
                                        ui.selectable_value(&mut blk.mode, v, v.label());
                                    }
                                });
                            egui::ComboBox::from_id_salt(("sai_size", b))
                                .width(74.0)
                                .selected_text(blk.data_size.label())
                                .show_ui(ui, |ui| {
                                    for v in SaiDataSize::ALL {
                                        ui.selectable_value(&mut blk.data_size, v, v.label());
                                    }
                                });
                        });
                        ui.end_row();

                        ui.label(format!("{letter} frame"));
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt(("sai_sm", b))
                                .width(74.0)
                                .selected_text(blk.stereo_mono.label())
                                .show_ui(ui, |ui| {
                                    for v in SaiStereoMono::ALL {
                                        ui.selectable_value(&mut blk.stereo_mono, v, v.label());
                                    }
                                });
                            ui.add(
                                egui::DragValue::new(&mut blk.slot_count)
                                    .range(1..=16)
                                    .prefix("slots "),
                            );
                            ui.add(
                                egui::DragValue::new(&mut blk.frame_length)
                                    .range(8..=256)
                                    .suffix(" bit"),
                            )
                            .on_hover_text(
                                "Frame length in BITS — slots x slot size. 32 is one 16-bit \
                                 stereo frame.",
                            );
                            ui.add(
                                egui::DragValue::new(&mut blk.buffer_len)
                                    .range(32..=8192)
                                    .prefix("buf "),
                            );
                        });
                        if blk != before {
                            cfg.set_block(*b, blk);
                        }
                        ui.end_row();

                        if is_async {
                            let (dir, field) = if *b == 1 {
                                (dma_map::Dir::Tx, &mut cfg.dma_a)
                            } else {
                                (dma_map::Dir::Rx, &mut cfg.dma_b)
                            };
                            // SAI has no entry in the hand-written DMA tables, so
                            // the picker offers nothing and the allocator does the
                            // choosing. The row is here for the day it does.
                            dma_one(
                                ui,
                                dma_map::Bus::Spi,
                                cfg.instance,
                                dir,
                                &format!("{letter} DMA"),
                                field,
                            );
                        }
                    }
                    let free: Vec<&str> = [(1u8, "A"), (2, "B")]
                        .into_iter()
                        .filter(|(b, _)| !wired.contains(b))
                        .map(|(_, l)| l)
                        .collect();
                    if !free.is_empty() {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(format!(
                                "add a stream by assigning SAI{} {} SCK/SD/FS on the canvas",
                                cfg.instance,
                                free.join(" / ")
                            ))
                            .size(10.5)
                            .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                    ui.label("");
                    ui.label(
                        egui::RichText::new(if is_async {
                            "asynchronous mode only: a sub-block slaved to the other one \
                             (`new_synchronous`) needs its clock pads left unwired, which is a \
                             wiring rule of its own"
                        } else {
                            "only the Async runtime emits SAI — embassy drives it from a DMA \
                             ring buffer per sub-block"
                        })
                        .size(10.5)
                        .color(egui::Color32::from_gray(140)),
                    );
                    ui.end_row();
                }
                // One DAC block. The channel rows come from the module's own
                // connections, so they mirror the canvas — same shape as the
                // PWM module's duty rows, for the same reason.
                ModuleConfig::Dac(cfg) => {
                    let chans: Vec<(u8, String)> = conn_rows
                        .iter()
                        .filter_map(|(sig, pin)| {
                            Some((sig.strip_prefix("OUT")?.parse::<u8>().ok()?, pin.clone()))
                        })
                        .collect();
                    if chans.is_empty() {
                        ui.label("Channels");
                        ui.label(
                            egui::RichText::new("none wired yet")
                                .size(11.0)
                                .italics()
                                .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                    // An ESP's DAC is EIGHT bits (`Dac::write` takes a u8);
                    // the STM32's is twelve. The field stores twelve either
                    // way and the ESP codegen scales, so the slider only has
                    // to stop offering steps the hardware cannot land on.
                    let esp = crate::panels::mcu_module::codegen::family::is_esp(family);
                    let (top, mid) = if esp { (255u16, 128u16) } else { (4095, 2048) };
                    for (ch, pin) in &chans {
                        ui.label(format!("OUT{ch} start  ({pin})"));
                        let mut v = cfg.value_of(*ch).min(top);
                        ui.horizontal(|ui| {
                            if ui.add(egui::Slider::new(&mut v, 0..=top)).changed() {
                                cfg.set_value(*ch, v);
                            }
                            if ui.small_button("mid").clicked() {
                                cfg.set_value(*ch, mid);
                            }
                            if esp {
                                ui.label(
                                    egui::RichText::new("8-bit")
                                        .size(10.5)
                                        .color(egui::Color32::GRAY),
                                );
                            }
                        })
                        .response
                        .on_hover_text(
                            "The value the pad holds once `init` returns. There is no unset:                              the channel drives the pin the moment it is enabled, so the only                              honest choice is to say what it drives.",
                        );
                        ui.end_row();
                    }
                    let free: Vec<u8> = (1..=2u8)
                        .filter(|c| !chans.iter().any(|(w, _)| w == c))
                        .collect();
                    if !free.is_empty() {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(format!(
                                "add a channel by assigning DAC{} OUT{} on the canvas",
                                cfg.instance,
                                free.iter()
                                    .map(|c| c.to_string())
                                    .collect::<Vec<_>>()
                                    .join(" / ")
                            ))
                            .size(10.5)
                            .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                    if !is_async {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(
                                "only the Async runtime emits DAC code today — the blocking                                  backends generate GPIO and watchdogs only",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_gray(140)),
                        );
                        ui.end_row();
                    }
                }
                // One SPI block running as audio. Every setting here is a
                // field of embassy's `i2s::Config`, plus the ring buffer the
                // DMA owns — there is no blocking I2S to fall back on.
                ModuleConfig::I2s(cfg) => {
                    let is_esp =
                        crate::panels::mcu_module::codegen::family::is_esp(family);
                    ui.label("Sample rate");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut cfg.sample_rate_hz)
                                .range(8_000..=192_000)
                                .suffix(" Hz")
                                .speed(100.0),
                        );
                        for (label, hz) in [("44.1k", 44_100u32), ("48k", 48_000), ("96k", 96_000)]
                        {
                            if ui.small_button(label).clicked() {
                                cfg.sample_rate_hz = hz;
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Direction");
                    egui::ComboBox::from_id_salt("i2s_dir")
                        .selected_text(cfg.direction.label())
                        .show_ui(ui, |ui| {
                            for v in I2sDirection::ALL {
                                ui.selectable_value(&mut cfg.direction, v, v.label());
                            }
                        })
                        .response
                        .on_hover_text(
                            "embassy has a constructor per direction. Full duplex needs the \
                             newer SPI IP (spi_v4/v5) and a second data pad, neither of which \
                             the IDE can check yet.",
                        );
                    ui.end_row();

                    ui.label("Role");
                    egui::ComboBox::from_id_salt("i2s_mode")
                        .selected_text(cfg.mode.label())
                        .show_ui(ui, |ui| {
                            for v in I2sMode::options(family).iter().copied() {
                                ui.selectable_value(&mut cfg.mode, v, v.label());
                            }
                        });
                    ui.end_row();

                    ui.label("Standard");
                    egui::ComboBox::from_id_salt("i2s_std")
                        .selected_text(cfg.standard.label())
                        .show_ui(ui, |ui| {
                            for v in I2sStandard::options(family).iter().copied() {
                                ui.selectable_value(&mut cfg.standard, v, v.label());
                            }
                        });
                    ui.end_row();

                    ui.label("Format");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("i2s_fmt")
                            .selected_text(cfg.format.label())
                            .show_ui(ui, |ui| {
                                for v in I2sFormat::options(family).iter().copied() {
                                    ui.selectable_value(&mut cfg.format, v, v.label());
                                }
                            });
                        // esp-hal's master config has no bit-clock polarity
                        // knob — only a word-select one, which is a different
                        // signal. Hidden rather than shown doing nothing.
                        if !is_esp {
                            egui::ComboBox::from_id_salt("i2s_pol")
                                .selected_text(cfg.clock_polarity.label())
                                .show_ui(ui, |ui| {
                                    for v in I2sClockPolarity::ALL {
                                        ui.selectable_value(
                                            &mut cfg.clock_polarity,
                                            v,
                                            v.label(),
                                        );
                                    }
                                });
                        }
                    });
                    ui.end_row();

                    ui.label("Ring buffer");
                    ui.add(
                        egui::DragValue::new(&mut cfg.buffer_len)
                            .range(32..=8192)
                            .suffix(" samples"),
                    )
                    .on_hover_text(
                        "The DMA owns this buffer for the whole program. Too short and the \
                         audio breaks up on any scheduling hiccup; the samples are \
                         `format`-wide words.",
                    );
                    ui.end_row();

                    if is_async {
                        let inst = cfg.instance;
                        // The DMA requests are the SPI block's — same silicon,
                        // same request lines.
                        if cfg.direction.is_tx() {
                            dma_one(ui, dma_map::Bus::Spi, inst, dma_map::Dir::Tx, "DMA", &mut cfg.dma_tx);
                        } else {
                            dma_one(ui, dma_map::Bus::Spi, inst, dma_map::Dir::Rx, "DMA", &mut cfg.dma_rx);
                        }
                    }

                    ui.label("");
                    ui.label(
                        egui::RichText::new(if is_async {
                            format!(
                                "runs on SPI{} — an SPI module on the same instance describes \
                                 the same block, and only one of the two is built",
                                cfg.instance
                            )
                        } else {
                            "only the Async runtime emits I2S — embassy drives it from a DMA \
                             ring buffer, and there is no blocking form"
                                .to_owned()
                        })
                        .size(10.5)
                        .color(egui::Color32::from_gray(140)),
                    );
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
                    // Async only: `embassy_time::Duration` needs embassy-stm32's
                    // `time` feature, which only the async dependency line pulls
                    // in (through `time-driver-any`).
                    if is_async {
                        ui.label("Timeout");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut cfg.timeout_ms)
                                    .speed(10.0)
                                    .range(0..=60_000)
                                    .suffix(" ms"),
                            );
                            ui.label(
                                egui::RichText::new(if cfg.timeout_ms == 0 {
                                    "0 = embassy default (1000 ms)".to_string()
                                } else {
                                    String::new()
                                })
                                .size(10.5)
                                .color(egui::Color32::from_gray(130)),
                            );
                        })
                        .response
                        .on_hover_text(
                            "How long a transfer may take before it gives up. I2C hangs are a                              real failure mode - a device that stretches the clock forever, or a                              bus with no pull-ups, blocks for as long as this allows.",
                        );
                        ui.end_row();
                    }
                    ui.label("Address (7-bit)");
                    ui.add(
                        egui::DragValue::new(&mut cfg.address)
                            .range(0..=127)
                            .hexadecimal(2, false, true),
                    );
                    ui.end_row();
                    if is_async {
                        async_row(ui, &mut pending.1);
                        if pending.1 == AsyncBusMode::AsyncDma {
                            let inst = cfg.instance;
                            dma_row(
                                ui,
                                dma_map::Bus::I2c,
                                inst,
                                &mut cfg.dma_tx,
                                &mut cfg.dma_rx,
                            );
                        }
                    } else if is_native {
                        api_row_locked(ui);
                    } else {
                        // Same rule as the USART above, and the only backend
                        // whose half-bus behaviour this turn verified.
                        if family == "stm32f1" {
                            f1_half_bus_note(
                                ui,
                                "I2C",
                                ("SCL", "SDA"),
                                "stm32f1xx-hal takes the SCL+SDA pair",
                                wired_i2c,
                            );
                        }
                        api_row(ui, &mut pending.0);
                    }
                }
                ModuleConfig::Can(cfg) => {
                    let esp = crate::panels::mcu_module::codegen::family::is_esp(family);
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
                        })
                        .response
                        .on_hover_text(if esp {
                            "esp-hal ships timings for these four only. Anything else \
                             needs a hand-computed `BaudRate::Custom` in the generated \
                             file."
                        } else {
                            "Every node on the bus must agree on this."
                        });
                    ui.end_row();

                    let modes = CanMode::options(family);
                    if modes.len() > 1 {
                        ui.label("Mode");
                        egui::ComboBox::from_id_salt("canmode")
                            .selected_text(cfg.mode.label())
                            .show_ui(ui, |ui| {
                                for v in modes.iter().copied() {
                                    ui.selectable_value(&mut cfg.mode, v, v.label())
                                        .on_hover_text(v.hint());
                                }
                            });
                        ui.end_row();

                        // Only the ESP has the second constructor. Everywhere
                        // else this row would be a switch that changes nothing.
                        ui.label("Transceiver");
                        ui.checkbox(&mut cfg.transceiver, "on the pads")
                            .on_hover_text(
                                "A real CAN bus needs one. Clear this only for two boards \
                                 wired TX-to-RX directly, which esp-hal builds with a \
                                 different constructor.",
                            );
                        ui.end_row();
                    }
                    // Third pair on this family — see the USART and I2C above.
                    if family == "stm32f1" {
                        f1_half_bus_note(
                            ui,
                            "CAN",
                            ("TX", "RX"),
                            "stm32f1xx-hal assigns the CAN pads as a TX+RX pair",
                            wired_can,
                        );
                    }
                }
                ModuleConfig::Usb(cfg) => {
                    // Two controllers on one pad pair: which one the pads go to
                    // is the first question, and it decides every row below.
                    let roles = UsbRole::options(family);
                    if roles.len() > 1 {
                        ui.label("Controller");
                        egui::ComboBox::from_id_salt("usbrole")
                            .selected_text(cfg.role.label())
                            .show_ui(ui, |ui| {
                                for v in roles.iter().copied() {
                                    ui.selectable_value(&mut cfg.role, v, v.label())
                                        .on_hover_text(v.hint());
                                }
                            })
                            .response
                            .on_hover_text(
                                "Both land on the same two pads, so only one can have them. \
                                 Serial/JTAG is the built-in console; OTG is a device of \
                                 your own design.",
                            );
                        ui.end_row();
                    } else if !roles.is_empty() && cfg.role != roles[0] {
                        // Carried over from a chip that HAD the other controller.
                        // Put it back rather than offer one this chip lacks.
                        cfg.role = roles[0];
                    }
                    // The three descriptor fields belong to a `usb-device` stack
                    // — the STM32 path's, or the ESP's own OTG. Serial/JTAG
                    // enumerates as Espressif's fixed `303a:1001` and has no
                    // descriptors, so there they would change nothing.
                    if crate::panels::mcu_module::codegen::family::is_esp(family)
                        && !cfg.role.is_otg()
                    {
                        ui.label("Identity");
                        ui.label(
                            egui::RichText::new("303a:1001  ·  fixed in silicon")
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        )
                        .on_hover_text(
                            "The USB Serial/JTAG peripheral enumerates with Espressif's own                              VID:PID and a fixed descriptor set. Nothing here can change it                              - a board that needs its own identity uses a USB stack over the                              OTG controller instead, which this chip may not have.",
                        );
                        ui.end_row();
                        ui.label("Port");
                        ui.label(
                            egui::RichText::new("CDC serial, on the chip's own pads")
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        )
                        .on_hover_text(
                            "A board with a USB-UART bridge chip shows that as well; the two                              are different devices to the host.",
                        );
                        ui.end_row();
                        return;
                    }
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
                    if cfg.role.is_otg() {
                        ui.label("Stack");
                        ui.label(
                            egui::RichText::new("usb-device + usbd-serial")
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        )
                        .on_hover_text(
                            "Added to Cargo.toml for you. `main.rs` builds a CDC serial \
                             device on the bus; poll it in your loop, or swap the class \
                             for any other the crate offers.",
                        );
                        ui.end_row();
                        return;
                    }
                    // Not a HAL constraint like the four above — the USB init
                    // takes PA11/PA12 directly, so one pad would spend the other
                    // uninvited.
                    if family == "stm32f1" {
                        f1_half_bus_note(
                            ui,
                            "USB",
                            ("D-", "D+"),
                            "the generated init takes PA11/PA12 directly, so one pad \
                             would spend the other uninvited",
                            wired_usb,
                        );
                    }
                }
                // ── Custom: a hand-picked pin list ────────────────────────
                // No auto-wiring and no peripheral config — just the pins, in
                // the order they were added (that order is the generated
                // struct's field order and `new()`'s parameter order).
                ModuleConfig::Custom(cfg) => {
                    // Every label in a Custom module's panel is BOLD (user's ask).
                    // Struct name — pre-filled from the module name, then the
                    // user's own (so renaming the module can't break their impls).
                    //
                    // Label AND field share ONE grid cell: put the field in the
                    // second column and the pin rows below (which are wide, and
                    // live in the first) stretch that column, shoving this box to
                    // the far right, away from the Name field it belongs beside.
                    ui.horizontal(|ui| {
                        custom_field_label(ui, "Struct");
                        let hint = derived_struct_name(&cfg.custom_label, cfg.instance);
                        ui.add(
                            egui::TextEdit::singleline(&mut cfg.struct_name)
                                .desired_width(CUSTOM_FIELD_W)
                                .hint_text(hint)
                                .font(egui::FontId::proportional(11.0)),
                        )
                        .on_hover_text(
                            "Name of the generated struct. Empty = follow the module name.",
                        );
                    });
                    ui.end_row();

                    // "Pins" sits on its OWN row right under Struct, and the
                    // rows below start at the far left — the panel then reads as
                    // a list instead of a label with a block hanging off it.
                    ui.label(egui::RichText::new("Pins").strong());
                    ui.end_row();

                    let mut remove: Option<usize> = None;
                    for (i, pin) in cfg.pins.iter().enumerate() {
                        let num = *pin;
                        let name = pin_names
                            .get(&num)
                            .cloned()
                            .unwrap_or_else(|| format!("pin{num}"));
                        let cur = pin_funcs_current.get(&num);
                        let configured = cur.is_some_and(|f| *f != PinFunction::Unset);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{num}"))
                                    .size(10.0)
                                    .color(egui::Color32::from_gray(140)),
                            );
                            // The pin NAME is a button opening the same function
                            // list the chip shows, so a pin can be configured
                            // without hunting for it on the canvas. Amber while
                            // it still has no function.
                            let btn_col = if configured {
                                egui::Color32::from_rgb(120, 180, 235)
                            } else {
                                egui::Color32::from_rgb(230, 170, 70)
                            };
                            ui.menu_button(
                                egui::RichText::new(&name)
                                    .size(10.5)
                                    .strong()
                                    .color(btn_col),
                                |ui| {
                                    ui.set_min_width(210.0);
                                    ui.label(
                                        egui::RichText::new(format!("{name} - function"))
                                            .size(10.0)
                                            .color(egui::Color32::GRAY),
                                    );
                                    ui.separator();
                                    for f in pin_funcs.get(&num).into_iter().flatten() {
                                        let selected = cur == Some(f);
                                        if ui
                                            .selectable_label(
                                                selected,
                                                // Same rule as the in-chip
                                                // function list, so the two
                                                // cannot disagree about how
                                                // a signal is named.
                                                egui::RichText::new(f.list_label())
                                                    .size(10.5)
                                                    .color(f.color()),
                                            )
                                            .clicked()
                                        {
                                            *pin_fn_choice = Some((num, f.clone()));
                                            ui.close();
                                        }
                                    }
                                },
                            )
                            .response
                            .on_hover_text(if configured {
                                "Change this pin's function"
                            } else {
                                "This pin has NO function yet - pick one before Update"
                            });
                            if ui
                                .small_button(egui::RichText::new(ph::X).size(10.0))
                                .on_hover_text("Remove this pin from the module")
                                .clicked()
                            {
                                remove = Some(i);
                            }
                            // Mirror of the field inside the module's box.
                            let label = pin_labels.entry(num).or_default();
                            ui.add(
                                egui::TextEdit::singleline(label)
                                    .desired_width(110.0)
                                    .hint_text("name")
                                    .font(egui::FontId::proportional(10.0)),
                            )
                            .on_hover_text(
                                "Name appended to this pin's generated variable (and to its \
                                 field in the struct).",
                            );
                        });
                        ui.end_row();
                    }
                    if let Some(i) = remove {
                        cfg.pins.remove(i);
                    }
                    if cfg.pins.is_empty() {
                        ui.label(
                            egui::RichText::new("no pins yet - add at least one")
                                .size(10.0)
                                .italics()
                                .color(egui::Color32::from_rgb(220, 180, 90)),
                        );
                        ui.end_row();
                    }

                    // "+ Add pin" - only FREE pins (see `pin_blocked`).
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt(("custom_add_pin", m_id.clone()))
                            .selected_text(
                                egui::RichText::new(format!("{} Add pin", ph::PLUS)).size(10.5),
                            )
                            .show_ui(ui, |ui| {
                                let mut nums: Vec<usize> = pin_names.keys().copied().collect();
                                nums.sort_unstable();
                                let mut any = false;
                                for n in nums {
                                    if cfg.pins.contains(&n) || pin_blocked.contains(&n) {
                                        continue;
                                    }
                                    any = true;
                                    let nm = &pin_names[&n];
                                    if ui
                                        .selectable_label(
                                            false,
                                            egui::RichText::new(format!("{n}  {nm}"))
                                                .size(10.5)
                                                .monospace(),
                                        )
                                        .clicked()
                                    {
                                        cfg.pins.push(n);
                                    }
                                }
                                if !any {
                                    ui.label(
                                        egui::RichText::new(
                                            "no free pin left - only unassigned, non-reserved \
                                             pins can be added",
                                        )
                                        .size(10.0)
                                        .italics(),
                                    );
                                }
                            });
                    });
                    ui.end_row();

                    // Generating with an unconfigured pin would declare a field
                    // for a variable main.rs never binds, so Update turns AMBER
                    // and reports instead, naming the pins to fix.
                    let unconfigured: Vec<String> = cfg
                        .pins
                        .iter()
                        .filter(|n| {
                            pin_funcs_current
                                .get(n)
                                .map(|f| *f == PinFunction::Unset)
                                .unwrap_or(true)
                        })
                        .map(|n| {
                            pin_names
                                .get(n)
                                .cloned()
                                .unwrap_or_else(|| format!("pin{n}"))
                        })
                        .collect();
                    let incomplete = !unconfigured.is_empty();
                    let current_sig = custom_pins_sig(&cfg.pins, pin_sigs);
                    let pending = cfg.has_pending_pins(&current_sig);
                    let warn_id = egui::Id::new(("custom_update_warn", m_id.clone()));
                    ui.horizontal(|ui| {
                        let btn = ui.add_enabled(
                            pending && !cfg.pins.is_empty(),
                            egui::Button::new(
                                egui::RichText::new(format!("{} Update", ph::ARROWS_CLOCKWISE))
                                    .size(10.5)
                                    .color(if !pending {
                                        egui::Color32::GRAY
                                    } else if incomplete {
                                        egui::Color32::from_rgb(240, 165, 60)
                                    } else {
                                        egui::Color32::from_rgb(120, 210, 140)
                                    }),
                            ),
                        );
                        if btn
                            .on_hover_text(
                                "Generate the struct from the pins above into a NEW file \
                                 (`configs/custom_<name>_<n>.rs`) and point main.rs at it. \
                                 Earlier revisions stay on disk, uncompiled.",
                            )
                            .on_disabled_hover_text("No pin changes to apply.")
                            .clicked()
                        {
                            if incomplete {
                                ui.ctx()
                                    .data_mut(|d| d.insert_temp(warn_id, unconfigured.join(", ")));
                            } else {
                                if !cfg.applied_pins.is_empty() {
                                    cfg.revision += 1;
                                }
                                cfg.applied_pins = cfg.pins.clone();
                                cfg.applied_sig = current_sig.clone();
                            }
                        }
                        if pending {
                            ui.label(
                                egui::RichText::new("pin changes not generated yet")
                                    .size(9.5)
                                    .italics()
                                    .color(egui::Color32::from_rgb(220, 180, 90)),
                            );
                        }
                    });
                    ui.end_row();

                    // The "Not all pins configured" dialog raised above.
                    if let Some(list) = ui.ctx().data(|d| d.get_temp::<String>(warn_id)) {
                        let mut open = true;
                        let mut dismiss = false;
                        egui::Window::new("Not all pins configured")
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                            .open(&mut open)
                            .show(ui.ctx(), |ui| {
                                ui.label(
                                    egui::RichText::new(
                                        "These pins have no function yet, so the struct can't \
                                         be generated for them:",
                                    )
                                    .size(11.0),
                                );
                                ui.label(
                                    egui::RichText::new(&list)
                                        .strong()
                                        .monospace()
                                        .color(egui::Color32::from_rgb(240, 165, 60)),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Click a pin's name above (or the pin on the chip) and \
                                         choose In / Out / ADC / PWM...",
                                    )
                                    .size(10.5)
                                    .color(egui::Color32::GRAY),
                                );
                                ui.add_space(6.0);
                                if ui.button("OK").clicked() {
                                    dismiss = true;
                                }
                            });
                        if !open || dismiss {
                            ui.ctx().data_mut(|d| d.remove::<String>(warn_id));
                        }
                    }
                }
            }

            // Peripheral modules list their wired pins here; a custom module
            // already shows (and edits) its own pins above.
            if !is_custom {
                for (sig, pin) in &conn_rows {
                    ui.label(format!("{sig} {} pin", ph::ARROW_RIGHT));
                    ui.label(pin);
                    ui.end_row();
                }
            }
        });

    ui.add_space(4.0);
    // let label = |ui: &mut egui::Ui, t: &str| {
    //     ui.label(
    //         egui::RichText::new(t)
    //             .size(11.0)
    //             .color(egui::Color32::from_rgb(150, 150, 160)),
    //     );
    // };
    // label(ui, "RX data model");
    // ui.add(
    //     egui::TextEdit::multiline(m.config.rx_model_mut())
    //         .desired_rows(3)
    //         .desired_width(f32::INFINITY)
    //         .code_editor()
    //         .hint_text("data the device sends — e.g. struct Reading { temp: f32, .. }"),
    // );
    // label(ui, "TX data model");
    // ui.add(
    //     egui::TextEdit::multiline(m.config.tx_model_mut())
    //         .desired_rows(3)
    //         .desired_width(f32::INFINITY)
    //         .code_editor()
    //         .hint_text("data you send to the device — e.g. command frames"),
    // );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chip whose pins can serve `channels` of `timer`, one pad each, plus a
    /// decoy pad on another timer.
    fn chip(timer: u8, channels: &[u8]) -> HashMap<usize, Vec<PinFunction>> {
        let mut out: HashMap<usize, Vec<PinFunction>> = HashMap::new();
        for (i, ch) in channels.iter().enumerate() {
            out.insert(
                i,
                vec![PinFunction::TimerPwm {
                    timer,
                    channel: *ch,
                }],
            );
        }
        out.insert(
            99,
            vec![PinFunction::TimerPwm {
                timer: timer + 1,
                channel: 4,
            }],
        );
        out
    }

    /// A chip whose pins can serve the complementary pads `comp` of `timer`.
    fn chip_with_comp(timer: u8, channels: &[u8], comp: &[u8]) -> HashMap<usize, Vec<PinFunction>> {
        let mut out = chip(timer, channels);
        for (i, ch) in comp.iter().enumerate() {
            out.insert(
                1000 + i,
                vec![PinFunction::TimerPwmN {
                    timer,
                    channel: *ch,
                }],
            );
        }
        out
    }

    fn wired(labels: &[&str]) -> BTreeSet<String> {
        labels.iter().map(|s| (*s).to_owned()).collect()
    }

    /// The hint names the pads actually left on THIS timer.
    #[test]
    fn free_channels_are_what_the_chip_has_minus_what_is_wired() {
        let four = chip(2, &[1, 2, 3, 4]);
        assert_eq!(
            free_pwm_channels(2, &wired(&["CH1"]), &four),
            ["CH2", "CH3", "CH4"],
            "the common case the old fixed text got right"
        );
        // …and the one it got wrong: CH1 freed, CH2 taken.
        assert_eq!(
            free_pwm_channels(2, &wired(&["CH2"]), &four),
            ["CH1", "CH3", "CH4"],
            "CH1 is free and must be offered"
        );
    }

    /// Nothing left to offer — the row disappears instead of advertising an
    /// action that cannot be taken.
    #[test]
    fn a_fully_wired_timer_has_no_free_channel() {
        let four = chip(2, &[1, 2, 3, 4]);
        assert!(free_pwm_channels(2, &wired(&["CH1", "CH2", "CH3", "CH4"]), &four).is_empty());
    }

    /// Not every timer has four channels, and not every package bonds the pads
    /// it does have. TIM14 offers CH1 and nothing else.
    #[test]
    fn a_one_channel_timer_never_offers_channels_it_lacks() {
        let tim14 = chip(14, &[1]);
        assert!(
            free_pwm_channels(14, &wired(&["CH1"]), &tim14).is_empty(),
            "CH2/3/4 do not exist on TIM14"
        );
        assert_eq!(free_pwm_channels(14, &BTreeSet::new(), &tim14), ["CH1"]);
    }

    /// Another timer's pads are not this module's business.
    #[test]
    fn other_timers_do_not_leak_into_the_hint() {
        let tim2 = chip(2, &[1]);
        assert_eq!(free_pwm_channels(2, &BTreeSet::new(), &tim2), ["CH1"]);
        assert_eq!(free_pwm_channels(3, &BTreeSet::new(), &tim2), ["CH4"]);
    }

    /// A complementary pad is one more thing to assign on the canvas, so the
    /// hint offers it — on an advanced timer, where embassy can drive it.
    #[test]
    fn complementary_pads_are_offered_on_an_advanced_timer() {
        let tim1 = chip_with_comp(1, &[1, 2], &[1, 2]);
        assert_eq!(
            free_pwm_channels(1, &wired(&["CH1"]), &tim1),
            ["CH1N", "CH2", "CH2N"],
        );
        // Already wired ones drop out, exactly like the plain channels.
        assert_eq!(
            free_pwm_channels(1, &wired(&["CH1", "CH1N", "CH2", "CH2N"]), &tim1),
            Vec::<String>::new(),
        );
    }

    /// …and never on a timer whose complementary pads embassy cannot drive:
    /// TIM15/16/17 have a CH1N pad, but `ComplementaryPwm` covers TIM1/8/20.
    #[test]
    fn complementary_pads_are_not_offered_where_embassy_cannot_drive_them() {
        let tim16 = chip_with_comp(16, &[1], &[1]);
        assert_eq!(
            free_pwm_channels(16, &BTreeSet::new(), &tim16),
            ["CH1"],
            "CH1N exists on the chip but has no driver here"
        );
        assert!(is_advanced_timer(1) && is_advanced_timer(8) && is_advanced_timer(20));
        assert!(!is_advanced_timer(16) && !is_advanced_timer(2));
    }
}
