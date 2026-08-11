//! STM32WBA55 clock tree as a data-driven [`ClockGraph`] + diagram layout,
//! plus the bridge the WBA codegen uses to read the user's selections back.
//!
//! Topology (RM0515): HSI16 (fixed 16 MHz) and HSE (FIXED 32 MHz — the radio
//! requires exactly that crystal) feed the PLL1 chain `src → /M → ×N → /R`
//! and the SYSCLK mux (HSI16 / HSE32 / PLL1R, ≤ 100 MHz). AHB divides SYSCLK
//! into HCLK; APB1 / APB2 / APB7 divide HCLK into the peripheral clocks, with
//! the ×2 timer rule on APB1/APB2. Datasheet windows enforced by the embassy
//! runtime and mirrored here: PLL ref (src/M) ∈ 4–16 MHz, VCO (ref×N) ∈
//! 128–544 MHz, PLL1R ≤ 100 MHz.
//!
//! Default selections = the max-performance preset: HSE 32 / M=2 (ref 16) /
//! N=25 (VCO 400) / R=4 → **SYSCLK 100 MHz**, all bus prescalers /1.

use super::layout::{BlockDef, ClockLayout, LabelDef, OutputDef, TagDef, ValueSrc, Widget};
use super::model::{ClockGraph, Edge, LimitKey, Node, NodeKind, NodeState};
use crate::panels::mcu_module::clock::model::ClockLimits;

const M: u32 = 1_000_000;
/// WBA55 ceilings (RM0515 / DS14127): everything caps at 100 MHz.
pub const WBA_SYSCLK_MAX: u32 = 100 * M;

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

/// Datasheet ceilings for the WBA55 — shipped in the definition so the
/// diagram's over-limit red uses the right numbers (the F103 defaults would
/// flag the 100 MHz preset).
pub fn stm32wba_limits() -> ClockLimits {
    ClockLimits {
        sysclk_max: WBA_SYSCLK_MAX,
        sysclk_max_hsi_pll: WBA_SYSCLK_MAX,
        hclk_max: WBA_SYSCLK_MAX,
        pclk1_max: WBA_SYSCLK_MAX,
        pclk2_max: WBA_SYSCLK_MAX,
        adcclk_max: 36 * M,
        usbclk_hz: 48 * M,
        // The HSE crystal is fixed by the radio: exactly 32 MHz.
        hse_min_hz: 32 * M,
        hse_max_hz: 32 * M,
    }
}

