use eframe::egui;

// ── Info struct ──────────────────────────────────────────────────────────────

pub struct FunctionInfo {
    pub description: String,
    pub specs: Vec<(String, String)>,
}

// ── PinFunction enum ─────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug, Default)]
pub enum PinFunction {
    #[default]
    Unset,

    // ── GPIO ────────────────────────────────────────────────────────────────
    GpioInput,
    GpioOutput,

    // ── ADC ─────────────────────────────────────────────────────────────────
    /// Analog input — ADC{adc} channel {channel}
    AdcChannel { adc: u8, channel: u8 },

    // ── Timers / PWM ────────────────────────────────────────────────────────
    /// PWM output — TIM{timer} CH{channel}
    TimerPwm { timer: u8, channel: u8 },

    // ── USART ───────────────────────────────────────────────────────────────
    UsartTx(u8),   // USART{n} TX
    UsartRx(u8),   // USART{n} RX
    UsartCts(u8),  // USART{n} CTS (hardware flow control)
    UsartRts(u8),  // USART{n} RTS (hardware flow control)
    UsartCk(u8),   // USART{n} CK  (synchronous clock)

    // ── SPI ─────────────────────────────────────────────────────────────────
    SpiNss(u8),    // SPI{n} NSS  — chip select
    SpiSck(u8),    // SPI{n} SCK  — clock
    SpiMiso(u8),   // SPI{n} MISO — master in / slave out
    SpiMosi(u8),   // SPI{n} MOSI — master out / slave in

    // ── I2C ─────────────────────────────────────────────────────────────────
    I2cScl(u8),    // I2C{n} SCL — clock
    I2cSda(u8),    // I2C{n} SDA — data

    // ── USB ─────────────────────────────────────────────────────────────────
    UsbDm,         // USB D−  (PA11)
    UsbDp,         // USB D+  (PA12)

    // ── CAN ─────────────────────────────────────────────────────────────────
    CanRx,         // CAN RX (PA11 / PB8)
    CanTx,         // CAN TX (PA12 / PB9)

    // ── Debug interface ─────────────────────────────────────────────────────
    SwdIo,         // SWDIO / JTMS
    SwdClk,        // SWCLK / JTCK

    // ── Clock output ────────────────────────────────────────────────────────
    Mco,           // Master Clock Output (PA8)
}

// ── Display helpers ──────────────────────────────────────────────────────────

impl PinFunction {
    /// Full label shown on the function button
    pub fn label(&self) -> String {
        match self {
            PinFunction::Unset              => "Not configured".into(),
            PinFunction::GpioInput          => "GPIO Input".into(),
            PinFunction::GpioOutput         => "GPIO Output".into(),
            PinFunction::AdcChannel{adc, channel} => format!("ADC{adc}  IN{channel}"),
            PinFunction::TimerPwm{timer, channel} => format!("TIM{timer}  CH{channel}  (PWM)"),
            PinFunction::UsartTx(n)         => format!("USART{n}  TX"),
            PinFunction::UsartRx(n)         => format!("USART{n}  RX"),
            PinFunction::UsartCts(n)        => format!("USART{n}  CTS"),
            PinFunction::UsartRts(n)        => format!("USART{n}  RTS"),
            PinFunction::UsartCk(n)         => format!("USART{n}  CK"),
            PinFunction::SpiNss(n)          => format!("SPI{n}  NSS"),
            PinFunction::SpiSck(n)          => format!("SPI{n}  SCK"),
            PinFunction::SpiMiso(n)         => format!("SPI{n}  MISO"),
            PinFunction::SpiMosi(n)         => format!("SPI{n}  MOSI"),
            PinFunction::I2cScl(n)          => format!("I2C{n}  SCL"),
            PinFunction::I2cSda(n)          => format!("I2C{n}  SDA"),
            PinFunction::UsbDm              => "USB  D−".into(),
            PinFunction::UsbDp              => "USB  D+".into(),
            PinFunction::CanRx              => "CAN  RX".into(),
            PinFunction::CanTx              => "CAN  TX".into(),
            PinFunction::SwdIo              => "SWD  IO  (JTMS)".into(),
            PinFunction::SwdClk             => "SWD  CLK  (JTCK)".into(),
            PinFunction::Mco                => "MCO  (Master Clock Out)".into(),
        }
    }

