//! Per-family code-generation backends.
//!
//! A [`FamilyBackend`] captures the *family-specific* part of code generation —
//! the HAL idioms and `main.rs` skeleton that differ between chip families
//! (STM32F1 vs ESP32-C3 vs future RP2040/nRF…). Dispatch is keyed on
//! [`Mcu::family`](crate::panels::mcu_module::mcu::Mcu), which is finer-grained
//! than [`ToolchainKind`](crate::panels::mcu_module::mcu_catalog::ToolchainKind)
//! — all ARM HALs share `RustEmbedded` yet generate different code.
//!
//! **Adding a new family** = implement this trait in a new `struct` and add it
//! to [`BACKENDS`]. New *chips* inside an already-supported family need no code
//! here — they are pure data (a `.ron` definition).

use super::common::USER_TAIL;
use super::stm32;
use crate::panels::mcu_module::codegen_esp;
use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::modules;
use crate::panels::mcu_module::pins::logic::pin::Pin;

/// Family-specific `main.rs` generation. One implementor per chip family.
pub trait FamilyBackend {
    /// Family key this backend handles (matches `Mcu::family`).
    fn family_id(&self) -> &'static str;

    /// Build a brand-new `src/main.rs` from the MCU's pins + clock.
    fn fresh_main_rs(&self, mcu: &Mcu) -> String;

    /// Re-splice only the generated section of an existing `main.rs`,
    /// preserving user-editable code outside the markers.
    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String;

    /// Per-peripheral init module bodies for `src/pins/configs/` — `(file_name,
    /// generated_body)`. Default: none (families without separate config files).
    fn config_files(&self, _mcu: &Mcu) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// All four sides of the chip, in the canonical order codegen expects
/// (top, bottom, left, right — same as the old private `Mcu::all_pins`).
fn pins_of(mcu: &Mcu) -> Vec<&Pin> {
    mcu.iter_all_pins().collect()
}

// ── STM32F1 (stm32f1xx-hal) ─────────────────────────────────────────────────
struct Stm32f1Backend;

impl FamilyBackend for Stm32f1Backend {
    fn family_id(&self) -> &'static str {
        "stm32f1"
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        let all = pins_of(mcu);
        let usart = modules::usart_configs(&mcu.modules);
        let spi = modules::spi_configs(&mcu.modules);
        let i2c = modules::i2c_configs(&mcu.modules);
        let gen_ = stm32::make_generated_section(&mcu.name, &all, &mcu.clock, &usart, &spi, &i2c);
        let base = format!(
            "{header}{gen_}\n{tail}",
            header = stm32::invariant_header(&mcu.name, &mcu.id),
            tail = USER_TAIL,
        );
        // Only the ADC init helper lives after `fn main`; USART/SPI/I2C init are
        // in `src/pins/configs/`.
        stm32::ensure_helper_defs(base, &all)
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let all = pins_of(mcu);
        let usart = modules::usart_configs(&mcu.modules);
        let spi = modules::spi_configs(&mcu.modules);
        let i2c = modules::i2c_configs(&mcu.modules);
        let new_section =
            stm32::make_generated_section(&mcu.name, &all, &mcu.clock, &usart, &spi, &i2c);
        let spliced = stm32::splice_section(existing, &new_section, &mcu.name, &mcu.id);
        // Add the ADC helper if newly needed; preserve user-edited ones.
        stm32::ensure_helper_defs(spliced, &all)
    }

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        let all = pins_of(mcu);
        let usart = modules::usart_configs(&mcu.modules);
        let spi = modules::spi_configs(&mcu.modules);
        let i2c = modules::i2c_configs(&mcu.modules);
        stm32::config_files(&all, &usart, &spi, &i2c)
    }
}

// ── ESP32-C3 (esp-hal) ──────────────────────────────────────────────────────
struct Esp32Backend;

impl FamilyBackend for Esp32Backend {
    fn family_id(&self) -> &'static str {
        "esp32c3"
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        let usart = modules::usart_configs(&mcu.modules);
        let spi = modules::spi_configs(&mcu.modules);
        let i2c = modules::i2c_configs(&mcu.modules);
        codegen_esp::fresh_esp32c3_main_rs(&pins_of(mcu), &mcu.clock, &mcu.id, &usart, &spi, &i2c)
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let usart = modules::usart_configs(&mcu.modules);
        let spi = modules::spi_configs(&mcu.modules);
        let i2c = modules::i2c_configs(&mcu.modules);
        codegen_esp::update_esp32c3_main_rs(
            existing,
            &pins_of(mcu),
            &mcu.clock,
            &mcu.id,
            &usart,
            &spi,
            &i2c,
        )
    }
}

/// Registry of every known family backend. Add new families here.
const BACKENDS: &[&dyn FamilyBackend] = &[&Stm32f1Backend, &Esp32Backend];

/// Look up the backend for a family key, if one is registered.
///
/// Families without a backend yet (e.g. "stm8") return `None`; callers fall
/// back to "no code generation" so an unconfigured chip stays safe.
pub fn backend_for(family: &str) -> Option<&'static dyn FamilyBackend> {
    BACKENDS.iter().copied().find(|b| b.family_id() == family)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_families_resolve() {
        assert_eq!(backend_for("stm32f1").unwrap().family_id(), "stm32f1");
        assert_eq!(backend_for("esp32c3").unwrap().family_id(), "esp32c3");
    }

    #[test]
    fn unknown_family_is_none() {
        assert!(backend_for("stm8").is_none());
        assert!(backend_for("").is_none());
    }
}
