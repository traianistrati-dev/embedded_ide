//! Editor GUI components — status bar, diagnostics overlay.

pub mod status_bar;
pub mod diagnostics_overlay;

pub use status_bar::show_ra_status_bar;
pub use diagnostics_overlay::show_diagnostics_overlay;
