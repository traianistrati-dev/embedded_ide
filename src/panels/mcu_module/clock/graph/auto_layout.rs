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
//! - positions use a FIXED pitch, so the diagram grows with the graph. It used
//!   to be squeezed into a 1000×790 canvas, which only shrank a large tree until
//!   its labels were unreadable; the Clock tab's `Scene` pans and zooms instead.
//!
//! Two steps, so an editor can re-run only the second one:
//! - [`place`] → one [`NodeBox`] per node (WHERE things go);
//! - [`derive`] → the drawable primitives from those boxes: per node a name
//!   label above, a control (dropdown for mux / divider / choice / multiplier /
//!   gate, drag-MHz for a source, else a static box) and a live frequency tag
//!   below; per edge an orthogonal wire routed through a per-edge lane.
//!
//! The boxes are the SOURCE OF TRUTH and are kept in [`ClockLayout::nodes`]:
//! move one, call `derive` again, and the whole node moves with it.

use std::collections::{HashMap, VecDeque};

use super::layout::{BlockDef, ClockLayout, LabelDef, NodeBox, TagDef, ValueSrc, Widget};
use super::model::{ClockGraph, Edge, Node, NodeKind, NodeState};

const MARGIN: f32 = 46.0;
const NODE_W: f32 = 96.0;
const NODE_H: f32 = 26.0;
/// Distance between adjacent columns / rows. A fixed pitch, so the diagram grows
/// with the graph instead of being compressed into a fixed canvas — the Scene in
/// `gui/mod.rs` supplies zoom and pan.
const COL_PITCH: f32 = 190.0;
const ROW_PITCH: f32 = 74.0;
/// Where a wire turns down before entering its target, and how far apart the
/// lanes of several wires into the SAME node are kept.
const LANE_GAP: f32 = 16.0;
const LANE_STEP: f32 = 9.0;

/// Compute a full [`ClockLayout`] from a graph's topology. Deterministic and
/// pure — the same graph always lays out identically.
///
/// Two steps, separated so an editor can re-run only the second one: [`place`]
/// decides WHERE the nodes go, [`derive`] turns positions into drawable
/// primitives. After dragging a node you keep its box and call `derive` again.
pub fn auto_layout(graph: &ClockGraph) -> ClockLayout {
    derive(graph, place(graph))
}

