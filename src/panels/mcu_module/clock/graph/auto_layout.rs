//! Automatic diagram layout for a [`ClockGraph`] (clock roadmap — recommendation 1).
//!
//! The hand-authored layouts (F1 / F4 / WBA / ESP) place every box by hand.
//! A graph that arrives WITHOUT one — an AI-imported clock, or any future
//! family — used to render a BLANK diagram, because the renderer only draws
//! [`ClockLayout`] primitives. This computes a readable diagram from the graph
//! TOPOLOGY alone, so every graph draws and stays editable with no per-chip
//! hand-tuning.
//!
//! Layered (Sugiyama-lite):
//! - **x** = longest-path depth from the sources (a proper DAG layering, so
//!   every wire flows left→right at least one column);
//! - **y** = ordered within each column by a few barycenter sweeps, so edges
//!   cross as little as the heuristic manages;
//! - positions are fitted into the diagram's fixed 1000×790 virtual canvas.
//!
//! Each node emits: a name label above, a control (dropdown for mux / divider /
//! choice / multiplier, drag-MHz for a source, else a static box) and a live
//! frequency tag below; each edge emits an orthogonal 3-segment wire.

use std::collections::{HashMap, VecDeque};

use super::layout::{BlockDef, ClockLayout, LabelDef, TagDef, ValueSrc, Widget};
use super::model::{ClockGraph, NodeKind, NodeState};

// The renderer's fixed virtual canvas (see `gui/diagram.rs`). Everything must
// fit inside it — content beyond is clipped, so positions are fitted, not free.
const VW: f32 = 1000.0;
const VH: f32 = 790.0;
const MARGIN: f32 = 46.0;
const NODE_W: f32 = 96.0;
const NODE_H: f32 = 26.0;
/// Caps so a small graph isn't stretched across the whole canvas.
const COL_CAP: f32 = 190.0;
const ROW_CAP: f32 = 74.0;

