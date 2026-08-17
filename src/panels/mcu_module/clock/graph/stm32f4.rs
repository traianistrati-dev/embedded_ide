//! STM32F4 clock tree as a data-driven [`ClockGraph`] + diagram layout, plus
//! the bridge the F4 codegen reads the user's selections back through.
//!
//! Topology (RM0090): HSI (16 MHz) and HSE (external 4–26 MHz crystal) feed the
//! main PLL `src → /M → ×N → /P → PLLCLK` and the SYSCLK mux (HSI / HSE /
//! PLLCLK). AHB divides SYSCLK into HCLK; APB1 / APB2 divide HCLK into the
//! peripheral clocks, with the ×2 timer rule. Datasheet windows mirrored from
//! embassy's `max` (F411): VCO input (src/M) ∈ 1–2.1 MHz, VCO (×N) ∈ 100–432
//! MHz, SYSCLK ≤ 100 MHz (chip-dependent — the converter ships the real ceiling).
//!
//! Default selections = a crystal-free preset: HSI 16 / M=8 (VCO in 2) / N=100
//! (VCO 200) / P=2 → **SYSCLK 100 MHz**, APB1 /2 (PCLK1 50), APB2 /1.

use super::layout::{BlockDef, ClockLayout, LabelDef, OutputDef, TagDef, ValueSrc, Widget};
use super::model::{ClockGraph, Edge, LimitKey, Node, NodeKind, NodeState};
use crate::panels::mcu_module::clock::model::ClockLimits;

const M: u32 = 1_000_000;

fn n(id: &str, kind: NodeKind, state: NodeState) -> Node {
    Node {
        id: id.into(),
        kind,
        state,
        limit: None,
    }
}
fn n_lim(id: &str, kind: NodeKind, state: NodeState, limit: LimitKey) -> Node {
    Node {
        id: id.into(),
        kind,
        state,
        limit: Some(limit),
    }
}
fn e(from: &str, to: &str) -> Edge {
    Edge {
        from: from.into(),
        to: to.into(),
        input: 0,
    }
}
fn e_in(from: &str, to: &str, input: usize) -> Edge {
    Edge {
        from: from.into(),
        to: to.into(),
        input,
    }
}

/// F4 ceilings. `sysclk_max` is chip-dependent (F401 84, F411 100, F407 168,
/// F429 180 MHz); the two PCLK ceilings follow the chip's bus-split rule
/// (F401/F41x: PCLK1 = HCLK/2, PCLK2 = HCLK; larger parts: /4 and /2).
pub fn stm32f4_limits(sysclk_max: u32, pclk1_max: u32, pclk2_max: u32) -> ClockLimits {
    ClockLimits {
        sysclk_max,
        sysclk_max_hsi_pll: sysclk_max,
        hclk_max: sysclk_max,
        pclk1_max,
        pclk2_max,
        adcclk_max: 36 * M,
        usbclk_hz: 48 * M,
        hse_min_hz: 4 * M,
        hse_max_hz: 26 * M,
    }
}

/// The common default (F411-class): 100 MHz, PCLK1 50, PCLK2 100.
pub fn stm32f4_limits_default() -> ClockLimits {
    stm32f4_limits(100 * M, 50 * M, 100 * M)
}

/// STM32F2 ceilings, from embassy-stm32's own `#[cfg(stm32f2)] mod max`
/// (`rcc/f247.rs`): SYSCLK/HCLK 120 MHz, PCLK1 = /4, PCLK2 = /2. Those are
/// `rcc_assert!`s, i.e. a debug build PANICS AT BOOT on a violation — not a
/// datasheet nicety. The F2 shipped with F4's numbers (100/50/100), which are
/// wrong in both directions: too low for HCLK, too high for both APBs.
pub fn stm32f2_limits() -> ClockLimits {
    ClockLimits {
        // F2's ADC tops out below F4's; the rest come straight from `max`.
        adcclk_max: 30 * M,
        ..stm32f4_limits(120 * M, 30 * M, 60 * M)
    }
}

