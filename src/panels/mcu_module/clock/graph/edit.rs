//! Structural editing of a clock graph — the constructor's pure core (Phase 4).
//!
//! Every operation the editor offers (add / delete / connect / disconnect /
//! rename / retune a node's parameters) lives here as a plain function over
//! `(&mut ClockGraph, &mut Vec<NodeBox>)`, so the UI is only mouse handling and
//! the rules are unit-testable.
//!
//! Two invariants the ops maintain, because breaking either makes a graph that
//! cannot be evaluated:
//! - **ids stay unique** (they are the addressing scheme for edges, `TimerMul`
//!   references, layout `ValueSrc::Node` and — crucially — codegen);
//! - **the graph stays acyclic**; [`connect`] refuses an edge that would close a
//!   loop rather than leaving `evaluate` with unreachable nodes.
//!
//! The layout's drawable primitives are NOT patched here: the caller re-runs
//! [`super::auto_layout::derive`] after an edit, which regenerates them from the
//! boxes. So these functions only ever touch `boxes`.

use super::layout::NodeBox;
use super::model::{ClockGraph, Edge, Node, NodeKind, NodeState};
use crate::panels::mcu_module::clock::validate::Severity;

// ── Palette ───────────────────────────────────────────────────────────────────

/// The node kinds the editor can create. One entry per [`NodeKind`], each with
/// a starting shape the user then tunes in the properties panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteKind {
    Source,
    Mux,
    Divider,
    FixedDiv,
    Choice,
    Multiplier,
    Gate,
    TimerMul,
    Tap,
    Output,
}

impl PaletteKind {
    pub const ALL: [PaletteKind; 10] = [
        PaletteKind::Source,
        PaletteKind::Mux,
        PaletteKind::Divider,
        PaletteKind::FixedDiv,
        PaletteKind::Choice,
        PaletteKind::Multiplier,
        PaletteKind::Gate,
        PaletteKind::TimerMul,
        PaletteKind::Tap,
        PaletteKind::Output,
    ];

    /// Menu label — the datasheet's word for the element, not the type name.
    pub fn label(self) -> &'static str {
        match self {
            PaletteKind::Source => "Oscillator / source",
            PaletteKind::Mux => "Mux (selector)",
            PaletteKind::Divider => "Divider (/N)",
            PaletteKind::FixedDiv => "Fixed divider",
            PaletteKind::Choice => "Ratio choice (×n/d)",
            PaletteKind::Multiplier => "Multiplier (PLL ×N)",
            PaletteKind::Gate => "Enable gate (EN)",
            PaletteKind::TimerMul => "Timer ×1/×2 rule",
            PaletteKind::Tap => "Tap (named node)",
            PaletteKind::Output => "Output (delivered clock)",
        }
    }

    /// Prefix of the generated id (`mux1`, `div2`, …).
    fn id_stem(self) -> &'static str {
        match self {
            PaletteKind::Source => "src",
            PaletteKind::Mux => "mux",
            PaletteKind::Divider => "div",
            PaletteKind::FixedDiv => "fdiv",
            PaletteKind::Choice => "ratio",
            PaletteKind::Multiplier => "mul",
            PaletteKind::Gate => "en",
            PaletteKind::TimerMul => "tim",
            PaletteKind::Tap => "tap",
            PaletteKind::Output => "out",
        }
    }

    /// A usable starting shape — an 8 MHz crystal, a /1../8 divider, and so on.
    fn new_kind(self) -> NodeKind {
        match self {
            PaletteKind::Source => NodeKind::Source {
                min_hz: 8_000_000,
                max_hz: 8_000_000,
                gated: true,
            },
            PaletteKind::Mux => NodeKind::Mux { inputs: 2 },
            PaletteKind::Divider => NodeKind::Divider {
                options: vec![1, 2, 4, 8],
            },
            PaletteKind::FixedDiv => NodeKind::FixedDiv { by: 2 },
            PaletteKind::Choice => NodeKind::Choice {
                ratios: vec![(1, 1), (2, 3)],
            },
            PaletteKind::Multiplier => NodeKind::Multiplier { min: 2, max: 16 },
            PaletteKind::Gate => NodeKind::Gate,
            // Left dangling on purpose: which prescaler it follows is a choice,
            // and `issues` reports the empty reference until it is made.
            PaletteKind::TimerMul => NodeKind::TimerMul {
                prescaler: String::new(),
            },
            PaletteKind::Tap => NodeKind::Tap,
            PaletteKind::Output => NodeKind::Output,
        }
    }
}