/// Lay the graph out: one [`NodeBox`] per node, in `graph.nodes` order.
pub fn place(graph: &ClockGraph) -> Vec<NodeBox> {
    let n = graph.nodes.len();
    if n == 0 {
        return Vec::new();
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
        if let (Some(&u), Some(&v)) = (idx_of.get(e.from.as_str()), idx_of.get(e.to.as_str())) {
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

    // ── Place at a fixed pitch ───────────────────────────────────────────────
    // Coordinates are absolute, NOT squeezed into a fixed canvas: the renderer
    // allocates whatever the layout measures and the pan/zoom Scene handles the
    // viewport. Compressing a large tree into 1000×790 only made every node
    // smaller and its labels unreadable.
    let max_rows = by_layer.iter().map(|c| c.len()).max().unwrap_or(1);
    let full_h = ROW_PITCH * max_rows.saturating_sub(1) as f32 + NODE_H;

    let mut boxes: Vec<NodeBox> = graph
        .nodes
        .iter()
        .map(|nd| NodeBox {
            node: nd.id.clone(),
            x: 0.0,
            y: 0.0,
            w: NODE_W,
            h: NODE_H,
        })
        .collect();
    for (l, col) in by_layer.iter().enumerate() {
        let x = MARGIN + COL_PITCH * l as f32;
        // Centre each column against the tallest one, so the tree reads as a
        // spine rather than a top-aligned staircase.
        let col_h = ROW_PITCH * col.len().saturating_sub(1) as f32;
        let y0 = MARGIN + (full_h - col_h - NODE_H).max(0.0) / 2.0;
        for (r, &i) in col.iter().enumerate() {
            boxes[i].x = x;
            boxes[i].y = y0 + ROW_PITCH * r as f32;
        }
    }
    boxes
}

/// Keep the boxes a layout already has and place the nodes that lack one.
///
/// The case this exists for: a second AI pass (or the palette) adds nodes to a
/// tree the user has already arranged. Re-running [`place`] would throw that
/// arrangement away; this only fills the gaps, stacking newcomers below the
/// current extent where they are visible and easy to drag into place.
/// Boxes naming nodes the graph no longer has are dropped.
pub fn place_missing(graph: &ClockGraph, boxes: Vec<NodeBox>) -> Vec<NodeBox> {
    let mut kept: Vec<NodeBox> = boxes
        .into_iter()
        .filter(|b| graph.nodes.iter().any(|n| n.id == b.node))
        .collect();
    let bottom = kept
        .iter()
        .map(|b| b.y + b.h)
        .fold(0.0_f32, f32::max)
        .max(MARGIN);

    let missing: Vec<&str> = graph
        .nodes
        .iter()
        .map(|n| n.id.as_str())
        .filter(|id| !kept.iter().any(|b| b.node == *id))
        .collect();
    // Lay them out in rows under the diagram, so a big second pass doesn't end
    // up as one unreachable column.
    const PER_ROW: usize = 6;
    for (i, id) in missing.iter().enumerate() {
        kept.push(NodeBox {
            node: (*id).to_owned(),
            x: MARGIN + COL_PITCH * (i % PER_ROW) as f32,
            y: bottom + ROW_PITCH * (1 + i / PER_ROW) as f32,
            w: NODE_W,
            h: NODE_H,
        });
    }
    kept
}

/// The choices a node offers, as `(label, state to apply)` — or `None` when the
/// node has nothing to pick.
///
/// The single source of both the diagram's dropdowns and the fields view's, so
/// the two cannot drift apart. A `Source` is deliberately absent: its value is a
/// frequency to type, not a choice from a list.
pub fn options_for(graph: &ClockGraph, node: &Node) -> Option<Vec<(String, NodeState)>> {
    Some(match &node.kind {
        NodeKind::Mux { inputs } => (0..*inputs)
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
            .collect(),
        NodeKind::Divider { options } => options
            .iter()
            .enumerate()
            .map(|(k, v)| (format!("/{v}"), NodeState::Index(k)))
            .collect(),
        NodeKind::Choice { ratios } => ratios
            .iter()
            .enumerate()
            .map(|(k, (num, den))| {
                let label = if *den == 1 {
                    format!("×{num}")
                } else {
                    format!("×{num}/{den}")
                };
                (label, NodeState::Index(k))
            })
            .collect(),
        NodeKind::Multiplier { min, max } => (*min..=*max)
            .map(|v| (format!("×{v}"), NodeState::Value(v)))
            .collect(),
        // An EN box is a two-state pick, so it reuses the same dropdown.
        NodeKind::Gate => vec![
            ("EN on".to_owned(), NodeState::Fixed),
            ("EN off".to_owned(), NodeState::Unset),
        ],
        NodeKind::Source { .. }
        | NodeKind::FixedDiv { .. }
        | NodeKind::TimerMul { .. }
        | NodeKind::Tap
        | NodeKind::Output => return None,
    })
}

// ── Re-spacing an imported figure ─────────────────────────────────────────────

/// Two vendor coordinates this close mean the same column (or row) — CubeMX
/// jitters them by a pixel or two.
const CLUSTER_TOL: f32 = 12.0;
/// Blank space between two columns, after the widest box in the left one.
const COL_GAP: f32 = 26.0;
/// What one row costs us: the id label above, the 26 px control, the frequency
/// tag below, and air. CubeMX's own rows are ~58 px apart and carry neither
/// label nor tag, which is why its coordinates cannot be used as they are.
const IMPORT_ROW_PITCH: f32 = 66.0;
const MIN_NODE_W: f32 = 64.0;
const MAX_NODE_W: f32 = 168.0;

/// Roughly how wide a node needs to be, from the text it has to hold.
///
/// The id sits ABOVE the control and is usually the longer of the two —
/// `HSEPLLsourceDevisor` is 19 characters — so it, not the control, decides the
/// footprint. Approximated rather than measured: laying out needs a width before
/// there is a font to ask, and being 10% out only costs a little air.
fn node_width(id: &str) -> f32 {
    (id.chars().count() as f32 * 6.4 + 14.0).clamp(MIN_NODE_W, MAX_NODE_W)
}

/// Group sorted coordinates into clusters, returning each value's cluster index.
fn cluster(values: &[f32]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
    let mut ix = vec![0usize; values.len()];
    let mut current = 0usize;
    let mut anchor = f32::NEG_INFINITY;
    for &i in &order {
        if anchor == f32::NEG_INFINITY {
            anchor = values[i];
        } else if values[i] - anchor > CLUSTER_TOL {
            current += 1;
            anchor = values[i];
        }
        ix[i] = current;
    }
    ix
}

/// Re-space an imported figure: keep the vendor's ARRANGEMENT, use our pitch.
///
/// A CubeMX figure cannot be pasted in at its own coordinates. Measured on
/// `STM32F3.xml`: 34 columns, of which **33 sit closer together than the 96 px
/// box we were drawing**, and 22 vertical pairs closer than the ~50 px a node
/// needs — the tightest 1 px and 2 px apart. Overlap was arithmetic, not bad
/// luck: CubeMX draws small glyphs at natural widths and carries neither an id
/// label above nor a frequency below.
///
/// So the vendor's numbers are read as ORDER rather than position: x-clusters
/// become columns, y-clusters become rows, and both are re-laid at a pitch that
/// fits what we actually draw. What survives is everything a reader uses — which
/// node is left of which, and which nodes share a row — while overlap becomes
/// impossible by construction.
///
/// **Aligned rows are also what straightens the wires.** [`derive`] already
/// emits a bend-free wire when a source and its target sit at the same height;
/// with arbitrary vendor y's that almost never happened, and with a row grid it
/// happens for every single-input link.
///
/// Import-only, deliberately: running this over a layout the user has dragged
/// would throw their arrangement away.
pub fn respace(graph: &ClockGraph, boxes: Vec<NodeBox>) -> Vec<NodeBox> {
    if boxes.is_empty() {
        return boxes;
    }
    let cols = cluster(&boxes.iter().map(|b| b.x).collect::<Vec<_>>());
    let rows = cluster(&boxes.iter().map(|b| b.y).collect::<Vec<_>>());

    // Column x is cumulative, so a column of long names widens only itself.
    let n_cols = cols.iter().copied().max().unwrap_or(0) + 1;
    let mut col_w = vec![MIN_NODE_W; n_cols];
    for (b, &c) in boxes.iter().zip(&cols) {
        col_w[c] = col_w[c].max(node_width(&b.node));
    }
    let mut col_x = Vec::with_capacity(n_cols);
    let mut x = MARGIN;
    for w in &col_w {
        col_x.push(x);
        x += w + COL_GAP;
    }

    // Two nodes can share a cell (the vendor stacked them within the tolerance);
    // the later one takes the next free row in its column so nothing collides.
    let mut taken: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let mut row_of: Vec<usize> = Vec::with_capacity(boxes.len());
    for (&c, &r0) in cols.iter().zip(&rows) {
        let mut r = r0;
        while !taken.insert((c, r)) {
            r += 1;
        }
        row_of.push(r);
    }

    // Straighten the simple chains. A node fed by exactly one source, sitting in
    // a later column, is pulled onto that source's row when the cell is free —
    // which turns its wire into a bend-free horizontal run. Left to right, so a
    // chain straightens along its whole length rather than only at the first
    // link. Anything with several inputs is left alone: a mux must show its
    // inputs arriving separately, and only one of them could be straight anyway.
    let index: std::collections::HashMap<&str, usize> = boxes
        .iter()
        .enumerate()
        .map(|(i, b)| (b.node.as_str(), i))
        .collect();
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by_key(|&i| cols[i]);
    for i in order {
        let feeds: Vec<&Edge> = graph
            .edges
            .iter()
            .filter(|e| e.to == boxes[i].node)
            .collect();
        let [only] = feeds[..] else { continue };
        let Some(&src) = index.get(only.from.as_str()) else {
            continue;
        };
        let want = row_of[src];
        if cols[src] < cols[i] && want != row_of[i] && taken.insert((cols[i], want)) {
            taken.remove(&(cols[i], row_of[i]));
            row_of[i] = want;
        }
    }

    boxes
        .iter()
        .enumerate()
        .map(|(i, b)| NodeBox {
            node: b.node.clone(),
            x: col_x[cols[i]],
            y: MARGIN + row_of[i] as f32 * IMPORT_ROW_PITCH,
            w: col_w[cols[i]],
            h: NODE_H,
        })
        .collect()
}

/// Turn node positions into a drawable [`ClockLayout`]: per node a name label, a
/// control and a live frequency tag; per edge an orthogonal wire. `boxes` is
/// kept in [`ClockLayout::nodes`] as the layout's source of truth.
///
/// Nodes without a box are skipped (nothing to draw them at); boxes naming a
/// node the graph doesn't have are ignored. So a stale saved layout degrades
/// instead of panicking.
pub fn derive(graph: &ClockGraph, boxes: Vec<NodeBox>) -> ClockLayout {
    let mut lay = ClockLayout::default();
    let box_of: HashMap<&str, &NodeBox> = boxes.iter().map(|b| (b.node.as_str(), b)).collect();

    // ── Emit one control + label + freq tag per node ─────────────────────────
    for node in &graph.nodes {
        let Some(nb) = box_of.get(node.id.as_str()) else {
            continue;
        };
        let (x, y, w, h) = (nb.x, nb.y, nb.w, nb.h);
        lay.labels_above.push(LabelDef {
            x,
            y: y - 3.0,
            text: node.id.clone(),
            node: Some(node.id.clone()),
        });
        lay.tags.push(TagDef {
            x,
            y: y + h + 12.0,
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
                    w,
                    min_mhz: lo.max(0.0),
                    max_mhz: (*max_hz as f32 / 1e6).max(lo),
                });
            }
            // Everything selectable is one dropdown, and its options come from
            // the SHARED builder — so the fields view offers exactly the same
            // choices as the diagram, by construction rather than by agreement.
            NodeKind::Mux { .. }
            | NodeKind::Divider { .. }
            | NodeKind::Choice { .. }
            | NodeKind::Multiplier { .. }
            | NodeKind::Gate => {
                if let Some(options) = options_for(graph, node) {
                    lay.widgets.push(Widget::Combo {
                        node: node.id.clone(),
                        x,
                        y,
                        w,
                        options,
                    });
                }
            }
            // Non-editable nodes render as a static labelled box.
            NodeKind::FixedDiv { by } => {
                lay.blocks.push(box_at(nb, format!("/{by}")));
            }
            NodeKind::TimerMul { .. } => {
                lay.blocks.push(box_at(nb, "×tim".to_string()));
            }
            NodeKind::Tap | NodeKind::Output => {
                lay.blocks.push(box_at(nb, node.id.clone()));
            }
        }
    }

    // ── Orthogonal wires along the edges ─────────────────────────────────────
    // Each wire leaves the source's right edge, runs down a vertical lane just
    // before the target, and enters the target's left edge. Wires into the same
    // node get their own lane and their own entry height, so a multi-input mux
    // shows its inputs separately instead of one wire hiding the others.
    for e in &graph.edges {
        let (Some(from), Some(to)) = (
            box_of.get(e.from.as_str()).copied(),
            box_of.get(e.to.as_str()).copied(),
        ) else {
            continue;
        };
        let mut incoming: Vec<&Edge> = graph.edges.iter().filter(|o| o.to == e.to).collect();
        incoming.sort_by(|a, b| (a.input, &a.from).cmp(&(b.input, &b.from)));
        let rank = incoming
            .iter()
            .position(|o| o.from == e.from && o.input == e.input)
            .unwrap_or(0);
        let fan = incoming.len().max(1);

        let sx = from.x + from.w;
        let sy = from.y + from.h / 2.0;
        // Spread the entry points down the target's left edge.
        let ty = to.y + to.h * (rank + 1) as f32 / (fan + 1) as f32;

        if (sy - ty).abs() < 0.5 {
            lay.wires.push(vec![(sx, sy), (to.x, ty)]); // straight run, no bend
            continue;
        }
        // One lane per incoming wire, walking left from the target.
        let lane = to.x - LANE_GAP - LANE_STEP * rank as f32;
        // No room for lanes (target level with, or behind, the source).
        let lane = if lane > sx + 4.0 {
            lane
        } else {
            (sx + to.x) / 2.0
        };
        lay.wires
            .push(vec![(sx, sy), (lane, sy), (lane, ty), (to.x, ty)]);
    }

    lay.nodes = boxes;
    lay
}

