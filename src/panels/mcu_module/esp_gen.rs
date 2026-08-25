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
//! * **No package, and no pin numbering.** A die ships in several packages and
//!   the metadata names none of them, so a chip drawn from metadata alone gets
//!   a LOGICAL layout: its GPIOs, spread over four sides, numbered in order.
//!   That is a fiction — an ESP32-C5's `GPIO0` is package pin 9, not pin 1, and
//!   nineteen of its 48 pads are not GPIOs at all. [`PACKAGES`] carries the real
//!   thing, one transcribed datasheet table per part.
//! * **Which GPIOs are really usable.** `GPIO11` on a C3 is the flash power rail
//!   `VDD_SPI`; the metadata lists it as an ordinary GPIO. A part with a package
//!   table answers this by itself, because the rail appears there under its own
//!   name; for the rest there is [`RESERVED`].
//! * **No flash.** An ESP32's flash is a separate SPI part chosen by whoever
//!   built the module, so the same die ships as 2, 4, 8 or 16 MB.
//! * **No maximum frequency.** Taken from `esp-hal`'s own `CpuClock` enum
//!   instead — see [`super::esp_clocks::max_mhz`] — because the number that
//!   matters is the one the generated code can name.
//!
//! The hand-written `esp32c3.ron` still stays: it carries its QFN32 already, and
//! [`tests::the_generator_agrees_with_the_hand_written_c3`] uses it as the
//! yardstick — comparing what each pad can DO, which the metadata knows, and
//! ignoring where each pad IS, which only a datasheet can say.

use super::clock::graph::model::{ClockGraph, Edge, Node, NodeKind, NodeState};
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
///
/// Only consulted for a chip with no entry in [`PACKAGES`]. A packaged part has
/// no need of it: its rails are already on the drawing under their own names.
const RESERVED: &[(&str, &[u8])] = &[("esp32c3", &[11])];

/// A part's real package, transcribed from its datasheet.
struct Package {
    /// The chip id, as `esp-metadata` names it.
    chip: &'static str,
    /// `QFN40` - what the MCU header shows, and what nothing else can supply:
    /// the metadata describes a die and names no package at all.
    name: &'static str,
    /// Every pad, in the vendor's numbering, going counter-clockwise from the
    /// top of the left edge. A pad named `GPIOn` is usable and takes the
    /// metadata's function list; every other name is a supply rail, a crystal
    /// pad or an antenna feed, and is emitted RESERVED with no functions.
    ///
    /// The exposed thermal pad is left out where a datasheet numbers it (the
    /// C6 calls it pin 41), because it is not a side pin - which is also what
    /// keeps every list divisible by four.
    pads: &'static [(u8, &'static str)],
    /// GPIOs the die has that this package does not offer. Two real reasons,
    /// never a typo:
    ///
    /// * the pad is not bonded out - a C2's QFN24 keeps `GPIO11`-`GPIO17`
    ///   inside the package, wired to the flash die;
    /// * the pad IS on the package but under another name, because it is the
    ///   `VDD_SPI` rail that powers the flash. Offering it invites someone to
    ///   drive the supply of the chip they are about to boot from, which is
    ///   the same call the hand-written C3 makes for its own `GPIO11`.
    ///
    /// Checked against the metadata by
    /// [`tests::every_package_names_exactly_the_gpios_its_metadata_has`], so a
    /// mistyped number fails a test rather than quietly reserving a good pad.
    ///
    /// Read by that test and by nothing else — `package_layout` lays out the
    /// pads a package HAS, so an omitted GPIO simply never appears. This is a
    /// declared ledger, not an input: its whole job is to make the omission
    /// deliberate and reviewable. Hence the allow, which `cargo check --tests`
    /// never needed and `cargo build --release` does.
    #[cfg_attr(not(test), allow(dead_code))]
    off_package: &'static [u8],
}

/// The parts whose datasheet has been read. The rest keep the LOGICAL layout of
/// [`logical_layout`], because inventing pin numbers is better than getting
/// real ones wrong.
///
/// The ESP32-C3 is absent on purpose: its definition is hand-written, carries
/// its QFN32 already, and is the yardstick this generator is measured against.
const PACKAGES: &[Package] = &[
    Package {
        chip: "esp32",
        name: "QFN48",
        pads: ESP32_QFN48,
        off_package: &[20],
    },
    Package {
        chip: "esp32c2",
        name: "QFN24",
        pads: ESP32C2_QFN24,
        off_package: &[11, 12, 13, 14, 15, 16, 17],
    },
    Package {
        chip: "esp32c5",
        name: "QFN48",
        pads: ESP32C5_QFN48,
        off_package: &[19],
    },
    Package {
        chip: "esp32c6",
        name: "QFN40",
        pads: ESP32C6_QFN40,
        off_package: &[14, 27],
    },
    Package {
        chip: "esp32c61",
        name: "QFN40",
        pads: ESP32C61_QFN40,
        off_package: &[18],
    },
    Package {
        chip: "esp32h2",
        name: "QFN32",
        pads: ESP32H2_QFN32,
        off_package: &[6, 7],
    },
    Package {
        chip: "esp32s2",
        name: "QFN56",
        pads: ESP32S2_QFN56,
        off_package: &[],
    },
    Package {
        chip: "esp32s3",
        name: "QFN56",
        pads: ESP32S3_QFN56,
        off_package: &[],
    },
];

