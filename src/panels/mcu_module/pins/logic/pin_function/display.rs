use super::enum_::PinFunction;

// ── Display helpers ──────────────────────────────────────────────────────────

impl PinFunction {
    /// Short, filesystem-safe token for the selected function, used as the
    /// suffix of a generated pin file name (`pin<n>_<name>_<token>.rs`, e.g.
    /// `pin2_pc13_out.rs`). Always a valid Rust identifier so it can also be a
    /// `pub mod` name. Peripheral numbers are intentionally omitted — the pin
    /// number already makes each file name unique.
    pub fn file_token(&self) -> &'static str {
        match self {
            PinFunction::Unset       => "unset",
            PinFunction::GpioInput   => "in",
            PinFunction::GpioOutput  => "out",
            PinFunction::AdcChannel { .. } => "adc",
            PinFunction::TimerPwm { .. }   => "pwm",
            PinFunction::UsartTx(_)  => "uart_tx",
            PinFunction::UsartRx(_)  => "uart_rx",
            PinFunction::UsartCts(_) => "uart_cts",
            PinFunction::UsartRts(_) => "uart_rts",
            PinFunction::UsartCk(_)  => "uart_ck",
            PinFunction::LpuartTx(_)  => "lpuart_tx",
            PinFunction::LpuartRx(_)  => "lpuart_rx",
            PinFunction::LpuartCts(_) => "lpuart_cts",
            PinFunction::LpuartRts(_) => "lpuart_rts",
            PinFunction::SpiNss(_)   => "spi_nss",
            PinFunction::SpiSck(_)   => "spi_sck",
            PinFunction::SpiMiso(_)  => "spi_miso",
            PinFunction::SpiMosi(_)  => "spi_mosi",
            PinFunction::SpiRdy(_)   => "spi_rdy",
            PinFunction::I2cScl(_)   => "i2c_scl",
            PinFunction::I2cSda(_)   => "i2c_sda",
            PinFunction::UsbDm       => "usb_dm",
            PinFunction::UsbDp       => "usb_dp",
            PinFunction::CanRx       => "can_rx",
            PinFunction::CanTx       => "can_tx",
            PinFunction::SwdIo       => "swdio",
            PinFunction::SwdClk      => "swclk",
            PinFunction::Mco         => "mco",
            // The pin number already makes the file name unique, so a single
            // token is enough for every generic alternate function.
            PinFunction::Other(_)    => "af",
        }
    }

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
            PinFunction::LpuartTx(n)        => format!("LPUART{n}  TX"),
            PinFunction::LpuartRx(n)        => format!("LPUART{n}  RX"),
            PinFunction::LpuartCts(n)       => format!("LPUART{n}  CTS"),
            PinFunction::LpuartRts(n)       => format!("LPUART{n}  RTS"),
            PinFunction::SpiNss(n)          => format!("SPI{n}  NSS"),
            PinFunction::SpiSck(n)          => format!("SPI{n}  SCK"),
            PinFunction::SpiMiso(n)         => format!("SPI{n}  MISO"),
            PinFunction::SpiMosi(n)         => format!("SPI{n}  MOSI"),
            PinFunction::SpiRdy(n)          => format!("SPI{n}  RDY"),
            PinFunction::I2cScl(n)          => format!("I2C{n}  SCL"),
            PinFunction::I2cSda(n)          => format!("I2C{n}  SDA"),
            PinFunction::UsbDm              => "USB  D−".into(),
            PinFunction::UsbDp              => "USB  D+".into(),
            PinFunction::CanRx              => "CAN  RX".into(),
            PinFunction::CanTx              => "CAN  TX".into(),
            PinFunction::SwdIo              => "SWD  IO  (JTMS)".into(),
            PinFunction::SwdClk             => "SWD  CLK  (JTCK)".into(),
            PinFunction::Mco                => "MCO  (Master Clock Out)".into(),
            // The datasheet's own signal name is the label.
            PinFunction::Other(name)        => name.clone(),
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

        // ── LPUART{n}  {role} ────────────────────────────────────────────────
        // Checked BEFORE USART is irrelevant (the prefixes differ at index 0),
        // but keep it first so the intent is obvious.
        if let Some(rest) = s.strip_prefix("LPUART") {
            if let Some((num_str, role)) = rest.split_once("  ") {
                if let Ok(n) = num_str.parse::<u8>() {
                    return match role {
                        "TX" => Some(PinFunction::LpuartTx(n)),
                        "RX" => Some(PinFunction::LpuartRx(n)),
                        "CTS" => Some(PinFunction::LpuartCts(n)),
                        "RTS" => Some(PinFunction::LpuartRts(n)),
                        _ => None,
                    };
                }
            }
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
                        "RDY"  => Some(PinFunction::SpiRdy(n)),
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

        // ── Generic alternate function ───────────────────────────────────────
        // `label()` for `Other` is the datasheet signal name itself, which is
        // always UPPERCASE alphanumerics / `_` / `-` and never contains the
        // double space the labels above use — so this can't shadow them.
        if !s.is_empty()
            && s.starts_with(|c: char| c.is_ascii_uppercase())
            && !s.contains("  ")
            && s.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Some(PinFunction::Other(s.to_owned()));
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
            PinFunction::LpuartTx(_)    => "LPTX",
            PinFunction::LpuartRx(_)    => "LPRX",
            PinFunction::LpuartCts(_)   => "LPCTS",
            PinFunction::LpuartRts(_)   => "LPRTS",
            PinFunction::SpiNss(_)      => "NSS",
            PinFunction::SpiSck(_)      => "SCK",
            PinFunction::SpiMiso(_)     => "MISO",
            PinFunction::SpiMosi(_)     => "MOSI",
            PinFunction::SpiRdy(_)      => "RDY",
            PinFunction::I2cScl(_)      => "SCL",
            PinFunction::I2cSda(_)      => "SDA",
            PinFunction::UsbDm          => "D−",
            PinFunction::UsbDp          => "D+",
            PinFunction::CanRx          => "CAN",
            PinFunction::CanTx          => "CAN",
            PinFunction::SwdIo          => "SWD",
            PinFunction::SwdClk         => "SWD",
            PinFunction::Mco            => "MCO",
            PinFunction::Other(name)    => name,
        }
    }
}
