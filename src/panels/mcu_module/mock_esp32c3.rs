use super::mcu::Mcu;
use super::mcu_catalog::ToolchainKind;
use super::pins::logic::pin::Pin;
use super::pins::logic::pin_function::PinFunction;

// ── Matrix-IO helpers ─────────────────────────────────────────────────────────
//
// The ESP32-C3 routes almost every peripheral through a configurable IO Matrix,
// so the same set of functions is available on most GPIO pins.
//
// IMPORTANT: Pin::new() / Pin::new_with_analog() already add GpioInput and
// GpioOutput to available_functions automatically.  Therefore matrix_fns() must
// NOT include them — they would appear twice in the popup otherwise.
//
// Hardware-fixed exceptions / datasheet (Rev 2.4) notes:
//   • GPIO18 = USB D-   (USB Serial/JTAG; avoid for general use on SuperMini)
//   • GPIO19 = USB D+   (USB Serial/JTAG; avoid for general use on SuperMini)
//   • GPIO20 = UART0 RX default (IO MUX name: UORXD; reassignable via matrix)
//   • GPIO21 = UART0 TX default (IO MUX name: UOTXD; reassignable via matrix)
//
// ADC (12-bit SAR):
//   • ADC1 — GPIO0 (CH0), GPIO1 (CH1), GPIO2 (CH2), GPIO3 (CH3), GPIO4 (CH4)
//   • ADC2 — GPIO5 (CH0) only  ← NOT ADC1! ADC2 is a separate unit.
//
// Strapping pins (latched at reset, free for GPIO use after boot):
//   • GPIO2  — boot mode control (pull HIGH for normal SPI boot)
//   • GPIO8  — ROM print control / boot mode
//   • GPIO9  — boot mode (weak pull-up; HIGH = SPI boot, LOW = download mode)
//
// JTAG (via IO MUX — used by external debug probes):
//   • GPIO4 = MTMS (TMS), GPIO5 = MTDI (TDI)
//   • GPIO6 = MTCK (TCK), GPIO7 = MTDO (TDO, output-only via IO MUX)
//   Built-in USB-JTAG is available on GPIO18/GPIO19 (USB D-/D+).
//
// LEDC (LED PWM Controller):
//   • 6 independent channels (CH0–CH5), up to 14-bit resolution
//   • Any GPIO can be assigned to any LEDC channel via the GPIO Matrix
//
// Crystal oscillators:
//   • Main XTAL (40 MHz): dedicated pads XTAL_P (pin 30) / XTAL_N (pin 29)
//   • Optional 32 kHz XTAL: GPIO0 (XTAL_32K_P, pin 4) / GPIO1 (XTAL_32K_N, pin 5)
//
// GPIO11 note: GPIO11 is NOT bonded out in the QFN-32 package — no physical pad.
//
// ── QFN-32 pin table (datasheet Rev 2.4, Table 2-1) ─────────────────────────
//
//   Pin  1 : LNA_IN       Analog  RF antenna feed (reserved)
//   Pin  2 : VDD3P3       Power   RF/analog 3.3 V supply
//   Pin  3 : VDD3P3       Power   RF/analog 3.3 V supply (second pad)
//   Pin  4 : XTAL_32K_P   IO      GPIO0 / optional 32 kHz XTAL P (ADC1 CH0)
//   Pin  5 : XTAL_32K_N   IO      GPIO1 / optional 32 kHz XTAL N (ADC1 CH1)
//   Pin  6 : GPIO2        IO      ADC1 CH2; ⚠ strapping pin
//   Pin  7 : CHIP_EN      Analog  Chip enable, active-HIGH (analog comparator)
//   Pin  8 : GPIO3        IO      ADC1 CH3
//   Pin  9 : MTMS         IO      GPIO4 / JTAG TMS (ADC1 CH4)
//   Pin 10 : MTDI         IO      GPIO5 / JTAG TDI (ADC2 CH0)
//   Pin 11 : VDD3P3_RTC   Power   RTC domain 3.3 V supply
//   Pin 12 : MTCK         IO      GPIO6 / JTAG TCK
//   Pin 13 : MTDO         IO      GPIO7 / JTAG TDO (output-only via IO MUX)
//   Pin 14 : GPIO8        IO      ⚠ strapping pin (ROM print / boot mode)
//   Pin 15 : GPIO9        IO      ⚠ strapping pin (HIGH = SPI boot)
//   Pin 16 : GPIO10       IO      general purpose
//   Pin 17 : VDD3P3_CPU   Power   Digital core 3.3 V supply
//   Pin 18 : VDD_SPI      Power   SPI flash power supply (3.3 V or 1.8 V)
//   Pin 19 : SPIHD        IO      GPIO12 / SPI flash HOLD (⚠ internal on WROOM/MINI)
//   Pin 20 : SPIWP        IO      GPIO13 / SPI flash WP   (⚠ internal on WROOM/MINI)
//   Pin 21 : SPICS0       IO      GPIO14 / SPI flash CS   (⚠ internal on WROOM/MINI)
//   Pin 22 : SPICLK       IO      GPIO15 / SPI flash CLK  (⚠ internal on WROOM/MINI)
//   Pin 23 : SPID         IO      GPIO16 / SPI flash MOSI (⚠ internal on WROOM/MINI)
//   Pin 24 : SPIQ         IO      GPIO17 / SPI flash MISO (⚠ internal on WROOM/MINI)
//   Pin 25 : GPIO18       IO      USB Serial/JTAG D-
//   Pin 26 : GPIO19       IO      USB Serial/JTAG D+
//   Pin 27 : UORXD        IO      GPIO20 / UART0 RX (default)
//   Pin 28 : UOTXD        IO      GPIO21 / UART0 TX (default)
//   Pin 29 : XTAL_N       Analog  40 MHz crystal N (reserved)
//   Pin 30 : XTAL_P       Analog  40 MHz crystal P (reserved)
//   Pin 31 : VDDA         Power   Analog supply 3.3 V
//   Pin 32 : VDDA         Power   Analog supply 3.3 V (second pad)
//   Pin 33 : GND          Power   Exposed thermal pad (EP); solder to GND plane