/// ESP32, QFN48 - the "Pin Overview" table of its datasheet.
///
/// The original ESP32. `GPIO20` is on the die but bonded out only on the
/// PICO variants, so the QFN48 does not offer it.
const ESP32_QFN48: &[(u8, &str)] = &[
    // Left edge, top to bottom.
    (1, "VDDA"),
    (2, "LNA_IN"),
    (3, "VDD3P3"),
    (4, "VDD3P3"),
    (5, "GPIO36"),
    (6, "GPIO37"),
    (7, "GPIO38"),
    (8, "GPIO39"),
    (9, "CHIP_PU"),
    (10, "GPIO34"),
    (11, "GPIO35"),
    (12, "GPIO32"),
    // Bottom edge, left to right.
    (13, "GPIO33"),
    (14, "GPIO25"),
    (15, "GPIO26"),
    (16, "GPIO27"),
    (17, "GPIO14"),
    (18, "GPIO12"),
    (19, "VDD3P3_RTC"),
    (20, "GPIO13"),
    (21, "GPIO15"),
    (22, "GPIO2"),
    (23, "GPIO0"),
    (24, "GPIO4"),
    // Right edge, bottom to top.
    (25, "GPIO16"),
    (26, "VDD_SDIO"),
    (27, "GPIO17"),
    (28, "GPIO9"),
    (29, "GPIO10"),
    (30, "GPIO11"),
    (31, "GPIO6"),
    (32, "GPIO7"),
    (33, "GPIO8"),
    (34, "GPIO5"),
    (35, "GPIO18"),
    (36, "GPIO23"),
    // Top edge, right to left.
    (37, "VDD3P3_CPU"),
    (38, "GPIO19"),
    (39, "GPIO22"),
    (40, "GPIO3"),
    (41, "GPIO1"),
    (42, "GPIO21"),
    (43, "VDDA"),
    (44, "XTAL_N"),
    (45, "XTAL_P"),
    (46, "VDDA"),
    (47, "CAP2"),
    (48, "CAP1"),
];

/// ESP8684, QFN24 - the "Pin Overview" table of its datasheet.
///
/// Sold as ESP8684, which is the name on its datasheet. The QFN24 keeps
/// `GPIO11`-`GPIO17` inside the package, wired to the flash die.
const ESP32C2_QFN24: &[(u8, &str)] = &[
    // Left edge, top to bottom.
    (1, "ANT"),
    (2, "VDDA3P3"),
    (3, "VDDA3P3"),
    (4, "GPIO0"),
    (5, "GPIO1"),
    (6, "GPIO2"),
    // Bottom edge, left to right.
    (7, "CHIP_EN"),
    (8, "GPIO3"),
    (9, "GPIO4"),
    (10, "GPIO5"),
    (11, "VDD3P3_RTC"),
    (12, "GPIO6"),
    // Right edge, bottom to top.
    (13, "GPIO7"),
    (14, "GPIO8"),
    (15, "GPIO9"),
    (16, "GPIO10"),
    (17, "VDD3P3_CPU"),
    (18, "GPIO18"),
    // Top edge, right to left.
    (19, "GPIO19"),
    (20, "GPIO20"),
    (21, "VDDA"),
    (22, "XTAL_N"),
    (23, "XTAL_P"),
    (24, "VDDA"),
];

/// ESP32-C5, QFN48 - the "Pin Overview" table of its datasheet.
///
/// Pin 29 is `VDD_SPI/NC`, so `GPIO19` is a rail rather than a pad. The
/// `/NC` suffixes on pins 26-32 mean those pads are the in-package flash bus
/// on the variants that have one and Not Connected on the ones that do not;
/// they stay usable, since the die cannot say how it was packaged.
const ESP32C5_QFN48: &[(u8, &str)] = &[
    // Left edge, top to bottom.
    (1, "VDDA6"),
    (2, "GND"),
    (3, "VDDA7"),
    (4, "XTAL_N"),
    (5, "XTAL_P"),
    (6, "VDDA8"),
    (7, "CHIP_PU"),
    (8, "VDDPST1"),
    (9, "GPIO0"),
    (10, "GPIO1"),
    (11, "GPIO2"),
    (12, "GPIO3"),
    // Bottom edge, left to right.
    (13, "GPIO4"),
    (14, "GPIO5"),
    (15, "GPIO6"),
    (16, "GPIO7"),
    (17, "GPIO8"),
    (18, "GPIO9"),
    (19, "GPIO10"),
    (20, "GPIO11"),
    (21, "GPIO12"),
    (22, "GPIO13"),
    (23, "GPIO14"),
    (24, "VDDPST2"),
    // Right edge, bottom to top.
    (25, "GPIO15"),
    (26, "GPIO16"),
    (27, "GPIO17"),
    (28, "GPIO18"),
    (29, "VDD_SPI"),
    (30, "GPIO20"),
    (31, "GPIO21"),
    (32, "GPIO22"),
    (33, "GPIO23"),
    (34, "GPIO24"),
    (35, "GPIO25"),
    (36, "GPIO26"),
    // Top edge, right to left.
    (37, "GPIO27"),
    (38, "GPIO28"),
    (39, "VDDPST3"),
    (40, "VDDA1"),
    (41, "VDDA2"),
    (42, "ANT_2G"),
    (43, "GND"),
    (44, "VDDA3"),
    (45, "VDDA4"),
    (46, "VDDA5"),
    (47, "GND"),
    (48, "ANT_5G"),
];

