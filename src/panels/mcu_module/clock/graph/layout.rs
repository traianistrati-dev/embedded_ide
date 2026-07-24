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

use super::model::{LimitKey, NodeState};
use crate::panels::mcu_module::clock::model::ClockLimits;

/// Which frequency a box/tag displays. Resolved at draw time from the evaluated
/// graph frequencies. The named variants are F103 conveniences; [`ValueSrc::Node`]
/// references an arbitrary graph node by id, so any chip's layout works.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// An arbitrary graph node, by id — for chips beyond STM32F1.
    Node(String),
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

/// An interactive control overlaid on the diagram, editing one graph node's
/// state. Kept deliberately simple + uniform: a dropdown whose options each
/// carry the [`NodeState`] to apply (so muxes, dividers and multipliers are all
/// just "pick an option"). Position is in virtual-canvas coords.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Widget {
    /// Dropdown over `options` (label → state) bound to node `node`.
    Combo {
        node: String,
        x: f32,
        y: f32,
        w: f32,
        options: Vec<(String, NodeState)>,
    },
    /// CubeMX-style trapezoid mux with one radio button per input — same look
    /// as the hand-tuned F103 muxes. `inputs` = (label, dy from `y` where the
    /// input enters, state to apply on pick). `flip` mirrors it horizontally
    /// (inputs on the right, output to the left — e.g. MCO).
    MuxRadios {
        node: String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        flip: bool,
        inputs: Vec<(String, f32, NodeState)>,
    },
    /// Drag-editable frequency (MHz) for a `Source` node (e.g. the HSE
    /// crystal). Keeps the node's `enabled` flag untouched.
    DragMhz {
        node: String,
        x: f32,
        y: f32,
        w: f32,
        min_mhz: f32,
        max_mhz: f32,
    },
}

impl Widget {
    /// The graph node this control edits.
    pub fn node_id(&self) -> &str {
        match self {
            Widget::Combo { node, .. }
            | Widget::MuxRadios { node, .. }
            | Widget::DragMhz { node, .. } => node,
        }
    }
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
    /// Interactive controls (dropdowns) editing graph node states.
    #[serde(default)]
    pub widgets: Vec<Widget>,
}

impl ClockLayout {
    /// `true` when the layout carries no drawable primitives at all — the cue
    /// to auto-generate one from the graph topology (see
    /// [`super::auto_layout::auto_layout`]). An AI-imported clock lands here.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
            && self.outputs.is_empty()
            && self.tags.is_empty()
            && self.labels_above.is_empty()
            && self.mux_titles.is_empty()
            && self.wires.is_empty()
            && self.widgets.is_empty()
    }
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
    let combo = |node: &str, x, y, w, options: Vec<(String, NodeState)>| Widget::Combo {
        node: node.to_owned(), x, y, w, options,
    };
    let mux = |node: &str, x, y, w, h, flip, inputs: Vec<(String, f32, NodeState)>| {
        Widget::MuxRadios { node: node.to_owned(), x, y, w, h, flip, inputs }
    };
    // One trapezoid input: (label, dy where the wire enters, mux index to pick).
    let mi = |label: &str, dy: f32, i: usize| (label.to_owned(), dy, NodeState::Index(i));
    // Index-based options for a divider (`/N`), and value-based for the PLL mul.
    let div_opts = |vals: &[u32]| -> Vec<(String, NodeState)> {
        vals.iter().enumerate().map(|(i, v)| (format!("/ {v}"), NodeState::Index(i))).collect()
    };
    let mul_opts: Vec<(String, NodeState)> =
        (2..=16u32).map(|v| (format!("×{v}"), NodeState::Value(v))).collect();
    let s = |label: &str, state: NodeState| (label.to_owned(), state);

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
            out(820.0, 109.0, 160.0, 26.0, "RTCCLK -> RTC", ValueSrc::Rtc, None),
            out(820.0, 149.0, 160.0, 26.0, "IWDGCLK <- LSI", ValueSrc::Fixed(40_000), None),
            out(820.0, 232.0, 160.0, 26.0, "USBCLK -> USB", ValueSrc::Usb, None),
            out(820.0, 272.0, 160.0, 26.0, "FLITFCLK <- HSI", ValueSrc::Flitf, None),
            out(820.0, 346.0, 160.0, 28.0, "HCLK -> AHB / core / DMA", ValueSrc::Hclk, Some(LimitKey::HclkMax)),
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
            tag(460.0, 442.0, "PLLCLK", ValueSrc::Pllclk, Some(LimitKey::SysclkMax)),
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
            vec![(120.0, 300.0), (120.0, 383.0), (175.0, 383.0)], // HSI → /2 (single bend)
            vec![(215.0, 383.0), (226.0, 383.0)],
            vec![(120.0, 500.0), (148.0, 500.0)],
            vec![(208.0, 502.0), (226.0, 502.0)],
            vec![(290.0, 442.0), (336.0, 442.0)],
            vec![(420.0, 442.0), (456.0, 442.0)], // PLLMUL → PLLCLK (tag now at 460,442)
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
        // Interactive controls for the F103 *graph* path — same look and the
        // SAME positions as the typed path's `interactive_nodes`: trapezoid
        // mux radios for the four muxes, a drag-MHz for the HSE crystal, and
        // dropdowns for the prescalers (the typed path ignores these).
        widgets: vec![
            mux("rtc", 250.0, 72.0, 40.0, 100.0, false, vec![
                mi("HSE/128", 28.0, 0),
                mi("LSE", 50.0, 1),
                mi("LSI", 72.0, 2),
            ]),
            mux("pllsrc", 250.0, 370.0, 40.0, 145.0, false, vec![
                mi("HSI/2", 13.0, 0),
                mi("HSE", 132.0, 1),
            ]),
            mux("sw", 470.0, 300.0, 40.0, 120.0, false, vec![
                mi("HSI", 24.0, 0),
                mi("HSE", 60.0, 1),
                mi("PLLCLK", 96.0, 2),
            ]),
            mux("mco", 250.0, 580.0, 40.0, 116.0, true, vec![
                mi("SYSCLK", 20.0, 0),
                mi("HSI", 48.0, 1),
                mi("HSE", 76.0, 2),
                mi("PLL/2", 104.0, 3),
            ]),
            Widget::DragMhz {
                node: "hse".to_owned(),
                x: 28.0, y: 533.0, w: 92.0,
                min_mhz: 1.0, max_mhz: 25.0,
            },
            combo("pllxtpre", 150.0, 490.0, 60.0,
                vec![s("/ 1", NodeState::Index(0)), s("/ 2", NodeState::Index(1))]),
            combo("pllmul", 336.0, 430.0, 84.0, mul_opts),
            combo("ahb", 540.0, 347.0, 86.0, div_opts(&[1, 2, 4, 8, 16, 64, 128, 256, 512])),
            combo("apb1", 720.0, 530.0, 66.0, div_opts(&[1, 2, 4, 8, 16])),
            combo("apb2", 720.0, 650.0, 66.0, div_opts(&[1, 2, 4, 8, 16])),
            combo("adc", 720.0, 750.0, 66.0, div_opts(&[2, 4, 6, 8])),
            combo("usb", 470.0, 232.0, 90.0,
                vec![s("/ 1.5", NodeState::Index(0)), s("/ 1", NodeState::Index(1))]),
            combo("systick", 700.0, 418.0, 66.0,
                vec![s("/ 8", NodeState::Index(0)), s("/ 1", NodeState::Index(1))]),
        ],
    }
}
