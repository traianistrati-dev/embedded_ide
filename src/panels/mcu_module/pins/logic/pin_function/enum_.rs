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

    // ── SAI (audio) ─────────────────────────────────────────────────────────
    // One SAI carries TWO independent sub-blocks, A and B, each with its own
    // pads and its own direction — a codec is usually TX on one and RX on the
    // other. `block` is 1 for A and 2 for B.
    /// SAI{sai} sub-block {block} bit clock.
    SaiSck {
        sai: u8,
        block: u8,
    },
    /// SAI{sai} sub-block {block} serial data.
    SaiSd {
        sai: u8,
        block: u8,
    },
    /// SAI{sai} sub-block {block} frame synchronisation.
    SaiFs {
        sai: u8,
        block: u8,
    },
    /// SAI{sai} sub-block {block} master clock out — optional, for the codec.
    SaiMclk {
        sai: u8,
        block: u8,
    },

    // ── DAC ─────────────────────────────────────────────────────────────────
    /// Analog output — DAC{dac} channel {channel}.
    ///
    /// The mirror of an ADC channel: the pin is driven to a voltage the program
    /// writes, rather than read. One peripheral carries one or two of them, and
    /// they share nothing but the block.
    DacOut {
        dac: u8,
        channel: u8,
    },

    // ── I2S (audio) ─────────────────────────────────────────────────────────
    /// I2S{n} bit clock — the pin the datasheet calls CK (SCK/BCLK elsewhere).
    ///
    /// I2S is not a peripheral of its own on STM32: the SPI blocks double as
    /// I2S, so `I2S2` IS `SPI2` and the two cannot both be built.
    I2sCk(u8),
    /// I2S{n} word select — LRCK: which channel the current frame carries.
    I2sWs(u8),
    /// I2S{n} serial data.
    I2sSd(u8),
    /// I2S{n} master clock out — an oversampled clock for the codec, optional.
    I2sMck(u8),

    // ── Timers / PWM ────────────────────────────────────────────────────────
    /// PWM output — TIM{timer} CH{channel}
    TimerPwm {
        timer: u8,
        channel: u8,
    },
    /// Complementary PWM output — TIM{timer} CH{channel}N.
    ///
    /// The inverse of its `CH{channel}` sibling, with a dead time between the
    /// two so a half-bridge is never driven high on both sides at once. Only
    /// the advanced-control timers have these outputs, and embassy reaches them
    /// through `ComplementaryPwm` rather than `SimplePwm`.
    TimerPwmN {
        timer: u8,
        channel: u8,
    },
    /// Break input — TIM{timer} BKIN (`input` 1) or BKIN2 (`input` 2).
    ///
    /// A fault line coming in from the board: when it asserts, the timer cuts
    /// every output in hardware, without waiting for software. Only the
    /// advanced-control timers have it, and embassy reaches the break bits
    /// through `ComplementaryPwm`.
    TimerBreak {
        timer: u8,
        input: u8,
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
