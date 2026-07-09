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

    /// `true` when this function belongs to a serial communication bus
    /// (USART / SPI / I2C / USB / CAN). GPIO, ADC, timer/PWM, SWD and MCO are
    /// NOT buses. Used to highlight multi-function bus pins on the chip.
    ///
    /// Keep the variant grouping in sync with [`PinFunction::color`] above.
    pub fn is_bus(&self) -> bool {
        matches!(
            self,
            PinFunction::UsartTx(_)
                | PinFunction::UsartRx(_)
                | PinFunction::UsartCts(_)
                | PinFunction::UsartRts(_)
                | PinFunction::UsartCk(_)
                | PinFunction::SpiNss(_)
                | PinFunction::SpiSck(_)
                | PinFunction::SpiMiso(_)
                | PinFunction::SpiMosi(_)
                | PinFunction::I2cScl(_)
                | PinFunction::I2cSda(_)
                | PinFunction::UsbDm
                | PinFunction::UsbDp
                | PinFunction::CanRx
                | PinFunction::CanTx
        )
    }
}

#[cfg(test)]
mod is_bus_tests {
    use super::PinFunction;

    #[test]
    fn communication_buses_are_buses() {
        for f in [
            PinFunction::UsartTx(1),
            PinFunction::UsartRx(2),
            PinFunction::UsartCts(3),
            PinFunction::UsartRts(1),
            PinFunction::UsartCk(2),
            PinFunction::SpiNss(1),
            PinFunction::SpiSck(2),
            PinFunction::SpiMiso(1),
            PinFunction::SpiMosi(2),
            PinFunction::I2cScl(1),
            PinFunction::I2cSda(2),
            PinFunction::UsbDm,
            PinFunction::UsbDp,
            PinFunction::CanRx,
            PinFunction::CanTx,
        ] {
            assert!(f.is_bus(), "{f:?} should be a bus function");
        }
    }

    #[test]
    fn gpio_analog_timer_debug_clock_are_not_buses() {
        for f in [
            PinFunction::Unset,
            PinFunction::GpioInput,
            PinFunction::GpioOutput,
            PinFunction::AdcChannel { adc: 1, channel: 0 },
            PinFunction::TimerPwm { timer: 2, channel: 1 },
            PinFunction::SwdIo,
            PinFunction::SwdClk,
            PinFunction::Mco,
        ] {
            assert!(!f.is_bus(), "{f:?} should NOT be a bus function");
        }
    }
}
