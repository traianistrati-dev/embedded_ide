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
            PinFunction::Unset => "unset",
            PinFunction::GpioInput => "in",
            PinFunction::GpioOutput => "out",
            PinFunction::GpioAnalog => "analog",
            PinFunction::AdcChannel { .. } => "adc",
            PinFunction::TimerPwm { .. } => "pwm",
            PinFunction::TimerPwmN { .. } => "pwmn",
            PinFunction::TimerBreak { .. } => "bkin",
            PinFunction::DacOut { .. } => "dac",
            PinFunction::I2sCk(_) => "i2s_ck",
            PinFunction::I2sWs(_) => "i2s_ws",
            PinFunction::I2sSd(_) => "i2s_sd",
            PinFunction::I2sMck(_) => "i2s_mck",
            PinFunction::UsartTx(_) => "uart_tx",
            PinFunction::UsartRx(_) => "uart_rx",
            PinFunction::UsartCts(_) => "uart_cts",
            PinFunction::UsartRts(_) => "uart_rts",
            PinFunction::UsartCk(_) => "uart_ck",
            PinFunction::LpuartTx(_) => "lpuart_tx",
            PinFunction::LpuartRx(_) => "lpuart_rx",
            PinFunction::LpuartCts(_) => "lpuart_cts",
            PinFunction::LpuartRts(_) => "lpuart_rts",
            PinFunction::SpiNss(_) => "spi_nss",
            PinFunction::SpiSck(_) => "spi_sck",
            PinFunction::SpiMiso(_) => "spi_miso",
            PinFunction::SpiMosi(_) => "spi_mosi",
            PinFunction::SpiRdy(_) => "spi_rdy",
            PinFunction::I2cScl(_) => "i2c_scl",
            PinFunction::I2cSda(_) => "i2c_sda",
            PinFunction::UsbDm => "usb_dm",
            PinFunction::UsbDp => "usb_dp",
            PinFunction::CanRx => "can_rx",
            PinFunction::CanTx => "can_tx",
            PinFunction::SwdIo => "swdio",
            PinFunction::SwdClk => "swclk",
            PinFunction::Mco => "mco",
            // The pin number already makes the file name unique, so a single
            // token is enough for every generic alternate function.
            PinFunction::Other(_) => "af",
        }
    }

    /// Full label shown on the function button
    pub fn label(&self) -> String {
        match self {
            PinFunction::Unset => "Not configured".into(),
            PinFunction::GpioInput => "GPIO Input".into(),
            PinFunction::GpioOutput => "GPIO Output".into(),
            PinFunction::GpioAnalog => "GPIO Analog".into(),
            PinFunction::AdcChannel { adc, channel } => format!("ADC{adc}  IN{channel}"),
            PinFunction::TimerPwm { timer, channel } => format!("TIM{timer}  CH{channel}  (PWM)"),
            PinFunction::TimerPwmN { timer, channel } => {
                format!("TIM{timer}  CH{channel}N  (PWM)")
            }
            PinFunction::TimerBreak { timer, input } => {
                let n = if *input == 1 {
                    String::new()
                } else {
                    input.to_string()
                };
                format!("TIM{timer}  BKIN{n}  (break)")
            }
            PinFunction::DacOut { dac, channel } => format!("DAC{dac}  OUT{channel}"),
            PinFunction::I2sCk(n) => format!("I2S{n}  CK"),
            PinFunction::I2sWs(n) => format!("I2S{n}  WS"),
            PinFunction::I2sSd(n) => format!("I2S{n}  SD"),
            PinFunction::I2sMck(n) => format!("I2S{n}  MCK"),
            PinFunction::UsartTx(n) => format!("USART{n}  TX"),
            PinFunction::UsartRx(n) => format!("USART{n}  RX"),
            PinFunction::UsartCts(n) => format!("USART{n}  CTS"),
            PinFunction::UsartRts(n) => format!("USART{n}  RTS"),
            PinFunction::UsartCk(n) => format!("USART{n}  CK"),
            PinFunction::LpuartTx(n) => format!("LPUART{n}  TX"),
            PinFunction::LpuartRx(n) => format!("LPUART{n}  RX"),
            PinFunction::LpuartCts(n) => format!("LPUART{n}  CTS"),
            PinFunction::LpuartRts(n) => format!("LPUART{n}  RTS"),
            PinFunction::SpiNss(n) => format!("SPI{n}  NSS"),
            PinFunction::SpiSck(n) => format!("SPI{n}  SCK"),
            PinFunction::SpiMiso(n) => format!("SPI{n}  MISO"),
            PinFunction::SpiMosi(n) => format!("SPI{n}  MOSI"),
            PinFunction::SpiRdy(n) => format!("SPI{n}  RDY"),
            PinFunction::I2cScl(n) => format!("I2C{n}  SCL"),
            PinFunction::I2cSda(n) => format!("I2C{n}  SDA"),
            PinFunction::UsbDm => "USB  D−".into(),
            PinFunction::UsbDp => "USB  D+".into(),
            PinFunction::CanRx => "CAN  RX".into(),
            PinFunction::CanTx => "CAN  TX".into(),
            PinFunction::SwdIo => "SWD  IO  (JTMS)".into(),
            PinFunction::SwdClk => "SWD  CLK  (JTCK)".into(),
            PinFunction::Mco => "MCO  (Master Clock Out)".into(),
            // The datasheet's own signal name is the label.
            PinFunction::Other(name) => name.clone(),
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
            "GPIO Input" => return Some(PinFunction::GpioInput),
            "GPIO Output" => return Some(PinFunction::GpioOutput),
            "GPIO Analog" => return Some(PinFunction::GpioAnalog),
            "USB  D−" => return Some(PinFunction::UsbDm),
            "USB  D+" => return Some(PinFunction::UsbDp),
            "CAN  RX" => return Some(PinFunction::CanRx),
            "CAN  TX" => return Some(PinFunction::CanTx),
            "SWD  IO  (JTMS)" => return Some(PinFunction::SwdIo),
            "SWD  CLK  (JTCK)" => return Some(PinFunction::SwdClk),
            "MCO  (Master Clock Out)" => return Some(PinFunction::Mco),
            _ => {}
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
                        "TX" => Some(PinFunction::UsartTx(n)),
                        "RX" => Some(PinFunction::UsartRx(n)),
                        "CTS" => Some(PinFunction::UsartCts(n)),
                        "RTS" => Some(PinFunction::UsartRts(n)),
                        "CK" => Some(PinFunction::UsartCk(n)),
                        _ => None,
                    };
                }
            }
        }

        // ── SPI{n}  {role} ───────────────────────────────────────────────────
        if let Some(rest) = s.strip_prefix("SPI") {
            if let Some((num_str, role)) = rest.split_once("  ") {
                if let Ok(n) = num_str.parse::<u8>() {
                    return match role {
                        "NSS" => Some(PinFunction::SpiNss(n)),
                        "SCK" => Some(PinFunction::SpiSck(n)),
                        "MISO" => Some(PinFunction::SpiMiso(n)),
                        "MOSI" => Some(PinFunction::SpiMosi(n)),
                        "RDY" => Some(PinFunction::SpiRdy(n)),
                        _ => None,
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
                        _ => None,
                    };
                }
            }
        }

        // ── ADC{adc}  IN{channel} ────────────────────────────────────────────
        // label() → format!("ADC{adc}  IN{channel}")
        if let Some(rest) = s.strip_prefix("ADC") {
            // rest = "1  IN3"  →  split on "  IN"
            if let Some((adc_str, ch_str)) = rest.split_once("  IN") {
                if let (Ok(adc), Ok(channel)) = (adc_str.parse::<u8>(), ch_str.parse::<u8>()) {
                    return Some(PinFunction::AdcChannel { adc, channel });
                }
            }
        }

        // ── TIM{timer}  CH{channel}  (PWM) ───────────────────────────────────
        // label() → format!("TIM{timer}  CH{channel}  (PWM)")
        if let Some(rest) = s.strip_prefix("TIM") {
            // ── TIM{timer}  BKIN[2]  (break) ─────────────────────────────────
            if let Some((timer_str, tail)) = rest.split_once("  BKIN") {
                if let Ok(timer) = timer_str.parse::<u8>() {
                    let input = if tail.starts_with('2') { 2 } else { 1 };
                    return Some(PinFunction::TimerBreak { timer, input });
                }
            }
            // rest = "2  CH1  (PWM)"
            if let Some((timer_str, ch_rest)) = rest.split_once("  CH") {
                // ch_rest = "1  (PWM)" — take everything up to the next "  "
                let ch_num_str = ch_rest.split("  ").next().unwrap_or("");
                // A trailing N is the complementary output: "CH1N", not CH1.
                let (ch_num_str, complementary) = match ch_num_str.strip_suffix('N') {
                    Some(head) => (head, true),
                    None => (ch_num_str, false),
                };
                if let (Ok(timer), Ok(channel)) =
                    (timer_str.parse::<u8>(), ch_num_str.parse::<u8>())
                {
                    return Some(if complementary {
                        PinFunction::TimerPwmN { timer, channel }
                    } else {
                        PinFunction::TimerPwm { timer, channel }
                    });
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
            PinFunction::Unset => None,
            PinFunction::GpioInput => Some("Input"),
            PinFunction::GpioOutput => Some("Output"),
            PinFunction::AdcChannel { .. } => Some("Analog"),
            // Everything else is an alternate-function pin
            _ => Some("Alternate"),
        }
    }

    /// Short badge shown on the pin in the diagram
    /// The label for the function list inside the chip: the full name, with
    /// the short tag in front ONLY when it says something the full name does
    /// not.
    ///
    /// The list used to read `OUT  —  GPIO Output`, `SDA  —  I2C3 SDA`,
    /// `DCMIPP_D8  —  DCMIPP_D8`. The tag is the same word twice in almost
    /// every row, which costs a third of the width and buys nothing.
    ///
    /// "Says something new" is decided by SUBSEQUENCE, not equality: the tag
    /// is redundant when each of its letters and digits appears in the full
    /// name in the same order. Substring would have been the obvious test but
    /// it keeps `LPTX  —  LPUART1 TX`, where the tag is plainly redundant -
    /// its letters are all there, just not adjacent.
    ///
    /// With today's table every tag turns out redundant, so the list shows
    /// full names only. The rule stays anyway: it is what a NEW function is
    /// measured against, and a tag that does earn its place will show up
    /// without anyone having to notice.
    ///
    /// The subsequence test is loose — `OUT` is a subsequence of
    /// `GPIO Input` — and that is safe only because the two strings always
    /// come from the SAME variant, read off `self` here. Do not reuse
    /// [`tag_is_redundant`] to compare a tag with somebody else's name.
    pub fn list_label(&self) -> String {
        let long = self.label();
        if tag_is_redundant(self.short_label(), &long) {
            long
        } else {
            format!("{}  —  {}", self.short_label(), long)
        }
    }

    pub fn short_label(&self) -> &str {
        match self {
            PinFunction::Unset => "—",
            PinFunction::GpioInput => "IN",
            PinFunction::GpioOutput => "OUT",
            PinFunction::GpioAnalog => "ANA",
            PinFunction::AdcChannel { .. } => "ADC",
            // Both PWM shapes share a tag: the label already spells out which
            // channel, and CH1N reads as complementary on its own.
            PinFunction::TimerPwm { .. } | PinFunction::TimerPwmN { .. } => "PWM",
            PinFunction::TimerBreak { .. } => "BRK",
            PinFunction::DacOut { .. } => "DAC",
            PinFunction::I2sCk(_)
            | PinFunction::I2sWs(_)
            | PinFunction::I2sSd(_)
            | PinFunction::I2sMck(_) => "I2S",
            PinFunction::UsartTx(_) => "TX",
            PinFunction::UsartRx(_) => "RX",
            PinFunction::UsartCts(_) => "CTS",
            PinFunction::UsartRts(_) => "RTS",
            PinFunction::UsartCk(_) => "CK",
            PinFunction::LpuartTx(_) => "LPTX",
            PinFunction::LpuartRx(_) => "LPRX",
            PinFunction::LpuartCts(_) => "LPCTS",
            PinFunction::LpuartRts(_) => "LPRTS",
            PinFunction::SpiNss(_) => "NSS",
            PinFunction::SpiSck(_) => "SCK",
            PinFunction::SpiMiso(_) => "MISO",
            PinFunction::SpiMosi(_) => "MOSI",
            PinFunction::SpiRdy(_) => "RDY",
            PinFunction::I2cScl(_) => "SCL",
            PinFunction::I2cSda(_) => "SDA",
            PinFunction::UsbDm => "D−",
            PinFunction::UsbDp => "D+",
            PinFunction::CanRx => "CAN",
            PinFunction::CanTx => "CAN",
            PinFunction::SwdIo => "SWD",
            PinFunction::SwdClk => "SWD",
            PinFunction::Mco => "MCO",
            PinFunction::Other(name) => name,
        }
    }
}

