//! Raspberry Pi Pico / Pico 2 — `rp2040-hal` and `rp235x-hal`, blocking.
//!
//! These are BOARDS, not bare chips: the pin numbers are the 40-pin header's,
//! because that is what is silkscreened and what a user counts to. GP23/24/25/29
//! are not on the header but are on the board, and GP25 drives the LED.
//!
//! Two things make this family unlike every other one here.
//!
//! **The chip cannot boot from flash on its own.** On RP2040 the boot ROM copies
//! 256 bytes from the start of flash into RAM and runs them; that stage sets up
//! the QSPI flash for execute-in-place. It is a linked artifact, not code the
//! user writes, so it is generated inside the marked block. RP2350 replaced it
//! with an image block the HAL supplies.
//!
//! **There are two cores.** That is why the generated `Cargo.toml` does not
//! enable `cortex-m/critical-section-single-core` — see `cargo_toml_rp`.

use super::common::{GEN_BEGIN, GEN_END, USER_TAIL, mcu_id_marker_line, var_suffix};
use super::family::FamilyBackend;
use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::pins::PinFunction;
use crate::panels::mcu_module::pins::logic::pin::GpioMode;

pub struct RpBackend;

/// The two families this backend serves.
pub fn is_rp(family: &str) -> bool {
    matches!(family, "rp2040" | "rp235x")
}

/// `rp2040_hal` / `rp235x_hal` — the crate name as it is written in Rust.
fn hal_crate(family: &str) -> &'static str {
    if family == "rp2040" {
        "rp2040_hal"
    } else {
        "rp235x_hal"
    }
}

