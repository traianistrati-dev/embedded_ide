//! The selected pin's function list — painted INSIDE the chip body.
//!
//! It lives in the chip (over the body, under the pin stubs) and is driven by
//! `Mcu::fn_scroll_offset`: a hand-painted list rather than an egui `ScrollArea`,
//! because it is drawn inside the [`egui::Scene`] of the Pins canvas and must
//! scale with it.
//!
//! Two things the caller must do for it, both because of that Scene:
//! * the pin NUMBERS around the body are hidden while it is open (they are
//!   painted at the same place and would show through the rows) — see
//!   [`super::chip`];
//! * the wheel over it must scroll it instead of zooming the canvas. It cannot
//!   do that itself: `mcu_panel.rs` intercepts the wheel BEFORE this runs, so it
//!   is the one that feeds `fn_scroll_offset`. This function hands back its rect
//!   in SCREEN coordinates for exactly that test.

use super::info;
use crate::panels::mcu_module::mcu::model::Mcu;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
use eframe::egui;

/// Width of the trailing ⓘ button.
const INFO_BTN_W: f32 = 22.0;
/// Gap between a function button and its ⓘ.
const GAP: f32 = 4.0;
/// Height of one function button, and the pitch between two rows.
const BTN_H: f32 = 28.0;
const ITEM_H: f32 = BTN_H + 6.0;
/// Scrollbar width + its gap from the buttons.
const SB_W: f32 = 4.0;
const SB_GAP: f32 = 3.0;

/// The functions offered for `num`: everything the pin can do, minus what is
/// already taken by another pin (GPIO In/Out are always offered — any pin can be
/// one).
fn selectable_functions(mcu: &Mcu, num: usize) -> Option<(String, Vec<PinFunction>, PinFunction)> {
    let used_elsewhere: Vec<PinFunction> = mcu
        .iter_all_pins()
        .filter(|p| p.number != num && p.selected_function != PinFunction::Unset)
        .map(|p| p.selected_function.clone())
        .collect();
    let pin = mcu.find_pin(num)?;
    let mut funcs: Vec<PinFunction> = pin
        .available_functions
        .iter()
        .filter(|f| {
            matches!(f, PinFunction::GpioInput | PinFunction::GpioOutput)
                || !used_elsewhere.contains(f)
        })
        .cloned()
        .collect();
    // The pin's CURRENT function always heads the list: it is the one the user
    // came to see (and to click again to clear), and on a long list it would
    // otherwise sit scrolled out of sight.
    if let Some(i) = funcs.iter().position(|f| *f == pin.selected_function) {
        let cur = funcs.remove(i);
        funcs.insert(0, cur);
    }
    Some((pin.name.clone(), funcs, pin.selected_function.clone()))
}