/// ESP32-C6, QFN40 - the "Pin Overview" table of its datasheet.
///
/// The QFN40, which is the die Espressif's own C6 modules carry. A QFN32
/// exists too (Table 7-2) and bonds fewer pads; the larger package is the
/// superset, so it is the one described here.
const ESP32C6_QFN40: &[(u8, &str)] = &[
    // Left edge, top to bottom.
    (1, "ANT"),
    (2, "VDDA3P3"),
    (3, "VDDA3P3"),
    (4, "CHIP_PU"),
    (5, "VDDPST1"),
    (6, "GPIO0"),
    (7, "GPIO1"),
    (8, "GPIO2"),
    (9, "GPIO3"),
    (10, "GPIO4"),
    // Bottom edge, left to right.
    (11, "GPIO5"),
    (12, "GPIO6"),
    (13, "GPIO7"),
    (14, "GPIO8"),
    (15, "GPIO9"),
    (16, "GPIO10"),
    (17, "GPIO11"),
    (18, "GPIO12"),
    (19, "GPIO13"),
    (20, "GPIO24"),
    // Right edge, bottom to top.
    (21, "GPIO25"),
    (22, "GPIO26"),
    (23, "VDD_SPI"),
    (24, "GPIO28"),
    (25, "GPIO29"),
    (26, "GPIO30"),
    (27, "GPIO15"),
    (28, "VDDPST2"),
    (29, "GPIO16"),
    (30, "GPIO17"),
    // Top edge, right to left.
    (31, "GPIO18"),
    (32, "GPIO19"),
    (33, "GPIO20"),
    (34, "GPIO21"),
    (35, "GPIO22"),
    (36, "GPIO23"),
    (37, "VDDA1"),
    (38, "XTAL_N"),
    (39, "XTAL_P"),
    (40, "VDDA2"),
];

/// ESP32-C61, QFN40 - the "Pin Overview" table of its datasheet.
///
/// Its GPIOs are not in package order at all - the SDIO block sits at pins
/// 13-18 and `GPIO7` alone at pin 36. Pin 24 is `VDD_SPI/NC`, so `GPIO18`
/// is a rail. An LGA40 with the same pinout also exists.
const ESP32C61_QFN40: &[(u8, &str)] = &[
    // Left edge, top to bottom.
    (1, "ANT_2G"),
    (2, "VDDA3"),
    (3, "VDDA4"),
    (4, "CHIP_PU"),
    (5, "VDDPST1"),
    (6, "GPIO0"),
    (7, "GPIO1"),
    (8, "GPIO2"),
    (9, "GPIO3"),
    (10, "GPIO4"),
    // Bottom edge, left to right.
    (11, "GPIO5"),
    (12, "GPIO6"),
    (13, "GPIO25"),
    (14, "GPIO26"),
    (15, "GPIO27"),
    (16, "GPIO28"),
    (17, "GPIO22"),
    (18, "GPIO23"),
    (19, "GPIO14"),
    (20, "GPIO15"),
    // Right edge, bottom to top.
    (21, "VDDPST2"),
    (22, "GPIO16"),
    (23, "GPIO17"),
    (24, "VDD_SPI"),
    (25, "GPIO19"),
    (26, "GPIO20"),
    (27, "GPIO21"),
    (28, "GPIO12"),
    (29, "GPIO13"),
    (30, "GPIO24"),
    // Top edge, right to left.
    (31, "GPIO8"),
    (32, "GPIO9"),
    (33, "GPIO10"),
    (34, "GPIO11"),
    (35, "GPIO29"),
    (36, "GPIO7"),
    (37, "VDDA1"),
    (38, "XTAL_N"),
    (39, "XTAL_P"),
    (40, "VDDA2"),
];

/// ESP32-H2, QFN32 - the "Pin Overview" table of its datasheet.
///
/// `GPIO6` and `GPIO7` are on the die but not bonded out here.
const ESP32H2_QFN32: &[(u8, &str)] = &[
    // Left edge, top to bottom.
    (1, "VDD3P3"),
    (2, "VDD3P3"),
    (3, "GPIO0"),
    (4, "GPIO1"),
    (5, "GPIO2"),
    (6, "GPIO3"),
    (7, "GPIO4"),
    (8, "GPIO5"),
    // Bottom edge, left to right.
    (9, "VDDPST1"),
    (10, "GPIO8"),
    (11, "GPIO9"),
    (12, "GPIO10"),
    (13, "GPIO11"),
    (14, "GPIO12"),
    (15, "GPIO13"),
    (16, "GPIO14"),
    // Right edge, bottom to top.
    (17, "CHIP_EN"),
    (18, "VBAT"),
    (19, "VDDA_PMU"),
    (20, "VDDPST2"),
    (21, "GPIO22"),
    (22, "GPIO23"),
    (23, "GPIO24"),
    (24, "GPIO25"),
    // Top edge, right to left.
    (25, "GPIO26"),
    (26, "GPIO27"),
    (27, "VDD3P3"),
    (28, "XTAL_N"),
    (29, "XTAL_P"),
    (30, "VDD3P3"),
    (31, "VDD3P3"),
    (32, "ANT"),
];