/// The PLLN window for the F2, which is NOT the F4's.
///
/// embassy's `PllMul` is the chip's own PAC enum (`pac::rcc::vals::Plln`), and
/// metapac's `rcc_f2` block only has `MUL192..=MUL432` — 241 variants against
/// F4/F7's 385. So an N the F4 accepts, say 144, is not merely out of spec on an
/// F2: it names a variant that does not exist, and the generated code does not
/// compile. embassy's `max::PLL_VCO` for F2 (192..432 MHz at 1 MHz PLL input)
/// says the same thing in frequency terms.
pub const F2_PLL_N: (u32, u32) = (192, 432);
/// F4/F7's window. Narrower than the PAC enum (which starts at 2) because the
/// datasheet requires 50 — the PAC is not the tighter constraint here.
pub const F4_PLL_N: (u32, u32) = (50, 432);

/// Build the F4 graph with the crystal-free 100 MHz HSI→PLL preset selected.
pub fn stm32f4_graph() -> ClockGraph {
    // /8 → 2 MHz, ×100 → VCO 200, /2 → 100 MHz.
    f247_graph(F4_PLL_N, 1, 100)
}

/// The same tree for the F2, whose PLLN floor of 192 makes the F4's preset
/// unreachable: at PLLM /8 (2 MHz in) the smallest legal N already gives a
/// 192 MHz VCO and a 96 MHz SYSCLK. Dividing to 1 MHz instead puts the whole
/// useful range back in reach — and lands on the same 100 MHz default.
///
/// The rule worth remembering for this family: with PLLM /16 from HSI16,
/// N = 2 x SYSCLK[MHz] at /P = 2, so N in 192..432 covers 96..216 MHz.
pub fn stm32f2_graph() -> ClockGraph {
    // /16 → 1 MHz, ×200 → VCO 200, /2 → 100 MHz.
    f247_graph(F2_PLL_N, 3, 200)
}

/// The shared F2/F4/F7 tree (embassy's `rcc/f247.rs` covers all three), with the
/// PLLN window and the default PLLM/PLLN that differ between them.
///
/// `m_index` indexes the PLLM divisor list below, not the divisor itself.
fn f247_graph(pll_n: (u32, u32), m_index: usize, n_default: u32) -> ClockGraph {
    let bus_div: Vec<u32> = vec![1, 2, 4, 8, 16];
    // HPRE skips /32 on F4.
    let ahb_div: Vec<u32> = vec![1, 2, 4, 8, 16, 64, 128, 256, 512];
    let pll_m: Vec<u32> = vec![4, 8, 12, 16, 20, 25];
    let pll_p: Vec<u32> = vec![2, 4, 6, 8];

    let nodes = vec![
        // ── Sources ──
        n(
            "hsi",
            NodeKind::Source {
                min_hz: 16 * M,
                max_hz: 16 * M,
                gated: false,
            },
            NodeState::Source {
                enabled: true,
                hz: 16 * M,
            },
        ),
        n(
            "hse",
            NodeKind::Source {
                min_hz: 4 * M,
                max_hz: 26 * M,
                gated: true,
            },
            NodeState::Source {
                enabled: false,
                hz: 8 * M,
            },
        ),
        // ── Main PLL: src mux → /M → ×N → /P ──
        n("pllsrc", NodeKind::Mux { inputs: 2 }, NodeState::Index(0)), // HSI
        n(
            "pllm",
            NodeKind::Divider { options: pll_m },
            NodeState::Index(m_index),
        ),
        n(
            "plln",
            NodeKind::Multiplier {
                min: pll_n.0,
                max: pll_n.1,
            },
            NodeState::Value(n_default),
        ),
        n(
            "pllp",
            NodeKind::Divider { options: pll_p },
            NodeState::Index(0),
        ), // /2 → 100 MHz
        n_lim(
            "pllclk",
            NodeKind::Tap,
            NodeState::Fixed,
            LimitKey::SysclkMax,
        ),
        // ── SYSCLK + buses ──
        n("sw", NodeKind::Mux { inputs: 3 }, NodeState::Index(2)), // PLLCLK
        n_lim(
            "sysclk",
            NodeKind::Tap,
            NodeState::Fixed,
            LimitKey::SysclkMax,
        ),
        n(
            "ahb",
            NodeKind::Divider { options: ahb_div },
            NodeState::Index(0),
        ),
        n_lim(
            "hclk",
            NodeKind::Output,
            NodeState::Fixed,
            LimitKey::HclkMax,
        ),
        n(
            "apb1",
            NodeKind::Divider {
                options: bus_div.clone(),
            },
            NodeState::Index(1),
        ), // /2
        n_lim(
            "pclk1",
            NodeKind::Output,
            NodeState::Fixed,
            LimitKey::Pclk1Max,
        ),
        n(
            "apb2",
            NodeKind::Divider { options: bus_div },
            NodeState::Index(0),
        ),
        n_lim(
            "pclk2",
            NodeKind::Output,
            NodeState::Fixed,
            LimitKey::Pclk2Max,
        ),
        // ── Timer ×2 rule ──
        n(
            "tim_apb1",
            NodeKind::TimerMul {
                prescaler: "apb1".into(),
            },
            NodeState::Fixed,
        ),
        n(
            "tim_apb2",
            NodeKind::TimerMul {
                prescaler: "apb2".into(),
            },
            NodeState::Fixed,
        ),
        // ── Low-speed (display only) ──
        n(
            "lse",
            NodeKind::Source {
                min_hz: 32_768,
                max_hz: 32_768,
                gated: true,
            },
            NodeState::Source {
                enabled: true,
                hz: 32_768,
            },
        ),
        n(
            "lsi",
            NodeKind::Source {
                min_hz: 32_000,
                max_hz: 32_000,
                gated: false,
            },
            NodeState::Source {
                enabled: true,
                hz: 32_000,
            },
        ),
    ];

    let edges = vec![
        e_in("hsi", "pllsrc", 0),
        e_in("hse", "pllsrc", 1),
        e("pllsrc", "pllm"),
        e("pllm", "plln"),
        e("plln", "pllp"),
        e("pllp", "pllclk"),
        e_in("hsi", "sw", 0),
        e_in("hse", "sw", 1),
        e_in("pllclk", "sw", 2),
        e("sw", "sysclk"),
        e("sysclk", "ahb"),
        e("ahb", "hclk"),
        e("hclk", "apb1"),
        e("apb1", "pclk1"),
        e("hclk", "apb2"),
        e("apb2", "pclk2"),
        e("pclk1", "tim_apb1"),
        e("pclk2", "tim_apb2"),
    ];

    ClockGraph { nodes, edges }
}

