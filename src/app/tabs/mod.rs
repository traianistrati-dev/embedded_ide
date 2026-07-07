//! UI tabs for diagnostics and MCU configuration panels.

pub mod mcu_tab;
pub mod cargo_tab;
pub mod ra_tab;
pub mod clippy_tab;
pub mod dfu_tab;
pub mod serial_tab;
pub mod terminal_tab;
pub mod activity_tab;
pub mod tools_tab;
pub mod git_tab;

// Re-export all tab functions for convenience
pub use mcu_tab::show_peripherals_tab;
pub use cargo_tab::show_cargo_tab;
pub use clippy_tab::show_clippy_tab;
pub use ra_tab::show_ra_tab;
pub use dfu_tab::show_dfu_tab;
pub use serial_tab::show_serial_tab;
pub use terminal_tab::show_terminal_tab;
pub use activity_tab::show_activity_tab;
pub use tools_tab::show_tools_tab;
pub use git_tab::show_git_tab;