/// `GP13` -> `13`. The definition names header pins after the GPIO they carry,
/// so the number in the name IS the GPIO index; power and ground pins have no
/// digits and are reserved anyway.
fn gpio_index(name: &str) -> Option<u8> {
    name.strip_prefix("GP")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// The GPIO bindings, in header order.
/// A pad the user wired but has not written code for yet is not a mistake -
/// it is a pad they are about to use. Same words the F1 backend uses.
const ALLOW: &str = "    #[allow(unused_mut, unused_variables)]
";

fn gpio_lines(mcu: &Mcu) -> String {
    let mut out = String::new();
    for p in mcu.iter_all_pins().filter(|p| !p.reserved) {
        let Some(n) = gpio_index(&p.name) else {
            continue;
        };
        let sfx = var_suffix(&p.selected_function);
        match p.selected_function {
            PinFunction::GpioOutput => out.push_str(&format!(
                "{ALLOW}    let mut gp{n}{sfx} = pins.gpio{n}.into_push_pull_output();\n"
            )),
            PinFunction::GpioInput => out.push_str(&format!(
                "{ALLOW}    let gp{n}{sfx} = pins.gpio{n}.into_pull_up_input();\n"
            )),
            _ => {}
        }
    }
    out
}

/// The boot stage, which differs between the two chips and is not optional on
/// either.
fn boot_block(family: &str) -> String {
    if family == "rp2040" {
        "/// Second-stage bootloader. The boot ROM copies these 256 bytes from the\n\
         /// start of flash into RAM and runs them, and they set up the QSPI flash\n\
         /// for execute-in-place. Without it the chip boots into nothing.\n\
         #[link_section = \".boot2\"]\n\
         #[used]\n\
         pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_W25Q080;\n"
            .to_owned()
    } else {
        "/// The image block the RP2350 boot ROM looks for. It replaces RP2040's\n\
         /// second-stage bootloader and is supplied by the HAL.\n\
         #[link_section = \".start_block\"]\n\
         #[used]\n\
         pub static IMAGE_DEF: rp235x_hal::block::ImageDef = rp235x_hal::block::ImageDef::secure_exe();\n"
            .to_owned()
    }
}

/// One PLL's three numbers, straight off the tree.
///
/// `PLLConfig` is exactly FBDIV / POSTDIV1 / POSTDIV2 plus the VCO those imply,
/// which is why the graph models them separately rather than as one opaque
/// "PLL": every field here is a node the user can see and change.
struct Pll {
    vco_mhz: u32,
    pd1: u32,
    pd2: u32,
}

/// The post-divider options, in the order the graph lists them.
const PD: [u32; 7] = [1, 2, 3, 4, 5, 6, 7];

fn pll_from(mcu: &Mcu, prefix: &str, xtal_hz: u32) -> Pll {
    use crate::panels::mcu_module::clock::graph::model::NodeState;
    use crate::panels::mcu_module::clock::model::ClockConfig;
    let ClockConfig::Graph(gc) = &mcu.clock else {
        // A chip with no tree cannot answer; the caller's defaults stand.
        return Pll {
            vco_mhz: 1500,
            pd1: 6,
            pd2: 2,
        };
    };
    let value = |id: String| match gc.graph.node(&id).map(|n| &n.state) {
        Some(NodeState::Value(v)) => Some(*v),
        Some(NodeState::Index(i)) => PD.get(*i).copied(),
        _ => None,
    };
    let fb = value(format!("{prefix}_fb")).unwrap_or(125);
    Pll {
        vco_mhz: xtal_hz / 1_000_000 * fb,
        pd1: value(format!("{prefix}_pd1")).unwrap_or(6),
        pd2: value(format!("{prefix}_pd2")).unwrap_or(2),
    }
}

/// The crystal, as the tree states it.
fn xtal_hz(mcu: &Mcu) -> u32 {
    use crate::panels::mcu_module::clock::graph::model::NodeState;
    use crate::panels::mcu_module::clock::model::ClockConfig;
    let ClockConfig::Graph(gc) = &mcu.clock else {
        return 12_000_000;
    };
    match gc.graph.node("xosc").map(|n| &n.state) {
        Some(NodeState::Source { hz, .. }) => *hz,
        _ => 12_000_000,
    }
}

/// Which GPIO carries each role of one bus instance.
///
/// The definition already says it — every header pin lists the SPI/UART/I2C
/// role its FUNCSEL table gives it — so the backend never has to know the table
/// itself, only how to read the wiring back out.
fn bus_pins(
    mcu: &Mcu,
    want: impl Fn(&PinFunction) -> Option<(u8, &'static str)>,
) -> Vec<(u8, &'static str, u8)> {
    let mut out = Vec::new();
    for p in mcu.iter_all_pins().filter(|p| !p.reserved) {
        let Some(n) = gpio_index(&p.name) else {
            continue;
        };
        if let Some((inst, role)) = want(&p.selected_function) {
            out.push((inst, role, n));
        }
    }
    out.sort_unstable();
    out
}

/// One instance's pin for a role, if it is wired.
fn role_of(pins: &[(u8, &'static str, u8)], inst: u8, role: &str) -> Option<u8> {
    pins.iter()
        .find(|(i, r, _)| *i == inst && *r == role)
        .map(|(_, _, n)| *n)
}

/// The instances that have any pin at all, ascending.
fn instances(pins: &[(u8, &'static str, u8)]) -> Vec<u8> {
    let mut v: Vec<u8> = pins.iter().map(|(i, _, _)| *i).collect();
    v.dedup();
    v
}

/// The signal a pin carries, as one name: `"UART0 TX"`, `"PWM3 A"`.
///
/// Two pads CAN claim the same signal on this chip — GP0 and GP16 are both
/// UART0 TX, GP4 and GP20 are both UART1 TX, and so on the whole way up. That
/// is the FUNCSEL table, not a mistake in the definition.
fn signal_name(f: &PinFunction) -> Option<String> {
    Some(match f {
        PinFunction::UsartTx(i) => format!("UART{i} TX"),
        PinFunction::UsartRx(i) => format!("UART{i} RX"),
        PinFunction::SpiSck(i) => format!("SPI{i} SCK"),
        PinFunction::SpiMosi(i) => format!("SPI{i} TX"),
        PinFunction::SpiMiso(i) => format!("SPI{i} RX"),
        PinFunction::I2cSda(i) => format!("I2C{i} SDA"),
        PinFunction::I2cScl(i) => format!("I2C{i} SCL"),
        PinFunction::TimerPwm { timer, channel } => {
            format!("PWM{timer} {}", if *channel == 1 { "A" } else { "B" })
        }
        _ => return None,
    })
}

/// Say so when two pads claim one signal.
///
/// rp-hal takes ONE pin per role, so only the lowest-numbered pad can be
/// configured — and the code that did so simply used the first it found and
/// dropped the rest without a word. The project then built, ran, and left a pad
/// the user had deliberately wired doing nothing, with nothing anywhere saying
/// why.
///
/// The generator does not choose between them: it takes the lowest so the output
/// is stable, and names both so the person who wired them can decide.
fn ambiguity_notes(mcu: &Mcu) -> String {
    let mut by_signal: std::collections::BTreeMap<String, Vec<u8>> =
        std::collections::BTreeMap::new();
    for p in mcu.iter_all_pins().filter(|p| !p.reserved) {
        let (Some(n), Some(sig)) = (gpio_index(&p.name), signal_name(&p.selected_function)) else {
            continue;
        };
        by_signal.entry(sig).or_default().push(n);
    }
    let mut o = String::new();
    for (sig, mut pads) in by_signal {
        if pads.len() < 2 {
            continue;
        }
        pads.sort_unstable();
        let used = pads[0];
        let rest: Vec<String> = pads[1..].iter().map(|n| format!("GP{n}")).collect();
        o.push_str(&format!(
            "    // {sig} is wired to GP{used} and {}. Only GP{used} is configured:\n",
            rest.join(" and ")
        ));
        o.push_str(
            "    // the HAL takes one pin per role. Unassign the other on the Pins canvas.\n",
        );
    }
    o
}

fn uart_pins(mcu: &Mcu) -> Vec<(u8, &'static str, u8)> {
    bus_pins(mcu, |f| match f {
        PinFunction::UsartTx(i) => Some((*i, "tx")),
        PinFunction::UsartRx(i) => Some((*i, "rx")),
        _ => None,
    })
}

fn spi_pins(mcu: &Mcu) -> Vec<(u8, &'static str, u8)> {
    bus_pins(mcu, |f| match f {
        PinFunction::SpiSck(i) => Some((*i, "sck")),
        PinFunction::SpiMosi(i) => Some((*i, "mosi")),
        PinFunction::SpiMiso(i) => Some((*i, "miso")),
        _ => None,
    })
}

fn i2c_pins(mcu: &Mcu) -> Vec<(u8, &'static str, u8)> {
    bus_pins(mcu, |f| match f {
        PinFunction::I2cSda(i) => Some((*i, "sda")),
        PinFunction::I2cScl(i) => Some((*i, "scl")),
        _ => None,
    })
}

/// UART, SPI and I2C, in that order.
///
/// Each is emitted only when BOTH of its required pads are wired. rp-hal's
/// constructors take the pins by value in a fixed order and there is no
/// `NoPin` — so half a bus is not a smaller bus here, it is a type error.
fn bus_lines(mcu: &Mcu, _hal: &str) -> String {
    let mut o = ambiguity_notes(mcu);

    let uart = uart_pins(mcu);
    for i in instances(&uart) {
        let (Some(tx), Some(rx)) = (role_of(&uart, i, "tx"), role_of(&uart, i, "rx")) else {
            o.push_str(&format!(
                "    // UART{i}: only one of TX/RX is wired, and the constructor takes the\n    // pair. Wire the other pad on the Pins canvas.\n"
            ));
            continue;
        };
        o.push_str(&format!(
            "    let uart{i} = pins::configs::uart{i}::init(\n        pac.UART{i},\n        pins.gpio{tx},\n        pins.gpio{rx},\n        &mut pac.RESETS,\n        clocks.peripheral_clock.freq(),\n    );\n    let _ = &uart{i};\n"
        ));
    }

    let spi = spi_pins(mcu);
    for i in instances(&spi) {
        let (Some(sck), Some(mosi), Some(miso)) = (
            role_of(&spi, i, "sck"),
            role_of(&spi, i, "mosi"),
            role_of(&spi, i, "miso"),
        ) else {
            o.push_str(&format!(
                "    // SPI{i}: the constructor takes (MOSI, MISO, SCK) together, so all\n    // three pads have to be wired before anything can be built.\n"
            ));
            continue;
        };
        o.push_str(&format!(
            "    let spi{i} = pins::configs::spi{i}::init(\n        pac.SPI{i},\n        pins.gpio{mosi},\n        pins.gpio{miso},\n        pins.gpio{sck},\n        &mut pac.RESETS,\n        clocks.peripheral_clock.freq(),\n    );\n    let _ = &spi{i};\n"
        ));
    }

    let i2c = i2c_pins(mcu);
    for i in instances(&i2c) {
        let (Some(sda), Some(scl)) = (role_of(&i2c, i, "sda"), role_of(&i2c, i, "scl")) else {
            o.push_str(&format!(
                "    // I2C{i}: SDA and SCL are taken together; wire the missing one.\n"
            ));
            continue;
        };
        o.push_str(&format!(
            "    let i2c{i} = pins::configs::i2c{i}::init(\n        pac.I2C{i},\n        pins.gpio{sda},\n        pins.gpio{scl},\n        &mut pac.RESETS,\n        &clocks.system_clock,\n    );\n    let _ = &i2c{i};\n"
        ));
    }
    o
}

/// PWM slices and ADC inputs.
///
/// A slice is not indexable on rp-hal — `Slices` exposes `pwm0`..`pwm7` as
/// FIELDS — so the generated code names each one. Channel A is the even GPIO of
/// the pair and B the odd one, which is why the definition stores the slice as
/// the timer and 1/2 as the channel.
fn pwm_adc_lines(mcu: &Mcu, hal: &str) -> String {
    let mut o = String::new();

    let mut pwm: Vec<(u8, u8, u8)> = Vec::new();
    let mut adc: Vec<(u8, u8)> = Vec::new();
    for p in mcu.iter_all_pins().filter(|p| !p.reserved) {
        let Some(n) = gpio_index(&p.name) else {
            continue;
        };
        match p.selected_function {
            PinFunction::TimerPwm { timer, channel } => pwm.push((timer, channel, n)),
            PinFunction::AdcChannel { channel, .. } => adc.push((channel, n)),
            _ => {}
        }
    }
    pwm.sort_unstable();
    adc.sort_unstable();

    if !pwm.is_empty() {
        o.push_str(
            "    // All eight slices come from one PWM peripheral, so main.rs owns
",
        );
        o.push_str(
            "    // the set and lends each wired one to its config module.
",
        );
        o.push_str(&format!(
            "    let mut pwm_slices = {hal}::pwm::Slices::new(pac.PWM, &mut pac.RESETS);
"
        ));
        let mut by_slice: std::collections::BTreeMap<u8, Vec<(u8, u8)>> =
            std::collections::BTreeMap::new();
        for (slice, channel, n) in &pwm {
            by_slice.entry(*slice).or_default().push((*channel, *n));
        }
        for (slice, chans) in &by_slice {
            let args: Vec<String> = chans.iter().map(|(_, n)| format!("pins.gpio{n}")).collect();
            o.push_str(&format!(
                "    pins::configs::pwm{slice}::init(&mut pwm_slices.pwm{slice}, {});
",
                args.join(", ")
            ));
        }
    }

    if !adc.is_empty() {
        o.push_str(&format!(
            "    let mut adc = {hal}::adc::Adc::new(pac.ADC, &mut pac.RESETS);\n"
        ));
        o.push_str("    let _ = &mut adc;\n");
        for (channel, n) in &adc {
            o.push_str(&format!(
                "    // ADC{channel}, on GP{n}. Read it with `adc.read(&mut adc{channel})`.\n"
            ));
            o.push_str(&format!(
                "    let mut adc{channel} = {hal}::adc::AdcPin::new(pins.gpio{n}).unwrap();\n"
            ));
            o.push_str(&format!("    let _ = &mut adc{channel};\n"));
        }
    }
    o
}

/// The generated region: boot stage, clocks, the GPIO bank, and the pins.
///
/// Built line by line rather than as one continued literal: rustfmt joins a
/// `\`-continued string back onto one physical line and the continuation turns
/// into a run of spaces inside the generated file.
fn section(mcu: &Mcu) -> String {
    let hal = hal_crate(&mcu.family);
    let xtal = xtal_hz(mcu);
    let sys = pll_from(mcu, "pll_sys", xtal);
    let usb = pll_from(mcu, "pll_usb", xtal);
    let mut o = String::new();
    o.push_str(GEN_BEGIN);
    o.push('\n');
    o.push_str(&boot_block(&mcu.family));
    o.push('\n');

    o.push_str("/// The crystal on the board, and each PLL as the Clock tab has it.\n");
    o.push_str(&format!("const XTAL_FREQ_HZ: u32 = {xtal};\n\n"));
    for (name, cfg, what) in [
        ("PLL_SYS_CFG", &sys, "the system clock"),
        ("PLL_USB_CFG", &usb, "USB, which needs exactly 48 MHz"),
    ] {
        o.push_str(&format!(
            "/// {what}: {} MHz VCO / {} / {} = {} MHz.\n",
            cfg.vco_mhz,
            cfg.pd1,
            cfg.pd2,
            cfg.vco_mhz / cfg.pd1 / cfg.pd2
        ));
        o.push_str(&format!(
            "const {name}: {hal}::pll::PLLConfig = {hal}::pll::PLLConfig {{\n"
        ));
        o.push_str(&format!(
            "    vco_freq: {hal}::fugit::HertzU32::MHz({}),\n",
            cfg.vco_mhz
        ));
        o.push_str("    refdiv: 1,\n");
        o.push_str(&format!("    post_div1: {},\n", cfg.pd1));
        o.push_str(&format!("    post_div2: {},\n", cfg.pd2));
        o.push_str("};\n\n");
    }

    o.push_str(&format!("#[{hal}::entry]\n"));
    o.push_str("fn main() -> ! {\n");
    o.push_str(&format!(
        "    let mut pac = {hal}::pac::Peripherals::take().unwrap();\n"
    ));
    o.push_str(&format!(
        "    let mut watchdog = {hal}::Watchdog::new(pac.WATCHDOG);\n"
    ));
    o.push_str("\n");
    o.push_str("    // Built from the Clock tab, not from the HAL's fixed default: the two\n");
    o.push_str("    // PLLConfigs above are this tree's FBDIV and POSTDIV values.\n");
    o.push_str("    //\n");
    o.push_str("    // `map_err(|_| false)` because these error types carry no Debug, so\n");
    o.push_str("    // `.unwrap()` alone cannot name them.\n");
    o.push_str(&format!(
        "    let xosc = {hal}::xosc::setup_xosc_blocking(\n"
    ));
    o.push_str("        pac.XOSC,\n");
    o.push_str(&format!(
        "        {hal}::fugit::HertzU32::Hz(XTAL_FREQ_HZ),\n"
    ));
    o.push_str("    )\n    .map_err(|_| false)\n    .unwrap();\n");
    o.push_str(&format!(
        "    let mut clocks = {hal}::clocks::ClocksManager::new(pac.CLOCKS);\n"
    ));
    for (var, peri, cfg) in [
        ("pll_sys", "PLL_SYS", "PLL_SYS_CFG"),
        ("pll_usb", "PLL_USB", "PLL_USB_CFG"),
    ] {
        o.push_str(&format!(
            "    let {var} = {hal}::pll::setup_pll_blocking(\n"
        ));
        o.push_str(&format!("        pac.{peri},\n"));
        o.push_str("        xosc.operating_frequency(),\n");
        o.push_str(&format!("        {cfg},\n"));
        o.push_str("        &mut clocks,\n        &mut pac.RESETS,\n    )\n");
        o.push_str("    .map_err(|_| false)\n    .unwrap();\n");
    }
    o.push_str("    clocks\n        .init_default(&xosc, &pll_sys, &pll_usb)\n");
    o.push_str("        .map_err(|_| false)\n        .unwrap();\n");
    o.push_str("    let _ = &mut watchdog;\n\n");

    o.push_str("    // Every GPIO comes from one bank, taken once.\n");
    o.push_str(&format!("    let sio = {hal}::Sio::new(pac.SIO);\n"));
    o.push_str("    #[allow(unused_variables)]\n");
    o.push_str(&format!("    let pins = {hal}::gpio::Pins::new(\n"));
    o.push_str("        pac.IO_BANK0,\n        pac.PADS_BANK0,\n        sio.gpio_bank0,\n        &mut pac.RESETS,\n    );\n\n");
    o.push_str(&gpio_lines(mcu));
    o.push_str(&bus_lines(mcu, hal));
    if radio_led(mcu) {
        // Not a gap in this backend — there IS no blocking path. `cyw43` is
        // async to the bottom: embassy-sync, embassy-time, embedded-hal-async
        // and a spawned runner task. Saying so beats emitting nothing.
        o.push_str(
            "    // WL_LED is driven, but this project is Blocking.
",
        );
        o.push_str(
            "    //
",
        );
        o.push_str(
            "    // The LED hangs off the CYW43 radio, and the radio's driver is async
",
        );
        o.push_str(
            "    // only - there is no blocking version of it to call. Switch Runtime to
",
        );
        o.push_str(
            "    // Async in the System tab and this becomes the wireless bring-up.
",
        );
    }
    o.push_str(&pwm_adc_lines(mcu, hal));
    o.push_str(GEN_END);
    o.push('\n');
    o
}

fn header(mcu: &Mcu) -> String {
    let hal = if mcu.family == "rp2040" {
        "rp2040-hal"
    } else {
        "rp235x-hal"
    };
    format!(
        "// Auto-generated by Embedded IDE\n\
         // MCU: {} | HAL: {hal} (blocking)\n\
         {}\n\
         #![no_std]\n\
         #![no_main]\n\
         \n\
         pub mod pins;\n\
         \n\
         use panic_halt as _;\n\
         #[allow(unused_imports)]\n\
         use embedded_hal::digital::{{InputPin, OutputPin}};\n\
         // `Clock` carries `.freq()`, which every bus constructor asks the\n\
         // peripheral clock for.\n\
         #[allow(unused_imports)]\n\
         use {hal_crate}::Clock;\n\
         // `SetDutyCycle` carries `max_duty_cycle` and `set_duty_cycle`; they\n\
         // are embedded-hal's, not rp-hal's own.\n\
         #[allow(unused_imports)]\n\
         use embedded_hal::pwm::SetDutyCycle;\n\
         \n",
        mcu.name,
        mcu_id_marker_line(&mcu.id),
        hal_crate = hal_crate(&mcu.family),
    )
}

/// `src/pins/configs/uart{n}.rs`, `spi{n}.rs`, `i2c{n}.rs`.
///
/// Each owns its peripheral outright — unlike PWM, where all eight slices come
/// from one block — so `init` takes it by value and hands back a `Handle` the
/// caller keeps.
fn bus_config_file(hal: &str, kind: &str, n: u8, pads: &[(&str, u8)]) -> String {
    let mut o = String::new();
    o.push_str("// <<< GENERATED>>>\n");
    o.push_str(
        "// Peripheral config (from the Virtual Module) — auto-updated; edit in the module.\n",
    );
    match kind {
        "uart" => o.push_str("const BAUDRATE: u32 = 115_200;\n"),
        "spi" => o.push_str("const SPI_HZ: u32 = 1_000_000;\n"),
        _ => o.push_str("const I2C_HZ: u32 = 400_000;\n"),
    }
    o.push_str("// <<< GENERATED END >>>\n\n");
    o.push_str("// Everything below is editable — your changes are preserved on regeneration.\n");
    // No `use Clock` here: main.rs asks the clock for its frequency and passes
    // the value in, so these modules never touch the trait.
    let gpio = |n: u8| {
        format!(
            "{hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{n}, {hal}::gpio::Function{}, {hal}::gpio::PullDown>",
            match kind {
                "uart" => "Uart",
                "spi" => "Spi",
                _ => "I2c",
            }
        )
    };

    match kind {
        "uart" => {
            let tx = pads.iter().find(|(r, _)| *r == "tx").unwrap().1;
            let rx = pads.iter().find(|(r, _)| *r == "rx").unwrap().1;
            o.push_str(&format!(
                "\n/// The concrete type `init` hands back, so it can be a struct field.\npub type Handle = {hal}::uart::UartPeripheral<\n    {hal}::uart::Enabled,\n    {hal}::pac::UART{n},\n    ({}, {}),\n>;\n\n",
                gpio(tx), gpio(rx)
            ));
            o.push_str(&format!(
                "/// UART{n} on GP{tx} (TX) and GP{rx} (RX), at BAUDRATE.\npub fn init(\n    uart: {hal}::pac::UART{n},\n    tx: {hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{tx}, {hal}::gpio::FunctionNull, {hal}::gpio::PullDown>,\n    rx: {hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{rx}, {hal}::gpio::FunctionNull, {hal}::gpio::PullDown>,\n    resets: &mut {hal}::pac::RESETS,\n    peri_freq: {hal}::fugit::HertzU32,\n) -> Handle {{\n"
            ));
            o.push_str(&format!(
                "    {hal}::uart::UartPeripheral::new(uart, (tx.into_function(), rx.into_function()), resets)\n        .enable(\n            {hal}::uart::UartConfig::new(\n                {hal}::fugit::HertzU32::Hz(BAUDRATE),\n                {hal}::uart::DataBits::Eight,\n                None,\n                {hal}::uart::StopBits::One,\n            ),\n            peri_freq,\n        )\n        .unwrap()\n}}\n"
            ));
        }
        "spi" => {
            let sck = pads.iter().find(|(r, _)| *r == "sck").unwrap().1;
            let mosi = pads.iter().find(|(r, _)| *r == "mosi").unwrap().1;
            let miso = pads.iter().find(|(r, _)| *r == "miso").unwrap().1;
            o.push_str(&format!(
                "\n/// The concrete type `init` hands back.\npub type Handle = {hal}::spi::Spi<\n    {hal}::spi::Enabled,\n    {hal}::pac::SPI{n},\n    ({}, {}, {}),\n    8,\n>;\n\n",
                gpio(mosi), gpio(miso), gpio(sck)
            ));
            o.push_str(&format!(
                "/// SPI{n}: SCK GP{sck}, TX GP{mosi}, RX GP{miso}. `Spi::new` takes the three\n/// together and there is no `NoPin`, so a half-wired bus cannot be built.\npub fn init(\n    spi: {hal}::pac::SPI{n},\n    mosi: {hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{mosi}, {hal}::gpio::FunctionNull, {hal}::gpio::PullDown>,\n    miso: {hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{miso}, {hal}::gpio::FunctionNull, {hal}::gpio::PullDown>,\n    sck: {hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{sck}, {hal}::gpio::FunctionNull, {hal}::gpio::PullDown>,\n    resets: &mut {hal}::pac::RESETS,\n    peri_freq: {hal}::fugit::HertzU32,\n) -> Handle {{\n"
            ));
            o.push_str(&format!(
                "    {hal}::spi::Spi::<_, _, _, 8>::new(\n        spi,\n        (mosi.into_function(), miso.into_function(), sck.into_function()),\n    )\n    .init(\n        resets,\n        peri_freq,\n        {hal}::fugit::HertzU32::Hz(SPI_HZ),\n        embedded_hal::spi::MODE_0,\n    )\n}}\n"
            ));
        }
        _ => {
            let sda = pads.iter().find(|(r, _)| *r == "sda").unwrap().1;
            let scl = pads.iter().find(|(r, _)| *r == "scl").unwrap().1;
            let p = |n: u8| {
                format!(
                    "{hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{n}, {hal}::gpio::FunctionI2c, {hal}::gpio::PullUp>"
                )
            };
            o.push_str(&format!(
                "\n/// The concrete type `init` hands back.\npub type Handle = {hal}::i2c::I2C<{hal}::pac::I2C{n}, ({}, {})>;\n\n",
                p(sda), p(scl)
            ));
            o.push_str(&format!(
                "/// I2C{n} on GP{sda} (SDA) and GP{scl} (SCL), at I2C_HZ.\npub fn init(\n    i2c: {hal}::pac::I2C{n},\n    sda: {hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{sda}, {hal}::gpio::FunctionNull, {hal}::gpio::PullDown>,\n    scl: {hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{scl}, {hal}::gpio::FunctionNull, {hal}::gpio::PullDown>,\n    resets: &mut {hal}::pac::RESETS,\n    sys_clock: &{hal}::clocks::SystemClock,\n) -> Handle {{\n"
            ));
            o.push_str(&format!(
                "    {hal}::i2c::I2C::i2c{n}(\n        i2c,\n        sda.reconfigure(),\n        scl.reconfigure(),\n        {hal}::fugit::HertzU32::Hz(I2C_HZ),\n        resets,\n        sys_clock,\n    )\n}}\n"
            ));
        }
    }
    o
}

/// `src/pins/configs/pwm{slice}.rs` — one PWM slice, its frequency and the duty
/// of each channel it drives.
///
/// The slice itself is NOT taken by value: on this chip `Slices::new` hands out
/// all eight at once from one `PWM` peripheral, so `main.rs` owns the set and
/// lends one out. That is the opposite of STM32, where each timer is its own
/// peripheral and the config file can own it outright.
fn pwm_config_file(mcu: &Mcu, slice: u8, chans: &[(u8, u8)]) -> String {
    let hal = hal_crate(&mcu.family);
    let cfg = crate::panels::mcu_module::modules::timer_configs(&mcu.modules);
    let cfg = cfg.get(&slice);
    let mut o = String::new();

    o.push_str("// <<< GENERATED>>>\n");
    o.push_str(
        "// Peripheral config (from the Virtual Module) — auto-updated; edit in the module.\n",
    );
    o.push_str("// Duty per channel, in HUNDREDTHS of a percent — 750 is 7.5 %, which is what a\n");
    o.push_str("// hobby servo wants and what whole percent cannot say.\n");
    for (channel, _) in chans {
        let x100 = cfg.map_or(0, |c| c.duty_x100_of(*channel));
        let name = if *channel == 1 { "A" } else { "B" };
        o.push_str(&format!(
            "const DUTY_{name}_X100: u32 = {x100}; // {} %\n",
            super::common::duty_percent_str(x100)
        ));
    }
    o.push_str("// <<< GENERATED END >>>\n\n");

    o.push_str("// Everything below is editable — your changes are preserved on regeneration.\n");
    o.push_str(&format!(
        "use {hal}::pwm::{{FreeRunning, Pwm{slice}, Slice}};\n"
    ));
    o.push_str("use embedded_hal::pwm::SetDutyCycle;\n\n");

    o.push_str(&format!(
        "/// The slice this module drives. `main.rs` owns the whole set and lends\n/// this one out, because all eight come from one `PWM` peripheral.\npub type Handle = Slice<Pwm{slice}, FreeRunning>;\n\n"
    ));

    // init
    let params: Vec<String> = chans
        .iter()
        .map(|(channel, n)| {
            let name = if *channel == 1 { "a" } else { "b" };
            format!(
                "    gp{n}: {hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{n}, {hal}::gpio::FunctionNull, {hal}::gpio::PullDown>,\n    // channel {}\n",
                name.to_uppercase()
            )
        })
        .collect();
    o.push_str(&format!(
        "/// Enable the slice and route each wired channel to its pad.\npub fn init(\n    slice: &mut Handle,\n{}) {{\n",
        params.join("")
    ));
    o.push_str("    slice.set_ph_correct();\n    slice.enable();\n");
    for (channel, n) in chans {
        let ch = if *channel == 1 {
            "channel_a"
        } else {
            "channel_b"
        };
        let name = if *channel == 1 { "A" } else { "B" };
        o.push_str(&format!("    slice.{ch}.output_to(gp{n});\n"));
        o.push_str(&format!(
            "    let max = slice.{ch}.max_duty_cycle() as u32;\n"
        ));
        o.push_str(&format!(
            "    slice.{ch}.set_duty_cycle((max * DUTY_{name}_X100 / 10_000) as u16).unwrap();\n"
        ));
    }
    o.push_str("}\n\n");

    // DutyHandle, the same shape as every other backend.
    let first = chans.first().map_or(1, |(c, _)| *c);
    let first_name = if first == 1 { "a" } else { "b" };
    o.push_str("/// Set a channel's duty in the same units the `DUTY_*` constants above use —\n");
    o.push_str("/// HUNDREDTHS of a percent, so `10_000` is 100 % and `750` is 7.5 %.\n///\n");
    o.push_str("/// A trait rather than an inherent method because `Handle` is rp-hal's own\n");
    o.push_str("/// type, which this crate does not own. One method per WIRED channel: the\n");
    o.push_str("/// channel is part of the NAME rather than an argument, so a channel this\n");
    o.push_str("/// slice has no pad for cannot be asked for at all.\n");
    o.push_str("pub trait DutyHandle {\n");
    o.push_str(&format!(
        "    /// Channel {}, the first one wired to this slice.\n    fn set_duty_pwm_{slice}(&mut self, value: u32);\n",
        first_name.to_uppercase()
    ));
    for (channel, _) in chans {
        let name = if *channel == 1 { "a" } else { "b" };
        o.push_str(&format!(
            "\n    /// Channel {}.\n    fn set_duty_pwm_{slice}_{name}(&mut self, value: u32);\n",
            name.to_uppercase()
        ));
    }
    o.push_str("}\n\nimpl DutyHandle for Handle {\n");
    o.push_str(&format!(
        "    fn set_duty_pwm_{slice}(&mut self, value: u32) {{\n        self.set_duty_pwm_{slice}_{first_name}(value);\n    }}\n"
    ));
    for (channel, _) in chans {
        let name = if *channel == 1 { "a" } else { "b" };
        let ch = if *channel == 1 {
            "channel_a"
        } else {
            "channel_b"
        };
        o.push_str(&format!(
            "\n    fn set_duty_pwm_{slice}_{name}(&mut self, value: u32) {{\n"
        ));
        o.push_str(&format!(
            "        let max = self.{ch}.max_duty_cycle() as u32;\n"
        ));
        o.push_str(&format!(
            "        self.{ch}.set_duty_cycle((max * value / 10_000) as u16).unwrap();\n    }}\n"
        ));
    }
    o.push_str("}\n");
    o
}

impl FamilyBackend for RpBackend {
    fn family_id(&self) -> &'static str {
        "rp2040"
    }

    fn handles(&self, family: &str) -> bool {
        is_rp(family)
    }

    /// rp-hal picks pulls through `into_*_input` / `into_push_pull_output`,
    /// which this backend chooses for the user; offering modes it would ignore
    /// would be a lie.
    fn gpio_modes(&self, _func: &PinFunction) -> &'static [GpioMode] {
        &[]
    }

    /// One file per wired PWM slice.
    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        let mut by_slice: std::collections::BTreeMap<u8, Vec<(u8, u8)>> =
            std::collections::BTreeMap::new();
        for p in mcu.iter_all_pins().filter(|p| !p.reserved) {
            let Some(n) = gpio_index(&p.name) else {
                continue;
            };
            if let PinFunction::TimerPwm { timer, channel } = p.selected_function {
                by_slice.entry(timer).or_default().push((channel, n));
            }
        }
        let mut out: Vec<(String, String)> = by_slice
            .into_iter()
            .map(|(slice, mut chans)| {
                chans.sort_unstable();
                (
                    format!("pwm{slice}.rs"),
                    pwm_config_file(mcu, slice, &chans),
                )
            })
            .collect();

        // The three buses that own their peripheral. Only fully wired ones get a
        // file: `init` names every pad in its signature, so half a bus has no
        // signature to write.
        let hal = hal_crate(&mcu.family);
        for (kind, roles, pins) in [
            ("uart", &["tx", "rx"][..], uart_pins(mcu)),
            ("spi", &["sck", "mosi", "miso"][..], spi_pins(mcu)),
            ("i2c", &["sda", "scl"][..], i2c_pins(mcu)),
        ] {
            for i in instances(&pins) {
                let pads: Vec<(&str, u8)> = roles
                    .iter()
                    .filter_map(|r| role_of(&pins, i, r).map(|n| (*r, n)))
                    .collect();
                if pads.len() == roles.len() {
                    out.push((
                        format!("{kind}{i}.rs"),
                        bus_config_file(hal, kind, i, &pads),
                    ));
                }
            }
        }
        out
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        format!("{}{}{USER_TAIL}", header(mcu), section(mcu))
    }

    /// Replace ONLY the marked block, keeping what the user wrote on either
    /// side of it.
    ///
    /// Not `embassy_async::splice_section`: that one regenerates everything
    /// ABOVE the markers too, so a runtime switch can rewrite the imports. It is
    /// right for a backend with three runtimes and wrong for this one — reusing
    /// it here rebuilt the file with embassy-stm32's header on a Pico, and took
    /// the user's own `use` lines with it.
    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let (Some(begin), Some(end_start)) = (existing.find(GEN_BEGIN), existing.find(GEN_END))
        else {
            // No block to replace - the file is not ours, so start over rather
            // than splice into something unrecognised.
            return self.fresh_main_rs(mcu);
        };
        let end = end_start + GEN_END.len();
        format!(
            "{}{}{}",
            &existing[..begin],
            section(mcu).trim_end_matches('\n'),
            &existing[end..]
        )
    }
}

#[cfg(test)]
mod clock_authoring {
    use crate::panels::mcu_module::clock::graph::auto_layout::auto_layout;
    use crate::panels::mcu_module::clock::graph::config::GraphClock;
    use crate::panels::mcu_module::clock::graph::model::{
        ClockGraph, Edge, Node, NodeKind, NodeState,
    };

    /// The Pico clock tree, straight from the datasheet.
    ///
    /// `XOSC` is 12 MHz on both boards. Each PLL multiplies it into a VCO and
    /// then divides twice — that is genuinely how the silicon is arranged, and
    /// modelling it as one opaque "PLL" would make the numbers unexplainable in
    /// the Clock tab.
    ///
    /// `fb` / `pd1` / `pd2` are the datasheet's FBDIV, POSTDIV1 and POSTDIV2.
    fn rp_graph(sys_fb: u32, sys_pd1: usize, sys_pd2: usize) -> ClockGraph {
        let div = |opts: &[u32]| NodeKind::Divider {
            options: opts.to_vec(),
        };
        const PD: [u32; 7] = [1, 2, 3, 4, 5, 6, 7];
        ClockGraph {
            nodes: vec![
                Node {
                    id: "xosc".into(),
                    kind: NodeKind::Source {
                        min_hz: 12_000_000,
                        max_hz: 12_000_000,
                        gated: false,
                    },
                    state: NodeState::Source {
                        enabled: true,
                        hz: 12_000_000,
                    },
                    limit: None,
                },
                Node {
                    id: "pll_sys_fb".into(),
                    kind: NodeKind::Multiplier { min: 16, max: 320 },
                    state: NodeState::Value(sys_fb),
                    limit: None,
                },
                Node {
                    id: "pll_sys_pd1".into(),
                    kind: div(&PD),
                    state: NodeState::Index(sys_pd1),
                    limit: None,
                },
                Node {
                    id: "pll_sys_pd2".into(),
                    kind: div(&PD),
                    state: NodeState::Index(sys_pd2),
                    limit: None,
                },
                Node {
                    id: "clk_sys".into(),
                    kind: NodeKind::Output,
                    state: NodeState::Fixed,
                    limit: None,
                },
                Node {
                    id: "clk_peri".into(),
                    kind: NodeKind::Output,
                    state: NodeState::Fixed,
                    limit: None,
                },
                Node {
                    id: "pll_usb_fb".into(),
                    kind: NodeKind::Multiplier { min: 16, max: 320 },
                    state: NodeState::Value(100),
                    limit: None,
                },
                Node {
                    id: "pll_usb_pd1".into(),
                    kind: div(&PD),
                    state: NodeState::Index(4),
                    limit: None,
                },
                Node {
                    id: "pll_usb_pd2".into(),
                    kind: div(&PD),
                    state: NodeState::Index(4),
                    limit: None,
                },
                Node {
                    id: "clk_usb".into(),
                    kind: NodeKind::Output,
                    state: NodeState::Fixed,
                    limit: None,
                },
                Node {
                    id: "clk_ref".into(),
                    kind: NodeKind::Output,
                    state: NodeState::Fixed,
                    limit: None,
                },
            ],
            edges: vec![
                Edge {
                    from: "xosc".into(),
                    to: "pll_sys_fb".into(),
                    input: 0,
                },
                Edge {
                    from: "pll_sys_fb".into(),
                    to: "pll_sys_pd1".into(),
                    input: 0,
                },
                Edge {
                    from: "pll_sys_pd1".into(),
                    to: "pll_sys_pd2".into(),
                    input: 0,
                },
                Edge {
                    from: "pll_sys_pd2".into(),
                    to: "clk_sys".into(),
                    input: 0,
                },
                Edge {
                    from: "clk_sys".into(),
                    to: "clk_peri".into(),
                    input: 0,
                },
                Edge {
                    from: "xosc".into(),
                    to: "pll_usb_fb".into(),
                    input: 0,
                },
                Edge {
                    from: "pll_usb_fb".into(),
                    to: "pll_usb_pd1".into(),
                    input: 0,
                },
                Edge {
                    from: "pll_usb_pd1".into(),
                    to: "pll_usb_pd2".into(),
                    input: 0,
                },
                Edge {
                    from: "pll_usb_pd2".into(),
                    to: "clk_usb".into(),
                    input: 0,
                },
                Edge {
                    from: "xosc".into(),
                    to: "clk_ref".into(),
                    input: 0,
                },
            ],
        }
    }

    /// Write the `clock:` block for both boards, for splicing into their `.ron`.
    ///
    /// The FIGURE is not hand-placed: `auto_layout` derives it from the graph,
    /// the same way an imported CubeMX tree gets one.
    ///
    /// ```text
    /// cargo test --bin embedded_ide_0 emit_rp_clock_blocks -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "authoring tool: writes the clock blocks to the temp dir"]
    fn emit_rp_clock_blocks() {
        for (name, fb, pd1, pd2, want_hz) in [
            ("rp2040", 125u32, 5usize, 1usize, 125_000_000u32),
            ("rp2350", 125, 4, 1, 150_000_000),
        ] {
            let graph = rp_graph(fb, pd1, pd2);
            // Prove the defaults before writing them down: 12 MHz x FBDIV,
            // divided by POSTDIV1 then POSTDIV2.
            let pd = |i: usize| [1u32, 2, 3, 4, 5, 6, 7][i];
            let got = 12_000_000 * fb / pd(pd1) / pd(pd2);
            assert_eq!(got, want_hz, "{name} clk_sys");
            let usb = 12_000_000 * 100 / 5 / 5;
            assert_eq!(usb, 48_000_000, "clk_usb must be exactly 48 MHz");

            let gc = GraphClock {
                layout: auto_layout(&graph),
                graph,
                bindings: Default::default(),
            };
            let text =
                ron::ser::to_string_pretty(&gc, ron::ser::PrettyConfig::new()).expect("serialise");
            let path = std::env::temp_dir().join(format!("eide_{name}_clock.ron"));
            std::fs::write(&path, text).expect("write");
            println!("wrote {} ({} nodes)", path.display(), gc.graph.nodes.len());
        }
    }
}

#[cfg(test)]
mod emit_for_manual_compile {
    use crate::panels::mcu_module::{builtins, pins::PinFunction, project_gen};

    /// A Pico / Pico 2 project on disk, for a real cross-compile.
    ///
    /// Nothing about this backend is believable until a compiler has seen it:
    /// the boot stage, the PLL sequence and the GPIO bank are all APIs read from
    /// documentation, and documentation is where this session's every wrong
    /// guess came from.
    ///
    /// ```text
    /// cargo test --bin embedded_ide_0 emit_rp_project -- --ignored --nocapture
    /// cd %TEMP%\eide_rp2040_check && cargo check --target thumbv6m-none-eabi
    /// ```
    #[test]
    #[ignore = "writes projects to disk for a manual cross-compile"]
    fn emit_rp_project() {
        for (id, dir_name) in [
            ("rp2040_pico", "eide_rp2040_check"),
            ("rp2350_pico2", "eide_rp2350_check"),
            // The wireless boards generate through the same backend; what
            // differs is that GP23/24/25/29 are the radio's, so the LED cannot
            // be wired and the emitter has to skip it without complaining.
            ("rp2040_pico_w", "eide_rp2040w_check"),
            ("rp2350_pico2_w", "eide_rp2350w_check"),
        ] {
            let def = builtins::builtin_definitions()
                .into_iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("built-in {id}"));
            let mut mcu = def.build_mcu();
            // The on-board LED and one input, so the GPIO half is exercised.
            for p in mcu.iter_all_pins_mut() {
                // One of each bus, on the pads their FUNCSEL table gives them:
                // UART0 on GP0/1, I2C0 on GP4/5, SPI0 on GP18/19/16. Anything
                // else would be a wiring the chip cannot make.
                match p.name.as_str() {
                    n if n.starts_with("GP25") => p.selected_function = PinFunction::GpioOutput,
                    "GP0" => p.selected_function = PinFunction::UsartTx(0),
                    "GP1" => p.selected_function = PinFunction::UsartRx(0),
                    "GP4" => p.selected_function = PinFunction::I2cSda(0),
                    "GP5" => p.selected_function = PinFunction::I2cScl(0),
                    "GP18" => p.selected_function = PinFunction::SpiSck(0),
                    "GP19" => p.selected_function = PinFunction::SpiMosi(0),
                    "GP16" => p.selected_function = PinFunction::SpiMiso(0),
                    // PWM slice 3 (both channels) and one ADC input.
                    "GP6" => {
                        p.selected_function = PinFunction::TimerPwm {
                            timer: 3,
                            channel: 1,
                        }
                    }
                    "GP7" => {
                        p.selected_function = PinFunction::TimerPwm {
                            timer: 3,
                            channel: 2,
                        }
                    }
                    "GP26" => p.selected_function = PinFunction::AdcChannel { adc: 0, channel: 0 },
                    _ => {}
                }
            }
            // The modules the wiring implies, then non-default duties on the
            // PWM slice: 7.5 % and 10 %, so the generated code cannot pass by
            // accident on the module's default of zero.
            mcu.reconcile_modules();
            for m in &mut mcu.modules {
                if let crate::panels::mcu_module::modules::ModuleConfig::Timer(c) = &mut m.config {
                    c.freq_hz = 20_000;
                    c.set_duty_x100(1, 750);
                    c.set_duty_x100(2, 1_000);
                }
            }
            let main_rs = mcu.fresh_main_rs();
            let files = project_gen::build_project_files(&def.project, &def.toolchain, &main_rs);
            // `sync_pin_files` keeps `src/pins/mod.rs` in a real project, and the
            // generated header declares `pub mod pins;` — so the harness has to
            // supply it too, or it compiles a project shape the app never
            // produces. Which is exactly what happened: the invariant test went
            // green while `cargo check` said "file not found for module `pins`".
            let configs = mcu.config_files();
            let mut user: Vec<(String, String)> = vec![
                (
                    "src/pins/mod.rs".into(),
                    "pub mod configs;
"
                    .into(),
                ),
                (
                    "src/pins/configs/mod.rs".into(),
                    configs
                        .iter()
                        .map(|(n, _)| {
                            format!(
                                "pub mod {};
",
                                n.trim_end_matches(".rs")
                            )
                        })
                        .collect(),
                ),
            ];
            user.extend(
                configs
                    .into_iter()
                    .map(|(name, body)| (format!("src/pins/configs/{name}"), body)),
            );
            let dir = std::env::temp_dir().join(dir_name);
            let _ = std::fs::remove_dir_all(&dir);
            project_gen::write_project(&dir, &files, &user, &mcu.mcu_config_text(), "")
                .expect("write rp project");
            println!("wrote {}", dir.display());
            println!("target: {}", def.project.target);
        }
    }
}

#[cfg(test)]
mod regeneration {
    use crate::panels::mcu_module::{builtins, pins::PinFunction};

    /// Re-generating must replace the marked block and keep everything else.
    ///
    /// This is the one failure a compiler cannot catch: a splice that drops the
    /// user's loop still produces a file that builds, and the loss is only
    /// noticed later, by the person who wrote it.
    #[test]
    fn regeneration_keeps_the_users_code() {
        let def = builtins::builtin_definitions()
            .into_iter()
            .find(|d| d.id == "rp2040_pico")
            .expect("built-in Pico");
        let mut mcu = def.build_mcu();
        for p in mcu.iter_all_pins_mut() {
            if p.name.starts_with("GP25") {
                p.selected_function = PinFunction::GpioOutput;
            }
        }
        let first = mcu.fresh_main_rs();

        // What a user would add: an import above the block and code below it.
        let edited = first
            .replace(
                "use panic_halt as _;",
                "use panic_halt as _;\nuse my_crate::Thing;",
            )
            .replace(
                "        // Your main loop code here.",
                "        gp25.set_high().unwrap();\n        my_own_helper();",
            );

        // Wire a second pad, so the block genuinely has to change.
        for p in mcu.iter_all_pins_mut() {
            if p.name == "GP16" {
                p.selected_function = PinFunction::GpioInput;
            }
        }
        let again = mcu.update_main_rs(&edited);

        assert!(
            again.contains("use my_crate::Thing;"),
            "import above the block:\n{again}"
        );
        assert!(
            again.contains("my_own_helper();"),
            "code below the block:\n{again}"
        );
        assert!(
            again.contains("pins.gpio16.into_pull_up_input()"),
            "the new pad:\n{again}"
        );
        assert!(
            again.contains("pins.gpio25.into_push_pull_output()"),
            "the old pad:\n{again}"
        );
        // And it must not have grown a second copy of the block.
        assert_eq!(
            again.matches("#[rp2040_hal::entry]").count(),
            1,
            "the generated block was duplicated:\n{again}"
        );
    }
}

#[cfg(test)]
mod header_layout {
    use crate::panels::mcu_module::builtins;

    /// The 40-pin header is numbered like a DIP: down the left side, then UP the
    /// right. So pin 21 sits at the BOTTOM right, facing pin 20 — and since the
    /// canvas draws each side top-to-bottom in list order, the right column has
    /// to be stored descending.
    ///
    /// Generated ascending the first time, which put GP16 at the top right and
    /// VBUS at the bottom: a board that matches no photograph and no silkscreen.
    #[test]
    fn the_header_is_numbered_like_the_silkscreen() {
        for id in ["rp2040_pico", "rp2350_pico2"] {
            let mcu = builtins::builtin_definitions()
                .into_iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("built-in {id}"))
                .build_mcu();

            let left: Vec<usize> = mcu.left_pins.iter().map(|p| p.number).collect();
            let right: Vec<usize> = mcu.right_pins.iter().map(|p| p.number).collect();

            assert_eq!(left, (1..=20).collect::<Vec<_>>(), "{id}: left side");
            assert_eq!(
                right,
                (21..=40).rev().collect::<Vec<_>>(),
                "{id}: the right side runs UP the board"
            );
            // The two that face each other at the bottom.
            assert_eq!(*left.last().unwrap(), 20, "{id}");
            assert_eq!(*right.last().unwrap(), 21, "{id}");

            // And the pin those two carry, because the numbers alone would pass
            // on a board whose names were shuffled.
            let name_of = |n: usize| {
                mcu.iter_all_pins()
                    .find(|p| p.number == n)
                    .map(|p| p.name.clone())
                    .unwrap_or_default()
            };
            assert_eq!(name_of(1), "GP0", "{id}");
            assert_eq!(name_of(20), "GP15", "{id}");
            assert_eq!(name_of(21), "GP16", "{id}");
            assert_eq!(name_of(40), "VBUS", "{id}");

            // The four that are on the BOARD but not on the header. They sit on
            // the top edge because that is where they are: the USB connector is
            // at the pin-1 end, and the LED is beside it.
            let top: Vec<String> = mcu.top_pins.iter().map(|p| p.name.clone()).collect();
            assert!(
                top.iter().any(|n| n.starts_with("GP25")),
                "{id}: the LED must be reachable, or the first thing anyone tries cannot be done: {top:?}"
            );
            assert!(
                mcu.bottom_pins.is_empty(),
                "{id}: nothing belongs on the bottom edge"
            );
        }
    }
}

