//! Generic clock-graph evaluator (Phase 1).
//!
//! Propagates frequencies through any [`ClockGraph`] by repeated relaxation:
//! a node is computed once all the nodes feeding its inputs are known. Clock
//! trees are DAGs, so this reaches a fixpoint in a few passes. The result maps
//! every node id to its output frequency in Hz.

use std::collections::BTreeMap;

use super::model::{ClockGraph, Node, NodeKind, NodeState};

/// Compute the output frequency (Hz) of every node in `graph`.
pub fn evaluate(graph: &ClockGraph) -> BTreeMap<String, u32> {
    // Inputs feeding each target node, ordered by input index.
    let mut inputs: BTreeMap<&str, Vec<(usize, &str)>> = BTreeMap::new();
    for e in &graph.edges {
        inputs
            .entry(e.to.as_str())
            .or_default()
            .push((e.input, e.from.as_str()));
    }
    for v in inputs.values_mut() {
        v.sort_by_key(|(i, _)| *i);
    }

    let by_id: BTreeMap<&str, &Node> = graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let mut out: BTreeMap<String, u32> = BTreeMap::new();
    let mut remaining: Vec<&Node> = graph.nodes.iter().collect();

    // Relax until nothing new can be resolved (DAG → terminates; a cycle would
    // simply leave the unreachable nodes unevaluated rather than loop forever).
    loop {
        let mut progressed = false;
        let mut still: Vec<&Node> = Vec::new();
        for node in remaining {
            let node_inputs = inputs.get(node.id.as_str());
            let ready = node_inputs
                .map(|v| v.iter().all(|(_, from)| out.contains_key(*from)))
                .unwrap_or(true);
            if !ready {
                still.push(node);
                continue;
            }
            let freq = eval_node(node, node_inputs, &out, &by_id);
            out.insert(node.id.clone(), freq);
            progressed = true;
        }
        remaining = still;
        if remaining.is_empty() || !progressed {
            break;
        }
    }

    out
}

/// Compute one node's output from its already-resolved inputs.
fn eval_node(
    node: &Node,
    inputs: Option<&Vec<(usize, &str)>>,
    out: &BTreeMap<String, u32>,
    by_id: &BTreeMap<&str, &Node>,
) -> u32 {
    let input_at = |idx: usize| -> u32 {
        inputs
            .and_then(|v| v.iter().find(|(i, _)| *i == idx))
            .and_then(|(_, from)| out.get(*from).copied())
            .unwrap_or(0)
    };
    let primary = input_at(0);

    match (&node.kind, &node.state) {
        // An unselected node (disabled mux) produces no clock.
        (_, NodeState::Unset) => 0,

        (NodeKind::Source { .. }, NodeState::Source { enabled, hz }) => {
            if *enabled { *hz } else { 0 }
        }
        // A source without explicit state falls back to its fixed minimum.
        (NodeKind::Source { min_hz, .. }, _) => *min_hz,

        (NodeKind::Mux { .. }, NodeState::Index(i)) => input_at(*i),

        (NodeKind::Divider { options }, NodeState::Index(i)) => {
            primary / (*options.get(*i).unwrap_or(&1)).max(1)
        }
        (NodeKind::FixedDiv { by }, _) => primary / (*by).max(1),

        (NodeKind::Choice { ratios }, NodeState::Index(i)) => {
            let (num, den) = ratios.get(*i).copied().unwrap_or((1, 1));
            ((primary as u64 * num as u64) / den.max(1) as u64) as u32
        }

        (NodeKind::Multiplier { .. }, NodeState::Value(v)) => primary.saturating_mul(*v),

        (NodeKind::Tap, _) => primary,
        (NodeKind::Output, _) => primary,

        (NodeKind::TimerMul { prescaler }, _) => {
            let presc = by_id.get(prescaler.as_str()).map(|n| divisor_of(n)).unwrap_or(1);
            if presc <= 1 { primary } else { primary.saturating_mul(2) }
        }

        // Mismatched kind/state (shouldn't happen for well-formed graphs).
        _ => 0,
    }
}

