//! Per-peripheral init modules for ESP projects — `src/pins/configs/*.rs`.
//!
//! The ESP twin of [`super::codegen::stm32::config_files`]. Same deal as on
//! STM32: `main.rs` binds the pins and hands them to a small module that owns
//! the peripheral's settings and its `init(...)`, so the bus setup is one
//! readable place the user can edit — instead of a builder chain wedged between
//! the GEN markers where every edit is overwritten.
//!
//! Layout of a generated file:
//!
//! ```text
//! use esp_hal::…;                  ← the consts below name these types
//!
//! // <<< GENERATED>>>
//! const BAUDRATE: u32 = 115200;    ← from the Virtual Module, re-written on change
//! // <<< GENERATED END >>>
//!
//! pub fn init<'d>(uart: impl Instance + 'd, …) -> Uart<'d, Blocking> { … }
//! ```                              ← the user's half: edit freely, never touched
//!
//! `init` is GENERIC over the pins (`impl PeripheralInput<'d>` /
//! `PeripheralOutput<'d>`, the bounds esp-hal's own builders take), so the file
//! is identical for every chip in the family — only the instance number differs,
//! and the concrete pins are chosen at the call site in `main.rs`. That is also
//! what keeps the pins VISIBLE in `main.rs`: they stay `let gpio5_… =
//! peripherals.GPIO5; // USART1  RX` lines, which is how a reopened project
//! restores them (see [`super::codegen::parse_main_rs`]).
//!
//! The signature mirrors what the canvas WIRED: an SPI with no MISO gets an
//! `init` without a `miso` parameter, rather than one that ignores it. A
//! parameter nobody passes is a warning in the user's build and a lie about
//! which pins the peripheral uses.
//!
//! Real-compile verified on ESP32-C3 / esp-hal 1.1.

use super::modules::{I2cModuleConfig, Parity, SpiModuleConfig, StopBits, UsartModuleConfig};
use std::collections::BTreeMap;

/// Marker pair bounding the auto-updated half of a config file. Identical to the
/// STM32 templates so `sync_config_files` splices both the same way.
const GEN_BEGIN: &str = "// <<< GENERATED>>>";
const GEN_END: &str = "// <<< GENERATED END >>>";

/// `use` items, then the marker block around `consts`, then the user's `body`.
fn file(uses: &str, consts: &str, body: &str) -> String {
    format!(
        "{uses}\n\
         {GEN_BEGIN}\n\
         // Peripheral config (from the Virtual Module) — auto-updated; edit in the module.\n\
         {consts}{GEN_END}\n\
         \n\
         {body}"
    )
}

/// One `    <name>: <bound>,` line per wired signal, in `spec` order.
fn params_for(sigs: &[&str], spec: &[(&str, &str)]) -> String {
    spec.iter()
        .filter(|(s, _)| sigs.contains(s))
        .map(|(name, bound)| format!("    {name}: {bound},\n"))
        .collect()
}

/// One `        .<method>(<name>)` link per wired signal, in `spec` order.
fn chain_for(sigs: &[&str], spec: &[(&str, &str)]) -> String {
    spec.iter()
        .filter(|(s, _)| sigs.contains(s))
        .map(|(name, method)| format!("        .{method}({name})\n"))
        .collect()
}

// ── UART ─────────────────────────────────────────────────────────────────────

fn data_bits_variant(bits: u8) -> &'static str {
    match bits {
        5 => "_5",
        6 => "_6",
        7 => "_7",
        _ => "_8",
    }
}

fn parity_variant(p: Parity) -> &'static str {
    match p {
        Parity::None => "None",
        Parity::Even => "Even",
        Parity::Odd => "Odd",
    }
}

fn stop_bits_variant(s: StopBits) -> &'static str {
    match s {
        StopBits::One => "_1",
        StopBits::Two => "_2",
    }
}

