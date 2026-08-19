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
            &mcu.watchdog_and_custom_inits(),
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
            &mcu.watchdog_and_custom_inits(),
        );
        let spliced = stm32::splice_section(existing, &new_section, &mcu.name, &mcu.id);
        // Clean up the init helpers older versions appended after `fn main`.
        stm32::strip_obsolete_helpers(spliced)
    }

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        let all = pins_of(mcu);
        let (usart, spi, i2c) = resolve_bus_configs(mcu);
        let can = modules::can_configs(&mcu.modules);
        let mut files = stm32::config_files(
            &all,
            &usart,
            &spi,
            &i2c,
            &can,
            &mcu.clock,
            mcu.gpio_native(),
        );
        // Pin-less, from the Configuration tab rather than the pins. The F1
        // gets only the IWDG - `stm32f1xx-hal` has no window watchdog - and
        // `watchdog_gen` enforces that itself.
        files.extend(super::watchdog_gen::config_files(
            &mcu.watchdog,
            &mcu.family,
        ));
        files
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

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        esp_config_files(mcu, EspRuntime::Blocking)
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
        &mcu.name,
        &mcu.id,
        &usart,
        &spi,
        &i2c,
        &mcu.custom_module_inits(),
        runtime,
    )
}

/// The `src/pins/configs/*.rs` an ESP project needs — one per wired bus
/// instance. Shared by both ESP backends: the config modules are the same
/// blocking esp-hal drivers on either runtime (esp-rtos does not change how a
/// `Uart` is built), so Async gets exactly the same files.
fn esp_config_files(mcu: &Mcu, runtime: EspRuntime) -> Vec<(String, String)> {
    let all = pins_of(mcu);
    let configured: Vec<&Pin> = all
        .iter()
        .copied()
        .filter(|p| !p.reserved && p.selected_function != PinFunction::Unset)
        .collect();
    let (uart, spi_n, i2c_n) = codegen_esp::bus_instances(&configured);
    crate::panels::mcu_module::codegen_esp_configs::config_files(
        &uart,
        &spi_n,
        &i2c_n,
        &modules::usart_configs(&mcu.modules),
        &modules::spi_configs(&mcu.modules),
        &modules::i2c_configs(&mcu.modules),
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
        &mcu.name,
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

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        esp_config_files(mcu, EspRuntime::Async)
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
                &mcu.watchdog_and_custom_inits(),
                mcu.clock_manual,
            ),
            tail = USER_TAIL,
        )
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let all = pins_of(mcu);
        let section = wba::make_generated_section(
            &mcu.name,
            &all,
            &mcu.clock,
            &mcu.watchdog_and_custom_inits(),
            mcu.clock_manual,
        );
        let section = super::common::keep_manual_clock(existing, section, mcu.clock_manual);
        wba::splice_section(existing, &section, &mcu.name, &mcu.id)
    }

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        // Like the generic embassy backend: bus inits stay inline in
        // main.rs, so `pins/configs/` here holds only the pin-less
        // peripherals the Configuration tab owns.
        super::watchdog_gen::config_files(&mcu.watchdog, &mcu.family)
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
            // A generic alternate function: the mode is how the user states the
            // DIRECTION, which is what turns the vendor's AF index into a real
            // `set_as_af_unchecked` line (see `embassy_common::pin_binding_line`).
            // Both directions are offered because an AF signal can be either —
            // `SAI1_SD_A` is an input in one configuration and an output in
            // another — and guessing would silently mis-wire the pad.
            PinFunction::Other(_) => &[
                GpioMode::Floating,
                GpioMode::PullUp,
                GpioMode::PullDown,
                GpioMode::PushPull,
                GpioMode::OpenDrain,
            ],
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
                &rcc::graph_clock_block(&mcu.family, &mcu.clock, mcu.clock_manual),
                &mcu.watchdog_and_custom_inits(),
            ),
            tail = USER_TAIL,
        )
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let all = pins_of(mcu);
        let section = embassy_common::make_generated_section(
            &mcu.name,
            &all,
            &rcc::graph_clock_block(&mcu.family, &mcu.clock, mcu.clock_manual),
            &mcu.watchdog_and_custom_inits(),
        );
        let section = super::common::keep_manual_clock(existing, section, mcu.clock_manual);
        embassy_common::splice_section(existing, &section, &mcu.name, &mcu.id)
    }

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        // Only the watchdogs: this backend keeps every bus init inline in
        // main.rs, so `pins/configs/` exists here purely for the tab-driven,
        // pin-less peripherals.
        super::watchdog_gen::config_files(&mcu.watchdog, &mcu.family)
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
        let section = super::common::keep_manual_clock(existing, section, mcu.clock_manual);
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
    embassy_async::async_peripherals(
        &mcu.family,
        mcu.dma.as_ref(),
        &mcu.irq_vectors,
        &all,
        &usart,
        &spi,
        &i2c,
    )
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
        &rcc::graph_clock_block(&mcu.family, &mcu.clock, mcu.clock_manual),
        &periphs.init_calls,
        &periphs.dma_irqs,
        &mcu.watchdog_and_custom_inits(),
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

