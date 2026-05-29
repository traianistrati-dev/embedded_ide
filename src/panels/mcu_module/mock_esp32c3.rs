use super::mcu::Mcu;
use super::mcu_catalog::ToolchainKind;
use super::pin_module::pin::Pin;
use super::pin_module::pin_function::PinFunction;

// ── Matrix-IO helpers ─────────────────────────────────────────────────────────
//
// The ESP32-C3 routes almost every peripheral through a configurable IO Matrix,
// so the same set of functions is available on most GPIO pins.
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
//   • 6 independent channels, up to 14-bit resolution
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

/// Full set of matrix-IO peripherals available on a general-purpose GPIO.
/// Peripherals are ordered by priority / likelihood of use.
fn matrix_fns() -> Vec<PinFunction> {
    vec![
        // ── GPIO ─────────────────────────────────────────────────────────────
        PinFunction::GpioOutput,
        PinFunction::GpioInput,

        // ── LEDC (LED PWM Controller) — 6 channels assignable to any GPIO ──
        PinFunction::TimerPwm { timer: 0, channel: 0 },
        PinFunction::TimerPwm { timer: 0, channel: 1 },
        PinFunction::TimerPwm { timer: 0, channel: 2 },
        PinFunction::TimerPwm { timer: 0, channel: 3 },
        PinFunction::TimerPwm { timer: 0, channel: 4 },
        PinFunction::TimerPwm { timer: 0, channel: 5 },

        // ── UART0 — default pins are GPIO20/GPIO21 but any GPIO can be used
        PinFunction::UsartTx(0),
        PinFunction::UsartRx(0),
        // ── UART1 — always matrix-routed, no fixed pins
        PinFunction::UsartTx(1),
        PinFunction::UsartRx(1),

        // ── SPI2 (FSPI) — matrix-routed; SPI0/1 reserved for internal flash
        PinFunction::SpiSck(2),
        PinFunction::SpiMosi(2),
        PinFunction::SpiMiso(2),
        PinFunction::SpiNss(2),

        // ── I2C0 — single I2C controller, fully matrix-routed
        PinFunction::I2cScl(0),
        PinFunction::I2cSda(0),

        // ── TWAI (CAN-compatible, ISO 11898-1) — matrix-routed
        PinFunction::CanTx,
        PinFunction::CanRx,
    ]
}

