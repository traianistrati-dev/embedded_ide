//! Code generation for MCU projects — routes to toolchain-specific generators.
//!
//! Handles:
//! - STM32 (Rust Embedded HAL) via `stm32` module
//! - ESP32-C3 (esp-hal) via parent `codegen_esp` module
//! - STM8 (SDCC C) — not yet implemented

pub mod common;
pub mod stm32;

use super::mcu::Mcu;
use super::mcu_catalog::ToolchainKind;
use super::pins::logic::pin::Pin;

// Re-export public API for backward compatibility
pub use common::{parse_main_rs, GEN_BEGIN, GEN_END, USER_TAIL};

// ── Public API on Mcu ─────────────────────────────────────────────────────────

impl Mcu {
    /// Build a brand-new `src/main.rs` (called when the MCU type is first
    /// selected or reset).  Dispatches on `self.toolchain`.
    pub fn fresh_main_rs(&self) -> String {
        match self.toolchain {
            ToolchainKind::RustEmbedded => {
                let all = self.all_pins();
                let gen_ = stm32::make_generated_section(&self.name, &all);
                format!(
                    "{header}{gen_}\n{tail}",
                    header = stm32::invariant_header(&self.name),
                    tail = USER_TAIL,
                )
            }
            ToolchainKind::EspRust => {
                let all = self.all_pins();
                super::codegen_esp::fresh_esp32c3_main_rs(&all)
            }
            ToolchainKind::SdccC => {
                // STM8 — not yet implemented
                String::new()
            }
        }
    }

    /// Update `existing` in-place: replace only the generated section
    /// (between the markers), preserving the user-editable parts.
    ///
    /// For toolchains without a GEN block (EspRust, SdccC) the existing
    /// file is returned unchanged — pin state is not reflected in code yet.
    pub fn update_main_rs(&self, existing: &str) -> String {
        match self.toolchain {
            ToolchainKind::RustEmbedded => {
                let all = self.all_pins();
                let new_section = stm32::make_generated_section(&self.name, &all);
                stm32::splice_section(existing, &new_section, &self.name)
            }
            ToolchainKind::EspRust => {
                let all = self.all_pins();
                super::codegen_esp::update_esp32c3_main_rs(existing, &all)
            }
            // SdccC — not yet implemented; return file unchanged.
            ToolchainKind::SdccC => existing.to_owned(),
        }
    }

    /// Kept for any remaining call sites — delegates to `fresh_main_rs`.
    #[allow(dead_code)]
    pub fn generate_code(&self) -> String {
        self.fresh_main_rs()
    }

    fn all_pins(&self) -> Vec<&Pin> {
        self.top_pins
            .iter()
            .chain(self.bottom_pins.iter())
            .chain(self.left_pins.iter())
            .chain(self.right_pins.iter())
            .collect()
    }
}