/// What the **Blocking** runtime actually generates for `family` — the HAL crate
/// and the driver style.
///
/// Family-dependent because the answer is: `stm32f1xx-hal` with the portable
/// bridges on F1, `embassy-stm32` used SYNCHRONOUSLY on every other STM32, and
/// `esp-hal` on the ESP parts. A single fixed sentence described only the F1
/// case, so a user on an F2/F4/H5 read "portable blocking drivers" in the UI and
/// then found `embassy_stm32::init(...)` in `main.rs` — which reads as the
/// Runtime choice having been ignored. It has not: Blocking emits
/// `#[entry] fn main() -> !` with no executor and no `.await`; embassy-stm32 is
/// simply that family's HAL, and it is a perfectly ordinary blocking one.
pub fn blocking_hal_note(family: &str) -> &'static str {
    if family == "stm32f1" {
        "stm32f1xx-hal  ·  portable blocking drivers (embedded-io / \
         embedded-hal 1.0); per-module Native opt-in"
    } else if family == "stm32wba" || (family.starts_with("stm32") && family != "stm32f1") {
        "embassy-stm32 used SYNCHRONOUSLY — it is this family's HAL; no \
         executor, no .await"
    } else if family.starts_with("esp") {
        "esp-hal  ·  blocking drivers"
    } else {
        "the family's HAL, blocking"
    }
}

/// Does this family have the `pins/configs/io.rs` GPIO bridge (the Portable /
/// Native choice)? Only the F1 backend generates it — elsewhere GPIO binds
/// straight to the HAL's own types, so the choice does not exist and its UI
/// section is hidden rather than shown greyed out.
pub fn gpio_bridge_supported(family: &str) -> bool {
    native_supported(family)
}

/// Why the **Native** runtime is unavailable on `family`, or `None` when it is
/// available. Phrased for the System tab, next to the card it explains.
///
/// The distinction Native draws — concrete HAL types instead of the portable
/// bridges — only exists where the bridges do. On an embassy family the blocking
/// types are ALREADY concrete, so Native would emit byte-for-byte what Blocking
/// emits: not a missing feature, a choice with no object.
pub fn native_unavailable_reason(family: &str) -> Option<String> {
    if native_supported(family) {
        return None;
    }
    Some(if family.starts_with("esp") {
        format!(
            "`{family}` uses the ESP HAL's own scheme — there are no portable bridges to opt out of."
        )
    } else {
        format!(
            "`{family}` runs on embassy-stm32, whose blocking types are already concrete — \
             Blocking generates exactly that code, so Native would be identical to it."
        )
    })
}