fn uart_file(n: u8, sigs: &[&str], cfg: Option<&UsartModuleConfig>) -> String {
    let consts = format!(
        "const BAUDRATE: u32 = {};\nconst DATA_BITS: DataBits = DataBits::{};\nconst PARITY: Parity = Parity::{};\nconst STOP_BITS: StopBits = StopBits::{};\n",
        cfg.map_or(115_200, |c| c.baud_rate),
        data_bits_variant(cfg.map_or(8, |c| c.data_bits)),
        parity_variant(cfg.map_or(Parity::None, |c| c.parity)),
        stop_bits_variant(cfg.map_or(StopBits::One, |c| c.stop_bits)),
    );
    let params = params_for(
        sigs,
        &[
            ("rx", "impl PeripheralInput<'d>"),
            ("tx", "impl PeripheralOutput<'d>"),
        ],
    );
    let chain = chain_for(sigs, &[("rx", "with_rx"), ("tx", "with_tx")]);
    let body = format!(
        "/// UART{n} — blocking driver.\n\
         ///\n\
         /// Generic over the pins so this file never names a GPIO: `main.rs`\n\
         /// passes the ones wired on the Pins canvas.\n\
         pub fn init<'d>(\n\
         \x20   uart: impl Instance + 'd,\n\
         {params}) -> Uart<'d, Blocking> {{\n\
         \x20   let config = Config::default()\n\
         \x20       .with_baudrate(BAUDRATE)\n\
         \x20       .with_data_bits(DATA_BITS)\n\
         \x20       .with_parity(PARITY)\n\
         \x20       .with_stop_bits(STOP_BITS);\n\
         \x20   // `unwrap`: these values come from the Virtual Module's UI, which\n\
         \x20   // range-limits them — a failure here is a bug in the generator,\n\
         \x20   // not a runtime condition the firmware could recover from.\n\
         \x20   Uart::new(uart, config)\n\
         \x20       .unwrap()\n\
         {chain}}}\n"
    );
    file(
        "use esp_hal::Blocking;\n\
         use esp_hal::gpio::interconnect::{PeripheralInput, PeripheralOutput};\n\
         use esp_hal::uart::{Config, DataBits, Instance, Parity, StopBits, Uart};\n",
        &consts,
        &body,
    )
}

// ── SPI ──────────────────────────────────────────────────────────────────────

fn spi_file(n: u8, sigs: &[&str], cfg: Option<&SpiModuleConfig>) -> String {
    let consts = format!(
        "const FREQUENCY_HZ: u32 = {};\nconst MODE: Mode = Mode::_{}; // CPOL/CPHA, 0..=3\n",
        cfg.map_or(1_000_000, |c| c.clock_hz),
        cfg.map_or(0, |c| c.mode).min(3),
    );
    let spec: &[(&str, &str)] = &[
        ("sck", "impl PeripheralOutput<'d>"),
        ("mosi", "impl PeripheralOutput<'d>"),
        ("miso", "impl PeripheralInput<'d>"),
        ("cs", "impl PeripheralOutput<'d>"),
    ];
    let params = params_for(sigs, spec);
    let chain = chain_for(
        sigs,
        &[
            ("sck", "with_sck"),
            ("mosi", "with_mosi"),
            ("miso", "with_miso"),
            ("cs", "with_cs"),
        ],
    );
    let body = format!(
        "/// SPI{n} master — blocking driver.\n\
         ///\n\
         /// Takes exactly the lines wired on the Pins canvas: no MISO wired means\n\
         /// no `miso` parameter here.\n\
         pub fn init<'d>(\n\
         \x20   spi: impl Instance + 'd,\n\
         {params}) -> Spi<'d, Blocking> {{\n\
         \x20   let config = Config::default()\n\
         \x20       .with_frequency(Rate::from_hz(FREQUENCY_HZ))\n\
         \x20       .with_mode(MODE);\n\
         \x20   Spi::new(spi, config)\n\
         \x20       .unwrap()\n\
         {chain}}}\n"
    );
    file(
        "use esp_hal::Blocking;\n\
         use esp_hal::gpio::interconnect::{PeripheralInput, PeripheralOutput};\n\
         use esp_hal::spi::Mode;\n\
         use esp_hal::spi::master::{Config, Instance, Spi};\n\
         use esp_hal::time::Rate;\n",
        &consts,
        &body,
    )
}

// ── I2C ──────────────────────────────────────────────────────────────────────

fn i2c_file(n: u8, sigs: &[&str], cfg: Option<&I2cModuleConfig>) -> String {
    let consts = format!(
        "const FREQUENCY_HZ: u32 = {};\n\
         // 7-bit address of the device on this bus — for YOUR code, not for `init`:\n\
         // an esp-hal I2C master takes the address per transaction.\n\
         pub const DEVICE_ADDRESS: u8 = 0x{:02X};\n",
        cfg.map_or(100_000, |c| c.clock_hz),
        cfg.map_or(0, |c| c.address),
    );
    let bound = "impl PeripheralInput<'d> + PeripheralOutput<'d>";
    let params = params_for(sigs, &[("scl", bound), ("sda", bound)]);
    let chain = chain_for(sigs, &[("scl", "with_scl"), ("sda", "with_sda")]);
    let body = format!(
        "/// I2C{n} master — blocking driver.\n\
         pub fn init<'d>(\n\
         \x20   i2c: impl Instance + 'd,\n\
         {params}) -> I2c<'d, Blocking> {{\n\
         \x20   let config = Config::default().with_frequency(Rate::from_hz(FREQUENCY_HZ));\n\
         \x20   I2c::new(i2c, config)\n\
         \x20       .unwrap()\n\
         {chain}}}\n"
    );
    file(
        "use esp_hal::Blocking;\n\
         use esp_hal::gpio::interconnect::{PeripheralInput, PeripheralOutput};\n\
         use esp_hal::i2c::master::{Config, I2c, Instance};\n\
         use esp_hal::time::Rate;\n",
        &consts,
        &body,
    )
}

