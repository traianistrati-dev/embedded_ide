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
        o.push_str("    // the HAL takes one pin per role. Unassign the other on the Pins canvas.\n");
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
        o.push_str("    // All eight slices come from one PWM peripheral, so main.rs owns
");
        o.push_str("    // the set and lends each wired one to its config module.
");
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
    o.push_str("// Peripheral config (from the Virtual Module) — auto-updated; edit in the module.\n");
    match kind {
        "uart" => o.push_str("const BAUDRATE: u32 = 115_200;\n"),
        "spi" => o.push_str("const SPI_HZ: u32 = 1_000_000;\n"),
        _ => o.push_str("const I2C_HZ: u32 = 400_000;\n"),
    }
    o.push_str("// <<< GENERATED END >>>\n\n");
    o.push_str("// Everything below is editable — your changes are preserved on regeneration.\n");
    // No `use Clock` here: main.rs asks the clock for its frequency and passes
    // the value in, so these modules never touch the trait.
    let gpio = |n: u8| format!("{hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{n}, {hal}::gpio::Function{}, {hal}::gpio::PullDown>",
        match kind { "uart" => "Uart", "spi" => "Spi", _ => "I2c" });

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
            let p = |n: u8| format!("{hal}::gpio::Pin<{hal}::gpio::bank0::Gpio{n}, {hal}::gpio::FunctionI2c, {hal}::gpio::PullUp>");
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
    o.push_str("// Peripheral config (from the Virtual Module) — auto-updated; edit in the module.\n");
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
        let ch = if *channel == 1 { "channel_a" } else { "channel_b" };
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
        let ch = if *channel == 1 { "channel_a" } else { "channel_b" };
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
            let Some(n) = gpio_index(&p.name) else { continue };
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
                ("src/pins/mod.rs".into(), "pub mod configs;
".into()),
                (
                    "src/pins/configs/mod.rs".into(),
                    configs
                        .iter()
                        .map(|(n, _)| format!("pub mod {};
", n.trim_end_matches(".rs")))
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
            assert!(mcu.bottom_pins.is_empty(), "{id}: nothing belongs on the bottom edge");
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
        assert!(!code.contains("is wired to GP"), "no clash, no note:\n{code}");
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
                "GP6" => p.selected_function = PinFunction::TimerPwm { timer: 3, channel: 1 },
                _ => {}
            }
        }
        let files = mcu.config_files();
        assert_eq!(files.len(), 4, "one per peripheral: {:?}",
            files.iter().map(|(n, _)| n).collect::<Vec<_>>());

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
            assert!(outside.contains("pub fn init"), "{name}: init must survive:\n{body}");
            assert!(outside.contains("pub type Handle"), "{name}: Handle must survive");
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
