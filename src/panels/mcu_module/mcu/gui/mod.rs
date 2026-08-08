//! MCU GUI rendering — orchestrates chip visualization and pin configuration.
//!
//! The draw() method coordinates multiple components:
//! - layout: geometry calculations
//! - chip: chip body + pin rendering
//! - panel: function selection UI
//! - info: information popup

pub mod chip;
pub mod clock;
pub mod info;
pub mod io_arrows;
pub mod layout;
pub mod modules;
pub mod panel;
pub mod rotate;

use crate::panels::mcu_module::mcu::logic::partner_functions;
use crate::panels::mcu_module::mcu::model::{Mcu, PIN_HEIGHT};
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
use eframe::egui;

impl Mcu {
    /// Main rendering method — draws the chip and handles user interactions.
    /// Returns `(pin_number, pin_name, selected_function)` when a pin configuration changes.
    pub fn draw(&mut self, ui: &mut egui::Ui) -> Option<(usize, String, PinFunction)> {
        let top_count = self.top_pins.len();
        let left_count = self.left_pins.len();

        // Drop any module wire whose pin was re-purposed away from USART.
        self.reconcile_modules();

        let (mcu_width, mcu_height, base_w, base_h) =
            layout::calculate_layout(top_count, left_count);

        // Diagram rotation (view-only): a 2-sided chip turns 90°, a 4-sided one
        // becomes a 45° diamond. `local_chip` is the un-rotated body used to
        // compute pin geometry; `display_chip` is the axis-aligned rect the body
        // + info panel + modules use; `content_rect` is where the inner panel
        // draws (shrunk to fit the diamond). See `rotate.rs`.
        let rot_mode = rotate::RotMode::of(self);

        // Reserve a margin all around the chip for virtual modules and in/out
        // arrows, so their boxes/arrows + wires sit beyond the pins (on the
        // pins' own side) without overlapping the chip. Use the larger of the
        // two when both are present.
        let mut mx = 0.0_f32;
        let mut my = 0.0_f32;
        if !self.modules.is_empty() {
            mx = modules::MARGIN_X;
            my = modules::MARGIN_Y;
        }
        let has_io = io_arrows::has_io_pins(self);
        if has_io {
            mx = mx.max(io_arrows::MARGIN_X);
            my = my.max(io_arrows::MARGIN_Y);
        }
        // Canvas size follows the rotation — a diamond needs a bigger square box
        // (its bounding circle spans the chip's diagonal), a 90° chip swaps axes.
        let (canvas_w, canvas_h) = match rot_mode {
            rotate::RotMode::Diamond => {
                let diag = (mcu_width * mcu_width + mcu_height * mcu_height).sqrt();
                let ext = diag + 2.0 * (PIN_HEIGHT + 64.0);
                (ext, ext)
            }
            rotate::RotMode::Quarter => (base_h, base_w),
            rotate::RotMode::None => (base_w, base_h),
        };
        // Grow the painter to cover modules dragged far from the chip, so the
        // Scene's auto-fit encompasses them instead of clipping at the panel
        // edge. The chip stays centred; the extra span is just empty canvas.
        let drag_ext = modules::dragged_half_extent(self).max(io_arrows::dragged_half_extent(self));
        let half_w = (canvas_w / 2.0 + mx).max(drag_ext.x + 16.0);
        let half_h = (canvas_h / 2.0 + my).max(drag_ext.y + 16.0);
        // The canvas senses CLICKS so empty space can clear the selection. Sensing
        // click (not drag) leaves the Scene's drag-pan alone: egui hit-tests click
        // and drag targets separately, and every pin / module / field registers
        // its own `interact` LATER, i.e. above this one — so a click only reaches
        // the background when it landed on none of them.
        let (response, painter) =
            ui.allocate_painter(egui::vec2(2.0 * half_w, 2.0 * half_h), egui::Sense::click());

        let rect = response.rect;
        let center = rect.center();
        let local_chip = egui::Rect::from_center_size(center, egui::vec2(mcu_width, mcu_height));
        let rot = rotate::Rot::new(center, rot_mode.angle());
        let display_chip = match rot_mode {
            rotate::RotMode::None => local_chip,
            rotate::RotMode::Quarter => {
                egui::Rect::from_center_size(center, egui::vec2(mcu_height, mcu_width))
            }
            rotate::RotMode::Diamond => egui::Rect::from_points(&rot.quad(local_chip)),
        };
        // The inner info panel stays upright — full body, or the largest upright
        // square inside the diamond (≈ 0.71× the body).
        let content_rect = match rot_mode {
            rotate::RotMode::Diamond => {
                let s = mcu_width.min(mcu_height) / std::f32::consts::SQRT_2;
                egui::Rect::from_center_size(center, egui::vec2(s, s))
            }
            _ => display_chip,
        };

        // ── Chip body ───────────────────────────────────────────────────────
        match rot_mode {
            rotate::RotMode::Diamond => chip::draw_chip_body_diamond(&painter, local_chip, rot),
            _ => chip::draw_chip_body(&painter, display_chip),
        }

        // ── Pins + click detection ───────────────────────────────────────────
        let clicked_pin = match rot_mode {
            rotate::RotMode::None => {
                chip::render_pins_and_detect_clicks(self, &painter, display_chip, ui)
            }
            _ => chip::render_pins_rotated(self, &painter, local_chip, rot, rot_mode, ui),
        };

        // ── Virtual modules (boxes + wires) around the chip ───────────────────
        if !self.modules.is_empty() {
            modules::draw_modules(self, &painter, local_chip, display_chip, rot, ui);
        }

        // ── In/out arrows + rename fields for GPIO In/Out/PWM pins ────────────
        if has_io {
            io_arrows::draw_io_arrows(self, &painter, local_chip, rot, ui);
        }

        // Toggle selected_pin (click again to deselect); reset scroll on change.
        if let Some(n) = clicked_pin {
            let prev = self.selected_pin;
            self.selected_pin = if self.selected_pin == Some(n) {
                None
            } else {
                Some(n)
            };
            if prev != self.selected_pin {
                self.fn_scroll_offset = 0.0;
            }
            // Selecting a CONFIGURED pin also asks the editor to jump to the
            // line that binds its variable. Only on the click that selects: the
            // second click deselects, and jumping again there would be noise. An
            // Unset pin has no binding to jump to.
            if self.selected_pin == Some(n) {
                self.request_pin_goto(n);
            }
        }

        // A click that reached the BACKGROUND — no pin, no module box, no pin
        // field took it — means "focus nothing": drop both selections and ask the
        // module list to collapse, so the canvas and the list agree. Clicks on the
        // chip BODY are excluded: the selected pin's function panel lives there,
        // and closing it from under the pointer would fight the user.
        let on_body = response
            .interact_pointer_pos()
            .is_some_and(|p| display_chip.contains(p));
        if response.clicked() && !on_body {
            self.selected_pin = None;
            self.selected_module = None;
            self.collapse_modules = true;
        }

        // ── Inner chip panel ─────────────────────────────────────────────────
        // Extract selected pin data BEFORE any mutable borrow.
        let inner_data: Option<(usize, String, Vec<PinFunction>, PinFunction)> =
            self.selected_pin.and_then(|n: usize| {
                let used_elsewhere: Vec<PinFunction> = self
                    .iter_all_pins()
                    .filter(|p| p.number != n && p.selected_function != PinFunction::Unset)
                    .map(|p| p.selected_function.clone())
                    .collect();

                self.find_pin(n).map(|p| {
                    let visible_funcs: Vec<PinFunction> = p
                        .available_functions
                        .iter()
                        .filter(|f: &&PinFunction| {
                            matches!(f, PinFunction::GpioInput | PinFunction::GpioOutput)
                                || !used_elsewhere.contains(f)
                        })
                        .cloned()
                        .collect();

                    (
                        p.number,
                        p.name.clone(),
                        visible_funcs,
                        p.selected_function.clone(),
                    )
                })
            });

        let chip_name = self.name.clone();
        let mut new_function: Option<(usize, PinFunction)> = None;
        let mut toggle_info: Option<PinFunction> = None;

        if let Some((num, pin_name, funcs, selected_func)) = inner_data {
            // Header — pin number and name
            let header_pos = content_rect.center_top() + egui::vec2(0.0, 14.0);
            painter.text(
                header_pos,
                egui::Align2::CENTER_CENTER,
                format!("Pin {}  ·  {}", num, pin_name),
                egui::FontId::proportional(13.0),
                egui::Color32::WHITE,
            );

            // Separator line
            let sep_y = header_pos.y + 14.0;
            painter.line_segment(
                [
                    egui::pos2(content_rect.left() + 8.0, sep_y),
                    egui::pos2(content_rect.right() - 8.0, sep_y),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 100, 120)),
            );

            // ── Function button list (scrollable) ──────────────────────────
            // TODO: Move this to gui/panel.rs component
            let info_btn_w = 22.0;
            let gap = 4.0;
            let btn_h = 28.0;
            let item_h = btn_h + 6.0;
            let btn_x = content_rect.left() + 12.0;

            let content_top = sep_y + 12.0;
            let content_bottom = content_rect.bottom() - 8.0;
            let available_h = (content_bottom - content_top).max(0.0);
            let total_h = funcs.len() as f32 * item_h;
            let max_scroll = (total_h - available_h).max(0.0);

            self.fn_scroll_offset = self.fn_scroll_offset.clamp(0.0, max_scroll);

            let sb_w = 4.0;
            let sb_gap = 3.0;
            let btn_w = content_rect.width() - 24.0 - info_btn_w - gap - sb_w - sb_gap;

            let list_rect = egui::Rect::from_min_max(
                egui::pos2(btn_x - 4.0, content_top),
                egui::pos2(content_rect.right() - sb_w - sb_gap - 1.0, content_bottom),
            );

            // Handle mouse-wheel scrolling
            let pointer_in_list = ui.input(|i| {
                i.pointer
                    .hover_pos()
                    .map(|p| list_rect.contains(p))
                    .unwrap_or(false)
            });
            if pointer_in_list && max_scroll > 0.0 {
                let delta = ui.input(|i| i.smooth_scroll_delta.y);
                if delta != 0.0 {
                    self.fn_scroll_offset = (self.fn_scroll_offset - delta).clamp(0.0, max_scroll);
                }
            }

            // Scrollbar thumb
            if max_scroll > 0.0 {
                let sb_x = content_rect.right() - sb_w - 2.0;
                let track_h = available_h;
                let thumb_h = ((available_h / total_h) * track_h).max(16.0);
                let thumb_top =
                    content_top + (self.fn_scroll_offset / max_scroll) * (track_h - thumb_h);
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(sb_x, thumb_top),
                        egui::vec2(sb_w, thumb_h),
                    ),
                    sb_w / 2.0,
                    egui::Color32::from_rgba_premultiplied(180, 180, 210, 140),
                );
            }

            let list_painter = painter.with_clip_rect(list_rect);
            let mut btn_y = content_top - self.fn_scroll_offset;

            for (i, func) in funcs.iter().enumerate() {
                let btn_rect =
                    egui::Rect::from_min_size(egui::pos2(btn_x, btn_y), egui::vec2(btn_w, btn_h));
                let info_rect = egui::Rect::from_min_size(
                    egui::pos2(btn_x + btn_w + gap, btn_y),
                    egui::vec2(info_btn_w, btn_h),
                );

                let visible = btn_rect.bottom() > content_top && btn_rect.top() < content_bottom;

                let is_sel: bool = func == &selected_func;
                let bg: egui::Color32 = if is_sel {
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

                // ⓘ button
                let info_open = self.show_info.as_ref() == Some(func);
                let info_bg = if info_open {
                    egui::Color32::from_rgb(80, 120, 200)
                } else {
                    egui::Color32::from_rgb(55, 55, 75)
                };
                list_painter.rect_filled(info_rect, 5.0, info_bg);
                let ic = info_rect.center();
                let ir = 7.5_f32;
                list_painter.circle_stroke(ic, ir, egui::Stroke::new(1.5, egui::Color32::WHITE));
                list_painter.circle_filled(egui::pos2(ic.x, ic.y - 2.5), 1.3, egui::Color32::WHITE);
                list_painter.line_segment(
                    [egui::pos2(ic.x, ic.y - 0.5), egui::pos2(ic.x, ic.y + 4.0)],
                    egui::Stroke::new(1.8, egui::Color32::WHITE),
                );

                // Hover / click
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
                        let next = if func == &selected_func {
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

                btn_y += item_h;
            }
        } else {
            painter.text(
                content_rect.center(),
                egui::Align2::CENTER_CENTER,
                &chip_name,
                egui::FontId::proportional(22.0),
                egui::Color32::WHITE,
            );
        }

        // Apply the selected function to the pin (sets partners + clears info).
        let pin_changed =
            new_function.and_then(|(pin_num, func)| self.apply_pin_function(pin_num, func));

        // Toggle the info popup
        if let Some(func) = toggle_info {
            if self.show_info.as_ref() == Some(&func) {
                self.show_info = None;
            } else {
                self.show_info = Some(func);
            }
        }

        // ── Info popup window ────────────────────────────────────────────────
        if let Some(ref func) = self.show_info.clone() {
            let open = info::draw_info_popup(func, content_rect, ui);
            if !open {
                self.show_info = None;
            }
        }

        pin_changed
    }
}
