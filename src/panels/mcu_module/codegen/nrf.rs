//! Nordic nRF52 family — `nrf52833-hal`, blocking.
//!
//! The definition (`assets/mcus/nrf52833_microbit_v2.ron`) is a BOARD, like the
//! Pico ones: the pads are the edge connector's, numbered as the silkscreen has
//! them, and each name leads with the nRF port and pin (`P0.21 (ROW1)`) so the
//! generated code can find the GPIO.
//!
//! Two facts about the silicon that this backend honors:
//!
//! - **Every signal routes to every pin.** There is no alternate-function
//!   table: a UARTE, SPIM, TWIM or PWM output is connected by writing the pin
//!   number into the peripheral's `PSEL` register. Every constructor therefore
//!   takes a DEGRADED pin (`Pin<Output<PushPull>>`, no pin number in the type),
//!   which is why the `init` signatures here name no pad. The definition still
//!   offers SPI and I2C only on the board's labeled pads, so autowire lands
//!   where accessories expect them.
//! - **SPIM0/SPIM1 share their peripheral IDs with TWIM0/TWIM1.** One block is
//!   either an SPI master or an I2C master, never both, which is why the
//!   definition offers SPI on SPIM2 and I2C on TWIM0/TWIM1.
//!
//! The clock block reads the Clock tab: which mux feeds HFCLK (the 64 MHz
//! internal oscillator, or the 32 MHz crystal doubled) and which feeds LFCLK
//! (the RC, synthesized from HFCLK, or a crystal when the tree has one). That
//! is the whole of what `nrf52833-hal`'s `Clocks` can set.

use super::common::{
    GEN_BEGIN, GEN_END, USER_TAIL, blank_separated, mcu_id_marker_line, retarget_pristine_tail,
    var_suffix,
};
use super::family::FamilyBackend;
use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::modules::UsartModuleConfig;
use crate::panels::mcu_module::pins::PinFunction;
use crate::panels::mcu_module::pins::logic::pin::GpioMode;

pub struct NrfBackend;

/// Whether `family` is one of Nordic's nRF52 parts.
///
/// A prefix, not a list: the family key is the chip (`nrf52833`), the way it
/// is on the ESP parts, and every nRF52 shares the HAL shape. An nRF52840
/// definition then needs no change here.
pub fn is_nrf(family: &str) -> bool {
    family.starts_with("nrf52")
}

/// `nrf52833_hal` — the HAL crate as it is written in Rust. One crate per
/// chip, named after it, so the family key IS the crate name.
fn hal_crate(family: &str) -> String {
    format!("{family}_hal")
}

/// A GPIO as (port, pin): `P0.21 (ROW1)` -> `(0, 21)`.
///
/// `pub(crate)` for the same reason `rp::gpio_index` is: anything else that
/// has to name the pin a pad carries must parse it the way the emitter does.
pub(crate) fn nrf_pin(name: &str) -> Option<(u8, u8)> {
    let head = name.split_whitespace().next()?;
    let (port, pin) = head.strip_prefix('P')?.split_once('.')?;
    let (port, pin): (u8, u8) = (port.parse().ok()?, pin.parse().ok()?);
    (port <= 1 && pin < 32).then_some((port, pin))
}

/// `P0.21` — how the generated comments name a pin.
fn label((port, pin): (u8, u8)) -> String {
    format!("P{port}.{pin:02}")
}

/// `p0_21` — the identifier a pin's binding is built from.
fn ident((port, pin): (u8, u8)) -> String {
    format!("p{port}_{pin:02}")
}

/// `port0.p0_21` — the field of the `Parts` struct that owns the pin.
fn field((port, pin): (u8, u8)) -> String {
    format!("port{port}.p{port}_{pin:02}")
}

/// The board's own name for a pad, for comments: `P0.21 (ROW1)` -> `ROW1`.
fn board_name(name: &str) -> Option<&str> {
    let start = name.find('(')? + 1;
    let end = name.rfind(')')?;
    (start < end).then(|| name[start..end].trim())
}

/// A pad the user wired but has not written code for yet is not a mistake -
/// it is a pad they are about to use. Same words the F1 and RP backends use.
const ALLOW: &str = "    #[allow(unused_mut, unused_variables)]
";

/// The two NFC antenna pins. On the nRF52833 they are NFC until the UICR's
/// `NFCPINS` register is cleared, and the UICR is flash: nothing this code
/// emits at run time can change it.
const NFC_PINS: [(u8, u8); 2] = [(0, 9), (0, 10)];

// ── Clock tab ───────────────────────────────────────────────────────────────

