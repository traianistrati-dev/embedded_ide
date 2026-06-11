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
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::layout::stm32f1_layout;
    use super::super::stm32f1::stm32f1_graph;
    use crate::panels::mcu_module::clock::model::{ClockLimits, Stm32f1Clock};

    #[test]
    fn graph_clock_round_trips_via_ron() {
        let gc = GraphClock {
            graph: stm32f1_graph(&Stm32f1Clock::default()),
            layout: stm32f1_layout(&ClockLimits::default()),
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
        };
        let text = ron::to_string(&gc.graph).unwrap();
        let parsed_graph: ClockGraph = ron::from_str(&text).unwrap();
        assert_eq!(parsed_graph, gc.graph);
    }
}