/// Compact CubeMX-style diagram for the F4 tree (1000×790 virtual space):
/// sources left, PLL chain across the middle, SYSCLK mux, bus dividers and the
/// delivered clocks down the right margin.
pub fn stm32f4_layout() -> ClockLayout {
    f247_layout(F4_PLL_N)
}

/// The same diagram for the F2 — identical topology, but the ×N dropdown must
/// only offer values its PAC actually has (see [`F2_PLL_N`]). Offering 144 there
/// is how the uncompilable `PllMul::MUL144` got generated in the first place.
pub fn stm32f2_layout() -> ClockLayout {
    f247_layout(F2_PLL_N)
}

fn f247_layout(pll_n: (u32, u32)) -> ClockLayout {
    let combo =
        |node: &str, x: f32, y: f32, w: f32, options: Vec<(String, NodeState)>| Widget::Combo {
            node: node.into(),
            x,
            y,
            w,
            options,
        };
    let div_opts = |values: &[u32]| -> Vec<(String, NodeState)> {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| (format!("/{v}"), NodeState::Index(i)))
            .collect()
    };
    let mul_opts = |values: &[u32]| -> Vec<(String, NodeState)> {
        values
            .iter()
            .map(|&v| (format!("x{v}"), NodeState::Value(v)))
            .collect()
    };
    let bus = [1u32, 2, 4, 8, 16];
    let pll_m = [4u32, 8, 12, 16, 20, 25];
    let pll_p = [2u32, 4, 6, 8];
    // Curated ×N list around the useful VCO window (the full range lives in the
    // graph node, and in RON). Filtered to the family's window: the F2 floor of
    // 192 drops the first nine of these, which is the whole point — every one of
    // them names a `PllMul` variant that family does not have.
    let plln: Vec<u32> = [
        50u32, 72, 84, 96, 100, 120, 144, 168, 180, 192, 200, 216, 240, 288, 336, 432,
    ]
    .into_iter()
    .filter(|n| (pll_n.0..=pll_n.1).contains(n))
    .collect();

    ClockLayout {
        // Hand-authored: the primitives below ARE the layout, so there are
        // no node boxes to derive them from.
        nodes: Vec::new(),
        blocks: vec![
            BlockDef {
                x: 40.0,
                y: 150.0,
                w: 130.0,
                h: 40.0,
                label: "HSI\n16 MHz RC".into(),
                node: None,
            },
            BlockDef {
                x: 40.0,
                y: 260.0,
                w: 130.0,
                h: 40.0,
                label: "HSE\n4–26 MHz".into(),
                node: None,
            },
            BlockDef {
                x: 40.0,
                y: 620.0,
                w: 130.0,
                h: 34.0,
                label: "LSE 32.768 kHz".into(),
                node: None,
            },
            BlockDef {
                x: 40.0,
                y: 680.0,
                w: 130.0,
                h: 34.0,
                label: "LSI 32 kHz".into(),
                node: None,
            },
        ],
        outputs: vec![
            OutputDef {
                x: 850.0,
                y: 120.0,
                w: 130.0,
                h: 34.0,
                label: "SYSCLK".into(),
                src: ValueSrc::Node("sysclk".into()),
                limit: Some(LimitKey::SysclkMax),
            },
            OutputDef {
                x: 850.0,
                y: 180.0,
                w: 130.0,
                h: 34.0,
                label: "HCLK (AHB)".into(),
                src: ValueSrc::Node("hclk".into()),
                limit: Some(LimitKey::HclkMax),
            },
            OutputDef {
                x: 850.0,
                y: 240.0,
                w: 130.0,
                h: 34.0,
                label: "PCLK1 (APB1)".into(),
                src: ValueSrc::Node("pclk1".into()),
                limit: Some(LimitKey::Pclk1Max),
            },
            OutputDef {
                x: 850.0,
                y: 300.0,
                w: 130.0,
                h: 34.0,
                label: "PCLK2 (APB2)".into(),
                src: ValueSrc::Node("pclk2".into()),
                limit: Some(LimitKey::Pclk2Max),
            },
            OutputDef {
                x: 850.0,
                y: 420.0,
                w: 130.0,
                h: 34.0,
                label: "APB1 timers".into(),
                src: ValueSrc::Node("tim_apb1".into()),
                limit: None,
            },
            OutputDef {
                x: 850.0,
                y: 480.0,
                w: 130.0,
                h: 34.0,
                label: "APB2 timers".into(),
                src: ValueSrc::Node("tim_apb2".into()),
                limit: None,
            },
        ],
        tags: vec![
            TagDef {
                x: 545.0,
                y: 285.0,
                name: "PLLCLK".into(),
                src: ValueSrc::Node("pllclk".into()),
                limit: Some(LimitKey::SysclkMax),
            },
            TagDef {
                x: 455.0,
                y: 250.0,
                name: "VCO".into(),
                src: ValueSrc::Node("plln".into()),
                limit: None,
            },
        ],
        labels_above: vec![
            LabelDef {
                x: 250.0,
                y: 292.0,
                text: "PLL /M".into(),
                node: None,
            },
            LabelDef {
                x: 340.0,
                y: 292.0,
                text: "PLL xN".into(),
                node: None,
            },
            LabelDef {
                x: 430.0,
                y: 292.0,
                text: "PLL /P".into(),
                node: None,
            },
            LabelDef {
                x: 620.0,
                y: 445.0,
                text: "AHB".into(),
                node: None,
            },
            LabelDef {
                x: 700.0,
                y: 445.0,
                text: "APB1".into(),
                node: None,
            },
            LabelDef {
                x: 620.0,
                y: 515.0,
                text: "APB2".into(),
                node: None,
            },
        ],
        mux_titles: vec![
            LabelDef {
                x: 250.0,
                y: 195.0,
                text: "PLLSRC".into(),
                node: None,
            },
            LabelDef {
                x: 640.0,
                y: 135.0,
                text: "SYSCLK mux".into(),
                node: None,
            },
        ],
        wires: vec![
            vec![
                (170.0, 170.0),
                (215.0, 170.0),
                (215.0, 210.0),
                (235.0, 210.0),
            ],
            vec![(215.0, 170.0), (580.0, 155.0)],
            vec![
                (170.0, 280.0),
                (215.0, 280.0),
                (215.0, 240.0),
                (235.0, 240.0),
            ],
            vec![
                (215.0, 280.0),
                (560.0, 280.0),
                (560.0, 175.0),
                (580.0, 175.0),
            ],
            vec![(320.0, 225.0), (335.0, 310.0)],
            vec![(415.0, 310.0), (425.0, 310.0)],
            vec![
                (505.0, 310.0),
                (540.0, 310.0),
                (540.0, 195.0),
                (580.0, 195.0),
            ],
            vec![(700.0, 175.0), (850.0, 137.0)],
            vec![(770.0, 175.0), (770.0, 470.0), (615.0, 470.0)],
            vec![(690.0, 470.0), (850.0, 197.0)],
        ],
        widgets: vec![
            Widget::MuxRadios {
                node: "pllsrc".into(),
                x: 235.0,
                y: 200.0,
                w: 85.0,
                h: 55.0,
                flip: false,
                inputs: vec![
                    ("HSI".into(), 10.0, NodeState::Index(0)),
                    ("HSE".into(), 40.0, NodeState::Index(1)),
                ],
            },
            combo("pllm", 250.0, 296.0, 70.0, div_opts(&pll_m)),
            combo("plln", 340.0, 296.0, 75.0, mul_opts(&plln)),
            combo("pllp", 430.0, 296.0, 70.0, div_opts(&pll_p)),
            Widget::MuxRadios {
                node: "sw".into(),
                x: 580.0,
                y: 140.0,
                w: 120.0,
                h: 70.0,
                flip: false,
                inputs: vec![
                    ("HSI".into(), 15.0, NodeState::Index(0)),
                    ("HSE".into(), 35.0, NodeState::Index(1)),
                    ("PLLCLK".into(), 55.0, NodeState::Index(2)),
                ],
            },
            combo("ahb", 615.0, 450.0, 70.0, div_opts(&bus)),
            combo("apb1", 695.0, 450.0, 70.0, div_opts(&bus)),
            combo("apb2", 615.0, 520.0, 70.0, div_opts(&bus)),
        ],
    }
}