// ── Peripheral function lists ─────────────────────────────────────────────────

/// Matrix-routed peripherals available on every general-purpose GPIO.
///
/// Does NOT include GpioInput / GpioOutput — those are added automatically by
/// Pin::new() / Pin::new_with_analog() and must not appear here a second time.
fn matrix_fns() -> Vec<PinFunction> {
    vec![
        // ── LEDC (LED PWM Controller) — 6 independent channels ────────────────
        PinFunction::TimerPwm {
            timer: 0,
            channel: 0,
        },
        PinFunction::TimerPwm {
            timer: 0,
            channel: 1,
        },
        PinFunction::TimerPwm {
            timer: 0,
            channel: 2,
        },
        PinFunction::TimerPwm {
            timer: 0,
            channel: 3,
        },
        PinFunction::TimerPwm {
            timer: 0,
            channel: 4,
        },
        PinFunction::TimerPwm {
            timer: 0,
            channel: 5,
        },
        // ── UART0 — default pins GPIO20/GPIO21, but re-routable ──────────────
        PinFunction::UsartTx(0),
        PinFunction::UsartRx(0),
        // ── UART1 — always matrix-routed, no fixed pins ───────────────────────
        PinFunction::UsartTx(1),
        PinFunction::UsartRx(1),
        // ── SPI2 (GPSPI2 / FSPI) — the only user SPI; SPI0/1 are flash-only ──
        PinFunction::SpiSck(2),
        PinFunction::SpiMosi(2),
        PinFunction::SpiMiso(2),
        PinFunction::SpiNss(2),
        // ── I2C0 — single I2C controller, fully matrix-routed ─────────────────
        PinFunction::I2cScl(0),
        PinFunction::I2cSda(0),
        // ── TWAI (CAN-compatible, ISO 11898-1) — matrix-routed ────────────────
        PinFunction::RmtChannel(0),
        PinFunction::RmtChannel(1),
        PinFunction::RmtChannel(2),
        PinFunction::RmtChannel(3),
        PinFunction::I2sCk(0),
        PinFunction::I2sWs(0),
        PinFunction::I2sSd(0),
        PinFunction::I2sMck(0),
        PinFunction::CanTx,
        PinFunction::CanRx,
    ]
}

/// Matrix functions for UART0-default pins (GPIO20 = RX, GPIO21 = TX).
/// UART0 RX/TX are promoted to the top; duplicates filtered from the rest.
fn uart0_rx_pin_fns() -> Vec<PinFunction> {
    let mut fns = vec![
        PinFunction::UsartRx(0), // primary: UART0 RX via IO MUX
        PinFunction::UsartTx(0), // also available via matrix
    ];
    fns.extend(
        matrix_fns()
            .into_iter()
            .filter(|f| !matches!(f, PinFunction::UsartTx(0) | PinFunction::UsartRx(0))),
    );
    fns
}

fn uart0_tx_pin_fns() -> Vec<PinFunction> {
    let mut fns = vec![
        PinFunction::UsartTx(0), // primary: UART0 TX via IO MUX
        PinFunction::UsartRx(0), // also available via matrix
    ];
    fns.extend(
        matrix_fns()
            .into_iter()
            .filter(|f| !matches!(f, PinFunction::UsartTx(0) | PinFunction::UsartRx(0))),
    );
    fns
}