/// ESP32-S2, QFN56 - the "Pin Overview" table of its datasheet.
///
/// Every GPIO the die has reaches a pad. `VDD_SPI` is pin 30 and carries no
/// GPIO number at all on this part, unlike the C5, C6 and C61.
const ESP32S2_QFN56: &[(u8, &str)] = &[
    // Left edge, top to bottom.
    (1, "VDDA"),
    (2, "LNA_IN"),
    (3, "VDD3P3"),
    (4, "VDD3P3"),
    (5, "GPIO0"),
    (6, "GPIO1"),
    (7, "GPIO2"),
    (8, "GPIO3"),
    (9, "GPIO4"),
    (10, "GPIO5"),
    (11, "GPIO6"),
    (12, "GPIO7"),
    (13, "GPIO8"),
    (14, "GPIO9"),
    // Bottom edge, left to right.
    (15, "GPIO10"),
    (16, "GPIO11"),
    (17, "GPIO12"),
    (18, "GPIO13"),
    (19, "GPIO14"),
    (20, "VDD3P3_RTC"),
    (21, "GPIO15"),
    (22, "GPIO16"),
    (23, "GPIO17"),
    (24, "GPIO18"),
    (25, "GPIO19"),
    (26, "GPIO20"),
    (27, "VDD3P3_RTC_IO"),
    (28, "GPIO21"),
    // Right edge, bottom to top.
    (29, "GPIO26"),
    (30, "VDD_SPI"),
    (31, "GPIO27"),
    (32, "GPIO28"),
    (33, "GPIO29"),
    (34, "GPIO30"),
    (35, "GPIO31"),
    (36, "GPIO32"),
    (37, "GPIO33"),
    (38, "GPIO34"),
    (39, "GPIO35"),
    (40, "GPIO36"),
    (41, "GPIO37"),
    (42, "GPIO38"),
    // Top edge, right to left.
    (43, "GPIO39"),
    (44, "GPIO40"),
    (45, "VDD3P3_CPU"),
    (46, "GPIO41"),
    (47, "GPIO42"),
    (48, "GPIO43"),
    (49, "GPIO44"),
    (50, "GPIO45"),
    (51, "VDDA"),
    (52, "XTAL_N"),
    (53, "XTAL_P"),
    (54, "VDDA"),
    (55, "GPIO46"),
    (56, "CHIP_PU"),
];

/// ESP32-S3, QFN56 - the "Pin Overview" table of its datasheet.
///
/// Every GPIO reaches a pad. `GPIO47` and `GPIO48` sit at pins 37 and 36,
/// out of order, because they are the differential SPI clock pair.
const ESP32S3_QFN56: &[(u8, &str)] = &[
    // Left edge, top to bottom.
    (1, "LNA_IN"),
    (2, "VDD3P3"),
    (3, "VDD3P3"),
    (4, "CHIP_PU"),
    (5, "GPIO0"),
    (6, "GPIO1"),
    (7, "GPIO2"),
    (8, "GPIO3"),
    (9, "GPIO4"),
    (10, "GPIO5"),
    (11, "GPIO6"),
    (12, "GPIO7"),
    (13, "GPIO8"),
    (14, "GPIO9"),
    // Bottom edge, left to right.
    (15, "GPIO10"),
    (16, "GPIO11"),
    (17, "GPIO12"),
    (18, "GPIO13"),
    (19, "GPIO14"),
    (20, "VDD3P3_RTC"),
    (21, "GPIO15"),
    (22, "GPIO16"),
    (23, "GPIO17"),
    (24, "GPIO18"),
    (25, "GPIO19"),
    (26, "GPIO20"),
    (27, "GPIO21"),
    (28, "GPIO26"),
    // Right edge, bottom to top.
    (29, "VDD_SPI"),
    (30, "GPIO27"),
    (31, "GPIO28"),
    (32, "GPIO29"),
    (33, "GPIO30"),
    (34, "GPIO31"),
    (35, "GPIO32"),
    (36, "GPIO48"),
    (37, "GPIO47"),
    (38, "GPIO33"),
    (39, "GPIO34"),
    (40, "GPIO35"),
    (41, "GPIO36"),
    (42, "GPIO37"),
    // Top edge, right to left.
    (43, "GPIO38"),
    (44, "GPIO39"),
    (45, "GPIO40"),
    (46, "VDD3P3_CPU"),
    (47, "GPIO41"),
    (48, "GPIO42"),
    (49, "GPIO43"),
    (50, "GPIO44"),
    (51, "GPIO45"),
    (52, "GPIO46"),
    (53, "XTAL_N"),
    (54, "XTAL_P"),
    (55, "VDDA"),
    (56, "VDDA"),
];

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
/// Whether a function has to DRIVE the pad.
///
/// The distinction only matters on parts that have pads which cannot: an
/// ESP32's GPIO34..39 are input-only. Getting this wrong is not a warning — the
/// generated project fails on `PeripheralOutput`, and it does so for whichever
/// function landed there, not just for `GpioOutput`. The first fix only covered
/// GPIO output, and the very next build put a UART TX, an I2C pair and a PWM
/// channel on those same pads.
///
/// Open-drain still counts as driving: I2C pulls the line down.
fn drives(f: &PinFunction) -> bool {
    !matches!(
        f,
        PinFunction::GpioInput
            | PinFunction::UsartRx(_)
            | PinFunction::SpiMiso(_)
            | PinFunction::CanRx
            | PinFunction::AdcChannel { .. }
    )
}

fn functions_for(chip: &EspChip, pad: super::esp_metadata::Gpio) -> Vec<PinFunction> {
    let gpio = pad.number;
    let mut out = Vec::new();
    if pad.input {
        out.push(PinFunction::GpioInput);
    }
    if pad.output {
        out.push(PinFunction::GpioOutput);
    }

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

    // Last, and over EVERYTHING above: a pad that cannot drive keeps only the
    // functions that read.
    if !pad.output {
        out.retain(|f| !drives(f));
    }
    out
}

/// The chip's pin layout: its real package when the datasheet has been read for
/// it, the logical arrangement otherwise.
fn layout(chip: &EspChip, reserved: &[u8]) -> PinLayout {
    match PACKAGES.iter().find(|p| p.chip == chip.id) {
        Some(pkg) => package_layout(chip, pkg.pads),
        None => logical_layout(chip, reserved),
    }
}