    /// Reverse of `label()` — parses a label string back into a `PinFunction`.
    ///
    /// Used by `codegen::parse_main_rs()` to restore pin state from an
    /// existing `src/main.rs` when opening a project.
    /// Returns `None` for unrecognised or unsupported labels (e.g. "Not configured").
    pub fn from_label(s: &str) -> Option<PinFunction> {
        // ── Fixed / parameter-free variants ──────────────────────────────────
        match s {
            "GPIO Input"              => return Some(PinFunction::GpioInput),
            "GPIO Output"             => return Some(PinFunction::GpioOutput),
            "USB  D−"                 => return Some(PinFunction::UsbDm),
            "USB  D+"                 => return Some(PinFunction::UsbDp),
            "CAN  RX"                 => return Some(PinFunction::CanRx),
            "CAN  TX"                 => return Some(PinFunction::CanTx),
            "SWD  IO  (JTMS)"         => return Some(PinFunction::SwdIo),
            "SWD  CLK  (JTCK)"        => return Some(PinFunction::SwdClk),
            "MCO  (Master Clock Out)" => return Some(PinFunction::Mco),
            _                         => {}
        }

        // ── USART{n}  {role} ─────────────────────────────────────────────────
        if let Some(rest) = s.strip_prefix("USART") {
            if let Some((num_str, role)) = rest.split_once("  ") {
                if let Ok(n) = num_str.parse::<u8>() {
                    return match role {
                        "TX"  => Some(PinFunction::UsartTx(n)),
                        "RX"  => Some(PinFunction::UsartRx(n)),
                        "CTS" => Some(PinFunction::UsartCts(n)),
                        "RTS" => Some(PinFunction::UsartRts(n)),
                        "CK"  => Some(PinFunction::UsartCk(n)),
                        _     => None,
                    };
                }
            }
        }

        // ── SPI{n}  {role} ───────────────────────────────────────────────────
        if let Some(rest) = s.strip_prefix("SPI") {
            if let Some((num_str, role)) = rest.split_once("  ") {
                if let Ok(n) = num_str.parse::<u8>() {
                    return match role {
                        "NSS"  => Some(PinFunction::SpiNss(n)),
                        "SCK"  => Some(PinFunction::SpiSck(n)),
                        "MISO" => Some(PinFunction::SpiMiso(n)),
                        "MOSI" => Some(PinFunction::SpiMosi(n)),
                        _      => None,
                    };
                }
            }
        }

        // ── I2C{n}  {role} ───────────────────────────────────────────────────
        if let Some(rest) = s.strip_prefix("I2C") {
            if let Some((num_str, role)) = rest.split_once("  ") {
                if let Ok(n) = num_str.parse::<u8>() {
                    return match role {
                        "SCL" => Some(PinFunction::I2cScl(n)),
                        "SDA" => Some(PinFunction::I2cSda(n)),
                        _     => None,
                    };
                }
            }
        }

        // ── ADC{adc}  IN{channel} ────────────────────────────────────────────
        // label() → format!("ADC{adc}  IN{channel}")
        if let Some(rest) = s.strip_prefix("ADC") {
            // rest = "1  IN3"  →  split on "  IN"
            if let Some((adc_str, ch_str)) = rest.split_once("  IN") {
                if let (Ok(adc), Ok(channel)) =
                    (adc_str.parse::<u8>(), ch_str.parse::<u8>())
                {
                    return Some(PinFunction::AdcChannel { adc, channel });
                }
            }
        }

        // ── TIM{timer}  CH{channel}  (PWM) ───────────────────────────────────
        // label() → format!("TIM{timer}  CH{channel}  (PWM)")
        if let Some(rest) = s.strip_prefix("TIM") {
            // rest = "2  CH1  (PWM)"
            if let Some((timer_str, ch_rest)) = rest.split_once("  CH") {
                // ch_rest = "1  (PWM)" — take everything up to the next "  "
                let ch_num_str = ch_rest.split("  ").next().unwrap_or("");
                if let (Ok(timer), Ok(channel)) =
                    (timer_str.parse::<u8>(), ch_num_str.parse::<u8>())
                {
                    return Some(PinFunction::TimerPwm { timer, channel });
                }
            }
        }

        None
    }