fn box_at(nb: &NodeBox, label: String) -> BlockDef {
    BlockDef {
        x: nb.x,
        y: nb.y,
        w: nb.w,
        h: nb.h,
        label,
        node: Some(nb.node.clone()),
    }
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
        Node {
            id: id.into(),
            kind,
            state,
            limit: None,
        }
    }
    fn edge(from: &str, to: &str) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            input: 0,
        }
    }

    #[test]
    fn empty_graph_yields_empty_layout() {
        let lay = auto_layout(&ClockGraph {
            nodes: vec![],
            edges: vec![],
        });
        assert!(lay.is_empty());
    }

    #[test]
    fn source_divider_output_lays_out_left_to_right_and_in_canvas() {
        let g = ClockGraph {
            nodes: vec![
                node(
                    "hsi",
                    NodeKind::Source {
                        min_hz: 16_000_000,
                        max_hz: 16_000_000,
                        gated: true,
                    },
                    NodeState::Source {
                        enabled: true,
                        hz: 16_000_000,
                    },
                ),
                node(
                    "ahb",
                    NodeKind::Divider {
                        options: vec![1, 2, 4],
                    },
                    NodeState::Index(0),
                ),
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

        // Everything stays inside the measured extent (which is now the canvas).
        let (w, h) = lay.bounds();
        for l in &lay.labels_above {
            assert!(
                l.x >= 0.0 && l.x <= w && l.y >= 0.0 && l.y <= h,
                "{l:?} outside the measured bounds {w}×{h}"
            );
        }
    }

    /// The diagram and the fields view must offer the SAME choices — they do,
    /// because both ask this one function.
    #[test]
    fn options_are_built_once_for_both_views() {
        let g = chain();
        let div = g.node("ahb").unwrap();
        let opts = options_for(&g, div).expect("a divider is selectable");
        assert_eq!(
            opts.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
            ["/1", "/2", "/4"]
        );
        assert_eq!(opts[1].1, NodeState::Index(1));

        // And the layout's dropdown carries exactly that list.
        let lay = auto_layout(&g);
        let Some(Widget::Combo { options, .. }) = lay.widgets.iter().find(|w| w.node_id() == "ahb")
        else {
            panic!("the divider draws a dropdown");
        };
        assert_eq!(options, &opts);
    }

    /// A node with nothing to pick says so, rather than offering an empty list.
    #[test]
    fn nodes_with_nothing_to_pick_have_no_options() {
        let g = chain();
        for id in ["hsi", "hclk"] {
            assert!(
                options_for(&g, g.node(id).unwrap()).is_none(),
                "`{id}` has nothing to select"
            );
        }
    }

    /// A big graph GROWS instead of being squeezed: with the old fixed canvas
    /// the columns were compressed to fit 1000×790, which shrank every node.
    #[test]
    fn a_large_graph_outgrows_the_old_fixed_canvas() {
        // A 12-deep chain (12 columns) with a 20-wide fan-out at the end.
        let mut nodes = vec![node(
            "src",
            NodeKind::Source {
                min_hz: 8_000_000,
                max_hz: 8_000_000,
                gated: false,
            },
            NodeState::Source {
                enabled: true,
                hz: 8_000_000,
            },
        )];
        let mut edges = Vec::new();
        let mut prev = "src".to_string();
        for i in 0..11 {
            let id = format!("tap{i}");
            nodes.push(node(&id, NodeKind::Tap, NodeState::Fixed));
            edges.push(edge(&prev, &id));
            prev = id;
        }
        for i in 0..20 {
            let id = format!("out{i}");
            nodes.push(node(&id, NodeKind::Output, NodeState::Fixed));
            edges.push(edge(&prev, &id));
        }

        let lay = auto_layout(&ClockGraph { nodes, edges });
        let (w, h) = lay.bounds();
        assert!(
            w > 1000.0,
            "13 columns must exceed the old 1000 wide, got {w}"
        );
        assert!(h > 790.0, "20 rows must exceed the old 790 tall, got {h}");

        // The pitch is fixed, so adjacent columns stay a readable distance apart
        // however many there are.
        let x = |id: &str| lay.labels_above.iter().find(|l| l.text == id).unwrap().x;
        assert_eq!(x("tap1") - x("tap0"), COL_PITCH);
    }

    /// A three-node chain used by the box/derive tests.
    fn chain() -> ClockGraph {
        ClockGraph {
            nodes: vec![
                node(
                    "hsi",
                    NodeKind::Source {
                        min_hz: 16_000_000,
                        max_hz: 16_000_000,
                        gated: true,
                    },
                    NodeState::Source {
                        enabled: true,
                        hz: 16_000_000,
                    },
                ),
                node(
                    "ahb",
                    NodeKind::Divider {
                        options: vec![1, 2, 4],
                    },
                    NodeState::Index(0),
                ),
                node("hclk", NodeKind::Output, NodeState::Fixed),
            ],
            edges: vec![edge("hsi", "ahb"), edge("ahb", "hclk")],
        }
    }

    /// The layout now carries a box per node — the handle an editor drags.
    #[test]
    fn every_node_gets_a_box() {
        let g = chain();
        let lay = auto_layout(&g);
        let ids: Vec<&str> = lay.nodes.iter().map(|b| b.node.as_str()).collect();
        assert_eq!(ids, ["hsi", "ahb", "hclk"], "one box per node, in order");
        assert!(lay.nodes.iter().all(|b| b.w > 0.0 && b.h > 0.0));
    }

    /// Moving a box and re-deriving moves the node's label, control and tag with
    /// it — that is what makes the boxes the source of truth (Phase 3's drag).
    #[test]
    fn moving_a_box_moves_the_whole_node() {
        let g = chain();
        let mut boxes = place(&g);
        let before = derive(&g, boxes.clone());

        let ahb = boxes.iter_mut().find(|b| b.node == "ahb").unwrap();
        let (ox, oy) = (ahb.x, ahb.y);
        ahb.x += 40.0;
        ahb.y += 25.0;
        let after = derive(&g, boxes);

        let label = |l: &ClockLayout| {
            let e = l
                .labels_above
                .iter()
                .find(|e| e.text == "ahb")
                .expect("label");
            (e.x, e.y)
        };
        let control = |l: &ClockLayout| match l.widgets.iter().find(|w| w.node_id() == "ahb") {
            Some(Widget::Combo { x, y, .. }) => (*x, *y),
            other => panic!("expected the divider's dropdown, got {other:?}"),
        };
        assert_eq!(control(&before), (ox, oy));
        assert_eq!(control(&after), (ox + 40.0, oy + 25.0));
        assert_eq!(label(&after).0 - label(&before).0, 40.0);
        assert_eq!(label(&after).1 - label(&before).1, 25.0);
        // The frequency tag rides along too.
        let tag_y = |l: &ClockLayout| {
            l.tags
                .iter()
                .find(|t| t.src == ValueSrc::Node("ahb".into()))
                .unwrap()
                .y
        };
        assert_eq!(tag_y(&after) - tag_y(&before), 25.0);
    }

    /// Wires are derived from the edges and meet the boxes they connect.
    #[test]
    fn wires_run_between_the_box_edges() {
        let g = chain();
        let lay = auto_layout(&g);
        let bx = |id: &str| lay.nodes.iter().find(|b| b.node == id).unwrap().clone();
        let (hsi, ahb) = (bx("hsi"), bx("ahb"));

        let w = lay
            .wires
            .iter()
            .find(|w| (w[0].0 - (hsi.x + hsi.w)).abs() < 0.01)
            .expect("a wire leaving hsi's right edge");
        let end = *w.last().unwrap();
        assert!(
            (end.0 - ahb.x).abs() < 0.01,
            "wire must end on ahb's left edge, got {end:?}"
        );
        assert!(
            end.1 >= ahb.y && end.1 <= ahb.y + ahb.h,
            "entry point must be within ahb's height"
        );
    }

    /// Several inputs into one mux each get their own entry height, so they stay
    /// distinguishable instead of overlapping into a single line.
    #[test]
    fn wires_into_one_mux_do_not_overlap() {
        let src = |id: &str, hz: u32| {
            node(
                id,
                NodeKind::Source {
                    min_hz: hz,
                    max_hz: hz,
                    gated: false,
                },
                NodeState::Source { enabled: true, hz },
            )
        };
        let g = ClockGraph {
            nodes: vec![
                src("a", 8_000_000),
                src("b", 16_000_000),
                src("c", 32_000_000),
                node("sw", NodeKind::Mux { inputs: 3 }, NodeState::Index(0)),
            ],
            edges: vec![
                Edge {
                    from: "a".into(),
                    to: "sw".into(),
                    input: 0,
                },
                Edge {
                    from: "b".into(),
                    to: "sw".into(),
                    input: 1,
                },
                Edge {
                    from: "c".into(),
                    to: "sw".into(),
                    input: 2,
                },
            ],
        };
        let lay = auto_layout(&g);
        let sw = lay.nodes.iter().find(|b| b.node == "sw").unwrap();
        let mut entries: Vec<f32> = lay
            .wires
            .iter()
            .map(|w| w.last().unwrap().1)
            .filter(|y| *y >= sw.y && *y <= sw.y + sw.h)
            .collect();
        entries.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(entries.len(), 3);
        for pair in entries.windows(2) {
            assert!(
                pair[1] - pair[0] > 1.0,
                "entry points must be distinct, got {entries:?}"
            );
        }
    }

    /// A second AI pass (or the palette) adds nodes to a tree the user has
    /// already arranged: the arrangement survives, the newcomers get a spot.
    #[test]
    fn place_missing_keeps_the_arrangement_and_fills_the_gaps() {
        let g = chain();
        let mut boxes = place(&g);
        // The user dragged one somewhere deliberate.
        let ahb = boxes.iter_mut().find(|b| b.node == "ahb").unwrap();
        ahb.x = 700.0;
        ahb.y = 40.0;
        let arranged = boxes.clone();

        // A branch arrives with two new nodes; one old box is now stale.
        let mut grown = g.clone();
        for id in ["lse", "rtc"] {
            grown.nodes.push(node(id, NodeKind::Tap, NodeState::Fixed));
        }
        boxes.push(NodeBox {
            node: "gone".into(),
            x: 0.0,
            y: 0.0,
            w: 96.0,
            h: 26.0,
        });

        let out = place_missing(&grown, boxes);
        assert_eq!(out.len(), 5, "3 kept + 2 placed, the stale one dropped");
        for old in &arranged {
            let same = out.iter().find(|b| b.node == old.node).unwrap();
            assert_eq!((same.x, same.y), (old.x, old.y), "{} moved", old.node);
        }
        let below = arranged.iter().map(|b| b.y + b.h).fold(0.0, f32::max);
        for id in ["lse", "rtc"] {
            let b = out.iter().find(|b| b.node == id).unwrap();
            assert!(b.y > below, "`{id}` must land below the existing diagram");
        }
        assert!(!out.iter().any(|b| b.node == "gone"));
    }

    /// An `EN` gate gets the same dropdown treatment as a mux (on / off).
    #[test]
    fn a_gate_gets_an_on_off_dropdown() {
        let g = ClockGraph {
            nodes: vec![
                node(
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
                node("en", NodeKind::Gate, NodeState::Fixed),
            ],
            edges: vec![edge("lse", "en")],
        };
        let lay = auto_layout(&g);
        let w = lay
            .widgets
            .iter()
            .find(|w| w.node_id() == "en")
            .expect("gate control");
        let Widget::Combo { options, .. } = w else {
            panic!("a gate is a two-option dropdown, got {w:?}");
        };
        assert_eq!(options.len(), 2);
        assert!(options.iter().any(|(_, s)| *s == NodeState::Fixed));
        assert!(options.iter().any(|(_, s)| *s == NodeState::Unset));
    }
}
