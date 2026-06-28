use eframe::egui;

// ── Info struct ──────────────────────────────────────────────────────────────

pub struct FunctionInfo {
    pub description: String,
    pub specs: Vec<(String, String)>,
}

// ── PinFunction enum ─────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize, Hash)]
pub enum PinFunction {
    #[default]
    Unset,

    // ── GPIO ────────────────────────────────────────────────────────────────
    GpioInput,
    GpioOutput,

    // ── ADC ─────────────────────────────────────────────────────────────────
    /// Analog input — ADC{adc} channel {channel}
    AdcChannel {
        adc: u8,
        channel: u8,
    },

    // ── Timers / PWM ────────────────────────────────────────────────────────
    /// PWM output — TIM{timer} CH{channel}
    TimerPwm {
        timer: u8,
        channel: u8,
    },

    // ── USART ───────────────────────────────────────────────────────────────
    UsartTx(u8),  // USART{n} TX
    UsartRx(u8),  // USART{n} RX
    UsartCts(u8), // USART{n} CTS (hardware flow control)
    UsartRts(u8), // USART{n} RTS (hardware flow control)
    UsartCk(u8),  // USART{n} CK  (synchronous clock)

    // ── SPI ─────────────────────────────────────────────────────────────────
    SpiNss(u8),  // SPI{n} NSS  — chip select
    SpiSck(u8),  // SPI{n} SCK  — clock
    SpiMiso(u8), // SPI{n} MISO — master in / slave out
    SpiMosi(u8), // SPI{n} MOSI — master out / slave in

    // ── I2C ─────────────────────────────────────────────────────────────────
    I2cScl(u8), // I2C{n} SCL — clock
    I2cSda(u8), // I2C{n} SDA — data

    // ── USB ─────────────────────────────────────────────────────────────────
    UsbDm, // USB D−  (PA11)
    UsbDp, // USB D+  (PA12)

    // ── CAN ─────────────────────────────────────────────────────────────────
    CanRx, // CAN RX (PA11 / PB8)
    CanTx, // CAN TX (PA12 / PB9)

    // ── Debug interface ─────────────────────────────────────────────────────
    SwdIo,  // SWDIO / JTMS
    SwdClk, // SWCLK / JTCK

    // ── Clock output ────────────────────────────────────────────────────────
    Mco, // Master Clock Output (PA8)
}
