//! STM32G4 clock tree as a data-driven [`ClockGraph`] — the second family to go
//! through the FAMILY-keyed RCC codegen (see [`super::super::codegen::rcc`]).
//!
//! Topology (RM0440): HSI16 and HSE (4–48 MHz crystal) feed the main PLL
//! `src → /M → ×N → /R → PLLCLK` and the SYSCLK mux (HSI / HSE / PLLCLK). AHB
//! divides SYSCLK into HCLK; APB1 / APB2 divide HCLK, with the ×2 timer rule.
//! G4 uses the PLL **R** output for SYSCLK (like WBA), a nested `source:` inside
//! the `Pll { … }` and a real crystal HSE — all captured by `RccDescriptor::g4`.
//!
//! Default selections = a crystal-free 150 MHz preset (the non-boost ceiling):
//! HSI16 / M=4 (4 MHz PLL in) / N=75 (VCO 300) / R=2 → **SYSCLK 150 MHz**, all
//! buses /1. G4 needs NO hand-authored diagram layout — the generic
//! [`auto_layout`](super::auto_layout::auto_layout) draws it from this topology.

use super::model::{ClockGraph, Edge, LimitKey, Node, NodeKind, NodeState};

const M: u32 = 1_000_000;

fn n(id: &str, kind: NodeKind, state: NodeState) -> Node {
    Node { id: id.into(), kind, state, limit: None }
}
fn n_lim(id: &str, kind: NodeKind, state: NodeState, limit: LimitKey) -> Node {
    Node { id: id.into(), kind, state, limit: Some(limit) }
}
fn e(from: &str, to: &str) -> Edge {
    Edge { from: from.into(), to: to.into(), input: 0 }
}
fn e_in(from: &str, to: &str, input: usize) -> Edge {
    Edge { from: from.into(), to: to.into(), input }
}

/// Build the G4 graph with the crystal-free 150 MHz HSI→PLL preset selected.
/// Node ids match what [`read_rcc_values`](super::super::codegen::rcc::read_rcc_values)
/// expects (`hse`/`pllsrc`/`pllm`/`plln`/`pllr`/`sw`/`ahb`/`apb1`/`apb2`).
pub fn stm32g4_graph() -> ClockGraph {
    let bus_div: Vec<u32> = vec![1, 2, 4, 8, 16];
    // HPRE skips /32 (same field encoding as F4).
    let ahb_div: Vec<u32> = vec![1, 2, 4, 8, 16, 64, 128, 256, 512];
    let pll_m: Vec<u32> = vec![1, 2, 4, 6, 8, 16]; // PllPreDiv (1–16); safe subset
    let pll_r: Vec<u32> = vec![2, 4, 6, 8]; // PllRDiv — the only valid G4 R divisors

    let nodes = vec![
        // ── Sources ──
        n(
            "hsi",
            NodeKind::Source { min_hz: 16 * M, max_hz: 16 * M, gated: false },
            NodeState::Source { enabled: true, hz: 16 * M },
        ),
        n(
            "hse",
            NodeKind::Source { min_hz: 4 * M, max_hz: 48 * M, gated: true },
            NodeState::Source { enabled: false, hz: 8 * M },
        ),
        // ── Main PLL: src mux → /M → ×N → /R ──
        n("pllsrc", NodeKind::Mux { inputs: 2 }, NodeState::Index(0)), // HSI
        n("pllm", NodeKind::Divider { options: pll_m }, NodeState::Index(2)), // /4 → 4 MHz
        n("plln", NodeKind::Multiplier { min: 8, max: 127 }, NodeState::Value(75)), // VCO 300
        n("pllr", NodeKind::Divider { options: pll_r }, NodeState::Index(0)), // /2 → 150 MHz
        n_lim("pllclk", NodeKind::Tap, NodeState::Fixed, LimitKey::SysclkMax),
        // ── SYSCLK + buses ──
        n("sw", NodeKind::Mux { inputs: 3 }, NodeState::Index(2)), // PLLCLK
        n_lim("sysclk", NodeKind::Tap, NodeState::Fixed, LimitKey::SysclkMax),
        n("ahb", NodeKind::Divider { options: ahb_div }, NodeState::Index(0)),
        n_lim("hclk", NodeKind::Output, NodeState::Fixed, LimitKey::HclkMax),
        n("apb1", NodeKind::Divider { options: bus_div.clone() }, NodeState::Index(0)),
        n_lim("pclk1", NodeKind::Output, NodeState::Fixed, LimitKey::Pclk1Max),
        n("apb2", NodeKind::Divider { options: bus_div }, NodeState::Index(0)),
        n_lim("pclk2", NodeKind::Output, NodeState::Fixed, LimitKey::Pclk2Max),
        // ── Timer ×2 rule ──
        n("tim_apb1", NodeKind::TimerMul { prescaler: "apb1".into() }, NodeState::Fixed),
        n("tim_apb2", NodeKind::TimerMul { prescaler: "apb2".into() }, NodeState::Fixed),
    ];

    let edges = vec![
        e_in("hsi", "pllsrc", 0),
        e_in("hse", "pllsrc", 1),
        e("pllsrc", "pllm"),
        e("pllm", "plln"),
        e("plln", "pllr"),
        e("pllr", "pllclk"),
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

/// Recognise a G4 graph (for the New MCU form's clock-choice dropdown). G4 uses
/// the PLL `pllr` output (like WBA) but, unlike the others, has neither F4's
/// `pllp` output node nor WBA's extra `apb7` bus — that pair pins it uniquely.
pub fn is_g4_graph(g: &ClockGraph) -> bool {
    g.node("pllr").is_some() && g.node("pllp").is_none() && g.node("apb7").is_none()
}

#[cfg(test)]
mod tests {
    use super::super::evaluate;
    use super::*;

    #[test]
    fn default_preset_computes_150mhz_sysclk() {
        let f = evaluate(&stm32g4_graph());
        assert_eq!(f.get("sysclk").copied(), Some(150 * M));
        assert_eq!(f.get("hclk").copied(), Some(150 * M));
        // All buses /1 → PCLK1/PCLK2 = HCLK.
        assert_eq!(f.get("pclk1").copied(), Some(150 * M));
        assert_eq!(f.get("pclk2").copied(), Some(150 * M));
    }

    #[test]
    fn is_g4_graph_distinguishes_families() {
        use super::super::{stm32f4_graph, stm32wba_graph};
        assert!(is_g4_graph(&stm32g4_graph()));
        assert!(!is_g4_graph(&stm32f4_graph())); // F4 → pllp
        assert!(!is_g4_graph(&stm32wba_graph())); // WBA → pll1r
    }
}
