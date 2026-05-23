use eframe::egui;

pub const PIN_FONT_SIZE: f32 = 10.0;
pub const PIN_ROUNDING: f32 = 0.0;

pub struct Pin {
    pub name: String,
    pub number: usize,
    pub reserved: bool,
    // rect: egui::Rect,
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

    pub fn get_backgroung_collor(&self) -> egui::Color32 {
        match self.reserved {
            true => match self.name.as_str() {
                "VDD" | "VDDA" => egui::Color32::RED,
                "VBAT" => egui::Color32::LIGHT_RED,
                "VSS" | "VSSA" => egui::Color32::BLACK,
                _ => egui::Color32::LIGHT_GRAY,
            },
            false => egui::Color32::LIGHT_BLUE,
        }
    }
    pub fn get_text_collor(&self) -> egui::Color32 {
        match self.reserved {
            true => match self.name.as_str().trim() {
                "VSS" | "VSSA" => egui::Color32::WHITE,
                _ => egui::Color32::BLACK,
            },
            false => egui::Color32::BLACK,
        }
    }
}