/// Compute a full [`ClockLayout`] from a graph's topology. Deterministic and
/// pure — the same graph always lays out identically.
pub fn auto_layout(graph: &ClockGraph) -> ClockLayout {
    let mut lay = ClockLayout::default();
    let n = graph.nodes.len();
    if n == 0 {
        return lay;
    }

    let idx_of: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, nd)| (nd.id.as_str(), i))
        .collect();

    // Adjacency + in-degree over resolvable edges (unknown endpoints skipped).
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut pred: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for e in &graph.edges {
        if let (Some(&u), Some(&v)) =
            (idx_of.get(e.from.as_str()), idx_of.get(e.to.as_str()))
        {
            succ[u].push(v);
            pred[v].push(u);
            indeg[v] += 1;
        }
    }

    // ── Longest-path layering via a Kahn topological sweep ────────────────────
    let mut layer = vec![0usize; n];
    let mut deg = indeg.clone();
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| deg[i] == 0).collect();
    while let Some(u) = queue.pop_front() {
        for &v in &succ[u] {
            layer[v] = layer[v].max(layer[u] + 1);
            deg[v] -= 1;
            if deg[v] == 0 {
                queue.push_back(v);
            }
        }
    }
    // A cycle (shouldn't happen for a clock DAG) leaves its nodes at layer 0 —
    // the diagram still renders, just flatter.
    let num_layers = layer.iter().copied().max().unwrap_or(0) + 1;

    // ── Order within each layer (barycenter sweeps) ──────────────────────────
    let mut by_layer: Vec<Vec<usize>> = vec![Vec::new(); num_layers];
    for i in 0..n {
        by_layer[layer[i]].push(i);
    }
    let mut row = vec![0usize; n];
    for col in &by_layer {
        for (r, &i) in col.iter().enumerate() {
            row[i] = r;
        }
    }
    for pass in 0..4 {
        let down = pass % 2 == 0; // alternate: order by upstream, then downstream
        let layers: Vec<usize> = if down {
            (1..num_layers).collect()
        } else {
            (0..num_layers.saturating_sub(1)).rev().collect()
        };
        for l in layers {
            let mut col = std::mem::take(&mut by_layer[l]);
            col.sort_by(|&a, &b| {
                let ka = bary(if down { &pred[a] } else { &succ[a] }, &row, row[a]);
                let kb = bary(if down { &pred[b] } else { &succ[b] }, &row, row[b]);
                ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
            });
            for (r, &i) in col.iter().enumerate() {
                row[i] = r;
            }
            by_layer[l] = col;
        }
    }

    // ── Fit coordinates into the virtual canvas ──────────────────────────────
    let usable_w = (VW - 2.0 * MARGIN - NODE_W).max(0.0);
    let col_step = if num_layers > 1 {
        (usable_w / (num_layers - 1) as f32).min(COL_CAP)
    } else {
        0.0
    };
    let used_w = col_step * num_layers.saturating_sub(1) as f32 + NODE_W;
    let x0 = ((VW - used_w) / 2.0).max(MARGIN);

    let max_rows = by_layer.iter().map(|c| c.len()).max().unwrap_or(1);
    let usable_h = (VH - 2.0 * MARGIN - NODE_H).max(0.0);
    let row_step = if max_rows > 1 {
        (usable_h / (max_rows - 1) as f32).min(ROW_CAP)
    } else {
        0.0
    };

    let mut pos = vec![(0f32, 0f32); n];
    for (l, col) in by_layer.iter().enumerate() {
        let x = x0 + col_step * l as f32;
        let col_h = row_step * col.len().saturating_sub(1) as f32;
        let y0 = ((VH - col_h - NODE_H) / 2.0).max(MARGIN);
        for (r, &i) in col.iter().enumerate() {
            pos[i] = (x, y0 + row_step * r as f32);
        }
    }

    // ── Emit one control + label + freq tag per node ─────────────────────────
    for (i, node) in graph.nodes.iter().enumerate() {
        let (x, y) = pos[i];
        lay.labels_above.push(LabelDef { x, y: y - 3.0, text: node.id.clone() });
        lay.tags.push(TagDef {
            x,
            y: y + NODE_H + 12.0,
            name: String::new(), // the value alone; the id is the label above
            src: ValueSrc::Node(node.id.clone()),
            limit: node.limit,
        });

        match &node.kind {
            NodeKind::Source { min_hz, max_hz, .. } => {
                let lo = *min_hz as f32 / 1e6;
                lay.widgets.push(Widget::DragMhz {
                    node: node.id.clone(),
                    x,
                    y,
                    w: NODE_W,
                    min_mhz: lo.max(0.0),
                    max_mhz: (*max_hz as f32 / 1e6).max(lo),
                });
            }
            NodeKind::Mux { inputs } => {
                let options = (0..*inputs)
                    .map(|k| {
                        // Label each input by the node that feeds it.
                        let label = graph
                            .edges
                            .iter()
                            .find(|e| e.to == node.id && e.input == k)
                            .map(|e| e.from.clone())
                            .unwrap_or_else(|| format!("in{k}"));
                        (label, NodeState::Index(k))
                    })
                    .collect();
                lay.widgets.push(Widget::Combo { node: node.id.clone(), x, y, w: NODE_W, options });
            }
            NodeKind::Divider { options } => {
                let opts = options
                    .iter()
                    .enumerate()
                    .map(|(k, v)| (format!("/{v}"), NodeState::Index(k)))
                    .collect();
                lay.widgets.push(Widget::Combo { node: node.id.clone(), x, y, w: NODE_W, options: opts });
            }
            NodeKind::Choice { ratios } => {
                let opts = ratios
                    .iter()
                    .enumerate()
                    .map(|(k, (num, den))| {
                        let label = if *den == 1 { format!("×{num}") } else { format!("×{num}/{den}") };
                        (label, NodeState::Index(k))
                    })
                    .collect();
                lay.widgets.push(Widget::Combo { node: node.id.clone(), x, y, w: NODE_W, options: opts });
            }
            NodeKind::Multiplier { min, max } => {
                let opts = (*min..=*max).map(|v| (format!("×{v}"), NodeState::Value(v))).collect();
                lay.widgets.push(Widget::Combo { node: node.id.clone(), x, y, w: NODE_W, options: opts });
            }
            // Non-editable nodes render as a static labelled box.
            NodeKind::FixedDiv { by } => {
                lay.blocks.push(box_at(x, y, format!("/{by}")));
            }
            NodeKind::TimerMul { .. } => {
                lay.blocks.push(box_at(x, y, "×tim".to_string()));
            }
            NodeKind::Tap | NodeKind::Output => {
                lay.blocks.push(box_at(x, y, node.id.clone()));
            }
        }
    }

    // ── Orthogonal 3-segment wires along the edges ───────────────────────────
    for e in &graph.edges {
        if let (Some(&u), Some(&v)) =
            (idx_of.get(e.from.as_str()), idx_of.get(e.to.as_str()))
        {
            let (xu, yu) = pos[u];
            let (xv, yv) = pos[v];
            let sx = xu + NODE_W;
            let sy = yu + NODE_H / 2.0;
            let tx = xv;
            let ty = yv + NODE_H / 2.0;
            let midx = (sx + tx) / 2.0;
            lay.wires.push(vec![(sx, sy), (midx, sy), (midx, ty), (tx, ty)]);
        }
    }

    lay
}

