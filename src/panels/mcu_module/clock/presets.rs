//! Named clock presets — one-click sane configurations for the Clock tab.

use super::model::{PllSrc, Stm32f1Clock, SysclkSrc, UsbPre};

/// A labelled clock configuration the user can apply with one click.
pub struct Preset {
    pub name: &'static str,
    pub description: &'static str,
    pub config: Stm32f1Clock,
}

/// All STM32F1 presets, in display order.
pub fn stm32f1_presets() -> Vec<Preset> {
    vec![
        Preset {
            name: "72 MHz (HSE 8 + PLL×9)",
            description: "Max performance. SYSCLK 72, PCLK1 36, USB 48 MHz.",
            config: Stm32f1Clock::default(),
        },
        Preset {
            name: "HSE 8 MHz direct",
            description: "Crystal straight to SYSCLK, PLL off. Low power, no USB.",
            config: Stm32f1Clock {
                sysclk_src: SysclkSrc::Hse,
                ..Stm32f1Clock::default()
            },
        },
        Preset {
            name: "HSI 8 MHz",
            description: "Internal RC only — no crystal needed. No USB.",
            config: Stm32f1Clock {
                hse_enabled: false,
                sysclk_src: SysclkSrc::Hsi,
                ..Stm32f1Clock::default()
            },
        },
        Preset {
            name: "HSI→PLL 64 MHz",
            description: "No crystal. HSI/2 ×16 = 64 MHz (footnote-1 max). No USB.",
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
