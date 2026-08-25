//! Turning [`EspChip`] metadata into a chip definition the IDE can select.
//!
//! Development-time only: the definitions this produces are written to
//! `assets/mcus/` by [`tests::regenerate_esp_ron`] and compiled in, exactly as
//! the two hand-written ones already are. Nothing here runs on a user's machine.
//!
//! # The GPIO matrix decides the shape of this file
//!
//! On an STM32 a pin has a short, fixed list of alternate functions, and the
//! vendor publishes it per pin. An ESP32 routes almost any peripheral signal to
//! almost any pad through the GPIO matrix, so the honest answer to "which pins
//! can be SPI clock" is "all of them". Every usable GPIO therefore receives
//! every routable function, which is what the hand-written ESP32-C3 definition
//! already does — 21 pads × SCK, MOSI, MISO, NSS, SCL, SDA, two UARTs and six
//! PWM channels.
//!
//! # What the metadata cannot say
//!
//! It describes a die, not a part:
//!
//! * **No package, and no pin numbering.** The hand-written ESP32-C3 carries the
//!   real QFN32 pinout — `LNA_IN` on pin 1, `XTAL_P`, `CHIP_EN`, four power rails
//!   — and none of that is in the metadata. Generated chips get a LOGICAL
//!   layout: the GPIOs, spread over four sides, numbered in order.
//! * **No flash.** An ESP32's flash is a separate SPI part chosen by whoever
//!   built the module, so the same die ships as 2, 4, 8 or 16 MB.
//! * **No maximum frequency.** Left unset rather than typed in from a datasheet
//!   this code cannot check; it belongs with the clock tree.
//! * **Which GPIOs are really usable.** GPIO11 on a C3 is the flash power rail
//!   `VDD_SPI`; the metadata lists it as an ordinary GPIO. See [`RESERVED`].
//!
//! That is why the hand-written `esp32c3.ron` stays: it is strictly better than
//! anything derivable here, and [`tests::the_generator_agrees_with_the_hand_written_c3`]
//! uses it as the yardstick — comparing what each pad can DO, which the metadata
//! does know, and ignoring where each pad IS, which it does not.

use super::esp_metadata::EspChip;
use super::mcu_catalog::ToolchainKind;
use super::mcu_def::{McuDefinition, PinDef, PinLayout, ProjectDef};
use super::pins::logic::pin_function::PinFunction;

/// GPIOs the metadata lists but a board cannot use, per chip.
///
/// The die really does have these pads; the part wires them to something else.
/// `GPIO11` on an ESP32-C3 is `VDD_SPI`, the rail that powers the external flash
/// — offering it as a general-purpose pin would invite someone to drive the
/// supply of the chip they are about to boot from.
///
/// Deliberately short and per-chip: this is datasheet knowledge, not metadata,
/// so every entry is a claim someone has to stand behind. The C3 entry is the
/// one the hand-written definition already makes.
const RESERVED: &[(&str, &[u8])] = &[("esp32c3", &[11])];

/// How many LEDC channels the PWM driver exposes, per chip.
///
/// Not in the metadata. Taken from `esp-hal`'s own `#[cfg]` on
/// `ledc::channel::Number` — `not(any(esp32c2, esp32c3, esp32c6, esp32h2))`
/// gates channels 6 and 7 — because the number that matters is the one the
/// generated code has to compile against, not the one a datasheet quotes.
///
/// Chips with no `LEDC` peripheral at all never reach this: see
/// [`pwm_channels`].
const LEDC_SIX: &[&str] = &["esp32c2", "esp32c3", "esp32c6", "esp32h2"];

/// The PWM channels this chip's pads can carry, or none.
///
/// `esp32c5` and `esp32c61` have no `LEDC` in their metadata, so they get no PWM
/// functions rather than functions that reach no driver.
fn pwm_channels(chip: &EspChip) -> u8 {
    if !chip.peripherals.iter().any(|p| p == "LEDC") {
        return 0;
    }
    if LEDC_SIX.contains(&chip.id.as_str()) {
        6
    } else {
        8
    }
}