/// The state a node of `kind` should start in — and the state it must be forced
/// back to after its parameters change (a divider whose option list shrank can
/// be left pointing past the end).
pub fn clamp_state(node: &mut Node) {
    node.state = match (&node.kind, &node.state) {
        (NodeKind::Source { min_hz, max_hz, .. }, st) => {
            let (enabled, hz) = match st {
                NodeState::Source { enabled, hz } => (*enabled, *hz),
                _ => (true, *min_hz),
            };
            NodeState::Source {
                enabled,
                hz: hz.clamp(*min_hz, (*max_hz).max(*min_hz)),
            }
        }
        (NodeKind::Mux { inputs }, st) => match st {
            // An unselected mux is a legitimate state (RTC/MCO off).
            NodeState::Unset => NodeState::Unset,
            NodeState::Index(i) if *i < *inputs => NodeState::Index(*i),
            _ => NodeState::Index(0),
        },
        (NodeKind::Divider { options }, st) => match st {
            NodeState::Index(i) if *i < options.len() => NodeState::Index(*i),
            _ => NodeState::Index(0),
        },
        (NodeKind::Choice { ratios }, st) => match st {
            NodeState::Index(i) if *i < ratios.len() => NodeState::Index(*i),
            _ => NodeState::Index(0),
        },
        (NodeKind::Multiplier { min, max }, st) => {
            let v = match st {
                NodeState::Value(v) => *v,
                _ => *min,
            };
            NodeState::Value(v.clamp(*min, (*max).max(*min)))
        }
        // A gate keeps its on/off; everything else has no state to hold.
        (NodeKind::Gate, NodeState::Unset) => NodeState::Unset,
        _ => NodeState::Fixed,
    };
}

// ── Operations ────────────────────────────────────────────────────────────────

