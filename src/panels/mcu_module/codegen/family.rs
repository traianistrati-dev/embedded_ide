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
use super::{embassy_async, embassy_common, rcc, rtic, stm32, wba};
use crate::panels::mcu_module::codegen_esp::{self, EspRuntime};
use crate::panels::mcu_module::mcu::{Mcu, Runtime};
use crate::panels::mcu_module::modules::{
    self, ApiStyle, I2cModuleConfig, SpiModuleConfig, UsartModuleConfig,
};
use crate::panels::mcu_module::pins::logic::pin::{GpioMode, Pin};
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
use std::collections::BTreeMap;

/// The USART/SPI/I2C configs for `mcu`'s blocking codegen, with `api_style`
/// forced to Native when the project Runtime is Native (an "all concrete HAL"
/// project). On Blocking each module keeps its own Portable/Native choice.
fn resolve_bus_configs(
    mcu: &Mcu,
) -> (
    BTreeMap<u8, UsartModuleConfig>,
    BTreeMap<u8, SpiModuleConfig>,
    BTreeMap<u8, I2cModuleConfig>,
) {
    let mut usart = modules::usart_configs(&mcu.modules);
    let mut spi = modules::spi_configs(&mcu.modules);
    let mut i2c = modules::i2c_configs(&mcu.modules);
    if mcu.is_native() {
        for c in usart.values_mut() {
            c.api_style = ApiStyle::Native;
        }
        for c in spi.values_mut() {
            c.api_style = ApiStyle::Native;
        }
        for c in i2c.values_mut() {
            c.api_style = ApiStyle::Native;
        }
    }
    (usart, spi, i2c)
}

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

    /// The drive / pull modes this backend can generate for `func` — what the
    /// mode list under the selected function offers. The FIRST entry is the
    /// default, i.e. what a pin with `io_mode: None` generates.
    ///
    /// The default implementation is the full set, which the `into_*` HALs
    /// (stm32f1xx-hal & friends) all provide. A backend whose HAL spells one of
    /// them differently — embassy's open-drain output is a different TYPE, not
    /// an argument — overrides this to offer only what it actually emits, so the
    /// list can never promise code the generator can't write.
    fn gpio_modes(&self, func: &PinFunction) -> &'static [GpioMode] {
        match func {
            PinFunction::GpioInput => &[GpioMode::Floating, GpioMode::PullUp, GpioMode::PullDown],
            PinFunction::GpioOutput => &[GpioMode::PushPull, GpioMode::OpenDrain],
            _ => &[],
        }
    }
}

/// The modes offered for `pin` on `mcu`'s current backend — empty when the pin
/// is not a GPIO In/Out, or the family has no backend. The single entry point
/// the UI uses, so it can't drift from what codegen supports.
pub fn gpio_modes_for(mcu: &Mcu, func: &PinFunction) -> &'static [GpioMode] {
    backend_for_runtime(&mcu.family, mcu.runtime).map_or(&[], |b| b.gpio_modes(func))
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
        let (usart, spi, i2c) = resolve_bus_configs(mcu);
        let can = modules::can_configs(&mcu.modules);
        let usb = modules::usb_configs(&mcu.modules);
        let gen_ = stm32::make_generated_section(
            &mcu.name,
            &all,
            &mcu.clock,
            &usart,
            &spi,
            &i2c,
            &can,
            &usb,
            mcu.gpio_native(),
            &mcu.custom_module_inits(),
        );
        let base = format!(
            "{header}{gen_}\n{tail}",
            header = stm32::invariant_header(&mcu.name, &mcu.id),
            tail = USER_TAIL,
        );
        // Nothing lives after `fn main` any more: USART/SPI/I2C init are in
        // `src/pins/configs/` and the ADC is one line inside the GEN block.
        stm32::strip_obsolete_helpers(base)
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let all = pins_of(mcu);
        let (usart, spi, i2c) = resolve_bus_configs(mcu);
        let can = modules::can_configs(&mcu.modules);
        let usb = modules::usb_configs(&mcu.modules);
        let new_section = stm32::make_generated_section(
            &mcu.name,
            &all,
            &mcu.clock,
            &usart,
            &spi,
            &i2c,
            &can,
            &usb,
            mcu.gpio_native(),
            &mcu.custom_module_inits(),
        );
        let spliced = stm32::splice_section(existing, &new_section, &mcu.name, &mcu.id);
        // Clean up the init helpers older versions appended after `fn main`.
        stm32::strip_obsolete_helpers(spliced)
    }

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        let all = pins_of(mcu);
        let (usart, spi, i2c) = resolve_bus_configs(mcu);
        let can = modules::can_configs(&mcu.modules);
        stm32::config_files(
            &all,
            &usart,
            &spi,
            &i2c,
            &can,
            &mcu.clock,
            mcu.gpio_native(),
        )
    }
}

// ── ESP32-C3 (esp-hal) ──────────────────────────────────────────────────────
struct Esp32Backend;