    /// Returns the stm32f1xx-hal GPIO mode type name used to generate
    /// `pub type PinType = Pin<PORT, N, MODE>;` in the per-pin source file.
    /// Returns `None` for `Unset` (no file should be written).
    pub fn hal_gpio_mode(&self) -> Option<&'static str> {
        match self {
            PinFunction::Unset          => None,
            PinFunction::GpioInput      => Some("Input"),
            PinFunction::GpioOutput     => Some("Output"),
            PinFunction::AdcChannel{..} => Some("Analog"),
            // Everything else is an alternate-function pin
            _ => Some("Alternate"),
        }
    }

    /// Short badge shown on the pin in the diagram
    pub fn short_label(&self) -> &str {
        match self {
            PinFunction::Unset          => "—",
            PinFunction::GpioInput      => "IN",
            PinFunction::GpioOutput     => "OUT",
            PinFunction::AdcChannel{..} => "ADC",
            PinFunction::TimerPwm{..}   => "PWM",
            PinFunction::UsartTx(_)     => "TX",
            PinFunction::UsartRx(_)     => "RX",
            PinFunction::UsartCts(_)    => "CTS",
            PinFunction::UsartRts(_)    => "RTS",
            PinFunction::UsartCk(_)     => "CK",
            PinFunction::SpiNss(_)      => "NSS",
            PinFunction::SpiSck(_)      => "SCK",
            PinFunction::SpiMiso(_)     => "MISO",
            PinFunction::SpiMosi(_)     => "MOSI",
            PinFunction::I2cScl(_)      => "SCL",
            PinFunction::I2cSda(_)      => "SDA",
            PinFunction::UsbDm          => "D−",
            PinFunction::UsbDp          => "D+",
            PinFunction::CanRx          => "CAN",
            PinFunction::CanTx          => "CAN",
            PinFunction::SwdIo          => "SWD",
            PinFunction::SwdClk         => "SWD",
            PinFunction::Mco            => "MCO",
        }
    }

    /// Background color — grouped by peripheral category
    pub fn color(&self) -> egui::Color32 {
        match self {
            PinFunction::Unset              => egui::Color32::LIGHT_BLUE,
            PinFunction::GpioInput          => egui::Color32::from_rgb(70, 160, 70),
            PinFunction::GpioOutput         => egui::Color32::from_rgb(200, 120, 50),
            PinFunction::AdcChannel{..}     => egui::Color32::from_rgb(150, 70, 200),
            PinFunction::TimerPwm{..}       => egui::Color32::from_rgb(190, 170, 30),
            PinFunction::UsartTx(_)
            | PinFunction::UsartRx(_)
            | PinFunction::UsartCts(_)
            | PinFunction::UsartRts(_)
            | PinFunction::UsartCk(_)       => egui::Color32::from_rgb(50, 110, 200),
            PinFunction::SpiNss(_)
            | PinFunction::SpiSck(_)
            | PinFunction::SpiMiso(_)
            | PinFunction::SpiMosi(_)       => egui::Color32::from_rgb(30, 170, 170),
            PinFunction::I2cScl(_)
            | PinFunction::I2cSda(_)        => egui::Color32::from_rgb(60, 180, 100),
            PinFunction::UsbDm
            | PinFunction::UsbDp            => egui::Color32::from_rgb(190, 50, 160),
            PinFunction::CanRx
            | PinFunction::CanTx            => egui::Color32::from_rgb(200, 130, 20),
            PinFunction::SwdIo
            | PinFunction::SwdClk           => egui::Color32::from_rgb(190, 50, 50),
            PinFunction::Mco                => egui::Color32::from_rgb(130, 130, 130),
        }
    }

    /// Detailed info shown in the ⓘ popup window
    pub fn info(&self) -> FunctionInfo {
        match self {
            PinFunction::Unset => FunctionInfo {
                description: "Pin is not configured. Select a function to enable it.".into(),
                specs: vec![],
            },

            PinFunction::GpioInput => FunctionInfo {
                description: "Digital input. Reads the logic level on the pin (HIGH / LOW).".into(),
                specs: vec![
                    ("Input voltage".into(), "0 V – 3.3 V (5 V tolerant on most pins)".into()),
                    ("Input modes".into(), "Floating, Pull-up (40 kΩ), Pull-down (40 kΩ)".into()),
                    ("Interrupt".into(), "EXTI — rising, falling, or both edges".into()),
                    ("Max frequency".into(), "Up to 72 MHz input signal".into()),
                ],
            },

            PinFunction::GpioOutput => FunctionInfo {
                description: "Digital output. Drives the pin HIGH (3.3 V) or LOW (0 V).".into(),
                specs: vec![
                    ("Output modes".into(), "Push-pull or Open-drain".into()),
                    ("Max current per pin".into(), "±25 mA (sink or source)".into()),
                    ("Total GPIO current".into(), "Max 80 mA sourced, 80 mA sunk".into()),
                    ("Toggle speed".into(), "2 MHz / 10 MHz / 50 MHz (configurable)".into()),
                    ("Output voltage".into(), "0 V (LOW) or VDD 3.3 V (HIGH)".into()),
                ],
            },

            PinFunction::AdcChannel { adc, channel } => FunctionInfo {
                description: format!(
                    "Analog-to-Digital Converter input. ADC{adc} channel IN{channel}. \
                     Converts an analog voltage (0–VDDA) to a 12-bit digital value."
                ),
                specs: vec![
                    ("Resolution".into(), "12-bit  →  0 – 4095 counts".into()),
                    ("Reference voltage".into(), "VDDA (typ. 3.3 V)".into()),
                    ("External channels".into(), "10 channels  (IN0 – IN9)".into()),
                    ("Conversion speed".into(), "Up to 1 MSPS (1 MHz)".into()),
                    ("Sample time".into(), "1.5 – 239.5 ADC clock cycles".into()),
                    ("ADC clock".into(), "Up to 14 MHz (APB2 / prescaler)".into()),
                    ("Modes".into(), "Single, continuous, scan, injected".into()),
                    (format!("This pin").into(), format!("ADC{adc}  IN{channel}").into()),
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
                    (format!("Timer TIM{timer}").into(),
                        if *timer == 1 { "Advanced — dead-time, break, complementary outputs".into() }
                        else           { "General purpose — 4 channels, input capture, output compare".into() }),
                    (format!("Channel CH{channel}").into(), "Output compare / PWM mode 1 or 2".into()),
                ],
            },

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

            PinFunction::SpiNss(n) => FunctionInfo {
                description: format!(
                    "SPI{n} Chip Select / Slave Select (NSS). \
                     Pulled LOW to activate the target device."
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
                description: format!(
                    "I2C{n} data line (SDA). Bidirectional open-drain data bus."
                ),
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
                description: "CAN bus receive line. Connects to the CAN transceiver RX output.".into(),
                specs: can_specs(),
            },
            PinFunction::CanTx => FunctionInfo {
                description: "CAN bus transmit line. Connects to the CAN transceiver TX input.".into(),
                specs: can_specs(),
            },

            PinFunction::SwdIo => FunctionInfo {
                description: "SWD bidirectional data line (SWDIO). \
                               Used by debuggers (ST-Link, J-Link) to program and debug the MCU.".into(),
                specs: vec![
                    ("Interface".into(), "ARM Serial Wire Debug (SWD)".into()),
                    ("Role".into(), "SWDIO — bidirectional data".into()),
                    ("Also".into(), "JTMS — JTAG mode select".into()),
                    ("Speed".into(), "Up to several MHz".into()),
                    ("Note".into(), "Can be reconfigured as GPIO after boot".into()),
                ],
            },
            PinFunction::SwdClk => FunctionInfo {
                description: "SWD clock line (SWCLK). Driven by the debugger/programmer.".into(),
                specs: vec![
                    ("Interface".into(), "ARM Serial Wire Debug (SWD)".into()),
                    ("Role".into(), "SWCLK — clock input".into()),
                    ("Also".into(), "JTCK — JTAG clock".into()),
                    ("Note".into(), "Can be reconfigured as GPIO after boot".into()),
                ],
            },

            PinFunction::Mco => FunctionInfo {
                description: "Master Clock Output. Outputs an internal clock signal on PA8 \
                               for use by external devices.".into(),
                specs: vec![
                    ("Sources".into(), "SYSCLK, HSI, HSE, PLL/2".into()),
                    ("Max frequency".into(), "50 MHz".into()),
                    ("Use cases".into(), "Clock an external device, measure SYSCLK".into()),
                ],
            },
        }
    }
}