// ── Codegen bridge ────────────────────────────────────────────────────────────

/// Which source drives SYSCLK.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum F4Sys {
    Hsi,
    Hse,
    Pll,
}

/// The F4 selections the codegen needs, read back from the graph. Field values
/// are the HUMAN numbers (M=8 means "/8", N=100 means "×100", P=2 means "/2").
#[derive(Clone, Debug, PartialEq)]
pub struct F4Clock {
    pub sys: F4Sys,
    pub hse_on: bool,
    pub hse_hz: u32,
    pub pll_src_hse: bool,
    pub pll_m: u32,
    pub pll_n: u32,
    pub pll_p: u32,
    pub ahb: u32,
    pub apb1: u32,
    pub apb2: u32,
}

impl Default for F4Clock {
    /// The reset state: HSI sysclk, everything /1 (embassy's own default).
    fn default() -> Self {
        Self {
            sys: F4Sys::Hsi,
            hse_on: false,
            hse_hz: 8 * M,
            pll_src_hse: false,
            pll_m: 8,
            pll_n: 100,
            pll_p: 2,
            ahb: 1,
            apb1: 1,
            apb2: 1,
        }
    }
}

/// `true` when this graph is the F4 tree (drives codegen dispatch + the form's
/// Edit detection). The `pllp` divider is unique to F4 among our graphs.
pub fn is_f4_graph(g: &ClockGraph) -> bool {
    g.node("pllp").is_some() && g.node("sw").is_some()
}