impl FamilyBackend for Esp32Backend {
    fn family_id(&self) -> &'static str {
        "esp32c3"
    }

    /// esp-hal configures pulls through `InputConfig`/`OutputConfig` builders,
    /// which this backend does not emit yet — so it offers no choice rather than
    /// listing modes it would silently ignore.
    fn gpio_modes(&self, _func: &PinFunction) -> &'static [GpioMode] {
        &[]
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        esp_fresh_main_rs(mcu, EspRuntime::Blocking)
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        esp_update_main_rs(mcu, existing, EspRuntime::Blocking)
    }
}

/// A fresh ESP `main.rs` on `runtime` — shared by the blocking and async ESP
/// backends, which differ only in that argument.
fn esp_fresh_main_rs(mcu: &Mcu, runtime: EspRuntime) -> String {
    let usart = modules::usart_configs(&mcu.modules);
    let spi = modules::spi_configs(&mcu.modules);
    let i2c = modules::i2c_configs(&mcu.modules);
    codegen_esp::fresh_esp32c3_main_rs(
        &pins_of(mcu),
        &mcu.clock,
        &mcu.id,
        &usart,
        &spi,
        &i2c,
        &mcu.custom_module_inits(),
        runtime,
    )
}

/// Re-splice an existing ESP `main.rs` on `runtime` (see [`esp_fresh_main_rs`]).
fn esp_update_main_rs(mcu: &Mcu, existing: &str, runtime: EspRuntime) -> String {
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
        &mcu.custom_module_inits(),
        runtime,
    )
}

// ── Async ESP32-C3 (esp-rtos + embassy-executor) ────────────────────────────
// The [`Runtime::Async`] counterpart of [`Esp32Backend`]: the SAME esp-hal
// bindings, entered through `#[esp_rtos::main] async fn main(Spawner)` with the
// esp-rtos scheduler driving the executor. Like [`AsyncEmbassyBackend`] it is
// selected by [`backend_for_runtime`], never listed in [`BACKENDS`].
//
// `esp-rtos`, not `esp-hal-embassy`: the latter needs esp-hal's private
// `__esp_hal_embassy` feature, which esp-hal 1.1 (the version the project
// template pins) dropped — cargo fails to resolve before compiling anything.
struct AsyncEspBackend;

impl FamilyBackend for AsyncEspBackend {
    fn family_id(&self) -> &'static str {
        "esp32c3-async" // label only — dispatch is via `backend_for_runtime`
    }

    /// Same reason as [`Esp32Backend`]: the `InputConfig`/`OutputConfig`
    /// builders that carry the pulls are not emitted yet.
    fn gpio_modes(&self, _func: &PinFunction) -> &'static [GpioMode] {
        &[]
    }

    fn handles(&self, family: &str) -> bool {
        family == "esp32c3"
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        esp_fresh_main_rs(mcu, EspRuntime::Async)
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        esp_update_main_rs(mcu, existing, EspRuntime::Async)
    }
}

/// Chosen by runtime rather than family, so — like [`ASYNC_EMBASSY_BACKEND`] —
/// it lives outside [`BACKENDS`].
static ASYNC_ESP_BACKEND: AsyncEspBackend = AsyncEspBackend;

// ── STM32WBA (embassy-stm32, blocking) ──────────────────────────────────────
struct WbaBackend;

impl FamilyBackend for WbaBackend {
    fn family_id(&self) -> &'static str {
        "stm32wba"
    }

    /// embassy takes the pull as an argument (`Input::new(p.PB5, Pull::Up)`) but
    /// spells an open-drain output as a different TYPE (`OutputOpenDrain`), which
    /// this backend does not emit — so outputs offer push-pull only.
    fn gpio_modes(&self, func: &PinFunction) -> &'static [GpioMode] {
        match func {
            PinFunction::GpioInput => &[GpioMode::Floating, GpioMode::PullUp, GpioMode::PullDown],
            PinFunction::GpioOutput => &[GpioMode::PushPull],
            _ => &[],
        }
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        let all = pins_of(mcu);
        format!(
            "{header}{section}\n{tail}",
            header = wba::invariant_header(&mcu.name, &mcu.id),
            section = wba::make_generated_section(
                &mcu.name,
                &all,
                &mcu.clock,
                &mcu.custom_module_inits(),
            ),
            tail = USER_TAIL,
        )
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let all = pins_of(mcu);
        let section =
            wba::make_generated_section(&mcu.name, &all, &mcu.clock, &mcu.custom_module_inits());
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

    /// embassy takes the pull as an argument (`Input::new(p.PB5, Pull::Up)`) but
    /// spells an open-drain output as a different TYPE (`OutputOpenDrain`), which
    /// this backend does not emit — so outputs offer push-pull only.
    fn gpio_modes(&self, func: &PinFunction) -> &'static [GpioMode] {
        match func {
            PinFunction::GpioInput => &[GpioMode::Floating, GpioMode::PullUp, GpioMode::PullDown],
            PinFunction::GpioOutput => &[GpioMode::PushPull],
            _ => &[],
        }
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
                &mcu.custom_module_inits(),
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
            &mcu.custom_module_inits(),
        );
        embassy_common::splice_section(existing, &section, &mcu.name, &mcu.id)
    }
}