/// The instance number in a peripheral name: `SPI2` -> 2.
///
/// SPI is the one bus the metadata numbers by name rather than by field, and the
/// number is not an index — an ESP32-C3's only SPI master is `SPI2`, because
/// SPI0 and SPI1 are the flash controller's.
fn instance_number(name: &str) -> Option<u8> {
    name.trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .ok()
}

/// Every function a usable pad can be switched to.
///
/// Order matters only for the generated file's readability; the UI sorts its own
/// lists. This follows the hand-written C3 so the two can be diffed.
fn functions_for(chip: &EspChip, gpio: u8) -> Vec<PinFunction> {
    let mut out = vec![PinFunction::GpioInput, PinFunction::GpioOutput];

    // The functions that are NOT routable. Everything else on a pad gets there
    // through the GPIO matrix; these are bonded to one pad each, which is why
    // the vendor lists them per pad rather than per peripheral.
    //
    // USB comes from the same list as the ADC channels - the metadata calls
    // both "analog functions" - and dropping it is how GPIO18/19 lose the only
    // two pins that can carry USB at all.
    if let Some((dm, dp)) = chip.usb_pads() {
        if gpio == dm {
            out.push(PinFunction::UsbDm);
        }
        if gpio == dp {
            out.push(PinFunction::UsbDp);
        }
    }
    for (channel, pad) in &chip.adc {
        if *pad != gpio {
            continue;
        }
        // "ADC1_CH0" -> (1, 0)
        if let Some((unit, ch)) = channel
            .strip_prefix("ADC")
            .and_then(|r| r.split_once("_CH"))
            .and_then(|(u, c)| Some((u.parse::<u8>().ok()?, c.parse::<u8>().ok()?)))
        {
            out.push(PinFunction::AdcChannel {
                adc: unit,
                channel: ch,
            });
        }
    }

    for ch in 0..pwm_channels(chip) {
        out.push(PinFunction::TimerPwm {
            timer: 0,
            channel: ch,
        });
    }

    for u in &chip.uarts {
        out.push(PinFunction::UsartTx(u.id));
        out.push(PinFunction::UsartRx(u.id));
    }

    for s in &chip.spi {
        let Some(n) = instance_number(&s.instance) else {
            continue;
        };
        out.push(PinFunction::SpiSck(n));
        out.push(PinFunction::SpiMosi(n));
        out.push(PinFunction::SpiMiso(n));
        out.push(PinFunction::SpiNss(n));
    }

    for i in &chip.i2c {
        out.push(PinFunction::I2cScl(i.id));
        out.push(PinFunction::I2cSda(i.id));
    }

    // TWAI is Espressif's CAN. Unnumbered here because `CanTx`/`CanRx` are, and
    // no RISC-V part has a second one.
    if chip.peripherals.iter().any(|p| p == "TWAI0") {
        out.push(PinFunction::CanTx);
        out.push(PinFunction::CanRx);
    }

    out
}

/// Spread the pads over the four sides of a square, in order.
///
/// A LOGICAL layout — see the module docs. The pin numbers are positions in this
/// arrangement, not the part's real pin numbers, which the metadata does not
/// carry. Going round rather than down one side keeps a 31-GPIO chip from
/// drawing as a strip.
fn layout(chip: &EspChip, reserved: &[u8]) -> PinLayout {
    let pads: Vec<u8> = chip.gpios.clone();
    let per = pads.len().div_ceil(4);
    let mut sides: Vec<Vec<PinDef>> = vec![Vec::new(); 4];
    for (ix, gpio) in pads.iter().enumerate() {
        let is_reserved = reserved.contains(gpio);
        sides[(ix / per).min(3)].push(PinDef {
            number: ix + 1,
            name: format!("GPIO{gpio}"),
            reserved: is_reserved,
            functions: if is_reserved {
                Vec::new()
            } else {
                functions_for(chip, *gpio)
            },
            // No alternate-function indices: the GPIO matrix has none to
            // publish, which is the whole difference from an STM32 pad.
            af: Vec::new(),
            fn_owner: Vec::new(),
        });
    }
    let mut it = sides.into_iter();
    PinLayout {
        left: it.next().unwrap_or_default(),
        bottom: it.next().unwrap_or_default(),
        right: it.next().unwrap_or_default(),
        top: it.next().unwrap_or_default(),
        grid: None,
    }
}

