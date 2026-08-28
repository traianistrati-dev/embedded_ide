//! Chip body and pin rendering — draws the MCU chip and its pins on 4 sides.

use super::geometry::{PinGeom, PinPlace, PinSide, pin_geometry};
use super::rotate::{Rot, RotMode, ScreenSide};
use crate::panels::mcu_module::mcu::model::{Mcu, PIN_HEIGHT, PIN_WIDTH};
use crate::panels::mcu_module::pins::logic::pin::{PIN_FONT_SIZE, Pin};
use eframe::egui;
/// Default number colour (plain GPIO / analog / power pins).
const NUM_COLOR: egui::Color32 = egui::Color32::WHITE;
/// Number colour for pins carrying a serial-bus function (USART / SPI / I2C /
/// USB / CAN) — orange so multi-function comm pins stand out from plain I/O.
const NUM_COLOR_BUS: egui::Color32 = egui::Color32::from_rgb(255, 150, 40);
/// `FontId` isn't const (needs a runtime `f32` size), so this is a small fn.
fn num_font() -> egui::FontId {
    egui::FontId::monospace(11.0)
}

/// The pin number, painted just inside the chip edge.
///
/// `show` is false while a pin is SELECTED: the body then carries that pin's
/// function list, and the numbers — painted at the very same place, inside the
/// perimeter — would show through its rows.
fn draw_number(
    painter: &egui::Painter,
    show: bool,
    pos: egui::Pos2,
    align: egui::Align2,
    pin: &Pin,
) {
    if !show {
        return;
    }
    let color = if pin.has_bus_function() {
        NUM_COLOR_BUS
    } else {
        NUM_COLOR
    };
    painter.text(pos, align, pin.number, num_font(), color);
}

/// The package designator of a ball ("A2"), drawn just under it — the label the
/// datasheet's ballout puts there, and the only way to find a ball on a real
/// package (its pin NUMBER is our own ordinal and means nothing to the board).
fn draw_designator(
    painter: &egui::Painter,
    show: bool,
    pos: egui::Pos2,
    align: egui::Align2,
    designator: &str,
    pin: &Pin,
) {
    if !show {
        return;
    }
    let color = if pin.has_bus_function() {
        NUM_COLOR_BUS
    } else {
        NUM_COLOR
    };
    painter.text(pos, align, designator, num_font(), color);
}

/// A painter that fades everything drawn through it to half opacity — how a pin
/// that does NOT match the toolbar search is shown, so the matches read as the
/// bright ones without changing any of their own colours.
///
/// `dim` comes from [`Mcu::pin_search_highlight`], which is `None` (nothing
/// faded) both when the box is empty and when the query matches no pin at all.
fn dimmed(painter: &egui::Painter, dim: bool) -> egui::Painter {
    let mut p = painter.clone();
    if dim {
        p.set_opacity(SEARCH_DIM);
    }
    p
}

/// Opacity of a pin filtered out by the search box.
const SEARCH_DIM: f32 = 0.3;

/// A bare chip's body — dark grey plastic.
pub const CHIP_FILL: egui::Color32 = egui::Color32::from_rgb(45, 45, 55);

/// A BOARD's body — green solder mask. The pins along its edges are header
/// positions, not chip pins, and the diagram should not have to say so twice.
pub const BOARD_FILL: egui::Color32 = egui::Color32::from_rgb(18, 92, 40);