// ── Async STM32 (embassy-stm32 + embassy-executor) ──────────────────────────
// The [`Runtime::Async`] counterpart of [`StmEmbassyBackend`]: the SAME
// embassy-stm32 GPIO codegen, but the entry point is `#[embassy_executor::main]
// async fn main(Spawner)` driven by the executor instead of `#[entry] fn main()
// -> !`. Selected by [`backend_for_runtime`] — never listed in [`BACKENDS`],
// since it is chosen by runtime, not family. Handles every STM32 family that
// runs on embassy-stm32; `stm32f1` (on stm32f1xx-hal) has no async path yet, so
// it is excluded and the System-tab toggle is disabled for it.
struct AsyncEmbassyBackend;

impl FamilyBackend for AsyncEmbassyBackend {
    fn family_id(&self) -> &'static str {
        "stm32-async" // label only — dispatch is via `backend_for_runtime`
    }

    /// embassy takes the pull as an argument (`Input::new(p.PB5, Pull::Up)`) but
    /// spells an open-drain output as a different TYPE (`OutputOpenDrain`), which
    /// this backend does not emit — so outputs offer push-pull only.
    fn gpio_modes(&self, func: &PinFunction) -> &'static [GpioMode] {
        match func {
            PinFunction::GpioInput => &[GpioMode::Floating, GpioMode::PullUp, GpioMode::PullDown],
            PinFunction::GpioOutput => &[GpioMode::PushPull],
            _ => &[],
        }
    }

    fn handles(&self, family: &str) -> bool {
        family.starts_with("stm32") && family != "stm32f1"
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        format!(
            "{header}{section}\n{tail}",
            header = embassy_async::invariant_header(&mcu.name, &mcu.id),
            section = async_section(mcu),
            tail = USER_TAIL,
        )
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let section = async_section(mcu);
        embassy_async::splice_section(existing, &section, &mcu.name, &mcu.id)
    }

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        // Async bus peripherals: USART (BufferedUart → embedded-io-async), SPI +
        // I2C (blocking `embedded-hal` 1.0 or async-DMA `embedded-hal-async`, per
        // the module's AsyncBusMode). GPIO stays inline in main.rs (no io.rs).
        async_periphs(mcu).config_files
    }
}

/// The async peripherals (USART/SPI/I2C) derived from `mcu`'s modules + pins.
fn async_periphs(mcu: &Mcu) -> embassy_async::AsyncPeriphs {
    let all = pins_of(mcu);
    let usart = modules::usart_configs(&mcu.modules);
    let spi = modules::spi_configs(&mcu.modules);
    let i2c = modules::i2c_configs(&mcu.modules);
    embassy_async::async_peripherals(&all, &usart, &spi, &i2c)
}

/// The async generated section for `mcu`: the GPIO/raw pin bindings (minus the
/// pins a bus driver consumes) followed by the peripheral `init(...)` calls.
fn async_section(mcu: &Mcu) -> String {
    let all = pins_of(mcu);
    let periphs = async_periphs(mcu);
    // Pins a bus driver moves into its peripheral must NOT also be bound raw.
    let gpio_pins: Vec<&Pin> = all
        .iter()
        .copied()
        .filter(|p| !periphs.consumed_pins.contains(&p.name))
        .collect();
    embassy_async::make_generated_section(
        &mcu.name,
        &gpio_pins,
        &rcc::graph_clock_block(&mcu.family, &mcu.clock),
        &periphs.init_calls,
        &mcu.custom_module_inits(),
    )
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

/// The async backend is chosen by runtime (not family), so it lives outside
/// [`BACKENDS`]; a `static` gives the `&'static` the dispatch returns.
static ASYNC_EMBASSY_BACKEND: AsyncEmbassyBackend = AsyncEmbassyBackend;

// ── RTIC (STM32F1) ──────────────────────────────────────────────────────────
/// Same chip, same HAL, same init as the blocking backend — only the program
/// shape differs (see [`super::rtic`]). It therefore reuses `Stm32f1Backend`'s
/// `config_files`: the `pins/configs/*.rs` are called from `#[init]` unchanged.
struct RticBackend;

impl FamilyBackend for RticBackend {
    fn family_id(&self) -> &'static str {
        "stm32f1"
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        let all = pins_of(mcu);
        let (usart, spi, i2c) = resolve_bus_configs(mcu);
        let can = modules::can_configs(&mcu.modules);
        let usb = modules::usb_configs(&mcu.modules);
        let section = rtic::make_generated_section(
            &mcu.name,
            &all,
            &mcu.clock,
            &usart,
            &spi,
            &i2c,
            &can,
            &usb,
            mcu.gpio_native(),
            &mcu.custom_module_inits(),
        );
        format!(
            "{}{section}
{}",
            rtic::invariant_header(&mcu.name, &mcu.id),
            rtic::RTIC_USER_TAIL
        )
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let all = pins_of(mcu);
        let (usart, spi, i2c) = resolve_bus_configs(mcu);
        let can = modules::can_configs(&mcu.modules);
        let usb = modules::usb_configs(&mcu.modules);
        let section = rtic::make_generated_section(
            &mcu.name,
            &all,
            &mcu.clock,
            &usart,
            &spi,
            &i2c,
            &can,
            &usb,
            mcu.gpio_native(),
            &mcu.custom_module_inits(),
        );
        rtic::splice_rtic_section(existing, &section, &mcu.name, &mcu.id)
    }

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        Stm32f1Backend.config_files(mcu)
    }
}