/// Why **RTIC** is unavailable on `family`, or `None` when it is available.
///
/// Unlike Native, this one IS missing work rather than a category error: RTIC 2
/// runs on any Cortex-M, so an F2/F4/H5 would be eligible. What is F1-specific is
/// the generated code.
pub fn rtic_unavailable_reason(family: &str) -> Option<String> {
    if rtic_supported(family) {
        return None;
    }
    Some(if family.starts_with("esp") {
        format!("RTIC 2 has Cortex-M backends only, and `{family}` is RISC-V.")
    } else {
        format!(
            "Not written for `{family}` yet: the generated interrupt tasks use \
             stm32f1xx-hal's ExtiPin (make_interrupt_source / trigger_on_edge / \
             clear_interrupt_pending_bit), which embassy-stm32 does not expose."
        )
    })
}

/// Why the **Async** runtime is unavailable on `family`, or `None` when it is
/// available. Phrased for the System tab, next to the card it explains.
///
/// Like [`rtic_unavailable_reason`] and unlike [`native_unavailable_reason`],
/// this is missing WORK rather than a choice with no object: `embassy-stm32`
/// does publish chip features for the F1 (`stm32f103c8` among 95 of them), so
/// the family is eligible. What is in the way is that F1 is the one STM32 this
/// IDE routes to `stm32f1xx-hal` — the backend that gives it USB, the GPIO
/// Portable/Native bridge and RTIC, none of which the embassy path emits.
///
/// Worth saying out loud in the UI, because the consequence is not obvious: the
/// DMA choices (a USART's Buffered/DMA transport, a SPI/I2C bus' Async-DMA mode,
/// and the per-module channel pickers) all live on the Async runtime, so an F1
/// shows none of them.
pub fn async_unavailable_reason(family: &str) -> Option<String> {
    if async_supported(family) {
        return None;
    }
    Some(if family == "stm32f1" {
        "Not written for `stm32f1` yet: it is the one STM32 family this IDE builds on          stm32f1xx-hal (which is what gives it USB, the GPIO bridge and RTIC), while the          async runtime is embassy-stm32 throughout — and embassy-stm32 does support the          F1, so this is work, not a limit of the chip. The DMA transport and channel          pickers live on this runtime, so they are hidden here too."
            .to_owned()
    } else {
        format!(
            "No async backend for `{family}`: the runtime is embassy-stm32 on ARM and esp-rtos on the ESP parts."
        )
    })
}

/// Does a USB virtual module DO anything on this family?
///
/// - **stm32f1** — yes: the backend emits the whole CDC-ACM device
///   (`usb-device` + `usbd-serial` + the HAL's `stm32-usbd` feature).
/// - **ESP** — yes: the USB Serial/JTAG peripheral is hardware-fixed to two
///   pins, and the generated code says so. Nothing to configure, but the module
///   shows a real thing on the diagram.
/// - **every other STM32** — no. The embassy backends generate no USB code at
///   all, so adding the module produced exactly two stray dependencies and
///   nothing else. Offering a peripheral that cannot be generated is how a
///   configuration silently fails to reach `main.rs`.
///
/// Keyed on the family rather than on the pins, unlike the rest of
/// [`crate::panels::mcu_module::mcu::Mcu::supports_module`]: the pins exist on
/// every chip with USB, so only the BACKEND can answer this.
pub fn usb_supported(family: &str) -> bool {
    family == "stm32f1" || family.starts_with("esp")
}

#[cfg(test)]
mod blocking_note_tests {
    use super::{backend_for, blocking_hal_note, gpio_bridge_supported};

    /// The card must name the HAL the chip will ACTUALLY get — the mismatch
    /// that made a Blocking F2 project look mis-generated.
    #[test]
    fn the_note_matches_the_backend_that_will_run() {
        for family in ["stm32f2", "stm32f4", "stm32g0", "stm32h5", "stm32wba"] {
            let note = blocking_hal_note(family);
            assert!(
                note.contains("embassy-stm32"),
                "{family} generates embassy code on Blocking, so say so: {note}"
            );
            assert!(
                note.contains("no executor"),
                "and say it is the SYNC use of it: {note}"
            );
            assert!(
                backend_for(family).is_some(),
                "fixture: {family} has a backend"
            );
        }
        let f1 = blocking_hal_note("stm32f1");
        assert!(f1.contains("stm32f1xx-hal"), "{f1}");
        assert!(
            !f1.contains("embassy"),
            "F1 does not use embassy on Blocking: {f1}"
        );
        assert!(blocking_hal_note("esp32c3").contains("esp-hal"));
    }

