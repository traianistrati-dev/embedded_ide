use super::pin::Pin;

pub fn draw_mock_mcu_stm32f103c8tx(ui: &mut eframe::egui::Ui) {
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
        Pin::new(13, "PA3"),
        Pin::new(14, "PA4"),
        Pin::new(15, "PA5"),
        Pin::new(16, "PA6"),
        Pin::new(17, "PA7"),
        Pin::new(18, "PB0"),
        Pin::new(19, "PB1"),
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
        Pin::new(10, "PA0"),
        Pin::new(11, "PA1"),
        Pin::new(12, "PA2"),
    ];

    let right_pins = vec![
        Pin::new_reserved(36, "VDD"),
        Pin::new_reserved(35, "VSS"),
        Pin::new(34, "PC14"),
        Pin::new(33, "PC15"),
        Pin::new(32, "PD0"),
        Pin::new(31, "PD1"),
        Pin::new(30, "NRST"),
        Pin::new(29, "VSSA"),
        Pin::new(28, "VDDA"),
        Pin::new(27, "PA0"),
        Pin::new(26, "PA1"),
        Pin::new(25, "PA2"),
    ];

    let mcu = super::mcu::Mcu::new(
        "STM32F103C8Tx".to_owned(),
        top_pins,
        bottom_pins,
        left_pins,
        right_pins,
    );

    mcu.draw(ui);
}
