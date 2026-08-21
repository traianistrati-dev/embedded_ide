use super::enum_::{FunctionInfo, PinFunction};

impl PinFunction {
    /// Detailed info shown in the ⓘ popup window
    pub fn info(&self) -> FunctionInfo {
        match self {
            PinFunction::Unset => FunctionInfo {
                description: "Pin is not configured. Select a function to enable it.".into(),
                specs: vec![],
            },

            PinFunction::GpioAnalog => FunctionInfo {
                description: "Analog mode. The digital input buffer and the pull resistors are disabled and the pad is left to an on-chip analog block (ADC, DAC, comparator, op-amp). Use it when the datasheet lists an analog additional function on this pin but the IDE has no dedicated entry for it — an ADC input that IS listed is better selected as such, since that one also names the channel."
                    .into(),
                specs: vec![
                    (
                        "Consumption".into(),
                        "Lowest-leakage pin state — the recommended setting for unused pins".into(),
                    ),
                    (
                        "Digital read".into(),
                        "None: the input buffer is off, so the pin always reads 0".into(),
                    ),
                ],
            },

            PinFunction::GpioInput => FunctionInfo {
                description: "Digital input. Reads the logic level on the pin (HIGH / LOW).".into(),
                specs: vec![
                    (
                        "Input voltage".into(),
                        "0 V – 3.3 V (5 V tolerant on most pins)".into(),
                    ),
                    (
                        "Input modes".into(),
                        "Floating, Pull-up (40 kΩ), Pull-down (40 kΩ)".into(),
                    ),
                    (
                        "Interrupt".into(),
                        "EXTI — rising, falling, or both edges".into(),
                    ),
                    ("Max frequency".into(), "Up to 72 MHz input signal".into()),
                ],
            },

            PinFunction::GpioOutput => FunctionInfo {
                description: "Digital output. Drives the pin HIGH (3.3 V) or LOW (0 V).".into(),
                specs: vec![
                    ("Output modes".into(), "Push-pull or Open-drain".into()),
                    (
                        "Max current per pin".into(),
                        "±25 mA (sink or source)".into(),
                    ),
                    (
                        "Total GPIO current".into(),
                        "Max 80 mA sourced, 80 mA sunk".into(),
                    ),
                    (
                        "Toggle speed".into(),
                        "2 MHz / 10 MHz / 50 MHz (configurable)".into(),
                    ),
                    (
                        "Output voltage".into(),
                        "0 V (LOW) or VDD 3.3 V (HIGH)".into(),
                    ),
                ],
            },

            PinFunction::AdcChannel { adc, channel } => FunctionInfo {
                description: format!(
                    "Analog-to-Digital Converter input. ADC{adc} channel IN{channel}. \
                     Converts an analog voltage (0–VDDA) to a 12-bit digital value."
                ),
                specs: vec![
                    ("Resolution".into(), "12-bit  ->  0 - 4095 counts".into()),
                    ("Reference voltage".into(), "VDDA (typ. 3.3 V)".into()),
                    (
                        "External channels".into(),
                        "10 channels  (IN0 – IN9)".into(),
                    ),
                    ("Conversion speed".into(), "Up to 1 MSPS (1 MHz)".into()),
                    ("Sample time".into(), "1.5 – 239.5 ADC clock cycles".into()),
                    ("ADC clock".into(), "Up to 14 MHz (APB2 / prescaler)".into()),
                    ("Modes".into(), "Single, continuous, scan, injected".into()),
                    (
                        format!("This pin").into(),
                        format!("ADC{adc}  IN{channel}").into(),
                    ),
                ],
            },

            PinFunction::TimerPwm { timer, channel } => FunctionInfo {
                description: format!(
                    "PWM output on TIM{timer} channel CH{channel}. \
                     Generates a square wave with configurable frequency and duty cycle."
                ),
                specs: vec![
                    ("Timer width".into(), "16-bit counter (0 – 65535)".into()),
                    ("Timer clock".into(), "Up to 72 MHz (APB1×2 or APB2)".into()),
                    ("Frequency range".into(), "~1.1 Hz – 72 MHz".into()),
                    ("Resolution".into(), "16-bit duty cycle (0 – 65535)".into()),
                    (
                        format!("Timer TIM{timer}").into(),
                        if *timer == 1 {
                            "Advanced — dead-time, break, complementary outputs".into()
                        } else {
                            "General purpose — 4 channels, input capture, output compare".into()
                        },
                    ),
                    (
                        format!("Channel CH{channel}").into(),
                        "Output compare / PWM mode 1 or 2".into(),
                    ),
                ],
            },

            PinFunction::TimerPwmN { timer, channel } => FunctionInfo {
                description: format!(
                    "Complementary PWM output on TIM{timer} channel CH{channel}N. Carries the                      inverse of CH{channel}, with a dead time between the two edges so the two                      sides of a half-bridge are never driven on at once."
                ),
                specs: vec![
                    (
                        "Pairs with".into(),
                        format!("TIM{timer} CH{channel} — same duty, opposite level"),
                    ),
                    (
                        "Dead time".into(),
                        "Inserted by the timer, one setting for the whole timer".into(),
                    ),
                    (
                        "Available on".into(),
                        "Advanced-control timers only (TIM1 / TIM8 / TIM20)".into(),
                    ),
                    (
                        "Typical use".into(),
                        "Motor drive, half/full bridges, SMPS".into(),
                    ),
                ],
            },

            PinFunction::TimerBreak { timer, input } => FunctionInfo {
                description: format!(
                    "Break input {input} of TIM{timer}. A fault line from the board: when it \
                     asserts, the timer disables every output in hardware — no interrupt, no \
                     software in the path."
                ),
                specs: vec![
                    (
                        "Acts on".into(),
                        format!("every channel of TIM{timer}, main and complementary"),
                    ),
                    (
                        "Polarity".into(),
                        "Active low by default: a broken wire reads as a fault".into(),
                    ),
                    (
                        "After the fault".into(),
                        "Outputs stay off until software re-enables them, or automatically \
                         with AOE"
                            .into(),
                    ),
                    (
                        "Available on".into(),
                        "Advanced-control timers only (TIM1 / TIM8 / TIM20)".into(),
                    ),
                ],
            },

            PinFunction::OspiClk { port }
            | PinFunction::OspiNcs { port }
            | PinFunction::OspiDqs { port }
            | PinFunction::OspiIo { port, .. } => {
                let role = match self {
                    PinFunction::OspiClk { .. } => "clock".to_owned(),
                    PinFunction::OspiNcs { .. } => "chip select".to_owned(),
                    PinFunction::OspiDqs { .. } => "data strobe (DQS)".to_owned(),
                    PinFunction::OspiIo { lane, .. } => format!("data line {lane}"),
                    _ => String::new(),
                };
                FunctionInfo {
                    description: format!(
                        "OCTOSPI port {port} {role}. QUADSPI's successor: up to EIGHT data lines, \
                         and it speaks single, dual, quad and octal SPI as well as HyperBus."
                    ),
                    specs: vec![
                        (
                            "Width".into(),
                            "1, 2, 4 or 8 data lines — how many you wire narrows the mode".into(),
                        ),
                        (
                            "DQS".into(),
                            "Only the octal double-rate modes read it; leave it unwired otherwise"
                                .into(),
                        ),
                        (
                            "Port".into(),
                            "The IO manager routes a port to a controller; the default is \
                             port 1 to OCTOSPI1"
                                .into(),
                        ),
                        (
                            "Typical use".into(),
                            "Octal NOR flash, HyperRAM, PSRAM, execute-in-place".into(),
                        ),
                    ],
                }
            }

            PinFunction::QspiClk | PinFunction::QspiNcs { .. } | PinFunction::QspiIo { .. } => {
                let role = match self {
                    PinFunction::QspiClk => "clock, shared by both banks".to_owned(),
                    PinFunction::QspiNcs { bank } => format!("bank {bank} chip select"),
                    PinFunction::QspiIo { bank, lane } => format!("bank {bank} data line {lane}"),
                    _ => String::new(),
                };
                FunctionInfo {
                    description: format!(
                        "QUADSPI {role}. Four data lines instead of one, so an external flash \
                         reads about four times faster than over plain SPI — and the controller \
                         can map it straight into the address space."
                    ),
                    specs: vec![
                        (
                            "Bank".into(),
                            "Two chip selects, each with its own four data lines; the clock is \
                             shared"
                                .into(),
                        ),
                        (
                            "Per bank".into(),
                            "6 pads: CLK, NCS and IO0-IO3".into(),
                        ),
                        (
                            "Both banks".into(),
                            "Dual-flash mode: two chips read as one, eight bits wide".into(),
                        ),
                        (
                            "Typical use".into(),
                            "External NOR flash, PSRAM, execute-in-place".into(),
                        ),
                    ],
                }
            }

            PinFunction::SdmmcCk { unit }
            | PinFunction::SdmmcCmd { unit }
            | PinFunction::SdmmcD { unit, .. } => {
                let name = if *unit == 0 {
                    "SDIO".to_owned()
                } else {
                    format!("SDMMC{unit}")
                };
                let role = match self {
                    PinFunction::SdmmcCk { .. } => "clock".to_owned(),
                    PinFunction::SdmmcCmd { .. } => "command line".to_owned(),
                    PinFunction::SdmmcD { lane, .. } => format!("data line {lane}"),
                    _ => String::new(),
                };
                FunctionInfo {
                    description: format!(
                        "{name} {role}. The SD card / eMMC controller: a clock, a command line, \
                         and one, four or eight data lines — how many you wire IS the bus width."
                    ),
                    specs: vec![
                        (
                            "Bus width".into(),
                            "1, 4 or 8 lines — wire D0 alone, D0–D3, or D0–D7".into(),
                        ),
                        (
                            "Pull-ups".into(),
                            "CMD and the data lines need them; the driver sets the internal ones"
                                .into(),
                        ),
                        (
                            "Needs".into(),
                            "DMA on the older controllers; the newer ones have their own".into(),
                        ),
                        ("Typical use".into(), "SD cards, eMMC, SDIO radios".into()),
                    ],
                }
            }

            PinFunction::SaiSck { sai, block }
            | PinFunction::SaiSd { sai, block }
            | PinFunction::SaiFs { sai, block }
            | PinFunction::SaiMclk { sai, block } => {
                let letter = if *block == 1 { "A" } else { "B" };
                let role = match self {
                    PinFunction::SaiSck { .. } => "bit clock (SCK)",
                    PinFunction::SaiSd { .. } => "serial data (SD)",
                    PinFunction::SaiFs { .. } => "frame sync (FS)",
                    _ => "master clock out (MCLK) — optional, for the codec",
                };
                FunctionInfo {
                    description: format!(
                        "SAI{sai} sub-block {letter} {role}. The bigger sibling of I2S: the same \
                         kind of signals, but two INDEPENDENT sub-blocks per unit, so one can \
                         transmit while the other receives."
                    ),
                    specs: vec![
                        (
                            "Sub-blocks".into(),
                            format!("SAI{sai} A and B, each with its own pads and direction"),
                        ),
                        (
                            "Needs".into(),
                            "DMA: embassy drives SAI from a ring buffer per sub-block".into(),
                        ),
                        (
                            "Frame".into(),
                            "Slot count, slot size and frame length are all configurable — \
                             I2S, TDM and PCM are all this block"
                                .into(),
                        ),
                        (
                            "Typical use".into(),
                            "Audio codecs, multi-channel TDM, digital microphones".into(),
                        ),
                    ],
                }
            }

            PinFunction::DacOut { dac, channel } => FunctionInfo {
                description: format!(
                    "Analog output — DAC{dac} channel {channel}. The program writes a number \
                     and the pin holds the matching voltage; the mirror of an ADC input."
                ),
                specs: vec![
                    ("Resolution".into(), "12 bit (0 – 4095), or 8 bit".into()),
                    (
                        "Range".into(),
                        "0 V to VREF+, buffered — it can drive a light load directly".into(),
                    ),
                    (
                        "Shares the block with".into(),
                        format!("the other DAC{dac} channel, if this part has one"),
                    ),
                    (
                        "Typical use".into(),
                        "Bias points, waveform generation, analog set-points".into(),
                    ),
                ],
            },

            PinFunction::I2sCk(n)
            | PinFunction::I2sWs(n)
            | PinFunction::I2sSd(n)
            | PinFunction::I2sMck(n) => {
                let role = match self {
                    PinFunction::I2sCk(_) => "bit clock (CK) — one edge per data bit",
                    PinFunction::I2sWs(_) => {
                        "word select (WS/LRCK) — which channel this frame carries"
                    }
                    PinFunction::I2sSd(_) => "serial data (SD)",
                    _ => "master clock out (MCK) — an oversampled clock for the codec",
                };
                FunctionInfo {
                    description: format!(
                        "I2S{n} {role}. Digital audio: a bit clock, a word-select line that \
                         alternates left/right, and one data line per direction."
                    ),
                    specs: vec![
                        (
                            "Shares silicon with".into(),
                            format!("SPI{n} — the same block, so only one of the two can run"),
                        ),
                        (
                            "Needs".into(),
                            "DMA: embassy drives I2S from a ring buffer, never byte by byte"
                                .into(),
                        ),
                        (
                            "MCK".into(),
                            "Optional — only codecs that want a master clock need it".into(),
                        ),
                        (
                            "Typical use".into(),
                            "Audio codecs, DACs, MEMS microphones".into(),
                        ),
                    ],
                }
            }

            PinFunction::UsartTx(n) => FunctionInfo {
                description: format!(
                    "USART{n} transmit pin (TX). Sends serial data to an external device."
                ),
                specs: usart_common_specs(*n),
            },
            PinFunction::UsartRx(n) => FunctionInfo {
                description: format!(
                    "USART{n} receive pin (RX). Receives serial data from an external device."
                ),
                specs: usart_common_specs(*n),
            },
            PinFunction::UsartCts(n) => FunctionInfo {
                description: format!(
                    "USART{n} Clear-To-Send (CTS). Hardware flow control input — \
                     transmission is paused when CTS is HIGH."
                ),
                specs: usart_common_specs(*n),
            },
            PinFunction::UsartRts(n) => FunctionInfo {
                description: format!(
                    "USART{n} Request-To-Send (RTS). Hardware flow control output — \
                     signals the sender that this device is ready to receive."
                ),
                specs: usart_common_specs(*n),
            },
            PinFunction::UsartCk(n) => FunctionInfo {
                description: format!(
                    "USART{n} synchronous clock (CK). Used in synchronous (SPI-like) mode."
                ),
                specs: usart_common_specs(*n),
            },

            // ── LPUART (low-power UART — its own peripheral) ─────────────────
            PinFunction::LpuartTx(n) => FunctionInfo {
                description: format!(
                    "LPUART{n} transmit (TX). Low-power UART — runs from LSE/HSI so it \
                     keeps receiving in Stop mode, at lower maximum baud rates than USART."
                ),
                specs: usart_common_specs(*n),
            },
            PinFunction::LpuartRx(n) => FunctionInfo {
                description: format!(
                    "LPUART{n} receive (RX). Can wake the MCU from Stop mode on a start bit."
                ),
                specs: usart_common_specs(*n),
            },
            PinFunction::LpuartCts(n) => FunctionInfo {
                description: format!("LPUART{n} Clear-To-Send (CTS). Hardware flow-control input."),
                specs: usart_common_specs(*n),
            },
            PinFunction::LpuartRts(n) => FunctionInfo {
                description: format!(
                    "LPUART{n} Request-To-Send (RTS). Flow-control output; the same pin \
                     also serves as DE (driver enable) for RS485 transceivers."
                ),
                specs: usart_common_specs(*n),
            },

            PinFunction::SpiNss(n) => FunctionInfo {
                description: format!(
                    "SPI{n} Chip Select / Slave Select (NSS). \
                     Pulled LOW to activate the target device."
                ),
                specs: spi_common_specs(*n),
            },
            PinFunction::SpiRdy(n) => FunctionInfo {
                description: format!(
                    "SPI{n} RDY — slave-ready handshake. The slave drives it to tell the \
                     master it can accept the next transfer (STM32WBA / U5 and later)."
                ),
                specs: spi_common_specs(*n),
            },
            PinFunction::SpiSck(n) => FunctionInfo {
                description: format!("SPI{n} clock output (SCK). Generated by the SPI master."),
                specs: spi_common_specs(*n),
            },
            PinFunction::SpiMiso(n) => FunctionInfo {
                description: format!(
                    "SPI{n} MISO — Master In / Slave Out. \
                     Data line from the slave device to the master."
                ),
                specs: spi_common_specs(*n),
            },
            PinFunction::SpiMosi(n) => FunctionInfo {
                description: format!(
                    "SPI{n} MOSI — Master Out / Slave In. \
                     Data line from the master device to the slave."
                ),
                specs: spi_common_specs(*n),
            },

            PinFunction::I2cScl(n) => FunctionInfo {
                description: format!(
                    "I2C{n} clock line (SCL). Driven by the master to synchronize data transfer."
                ),
                specs: i2c_common_specs(*n),
            },
            PinFunction::I2cSda(n) => FunctionInfo {
                description: format!("I2C{n} data line (SDA). Bidirectional open-drain data bus."),
                specs: i2c_common_specs(*n),
            },

            PinFunction::UsbDm => FunctionInfo {
                description: "USB Full-Speed D− differential data line.".into(),
                specs: usb_specs(),
            },
            PinFunction::UsbDp => FunctionInfo {
                description: "USB Full-Speed D+ differential data line.".into(),
                specs: usb_specs(),
            },

            PinFunction::CanRx => FunctionInfo {
                description: "CAN bus receive line. Connects to the CAN transceiver RX output."
                    .into(),
                specs: can_specs(),
            },
            PinFunction::CanTx => FunctionInfo {
                description: "CAN bus transmit line. Connects to the CAN transceiver TX input."
                    .into(),
                specs: can_specs(),
            },

            PinFunction::SwdIo => FunctionInfo {
                description: "SWD bidirectional data line (SWDIO). \
                               Used by debuggers (ST-Link, J-Link) to program and debug the MCU."
                    .into(),
                specs: vec![
                    ("Interface".into(), "ARM Serial Wire Debug (SWD)".into()),
                    ("Role".into(), "SWDIO — bidirectional data".into()),
                    ("Also".into(), "JTMS — JTAG mode select".into()),
                    ("Speed".into(), "Up to several MHz".into()),
                    (
                        "Note".into(),
                        "Can be reconfigured as GPIO after boot".into(),
                    ),
                ],
            },
            PinFunction::SwdClk => FunctionInfo {
                description: "SWD clock line (SWCLK). Driven by the debugger/programmer.".into(),
                specs: vec![
                    ("Interface".into(), "ARM Serial Wire Debug (SWD)".into()),
                    ("Role".into(), "SWCLK — clock input".into()),
                    ("Also".into(), "JTCK — JTAG clock".into()),
                    (
                        "Note".into(),
                        "Can be reconfigured as GPIO after boot".into(),
                    ),
                ],
            },

            PinFunction::Other(name) => FunctionInfo {
                description: format!(
                    "{name} — alternate function carried verbatim from the datasheet. \
                     The IDE doesn't model this peripheral natively, so no init code is \
                     generated: the pin is configured as an alternate function and its \
                     peripheral singleton is handed to you to drive directly."
                ),
                specs: vec![("Signal".into(), name.clone())],
            },

            PinFunction::Mco => FunctionInfo {
                description: "Master Clock Output. Outputs an internal clock signal on PA8 \
                               for use by external devices."
                    .into(),
                specs: vec![
                    ("Sources".into(), "SYSCLK, HSI, HSE, PLL/2".into()),
                    ("Max frequency".into(), "50 MHz".into()),
                    (
                        "Use cases".into(),
                        "Clock an external device, measure SYSCLK".into(),
                    ),
                ],
            },
        }
    }
}

