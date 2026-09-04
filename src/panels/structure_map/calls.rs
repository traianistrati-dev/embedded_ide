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
///
/// Public because the status line names it: a cap the user cannot see is a cap
/// that turns a partial answer into a confident one.
pub const MAX_SYMBOLS: usize = 400;

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
    /// The rust-analyzer generation the in-flight request was sent under.
    ///
    /// `LspState::reset` bumps the generation AND clears `calls_refs_pending` /
    /// `calls_refs_results`, so a restart mid-request means the reply can never
    /// arrive. `in_flight` is cleared only by that reply, and `running()` is
    /// true while it is set, so nothing further is ever sent: the pass wedges on
    /// "analyzing calls N/total..." until a content change happens to rebuild
    /// it. Comparing this against the live generation is what lets the request
    /// be re-queued instead.
    pub sent_at_generation: u64,
    /// The one in-flight request: `(local key, node, row)`.
    pub in_flight: Option<(usize, usize, usize)>,
    pub edges: Vec<CallEdge>,
    /// Calls WITHIN one module (`from_node == to_node`) — the flow inside a
    /// single file.
    ///
    /// Kept apart from [`Self::edges`] on purpose. Between collapsed boxes an
    /// intra-module edge says nothing the dep arrows do not already say, which
    /// is why the pass used to drop it on the floor; but it is exactly the
    /// order in which a file's own items call each other, so it is what an
    /// EXPANDED node draws. Separate collections mean turning this on cannot
    /// change a single pixel of the collapsed diagram.
    pub inner_edges: Vec<CallEdge>,
    /// TOTAL reference sites found per queried symbol `(node, row)` — shown as
    /// a count next to the symbol row (many sites aggregate into few edges, so
    /// the count keeps the full picture visible).
    pub ref_counts: HashMap<(usize, usize), usize>,
    /// Reference sites per drawn EDGE (caller row → callee row) — scales the
    /// edge's stroke width, so heavy relationships read thicker.
    pub pair_counts: HashMap<CallEdge, usize>,
    seen: HashSet<CallEdge>,
    seen_inner: HashSet<CallEdge>,
    pub done: usize,
    pub total: usize,
    /// Symbols the cap left unqueried, or 0.
    ///
    /// [`Self::total`] counts what was QUEUED, so a truncated pass finishes at
    /// "400/400" and then reports its edge count - reading as a complete answer
    /// while every symbol past the cap was never searched and its call edges
    /// were never drawn. The count is kept so the status line can say so.
    pub skipped: usize,
    /// `true` while paused because a file's text isn't synced to RA yet.
    pub waiting_sync: bool,
    /// Last state line written to the debug log — dedup so the per-frame tick
    /// doesn't spam the file.
    pub last_log: String,
}