    /// The GPIO Portable/Native choice exists only where `io.rs` is generated.
    #[test]
    fn the_gpio_bridge_is_f1_only() {
        assert!(gpio_bridge_supported("stm32f1"));
        for family in ["stm32f2", "stm32f4", "stm32wba", "esp32c3"] {
            assert!(!gpio_bridge_supported(family), "{family}");
        }
    }

    /// A greyed card must be able to say why — and the two reasons are NOT the
    /// same kind of thing, which is the point of having separate texts.
    #[test]
    fn every_unavailable_runtime_can_explain_itself() {
        use super::{native_unavailable_reason, rtic_unavailable_reason};
        // Available: nothing to explain.
        assert!(native_unavailable_reason("stm32f1").is_none());
        assert!(rtic_unavailable_reason("stm32f1").is_none());

        for family in ["stm32f2", "stm32f4", "stm32h5"] {
            let native =
                native_unavailable_reason(family).expect("greyed means it must explain itself");
            // Native is a category error: it would generate the same code.
            assert!(native.contains(family), "name the chip's family: {native}");
            assert!(
                native.contains("identical") || native.contains("already concrete"),
                "say it would be the same code, not that it is missing: {native}"
            );

            let rtic =
                rtic_unavailable_reason(family).expect("greyed means it must explain itself");
            // RTIC is missing work — it must NOT read as impossible.
            assert!(
                rtic.contains("ExtiPin") && rtic.contains("yet"),
                "name what is missing: {rtic}"
            );
        }

        // RISC-V is a different answer again.
        let esp = rtic_unavailable_reason("esp32c3").expect("greyed means it must explain itself");
        assert!(esp.contains("RISC-V"), "{esp}");
    }

    /// Async is greyed on exactly one family, and that card has to say why —
    /// the DMA choices hang off this runtime, so "no DMA option" is the visible
    /// symptom of a card the user cannot click.
    #[test]
    fn the_async_card_explains_the_one_family_it_is_greyed_on() {
        use super::async_unavailable_reason;
        // Every family with a backend: nothing to explain.
        for family in [
            "stm32f2", "stm32f4", "stm32g0", "stm32h5", "stm32wba", "esp32c3",
        ] {
            assert!(
                async_unavailable_reason(family).is_none(),
                "{family} has an async backend"
            );
        }
        let f1 = async_unavailable_reason("stm32f1").expect("greyed means it must explain itself");
        // Missing work, not a limit of the chip - the same category as RTIC,
        // and it must not read as impossible.
        assert!(
            f1.contains("yet"),
            "say it is not done, not that it cannot be: {f1}"
        );
        assert!(
            f1.contains("stm32f1xx-hal") && f1.contains("embassy-stm32"),
            "name both HALs, because the trade-off IS the reason: {f1}"
        );
        assert!(
            f1.contains("DMA"),
            "the user arrives here asking why DMA is missing: {f1}"
        );
        // A family with no backend at all gets the generic answer, not the F1 one.
        let other = async_unavailable_reason("stm8").expect("no backend, no async");
        assert!(
            other.contains("stm8") && !other.contains("stm32f1xx-hal"),
            "{other}"
        );
    }

