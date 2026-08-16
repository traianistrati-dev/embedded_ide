//! The **Settings** group in the project panel's Tools dropdown.
//!
//! One home for the IDE's own preferences — the ones that live in
//! `<per-user config dir>` and apply to every window and every project. Two
//! things deliberately do NOT belong here:
//!
//! - **project configuration** (runtime, auto-build, strict lints, clock…) —
//!   that is part of the project and travels with it in `mcu.config`, so it
//!   belongs in the System tab next to what it configures;
//! - **view state** (collapsed panels, diff line background, zoom) — toggled
//!   where it is seen, on the toolbar of the thing it affects.
//!
//! Adding a setting is meant to be one block in [`show`]: read its stored value,
//! render a control, write it back on change. Reading inside this function is
//! what keeps it cheap — the closure runs only while the menu is open, not every
//! frame.

use crate::startup::StartupMode;
use eframe::egui;

/// Render the Settings submenu body.
pub(super) fn show(ui: &mut egui::Ui) {
    // ── Startup ──────────────────────────────────────────────────────────────
    // This checkbox also exists inside the startup picker, but that one is not
    // enough on its own: with the setting off, the picker never appears, so its
    // own switch becomes unreachable. This is the copy that is always there.
    let mut ask = crate::startup::load_mode() == StartupMode::AlwaysAsk;
    if ui
        .checkbox(&mut ask, "Ask at startup")
        .on_hover_text(
            "On: every new window asks which project to open, listing the recent ones \
             and greying out those already open elsewhere.\n\
             Off: a window reopens the project it had last — unless another window \
             already has it, which always asks.",
        )
        .changed()
    {
        crate::startup::save_mode(if ask {
            StartupMode::AlwaysAsk
        } else {
            StartupMode::ReopenLast
        });
    }
}
