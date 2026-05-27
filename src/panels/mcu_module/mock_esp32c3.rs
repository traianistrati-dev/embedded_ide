use super::mcu::Mcu;
use super::mcu_catalog::ToolchainKind;
use super::pin_module::pin::Pin;
use super::pin_module::pin_function::PinFunction;

// ── Matrix-IO helpers ─────────────────────────────────────────────────────────
//
// The ESP32-C3 routes almost every peripheral through a configurable IO Matrix,
// so the same set of functions is available on most GPIO pins.
//
// Hardware-fixed exceptions:
//   • GPIO18 = USB D-   (cannot be used for anything else)
//   • GPIO19 = USB D+   (cannot be used for anything else)
//   • GPIO0–GPIO5  = ADC1 channels 0–5  (ADC input, no other special hardware)
//   • GPIO20/21 = default UART0 pins (can be remapped via matrix IO)

/// Full set of matrix-IO peripherals available on a general-purpose GPIO.
fn matrix_fns() -> Vec<PinFunction> {
    vec![
        // UART0 — default pins are GPIO20/21 but any GPIO can be used
        PinFunction::UsartTx(0),
        PinFunction::UsartRx(0),
        // UART1 — always matrix-routed
        PinFunction::UsartTx(1),
        PinFunction::UsartRx(1),
        // SPI2 (FSPI) — matrix-routed
        PinFunction::SpiSck(2),
        PinFunction::SpiMosi(2),
        PinFunction::SpiMiso(2),
        PinFunction::SpiNss(2),
        // I2C0 — matrix-routed
        PinFunction::I2cScl(0),
        PinFunction::I2cSda(0),
        // TWAI (CAN-compatible) — matrix-routed
        PinFunction::CanTx,
        PinFunction::CanRx,
    ]
}

/// Same as `matrix_fns()` but also includes ADC1 channel (hardware-fixed).
/// Used for GPIO0–GPIO5 which are the only ADC-capable pins on the ESP32-C3.
fn adc_matrix_fns(channel: u8) -> (u8, u8, Vec<PinFunction>) {
    // Returns (adc_num, adc_channel, extra_peripheral_functions)
    (1, channel, matrix_fns())
}

// ─────────────────────────────────────────────────────────────────────────────

