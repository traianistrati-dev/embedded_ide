use eframe::egui::epaint::TextShape;
use eframe::egui::{Color32, FontId};

use eframe::egui::Shape;

//use eframe::egui;

const PIN_FONT_SIZE: f32 = 10.0;

pub struct Pin {
    name: String,
    number: usize,
    reserved: bool,
}

pub fn draw_horizontal_text(painter: &eframe::egui::Painter, pos: eframe::egui::Pos2, pin: &Pin) {
    painter.text(
        pos,
        eframe::egui::Align2::LEFT_CENTER,
        pin.name.as_str(),
        eframe::egui::FontId::monospace(PIN_FONT_SIZE),
        pin.get_pin_text_collor(),
    );
}

pub fn draw_vertical_text(painter: &eframe::egui::Painter, pos: eframe::egui::Pos2, pin: &Pin) {
    let galley = painter.layout_no_wrap(
        pin.name.to_owned(),
        FontId::monospace(PIN_FONT_SIZE),
        Color32::BLACK,
    );

    let text_shape = TextShape {
        pos,
        galley,
        underline: eframe::egui::epaint::Stroke::NONE, //Default::default(),
        override_text_color: Some(pin.get_pin_text_collor()),
        angle: -std::f32::consts::FRAC_PI_2,
        fallback_color: Color32::BLACK,
        opacity_factor: 1.0,
    };

    let shape = Shape::Text(text_shape);
    painter.add(shape);
}

impl Pin {
    pub fn new(number: usize, name: &str) -> Self {
        Self {
            number,
            name: name.to_owned(),
            reserved: false,
        }
    }

    pub fn new_reserved(number: usize, name: &str) -> Self {
        Self {
            number,
            name: name.to_owned(),
            reserved: true,
        }
    }

    pub fn get_pin_collor(&self) -> eframe::egui::Color32 {
        match self.reserved {
            true => match self.name.as_str() {
                "VDD" | "VDDA" => eframe::egui::Color32::RED,
                "VBAT" => eframe::egui::Color32::LIGHT_RED,
                "VSS" | "VSSA" => eframe::egui::Color32::BLACK,
                _ => eframe::egui::Color32::LIGHT_GRAY,
            },
            false => eframe::egui::Color32::LIGHT_BLUE,
        }
    }
    pub fn get_pin_text_collor(&self) -> eframe::egui::Color32 {
        match self.reserved {
            true => match self.name.as_str().trim() {
                "VSS" | "VSSA" => eframe::egui::Color32::WHITE,
                _ => eframe::egui::Color32::BLACK,
            },
            false => eframe::egui::Color32::BLACK,
        }
    }
}
