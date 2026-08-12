//! MCU data model — struct definition and rendering constants.

use crate::panels::mcu_module::clock::graph::ClockGraph;
use crate::panels::mcu_module::clock::{ClockConfig, ClockLimits, ClockPreset};
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use crate::panels::mcu_module::pins::logic::pin::Pin;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

// ── Rendering constants ───────────────────────────────────────────────────────

pub const PIN_HEIGHT: f32 = 50.0;
pub const PIN_WIDTH: f32 = 30.0;
pub const PIN_SPACING: f32 = 3.0;

// ── Runtime ────────────────────────────────────────────────────────────────

/// Project-level execution model — which code-generation runtime the firmware
/// targets.
///
/// - [`Runtime::Blocking`] (default): the classic bare-metal path — a
///   `#[cortex_m_rt::entry] fn main() -> !` with PORTABLE blocking APIs
///   (`embedded-io` / `embedded-hal` 1.0 bridges), so driver code is portable
///   across HALs. Per-module the peripheral can opt into Native.
/// - [`Runtime::Native`]: also bare-metal `#[entry] fn main()`, but the
///   peripherals expose the CONCRETE HAL types (`Serial`/`Spi`/`BlockingI2c` on
///   stm32f1xx-hal) — no portable bridges, no extra trait crates, max HAL
///   features. A project-wide "all native" shortcut; the per-module Portable/
///   Native selector is subsumed. Only where the concrete-HAL templates exist
///   ([`super::super::codegen::family::native_supported`] = STM32F1).
/// - [`Runtime::Async`]: an embassy async project — `#[embassy_executor::main]
///   async fn main(Spawner)` on `embassy-stm32`, with `.await`-able drivers
///   (`embedded-io-async` / `embedded-hal-async`). Selected in the System tab;
///   for STM32 it re-targets code generation to the async embassy backend.
///
/// - [`Runtime::Rtic`]: an RTIC 2.x project — `#[rtic::app]` with `Shared`,
///   `Local`, `#[init]`, `#[idle]`, and one hardware task per interrupt-enabled
///   input pin. Still bare-metal and still the concrete HAL, but the scheduler
///   owns `main`: clock + peripheral init moves into `#[init]`, and an interrupt
///   is a `#[task(binds = ...)]` rather than an `#[interrupt] fn` the user has
///   to write. Only where a cortex-m PAC + the `ExtiPin` trait exist
///   ([`super::super::codegen::family::rtic_supported`] = STM32F1).
///
/// Persisted in `mcu.config` (`@runtime`); old projects (no section) load as
/// `Blocking`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Runtime {
    #[default]
    Blocking,
    Native,
    Async,
    Rtic,
}

impl Runtime {
    /// The token written to / read from the `mcu.config` `@runtime` section.
    pub fn as_token(self) -> &'static str {
        match self {
            Self::Blocking => "Blocking",
            Self::Native => "Native",
            Self::Async => "Async",
            Self::Rtic => "Rtic",
        }
    }

    /// Parse the `@runtime` token; anything unrecognised (incl. a missing
    /// section) is the safe [`Runtime::Blocking`] default.
    pub fn from_token(s: &str) -> Self {
        match s.trim() {
            "Async" => Self::Async,
            "Native" => Self::Native,
            "Rtic" => Self::Rtic,
            _ => Self::Blocking,
        }
    }
}

// ── Auto-build on Save ───────────────────────────────────────────────────────

/// What the IDE does on Project Save when a library changed in `Cargo.toml`.
/// A workflow preference (chosen in the System tab), NOT a codegen choice — it
/// doesn't regenerate anything, so it's applied immediately (no staged Apply)
/// and kept out of the codegen state hash. Persisted in `mcu.config` (`@autobuild`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AutoBuild {
    /// Never auto-build; the user runs Build/Check manually.
    Off,
    /// Run `cargo check` (fast; catches errors + resolves new deps). Default.
    #[default]
    Check,
    /// Run `cargo build --release` (slower; a full optimized build).
    Release,
}

impl AutoBuild {
    pub fn as_token(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Check => "Check",
            Self::Release => "Release",
        }
    }
    pub fn from_token(s: &str) -> Self {
        match s.trim() {
            "Off" => Self::Off,
            "Release" => Self::Release,
            _ => Self::Check,
        }
    }
}

// ── Mcu struct ───────────────────────────────────────────────────────────────