/// Add a node of `kind` at `(x, y)`. Returns the generated id.
pub fn add_node(
    graph: &mut ClockGraph,
    boxes: &mut Vec<NodeBox>,
    kind: PaletteKind,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> String {
    let id = unique_id(graph, kind.id_stem());
    let mut node = Node {
        id: id.clone(),
        kind: kind.new_kind(),
        state: NodeState::Fixed,
        limit: None,
    };
    clamp_state(&mut node);
    graph.nodes.push(node);
    boxes.push(NodeBox {
        node: id.clone(),
        x,
        y,
        w,
        h,
    });
    id
}

/// Delete a node and every edge touching it. `false` when there is no such node.
///
/// A mux keeps its declared input count — the freed input simply has nothing
/// feeding it, which [`issues`] reports; silently renumbering inputs would move
/// the user's other selections under them.
pub fn remove_node(graph: &mut ClockGraph, boxes: &mut Vec<NodeBox>, id: &str) -> bool {
    if !graph.nodes.iter().any(|n| n.id == id) {
        return false;
    }
    graph.nodes.retain(|n| n.id != id);
    graph.edges.retain(|e| e.from != id && e.to != id);
    boxes.retain(|b| b.node != id);
    // A timer rule that followed this prescaler now points nowhere.
    for n in &mut graph.nodes {
        if let NodeKind::TimerMul { prescaler } = &mut n.kind
            && prescaler == id
        {
            prescaler.clear();
        }
    }
    true
}

/// Rename a node, updating every reference to it: edges, `TimerMul` prescaler
/// references and the layout boxes.
pub fn rename_node(
    graph: &mut ClockGraph,
    boxes: &mut [NodeBox],
    old: &str,
    new: &str,
) -> Result<(), String> {
    let new = new.trim();
    if new.is_empty() {
        return Err("An id cannot be empty.".into());
    }
    if new.split_whitespace().count() > 1 {
        return Err("An id cannot contain spaces.".into());
    }
    if new == old {
        return Ok(());
    }
    if graph.nodes.iter().any(|n| n.id == new) {
        return Err(format!("`{new}` is already taken."));
    }
    let Some(node) = graph.nodes.iter_mut().find(|n| n.id == old) else {
        return Err(format!("No node `{old}`."));
    };
    node.id = new.to_owned();
    for e in &mut graph.edges {
        if e.from == old {
            e.from = new.to_owned();
        }
        if e.to == old {
            e.to = new.to_owned();
        }
    }
    for n in &mut graph.nodes {
        if let NodeKind::TimerMul { prescaler } = &mut n.kind
            && prescaler == old
        {
            *prescaler = new.to_owned();
        }
    }
    for b in boxes.iter_mut() {
        if b.node == old {
            b.node = new.to_owned();
        }
    }
    Ok(())
}

/// Wire `from`'s output into `to`. Returns the input index it landed on.
///
/// A mux takes the first free input; every other kind has a single input, so an
/// existing connection is REPLACED (dragging a new source onto a divider is the
/// common gesture, and the wire visibly moves).
pub fn connect(graph: &mut ClockGraph, from: &str, to: &str) -> Result<usize, String> {
    if from == to {
        return Err("A node cannot feed itself.".into());
    }
    let Some(target) = graph.nodes.iter().find(|n| n.id == to) else {
        return Err(format!("No node `{to}`."));
    };
    if !graph.nodes.iter().any(|n| n.id == from) {
        return Err(format!("No node `{from}`."));
    }
    if matches!(target.kind, NodeKind::Source { .. }) {
        return Err("An oscillator has no input.".into());
    }
    if graph.edges.iter().any(|e| e.from == from && e.to == to) {
        return Err("Already connected.".into());
    }
    // Closing a loop would leave the evaluator with nodes it can never resolve.
    if reaches(graph, to, from) {
        return Err("That would create a loop.".into());
    }

    let input = match &target.kind {
        NodeKind::Mux { inputs } => {
            let taken: Vec<usize> = graph
                .edges
                .iter()
                .filter(|e| e.to == to)
                .map(|e| e.input)
                .collect();
            match (0..*inputs).find(|i| !taken.contains(i)) {
                Some(i) => i,
                None => {
                    return Err(format!(
                        "All {inputs} mux inputs are taken — raise the input count first."
                    ));
                }
            }
        }
        _ => {
            graph.edges.retain(|e| e.to != to);
            0
        }
    };
    graph.edges.push(Edge {
        from: from.to_owned(),
        to: to.to_owned(),
        input,
    });
    Ok(input)
}

/// Remove one edge. `false` when it wasn't there.
pub fn disconnect(graph: &mut ClockGraph, from: &str, to: &str, input: usize) -> bool {
    let before = graph.edges.len();
    graph
        .edges
        .retain(|e| !(e.from == from && e.to == to && e.input == input));
    graph.edges.len() != before
}

/// Can `from` reach `to` by following edges? Used to keep the graph acyclic.
fn reaches(graph: &ClockGraph, from: &str, to: &str) -> bool {
    let mut seen: Vec<&str> = vec![from];
    let mut stack: Vec<&str> = vec![from];
    while let Some(cur) = stack.pop() {
        if cur == to {
            return true;
        }
        for e in graph.edges.iter().filter(|e| e.from == cur) {
            if !seen.contains(&e.to.as_str()) {
                seen.push(&e.to);
                stack.push(&e.to);
            }
        }
    }
    false
}

/// First free `<stem><n>` id, counting from 1.
fn unique_id(graph: &ClockGraph, stem: &str) -> String {
    (1..)
        .map(|n| format!("{stem}{n}"))
        .find(|id| !graph.nodes.iter().any(|n| n.id == *id))
        .unwrap_or_else(|| stem.to_owned())
}

// ── Validation ────────────────────────────────────────────────────────────────

/// A structural problem with the graph being edited. Distinct from
/// [`super::validate::over_limits`], which checks FREQUENCIES against the
/// datasheet — these are about the graph being well-formed at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// The node it is about, when it is about one.
    pub node: Option<String>,
    pub msg: String,
    pub severity: Severity,
}

