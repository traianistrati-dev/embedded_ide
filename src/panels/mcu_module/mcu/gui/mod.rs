//! MCU GUI rendering — orchestrates chip visualization and pin configuration.
//!
//! The draw() method coordinates multiple components:
//! - layout: geometry calculations
//! - chip: chip body + pin rendering
//! - panel: function selection UI
//! - info: information popup

pub mod chip;
pub mod clock;
pub mod geometry;
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
    /// Main rendering method — draws the chip, its pins, the selected pin's
    /// function list inside the body, the virtual modules and the in/out fields.
    ///
    /// Returns `(pin_number, pin_name, selected_function)` when a pin's function
    /// changes, plus the function list's rect in SCREEN coordinates — the caller
    /// needs it to route the mouse wheel to the list instead of the canvas zoom
    /// (see [`panel::draw_pin_functions`]).
    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
    ) -> (Option<(usize, String, PinFunction)>, egui::Rect) {
        let top_count = self.top_pins.len();
        let left_count = self.left_pins.len();

        // Drop any module wire whose pin was re-purposed away from USART.
        self.reconcile_modules();

        let (mcu_width, mcu_height, base_w, base_h) = match &self.grid {
            // A ball-grid package has no edge pins to size the body from — the
            // body must instead be big enough to HOLD the grid.
            Some(g) => layout::calculate_grid_layout(geometry::grid_body_size(g)),
            None => layout::calculate_layout(top_count, left_count),
        };

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

        // Toggle selected_pin (click again to deselect); reset the list scroll on
        // change, so another pin's list starts at the top.
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
        // module list to collapse, so the canvas and the list agree. The chip
        // BODY is not background: it is the chip itself, and a click that merely
        // missed a pin by a few pixels shouldn't throw the selection away.
        let on_body = response
            .interact_pointer_pos()
            .is_some_and(|p| display_chip.contains(p));
        if response.clicked() && !on_body {
            self.selected_pin = None;
            self.selected_module = None;
            self.collapse_modules = true;
        }

        // ── Inner chip panel ─────────────────────────────────────────────────
        // A selected pin turns the body into its function list; otherwise the
        // body carries the chip's name — but ONLY on a package whose pins are on
        // the outside. A ball grid fills that same body with pads, so the name
        // would be painted straight through them (and through their designators).
        // The chip is named in the tab header anyway; the balls are not.
        if self.selected_pin.is_some() {
            panel::draw_pin_functions(self, &painter, ui, content_rect)
        } else {
            if !self.has_inner_pins() {
                painter.text(
                    content_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &self.name,
                    egui::FontId::proportional(22.0),
                    egui::Color32::WHITE,
                );
            }
            (None, egui::Rect::NOTHING)
        }
    }
}