#[cfg(test)]
mod ambiguous_wiring {
    use crate::panels::mcu_module::{builtins, pins::PinFunction};

    /// Two pads claiming one signal must be NAMED, not silently reduced to one.
    ///
    /// GP0 and GP16 are both UART0 TX on this chip — that is the FUNCSEL table,
    /// so a user can wire both without doing anything wrong. rp-hal takes one
    /// pin per role, and the code that chose silently left the other pad
    /// unconfigured with nothing to explain it.
    #[test]
    fn two_pads_on_one_signal_are_both_named() {
        let mut mcu = builtins::builtin_definitions()
            .into_iter()
            .find(|d| d.id == "rp2040_pico")
            .expect("built-in Pico")
            .build_mcu();
        for p in mcu.iter_all_pins_mut() {
            match p.name.as_str() {
                // Both are UART0 TX. Both are legal. Only one can be built.
                "GP0" | "GP16" => p.selected_function = PinFunction::UsartTx(0),
                "GP1" => p.selected_function = PinFunction::UsartRx(0),
                _ => {}
            }
        }
        let code = mcu.fresh_main_rs();
        assert!(
            code.contains("UART0 TX is wired to GP0 and GP16"),
            "the clash must be named:\n{code}"
        );
        assert!(code.contains("Only GP0 is configured"), "{code}");
        // The lowest pad is the one built, so the output does not depend on
        // which order the canvas happened to hand them over.
        // main.rs hands the pad to the config module now, rather than
        // reconfiguring it in place.
        assert!(code.contains("pins.gpio0,"), "{code}");
        assert!(!code.contains("pins.gpio16,"), "{code}");
    }

