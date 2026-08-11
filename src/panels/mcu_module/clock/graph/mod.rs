//! Data-driven clock-graph (Phase 1 — model + evaluator, not yet wired to UI).
//!
//! - `model`   — the typed node taxonomy + `ClockGraph`.
//! - `eval`    — generic topological frequency evaluator.
//! - `stm32f1` — bridge that builds the F103 tree as a graph and proves the
//!               evaluator matches the hardcoded `compute.rs`.

pub mod auto_layout;
pub mod config;
pub mod cubemx;
pub mod edit;
pub mod esp32c3;
pub mod eval;
pub mod extract;
pub mod extract_tree;
pub mod import;
pub mod layout;
pub mod model;
pub mod render;
pub mod stm32f1;
pub mod stm32f4;
pub mod stm32g0;
pub mod stm32g4;
pub mod stm32l4;
pub mod stm32wba;
pub mod validate;

pub use auto_layout::{auto_layout, derive, place, place_missing};
pub use config::GraphClock;
pub use esp32c3::{esp32c3_graph, esp32c3_layout};
pub use eval::evaluate;
pub use import::{export_clock_ron, parse_clock_ron};
pub use layout::{ClockLayout, NodeBox, ValueSrc, stm32f1_layout};
pub use model::{ClockGraph, Edge, LimitKey, Node, NodeKind, NodeState};
pub use render::{graph_frequencies, value_from_graph, value_node_id};
pub use stm32f1::{graph_to_stm32f1, stm32f1_graph};
pub use stm32f4::{
    F4Clock, F4Sys, graph_to_f4, is_f4_graph, stm32f4_graph, stm32f4_layout, stm32f4_limits,
    stm32f4_limits_default,
};
pub use stm32g0::{is_g0_graph, stm32g0_graph};
pub use stm32g4::{is_g4_graph, stm32g4_graph};
pub use stm32l4::{is_l4_graph, stm32l4_graph};
pub use stm32wba::{
    WbaClock, WbaSys, graph_to_wba, is_wba_graph, stm32wba_graph, stm32wba_layout, stm32wba_limits,
};
pub use validate::{Overflow, over_limits};
