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
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SysclkSrc {
    Hsi,
    Hse,
    Pll,
}

/// PLL input — combines the `PLLSRC` mux and the `PLLXTPRE` (/1 or /2) divider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PllSrc {
    /// HSI / 2
    HsiDiv2,
    /// HSE (PLLXTPRE = /1)
    Hse,
    /// HSE / 2 (PLLXTPRE = /2)
    HseDiv2,
}

/// USB prescaler — the `USB Prescaler /1, 1.5` block. USBCLK must equal 48 MHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UsbPre {
    /// USBCLK = PLLCLK / 1.5  (the usual choice for a 72 MHz PLL)
    Div1_5,
    /// USBCLK = PLLCLK / 1     (for a 48 MHz PLL)
    Div1,
}

/// Main Clock Output (MCO) selection — the bottom-left mux in Figure 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RtcSrc {
    None,
    /// HSE / 128
    HseDiv128,
    Lse,
    Lsi,
}

/// Cortex SysTick source — `to Cortex System timer` in Figure 2.
/// UI/diagram only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

// ── Per-chip datasheet limits ─────────────────────────────────────────────────

/// Datasheet frequency ceilings for one chip — consumed by `validate`, the
/// frequency table and the diagram's red over-limit tags.
///
/// Defaults are the STM32F103 (performance line) Figure-2 values. Imported
/// `.ron` definitions may override any subset via the `clock_limits` field
/// (e.g. a 24 MHz value-line part); omitted fields keep the F103 default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ClockLimits {
    /// Maximum SYSCLK / PLL output.
    pub sysclk_max: u32,
    /// Maximum SYSCLK when the PLL is fed from HSI (datasheet footnote 1).
    pub sysclk_max_hsi_pll: u32,
    /// Maximum HCLK (AHB / core).
    pub hclk_max: u32,
    /// Maximum PCLK1 (APB1).
    pub pclk1_max: u32,
    /// Maximum PCLK2 (APB2).
    pub pclk2_max: u32,
    /// Maximum ADC clock.
    pub adcclk_max: u32,
    /// Exact USB clock requirement (footnote 2).
    pub usbclk_hz: u32,
    /// Valid HSE crystal range.
    pub hse_min_hz: u32,
    pub hse_max_hz: u32,
}

impl Default for ClockLimits {
    /// STM32F103 datasheet limits (Figure 2 + footnotes).
    fn default() -> Self {
        Self {
            sysclk_max: 72_000_000,
            sysclk_max_hsi_pll: 64_000_000,
            hclk_max: 72_000_000,
            pclk1_max: 36_000_000,
            pclk2_max: 72_000_000,
            adcclk_max: 14_000_000,
            usbclk_hz: 48_000_000,
            hse_min_hz: HSE_MIN_HZ,
            hse_max_hz: HSE_MAX_HZ,
        }
    }
}

// ── STM32F1 clock configuration ───────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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

/// Per-MCU clock configuration.
///
/// The data-driven [`graph`](super::graph) is the ONLY runtime clock model —
/// the old typed `Stm32f1(Stm32f1Clock)` variant was retired. `Stm32f1Clock`
/// itself survives as (a) the compact authoring format in `.ron`
/// ([`ClockDef::Stm32f1`](crate::panels::mcu_module::mcu_def::ClockDef) is
/// auto-upgraded to a graph at load), (b) the codegen intermediate
/// (`graph_to_stm32f1` → `rcc.cfgr` chain), and (c) the `@clock` persist format.
#[derive(Clone, Debug, PartialEq)]
pub enum ClockConfig {
    /// Data-driven clock tree + diagram (from the chip's `.ron` or built-in).
    Graph(super::graph::GraphClock),
    /// No modelled clock tree yet (ESP32-C3 built-in, STM8, …).
    None,
}
