use super::mcu::Mcu;
use super::pin_module::pin::Pin;

/// Builds the STM32F103C8Tx MCU with all 48 pins correctly mapped.
/// PA0–PA7, PB0, PB1 have ADC support (new_with_analog).
pub fn create_stm32f103c8tx() -> Mcu {
    let top_pins = vec![
        Pin::new_reserved(48, "VDD"),
        Pin::new_reserved(47, "VSS"),
        Pin::new(46, "PB9"),
        Pin::new(45, "PB8"),
        Pin::new_reserved(44, "BOOT0"),
        Pin::new(43, "PB7"),
        Pin::new(42, "PB6"),
        Pin::new(41, "PB5"),
        Pin::new(40, "PB4"),
        Pin::new(39, "PB3"),
        Pin::new(38, "PA15"),
        Pin::new(37, "PA14"),
    ];

    let bottom_pins = vec![
        Pin::new_with_analog(13, "PA3"),
        Pin::new_with_analog(14, "PA4"),
        Pin::new_with_analog(15, "PA5"),
        Pin::new_with_analog(16, "PA6"),
        Pin::new_with_analog(17, "PA7"),
        Pin::new_with_analog(18, "PB0"),
        Pin::new_with_analog(19, "PB1"),
        Pin::new(20, "PB2"),
        Pin::new(21, "PB10"),
        Pin::new(22, "PB11"),
        Pin::new_reserved(23, "VSS"),
        Pin::new_reserved(24, "VDD"),
    ];

    let left_pins = vec![
        Pin::new_reserved(1, "VBAT"),
        Pin::new(2, "PC13"),
        Pin::new(3, "PC14"),
        Pin::new(4, "PC15"),
        Pin::new(5, "PD0"),
        Pin::new(6, "PD1"),
        Pin::new_reserved(7, "NRST"),
        Pin::new_reserved(8, "VSSA"),
        Pin::new_reserved(9, "VDDA"),
        Pin::new_with_analog(10, "PA0"),
        Pin::new_with_analog(11, "PA1"),
        Pin::new_with_analog(12, "PA2"),
    ];

    let right_pins = vec![
        Pin::new_reserved(36, "VDD"),
        Pin::new_reserved(35, "VSS"),
        Pin::new(34, "PA13"),
        Pin::new(33, "PA12"),
        Pin::new(32, "PA11"),
        Pin::new(31, "PA10"),
        Pin::new(30, "PA9"),
        Pin::new(29, "PA8"),
        Pin::new(28, "PB15"),
        Pin::new(27, "PB14"),
        Pin::new(26, "PB13"),
        Pin::new(25, "PB12"),
    ];

    Mcu::new(
        "STM32F103C8Tx".to_owned(),
        top_pins,
        bottom_pins,
        left_pins,
        right_pins,
    )
}