/// Returns (adc_num, adc_channel, extra_peripheral_functions) for ADC1 pins.
/// GPIO0–GPIO4 only — they map to ADC1 channels 0–4.
fn adc1_fns(channel: u8) -> (u8, u8, Vec<PinFunction>) {
    (1, channel, matrix_fns())
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
    let top_pins = vec![
        // Pin 1: LNA_IN — RF antenna feed (reserved, not user-accessible)
        Pin::new_reserved(1, "LNA_IN"),

        // Pin 2: VDD3P3 — RF/analog 3.3 V supply (first pad)
        Pin::new_reserved(2, "VDD3P3"),

        // Pin 3: VDD3P3 — RF/analog 3.3 V supply (second pad)
        Pin::new_reserved(3, "VDD3P3"),

        // Pin 4: GPIO0 — ADC1 CH0  (IO MUX primary name: XTAL_32K_P)
        //   Also serves as the optional 32 kHz RTC crystal input (P side).
        {
            let (adc, ch, fns) = adc1_fns(0);
            Pin::new_with_analog(4, "GPIO0", adc, ch).with_functions(fns)
        },

        // Pin 5: GPIO1 — ADC1 CH1  (IO MUX primary name: XTAL_32K_N)
        //   Also serves as the optional 32 kHz RTC crystal input (N side).
        {
            let (adc, ch, fns) = adc1_fns(1);
            Pin::new_with_analog(5, "GPIO1", adc, ch).with_functions(fns)
        },

        // Pin 6: GPIO2 — ADC1 CH2  ⚠ strapping pin (HIGH = SPI boot)
        //   Must be HIGH (floating or pulled-up) for normal operation.
        {
            let (adc, ch, fns) = adc1_fns(2);
            Pin::new_with_analog(6, "GPIO2", adc, ch).with_functions(fns)
        },

        // Pin 7: CHIP_EN — chip enable / reset (active-HIGH)
        //   Pull HIGH via 10 kΩ for normal operation; pull LOW to reset the chip.
        //   Datasheet type: Analog (input to an internal analog comparator /
        //   brownout-detect circuit). Official pin name in Table 2-1: "CHIP_EN".
        Pin::new_reserved(7, "CHIP_EN"),

        // Pin 8: GPIO3 — ADC1 CH3
        {
            let (adc, ch, fns) = adc1_fns(3);
            Pin::new_with_analog(8, "GPIO3", adc, ch).with_functions(fns)
        },
    ];

    // ── RIGHT pins — visual top → bottom (chip pins 9 → 16) ─────────────────
    let right_pins = vec![
        // Pin 9: GPIO4 — ADC1 CH4  (IO MUX primary name: MTMS — JTAG TMS)
        //   External JTAG TMS signal is available on this pin via IO MUX F2.
        {
            let (adc, ch, fns) = adc1_fns(4);
            Pin::new_with_analog(9, "GPIO4", adc, ch).with_functions(fns)
        },

        // Pin 10: GPIO5 — ADC2 CH0  ⚠ ADC2, NOT ADC1! (IO MUX: MTDI — JTAG TDI)
        //   ADC2 is a separate 12-bit SAR unit with only this one channel.
        //   ADC2 is not factory-calibrated; accuracy may be lower than ADC1.
        Pin::new_with_analog(10, "GPIO5", 2, 0).with_functions(matrix_fns()),

        // Pin 11: VDD3P3_RTC — RTC / ultra-low-power domain 3.3 V supply
        //   Requires 1 µF + 100 nF decoupling capacitors.
        Pin::new_reserved(11, "VDD3P3_RTC"),

        // Pin 12: GPIO6 — general purpose  (IO MUX primary name: MTCK — JTAG TCK)
        Pin::new(12, "GPIO6").with_functions(matrix_fns()),

        // Pin 13: GPIO7 — general purpose  (IO MUX primary name: MTDO — JTAG TDO)
        //   TDO is output-only when used as external JTAG via IO MUX.
        Pin::new(13, "GPIO7").with_functions(matrix_fns()),

        // Pin 14: GPIO8 — general purpose  ⚠ strapping pin (ROM print / boot mode)
        //   Sampled at reset; LOW suppresses ROM bootloader output on UART0.
        //   Built-in LED on many SuperMini / DevKit boards is wired to GPIO8.
        Pin::new(14, "GPIO8").with_functions(matrix_fns()),

        // Pin 15: GPIO9 — general purpose  ⚠ strapping pin (weak pull-up)
        //   HIGH = SPI boot (normal), LOW = download mode (programming).
        Pin::new(15, "GPIO9").with_functions(matrix_fns()),

        // Pin 16: GPIO10 — general purpose
        Pin::new(16, "GPIO10").with_functions(matrix_fns()),
    ];

    // ── BOTTOM pins — visual left → right (chip pins 24 → 17) ───────────────
    //
    // Physical bottom row runs right-to-left (pins 17→24), so reversing gives
    // the left-to-right visual order used in the pin diagram: 24→17.
    let bottom_pins = vec![
        // Pin 24: GPIO17 — SPI flash SPIQ (MISO) on WROOM/MINI modules
        //   IO MUX primary name: SPIQ.
        //   ⚠ Do NOT use on WROOM/MINI — connected to internal flash.
        Pin::new(24, "GPIO17").with_functions(matrix_fns()),

        // Pin 23: GPIO16 — SPI flash SPID (MOSI) on WROOM/MINI modules
        //   IO MUX primary name: SPID.
        //   ⚠ Do NOT use on WROOM/MINI — connected to internal flash.
        Pin::new(23, "GPIO16").with_functions(matrix_fns()),

        // Pin 22: GPIO15 — SPI flash SPICLK on WROOM/MINI modules
        //   IO MUX primary name: SPICLK.
        //   ⚠ Do NOT use on WROOM/MINI — connected to internal flash.
        Pin::new(22, "GPIO15").with_functions(matrix_fns()),

        // Pin 21: GPIO14 — SPI flash SPICS0 on WROOM/MINI modules
        //   IO MUX primary name: SPICS0.
        //   ⚠ Do NOT use on WROOM/MINI — connected to internal flash.
        Pin::new(21, "GPIO14").with_functions(matrix_fns()),

        // Pin 20: GPIO13 — SPI flash SPIWP on WROOM/MINI modules
        //   IO MUX primary name: SPIWP.
        //   ⚠ Do NOT use on WROOM/MINI — connected to internal flash.
        Pin::new(20, "GPIO13").with_functions(matrix_fns()),

        // Pin 19: GPIO12 — SPI flash SPIHD on WROOM/MINI modules
        //   IO MUX primary name: SPIHD.
        //   ⚠ Do NOT use on WROOM/MINI — connected to internal flash.
        Pin::new(19, "GPIO12").with_functions(matrix_fns()),

        // Pin 18: VDD_SPI — SPI flash power supply (3.3 V or 1.8 V, set via eFuse)
        //   Tied to VDD3P3_CPU on WROOM/MINI modules (3.3 V flash).
        Pin::new_reserved(18, "VDD_SPI"),

        // Pin 17: VDD3P3_CPU — digital core 3.3 V supply
        //   Requires 10 µF + 100 nF decoupling capacitors.
        Pin::new_reserved(17, "VDD3P3_CPU"),
    ];

    // ── LEFT pins — visual top → bottom (chip pins 32 → 25) ─────────────────
    //
    // Physical left column runs bottom-to-top (pins 25→32), so reversing gives
    // the top-to-bottom visual order used in the pin diagram: 32→25.
    //
    // QFN-32 left-side pinout (datasheet Rev 2.4, Table 2-1):
    //   32      : VDDA    — analog supply 3.3 V (second pad)
    //   31      : VDDA    — analog supply 3.3 V (first pad)
    //   30      : XTAL_P  — 40 MHz main crystal P (analog, reserved)
    //   29      : XTAL_N  — 40 MHz main crystal N (analog, reserved)
    //   28      : UOTXD   — GPIO21 / UART0 TX default
    //   27      : UORXD   — GPIO20 / UART0 RX default
    //   26      : GPIO19  — USB Serial/JTAG D+
    //   25      : GPIO18  — USB Serial/JTAG D-
    //   EP(33)  : GND     — exposed thermal pad; must be soldered to PCB ground plane
    let left_pins = vec![
        // Pin 32: VDDA — analog supply 3.3 V (second pad)
        Pin::new_reserved(32, "VDDA"),

        // Pin 31: VDDA — analog supply 3.3 V (first pad)
        Pin::new_reserved(31, "VDDA"),

        // Pin 30: XTAL_P — 40 MHz main crystal oscillator P (analog, reserved)
        Pin::new_reserved(30, "XTAL_P"),

        // Pin 29: XTAL_N — 40 MHz main crystal oscillator N (analog, reserved)
        Pin::new_reserved(29, "XTAL_N"),

        // Pin 28: GPIO21 — UART0 TX (default); matrix-routed to any peripheral
        //   IO MUX primary name: UOTXD.
        Pin::new(28, "GPIO21").with_functions({
            let mut fns = vec![
                PinFunction::UsartTx(0), // default UART0 TX
                PinFunction::UsartRx(0),
            ];
            // Append remaining matrix functions (skip duplicate UART0 entries)
            fns.extend(matrix_fns().into_iter().filter(|f| {
                !matches!(f, PinFunction::UsartTx(0) | PinFunction::UsartRx(0))
            }));
            fns
        }),

        // Pin 27: GPIO20 — UART0 RX (default); matrix-routed to any peripheral
        //   IO MUX primary name: UORXD.
        Pin::new(27, "GPIO20").with_functions({
            let mut fns = vec![
                PinFunction::UsartRx(0), // default UART0 RX
                PinFunction::UsartTx(0),
            ];
            fns.extend(matrix_fns().into_iter().filter(|f| {
                !matches!(f, PinFunction::UsartTx(0) | PinFunction::UsartRx(0))
            }));
            fns
        }),

        // Pin 26: GPIO19 — USB Serial/JTAG D+
        //   Primary function: USB D+. Can be repurposed as GPIO/matrix peripheral
        //   if USB is permanently disabled via eFuse.
        //   ⚠ Avoid for general GPIO use if USB connectivity is needed.
        Pin::new(26, "GPIO19").with_functions({
            let mut fns = vec![PinFunction::UsbDp];
            fns.extend(matrix_fns());
            fns
        }),

        // Pin 25: GPIO18 — USB Serial/JTAG D-
        //   Primary function: USB D-. Can be repurposed as GPIO/matrix peripheral
        //   if USB is permanently disabled via eFuse.
        //   ⚠ Avoid for general GPIO use if USB connectivity is needed.
        Pin::new(25, "GPIO18").with_functions({
            let mut fns = vec![PinFunction::UsbDm];
            fns.extend(matrix_fns());
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