/// What the tree chose for the two clocks.
struct ClockChoice {
    /// HFCLK from the 32 MHz crystal (doubled) rather than the internal 64 MHz
    /// oscillator.
    hfxo: bool,
    lf: LfSource,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LfSource {
    Rc,
    Synth,
    Xtal,
}

/// The node feeding mux `mux` at the input its state selects.
fn mux_source(mcu: &Mcu, mux: &str) -> Option<String> {
    use crate::panels::mcu_module::clock::graph::model::NodeState;
    use crate::panels::mcu_module::clock::model::ClockConfig;
    let ClockConfig::Graph(gc) = &mcu.clock else {
        return None;
    };
    let idx = match gc.graph.node(mux).map(|n| &n.state) {
        Some(NodeState::Index(i)) => *i,
        _ => return None,
    };
    gc.graph
        .edges
        .iter()
        .find(|e| e.to == mux && e.input == idx)
        .map(|e| e.from.clone())
}

/// Read off the tree by following each mux's selected edge back to its
/// source, so the choice survives a node being renamed in the editor as long
/// as the mux keeps its id. A tree with no mux (or no tree) means the reset
/// defaults: internal oscillator and internal RC.
fn clock_choice(mcu: &Mcu) -> ClockChoice {
    let hf = mux_source(mcu, "hfclk_src");
    let lf = mux_source(mcu, "lfclk_src");
    ClockChoice {
        hfxo: hf.as_deref().is_some_and(|s| s.starts_with("hfxo")),
        lf: match lf.as_deref() {
            Some("lfsynth") => LfSource::Synth,
            Some("lfxo") => LfSource::Xtal,
            _ => LfSource::Rc,
        },
    }
}

fn clock_lines(mcu: &Mcu, hal: &str) -> String {
    let c = clock_choice(mcu);
    let mut o = String::new();
    o.push_str("    // From the Clock tab. HFCLK is 64 MHz either way; the choice is whether\n");
    o.push_str("    // the 32 MHz crystal is started, which the radio and USB need. LFCLK is\n");
    o.push_str("    // 32.768 kHz from whichever source the tree selects.\n");
    o.push_str(&format!(
        "    let clocks = {hal}::clocks::Clocks::new(p.CLOCK)\n"
    ));
    if c.hfxo {
        o.push_str("        .enable_ext_hfosc()\n");
    }
    match c.lf {
        LfSource::Rc => o.push_str("        .set_lfclk_src_rc()\n"),
        LfSource::Synth => o.push_str("        .set_lfclk_src_synth()\n"),
        LfSource::Xtal => o.push_str(&format!(
            "        .set_lfclk_src_external({hal}::clocks::LfOscConfiguration::NoExternalNoBypass)\n"
        )),
    }
    o.push_str("        .start_lfclk();\n");
    o.push_str("    let _ = &clocks;\n\n");
    o
}

// ── GPIO ────────────────────────────────────────────────────────────────────

fn gpio_lines(mcu: &Mcu, hal: &str) -> String {
    let mut pins_out: Vec<String> = Vec::new();
    for p in mcu.iter_all_pins().filter(|p| !p.reserved) {
        let Some(pp) = nrf_pin(&p.name) else {
            continue;
        };
        let sfx = var_suffix(&p.selected_function);
        let var = format!("{}_{sfx}", ident(pp));
        let note = board_name(&p.name).map_or(String::new(), |b| format!(" ({b})"));
        match p.selected_function {
            PinFunction::GpioOutput => {
                // `for_output` / `for_input`: the default is the FIRST mode the
                // panel offers (push-pull, floating), and a mode left over from
                // the other direction is ignored rather than misread.
                let ctor = match GpioMode::for_output(p.io_mode) {
                    GpioMode::OpenDrain => format!(
                        "into_open_drain_output({hal}::gpio::OpenDrainConfig::Standard0Disconnect1, {hal}::gpio::Level::High)"
                    ),
                    _ => format!("into_push_pull_output({hal}::gpio::Level::Low)"),
                };
                pins_out.push(format!(
                    "    // {}{note}\n{ALLOW}    let mut {var} = {}.{ctor};\n",
                    label(pp),
                    field(pp)
                ));
            }
            PinFunction::GpioInput => {
                let ctor = match GpioMode::for_input(p.io_mode) {
                    GpioMode::PullUp => "into_pullup_input()",
                    GpioMode::PullDown => "into_pulldown_input()",
                    _ => "into_floating_input()",
                };
                let mut entry = format!(
                    "    // {}{note}\n{ALLOW}    let {var} = {}.{ctor};\n",
                    label(pp),
                    field(pp)
                );
                // The edge is set in the pin panel on every runtime. Nothing
                // here acts on it yet, and saying so beats dropping it.
                if p.irq.is_some() {
                    entry.push_str(&format!(
                        "    // {} is armed for an interrupt, which this Blocking project does\n    // not generate: poll it, or wire GPIOTE by hand.\n",
                        label(pp)
                    ));
                }
                pins_out.push(entry);
            }
            _ => {}
        }
    }
    blank_separated(pins_out)
}

/// The NFC pads, if anything is wired to them.
///
/// The definition offers them as GPIO because they ARE GPIO once the UICR
/// says so, and the HAL cannot say so at run time. Left unsaid, a button on
/// pad 8 reads as a dead input with no error anywhere.
fn nfc_note(mcu: &Mcu) -> String {
    let used: Vec<String> = mcu
        .iter_all_pins()
        .filter(|p| !p.reserved && p.selected_function != PinFunction::Unset)
        .filter_map(|p| nrf_pin(&p.name))
        .filter(|pp| NFC_PINS.contains(pp))
        .map(label)
        .collect();
    if used.is_empty() {
        return String::new();
    }
    format!(
        "    // {} {} the NFC antenna pins, and stay NFC until bit 0 of the UICR's\n    // NFCPINS register (0x1000120C) is cleared. The UICR is flash, written\n    // through the NVMC, and nrf-hal has no helper: clear it once from code\n    // (NVMC.CONFIG = WEN, write 0xFFFFFFFE to 0x1000120C, wait for NVMC.READY,\n    // CONFIG = REN, then reset). A debug probe can run the same sequence, but a\n    // lone write to the UICR without CONFIG = WEN first is ignored.\n",
        used.join(" and "),
        if used.len() == 1 { "is one of" } else { "are" }
    )
}

// ── Buses ───────────────────────────────────────────────────────────────────

/// Which pin carries each role of one bus instance.
fn bus_pins(
    mcu: &Mcu,
    want: impl Fn(&PinFunction) -> Option<(u8, &'static str)>,
) -> Vec<(u8, &'static str, (u8, u8))> {
    let mut out = Vec::new();
    for p in mcu.iter_all_pins().filter(|p| !p.reserved) {
        let Some(pp) = nrf_pin(&p.name) else {
            continue;
        };
        if let Some((inst, role)) = want(&p.selected_function) {
            out.push((inst, role, pp));
        }
    }
    // Sorted so that, when two pads claim one role, the lowest pin wins - the
    // same pad `ambiguity_notes` names as the one configured.
    out.sort_unstable();
    out
}

fn role_of(pins: &[(u8, &'static str, (u8, u8))], inst: u8, role: &str) -> Option<(u8, u8)> {
    pins.iter()
        .find(|(i, r, _)| *i == inst && *r == role)
        .map(|(_, _, pp)| *pp)
}

fn instances(pins: &[(u8, &'static str, (u8, u8))]) -> Vec<u8> {
    let mut v: Vec<u8> = pins.iter().map(|(i, _, _)| *i).collect();
    v.dedup();
    v
}

fn uart_pins(mcu: &Mcu) -> Vec<(u8, &'static str, (u8, u8))> {
    bus_pins(mcu, |f| match f {
        PinFunction::UsartTx(i) => Some((*i, "txd")),
        PinFunction::UsartRx(i) => Some((*i, "rxd")),
        PinFunction::UsartCts(i) => Some((*i, "cts")),
        PinFunction::UsartRts(i) => Some((*i, "rts")),
        _ => None,
    })
}

fn spi_pins(mcu: &Mcu) -> Vec<(u8, &'static str, (u8, u8))> {
    bus_pins(mcu, |f| match f {
        PinFunction::SpiSck(i) => Some((*i, "sck")),
        PinFunction::SpiMosi(i) => Some((*i, "mosi")),
        PinFunction::SpiMiso(i) => Some((*i, "miso")),
        _ => None,
    })
}

fn i2c_pins(mcu: &Mcu) -> Vec<(u8, &'static str, (u8, u8))> {
    bus_pins(mcu, |f| match f {
        PinFunction::I2cSda(i) => Some((*i, "sda")),
        PinFunction::I2cScl(i) => Some((*i, "scl")),
        _ => None,
    })
}

/// The signal a pin carries, as one name: `"UARTE0 TXD"`, `"PWM0 channel 2"`.
///
/// Two pads CAN claim one signal here more easily than anywhere else: every
/// pad offers every instance, so nothing in the definition stops a user from
/// putting UARTE0 TXD on two pads. The HAL takes one.
fn signal_name(f: &PinFunction) -> Option<String> {
    Some(match f {
        PinFunction::UsartTx(i) => format!("UARTE{i} TXD"),
        PinFunction::UsartRx(i) => format!("UARTE{i} RXD"),
        PinFunction::UsartCts(i) => format!("UARTE{i} CTS"),
        PinFunction::UsartRts(i) => format!("UARTE{i} RTS"),
        PinFunction::SpiSck(i) => format!("SPIM{i} SCK"),
        PinFunction::SpiMosi(i) => format!("SPIM{i} MOSI"),
        PinFunction::SpiMiso(i) => format!("SPIM{i} MISO"),
        PinFunction::I2cSda(i) => format!("TWIM{i} SDA"),
        PinFunction::I2cScl(i) => format!("TWIM{i} SCL"),
        PinFunction::TimerPwm { timer, channel } => format!("PWM{timer} channel {channel}"),
        _ => return None,
    })
}

/// Say so when two pads claim one signal; the lowest pin is the one
/// configured, and the note names the rest.
fn ambiguity_notes(mcu: &Mcu) -> String {
    let mut by_signal: std::collections::BTreeMap<String, Vec<(u8, u8)>> =
        std::collections::BTreeMap::new();
    for p in mcu.iter_all_pins().filter(|p| !p.reserved) {
        let (Some(pp), Some(sig)) = (nrf_pin(&p.name), signal_name(&p.selected_function)) else {
            continue;
        };
        by_signal.entry(sig).or_default().push(pp);
    }
    let mut o = String::new();
    for (sig, mut pads) in by_signal {
        if pads.len() < 2 {
            continue;
        }
        pads.sort_unstable();
        let used = label(pads[0]);
        let rest: Vec<String> = pads[1..].iter().map(|pp| label(*pp)).collect();
        o.push_str(&format!(
            "    // {sig} is wired to {used} and {}. Only {used} is configured:\n",
            rest.join(" and ")
        ));
        o.push_str(
            "    // the PSEL register holds one pin. Unassign the other on the Pins canvas.\n",
        );
    }
    o
}

/// `port0.p0_06.into_push_pull_output(Level::High).degrade()` — an output
/// handed to a peripheral, at the level the line idles at until the block
/// takes it over: High for a UART's TXD and RTS, Low for a clock, a data line
/// or a PWM pad (a speaker pad driven High for a moment is a click).
fn out_arg(hal: &str, pp: (u8, u8), level: &str) -> String {
    format!(
        "{}.into_push_pull_output({hal}::gpio::Level::{level}).degrade()",
        field(pp)
    )
}

/// `port1.p1_08.into_floating_input().degrade()` — an input handed to a
/// peripheral.
fn in_arg(pp: (u8, u8)) -> String {
    format!("{}.into_floating_input().degrade()", field(pp))
}

fn opt(arg: Option<String>) -> String {
    arg.map_or("None".to_owned(), |a| format!("Some({a})"))
}

/// UARTE, SPIM and TWIM, in that order.
///
/// A UARTE needs both TXD and RXD (`uarte::Pins` has no `Option` for them);
/// CTS and RTS ride along when wired, and the HAL turns flow control on only
/// when it has BOTH. A SPIM needs SCK and takes MOSI and MISO as options. A
/// TWIM needs both lines.
fn bus_lines(mcu: &Mcu, hal: &str) -> String {
    let mut o = ambiguity_notes(mcu);

    let uart = uart_pins(mcu);
    for i in instances(&uart) {
        let (Some(txd), Some(rxd)) = (role_of(&uart, i, "txd"), role_of(&uart, i, "rxd")) else {
            o.push_str(&format!(
                "    // UARTE{i}: only one of TXD/RXD is wired, and the constructor takes the\n    // pair. Wire the other pad on the Pins canvas.\n"
            ));
            continue;
        };
        let cts = role_of(&uart, i, "cts").map(in_arg);
        let rts = role_of(&uart, i, "rts").map(|pp| out_arg(hal, pp, "High"));
        o.push_str(&format!(
            "    let uarte{i} = pins::configs::uarte{i}::init(\n        p.UARTE{i},\n        {},\n        {},\n        {},\n        {},\n    );\n    let _ = &uarte{i};\n",
            out_arg(hal, txd, "High"),
            in_arg(rxd),
            opt(cts),
            opt(rts),
        ));
    }

    let spi = spi_pins(mcu);
    for i in instances(&spi) {
        let Some(sck) = role_of(&spi, i, "sck") else {
            o.push_str(&format!(
                "    // SPIM{i}: SCK is not wired, and a SPIM without a clock is nothing.\n    // MOSI and MISO are each optional; SCK is not.\n"
            ));
            continue;
        };
        let mosi = role_of(&spi, i, "mosi").map(|pp| out_arg(hal, pp, "Low"));
        let miso = role_of(&spi, i, "miso").map(in_arg);
        o.push_str(&format!(
            "    let spim{i} = pins::configs::spim{i}::init(\n        p.SPIM{i},\n        {},\n        {},\n        {},\n    );\n    let _ = &spim{i};\n",
            out_arg(hal, sck, "Low"),
            opt(mosi),
            opt(miso),
        ));
    }

    let i2c = i2c_pins(mcu);
    for i in instances(&i2c) {
        let (Some(scl), Some(sda)) = (role_of(&i2c, i, "scl"), role_of(&i2c, i, "sda")) else {
            o.push_str(&format!(
                "    // TWIM{i}: SCL and SDA are taken together; wire the missing one.\n"
            ));
            continue;
        };
        o.push_str(&format!(
            "    let twim{i} = pins::configs::twim{i}::init(\n        p.TWIM{i},\n        {},\n        {},\n    );\n    let _ = &twim{i};\n",
            in_arg(scl),
            in_arg(sda),
        ));
    }
    o
}

// ── PWM and SAADC ───────────────────────────────────────────────────────────

/// One block's wired channels: `(channel, pin)`, ascending.
type PwmChannels = Vec<(u8, (u8, u8))>;

/// Every wired PWM channel, by instance, the lowest pin winning a channel two
/// pads claim (see `ambiguity_notes`).
fn pwm_channels(mcu: &Mcu) -> std::collections::BTreeMap<u8, PwmChannels> {
    let mut all: Vec<(u8, u8, (u8, u8))> = mcu
        .iter_all_pins()
        .filter(|p| !p.reserved)
        .filter_map(|p| match p.selected_function {
            PinFunction::TimerPwm { timer, channel } => {
                nrf_pin(&p.name).map(|pp| (timer, channel, pp))
            }
            _ => None,
        })
        .collect();
    all.sort_unstable();
    let mut by_inst: std::collections::BTreeMap<u8, PwmChannels> =
        std::collections::BTreeMap::new();
    for (inst, ch, pp) in all {
        let chans = by_inst.entry(inst).or_default();
        if chans.iter().all(|(c, _)| *c != ch) {
            chans.push((ch, pp));
        }
    }
    by_inst
}

fn pwm_adc_lines(mcu: &Mcu, hal: &str) -> String {
    let mut o = String::new();

    for (inst, chans) in pwm_channels(mcu) {
        let args: Vec<String> = chans
            .iter()
            .map(|(_, pp)| format!("        {},\n", out_arg(hal, *pp, "Low")))
            .collect();
        o.push_str(&format!(
            "    let pwm{inst} = pins::configs::pwm{inst}::init(\n        p.PWM{inst},\n{}    );\n    let _ = &pwm{inst};\n",
            args.join("")
        ));
    }

    let mut adc: Vec<(u8, (u8, u8), String)> = mcu
        .iter_all_pins()
        .filter(|p| !p.reserved)
        .filter_map(|p| match p.selected_function {
            PinFunction::AdcChannel { channel, .. } => {
                nrf_pin(&p.name).map(|pp| (channel, pp, var_suffix(&p.selected_function)))
            }
            _ => None,
        })
        .collect();
    adc.sort_unstable();
    if !adc.is_empty() {
        o.push_str("    // One SAADC for every analog input; a read names the pin it samples.\n");
        o.push_str(&format!(
            "    let mut saadc = {hal}::saadc::Saadc::new(p.SAADC, {hal}::saadc::SaadcConfig::default());\n"
        ));
        o.push_str("    let _ = &mut saadc;\n");
        for (channel, pp, sfx) in &adc {
            let var = format!("{}_{sfx}", ident(*pp));
            o.push_str(&format!(
                "    // AIN{channel} on {}. Read it with `saadc.read_channel(&mut {var})`.\n",
                label(*pp)
            ));
            // NOT degraded: the SAADC channel is a property of the typed pin.
            o.push_str(&format!(
                "    let mut {var} = {}.into_floating_input();\n    let _ = &mut {var};\n",
                field(*pp)
            ));
        }
    }
    o
}

// ── The generated region ────────────────────────────────────────────────────

fn section(mcu: &Mcu) -> String {
    let hal = hal_crate(&mcu.family);
    let mut o = String::new();
    o.push_str(GEN_BEGIN);
    o.push('\n');
    o.push_str("#[cortex_m_rt::entry]\n");
    o.push_str("fn main() -> ! {\n");
    o.push_str(&format!(
        "    let p = {hal}::pac::Peripherals::take().unwrap();\n\n"
    ));
    o.push_str(&clock_lines(mcu, &hal));
    // Port 1 only where the definition names a P1 pin: the nRF52832, 52810 and
    // 52811 have P0 alone, and their HALs have no `p1` module to take.
    let has_p1 = mcu
        .iter_all_pins()
        .any(|p| nrf_pin(&p.name).is_some_and(|(port, _)| port == 1));
    if has_p1 {
        o.push_str("    // Both ports, taken once. Every pad below is moved out of one of them.\n");
    } else {
        o.push_str("    // The port, taken once. Every pad below is moved out of it.\n");
    }
    o.push_str("    #[allow(unused_variables)]\n");
    o.push_str(&format!(
        "    let port0 = {hal}::gpio::p0::Parts::new(p.P0);\n"
    ));
    if has_p1 {
        o.push_str("    #[allow(unused_variables)]\n");
        o.push_str(&format!(
            "    let port1 = {hal}::gpio::p1::Parts::new(p.P1);\n"
        ));
    }
    o.push('\n');
    o.push_str(&nfc_note(mcu));
    let gpio = gpio_lines(mcu, &hal);
    let rest = format!("{}{}", bus_lines(mcu, &hal), pwm_adc_lines(mcu, &hal));
    o.push_str(&gpio);
    if !gpio.is_empty() && !rest.is_empty() {
        o.push('\n');
    }
    o.push_str(&rest);
    o.push_str(GEN_END);
    o.push('\n');
    o
}

fn header(mcu: &Mcu) -> String {
    format!(
        "// Auto-generated by Embedded IDE\n\
         // MCU: {} | HAL: {}-hal (blocking)\n\
         {}\n\
         #![no_std]\n\
         #![no_main]\n\
         \n\
         pub mod pins;\n\
         \n\
         use panic_halt as _;\n\
         // Where `set_high` / `is_low` / `toggle` come from: nrf-hal implements\n\
         // embedded-hal 1.0 and does not re-export it.\n\
         #[allow(unused_imports)]\n\
         use embedded_hal::digital::{{InputPin, OutputPin, StatefulOutputPin}};\n\
         \n",
        mcu.name,
        mcu.family,
        mcu_id_marker_line(&mcu.id),
    )
}

// ── src/pins/configs/*.rs ───────────────────────────────────────────────────

/// The speed this bus runs at, as the Virtual Module has it.
fn bus_speed(mcu: &Mcu, kind: &str, n: u8) -> u32 {
    use crate::panels::mcu_module::modules;
    match kind {
        "uarte" => modules::usart_configs(&mcu.modules)
            .get(&n)
            .map_or(115_200, |c| c.baud_rate),
        "spim" => modules::spi_configs(&mcu.modules)
            .get(&n)
            .map_or(1_000_000, |c| c.clock_hz),
        _ => modules::i2c_configs(&mcu.modules)
            .get(&n)
            .map_or(400_000, |c| c.clock_hz),
    }
}

/// The UARTE's fixed baud settings, as the PAC names them.
const BAUDS: [(u32, &str); 18] = [
    (1_200, "BAUD1200"),
    (2_400, "BAUD2400"),
    (4_800, "BAUD4800"),
    (9_600, "BAUD9600"),
    (14_400, "BAUD14400"),
    (19_200, "BAUD19200"),
    (28_800, "BAUD28800"),
    (31_250, "BAUD31250"),
    (38_400, "BAUD38400"),
    (56_000, "BAUD56000"),
    (57_600, "BAUD57600"),
    (76_800, "BAUD76800"),
    (115_200, "BAUD115200"),
    (230_400, "BAUD230400"),
    (250_000, "BAUD250000"),
    (460_800, "BAUD460800"),
    (921_600, "BAUD921600"),
    (1_000_000, "BAUD1M"),
];

/// The nearest baud the UARTE has.
fn baud_variant(hz: u32) -> (u32, &'static str) {
    BAUDS
        .iter()
        .copied()
        .min_by_key(|(b, _)| b.abs_diff(hz))
        .unwrap_or((115_200, "BAUD115200"))
}

/// The highest SPIM rate at or below `hz`. SPIM0..2 top out at 8 MHz; the
/// PAC's M16 and M32 belong to SPIM3, which the definition does not offer.
fn spim_frequency(hz: u32) -> (u32, &'static str) {
    const RATES: [(u32, &str); 7] = [
        (125_000, "K125"),
        (250_000, "K250"),
        (500_000, "K500"),
        (1_000_000, "M1"),
        (2_000_000, "M2"),
        (4_000_000, "M4"),
        (8_000_000, "M8"),
    ];
    RATES
        .iter()
        .copied()
        .rev()
        .find(|(r, _)| *r <= hz)
        .unwrap_or(RATES[0])
}

/// The highest TWIM rate at or below `hz`.
fn twim_frequency(hz: u32) -> (u32, &'static str) {
    const RATES: [(u32, &str); 3] = [(100_000, "K100"), (250_000, "K250"), (400_000, "K400")];
    RATES
        .iter()
        .copied()
        .rev()
        .find(|(r, _)| *r <= hz)
        .unwrap_or(RATES[0])
}

fn bus_config_file(
    hal: &str,
    kind: &str,
    n: u8,
    hz: u32,
    frame: Option<&UsartModuleConfig>,
    spi_mode: u8,
) -> String {
    use crate::panels::mcu_module::modules::{Parity, StopBits};
    let mut o = String::new();
    o.push_str("// <<< GENERATED>>>\n");
    o.push_str(
        "// Peripheral config (from the Virtual Module) — auto-updated; edit in the module.\n",
    );
    match kind {
        "uarte" => {
            let (got, variant) = baud_variant(hz);
            if got != hz {
                o.push_str(&format!(
                    "// {hz} baud asked for; the UARTE has fixed rates and {got} is the nearest.\n"
                ));
            }
            o.push_str(&format!(
                "pub const BAUDRATE: {hal}::uarte::Baudrate = {hal}::uarte::Baudrate::{variant};\n"
            ));
            let d = UsartModuleConfig::new(0);
            let c = frame.unwrap_or(&d);
            let parity = match c.parity {
                Parity::None => "EXCLUDED",
                Parity::Even => "INCLUDED",
                Parity::Odd => {
                    o.push_str("// Odd parity asked for; the UARTE generates even parity only.\n");
                    "INCLUDED"
                }
            };
            o.push_str(&format!(
                "pub const PARITY: {hal}::uarte::Parity = {hal}::uarte::Parity::{parity};\n"
            ));
            if c.data_bits != 8 || c.stop_bits != StopBits::One {
                o.push_str("// The UARTE frames 8 data bits and 1 stop bit; the module's other\n// setting is not reachable through the HAL.\n");
            }
        }
        "spim" => {
            let (got, variant) = spim_frequency(hz);
            if got != hz {
                o.push_str(&format!(
                    "// {hz} Hz asked for; the SPIM has fixed rates and {got} is the highest at or below it.\n"
                ));
            }
            o.push_str(&format!(
                "pub const FREQUENCY: {hal}::spim::Frequency = {hal}::spim::Frequency::{variant};\n"
            ));
            // In the regenerated half, because the Virtual Module owns it. It
            // sat below the markers as a fixed `MODE_0` at first, so the
            // panel's mode combo changed nothing on this chip.
            o.push_str(&format!(
                "/// Mode {mode}, as the Virtual Module has it.\npub const MODE: {hal}::spim::Mode = {hal}::spim::MODE_{mode};\n",
                mode = spi_mode.min(3)
            ));
        }
        _ => {
            let (got, variant) = twim_frequency(hz);
            if got != hz {
                o.push_str(&format!(
                    "// {hz} Hz asked for; the TWIM has fixed rates and {got} is the highest at or below it.\n"
                ));
            }
            o.push_str(&format!(
                "pub const FREQUENCY: {hal}::twim::Frequency = {hal}::twim::Frequency::{variant};\n"
            ));
        }
    }
    o.push_str("// <<< GENERATED END >>>\n\n");
    o.push_str("// Everything below is editable — your changes are preserved on regeneration.\n");
    // The TWIM takes two inputs and nothing else, and an unused import is a
    // warning the matrix counts as a failure.
    if kind == "twim" {
        o.push_str(&format!("use {hal}::gpio::{{Floating, Input, Pin}};\n\n"));
    } else {
        o.push_str(&format!(
            "use {hal}::gpio::{{Floating, Input, Output, Pin, PushPull}};\n\n"
        ));
    }
    match kind {
        "uarte" => {
            o.push_str(&format!(
                "/// The concrete type `init` hands back, so it can be a struct field.\npub type Handle = {hal}::uarte::Uarte<{hal}::pac::UARTE{n}>;\n\n"
            ));
            o.push_str(&format!(
                "/// UARTE{n} at BAUDRATE. Any pin can carry any role (the PSEL registers\n/// hold pin numbers), so `main.rs` picks them. Flow control is on only when\n/// BOTH `cts` and `rts` are given: the HAL sets HWFC from the pair.\npub fn init(\n    uarte: {hal}::pac::UARTE{n},\n    txd: Pin<Output<PushPull>>,\n    rxd: Pin<Input<Floating>>,\n    cts: Option<Pin<Input<Floating>>>,\n    rts: Option<Pin<Output<PushPull>>>,\n) -> Handle {{\n    {hal}::uarte::Uarte::new(\n        uarte,\n        {hal}::uarte::Pins {{ rxd, txd, cts, rts }},\n        PARITY,\n        BAUDRATE,\n    )\n}}\n"
            ));
        }
        "spim" => {
            o.push_str(&format!(
                "/// The concrete type `init` hands back.\npub type Handle = {hal}::spim::Spim<{hal}::pac::SPIM{n}>;\n\n"
            ));
            o.push_str(
                "/// The byte clocked out while only receiving.\npub const ORC: u8 = 0x00;\n\n",
            );
            o.push_str(&format!(
                "/// SPIM{n} at FREQUENCY. SCK is required; MOSI and MISO are each optional,\n/// which is how a write-only or read-only bus is built. Chip select is a\n/// plain GPIO output, driven by the caller.\npub fn init(\n    spim: {hal}::pac::SPIM{n},\n    sck: Pin<Output<PushPull>>,\n    mosi: Option<Pin<Output<PushPull>>>,\n    miso: Option<Pin<Input<Floating>>>,\n) -> Handle {{\n    {hal}::spim::Spim::new(\n        spim,\n        {hal}::spim::Pins {{\n            sck: Some(sck),\n            mosi,\n            miso,\n        }},\n        FREQUENCY,\n        MODE,\n        ORC,\n    )\n}}\n"
            ));
        }
        _ => {
            o.push_str(&format!(
                "/// The concrete type `init` hands back.\npub type Handle = {hal}::twim::Twim<{hal}::pac::TWIM{n}>;\n\n"
            ));
            o.push_str(&format!(
                "/// TWIM{n} at FREQUENCY. Both lines are inputs to the GPIO block: the\n/// TWIM drives them open-drain itself, and the pull-ups are on the bus.\npub fn init(\n    twim: {hal}::pac::TWIM{n},\n    scl: Pin<Input<Floating>>,\n    sda: Pin<Input<Floating>>,\n) -> Handle {{\n    {hal}::twim::Twim::new(twim, {hal}::twim::Pins {{ scl, sda }}, FREQUENCY)\n}}\n"
            ));
        }
    }
    o
}

/// The PWM counter's base clock, before the prescaler.
const PWM_CLOCK_HZ: u32 = 16_000_000;
/// The highest COUNTERTOP the block takes.
const PWM_MAX_TOP: u32 = 32_767;

/// `(divider, Prescaler name)` for a PWM asked to run at `freq_hz`: the
/// smallest divider whose counter top fits in 15 bits, so the duty keeps as
/// much resolution as the frequency allows.
fn pwm_prescaler(freq_hz: u32) -> (u32, &'static str) {
    const DIVS: [(u32, &str); 8] = [
        (1, "Div1"),
        (2, "Div2"),
        (4, "Div4"),
        (8, "Div8"),
        (16, "Div16"),
        (32, "Div32"),
        (64, "Div64"),
        (128, "Div128"),
    ];
    let freq = freq_hz.max(1);
    DIVS.iter()
        .copied()
        .find(|(d, _)| PWM_CLOCK_HZ / d / freq <= PWM_MAX_TOP)
        .unwrap_or(DIVS[7])
}

/// The frequency the pad really sees for `freq_hz` through `div`: the HAL's
/// `set_period` computes `top = clock / div / freq` in integers, and the
/// output is `clock / div / top`, clamped where `top` would overflow.
fn pwm_actual_hz(freq_hz: u32, div: u32) -> u32 {
    let clk = PWM_CLOCK_HZ / div;
    let top = (clk / freq_hz.max(1)).clamp(1, PWM_MAX_TOP);
    clk / top
}

/// `src/pins/configs/pwm{n}.rs` — one PWM block, its frequency and the duty
/// of each channel it drives.
fn pwm_config_file(mcu: &Mcu, inst: u8, chans: &[(u8, (u8, u8))]) -> String {
    let hal = hal_crate(&mcu.family);
    let cfg = crate::panels::mcu_module::modules::timer_configs(&mcu.modules);
    let cfg = cfg.get(&inst);
    let want = cfg.map_or(0, |c| c.freq_hz);
    let mut o = String::new();

    o.push_str("// <<< GENERATED>>>\n");
    o.push_str(
        "// Peripheral config (from the Virtual Module) — auto-updated; edit in the module.\n",
    );
    if want > 0 {
        let (div, name) = pwm_prescaler(want);
        let got = pwm_actual_hz(want, div);
        o.push_str(&format!("pub const FREQ_HZ: u32 = {want};"));
        if got != want {
            o.push_str(&format!(" // the pad sees {got} Hz"));
        }
        o.push('\n');
        o.push_str(&format!(
            "/// {} MHz / {div}: the smallest divider that keeps the counter top within 15 bits.\npub const PRESCALER: {hal}::pwm::Prescaler = {hal}::pwm::Prescaler::{name};\n",
            PWM_CLOCK_HZ / 1_000_000
        ));
    } else {
        o.push_str("// No frequency set in the module: the HAL's default stands (16 MHz over a\n// 32767 top, about 488 Hz). Set one in the Virtual Module.\n");
        o.push_str("pub const FREQ_HZ: u32 = 0;\n");
        o.push_str(&format!(
            "pub const PRESCALER: {hal}::pwm::Prescaler = {hal}::pwm::Prescaler::Div1;\n"
        ));
    }
    o.push_str("// Duty per channel, in HUNDREDTHS of a percent — 750 is 7.5 %, which is what a\n");
    o.push_str("// hobby servo wants and what whole percent cannot say.\n");
    for (ch, _) in chans {
        let x100 = cfg.map_or(0, |c| c.duty_x100_of(*ch));
        o.push_str(&format!(
            "pub const DUTY_C{ch}_X100: u32 = {x100}; // {} %\n",
            super::common::duty_percent_str(x100)
        ));
    }
    o.push_str("// <<< GENERATED END >>>\n\n");

    o.push_str("// Everything below is editable — your changes are preserved on regeneration.\n");
    o.push_str(&format!(
        "use {hal}::gpio::{{Output, Pin, PushPull}};\nuse {hal}::pwm::Channel;\n\n"
    ));
    o.push_str(&format!(
        "/// The concrete type `init` hands back. The block owns its four channels;\n/// each wired one is routed to a pad below.\npub type Handle = {hal}::pwm::Pwm<{hal}::pac::PWM{inst}>;\n\n"
    ));

    let params: Vec<String> = chans
        .iter()
        .map(|(ch, pp)| {
            format!(
                "    // channel {ch}, on {}\n    c{ch}: Pin<Output<PushPull>>,\n",
                label(*pp)
            )
        })
        .collect();
    o.push_str(&format!(
        "/// Bring the block up at FREQ_HZ and route each wired channel to its pad.\npub fn init(\n    pwm: {hal}::pac::PWM{inst},\n{}) -> Handle {{\n    let pwm = {hal}::pwm::Pwm::new(pwm);\n",
        params.join("")
    ));
    if want > 0 {
        o.push_str("    // The prescaler first: `set_period` derives the counter top from it.\n");
        o.push_str("    pwm.set_prescaler(PRESCALER);\n");
        o.push_str(&format!(
            "    pwm.set_period({hal}::time::Hertz(FREQ_HZ));\n"
        ));
    }
    for (ch, _) in chans {
        o.push_str(&format!("    pwm.set_output_pin(Channel::C{ch}, c{ch});\n"));
    }
    o.push_str("    pwm.enable();\n");
    o.push_str("    let max = pwm.max_duty() as u32;\n");
    for (ch, _) in chans {
        o.push_str(&format!(
            "    pwm.set_duty_on(Channel::C{ch}, (max * DUTY_C{ch}_X100 / 10_000) as u16);\n"
        ));
    }
    o.push_str("    pwm\n}\n\n");

    let first = chans.first().map_or(0, |(c, _)| *c);
    o.push_str("/// Set a channel's duty in the same units the `DUTY_*` constants above use —\n");
    o.push_str("/// HUNDREDTHS of a percent, so `10_000` is 100 % and `750` is 7.5 %.\n///\n");
    o.push_str("/// A trait rather than an inherent method because `Handle` is nrf-hal's own\n");
    o.push_str("/// type, which this crate does not own. One method per WIRED channel: the\n");
    o.push_str("/// channel is part of the NAME rather than an argument, so a channel this\n");
    o.push_str("/// block has no pad for cannot be asked for at all.\n");
    o.push_str("pub trait DutyHandle {\n");
    o.push_str(&format!(
        "    /// Channel {first}, the first one wired to this block.\n    fn set_duty_pwm_{inst}(&mut self, value: u32);\n"
    ));
    for (ch, _) in chans {
        o.push_str(&format!(
            "\n    /// Channel {ch}.\n    fn set_duty_pwm_{inst}_c{ch}(&mut self, value: u32);\n"
        ));
    }
    o.push_str("}\n\nimpl DutyHandle for Handle {\n");
    o.push_str(&format!(
        "    fn set_duty_pwm_{inst}(&mut self, value: u32) {{\n        self.set_duty_pwm_{inst}_c{first}(value);\n    }}\n"
    ));
    for (ch, _) in chans {
        o.push_str(&format!(
            "\n    fn set_duty_pwm_{inst}_c{ch}(&mut self, value: u32) {{\n        let max = self.max_duty() as u32;\n        self.set_duty_on(Channel::C{ch}, (max * value / 10_000) as u16);\n    }}\n"
        ));
    }
    o.push_str("}\n");
    o
}

impl FamilyBackend for NrfBackend {
    fn family_id(&self) -> &'static str {
        "nrf52833"
    }

    fn handles(&self, family: &str) -> bool {
        is_nrf(family)
    }

    // `gpio_modes` is the trait default: nrf-hal has all three input pulls and
    // both output drives as `into_*` methods, which is exactly the full set.

    fn config_files(&self, mcu: &Mcu) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = pwm_channels(mcu)
            .into_iter()
            .map(|(inst, chans)| (format!("pwm{inst}.rs"), pwm_config_file(mcu, inst, &chans)))
            .collect();

        let hal = hal_crate(&mcu.family);
        let ucfgs = crate::panels::mcu_module::modules::usart_configs(&mcu.modules);
        let scfgs = crate::panels::mcu_module::modules::spi_configs(&mcu.modules);
        // Only a bus `main.rs` can construct gets a file - the same rule
        // `bus_lines` applies, so a file never exists without its `init` call.
        for (kind, required, pins) in [
            ("uarte", &["txd", "rxd"][..], uart_pins(mcu)),
            ("spim", &["sck"][..], spi_pins(mcu)),
            ("twim", &["scl", "sda"][..], i2c_pins(mcu)),
        ] {
            for i in instances(&pins) {
                if required.iter().all(|r| role_of(&pins, i, r).is_some()) {
                    let frame = (kind == "uarte").then(|| ucfgs.get(&i)).flatten();
                    let spi_mode = scfgs.get(&i).map_or(0, |c| c.mode);
                    out.push((
                        format!("{kind}{i}.rs"),
                        bus_config_file(&hal, kind, i, bus_speed(mcu, kind, i), frame, spi_mode),
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
    /// side of it. The same splice as the RP backend, for the same reason:
    /// `embassy_async::splice_section` rewrites the header too.
    fn update_main_rs(&self, mcu: &Mcu, existing: &str) -> String {
        let (Some(begin), Some(end_start)) = (existing.find(GEN_BEGIN), existing.find(GEN_END))
        else {
            return self.fresh_main_rs(mcu);
        };
        let end = end_start + GEN_END.len();
        format!(
            "{}{}{}",
            &existing[..begin],
            section(mcu).trim_end_matches('\n'),
            retarget_pristine_tail(&existing[end..], false)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nrf52_parts_are_nrf_and_nothing_else_is() {
        assert!(is_nrf("nrf52833"));
        assert!(is_nrf("nrf52840"));
        for other in ["nrf51", "nrf5340", "rp2040", "stm32f4", "esp32c3", ""] {
            assert!(!is_nrf(other), "{other:?} matched");
        }
    }

    /// The BBC micro:bit v2 definition must parse, build, and keep the facts
    /// the rest of the IDE reads straight off it. Not a built-in yet, so no
    /// other test loads it - and a `.ron` nobody loads is documentation that
    /// rots.
    #[test]
    fn the_microbit_v2_definition_parses_and_builds() {
        use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
        use crate::panels::mcu_module::mcu_def::McuDefinition;

        const SRC: &str = include_str!("../../../../assets/mcus/nrf52833_microbit_v2.ron");
        let def: McuDefinition =
            ron::from_str(SRC).expect("the micro:bit v2 definition must parse");

        assert!(is_nrf(&def.family));
        assert_eq!(def.board_chip.as_deref(), Some("nRF52833"));
        assert_eq!(def.toolchain, ToolchainKind::RustEmbedded);
        let p = &def.project;
        assert_eq!(p.target, "thumbv7em-none-eabihf");
        assert_eq!(p.probe_chip, "nRF52833_xxAA");
        assert_eq!(
            (p.flash_origin.as_str(), p.flash_size.as_str()),
            ("0x00000000", "512K")
        );
        assert_eq!(
            (p.ram_origin.as_str(), p.ram_size.as_str()),
            ("0x20000000", "128K")
        );
        assert!(
            p.hal_dep_async
                .as_deref()
                .is_some_and(|l| l.contains("nfc-pins-as-gpio")),
            "pads 8 and 9 do not exist in embassy-nrf without it"
        );

        assert_eq!(def.pins.bottom.len(), 25, "25 edge-connector contacts");
        let mcu = def.build_mcu();
        assert_eq!(mcu.iter_all_pins().count(), 25 + 14);

        let mut numbers: Vec<usize> = mcu.iter_all_pins().map(|p| p.number).collect();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers.len(), 39, "pin numbers must be unique");
    }

    /// Every GPIO pad names its nRF port and pin first, and no GPIO is claimed
    /// twice. A duplicated `P0.xx` would generate two owners for one pin.
    #[test]
    fn every_gpio_pad_names_a_distinct_nrf_pin() {
        use crate::panels::mcu_module::mcu_def::McuDefinition;

        const SRC: &str = include_str!("../../../../assets/mcus/nrf52833_microbit_v2.ron");
        let def: McuDefinition = ron::from_str(SRC).unwrap();
        let mcu = def.build_mcu();

        let mut seen = std::collections::BTreeSet::new();
        for pin in mcu.iter_all_pins() {
            let head = pin.name.split_whitespace().next().unwrap_or("");
            let is_gpio = head.len() == 5
                && (head.starts_with("P0.") || head.starts_with("P1."))
                && head[3..].chars().all(|c| c.is_ascii_digit());
            if !is_gpio {
                assert!(
                    pin.reserved,
                    "{}: not a GPIO, so it must be reserved",
                    pin.name
                );
                continue;
            }
            assert!(seen.insert(head.to_owned()), "{head} appears twice");
        }
        assert_eq!(
            seen.len(),
            19 + 14,
            "19 GPIO contacts plus 14 on-board nets"
        );
    }

    /// Where autowire puts each bus on this board, stated as it is today.
    ///
    /// SPI lands on pads 13/14/15, the only pads that offer it. I2C and UART
    /// are different: the internal sensor bus and the USB-serial nets offer
    /// them too, and autowire picks those FIRST. For UART that is the default
    /// most users want (serial to the PC). For I2C it is right for the on-board
    /// motion sensor and wrong for an external device, which then needs the
    /// second pick - pads 19/20 - once the internal pads are taken.
    ///
    /// Open until on-board device modules exist to claim the internal nets
    /// explicitly; at that point a plain I2C module should prefer the edge.
    #[test]
    fn where_autowire_puts_each_bus_on_the_board() {
        use crate::panels::mcu_module::mcu_def::McuDefinition;
        use crate::panels::mcu_module::modules::ModuleSignal as S;
        use crate::panels::mcu_module::modules::autowire::pick_pins;
        use std::collections::HashSet;

        const SRC: &str = include_str!("../../../../assets/mcus/nrf52833_microbit_v2.ron");
        let def: McuDefinition = ron::from_str(SRC).unwrap();
        let mcu = def.build_mcu();
        let name_of = |n: usize| mcu.find_pin(n).map(|p| p.name.clone()).unwrap();
        let none = HashSet::new();

        let (inst, spi) = pick_pins(
            &mcu,
            &none,
            &HashSet::new(),
            &[S::Sck, S::Mosi, S::Miso],
            &[],
        )
        .expect("SPI must wire");
        assert_eq!(inst, 2, "SPIM2, the one that shares no ID with a TWIM");
        let spi: Vec<String> = spi.into_iter().map(|(_, n)| name_of(n)).collect();
        for want in ["pad 13", "pad 14", "pad 15"] {
            assert!(
                spi.iter().any(|n| n.contains(want)),
                "SPI missed {want}: {spi:?}"
            );
        }

        // First picks: the on-board nets.
        let (_, first) =
            pick_pins(&mcu, &none, &HashSet::new(), &[S::Scl, S::Sda], &[]).expect("I2C must wire");
        let first: Vec<String> = first.into_iter().map(|(_, n)| name_of(n)).collect();
        assert!(
            first.iter().all(|n| n.contains("INT_")),
            "I2C first pick: {first:?}"
        );
        let (_, uart) =
            pick_pins(&mcu, &none, &HashSet::new(), &[S::Tx, S::Rx], &[]).expect("UART must wire");
        let uart: Vec<String> = uart.into_iter().map(|(_, n)| name_of(n)).collect();
        for want in ["P0.06 (UART_TX)", "P1.08 (UART_RX)"] {
            assert!(uart.iter().any(|n| n == want), "UART first pick: {uart:?}");
        }

        // With the sensor bus taken, I2C moves to the labeled edge pads.
        let used: HashSet<usize> = mcu
            .iter_all_pins()
            .filter(|p| p.name.contains("INT_"))
            .map(|p| p.number)
            .collect();
        let (_, i2c) =
            pick_pins(&mcu, &used, &HashSet::new(), &[S::Scl, S::Sda], &[]).expect("I2C must wire");
        let i2c: Vec<String> = i2c.into_iter().map(|(_, n)| name_of(n)).collect();
        for want in ["pad 19", "pad 20"] {
            assert!(
                i2c.iter().any(|n| n.contains(want)),
                "I2C missed {want}: {i2c:?}"
            );
        }
    }

    /// HFCLK is 64 MHz from either source, and LFCLK is 32.768 kHz from either
    /// of the two the board can use. There is no LFXO node: the crystal is not
    /// fitted, and offering it would generate a `start_lfclk()` that never
    /// returns.
    #[test]
    fn the_clock_graph_delivers_64_mhz_and_32768_hz_from_every_source() {
        use crate::panels::mcu_module::clock::graph::eval::evaluate;
        use crate::panels::mcu_module::clock::graph::model::NodeState;
        use crate::panels::mcu_module::mcu_def::{ClockDef, McuDefinition};

        const SRC: &str = include_str!("../../../../assets/mcus/nrf52833_microbit_v2.ron");
        let def: McuDefinition = ron::from_str(SRC).unwrap();
        let ClockDef::Graph(gc) = def.effective_clock() else {
            panic!("the definition carries its own graph");
        };
        assert!(
            gc.graph.node("lfxo").is_none(),
            "no 32.768 kHz crystal on the board"
        );

        for hf in 0..2 {
            for lf in 0..2 {
                let mut g = gc.graph.clone();
                g.node_mut("hfclk_src").unwrap().state = NodeState::Index(hf);
                g.node_mut("lfclk_src").unwrap().state = NodeState::Index(lf);
                let hz = evaluate(&g);
                assert_eq!(hz["hfclk"], 64_000_000, "hfclk_src={hf}");
                assert_eq!(hz["lfclk"], 32_768, "hfclk_src={hf} lfclk_src={lf}");
            }
        }
    }
}

#[cfg(test)]
mod blocking_codegen {
    use crate::panels::mcu_module::clock::graph::model::NodeState;
    use crate::panels::mcu_module::clock::model::ClockConfig;
    use crate::panels::mcu_module::mcu::Mcu;
    use crate::panels::mcu_module::pins::PinFunction;
    use crate::panels::mcu_module::pins::logic::pin::GpioMode;
    use crate::panels::mcu_module::{builtins, project_gen};

    /// The built-in micro:bit, wired as `wire` says, by pad name prefix.
    pub(super) fn microbit(wire: &[(&str, PinFunction)]) -> Mcu {
        let mut mcu = builtins::builtin_definitions()
            .into_iter()
            .find(|d| d.id == "nrf52833_microbit_v2")
            .expect("built-in micro:bit v2")
            .build_mcu();
        for p in mcu.iter_all_pins_mut() {
            if let Some((_, f)) = wire.iter().find(|(head, _)| p.name.starts_with(head)) {
                p.selected_function = f.clone();
            }
        }
        mcu
    }

    /// One of everything, on the pads the board labels for it.
    pub(super) fn everything() -> Mcu {
        let mut mcu = microbit(&[
            ("P0.21", PinFunction::GpioOutput),
            ("P0.14", PinFunction::GpioInput),
            ("P0.06", PinFunction::UsartTx(0)),
            ("P1.08", PinFunction::UsartRx(0)),
            ("P0.08", PinFunction::I2cScl(0)),
            ("P0.16", PinFunction::I2cSda(0)),
            ("P0.17", PinFunction::SpiSck(2)),
            ("P0.13", PinFunction::SpiMosi(2)),
            ("P0.01", PinFunction::SpiMiso(2)),
            (
                "P0.00",
                PinFunction::TimerPwm {
                    timer: 0,
                    channel: 0,
                },
            ),
            (
                "P1.02",
                PinFunction::TimerPwm {
                    timer: 0,
                    channel: 1,
                },
            ),
            ("P0.02", PinFunction::AdcChannel { adc: 0, channel: 0 }),
        ]);
        mcu.reconcile_modules();
        for m in &mut mcu.modules {
            if let crate::panels::mcu_module::modules::ModuleConfig::Timer(c) = &mut m.config {
                c.freq_hz = 50;
                c.set_duty_x100(0, 750);
                c.set_duty_x100(1, 1_000);
            }
        }
        mcu
    }

    /// Set a mux on the tree to `idx`.
    fn select(mcu: &mut Mcu, mux: &str, idx: usize) {
        let ClockConfig::Graph(gc) = &mut mcu.clock else {
            panic!("the micro:bit carries a graph");
        };
        gc.graph.node_mut(mux).unwrap().state = NodeState::Index(idx);
    }

    /// The tree's two muxes become the two `Clocks` calls, and nothing else
    /// does: the internal choices emit no `enable_ext_hfosc`, the crystal
    /// choice does, and LFCLK follows its own mux.
    #[test]
    fn the_clock_block_follows_the_two_muxes() {
        let mut mcu = microbit(&[]);
        let main = mcu.fresh_main_rs();
        assert!(!main.contains("enable_ext_hfosc"), "{main}");
        assert!(main.contains(".set_lfclk_src_rc()"), "{main}");
        assert!(main.contains(".start_lfclk();"), "{main}");
        assert!(!main.contains("lfxo"), "{main}");

        select(&mut mcu, "hfclk_src", 1);
        select(&mut mcu, "lfclk_src", 1);
        let main = mcu.fresh_main_rs();
        assert!(main.contains(".enable_ext_hfosc()"), "{main}");
        assert!(main.contains(".set_lfclk_src_synth()"), "{main}");
        assert!(!main.contains("set_lfclk_src_rc"), "{main}");
    }

    /// Every wired pad reaches `main.rs` as the call its HAL wants, each bus
    /// through its config file, and the ADC on the TYPED pin.
    #[test]
    fn a_wired_board_generates_every_peripheral() {
        let mcu = everything();
        let main = mcu.fresh_main_rs();
        for want in [
            "#[cortex_m_rt::entry]",
            "nrf52833_hal::pac::Peripherals::take()",
            "let port0 = nrf52833_hal::gpio::p0::Parts::new(p.P0);",
            "let port1 = nrf52833_hal::gpio::p1::Parts::new(p.P1);",
            "let mut p0_21_out = port0.p0_21.into_push_pull_output(nrf52833_hal::gpio::Level::Low);",
            "let p0_14_in = port0.p0_14.into_floating_input();",
            "pins::configs::uarte0::init(\n        p.UARTE0,\n        port0.p0_06.into_push_pull_output(nrf52833_hal::gpio::Level::High).degrade(),\n        port1.p1_08.into_floating_input().degrade(),\n        None,\n        None,",
            "pins::configs::twim0::init(\n        p.TWIM0,\n        port0.p0_08.into_floating_input().degrade(),\n        port0.p0_16.into_floating_input().degrade(),",
            "pins::configs::spim2::init(\n        p.SPIM2,\n        port0.p0_17.into_push_pull_output(nrf52833_hal::gpio::Level::Low).degrade(),\n        Some(port0.p0_13",
            "Some(port0.p0_01.into_floating_input().degrade()),",
            "pins::configs::pwm0::init(\n        p.PWM0,\n        port0.p0_00.into_push_pull_output(nrf52833_hal::gpio::Level::Low).degrade(),\n        port1.p1_02",
            "let mut saadc = nrf52833_hal::saadc::Saadc::new(p.SAADC, nrf52833_hal::saadc::SaadcConfig::default());",
            "let mut p0_02_adc0_in0 = port0.p0_02.into_floating_input();",
        ] {
            assert!(main.contains(want), "missing {want:?} in:\n{main}");
        }
        // The board's own names ride along as comments.
        assert!(main.contains("// P0.21 (ROW1)"), "{main}");
        assert!(main.contains("// P0.14 (pad 5, BTN_A)"), "{main}");

        let files = mcu.config_files();
        let mut names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["pwm0.rs", "spim2.rs", "twim0.rs", "uarte0.rs"]);
    }

    /// The pull and drive chosen in the pin panel pick the `into_*` method,
    /// an unset mode generates the FIRST one the panel offers (which is what
    /// the panel highlights), and a mode left over from the other direction
    /// is ignored.
    #[test]
    fn the_io_mode_picks_the_constructor() {
        let mut mcu = microbit(&[
            ("P0.21", PinFunction::GpioOutput),
            ("P0.14", PinFunction::GpioInput),
            ("P0.23", PinFunction::GpioInput),
            ("P0.11", PinFunction::GpioInput),
            ("P0.22", PinFunction::GpioOutput),
        ]);
        for p in mcu.iter_all_pins_mut() {
            match p.name.split_whitespace().next() {
                Some("P0.21") => p.io_mode = Some(GpioMode::OpenDrain),
                Some("P0.14") => p.io_mode = Some(GpioMode::PullUp),
                Some("P0.23") => p.io_mode = Some(GpioMode::PullDown),
                // Stale: an output mode on a pin that is now an input, and an
                // input mode on an output.
                Some("P0.11") => p.io_mode = Some(GpioMode::OpenDrain),
                Some("P0.22") => p.io_mode = Some(GpioMode::PullDown),
                _ => {}
            }
        }
        let main = mcu.fresh_main_rs();
        assert!(
            main.contains("port0.p0_21.into_open_drain_output(nrf52833_hal::gpio::OpenDrainConfig::Standard0Disconnect1, nrf52833_hal::gpio::Level::High)"),
            "{main}"
        );
        assert!(main.contains("port0.p0_14.into_pullup_input()"), "{main}");
        assert!(main.contains("port0.p0_23.into_pulldown_input()"), "{main}");
        assert!(main.contains("port0.p0_11.into_floating_input()"), "{main}");
        assert!(
            main.contains("port0.p0_22.into_push_pull_output(nrf52833_hal::gpio::Level::Low)"),
            "{main}"
        );

        // The panel's first chip and the generated default are the same thing.
        use crate::panels::mcu_module::codegen::family::gpio_modes_for;
        assert_eq!(
            gpio_modes_for(&mcu, &PinFunction::GpioInput).first(),
            Some(&GpioMode::for_input(None))
        );
        assert_eq!(
            gpio_modes_for(&mcu, &PinFunction::GpioOutput).first(),
            Some(&GpioMode::for_output(None))
        );
    }

    /// Half a bus is a comment naming the missing pad, and no config file.
    #[test]
    fn half_wired_buses_say_so_and_get_no_file() {
        let mcu = microbit(&[
            ("P0.06", PinFunction::UsartTx(0)),
            ("P0.08", PinFunction::I2cScl(0)),
            // A SPIM with only MOSI: SCK is the one pad it cannot do without.
            ("P0.13", PinFunction::SpiMosi(2)),
        ]);
        let main = mcu.fresh_main_rs();
        assert!(
            main.contains("// UARTE0: only one of TXD/RXD is wired"),
            "{main}"
        );
        assert!(
            main.contains("// TWIM0: SCL and SDA are taken together"),
            "{main}"
        );
        assert!(main.contains("// SPIM2: SCK is not wired"), "{main}");
        assert!(!main.contains("::init("), "{main}");
        assert!(mcu.config_files().is_empty());

        // SCK alone IS a bus on this chip: MOSI and MISO are options.
        let mcu = microbit(&[("P0.17", PinFunction::SpiSck(2))]);
        let main = mcu.fresh_main_rs();
        assert!(
            main.contains("pins::configs::spim2::init(\n        p.SPIM2,\n        port0.p0_17.into_push_pull_output(nrf52833_hal::gpio::Level::Low).degrade(),\n        None,\n        None,"),
            "{main}"
        );
    }

    /// CTS and RTS ride along into the UARTE when wired.
    #[test]
    fn flow_control_pads_reach_the_constructor() {
        let mcu = microbit(&[
            ("P0.06", PinFunction::UsartTx(0)),
            ("P1.08", PinFunction::UsartRx(0)),
            ("P0.02", PinFunction::UsartCts(0)),
            ("P0.03", PinFunction::UsartRts(0)),
        ]);
        let main = mcu.fresh_main_rs();
        assert!(
            main.contains("Some(port0.p0_02.into_floating_input().degrade()),\n        Some(port0.p0_03.into_push_pull_output(nrf52833_hal::gpio::Level::High).degrade()),"),
            "{main}"
        );
    }

    /// Two pads on one signal: the lower pin is configured and both are named.
    #[test]
    fn two_pads_on_one_signal_are_both_named() {
        let mcu = microbit(&[
            ("P0.06", PinFunction::UsartTx(0)),
            ("P0.02", PinFunction::UsartTx(0)),
            ("P1.08", PinFunction::UsartRx(0)),
        ]);
        let main = mcu.fresh_main_rs();
        assert!(
            main.contains("// UARTE0 TXD is wired to P0.02 and P0.06. Only P0.02 is configured:"),
            "{main}"
        );
        assert!(main.contains("port0.p0_02.into_push_pull_output"), "{main}");
        assert!(
            !main.contains("port0.p0_06.into_push_pull_output"),
            "{main}"
        );
    }

    /// Anything on the NFC pads gets the UICR note; nothing else does.
    #[test]
    fn using_an_nfc_pad_is_flagged() {
        let mcu = microbit(&[("P0.10", PinFunction::GpioInput)]);
        let main = mcu.fresh_main_rs();
        assert!(
            main.contains("// P0.10 is one of the NFC antenna pins"),
            "{main}"
        );
        assert!(main.contains("NFCPINS"), "{main}");

        let mcu = microbit(&[("P0.14", PinFunction::GpioInput)]);
        assert!(!mcu.fresh_main_rs().contains("NFCPINS"));
    }

    /// A definition with no P1 pin (an nRF52832-style part) takes only port 0:
    /// its HAL has no `p1` module, so a `port1` line would not compile.
    #[test]
    fn a_single_port_part_takes_only_port_zero() {
        let mut mcu = microbit(&[("P0.21", PinFunction::GpioOutput)]);
        for p in mcu.iter_all_pins_mut() {
            if let Some(rest) = p.name.strip_prefix("P1.") {
                p.name = format!("NC{rest}");
            }
        }
        let main = mcu.fresh_main_rs();
        assert!(
            main.contains("let port0 = nrf52833_hal::gpio::p0::Parts::new(p.P0);"),
            "{main}"
        );
        assert!(!main.contains("port1"), "{main}");
        assert!(main.contains("// The port, taken once."), "{main}");
        assert!(main.contains("port0.p0_21.into_push_pull_output"), "{main}");

        // And the stock micro:bit, which has P1 pads, takes both.
        let main = microbit(&[("P0.21", PinFunction::GpioOutput)]).fresh_main_rs();
        assert!(
            main.contains("let port1 = nrf52833_hal::gpio::p1::Parts::new(p.P1);"),
            "{main}"
        );
        assert!(main.contains("// Both ports, taken once."), "{main}");
    }

    /// The module's numbers land in the config files as the HAL's fixed
    /// settings: nearest baud, highest rate at or below, and a prescaler that
    /// keeps the 15-bit counter top in range.
    #[test]
    fn the_config_files_carry_the_modules_settings() {
        let mut mcu = everything();
        for m in &mut mcu.modules {
            use crate::panels::mcu_module::modules::{ModuleConfig, Parity};
            match &mut m.config {
                ModuleConfig::Usart(c) => {
                    c.baud_rate = 9_600;
                    c.parity = Parity::Even;
                }
                ModuleConfig::Spi(c) => {
                    c.clock_hz = 3_000_000;
                    c.mode = 3;
                }
                ModuleConfig::I2c(c) => c.clock_hz = 100_000,
                _ => {}
            }
        }
        let files = mcu.config_files();
        let body = |name: &str| {
            files
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, b)| b.as_str())
                .unwrap_or_else(|| panic!("no {name}"))
        };

        let uarte = body("uarte0.rs");
        assert!(uarte.contains("Baudrate::BAUD9600;"), "{uarte}");
        assert!(uarte.contains("Parity::INCLUDED;"), "{uarte}");
        assert!(
            uarte.contains("uarte::Pins { rxd, txd, cts, rts }"),
            "{uarte}"
        );

        let spim = body("spim2.rs");
        assert!(spim.contains("// 3000000 Hz asked for"), "{spim}");
        assert!(spim.contains("Frequency::M2;"), "{spim}");
        assert!(spim.contains("MODE_3;"), "{spim}");
        // And it is in the regenerated half, where the module can change it.
        let e = spim.find("// <<< GENERATED END >>>").unwrap();
        assert!(spim[..e].contains("pub const MODE"), "{spim}");

        let twim = body("twim0.rs");
        assert!(twim.contains("Frequency::K100;"), "{twim}");
        assert!(!twim.contains("asked for"), "{twim}");

        // 50 Hz: 16 MHz / 50 = 320 000, far over 32 767, so the divider climbs
        // until the top fits: /16 gives 20 000.
        let pwm = body("pwm0.rs");
        assert!(pwm.contains("pub const FREQ_HZ: u32 = 50;"), "{pwm}");
        assert!(pwm.contains("Prescaler::Div16;"), "{pwm}");
        assert!(
            pwm.contains("pub const DUTY_C0_X100: u32 = 750; // 7.5 %"),
            "{pwm}"
        );
        assert!(
            pwm.contains("pub const DUTY_C1_X100: u32 = 1000; // 10 %"),
            "{pwm}"
        );
        assert!(
            pwm.contains("pwm.set_output_pin(Channel::C0, c0);"),
            "{pwm}"
        );
        assert!(
            pwm.contains("fn set_duty_pwm_0_c1(&mut self, value: u32)"),
            "{pwm}"
        );
    }

    #[test]
    fn the_prescaler_is_the_smallest_that_fits() {
        assert_eq!(super::pwm_prescaler(20_000), (1, "Div1"));
        assert_eq!(super::pwm_prescaler(1_000), (1, "Div1"));
        assert_eq!(super::pwm_prescaler(400), (2, "Div2"));
        assert_eq!(super::pwm_prescaler(50), (16, "Div16"));
        assert_eq!(super::pwm_prescaler(1), (128, "Div128"));
        assert_eq!(super::pwm_actual_hz(50, 16), 50);
        assert_eq!(super::pwm_actual_hz(1_000, 1), 1_000);
        // 8 MHz / 300 = 26 666 rem 200: the top rounds down, and the integer
        // division on the way back lands on the number asked for anyway.
        assert_eq!(super::pwm_actual_hz(300, 2), 300);
        // 16 MHz / 128 = 125 kHz; at 1 Hz the top would be 125 000, which the
        // 15-bit counter clamps to 32 767, so the pad runs at 3 Hz instead.
        assert_eq!(super::pwm_actual_hz(1, 128), 3);
    }

    #[test]
    fn the_fixed_rates_round_the_way_the_comments_say() {
        assert_eq!(super::baud_variant(115_200).1, "BAUD115200");
        assert_eq!(super::baud_variant(110_000).1, "BAUD115200");
        assert_eq!(super::baud_variant(1_000_000).1, "BAUD1M");
        assert_eq!(super::spim_frequency(8_000_000).1, "M8");
        assert_eq!(super::spim_frequency(16_000_000).1, "M8");
        assert_eq!(super::spim_frequency(1).1, "K125");
        assert_eq!(super::twim_frequency(400_000).1, "K400");
        assert_eq!(super::twim_frequency(399_999).1, "K250");
        assert_eq!(super::twim_frequency(0).1, "K100");
    }

    /// Re-generating must replace the marked block and keep everything else.
    #[test]
    fn regeneration_keeps_the_users_code() {
        let mut mcu = microbit(&[("P0.21", PinFunction::GpioOutput)]);
        let first = mcu.fresh_main_rs();
        let edited = first
            .replace(
                "use panic_halt as _;",
                "use panic_halt as _;\nuse my_crate::Thing;",
            )
            .replace(
                "        // Your main loop code here.",
                "        p0_21_out.set_high().unwrap();\n        my_own_helper();",
            );
        for p in mcu.iter_all_pins_mut() {
            if p.name.starts_with("P0.14") {
                p.selected_function = PinFunction::GpioInput;
            }
        }
        let again = mcu.update_main_rs(&edited);
        assert!(again.contains("use my_crate::Thing;"), "{again}");
        assert!(again.contains("my_own_helper();"), "{again}");
        assert!(
            again.contains("port0.p0_14.into_floating_input()"),
            "{again}"
        );
        assert!(
            again.contains("port0.p0_21.into_push_pull_output"),
            "{again}"
        );
        assert_eq!(again.matches("#[cortex_m_rt::entry]").count(), 1, "{again}");
    }

    /// The editable half of every config file sits OUTSIDE the markers.
    #[test]
    fn only_the_constants_are_regenerated() {
        const BEGIN: &str = "// <<< GENERATED>>>";
        const END: &str = "// <<< GENERATED END >>>";
        let files = everything().config_files();
        assert_eq!(files.len(), 4);
        for (name, body) in &files {
            assert_eq!(body.matches(BEGIN).count(), 1, "{name}");
            assert_eq!(body.matches(END).count(), 1, "{name}");
            let b = body.find(BEGIN).unwrap();
            let e = body.find(END).unwrap();
            assert!(b < e, "{name}");
            let inside = &body[b..e];
            let outside = &body[e..];
            for forbidden in ["pub fn init", "pub trait", "pub type Handle"] {
                assert!(
                    !inside.contains(forbidden),
                    "{name}: {forbidden} inside:\n{body}"
                );
            }
            assert!(outside.contains("pub fn init"), "{name}");
            assert!(outside.contains("pub type Handle"), "{name}");
            assert!(inside.contains("const "), "{name}");
        }
    }

    /// The other branches: crystal HFCLK, synthesized LFCLK, open-drain and
    /// pull-down pins, flow control on the UARTE, SPI mode 3 on a bus with only
    /// SCK, and a PWM with no frequency set.
    fn the_other_branches() -> Mcu {
        let mut mcu = microbit(&[
            ("P0.21", PinFunction::GpioOutput),
            ("P0.14", PinFunction::GpioInput),
            ("P0.06", PinFunction::UsartTx(0)),
            ("P1.08", PinFunction::UsartRx(0)),
            ("P0.02", PinFunction::UsartCts(0)),
            ("P0.03", PinFunction::UsartRts(0)),
            ("P0.17", PinFunction::SpiSck(2)),
            (
                "P0.00",
                PinFunction::TimerPwm {
                    timer: 1,
                    channel: 2,
                },
            ),
        ]);
        for p in mcu.iter_all_pins_mut() {
            match p.name.split_whitespace().next() {
                Some("P0.21") => p.io_mode = Some(GpioMode::OpenDrain),
                Some("P0.14") => p.io_mode = Some(GpioMode::PullDown),
                _ => {}
            }
        }
        select(&mut mcu, "hfclk_src", 1);
        select(&mut mcu, "lfclk_src", 1);
        mcu.reconcile_modules();
        for m in &mut mcu.modules {
            if let crate::panels::mcu_module::modules::ModuleConfig::Spi(c) = &mut m.config {
                c.mode = 3;
            }
        }
        mcu
    }

    /// Two micro:bit projects on disk, for a real cross-compile.
    ///
    /// Every HAL call above was read from the nrf-hal-common 0.19 source, and
    /// the compiler is still the only thing that can say the reading was
    /// right: type-state on `Clocks`, `degrade()` on every bus pin, the SAADC
    /// on the typed one. Two projects rather than one because a branch the
    /// emitted project never takes is a branch the compiler never sees.
    ///
    /// ```text
    /// cargo test --bin embedded_ide_0 emit_nrf_project -- --ignored --nocapture
    /// cd %TEMP%\eide_nrf52833_check && cargo check --target thumbv7em-none-eabihf
    /// ```
    #[test]
    #[ignore = "writes projects to disk for a manual cross-compile"]
    fn emit_nrf_project() {
        for (mcu, dir_name) in [
            (everything(), "eide_nrf52833_check"),
            (the_other_branches(), "eide_nrf52833_alt_check"),
        ] {
            emit(&mcu, dir_name);
        }
    }

    fn emit(mcu: &Mcu, dir_name: &str) {
        let def = builtins::builtin_definitions()
            .into_iter()
            .find(|d| d.id == "nrf52833_microbit_v2")
            .expect("built-in micro:bit v2");
        let main_rs = mcu.fresh_main_rs();
        let files = project_gen::build_project_files(&def.project, &def.toolchain, &main_rs);
        let configs = mcu.config_files();
        let mut user: Vec<(String, String)> = vec![
            ("src/pins/mod.rs".into(), "pub mod configs;\n".into()),
            (
                "src/pins/configs/mod.rs".into(),
                configs
                    .iter()
                    .map(|(n, _)| format!("pub mod {};\n", n.trim_end_matches(".rs")))
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
            .expect("write nrf project");
        println!("wrote {}", dir.display());
        println!("target: {}", def.project.target);
    }
}