/// The part's real pinout, laid out the way a QFN is numbered: pin 1 at the top
/// of the LEFT edge, counter-clockwise from there.
///
/// The two far sides are reversed because each side is drawn top-to-bottom or
/// left-to-right, while the numbering runs up the right edge and back along the
/// top. This reproduces the arrangement the hand-written ESP32-C3 uses, which is
/// the yardstick for what an ESP chip should look like.
fn package_layout(chip: &EspChip, pads: &[(u8, &str)]) -> PinLayout {
    let per = pads.len() / 4;
    let one = |&(number, name): &(u8, &str)| {
        // A pad the metadata does not know as a GPIO is a supply, a crystal or
        // an antenna feed. It is a real pin of the package and belongs on the
        // drawing, but nothing can be assigned to it.
        let gpio = name
            .strip_prefix("GPIO")
            .and_then(|n| n.parse::<u8>().ok())
            .and_then(|n| chip.gpios.iter().find(|g| g.number == n).copied());
        PinDef {
            number: number as usize,
            name: name.to_owned(),
            reserved: gpio.is_none(),
            functions: gpio.map(|g| functions_for(chip, g)).unwrap_or_default(),
            af: Vec::new(),
            fn_owner: Vec::new(),
        }
    };
    let side = |from: usize| pads[from..from + per].iter().map(one).collect::<Vec<_>>();
    let (mut right, mut top) = (side(2 * per), side(3 * per));
    right.reverse();
    top.reverse();
    PinLayout {
        left: side(0),
        bottom: side(per),
        right,
        top,
        grid: None,
    }
}

