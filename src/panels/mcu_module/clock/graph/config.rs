//! The importable clock unit (Phase 4): a [`ClockGraph`] (semantic topology +
//! node states) bundled with its [`ClockLayout`] (diagram positions). This is
//! what a chip's `.ron` ships for `ClockDef::Graph`, so a brand-new MCU can
//! carry its own clock tree *and* diagram as pure data — no recompile.

use serde::{Deserialize, Serialize};

use super::layout::ClockLayout;
use super::model::ClockGraph;

/// A complete, importable clock: the evaluatable graph plus its diagram layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphClock {
    pub graph: ClockGraph,
    #[serde(default)]
    pub layout: ClockLayout,
    /// Which node carries each id the code generator reads: `codegen id -> node
    /// id in THIS graph` (`"sw" -> "SysClkSource"`).
    ///
    /// Code generation addresses nodes by fixed names (`sw`, `ahb`, `pllm`, …).
    /// A tree written for this IDE uses them directly and needs nothing here.
    /// An IMPORTED tree does not: CubeMX calls the same nodes `SysClkSource`,
    /// `AHBPrescaler`, `PLLM` — the names from ST's own figure. Renaming them to
    /// suit the generator would throw away the vocabulary of the datasheet the
    /// user is reading, so the mapping is recorded instead, and
    /// [`for_codegen`](Self::for_codegen) applies it.
    ///
    /// Empty (the common case) means "the graph already uses the canonical
    /// ids" — and then `for_codegen` is a no-op that borrows.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub bindings: std::collections::BTreeMap<String, String>,
}

impl GraphClock {
    /// The graph as CODE GENERATION expects to address it: bound nodes renamed
    /// to their canonical ids.
    ///
    /// Returning a [`Cow`] keeps the unbound case exactly as it was — the same
    /// `&ClockGraph`, byte-identical output, no clone. Only a tree that declares
    /// bindings pays for one.
    ///
    /// Every reference is rewritten with the id: edges and `TimerMul` prescaler
    /// references, so the renamed graph still evaluates identically.
    pub fn for_codegen(&self) -> std::borrow::Cow<'_, ClockGraph> {
        use super::model::NodeKind;
        use std::borrow::Cow;

        if self.bindings.is_empty() {
            return Cow::Borrowed(&self.graph);
        }
        // node id -> canonical id, i.e. the map inverted.
        let rename: std::collections::BTreeMap<&str, &str> = self
            .bindings
            .iter()
            .map(|(canonical, node)| (node.as_str(), canonical.as_str()))
            .collect();
        let new_id = |id: &str| {
            rename
                .get(id)
                .map_or_else(|| id.to_owned(), |c| (*c).to_owned())
        };

        let mut g = self.graph.clone();
        for n in &mut g.nodes {
            n.id = new_id(&n.id);
            if let NodeKind::TimerMul { prescaler } = &mut n.kind {
                *prescaler = new_id(prescaler);
            }
        }
        for e in &mut g.edges {
            e.from = new_id(&e.from);
            e.to = new_id(&e.to);
        }
        Cow::Owned(g)
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::stm32f1_layout;
    use super::super::stm32f1::stm32f1_graph;
    use super::*;
    use crate::panels::mcu_module::clock::model::{ClockLimits, Stm32f1Clock};

    #[test]
    fn graph_clock_round_trips_via_ron() {
        let gc = GraphClock {
            graph: stm32f1_graph(&Stm32f1Clock::default()),
            layout: stm32f1_layout(&ClockLimits::default()),
            bindings: Default::default(),
        };
        let text = ron::to_string(&gc).expect("serialize GraphClock");
        let back: GraphClock = ron::from_str(&text).expect("parse GraphClock");
        assert_eq!(gc, back, "GraphClock RON round-trip must be lossless");
    }

    #[test]
    fn missing_layout_defaults_empty() {
        // A `.ron` may omit `layout` (e.g. a headless clock); it defaults empty.
        let gc = GraphClock {
            graph: stm32f1_graph(&Stm32f1Clock::default()),
            layout: ClockLayout::default(),
            bindings: Default::default(),
        };
        let text = ron::to_string(&gc.graph).unwrap();
        let parsed_graph: ClockGraph = ron::from_str(&text).unwrap();
        assert_eq!(parsed_graph, gc.graph);
    }
}
