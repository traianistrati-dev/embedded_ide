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
    /// Analog mode — the digital buffer is off and the pin is left to an
    /// on-chip analog block. NOT the same as [`PinFunction::AdcChannel`]: that
    /// one names a specific ADC input, this is the pin state a datasheet calls
    /// "additional function / analog" and CubeMX calls `GPIO_Analog`. Every
    /// GPIO whose vendor `IOModes` lists `Analog` offers it.
    GpioAnalog,

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

    // ── LPUART ──────────────────────────────────────────────────────────────
    // Low-power UART — a peripheral of its own (own instance numbering), not a
    // USART, so it gets its own variants.
    LpuartTx(u8),  // LPUART{n} TX
    LpuartRx(u8),  // LPUART{n} RX
    LpuartCts(u8), // LPUART{n} CTS
    LpuartRts(u8), // LPUART{n} RTS — doubles as DE (RS485 driver enable)

    // ── SPI ─────────────────────────────────────────────────────────────────
    SpiNss(u8),  // SPI{n} NSS  — chip select
    SpiSck(u8),  // SPI{n} SCK  — clock
    SpiMiso(u8), // SPI{n} MISO — master in / slave out
    SpiMosi(u8), // SPI{n} MOSI — master out / slave in
    SpiRdy(u8),  // SPI{n} RDY  — slave-ready handshake (STM32WBA / U5 …)

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

    // ── Anything the IDE doesn't model natively ─────────────────────────────
    /// A generic alternate function, carrying the datasheet's signal name
    /// verbatim (`SAI1_SD_A`, `FMC_A0`, `DCMI_D3`, `QUADSPI_CLK`, `ETH_MDIO`,
    /// `TIM1_CH1N`, …). One variant instead of a per-peripheral explosion: the
    /// IDE has no driver codegen for these, so the exact name IS the useful
    /// information. Guarantees an import never silently drops a pin function.
    /// Token form: `af:<lowercased name>`.
    Other(String),
}
