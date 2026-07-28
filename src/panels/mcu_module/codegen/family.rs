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
use super::{embassy_common, rcc, stm32, wba};
use crate::panels::mcu_module::codegen_esp;
use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::modules;
use crate::panels::mcu_module::pins::logic::pin::Pin;

/// Family-specific `main.rs` generation. One implementor per chip family.
pub trait FamilyBackend {
    /// Family key this backend handles (matches `Mcu::family`). For a backend
    /// that spans several families (see [`StmEmbassyBackend`]) this is only a
    /// label — [`handles`](FamilyBackend::handles) decides the actual match.
    fn family_id(&self) -> &'static str;

    /// Whether this backend generates code for `family`. Default: an exact
    /// [`family_id`](FamilyBackend::family_id) match; multi-family backends
    /// override it.
    fn handles(&self, family: &str) -> bool {
        family == self.family_id()
    }

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
        let can = modules::can_configs(&mcu.modules);
        let usb = modules::usb_configs(&mcu.modules);
        let gen_ = stm32::make_generated_section(
            &mcu.name, &all, &mcu.clock, &usart, &spi, &i2c, &can, &usb,
        );
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
        let can = modules::can_configs(&mcu.modules);
        let usb = modules::usb_configs(&mcu.modules);
        let new_section = stm32::make_generated_section(
            &mcu.name, &all, &mcu.clock, &usart, &spi, &i2c, &can, &usb,
        );
        let spliced = stm32::splice_section(existing, &new_section, &mcu.name, &mcu.id);
        // Add the ADC helper if newly needed; preserve user-edited ones.
        stm32::ensure_helper_defs(spliced, &all)
    }

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        let all = pins_of(mcu);
        let usart = modules::usart_configs(&mcu.modules);
        let spi = modules::spi_configs(&mcu.modules);
        let i2c = modules::i2c_configs(&mcu.modules);
        let can = modules::can_configs(&mcu.modules);
        stm32::config_files(&all, &usart, &spi, &i2c, &can, &mcu.clock)
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

// ── STM32WBA (embassy-stm32, blocking) ──────────────────────────────────────
struct WbaBackend;

impl FamilyBackend for WbaBackend {
    fn family_id(&self) -> &'static str {
        "stm32wba"
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        let all = pins_of(mcu);
        format!(
            "{header}{section}\n{tail}",
            header = wba::invariant_header(&mcu.name, &mcu.id),
            section = wba::make_generated_section(&mcu.name, &all, &mcu.clock),
            tail = USER_TAIL,
        )
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let all = pins_of(mcu);
        let section = wba::make_generated_section(&mcu.name, &all, &mcu.clock);
        wba::splice_section(existing, &section, &mcu.name, &mcu.id)
    }
    // No per-peripheral config files yet — bus init is documented inline (v1).
}

// ── Generic STM32 (embassy-stm32, blocking) ─────────────────────────────────
// Covers every STM32 family embassy supports that has no dedicated backend
// (all but `stm32f1`, which uses stm32f1xx-hal, and `stm32wba`, which adds RCC
// clock mapping). The GPIO codegen is uniform across families — see
// `embassy_common`; the clock is left at embassy's reset default (per-family
// RCC graphs are a later step). This is what turns the many STM32 chips the
// pin-data XML importer adds from data-only into buildable projects.
struct StmEmbassyBackend;

impl FamilyBackend for StmEmbassyBackend {
    fn family_id(&self) -> &'static str {
        "stm32" // label only — `handles` does the real matching
    }

    fn handles(&self, family: &str) -> bool {
        // Every STM32 family except the two with their own backends.
        family.starts_with("stm32") && family != "stm32f1" && family != "stm32wba"
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        let all = pins_of(mcu);
        format!(
            "{header}{section}\n{tail}",
            header = embassy_common::invariant_header(&mcu.name, &mcu.id),
            // The RCC block is selected by FAMILY (data), not by sniffing the
            // graph's shape — `rcc::rcc_recipe` maps f4/wba (and any future
            // family) to its ReadSpec + descriptor; others get the reset default.
            section = embassy_common::make_generated_section(
                &mcu.name,
                &all,
                &rcc::graph_clock_block(&mcu.family, &mcu.clock),
            ),
            tail = USER_TAIL,
        )
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let all = pins_of(mcu);
        let section = embassy_common::make_generated_section(
            &mcu.name,
            &all,
            &rcc::graph_clock_block(&mcu.family, &mcu.clock),
        );
        embassy_common::splice_section(existing, &section, &mcu.name, &mcu.id)
    }
}

