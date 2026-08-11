//! Chip body and pin rendering — draws the MCU chip and its pins on 4 sides.

use super::layout;
use super::rotate::{Rot, RotMode, ScreenSide};
use crate::panels::mcu_module::mcu::model::{Mcu, PIN_HEIGHT, PIN_SPACING, PIN_WIDTH};
use crate::panels::mcu_module::pins::logic::pin::{PIN_FONT_SIZE, Pin};
use eframe::egui;

// ── Pin-number label (drawn INSIDE the chip body) ───────────────────────────
/// Inset from the chip edge for the number.
const NUM_MARGIN: f32 = 4.0;
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

/// Draw the chip body (gray rectangle).
pub fn draw_chip_body(painter: &egui::Painter, chip_rect: egui::Rect) {
    painter.rect_filled(chip_rect, 4.0, egui::Color32::from_rgb(45, 45, 55));
}

/// Draw the chip body as a rotated quad — the 45° diamond (QFP rotation).
pub fn draw_chip_body_diamond(painter: &egui::Painter, chip_rect: egui::Rect, rot: Rot) {
    painter.add(egui::Shape::convex_polygon(
        rot.quad(chip_rect),
        egui::Color32::from_rgb(45, 45, 55),
        egui::Stroke::NONE,
    ));
}

// ── Rotated pin rendering ────────────────────────────────────────────────────
/// One pin's LOCAL (un-rotated) geometry — identical to the default per-side
/// layout above — so the rotated renderers can compute once then apply [`Rot`].
struct LocalPin<'a> {
    pin: &'a Pin,
    /// Pin stub rect (local frame).
    rect: egui::Rect,
    /// Unit vector pointing away from the chip.
    outward: egui::Vec2,
    /// Pin-number position, just inside the chip edge.
    num_pos: egui::Pos2,
}

