use super::enum_::PinFunction;
use eframe::egui;

impl PinFunction {
    /// Background color — grouped by peripheral category
    pub fn color(&self) -> egui::Color32 {
        match self {
            PinFunction::Unset => egui::Color32::LIGHT_BLUE,
            PinFunction::GpioInput => egui::Color32::from_rgb(70, 160, 70),
            PinFunction::GpioOutput => egui::Color32::from_rgb(200, 120, 50),
            // Analog mode shares the ADC purple: what it does is hand the pin
            // to an analog block, it just doesn't name which one.
            PinFunction::GpioAnalog => egui::Color32::from_rgb(150, 70, 200),
            PinFunction::AdcChannel { .. } => egui::Color32::from_rgb(150, 70, 200),
            // The mirror of the ADC's purple: analog, but driven, not read.
            PinFunction::DacOut { .. } => egui::Color32::from_rgb(180, 90, 160),
            PinFunction::TimerPwm { .. } | PinFunction::TimerPwmN { .. } => {
                egui::Color32::from_rgb(190, 170, 30)
            }
            // Audio: near SPI's teal, since I2S rides the SPI block, but shifted
            // enough that the two read apart on the chip.
            PinFunction::I2sCk(_)
            | PinFunction::I2sWs(_)
            | PinFunction::I2sSd(_)
            | PinFunction::I2sMck(_) => egui::Color32::from_rgb(90, 140, 200),
            // Audio too, but a deeper blue: SAI is the bigger, multi-block one.
            PinFunction::SaiSck { .. }
            | PinFunction::SaiSd { .. }
            | PinFunction::SaiFs { .. }
            | PinFunction::SaiMclk { .. } => egui::Color32::from_rgb(60, 110, 175),
            // Storage: a warm grey-green, apart from every audio blue.
            PinFunction::SdmmcCk { .. }
            | PinFunction::SdmmcCmd { .. }
            | PinFunction::SdmmcD { .. } => egui::Color32::from_rgb(120, 155, 110),
            // Storage too, but external memory rather than a card socket.
            PinFunction::QspiClk
            | PinFunction::QspiNcs { .. }
            | PinFunction::QspiIo { .. } => egui::Color32::from_rgb(150, 175, 95),
            // The same family of job as QUADSPI, a shade further along.
            PinFunction::OspiClk { .. }
            | PinFunction::OspiNcs { .. }
            | PinFunction::OspiDqs { .. }
            | PinFunction::OspiIo { .. } => egui::Color32::from_rgb(175, 190, 80),
            // A fault input, not an output — read at a glance as the one pin on
            // the timer that stops everything.
            PinFunction::TimerBreak { .. } => egui::Color32::from_rgb(200, 80, 60),
            PinFunction::UsartTx(_)
            | PinFunction::UsartRx(_)
            | PinFunction::UsartCts(_)
            | PinFunction::UsartRts(_)
            | PinFunction::UsartCk(_) => egui::Color32::from_rgb(50, 110, 200),
            // LPUART — same family as USART, slightly lighter blue.
            PinFunction::LpuartTx(_)
            | PinFunction::LpuartRx(_)
            | PinFunction::LpuartCts(_)
            | PinFunction::LpuartRts(_) => egui::Color32::from_rgb(80, 140, 220),
            PinFunction::SpiNss(_)
            | PinFunction::SpiSck(_)
            | PinFunction::SpiMiso(_)
            | PinFunction::SpiMosi(_)
            | PinFunction::SpiRdy(_) => egui::Color32::from_rgb(30, 170, 170),
            PinFunction::I2cScl(_) | PinFunction::I2cSda(_) => {
                egui::Color32::from_rgb(60, 180, 100)
            }
            PinFunction::UsbDm | PinFunction::UsbDp => egui::Color32::from_rgb(190, 50, 160),
            PinFunction::CanRx | PinFunction::CanTx => egui::Color32::from_rgb(200, 130, 20),
            PinFunction::SwdIo | PinFunction::SwdClk => egui::Color32::from_rgb(190, 50, 50),
            PinFunction::Mco => egui::Color32::from_rgb(130, 130, 130),
            // Generic alternate functions (SAI / FMC / DCMI / …) — muted slate
            // so they read as "carried, but not natively modelled".
            PinFunction::Other(_) => egui::Color32::from_rgb(105, 115, 135),
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
                | PinFunction::LpuartTx(_)
                | PinFunction::LpuartRx(_)
                | PinFunction::LpuartCts(_)
                | PinFunction::LpuartRts(_)
                | PinFunction::SpiNss(_)
                | PinFunction::SpiSck(_)
                | PinFunction::SpiMiso(_)
                | PinFunction::SpiMosi(_)
                | PinFunction::SpiRdy(_)
                | PinFunction::I2cScl(_)
                | PinFunction::I2cSda(_)
                | PinFunction::I2sCk(_)
                | PinFunction::I2sWs(_)
                | PinFunction::I2sSd(_)
                | PinFunction::I2sMck(_)
                | PinFunction::SaiSck { .. }
                | PinFunction::SaiSd { .. }
                | PinFunction::SaiFs { .. }
                | PinFunction::SaiMclk { .. }
                | PinFunction::SdmmcCk { .. }
                | PinFunction::SdmmcCmd { .. }
                | PinFunction::SdmmcD { .. }
                | PinFunction::QspiClk
                | PinFunction::QspiNcs { .. }
                | PinFunction::QspiIo { .. }
                | PinFunction::OspiClk { .. }
                | PinFunction::OspiNcs { .. }
                | PinFunction::OspiDqs { .. }
                | PinFunction::OspiIo { .. }
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
            PinFunction::TimerPwm {
                timer: 2,
                channel: 1,
            },
            PinFunction::SwdIo,
            PinFunction::SwdClk,
            PinFunction::Mco,
        ] {
            assert!(!f.is_bus(), "{f:?} should NOT be a bus function");
        }
    }
}
