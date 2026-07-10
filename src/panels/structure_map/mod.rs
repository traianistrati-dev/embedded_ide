//! "Structure" tab of the MCU Configurator — a module-relationship diagram of
//! the current project (main.rs + every file in the project tree).
//!
//! Phase 1 scope: **module-level** nodes and edges, built by PARSING the source
//! text (no LSP round-trips, so it can never disturb rust-analyzer's request
//! pipeline the way per-keystroke sync would — see the inline-type-hints
//! did_change lesson). Edges:
//!   * containment — `mod child;` declarations (parent → child), dashed;
//!   * dependency  — `use crate::…` / `super::…` / inline `a::b::c` path
//!     chains resolved against the known module set (user → used), solid.
//!
//! `parse` builds the graph, `layout` assigns layered positions, `gui` draws
//! it (fit + zoom + pan; click on a node opens its file in the editor).

pub mod gui;
pub mod layout;
pub mod parse;
