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
/// Outer margin of the virtual canvas — also the drag clamp (nodes can't be
/// dragged to negative coordinates, which would break the fit math).
pub const MARGIN: f32 = 14.0;

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

/// Compute the layered layout for `graph` (module edges only).
pub fn layout(graph: &ModuleGraph) -> GraphLayout {
    layout_with_calls(graph, &[])
}

/// Compute the layered layout for `graph`, also minimizing overlap against the
/// Phase-3 call edges (`calls` = node-level `(from, to)` pairs). Beyond the
/// barycenter sweeps, a TRANSPOSE pass swaps adjacent nodes within a layer
/// whenever that lowers a geometric cost = edge–edge crossings + a penalty for
/// every edge that passes THROUGH a non-endpoint node box — directly the "paths
/// should overlap as little as possible" objective. One-time cost per layout
/// (cached by content hash), tiny at this graph size.
pub fn layout_with_calls(graph: &ModuleGraph, calls: &[(usize, usize)]) -> GraphLayout {
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
    // Everything the cost/ordering should care about, calls included.
    let all_edges: Vec<(usize, usize)> = edges
        .iter()
        .copied()
        .chain(calls.iter().copied().filter(|(a, b)| a != b))
        .collect();

    // ── Ring assignment: hub-centered layout ──────────────────────────────
    // The crate root (`main`, node 0) sits in the MIDDLE; every other node's
    // ring is its USE distance from main — an undirected BFS over dep/call
    // edges, with containment edges counting only BETWEEN non-main nodes
    // (`mod x;` in main.rs is structure, not usage — so a subtree's leaves
    // land next to main and its root on the far side, the shape the user
    // arranges by hand). Connected components (main removed) go above/below
    // main, size-balanced; the component holding main's first `mod` goes UP;
    // fully isolated modules park on the topmost ring.
    let layer: Vec<usize> = {
        // USE edges = deps + calls (NOT containment — `mod x;` is structure,
        // not usage; a containment hub-link would drag subtree roots next to
        // main and defeat the leaves-near/root-far shape).
        let use_edges: Vec<(usize, usize)> = graph
            .deps
            .iter()
            .copied()
            .chain(calls.iter().copied())
            .filter(|(a, b)| a != b)
            .collect();
        // Undirected adjacency among NON-main nodes: every edge kind.
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &(u, v) in &all_edges {
            if u != 0 && v != 0 && u != v {
                adj[u].push(v);
                adj[v].push(u);
            }
        }
        let mut hub_linked = vec![false; n];
        for &(u, v) in &use_edges {
            if u == 0 && v != 0 {
                hub_linked[v] = true;
            } else if v == 0 && u != 0 {
                hub_linked[u] = true;
            }
        }
        // Directed dep/call in-degree among non-main nodes — for picking BFS
        // entries in components main never references (their "sources").
        let mut in_deg = vec![0usize; n];
        let mut out_deg = vec![0usize; n];
        for &(u, v) in &use_edges {
            if u != 0 && v != 0 {
                in_deg[v] += 1;
                out_deg[u] += 1;
            }
        }

        // Components over nodes 1..n (discovery order = node index order).
        let mut comp_of = vec![usize::MAX; n];
        let mut comps: Vec<Vec<usize>> = Vec::new();
        for start in 1..n {
            if comp_of[start] != usize::MAX {
                continue;
            }
            let id = comps.len();
            let mut members = vec![start];
            comp_of[start] = id;
            let mut qi = 0;
            while qi < members.len() {
                let m = members[qi];
                qi += 1;
                for &next in &adj[m] {
                    if comp_of[next] == usize::MAX {
                        comp_of[next] = id;
                        members.push(next);
                    }
                }
            }
            comps.push(members);
        }

        // Ring of each node inside its component: BFS from the entry set —
        // nodes main references directly; else the component's own dep/call
        // sources; else its lowest-index node.
        let mut ring = vec![0usize; n];
        for members in &comps {
            let mut entry: Vec<usize> =
                members.iter().copied().filter(|&m| hub_linked[m]).collect();
            if entry.is_empty() {
                entry = members
                    .iter()
                    .copied()
                    .filter(|&m| in_deg[m] == 0 && out_deg[m] > 0)
                    .collect();
            }
            if entry.is_empty() {
                entry = vec![*members.iter().min().unwrap()];
            }
            let mut q: std::collections::VecDeque<usize> = entry.iter().copied().collect();
            for &e in &entry {
                ring[e] = 1;
            }
            while let Some(m) = q.pop_front() {
                for &next in &adj[m] {
                    if ring[next] == 0 && next != 0 {
                        ring[next] = ring[m] + 1;
                        q.push_back(next);
                    }
                }
            }
        }

        // Side per component: the one holding main's first `mod` child is UP;
        // the rest greedily balance node counts. Isolated singletons park top.
        let first_child_comp = graph
            .contains
            .iter()
            .filter(|&&(u, v)| u == 0 && v != 0)
            .map(|&(_, v)| comp_of[v])
            .min();
        let mut order: Vec<usize> = (0..comps.len()).collect();
        order.sort_by_key(|&c| {
            (
                if Some(c) == first_child_comp { 0 } else { 1 },
                usize::MAX - comps[c].len(),
                comps[c].iter().min().copied().unwrap_or(usize::MAX),
            )
        });
        let mut row = vec![0i32; n]; // main stays 0
        let (mut up_total, mut down_total) = (0usize, 0usize);
        let mut parked: Vec<usize> = Vec::new();
        for c in order {
            let members = &comps[c];
            let lone = members.len() == 1 && {
                let m = members[0];
                adj[m].is_empty() && !hub_linked[m]
            };
            if lone {
                parked.push(members[0]);
                continue;
            }
            let up = up_total <= down_total;
            for &m in members {
                row[m] = if up { -(ring[m] as i32) } else { ring[m] as i32 };
            }
            if up {
                up_total += members.len();
            } else {
                down_total += members.len();
            }
        }
        let min_row = row.iter().copied().min().unwrap_or(0);
        let park_row = min_row - 1;
        for &m in &parked {
            row[m] = park_row;
        }
        let base = row.iter().copied().min().unwrap_or(0);
        row.iter().map(|&r| (r - base) as usize).collect()
    };

    // ── Group into layers (stable initial order = node index) ─────────────
    let max_layer = layer.iter().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); max_layer + 1];
    for (i, &l) in layer.iter().enumerate() {
        layers[l].push(i);
    }

    // Neighbor sets (both directions) for the barycenter heuristic — call
    // edges participate too, so heavily-calling modules gravitate together.
    let mut neighbors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(u, v) in &all_edges {
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

    // Layer Y baselines — depend only on the layer assignment (tallest node of
    // each layer above), NOT on in-layer order, so they can be fixed here and
    // reused by the geometric cost below and the final positions.
    let mut layer_y = vec![MARGIN; max_layer + 1];
    for l in 1..=max_layer {
        let above_h = layers[l - 1]
            .iter()
            .map(|&i| height_of(i))
            .fold(0.0, f32::max);
        layer_y[l] = layer_y[l - 1] + above_h + V_GAP;
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

    // ── Transpose: adjacent in-layer swaps that reduce path overlap ────────
    // Cost = proper edge–edge crossings (edges sharing a node excluded — port
    // spreading in the GUI separates those) + 4× for every edge that passes
    // THROUGH a non-endpoint node box. Straight-segment geometry approximates
    // the drawn curves well enough to order nodes by.
    let cost = |centers: &[f32]| -> f32 {
        let seg = |u: usize, v: usize| -> ((f32, f32), (f32, f32)) {
            if layer[v] > layer[u] {
                (
                    (centers[u], layer_y[layer[u]] + height_of(u)),
                    (centers[v], layer_y[layer[v]]),
                )
            } else {
                // Back edge (cycle): leaves the top, enters the bottom.
                (
                    (centers[u], layer_y[layer[u]]),
                    (centers[v], layer_y[layer[v]] + height_of(v)),
                )
            }
        };
        let segs: Vec<((f32, f32), (f32, f32), usize, usize)> = all_edges
            .iter()
            .map(|&(u, v)| {
                let (a, b) = seg(u, v);
                (a, b, u, v)
            })
            .collect();
        let mut c = 0.0;
        for i in 0..segs.len() {
            let (a1, a2, u1, v1) = segs[i];
            for &(b1, b2, u2, v2) in segs.iter().skip(i + 1) {
                if u1 == u2 || u1 == v2 || v1 == u2 || v1 == v2 {
                    continue;
                }
                if segments_cross(a1, a2, b1, b2) {
                    c += 1.0;
                }
            }
            for k in 0..n {
                if k == u1 || k == v1 {
                    continue;
                }
                let (w, h) = (width_of(k), height_of(k));
                if seg_hits_rect(a1, a2, centers[k] - w / 2.0, layer_y[layer[k]], w, h) {
                    c += 4.0;
                }
            }
        }
        c
    };
    let mut best = cost(&centers);
    if best > 0.0 {
        for _ in 0..3 {
            let mut improved = false;
            for li in 0..layers.len() {
                for a in 0..layers[li].len().saturating_sub(1) {
                    layers[li].swap(a, a + 1);
                    pack(&layers[li], &mut centers);
                    let c = cost(&centers);
                    if c + 0.01 < best {
                        best = c;
                        improved = true;
                    } else {
                        layers[li].swap(a, a + 1); // revert
                        pack(&layers[li], &mut centers);
                    }
                }
            }
            if !improved || best == 0.0 {
                break;
            }
        }
    }

    // ── Final positions ────────────────────────────────────────────────────
    // X: shift so the leftmost node starts at MARGIN (layer Y fixed above).
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

// ── Manual position overrides (drag & drop) ─────────────────────────────────

/// Re-place every node whose file appears in `overrides` (keyed by
/// `ModuleNode::file_rel` — stable across graph rebuilds, unlike node indices)
/// at its user-dragged position, then refresh the layout bounds. Called after
/// every automatic layout, so pinned nodes stay put while the rest auto-flow.
pub fn apply_overrides(
    lay: &mut GraphLayout,
    graph: &ModuleGraph,
    overrides: &std::collections::BTreeMap<String, (f32, f32)>,
) {
    if overrides.is_empty() {
        return;
    }
    for (i, node) in graph.nodes.iter().enumerate() {
        if let Some(&(x, y)) = overrides.get(&node.file_rel) {
            lay.pos[i].x = x.max(MARGIN);
            lay.pos[i].y = y.max(MARGIN);
        }
    }
    recompute_bounds(lay);
}

/// Refresh `width`/`height` from the node boxes (after drags / overrides).
pub fn recompute_bounds(lay: &mut GraphLayout) {
    let mut max_x = 0.0f32;
    let mut max_y = 0.0f32;
    for p in &lay.pos {
        max_x = max_x.max(p.x + p.w);
        max_y = max_y.max(p.y + p.h);
    }
    lay.width = max_x + MARGIN;
    lay.height = max_y + MARGIN;
}

// ── Geometry helpers (transpose cost + GUI edge routing) ────────────────────

fn orient(a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// `true` when segments `p1–p2` and `q1–q2` PROPERLY cross (shared endpoints
/// and mere touching don't count — those are handled by port spreading).
pub fn segments_cross(p1: (f32, f32), p2: (f32, f32), q1: (f32, f32), q2: (f32, f32)) -> bool {
    let d1 = orient(q1, q2, p1);
    let d2 = orient(q1, q2, p2);
    let d3 = orient(p1, p2, q1);
    let d4 = orient(p1, p2, q2);
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// `true` when segment `p1–p2` passes through the `(x, y, w, h)` rectangle
/// (an endpoint strictly inside, or a proper crossing of any side).
pub fn seg_hits_rect(p1: (f32, f32), p2: (f32, f32), x: f32, y: f32, w: f32, h: f32) -> bool {
    let inside =
        |p: (f32, f32)| p.0 > x && p.0 < x + w && p.1 > y && p.1 < y + h;
    if inside(p1) || inside(p2) {
        return true;
    }
    let c = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    segments_cross(p1, p2, c[0], c[1])
        || segments_cross(p1, p2, c[1], c[2])
        || segments_cross(p1, p2, c[2], c[3])
        || segments_cross(p1, p2, c[3], c[0])
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::parse::build_graph;
    use super::*;

    /// Hub layout: main sits BETWEEN its subtrees — the component holding
    /// main's first `mod` goes above, the next balances below; inside a
    /// subtree the leaves main uses sit NEXT to main and the subtree root on
    /// the far side (the arrangement the user builds by hand).
    #[test]
    fn hub_sits_between_subtrees_with_leaves_adjacent() {
        let main_rs = "\
mod p;
mod m;
use crate::p::cfg::usart::init;
use crate::m::report::decode;
"
        .to_owned();
        let files = vec![
            ("p/mod.rs".into(), "pub mod cfg;\n".into()),
            ("p/cfg/mod.rs".into(), "pub mod usart;\n".into()),
            ("p/cfg/usart.rs".into(), "pub fn init() {}\n".into()),
            ("m/mod.rs".into(), "pub mod report;\n".into()),
            ("m/report.rs".into(), "pub fn decode() {}\n".into()),
        ];
        let g = build_graph(&main_rs, &files);
        let lay = layout(&g);
        let at = |path: &str| {
            let i = g.nodes.iter().position(|n| n.path == path).unwrap();
            lay.pos[i]
        };
        let main = lay.pos[0];
        // p-side above main (first `mod`), m-side below.
        for p in ["p", "p::cfg", "p::cfg::usart"] {
            assert!(at(p).y < main.y, "{p} must sit above main");
        }
        for m in ["m", "m::report"] {
            assert!(at(m).y > main.y, "{m} must sit below main");
        }
        // Leaves adjacent to main, subtree roots on the far side.
        assert!(at("p").y < at("p::cfg").y && at("p::cfg").y < at("p::cfg::usart").y);
        assert!(at("m::report").y < at("m").y);
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
        // Adjacent rings never overlap vertically, whichever side of the hub
        // `a` landed on.
        let (pa, pm) = (lay.pos[a], lay.pos[0]);
        assert!(
            pa.bottom() <= pm.y || pa.y >= pm.bottom(),
            "a's band must not overlap main's"
        );
        assert!(lay.height >= pa.y + pa.h);
    }

    #[test]
    fn truncated_symbol_list_caps_rows() {
        assert_eq!(shown_rows(0), 0);
        assert_eq!(shown_rows(MAX_SYMBOL_ROWS), MAX_SYMBOL_ROWS);
        assert_eq!(shown_rows(MAX_SYMBOL_ROWS + 5), MAX_SYMBOL_ROWS + 1);
    }

    #[test]
    fn segment_geometry_helpers() {
        // An X-crossing…
        assert!(segments_cross((0.0, 0.0), (10.0, 10.0), (0.0, 10.0), (10.0, 0.0)));
        // …parallel lines don't cross, and a shared endpoint doesn't count.
        assert!(!segments_cross((0.0, 0.0), (10.0, 0.0), (0.0, 5.0), (10.0, 5.0)));
        assert!(!segments_cross((0.0, 0.0), (10.0, 10.0), (10.0, 10.0), (20.0, 0.0)));
        // Through the box, ending inside it, and missing it entirely.
        assert!(seg_hits_rect((0.0, 5.0), (20.0, 5.0), 5.0, 0.0, 10.0, 10.0));
        assert!(seg_hits_rect((0.0, 0.0), (8.0, 5.0), 5.0, 0.0, 10.0, 10.0));
        assert!(!seg_hits_rect((0.0, 20.0), (20.0, 20.0), 5.0, 0.0, 10.0, 10.0));
    }

    /// The dep edges `a→y` and `b→x` start out crossed (creation order puts
    /// `x` left of `y` while `a` sits left of `b`); the ordering optimization
    /// must untangle them.
    #[test]
    fn ordering_untangles_crossing_dep_edges() {
        let main_rs = "mod a;\nmod b;\nmod x;\nmod y;\n".to_owned();
        let files = vec![
            ("a.rs".into(), "use crate::y::f;\n".into()),
            ("b.rs".into(), "use crate::x::g;\n".into()),
            ("x.rs".into(), "pub fn g() {}\n".into()),
            ("y.rs".into(), "pub fn f() {}\n".into()),
        ];
        let g = build_graph(&main_rs, &files);
        let lay = layout(&g);
        let at = |p: &str| {
            let i = g.nodes.iter().position(|n| n.path == p).unwrap();
            lay.pos[i]
        };
        let (a, b, x, y) = (at("a"), at("b"), at("x"), at("y"));
        assert!(
            !segments_cross(
                (a.center_x(), a.bottom()),
                (y.center_x(), y.y),
                (b.center_x(), b.bottom()),
                (x.center_x(), x.y),
            ),
            "a→y and b→x must not cross after ordering"
        );
    }

    #[test]
    fn overrides_pin_nodes_by_file_and_grow_bounds() {
        let main_rs = "mod a;\n".to_owned();
        let files = vec![("a.rs".into(), "pub fn f() {}\n".into())];
        let g = build_graph(&main_rs, &files);
        let mut lay = layout(&g);
        let auto_main = (lay.pos[0].x, lay.pos[0].y);

        let mut ov = std::collections::BTreeMap::new();
        ov.insert("a.rs".into(), (400.0, 300.0));
        ov.insert("gone.rs".into(), (9.0, 9.0)); // stale key → ignored
        apply_overrides(&mut lay, &g, &ov);

        assert_eq!((lay.pos[1].x, lay.pos[1].y), (400.0, 300.0), "a.rs pinned");
        assert_eq!((lay.pos[0].x, lay.pos[0].y), auto_main, "main untouched");
        assert!(
            lay.width >= 400.0 + lay.pos[1].w && lay.height >= 300.0 + lay.pos[1].h,
            "bounds must grow to cover the dragged node"
        );

        // Below-margin positions are clamped (negative coords break fit math).
        ov.insert("a.rs".into(), (-50.0, -50.0));
        apply_overrides(&mut lay, &g, &ov);
        assert_eq!((lay.pos[1].x, lay.pos[1].y), (MARGIN, MARGIN));
    }

    /// With NO module edge distinguishing `c` from `d`, a call pair `b → d`
    /// must pull `d` toward `b`'s side so the call edge doesn't cross `a`'s
    /// containment links (calls participate in the ordering).
    #[test]
    fn call_edges_influence_ordering() {
        let main_rs = "mod a;\nmod b;\n".to_owned();
        let files = vec![
            ("a/mod.rs".into(), "pub mod d;\npub mod c;\n".into()),
            ("a/d.rs".into(), "pub fn f() {}\n".into()),
            ("a/c.rs".into(), "pub fn g() {}\n".into()),
            ("b.rs".into(), "pub fn caller() {}\n".into()),
        ];
        let g = build_graph(&main_rs, &files);
        let idx = |p: &str| g.nodes.iter().position(|n| n.path == p).unwrap();
        let (a, ac, ad, b) = (idx("a"), idx("a::c"), idx("a::d"), idx("b"));
        let lay = layout_with_calls(&g, &[(b, ad)]); // b calls into a::d
        let seg_down = |i: usize, j: usize| {
            (
                (lay.pos[i].center_x(), lay.pos[i].bottom()),
                (lay.pos[j].center_x(), lay.pos[j].y),
            )
        };
        let (call_from, call_to) = seg_down(b, ad);
        let (link_from, link_to) = seg_down(a, ac);
        assert!(
            !segments_cross(call_from, call_to, link_from, link_to),
            "the call edge must not cross a's containment link to c"
        );
    }
}