fn box_at(x: f32, y: f32, label: String) -> BlockDef {
    BlockDef { x, y, w: NODE_W, h: NODE_H, label }
}

/// Mean row of `neigh`; falls back to `own` (keep position) when a node has no
/// neighbour in the reference direction, so unconstrained nodes stay put.
fn bary(neigh: &[usize], row: &[usize], own: usize) -> f32 {
    if neigh.is_empty() {
        return own as f32;
    }
    neigh.iter().map(|&i| row[i] as f32).sum::<f32>() / neigh.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::clock::graph::model::{Edge, Node};

    fn node(id: &str, kind: NodeKind, state: NodeState) -> Node {
        Node { id: id.into(), kind, state, limit: None }
    }
    fn edge(from: &str, to: &str) -> Edge {
        Edge { from: from.into(), to: to.into(), input: 0 }
    }

    #[test]
    fn empty_graph_yields_empty_layout() {
        let lay = auto_layout(&ClockGraph { nodes: vec![], edges: vec![] });
        assert!(lay.is_empty());
    }

    #[test]
    fn source_divider_output_lays_out_left_to_right_and_in_canvas() {
        let g = ClockGraph {
            nodes: vec![
                node("hsi", NodeKind::Source { min_hz: 16_000_000, max_hz: 16_000_000, gated: true },
                    NodeState::Source { enabled: true, hz: 16_000_000 }),
                node("ahb", NodeKind::Divider { options: vec![1, 2, 4] }, NodeState::Index(0)),
                node("hclk", NodeKind::Output, NodeState::Fixed),
            ],
            edges: vec![edge("hsi", "ahb"), edge("ahb", "hclk")],
        };
        let lay = auto_layout(&g);
        assert!(!lay.is_empty());
        // One control per editable node (source drag + divider combo), one
        // static box for the output, a label + freq tag per node, a wire per edge.
        assert_eq!(lay.widgets.len(), 2);
        assert_eq!(lay.blocks.len(), 1);
        assert_eq!(lay.labels_above.len(), 3);
        assert_eq!(lay.tags.len(), 3);
        assert_eq!(lay.wires.len(), 2);

        // Layering: hsi is left of ahb is left of hclk (strictly increasing x).
        let x = |id: &str| lay.labels_above.iter().find(|l| l.text == id).unwrap().x;
        assert!(x("hsi") < x("ahb"), "source must be left of divider");
        assert!(x("ahb") < x("hclk"), "divider must be left of output");

        // Everything stays inside the virtual canvas.
        for l in &lay.labels_above {
            assert!(l.x >= 0.0 && l.x <= VW && l.y >= 0.0 && l.y <= VH, "{l:?} off canvas");
        }
    }
}
