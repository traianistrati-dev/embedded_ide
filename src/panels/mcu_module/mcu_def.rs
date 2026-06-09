//! Data-driven MCU definition (Phase 1 foundation).
//!
//! A serializable description of a chip — pins, clock, project params — that can
//! be loaded from a RON file.  [`McuDefinition::build_mcu`] turns it into the
//! runtime [`Mcu`] the configurator draws.
//!
//! This runs **in parallel** with the existing hardcoded factories
//! (`mock_mcu.rs`, `mock_esp32c3.rs`); later phases make the definition the
//! single source of truth and add the registry + family backends.

use serde::{Deserialize, Serialize};

use super::clock::{ClockConfig, Stm32f1Clock};
use super::mcu::Mcu;
use super::mcu_catalog::ToolchainKind;
use super::pins::logic::pin::Pin;
use super::pins::logic::pin_function::PinFunction;

/// One pin in a chip definition — the data form of [`Pin`] without runtime state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PinDef {
    pub number: usize,
    pub name: String,
    #[serde(default)]
    pub reserved: bool,
    /// Complete list of selectable functions (e.g. `[GpioInput, GpioOutput, …]`).
    /// Empty for reserved pins (VDD, VSS, …).
    #[serde(default)]
    pub functions: Vec<PinFunction>,
}

impl PinDef {
    /// Extract a `PinDef` from a runtime [`Pin`] — used by round-trip tests and
    /// to export the current hardcoded factories to RON.
    pub fn from_pin(p: &Pin) -> Self {
        Self {
            number: p.number,
            name: p.name.clone(),
            reserved: p.reserved,
            functions: p.available_functions.clone(),
        }
    }

    /// Build a runtime [`Pin`] (the selected function starts `Unset`).
    pub fn to_pin(&self) -> Pin {
        Pin {
            number: self.number,
            name: self.name.clone(),
            reserved: self.reserved,
            available_functions: self.functions.clone(),
            selected_function: PinFunction::Unset,
        }
    }
}

/// The four physical sides of the chip, drawn around the package.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct PinLayout {
    #[serde(default)]
    pub top: Vec<PinDef>,
    #[serde(default)]
    pub bottom: Vec<PinDef>,
    #[serde(default)]
    pub left: Vec<PinDef>,
    #[serde(default)]
    pub right: Vec<PinDef>,
}

/// Project-generation parameters: target triple, HAL dependency line, memory
/// layout and probe/flash chip — everything `project_gen` needs to emit a
/// buildable Cargo project. Consumed alongside the chip's `ToolchainKind`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectDef {
    pub pkg_name: String,
    pub target: String,
    #[serde(default)]
    pub flash_origin: String,
    #[serde(default)]
    pub flash_size: String,
    #[serde(default)]
    pub ram_origin: String,
    #[serde(default)]
    pub ram_size: String,
    pub hal_dep: String,
    pub probe_chip: String,
    #[serde(default)]
    pub memory_comment: String,
}

/// Clock model + defaults for this chip, keyed per clock family.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClockDef {
    Stm32f1(Stm32f1Clock),
    /// No modelled clock tree yet (ESP32-C3, STM8, …).
    None,
}

impl ClockDef {
    fn to_config(&self) -> ClockConfig {
        match self {
            ClockDef::Stm32f1(c) => ClockConfig::Stm32f1(c.clone()),
            ClockDef::None => ClockConfig::None,
        }
    }
}

/// A complete, importable MCU definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McuDefinition {
    /// Stable identifier, e.g. "stm32f103c8t6".
    pub id: String,
    /// Display name shown in the chip selector.
    pub display_name: String,
    /// Family / backend key (e.g. "stm32f1", "esp32c3") — selects the codegen
    /// + clock backend in later phases.
    pub family: String,
    #[serde(default)]
    pub package: String,
    #[serde(default)]
    pub cpu: String,
    pub toolchain: ToolchainKind,
    pub project: ProjectDef,
    #[serde(default)]
    pub pins: PinLayout,
    pub clock: ClockDef,
}

impl McuDefinition {
    /// Build the runtime [`Mcu`] (pin diagram + clock) from this definition.
    pub fn build_mcu(&self) -> Mcu {
        let map = |defs: &[PinDef]| defs.iter().map(PinDef::to_pin).collect::<Vec<_>>();
        let mut mcu = Mcu::new(
            self.display_name.clone(),
            self.family.clone(),
            self.toolchain.clone(),
            map(&self.pins.top),
            map(&self.pins.bottom),
            map(&self.pins.left),
            map(&self.pins.right),
        );
        mcu.clock = self.clock.to_config();
        mcu
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::builtins::builtin_for;
    use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;

    fn stm_def() -> McuDefinition {
        builtin_for("stm32f103c8t6").expect("built-in stm32f103c8t6 definition")
    }

    #[test]
    fn ron_round_trips_definition() {
        let def = stm_def();
        let ron = ron::to_string(&def).expect("serialize to RON");
        let parsed: McuDefinition = ron::from_str(&ron).expect("parse RON");
        assert_eq!(def, parsed, "RON round-trip must be lossless");
    }

    #[test]
    fn pindef_round_trips_a_pin() {
        // A real pin carrying multiple peripheral functions.
        let pin = create_stm32f103c8tx()
            .top_pins
            .into_iter()
            .find(|p| !p.reserved && !p.available_functions.is_empty())
            .unwrap();
        let back = PinDef::from_pin(&pin).to_pin();
        assert_eq!(back.number, pin.number);
        assert_eq!(back.name, pin.name);
        assert_eq!(back.reserved, pin.reserved);
        assert_eq!(back.available_functions, pin.available_functions);
    }

    #[test]
    fn build_mcu_matches_factory() {
        let factory = create_stm32f103c8tx();
        let built = stm_def().build_mcu();

        let same = |a: &[Pin], b: &[Pin]| {
            a.len() == b.len()
                && a.iter().zip(b).all(|(x, y)| {
                    x.number == y.number
                        && x.name == y.name
                        && x.reserved == y.reserved
                        && x.available_functions == y.available_functions
                })
        };
        assert!(same(&built.top_pins, &factory.top_pins), "top pins differ");
        assert!(same(&built.bottom_pins, &factory.bottom_pins), "bottom pins differ");
        assert!(same(&built.left_pins, &factory.left_pins), "left pins differ");
        assert!(same(&built.right_pins, &factory.right_pins), "right pins differ");
        assert_eq!(built.clock, factory.clock, "clock differs");
    }
}