static RTIC_BACKEND: RticBackend = RticBackend;

/// Whether an Async runtime has a backend for `family` — an embassy-capable
/// STM32 family (embassy-stm32) or the ESP32-C3 (esp-rtos). Drives both codegen
/// dispatch and the System-tab toggle's enabled state.
pub fn async_supported(family: &str) -> bool {
    ASYNC_EMBASSY_BACKEND.handles(family) || ASYNC_ESP_BACKEND.handles(family)
}

/// Whether `family`'s async path is the ESP one (esp-rtos + embassy-executor)
/// rather than embassy-stm32. The two need DIFFERENT `[dependencies]` — same
/// crate names, different versions and features — so the Cargo.toml sync has to
/// tell them apart (see `project_gen::AsyncFlavor`).
pub fn async_is_esp(family: &str) -> bool {
    ASYNC_ESP_BACKEND.handles(family)
}

/// Whether an RTIC project can be generated for `family`.
///
/// Narrow on purpose. RTIC 2 only has cortex-m backends, which rules out the
/// RISC-V ESP parts outright; and the generated interrupt tasks are written
/// against `stm32f1xx-hal`'s `ExtiPin` trait (`make_interrupt_source`,
/// `trigger_on_edge`, `clear_interrupt_pending_bit`), which the embassy-stm32
/// families do not expose. Widening this means writing those task bodies for
/// another HAL, not flipping a flag.
pub fn rtic_supported(family: &str) -> bool {
    family == "stm32f1"
}

/// Whether a Native runtime is meaningful for `family` — i.e. the backend has
/// concrete-HAL (Portable/Native) peripheral templates. Only `stm32f1`
/// (stm32f1xx-hal) does; other STM32 families run on embassy-stm32 whose blocking
/// bindings are already concrete (no separate Native form), and ESP has its own
/// scheme. Drives the System-tab card's enabled state + the codegen forcing.
pub fn native_supported(family: &str) -> bool {
    family == "stm32f1"
}

/// Look up the backend for a family key, if one is registered.
///
/// Families without a backend yet (e.g. "stm8") return `None`; callers fall
/// back to "no code generation" so an unconfigured chip stays safe.
pub fn backend_for(family: &str) -> Option<&'static dyn FamilyBackend> {
    BACKENDS.iter().copied().find(|b| b.handles(family))
}