/// Spread the pads over the four sides of a square, in order.
///
/// A LOGICAL layout — see the module docs. The pin numbers are positions in this
/// arrangement, not the part's real pin numbers, which the metadata does not
/// carry. Going round rather than down one side keeps a 31-GPIO chip from
/// drawing as a strip.
fn logical_layout(chip: &EspChip, reserved: &[u8]) -> PinLayout {
    let pads = &chip.gpios;
    let per = pads.len().div_ceil(4);
    let mut sides: Vec<Vec<PinDef>> = vec![Vec::new(); 4];
    for (ix, pad) in pads.iter().enumerate() {
        let is_reserved = reserved.contains(&pad.number);
        sides[(ix / per).min(3)].push(PinDef {
            number: ix + 1,
            name: format!("GPIO{}", pad.number),
            reserved: is_reserved,
            functions: if is_reserved {
                Vec::new()
            } else {
                functions_for(chip, *pad)
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

/// The CPU-clock chain, or `None` when it cannot be built from evidence.
///
/// # Deliberately smaller than the silicon
///
/// The metadata describes the whole tree, and it is derivable: muxes, dividers
/// and constants, in three regular shapes. But it describes what the HARDWARE
/// can express, and `esp-hal` exposes far less — an ESP32-H2's `CpuClkConfig`
/// is a divisor of 0..=255, while its `CpuClock` enum has exactly one variant.
/// A faithful tree would therefore offer 96/13 = 7.4 MHz, which the code
/// generator cannot emit, and the Clock tab would be promising settings that
/// silently snap to something else on the way out.
///
/// So the divider carries exactly the options `esp-hal` can name. That is also
/// what the hand-written ESP32-C3 graph does — `Divider { options: [3, 6] }`
/// gives 160 and 80 and nothing between them.
///
/// # It refuses rather than guesses
///
/// Every number comes from one of two checked sources: the PLL and crystal from
/// Espressif's metadata, the CPU frequencies from `esp-hal`. The divisors are
/// what reconciles them — and if a division is not exact, the two sources
/// disagree about the chip and no graph is built at all.
fn clock_graph(chip: &EspChip) -> Option<ClockGraph> {
    let pll = chip.pll_hz?;
    let xtal = *chip.xtal_hz.first()?;
    let opts = super::esp_clocks::cpu_options(&chip.id);
    if opts.is_empty() {
        return None;
    }
    // Fastest first, so index 0 is the default a project boots at.
    let mut divs: Vec<u32> = Vec::new();
    for mhz in opts.iter().rev() {
        let hz = mhz * 1_000_000;
        if hz == 0 || pll % hz != 0 {
            return None;
        }
        divs.push(pll / hz);
    }

    let n = |id: &str, kind: NodeKind, state: NodeState| Node {
        id: id.to_owned(),
        kind,
        state,
        // No datasheet ceilings: `esp-hal`'s own enum already bounds what the
        // CPU node can be set to, so a second limit could only disagree.
        limit: None,
    };
    let e = |from: &str, to: &str, input: usize| Edge {
        from: from.to_owned(),
        to: to.to_owned(),
        input,
    };
    Some(ClockGraph {
        nodes: vec![
            n(
                "xtal",
                NodeKind::Source {
                    min_hz: xtal,
                    max_hz: *chip.xtal_hz.last().unwrap_or(&xtal),
                    gated: false,
                },
                NodeState::Source {
                    enabled: true,
                    hz: xtal,
                },
            ),
            // A fixed source, not a multiplier: the metadata states the PLL's
            // output directly and says nothing about how it reaches it.
            n(
                "pll",
                NodeKind::Source {
                    min_hz: pll,
                    max_hz: pll,
                    gated: false,
                },
                NodeState::Source {
                    enabled: true,
                    hz: pll,
                },
            ),
            n(
                "cpu_div",
                NodeKind::Divider { options: divs },
                NodeState::Index(0),
            ),
            n("cpu", NodeKind::Mux { inputs: 2 }, NodeState::Index(1)),
        ],
        edges: vec![
            e("pll", "cpu_div", 0),
            e("xtal", "cpu", 0),
            e("cpu_div", "cpu", 1),
        ],
    })
}

/// Build the definition for one chip.
///
/// Works for Xtensa parts too — the description is sound, and their metadata is
/// as complete as anyone's. What is missing is downstream: nothing in the IDE
/// can invoke `cargo +esp`, so those definitions are not registered as built-ins
/// and the chip picker does not offer them. See
/// [`Arch::needs_esp_toolchain`](super::esp_metadata::Arch::needs_esp_toolchain).
pub fn definition(chip: &EspChip) -> Result<McuDefinition, String> {
    let Some(target) = chip.arch.target(&chip.id) else {
        return Err(format!(
            "{}: no Rust target triple for {:?}",
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
        // From the datasheet table when there is one; otherwise unknown and
        // left so, since an ESP32 die ships in several modules and the metadata
        // names none of them.
        package: PACKAGES
            .iter()
            .find(|p| p.chip == chip.id)
            .map(|p| p.name.to_owned())
            .unwrap_or_default(),
        cpu: match chip.arch {
            super::esp_metadata::Arch::RiscV => "RISC-V 32-bit",
            super::esp_metadata::Arch::Xtensa => "Xtensa LX",
        }
        .to_owned(),
        // NOT from the metadata, which carries no frequency at all, and not
        // from a datasheet either: the fastest this chip can be SET to is the
        // top of `esp-hal`'s own `CpuClock` enum, and that is the only number
        // the generated code can name. An H2 is 96, not the 160 a family
        // resemblance would suggest.
        max_mhz: super::esp_clocks::max_mhz(&chip.id),
        // The die's SRAM, straight from the metadata's DRAM region. The FLASH
        // stays unknown on purpose: it is a separate SPI part chosen by whoever
        // built the module, so the same die ships as 2, 4, 8 or 16 MB.
        sram_kb: Some(chip.dram_bytes / 1024),
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
        // Stored as a graph rather than a new `ClockDef` variant per chip: the
        // shape is the same for all of them and only the numbers differ, so a
        // variant would be three names for one topology. `None` where the two
        // sources cannot be reconciled — see [`clock_graph`].
        clock: match clock_graph(chip) {
            Some(graph) => super::mcu_def::ClockDef::Graph(super::clock::graph::GraphClock {
                layout: super::clock::graph::auto_layout::auto_layout(&graph),
                graph,
                bindings: Default::default(),
            }),
            None => super::mcu_def::ClockDef::None,
        },
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

    /// A datasheet table is typed in by hand, so it gets checked like one.
    ///
    /// Every pad numbered once, from 1 with no gaps, and four equal sides — the
    /// three ways a transcription goes wrong that produce a plausible-looking
    /// chip rather than a crash. A package that numbers its exposed thermal pad
    /// (the C6 calls it pin 41) fails the divisibility check, which is exactly
    /// how it should be caught: that pad is not a side pin.
    #[test]
    fn every_package_table_is_a_complete_numbering() {
        for pkg in PACKAGES {
            let numbers: Vec<u8> = pkg.pads.iter().map(|(n, _)| *n).collect();
            let want: Vec<u8> = (1..=pkg.pads.len() as u8).collect();
            assert_eq!(
                numbers,
                want,
                "{} ({}): pins are not 1..={}",
                pkg.chip,
                pkg.name,
                pkg.pads.len()
            );
            assert_eq!(
                pkg.pads.len() % 4,
                0,
                "{} ({}): {} pads do not divide over four sides",
                pkg.chip,
                pkg.name,
                pkg.pads.len()
            );
        }
    }

    /// Every chip is described exactly once, and never one that is hand-written.
    #[test]
    fn no_chip_is_described_twice_or_by_two_sources() {
        let mut seen = BTreeSet::new();
        for pkg in PACKAGES {
            assert!(seen.insert(pkg.chip), "{} appears twice", pkg.chip);
            assert_ne!(
                pkg.chip, "esp32c3",
                "the C3 is hand-written; a table here would fight it"
            );
        }
    }

    /// The transcribed tables and the vendor metadata must name the same GPIOs.
    ///
    /// This is the check that matters. A mistyped number would silently turn a
    /// usable pad into a reserved one (the name would not resolve to a GPIO) or
    /// move a peripheral onto the wrong pin, and nothing downstream would
    /// notice. Everything the package leaves out has to be declared in
    /// `off_package` with a reason — see [`Package::off_package`].
    #[test]
    #[ignore]
    fn every_package_names_exactly_the_gpios_its_metadata_has() {
        let dir = esp_metadata::vendor_dir().expect("esp-metadata in the cargo registry");
        for pkg in PACKAGES {
            let c =
                esp_metadata::load(&dir, pkg.chip).unwrap_or_else(|e| panic!("{}: {e}", pkg.chip));
            let on_package: BTreeSet<u8> = pkg
                .pads
                .iter()
                .filter_map(|(_, n)| n.strip_prefix("GPIO")?.parse().ok())
                .collect();
            let on_die: BTreeSet<u8> = c.gpios.iter().map(|g| g.number).collect();
            assert!(
                on_package.is_subset(&on_die),
                "{}: the table names a GPIO the chip does not have: {:?}",
                pkg.chip,
                on_package.difference(&on_die).collect::<Vec<_>>()
            );
            assert_eq!(
                on_die.difference(&on_package).copied().collect::<Vec<_>>(),
                pkg.off_package.to_vec(),
                "{} ({}): what the package leaves out does not match off_package",
                pkg.chip,
                pkg.name
            );
        }
    }

    /// Every packaged chip draws as its real part, not as N pins numbered 1..N.
    #[test]
    #[ignore]
    fn every_packaged_chip_carries_its_datasheet_pinout() {
        let dir = esp_metadata::vendor_dir().expect("esp-metadata in the cargo registry");
        for pkg in PACKAGES {
            let c = esp_metadata::load(&dir, pkg.chip).unwrap();
            let def = definition(&c).unwrap_or_else(|e| panic!("{}: {e}", pkg.chip));
            assert_eq!(def.package, pkg.name, "{}", pkg.chip);
            let per = pkg.pads.len() / 4;
            let sides = [
                &def.pins.left,
                &def.pins.bottom,
                &def.pins.right,
                &def.pins.top,
            ];
            assert!(
                sides.iter().all(|s| s.len() == per),
                "{}: sides are not {per} pads each",
                pkg.chip
            );
            // Counter-clockwise from the top of the left edge, exactly as the
            // hand-written C3 is arranged: the two far sides run backwards
            // because each is drawn top-to-bottom or left-to-right.
            assert_eq!(def.pins.left[0].number, 1, "{}", pkg.chip);
            assert_eq!(def.pins.bottom[0].number, per + 1, "{}", pkg.chip);
            assert_eq!(def.pins.right[0].number, 3 * per, "{}", pkg.chip);
            assert_eq!(def.pins.top[0].number, 4 * per, "{}", pkg.chip);
            // A supply rail is on the drawing and unassignable; every GPIO the
            // package does bond stays assignable.
            for p in sides.iter().flat_map(|s| s.iter()) {
                let is_gpio = p.name.starts_with("GPIO");
                assert_eq!(
                    p.reserved, !is_gpio,
                    "{}: pin {} ({}) has the wrong reserved flag",
                    pkg.chip, p.number, p.name
                );
                assert_eq!(
                    p.functions.is_empty(),
                    !is_gpio,
                    "{}: pin {} ({}) has the wrong function list",
                    pkg.chip,
                    p.number,
                    p.name
                );
            }
        }
    }

    /// The ESP32-C5 in detail — the part whose datasheet started all this.
    #[test]
    #[ignore]
    fn the_c5_carries_its_datasheet_pinout() {
        let def = definition(&chip("esp32c5")).expect("c5 generates");
        assert_eq!(def.package, "QFN48");
        let pad = |name: &str| {
            def.pins
                .left
                .iter()
                .chain(&def.pins.bottom)
                .chain(&def.pins.right)
                .chain(&def.pins.top)
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{name} is not on the package"))
        };
        assert_eq!(pad("VDDA6").number, 1);
        assert_eq!(pad("GPIO0").number, 9, "GPIO0 is package pin 9");
        assert!(pad("VDD_SPI").reserved && pad("VDD_SPI").functions.is_empty());
        assert!(pad("ANT_5G").reserved);
        assert!(!pad("GPIO16").reserved, "the flash bus stays usable");
    }

    #[test]
    fn an_instance_number_comes_from_the_name_not_a_counter() {
        // SPI0 and SPI1 belong to the flash controller, so the first SPI a user
        // can have is SPI2 — and it must be called 2.
        assert_eq!(instance_number("SPI2"), Some(2));
        assert_eq!(instance_number("SPI3"), Some(3));
        assert_eq!(instance_number("LEDC"), None);
    }

    /// The Xtensa parts, and the one file that makes them buildable.
    #[test]
    #[ignore]
    fn xtensa_chips_ship_with_the_esp_toolchain_pinned() {
        let dir = esp_metadata::vendor_dir();
        let Some(dir) = dir else { return };
        for (id, target) in [
            ("esp32", "xtensa-esp32-none-elf"),
            ("esp32s2", "xtensa-esp32s2-none-elf"),
            ("esp32s3", "xtensa-esp32s3-none-elf"),
        ] {
            let c = esp_metadata::load(&dir, id).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(c.arch.needs_esp_toolchain(), "{id}");
            let d = definition(&c).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(d.project.target, target);
            assert_eq!(d.cpu, "Xtensa LX");
            let pads = pad_functions(&d);
            println!(
                "{id:<8} {:>2} pads  {:>3} MHz  {:?} SRAM KiB  target {target}",
                pads.len(),
                d.max_mhz.unwrap_or(0),
                d.sram_kb,
            );
            assert!(!pads.is_empty(), "{id} has no usable pad");
            // Their metadata states no PLL that divides into 240 MHz, so the
            // graph refuses - and refusing is the designed answer.
            assert_eq!(
                d.clock,
                super::super::mcu_def::ClockDef::None,
                "{id} grew a clock tree — recheck `clock_graph`"
            );
            // Selectable — and the generated project must carry the toolchain
            // pin, or every cargo the IDE launches would use stock rustc and
            // fail inside `core`.
            let def = super::super::builtins::builtin_for(id)
                .unwrap_or_else(|| panic!("{id} is not a built-in"));
            let files = super::super::project_gen::build_project_files(
                &def.project,
                &def.toolchain,
                "fn main() {}",
            );
            assert!(
                files.rust_toolchain.contains("channel = \"esp\""),
                "{id} has no toolchain pin:
{}",
                files.rust_toolchain
            );
        }

        // …and a RISC-V part must NOT get one: it builds on stable, and pinning
        // it to a channel it does not need would break on the next upgrade.
        for id in ["esp32c3", "esp32c6"] {
            let def = super::super::builtins::builtin_for(id).unwrap();
            let files = super::super::project_gen::build_project_files(
                &def.project,
                &def.toolchain,
                "fn main() {}",
            );
            assert!(files.rust_toolchain.is_empty(), "{id} was pinned");
        }
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
    /// Every Espressif part except the C3, which keeps its hand-written
    /// definition.
    ///
    /// The C5 and C61 were held back for a while: the generated `Cargo.toml`
    /// pinned `esp-println = "0.13"`, which has no feature for either. That was
    /// a pin of ours, not a gap upstream — `esp-println` 0.17 carries both.
    ///
    /// The three Xtensa parts were held back for a better reason — nothing could
    /// invoke Espressif's fork of rustc — until the generated project started
    /// carrying a `rust-toolchain.toml`, which makes every `cargo` inside it
    /// use that fork without the IDE knowing anything about it.
    const GENERATED: [&str; 8] = [
        "esp32", "esp32c2", "esp32c5", "esp32c6", "esp32c61", "esp32h2", "esp32s2", "esp32s3",
    ];

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

    /// The graph must be able to produce EVERY frequency `esp-hal` can name, and
    /// nothing else.
    ///
    /// This is the whole safety argument for deriving a clock tree. The PLL and
    /// crystal come from Espressif's metadata; the CPU options come from
    /// `esp-hal`. They are independent sources, and the divider is what has to
    /// reconcile them — so evaluating the graph at each divider setting and
    /// getting back exactly `cpu_options` proves both readings agree about the
    /// chip. A tree that showed a frequency the generator cannot emit would be
    /// worse than the honest `ClockDef::None` it replaced.
    #[test]
    #[ignore]
    fn a_derived_tree_produces_exactly_the_frequencies_esp_hal_can_name() {
        use super::super::clock::graph::evaluate;
        use super::super::clock::graph::model::NodeState;

        let dir = esp_metadata::vendor_dir().expect("esp-metadata in the cargo registry");
        for id in GENERATED {
            let c = esp_metadata::load(&dir, id).unwrap();
            // The Xtensa parts state no PLL that divides into 240 MHz, so they
            // get no tree at all — asserted in `xtensa_chips_ship_…`, not here.
            let Some(mut graph) = clock_graph(&c) else {
                assert!(c.arch.needs_esp_toolchain(), "{id}: no clock graph");
                continue;
            };
            let want = super::super::esp_clocks::cpu_options(id);

            let divs = graph
                .nodes
                .iter()
                .find(|n| n.id == "cpu_div")
                .map(|n| match &n.kind {
                    super::NodeKind::Divider { options } => options.clone(),
                    other => panic!("{id}: cpu_div is {other:?}"),
                })
                .expect("a cpu_div node");
            assert_eq!(divs.len(), want.len(), "{id}: {divs:?} vs {want:?}");

            let mut got: Vec<u32> = Vec::new();
            for ix in 0..divs.len() {
                for node in &mut graph.nodes {
                    if node.id == "cpu_div" {
                        node.state = NodeState::Index(ix);
                    }
                }
                let hz = evaluate(&graph).get("cpu").copied().unwrap_or(0);
                assert_eq!(hz % 1_000_000, 0, "{id}: {hz} Hz is not a whole MHz");
                got.push(hz / 1_000_000);
            }
            got.sort_unstable();
            println!(
                "{id:<9} xtal {:?} MHz  PLL {} MHz  divisors {divs:?}  ->  CPU {got:?} MHz",
                c.xtal_hz.iter().map(|h| h / 1_000_000).collect::<Vec<_>>(),
                c.pll_hz.unwrap() / 1_000_000,
            );
            assert_eq!(got, want, "{id}: the graph and esp-hal disagree");
        }
    }

    /// A chip whose sources cannot be reconciled gets no tree, not a wrong one.
    #[test]
    fn an_unreconcilable_chip_gets_no_graph() {
        let dir = esp_metadata::vendor_dir();
        let Some(dir) = dir else { return };
        let Ok(mut c) = esp_metadata::load(&dir, "esp32c6") else {
            return;
        };
        assert!(clock_graph(&c).is_some(), "the real C6 should reconcile");
        // A PLL that does not divide into 160 MHz cannot be this chip's.
        c.pll_hz = Some(333_000_000);
        assert!(clock_graph(&c).is_none(), "built a tree from a bad PLL");
        // …and neither can no PLL at all.
        c.pll_hz = None;
        assert!(clock_graph(&c).is_none());
    }

    /// Input-only pads must not be offered as outputs.
    ///
    /// The ESP32's GPIO34..39 can only read. The vendor states it with an EMPTY
    /// second capability group — `GPIO34() () ([Input] [])` — which a reader
    /// that takes the number and skips the brackets never sees. The generated
    /// project then said `Output::new(peripherals.GPIO34)` and the compiler
    /// rejected it on `PeripheralOutput`. That is how this was found; this is so
    /// it stays found.
    #[test]
    #[ignore]
    fn input_only_pads_are_not_offered_as_outputs() {
        let c = chip("esp32");
        for n in 34..=39u8 {
            let pad = c.gpios.iter().find(|g| g.number == n).expect("pad exists");
            assert!(pad.input && !pad.output, "GPIO{n}: {pad:?}");
        }
        let d = definition(&c).unwrap();
        let pads = pad_functions(&d);
        for n in 34..=39u8 {
            let f = &pads[&format!("GPIO{n}")];
            assert!(f.contains("GpioInput"), "GPIO{n} lost its input");
            // EVERY driving function, not just GPIO output. The first fix
            // covered only that, and the next build put a UART TX, an I2C pair
            // and a PWM channel on these same pads.
            for driver in [
                "GpioOutput",
                "UsartTx",
                "SpiSck",
                "SpiMosi",
                "SpiNss",
                "I2cScl",
                "I2cSda",
                "TimerPwm",
                "CanTx",
                "UsbDm",
                "UsbDp",
            ] {
                assert!(
                    !f.iter().any(|x| x.starts_with(driver)),
                    "GPIO{n} is offered {driver}, but it cannot drive"
                );
            }
            // …and the reading half is still there.
            assert!(
                f.iter().any(|x| x.starts_with("UsartRx")),
                "GPIO{n} lost UsartRx"
            );
        }
        // …and a pad that CAN drive still can.
        let f = &pads["GPIO33"];
        assert!(f.contains("GpioOutput") && f.contains("GpioInput"));

        // Every RISC-V part drives every pad, so nothing there changed.
        for id in ["esp32c3", "esp32c6"] {
            let c = chip(id);
            assert!(
                c.gpios.iter().all(|g| g.input && g.output),
                "{id} has a one-way pad — recheck"
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