/// Draw the selected pin's header + function list inside `content_rect` (the
/// upright area of the chip body, scene coords).
///
/// Returns the `(number, name, function)` change when the user picks one, plus
/// the list's rect in SCREEN coordinates — [`egui::Rect::NOTHING`] when no pin is
/// selected, i.e. when nothing was drawn.
pub fn draw_pin_functions(
    mcu: &mut Mcu,
    painter: &egui::Painter,
    ui: &mut egui::Ui,
    content_rect: egui::Rect,
) -> (Option<(usize, String, PinFunction)>, egui::Rect) {
    let Some(num) = mcu.selected_pin else {
        return (None, egui::Rect::NOTHING);
    };
    let Some((pin_name, funcs, selected_func)) = selectable_functions(mcu, num) else {
        return (None, egui::Rect::NOTHING);
    };

    // ── Header ───────────────────────────────────────────────────────────────
    let header_pos = content_rect.center_top() + egui::vec2(0.0, 14.0);
    painter.text(
        header_pos,
        egui::Align2::CENTER_CENTER,
        format!("Pin {num}  ·  {pin_name}"),
        egui::FontId::proportional(13.0),
        egui::Color32::WHITE,
    );
    let sep_y = header_pos.y + 14.0;
    painter.line_segment(
        [
            egui::pos2(content_rect.left() + 8.0, sep_y),
            egui::pos2(content_rect.right() - 8.0, sep_y),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 100, 120)),
    );

    // ── Geometry ─────────────────────────────────────────────────────────────
    let btn_x = content_rect.left() + 12.0;
    let content_top = sep_y + 12.0;
    let content_bottom = content_rect.bottom() - 8.0;
    let available_h = (content_bottom - content_top).max(0.0);
    let total_h = funcs.len() as f32 * ITEM_H;
    let max_scroll = (total_h - available_h).max(0.0);
    mcu.fn_scroll_offset = mcu.fn_scroll_offset.clamp(0.0, max_scroll);
    let btn_w = content_rect.width() - 24.0 - INFO_BTN_W - GAP - SB_W - SB_GAP;

    let list_rect = egui::Rect::from_min_max(
        egui::pos2(btn_x - 4.0, content_top),
        egui::pos2(content_rect.right() - SB_W - SB_GAP - 1.0, content_bottom),
    );

    // ── Scrollbar thumb ──────────────────────────────────────────────────────
    if max_scroll > 0.0 {
        let sb_x = content_rect.right() - SB_W - 2.0;
        let thumb_h = ((available_h / total_h) * available_h).max(16.0);
        let thumb_top = content_top + (mcu.fn_scroll_offset / max_scroll) * (available_h - thumb_h);
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(sb_x, thumb_top), egui::vec2(SB_W, thumb_h)),
            SB_W / 2.0,
            egui::Color32::from_rgba_premultiplied(180, 180, 210, 140),
        );
    }

    // ── Rows ─────────────────────────────────────────────────────────────────
    let list_painter = painter.with_clip_rect(list_rect);
    let mut btn_y = content_top - mcu.fn_scroll_offset;
    let mut new_function: Option<(usize, PinFunction)> = None;
    let mut toggle_info: Option<PinFunction> = None;
    let show_info = mcu.show_info.clone();

    for (i, func) in funcs.iter().enumerate() {
        let btn_rect =
            egui::Rect::from_min_size(egui::pos2(btn_x, btn_y), egui::vec2(btn_w, BTN_H));
        let info_rect = egui::Rect::from_min_size(
            egui::pos2(btn_x + btn_w + GAP, btn_y),
            egui::vec2(INFO_BTN_W, BTN_H),
        );
        let visible = btn_rect.bottom() > content_top && btn_rect.top() < content_bottom;

        let is_sel = func == &selected_func;
        let bg = if is_sel {
            func.color()
        } else {
            egui::Color32::from_rgb(65, 65, 80)
        };
        list_painter.rect_filled(btn_rect, 5.0, bg);
        list_painter.text(
            btn_rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{}  —  {}", func.short_label(), func.label()),
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );

        // ⓘ button — hand-drawn so it matches the painted list.
        let info_bg = if show_info.as_ref() == Some(func) {
            egui::Color32::from_rgb(80, 120, 200)
        } else {
            egui::Color32::from_rgb(55, 55, 75)
        };
        list_painter.rect_filled(info_rect, 5.0, info_bg);
        let ic = info_rect.center();
        list_painter.circle_stroke(ic, 7.5, egui::Stroke::new(1.5, egui::Color32::WHITE));
        list_painter.circle_filled(egui::pos2(ic.x, ic.y - 2.5), 1.3, egui::Color32::WHITE);
        list_painter.line_segment(
            [egui::pos2(ic.x, ic.y - 0.5), egui::pos2(ic.x, ic.y + 4.0)],
            egui::Stroke::new(1.8, egui::Color32::WHITE),
        );

        // Only rows actually on screen take clicks — a scrolled-away button must
        // not keep a hit area over the chip.
        if visible {
            let btn_response = ui.interact(
                btn_rect,
                ui.id().with(("fn_btn", num, i)),
                egui::Sense::click(),
            );
            if btn_response.hovered() {
                list_painter.rect_stroke(
                    btn_rect,
                    5.0,
                    egui::Stroke::new(1.5, egui::Color32::WHITE),
                    egui::StrokeKind::Middle,
                );
            }
            if btn_response.clicked() {
                // Clicking the ACTIVE function clears the pin — it is a toggle.
                let next = if is_sel {
                    PinFunction::Unset
                } else {
                    func.clone()
                };
                new_function = Some((num, next));
            }

            let info_response = ui.interact(
                info_rect,
                ui.id().with(("info_btn", num, i)),
                egui::Sense::click(),
            );
            if info_response.hovered() {
                list_painter.rect_stroke(
                    info_rect,
                    5.0,
                    egui::Stroke::new(1.5, egui::Color32::WHITE),
                    egui::StrokeKind::Middle,
                );
            }
            if info_response.clicked() {
                toggle_info = Some(func.clone());
            }
        }

        btn_y += ITEM_H;
    }

    // ── Apply ────────────────────────────────────────────────────────────────
    // Applying also clears `show_info`, so it runs before the toggle below.
    let changed = new_function.and_then(|(n, f)| mcu.apply_pin_function(n, f));
    if let Some(func) = toggle_info {
        mcu.show_info = if mcu.show_info.as_ref() == Some(&func) {
            None
        } else {
            Some(func)
        };
    }

    // This all lives inside the canvas' Scene, so scene coords ≠ screen coords as
    // soon as the user zooms or pans. Map the list rect out to screen space for
    // the caller's wheel test — and anchor the ⓘ window there too, instead of on
    // the scene-space chip rect (which would place it wherever the diagram is).
    let to_global = ui
        .ctx()
        .layer_transform_to_global(ui.layer_id())
        .unwrap_or_default();
    let list_screen = to_global * list_rect;

    if let Some(func) = mcu.show_info.clone()
        && !info::draw_info_popup(&func, list_screen, ui)
    {
        mcu.show_info = None;
    }

    (changed, list_screen)
}