/// Look up the backend honouring the project [`Runtime`]. Async re-targets every
/// embassy-capable STM32 family to the async embassy backend and the ESP32-C3 to
/// the esp-rtos one; everything else — and all of Blocking — falls through to
/// [`backend_for`].
///
/// [`Runtime`]: crate::panels::mcu_module::mcu::Runtime
pub fn backend_for_runtime(family: &str, runtime: Runtime) -> Option<&'static dyn FamilyBackend> {
    if runtime == Runtime::Async && ASYNC_ESP_BACKEND.handles(family) {
        return Some(&ASYNC_ESP_BACKEND);
    }
    if runtime == Runtime::Async && async_supported(family) {
        return Some(&ASYNC_EMBASSY_BACKEND);
    }
    if runtime == Runtime::Rtic && rtic_supported(family) {
        return Some(&RTIC_BACKEND);
    }
    backend_for(family)
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
        for fam in [
            "stm32f4", "stm32g0", "stm32g4", "stm32l4", "stm32h7", "stm32c0",
        ] {
            let b = backend_for(fam).unwrap_or_else(|| panic!("no backend for {fam}"));
            assert_eq!(
                b.family_id(),
                "stm32",
                "{fam} should hit the generic backend"
            );
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

    /// The Async runtime re-targets embassy-capable STM32 families to the async
    /// backend, while Blocking (and F1/ESP under Async) keep their default one.
    #[test]
    fn async_runtime_dispatch() {
        // Async supported for embassy STM32 families, not F1/ESP/unknown.
        assert!(async_supported("stm32f4"));
        assert!(async_supported("stm32g0"));
        assert!(async_supported("stm32wba"));
        assert!(!async_supported("stm32f1"));
        assert!(async_supported("esp32c3")); // esp-rtos
        assert!(!async_is_esp("stm32f4"));
        assert!(async_is_esp("esp32c3"));
        assert!(!async_supported("rp2040"));

        // Blocking always uses the family default.
        assert_eq!(
            backend_for_runtime("stm32f4", Runtime::Blocking)
                .unwrap()
                .family_id(),
            "stm32"
        );
        // Async on an embassy family → the async backend.
        assert_eq!(
            backend_for_runtime("stm32f4", Runtime::Async)
                .unwrap()
                .family_id(),
            "stm32-async"
        );
        // Async on F1 is inert — it stays on the F1 (stm32f1xx-hal) backend.
        assert_eq!(
            backend_for_runtime("stm32f1", Runtime::Async)
                .unwrap()
                .family_id(),
            "stm32f1"
        );
        // ESP32-C3 async is a DIFFERENT backend (esp-rtos), not the embassy one.
        assert_eq!(
            backend_for_runtime("esp32c3", Runtime::Async)
                .unwrap()
                .family_id(),
            "esp32c3-async"
        );
        assert_eq!(
            backend_for_runtime("esp32c3", Runtime::Blocking)
                .unwrap()
                .family_id(),
            "esp32c3"
        );
        // Native / RTIC are still not offered on ESP — they fall back to blocking.
        for rt in [Runtime::Native, Runtime::Rtic] {
            assert_eq!(
                backend_for_runtime("esp32c3", rt).unwrap().family_id(),
                "esp32c3"
            );
        }
    }

    /// Switching the ESP runtime re-splices ONLY the generated block, both ways,
    /// leaving the user's loop body untouched — the whole entry-point difference
    /// lives inside the markers, so the invariant header never moves.
    #[test]
    fn esp_runtime_switch_reslices_and_keeps_user_tail() {
        use crate::panels::mcu_module::mcu::Runtime;
        const MARK: &str = "        my_own_call(); // USER";
        let mut mcu = crate::panels::mcu_module::mock_esp32c3::create_esp32c3();

        let blocking = mcu.fresh_main_rs().replace(
            "        // Your main loop code here.",
            &format!("        // Your main loop code here.\n{MARK}"),
        );
        assert!(blocking.contains(MARK), "test fixture wrote the user line");

        mcu.runtime = Runtime::Async;
        let to_async = mcu.update_main_rs(&blocking);
        assert!(to_async.contains("#[esp_rtos::main]"), "{to_async}");
        assert!(!to_async.contains("#[esp_hal::main]"), "{to_async}");
        assert!(to_async.contains(MARK), "user tail survives:\n{to_async}");

        mcu.runtime = Runtime::Blocking;
        let back = mcu.update_main_rs(&to_async);
        assert!(back.contains("#[esp_hal::main]"), "{back}");
        assert!(!back.contains("esp_rtos"), "scheduler gone:\n{back}");
        assert!(!back.contains("Spawner"), "async import gone:\n{back}");
        assert!(
            back.contains(MARK),
            "user tail survives the way back:\n{back}"
        );
        // Round-trip is stable — a switch there and back is not a rewrite.
        assert_eq!(back, blocking, "blocking -> async -> blocking is identity");
    }

    /// Writes the EXACT files an Async ESP32-C3 project generates (main.rs,
    /// Cargo.toml with the async deps, .cargo/config.toml) into
    /// `%ESP_ASYNC_OUT%`, so the generated code can be compiled for real against
    /// the riscv target — what "verified" means for a codegen change here:
    ///
    /// ```text
    /// ESP_ASYNC_OUT=/some/dir cargo test write_esp_async_project -- --ignored
    /// cd /some/dir && cargo check --target riscv32imc-unknown-none-elf
    /// ```
    #[test]
    #[ignore]
    fn write_esp_async_project() {
        use crate::panels::mcu_module::mcu::Runtime;
        use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
        use crate::panels::mcu_module::project_gen::{self, AsyncFlavor, ConfigFile};
        use std::fs;
        use std::path::PathBuf;

        let out = PathBuf::from(
            std::env::var("ESP_ASYNC_OUT").expect("set ESP_ASYNC_OUT to the target folder"),
        );
        let def = crate::panels::mcu_module::builtins::builtin_for("esp32c3").expect("esp32c3 def");

        // GPIO in + out and one of every bus — the whole surface the ESP backend
        // emits, so a compile of this project covers the generator, not just the
        // entry point.
        let mut mcu = crate::panels::mcu_module::mock_esp32c3::create_esp32c3();
        // `ESP_ASYNC_RUNTIME=blocking` writes the blocking project instead — the
        // A/B that tells a codegen error apart from one the async path added.
        let async_rt = std::env::var("ESP_ASYNC_RUNTIME").as_deref() != Ok("blocking");
        mcu.runtime = if async_rt {
            Runtime::Async
        } else {
            Runtime::Blocking
        };
        for (name, func) in [
            ("GPIO2", PinFunction::GpioOutput),
            ("GPIO3", PinFunction::GpioInput),
            ("GPIO21", PinFunction::UsartTx(0)),
            ("GPIO20", PinFunction::UsartRx(0)),
            ("GPIO4", PinFunction::SpiSck(2)),
            ("GPIO5", PinFunction::SpiMosi(2)),
            ("GPIO6", PinFunction::SpiMiso(2)),
            ("GPIO7", PinFunction::SpiNss(2)),
            ("GPIO8", PinFunction::I2cScl(0)),
            ("GPIO9", PinFunction::I2cSda(0)),
        ] {
            let pin = mcu
                .iter_all_pins_mut()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{name} exists"));
            pin.selected_function = func;
        }

        let cargo_toml =
            project_gen::gen_config(ConfigFile::CargoToml, &def.project, &def.toolchain);
        let cargo_toml = project_gen::ensure_async_deps(
            &cargo_toml,
            async_rt,
            AsyncFlavor::Esp(&def.project.probe_chip),
            false,
            false,
            false,
            &[],
        );
        fs::create_dir_all(out.join("src/pins")).unwrap();
        fs::create_dir_all(out.join(".cargo")).unwrap();
        fs::write(out.join("Cargo.toml"), cargo_toml).unwrap();
        fs::write(
            out.join(".cargo/config.toml"),
            project_gen::gen_config(ConfigFile::CargoConfig, &def.project, &def.toolchain),
        )
        .unwrap();
        fs::write(out.join("src/main.rs"), mcu.fresh_main_rs()).unwrap();
        fs::write(out.join("src/pins/mod.rs"), "").unwrap(); // header declares it
    }

    /// The ESP async backend swaps the entry point for `#[esp_rtos::main]` and
    /// starts the scheduler, while the pin bindings stay the same esp-hal calls.
    #[test]
    fn esp_async_backend_emits_esp_rtos_main() {
        use crate::panels::mcu_module::mcu::Runtime;
        let mut mcu = crate::panels::mcu_module::mock_esp32c3::create_esp32c3();

        // Blocking — the esp-hal entry, no scheduler.
        let blocking = mcu.fresh_main_rs();
        assert!(blocking.contains("#[esp_hal::main]"), "{blocking}");
        assert!(!blocking.contains("esp_rtos"), "{blocking}");

        mcu.runtime = Runtime::Async;
        let code = mcu.fresh_main_rs();
        assert!(code.contains("#[esp_rtos::main]"), "{code}");
        assert!(code.contains("async fn main(_spawner: Spawner)"), "{code}");
        assert!(code.contains("use embassy_executor::Spawner;"), "{code}");
        assert!(
            code.contains("esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);"),
            "{code}"
        );
        // The scheduler must start BEFORE anything that could await.
        let init = code.find("esp_hal::init").expect("init");
        let start = code.find("esp_rtos::start").expect("start");
        assert!(init < start, "start comes after esp_hal::init:\n{code}");
        // ...and the whole runtime difference stays inside the generated block,
        // so a runtime switch never rewrites the user's tail.
        let end = code.find(super::super::GEN_END).expect("end");
        assert!(start < end, "scheduler start inside GEN:\n{code}");
    }

    /// The async backend emits an `#[embassy_executor::main] async fn main`
    /// skeleton (via `Mcu::fresh_main_rs` honouring `runtime`), distinct from the
    /// blocking `#[entry] fn main() -> !`.
    #[test]
    fn async_backend_emits_embassy_executor_main() {
        use crate::panels::mcu_module::mcu::Runtime;
        use crate::panels::mcu_module::mcu_def::McuDefinition;
        let mut def: McuDefinition =
            crate::panels::mcu_module::builtins::builtin_for("stm32f103c8t6").unwrap();
        // Retarget to an embassy family (F4) so async applies.
        def.family = "stm32f4".into();
        def.id = "stm32f411re".into();
        let mut mcu = def.build_mcu();

        // Blocking first — the classic entry.
        assert!(mcu.fresh_main_rs().contains("fn main() -> !"));

        // Flip to async — the executor entry + async header.
        mcu.runtime = Runtime::Async;
        let code = mcu.fresh_main_rs();
        assert!(code.contains("HAL: embassy-stm32 (async)"));
        assert!(code.contains("#[embassy_executor::main]"));
        assert!(code.contains("async fn main(_spawner: Spawner)"));
        assert!(code.contains("use embassy_executor::Spawner;"));
        // Crate-level allow (the executor macro drops a fn-level one — see
        // `embassy_async::invariant_header`).
        assert!(code.contains("#![allow(unused_variables, unused_mut)]"));
        assert!(
            !code.contains("fn main() -> !"),
            "no blocking entry in async"
        );
        assert!(
            !code.contains("use cortex_m_rt::entry;"),
            "no cortex_m_rt entry import"
        );
        assert!(code.contains(crate::panels::mcu_module::codegen::GEN_BEGIN));
    }

    /// Toggling runtime on an EXISTING file re-splices the header + entry both
    /// ways while preserving the user's loop body after the markers.
    #[test]
    fn runtime_switch_reslices_header_both_ways_and_keeps_user_tail() {
        use crate::panels::mcu_module::mcu::Runtime;
        use crate::panels::mcu_module::mcu_def::McuDefinition;
        let mut def: McuDefinition =
            crate::panels::mcu_module::builtins::builtin_for("stm32f103c8t6").unwrap();
        def.family = "stm32g0".into();
        def.id = "stm32g0b1re".into();
        let mut mcu = def.build_mcu();

        // A blocking file with a hand-edited loop body.
        let blocking = mcu.fresh_main_rs();
        let user = blocking.replace(
            "// Your main loop code here.",
            "defmt::info!(\"tick\"); // USER EDIT",
        );
        assert!(user.contains("fn main() -> !"));

        // → Async: async header/entry replace the blocking ones; user tail kept.
        mcu.runtime = Runtime::Async;
        let now_async = mcu.update_main_rs(&user);
        assert!(now_async.contains("#[embassy_executor::main]"));
        assert!(now_async.contains("HAL: embassy-stm32 (async)"));
        assert!(!now_async.contains("fn main() -> !"), "blocking entry gone");
        assert!(
            !now_async.contains("use cortex_m_rt::entry;"),
            "blocking import gone"
        );
        assert!(now_async.contains("USER EDIT"), "user tail preserved");
        assert_eq!(
            now_async
                .matches(crate::panels::mcu_module::codegen::GEN_BEGIN)
                .count(),
            1
        );

        // → back to Blocking: blocking header/entry restored, still one GEN block.
        mcu.runtime = Runtime::Blocking;
        let back = mcu.update_main_rs(&now_async);
        assert!(back.contains("fn main() -> !"));
        assert!(back.contains("HAL: embassy-stm32 (blocking)"));
        assert!(
            !back.contains("#[embassy_executor::main]"),
            "async entry gone"
        );
        assert!(
            !back.contains("use embassy_executor::Spawner;"),
            "async import gone"
        );
        assert!(back.contains("USER EDIT"), "user tail still preserved");
        assert_eq!(
            back.matches(crate::panels::mcu_module::codegen::GEN_BEGIN)
                .count(),
            1
        );
    }

    /// An async USART (both TX+RX wired) emits a `usart{n}.rs` config file with
    /// the embassy `BufferedUart` → embedded-io-async bridge, a call into it from
    /// `main.rs`, and does NOT also bind the TX/RX pins raw (they're moved into
    /// the driver).
    #[test]
    fn async_usart_emits_config_file_and_init_call() {
        use crate::panels::mcu_module::mcu::Runtime;
        use crate::panels::mcu_module::mcu_def::McuDefinition;
        use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

        let mut def: McuDefinition =
            crate::panels::mcu_module::builtins::builtin_for("stm32f103c8t6").unwrap();
        def.family = "stm32f4".into();
        def.id = "stm32f411re".into();
        let mut mcu = def.build_mcu();
        mcu.runtime = Runtime::Async;

        // Wire USART1 TX + RX on whichever pins offer them.
        let tx = mcu
            .iter_all_pins()
            .find(|p| p.available_functions.contains(&PinFunction::UsartTx(1)))
            .map(|p| p.number)
            .expect("a UsartTx(1) pin");
        let rx = mcu
            .iter_all_pins()
            .find(|p| p.available_functions.contains(&PinFunction::UsartRx(1)))
            .map(|p| p.number)
            .expect("a UsartRx(1) pin");
        mcu.apply_pin_function(tx, PinFunction::UsartTx(1));
        mcu.apply_pin_function(rx, PinFunction::UsartRx(1));

        // main.rs: calls into the config module, no raw binding for the USART pins.
        let code = mcu.fresh_main_rs();
        assert!(
            code.contains("pins::configs::usart1::init(p.USART1,"),
            "USART init call missing:\n{code}"
        );
        assert!(
            !code.contains("usart1_tx = p."),
            "USART TX pin must not be bound raw:\n{code}"
        );

        // The config file is the async BufferedUart bridge.
        let cfgs = mcu.config_files();
        let usart1 = &cfgs
            .iter()
            .find(|(n, _)| n == "usart1.rs")
            .expect("usart1.rs config file")
            .1;
        assert!(usart1.contains("BufferedUart"));
        assert!(usart1.contains("embedded_io_async::Read"));
        assert!(usart1.contains("bind_interrupts!"));
        assert_eq!(usart1.matches("pub fn init").count(), 1);
    }

    /// Async SPI/I2C: a blocking-mode module emits `new_blocking` + embedded-hal
    /// 1.0 with a no-DMA init call; an async-DMA module emits `Spi::new` +
    /// embedded-hal-async with the DMA TODO placeholder in `main.rs`.
    #[test]
    fn async_spi_i2c_blocking_and_dma_codegen() {
        use crate::panels::mcu_module::mcu::Runtime;
        use crate::panels::mcu_module::mcu_def::McuDefinition;
        use crate::panels::mcu_module::modules::{
            AsyncBusMode, ModuleConfig, ModuleKind, SpiModuleConfig, VirtualModule,
        };
        use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

        let mut def: McuDefinition =
            crate::panels::mcu_module::builtins::builtin_for("stm32f103c8t6").unwrap();
        def.family = "stm32f4".into();
        def.id = "stm32f411re".into();
        let mut mcu = def.build_mcu();
        mcu.runtime = Runtime::Async;

        // Wire SPI1 (SCK/MOSI/MISO) + I2C1 (SCL/SDA) on whatever pins offer them.
        for f in [
            PinFunction::SpiSck(1),
            PinFunction::SpiMosi(1),
            PinFunction::SpiMiso(1),
            PinFunction::I2cScl(1),
            PinFunction::I2cSda(1),
        ] {
            let num = mcu
                .iter_all_pins()
                .find(|p| p.available_functions.contains(&f))
                .map(|p| p.number)
                .unwrap_or_else(|| panic!("no pin for {f:?}"));
            mcu.apply_pin_function(num, f);
        }

        // Default (no module) → blocking SPI/I2C: new_blocking, embedded-hal 1.0,
        // and init calls WITHOUT the DMA TODO.
        let code = mcu.fresh_main_rs();
        assert!(
            code.contains("pins::configs::spi1::init(p.SPI1,"),
            "SPI call:\n{code}"
        );
        assert!(
            code.contains("pins::configs::i2c1::init(p.I2C1,"),
            "I2C call"
        );
        assert!(
            !code.contains("DMA_TX_TODO"),
            "blocking calls carry no DMA TODO"
        );
        let cfgs = mcu.config_files();
        let spi1 = &cfgs
            .iter()
            .find(|(n, _)| n == "spi1.rs")
            .expect("spi1.rs")
            .1;
        assert!(spi1.contains("Spi::new_blocking"));
        assert!(spi1.contains("embedded_hal::spi::SpiBus"));
        let i2c1 = &cfgs
            .iter()
            .find(|(n, _)| n == "i2c1.rs")
            .expect("i2c1.rs")
            .1;
        assert!(i2c1.contains("I2c::new_blocking"));
        assert!(i2c1.contains("embedded_hal::i2c::I2c"));

        // Switch SPI1 to async-DMA via its module → DMA driver + TODO in main.rs.
        mcu.modules.push(VirtualModule {
            id: "gi_spi_1".into(),
            kind: ModuleKind::GenericInterfaceSpi,
            name: "GI_SPI1".into(),
            pos: (0.0, 0.0),
            config: ModuleConfig::Spi(SpiModuleConfig {
                async_mode: AsyncBusMode::AsyncDma,
                ..SpiModuleConfig::new(1)
            }),
            connections: vec![],
        });
        let code = mcu.fresh_main_rs();
        assert!(
            code.contains("DMA_TX_TODO"),
            "async-DMA call has the DMA TODO:\n{code}"
        );
        let cfgs = mcu.config_files();
        let spi1 = &cfgs.iter().find(|(n, _)| n == "spi1.rs").unwrap().1;
        assert!(spi1.contains("Spi::new(spi"), "DMA driver: {spi1}");
        assert!(spi1.contains("embedded_hal_async::spi::SpiBus"));
        // The USART/GPIO-less project still keeps exactly one gen block.
        assert_eq!(
            code.matches(crate::panels::mcu_module::codegen::GEN_BEGIN)
                .count(),
            1
        );
    }

    /// The Native runtime forces concrete stm32f1xx-hal codegen for the bus
    /// peripherals even when the module keeps its default `Portable` api_style —
    /// the project-wide "all native" behaviour. `native_supported` gates it to F1.
    #[test]
    fn native_runtime_forces_concrete_hal() {
        use crate::panels::mcu_module::mcu::Runtime;
        use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;
        use crate::panels::mcu_module::modules::{ApiStyle, ModuleConfig, ModuleKind};

        assert!(native_supported("stm32f1"));
        assert!(!native_supported("stm32f4"));

        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        let n = mcu.modules[0].instance();
        // The module keeps its default Portable api_style throughout.
        assert!(
            matches!(&mcu.modules[0].config, ModuleConfig::Usart(c) if c.api_style == ApiStyle::Portable)
        );

        let usart_body = |mcu: &crate::panels::mcu_module::mcu::Mcu| {
            mcu.config_files()
                .into_iter()
                .find(|(name, _)| name == &format!("usart{n}.rs"))
                .expect("usart config file")
                .1
        };

        // Blocking + Portable module → portable embedded-io init.
        // The Portable USART now returns a NAMED type (see `Handle`), so an RTIC
        // `Local` field can name it; it is still embedded-io Read + Write.
        let body = usart_body(&mcu);
        assert!(body.contains("pub type Handle = SerialIo<"), "{body}");
        assert!(body.contains(") -> Handle {"), "{body}");

        // Native runtime OVERRIDES it → concrete `Serial` split, no embedded-io.
        mcu.runtime = Runtime::Native;
        assert!(mcu.is_native());
        let body = usart_body(&mcu);
        assert!(
            body.contains(&format!(
                "-> (serial::Tx<pac::USART{n}>, serial::Rx<pac::USART{n}>)"
            )),
            "{body}"
        );
        assert!(
            !body.contains("embedded_io"),
            "native forced -> no embedded-io:\n{body}"
        );
        // …and main.rs uses the split `(Tx, Rx)` tuple binding.
        let code = mcu.fresh_main_rs();
        assert!(
            code.contains(&format!("let (mut _tx{n}")),
            "native tuple binding:\n{code}"
        );
        assert!(
            !code.contains(&format!("let mut _serial{n}")),
            "not the single-value form"
        );
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