    /// USB follows what the BACKEND writes, not what the pins allow.
    #[test]
    fn usb_follows_the_backend_not_the_pins() {
        use super::usb_supported;
        // F1 emits the full CDC device; ESP acknowledges its fixed peripheral.
        assert!(usb_supported("stm32f1"));
        assert!(usb_supported("esp32c3"));
        // The embassy families emit nothing — offering it there added two
        // dependencies and no code.
        for family in ["stm32f2", "stm32f4", "stm32g0", "stm32h5", "stm32wba"] {
            assert!(!usb_supported(family), "{family}");
        }
    }
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
        // A family with no RCC recipe (h7). Its clock is no longer left at the
        // reset default: `generic_recipe` reads the tree's canonical spine and
        // emits an explicit block, which is the point of that path — a chip
        // whose Clock tab is full has no business generating `Default::default`.
        def.family = "stm32h7".into();
        def.id = "stm32h743zi".into();
        let code = def.build_mcu().fresh_main_rs();
        assert!(code.contains("HAL: embassy-stm32 (blocking"));
        assert!(
            code.contains("no executor"),
            "the header must say WHY embassy appears on the blocking runtime"
        );
        assert!(code.contains("fn main() -> !"));
        assert!(
            code.contains("embassy_stm32::init(config)")
                && code.contains("no verified RCC recipe for this family"),
            "the tree drives the clock, and the block says the shape is a guess"
        );
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

    /// The usage examples at the bottom of every `pins/configs/*.rs` name a
    /// handle. That handle must be the one `main.rs` actually binds — including
    /// the Virtual Module's label (`_serial1_mw_radar`, not `_serial1`) — and no
    /// `{PLACEHOLDER}` may survive into the file.
    ///
    /// An example that names a variable which does not exist is worse than no
    /// example: it sends the reader debugging our documentation.
    #[test]
    fn config_examples_name_handles_that_main_rs_really_binds() {
        use crate::panels::mcu_module::mcu::Runtime;
        use crate::panels::mcu_module::modules::{ApiStyle, ModuleConfig, ModuleKind};
        use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

        // Pull `_serial1_label`-shaped identifiers out of the comment block.
        let handles_in = |file: &str| -> Vec<String> {
            let mut out = Vec::new();
            for line in file.lines().filter(|l| l.trim_start().starts_with("//")) {
                let mut rest = line;
                while let Some(i) = rest.find('_') {
                    let tail = &rest[i..];
                    let ident: String = tail
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    let is_handle = ["_serial", "_spi", "_i2c", "_can", "_tx", "_rx", "_uart"]
                        .iter()
                        .any(|p| ident.starts_with(p));
                    // Only when it is CALLED — `_i2c1.write(…)`, not prose.
                    if is_handle && tail[ident.len()..].starts_with('.') {
                        out.push(ident.clone());
                    }
                    rest = &tail[ident.len().max(1)..];
                }
            }
            out.sort();
            out.dedup();
            out
        };

        let stm32 = |family: &str, id: &str| {
            let mut def =
                crate::panels::mcu_module::builtins::builtin_for("stm32f103c8t6").unwrap();
            def.family = family.into();
            def.id = id.into();
            def.build_mcu()
        };

        // (label, mcu, runtime, per-module api style)
        let cases: Vec<(&str, Mcu, Runtime, Option<ApiStyle>)> = vec![
            (
                "stm32f1 portable",
                stm32("stm32f1", "stm32f103c8t6"),
                Runtime::Blocking,
                Some(ApiStyle::Portable),
            ),
            (
                "stm32f1 native",
                stm32("stm32f1", "stm32f103c8t6"),
                Runtime::Blocking,
                Some(ApiStyle::Native),
            ),
            (
                "stm32f4 async",
                stm32("stm32f4", "stm32f411re"),
                Runtime::Async,
                None,
            ),
            (
                "esp blocking",
                crate::panels::mcu_module::mock_esp32c3::create_esp32c3(),
                Runtime::Blocking,
                None,
            ),
            (
                "esp async",
                crate::panels::mcu_module::mock_esp32c3::create_esp32c3(),
                Runtime::Async,
                None,
            ),
        ];

        for (what, mut mcu, runtime, style) in cases {
            mcu.runtime = runtime;
            // Wire a USART, an SPI and an I2C the way the user would, and give
            // one of them a label — the case that made `{HANDLE}` necessary.
            for kind in [
                ModuleKind::GenericInterfaceUsart,
                ModuleKind::GenericInterfaceSpi,
                ModuleKind::GenericInterfaceI2c,
            ] {
                mcu.add_module(kind);
            }
            for md in &mut mcu.modules {
                match &mut md.config {
                    ModuleConfig::Usart(c) => {
                        c.custom_label = "mw radar".into();
                        if let Some(s) = style {
                            c.api_style = s;
                        }
                    }
                    ModuleConfig::Spi(c) => {
                        if let Some(s) = style {
                            c.api_style = s;
                        }
                    }
                    ModuleConfig::I2c(c) => {
                        if let Some(s) = style {
                            c.api_style = s;
                        }
                    }
                    _ => {}
                }
            }
            let _ = PinFunction::Unset; // (kept in scope for the import above)

            let main_rs = mcu.fresh_main_rs();
            let files = mcu.config_files();
            assert!(!files.is_empty(), "{what}: wired buses produce configs");

            for (name, body) in &files {
                assert!(
                    !body.contains("{HANDLE}") && !body.contains("{TX}") && !body.contains("{RX}"),
                    "{what}/{name}: unsubstituted placeholder:\n{body}"
                );
                for h in handles_in(body) {
                    assert!(
                        main_rs.contains(&h),
                        "{what}/{name}: the example calls `{h}`, which main.rs never binds.\n\
                         main.rs:\n{main_rs}"
                    );
                }
            }
        }
    }

