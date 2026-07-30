//! MCU data model — struct definition and rendering constants.

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
/// Persisted in `mcu.config` (`@runtime`); old projects (no section) load as
/// `Blocking`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Runtime {
    #[default]
    Blocking,
    Native,
    Async,
}

impl Runtime {
    /// The token written to / read from the `mcu.config` `@runtime` section.
    pub fn as_token(self) -> &'static str {
        match self {
            Self::Blocking => "Blocking",
            Self::Native => "Native",
            Self::Async => "Async",
        }
    }

    /// Parse the `@runtime` token; anything unrecognised (incl. a missing
    /// section) is the safe [`Runtime::Blocking`] default.
    pub fn from_token(s: &str) -> Self {
        match s.trim() {
            "Async" => Self::Async,
            "Native" => Self::Native,
            _ => Self::Blocking,
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
    /// Vertical scroll offset (pixels) for the function-list panel inside the chip.
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
    /// Virtual electronic modules (e.g. GI_USART) wired to the chip's pins and
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
    /// Transient: id of a module the user clicked on the canvas, so the module
    /// list (below the chip) expands its entry next frame. Consumed + cleared by
    /// the panel. Not part of project state.
    pub expand_module: Option<String>,
}
