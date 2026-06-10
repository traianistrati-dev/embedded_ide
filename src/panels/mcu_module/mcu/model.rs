//! MCU data model — struct definition and rendering constants.

use crate::panels::mcu_module::clock::{ClockConfig, ClockLimits, ClockPreset};
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use crate::panels::mcu_module::pins::logic::pin::Pin;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

// ── Rendering constants ───────────────────────────────────────────────────────

pub const PIN_HEIGHT: f32 = 50.0;
pub const PIN_WIDTH: f32 = 30.0;
pub const PIN_SPACING: f32 = 3.0;

// ── Mcu struct ───────────────────────────────────────────────────────────────

/// Represents a microcontroller with four sides of pins and UI state.
#[derive(Clone)]
pub struct Mcu {
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
}