    /// Every pin the ESP backend WRITES must parse back out of the file it
    /// wrote. This is the loop a project actually goes through on reload —
    /// `apply_saved_pins` wipes the diagram and re-applies only what
    /// [`parse_main_rs`] returns — so a pin that does not round-trip comes back
    /// Unset and takes its Virtual Module's wiring with it.
    ///
    /// Regression: moving the bus labels onto their own line (to stop a trailing
    /// comment swallowing the chain's `;`) left the `.with_xxx(...)` lines
    /// unlabelled, and USART/SPI/I2C pins silently stopped surviving a restart —
    /// 1 of 5 pins came back.
    ///
    /// [`parse_main_rs`]: super::super::parse_main_rs
    #[test]
    fn every_esp_pin_survives_a_generate_parse_round_trip() {
        use crate::panels::mcu_module::mcu::Runtime;
        use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
        use std::collections::BTreeMap;

        // One of everything the ESP backend can emit a pin for.
        let plan = [
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
        ];

        for runtime in [Runtime::Blocking, Runtime::Async] {
            let mut mcu = crate::panels::mcu_module::mock_esp32c3::create_esp32c3();
            mcu.runtime = runtime;
            for (name, func) in &plan {
                let pin = mcu
                    .iter_all_pins_mut()
                    .find(|p| p.name == *name)
                    .unwrap_or_else(|| panic!("{name} exists"));
                pin.selected_function = func.clone();
            }
            let code = mcu.fresh_main_rs();
            let parsed: BTreeMap<String, PinFunction> =
                super::super::parse_main_rs(&code).into_iter().collect();

            for (name, func) in &plan {
                assert_eq!(
                    parsed.get(*name),
                    Some(func),
                    "{runtime:?}: {name} did not round-trip\n{code}"
                );
            }
            assert_eq!(parsed.len(), plan.len(), "{runtime:?}: {parsed:?}");

            // ...and re-applying the parse must reproduce the same file, which
            // is what makes a reopened project regenerate identical code.
            let mut reloaded = crate::panels::mcu_module::mock_esp32c3::create_esp32c3();
            reloaded.runtime = runtime;
            reloaded.apply_saved_pins(&super::super::parse_main_rs(&code));
            assert_eq!(
                reloaded.fresh_main_rs(),
                code,
                "{runtime:?}: reload changed the generated file"
            );
        }
    }

