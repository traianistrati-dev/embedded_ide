//! Clock-tree configuration state.
//!
//! `ClockConfig` is an extensible, per-family enum so other MCUs (ESP32-C3,
//! STM8, …) can grow their own clock models later.  Only STM32F1 is modelled
//! for now; everything else carries `ClockConfig::None`.
//!
//! `Stm32f1Clock` mirrors the configurable nodes of the STM32F103 clock tree
//! (datasheet Figure 2): the HSE crystal, the PLL source/multiplier, the SYSCLK
//! source mux, and the AHB / APB1 / APB2 / ADC / USB prescalers.  Derived
//! frequencies are computed in `compute.rs`, not stored here.

// ── Internal oscillator constants ─────────────────────────────────────────────

/// High-speed internal RC oscillator — fixed 8 MHz on STM32F1.
pub const HSI_HZ: u32 = 8_000_000;

/// Valid HSE crystal range (datasheet: 4–16 MHz).
pub const HSE_MIN_HZ: u32 = 4_000_000;
pub const HSE_MAX_HZ: u32 = 16_000_000;

// ── Selectable node options (for UI dropdowns) ────────────────────────────────

/// SYSCLK source — the `SW` mux in Figure 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SysclkSrc {
    Hsi,
    Hse,
    Pll,
}

/// PLL input — combines the `PLLSRC` mux and the `PLLXTPRE` (/1 or /2) divider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PllSrc {
    /// HSI / 2
    HsiDiv2,
    /// HSE (PLLXTPRE = /1)
    Hse,
    /// HSE / 2 (PLLXTPRE = /2)
    HseDiv2,
}

/// USB prescaler — the `USB Prescaler /1, 1.5` block. USBCLK must equal 48 MHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbPre {
    /// USBCLK = PLLCLK / 1.5  (the usual choice for a 72 MHz PLL)
    Div1_5,
    /// USBCLK = PLLCLK / 1     (for a 48 MHz PLL)
    Div1,
}

/// Main Clock Output (MCO) selection — the bottom-left mux in Figure 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mco {
    None,
    Sysclk,
    Hsi,
    Hse,
    /// PLLCLK / 2
    PllDiv2,
}

/// RTC clock source — the `RTCSEL` / RTC Clock Mux in Figure 2.
/// UI/diagram only (the stm32f1xx-hal `freeze` chain doesn't configure RTC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtcSrc {
    None,
    /// HSE / 128
    HseDiv128,
    Lse,
    Lsi,
}

/// Cortex SysTick source — `to Cortex System timer` in Figure 2.
/// UI/diagram only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystickSrc {
    /// HCLK / 8 (datasheet default)
    HclkDiv8,
    /// HCLK (no division)
    Hclk,
}

// ── Allowed prescaler value tables ────────────────────────────────────────────

/// AHB prescaler (HPRE) divider options.
pub const AHB_PRESCALERS: &[u16] = &[1, 2, 4, 8, 16, 64, 128, 256, 512];
/// APB1 / APB2 prescaler (PPRE1 / PPRE2) divider options.
pub const APB_PRESCALERS: &[u8] = &[1, 2, 4, 8, 16];
/// ADC prescaler divider options.
pub const ADC_PRESCALERS: &[u8] = &[2, 4, 6, 8];
/// PLL multiplier range (PLLMUL).
pub const PLL_MUL_MIN: u8 = 2;
pub const PLL_MUL_MAX: u8 = 16;

// ── STM32F1 clock configuration ───────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Stm32f1Clock {
    /// External crystal frequency in Hz (only meaningful when `hse_enabled`).
    pub hse_hz: u32,
    /// Whether the HSE oscillator is enabled.
    pub hse_enabled: bool,
    /// SYSCLK source (HSI / HSE / PLL).
    pub sysclk_src: SysclkSrc,
    /// PLL input selection (HSI/2, HSE, HSE/2).
    pub pll_src: PllSrc,
    /// PLL multiplier (2..=16).
    pub pll_mul: u8,
    /// AHB prescaler → HCLK.
    pub ahb_pre: u16,
    /// APB1 prescaler → PCLK1 (max 36 MHz).
    pub apb1_pre: u8,
    /// APB2 prescaler → PCLK2 (max 72 MHz).
    pub apb2_pre: u8,
    /// ADC prescaler (divides PCLK2) → ADCCLK (max 14 MHz).
    pub adc_pre: u8,
    /// USB prescaler → USBCLK (must be 48 MHz when USB is used).
    pub usb_pre: UsbPre,
    /// Main clock output selection.
    pub mco: Mco,
    /// RTC clock source (diagram only — not emitted in the HAL freeze chain).
    pub rtc_src: RtcSrc,
    /// Cortex SysTick source (diagram only).
    pub systick_src: SystickSrc,
    /// Clock Security System enable (diagram only).
    pub css_on: bool,
}

impl Default for Stm32f1Clock {
    /// The classic "Blue Pill" 72 MHz configuration — identical in effect to the
    /// previously-hardcoded `use_hse(8).sysclk(72).pclk1(36)` chain:
    /// HSE 8 MHz → PLL ×9 → SYSCLK 72 MHz, APB1 /2 (36), APB2 /1 (72),
    /// ADC /6 (12 MHz), USB /1.5 (48 MHz).
    fn default() -> Self {
        Self {
            hse_hz: 8_000_000,
            hse_enabled: true,
            sysclk_src: SysclkSrc::Pll,
            pll_src: PllSrc::Hse,
            pll_mul: 9,
            ahb_pre: 1,
            apb1_pre: 2,
            apb2_pre: 1,
            adc_pre: 6,
            usb_pre: UsbPre::Div1_5,
            mco: Mco::None,
            rtc_src: RtcSrc::Lsi,
            systick_src: SystickSrc::HclkDiv8,
            css_on: false,
        }
    }
}

// ── Extensible per-MCU wrapper ────────────────────────────────────────────────

/// Per-MCU clock configuration. Extend with more variants as families gain
/// their own clock models.
#[derive(Clone, Debug, PartialEq)]
pub enum ClockConfig {
    Stm32f1(Stm32f1Clock),
    /// No modelled clock tree yet (ESP32-C3, STM8, …).
    None,
}

impl ClockConfig {
    /// Borrow the STM32F1 config, if this is one.
    pub fn as_stm32f1(&self) -> Option<&Stm32f1Clock> {
        match self {
            ClockConfig::Stm32f1(c) => Some(c),
            ClockConfig::None => None,
        }
    }

    /// Mutably borrow the STM32F1 config, if this is one.
    pub fn as_stm32f1_mut(&mut self) -> Option<&mut Stm32f1Clock> {
        match self {
            ClockConfig::Stm32f1(c) => Some(c),
            ClockConfig::None => None,
        }
    }
}