impl CallPass {
    /// Queue every top-level symbol of every module (capped at [`MAX_SYMBOLS`]).
    pub fn new(graph: &ModuleGraph, hash: u64) -> Self {
        let available: usize = graph.nodes.iter().map(|n| n.symbols.len()).sum();
        let queue: VecDeque<(usize, usize)> = graph
            .nodes
            .iter()
            .enumerate()
            .flat_map(|(ni, n)| (0..n.symbols.len()).map(move |ri| (ni, ri)))
            .take(MAX_SYMBOLS)
            .collect();
        let total = queue.len();
        let skipped = available.saturating_sub(total);
        Self {
            hash,
            queue,
            in_flight: None,
            sent_at_generation: 0,
            edges: Vec::new(),
            inner_edges: Vec::new(),
            ref_counts: HashMap::new(),
            pair_counts: HashMap::new(),
            seen: HashSet::new(),
            seen_inner: HashSet::new(),
            done: 0,
            total,
            skipped,
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

    /// Drop an in-flight request whose reply can no longer come, putting its
    /// symbol back at the FRONT of the queue so nothing is lost.
    ///
    /// Returns true when it did. Called with the live generation each frame.
    pub fn abandon_if_restarted(&mut self, generation: u64) -> bool {
        if self.sent_at_generation == generation {
            return false;
        }
        match self.in_flight.take() {
            Some((_, node, row)) => {
                self.queue.push_front((node, row));
                true
            }
            None => false,
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
                // A file's own flow. Self-calls are skipped: direct recursion
                // draws as a loop from a row to itself, which carries no
                // ordering information and only clutters the layout.
                if from_row != to_row {
                    let edge = CallEdge {
                        from_node,
                        from_row,
                        to_node,
                        to_row,
                    };
                    if self.seen_inner.insert(edge) {
                        self.inner_edges.push(edge);
                    }
                }
                continue;
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

    /// The cap is counted, not just applied.
    ///
    /// `total` is what got QUEUED, so a truncated pass runs to "400/400" and
    /// then reports its edge count - which reads as the whole answer while every
    /// symbol past the cap was never searched. `skipped` is what lets the status
    /// line say otherwise.
    /// A restart mid-request must not wedge the pass.
    ///
    /// `LspState::reset` bumps the generation AND clears the call-graph reply
    /// channel, so the outstanding request can never be answered. `in_flight`
    /// is cleared only by that reply and `running()` is true while it is set,
    /// so before this the pass sat on "analyzing calls N/total..." for good -
    /// Save did not help (same content, same hash, same pass) and neither did
    /// reopening the tab. Only an unrelated edit rebuilt it.
    #[test]
    fn a_restart_mid_request_requeues_instead_of_wedging() {
        let g = build_graph("fn main() {}\nfn helper() {}\n", &[]);
        let mut pass = CallPass::new(&g, 0);
        let queued = pass.queue.len();

        // Send one, the way the driver does.
        let sym = pass.queue.pop_front().expect("a symbol");
        pass.in_flight = Some((7, sym.0, sym.1));
        pass.sent_at_generation = 3;
        assert!(pass.running());

        // Same generation: nothing to abandon, the reply is still coming.
        assert!(!pass.abandon_if_restarted(3));
        assert!(pass.in_flight.is_some());

        // The analyzer restarted. The reply is gone, so take the symbol back.
        assert!(pass.abandon_if_restarted(4));
        assert!(pass.in_flight.is_none(), "the dead request was dropped");
        assert_eq!(
            pass.queue.len(),
            queued,
            "and its symbol is back in the queue, not lost"
        );
        assert_eq!(
            pass.queue.front().copied(),
            Some(sym),
            "at the front, so it is retried first"
        );
        assert!(
            pass.running(),
            "so the pass carries on rather than stalling"
        );
    }

    /// Nothing in flight and a restart: nothing to do, and no phantom re-queue.
    #[test]
    fn a_restart_with_nothing_in_flight_changes_nothing() {
        let g = build_graph("fn main() {}\n", &[]);
        let mut pass = CallPass::new(&g, 0);
        let before = pass.queue.len();
        assert!(!pass.abandon_if_restarted(99));
        assert_eq!(pass.queue.len(), before);
    }

    #[test]
    fn the_symbol_cap_reports_what_it_dropped() {
        // One module carrying more symbols than the cap allows.
        let over = MAX_SYMBOLS + 37;
        let body: String = (0..over).map(|i| format!("fn f{i}() {{}}\n")).collect();
        let g = build_graph(&body, &[]);
        let pass = CallPass::new(&g, 0);

        assert_eq!(pass.total, MAX_SYMBOLS, "the cap still applies");
        assert_eq!(pass.skipped, 37, "and says how many it left out");
        assert_eq!(
            pass.total + pass.skipped,
            over,
            "the two must account for every symbol, or the message lies"
        );
    }

    /// A project under the cap reports nothing dropped, so the note never
    /// appears where it would only be noise.
    #[test]
    fn a_project_under_the_cap_skips_nothing() {
        let g = build_graph("fn main() {}\nfn helper() {}\n", &[]);
        let pass = CallPass::new(&g, 0);
        assert_eq!(pass.skipped, 0);
        assert_eq!(pass.total, 2);
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

/// The rows of one module, ordered the way its own code calls them.
///
/// A call graph has no total order - a row can be called from several places,
/// not called at all, or sit in a cycle - so "call order" has to be a stated
/// rule rather than a sort. This one:
///
///   1. Roots first: rows nothing in this module calls. They are the entry
///      points, and a file with no internal calls keeps its source order.
///   2. From each root, depth-first through its callees, so a caller is always
///      followed by what it calls.
///   3. Ties broken by ROW INDEX, which is source order - two callees of the
///      same function keep the order they appear in the file.
///   4. Cycles: a row already placed is never placed again, so the back-edge
///      that closes a cycle is simply not followed. Nothing is lost - the row
///      is on the list, once.
///   5. Anything still unplaced (a cycle no root reaches) follows in row order,
///      so every row appears exactly once whatever the graph looks like.
///
/// Deterministic for a given edge set: same input, same order, every frame.
pub fn call_order(row_count: usize, edges: &[CallEdge]) -> Vec<usize> {
    let mut callees: Vec<Vec<usize>> = vec![Vec::new(); row_count];
    let mut called = vec![false; row_count];
    for e in edges {
        if e.from_row < row_count && e.to_row < row_count {
            callees[e.from_row].push(e.to_row);
            called[e.to_row] = true;
        }
    }
    for c in &mut callees {
        c.sort_unstable();
        c.dedup();
    }

    let mut out = Vec::with_capacity(row_count);
    let mut placed = vec![false; row_count];
    let visit = |start: usize, out: &mut Vec<usize>, placed: &mut Vec<bool>| {
        // Explicit stack, not recursion: a deep or cyclic graph must not be
        // able to blow the frame in a UI thread.
        let mut stack = vec![start];
        while let Some(r) = stack.pop() {
            if placed[r] {
                continue;
            }
            placed[r] = true;
            out.push(r);
            // Reversed, so the lowest row index is popped first and source
            // order survives the stack.
            for &c in callees[r].iter().rev() {
                if !placed[c] {
                    stack.push(c);
                }
            }
        }
    };

    for (r, &is_called) in called.iter().enumerate() {
        if !is_called {
            visit(r, &mut out, &mut placed);
        }
    }
    // Index-based on purpose: `placed` is written by `visit` inside the loop,
    // so it cannot also be borrowed by an iterator over itself.
    for r in 0..row_count {
        if !placed[r] {
            visit(r, &mut out, &mut placed);
        }
    }
    out
}

#[cfg(test)]
mod call_order_tests {
    use super::{CallEdge, call_order};

    fn e(from_row: usize, to_row: usize) -> CallEdge {
        CallEdge {
            from_node: 0,
            from_row,
            to_node: 0,
            to_row,
        }
    }

    /// A caller comes before what it calls.
    #[test]
    fn a_caller_precedes_its_callee() {
        // 0 calls 2, 2 calls 1. Source order would be 0,1,2.
        let o = call_order(3, &[e(0, 2), e(2, 1)]);
        assert_eq!(o, vec![0, 2, 1]);
    }

    /// Two callees of one function keep the order they appear in the file.
    #[test]
    fn ties_fall_back_to_source_order() {
        let o = call_order(3, &[e(0, 2), e(0, 1)]);
        assert_eq!(o, vec![0, 1, 2], "row index breaks the tie, not edge order");
    }

    /// A file with no internal calls is left exactly as written.
    #[test]
    fn no_calls_means_source_order() {
        assert_eq!(call_order(4, &[]), vec![0, 1, 2, 3]);
    }

    /// A cycle terminates, and every row still appears exactly once.
    ///
    /// The back edge that closes the cycle is simply not followed - without
    /// this the walk would spin forever on a UI thread.
    #[test]
    fn a_cycle_terminates_and_loses_nothing() {
        let o = call_order(3, &[e(0, 1), e(1, 2), e(2, 0)]);
        assert_eq!(o.len(), 3);
        let mut sorted = o.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2], "every row exactly once");
    }

    /// A cycle NO root reaches is still emitted - nothing may vanish.
    #[test]
    fn an_unreachable_cycle_is_still_listed() {
        // 0 is a root; 1 and 2 call only each other, so neither is a root.
        let o = call_order(3, &[e(1, 2), e(2, 1)]);
        assert_eq!(o.len(), 3);
        assert_eq!(o[0], 0, "the real root leads");
        let mut sorted = o.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2]);
    }

    /// Same input, same answer - the layout must not shuffle between frames.
    #[test]
    fn the_order_is_deterministic() {
        let edges = [e(0, 3), e(3, 1), e(0, 2), e(2, 1)];
        let first = call_order(4, &edges);
        for _ in 0..5 {
            assert_eq!(call_order(4, &edges), first);
        }
    }

    /// An edge naming a row the module does not have is ignored, not a panic.
    #[test]
    fn out_of_range_rows_are_ignored() {
        let o = call_order(2, &[e(0, 9), e(7, 1)]);
        assert_eq!(o.len(), 2);
    }
}