/// Build the WBA55 graph with the 100 MHz PLL preset selected.
pub fn stm32wba_graph() -> ClockGraph {
    let bus_div: Vec<u32> = vec![1, 2, 4, 8, 16];
    let pll_div: Vec<u32> = vec![1, 2, 3, 4, 5, 6, 7, 8];

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
                min_hz: 32 * M,
                max_hz: 32 * M,
                gated: true,
            },
            NodeState::Source {
                enabled: true,
                hz: 32 * M,
            },
        ),
        // ── PLL1 chain: src mux → /M → ×N → /R ──
        n("pllsrc", NodeKind::Mux { inputs: 2 }, NodeState::Index(1)), // HSE
        n(
            "pllm",
            NodeKind::Divider {
                options: pll_div.clone(),
            },
            NodeState::Index(1), // /2 → ref 16 MHz
        ),
        n(
            "plln",
            NodeKind::Multiplier { min: 4, max: 512 },
            NodeState::Value(25), // VCO 400 MHz
        ),
        n(
            "pllr",
            NodeKind::Divider {
                options: pll_div.clone(),
            },
            NodeState::Index(3), // /4 → 100 MHz
        ),
        n_lim(
            "pll1r",
            NodeKind::Tap,
            NodeState::Fixed,
            LimitKey::SysclkMax,
        ),
        // ── SYSCLK + buses ──
        n("sw", NodeKind::Mux { inputs: 3 }, NodeState::Index(2)), // PLL1R
        n_lim(
            "sysclk",
            NodeKind::Tap,
            NodeState::Fixed,
            LimitKey::SysclkMax,
        ),
        n(
            "ahb",
            NodeKind::Divider {
                options: bus_div.clone(),
            },
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
            NodeState::Index(0),
        ),
        n_lim(
            "pclk1",
            NodeKind::Output,
            NodeState::Fixed,
            LimitKey::Pclk1Max,
        ),
        n(
            "apb2",
            NodeKind::Divider {
                options: bus_div.clone(),
            },
            NodeState::Index(0),
        ),
        n_lim(
            "pclk2",
            NodeKind::Output,
            NodeState::Fixed,
            LimitKey::Pclk2Max,
        ),
        n(
            "apb7",
            NodeKind::Divider { options: bus_div },
            NodeState::Index(0),
        ),
        // Same 100 MHz ceiling as HCLK — LimitKey has no dedicated PCLK7 slot.
        n_lim(
            "pclk7",
            NodeKind::Output,
            NodeState::Fixed,
            LimitKey::HclkMax,
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
        e("plln", "pllr"),
        e("pllr", "pll1r"),
        e_in("hsi", "sw", 0),
        e_in("hse", "sw", 1),
        e_in("pll1r", "sw", 2),
        e("sw", "sysclk"),
        e("sysclk", "ahb"),
        e("ahb", "hclk"),
        e("hclk", "apb1"),
        e("apb1", "pclk1"),
        e("hclk", "apb2"),
        e("apb2", "pclk2"),
        e("hclk", "apb7"),
        e("apb7", "pclk7"),
        e("pclk1", "tim_apb1"),
        e("pclk2", "tim_apb2"),
    ];

    ClockGraph { nodes, edges }
}

/// Compact CubeMX-style diagram for the WBA55 tree (1000×790 virtual space):
/// sources left, PLL chain across the middle, SYSCLK mux, bus dividers and
/// the delivered clocks down the right margin.
pub fn stm32wba_layout() -> ClockLayout {
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
    let pll = [1u32, 2, 3, 4, 5, 6, 7, 8];
    // Curated ×N list around the useful VCO window (full 4..512 lives in RON).
    let plln = [10u32, 12, 15, 16, 20, 24, 25, 30, 32, 36, 40, 48, 50, 60];

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
                label: "HSI16\n16 MHz RC".into(),
            },
            BlockDef {
                x: 40.0,
                y: 260.0,
                w: 130.0,
                h: 40.0,
                label: "HSE\n32 MHz crystal".into(),
            },
            BlockDef {
                x: 40.0,
                y: 620.0,
                w: 130.0,
                h: 34.0,
                label: "LSE 32.768 kHz".into(),
            },
            BlockDef {
                x: 40.0,
                y: 680.0,
                w: 130.0,
                h: 34.0,
                label: "LSI 32 kHz".into(),
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
                y: 360.0,
                w: 130.0,
                h: 34.0,
                label: "PCLK7 (APB7)".into(),
                src: ValueSrc::Node("pclk7".into()),
                limit: Some(LimitKey::HclkMax),
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
                name: "PLL1R".into(),
                src: ValueSrc::Node("pll1r".into()),
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
            },
            LabelDef {
                x: 340.0,
                y: 292.0,
                text: "PLL xN".into(),
            },
            LabelDef {
                x: 430.0,
                y: 292.0,
                text: "PLL /R".into(),
            },
            LabelDef {
                x: 620.0,
                y: 445.0,
                text: "AHB".into(),
            },
            LabelDef {
                x: 700.0,
                y: 445.0,
                text: "APB1".into(),
            },
            LabelDef {
                x: 620.0,
                y: 515.0,
                text: "APB2".into(),
            },
            LabelDef {
                x: 700.0,
                y: 515.0,
                text: "APB7".into(),
            },
        ],
        mux_titles: vec![
            LabelDef {
                x: 250.0,
                y: 195.0,
                text: "PLLSRC".into(),
            },
            LabelDef {
                x: 640.0,
                y: 135.0,
                text: "SYSCLK mux".into(),
            },
        ],
        wires: vec![
            // HSI → PLLSRC(0) / SYSCLK mux(0)
            vec![
                (170.0, 170.0),
                (215.0, 170.0),
                (215.0, 210.0),
                (235.0, 210.0),
            ],
            vec![(215.0, 170.0), (580.0, 155.0)],
            // HSE → PLLSRC(1) / SYSCLK mux(1)
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
            // PLL chain
            vec![(320.0, 225.0), (335.0, 310.0)],
            vec![(415.0, 310.0), (425.0, 310.0)],
            vec![
                (505.0, 310.0),
                (540.0, 310.0),
                (540.0, 195.0),
                (580.0, 195.0),
            ],
            // SYSCLK mux → SYSCLK out + AHB
            vec![(700.0, 175.0), (850.0, 137.0)],
            vec![(770.0, 175.0), (770.0, 470.0), (615.0, 470.0)],
            // Bus chain (schematic)
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
                    ("HSI16".into(), 10.0, NodeState::Index(0)),
                    ("HSE32".into(), 40.0, NodeState::Index(1)),
                ],
            },
            combo("pllm", 250.0, 296.0, 70.0, div_opts(&pll)),
            combo("plln", 340.0, 296.0, 75.0, mul_opts(&plln)),
            combo("pllr", 430.0, 296.0, 70.0, div_opts(&pll)),
            Widget::MuxRadios {
                node: "sw".into(),
                x: 580.0,
                y: 140.0,
                w: 120.0,
                h: 70.0,
                flip: false,
                inputs: vec![
                    ("HSI16".into(), 15.0, NodeState::Index(0)),
                    ("HSE32".into(), 35.0, NodeState::Index(1)),
                    ("PLL1R".into(), 55.0, NodeState::Index(2)),
                ],
            },
            combo("ahb", 615.0, 450.0, 70.0, div_opts(&bus)),
            combo("apb1", 695.0, 450.0, 70.0, div_opts(&bus)),
            combo("apb2", 615.0, 520.0, 70.0, div_opts(&bus)),
            combo("apb7", 695.0, 520.0, 70.0, div_opts(&bus)),
        ],
    }
}

