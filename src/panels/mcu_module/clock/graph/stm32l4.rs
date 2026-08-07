//! STM32L4 clock tree as a data-driven [`ClockGraph`].
//!
//! Topology (RM0351): HSI16 and HSE (4–48 MHz) feed the main PLL
//! `src → /M → ×N → /R → PLLCLK` and the SYSCLK mux; AHB → HCLK; APB1/APB2.
//! Structurally identical to G4 (nested PLL source, R output) — the codegen
//! difference is only `RccDescriptor::l4().hsi_needs_enable` (L4 boots with HSI
//! OFF) and `ReadSpec::l4().reset_is_hw_default = false` (L4 boots on MSI).
//!
//! **MSI** is L4's defining low-power oscillator and its power-on SYSCLK source.
//! It is modelled here as a source node so it SHOWS in the diagram and uniquely
//! marks an L4 graph (no other family has `msi`). It is NOT yet a selectable
//! SYSCLK/PLL source in codegen — that needs the generic reader/emitter to gain
//! MSI (a `SysSource::Msi` + `MSIRange` extension), deliberately deferred to
//! keep the byte-identical-tested core stable. The shipped preset is HSI→PLL.
//!
//! Default = the 80 MHz HSI preset (L4's ceiling): HSI16 / M=1 (16 MHz PLL in) /
//! N=10 (VCO 160) / R=2 → **SYSCLK 80 MHz**, all buses /1. Auto-generated layout.

use super::model::{ClockGraph, Edge, LimitKey, Node, NodeKind, NodeState};

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

/// Build the L4 graph with the crystal-free 80 MHz HSI→PLL preset selected.
pub fn stm32l4_graph() -> ClockGraph {
    let bus_div: Vec<u32> = vec![1, 2, 4, 8, 16];
    let ahb_div: Vec<u32> = vec![1, 2, 4, 8, 16, 64, 128, 256, 512];
    let pll_m: Vec<u32> = vec![1, 2, 4, 6, 8]; // PllPreDiv (1–8 on L4)
    let pll_r: Vec<u32> = vec![2, 4, 6, 8]; // PllRDiv {2,4,6,8}

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
                max_hz: 48 * M,
                gated: true,
            },
            NodeState::Source {
                enabled: false,
                hz: 8 * M,
            },
        ),
        // MSI — L4's power-on oscillator. Display-only for now (see module doc):
        // present so the diagram shows it and to mark the graph as L4.
        n(
            "msi",
            NodeKind::Source {
                min_hz: 100_000,
                max_hz: 48 * M,
                gated: true,
            },
            NodeState::Source {
                enabled: true,
                hz: 4 * M,
            },
        ),
        // ── Main PLL: src mux → /M → ×N → /R ──
        n("pllsrc", NodeKind::Mux { inputs: 2 }, NodeState::Index(0)), // HSI
        n(
            "pllm",
            NodeKind::Divider { options: pll_m },
            NodeState::Index(0),
        ), // /1 → 16 MHz
        n(
            "plln",
            NodeKind::Multiplier { min: 8, max: 127 },
            NodeState::Value(10),
        ), // VCO 160
        n(
            "pllr",
            NodeKind::Divider { options: pll_r },
            NodeState::Index(0),
        ), // /2 → 80 MHz
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

/// Recognise an L4 graph (New MCU form dropdown). L4 is the only family that
/// models the `msi` oscillator — a clean, unique marker (its topology is
/// otherwise identical to G4).
pub fn is_l4_graph(g: &ClockGraph) -> bool {
    g.node("msi").is_some() && g.node("pllr").is_some()
}

#[cfg(test)]
mod tests {
    use super::super::evaluate;
    use super::*;

    #[test]
    fn default_preset_computes_80mhz_sysclk() {
        let f = evaluate(&stm32l4_graph());
        assert_eq!(f.get("sysclk").copied(), Some(80 * M));
        assert_eq!(f.get("hclk").copied(), Some(80 * M));
    }

    #[test]
    fn is_l4_graph_distinguishes_from_g4() {
        use super::super::{stm32g0_graph, stm32g4_graph};
        assert!(is_l4_graph(&stm32l4_graph()));
        assert!(!is_l4_graph(&stm32g4_graph())); // no msi node
        assert!(!is_l4_graph(&stm32g0_graph()));
    }
}
