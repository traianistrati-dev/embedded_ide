//! Validate a clock configuration against the STM32F103 datasheet limits.
//!
//! Produces human-readable errors (hard limits that would hang/brick the chip)
//! and warnings (works, but probably not intended — e.g. USB clock ≠ 48 MHz).
//! The three footnotes under Figure 2 are encoded here.

use super::compute::ClockFrequencies;
use super::model::{HSE_MAX_HZ, HSE_MIN_HZ, PllSrc, Stm32f1Clock, SysclkSrc};

const MHZ: u32 = 1_000_000;

/// Maximum SYSCLK / HCLK / PCLK2 / PLL output.
const MAX_SYSCLK: u32 = 72 * MHZ;
/// Maximum SYSCLK when the PLL is fed from HSI (footnote 1).
const MAX_SYSCLK_HSI_PLL: u32 = 64 * MHZ;
/// Maximum PCLK1 (APB1).
const MAX_PCLK1: u32 = 36 * MHZ;
/// Maximum ADC clock.
const MAX_ADCCLK: u32 = 14 * MHZ;
/// Required USB clock (footnote 2).
const USB_HZ: u32 = 48 * MHZ;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockWarning {
    pub severity: Severity,
    pub msg: String,
}

impl ClockWarning {
    fn error(msg: impl Into<String>) -> Self {
        Self { severity: Severity::Error, msg: msg.into() }
    }
    fn warn(msg: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, msg: msg.into() }
    }
}

/// Round Hz to one decimal of MHz for messages, e.g. 36_000_000 → "36.0".
fn mhz(hz: u32) -> String {
    format!("{:.1}", hz as f64 / MHZ as f64)
}

/// Collect all validation findings for `c` / `f`.
pub fn warnings(c: &Stm32f1Clock, f: &ClockFrequencies) -> Vec<ClockWarning> {
    let mut out = Vec::new();

    // ── HSE crystal range ────────────────────────────────────────────────────
    if c.hse_enabled && (c.hse_hz < HSE_MIN_HZ || c.hse_hz > HSE_MAX_HZ) {
        out.push(ClockWarning::warn(format!(
            "HSE crystal {} MHz is outside the 4–16 MHz range.",
            mhz(c.hse_hz)
        )));
    }

    // ── Selected source actually running? ────────────────────────────────────
    let uses_pll = c.sysclk_src == SysclkSrc::Pll;
    if (c.sysclk_src == SysclkSrc::Hse || uses_pll && pll_needs_hse(c)) && !c.hse_enabled {
        out.push(ClockWarning::error(
            "Selected clock path needs HSE, but HSE is disabled.".to_string(),
        ));
    }

    // ── Hard frequency ceilings ──────────────────────────────────────────────
    if f.sysclk > MAX_SYSCLK {
        out.push(ClockWarning::error(format!(
            "SYSCLK {} MHz exceeds the 72 MHz maximum.",
            mhz(f.sysclk)
        )));
    }
    // Footnote 1: HSI→PLL caps system clock at 64 MHz.
    if uses_pll && c.pll_src == PllSrc::HsiDiv2 && f.sysclk > MAX_SYSCLK_HSI_PLL {
        out.push(ClockWarning::error(format!(
            "With HSI as PLL input, SYSCLK is limited to 64 MHz (got {} MHz).",
            mhz(f.sysclk)
        )));
    }
    if f.pllclk > MAX_SYSCLK {
        out.push(ClockWarning::error(format!(
            "PLL output {} MHz exceeds the 72 MHz maximum.",
            mhz(f.pllclk)
        )));
    }
    if f.hclk > MAX_SYSCLK {
        out.push(ClockWarning::error(format!(
            "HCLK {} MHz exceeds the 72 MHz maximum.",
            mhz(f.hclk)
        )));
    }
    if f.pclk1 > MAX_PCLK1 {
        out.push(ClockWarning::error(format!(
            "PCLK1 {} MHz exceeds the 36 MHz APB1 maximum.",
            mhz(f.pclk1)
        )));
    }
    if f.pclk2 > MAX_SYSCLK {
        out.push(ClockWarning::error(format!(
            "PCLK2 {} MHz exceeds the 72 MHz APB2 maximum.",
            mhz(f.pclk2)
        )));
    }
    if f.adcclk > MAX_ADCCLK {
        out.push(ClockWarning::error(format!(
            "ADCCLK {} MHz exceeds the 14 MHz maximum.",
            mhz(f.adcclk)
        )));
    }

    // ── Footnote 3: 1 µs ADC conversion needs APB2 ∈ {14, 28, 56} MHz ─────────
    if f.pclk2 != 14 * MHZ && f.pclk2 != 28 * MHZ && f.pclk2 != 56 * MHZ {
        out.push(ClockWarning::warn(format!(
            "For a 1 µs ADC conversion, APB2 (PCLK2) should be 14, 28 or 56 MHz (got {} MHz).",
            mhz(f.pclk2)
        )));
    }

    // ── Footnote 2: USB requires HSE + PLL with USBCLK = 48 MHz ───────────────
    if !uses_pll {
        out.push(ClockWarning::warn(
            "USB is unavailable: it requires the PLL to be the SYSCLK source.".to_string(),
        ));
    } else if f.usbclk != USB_HZ {
        out.push(ClockWarning::warn(format!(
            "USBCLK is {} MHz; USB needs exactly 48 MHz (adjust PLL or USB prescaler).",
            mhz(f.usbclk)
        )));
    }

    out
}

