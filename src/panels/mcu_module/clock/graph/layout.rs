//! Data description of a clock diagram's **static layer** (Phase 3).
//!
//! The hand-tuned Figure-2 positions that used to be hardcoded in
//! `gui/diagram.rs` now live here as data: labelled blocks, right-margin output
//! boxes, on-chain frequency tags, node labels, mux titles and routed wires.
//! `gui/diagram.rs` renders by iterating this structure (reusing the same
//! CubeMX-style primitives), so a chip can ship a different layout to get a
//! different diagram — the answer to "import the clock diagram per MCU".
//!
//! Coordinates are in the diagram's 1000×790 virtual space. Values shown in
//! boxes/tags are *not* stored — each carries a [`ValueSrc`] resolved live from
//! the graph-evaluated frequencies, so the diagram stays correct as the user
//! edits nodes.

use serde::{Deserialize, Serialize};

use super::model::LimitKey;
use crate::panels::mcu_module::clock::model::ClockLimits;

/// Which frequency a box/tag displays. Resolved at draw time from the evaluated
/// frequencies (+ the live config for the diagram-only RTC / MCO / SysTick muxes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueSrc {
    Hclk,
    Sysclk,
    Pclk1,
    Pclk2,
    /// APB1 timer clock (×2 rule).
    Pclk1Tim,
    /// APB2 timer clock (×2 rule).
    Pclk2Tim,
    Adc,
    Usb,
    Pllclk,
    Flitf,
    /// SysTick (honours the SysTick source mux).
    Systick,
    /// RTC clock (honours RTCSEL).
    Rtc,
    /// MCO pin (honours the MCO mux).
    Mco,
    /// A constant (e.g. IWDG = LSI 40 kHz).
    Fixed(u32),
}

/// A static labelled rectangle (oscillators, fixed dividers).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockDef {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub label: String,
}

/// A delivered-clock box on the right margin (value + label, red over limit).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputDef {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub label: String,
    pub src: ValueSrc,
    pub limit: Option<LimitKey>,
}

/// An on-chain frequency tag ("NAME value").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TagDef {
    pub x: f32,
    pub y: f32,
    pub name: String,
    pub src: ValueSrc,
    pub limit: Option<LimitKey>,
}

/// A free-standing text label (node names above dropdowns; mux titles).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabelDef {
    pub x: f32,
    pub y: f32,
    pub text: String,
}

/// The complete static diagram description.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClockLayout {
    pub blocks: Vec<BlockDef>,
    pub outputs: Vec<OutputDef>,
    pub tags: Vec<TagDef>,
    /// Node-name labels, drawn LEFT_BOTTOM-anchored above their dropdowns.
    pub labels_above: Vec<LabelDef>,
    /// Mux titles, drawn CENTER_BOTTOM-anchored above each mux.
    pub mux_titles: Vec<LabelDef>,
    /// Routed wire polylines (arrowhead on the last segment).
    pub wires: Vec<Vec<(f32, f32)>>,
}

const M: u32 = 1_000_000;