// ── Codegen bridge ────────────────────────────────────────────────────────────

/// Which source drives SYSCLK.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WbaSys {
    Hsi,
    Hse,
    Pll,
}

/// The WBA selections the codegen needs, read back from the graph. Field
/// values are the HUMAN numbers (M=2 means "/2", N=25 means "×25").
#[derive(Clone, Debug, PartialEq)]
pub struct WbaClock {
    pub sys: WbaSys,
    pub hse_on: bool,
    pub pll_src_hse: bool,
    pub pll_m: u32,
    pub pll_n: u32,
    pub pll_r: u32,
    pub ahb: u32,
    pub apb1: u32,
    pub apb2: u32,
    pub apb7: u32,
}

impl Default for WbaClock {
    /// The reset state: HSI16 sysclk, everything /1 (embassy's own default).
    fn default() -> Self {
        Self {
            sys: WbaSys::Hsi,
            hse_on: false,
            pll_src_hse: true,
            pll_m: 2,
            pll_n: 25,
            pll_r: 4,
            ahb: 1,
            apb1: 1,
            apb2: 1,
            apb7: 1,
        }
    }
}

/// `true` when this graph is the WBA tree (drives both the codegen dispatch
/// and the form's Edit detection).
pub fn is_wba_graph(g: &ClockGraph) -> bool {
    g.node("pll1r").is_some() && g.node("apb7").is_some()
}

