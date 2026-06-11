//! Generic, data-driven clock-graph model (Phase 1).
//!
//! An MCU clock tree — for *any* family — is a directed acyclic graph: a set of
//! typed [`Node`]s connected by [`Edge`]s. The node *kinds* are a small fixed
//! taxonomy (oscillator, mux, divider, multiplier, …); the topology, the per-node
//! options and the layout are pure **data** that a chip can carry in its `.ron`
//! definition. Frequencies are derived by a generic evaluator (`eval.rs`), so a
//! new chip's clock tree is importable without bespoke arithmetic.
//!
//! This module runs *in parallel* with the hardcoded [`Stm32f1Clock`] model — it
//! is not wired into the UI or codegen yet (see the phased plan). The
//! [`super::stm32f1`] bridge proves the evaluator reproduces `compute.rs`.

use serde::{Deserialize, Serialize};

/// Stable identifier for a node, unique within a graph (e.g. "pllmul", "hclk").
pub type NodeId = String;

/// Which datasheet ceiling (in [`ClockLimits`](crate::panels::mcu_module::clock::model::ClockLimits))
/// an [`NodeKind::Output`] is checked against. UI/validate concern only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LimitKey {
    SysclkMax,
    HclkMax,
    Pclk1Max,
    Pclk2Max,
    AdcclkMax,
    UsbclkHz,
}

/// The behaviour of a node — how it turns its input frequency(ies) into an
/// output. Immutable topology data (the user's *selection* lives in [`NodeState`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    /// Oscillator / clock source. `gated` sources can be turned off; a fixed
    /// source uses `min_hz == max_hz`. The live frequency lives in the state.
    Source { min_hz: u32, max_hz: u32, gated: bool },
    /// 1-of-N selector. Inputs arrive via edges, addressed by their `input` index.
    Mux { inputs: usize },
    /// Configurable integer divider chosen from a discrete option set.
    Divider { options: Vec<u32> },
    /// Fixed integer divider by a constant (e.g. HSI/2, HSE/128).
    FixedDiv { by: u32 },
    /// Two-or-more-way ratio choice: output = input × num / den for the picked
    /// option. Covers non-integer / selectable ratios — PLLXTPRE (/1, /2),
    /// SysTick (/8, /1), USB (/1.5 = ×2/3, /1).
    Choice { ratios: Vec<(u32, u32)> },
    /// Integer multiplier (PLL) chosen from an inclusive range.
    Multiplier { min: u32, max: u32 },
    /// Pass-through tap — a named, fan-out-able intermediate (PLLCLK, SYSCLK…).
    Tap,
    /// Timer-clock rule: ×1 when the referenced prescaler's divisor is 1, else ×2.
    TimerMul { prescaler: NodeId },
    /// Leaf sink — a delivered clock (HCLK, PCLK1, …). Whether it has a ceiling
    /// is a `Node`-level concern ([`Node::limit`]), not the kind's.
    Output,
}

/// The mutable, user-selectable part of a node — what gets serialized for
/// round-trip and edited in the diagram.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeState {
    /// No selectable state (FixedDiv, Tap, Output, TimerMul).
    Fixed,
    /// A mux with nothing selected → output 0 (e.g. RTC/MCO disabled).
    Unset,
    /// Oscillator on/off + current frequency.
    Source { enabled: bool, hz: u32 },
    /// Selected option index — for Mux / Divider / Choice.
    Index(usize),
    /// Multiplier value.
    Value(u32),
}

/// One node: identity + behaviour + current selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub state: NodeState,
    /// Datasheet ceiling this node's output is validated against, if any. Lives
    /// here (not inside `Output`) so fan-out taps like SYSCLK / PLLCLK can be
    /// bounded too. `None` = unbounded.
    #[serde(default)]
    pub limit: Option<LimitKey>,
}

/// A directed connection: `from`'s output feeds `to`'s input number `input`.
/// `input` matters only for multi-input nodes (muxes); single-input nodes use 0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    #[serde(default)]
    pub input: usize,
}

/// A complete clock tree as data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClockGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl ClockGraph {
    /// Borrow a node by id.
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Mutably borrow a node by id (used by the diagram to edit selections).
    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
}
