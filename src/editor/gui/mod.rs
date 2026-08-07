//! Editor GUI components — status bar, diagnostics overlay, text-position helpers.

pub mod code_editor;
pub mod diagnostics_overlay;
pub mod status_bar;
pub mod text_pos;

pub use diagnostics_overlay::{show_diagnostics_overlay, show_inlay_hint, show_line_band};
pub use status_bar::show_ra_status_bar;
