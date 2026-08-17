//! A minimal, family-neutral clock tree — the starting point for a chip that
//! has none.
//!
//! Every MCU clock tree has the same spine: an internal RC and an optional
//! crystal feed a PLL and a system-clock selector, which feeds the AHB divider
//! and from there the APB buses. Thirteen nodes cover it, and that is what this
//! builds.
//!
//! **The point is the NAMES, not the shape.** The nodes carry the ids code
//! generation reads (`hse`, `pllsrc`, `pllm`, `plln`, `pllp`, `sw`, `ahb`,
//! `apb1`, `apb2`), so [`bind::propose`](super::bind::propose) resolves them to
//! themselves and the editor's Checks panel is green from the start. A family
//! that later gains an RCC recipe generates from this tree with nothing to wire
//! up.
//!
//! **What it does NOT do is invent limits.** No node carries a
//! [`LimitKey`](super::model::LimitKey): a generic tree does not know this
//! chip's ceilings, and validating 100 MHz against the STM32F103 defaults would
//! flag a perfectly legal H5 configuration — a diagram that looks authoritative
//! and is wrong twice over. Frequencies are computed; ceilings stay unclaimed
//! until the user or a real import supplies them.
//!
//! The oscillator frequencies are the common modern-STM32 values (HSI 16 MHz,
//! HSE 8 MHz) but their RANGES are deliberately wide, so the first thing the
//! user does — typing the real numbers from page one of the datasheet — needs no
//! dialog, just the fields list.

use super::model::{ClockGraph, Edge, Node, NodeKind, NodeState};

const M: u32 = 1_000_000;