/// Everything structurally wrong with `graph`, worst first.
pub fn issues(graph: &ClockGraph) -> Vec<Issue> {
    let mut out = Vec::new();
    let err = |node: Option<&str>, msg: String| Issue {
        node: node.map(str::to_owned),
        msg,
        severity: Severity::Error,
    };
    let warn = |node: Option<&str>, msg: String| Issue {
        node: node.map(str::to_owned),
        msg,
        severity: Severity::Warning,
    };

    // Duplicate ids — everything else addresses nodes by id, so this first.
    for (i, n) in graph.nodes.iter().enumerate() {
        if graph.nodes[..i].iter().any(|o| o.id == n.id) {
            out.push(err(Some(&n.id), format!("Duplicate id `{}`.", n.id)));
        }
    }

    // A cycle: fewer nodes come out of a topological sweep than went in.
    if let Some(stuck) = cycle_members(graph) {
        out.push(err(
            None,
            format!("Loop through: {}. A clock tree must be acyclic.", stuck),
        ));
    }

    for n in &graph.nodes {
        let incoming: Vec<&Edge> = graph.edges.iter().filter(|e| e.to == n.id).collect();
        match &n.kind {
            NodeKind::Source { min_hz, max_hz, .. } => {
                if min_hz > max_hz {
                    out.push(err(Some(&n.id), "Minimum is above maximum.".into()));
                }
            }
            NodeKind::Mux { inputs } => {
                for k in 0..*inputs {
                    if !incoming.iter().any(|e| e.input == k) {
                        out.push(warn(
                            Some(&n.id),
                            format!("Mux input {k} has nothing connected."),
                        ));
                    }
                }
            }
            NodeKind::Divider { options } => {
                if options.is_empty() || options.contains(&0) {
                    out.push(err(
                        Some(&n.id),
                        "Divisor list must be non-empty and free of zeros.".into(),
                    ));
                }
            }
            NodeKind::FixedDiv { by } if *by == 0 => {
                out.push(err(Some(&n.id), "Cannot divide by zero.".into()));
            }
            NodeKind::Choice { ratios } => {
                if ratios.is_empty() || ratios.iter().any(|(_, d)| *d == 0) {
                    out.push(err(
                        Some(&n.id),
                        "Ratio list must be non-empty and free of zero denominators.".into(),
                    ));
                }
            }
            NodeKind::Multiplier { min, max } if min > max => {
                out.push(err(Some(&n.id), "Minimum is above maximum.".into()));
            }
            NodeKind::TimerMul { prescaler } => {
                if prescaler.is_empty() {
                    out.push(err(
                        Some(&n.id),
                        "No prescaler chosen — the ×1/×2 rule has nothing to follow.".into(),
                    ));
                } else if !graph.nodes.iter().any(|o| o.id == *prescaler) {
                    out.push(err(
                        Some(&n.id),
                        format!("Prescaler `{prescaler}` does not exist."),
                    ));
                }
            }
            _ => {}
        }

        // Everything except an oscillator needs something feeding it.
        if !matches!(n.kind, NodeKind::Source { .. }) && incoming.is_empty() {
            out.push(warn(Some(&n.id), "Nothing feeds this node.".into()));
        }
    }

    out.sort_by_key(|i| match i.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    });
    out
}

