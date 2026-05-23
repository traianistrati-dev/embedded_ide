use super::pin_function::PinFunction;

pub const PIN_FONT_SIZE: f32 = 10.0;
pub const PIN_ROUNDING: f32 = 0.0;

pub struct Pin {
    pub name: String,
    pub number: usize,
    pub reserved: bool,
    pub available_functions: Vec<PinFunction>,
    pub selected_function: PinFunction,
}

impl Pin {
    /// Standard GPIO pin (Input + Output)
    pub fn new(number: usize, name: &str) -> Self {
        Self {
            number,
            name: name.to_owned(),
            reserved: false,
            available_functions: vec![PinFunction::GpioInput, PinFunction::GpioOutput],
            selected_function: PinFunction::Unset,
        }
    }

    /// GPIO pin with ADC support (Input + Output + Analog)
    pub fn new_with_analog(number: usize, name: &str) -> Self {
        Self {
            number,
            name: name.to_owned(),
            reserved: false,
            available_functions: vec![
                PinFunction::GpioInput,
                PinFunction::GpioOutput,
                PinFunction::Analog,
            ],
            selected_function: PinFunction::Unset,
        }
    }

    /// Reserved pin (VDD, VSS, NRST, etc.) — not user-configurable
    pub fn new_reserved(number: usize, name: &str) -> Self {
        Self {
            number,
            name: name.to_owned(),
            reserved: true,
            available_functions: vec![],
            selected_function: PinFunction::Unset,
        }
    }

    pub fn get_background_color(&self) -> eframe::egui::Color32 {
        if self.reserved {
            return match self.name.as_str() {
                "VDD" | "VDDA" => eframe::egui::Color32::from_rgb(200, 50, 50),
                "VBAT" => eframe::egui::Color32::from_rgb(220, 100, 100),
                "VSS" | "VSSA" => eframe::egui::Color32::from_rgb(30, 30, 30),
                _ => eframe::egui::Color32::LIGHT_GRAY,
            };
        }
        self.selected_function.color()
    }

    pub fn get_text_color(&self) -> eframe::egui::Color32 {
        if self.reserved {
            return match self.name.as_str() {
                "VSS" | "VSSA" => eframe::egui::Color32::WHITE,
                _ => eframe::egui::Color32::BLACK,
            };
        }
        text_color_for(&self.selected_function)
    }
}

fn text_color_for(_func: &PinFunction) -> eframe::egui::Color32 {
    eframe::egui::Color32::BLACK
}
