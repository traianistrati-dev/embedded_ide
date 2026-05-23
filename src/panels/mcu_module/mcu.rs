use crate::panels::mcu_module::pin_module::pin::Pin;
use crate::panels::mcu_module::pin_module::pin_function::PinFunction;
use eframe::egui;

const PIN_HEIGHT: f32 = 50.0;
const PIN_WIDTH: f32 = 30.0;
const PIN_SPACING: f32 = 3.0;

pub struct Mcu {
    name: String,
    top_pins: Vec<Pin>,
    bottom_pins: Vec<Pin>,
    left_pins: Vec<Pin>,
    right_pins: Vec<Pin>,
    /// Currently selected pin number (None = no pin selected)
    selected_pin: Option<usize>,
}

impl Mcu {
    pub fn new(
        name: String,
        top_pins: Vec<Pin>,
        bottom_pins: Vec<Pin>,
        left_pins: Vec<Pin>,
        right_pins: Vec<Pin>,
    ) -> Self {
        Self {
            name,
            top_pins,
            bottom_pins,
            left_pins,
            right_pins,
            selected_pin: None,
        }
    }

    /// Finds a pin by number (immutable)
    fn find_pin(&self, number: usize) -> Option<&Pin> {
        self.top_pins
            .iter()
            .chain(self.bottom_pins.iter())
            .chain(self.left_pins.iter())
            .chain(self.right_pins.iter())
            .find(|p| p.number == number)
    }

    /// Finds a pin by number (mutable)
    fn find_pin_mut(&mut self, number: usize) -> Option<&mut Pin> {
        self.top_pins
            .iter_mut()
            .chain(self.bottom_pins.iter_mut())
            .chain(self.left_pins.iter_mut())
            .chain(self.right_pins.iter_mut())
            .find(|p| p.number == number)
    }

    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let top_count = self.top_pins.len();
        let left_count = self.left_pins.len();

        let mcu_width = (top_count as f32 * (PIN_WIDTH + PIN_SPACING)) + PIN_SPACING * 2.0;
        let mcu_height = (left_count as f32 * (PIN_WIDTH + PIN_SPACING)) + PIN_SPACING * 2.0;

        let canvas_w = mcu_width + PIN_HEIGHT * 2.0 + 20.0;
        let canvas_h = mcu_height + PIN_HEIGHT * 2.0 + 20.0;

        let (response, painter) =
            ui.allocate_painter(egui::vec2(canvas_w, canvas_h), egui::Sense::hover());

        let rect = response.rect;
        let chip_rect =
            egui::Rect::from_center_size(rect.center(), egui::vec2(mcu_width, mcu_height));

        // ── Chip body ───────────────────────────────────────────────────────
        painter.rect_filled(chip_rect, 4.0, egui::Color32::from_rgb(45, 45, 55));

        // ── Pins + click detection ───────────────────────────────────────────
        let mut clicked_pin: Option<usize> = None;
        let selected = self.selected_pin;

        // RIGHT
        for (i, pin) in self.right_pins.iter().enumerate() {
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
            if hit {
                clicked_pin = Some(pin.number);
            }
        }

        // LEFT
        for (i, pin) in self.left_pins.iter().enumerate() {
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
            if hit {
                clicked_pin = Some(pin.number);
            }
        }

        // TOP
        for (i, pin) in self.top_pins.iter().enumerate() {
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
            if hit {
                clicked_pin = Some(pin.number);
            }
        }

        // BOTTOM
        for (i, pin) in self.bottom_pins.iter().enumerate() {
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
            if hit {
                clicked_pin = Some(pin.number);
            }
        }

        // Toggle selected_pin (click again to deselect)
        if let Some(n) = clicked_pin {
            self.selected_pin = if self.selected_pin == Some(n) {
                None
            } else {
                Some(n)
            };
        }

        // ── Inner chip panel ─────────────────────────────────────────────────
        // Extract selected pin data BEFORE any mutable borrow
        let inner_data: Option<(usize, String, Vec<PinFunction>, PinFunction)> =
            self.selected_pin.and_then(|n| {
                self.find_pin(n).map(|p| {
                    (
                        p.number,
                        p.name.clone(),
                        p.available_functions.clone(),
                        p.selected_function.clone(),
                    )
                })
            });

        let chip_name = self.name.clone();
        let mut new_function: Option<(usize, PinFunction)> = None;

        if let Some((num, pin_name, funcs, selected_func)) = inner_data {
            // Header — pin number and name
            let header_pos = chip_rect.center_top() + egui::vec2(0.0, 14.0);
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
                    egui::pos2(chip_rect.left() + 8.0, sep_y),
                    egui::pos2(chip_rect.right() - 8.0, sep_y),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 100, 120)),
            );

            // Function buttons
            let btn_w = chip_rect.width() - 24.0;
            let btn_h = 28.0;
            let btn_x = chip_rect.left() + 12.0;
            let mut btn_y = sep_y + 12.0;

            for (i, func) in funcs.iter().enumerate() {
                let btn_rect =
                    egui::Rect::from_min_size(egui::pos2(btn_x, btn_y), egui::vec2(btn_w, btn_h));

                let is_sel = func == &selected_func;
                let bg = if is_sel {
                    func.color()
                } else {
                    egui::Color32::from_rgb(65, 65, 80)
                };

                painter.rect_filled(btn_rect, 5.0, bg);

                // Label: short label on the left, full label on the right
                painter.text(
                    btn_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}  —  {}", func.short_label(), func.label()),
                    egui::FontId::proportional(11.0),
                    egui::Color32::WHITE,
                );

                // Hover border
                let btn_response = ui.interact(
                    btn_rect,
                    ui.id().with(("fn_btn", num, i)),
                    egui::Sense::click(),
                );
                if btn_response.hovered() {
                    painter.rect_stroke(
                        btn_rect,
                        5.0,
                        egui::Stroke::new(1.5, egui::Color32::WHITE),
                        egui::StrokeKind::Middle,
                    );
                }
                if btn_response.clicked() {
                    new_function = Some((num, func.clone()));
                }

                btn_y += btn_h + 6.0;
            }
        } else {
            // No pin selected — show the chip name
            painter.text(
                chip_rect.center(),
                egui::Align2::CENTER_CENTER,
                &chip_name,
                egui::FontId::proportional(22.0),
                egui::Color32::WHITE,
            );
        }

        // Apply the selected function to the pin
        if let Some((pin_num, func)) = new_function {
            if let Some(pin) = self.find_pin_mut(pin_num) {
                pin.selected_function = func;
            }
        }
    }
}
