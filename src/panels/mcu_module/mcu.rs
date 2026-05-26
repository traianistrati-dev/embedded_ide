use crate::panels::mcu_module::pin_module::pin::Pin;
use crate::panels::mcu_module::pin_module::pin_function::PinFunction;
use eframe::egui;

const PIN_HEIGHT: f32 = 50.0;
const PIN_WIDTH: f32 = 30.0;
const PIN_SPACING: f32 = 3.0;

pub struct Mcu {
    pub name: String,
    pub top_pins: Vec<Pin>,
    pub bottom_pins: Vec<Pin>,
    pub left_pins: Vec<Pin>,
    pub right_pins: Vec<Pin>,
    /// Currently selected pin number (None = no pin selected)
    selected_pin: Option<usize>,
    /// Function whose ⓘ info window is open (None = closed)
    show_info: Option<PinFunction>,
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
            show_info: None,
        }
    }

    /// Returns `(number, name, selected_function)` for every non-reserved pin.
    /// Used by the IDE to sync the `pins/` source-file directory.
    pub fn all_pin_functions(&self) -> Vec<(usize, String, PinFunction)> {
        self.top_pins
            .iter()
            .chain(self.bottom_pins.iter())
            .chain(self.left_pins.iter())
            .chain(self.right_pins.iter())
            .filter(|p| !p.reserved)
            .map(|p| (p.number, p.name.clone(), p.selected_function.clone()))
            .collect()
    }

    /// Resets all non-reserved pins to Unset and clears selection/info state.
    pub fn reset_all_pins(&mut self) {
        for pin in self
            .top_pins
            .iter_mut()
            .chain(self.bottom_pins.iter_mut())
            .chain(self.left_pins.iter_mut())
            .chain(self.right_pins.iter_mut())
        {
            if !pin.reserved {
                pin.selected_function = PinFunction::Unset;
            }
        }
        self.selected_pin = None;
        self.show_info = None;
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

    pub fn draw(&mut self, ui: &mut egui::Ui) -> Option<(usize, String, PinFunction)> {
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
        let mut toggle_info: Option<PinFunction> = None;

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
            let info_btn_w = 22.0;
            let gap = 4.0;
            let btn_w = chip_rect.width() - 24.0 - info_btn_w - gap;
            let btn_h = 28.0;
            let btn_x = chip_rect.left() + 12.0;
            let mut btn_y = sep_y + 12.0;

            for (i, func) in funcs.iter().enumerate() {
                let btn_rect =
                    egui::Rect::from_min_size(egui::pos2(btn_x, btn_y), egui::vec2(btn_w, btn_h));
                let info_rect = egui::Rect::from_min_size(
                    egui::pos2(btn_x + btn_w + gap, btn_y),
                    egui::vec2(info_btn_w, btn_h),
                );

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

                // ⓘ button — drawn with painter primitives (avoids missing-glyph issue)
                let info_open = self.show_info.as_ref() == Some(func);
                let info_bg = if info_open {
                    egui::Color32::from_rgb(80, 120, 200)
                } else {
                    egui::Color32::from_rgb(55, 55, 75)
                };
                painter.rect_filled(info_rect, 5.0, info_bg);
                // Draw circle outline
                let ic = info_rect.center();
                let ir = 7.5_f32;
                painter.circle_stroke(ic, ir, egui::Stroke::new(1.5, egui::Color32::WHITE));
                // Draw the "i" glyph: top dot + vertical stem
                painter.circle_filled(egui::pos2(ic.x, ic.y - 2.5), 1.3, egui::Color32::WHITE);
                painter.line_segment(
                    [egui::pos2(ic.x, ic.y - 0.5), egui::pos2(ic.x, ic.y + 4.0)],
                    egui::Stroke::new(1.8, egui::Color32::WHITE),
                );

                // Hover / click — main button
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
                    // Click on the already-selected function → deselect (Unset);
                    // click on a different function → select it.
                    let next = if func == &selected_func {
                        PinFunction::Unset
                    } else {
                        func.clone()
                    };
                    new_function = Some((num, next));
                }

                // Hover / click — info button
                let info_response = ui.interact(
                    info_rect,
                    ui.id().with(("info_btn", num, i)),
                    egui::Sense::click(),
                );
                if info_response.hovered() {
                    painter.rect_stroke(
                        info_rect,
                        5.0,
                        egui::Stroke::new(1.5, egui::Color32::WHITE),
                        egui::StrokeKind::Middle,
                    );
                }
                if info_response.clicked() {
                    toggle_info = Some(func.clone());
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

        // Apply the selected function to the pin; always close the info popup.
        // Capture what changed so the caller can react (e.g. create pin source files).
        let mut pin_changed: Option<(usize, String, PinFunction)> = None;
        if let Some((pin_num, func)) = new_function {
            if let Some(pin) = self.find_pin_mut(pin_num) {
                pin_changed = Some((pin.number, pin.name.clone(), func.clone()));
                pin.selected_function = func;
            }
            self.show_info = None;
        }

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
            let info = func.info();
            let mut open = true;

            // Anchor to chip body center so the window opens within the MCU panel
            let popup_pos = egui::pos2(chip_rect.center().x - 170.0, chip_rect.center().y - 100.0);

            egui::Window::new(format!("{}", func.label()))
                .open(&mut open)
                .resizable(true)
                .default_width(340.0)
                .default_pos(popup_pos)
                .show(ui.ctx(), |ui| {
                    // Description
                    ui.label(
                        egui::RichText::new(&info.description)
                            .size(14.0)
                            .color(egui::Color32::from_rgb(20, 20, 20)),
                    );

                    if !info.specs.is_empty() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);

                        // Specs grid
                        egui::Grid::new("info_specs_grid")
                            .num_columns(2)
                            .spacing([12.0, 6.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for (key, value) in &info.specs {
                                    ui.label(
                                        egui::RichText::new(key)
                                            .size(12.0)
                                            .color(egui::Color32::from_rgb(0, 50, 250)),
                                    );
                                    ui.label(
                                        egui::RichText::new(value)
                                            .size(12.0)
                                            .color(egui::Color32::DARK_GRAY),
                                    );
                                    ui.end_row();
                                }
                            });
                    }
                });

            if !open {
                self.show_info = None;
            }
        }

        pin_changed
    }
}