/// Matrix functions for USB pins (GPIO18/GPIO19).
/// USB D-/D+ promoted to top; all matrix peripherals still listed as alternatives
/// (usable only if USB is permanently disabled via eFuse).
fn usb_dm_pin_fns() -> Vec<PinFunction> {
    let mut fns = vec![PinFunction::UsbDm];
    fns.extend(matrix_fns());
    fns
}

fn usb_dp_pin_fns() -> Vec<PinFunction> {
    let mut fns = vec![PinFunction::UsbDp];
    fns.extend(matrix_fns());
    fns
}

// ─────────────────────────────────────────────────────────────────────────────

/// Builds the ESP32-C3 MCU with all QFN-32 package pins mapped.
///
/// Physical package: QFN-32, 5 × 5 mm.
/// Source: *ESP32-C3 Series Datasheet* Rev 2.4 (Espressif), Table 2-1.
///
/// QFN-32 pin numbering — clockwise starting at the top-left corner:
///   Top    pins  1 →  8  (left → right)
///   Right  pins  9 → 16  (top  → bottom)
///   Bottom pins 17 → 24  (right → left, physical; visual order: 24 → 17)
///   Left   pins 25 → 32  (bottom → top, physical; visual order: 32 → 25)
///   EP (pin 33)     GND  — exposed thermal pad (centre); must be soldered to GND
///
/// ADC summary (datasheet §4.1.2):
///   ADC1: GPIO0 CH0, GPIO1 CH1, GPIO2 CH2, GPIO3 CH3, GPIO4 CH4  (5 channels)
///   ADC2: GPIO5 CH0  (1 channel — separate ADC unit, NOT ADC1!)
///
/// GPIO11 is NOT bonded out in the QFN-32 package.
/// GPIO12–GPIO17 are connected to the internal SPI flash on WROOM/MINI modules.
pub fn create_esp32c3() -> Mcu {
    // ── TOP pins — visual left → right (chip pins 1 → 8) ────────────────────
    let left_pins = vec![
        // Pin 1: LNA_IN — RF antenna feed (reserved, not user-accessible)
        Pin::new_reserved(1, "LNA_IN"),
        // Pin 2: VDD3P3 — RF/analog 3.3 V supply (first pad)
        Pin::new_reserved(2, "VDD3P3"),
        // Pin 3: VDD3P3 — RF/analog 3.3 V supply (second pad)
        Pin::new_reserved(3, "VDD3P3"),
        // Pin 4: GPIO0 — ADC1 CH0  (IO MUX: XTAL_32K_P)
        // Constructor adds: GpioInput, GpioOutput, AdcChannel{1,0}
        // with_functions adds: LEDC, UART0/1, SPI2, I2C0, TWAI (no duplicates)
        Pin::new_with_analog(4, "GPIO0", 1, 0).with_functions(matrix_fns()),
        // Pin 5: GPIO1 — ADC1 CH1  (IO MUX: XTAL_32K_N)
        Pin::new_with_analog(5, "GPIO1", 1, 1).with_functions(matrix_fns()),
        // Pin 6: GPIO2 — ADC1 CH2  ⚠ strapping pin (HIGH = SPI boot)
        Pin::new_with_analog(6, "GPIO2", 1, 2).with_functions(matrix_fns()),
        // Pin 7: CHIP_EN — chip enable / reset (active-HIGH analog input)
        Pin::new_reserved(7, "CHIP_EN"),
        // Pin 8: GPIO3 — ADC1 CH3
        Pin::new_with_analog(8, "GPIO3", 1, 3).with_functions(matrix_fns()),
    ];

    // ── RIGHT pins — visual top → bottom (chip pins 9 → 16) ─────────────────
    let bottom_pins = vec![
        // Pin 9: GPIO4 — ADC1 CH4  (IO MUX: MTMS — JTAG TMS)
        Pin::new_with_analog(9, "GPIO4", 1, 4).with_functions(matrix_fns()),
        // Pin 10: GPIO5 — ADC2 CH0  ⚠ ADC2, NOT ADC1!  (IO MUX: MTDI — JTAG TDI)
        // ADC2 is a separate unit (not factory-calibrated).
        Pin::new_with_analog(10, "GPIO5", 2, 0).with_functions(matrix_fns()),
        // Pin 11: VDD3P3_RTC — RTC domain 3.3 V supply
        Pin::new_reserved(11, "VDD3P3_RTC"),
        // Pin 12: GPIO6 — general purpose  (IO MUX: MTCK — JTAG TCK)
        Pin::new(12, "GPIO6").with_functions(matrix_fns()),
        // Pin 13: GPIO7 — general purpose  (IO MUX: MTDO — JTAG TDO, output-only)
        Pin::new(13, "GPIO7").with_functions(matrix_fns()),
        // Pin 14: GPIO8 — general purpose  ⚠ strapping pin (ROM print / boot mode)
        Pin::new(14, "GPIO8").with_functions(matrix_fns()),
        // Pin 15: GPIO9 — general purpose  ⚠ strapping pin (HIGH = SPI boot)
        Pin::new(15, "GPIO9").with_functions(matrix_fns()),
        // Pin 16: GPIO10 — general purpose
        Pin::new(16, "GPIO10").with_functions(matrix_fns()),
    ];

    // ── BOTTOM pins — visual left → right (chip pins 24 → 17) ───────────────
    //
    // Physical bottom row runs right-to-left (pins 17→24), so reversing gives
    // the left-to-right visual order used in the pin diagram: 24→17.
    let right_pins = vec![
        // Pin 24: GPIO17 — IO MUX: SPIQ (SPI flash MISO on WROOM/MINI)
        // ⚠ Do NOT use on WROOM/MINI — connected to internal flash.
        Pin::new(24, "GPIO17").with_functions(matrix_fns()),
        // Pin 23: GPIO16 — IO MUX: SPID (SPI flash MOSI on WROOM/MINI)
        Pin::new(23, "GPIO16").with_functions(matrix_fns()),
        // Pin 22: GPIO15 — IO MUX: SPICLK (SPI flash CLK on WROOM/MINI)
        Pin::new(22, "GPIO15").with_functions(matrix_fns()),
        // Pin 21: GPIO14 — IO MUX: SPICS0 (SPI flash CS on WROOM/MINI)
        Pin::new(21, "GPIO14").with_functions(matrix_fns()),
        // Pin 20: GPIO13 — IO MUX: SPIWP (SPI flash WP on WROOM/MINI)
        Pin::new(20, "GPIO13").with_functions(matrix_fns()),
        // Pin 19: GPIO12 — IO MUX: SPIHD (SPI flash HOLD on WROOM/MINI)
        Pin::new(19, "GPIO12").with_functions(matrix_fns()),
        // Pin 18: VDD_SPI — SPI flash power supply (3.3 V or 1.8 V, set by eFuse)
        Pin::new_reserved(18, "VDD_SPI"),
        // Pin 17: VDD3P3_CPU — digital core 3.3 V supply
        Pin::new_reserved(17, "VDD3P3_CPU"),
    ];

    // ── LEFT pins — visual top → bottom (chip pins 32 → 25) ─────────────────
    //
    // Physical left column runs bottom-to-top (pins 25→32), so reversing gives
    // the top-to-bottom visual order used in the pin diagram: 32→25.
    let top_pins = vec![
        // Pin 32: VDDA — analog supply 3.3 V (second pad)
        Pin::new_reserved(32, "VDDA"),
        // Pin 31: VDDA — analog supply 3.3 V (first pad)
        Pin::new_reserved(31, "VDDA"),
        // Pin 30: XTAL_P — 40 MHz main crystal oscillator P (analog, reserved)
        Pin::new_reserved(30, "XTAL_P"),
        // Pin 29: XTAL_N — 40 MHz main crystal oscillator N (analog, reserved)
        Pin::new_reserved(29, "XTAL_N"),
        // Pin 28: GPIO21 — IO MUX: UOTXD (UART0 TX default)
        // Constructor adds: GpioInput, GpioOutput
        // uart0_tx_pin_fns() adds: UsartTx(0) first, then all other matrix fns
        Pin::new(28, "GPIO21").with_functions(uart0_tx_pin_fns()),
        // Pin 27: GPIO20 — IO MUX: UORXD (UART0 RX default)
        Pin::new(27, "GPIO20").with_functions(uart0_rx_pin_fns()),
        // Pin 26: GPIO19 — IO MUX: USB D+
        // ⚠ Avoid for general GPIO use if USB connectivity is needed.
        Pin::new(26, "GPIO19").with_functions(usb_dp_pin_fns()),
        // Pin 25: GPIO18 — IO MUX: USB D-
        // ⚠ Avoid for general GPIO use if USB connectivity is needed.
        Pin::new(25, "GPIO18").with_functions(usb_dm_pin_fns()),
    ];

    Mcu::new(
        "ESP32-C3".to_owned(),
        "esp32c3".to_owned(),
        ToolchainKind::EspRust,
        top_pins,
        bottom_pins,
        left_pins,
        right_pins,
    )
}
