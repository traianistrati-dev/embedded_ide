use crate::panels::mcu_module::pin_module::pin::Pin;
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
        }
    }

    pub fn draw(&self, ui: &mut egui::Ui) {
        let top_pins_number = self.top_pins.len();
        // let bottom_pins_number = self.bottom_pins.len();
        let left_pins_number = self.left_pins.len();
        // let right_pins_number = self.right_pins.len();

        let mcu_width =
            (top_pins_number as f32 * (PIN_WIDTH + PIN_SPACING)) + PIN_SPACING + PIN_SPACING;

        let mcu_height =
            (left_pins_number as f32 * (PIN_WIDTH + PIN_SPACING)) + PIN_SPACING + PIN_SPACING;

        let (response, painter) = ui.allocate_painter(
            egui::vec2(mcu_width * 2.0, mcu_height * 2.0),
            egui::Sense::hover(),
        );

        let rect = response.rect;

        // CHIP BODY
        let chip_rect =
            egui::Rect::from_center_size(rect.center(), egui::vec2(mcu_width, mcu_height));

        painter.rect_filled(chip_rect, 4.0, egui::Color32::DARK_GRAY);

        // CHIP LABEL
        painter.text(
            chip_rect.center(),
            egui::Align2::CENTER_CENTER,
            self.name.to_owned(),
            egui::FontId::proportional(28.0),
            egui::Color32::WHITE,
        );

        // RIGHT PINS
        for (i, pin) in self.right_pins.iter().enumerate() {
            let y = chip_rect.top() + PIN_SPACING + (i as f32 * (PIN_WIDTH + PIN_SPACING));
            let x = chip_rect.right();

            pin.draw_right(&painter, x, y, PIN_HEIGHT, PIN_WIDTH, Some(&ui));
        }

        // LEFT PINS
        for (i, pin) in self.left_pins.iter().enumerate() {
            let y = chip_rect.top() + PIN_SPACING + (i as f32 * (PIN_WIDTH + PIN_SPACING));
            let x = chip_rect.left() - PIN_HEIGHT;

            pin.draw_left(&painter, x, y, PIN_HEIGHT, PIN_WIDTH, Some(&ui));
        }

        // TOP PINS
        for (i, pin) in self.top_pins.iter().enumerate() {
            let x = chip_rect.left() + PIN_SPACING + (i as f32 * (PIN_WIDTH + PIN_SPACING));
            let y = chip_rect.top() - PIN_HEIGHT;

            pin.draw_top(&painter, x, y, PIN_HEIGHT, PIN_WIDTH, Some(&ui));
        }

        // BOTTOM PINS
        for (i, pin) in self.bottom_pins.iter().enumerate() {
            let x = chip_rect.left() + PIN_SPACING + (i as f32 * (PIN_WIDTH + PIN_SPACING));
            let y = chip_rect.bottom();

            pin.draw_bottom(&painter, x, y, PIN_HEIGHT, PIN_WIDTH, Some(&ui));
        }
    }
}
