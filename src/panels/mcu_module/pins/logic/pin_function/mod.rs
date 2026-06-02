//! PinFunction module — microcontroller peripheral function representation.
//!
//! Organizes pin functions into logical components:
//! - enum_: PinFunction enum and FunctionInfo struct definitions
//! - display: label(), from_label(), short_label(), hal_gpio_mode() methods
//! - colors: color() method for UI rendering
//! - info: info() method with peripheral specifications

pub mod colors;
pub mod display;
pub mod enum_;
pub mod info;

// Re-export essentials for backward compatibility and convenience
pub use enum_::{FunctionInfo, PinFunction};