/// Read the user's selections back out of the graph. Missing/foreign nodes fall
/// back to the reset state, so a non-F4 graph degrades safely.
pub fn graph_to_f4(g: &ClockGraph) -> F4Clock {
    let mut c = F4Clock::default();
    let index_of = |id: &str| match g.node(id).map(|n| &n.state) {
        Some(NodeState::Index(i)) => Some(*i),
        _ => None,
    };
    let divisor_of = |id: &str| -> Option<u32> {
        let node = g.node(id)?;
        let NodeKind::Divider { options } = &node.kind else {
            return None;
        };
        let NodeState::Index(i) = node.state else {
            return None;
        };
        options.get(i).copied()
    };

    if let Some(NodeState::Source { enabled, hz }) = g.node("hse").map(|n| &n.state) {
        c.hse_on = *enabled;
        c.hse_hz = *hz;
    }
    c.sys = match index_of("sw") {
        Some(0) => F4Sys::Hsi,
        Some(1) => F4Sys::Hse,
        _ => F4Sys::Pll,
    };
    c.pll_src_hse = index_of("pllsrc") == Some(1);
    if let Some(m) = divisor_of("pllm") {
        c.pll_m = m;
    }
    if let Some(NodeState::Value(v)) = g.node("plln").map(|n| &n.state) {
        c.pll_n = *v;
    }
    if let Some(p) = divisor_of("pllp") {
        c.pll_p = p;
    }
    c.ahb = divisor_of("ahb").unwrap_or(1);
    c.apb1 = divisor_of("apb1").unwrap_or(1);
    c.apb2 = divisor_of("apb2").unwrap_or(1);
    // Sysclk on HSE / PLL-from-HSE implies the oscillator is in use.
    if c.sys == F4Sys::Hse || (c.sys == F4Sys::Pll && c.pll_src_hse) {
        c.hse_on = true;
    }
    c
}

