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
use super::embassy_async::splice_section;
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
        let Some(n) = gpio_index(&p.name) else { continue };
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
        return Pll { vco_mhz: 1500, pd1: 6, pd2: 2 };
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
fn bus_pins(mcu: &Mcu, want: impl Fn(&PinFunction) -> Option<(u8, &'static str)>) -> Vec<(u8, &'static str, u8)> {
    let mut out = Vec::new();
    for p in mcu.iter_all_pins().filter(|p| !p.reserved) {
        let Some(n) = gpio_index(&p.name) else { continue };
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

/// UART, SPI and I2C, in that order.
///
/// Each is emitted only when BOTH of its required pads are wired. rp-hal's
/// constructors take the pins by value in a fixed order and there is no
/// `NoPin` — so half a bus is not a smaller bus here, it is a type error.
fn bus_lines(mcu: &Mcu, hal: &str) -> String {
    let mut o = String::new();

    let uart = bus_pins(mcu, |f| match f {
        PinFunction::UsartTx(i) => Some((*i, "tx")),
        PinFunction::UsartRx(i) => Some((*i, "rx")),
        _ => None,
    });
    for i in instances(&uart) {
        let (Some(tx), Some(rx)) = (role_of(&uart, i, "tx"), role_of(&uart, i, "rx")) else {
            o.push_str(&format!(
                "    // UART{i}: only one of TX/RX is wired, and `UartPeripheral::new`\n    // takes the pair. Wire the other pad on the Pins canvas.\n"
            ));
            continue;
        };
        o.push_str(&format!("    let uart{i} = {hal}::uart::UartPeripheral::new(\n"));
        o.push_str(&format!("        pac.UART{i},\n"));
        o.push_str(&format!("        (\n            pins.gpio{tx}.into_function(),\n            pins.gpio{rx}.into_function(),\n        ),\n"));
        o.push_str("        &mut pac.RESETS,\n    )\n    .enable(\n");
        o.push_str(&format!("        {hal}::uart::UartConfig::default(),\n"));
        o.push_str("        clocks.peripheral_clock.freq(),\n    )\n    .unwrap();\n");
        o.push_str(&format!("    let _ = &uart{i};\n"));
    }

    let spi = bus_pins(mcu, |f| match f {
        PinFunction::SpiSck(i) => Some((*i, "sck")),
        PinFunction::SpiMosi(i) => Some((*i, "mosi")),
        PinFunction::SpiMiso(i) => Some((*i, "miso")),
        _ => None,
    });
    for i in instances(&spi) {
        let (Some(sck), Some(mosi), Some(miso)) = (
            role_of(&spi, i, "sck"),
            role_of(&spi, i, "mosi"),
            role_of(&spi, i, "miso"),
        ) else {
            o.push_str(&format!(
                "    // SPI{i}: `Spi::new` takes (MOSI, MISO, SCK) together, so all three\n    // pads have to be wired before anything can be built.\n"
            ));
            continue;
        };
        o.push_str(&format!("    let spi{i} = {hal}::spi::Spi::<_, _, _, 8>::new(\n"));
        o.push_str(&format!("        pac.SPI{i},\n"));
        o.push_str(&format!("        (\n            pins.gpio{mosi}.into_function(),\n            pins.gpio{miso}.into_function(),\n            pins.gpio{sck}.into_function(),\n        ),\n"));
        o.push_str("    )\n    .init(\n        &mut pac.RESETS,\n        clocks.peripheral_clock.freq(),\n");
        o.push_str(&format!("        {hal}::fugit::HertzU32::MHz(1),\n"));
        o.push_str("        embedded_hal::spi::MODE_0,\n    );\n");
        o.push_str(&format!("    let _ = &spi{i};\n"));
    }

    let i2c = bus_pins(mcu, |f| match f {
        PinFunction::I2cSda(i) => Some((*i, "sda")),
        PinFunction::I2cScl(i) => Some((*i, "scl")),
        _ => None,
    });
    for i in instances(&i2c) {
        let (Some(sda), Some(scl)) = (role_of(&i2c, i, "sda"), role_of(&i2c, i, "scl")) else {
            o.push_str(&format!(
                "    // I2C{i}: SDA and SCL are taken together; wire the missing one.\n"
            ));
            continue;
        };
        o.push_str(&format!("    let i2c{i} = {hal}::i2c::I2C::i2c{i}(\n"));
        o.push_str(&format!("        pac.I2C{i},\n        pins.gpio{sda}.reconfigure(),\n        pins.gpio{scl}.reconfigure(),\n"));
        o.push_str(&format!("        {hal}::fugit::RateExtU32::kHz(400u32),\n"));
        o.push_str("        &mut pac.RESETS,\n        &clocks.system_clock,\n    );\n");
        o.push_str(&format!("    let _ = &i2c{i};\n"));
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
        o.push_str(&format!("const {name}: {hal}::pll::PLLConfig = {hal}::pll::PLLConfig {{\n"));
        o.push_str(&format!("    vco_freq: {hal}::fugit::HertzU32::MHz({}),\n", cfg.vco_mhz));
        o.push_str("    refdiv: 1,\n");
        o.push_str(&format!("    post_div1: {},\n", cfg.pd1));
        o.push_str(&format!("    post_div2: {},\n", cfg.pd2));
        o.push_str("};\n\n");
    }

    o.push_str(&format!("#[{hal}::entry]\n"));
    o.push_str("fn main() -> ! {\n");
    o.push_str(&format!("    let mut pac = {hal}::pac::Peripherals::take().unwrap();\n"));
    o.push_str(&format!("    let mut watchdog = {hal}::Watchdog::new(pac.WATCHDOG);\n"));
    o.push_str("\n");
    o.push_str("    // Built from the Clock tab, not from the HAL's fixed default: the two\n");
    o.push_str("    // PLLConfigs above are this tree's FBDIV and POSTDIV values.\n");
    o.push_str("    //\n");
    o.push_str("    // `map_err(|_| false)` because these error types carry no Debug, so\n");
    o.push_str("    // `.unwrap()` alone cannot name them.\n");
    o.push_str(&format!("    let xosc = {hal}::xosc::setup_xosc_blocking(\n"));
    o.push_str("        pac.XOSC,\n");
    o.push_str(&format!("        {hal}::fugit::HertzU32::Hz(XTAL_FREQ_HZ),\n"));
    o.push_str("    )\n    .map_err(|_| false)\n    .unwrap();\n");
    o.push_str(&format!("    let mut clocks = {hal}::clocks::ClocksManager::new(pac.CLOCKS);\n"));
    for (var, peri, cfg) in [
        ("pll_sys", "PLL_SYS", "PLL_SYS_CFG"),
        ("pll_usb", "PLL_USB", "PLL_USB_CFG"),
    ] {
        o.push_str(&format!("    let {var} = {hal}::pll::setup_pll_blocking(\n"));
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
         \n",
        mcu.name,
        mcu_id_marker_line(&mcu.id),
        hal_crate = hal_crate(&mcu.family),
    )
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

    fn fresh_main_rs(&self, mcu: &Mcu) -> String {
        format!("{}{}{USER_TAIL}", header(mcu), section(mcu))
    }

    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        splice_section(existing, &section(mcu), &mcu.name, &mcu.id)
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
                Edge { from: "xosc".into(), to: "pll_sys_fb".into(), input: 0 },
                Edge { from: "pll_sys_fb".into(), to: "pll_sys_pd1".into(), input: 0 },
                Edge { from: "pll_sys_pd1".into(), to: "pll_sys_pd2".into(), input: 0 },
                Edge { from: "pll_sys_pd2".into(), to: "clk_sys".into(), input: 0 },
                Edge { from: "clk_sys".into(), to: "clk_peri".into(), input: 0 },
                Edge { from: "xosc".into(), to: "pll_usb_fb".into(), input: 0 },
                Edge { from: "pll_usb_fb".into(), to: "pll_usb_pd1".into(), input: 0 },
                Edge { from: "pll_usb_pd1".into(), to: "pll_usb_pd2".into(), input: 0 },
                Edge { from: "pll_usb_pd2".into(), to: "clk_usb".into(), input: 0 },
                Edge { from: "xosc".into(), to: "clk_ref".into(), input: 0 },
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
            let text = ron::ser::to_string_pretty(&gc, ron::ser::PrettyConfig::new())
                .expect("serialise");
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
                    _ => {}
                }
            }
            let main_rs = mcu.fresh_main_rs();
            let files = project_gen::build_project_files(&def.project, &def.toolchain, &main_rs);
            // `sync_pin_files` keeps `src/pins/mod.rs` in a real project, and the
            // generated header declares `pub mod pins;` — so the harness has to
            // supply it too, or it compiles a project shape the app never
            // produces. Which is exactly what happened: the invariant test went
            // green while `cargo check` said "file not found for module `pins`".
            let mut user: Vec<(String, String)> = vec![
                ("src/pins/mod.rs".into(), "pub mod configs;
".into()),
                ("src/pins/configs/mod.rs".into(), String::new()),
            ];
            user.extend(mcu.config_files());
            let dir = std::env::temp_dir().join(dir_name);
            let _ = std::fs::remove_dir_all(&dir);
            project_gen::write_project(&dir, &files, &user, &mcu.mcu_config_text(), "")
                .expect("write rp project");
            println!("wrote {}", dir.display());
            println!("target: {}", def.project.target);
        }
    }
}