/// Every pin with its local geometry, mirroring [`render_pins_and_detect_clicks`].
fn local_pins(mcu: &Mcu, chip_rect: egui::Rect) -> Vec<LocalPin<'_>> {
    let mut v = Vec::new();
    for (i, pin) in mcu.right_pins.iter().enumerate() {
        let y = chip_rect.top() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let rect = egui::Rect::from_min_size(
            egui::pos2(chip_rect.right(), y),
            egui::vec2(PIN_HEIGHT, PIN_WIDTH),
        );
        v.push(LocalPin {
            pin,
            rect,
            outward: egui::vec2(1.0, 0.0),
            num_pos: egui::pos2(chip_rect.right() - NUM_MARGIN, y + PIN_WIDTH / 2.0),
        });
    }
    for (i, pin) in mcu.left_pins.iter().enumerate() {
        let y = chip_rect.top() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let rect = egui::Rect::from_min_size(
            egui::pos2(chip_rect.left() - PIN_HEIGHT, y),
            egui::vec2(PIN_HEIGHT, PIN_WIDTH),
        );
        v.push(LocalPin {
            pin,
            rect,
            outward: egui::vec2(-1.0, 0.0),
            num_pos: egui::pos2(chip_rect.left() + NUM_MARGIN, y + PIN_WIDTH / 2.0),
        });
    }
    for (i, pin) in mcu.top_pins.iter().enumerate() {
        let x = chip_rect.left() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let rect = egui::Rect::from_min_size(
            egui::pos2(x, chip_rect.top() - PIN_HEIGHT),
            egui::vec2(PIN_WIDTH, PIN_HEIGHT),
        );
        v.push(LocalPin {
            pin,
            rect,
            outward: egui::vec2(0.0, -1.0),
            num_pos: egui::pos2(x + PIN_WIDTH / 2.0, chip_rect.top() + NUM_MARGIN),
        });
    }
    for (i, pin) in mcu.bottom_pins.iter().enumerate() {
        let x = chip_rect.left() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let rect = egui::Rect::from_min_size(
            egui::pos2(x, chip_rect.bottom()),
            egui::vec2(PIN_WIDTH, PIN_HEIGHT),
        );
        v.push(LocalPin {
            pin,
            rect,
            outward: egui::vec2(0.0, 1.0),
            num_pos: egui::pos2(x + PIN_WIDTH / 2.0, chip_rect.bottom() - NUM_MARGIN),
        });
    }
    v
}

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
    let mut clicked = None;
    for lp in local_pins(mcu, chip_rect) {
        let rr = egui::Rect::from_points(&rot.quad(lp.rect));
        let is_sel = selected == Some(lp.pin.number);
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
    let locals = local_pins(mcu, chip_rect);

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
                egui::Stroke::new(if is_sel { 3.0 } else { 1.5 }, egui::Color32::WHITE),
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

/// Render all pins on all 4 sides and detect clicks.
/// Returns `Some(pin_number)` if a pin was clicked.
pub fn render_pins_and_detect_clicks(
    mcu: &Mcu,
    painter: &egui::Painter,
    chip_rect: egui::Rect,
    ui: &mut egui::Ui,
) -> Option<usize> {
    let mut clicked_pin: Option<usize> = None;
    let selected = mcu.selected_pin;

    // RIGHT
    for (i, pin) in mcu.right_pins.iter().enumerate() {
        let y = chip_rect.top() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let x = chip_rect.right();
        let (_, hit) = pin.draw_right(
            &painter,
            x,
            y,
            PIN_HEIGHT,
            PIN_WIDTH,
            Some(ui),
            selected == Some(pin.number),
        );
        // Pin number inside the chip, right-aligned just inside the right edge.
        draw_number(
            painter,
            selected.is_none(),
            egui::pos2(chip_rect.right() - NUM_MARGIN, y + PIN_WIDTH / 2.0),
            egui::Align2::RIGHT_CENTER,
            pin,
        );
        if hit {
            clicked_pin = Some(pin.number);
        }
    }

    // LEFT
    for (i, pin) in mcu.left_pins.iter().enumerate() {
        let y = chip_rect.top() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let x = chip_rect.left() - PIN_HEIGHT;
        let (_, hit) = pin.draw_left(
            &painter,
            x,
            y,
            PIN_HEIGHT,
            PIN_WIDTH,
            Some(ui),
            selected == Some(pin.number),
        );
        // Pin number inside the chip, left-aligned just inside the left edge.
        draw_number(
            painter,
            selected.is_none(),
            egui::pos2(chip_rect.left() + NUM_MARGIN, y + PIN_WIDTH / 2.0),
            egui::Align2::LEFT_CENTER,
            pin,
        );
        if hit {
            clicked_pin = Some(pin.number);
        }
    }

    // TOP
    for (i, pin) in mcu.top_pins.iter().enumerate() {
        let x = chip_rect.left() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let y = chip_rect.top() - PIN_HEIGHT;
        let (_, hit) = pin.draw_top(
            &painter,
            x,
            y,
            PIN_HEIGHT,
            PIN_WIDTH,
            Some(ui),
            selected == Some(pin.number),
        );
        // Pin number inside the chip, just below the top edge.
        draw_number(
            painter,
            selected.is_none(),
            egui::pos2(x + PIN_WIDTH / 2.0, chip_rect.top() + NUM_MARGIN),
            egui::Align2::CENTER_TOP,
            pin,
        );
        if hit {
            clicked_pin = Some(pin.number);
        }
    }

    // BOTTOM
    for (i, pin) in mcu.bottom_pins.iter().enumerate() {
        let x = chip_rect.left() + PIN_SPACING + i as f32 * (PIN_WIDTH + PIN_SPACING);
        let y = chip_rect.bottom();
        let (_, hit) = pin.draw_bottom(
            &painter,
            x,
            y,
            PIN_HEIGHT,
            PIN_WIDTH,
            Some(ui),
            selected == Some(pin.number),
        );
        // Pin number inside the chip, just above the bottom edge.
        draw_number(
            painter,
            selected.is_none(),
            egui::pos2(x + PIN_WIDTH / 2.0, chip_rect.bottom() - NUM_MARGIN),
            egui::Align2::CENTER_BOTTOM,
            pin,
        );
        if hit {
            clicked_pin = Some(pin.number);
        }
    }

    clicked_pin
}
