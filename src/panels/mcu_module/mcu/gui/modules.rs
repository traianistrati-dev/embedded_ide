//! Render virtual modules (e.g. _USART) and their wires beside the chip on the
//! Pins canvas — a simplified schematic. Read-only (add/remove is in the Pins
//! tab toolbar; config is the Module panel).

use super::super::model::{Mcu, PIN_HEIGHT, PIN_SPACING, PIN_WIDTH};
use super::rotate::Rot;
use crate::panels::mcu_module::codegen::sanitize_label;
use crate::panels::mcu_module::modules::model::hz_label;
use crate::panels::mcu_module::modules::{
    ApiStyle, AsyncBusMode, ModuleConfig, ModuleKind, ModuleSignal, Parity, StopBits, VirtualModule,
};
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
use eframe::egui;
use egui_phosphor::regular as ph;
use std::collections::HashMap;

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

/// Every pin claimed by a Custom module — their rename fields are drawn INSIDE
/// that module's box, so `io_arrows` must not also float one beside the chip.
pub fn custom_module_pins(mcu: &Mcu) -> std::collections::HashSet<usize> {
    mcu.modules
        .iter()
        .filter(|m| m.kind.is_custom())
        .flat_map(|m| match &m.config {
            ModuleConfig::Custom(c) => c.pins.clone(),
            _ => Vec::new(),
        })
        .collect()
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
    let row_y = |i: usize| {
        chip_rect.top() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING) + PIN_WIDTH / 2.0
    };
    let col_x = |i: usize| {
        chip_rect.left() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING) + PIN_WIDTH / 2.0
    };
    if let Some(i) = mcu.right_pins.iter().position(|p| p.number == pin_num) {
        return Some((
            egui::pos2(chip_rect.right() + PIN_HEIGHT, row_y(i)),
            egui::vec2(1.0, 0.0),
        ));
    }
    if let Some(i) = mcu.left_pins.iter().position(|p| p.number == pin_num) {
        return Some((
            egui::pos2(chip_rect.left() - PIN_HEIGHT, row_y(i)),
            egui::vec2(-1.0, 0.0),
        ));
    }
    if let Some(i) = mcu.top_pins.iter().position(|p| p.number == pin_num) {
        return Some((
            egui::pos2(col_x(i), chip_rect.top() - PIN_HEIGHT),
            egui::vec2(0.0, -1.0),
        ));
    }
    if let Some(i) = mcu.bottom_pins.iter().position(|p| p.number == pin_num) {
        return Some((
            egui::pos2(col_x(i), chip_rect.bottom() + PIN_HEIGHT),
            egui::vec2(0.0, 1.0),
        ));
    }
    None
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
    let stroke = egui::Stroke::new(1.6, color);
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
/// matches its pins, plus the list-entry name + the "already added" palette
/// buttons.
pub fn module_color(kind: ModuleKind, instance: u8) -> egui::Color32 {
    let f = match kind {
        ModuleKind::GenericInterfaceUsart => PinFunction::UsartTx(instance),
        ModuleKind::GenericInterfaceSpi => PinFunction::SpiSck(instance),
        ModuleKind::GenericInterfaceI2c => PinFunction::I2cScl(instance),
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
        ModuleKind::GenericInterfaceSpi => format!("_spi{n}{sfx}"),
        ModuleKind::GenericInterfaceI2c => format!("_i2c{n}{sfx}"),
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
        egui::Stroke::new(2.0, egui::Color32::from_rgb(235, 70, 70)) // pending removal
    } else if selected {
        egui::Stroke::new(2.8, egui::Color32::WHITE) // 2× the connected border
    } else if connected {
        egui::Stroke::new(1.4, color) // border matches the pin colour
    } else {
        egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 90, 90)) // disconnected
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
            painter.line_segment([term, *anchor], egui::Stroke::new(1.6, color));
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
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(50)),
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
        // chip (io_arrows skips them, see `custom_module_pins`).
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
                        egui::Stroke::new(2.0, egui::Color32::WHITE),
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
    is_async: bool,
    is_native: bool,
    // The STAGED `(api_style, async_mode)` for this module — the api/async row
    // edit THIS, not `m.config`, so nothing regenerates until "Apply".
    pending: &mut (ApiStyle, AsyncBusMode),
) {
    // Read what we need off `m` BEFORE `m.config` is borrowed mutably below.
    let is_custom = m.kind.is_custom();
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
                ui.selectable_value(
                    mode,
                    AsyncBusMode::AsyncDma,
                    "Async-DMA (embedded-hal-async)",
                )
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
                            for b in [9600u32, 19200, 38400, 57600, 115200, 230400, 460800, 921600]
                            {
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
                    // Blocking → editable Portable|Native; Native runtime → shown
                    // locked on Native; async USART → hidden (always the
                    // embedded-io-async BufferedUart bridge, no choice).
                    if is_native {
                        api_row_locked(ui);
                    } else if !is_async {
                        api_row(ui, &mut pending.0);
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
                            for hz in [
                                125_000u32, 250_000, 500_000, 1_000_000, 2_000_000, 4_000_000,
                                8_000_000,
                            ] {
                                ui.selectable_value(&mut cfg.clock_hz, hz, hz_label(hz));
                            }
                        });
                    ui.end_row();
                    if is_async {
                        async_row(ui, &mut pending.1);
                    } else if is_native {
                        api_row_locked(ui);
                    } else {
                        api_row(ui, &mut pending.0);
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
                    ui.add(
                        egui::DragValue::new(&mut cfg.address)
                            .range(0..=127)
                            .hexadecimal(2, false, true),
                    );
                    ui.end_row();
                    if is_async {
                        async_row(ui, &mut pending.1);
                    } else if is_native {
                        api_row_locked(ui);
                    } else {
                        api_row(ui, &mut pending.0);
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
                                                egui::RichText::new(format!(
                                                    "{}  -  {}",
                                                    f.short_label(),
                                                    f.label()
                                                ))
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
