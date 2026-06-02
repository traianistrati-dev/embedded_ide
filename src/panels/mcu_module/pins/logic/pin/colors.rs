//! Pin color logic — background and text colors based on reserved status and function.

use super::model::Pin;
use eframe::egui;

impl Pin {
    /// Determine background color for this pin.
    ///
    /// Reserved pins (power, ground, reset) have specific colors.
    /// User-configurable pins inherit color from their selected function.
    pub fn get_background_color(&self) -> egui::Color32 {
        if self.reserved {
            return match self.name.as_str() {
                // STM32 power / ground names
                "VDD" | "VDDA" => egui::Color32::from_rgb(200, 50, 50),
                "VBAT" => egui::Color32::from_rgb(220, 100, 100),
                "VSS" | "VSSA" => egui::Color32::from_rgb(30, 30, 30),
                // ESP32-C3 power / ground names
                "VDD3P3" | "VDD3P3_CPU"
                | "VDD3P3_RTC" | "VDD_SPI" => egui::Color32::from_rgb(200, 50, 50),
                "GND" => egui::Color32::from_rgb(30, 30, 30),
                // Misc reserved (NRST, BOOT0, CHIP_PU, LNA_IN, …)
                _ => egui::Color32::LIGHT_GRAY,
            };
        }
        self.selected_function.color()
    }

    /// Determine text color for this pin.
    ///
    /// Reserved pins typically use black or white text based on background brightness.
    /// User-configurable pins use black text.
    pub fn get_text_color(&self) -> egui::Color32 {
        if self.reserved {
            return match self.name.as_str() {
                "VSS" | "VSSA" | "GND" => egui::Color32::WHITE,
                _ => egui::Color32::BLACK,
            };
        }
        egui::Color32::BLACK
    }
}