/// Represents a microcontroller with four sides of pins and UI state.
#[derive(Clone)]
pub struct Mcu {
    /// Stable definition id (e.g. "esp32c3-graph"). Written into the generated
    /// `main.rs` header so reopening restores the exact chip. Empty when built
    /// outside the registry (e.g. unit tests via [`Mcu::new`]).
    pub id: String,
    pub name: String,
    /// Family / backend key (e.g. "stm32f1", "esp32c3") — selects the
    /// [`FamilyBackend`](crate::panels::mcu_module::codegen::family::FamilyBackend)
    /// used for code generation. Distinguishes chips that share a toolchain
    /// (e.g. all ARM HALs are `RustEmbedded` but differ per family).
    pub family: String,
    /// Toolchain family — governs which build/flash pipeline is used.
    pub toolchain: ToolchainKind,
    pub top_pins: Vec<Pin>,
    pub bottom_pins: Vec<Pin>,
    pub left_pins: Vec<Pin>,
    pub right_pins: Vec<Pin>,
    /// Currently selected pin number (None = no pin selected)
    pub selected_pin: Option<usize>,
    /// Function whose ⓘ info window is open (None = closed)
    pub show_info: Option<PinFunction>,
    /// Vertical scroll offset (pixels) of the function list inside the chip.
    /// Fed by the wheel in `mcu_panel.rs` (which owns the canvas' wheel), clamped
    /// while the list is drawn.
    pub fn_scroll_offset: f32,
    /// Clock-tree configuration shown/edited in the "Clock" tab.
    /// `ClockConfig::None` for MCUs without a modelled clock tree yet.
    pub clock: ClockConfig,
    /// Per-chip datasheet frequency ceilings (validation + red diagram tags).
    /// Defaults to the STM32F103 values; imported `.ron` chips may override.
    pub clock_limits: ClockLimits,
    /// Chip-specific clock presets from the definition; empty → the family's
    /// built-in presets are shown in the Clock tab.
    pub clock_presets: Vec<ClockPreset>,
    /// Pristine clock-graph node states, exactly as the chip definition ships
    /// them — the target of the Clock tab's "Reset" button. Captured when the
    /// MCU is built, BEFORE any saved `@clock` state is adopted, so a reset
    /// really returns to the factory tree and not to whatever the project was
    /// opened with. `None` for chips with no modelled clock tree.
    pub clock_defaults: Option<ClockGraph>,
    /// Virtual electronic modules (e.g. _USART) wired to the chip's pins and
    /// drawn beside it on the Pins canvas.
    pub modules: Vec<crate::panels::mcu_module::modules::VirtualModule>,
    /// Execution model — Blocking (bare-metal) or Async (embassy). Chosen in the
    /// System tab; drives which family backend generates `main.rs` and the
    /// embassy runtime deps. Persisted in `mcu.config`.
    pub runtime: Runtime,
    /// GPIO In/Out init style (STM32F1 blocking path): `Portable` wraps pins in
    /// the `pins/configs/io.rs` embedded-hal 1.0 bridge (`DigitalOut`/`DigitalIn`
    /// + `Delay`); `Native` binds the raw HAL pin (no io.rs, no embedded-hal for
    /// GPIO). Chosen in the System tab. Forced to raw on the Native runtime;
    /// irrelevant on Async (embassy `Output`/`Input`). Persisted in `mcu.config`.
    pub gpio_api: crate::panels::mcu_module::modules::ApiStyle,

    // ── Staged codegen-style choices (System tab "Apply") ─────────────────────
    // The APPLIED fields above (`runtime`, `gpio_api`) + each module's
    // `config.api_style`/`async_mode` are what codegen reads and the regen hash
    // gates on. The `pending_*` below are what the UI edits; nothing regenerates
    // until the user clicks **Apply**, which commits pending → applied. All
    // transient (not persisted, not hashed).
    /// Staged runtime (edited by the System-tab cards; applied on "Apply").
    pub pending_runtime: Runtime,
    /// Staged GPIO api (System-tab cards).
    pub pending_gpio_api: crate::panels::mcu_module::modules::ApiStyle,
    /// Staged per-module `(api_style, async_mode)` keyed by module id. Lazily
    /// populated when a module's config panel is opened; a module with no entry
    /// has no staged change.
    pub pending_module_styles: std::collections::BTreeMap<
        String,
        (
            crate::panels::mcu_module::modules::ApiStyle,
            crate::panels::mcu_module::modules::AsyncBusMode,
        ),
    >,
    /// Transient: the inline "Apply — Confirm/Cancel" prompt is showing.
    pub pending_apply_confirm: bool,