/// Registry of every known family backend. Add new families here. Order
/// matters: the first backend whose [`handles`](FamilyBackend::handles) returns
/// true wins, so the multi-family [`StmEmbassyBackend`] must come LAST.
const BACKENDS: &[&dyn FamilyBackend] = &[
    &Stm32f1Backend,
    &Esp32Backend,
    &WbaBackend,
    &StmEmbassyBackend,
];

/// Look up the backend for a family key, if one is registered.
///
/// Families without a backend yet (e.g. "stm8") return `None`; callers fall
/// back to "no code generation" so an unconfigured chip stays safe.
pub fn backend_for(family: &str) -> Option<&'static dyn FamilyBackend> {
    BACKENDS.iter().copied().find(|b| b.handles(family))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_families_resolve() {
        assert_eq!(backend_for("stm32f1").unwrap().family_id(), "stm32f1");
        assert_eq!(backend_for("esp32c3").unwrap().family_id(), "esp32c3");
        assert_eq!(backend_for("stm32wba").unwrap().family_id(), "stm32wba");
    }

    #[test]
    fn unknown_family_is_none() {
        assert!(backend_for("stm8").is_none());
        assert!(backend_for("").is_none());
        assert!(backend_for("rp2040").is_none());
    }

    /// Any other STM32 family routes to the generic embassy backend, and it
    /// emits a complete, buildable embassy skeleton.
    #[test]
    fn other_stm32_families_use_the_generic_embassy_backend() {
        for fam in ["stm32f4", "stm32g0", "stm32g4", "stm32l4", "stm32h7", "stm32c0"] {
            let b = backend_for(fam).unwrap_or_else(|| panic!("no backend for {fam}"));
            assert_eq!(b.family_id(), "stm32", "{fam} should hit the generic backend");
        }
        // The dedicated backends still win over the generic one.
        assert_eq!(backend_for("stm32f1").unwrap().family_id(), "stm32f1");
        assert_eq!(backend_for("stm32wba").unwrap().family_id(), "stm32wba");

        use crate::panels::mcu_module::mcu_def::McuDefinition;
        let mut def: McuDefinition =
            crate::panels::mcu_module::builtins::builtin_for("stm32f103c8t6").unwrap();
        // Use a family WITHOUT an RCC recipe (h7) so the clock stays reset
        // default — g0/g4/l4 now read the graph and would emit a config block.
        def.family = "stm32h7".into();
        def.id = "stm32h743zi".into();
        let code = def.build_mcu().fresh_main_rs();
        assert!(code.contains("HAL: embassy-stm32 (blocking)"));
        assert!(code.contains("fn main() -> !"));
        assert!(code.contains("embassy_stm32::init(Default::default())"));
        assert!(code.contains(crate::panels::mcu_module::codegen::GEN_BEGIN));
    }

    /// The WBA backend produces a complete embassy skeleton with the markers,
    /// entry point and user tail — enough for the project to build.
    #[test]
    fn wba_backend_emits_a_complete_skeleton() {
        use crate::panels::mcu_module::mcu_def::McuDefinition;
        let mut def: McuDefinition =
            crate::panels::mcu_module::builtins::builtin_for("stm32f103c8t6").unwrap();
        def.family = "stm32wba".into();
        def.id = "stm32wba55cg".into();
        let mcu = def.build_mcu();
        let code = mcu.fresh_main_rs();
        assert!(code.contains("#![no_std]"));
        assert!(code.contains("fn main() -> !"));
        assert!(code.contains("embassy_stm32::init"));
        assert!(code.contains(crate::panels::mcu_module::codegen::GEN_BEGIN));
        assert!(code.contains(crate::panels::mcu_module::codegen::GEN_END));
    }
}