/// The `(file_name, body)` pairs for every bus instance the pins wire, given
/// `(instance, wired signals)` from
/// [`super::codegen_esp::bus_instances`].
///
/// Keyed on what the CANVAS wires, not on the Virtual Modules: a bus can be
/// wired without a module (the module only carries the settings), and the
/// generated `init` then uses esp-hal's defaults.
pub fn config_files(
    uart: &[(u8, Vec<&'static str>)],
    spi: &[(u8, Vec<&'static str>)],
    i2c: &[(u8, Vec<&'static str>)],
    usart_cfg: &BTreeMap<u8, UsartModuleConfig>,
    spi_cfg: &BTreeMap<u8, SpiModuleConfig>,
    i2c_cfg: &BTreeMap<u8, I2cModuleConfig>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (n, sigs) in uart {
        out.push((format!("uart{n}.rs"), uart_file(*n, sigs, usart_cfg.get(n))));
    }
    for (n, sigs) in spi {
        out.push((format!("spi{n}.rs"), spi_file(*n, sigs, spi_cfg.get(n))));
    }
    for (n, sigs) in i2c {
        out.push((format!("i2c{n}.rs"), i2c_file(*n, sigs, i2c_cfg.get(n))));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Virtual Module's settings reach the generated consts — that is the
    /// whole point of the auto-updated block.
    #[test]
    fn module_settings_land_inside_the_generated_block() {
        let mut cfg = UsartModuleConfig::new(1);
        cfg.baud_rate = 9600;
        cfg.data_bits = 7;
        cfg.parity = Parity::Even;
        cfg.stop_bits = StopBits::Two;
        let f = uart_file(1, &["rx", "tx"], Some(&cfg));

        assert!(f.contains("const BAUDRATE: u32 = 9600;"), "{f}");
        assert!(f.contains("DataBits::_7"), "{f}");
        assert!(f.contains("Parity::Even"), "{f}");
        assert!(f.contains("StopBits::_2"), "{f}");

        let (begin, end) = (f.find(GEN_BEGIN).unwrap(), f.find(GEN_END).unwrap());
        let baud = f.find("const BAUDRATE").unwrap();
        assert!(begin < baud && baud < end, "consts inside the block:\n{f}");
        // The `init` the user edits is BELOW the markers, and the `use` items
        // the consts need are ABOVE them.
        assert!(f.find("pub fn init").unwrap() > end, "{f}");
        assert!(f.find("use esp_hal::uart").unwrap() < begin, "{f}");
    }

    /// A bus with no module still gets a complete file, on esp-hal's defaults.
    #[test]
    fn a_bus_without_a_module_falls_back_to_defaults() {
        let f = uart_file(0, &["rx", "tx"], None);
        assert!(f.contains("const BAUDRATE: u32 = 115200;"), "{f}");
        assert!(f.contains("pub fn init"), "{f}");
    }

    /// `init` declares a parameter per WIRED line and nothing more — an unused
    /// parameter would warn in the user's build and misdescribe the peripheral.
    #[test]
    fn the_signature_mirrors_the_wiring() {
        let full = spi_file(2, &["sck", "mosi", "miso", "cs"], None);
        for (param, method) in [
            ("sck:", ".with_sck(sck)"),
            ("mosi:", ".with_mosi(mosi)"),
            ("miso:", ".with_miso(miso)"),
            ("cs:", ".with_cs(cs)"),
        ] {
            assert!(full.contains(param), "{param} declared:\n{full}");
            assert!(full.contains(method), "{method} chained:\n{full}");
        }

        // Three-wire bus: no CS anywhere.
        let no_cs = spi_file(2, &["sck", "mosi", "miso"], None);
        assert!(!no_cs.contains("cs:"), "no cs parameter:\n{no_cs}");
        assert!(!no_cs.contains("with_cs"), "no cs link:\n{no_cs}");
        assert!(no_cs.contains("miso:"), "{no_cs}");

        // TX-only UART: no rx parameter, no `.with_rx`.
        let tx_only = uart_file(1, &["tx"], None);
        assert!(!tx_only.contains("rx:"), "{tx_only}");
        assert!(!tx_only.contains("with_rx"), "{tx_only}");
        assert!(tx_only.contains(".with_tx(tx)"), "{tx_only}");
    }

    #[test]
    fn one_file_per_wired_instance() {
        let files = config_files(
            &[(1, vec!["rx", "tx"])],
            &[(2, vec!["sck", "mosi"])],
            &[(0, vec!["scl", "sda"])],
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["uart1.rs", "spi2.rs", "i2c0.rs"]);
    }
}