/// The parts of a BOARD that are not pins: the chip, and the radio can.
///
/// Drawn from the SAME rect the name is centred in, and the radio's height from
/// the pins it actually sits between — so the two move together when the body
/// is resized rather than drifting apart at some zoom nobody tested.
///
/// `name_rect` is where the board name will be written; the chip square sits
/// above it and the radio below.
pub fn draw_board_features(
    painter: &egui::Painter,
    mcu: &crate::panels::mcu_module::Mcu,
    chip_rect: egui::Rect,
    name_center: egui::Pos2,
    name_height: f32,
) {
    let Some(part) = mcu.board_chip.as_deref() else {
        return;
    };

    // ── The chip itself ─────────────────────────────────────────────────────
    // A square, because a package is square-ish and because the label inside it
    // has to stay readable when the board is narrow.
    let side = (chip_rect.width() * 0.42).clamp(48.0, 150.0);
    let chip_sq = egui::Rect::from_center_size(
        egui::pos2(
            name_center.x,
            name_center.y - name_height * 0.5 - side * 0.62,
        ),
        egui::vec2(side, side),
    );
    painter.rect_filled(chip_sq, 3.0, CHIP_FILL);
    painter.text(
        chip_sq.center(),
        egui::Align2::CENTER_CENTER,
        part,
        egui::FontId::proportional((side * 0.22).clamp(9.0, 20.0)),
        egui::Color32::WHITE,
    );

    // ── The radio can, on the boards that have one ──────────────────────────
    // Keyed on the WL_ pads rather than on the name: a board carrying the radio
    // has to describe those pads anyway, so there is nothing extra to get wrong.
    if !mcu.iter_all_pins().any(|p| p.name.starts_with("WL_")) {
        return;
    }
    // Between header pins 13 and 17 — where the module sits on the real board.
    let edge = |n: usize| super::geometry::pin_geom(mcu, chip_rect, n).map(|g| g.rect.center().y);
    let (Some(top), Some(bottom)) = (edge(RADIO_TOP_PIN), edge(RADIO_BOTTOM_PIN)) else {
        return;
    };
    let w = chip_rect.width() * 0.60;
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(chip_rect.center().x - w * 0.5, top),
            egui::pos2(chip_rect.center().x + w * 0.5, bottom),
        ),
        2.0,
        egui::Color32::from_rgb(200, 200, 200),
    );
}

/// The header pins the radio module sits between on a Pico W.
const RADIO_TOP_PIN: usize = 13;
const RADIO_BOTTOM_PIN: usize = 17;

/// Draw the chip body.
pub fn draw_chip_body(painter: &egui::Painter, chip_rect: egui::Rect, fill: egui::Color32) {
    painter.rect_filled(chip_rect, 4.0, fill);
}

/// Draw the chip body as a rotated quad — the 45° diamond (QFP rotation).
pub fn draw_chip_body_diamond(painter: &egui::Painter, chip_rect: egui::Rect, rot: Rot) {
    painter.add(egui::Shape::convex_polygon(
        rot.quad(chip_rect),
        CHIP_FILL,
        egui::Stroke::NONE,
    ));
}

// ── Rotated pin rendering ────────────────────────────────────────────────────
// Geometry comes from `super::geometry` — the same source the un-rotated
// renderer and the module anchors read, so a rotated diagram can't drift from
// an upright one.

/// Render pins for a rotated chip. `chip_rect` is the LOCAL (un-rotated) body
/// rect; `rot` carries the angle + centre. Returns the clicked pin number.
pub fn render_pins_rotated(
    mcu: &Mcu,
    painter: &egui::Painter,
    chip_rect: egui::Rect,
    rot: Rot,
    mode: RotMode,
    ui: &mut egui::Ui,
) -> Option<usize> {
    match mode {
        RotMode::Quarter => render_quarter(mcu, painter, chip_rect, rot, ui),
        RotMode::Diamond => render_diamond(mcu, painter, chip_rect, rot, ui),
        RotMode::None => render_pins_and_detect_clicks(mcu, painter, chip_rect, ui),
    }
}