    /// A label that trails the call — the shape older projects have on disk —
    /// still parses, so reopening one written before the layout change restores
    /// its pins too.
    #[test]
    fn legacy_trailing_bus_labels_still_parse() {
        use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
        let legacy = format!(
            "{}\n\
             #[esp_hal::main]\n\
             fn main() -> ! {{\n\
             \x20   let mut _uart0 = Uart::new(peripherals.UART0, UartConfig::default())\n\
             \x20       .with_rx(peripherals.GPIO20) // USART0  RX\n\
             \x20       .with_tx(peripherals.GPIO21) // USART0  TX;\n\
             {}\n",
            super::super::GEN_BEGIN,
            super::super::GEN_END,
        );
        let parsed = super::super::parse_main_rs(&legacy);
        assert_eq!(
            parsed,
            vec![
                ("GPIO20".to_owned(), PinFunction::UsartRx(0)),
                ("GPIO21".to_owned(), PinFunction::UsartTx(0)),
            ],
            "legacy trailing labels:\n{legacy}"
        );
    }

    /// A section header must NOT leak onto a later builder line — the label
    /// applies to the line immediately below it and nothing else.
    #[test]
    fn section_headers_do_not_label_pins() {
        let src = format!(
            "{}\n\
             \x20   // ── I2C0 ──\n\
             \x20   let mut _i2c0 = I2c::new(peripherals.I2C0, I2cConfig::default())\n\
             \x20       .unwrap()\n\
             \x20       .with_scl(peripherals.GPIO8)\n\
             {}\n",
            super::super::GEN_BEGIN,
            super::super::GEN_END,
        );
        assert!(
            super::super::parse_main_rs(&src).is_empty(),
            "an unlabelled pin must stay unparsed, not inherit the header"
        );
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
        // `ESP_ASYNC_IN=<file>` re-splices an EXISTING main.rs instead of writing
        // a fresh one — how a real project (older header, user code around the
        // markers) is put through the generator before compiling it.
        let main_rs = match std::env::var("ESP_ASYNC_IN") {
            Ok(src) => mcu.update_main_rs(&fs::read_to_string(src).expect("ESP_ASYNC_IN readable")),
            Err(_) => mcu.fresh_main_rs(),
        };
        fs::write(out.join("src/main.rs"), main_rs).unwrap();

        // The per-peripheral init modules main.rs calls into, plus the `mod`
        // declarations the real project tree writes (see
        // `ProjectTreeState::sync_config_files` / `sync_pin_files`).
        let cfgs = mcu.config_files();
        fs::create_dir_all(out.join("src/pins/configs")).unwrap();
        let mut decls = String::new();
        for (name, body) in &cfgs {
            fs::write(out.join("src/pins/configs").join(name), body).unwrap();
            decls.push_str(&format!("pub mod {};\n", name.trim_end_matches(".rs")));
        }
        fs::write(out.join("src/pins/configs/mod.rs"), decls).unwrap();
        fs::write(
            out.join("src/pins/mod.rs"),
            if cfgs.is_empty() {
                String::new()
            } else {
                "pub mod configs;\n".to_owned()
            },
        )
        .unwrap();
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
        assert!(back.contains("HAL: embassy-stm32 (blocking"));
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
        // This used to assert the DMA TODO. On a family WITH a channel table
        // there is none left to assert: the chip is an F4, so SPI1's channels
        // and their interrupt binding are filled in for the user.
        assert!(
            code.contains("p.DMA2_CH3, p.DMA2_CH0, Irqs"),
            "SPI1 gets its F4 channels:
{code}"
        );
        assert!(
            code.contains("DMA2_STREAM3 => embassy_stm32::dma::InterruptHandler"),
            "and the matching interrupt binding:
{code}"
        );
        assert!(
            !code.contains("DMA_TX_TODO"),
            "no TODO left on F4:
{code}"
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