impl F4Clock {
    /// The SYSCLK this configuration produces, mirroring the embassy math.
    pub fn sysclk_hz(&self) -> u32 {
        match self.sys {
            F4Sys::Hsi => 16 * M,
            F4Sys::Hse => self.hse_hz,
            F4Sys::Pll => {
                let src = if self.pll_src_hse {
                    self.hse_hz
                } else {
                    16 * M
                };
                (src / self.pll_m.max(1)) * self.pll_n / self.pll_p.max(1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::evaluate;
    use super::*;

    /// The F2 shares the tree but not the PLLN window: metapac's `rcc_f2` block
    /// only defines `MUL192..=MUL432`, so anything the F4 preset reaches below
    /// 192 names a `PllMul` variant that does not exist. This is the bug that
    /// produced `PllMul::MUL144` for an STM32F217.
    #[test]
    fn the_f2_pll_n_window_starts_at_192() {
        assert_eq!(F2_PLL_N, (192, 432));
        assert_eq!(F4_PLL_N, (50, 432));
        let g = stm32f2_graph();
        let Some(NodeKind::Multiplier { min, max }) = g.node("plln").map(|n| n.kind.clone()) else {
            panic!("the F2 graph must have a plln multiplier");
        };
        assert_eq!((min, max), F2_PLL_N);
    }

    /// The default preset has to be REACHABLE in that window. Keeping F4's
    /// /8 x100 would have shipped an N below the floor — a chip that cannot
    /// build out of the box.
    #[test]
    fn the_f2_default_preset_is_legal_and_still_100_mhz() {
        let g = stm32f2_graph();
        let f = evaluate(&g);
        assert_eq!(f.get("sysclk").copied(), Some(100 * M), "{f:?}");
        let Some(NodeState::Value(n)) = g.node("plln").map(|n| n.state.clone()) else {
            panic!("plln must carry a value");
        };
        assert!(
            (F2_PLL_N.0..=F2_PLL_N.1).contains(&n),
            "default N {n} is outside {F2_PLL_N:?}"
        );
        // embassy's `max::PLL_VCO` for F2 is 192..432 MHz and `PLL_IN` is
        // 0.95..2.1 MHz — both are `rcc_assert!`, i.e. a boot-time panic.
        let f_in = f.get("pllm").copied().expect("pllm frequency");
        assert!((950_000..=2_100_000).contains(&f_in), "PLL input {f_in} Hz");
        let vco = f_in * n;
        assert!((192 * M..=432 * M).contains(&vco), "VCO {vco} Hz");
    }

    /// The diagram's x-N dropdown must not offer what the family cannot encode:
    /// picking 144 from it is exactly how the broken project was configured.
    #[test]
    fn the_f2_dropdown_offers_no_illegal_multiplier() {
        let l = stm32f2_layout();
        let opts: Vec<u32> = l
            .widgets
            .iter()
            .filter_map(|w| match w {
                Widget::Combo { node, options, .. } if node == "plln" => Some(options),
                _ => None,
            })
            .flatten()
            .filter_map(|(_, st)| match st {
                NodeState::Value(v) => Some(*v),
                _ => None,
            })
            .collect();
        assert!(!opts.is_empty(), "the F2 layout must offer x-N choices");
        assert!(
            opts.iter().all(|n| (F2_PLL_N.0..=F2_PLL_N.1).contains(n)),
            "illegal multipliers offered: {opts:?}"
        );
        // The F4 keeps its own, wider set — this must not have narrowed both.
        let f4: Vec<u32> = stm32f4_layout()
            .widgets
            .iter()
            .filter_map(|w| match w {
                Widget::Combo { node, options, .. } if node == "plln" => Some(options),
                _ => None,
            })
            .flatten()
            .filter_map(|(_, st)| match st {
                NodeState::Value(v) => Some(*v),
                _ => None,
            })
            .collect();
        assert!(f4.contains(&50), "F4 must still offer x50: {f4:?}");
    }

    /// The shipped default is the 100 MHz HSI→PLL preset, and the generic
    /// evaluator agrees with the bridge's own arithmetic.
    #[test]
    fn default_graph_evaluates_to_100mhz() {
        let g = stm32f4_graph();
        let f = evaluate(&g);
        assert_eq!(f["pllclk"], 100 * M); // 16/8 ×100 /2
        assert_eq!(f["sysclk"], 100 * M);
        assert_eq!(f["hclk"], 100 * M);
        assert_eq!(f["pclk1"], 50 * M); // APB1 /2
        assert_eq!(f["pclk2"], 100 * M);
        assert_eq!(f["tim_apb1"], 100 * M); // /2 → ×2 rule
        assert_eq!(f["tim_apb2"], 100 * M); // /1 → ×1
        assert_eq!(graph_to_f4(&g).sysclk_hz(), f["sysclk"]);
    }

    #[test]
    fn bridge_reads_selections_back() {
        let mut g = stm32f4_graph();
        g.node_mut("sw").unwrap().state = NodeState::Index(0); // HSI direct
        g.node_mut("ahb").unwrap().state = NodeState::Index(2); // /4
        let c = graph_to_f4(&g);
        assert_eq!(c.sys, F4Sys::Hsi);
        assert_eq!(c.ahb, 4);
        assert_eq!(c.sysclk_hz(), 16 * M);
        let f = evaluate(&g);
        assert_eq!(f["hclk"], 4 * M);

        // HSE crystal (25 MHz) driving the PLL: /25 ×160 /2 = 80 MHz.
        let mut g = stm32f4_graph();
        g.node_mut("hse").unwrap().state = NodeState::Source {
            enabled: true,
            hz: 25 * M,
        };
        g.node_mut("pllsrc").unwrap().state = NodeState::Index(1); // HSE
        g.node_mut("pllm").unwrap().state = NodeState::Index(5); // /25
        g.node_mut("plln").unwrap().state = NodeState::Value(160);
        let c = graph_to_f4(&g);
        assert_eq!(c.pll_src_hse, true);
        assert_eq!((c.pll_m, c.pll_n, c.pll_p), (25, 160, 2));
        assert_eq!(c.hse_hz, 25 * M);
        assert_eq!(c.sysclk_hz(), 80 * M);
    }

    #[test]
    fn default_preset_within_f411_limits() {
        use super::super::over_limits;
        let g = stm32f4_graph();
        let freqs = evaluate(&g);
        let over = over_limits(&g, &stm32f4_limits_default(), &freqs);
        assert!(over.is_empty(), "over-limit: {over:?}");
    }

    #[test]
    fn f4_graph_detection() {
        assert!(is_f4_graph(&stm32f4_graph()));
        assert!(!is_f4_graph(&super::super::stm32wba_graph()));
        assert!(!is_f4_graph(&super::super::stm32f1_graph(
            &Default::default()
        )));
    }
}