/// 90° (2-sided): the rotated pin rect stays axis-aligned, so re-use the normal
/// per-(screen-)side pin renderer — labels + hit-testing come for free.
fn render_quarter(
    mcu: &Mcu,
    painter: &egui::Painter,
    chip_rect: egui::Rect,
    rot: Rot,
    ui: &mut egui::Ui,
) -> Option<usize> {
    let selected = mcu.selected_pin;
    let hits = mcu.pin_search_highlight();
    let mut clicked = None;
    for lp in pin_geometry(mcu, chip_rect) {
        let rr = egui::Rect::from_points(&rot.quad(lp.rect));
        let is_sel = selected == Some(lp.pin.number);
        let painter = &dimmed(
            painter,
            hits.as_ref().is_some_and(|h| !h.contains(&lp.pin.number)),
        );
        let (x, y) = (rr.min.x, rr.min.y);
        let hit = match ScreenSide::from_outward(rot.vec(lp.outward)) {
            ScreenSide::Right => {
                lp.pin
                    .draw_right(painter, x, y, PIN_HEIGHT, PIN_WIDTH, Some(ui), is_sel)
                    .1
            }
            ScreenSide::Left => {
                lp.pin
                    .draw_left(painter, x, y, PIN_HEIGHT, PIN_WIDTH, Some(ui), is_sel)
                    .1
            }
            ScreenSide::Top => {
                lp.pin
                    .draw_top(painter, x, y, PIN_HEIGHT, PIN_WIDTH, Some(ui), is_sel)
                    .1
            }
            ScreenSide::Bottom => {
                lp.pin
                    .draw_bottom(painter, x, y, PIN_HEIGHT, PIN_WIDTH, Some(ui), is_sel)
                    .1
            }
        };
        draw_number(
            painter,
            selected.is_none(),
            rot.apply(lp.num_pos),
            egui::Align2::CENTER_CENTER,
            lp.pin,
        );
        if hit {
            clicked = Some(lp.pin.number);
        }
    }
    clicked
}

/// 45° (4-sided): a true rotation — draw each pin as a rotated quad with a
/// horizontal outward label, and hit-test by inverse-rotating the pointer into
/// the local frame (one interaction over the whole chip avoids AABB overlap).
fn render_diamond(
    mcu: &Mcu,
    painter: &egui::Painter,
    chip_rect: egui::Rect,
    rot: Rot,
    ui: &mut egui::Ui,
) -> Option<usize> {
    let selected = mcu.selected_pin;
    let hits = mcu.pin_search_highlight();
    let locals: Vec<PinGeom> = pin_geometry(mcu, chip_rect).collect();

    let mut bb = egui::Rect::from_points(&rot.quad(chip_rect));
    for lp in &locals {
        bb = bb.union(egui::Rect::from_points(&rot.quad(lp.rect)));
    }
    let resp = ui.interact(bb, ui.id().with("chip_diamond_pins"), egui::Sense::click());
    // Inverse-rotate the pointer into the local frame. `hover_pos` applies the
    // Scene's layer transform, so this is correct at any zoom; on the click
    // frame the pointer is where the click landed, so it doubles as the click
    // position (avoids `interact_pointer_pos`, which is not layer-adjusted).
    let hover_local = resp.hover_pos().map(|p| rot.inverse(p));
    let clicked_now = resp.clicked();

    let mut clicked = None;
    for lp in &locals {
        let is_sel = selected == Some(lp.pin.number);
        let hovered = hover_local.is_some_and(|p| lp.rect.contains(p));
        let painter = &dimmed(
            painter,
            hits.as_ref().is_some_and(|h| !h.contains(&lp.pin.number)),
        );
        painter.add(egui::Shape::convex_polygon(
            rot.quad(lp.rect),
            lp.pin.get_background_color(),
            egui::Stroke::NONE,
        ));
        if is_sel || hovered {
            // Selected and hovered are both white here (see `SELECTION_COLOR`),
            // so the DOUBLE-thick border is what tells them apart.
            painter.add(egui::Shape::convex_polygon(
                rot.quad(lp.rect),
                egui::Color32::TRANSPARENT,
                egui::Stroke::new(if is_sel { 3.0_f32 } else { 1.5_f32 }, egui::Color32::WHITE),
            ));
        }
        // Label positioned exactly like the un-rotated layout (over the pin,
        // extending outward) but rigidly rotated by θ. A `TextShape` whose
        // pos/angle are the DEFAULT per-side placement carried through `rot`:
        // the base rotation pivots on the layout origin, so `pos = rot·pos_local`
        // + `angle = base + θ` reproduces the default rotated exactly.
        let (tcol, tsize) = if is_sel {
            (
                egui::Color32::WHITE,
                PIN_FONT_SIZE * crate::panels::mcu_module::pins::gui::draw::SELECTED_TEXT_SCALE,
            )
        } else {
            (lp.pin.get_text_color(), PIN_FONT_SIZE)
        };
        // Cut a name too long for the stub, same as the un-rotated sides. The
        // label runs along the stub, whose length is `PIN_HEIGHT` either way.
        let font = egui::FontId::monospace(tsize);
        let label = crate::panels::mcu_module::pins::gui::draw::text::fit(
            painter,
            &lp.pin.name,
            font.clone(),
            PIN_HEIGHT - 6.0,
        );
        let galley = painter.layout_no_wrap(label, font, tcol);
        let (pos_local, base) = if lp.outward.y == 0.0 {
            // left/right → default horizontal label (LEFT_CENTER at mid-height)
            (
                egui::pos2(
                    lp.rect.left() + 2.0,
                    lp.rect.center().y - galley.size().y / 2.0,
                ),
                0.0_f32,
            )
        } else {
            // top/bottom → default vertical (−90°) label
            (
                egui::pos2(
                    lp.rect.left() + lp.rect.width() / 3.4,
                    lp.rect.top() + lp.rect.height() - 4.0,
                ),
                -std::f32::consts::FRAC_PI_2,
            )
        };
        painter.add(egui::epaint::TextShape {
            pos: rot.apply(pos_local),
            galley,
            underline: egui::Stroke::NONE,
            override_text_color: Some(tcol),
            angle: base + rot.angle,
            fallback_color: tcol,
            opacity_factor: 1.0,
        });
        // Pin number, upright, nudged further inside the body (−outward) so the
        // rotated stub doesn't sit on top of it.
        draw_number(
            painter,
            selected.is_none(),
            rot.apply(lp.num_pos - lp.outward * 7.0),
            egui::Align2::CENTER_CENTER,
            lp.pin,
        );
        if clicked_now && hovered {
            clicked = Some(lp.pin.number);
        }
    }
    clicked
}

