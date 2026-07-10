//! Layered ("Sugiyama-lite") layout for the module graph (pure logic, tested).
//!
//! Virtual coordinates, origin top-left. The crate root lands on layer 0;
//! every edge (dependency or containment) pushes its target below its source
//! (longest-path layering, cycle-tolerant via bounded relaxation). Node order
//! inside a layer is refined with a couple of barycenter sweeps so edges cross
//! less; the GUI then scales the whole thing to the panel.

use super::parse::ModuleGraph;

/// Virtual-unit constants (scaled by the GUI to fit the panel).
const CHAR_W: f32 = 7.0;
const PAD_X: f32 = 22.0;
const MIN_W: f32 = 74.0;
const MAX_W: f32 = 220.0;
/// Header band: module name + fn/ty badge (a node with no symbol rows is just
/// this tall — the Phase-1 look).
pub const HEADER_H: f32 = 40.0;
/// One symbol row (Phase 2: top-level fn/struct/enum/trait listed in the node).
pub const ROW_H: f32 = 13.0;
/// Rows shown before truncating to a "+K more" row.
pub const MAX_SYMBOL_ROWS: usize = 8;
/// Symbol rows use a smaller font than the header.
const ROW_CHAR_W: f32 = 5.6;
/// Max chars of a symbol name used for width (GUI truncates the text to match).
pub const ROW_NAME_CHARS: usize = 26;
const ROW_PAD_BOTTOM: f32 = 6.0;
const H_GAP: f32 = 30.0;
const V_GAP: f32 = 52.0;
const MARGIN: f32 = 14.0;

/// Rows drawn for a node: every symbol, or MAX + a trailing "+K more" row.
pub fn shown_rows(symbol_count: usize) -> usize {
    if symbol_count > MAX_SYMBOL_ROWS {
        MAX_SYMBOL_ROWS + 1
    } else {
        symbol_count
    }
}

