//! Clock-tree configuration for the MCU Configurator "Clock" tab.
//!
//! Layers (mirrors the `pins` module split):
//! - `model`    — configuration state (`ClockConfig`, `Stm32f1Clock`).
//! - `compute`  — derive every frequency from a config (pure).
//! - `validate` — datasheet constraint checks → errors / warnings.
//! - `presets`  — one-click named configurations.
//! - `gui`      — the interactive Figure-2 diagram (added in a later phase).

pub mod compute;
pub mod graph;
pub mod gui;
pub mod model;
pub mod persist;
pub mod presets;
pub mod validate;

pub use compute::{ClockFrequencies, frequencies};
pub use model::{ClockConfig, ClockLimits, Stm32f1Clock};
pub use presets::ClockPreset;
pub use validate::{ClockWarning, Severity, warnings};
