//! The product's names: the one it is SHOWN by, and the ones its data is KEPT
//! under.
//!
//! Renamed from "Embedded IDE" (`embedded_ide_0`) to RustOnChip on 2026-09-20.
//! Only what a user reads moved. The names below the display name key data that
//! already exists on every install, and a new spelling would quietly move or
//! orphan it:
//!
//! - the imported chips in the config folder — a project on one then reopens on
//!   a DIFFERENT chip that shares its HAL crate, and the next Save rewrites
//!   `main.rs`, `Cargo.toml`, `memory.x` and `.cargo/config.toml` for it;
//! - every window's saved project pointer and layout (eframe's storage);
//! - gigabytes of build caches, which the stale-slot sweep would no longer find.
//!
//! So they are frozen here, under names that say why, and nothing may derive
//! them from `CARGO_PKG_NAME` — that would tie user data to the package name
//! and move it the next time the package is renamed.

/// What the user reads: window titles, dialogs, the banner of generated files.
pub const APP_DISPLAY_NAME: &str = "RustOnChip";

/// eframe's storage identity. eframe derives the folder holding `app.ron` — the
/// open-project pointer and the layout, one per window slot — from the app
/// name: `%APPDATA%\Embedded IDE[_N]\data` on Windows. It looks like a title
/// and is not one; the title is set separately.
///
/// Frozen. Setting `ViewportBuilder::with_app_id` would move this storage too:
/// eframe prefers the app id over the name.
pub const LEGACY_EFRAME_NAME: &str = "Embedded IDE";

/// The per-user config folder: imported chips, API keys, the paid datasheet
/// cache, recent projects, the crash log. Frozen.
pub const LEGACY_DATA_DIR: &str = "embedded_ide_0";

/// Prefix of the build-workspace slot folders (`…_check`, `…_check_2`, …).
/// Frozen — and never to be SHORTENED: the stale-slot sweep deletes unlocked
/// folders starting with it in the temp dir, and a bare product name there
/// would match the user's own folders.
pub const LEGACY_SLOT_PREFIX: &str = "embedded_ide_0_check";
