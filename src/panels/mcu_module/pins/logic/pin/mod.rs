//! Pin module — microcontroller pin representation and operations.
//!
//! Organizes pin functionality into logical components:
//! - model: Pin struct and rendering constants
//! - builders: Pin constructors and factory methods
//! - colors: Color logic for UI rendering

pub mod builders;
pub mod colors;
pub mod model;

// Re-export essentials for backward compatibility and convenience
pub use model::{Edge, GpioMode, PIN_FONT_SIZE, PIN_ROUNDING, Pin};
