use eframe::egui;

#[derive(Clone, PartialEq, Debug, Default)]
pub enum PinFunction {
    #[default]
    Unset,
    GpioInput,
    GpioOutput,
    Analog,
}

impl PinFunction {
    /// Full label displayed in the pin detail panel
    pub fn label(&self) -> &str {
        match self {
            PinFunction::Unset => "Not configured",
            PinFunction::GpioInput => "GPIO Input",
            PinFunction::GpioOutput => "GPIO Output",
            PinFunction::Analog => "Analog (ADC)",
        }
    }

    /// Short label displayed on the pin chip body button
    pub fn short_label(&self) -> &str {
        match self {
            PinFunction::Unset => "—",
            PinFunction::GpioInput => "IN",
            PinFunction::GpioOutput => "OUT",
            PinFunction::Analog => "AD",
        }
    }

    /// Background color used to represent this function visually
    pub fn color(&self) -> egui::Color32 {
        match self {
            PinFunction::Unset => egui::Color32::LIGHT_BLUE,
            PinFunction::GpioInput => egui::Color32::from_rgb(100, 190, 100),
            PinFunction::GpioOutput => egui::Color32::from_rgb(220, 130, 80),
            PinFunction::Analog => egui::Color32::from_rgb(180, 120, 220),
        }
    }
}