/// A node's virtual-space rectangle.
#[derive(Clone, Copy, Debug)]
pub struct NodePos {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl NodePos {
    pub fn center_x(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Positions for every node + the total virtual-space bounds.
#[derive(Clone, Debug, Default)]
pub struct GraphLayout {
    pub pos: Vec<NodePos>,
    pub width: f32,
    pub height: f32,
}

/// Compute the layered layout for `graph`.
pub fn layout(graph: &ModuleGraph) -> GraphLayout {
    let n = graph.nodes.len();
    if n == 0 {
        return GraphLayout::default();
    }
    let edges: Vec<(usize, usize)> = graph
        .deps
        .iter()
        .chain(graph.contains.iter())
        .copied()
        .collect();

    // ── Layering: bounded longest-path relaxation (cycle-safe) ────────────
    // layer[target] ≥ layer[source] + 1 for every edge; at most `n` sweeps, so
    // a dependency cycle can't loop forever (its layers just stop growing).
    let mut layer = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for &(u, v) in &edges {
            if layer[v] < layer[u] + 1 && layer[u] + 1 < n {
                layer[v] = layer[u] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // ── Group into layers (stable initial order = node index) ─────────────
    let max_layer = layer.iter().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); max_layer + 1];
    for (i, &l) in layer.iter().enumerate() {
        layers[l].push(i);
    }

    // Neighbor sets (both directions) for the barycenter heuristic.
    let mut neighbors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(u, v) in &edges {
        neighbors[u].push(v);
        neighbors[v].push(u);
    }

    // Node width: the widest of the module name and its symbol rows (each row
    // is "<glyph> <name>", name capped — the GUI truncates to match).
    let width_of = |i: usize| -> f32 {
        let node = &graph.nodes[i];
        let name_w = node.name.chars().count() as f32 * CHAR_W;
        let row_w = node
            .symbols
            .iter()
            .map(|s| (s.name.chars().count().min(ROW_NAME_CHARS) + 3) as f32 * ROW_CHAR_W)
            .fold(0.0, f32::max);
        (name_w.max(row_w) + PAD_X).clamp(MIN_W, MAX_W)
    };
    // Node height: header band + the symbol rows (none → Phase-1 compact box).
    let height_of = |i: usize| -> f32 {
        let rows = shown_rows(graph.nodes[i].symbols.len());
        if rows == 0 {
            HEADER_H
        } else {
            HEADER_H + rows as f32 * ROW_H + ROW_PAD_BOTTOM
        }
    };

    // Sequentially pack a layer around x = 0, returning each node's center x.
    let pack = |order: &[usize], centers: &mut [f32]| {
        if order.is_empty() {
            return; // defensive — layering leaves no gaps, but don't underflow
        }
        let total: f32 =
            order.iter().map(|&i| width_of(i)).sum::<f32>() + H_GAP * (order.len() - 1) as f32;
        let mut x = -total / 2.0;
        for &i in order {
            let w = width_of(i);
            centers[i] = x + w / 2.0;
            x += w + H_GAP;
        }
    };

    let mut centers = vec![0.0f32; n];
    for l in &layers {
        pack(l, &mut centers);
    }

    // ── Barycenter sweeps: sort each layer by the mean center of its
    //    neighbors, then repack. Two down-up passes settle small graphs. ────
    for _ in 0..2 {
        for rev in [false, true] {
            let order: Vec<usize> = if rev {
                (0..layers.len()).rev().collect()
            } else {
                (0..layers.len()).collect()
            };
            for li in order {
                let mut keyed: Vec<(f32, usize)> = layers[li]
                    .iter()
                    .map(|&i| {
                        let ns = &neighbors[i];
                        let key = if ns.is_empty() {
                            centers[i]
                        } else {
                            ns.iter().map(|&j| centers[j]).sum::<f32>() / ns.len() as f32
                        };
                        (key, i)
                    })
                    .collect();
                keyed.sort_by(|a, b| {
                    a.0.partial_cmp(&b.0)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| graph.nodes[a.1].name.cmp(&graph.nodes[b.1].name))
                });
                layers[li] = keyed.into_iter().map(|(_, i)| i).collect();
                pack(&layers[li], &mut centers);
            }
        }
    }

    // ── Final positions ────────────────────────────────────────────────────
    // Y: layers stack on the TALLEST node of each layer above (heights vary
    // with the symbol-row count). X: shift so the leftmost node is at MARGIN.
    let mut layer_y = vec![MARGIN; max_layer + 1];
    for l in 1..=max_layer {
        let above_h = layers[l - 1]
            .iter()
            .map(|&i| height_of(i))
            .fold(0.0, f32::max);
        layer_y[l] = layer_y[l - 1] + above_h + V_GAP;
    }
    let min_cx = (0..n)
        .map(|i| centers[i] - width_of(i) / 2.0)
        .fold(f32::MAX, f32::min);
    let mut pos = vec![
        NodePos { x: 0.0, y: 0.0, w: 0.0, h: HEADER_H };
        n
    ];
    let mut max_x = 0.0f32;
    let mut max_y = 0.0f32;
    for i in 0..n {
        let (w, h) = (width_of(i), height_of(i));
        let x = centers[i] - w / 2.0 - min_cx + MARGIN;
        let y = layer_y[layer[i]];
        pos[i] = NodePos { x, y, w, h };
        max_x = max_x.max(x + w);
        max_y = max_y.max(y + h);
    }

    GraphLayout { pos, width: max_x + MARGIN, height: max_y + MARGIN }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::parse::build_graph;
    use super::*;

    #[test]
    fn root_is_on_top_and_deps_below() {
        let main_rs = "mod a;\nuse crate::a::f;\n".to_owned();
        let files = vec![("a.rs".into(), "pub fn f() {}\n".into())];
        let g = build_graph(&main_rs, &files);
        let lay = layout(&g);
        assert_eq!(lay.pos.len(), 2);
        assert!(
            lay.pos[0].y < lay.pos[1].y,
            "main must sit above the module it uses"
        );
        assert!(lay.width > 0.0 && lay.height > 0.0);
    }

    #[test]
    fn cycle_terminates_and_separates_nodes() {
        let main_rs = "mod a;\nmod b;\n".to_owned();
        let files = vec![
            ("a.rs".into(), "use crate::b::f;\n".into()),
            ("b.rs".into(), "use crate::a::g;\n".into()),
        ];
        let g = build_graph(&main_rs, &files);
        let lay = layout(&g); // must not hang
        assert_eq!(lay.pos.len(), 3);
        // No two nodes on the same layer overlap horizontally.
        for i in 0..lay.pos.len() {
            for j in i + 1..lay.pos.len() {
                let (a, b) = (lay.pos[i], lay.pos[j]);
                if (a.y - b.y).abs() < 1.0 {
                    let overlap = a.x < b.x + b.w && b.x < a.x + a.w;
                    assert!(!overlap, "nodes {i} and {j} overlap");
                }
            }
        }
    }

    #[test]
    fn single_node_project_lays_out() {
        let g = build_graph("fn main() {}\n", &[]);
        let lay = layout(&g);
        assert_eq!(lay.pos.len(), 1);
        assert!(lay.width >= lay.pos[0].w);
    }

    #[test]
    fn symbol_rows_grow_the_node_and_layers_stack_below() {
        // `a` has 3 top-level items → taller node; `b` has none → header only.
        let main_rs = "mod a;\nmod b;\nuse crate::a::f1;\n".to_owned();
        let files = vec![
            (
                "a.rs".into(),
                "pub fn f1() {}\npub fn f2() {}\npub struct S;\n".into(),
            ),
            ("b.rs".into(), "// empty\n".into()),
        ];
        let g = build_graph(&main_rs, &files);
        let a = g.nodes.iter().position(|n| n.path == "a").unwrap();
        let b = g.nodes.iter().position(|n| n.path == "b").unwrap();
        let lay = layout(&g);
        assert_eq!(lay.pos[b].h, HEADER_H, "no symbols → compact header-only box");
        assert!(
            lay.pos[a].h > HEADER_H,
            "symbol rows must grow the node height"
        );
        // The layer below main starts below main's bottom edge (no overlap).
        assert!(lay.pos[a].y >= lay.pos[0].bottom());
        assert!(lay.height >= lay.pos[a].y + lay.pos[a].h);
    }

    #[test]
    fn truncated_symbol_list_caps_rows() {
        assert_eq!(shown_rows(0), 0);
        assert_eq!(shown_rows(MAX_SYMBOL_ROWS), MAX_SYMBOL_ROWS);
        assert_eq!(shown_rows(MAX_SYMBOL_ROWS + 5), MAX_SYMBOL_ROWS + 1);
    }
}