    /// And an unambiguous project says nothing at all.
    #[test]
    fn a_clean_wiring_gets_no_note() {
        let mut mcu = builtins::builtin_definitions()
            .into_iter()
            .find(|d| d.id == "rp2040_pico")
            .expect("built-in Pico")
            .build_mcu();
        for p in mcu.iter_all_pins_mut() {
            match p.name.as_str() {
                "GP0" => p.selected_function = PinFunction::UsartTx(0),
                "GP1" => p.selected_function = PinFunction::UsartRx(0),
                _ => {}
            }
        }
        let code = mcu.fresh_main_rs();
        assert!(
            !code.contains("is wired to GP"),
            "no clash, no note:\n{code}"
        );
    }
}

#[cfg(test)]
mod config_file_shape {
    use crate::panels::mcu_module::{builtins, pins::PinFunction};

    /// The editable half of a config file must sit OUTSIDE the markers.
    ///
    /// `sync_config_files` replaces everything between `<<< GENERATED>>>` and
    /// `<<< GENERATED END >>>` whenever a Virtual Module changes. So whatever
    /// lands inside is regenerated, and whatever the user wrote there is gone —
    /// silently, with no error, on a change as small as nudging a duty slider.
    ///
    /// Constants belong inside. `init`, `Handle` and `DutyHandle` do not: they
    /// are the parts a user rewrites.
    #[test]
    fn only_the_constants_are_regenerated() {
        const BEGIN: &str = "// <<< GENERATED>>>";
        const END: &str = "// <<< GENERATED END >>>";

        let mut mcu = builtins::builtin_definitions()
            .into_iter()
            .find(|d| d.id == "rp2040_pico")
            .expect("built-in Pico")
            .build_mcu();
        for p in mcu.iter_all_pins_mut() {
            match p.name.as_str() {
                "GP0" => p.selected_function = PinFunction::UsartTx(0),
                "GP1" => p.selected_function = PinFunction::UsartRx(0),
                "GP4" => p.selected_function = PinFunction::I2cSda(0),
                "GP5" => p.selected_function = PinFunction::I2cScl(0),
                "GP18" => p.selected_function = PinFunction::SpiSck(0),
                "GP19" => p.selected_function = PinFunction::SpiMosi(0),
                "GP16" => p.selected_function = PinFunction::SpiMiso(0),
                "GP6" => {
                    p.selected_function = PinFunction::TimerPwm {
                        timer: 3,
                        channel: 1,
                    }
                }
                _ => {}
            }
        }
        let files = mcu.config_files();
        assert_eq!(
            files.len(),
            4,
            "one per peripheral: {:?}",
            files.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );

        for (name, body) in &files {
            assert_eq!(body.matches(BEGIN).count(), 1, "{name}: one begin marker");
            assert_eq!(body.matches(END).count(), 1, "{name}: one end marker");
            let b = body.find(BEGIN).unwrap();
            let e = body.find(END).unwrap();
            assert!(b < e, "{name}: markers out of order");

            let inside = &body[b..e];
            let outside = &body[e..];
            for forbidden in ["pub fn init", "pub trait", "pub type Handle"] {
                assert!(
                    !inside.contains(forbidden),
                    "{name}: `{forbidden}` is inside the regenerated block, so a user's \
                     edit to it would be wiped by the next duty change:\n{body}"
                );
            }
            assert!(
                outside.contains("pub fn init"),
                "{name}: init must survive:\n{body}"
            );
            assert!(
                outside.contains("pub type Handle"),
                "{name}: Handle must survive"
            );
            // And the constants are where they belong.
            assert!(
                inside.contains("const "),
                "{name}: nothing regenerated at all?\n{body}"
            );
        }
    }
}

