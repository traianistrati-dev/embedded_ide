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

    // ── HSPI ────────────────────────────────────────────────────────────────
    // The high-speed external-memory controller on the top of the U5 line. The
    // pads are instance-numbered (`HSPI1_IO3`), not routed through an IO
    // manager the way the OCTOSPI's and XSPI's are.
    /// HSPI{unit} clock.
    HspiClk {
        unit: u8,
    },
    /// HSPI{unit} chip select.
    HspiNcs {
        unit: u8,
    },
    /// HSPI{unit} data strobe {index}.
    HspiDqs {
        unit: u8,
        index: u8,
    },
    /// HSPI{unit} data line {lane}, 0..=15 in silicon — embassy drives the
    /// first eight.
    HspiIo {
        unit: u8,
        lane: u8,
    },

    // ── XSPI ────────────────────────────────────────────────────────────────
    // OCTOSPI's successor, on the H7RS and N6: up to SIXTEEN data lines, two
    // chip selects and two strobes per port. Named after the manager's port,
    // same as the OCTOSPI.
    /// XSPI port {port} clock.
    XspiClk {
        port: u8,
    },
    /// XSPI port {port} chip select {cs} — 1 or 2, either drives the device.
    XspiNcs {
        port: u8,
        cs: u8,
    },
    /// XSPI port {port} data strobe {index} — 0, or 1 for the dual-strobe mode.
    XspiDqs {
        port: u8,
        index: u8,
    },
    /// XSPI port {port} data line {lane}, 0..=15.
    XspiIo {
        port: u8,
        lane: u8,
    },

    // ── OCTOSPI ─────────────────────────────────────────────────────────────
    // The vendor names these pads after the IO MANAGER's port (`OCTOSPIM_P1_*`),
    // not after the controller, so `port` is what travels here. The manager can
    // route either port to either controller; the IDE takes the default 1:1
    // mapping, which is what CubeMX and the boards use.
    /// OCTOSPI port {port} clock.
    OspiClk {
        port: u8,
    },
    /// OCTOSPI port {port} chip select.
    OspiNcs {
        port: u8,
    },
    /// OCTOSPI port {port} data strobe — only the octal DTR modes use it.
    OspiDqs {
        port: u8,
    },
    /// OCTOSPI port {port} data line {lane}, 0..=7.
    OspiIo {
        port: u8,
        lane: u8,
    },

    // ── QUADSPI ─────────────────────────────────────────────────────────────
    // One peripheral, up to two BANKS, each with its own chip select and four
    // data lines; the clock is shared. Which banks are wired decides the
    // constructor, so the bank travels in the function.
    /// QUADSPI clock — shared by both banks.
    QspiClk,
    /// QUADSPI bank {bank} chip select.
    QspiNcs {
        bank: u8,
    },
    /// QUADSPI bank {bank} data line {lane}, 0..=3.
    QspiIo {
        bank: u8,
        lane: u8,
    },

    // ── SDMMC / SDIO ────────────────────────────────────────────────────────
    // `unit` is the instance number, and 0 means the UN-NUMBERED `SDIO` that
    // F1/F2/F4/L1 carry — the same block the later families call SDMMC1.
    /// SDMMC{unit} clock.
    SdmmcCk {
        unit: u8,
    },
    /// SDMMC{unit} command line.
    SdmmcCmd {
        unit: u8,
    },
    /// SDMMC{unit} data line {lane}. One, four or eight of them are wired, and
    /// how many decides which constructor the codegen reaches for.
    SdmmcD {
        unit: u8,
        lane: u8,
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

    // ── PARL_IO ─────────────────────────────────────────────────────────────
    /// PARL_IO data line {lane} — Espressif's parallel IO port.
    ///
    /// A whole BYTE (or nibble, or word) moved per clock instead of a bit:
    /// what talks to an LED matrix, a fast ADC or a camera-style sensor. The
    /// port is one direction at a time, and the data lines are numbered from 0.
    ParlData {
        lane: u8,
    },
    /// PARL_IO clock. An OUTPUT when the port transmits and an INPUT when it
    /// receives — the port owns the clock either way, which is why one function
    /// covers both.
    ParlClk,
    /// PARL_IO valid/enable line, optional.
    ///
    /// Marks which clocks carry real data. Without it every clock counts, which
    /// is fine for a continuous stream and wrong for a framed one.
    ParlValid,

    // ── PARL_IO, receiving half ─────────────────────────────────────────────
    // A SECOND family, and the reason is that both halves can run at once: the
    // peripheral has separate `PARL_TX_*` and `PARL_RX_*` signals in the GPIO
    // matrix, so a sent bit and a received one are never the same wire. The
    // sending half above keeps the `Parl` names.
    /// PARL_IO RX data line {lane} — one pad of the receiving bus.
    ParlRxData {
        lane: u8,
    },
    /// PARL_IO RX clock. The port owns it either way — it is driven out when
    /// receiving on an internal clock and read in when the sender supplies one.
    ParlRxClk,
    /// PARL_IO RX valid/enable line, optional. Marks which clocks carry data.
    ParlRxValid,

    // ── Touch ───────────────────────────────────────────────────────────────
    /// Capacitive touch channel {0} — a pad that senses a finger.
    ///
    /// The channel number is NOT a choice: each one is welded to one GPIO, so
    /// the pad decides which channel it is. Ten of them, on the original ESP32
    /// alone — esp-hal builds no touch driver for the S2 or S3, whose silicon
    /// has the sensors.
    TouchPad(u8),

    // ── LCD_CAM ─────────────────────────────────────────────────────────────
    /// LCD_CAM data line {lane} — one pad of the parallel video bus.
    ///
    /// ONE family for all three modes, because the peripheral has one set of
    /// pads and only one mode at a time: lane 3 is lane 3 whether it is going
    /// out to an i8080 display or coming in from a camera. Which direction it
    /// takes is decided by the module's mode, not by the pad.
    LcdCamData {
        lane: u8,
    },
    /// LCD_CAM data/command select (i8080 `DC`/`RS`).
    ///
    /// Low says the byte on the bus is a command, high says it is data. The
    /// i8080 mode alone has it.
    LcdCamDc,
    /// LCD_CAM write strobe (i8080 `WR`). The display latches on its edge.
    LcdCamWr,
    /// LCD_CAM chip select (i8080 `CS`), optional.
    ///
    /// Leave it unwired to tie CS low on the board and give the pad to
    /// something else.
    LcdCamCs,
    /// LCD_CAM pixel clock.
    ///
    /// Driven OUT in RGB mode and read IN from the camera, which is the whole
    /// difference between the two: one of them owns the clock.
    LcdCamPclk,
    /// LCD_CAM vertical sync — start of frame.
    LcdCamVsync,
    /// LCD_CAM horizontal sync — start of line.
    LcdCamHsync,
    /// LCD_CAM data enable (RGB mode).
    ///
    /// Marks the clocks inside the active area. The blanking porches have no
    /// pixels on them, and this is what says so.
    LcdCamDe,

    // ── LCD_CAM, camera half ────────────────────────────────────────────────
    // A SECOND family, and the reason is that both halves can run at once: an
    // S3 driving a display while reading a sensor needs two sets of pads, so a
    // pad has to say which half it belongs to. The LCD half above keeps the
    // `LcdCam` names; everything the camera reads is `Cam`.
    /// Camera data line {lane} — one pad of the DVP bus, coming IN.
    CamData {
        lane: u8,
    },
    /// Camera pixel clock, driven by the SENSOR. An input, unlike the RGB
    /// panel's pixel clock, which this chip drives.
    CamPclk,
    /// Camera vertical sync — start of frame, from the sensor.
    CamVsync,
    /// Camera horizontal sync — start of line, from the sensor. Optional: a
    /// sensor in Vsync/DE mode uses HREF alone.
    CamHsync,
    /// Camera h-enable, the sensor's HREF: high while a line carries pixels.
    CamHenable,
    /// Camera master clock — the clock this chip GIVES the sensor.
    ///
    /// Unwired means slave mode: the sensor is clocked from somewhere else and
    /// this chip just reads what arrives.
    CamMclk,

    // ── MCPWM ───────────────────────────────────────────────────────────────
    /// MCPWM{unit} operator {operator}, output A.
    ///
    /// Espressif's MOTOR-control PWM, and a different peripheral from the LEDC
    /// that [`PinFunction::TimerPwm`] drives on an ESP. Where the LEDC dims
    /// LEDs, this one exists to switch a bridge: an operator's A and B outputs
    /// are a complementary pair, with dead time between them, so the two sides
    /// of a half-bridge are never on at once.
    McpwmA {
        unit: u8,
        operator: u8,
    },
    /// MCPWM{unit} operator {operator}, output B — A's counterpart.
    McpwmB {
        unit: u8,
        operator: u8,
    },

    // ── PCNT ────────────────────────────────────────────────────────────────
    /// PCNT unit {n} — the pulses being counted.
    ///
    /// Espressif's pulse counter: a hardware counter that follows edges on this
    /// pin without waking the CPU, with a glitch filter and limits that fire an
    /// interrupt. What reads a flow meter, a tachometer or a rotary encoder.
    /// PCNT unit {unit}, channel {channel} — the pulse input.
    ///
    /// A unit has TWO channels, each with its own edge and control inputs and
    /// its own counting rules, and both add into the same counter. That is what
    /// a quadrature encoder needs: one channel per phase.
    PcntEdge {
        unit: u8,
        channel: u8,
    },
    /// PCNT unit {n} — the control input that MODIFIES the counting.
    ///
    /// Optional, and what turns a counter into an encoder: its level decides
    /// whether an edge counts up, counts down, or is ignored. Wire an encoder's
    /// B phase here and its A phase to [`PinFunction::PcntEdge`], and the unit
    /// follows direction on its own.
    /// PCNT unit {unit}, channel {channel} — the control input, optional.
    ///
    /// What it does at each level is the channel's own setting: leaving both at
    /// "keep" is a plain counter, and setting one to "reverse" is what turns
    /// the pair into an encoder.
    PcntCtrl {
        unit: u8,
        channel: u8,
    },

    // ── RMT ─────────────────────────────────────────────────────────────────
    /// RMT channel {n} — Espressif's Remote Control transceiver.
    ///
    /// A pulse-train engine, not a bus: one pin, and a list of
    /// (level, duration) pairs the hardware clocks out or samples in. It is
    /// what drives an IR remote, a WS2812 strip or a 1-Wire line without the
    /// CPU timing each edge.
    ///
    /// The DIRECTION belongs to the channel, not to the pin: on every part
    /// after the original ESP32 the low channels transmit and the high ones
    /// receive, fixed in silicon. The module carries which, and the UI offers
    /// only the direction that channel has — see `RmtDirection::options`.
    ///
    /// STM32 has no counterpart, which is why the variant is numbered by
    /// CHANNEL rather than by instance: there is one RMT block per chip.
    RmtChannel(u8),

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
