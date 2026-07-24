//! STM32G0 clock tree as a data-driven [`ClockGraph`] — a third family on the
//! FAMILY-keyed RCC codegen (see [`super::super::codegen::rcc`]).
//!
//! Topology (RM0444): HSI16 and HSE (4–48 MHz) feed the main PLL
//! `src → /M → ×N → /R → PLLCLK` and the SYSCLK mux (HSI / HSE / PLLCLK). AHB
//! divides SYSCLK into HCLK; a SINGLE APB bus divides HCLK (G0 has no APB2),
//! with the ×2 timer rule. Like G4 the PLL nests its `source:` and drives SYSCLK
//! from the R output — the same `RccDescriptor` shape, only the ReadSpec's bus
//! list (one bus) and the option ranges differ.
//!
//! Default selections = the full-speed 64 MHz preset (G0's ceiling): HSI16 /
//! M=1 (16 MHz PLL in) / N=8 (VCO 128) / R=2 → **SYSCLK 64 MHz**, all buses /1.
//! No hand-authored layout — [`auto_layout`](super::auto_layout::auto_layout)
//! draws it.

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

/// Build the G0 graph with the crystal-free 64 MHz HSI→PLL preset selected.
pub fn stm32g0_graph() -> ClockGraph {
    let bus_div: Vec<u32> = vec![1, 2, 4, 8, 16];
    let ahb_div: Vec<u32> = vec![1, 2, 4, 8, 16, 64, 128, 256, 512];
    let pll_m: Vec<u32> = vec![1, 2, 3, 4, 6, 8]; // PllPreDiv (1–16); safe subset
    let pll_r: Vec<u32> = vec![2, 3, 4, 5, 6, 7, 8]; // PllRDiv — G0 allows all 2..8

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
        n("pllm", NodeKind::Divider { options: pll_m }, NodeState::Index(0)), // /1 → 16 MHz
        n("plln", NodeKind::Multiplier { min: 8, max: 86 }, NodeState::Value(8)), // VCO 128
        n("pllr", NodeKind::Divider { options: pll_r }, NodeState::Index(0)), // /2 → 64 MHz
        n_lim("pllclk", NodeKind::Tap, NodeState::Fixed, LimitKey::SysclkMax),
        // ── SYSCLK + single APB bus ──
        n("sw", NodeKind::Mux { inputs: 3 }, NodeState::Index(2)), // PLLCLK
        n_lim("sysclk", NodeKind::Tap, NodeState::Fixed, LimitKey::SysclkMax),
        n("ahb", NodeKind::Divider { options: ahb_div }, NodeState::Index(0)),
        n_lim("hclk", NodeKind::Output, NodeState::Fixed, LimitKey::HclkMax),
        n("apb1", NodeKind::Divider { options: bus_div }, NodeState::Index(0)),
        n_lim("pclk1", NodeKind::Output, NodeState::Fixed, LimitKey::Pclk1Max),
        // ── Timer ×2 rule (single bus) ──
        n("tim_apb1", NodeKind::TimerMul { prescaler: "apb1".into() }, NodeState::Fixed),
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
        e("pclk1", "tim_apb1"),
    ];

    ClockGraph { nodes, edges }
}

/// Recognise a G0 graph (New MCU form dropdown). G0 uses `pllr` (like G4/WBA)
/// but is the single-APB STM32 — no `apb2`, no F4 `pllp`.
pub fn is_g0_graph(g: &ClockGraph) -> bool {
    g.node("pllr").is_some() && g.node("apb2").is_none() && g.node("pllp").is_none()
}

#[cfg(test)]
mod tests {
    use super::super::evaluate;
    use super::*;

    #[test]
    fn default_preset_computes_64mhz_sysclk() {
        let f = evaluate(&stm32g0_graph());
        assert_eq!(f.get("sysclk").copied(), Some(64 * M));
        assert_eq!(f.get("hclk").copied(), Some(64 * M));
        assert_eq!(f.get("pclk1").copied(), Some(64 * M));
    }

    #[test]
    fn is_g0_graph_distinguishes_families() {
        use super::super::{stm32f4_graph, stm32g4_graph, stm32wba_graph};
        assert!(is_g0_graph(&stm32g0_graph()));
        assert!(!is_g0_graph(&stm32g4_graph())); // G4 has apb2
        assert!(!is_g0_graph(&stm32wba_graph())); // WBA has apb2/apb7
        assert!(!is_g0_graph(&stm32f4_graph())); // F4 → pllp
    }
}
