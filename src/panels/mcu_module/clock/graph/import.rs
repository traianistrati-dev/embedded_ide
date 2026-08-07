//! Import / export a clock tree as a standalone `.ron` file (Layer 1).
//!
//! The family graphs (`stm32wba_graph`, `stm32f4_graph`, …) are Rust builders;
//! this makes the SAME `GraphClock` shape a data file the New-MCU form can load
//! and save. That is what lets a brand-new family be added without a recompile,
//! and — later — what an AI datasheet extraction writes into.
//!
//! Pure: parsing, validation and pretty-printing only. The file dialog and the
//! form wiring live in the UI layer.

use super::config::GraphClock;
use super::eval::evaluate;
use super::model::ClockGraph;

/// Parse a clock `.ron` into a [`GraphClock`], then prove it is usable.
///
/// Accepts two shapes so a hand- or AI-written file needn't include diagram
/// coordinates:
///   * a full `GraphClock` (graph + layout), or
///   * a bare `ClockGraph` (layout defaults empty — the diagram auto-lays-out).
///
/// After parsing, the graph must actually EVALUATE to a non-trivial result:
/// a syntactically valid but disconnected tree (every frequency zero) is the
/// most likely way an extraction goes wrong, and it would otherwise import as a
/// dead diagram. That check is the cheap guard the whole feature leans on.
pub fn parse_clock_ron(text: &str) -> Result<GraphClock, String> {
    let gc = parse_shape(text)?;

    if gc.graph.nodes.is_empty() {
        return Err("the clock tree has no nodes".to_string());
    }
    let freqs = evaluate(&gc.graph);
    if !freqs.values().any(|&hz| hz > 0) {
        return Err(
            "the clock tree evaluates to all-zero frequencies — its sources or connections \
             are missing (check that every mux/divider is reachable from a source)"
                .to_string(),
        );
    }
    Ok(gc)
}

/// Try `GraphClock` first, then fall back to a bare `ClockGraph`. The two error
/// messages are combined so a real syntax error is not hidden behind the
/// fallback's "expected ClockGraph".
fn parse_shape(text: &str) -> Result<GraphClock, String> {
    match ron::from_str::<GraphClock>(text) {
        Ok(gc) => Ok(gc),
        Err(full_err) => match ron::from_str::<ClockGraph>(text) {
            Ok(graph) => Ok(GraphClock {
                graph,
                layout: Default::default(),
            }),
            Err(_) => Err(format!("not a valid clock .ron: {full_err}")),
        },
    }
}

/// Serialize a [`GraphClock`] to pretty RON — the template a user edits or hands
/// to an AI. `struct_names(true)` matches the chip `.ron` style so the two read
/// alike.
pub fn export_clock_ron(gc: &GraphClock) -> String {
    let pretty = ron::ser::PrettyConfig::default().struct_names(true);
    ron::ser::to_string_pretty(gc, pretty)
        .unwrap_or_else(|e| format!("// failed to serialize clock: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::clock::graph::{stm32f4_graph, stm32f4_layout};

    fn sample() -> GraphClock {
        GraphClock {
            graph: stm32f4_graph(),
            layout: stm32f4_layout(),
        }
    }

    #[test]
    fn export_then_import_round_trips() {
        let gc = sample();
        let text = export_clock_ron(&gc);
        let back = parse_clock_ron(&text).expect("re-import the exported clock");
        assert_eq!(gc, back, "export → import must be lossless");
    }

    #[test]
    fn a_bare_graph_without_layout_is_accepted() {
        // The layout is diagram-only; an AI/hand file may omit it entirely.
        let graph_only = ron::ser::to_string_pretty(
            &sample().graph,
            ron::ser::PrettyConfig::default().struct_names(true),
        )
        .unwrap();
        let gc = parse_clock_ron(&graph_only).expect("bare ClockGraph must parse");
        assert_eq!(gc.graph, sample().graph);
        assert!(
            gc.layout.blocks.is_empty(),
            "missing layout defaults to empty"
        );
    }

    #[test]
    fn garbage_is_rejected_with_a_readable_error() {
        let err = parse_clock_ron("this is not ron at all {{{").unwrap_err();
        assert!(err.contains("not a valid clock .ron"), "{err}");
    }

    #[test]
    fn a_disconnected_tree_that_evaluates_to_zero_is_rejected() {
        // THE guard the feature leans on: a structurally valid graph whose
        // sources are 0 Hz (or unreachable) would import as a dead diagram.
        // A single gated source stuck at 0 Hz with nothing else.
        use super::super::model::{Node, NodeKind, NodeState};
        let graph = ClockGraph {
            nodes: vec![Node {
                id: "dead".into(),
                kind: NodeKind::Source {
                    min_hz: 0,
                    max_hz: 100,
                    gated: true,
                },
                // A source stuck at 0 Hz — the exact "extraction forgot the
                // frequency" case.
                state: NodeState::Source {
                    enabled: true,
                    hz: 0,
                },
                limit: None,
            }],
            edges: vec![],
        };
        let text = ron::ser::to_string_pretty(
            &graph,
            ron::ser::PrettyConfig::default().struct_names(true),
        )
        .unwrap();
        let err = parse_clock_ron(&text).unwrap_err();
        assert!(err.contains("all-zero"), "{err}");
    }

    #[test]
    fn an_empty_tree_is_rejected() {
        let text = ron::ser::to_string_pretty(
            &ClockGraph {
                nodes: vec![],
                edges: vec![],
            },
            ron::ser::PrettyConfig::default().struct_names(true),
        )
        .unwrap();
        assert!(parse_clock_ron(&text).unwrap_err().contains("no nodes"));
    }
}
