//! Editor GUI components — status bar, diagnostics overlay, text-position helpers.

pub mod code_editor;
pub mod status_bar;
pub mod diagnostics_overlay;
pub mod text_pos;

pub use status_bar::show_ra_status_bar;
pub use diagnostics_overlay::show_diagnostics_overlay;