// ── Shared spec helpers ──────────────────────────────────────────────────────

fn usart_common_specs(n: u8) -> Vec<(String, String)> {
    let max_baud = if n == 1 {
        "4.5 Mbit/s  (APB2 72 MHz)"
    } else {
        "2.25 Mbit/s  (APB1 36 MHz)"
    };
    vec![
        ("Peripheral".into(), format!("USART{n}")),
        (
            "Mode".into(),
            "Full-duplex async (also sync with CK)".into(),
        ),
        ("Max baud rate".into(), max_baud.into()),
        ("Data bits".into(), "8 or 9 bits".into()),
        ("Stop bits".into(), "0.5 / 1 / 1.5 / 2".into()),
        ("Parity".into(), "None, Even, Odd".into()),
        (
            "Flow control".into(),
            "Hardware CTS/RTS or software XON/XOFF".into(),
        ),
        ("DMA".into(), "Supported on TX and RX".into()),
    ]
}

fn spi_common_specs(n: u8) -> Vec<(String, String)> {
    let max_speed = if n == 1 {
        "18 Mbit/s  (APB2 72 MHz / 4)"
    } else {
        "9 Mbit/s  (APB1 36 MHz / 4)"
    };
    vec![
        ("Peripheral".into(), format!("SPI{n}")),
        ("Mode".into(), "Master or Slave, full-duplex".into()),
        ("Max speed".into(), max_speed.into()),
        ("Data frame".into(), "8-bit or 16-bit".into()),
        ("Clock polarity".into(), "CPOL 0 or 1".into()),
        ("Clock phase".into(), "CPHA 0 or 1  ->  4 SPI modes".into()),
        ("NSS management".into(), "Hardware or software".into()),
        ("DMA".into(), "Supported on TX and RX".into()),
    ]
}