// ── Shared spec helpers ──────────────────────────────────────────────────────

fn usart_common_specs(n: u8) -> Vec<(String, String)> {
    let max_baud = if n == 1 { "4.5 Mbit/s  (APB2 72 MHz)" } else { "2.25 Mbit/s  (APB1 36 MHz)" };
    vec![
        ("Peripheral".into(), format!("USART{n}")),
        ("Mode".into(), "Full-duplex async (also sync with CK)".into()),
        ("Max baud rate".into(), max_baud.into()),
        ("Data bits".into(), "8 or 9 bits".into()),
        ("Stop bits".into(), "0.5 / 1 / 1.5 / 2".into()),
        ("Parity".into(), "None, Even, Odd".into()),
        ("Flow control".into(), "Hardware CTS/RTS or software XON/XOFF".into()),
        ("DMA".into(), "Supported on TX and RX".into()),
    ]
}

fn spi_common_specs(n: u8) -> Vec<(String, String)> {
    let max_speed = if n == 1 { "18 Mbit/s  (APB2 72 MHz / 4)" } else { "9 Mbit/s  (APB1 36 MHz / 4)" };
    vec![
        ("Peripheral".into(), format!("SPI{n}")),
        ("Mode".into(), "Master or Slave, full-duplex".into()),
        ("Max speed".into(), max_speed.into()),
        ("Data frame".into(), "8-bit or 16-bit".into()),
        ("Clock polarity".into(), "CPOL 0 or 1".into()),
        ("Clock phase".into(), "CPHA 0 or 1  →  4 SPI modes".into()),
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
        ("Bus type".into(), "Open-drain — requires pull-up resistors".into()),
        ("Pull-up value".into(), "4.7 kΩ (100 kHz) / 1 kΩ (400 kHz) typical".into()),
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
        ("Buffer memory".into(), "512 bytes packet buffer SRAM".into()),
        ("Voltage".into(), "D+ / D−  3.3 V".into()),
        ("Note".into(), "Requires 48 MHz USB clock (PLL must be configured)".into()),
    ]
}

fn can_specs() -> Vec<(String, String)> {
    vec![
        ("Standard".into(), "CAN 2.0A (11-bit ID) and 2.0B (29-bit ID)".into()),
        ("Max speed".into(), "1 Mbit/s".into()),
        ("TX mailboxes".into(), "3 transmit mailboxes".into()),
        ("RX FIFOs".into(), "2 receive FIFOs (3 messages each)".into()),
        ("Filters".into(), "14 configurable message filters".into()),
        ("Note".into(), "Requires external CAN transceiver (e.g. TJA1050)".into()),
    ]
}
