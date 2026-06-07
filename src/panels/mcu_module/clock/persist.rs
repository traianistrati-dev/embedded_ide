//! Exact serialise / parse of a [`Stm32f1Clock`] to a one-line comment.
//!
//! The generated `rcc.cfgr` chain expresses *target frequencies*, from which the
//! original mux/prescaler choices can't be recovered unambiguously.  So we also
//! emit a compact, machine-readable marker line inside the GEN block and parse
//! it back on project open — giving an exact, lossless round-trip of the Clock
//! tab state (mirroring how pins are restored from `main.rs`).

use super::model::{Mco, PllSrc, RtcSrc, Stm32f1Clock, SysclkSrc, SystickSrc, UsbPre};

/// Marker prefix for the clock config comment line.
pub const CLOCK_TAG: &str = "// @clock";

/// Serialise to a single comment line, e.g.
/// `// @clock hse=8000000 hse_on=1 src=pll pll=hse mul=9 ahb=1 apb1=2 apb2=1 adc=6 usb=1_5 mco=none`
pub fn to_comment(c: &Stm32f1Clock) -> String {
    let src = match c.sysclk_src {
        SysclkSrc::Hsi => "hsi",
        SysclkSrc::Hse => "hse",
        SysclkSrc::Pll => "pll",
    };
    let pll = match c.pll_src {
        PllSrc::HsiDiv2 => "hsi2",
        PllSrc::Hse => "hse",
        PllSrc::HseDiv2 => "hse2",
    };
    let usb = match c.usb_pre {
        UsbPre::Div1_5 => "1_5",
        UsbPre::Div1 => "1",
    };
    let mco = match c.mco {
        Mco::None => "none",
        Mco::Sysclk => "sysclk",
        Mco::Hsi => "hsi",
        Mco::Hse => "hse",
        Mco::PllDiv2 => "pll2",
    };
    let rtc = match c.rtc_src {
        RtcSrc::None => "none",
        RtcSrc::HseDiv128 => "hse128",
        RtcSrc::Lse => "lse",
        RtcSrc::Lsi => "lsi",
    };
    let systick = match c.systick_src {
        SystickSrc::HclkDiv8 => "hclk8",
        SystickSrc::Hclk => "hclk",
    };
    format!(
        "{CLOCK_TAG} hse={} hse_on={} src={src} pll={pll} mul={} ahb={} apb1={} apb2={} adc={} usb={usb} mco={mco} rtc={rtc} systick={systick} css={}",
        c.hse_hz,
        if c.hse_enabled { 1 } else { 0 },
        c.pll_mul,
        c.ahb_pre,
        c.apb1_pre,
        c.apb2_pre,
        c.adc_pre,
        if c.css_on { 1 } else { 0 },
    )
}

/// Parse a `// @clock …` line back into a config. Returns `None` if `line`
/// isn't a clock marker; unknown / missing fields fall back to the default.
pub fn from_comment(line: &str) -> Option<Stm32f1Clock> {
    let rest = line.trim().strip_prefix(CLOCK_TAG)?;
    let mut c = Stm32f1Clock::default();
    for tok in rest.split_whitespace() {
        let Some((k, v)) = tok.split_once('=') else {
            continue;
        };
        match k {
            "hse" => {
                if let Ok(n) = v.parse() {
                    c.hse_hz = n;
                }
            }
            "hse_on" => c.hse_enabled = v == "1",
            "src" => {
                c.sysclk_src = match v {
                    "hsi" => SysclkSrc::Hsi,
                    "hse" => SysclkSrc::Hse,
                    _ => SysclkSrc::Pll,
                }
            }
            "pll" => {
                c.pll_src = match v {
                    "hsi2" => PllSrc::HsiDiv2,
                    "hse2" => PllSrc::HseDiv2,
                    _ => PllSrc::Hse,
                }
            }
            "mul" => {
                if let Ok(n) = v.parse() {
                    c.pll_mul = n;
                }
            }
            "ahb" => {
                if let Ok(n) = v.parse() {
                    c.ahb_pre = n;
                }
            }
            "apb1" => {
                if let Ok(n) = v.parse() {
                    c.apb1_pre = n;
                }
            }
            "apb2" => {
                if let Ok(n) = v.parse() {
                    c.apb2_pre = n;
                }
            }
            "adc" => {
                if let Ok(n) = v.parse() {
                    c.adc_pre = n;
                }
            }
            "usb" => {
                c.usb_pre = match v {
                    "1" => UsbPre::Div1,
                    _ => UsbPre::Div1_5,
                }
            }
            "mco" => {
                c.mco = match v {
                    "sysclk" => Mco::Sysclk,
                    "hsi" => Mco::Hsi,
                    "hse" => Mco::Hse,
                    "pll2" => Mco::PllDiv2,
                    _ => Mco::None,
                }
            }
            "rtc" => {
                c.rtc_src = match v {
                    "hse128" => RtcSrc::HseDiv128,
                    "lse" => RtcSrc::Lse,
                    "lsi" => RtcSrc::Lsi,
                    _ => RtcSrc::None,
                }
            }
            "systick" => {
                c.systick_src = match v {
                    "hclk" => SystickSrc::Hclk,
                    _ => SystickSrc::HclkDiv8,
                }
            }
            "css" => c.css_on = v == "1",
            _ => {}
        }
    }
    Some(c)
}

/// Scan an entire `main.rs` for the clock marker line and parse it.
pub fn parse_from_source(source: &str) -> Option<Stm32f1Clock> {
    source
        .lines()
        .find(|l| l.trim_start().starts_with(CLOCK_TAG))
        .and_then(from_comment)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_default() {
        let c = Stm32f1Clock::default();
        let line = to_comment(&c);
        assert_eq!(from_comment(&line), Some(c));
    }

    #[test]
    fn round_trips_custom() {
        let c = Stm32f1Clock {
            hse_hz: 12_000_000,
            hse_enabled: false,
            sysclk_src: SysclkSrc::Hsi,
            pll_src: PllSrc::HsiDiv2,
            pll_mul: 16,
            ahb_pre: 2,
            apb1_pre: 4,
            apb2_pre: 2,
            adc_pre: 8,
            usb_pre: UsbPre::Div1,
            mco: Mco::PllDiv2,
            rtc_src: RtcSrc::Lse,
            systick_src: SystickSrc::Hclk,
            css_on: true,
        };
        let line = to_comment(&c);
        assert_eq!(from_comment(&line), Some(c));
    }

    #[test]
    fn finds_marker_in_source() {
        let c = Stm32f1Clock::default();
        let src = format!("// header\n{}\nfn main() {{}}", to_comment(&c));
        assert_eq!(parse_from_source(&src), Some(c));
    }

    #[test]
    fn non_marker_returns_none() {
        assert_eq!(from_comment("let x = 5;"), None);
        assert_eq!(parse_from_source("no markers here"), None);
    }
}