#[cfg(test)]
mod wireless_boards {
    use crate::panels::mcu_module::builtins;
    use crate::panels::mcu_module::pins::logic::pin::colors::reserved_role;

    /// On a W board the LED is NOT a GPIO, and the board has to say so.
    ///
    /// GP25 drives the on-board LED on a Pico and the CYW43's chip select on a
    /// Pico W. Someone coming from the non-W board will reach for it first, so
    /// the pad is reserved and its explanation names the surprise rather than
    /// leaving them to find it with a meter.
    #[test]
    fn the_led_is_not_a_gpio_on_a_wireless_board() {
        for id in ["rp2040_pico_w", "rp2350_pico2_w"] {
            let mcu = builtins::builtin_definitions()
                .into_iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("built-in {id}"))
                .build_mcu();

            let radio: Vec<&str> = mcu
                .iter_all_pins()
                .filter(|p| p.reserved && p.name.starts_with("WL_"))
                .map(|p| p.name.as_str())
                .collect();
            assert_eq!(radio.len(), 4, "{id}: the radio's four lines: {radio:?}");

            // Nothing on a W board may offer GP25 as a pin to configure.
            assert!(
                !mcu.iter_all_pins().any(|p| p.name.starts_with("GP25")),
                "{id}: GP25 is the radio's chip select here, not the LED"
            );
            // And the explanation has to say the thing that surprises people.
            let why = reserved_role("WL_CS");
            assert!(
                why.contains("LED is NOT here"),
                "{id}: the pad must name the surprise, not just its function: {why}"
            );
        }