/// Build the definition for one chip.
///
/// Refuses Xtensa outright: those parts need Espressif's rustc fork, and a
/// definition the IDE cannot build a project for is worse than no definition.
pub fn definition(chip: &EspChip) -> Result<McuDefinition, String> {
    let Some(target) = chip.arch.target(&chip.id) else {
        return Err(format!(
            "{}: {:?} needs Espressif's rustc fork (espup), which this IDE does not drive",
            chip.id, chip.arch
        ));
    };
    let reserved: &[u8] = RESERVED
        .iter()
        .find(|(id, _)| *id == chip.id)
        .map(|(_, r)| *r)
        .unwrap_or(&[]);

    Ok(McuDefinition {
        id: chip.id.clone(),
        display_name: chip.name.clone(),
        family: chip.id.clone(),
        // Unknown, and left so: an ESP32 die ships in several modules and the
        // metadata names none of them.
        package: String::new(),
        cpu: "RISC-V 32-bit".to_owned(),
        // The metadata carries no maximum frequency. Typing one in from a
        // datasheet would be a number nothing here could check; it arrives with
        // the clock tree.
        max_mhz: None,
        toolchain: ToolchainKind::EspRust,
        project: ProjectDef {
            pkg_name: chip.id.clone(),
            target: target.to_owned(),
            // No linker script: esp-hal generates memory.x and build.rs itself.
            flash_origin: String::new(),
            flash_size: String::new(),
            ram_origin: String::new(),
            ram_size: String::new(),
            // Empty on purpose. The ESP Cargo.toml is written from
            // `project_gen::ESP_HAL_REQ` and `probe_chip`, and this field is
            // never read on that path — the hand-written C3 still carries a
            // stale "esp-hal 0.23" here precisely because nothing consults it.
            hal_dep: String::new(),
            // Feeds the esp-hal, esp-println and bootloader features AND
            // `espflash --chip`, all four from this one string.
            probe_chip: chip.id.clone(),
            memory_comment: String::new(),
        },
        pins: layout(chip, reserved),
        // Modelled per family, which is later work. `None` means the Clock tab
        // says so rather than drawing a tree that is not this chip's.
        clock: super::mcu_def::ClockDef::None,
        dma: None,
        irq_vectors: Vec::new(),
        usart_ip: None,
        sdmmc_ip: None,
        clock_limits: Default::default(),
        clock_presets: Vec::new(),
    })
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::esp_metadata::{self, RISCV_CHIPS};
    use super::*;
    use std::collections::BTreeSet;

    fn chip(id: &str) -> EspChip {
        let dir = esp_metadata::vendor_dir().expect("esp-metadata in the cargo registry");
        esp_metadata::load(&dir, id).expect("chip parses")
    }

    /// `GPIO4 -> {"GpioInput", "UsartTx(0)", …}` for every non-reserved pad.
    fn pad_functions(def: &McuDefinition) -> std::collections::BTreeMap<String, BTreeSet<String>> {
        let mut out = std::collections::BTreeMap::new();
        for p in def
            .pins
            .top
            .iter()
            .chain(&def.pins.bottom)
            .chain(&def.pins.left)
            .chain(&def.pins.right)
        {
            if p.reserved || !p.name.starts_with("GPIO") {
                continue;
            }
            out.insert(
                p.name.clone(),
                p.functions.iter().map(|f| format!("{f:?}")).collect(),
            );
        }
        out
    }

    #[test]
    fn an_instance_number_comes_from_the_name_not_a_counter() {
        // SPI0 and SPI1 belong to the flash controller, so the first SPI a user
        // can have is SPI2 — and it must be called 2.
        assert_eq!(instance_number("SPI2"), Some(2));
        assert_eq!(instance_number("SPI3"), Some(3));
        assert_eq!(instance_number("LEDC"), None);
    }

    #[test]
    fn xtensa_is_refused_with_a_reason() {
        let dir = esp_metadata::vendor_dir();
        let Some(dir) = dir else { return };
        let Ok(esp32) = esp_metadata::load(&dir, "esp32") else {
            return;
        };
        let err = definition(&esp32).expect_err("xtensa must not produce a definition");
        assert!(err.contains("espup"), "{err}");
    }

    /// The yardstick: the hand-written ESP32-C3 was authored from the datasheet,
    /// so if the generator agrees with it about what every pad can DO, it can be
    /// trusted on the five chips there is nothing to compare against.
    ///
    /// Position is deliberately not compared — the metadata has no package.
    #[test]
    #[ignore]
    fn the_generator_agrees_with_the_hand_written_c3() {
        let generated = definition(&chip("esp32c3")).expect("c3 generates");
        let hand: McuDefinition =
            ron::from_str(include_str!("../../../assets/mcus/esp32c3.ron")).expect("c3 ron parses");

        let (g, h) = (pad_functions(&generated), pad_functions(&hand));
        assert_eq!(
            g.keys().collect::<Vec<_>>(),
            h.keys().collect::<Vec<_>>(),
            "different usable pads"
        );
        for (pad, want) in &h {
            let got = &g[pad];
            assert_eq!(
                got,
                want,
                "\n{pad}\n  only generated: {:?}\n  only hand-written: {:?}",
                got.difference(want).collect::<Vec<_>>(),
                want.difference(got).collect::<Vec<_>>()
            );
        }
        println!("{} pads agree, function for function", g.len());
    }

    /// Every RISC-V chip produces a definition that is at least coherent.
    #[test]
    #[ignore]
    fn every_riscv_chip_generates() {
        let dir = esp_metadata::vendor_dir().expect("esp-metadata in the cargo registry");
        for id in RISCV_CHIPS {
            let c = esp_metadata::load(&dir, id).unwrap();
            let d = definition(&c).unwrap_or_else(|e| panic!("{id}: {e}"));
            let pads = pad_functions(&d);
            let sample = pads.values().next().expect("at least one pad");
            println!(
                "{:<9} {:>2} pads  {:>3} functions each  target {}",
                id,
                pads.len(),
                sample.len(),
                d.project.target
            );
            assert_eq!(d.id, id);
            assert_eq!(d.project.probe_chip, id);
            assert!(!pads.is_empty(), "{id} has no usable pad");
            // Every pad carries the same ROUTABLE set — that IS the GPIO
            // matrix. Exactly two families are bonded to a pad instead and so
            // vary between them: analog channels and the USB differential pair.
            let routable = |set: &BTreeSet<String>| -> BTreeSet<String> {
                set.iter()
                    .filter(|f| !f.starts_with("AdcChannel") && !f.starts_with("Usb"))
                    .cloned()
                    .collect()
            };
            let base = routable(sample);
            for (pad, fns) in &pads {
                assert_eq!(
                    routable(fns),
                    base,
                    "{id}/{pad} differs outside its bonded functions"
                );
            }
        }
    }

    /// The chips this generator ships, which is every RISC-V part EXCEPT the C3.
    ///
    /// The C3 keeps its hand-written definition: it carries the real QFN32
    /// pinout, four power rails, the crystal and the antenna pin, none of which
    /// the metadata knows. Regenerating it would be a downgrade — so instead it
    /// serves as the yardstick, in
    /// [`the_generator_agrees_with_the_hand_written_c3`].
    const GENERATED: [&str; 3] = ["esp32c2", "esp32c6", "esp32h2"];

    /// Chips this generator can describe but the IDE does not ship.
    ///
    /// Their metadata parses and [`definition`] produces a coherent chip — but
    /// the project it would create cannot resolve its dependencies: no published
    /// `esp-println` (0.13 through 0.15) has an `esp32c5` or `esp32c61` feature,
    /// and the generated `Cargo.toml` asks for one. Shipping them would put two
    /// chips in the picker that fail at the first build.
    ///
    /// Kept as a list rather than deleted, so
    /// [`held_back_chips_still_generate`] keeps proving the only thing missing
    /// is upstream.
    const HELD_BACK: [&str; 2] = ["esp32c5", "esp32c61"];

    /// One-shot: write `assets/mcus/esp32*.ron` from the vendor metadata.
    ///
    /// `cargo test regenerate_esp_ron -- --ignored`
    ///
    /// The files it writes are committed and `include_str!`-ed by `builtins`,
    /// which is why nothing at runtime needs `esp-metadata` — the same shape as
    /// `builtins::regenerate_builtin_ron`.
    #[test]
    #[ignore]
    fn regenerate_esp_ron() {
        use ron::ser::PrettyConfig;
        let dir = esp_metadata::vendor_dir().expect("esp-metadata in the cargo registry");
        std::fs::create_dir_all("assets/mcus").unwrap();
        let pretty = PrettyConfig::default().struct_names(true);
        for id in GENERATED {
            let c = esp_metadata::load(&dir, id).unwrap_or_else(|e| panic!("{id}: {e}"));
            let def = definition(&c).unwrap_or_else(|e| panic!("{id}: {e}"));
            let text = ron::ser::to_string_pretty(&def, pretty.clone()).unwrap();
            let path = format!("assets/mcus/{id}.ron");
            std::fs::write(&path, &text).unwrap();
            println!("wrote {path}  ({} bytes)", text.len());
        }
    }

    /// The committed files must still say what the metadata says.
    ///
    /// Without this, a `.ron` edited by hand — or left behind by an older
    /// metadata release — would drift from its source with nothing to notice.
    #[test]
    #[ignore]
    fn the_committed_ron_matches_the_metadata() {
        let dir = esp_metadata::vendor_dir().expect("esp-metadata in the cargo registry");
        for id in GENERATED {
            let want = definition(&esp_metadata::load(&dir, id).unwrap()).unwrap();
            let text = std::fs::read_to_string(format!("assets/mcus/{id}.ron"))
                .unwrap_or_else(|e| panic!("{id}: {e} — run regenerate_esp_ron"));
            let got: McuDefinition = ron::from_str(&text).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(got.id, want.id);
            assert_eq!(got.project.target, want.project.target, "{id} target");
            assert_eq!(
                pad_functions(&got),
                pad_functions(&want),
                "{id} is out of date — run regenerate_esp_ron"
            );
        }
    }

    /// The held-back pair are held back for ONE reason, and it is not this code.
    ///
    /// If this ever fails because a definition stopped generating, the problem
    /// is here. If it passes and `esp-println` has meanwhile published the
    /// features, they can simply be moved into [`GENERATED`].
    #[test]
    #[ignore]
    fn held_back_chips_still_generate() {
        let dir = esp_metadata::vendor_dir().expect("esp-metadata in the cargo registry");
        for id in HELD_BACK {
            let c = esp_metadata::load(&dir, id).unwrap_or_else(|e| panic!("{id}: {e}"));
            let d = definition(&c).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(!pad_functions(&d).is_empty(), "{id} generated no pads");
            assert!(
                !std::path::Path::new(&format!("assets/mcus/{id}.ron")).exists(),
                "{id} is shipped, but its project cannot resolve esp-println"
            );
        }
    }

    /// The two chips whose metadata carries no LEDC must generate no PWM, rather
    /// than PWM the HAL has no driver for.
    #[test]
    #[ignore]
    fn chips_without_ledc_generate_no_pwm() {
        for id in ["esp32c5", "esp32c61"] {
            let d = definition(&chip(id)).unwrap();
            let has_pwm = pad_functions(&d)
                .values()
                .any(|f| f.iter().any(|n| n.starts_with("TimerPwm")));
            assert!(!has_pwm, "{id} generated PWM without a LEDC peripheral");
        }
        for id in ["esp32c3", "esp32c6"] {
            let d = definition(&chip(id)).unwrap();
            let n = pad_functions(&d)
                .values()
                .next()
                .unwrap()
                .iter()
                .filter(|f| f.starts_with("TimerPwm"))
                .count();
            assert_eq!(n, 6, "{id} should have six LEDC channels");
        }
    }
}
