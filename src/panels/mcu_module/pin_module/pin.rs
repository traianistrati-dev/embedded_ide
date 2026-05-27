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
    /// Standard GPIO pin — Input + Output only
    pub fn new(number: usize, name: &str) -> Self {
        Self {
            number,
            name: name.to_owned(),
            reserved: false,
            available_functions: vec![PinFunction::GpioInput, PinFunction::GpioOutput],
            selected_function: PinFunction::Unset,
        }
    }

    /// GPIO pin with ADC — Input + Output + AdcChannel{adc, channel}
    pub fn new_with_analog(number: usize, name: &str, adc: u8, channel: u8) -> Self {
        Self {
            number,
            name: name.to_owned(),
            reserved: false,
            available_functions: vec![
                PinFunction::GpioInput,
                PinFunction::GpioOutput,
                PinFunction::AdcChannel { adc, channel },
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

    /// Builder — appends additional peripheral functions to the pin
    pub fn with_functions(mut self, extra: Vec<PinFunction>) -> Self {
        self.available_functions.extend(extra);
        self
    }

    pub fn get_background_color(&self) -> eframe::egui::Color32 {
        if self.reserved {
            return match self.name.as_str() {
                // STM32 power / ground names
                "VDD" | "VDDA"                    => eframe::egui::Color32::from_rgb(200, 50, 50),
                "VBAT"                             => eframe::egui::Color32::from_rgb(220, 100, 100),
                "VSS" | "VSSA"                     => eframe::egui::Color32::from_rgb(30, 30, 30),
                // ESP32-C3 power / ground names
                "VDD3P3" | "VDD3P3_CPU"
                | "VDD3P3_RTC" | "VDD_SPI"        => eframe::egui::Color32::from_rgb(200, 50, 50),
                "GND"                              => eframe::egui::Color32::from_rgb(30, 30, 30),
                // Misc reserved (NRST, BOOT0, CHIP_PU, LNA_IN, …)
                _                                  => eframe::egui::Color32::LIGHT_GRAY,
            };
        }
        self.selected_function.color()
    }

    pub fn get_text_color(&self) -> eframe::egui::Color32 {
        if self.reserved {
            return match self.name.as_str() {
                "VSS" | "VSSA" | "GND" => eframe::egui::Color32::WHITE,
                _                       => eframe::egui::Color32::BLACK,
            };
        }
        eframe::egui::Color32::BLACK
    }
}