        // The non-W boards keep their LED, or the change went too far.
        for id in ["rp2040_pico", "rp2350_pico2"] {
            let mcu = builtins::builtin_definitions()
                .into_iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("built-in {id}"))
                .build_mcu();
            assert!(
                mcu.iter_all_pins().any(|p| p.name.starts_with("GP25")),
                "{id}: the LED is still a plain GPIO here"
            );
        }
    }
}

/// What the compiler taught me about embassy-rp's async constructors.
///
/// Both facts here compiled to nothing visible in a test that only looked at
/// the emitted text: a project whose buses were never emitted at all still
/// "compiled", and the wrong argument order only shows up as a trait bound.
#[cfg(test)]
mod async_dma_bindings {
    use super::async_bus_lines;
    use crate::panels::mcu_module::mcu::model::Runtime;
    use crate::panels::mcu_module::{builtins, pins::PinFunction};

    fn pico_with_every_bus() -> super::Mcu {
        let mut mcu = builtins::builtin_definitions()
            .into_iter()
            .find(|d| d.id == "rp2040_pico")
            .expect("built-in Pico")
            .build_mcu();
        mcu.runtime = Runtime::Async;
        for p in mcu.iter_all_pins_mut() {
            match p.name.as_str() {
                "GP0" => p.selected_function = PinFunction::UsartTx(0),
                "GP1" => p.selected_function = PinFunction::UsartRx(0),
                "GP4" => p.selected_function = PinFunction::I2cSda(0),
                "GP5" => p.selected_function = PinFunction::I2cScl(0),
                "GP18" => p.selected_function = PinFunction::SpiSck(0),
                "GP19" => p.selected_function = PinFunction::SpiMosi(0),
                "GP16" => p.selected_function = PinFunction::SpiMiso(0),
                _ => {}
            }
        }
        mcu
    }

    /// A DMA channel handed to a driver needs its OWN handler bound.
    ///
    /// Not the peripheral's — the channel's. All sixteen drain through the one
    /// DMA_IRQ_0, so the handlers stack up under a single entry, which the
    /// `bind_interrupts!` grammar allows and nothing else in this repo uses.
    #[test]
    fn every_dma_channel_gets_a_handler() {
        let (binding, body, _) = async_bus_lines(&pico_with_every_bus());
        // UART takes two channels and SPI two more.
        for ch in 0..4 {
            assert!(
                body.contains(&format!("p.DMA_CH{ch},")),
                "channel {ch} is handed out:
{body}"
            );
            assert!(
                binding.contains(&format!(
                    "dma::InterruptHandler<embassy_rp::peripherals::DMA_CH{ch}>"
                )),
                "channel {ch} is bound:
{binding}"
            );
        }
        assert_eq!(
            binding.matches("DMA_IRQ_0").count(),
            1,
            "one entry, not four:
{binding}"
        );
    }

    /// UART takes the binding BEFORE its channels, SPI after. Same crate.
    #[test]
    fn the_irq_argument_sits_where_each_driver_wants_it() {
        let (_, body, _) = async_bus_lines(&pico_with_every_bus());
        let uart = body.split("Uart::new").nth(1).expect("a uart");
        let uart = uart.split("let _").next().unwrap();
        assert!(
            uart.find("Irqs,").unwrap() < uart.find("p.DMA_CH").unwrap(),
            "uart: irq then channels:\n{uart}"
        );
        let spi = body.split("Spi::new").nth(1).expect("a spi");
        let spi = spi.split("let _").next().unwrap();
        assert!(
            spi.find("Irqs,").unwrap() > spi.find("p.DMA_CH").unwrap(),
            "spi: channels then irq:\n{spi}"
        );
    }
}

#[cfg(test)]
mod radio_led {
    use crate::panels::mcu_module::mcu::model::Runtime;
    use crate::panels::mcu_module::{builtins, pins::PinFunction};

    fn pico_w(runtime: Runtime, take_the_led: bool) -> super::Mcu {
        let mut mcu = builtins::builtin_definitions()
            .into_iter()
            .find(|d| d.id == "rp2040_pico_w")
            .expect("built-in Pico W")
            .build_mcu();
        mcu.runtime = runtime;
        if take_the_led {
            for p in mcu.iter_all_pins_mut() {
                if p.name == "WL_LED" {
                    p.selected_function = PinFunction::GpioOutput;
                }
            }
        }
        mcu
    }

    /// A W board carries no wifi stack until someone asks for the LED.
    ///
    /// The deps are gated on the PAD, not on the board: `cyw43` pulls in a
    /// whole wireless driver, and a Pico W project that only blinks a GPIO has
    /// no business linking one.
    #[test]
    fn an_untouched_pad_emits_nothing() {
        let code = pico_w(Runtime::Async, false).fresh_main_rs();
        assert!(!code.contains("cyw43"), "no radio until asked:\n{code}");
        // And the spawner stays underscored, or the project warns.
        assert!(code.contains("async fn main(_spawner: Spawner)"), "{code}");
    }

    /// Taking it brings up the radio AND wakes the spawner.
    #[test]
    fn taking_the_pad_brings_up_the_radio() {
        let code = pico_w(Runtime::Async, true).fresh_main_rs();
        for want in [
            "cyw43::new(",
            "cyw43_pio::PioSpi::new(",
            "cyw43_pio::RM2_CLOCK_DIVIDER",
            "PIO0_IRQ_0 =>",
            "control.init(clm).await;",
            // The task is a TOP-LEVEL item; inside `main` it does not compile.
            "#[embassy_executor::task]",
        ] {
            assert!(code.contains(want), "missing {want}:\n{code}");
        }
        assert!(
            code.contains("async fn main(spawner: Spawner)"),
            "spawning needs a live spawner:\n{code}"
        );
    }

    /// On Blocking there is no radio code to emit, and no pretending otherwise.
    ///
    /// `cyw43` is async to the bottom. A blocking project that silently dropped
    /// the LED would look like a codegen bug; one that emitted a blocking call
    /// would be fiction. It says what to do instead.
    #[test]
    fn blocking_says_why_rather_than_emitting_fiction() {
        let code = pico_w(Runtime::Blocking, true).fresh_main_rs();
        assert!(
            !code.contains("cyw43"),
            "no async driver on blocking:\n{code}"
        );
        assert!(
            code.contains("Switch Runtime to"),
            "it says what to do:\n{code}"
        );
    }
}

#[cfg(test)]
mod async_hal_line {
    use crate::panels::mcu_module::builtins;

    /// RP is the first family whose HAL CRATE changes with the runtime.
    ///
    /// `rp2040-hal` drives the chip blocking and `embassy-rp` drives it async —
    /// two different crates, not two feature sets. Everything before this had
    /// one HAL per chip, so the model had nowhere to say it.
    ///
    /// The feature is chip-specific, which is why the line lives on the chip and
    /// not in the backend: an RP2350**A** wants `rp235xa`, a **B** wants
    /// `rp235xb`, and a backend deriving it from the family would get one wrong.
    #[test]
    fn the_pico_boards_swap_hal_crate_on_async() {
        for (id, feature) in [
            ("rp2040_pico", "rp2040"),
            ("rp2040_pico_w", "rp2040"),
            ("rp2350_pico2", "rp235xa"),
            ("rp2350_pico2_w", "rp235xa"),
        ] {
            let def = builtins::builtin_definitions()
                .into_iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("built-in {id}"));

            let blocking = def.project.for_async(false);
            assert!(
                blocking.hal_dep.starts_with("rp2040-hal")
                    || blocking.hal_dep.starts_with("rp235x-hal"),
                "{id}: blocking keeps its own HAL: {}",
                blocking.hal_dep
            );

            let asynchronous = def.project.for_async(true);
            assert!(
                asynchronous.hal_dep.starts_with("embassy-rp"),
                "{id}: async swaps to embassy-rp: {}",
                asynchronous.hal_dep
            );
            assert!(
                asynchronous.hal_dep.contains(feature),
                "{id}: the chip feature has to be this board's: {}",
                asynchronous.hal_dep
            );
        }