/// Is `tag` already spelled out inside `long`?
///
/// Letters and digits only, case-insensitive, in order but not necessarily
/// adjacent. Punctuation and spacing are ignored on both sides, so `D−`
/// against `USB  D−` and `PWM` against `TIM4  CH3  (PWM)` both count.
/// An empty tag (the placeholder for an unset pin) is redundant by
/// definition.
fn tag_is_redundant(tag: &str, long: &str) -> bool {
    let keep = |s: &str| -> Vec<char> {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    let (tag, long) = (keep(tag), keep(long));
    let mut it = long.into_iter();
    tag.into_iter().all(|c| it.any(|l| l == c))
}

#[cfg(test)]
mod list_label_tests {
    use super::super::enum_::PinFunction;
    use super::tag_is_redundant;

    /// The rows from the report, each losing a tag that repeated the name.
    #[test]
    fn the_redundant_tags_are_gone() {
        let cases = [
            (PinFunction::GpioOutput, "GPIO Output"),
            (PinFunction::GpioInput, "GPIO Input"),
            (PinFunction::GpioAnalog, "GPIO Analog"),
            (PinFunction::I2cSda(3), "I2C3  SDA"),
            (PinFunction::UsartRx(1), "USART1  RX"),
            (PinFunction::UsartCts(8), "USART8  CTS"),
            (PinFunction::Other("DCMIPP_D8".into()), "DCMIPP_D8"),
        ];
        for (f, want) in cases {
            assert_eq!(f.list_label(), want, "{f:?}");
        }
    }

    /// Two the substring rule would have kept: the tag is spelled out, just
    /// not contiguously.
    #[test]
    fn a_split_tag_counts_as_redundant_too() {
        assert_eq!(
            PinFunction::TimerPwm {
                timer: 4,
                channel: 3
            }
            .list_label(),
            "TIM4  CH3  (PWM)"
        );
        assert_eq!(PinFunction::LpuartTx(1).list_label(), "LPUART1  TX");
    }

    /// Every variant the enum has today is redundant, so nothing in the list
    /// carries a tag. Pinned so that adding a function with a tag that DOES
    /// earn its place shows up here as a deliberate change rather than a
    /// surprise in the UI.
    #[test]
    fn nothing_in_the_current_table_needs_its_tag() {
        let all = [
            PinFunction::Unset,
            PinFunction::GpioInput,
            PinFunction::GpioOutput,
            PinFunction::GpioAnalog,
            PinFunction::AdcChannel { adc: 1, channel: 0 },
            PinFunction::TimerPwm {
                timer: 2,
                channel: 1,
            },
            PinFunction::TimerPwmN {
                timer: 1,
                channel: 1,
            },
            PinFunction::UsartTx(1),
            PinFunction::UsartRx(1),
            PinFunction::UsartCts(1),
            PinFunction::UsartRts(1),
            PinFunction::UsartCk(1),
            PinFunction::LpuartTx(1),
            PinFunction::LpuartRx(1),
            PinFunction::LpuartCts(1),
            PinFunction::LpuartRts(1),
            PinFunction::SpiNss(1),
            PinFunction::SpiSck(1),
            PinFunction::SpiMiso(1),
            PinFunction::SpiMosi(1),
            PinFunction::SpiRdy(1),
            PinFunction::I2cScl(1),
            PinFunction::I2cSda(1),
            PinFunction::UsbDm,
            PinFunction::UsbDp,
            PinFunction::CanRx,
            PinFunction::CanTx,
            PinFunction::SwdIo,
            PinFunction::SwdClk,
            PinFunction::Mco,
        ];
        for f in all {
            assert_eq!(f.list_label(), f.label(), "{f:?} still carries a tag");
        }
    }

    /// …and the predicate itself refuses a tag that is genuinely new.
    #[test]
    fn a_tag_that_adds_something_survives() {
        assert!(!tag_is_redundant("QSPI", "OCTOSPI1  IO0"));
        // Order matters: the same letters the wrong way round is not a match.
        assert!(!tag_is_redundant("XT", "USART1  TX"));
    }

    /// The rule is LOOSE, and knowing where is the point of this test.
    #[test]
    fn scattered_letters_can_match_by_accident() {
        // "OUT" is a subsequence of "GPIO Input" - o, u, t all appear in
        // order. A mismatched pair like this would be judged redundant.
        assert!(tag_is_redundant("OUT", "GPIO Input"));
        // It never bites, because a tag is only ever compared with the name
        // of the SAME function: `list_label` reads both off `self`. Recording
        // it so nobody reuses the predicate somewhere it could.
    }
}