    /// Transient one-shot: set by `apply_pending_style` so the NEXT config-file
    /// regeneration rewrites the peripheral `configs/*.rs` templates in FULL
    /// (not just the constants block). A Runtime / Init-API change swaps the
    /// whole `init()` template (blocking ⇄ async ⇄ native), which lives in the
    /// "editable" region a normal splice preserves — so without this the code
    /// would not update on Apply. Cleared after that regen.
    pub config_regen_forced: bool,

    /// What Save does when a Cargo.toml library changed (System-tab preference,
    /// persisted). Default `Check`.
    pub auto_build: AutoBuild,
    /// Strict "deny panics" Clippy profile (System-tab preference, persisted).
    /// When on, a `[lints.clippy]` block (pedantic/nursery + unwrap/panic/
    /// indexing/arithmetic/as… all `deny`) is written into the project
    /// `Cargo.toml`, and the GENERATED code (main.rs entry fn + `configs/*.rs`)
    /// is exempted with `#[allow]` so only the user's own code is linted.
    /// Default `false`. See `project_gen::ensure_strict_lints`.
    pub strict_lints: bool,
    /// Debug-friendly `[profile.release]` (Debug-tab toggle, persisted). When
    /// on, the project's release profile is relaxed to `opt-level = 0`,
    /// `lto = false`, `debug = true` so every source line keeps code of its own
    /// and can hold a breakpoint. Off (default) is the optimised profile that
    /// normally gets flashed — smaller and correctly timed, but breakpoints on
    /// lines the optimiser folded away never arm.
    /// See `project_gen::ensure_debug_build`.
    pub debug_build: bool,
    /// Transient: id of a module the user clicked on the canvas, so the module
    /// list (below the chip) expands its entry next frame. Consumed + cleared by
    /// the panel. Not part of project state.
    pub expand_module: Option<String>,
    /// Undo stack for explicit Virtual-module add/remove (Ctrl+Z on the Pins
    /// tab). Each entry snapshots the modules + pin state before the change so
    /// an unwanted add/remove can be reverted. Transient, capped, not persisted.
    pub module_undo: Vec<ModuleUndo>,
    /// Transient: id of the module whose "Remove" button is armed and awaiting
    /// an inline confirmation (delete resets its pins, so it's confirmed first).
    pub module_remove_confirm: Option<String>,
    /// Transient: number of a pin the user just clicked, requesting a jump to the
    /// line that defines its variable in the generated code. Consumed + cleared
    /// by the panel (`AppIde`), which owns the editor. Not part of project state.
    pub pin_goto: Option<usize>,
    /// Transient: id of a module the user just clicked, requesting the same jump
    /// for EVERY pin it wires. Same consumer as [`Mcu::pin_goto`].
    pub module_goto: Option<String>,
    /// Id of the module currently selected on the canvas — drawn white, bigger
    /// and double-bordered, the module counterpart of [`Mcu::selected_pin`].
    /// Clicking it again clears it. View state only; not persisted.
    pub selected_module: Option<String>,
    /// Transient one-shot: a click on EMPTY canvas (beside the chip, on no
    /// module box and no pin field) cleared the selection, so the module list
    /// below should collapse every entry too — the canvas and the list show the
    /// same "nothing is focused" state. Consumed + cleared by the panel.
    pub collapse_modules: bool,
    /// Diagram rotation toggle (persisted in `mcu.config` `@rotation`). `false`
    /// = default orientation. `true` = rotated one step clockwise: a 4-sided
    /// (QFP) chip becomes a 45° diamond, a 2-sided (DIP) chip turns 90°
    /// (vertical ⇄ horizontal). Purely a view/layout preference — never affects
    /// codegen. See `mcu/gui/rotate.rs`.
    pub rotated: bool,
    /// Manual on-canvas positions for in/out (GPIO In/Out/PWM) pin rename
    /// fields: pin number → field-CENTRE offset from the chip centre. Absent =
    /// auto-placed beside the pin. Draggable like virtual modules; persisted in
    /// `mcu.config` `@iopins`. View-only.
    pub io_pin_pos: std::collections::BTreeMap<usize, (f32, f32)>,
}

/// One Virtual-module undo snapshot (see [`Mcu::module_undo`]): the modules and
/// the pin state (`(number, function, custom_label)`) as they were BEFORE an
/// add/remove, plus a short label for the Undo button's hover.
#[derive(Clone)]
pub struct ModuleUndo {
    pub modules: Vec<crate::panels::mcu_module::modules::VirtualModule>,
    pub pins: Vec<(usize, PinFunction, String)>,
    pub label: String,
}