        // And a family that does NOT swap is untouched, whichever runtime.
        let f1 = builtins::builtin_definitions()
            .into_iter()
            .find(|d| d.id == "stm32f103c8t6")
            .expect("built-in F103");
        assert_eq!(
            f1.project.for_async(true).hal_dep,
            f1.project.hal_dep,
            "STM32 keeps one HAL for both runtimes"
        );
    }
}

// ── The async runtime, on `embassy-rp` ────────────────────────────────────────
//
// A DIFFERENT HAL from the blocking backend above — `embassy-rp`, not
// `rp2040-hal` — which is why the chip carries a second dependency line. It is
// also the only way to reach the CYW43 radio on a W board, and with it the LED:
// `cyw43` is async to the bone.
//
// Much shorter than the blocking template, because embassy-rp does for you what
// rp-hal makes explicit: `init` sets the clocks up and, on RP2040, supplies the
// second-stage bootloader itself.

pub struct AsyncRpBackend;

/// The GPIO bindings, in header order. embassy-rp names pads `PIN_25`, not
/// `gpio25`, and hands them over from one `Peripherals` struct.
fn async_gpio_lines(mcu: &Mcu) -> String {
    let mut out = String::new();
    for p in mcu.iter_all_pins().filter(|p| !p.reserved) {
        let Some(n) = gpio_index(&p.name) else {
            continue;
        };
        let sfx = var_suffix(&p.selected_function);
        match p.selected_function {
            PinFunction::GpioOutput => out.push_str(&format!(
                "{ALLOW}    let mut gp{n}{sfx} = Output::new(p.PIN_{n}, Level::Low);\n"
            )),
            PinFunction::GpioInput => out.push_str(&format!(
                "{ALLOW}    let gp{n}{sfx} = Input::new(p.PIN_{n}, Pull::Up);\n"
            )),
            _ => {}
        }
    }
    out
}

/// The buses, on embassy-rp.
///
/// Async constructors where they exist, which is why some of these need an
/// interrupt binding and some need DMA channels. The channels are handed out in
/// order: embassy-rp exposes twelve as separate peripherals, and nothing else
/// here claims one.
///
/// Returns `(interrupt binding, body)` — the binding is a top-level item and
/// cannot live inside `main`.
fn async_bus_lines(mcu: &Mcu) -> (String, String, String) {
    let mut irqs: Vec<String> = Vec::new();
    let mut o = String::new();
    let mut dma = 0u8;

    let uart = uart_pins(mcu);
    for i in instances(&uart) {
        let (Some(tx), Some(rx)) = (role_of(&uart, i, "tx"), role_of(&uart, i, "rx")) else {
            o.push_str(&format!("    // UART{i}: TX and RX are taken together.\n"));
            continue;
        };
        let (tdma, rdma) = (dma, dma + 1);
        dma += 2;
        o.push_str(&format!(
            "    let uart{i} = embassy_rp::uart::Uart::new(\n        p.UART{i},\n        p.PIN_{tx},\n        p.PIN_{rx},\n        Irqs,\n        p.DMA_CH{tdma},\n        p.DMA_CH{rdma},\n        embassy_rp::uart::Config::default(),\n    );\n    let _ = &uart{i};\n"
        ));
        irqs.push(format!(
            "    UART{i}_IRQ => embassy_rp::uart::InterruptHandler<embassy_rp::peripherals::UART{i}>;"
        ));
    }

    let spi = spi_pins(mcu);
    for i in instances(&spi) {
        let (Some(sck), Some(mosi), Some(miso)) = (
            role_of(&spi, i, "sck"),
            role_of(&spi, i, "mosi"),
            role_of(&spi, i, "miso"),
        ) else {
            o.push_str(&format!(
                "    // SPI{i}: all three pads are taken together.\n"
            ));
            continue;
        };
        let (tdma, rdma) = (dma, dma + 1);
        dma += 2;
        o.push_str(&format!(
            "    let spi{i} = embassy_rp::spi::Spi::new(\n        p.SPI{i},\n        p.PIN_{sck},\n        p.PIN_{mosi},\n        p.PIN_{miso},\n        p.DMA_CH{tdma},\n        p.DMA_CH{rdma},\n        Irqs,\n        embassy_rp::spi::Config::default(),\n    );\n    let _ = &spi{i};\n"
        ));
    }

    let i2c = i2c_pins(mcu);
    for i in instances(&i2c) {
        let (Some(sda), Some(scl)) = (role_of(&i2c, i, "sda"), role_of(&i2c, i, "scl")) else {
            o.push_str(&format!("    // I2C{i}: SDA and SCL are taken together.\n"));
            continue;
        };
        o.push_str(&format!(
            "    let i2c{i} = embassy_rp::i2c::I2c::new_async(\n        p.I2C{i},\n        p.PIN_{scl},\n        p.PIN_{sda},\n        Irqs,\n        embassy_rp::i2c::Config::default(),\n    );\n    let _ = &i2c{i};\n"
        ));
        irqs.push(format!(
            "    I2C{i}_IRQ => embassy_rp::i2c::InterruptHandler<embassy_rp::peripherals::I2C{i}>;"
        ));
    }

    let mut pwm: Vec<(u8, u8, u8)> = Vec::new();
    let mut adc: Vec<(u8, u8)> = Vec::new();
    for pin in mcu.iter_all_pins().filter(|p| !p.reserved) {
        let Some(n) = gpio_index(&pin.name) else {
            continue;
        };
        match pin.selected_function {
            PinFunction::TimerPwm { timer, channel } => pwm.push((timer, channel, n)),
            PinFunction::AdcChannel { channel, .. } => adc.push((channel, n)),
            _ => {}
        }
    }
    pwm.sort_unstable();
    adc.sort_unstable();
    let mut done: Vec<u8> = Vec::new();
    for (slice, channel, n) in &pwm {
        if done.contains(slice) {
            continue;
        }
        done.push(*slice);
        let ctor = if *channel == 1 {
            "new_output_a"
        } else {
            "new_output_b"
        };
        o.push_str(&format!(
            "    let pwm{slice} = embassy_rp::pwm::Pwm::{ctor}(\n        p.PWM_SLICE{slice},\n        p.PIN_{n},\n        embassy_rp::pwm::Config::default(),\n    );\n    let _ = &pwm{slice};\n"
        ));
    }

    if !adc.is_empty() {
        o.push_str("    let mut adc = embassy_rp::adc::Adc::new(p.ADC, Irqs, embassy_rp::adc::Config::default());\n    let _ = &mut adc;\n");
        irqs.push("    ADC_IRQ_FIFO => embassy_rp::adc::InterruptHandler;".to_owned());
        for (channel, n) in &adc {
            o.push_str(&format!(
                "    let mut adc{channel} = embassy_rp::adc::Channel::new_pin(p.PIN_{n}, embassy_rp::gpio::Pull::None);\n    let _ = &mut adc{channel};\n"
            ));
        }
    }

    // The radio takes one more channel, from the same counter — two drivers
    // both handed DMA_CH0 would compile and then fight at run time.
    let radio = if radio_led(mcu) {
        let (mut r_irqs, task, body) = radio_lines(dma);
        dma += 1;
        irqs.append(&mut r_irqs);
        o.push_str(&body);
        task
    } else {
        String::new()
    };

    if dma > 0 {
        // Every channel drains through the one DMA interrupt, so the
        // handlers for all of them hang off DMA_IRQ_0 together.
        let handlers: Vec<String> = (0..dma)
            .map(|c| {
                format!(
                    "        embassy_rp::dma::InterruptHandler<embassy_rp::peripherals::DMA_CH{c}>"
                )
            })
            .collect();
        irqs.push(format!("    DMA_IRQ_0 =>\n{};", handlers.join(",\n")));
    }

    let binding = if irqs.is_empty() {
        String::new()
    } else {
        format!(
            "// The handlers embassy needs bound before an async peripheral can run.\nembassy_rp::bind_interrupts!(struct Irqs {{\n{}\n}});\n\n",
            irqs.join("\n")
        )
    };
    (binding, o, radio)
}

/// Is the on-board LED wired up on a W board?
///
/// `WL_LED` is not a GPIO on the chip at all — it is pin 0 of the CYW43 radio's
/// own GPIO block, which is why a Pico W cannot blink without a wifi driver
/// running. The pad exists on the canvas so the board can OFFER the LED; this
/// asks whether the user took it.
fn radio_led(mcu: &Mcu) -> bool {
    mcu.iter_all_pins()
        .any(|p| p.name == "WL_LED" && p.selected_function == PinFunction::GpioOutput)
}

/// The bring-up for the CYW43 radio, purely so its GPIO0 can drive the LED.
///
/// Everything here is forced by the hardware, not chosen: the radio speaks a
/// half-duplex SPI no SPI block on the chip can produce, so it goes through a
/// PIO program; the driver is `async` all the way down, so there is no blocking
/// path to this LED at all; and the firmware is three Infineon binaries that
/// cannot ship in a generated project.
///
/// Returns `(irq entries, top-level items, main body)` — the task has to sit
/// outside `main`, and the interrupt entries have to join the shared binding.
fn radio_lines(dma: u8) -> (Vec<String>, String, String) {
    let irqs = vec![
        "    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<embassy_rp::peripherals::PIO0>;"
            .to_owned(),
    ];

    let task = "/// Drives the radio. `cyw43` does its own SPI, its own event loop and its own
/// power management, so nothing on the LED path works until this runs.
#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<
        'static,
        cyw43::SpiBus<
            embassy_rp::gpio::Output<'static>,
            cyw43_pio::PioSpi<'static, embassy_rp::peripherals::PIO0, 0>,
        >,
    >,
) -> ! {
    runner.run().await
}