/// The STM32F103 Figure-2 static layout (ported verbatim from the original
/// `gui/diagram.rs`). Takes `limits` only to print the HSE crystal range.
pub fn stm32f1_layout(limits: &ClockLimits) -> ClockLayout {
    let blk = |x, y, w, h, label: &str| BlockDef { x, y, w, h, label: label.to_owned() };
    let out = |x, y, w, h, label: &str, src, limit| OutputDef {
        x, y, w, h, label: label.to_owned(), src, limit,
    };
    let tag = |x, y, name: &str, src, limit| TagDef { x, y, name: name.to_owned(), src, limit };
    let lbl = |x, y, text: &str| LabelDef { x, y, text: text.to_owned() };

    ClockLayout {
        blocks: vec![
            blk(28.0, 78.0, 92.0, 34.0, "LSE OSC\n32.768 kHz"),
            blk(170.0, 84.0, 46.0, 22.0, "/128"),
            blk(28.0, 153.0, 92.0, 34.0, "LSI RC\n40 kHz"),
            blk(28.0, 283.0, 92.0, 34.0, "HSI RC\n8 MHz"),
            blk(175.0, 372.0, 40.0, 22.0, "/2"),
            blk(
                28.0, 483.0, 92.0, 34.0,
                &format!("HSE OSC\n{}–{} MHz", limits.hse_min_hz / M, limits.hse_max_hz / M),
            ),
        ],
        outputs: vec![
            out(820.0, 109.0, 160.0, 26.0, "RTCCLK → RTC", ValueSrc::Rtc, None),
            out(820.0, 149.0, 160.0, 26.0, "IWDGCLK ← LSI", ValueSrc::Fixed(40_000), None),
            out(820.0, 232.0, 160.0, 26.0, "USBCLK → USB", ValueSrc::Usb, None),
            out(820.0, 272.0, 160.0, 26.0, "FLITFCLK ← HSI", ValueSrc::Flitf, None),
            out(820.0, 346.0, 160.0, 28.0, "HCLK → AHB / core / DMA", ValueSrc::Hclk, Some(LimitKey::HclkMax)),
            out(820.0, 416.0, 160.0, 28.0, "Cortex SysTick", ValueSrc::Systick, None),
            out(820.0, 456.0, 160.0, 28.0, "FCLK (free-running)", ValueSrc::Hclk, None),
            out(820.0, 529.0, 160.0, 28.0, "APB1 peripherals", ValueSrc::Pclk1, Some(LimitKey::Pclk1Max)),
            out(820.0, 576.0, 160.0, 28.0, "APB1 timers", ValueSrc::Pclk1Tim, None),
            out(820.0, 649.0, 160.0, 28.0, "APB2 peripherals", ValueSrc::Pclk2, Some(LimitKey::Pclk2Max)),
            out(820.0, 696.0, 160.0, 28.0, "APB2 timers", ValueSrc::Pclk2Tim, None),
            out(820.0, 749.0, 160.0, 28.0, "ADC1/2", ValueSrc::Adc, Some(LimitKey::AdcclkMax)),
            out(28.0, 625.0, 106.0, 26.0, "MCO pin", ValueSrc::Mco, None),
        ],
        tags: vec![
            tag(424.0, 422.0, "PLLCLK", ValueSrc::Pllclk, Some(LimitKey::SysclkMax)),
            tag(516.0, 344.0, "SYSCLK", ValueSrc::Sysclk, Some(LimitKey::SysclkMax)),
            tag(592.0, 344.0, "HCLK", ValueSrc::Hclk, Some(LimitKey::HclkMax)),
            tag(792.0, 524.0, "PCLK1", ValueSrc::Pclk1, Some(LimitKey::Pclk1Max)),
            tag(792.0, 644.0, "PCLK2", ValueSrc::Pclk2, Some(LimitKey::Pclk2Max)),
        ],
        labels_above: vec![
            lbl(148.0, 478.0, "PLLXTPRE"),
            lbl(336.0, 418.0, "PLLMUL"),
            lbl(470.0, 228.0, "USB Prescaler"),
            lbl(540.0, 335.0, "AHB Prescaler"),
            lbl(700.0, 408.0, "SysTick"),
            lbl(720.0, 518.0, "APB1 Prescaler"),
            lbl(720.0, 638.0, "APB2 Prescaler"),
            lbl(720.0, 738.0, "ADC Prescaler"),
            lbl(28.0, 521.0, "HSE crystal"),
        ],
        mux_titles: vec![
            lbl(270.0, 64.0, "RTC Mux"),
            lbl(270.0, 364.0, "PLL Source"),
            lbl(490.0, 294.0, "System Clock Mux"),
            lbl(270.0, 574.0, "MCO Mux"),
        ],
        wires: vec![
            vec![(120.0, 300.0), (157.0, 300.0), (157.0, 383.0), (175.0, 383.0)],
            vec![(215.0, 383.0), (226.0, 383.0)],
            vec![(120.0, 500.0), (148.0, 500.0)],
            vec![(208.0, 502.0), (226.0, 502.0)],
            vec![(290.0, 442.0), (336.0, 442.0)],
            vec![(420.0, 442.0), (456.0, 442.0)],
            vec![(510.0, 360.0), (540.0, 360.0)],
            vec![(626.0, 360.0), (640.0, 360.0)],
            vec![(640.0, 360.0), (818.0, 360.0)],
            vec![(640.0, 360.0), (640.0, 700.0)],
            vec![(640.0, 430.0), (700.0, 430.0)],
            vec![(766.0, 430.0), (818.0, 430.0)],
            vec![(640.0, 470.0), (818.0, 470.0)],
            vec![(640.0, 543.0), (720.0, 543.0)],
            vec![(786.0, 543.0), (818.0, 543.0)],
            vec![(806.0, 556.0), (806.0, 590.0), (818.0, 590.0)],
            vec![(640.0, 663.0), (720.0, 663.0)],
            vec![(786.0, 663.0), (818.0, 663.0)],
            vec![(806.0, 676.0), (806.0, 710.0), (818.0, 710.0)],
            vec![(753.0, 676.0), (753.0, 763.0), (720.0, 763.0)],
            vec![(786.0, 763.0), (818.0, 763.0)],
            vec![(290.0, 122.0), (818.0, 122.0)],
            vec![(560.0, 245.0), (818.0, 245.0)],
            vec![(250.0, 638.0), (134.0, 638.0)],
        ],
    }
}
