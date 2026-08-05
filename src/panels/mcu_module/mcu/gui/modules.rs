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
            hy = hy.max(m.pos.1.abs()).max((m.pos.1 + BOX_H).abs());
        }
    }
    egui::vec2(hx, hy)
}

/// Outer connection point of an MCU pin (rotation applied). `None` if the pin
/// isn't on this chip. `chip_rect` is the LOCAL (un-rotated) body; `rot` is the
/// diagram rotation.
pub fn pin_anchor(mcu: &Mcu, chip_rect: egui::Rect, rot: Rot, pin_num: usize) -> Option<egui::Pos2> {
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

/// Snap a (rotated) outward vector to the nearest chip side.
fn side_from_outward(v: egui::Vec2) -> Side {
    if v.x.abs() >= v.y.abs() {
        if v.x >= 0.0 {
            Side::Right
        } else {
            Side::Left
        }
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
    native_forced: bool,
    // `Some(t)` (t in 0..1) while this module is pending deletion — its box
    // background pulses red so it's obvious on the diagram which one is being
    // removed. `None` = normal.
    removing_blink: Option<f32>,
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
    painter.rect_filled(rect, 6.0, fill);
    let stroke = if removing_blink.is_some() {
        egui::Stroke::new(2.0, egui::Color32::from_rgb(235, 70, 70)) // pending removal
    } else if connected {
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
        handle_preview(m, native_forced),
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
        conns: Vec<(ModuleSignal, egui::Pos2)>,
        side: Side,
        along: f32,
    }
    let chip_center = chip_rect.center();
    let mut sided: Vec<Sided> = Vec::new();
    let mut floating_idx: Vec<usize> = Vec::new();
    // Modules the user has dragged to a manual position (`pos != (0,0)`, stored
    // as an offset from the chip centre) are placed there directly, skipping the
    // auto-packing. Their wire conns are kept so the wires still track the pins.
    let mut manual_mods: Vec<(usize, Vec<(ModuleSignal, egui::Pos2)>)> = Vec::new();

    for (i, m) in mcu.modules.iter().enumerate() {
        let conns: Vec<(ModuleSignal, egui::Pos2, Side)> = m
            .connections
            .iter()
            .filter_map(|c| {
                pin_anchor_side(mcu, local_chip, rot, c.mcu_pin).map(|(p, s)| (c.signal, p, s))
            })
            .collect();
        if m.pos != (0.0, 0.0) {
            let conns2 = conns.iter().map(|(sig, p, _)| (*sig, *p)).collect();
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
            .filter(|(_, _, s)| *s == side)
            .map(|(_, p, _)| match side {
                Side::Top | Side::Bottom => p.x,
                Side::Left | Side::Right => p.y,
            })
            .collect();
        let along = on_side.iter().sum::<f32>() / on_side.len().max(1) as f32;
        let conns2 = conns.iter().map(|(sig, p, _)| (*sig, *p)).collect();
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
        Vec<(ModuleSignal, egui::Pos2)>,
        Side,
        bool,
        bool,
    )> = Vec::new();
    for target in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
        let mut group: Vec<&Sided> = sided.iter().filter(|e| e.side == target).collect();
        group.sort_by(|a, b| a.along.total_cmp(&b.along));
        let mut cursor = f32::MIN;
        for e in group {
            let rect = packed_rect(chip_rect, e.side, e.along, &mut cursor);
            boxes.push((e.idx, rect, e.conns.clone(), e.side, true, false));
        }
    }
    // Disconnected modules stack in the right margin.
    let mut fy = chip_rect.top();
    for i in floating_idx {
        let min = egui::pos2(chip_rect.right() + PIN_HEIGHT + PIN_GAP, fy);
        fy += BOX_H + BOX_GAP;
        boxes.push((
            i,
            egui::Rect::from_min_size(min, egui::vec2(BOX_W, BOX_H)),
            Vec::new(),
            Side::Right,
            false,
            false,
        ));
    }
    // Manually-dragged boxes: placed at chip centre + stored offset.
    for (i, conns) in manual_mods {
        let p = mcu.modules[i].pos;
        let rect =
            egui::Rect::from_min_size(chip_center + egui::vec2(p.0, p.1), egui::vec2(BOX_W, BOX_H));
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
        );

        for (sig, anchor) in conns {
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
    is_native: bool,
    // The STAGED `(api_style, async_mode)` for this module — the api/async row
    // edit THIS, not `m.config`, so nothing regenerates until "Apply".
    pending: &mut (ApiStyle, AsyncBusMode),
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
            }

            for (sig, pin) in &conn_rows {
                ui.label(format!("{sig} {} pin", ph::ARROW_RIGHT));
                ui.label(pin);
                ui.end_row();
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