"
    .to_owned();

    let body = format!(
        "    // The radio's firmware, written into `firmware/` with this project. They
    // are Infineon binaries under the Permissive Binary License, which is what
    // lets them ship; the licence text sits beside them. Replacing one with
    // your own build is safe - the IDE never overwrites a file already there.
    //
    // `nvram_rp2040.bin` is right on a Pico 2 W too: the name is the board it
    // was measured on, not the chip it runs on.
    let fw = cyw43::aligned_bytes!(\"../firmware/43439A0.bin\");
    let clm = cyw43::aligned_bytes!(\"../firmware/43439A0_clm.bin\");
    let nvram = cyw43::aligned_bytes!(\"../firmware/nvram_rp2040.bin\");

    // GP23/24/25/29 are the radio's, which is why the canvas reserves them.
    let pwr = embassy_rp::gpio::Output::new(p.PIN_23, Level::Low);
    let cs = embassy_rp::gpio::Output::new(p.PIN_25, Level::High);
    let mut pio = embassy_rp::pio::Pio::new(p.PIO0, Irqs);
    let spi = cyw43_pio::PioSpi::new(
        &mut pio.common,
        pio.sm0,
        // The RM2 divider, not the default one: the module on these boards does
        // not hold the link together at the faster clock.
        cyw43_pio::RM2_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        embassy_rp::dma::Channel::new(p.DMA_CH{dma}, Irqs),
    );

    static RADIO_STATE: static_cell::StaticCell<cyw43::State> = static_cell::StaticCell::new();
    let state = RADIO_STATE.init(cyw43::State::new());
    let (_net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw, nvram).await;
    // embassy-executor 0.10: the task FUNCTION returns the Result (the pool can
    // be exhausted), so the `unwrap` goes inside `spawn`, not after it.
    spawner.spawn(cyw43_task(runner).unwrap());
    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    // The LED is GPIO0 ON THE RADIO, so it is driven through `control` rather
    // than through a pin: `wl_led.gpio_set(0, true).await` turns it on.
    #[allow(unused_mut, unused_variables)]
    let mut wl_led = control;
"
    );

    (irqs, task, body)
}

fn async_section(mcu: &Mcu) -> String {
    let (irq_binding, buses, radio_task) = async_bus_lines(mcu);
    let spawner = if radio_task.is_empty() {
        "_spawner"
    } else {
        "spawner"
    };
    let mut o = String::new();
    o.push_str(GEN_BEGIN);
    o.push('\n');
    if mcu.family == "rp235x" {
        o.push_str("/// The image block the RP2350 boot ROM looks for. embassy-rp supplies the\n");
        o.push_str("/// contents; the section placement is ours.\n");
        o.push_str("#[link_section = \".start_block\"]\n#[used]\n");
        o.push_str("pub static IMAGE_DEF: embassy_rp::block::ImageDef = embassy_rp::block::ImageDef::secure_exe();\n\n");
    }
    o.push_str(&irq_binding);
    o.push_str(&radio_task);
    o.push_str("#[embassy_executor::main]\n");
    o.push_str(&format!("async fn main({spawner}: Spawner) {{\n"));
    o.push_str("    // embassy-rp brings up the clocks itself. On RP2040 it also supplies the\n");
    o.push_str("    // second-stage bootloader, which the blocking HAL makes you declare.\n");
    o.push_str("    let p = embassy_rp::init(Default::default());\n\n");
    o.push_str(&async_gpio_lines(mcu));
    o.push_str(&buses);
    o.push_str(GEN_END);
    o.push('\n');
    o
}

fn async_header(mcu: &Mcu) -> String {
    format!(
        "// Auto-generated by Embedded IDE\n\
         // MCU: {} | HAL: embassy-rp (async)\n\
         {}\n\
         #![no_std]\n\
         #![no_main]\n\
         \n\
         pub mod pins;\n\
         \n\
         use embassy_executor::Spawner;\n\
         #[allow(unused_imports)]\n\
         use embassy_rp::gpio::{{Input, Level, Output, Pull}};\n\
         use panic_halt as _;\n\
         \n",
        mcu.name,
        mcu_id_marker_line(&mcu.id),
    )
}

impl FamilyBackend for AsyncRpBackend {
    /// A LABEL — dispatch is by runtime, via `backend_for_runtime`.
    fn family_id(&self) -> &'static str {
        "rp-async"
    }

    fn handles(&self, family: &str) -> bool {
        is_rp(family)
    }

    /// embassy-rp takes the pull in `Input::new`, which this backend chooses.
    fn gpio_modes(&self, _func: &PinFunction) -> &'static [GpioMode] {
        &[]
    }

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        format!(
            "{}{}{}",
            async_header(mcu),
            async_section(mcu),
            "\n    loop {\n        // Your main loop code here.\n    }\n}\n"
        )
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let (Some(begin), Some(end_start)) = (existing.find(GEN_BEGIN), existing.find(GEN_END))
        else {
            return self.fresh_main_rs(mcu);
        };
        let end = end_start + GEN_END.len();
        format!(
            "{}{}{}",
            &existing[..begin],
            async_section(mcu).trim_end_matches('\n'),
            &existing[end..]
        )
    }
}

#[cfg(test)]
mod emit_async_for_manual_compile {
    use crate::panels::mcu_module::mcu::model::Runtime;
    use crate::panels::mcu_module::{builtins, pins::PinFunction, project_gen};

    /// The Pico W's LED, which is not on the chip at all.
    ///
    /// %TEMP%\eide_rp2040w_radio_check + eide_rp2350w_radio_check
    ///
    /// The firmware ships with the IDE (Infineon's Permissive Binary License
    /// allows it), so `write_project` lays it down and this compiles with the
    /// same bytes a real board would run. Nothing here proves the RADIO comes
    /// up - that needs hardware.
    #[test]
    #[ignore]
    fn emit_rp_radio_project() {
        for (id, dir_name) in [
            ("rp2040_pico_w", "eide_rp2040w_radio_check"),
            ("rp2350_pico2_w", "eide_rp2350w_radio_check"),
        ] {
            let def = builtins::builtin_definitions()
                .into_iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("built-in {id}"));
            let mut mcu = def.build_mcu();
            mcu.runtime = Runtime::Async;
            for p in mcu.iter_all_pins_mut() {
                match p.name.as_str() {
                    "WL_LED" => p.selected_function = PinFunction::GpioOutput,
                    // One ordinary bus too, so the radio's DMA channel is proved
                    // to come AFTER the ones the buses took, not on top of them.
                    "GP0" => p.selected_function = PinFunction::UsartTx(0),
                    "GP1" => p.selected_function = PinFunction::UsartRx(0),
                    _ => {}
                }
            }
            let main_rs = mcu.fresh_main_rs();
            assert!(main_rs.contains("cyw43::new("), "the radio is brought up");
            assert!(
                main_rs.contains("p.DMA_CH2,"),
                "the radio takes the channel after the UART's two:
{main_rs}"
            );
            let project = def.project.for_async(true);
            let files = project_gen::build_project_files(&project, &def.toolchain, &main_rs);
            let user: Vec<(String, String)> = vec![
                (
                    "src/pins/mod.rs".into(),
                    "pub mod configs;
"
                    .into(),
                ),
                ("src/pins/configs/mod.rs".into(), String::new()),
            ];
            let dir = std::env::temp_dir().join(dir_name);
            let _ = std::fs::remove_dir_all(&dir);
            project_gen::write_project(&dir, &files, &user, &mcu.mcu_config_text(), "")
                .expect("write rp radio project");
            let toml_path = dir.join("Cargo.toml");
            let toml = std::fs::read_to_string(&toml_path).expect("read Cargo.toml");
            let toml = project_gen::ensure_async_deps(
                &toml,
                true,
                project_gen::AsyncFlavor::Rp,
                false,
                false,
                false,
                &[],
            );
            let toml = project_gen::ensure_cyw43_deps(&toml, true, &[]);
            // `static_cell` holds the driver state, and on the Pico's M0 that
            // needs a CAS the core does not have. The app adds this right after
            // the same two calls; without it only the RP2350 half builds.
            let toml = project_gen::ensure_m0_atomics(&toml, true, &project.target, &[]);
            std::fs::write(&toml_path, toml).expect("write Cargo.toml");

            // `write_project` put the real blobs in `firmware/` already —
            // this only proves it, because a silent miss here would look like
            // a codegen failure two hundred lines away.
            // SIZES, not existence. `write_cyw43_firmware` deliberately never
            // overwrites, so a stub left by an older run survives — and
            // `include_bytes!` resolves just as happily on 27 bytes as on
            // 231 KB, which would take the whole case green on junk firmware.
            for (name, want) in [
                ("43439A0.bin", 231_077),
                ("43439A0_clm.bin", 984),
                ("nvram_rp2040.bin", 742),
                ("LICENSE-permissive-binary-license-1.0.txt", 2_419),
            ] {
                let path = dir.join("firmware").join(name);
                let got = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                assert_eq!(got, want, "{name}: shipped whole, not stubbed");
            }
            println!("wrote {}", dir.display());
            println!("target: {}", def.project.target);
        }
    }

    /// A Pico project on the ASYNC runtime — a different HAL from the blocking
    /// one, so nothing about it is believable until a compiler has seen it.
    ///
    /// ```text
    /// cargo test --bin embedded_ide_0 emit_rp_async_project -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "writes projects to disk for a manual cross-compile"]
    fn emit_rp_async_project() {
        for (id, dir_name) in [
            ("rp2040_pico", "eide_rp2040_async_check"),
            ("rp2350_pico2", "eide_rp2350_async_check"),
        ] {
            let def = builtins::builtin_definitions()
                .into_iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("built-in {id}"));
            let mut mcu = def.build_mcu();
            mcu.runtime = Runtime::Async;
            for p in mcu.iter_all_pins_mut() {
                match p.name.as_str() {
                    n if n.starts_with("GP25") => p.selected_function = PinFunction::GpioOutput,
                    "GP0" => p.selected_function = PinFunction::UsartTx(0),
                    "GP1" => p.selected_function = PinFunction::UsartRx(0),
                    "GP4" => p.selected_function = PinFunction::I2cSda(0),
                    "GP5" => p.selected_function = PinFunction::I2cScl(0),
                    "GP18" => p.selected_function = PinFunction::SpiSck(0),
                    "GP19" => p.selected_function = PinFunction::SpiMosi(0),
                    "GP16" => p.selected_function = PinFunction::SpiMiso(0),
                    "GP6" => {
                        p.selected_function = PinFunction::TimerPwm {
                            timer: 3,
                            channel: 1,
                        }
                    }
                    "GP26" => p.selected_function = PinFunction::AdcChannel { adc: 0, channel: 0 },
                    _ => {}
                }
            }
            let main_rs = mcu.fresh_main_rs();
            // The chip names a DIFFERENT HAL crate on async; this is where that
            // choice becomes a Cargo.toml.
            let project = def.project.for_async(true);
            let files = project_gen::build_project_files(&project, &def.toolchain, &main_rs);
            let user: Vec<(String, String)> = vec![
                ("src/pins/mod.rs".into(), "pub mod configs;\n".into()),
                ("src/pins/configs/mod.rs".into(), String::new()),
            ];
            let dir = std::env::temp_dir().join(dir_name);
            let _ = std::fs::remove_dir_all(&dir);
            project_gen::write_project(&dir, &files, &user, &mcu.mcu_config_text(), "")
                .expect("write rp async project");
            // The async deps the runtime needs, added the way the app adds them.
            let toml_path = dir.join("Cargo.toml");
            let toml = std::fs::read_to_string(&toml_path).expect("read Cargo.toml");
            let toml = project_gen::ensure_async_deps(
                &toml,
                true,
                project_gen::AsyncFlavor::Rp,
                false,
                false,
                false,
                &[],
            );
            std::fs::write(&toml_path, toml).expect("write Cargo.toml");
            // The other half of the firmware gate: a board with no radio
            // must not carry 231 KB of it. The gate reads the GENERATED
            // CODE, so it is exactly the kind of thing that goes wrong
            // quietly when the emitter changes.
            assert!(
                !dir.join("firmware").exists(),
                "no radio wired, no firmware shipped"
            );
            println!("wrote {}", dir.display());
            println!("target: {}", def.project.target);
        }
    }
}