fn i2c_common_specs(n: u8) -> Vec<(String, String)> {
    vec![
        ("Peripheral".into(), format!("I2C{n}")),
        ("Standard mode".into(), "100 kHz".into()),
        ("Fast mode".into(), "400 kHz".into()),
        ("Addressing".into(), "7-bit or 10-bit".into()),
        (
            "Bus type".into(),
            "Open-drain — requires pull-up resistors".into(),
        ),
        (
            "Pull-up value".into(),
            "4.7 kΩ (100 kHz) / 1 kΩ (400 kHz) typical".into(),
        ),
        ("SMBus".into(), "Supported".into()),
        ("DMA".into(), "Supported".into()),
    ]
}

fn usb_specs() -> Vec<(String, String)> {
    vec![
        ("Standard".into(), "USB 2.0 Full-Speed".into()),
        ("Speed".into(), "12 Mbit/s".into()),
        ("Mode".into(), "Device only (no host/OTG)".into()),
        ("Endpoints".into(), "8 bidirectional (EP0 – EP7)".into()),
        (
            "Buffer memory".into(),
            "512 bytes packet buffer SRAM".into(),
        ),
        ("Voltage".into(), "D+ / D−  3.3 V".into()),
        (
            "Note".into(),
            "Requires 48 MHz USB clock (PLL must be configured)".into(),
        ),
    ]
}

fn can_specs() -> Vec<(String, String)> {
    vec![
        (
            "Standard".into(),
            "CAN 2.0A (11-bit ID) and 2.0B (29-bit ID)".into(),
        ),
        ("Max speed".into(), "1 Mbit/s".into()),
        ("TX mailboxes".into(), "3 transmit mailboxes".into()),
        (
            "RX FIFOs".into(),
            "2 receive FIFOs (3 messages each)".into(),
        ),
        ("Filters".into(), "14 configurable message filters".into()),
        (
            "Note".into(),
            "Requires external CAN transceiver (e.g. TJA1050)".into(),
        ),
    ]
}
