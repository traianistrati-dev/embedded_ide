/// Registry of all MCU chips available in the IDE.
/// `is_supported()` indicates whether the chip's pinout is implemented in the current build.
#[derive(PartialEq, Clone, Debug)]
pub enum McuType {
    Stm32f103c8t6,
    Stm8s103f3p6,
    Esp32c3,
}

impl McuType {
    /// Full list of chips shown in the selector dropdown
    pub fn all() -> Vec<McuType> {
        vec![
            McuType::Stm32f103c8t6,
            McuType::Stm8s103f3p6,
            McuType::Esp32c3,
        ]
    }

    /// Display label shown in the UI
    pub fn label(&self) -> &str {
        match self {
            McuType::Stm32f103c8t6 => "STM32F103C8T6",
            McuType::Stm8s103f3p6 => "STM8S103F3P6",
            McuType::Esp32c3 => "ESP32-C3",
        }
    }

    /// Returns true if this chip has a fully implemented pinout
    pub fn is_supported(&self) -> bool {
        matches!(self, McuType::Stm32f103c8t6)
    }

    /// CPU architecture family shown next to the dropdown
    pub fn family(&self) -> &str {
        match self {
            McuType::Stm32f103c8t6 => "ARM Cortex-M3",
            McuType::Stm8s103f3p6 => "STM8 8-bit",
            McuType::Esp32c3 => "RISC-V 32-bit",
        }
    }
}
