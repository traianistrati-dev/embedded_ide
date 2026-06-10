//! Named clock presets — one-click sane configurations for the Clock tab.
//!
//! [`ClockPreset`] is also the runtime form of chip-specific presets imported
//! from a `.ron` definition (`clock_presets`); [`stm32f1_presets`] is the
//! built-in family list used when a chip ships none of its own.

use super::model::{PllSrc, Stm32f1Clock, SysclkSrc, UsbPre};

/// A labelled clock configuration the user can apply with one click.
#[derive(Clone, Debug, PartialEq)]
pub struct ClockPreset {
    pub name: String,
    pub description: String,
    pub config: Stm32f1Clock,
}

/// Built-in STM32F1 presets, in display order — the fallback when the chip
/// definition declares no `clock_presets`.
pub fn stm32f1_presets() -> Vec<ClockPreset> {
    vec![
        ClockPreset {
            name: "72 MHz (HSE 8 + PLL×9)".to_owned(),
            description: "Max performance. SYSCLK 72, PCLK1 36, USB 48 MHz.".to_owned(),
            config: Stm32f1Clock::default(),
        },
        ClockPreset {
            name: "HSE 8 MHz direct".to_owned(),
            description: "Crystal straight to SYSCLK, PLL off. Low power, no USB.".to_owned(),
            config: Stm32f1Clock {
                sysclk_src: SysclkSrc::Hse,
                ..Stm32f1Clock::default()
            },
        },
        ClockPreset {
            name: "HSI 8 MHz".to_owned(),
            description: "Internal RC only — no crystal needed. No USB.".to_owned(),
            config: Stm32f1Clock {
                hse_enabled: false,
                sysclk_src: SysclkSrc::Hsi,
                ..Stm32f1Clock::default()
            },
        },
        ClockPreset {
            name: "HSI→PLL 64 MHz".to_owned(),
            description: "No crystal. HSI/2 ×16 = 64 MHz (footnote-1 max). No USB.".to_owned(),
            config: Stm32f1Clock {
                hse_enabled: false,
                sysclk_src: SysclkSrc::Pll,
                pll_src: PllSrc::HsiDiv2,
                pll_mul: 16,
                ahb_pre: 1,
                apb1_pre: 2, // 64/2 = 32 ≤ 36
                apb2_pre: 1, // 64
                adc_pre: 6,  // 64/6 ≈ 10.6 ≤ 14
                usb_pre: UsbPre::Div1_5,
                ..Stm32f1Clock::default()
            },
        },
    ]
}