/// Effective integer divisor of a divider-like node (used by `TimerMul`).
fn divisor_of(node: &Node) -> u32 {
    match (&node.kind, &node.state) {
        (NodeKind::Divider { options }, NodeState::Index(i)) => *options.get(*i).unwrap_or(&1),
        (NodeKind::FixedDiv { by }, _) => *by,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::model::{Edge, Node, NodeKind, NodeState};

    fn n(id: &str, kind: NodeKind, state: NodeState) -> Node {
        Node { id: id.into(), kind, state, limit: None }
    }
    fn e(from: &str, to: &str, input: usize) -> Edge {
        Edge { from: from.into(), to: to.into(), input }
    }

    /// A tiny graph: src 8 MHz → ×9 → /2 → output, plus a parallel mux pick.
    #[test]
    fn evaluates_a_simple_chain() {
        let g = ClockGraph {
            nodes: vec![
                n("src", NodeKind::Source { min_hz: 8_000_000, max_hz: 8_000_000, gated: false },
                  NodeState::Source { enabled: true, hz: 8_000_000 }),
                n("pll", NodeKind::Multiplier { min: 2, max: 16 }, NodeState::Value(9)),
                n("div", NodeKind::Divider { options: vec![1, 2, 4] }, NodeState::Index(1)),
                n("out", NodeKind::Output, NodeState::Fixed),
            ],
            edges: vec![
                e("src", "pll", 0),
                e("pll", "div", 0),
                e("div", "out", 0),
            ],
        };
        let r = evaluate(&g);
        assert_eq!(r["pll"], 72_000_000);
        assert_eq!(r["div"], 36_000_000);
        assert_eq!(r["out"], 36_000_000);
    }

    /// Mux selects between two sources; Choice applies a non-integer ratio.
    #[test]
    fn mux_and_ratio_choice() {
        let g = ClockGraph {
            nodes: vec![
                n("a", NodeKind::Source { min_hz: 8_000_000, max_hz: 8_000_000, gated: false },
                  NodeState::Source { enabled: true, hz: 8_000_000 }),
                n("b", NodeKind::Source { min_hz: 12_000_000, max_hz: 12_000_000, gated: false },
                  NodeState::Source { enabled: true, hz: 12_000_000 }),
                n("mux", NodeKind::Mux { inputs: 2 }, NodeState::Index(1)),
                // 72 MHz × 2/3 = 48 MHz (USB-style).
                n("ratio", NodeKind::Choice { ratios: vec![(2, 3), (1, 1)] }, NodeState::Index(0)),
            ],
            edges: vec![
                e("a", "mux", 0),
                e("b", "mux", 1),
                e("mux", "ratio", 0),
            ],
        };
        let r = evaluate(&g);
        assert_eq!(r["mux"], 12_000_000, "mux picks input 1 (b)");
        assert_eq!(r["ratio"], 8_000_000, "12 MHz × 2/3");
    }

    /// A disabled gated source produces 0, which propagates downstream.
    #[test]
    fn disabled_source_propagates_zero() {
        let g = ClockGraph {
            nodes: vec![
                n("hse", NodeKind::Source { min_hz: 4_000_000, max_hz: 16_000_000, gated: true },
                  NodeState::Source { enabled: false, hz: 8_000_000 }),
                n("pll", NodeKind::Multiplier { min: 2, max: 16 }, NodeState::Value(9)),
            ],
            edges: vec![e("hse", "pll", 0)],
        };
        let r = evaluate(&g);
        assert_eq!(r["hse"], 0);
        assert_eq!(r["pll"], 0);
    }

    /// TimerMul doubles only when the referenced prescaler divides by >1.
    #[test]
    fn timer_mul_follows_prescaler() {
        let mut g = ClockGraph {
            nodes: vec![
                n("hclk", NodeKind::Source { min_hz: 72_000_000, max_hz: 72_000_000, gated: false },
                  NodeState::Source { enabled: true, hz: 72_000_000 }),
                n("apb", NodeKind::Divider { options: vec![1, 2, 4, 8, 16] }, NodeState::Index(0)),
                n("pclk", NodeKind::Tap, NodeState::Fixed),
                n("tim", NodeKind::TimerMul { prescaler: "apb".into() }, NodeState::Fixed),
            ],
            edges: vec![
                e("hclk", "apb", 0),
                e("apb", "pclk", 0),
                e("pclk", "tim", 0),
            ],
        };

        // Prescaler /1 → timer = pclk (no doubling).
        let r = evaluate(&g);
        assert_eq!(r["pclk"], 72_000_000);
        assert_eq!(r["tim"], 72_000_000);

        // Prescaler /2 → pclk 36 MHz, timer doubles back to 72 MHz.
        g.node_mut("apb").unwrap().state = NodeState::Index(1);
        let r = evaluate(&g);
        assert_eq!(r["pclk"], 36_000_000);
        assert_eq!(r["tim"], 72_000_000);
    }
}
