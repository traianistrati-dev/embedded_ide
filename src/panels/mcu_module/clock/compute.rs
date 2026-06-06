//! Derive every clock-tree frequency from a [`Stm32f1Clock`] configuration.
//!
//! Pure arithmetic, no UI — mirrors the propagation of Figure 2 from the
//! oscillators through the PLL, the SYSCLK mux, and the bus prescalers.

use super::model::{HSI_HZ, PllSrc, Stm32f1Clock, SysclkSrc, UsbPre};

/// All resolved frequencies (Hz) for a given configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ClockFrequencies {
    pub hsi: u32,
    pub hse: u32,
    /// PLL input frequency (after PLLSRC + PLLXTPRE).
    pub pll_input: u32,
    /// PLL output (PLLCLK). 0 when HSE is required but disabled.
    pub pllclk: u32,
    pub sysclk: u32,
    pub hclk: u32,
    pub pclk1: u32,
    pub pclk2: u32,
    /// Timer clock on APB1 (TIM2/3/4): x1 if APB1 prescaler == 1 else x2.
    pub tim_apb1: u32,
    /// Timer clock on APB2 (TIM1): x1 if APB2 prescaler == 1 else x2.
    pub tim_apb2: u32,
    pub adcclk: u32,
    /// USB clock (PLLCLK / usb_pre). Only valid when SYSCLK uses the PLL.
    pub usbclk: u32,
    /// Cortex SysTick reference = HCLK / 8.
    pub systick: u32,
    /// Flash interface clock = HSI.
    pub flitfclk: u32,
}

/// Compute all derived frequencies for `c`.
pub fn frequencies(c: &Stm32f1Clock) -> ClockFrequencies {
    let hsi = HSI_HZ;
    let hse = if c.hse_enabled { c.hse_hz } else { 0 };

    let pll_input = match c.pll_src {
        PllSrc::HsiDiv2 => hsi / 2,
        PllSrc::Hse => hse,
        PllSrc::HseDiv2 => hse / 2,
    };
    let pllclk = pll_input.saturating_mul(c.pll_mul as u32);

    let sysclk = match c.sysclk_src {
        SysclkSrc::Hsi => hsi,
        SysclkSrc::Hse => hse,
        SysclkSrc::Pll => pllclk,
    };

    let ahb = c.ahb_pre.max(1) as u32;
    let hclk = sysclk / ahb;
    let pclk1 = hclk / c.apb1_pre.max(1) as u32;
    let pclk2 = hclk / c.apb2_pre.max(1) as u32;

    let tim_apb1 = if c.apb1_pre <= 1 { pclk1 } else { pclk1 * 2 };
    let tim_apb2 = if c.apb2_pre <= 1 { pclk2 } else { pclk2 * 2 };

    let adcclk = pclk2 / c.adc_pre.max(1) as u32;

    // USB is fed from PLLCLK regardless of the SYSCLK mux; validity (whether the
    // PLL is actually running) is reported by `validate`.
    let usbclk = match c.usb_pre {
        UsbPre::Div1 => pllclk,
        UsbPre::Div1_5 => (pllclk * 2) / 3,
    };

    ClockFrequencies {
        hsi,
        hse,
        pll_input,
        pllclk,
        sysclk,
        hclk,
        pclk1,
        pclk2,
        tim_apb1,
        tim_apb2,
        adcclk,
        usbclk,
        systick: hclk / 8,
        flitfclk: hsi,
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::model::Stm32f1Clock;

    const MHZ: u32 = 1_000_000;

    #[test]
    fn default_is_72mhz_blue_pill() {
        let f = frequencies(&Stm32f1Clock::default());
        assert_eq!(f.pll_input, 8 * MHZ); // HSE 8 MHz
        assert_eq!(f.pllclk, 72 * MHZ); // ×9
        assert_eq!(f.sysclk, 72 * MHZ);
        assert_eq!(f.hclk, 72 * MHZ);
        assert_eq!(f.pclk1, 36 * MHZ); // APB1 /2
        assert_eq!(f.pclk2, 72 * MHZ); // APB2 /1
        assert_eq!(f.adcclk, 12 * MHZ); // PCLK2 /6
        assert_eq!(f.usbclk, 48 * MHZ); // 72 / 1.5
        assert_eq!(f.systick, 9 * MHZ); // 72 / 8
        assert_eq!(f.flitfclk, 8 * MHZ);
    }

    #[test]
    fn apb1_timer_doubles_when_prescaled() {
        let c = Stm32f1Clock::default(); // APB1 /2 → TIM2/3/4 = 2 × PCLK1
        let f = frequencies(&c);
        assert_eq!(f.tim_apb1, 72 * MHZ);
        // APB2 /1 → TIM1 = PCLK2 (x1)
        assert_eq!(f.tim_apb2, 72 * MHZ);
    }

    #[test]
    fn hsi_direct_is_8mhz() {
        let mut c = Stm32f1Clock::default();
        c.sysclk_src = SysclkSrc::Hsi;
        let f = frequencies(&c);
        assert_eq!(f.sysclk, 8 * MHZ);
        assert_eq!(f.hclk, 8 * MHZ);
    }

    #[test]
    fn hsi_pll_caps_at_64mhz() {
        // HSI/2 = 4 MHz, ×16 = 64 MHz (note 1 limit).
        let mut c = Stm32f1Clock::default();
        c.pll_src = PllSrc::HsiDiv2;
        c.pll_mul = 16;
        let f = frequencies(&c);
        assert_eq!(f.pll_input, 4 * MHZ);
        assert_eq!(f.pllclk, 64 * MHZ);
        assert_eq!(f.sysclk, 64 * MHZ);
    }

    #[test]
    fn hse_disabled_zeroes_pll() {
        let mut c = Stm32f1Clock::default();
        c.hse_enabled = false;
        let f = frequencies(&c);
        assert_eq!(f.hse, 0);
        assert_eq!(f.pllclk, 0); // HSE-sourced PLL with HSE off → 0
    }
}