/// Builds the ESP32-C3 MCU with all 32 QFN-32 package pins mapped.
///
/// Physical package: QFN-32, 5 × 5 mm.
/// Peripheral data sourced from the *ESP32-C3 Technical Reference Manual* Rev 0.5
/// and the *ESP32-C3 Series Datasheet* Rev 1.4.
///
/// Pin numbering follows the QFN-32 convention: pin 1 at top-left corner,
/// counting clockwise:
///   Top    pins  1 →  8  (left → right)
///   Right  pins  9 → 16  (top  → bottom)
///   Bottom pins 17 → 24  (right → left)
///   Left   pins 25 → 32  (bottom → top)
///
/// GPIO11–GPIO17 are dedicated to internal SPI flash on WROOM/MINI modules.
/// On a bare-chip design they are fully accessible.
pub fn create_esp32c3() -> Mcu {
    // ── TOP pins — visual left → right (chip pins 1 → 8) ────────────────────
    let top_pins = vec![
        // Pin 1: LNA_IN — RF antenna feed (reserved, not user-accessible)
        Pin::new_reserved(1, "LNA_IN"),

        // Pin 2: VDD3P3 — RF/analog 3.3 V supply
        Pin::new_reserved(2, "VDD3P3"),

        // Pin 3: VDD3P3 — RF/analog 3.3 V supply (second pad)
        Pin::new_reserved(3, "VDD3P3"),

        // Pin 4: GPIO0 / XTAL_32K_P — ADC1 CH0
        {
            let (adc, ch, fns) = adc_matrix_fns(0);
            Pin::new_with_analog(4, "GPIO0", adc, ch).with_functions(fns)
        },

        // Pin 5: GPIO1 / XTAL_32K_N — ADC1 CH1
        {
            let (adc, ch, fns) = adc_matrix_fns(1);
            Pin::new_with_analog(5, "GPIO1", adc, ch).with_functions(fns)
        },

        // Pin 6: GPIO2 — ADC1 CH2  ⚠ strapping pin (boot mode)
        {
            let (adc, ch, fns) = adc_matrix_fns(2);
            Pin::new_with_analog(6, "GPIO2", adc, ch).with_functions(fns)
        },

        // Pin 7: GPIO3 — ADC1 CH3
        {
            let (adc, ch, fns) = adc_matrix_fns(3);
            Pin::new_with_analog(7, "GPIO3", adc, ch).with_functions(fns)
        },

        // Pin 8: GPIO4 — ADC1 CH4
        {
            let (adc, ch, fns) = adc_matrix_fns(4);
            Pin::new_with_analog(8, "GPIO4", adc, ch).with_functions(fns)
        },
    ];

    // ── RIGHT pins — visual top → bottom (chip pins 9 → 16) ─────────────────
    let right_pins = vec![
        // Pin 9: GPIO5 — ADC1 CH5
        {
            let (adc, ch, fns) = adc_matrix_fns(5);
            Pin::new_with_analog(9, "GPIO5", adc, ch).with_functions(fns)
        },

        // Pin 10: GPIO6 — general purpose
        Pin::new(10, "GPIO6").with_functions(matrix_fns()),

        // Pin 11: GPIO7 — general purpose
        Pin::new(11, "GPIO7").with_functions(matrix_fns()),

        // Pin 12: GPIO8 — general purpose  ⚠ JTAG TMS
        Pin::new(12, "GPIO8").with_functions(matrix_fns()),

        // Pin 13: GPIO9 — general purpose  ⚠ strapping pin / JTAG TDI
        Pin::new(13, "GPIO9").with_functions(matrix_fns()),

        // Pin 14: GPIO10 — general purpose
        Pin::new(14, "GPIO10").with_functions(matrix_fns()),

        // Pin 15: GPIO11 — SPI flash SPICS1 on WROOM/MINI modules
        Pin::new(15, "GPIO11").with_functions(matrix_fns()),

        // Pin 16: VDD_SPI — SPI flash VDD supply (reserved)
        Pin::new_reserved(16, "VDD_SPI"),
    ];

    // ── BOTTOM pins — visual left → right (chip pins 24 → 17) ───────────────
    //
    // The bottom row is physically laid out right-to-left (pins 17→24 going
    // right→left), so reversing gives the left-to-right visual order: 24→17.
    let bottom_pins = vec![
        // Pin 24: GPIO19 — USB D+  (hardware-fixed, no matrix override)
        Pin::new(24, "GPIO19").with_functions(vec![PinFunction::UsbDp]),

        // Pin 23: GPIO18 — USB D-  (hardware-fixed, no matrix override)
        Pin::new(23, "GPIO18").with_functions(vec![PinFunction::UsbDm]),

        // Pin 22: GPIO17 — SPI flash SPIQ (MISO) on WROOM/MINI modules
        Pin::new(22, "GPIO17").with_functions(matrix_fns()),

        // Pin 21: GPIO16 — SPI flash SPID (MOSI) on WROOM/MINI modules
        Pin::new(21, "GPIO16").with_functions(matrix_fns()),

        // Pin 20: GPIO15 — SPI flash SPICLK on WROOM/MINI modules
        Pin::new(20, "GPIO15").with_functions(matrix_fns()),

        // Pin 19: GPIO14 — SPI flash SPICS0 on WROOM/MINI modules
        Pin::new(19, "GPIO14").with_functions(matrix_fns()),

        // Pin 18: GPIO13 — SPI flash SPIWP on WROOM/MINI modules
        Pin::new(18, "GPIO13").with_functions(matrix_fns()),

        // Pin 17: GPIO12 — SPI flash SPIHD on WROOM/MINI modules
        Pin::new(17, "GPIO12").with_functions(matrix_fns()),
    ];

    // ── LEFT pins — visual top → bottom (chip pins 32 → 25) ─────────────────
    //
    // The left column is physically bottom-to-top (pins 25→32), so reversing
    // gives the top-to-bottom visual order: 32→25.
    let left_pins = vec![
        // Pin 32: GND — digital ground
        Pin::new_reserved(32, "GND"),

        // Pin 31: VDD3P3_CPU — digital 3.3 V supply
        Pin::new_reserved(31, "VDD3P3_CPU"),

        // Pin 30: VDD3P3_RTC — RTC / low-power domain 3.3 V supply
        Pin::new_reserved(30, "VDD3P3_RTC"),

        // Pin 29: GND — digital ground
        Pin::new_reserved(29, "GND"),

        // Pin 28: GND — digital ground (second pad)
        Pin::new_reserved(28, "GND"),

        // Pin 27: CHIP_PU — chip enable / reset (active-high)
        Pin::new_reserved(27, "CHIP_PU"),

        // Pin 26: GPIO21 — UART0 TX (default); matrix-routed to any function
        Pin::new(26, "GPIO21").with_functions({
            let mut fns = vec![
                PinFunction::UsartTx(0),   // default UART0 TX
                PinFunction::UsartRx(0),
            ];
            fns.extend_from_slice(&matrix_fns()[2..]); // skip duplicate UART0
            fns
        }),

        // Pin 25: GPIO20 — UART0 RX (default); matrix-routed to any function
        Pin::new(25, "GPIO20").with_functions({
            let mut fns = vec![
                PinFunction::UsartRx(0),   // default UART0 RX
                PinFunction::UsartTx(0),
            ];
            fns.extend_from_slice(&matrix_fns()[2..]); // skip duplicate UART0
            fns
        }),
    ];

    Mcu::new(
        "ESP32-C3".to_owned(),
        ToolchainKind::EspRust,
        top_pins,
        bottom_pins,
        left_pins,
        right_pins,
    )
}