/// The ids left unresolved by a topological sweep — i.e. the nodes in (or fed
/// only by) a cycle. `None` when the graph is a proper DAG.
fn cycle_members(graph: &ClockGraph) -> Option<String> {
    let mut indeg: Vec<(&str, usize)> = graph
        .nodes
        .iter()
        .map(|n| {
            (
                n.id.as_str(),
                graph
                    .edges
                    .iter()
                    .filter(|e| e.to == n.id && graph.nodes.iter().any(|o| o.id == e.from))
                    .count(),
            )
        })
        .collect();
    let mut done: Vec<&str> = Vec::new();
    loop {
        let Some(pos) = indeg.iter().position(|(_, d)| *d == 0) else {
            break;
        };
        let (id, _) = indeg.remove(pos);
        done.push(id);
        for e in graph.edges.iter().filter(|e| e.from == id) {
            if let Some(entry) = indeg.iter_mut().find(|(i, _)| *i == e.to) {
                entry.1 = entry.1.saturating_sub(1);
            }
        }
    }
    if indeg.is_empty() {
        None
    } else {
        let mut names: Vec<&str> = indeg.into_iter().map(|(i, _)| i).collect();
        names.sort_unstable();
        Some(names.join(", "))
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::clock::graph::evaluate;

    fn empty() -> (ClockGraph, Vec<NodeBox>) {
        (
            ClockGraph {
                nodes: Vec::new(),
                edges: Vec::new(),
            },
            Vec::new(),
        )
    }

    fn add(g: &mut ClockGraph, b: &mut Vec<NodeBox>, k: PaletteKind) -> String {
        add_node(g, b, k, 0.0, 0.0, 96.0, 26.0)
    }

    /// A tree built entirely through the editor evaluates — the point of the
    /// whole constructor.
    #[test]
    fn a_graph_built_from_scratch_evaluates() {
        let (mut g, mut b) = empty();
        let src = add(&mut g, &mut b, PaletteKind::Source);
        let div = add(&mut g, &mut b, PaletteKind::Divider);
        let out = add(&mut g, &mut b, PaletteKind::Output);
        connect(&mut g, &src, &div).unwrap();
        connect(&mut g, &div, &out).unwrap();

        // The palette's source starts at 8 MHz and the divider at /1.
        assert_eq!(evaluate(&g)[&out], 8_000_000);
        assert!(issues(&g).is_empty(), "{:?}", issues(&g));
        assert_eq!(b.len(), 3, "every node got a box");
    }

    /// A chip that starts with NO tree at all — the "Start an empty tree" route
    /// — must survive every step the tab takes: laying out, evaluating,
    /// validating and generating, before a single node exists.
    #[test]
    fn an_empty_tree_survives_the_whole_pipeline() {
        use crate::panels::mcu_module::clock::graph::{auto_layout, place};
        use crate::panels::mcu_module::clock::model::{ClockConfig, ClockLimits};
        use crate::panels::mcu_module::codegen::rcc::graph_clock_block;

        let (mut g, mut b) = empty();
        let lay = auto_layout(&g);
        assert!(lay.is_empty(), "nothing to draw yet");
        assert!(place(&g).is_empty());
        assert!(evaluate(&g).is_empty());
        assert!(issues(&g).is_empty(), "an empty tree is not malformed");
        let (w, h) = lay.bounds();
        assert!(w > 0.0 && h > 0.0, "the canvas still has a size");

        // Code generation falls back cleanly rather than panicking.
        let gc = crate::panels::mcu_module::clock::graph::GraphClock {
            graph: g.clone(),
            layout: lay,
            bindings: Default::default(),
        };
        let block = graph_clock_block("stm32h5", &ClockConfig::Graph(gc), false);
        assert!(block.contains("embassy_stm32::init"));
        let _ = ClockLimits::default();

        // And the first node added makes it a real tree.
        let src = add(&mut g, &mut b, PaletteKind::Source);
        assert_eq!(evaluate(&g)[&src], 8_000_000);
        assert_eq!(auto_layout(&g).nodes.len(), 1);
    }

    #[test]
    fn ids_are_unique_per_kind() {
        let (mut g, mut b) = empty();
        let a = add(&mut g, &mut b, PaletteKind::Mux);
        let c = add(&mut g, &mut b, PaletteKind::Mux);
        assert_eq!((a.as_str(), c.as_str()), ("mux1", "mux2"));
    }

    #[test]
    fn a_loop_is_refused() {
        let (mut g, mut b) = empty();
        let src = add(&mut g, &mut b, PaletteKind::Source);
        let t1 = add(&mut g, &mut b, PaletteKind::Tap);
        let t2 = add(&mut g, &mut b, PaletteKind::Tap);
        connect(&mut g, &src, &t1).unwrap();
        connect(&mut g, &t1, &t2).unwrap();
        let err = connect(&mut g, &t2, &t1).unwrap_err();
        assert!(err.contains("loop"), "{err}");
        assert!(connect(&mut g, &t1, &t1).is_err(), "no self-loop either");
    }

    #[test]
    fn a_mux_fills_its_inputs_in_order_then_refuses() {
        let (mut g, mut b) = empty();
        let mux = add(&mut g, &mut b, PaletteKind::Mux); // 2 inputs
        let a = add(&mut g, &mut b, PaletteKind::Source);
        let c = add(&mut g, &mut b, PaletteKind::Source);
        let d = add(&mut g, &mut b, PaletteKind::Source);
        assert_eq!(connect(&mut g, &a, &mux).unwrap(), 0);
        assert_eq!(connect(&mut g, &c, &mux).unwrap(), 1);
        assert!(connect(&mut g, &d, &mux).unwrap_err().contains("taken"));
    }

    /// A single-input node accepts a new feed by replacing the old one.
    #[test]
    fn reconnecting_a_divider_replaces_its_input() {
        let (mut g, mut b) = empty();
        let div = add(&mut g, &mut b, PaletteKind::Divider);
        let a = add(&mut g, &mut b, PaletteKind::Source);
        let c = add(&mut g, &mut b, PaletteKind::Source);
        connect(&mut g, &a, &div).unwrap();
        connect(&mut g, &c, &div).unwrap();
        let feeds: Vec<&str> = g
            .edges
            .iter()
            .filter(|e| e.to == div)
            .map(|e| e.from.as_str())
            .collect();
        assert_eq!(feeds, [c.as_str()], "the old input is gone");
    }

    #[test]
    fn deleting_a_node_takes_its_edges_and_references() {
        let (mut g, mut b) = empty();
        let src = add(&mut g, &mut b, PaletteKind::Source);
        let div = add(&mut g, &mut b, PaletteKind::Divider);
        let tim = add(&mut g, &mut b, PaletteKind::TimerMul);
        connect(&mut g, &src, &div).unwrap();
        connect(&mut g, &div, &tim).unwrap();
        if let Some(n) = g.node_mut(&tim) {
            n.kind = NodeKind::TimerMul {
                prescaler: div.clone(),
            };
        }

        assert!(remove_node(&mut g, &mut b, &div));
        assert!(g.node(&div).is_none());
        assert!(!g.edges.iter().any(|e| e.from == div || e.to == div));
        assert!(!b.iter().any(|x| x.node == div));
        let NodeKind::TimerMul { prescaler } = &g.node(&tim).unwrap().kind else {
            panic!("still a timer rule");
        };
        assert!(prescaler.is_empty(), "the dangling reference was cleared");
    }

    #[test]
    fn renaming_updates_every_reference() {
        let (mut g, mut b) = empty();
        let src = add(&mut g, &mut b, PaletteKind::Source);
        let div = add(&mut g, &mut b, PaletteKind::Divider);
        let tim = add(&mut g, &mut b, PaletteKind::TimerMul);
        connect(&mut g, &src, &div).unwrap();
        if let Some(n) = g.node_mut(&tim) {
            n.kind = NodeKind::TimerMul {
                prescaler: div.clone(),
            };
        }

        rename_node(&mut g, &mut b, &div, "apb1").unwrap();
        assert!(g.node("apb1").is_some());
        assert!(g.edges.iter().any(|e| e.to == "apb1"));
        assert!(b.iter().any(|x| x.node == "apb1"));
        let NodeKind::TimerMul { prescaler } = &g.node(&tim).unwrap().kind else {
            panic!()
        };
        assert_eq!(prescaler, "apb1");
    }

    #[test]
    fn renaming_rejects_duplicates_and_blanks() {
        let (mut g, mut b) = empty();
        let a = add(&mut g, &mut b, PaletteKind::Tap);
        let c = add(&mut g, &mut b, PaletteKind::Tap);
        assert!(rename_node(&mut g, &mut b, &a, &c).is_err());
        assert!(rename_node(&mut g, &mut b, &a, "  ").is_err());
        assert!(rename_node(&mut g, &mut b, &a, "two words").is_err());
        assert!(rename_node(&mut g, &mut b, &a, &a).is_ok(), "no-op rename");
    }

    /// Shrinking an option list must not leave the selection past the end.
    #[test]
    fn parameters_changing_clamp_the_selection() {
        let (mut g, mut b) = empty();
        let div = add(&mut g, &mut b, PaletteKind::Divider);
        let n = g.node_mut(&div).unwrap();
        n.state = NodeState::Index(3);
        n.kind = NodeKind::Divider {
            options: vec![1, 2],
        };
        clamp_state(n);
        assert_eq!(n.state, NodeState::Index(0));

        let mul = add(&mut g, &mut b, PaletteKind::Multiplier);
        let n = g.node_mut(&mul).unwrap();
        n.state = NodeState::Value(400);
        n.kind = NodeKind::Multiplier { min: 2, max: 16 };
        clamp_state(n);
        assert_eq!(n.state, NodeState::Value(16));
    }

    #[test]
    fn issues_report_the_real_problems() {
        let (mut g, mut b) = empty();
        let mux = add(&mut g, &mut b, PaletteKind::Mux);
        let _tim = add(&mut g, &mut b, PaletteKind::TimerMul);
        let src = add(&mut g, &mut b, PaletteKind::Source);
        connect(&mut g, &src, &mux).unwrap();

        let found = issues(&g);
        let says = |needle: &str| found.iter().any(|i| i.msg.contains(needle));
        assert!(says("Mux input 1"), "unfed mux input: {found:?}");
        assert!(says("No prescaler chosen"), "{found:?}");
        assert!(says("Nothing feeds"), "the timer node is unfed: {found:?}");
        assert_eq!(found[0].severity, Severity::Error, "errors sort first");

        // A duplicate id is reported even though the ops prevent making one.
        g.nodes.push(g.nodes[0].clone());
        assert!(issues(&g).iter().any(|i| i.msg.contains("Duplicate")));
    }

    /// A hand-made cycle (only reachable by editing the `.ron`) is named, not
    /// silently swallowed by the evaluator.
    #[test]
    fn a_cycle_is_named() {
        let (mut g, mut b) = empty();
        let a = add(&mut g, &mut b, PaletteKind::Tap);
        let c = add(&mut g, &mut b, PaletteKind::Tap);
        g.edges.push(Edge {
            from: a.clone(),
            to: c.clone(),
            input: 0,
        });
        g.edges.push(Edge {
            from: c.clone(),
            to: a.clone(),
            input: 0,
        });
        let found = issues(&g);
        let loops: Vec<&Issue> = found.iter().filter(|i| i.msg.contains("Loop")).collect();
        assert_eq!(loops.len(), 1, "{found:?}");
        assert!(loops[0].msg.contains(&a) && loops[0].msg.contains(&c));
    }
}