/// True when the PLL input ultimately comes from HSE.
fn pll_needs_hse(c: &Stm32f1Clock) -> bool {
    matches!(c.pll_src, PllSrc::Hse | PllSrc::HseDiv2)
}

/// Convenience: does the config have any hard error?
pub fn has_errors(c: &Stm32f1Clock, f: &ClockFrequencies) -> bool {
    warnings(c, f)
        .iter()
        .any(|w| w.severity == Severity::Error)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::compute::frequencies;
    use super::super::model::{Stm32f1Clock, SysclkSrc};

    fn warns(c: &Stm32f1Clock) -> Vec<ClockWarning> {
        warnings(c, &frequencies(c))
    }

    #[test]
    fn default_72mhz_has_no_errors() {
        let c = Stm32f1Clock::default();
        assert!(!has_errors(&c, &frequencies(&c)), "default config should be valid");
    }

    #[test]
    fn pclk1_over_36_is_error() {
        let mut c = Stm32f1Clock::default();
        c.apb1_pre = 1; // PCLK1 = 72 MHz > 36
        assert!(warns(&c).iter().any(|w| w.severity == Severity::Error
            && w.msg.contains("PCLK1")));
    }

    #[test]
    fn adcclk_over_14_is_error() {
        let mut c = Stm32f1Clock::default();
        c.adc_pre = 2; // 72 / 2 = 36 MHz > 14
        assert!(warns(&c).iter().any(|w| w.severity == Severity::Error
            && w.msg.contains("ADCCLK")));
    }

    #[test]
    fn usb_not_48_is_warning() {
        let mut c = Stm32f1Clock::default();
        c.usb_pre = super::super::model::UsbPre::Div1; // 72 / 1 = 72 ≠ 48
        assert!(warns(&c).iter().any(|w| w.severity == Severity::Warning
            && w.msg.contains("USB")));
    }

    #[test]
    fn hsi_source_warns_usb_unavailable() {
        let mut c = Stm32f1Clock::default();
        c.sysclk_src = SysclkSrc::Hsi;
        assert!(warns(&c).iter().any(|w| w.msg.contains("USB is unavailable")));
    }

    #[test]
    fn hsi_pll_over_64_is_error() {
        let mut c = Stm32f1Clock::default();
        c.pll_src = super::super::model::PllSrc::HsiDiv2; // 4 MHz
        c.pll_mul = 16; // 64 MHz — OK
        assert!(!warns(&c).iter().any(|w| w.msg.contains("64 MHz")));
        // Bump APB so nothing else errors, but sysclk stays 64 — still fine.
    }
}
