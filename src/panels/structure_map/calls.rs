//! Phase 3 — the cross-module call graph (symbol row → symbol row edges).
//!
//! For every top-level symbol in the module graph, one `textDocument/references`
//! lookup asks rust-analyzer for its usage sites; each site is attributed to
//! the top-level item that encloses it (via [`ModuleNode::enclosing_row`]'s
//! anchor rule) and becomes a `caller row → callee row` edge. Only CROSS-module
//! edges are kept — intra-module calls and `use`-line references (whose anchor
//! is `None`) would only duplicate what the dep arrows already show.
//!
//! Discipline (the hard-won LSP rules of this app):
//!   * requests are SERIALIZED — one in flight, the next goes out when the
//!     reply lands, and never while the usages pass has its own search running
//!     (`LspState::references_busy`);
//!   * NO `did_change` is ever sent — a symbol is queried only while
//!     rust-analyzer already holds the current text of its file
//!     (`last_sent_matches`); otherwise the pass pauses until the next save;
//!   * replies arrive on a dedicated channel (`take_calls_reference_results`)
//!     so the usages poll can't steal them.

use super::parse::ModuleGraph;
use crate::lsp::ReferenceLoc;
use std::collections::{HashMap, HashSet, VecDeque};

/// Safety cap on symbols queried per pass (each is a whole-crate search).
const MAX_SYMBOLS: usize = 400;

/// One `caller → callee` edge between symbol rows of two different modules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CallEdge {
    pub from_node: usize,
    pub from_row: usize,
    pub to_node: usize,
    pub to_row: usize,
}

/// The incremental call-graph pass for one content hash of the project.
pub struct CallPass {
    /// Content hash of the graph this pass belongs to (stale results dropped).
    pub hash: u64,
    /// Symbols still to query: `(node, row)`.
    pub queue: VecDeque<(usize, usize)>,
    /// The one in-flight request: `(local key, node, row)`.
    pub in_flight: Option<(usize, usize, usize)>,
    pub edges: Vec<CallEdge>,
    /// TOTAL reference sites found per queried symbol `(node, row)` — shown as
    /// a count next to the symbol row (many sites aggregate into few edges, so
    /// the count keeps the full picture visible).
    pub ref_counts: HashMap<(usize, usize), usize>,
    /// Reference sites per drawn EDGE (caller row → callee row) — scales the
    /// edge's stroke width, so heavy relationships read thicker.
    pub pair_counts: HashMap<CallEdge, usize>,
    seen: HashSet<CallEdge>,
    pub done: usize,
    pub total: usize,
    /// `true` while paused because a file's text isn't synced to RA yet.
    pub waiting_sync: bool,
    /// Last state line written to the debug log — dedup so the per-frame tick
    /// doesn't spam the file.
    pub last_log: String,
}

impl CallPass {
    /// Queue every top-level symbol of every module (capped at [`MAX_SYMBOLS`]).
    pub fn new(graph: &ModuleGraph, hash: u64) -> Self {
        let queue: VecDeque<(usize, usize)> = graph
            .nodes
            .iter()
            .enumerate()
            .flat_map(|(ni, n)| (0..n.symbols.len()).map(move |ri| (ni, ri)))
            .take(MAX_SYMBOLS)
            .collect();
        let total = queue.len();
        Self {
            hash,
            queue,
            in_flight: None,
            edges: Vec::new(),
            ref_counts: HashMap::new(),
            pair_counts: HashMap::new(),
            seen: HashSet::new(),
            done: 0,
            total,
            waiting_sync: false,
            last_log: String::new(),
        }
    }

    /// Debug-log `line` once (skips consecutive duplicates).
    pub fn log_once(&mut self, line: String) {
        if self.last_log != line {
            crate::lsp::debug_log(&line);
            self.last_log = line;
        }
    }

    /// `true` while the pass still has work (or a reply) outstanding.
    pub fn running(&self) -> bool {
        self.in_flight.is_some() || !self.queue.is_empty()
    }

