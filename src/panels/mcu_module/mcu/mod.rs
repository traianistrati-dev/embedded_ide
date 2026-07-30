//! MCU module — microcontroller representation with GUI visualization.
//!
//! Organizes MCU functionality into logical components:
//! - model: Mcu struct definition and constants
//! - logic: Business logic (state management, partner assignment)
//! - gui: Rendering and user interaction (draw method + components)

pub mod gui;
pub mod logic;
pub mod model;

// Re-export essentials for backward compatibility and convenience
pub use model::{Mcu, Runtime, PIN_HEIGHT, PIN_WIDTH, PIN_SPACING};
