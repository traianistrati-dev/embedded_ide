use super::enum_::PinFunction;
use eframe::egui;

impl PinFunction {
    /// Background color — grouped by peripheral category
    pub fn color(&self) -> egui::Color32 {
        match self {
            PinFunction::Unset              => egui::Color32::LIGHT_BLUE,
            PinFunction::GpioInput          => egui::Color32::from_rgb(70, 160, 70),
            PinFunction::GpioOutput         => egui::Color32::from_rgb(200, 120, 50),
            PinFunction::AdcChannel{..}     => egui::Color32::from_rgb(150, 70, 200),
            PinFunction::TimerPwm{..}       => egui::Color32::from_rgb(190, 170, 30),
            PinFunction::UsartTx(_)
            | PinFunction::UsartRx(_)
            | PinFunction::UsartCts(_)
            | PinFunction::UsartRts(_)
            | PinFunction::UsartCk(_)       => egui::Color32::from_rgb(50, 110, 200),
            PinFunction::SpiNss(_)
            | PinFunction::SpiSck(_)
            | PinFunction::SpiMiso(_)
            | PinFunction::SpiMosi(_)       => egui::Color32::from_rgb(30, 170, 170),
            PinFunction::I2cScl(_)
            | PinFunction::I2cSda(_)        => egui::Color32::from_rgb(60, 180, 100),
            PinFunction::UsbDm
            | PinFunction::UsbDp            => egui::Color32::from_rgb(190, 50, 160),
            PinFunction::CanRx
            | PinFunction::CanTx            => egui::Color32::from_rgb(200, 130, 20),
            PinFunction::SwdIo
            | PinFunction::SwdClk           => egui::Color32::from_rgb(190, 50, 50),
            PinFunction::Mco                => egui::Color32::from_rgb(130, 130, 130),
        }
    }
}