    /// Fresh request key — globally unique across passes (a per-pass counter
    /// restarting at 1 let a superseded pass's late reply be mistaken for the
    /// NEW pass's first request and mis-attribute its references).
    pub fn take_key(&mut self) -> usize {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Fold one symbol's reference sites into edges. `(to_node, to_row)` is the
    /// symbol that was queried; each site resolves to its caller module + row.
    pub fn add_references(
        &mut self,
        graph: &ModuleGraph,
        to_node: usize,
        to_row: usize,
        locs: &[ReferenceLoc],
    ) {
        if !locs.is_empty() {
            self.ref_counts.insert((to_node, to_row), locs.len());
        }
        for loc in locs {
            let Some((from_node, Some(from_row))) = map_reference(graph, loc) else {
                continue; // unknown file, or a use-line/attr site (anchor None)
            };
            if from_node == to_node {
                continue; // intra-module call — not drawn
            }
            let edge = CallEdge {
                from_node,
                from_row,
                to_node,
                to_row,
            };
            *self.pair_counts.entry(edge).or_insert(0) += 1;
            if self.seen.insert(edge) {
                self.edges.push(edge);
            }
        }
        self.done += 1;
    }
}

/// Map a reference site (absolute path + 0-based line) to `(module node,
/// enclosing symbol row)`. `None` when the path is no project file.
pub fn map_reference(graph: &ModuleGraph, loc: &ReferenceLoc) -> Option<(usize, Option<usize>)> {
    let node = node_for_path(graph, &loc.path)?;
    let row = graph.nodes[node].enclosing_row(loc.line as usize + 1);
    Some((node, row))
}

/// Find the module node whose file the absolute `path` ends with
/// (`…/mw_radar/src/utils.rs` → the `mw_radar::utils` node). `file_rel` is
/// project-root-relative, so only the workspace root is stripped here — do NOT
/// re-add a `src/` segment or nothing matches. Separators are normalized so
/// Windows `\` paths match.
pub fn node_for_path(graph: &ModuleGraph, path: &str) -> Option<usize> {
    let norm = path.replace('\\', "/");
    graph
        .nodes
        .iter()
        .position(|n| norm.ends_with(&format!("/{}", n.file_rel)))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::parse::build_graph;
    use super::*;

    fn loc(path: &str, line0: u32) -> ReferenceLoc {
        ReferenceLoc {
            path: path.into(),
            line: line0,
            character: 4,
        }
    }

    fn sample() -> ModuleGraph {
        let main_rs = "\
mod a;
mod b;
#[entry]
fn main() {
    a::helper();
}
";
        let files = vec![
            (
                "src/a.rs".into(),
                "pub fn helper() -> u8 {\n    0\n}\npub struct Cfg;\n".into(),
            ),
            (
                "src/b.rs".into(),
                "use crate::a::helper;\npub fn run() {\n    helper();\n}\n".into(),
            ),
        ];
        build_graph(&main_rs, &files)
    }

    #[test]
    fn attributes_call_sites_to_the_enclosing_item() {
        let g = sample();
        // b.rs: line 1 = use (anchor None), line 2 = fn run, line 3 = call site.
        let b = g.nodes.iter().position(|n| n.path == "b").unwrap();
        assert_eq!(g.nodes[b].enclosing_row(1), None, "use line has no row");
        assert_eq!(g.nodes[b].enclosing_row(3), Some(0), "call inside fn run");
        // a.rs: the closing `}` on line 3 ENDS fn helper's span, so line 4's
        // struct Cfg is its own row and a site AT line 3 still maps to helper.
        let a = g.nodes.iter().position(|n| n.path == "a").unwrap();
        assert_eq!(g.nodes[a].enclosing_row(2), Some(0));
        assert_eq!(g.nodes[a].enclosing_row(4), Some(1));
    }

    #[test]
    fn maps_paths_and_builds_cross_module_edges_only() {
        let g = sample();
        let a = g.nodes.iter().position(|n| n.path == "a").unwrap();
        let b = g.nodes.iter().position(|n| n.path == "b").unwrap();
        let mut pass = CallPass::new(&g, 1);
        assert_eq!(pass.total, 4, "main + helper + Cfg + run");

        // References to a::helper: main.rs line 4 (0-based) inside fn main,
        // b.rs use-line 0 (dropped) and call-site line 2 inside fn run,
        // plus one hit in an unknown (external) file (dropped).
        let locs = [
            loc("C:\\temp\\embedded_ide_0_check\\src\\main.rs", 4),
            loc("C:/temp/embedded_ide_0_check/src/b.rs", 0),
            loc("C:/temp/embedded_ide_0_check/src/b.rs", 2),
            loc("C:/cargo/registry/other-1.0.0/src/lib.rs", 10),
        ];
        pass.add_references(&g, a, 0, &locs);
        assert_eq!(pass.done, 1);
        assert_eq!(
            pass.edges,
            vec![
                CallEdge {
                    from_node: 0,
                    from_row: 0,
                    to_node: a,
                    to_row: 0
                },
                CallEdge {
                    from_node: b,
                    from_row: 0,
                    to_node: a,
                    to_row: 0
                },
            ],
            "main::main -> a::helper and b::run -> a::helper (use line + external dropped)"
        );

        // Duplicates are folded; intra-module sites are skipped.
        pass.add_references(&g, a, 0, &locs[..1]);
        assert_eq!(pass.edges.len(), 2);
        pass.add_references(&g, a, 1, &[loc("x/src/a.rs", 0)]);
        assert_eq!(pass.edges.len(), 2, "intra-module reference adds no edge");
    }

    #[test]
    fn symbol_columns_point_at_the_name() {
        let g = sample();
        let a = g.nodes.iter().position(|n| n.path == "a").unwrap();
        let helper = &g.nodes[a].symbols[0];
        // "pub fn helper..." → name starts at col 7.
        assert_eq!((helper.line, helper.col), (1, 7));
    }
}