/// Read the user's selections back out of the graph. Missing/foreign nodes
/// fall back to the reset state, so a non-WBA graph degrades safely.
pub fn graph_to_wba(g: &ClockGraph) -> WbaClock {
    let mut c = WbaClock::default();
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

    if let Some(NodeState::Source { enabled, .. }) = g.node("hse").map(|n| &n.state) {
        c.hse_on = *enabled;
    }
    c.sys = match index_of("sw") {
        Some(0) => WbaSys::Hsi,
        Some(1) => WbaSys::Hse,
        _ => WbaSys::Pll,
    };
    c.pll_src_hse = index_of("pllsrc") != Some(0);
    if let Some(m) = divisor_of("pllm") {
        c.pll_m = m;
    }
    if let Some(NodeState::Value(v)) = g.node("plln").map(|n| &n.state) {
        c.pll_n = *v;
    }
    if let Some(r) = divisor_of("pllr") {
        c.pll_r = r;
    }
    c.ahb = divisor_of("ahb").unwrap_or(1);
    c.apb1 = divisor_of("apb1").unwrap_or(1);
    c.apb2 = divisor_of("apb2").unwrap_or(1);
    c.apb7 = divisor_of("apb7").unwrap_or(1);
    // Sysclk on HSE / PLL-from-HSE implies the oscillator is in use.
    if c.sys == WbaSys::Hse || (c.sys == WbaSys::Pll && c.pll_src_hse) {
        c.hse_on = true;
    }
    c
}

impl WbaClock {
    /// The SYSCLK this configuration produces, mirroring the embassy math.
    pub fn sysclk_hz(&self) -> u32 {
        match self.sys {
            WbaSys::Hsi => 16 * M,
            WbaSys::Hse => 32 * M,
            WbaSys::Pll => {
                let src = if self.pll_src_hse { 32 * M } else { 16 * M };
                (src / self.pll_m.max(1)) * self.pll_n / self.pll_r.max(1)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::evaluate;
    use super::*;

    /// The shipped default is the 100 MHz preset, and the generic evaluator
    /// agrees with the bridge's own arithmetic on every delivered clock.
    #[test]
    fn default_graph_evaluates_to_100mhz() {
        let g = stm32wba_graph();
        let f = evaluate(&g);
        assert_eq!(f["pll1r"], 100 * M); // 32/2 ×25 /4
        assert_eq!(f["sysclk"], 100 * M);
        assert_eq!(f["hclk"], 100 * M);
        assert_eq!(f["pclk1"], 100 * M);
        assert_eq!(f["pclk7"], 100 * M);
        assert_eq!(f["tim_apb1"], 100 * M); // APB /1 → ×1 rule
        assert_eq!(graph_to_wba(&g).sysclk_hz(), f["sysclk"]);
    }

    /// Switching the SYSCLK mux + prescalers reads back through the bridge.
    #[test]
    fn bridge_reads_selections_back() {
        let mut g = stm32wba_graph();
        g.node_mut("sw").unwrap().state = NodeState::Index(0); // HSI
        g.node_mut("ahb").unwrap().state = NodeState::Index(2); // /4
        g.node_mut("apb7").unwrap().state = NodeState::Index(4); // /16
        let c = graph_to_wba(&g);
        assert_eq!(c.sys, WbaSys::Hsi);
        assert_eq!(c.ahb, 4);
        assert_eq!(c.apb7, 16);
        assert_eq!(c.sysclk_hz(), 16 * M);
        let f = evaluate(&g);
        assert_eq!(f["hclk"], 4 * M);
        assert_eq!(f["pclk7"], 250_000);
        // APB1 stays /1 → timer clock ×1; set /2 → ×2 rule kicks in.
        g.node_mut("apb1").unwrap().state = NodeState::Index(1);
        let f = evaluate(&g);
        assert_eq!(f["pclk1"], 2 * M);
        assert_eq!(f["tim_apb1"], 4 * M);
    }

    /// Every delivered clock in the default preset passes the WBA limits.
    #[test]
    fn default_preset_is_within_wba_limits() {
        use super::super::over_limits;
        let g = stm32wba_graph();
        let freqs = evaluate(&g);
        let over = over_limits(&g, &stm32wba_limits(), &freqs);
        assert!(over.is_empty(), "over-limit: {over:?}");
    }

    #[test]
    fn wba_graph_detection() {
        assert!(is_wba_graph(&stm32wba_graph()));
        let f1 = super::super::stm32f1_graph(&Default::default());
        assert!(!is_wba_graph(&f1));
    }
}