/// Render every pin upright and detect clicks.
/// Returns `Some(pin_number)` if a pin was clicked.
///
/// One loop over [`pin_geometry`], not four hand-rolled per-side loops: the
/// placement is the shared one, and only the per-side DRAW call differs (each
/// `Pin::draw_*` puts the label and the notch on its own edge).
pub fn render_pins_and_detect_clicks(
    mcu: &Mcu,
    painter: &egui::Painter,
    chip_rect: egui::Rect,
    ui: &mut egui::Ui,
) -> Option<usize> {
    let mut clicked_pin: Option<usize> = None;
    let selected = mcu.selected_pin;
    let hits = mcu.pin_search_highlight();

    for g in pin_geometry(mcu, chip_rect) {
        // Balls live INSIDE the body — the very area a selected pin's function
        // list takes over. Drawing them under it would show through the rows and,
        // worse, keep a click target beneath every row. So they step aside while
        // the list is open, exactly as the pin numbers do; clicking off the chip
        // closes the list and brings them back.
        if selected.is_some() && matches!(g.place, PinPlace::Ball { .. }) {
            continue;
        }
        let is_sel = selected == Some(g.pin.number);
        // Pins the search filtered out are painted through a half-opacity
        // painter — background, label, number and all, so nothing of theirs
        // stays at full strength.
        let painter = &dimmed(
            painter,
            hits.as_ref().is_some_and(|h| !h.contains(&g.pin.number)),
        );
        let hit = match &g.place {
            PinPlace::Edge(side) => {
                let draw = match side {
                    PinSide::Right => Pin::draw_right,
                    PinSide::Left => Pin::draw_left,
                    PinSide::Top => Pin::draw_top,
                    PinSide::Bottom => Pin::draw_bottom,
                };
                draw(
                    g.pin,
                    painter,
                    g.rect.min.x,
                    g.rect.min.y,
                    PIN_HEIGHT,
                    PIN_WIDTH,
                    Some(ui),
                    is_sel,
                )
                .1
            }
            PinPlace::Ball { .. } => g.pin.draw_ball(painter, g.rect, Some(ui), is_sel),
        };
        // The pin's identity on the package: its number along an edge, its
        // designator ("A2") under a ball.
        match &g.place {
            PinPlace::Edge(_) => {
                draw_number(painter, selected.is_none(), g.num_pos, g.num_align, g.pin)
            }
            PinPlace::Ball { designator } => draw_designator(
                painter,
                selected.is_none(),
                g.num_pos,
                g.num_align,
                designator,
                g.pin,
            ),
        }
        if hit {
            clicked_pin = Some(g.pin.number);
        }
    }

    clicked_pin
}
