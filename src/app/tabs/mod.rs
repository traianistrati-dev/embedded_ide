//! UI tabs for diagnostics and MCU configuration panels.

pub mod mcu_tab;
pub mod cargo_tab;
pub mod ra_tab;
pub mod dfu_tab;
pub mod tools_tab;

// Re-export all tab functions for convenience
pub use mcu_tab::show_peripherals_tab;
pub use cargo_tab::show_cargo_tab;
pub use ra_tab::show_ra_tab;
pub use dfu_tab::show_dfu_tab;
pub use tools_tab::show_tools_tab;