fn n(id: &str, kind: NodeKind, state: NodeState) -> Node {
    Node {
        id: id.into(),
        kind,
        state,
        // Deliberately none — see the module docs.
        limit: None,
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

/// The minimal tree, in its RESET configuration: SYSCLK on the internal RC, the
/// PLL present but unselected, every prescaler at /1.
///
/// Starting at reset rather than at some invented "max performance" is the
/// honest default — it is what the chip actually does at power-up, and it means
/// the tree claims no target frequency of its own.
pub fn minimal_graph() -> ClockGraph {
    let bus_div = vec![1, 2, 4, 8, 16];
    let nodes = vec![
        // ── Sources ──
        // Ranges are wide on purpose: the real values are two numbers from the
        // datasheet, typed straight into the fields list.
        n(
            "hsi",
            NodeKind::Source {
                min_hz: M,
                max_hz: 64 * M,
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
                min_hz: M,
                max_hz: 64 * M,
                gated: true,
            },
            NodeState::Source {
                enabled: true,
                hz: 8 * M,
            },
        ),
        // ── PLL: src -> /M -> xN -> /P ──
        n("pllsrc", NodeKind::Mux { inputs: 2 }, NodeState::Index(0)),
        n(
            "pllm",
            NodeKind::Divider {
                options: (1..=64).collect(),
            },
            NodeState::Index(0),
        ),
        n(
            "plln",
            NodeKind::Multiplier { min: 4, max: 512 },
            NodeState::Value(8),
        ),
        n(
            "pllp",
            NodeKind::Divider {
                options: vec![2, 4, 6, 8],
            },
            NodeState::Index(0),
        ),
        n("pllclk", NodeKind::Tap, NodeState::Fixed),
        // ── SYSCLK + buses ──
        // Index(0) = the internal RC, which is what a chip boots on.
        n("sw", NodeKind::Mux { inputs: 3 }, NodeState::Index(0)),
        n("sysclk", NodeKind::Tap, NodeState::Fixed),
        n(
            "ahb",
            NodeKind::Divider {
                options: vec![1, 2, 4, 8, 16, 64, 128, 256, 512],
            },
            NodeState::Index(0),
        ),
        n("hclk", NodeKind::Output, NodeState::Fixed),
        n(
            "apb1",
            NodeKind::Divider {
                options: bus_div.clone(),
            },
            NodeState::Index(0),
        ),
        n("pclk1", NodeKind::Output, NodeState::Fixed),
        n(
            "apb2",
            NodeKind::Divider { options: bus_div },
            NodeState::Index(0),
        ),
        n("pclk2", NodeKind::Output, NodeState::Fixed),
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
        e("ahb", "apb1"),
        e("apb1", "pclk1"),
        e("ahb", "apb2"),
        e("apb2", "pclk2"),
    ];

    ClockGraph { nodes, edges }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{bind, evaluate};
    use crate::panels::mcu_module::codegen::rcc::codegen_node_ids;

    /// At reset it runs on the internal RC, all the way to the buses.
    #[test]
    fn the_minimal_tree_evaluates_at_reset() {
        let f = evaluate(&minimal_graph());
        assert_eq!(f["sysclk"], 16 * M, "boots on HSI, like the hardware");
        assert_eq!(f["hclk"], 16 * M);
        assert_eq!(f["pclk1"], 16 * M);
        assert_eq!(f["pclk2"], 16 * M);
    }

    /// Selecting the PLL and tuning it does what the arithmetic says — the tree
    /// is a working model, not a picture.
    #[test]
    fn selecting_the_pll_changes_sysclk() {
        let mut g = minimal_graph();
        // HSE 8 MHz /1 x100 /2 = 400 MHz.
        g.node_mut("pllsrc").unwrap().state = NodeState::Index(1);
        g.node_mut("plln").unwrap().state = NodeState::Value(100);
        g.node_mut("sw").unwrap().state = NodeState::Index(2);
        let f = evaluate(&g);
        assert_eq!(f["pllclk"], 400 * M);
        assert_eq!(f["sysclk"], 400 * M);

        // And a bus prescaler divides it.
        g.node_mut("apb1").unwrap().state = NodeState::Index(2); // /4
        assert_eq!(evaluate(&g)["pclk1"], 100 * M);
    }

    /// THE point of the id choice: code generation binds to this tree with
    /// nothing to map — every binding is the identity.
    #[test]
    fn the_canonical_ids_bind_to_themselves() {
        let g = minimal_graph();
        // Checked against a family that HAS a recipe, since that is the id set
        // this tree is shaped for; the families it is offered to would generate
        // nothing yet either way.
        let ids = codegen_node_ids("stm32f4");
        let b = bind::propose(&ids, &g);
        assert!(
            b.iter().all(|(k, v)| k == v),
            "every id should resolve to itself: {b:?}"
        );
        assert!(
            bind::unbound(&ids, &b).is_empty(),
            "and none should be left over: {:?}",
            bind::unbound(&ids, &b)
        );
    }

    /// It must not pretend to know this chip's ceilings.
    #[test]
    fn it_claims_no_limits() {
        assert!(
            minimal_graph().nodes.iter().all(|n| n.limit.is_none()),
            "a generic tree cannot know the datasheet maxima"
        );
    }

    /// Well-formed by the editor's own rules, so the Checks panel opens green.
    #[test]
    fn it_is_structurally_sound() {
        use super::super::edit::issues;
        let found = issues(&minimal_graph());
        assert!(found.is_empty(), "{found:?}");
    }

    /// It is a tree the whole tab can work with the moment it is created: it
    /// lays out, it draws controls, and the fields view has something to list.
    #[test]
    fn it_is_ready_for_the_tab() {
        use super::super::{auto_layout, auto_layout::options_for};
        let g = minimal_graph();
        let lay = auto_layout(&g);

        assert_eq!(lay.nodes.len(), g.nodes.len(), "every node is placed");
        assert!(!lay.wires.is_empty(), "and wired");
        // The selectable ones are exactly what the fields list will show.
        let selectable: Vec<&str> = g
            .nodes
            .iter()
            .filter(|n| options_for(&g, n).is_some())
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(
            selectable,
            ["pllsrc", "pllm", "plln", "pllp", "sw", "ahb", "apb1", "apb2"],
            "the spine's knobs, in reading order"
        );
    }

    /// The oscillators are typeable — the two numbers a user corrects first.
    #[test]
    fn the_oscillators_are_adjustable() {
        let g = minimal_graph();
        for id in ["hsi", "hse"] {
            let NodeKind::Source { min_hz, max_hz, .. } = g.node(id).unwrap().kind else {
                panic!("`{id}` is an oscillator");
            };
            assert!(max_hz > min_hz, "`{id}` must take a range, not one value");
        }
    }
}
