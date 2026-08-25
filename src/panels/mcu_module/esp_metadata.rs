//! Reading Espressif's own per-chip data.
//!
//! The STM32 side has `families.xml`; this is the Espressif equivalent. It comes
//! from the `esp-metadata-generated` crate — a dependency of `esp-hal`, so a
//! copy lands in the cargo registry as soon as any ESP project is built.
//!
//! # It is Rust, not data
//!
//! The crate ships one file per chip, each a few thousand lines of
//! `macro_rules!` that `esp-hal` expands at compile time:
//!
//! ```text
//! _for_each_inner_uart!((0, UART0, Uart0, U0RXD, U0TXD, U0CTS, U0RTS));
//! _for_each_inner_gpio!((4, GPIO4(_0 => MTMS _2 => FSPIHD) (_2 => FSPIHD) ([Input] [Output])));
//! ```
//!
//! We cannot expand them — that would mean compiling per chip — so this reads
//! them as text. The shape is regular enough for that to be honest work rather
//! than a guess, and every claim it makes is checked against all six real files
//! by [`tests::the_real_metadata_parses`].
//!
//! # The `all(…)` trap
//!
//! Every `for_each` macro emits its instances one per line AND once more as a
//! single aggregate:
//!
//! ```text
//! _for_each_inner_uart!((0, UART0, …)); _for_each_inner_uart!((1, UART1, …));
//! _for_each_inner_uart!((all(0, UART0, …), (1, UART1, …)));
//! ```
//!
//! Reading the aggregate as another instance is how a two-UART chip becomes a
//! three-UART chip. Anything whose first field opens a nested group is skipped —
//! which also catches `for_each_analog_function`'s second, destructured form.
//!
//! # Scope
//!
//! Development-time only: the generated chip definitions are committed under
//! `assets/mcus/`, so nothing here runs on a user's machine and nobody needs
//! `esp-metadata` installed to pick an ESP chip.

use std::path::PathBuf;

/// Which instruction set the chip's main core speaks.
///
/// The difference is not academic: Xtensa needs Espressif's fork of rustc,
/// installed with `espup` and invoked as `cargo +esp`, and its target cannot be
/// added with `rustup target add`. Only the RISC-V parts build with a stock
/// toolchain, which is why they are the ones this IDE offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch {
    RiscV,
    Xtensa,
}

impl Arch {
    /// The Rust target triple, or `None` for Xtensa — whose target does not come
    /// from rustup at all, and which nothing here is ready to build for.
    pub fn target(self, chip: &str) -> Option<&'static str> {
        match self {
            // The C2/C3 cores have no atomics extension; everything later does.
            Arch::RiscV => Some(match chip {
                "esp32c2" | "esp32c3" => "riscv32imc-unknown-none-elf",
                _ => "riscv32imac-unknown-none-elf",
            }),
            Arch::Xtensa => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Uart {
    pub id: u8,
    /// `UART0` — the `esp_hal::peripherals` singleton.
    pub instance: String,
    pub rx: String,
    pub tx: String,
    pub cts: String,
    pub rts: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct I2cMaster {
    pub id: u8,
    pub instance: String,
    pub scl: String,
    pub sda: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SpiMaster {
    /// `SPI2`. Unlike UART and I2C, the vendor gives no numeric id here.
    pub instance: String,
    pub clk: String,
    /// Every chip-select this controller can drive — `FSPICS0..5`.
    pub cs: Vec<String>,
    /// The data lines, in the vendor's QSPI order: D, Q, WP, HD.
    pub data: Vec<String>,
}

impl SpiMaster {
    /// The signal a plain 3-wire SPI master drives out.
    ///
    /// `FSPID` — "D" for *data out*, in the QSPI naming the vendor uses
    /// throughout. It is MOSI, and calling it that here is the difference
    /// between a generated project that works and one wired backwards.
    pub fn mosi(&self) -> Option<&str> {
        self.data.first().map(String::as_str)
    }

    /// `FSPIQ` — "Q", the line the peripheral reads. MISO.
    pub fn miso(&self) -> Option<&str> {
        self.data.get(1).map(String::as_str)
    }
}

/// One chip, as the vendor's metadata describes it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EspChip {
    /// `esp32c3` — also the `esp-hal` feature and the `espflash --chip` name.
    pub id: String,
    /// `ESP32-C3`.
    pub name: String,
    pub arch: Arch,
    /// On-die SRAM. There is deliberately no flash figure: an ESP32's flash is a
    /// separate SPI chip chosen by whoever built the module, so the same part
    /// ships as 2, 4, 8 or 16 MB and the die cannot answer the question.
    pub dram_bytes: u32,
    /// Every GPIO number the chip has. All of them are input- and
    /// output-capable on every RISC-V part — checked, not assumed.
    pub gpios: Vec<u8>,
    pub uarts: Vec<Uart>,
    pub i2c: Vec<I2cMaster>,
    pub spi: Vec<SpiMaster>,
    /// `("ADC1_CH0", 0)` — the analog channel and the GPIO carrying it. Unlike
    /// every other signal here, an ADC channel is bonded to ONE pad: the GPIO
    /// matrix routes digital signals, not analog ones.
    pub adc: Vec<(String, u8)>,
    /// Every peripheral singleton the chip has, GPIOs excluded.
    pub peripherals: Vec<String>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Text scanning
// ──────────────────────────────────────────────────────────────────────────────

/// The body of `macro_rules! name { … }`, with all whitespace collapsed.
///
/// Collapsing first is what lets the rest of this file ignore rustfmt: the
/// generated crate is formatted, so a single macro call is wrapped across as
/// many as four lines, mid-identifier-list.
fn macro_body(src: &str, name: &str) -> Option<String> {
    let needle = format!("macro_rules! {name} {{");
    let start = src.find(&needle)? + needle.len();
    let mut depth = 1usize;
    let mut end = start;
    for (ix, c) in src[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + ix;
                    break;
                }
            }
            _ => {}
        }
    }
    Some(
        src[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// The text between the parenthesis at `open` and its match.
fn balanced(s: &str, open: usize) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    for (ix, c) in s[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[open + 1..open + ix]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every real instance emitted by `_for_each_inner_<name>!`, as the text inside
/// its tuple.
///
/// Aggregates are dropped — see the module docs on the `all(…)` trap.
fn instances(body: &str, name: &str) -> Vec<String> {
    let needle = format!("_for_each_inner_{name}!(");
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(found) = body[at..].find(&needle) {
        let open = at + found + needle.len() - 1;
        let Some(outer) = balanced(body, open) else {
            break;
        };
        at = open + outer.len() + 2;
        // One more layer: the argument is itself a tuple.
        let inner = outer.trim();
        let Some(tuple) = inner
            .strip_prefix('(')
            .and_then(|t| t.strip_suffix(')'))
            .map(str::trim)
        else {
            continue;
        };
        // `all(…)`, or a destructured form whose first field is a group.
        if tuple.starts_with("all(") || tuple.starts_with('(') {
            continue;
        }
        out.push(tuple.to_owned());
    }
    out
}

/// Split a tuple on its TOP-LEVEL commas, so `A[B, C] [D, E], F` is two fields.
fn fields(tuple: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in tuple.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_owned());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

/// The identifiers inside the first `[…]` group of `s`, if any.
fn bracket_list(s: &str) -> Vec<String> {
    let Some(open) = s.find('[') else {
        return Vec::new();
    };
    let Some(close) = s[open..].find(']') else {
        return Vec::new();
    };
    s[open + 1..open + close]
        .split(',')
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect()
}

/// The string a nullary macro returns: `macro_rules! chip { () => { "esp32c3" }; }`.
fn literal_macro(src: &str, name: &str) -> Option<String> {
    let body = macro_body(src, name)?;
    let open = body.find('"')?;
    let close = body[open + 1..].find('"')?;
    Some(body[open + 1..open + 1 + close].to_owned())
}

// ──────────────────────────────────────────────────────────────────────────────
// The parser
// ──────────────────────────────────────────────────────────────────────────────

/// Read one `_generated_<chip>.rs` into an [`EspChip`].
pub fn parse(src: &str) -> Result<EspChip, String> {
    let id = literal_macro(src, "chip").ok_or("no chip!() macro — is this esp-metadata?")?;
    let name = literal_macro(src, "chip_pretty").unwrap_or_else(|| id.to_uppercase());

    // The architecture is not a field; it is spelled once, in the interrupt
    // plumbing. Both spellings are searched so a file that names neither is an
    // error rather than a silent RISC-V.
    let arch = match (src.contains("xtensa"), src.contains("riscv")) {
        (true, false) => Arch::Xtensa,
        (false, true) => Arch::RiscV,
        _ => return Err(format!("{id}: cannot tell the architecture from the file")),
    };

    let dram_bytes = macro_body(src, "memory_range")
        .and_then(|b| {
            // `(size as str, "DRAM") => { "393216" };` — the size is the first
            // quoted run AFTER the arm's own quoted region name.
            const ARM: &str = "(size as str, \"DRAM\") => {";
            let rest = &b[b.find(ARM)? + ARM.len()..];
            let q1 = rest.find('"')?;
            let q2 = rest[q1 + 1..].find('"')?;
            rest[q1 + 1..q1 + 1 + q2].parse::<u32>().ok()
        })
        .ok_or_else(|| format!("{id}: no DRAM size"))?;

    let body_of = |m: &str| macro_body(src, m).unwrap_or_default();

    let gpio_body = body_of("for_each_gpio");
    let mut gpios: Vec<u8> = instances(&gpio_body, "gpio")
        .iter()
        .filter_map(|t| fields(t).first()?.parse::<u8>().ok())
        .collect();
    gpios.sort_unstable();
    gpios.dedup();
    if gpios.is_empty() {
        return Err(format!("{id}: no GPIOs"));
    }

    let uart_body = body_of("for_each_uart");
    let uarts = instances(&uart_body, "uart")
        .iter()
        .filter_map(|t| {
            let f = fields(t);
            // (id, UART0, Uart0, RX, TX, CTS, RTS)
            if f.len() < 7 {
                return None;
            }
            Some(Uart {
                id: f[0].parse().ok()?,
                instance: f[1].clone(),
                rx: f[3].clone(),
                tx: f[4].clone(),
                cts: f[5].clone(),
                rts: f[6].clone(),
            })
        })
        .collect();

    let i2c_body = body_of("for_each_i2c_master");
    let i2c = instances(&i2c_body, "i2c_master")
        .iter()
        .filter_map(|t| {
            let f = fields(t);
            // (id, I2C0, I2cExt0, SCL, SDA)
            if f.len() < 5 {
                return None;
            }
            Some(I2cMaster {
                id: f[0].parse().ok()?,
                instance: f[1].clone(),
                scl: f[3].clone(),
                sda: f[4].clone(),
            })
        })
        .collect();

    let spi_body = body_of("for_each_spi_master");
    let spi = instances(&spi_body, "spi_master")
        .iter()
        .filter_map(|t| {
            let f = fields(t);
            // (SPI2, Spi2, FSPICLK[CS0, …] [D, Q, WP, HD], true)
            if f.len() < 3 {
                return None;
            }
            let sig = &f[2];
            let clk = sig[..sig.find('[').unwrap_or(sig.len())].trim().to_owned();
            let cs = bracket_list(sig);
            // The SECOND bracket group; `bracket_list` only ever reads the first.
            let data = sig
                .find(']')
                .map(|e| bracket_list(&sig[e + 1..]))
                .unwrap_or_default();
            Some(SpiMaster {
                instance: f[0].clone(),
                clk,
                cs,
                data,
            })
        })
        .collect();

    let analog_body = body_of("for_each_analog_function");
    let adc = instances(&analog_body, "analog_function")
        .iter()
        .filter_map(|t| {
            let f = fields(t);
            if f.len() < 2 || !f[0].starts_with("ADC") {
                return None;
            }
            let pad = f[1].strip_prefix("GPIO")?.parse::<u8>().ok()?;
            Some((f[0].clone(), pad))
        })
        .collect();

    // Not a `for_each` tuple: the entries carry doc comments full of commas and
    // parentheses, so the names are read directly off the `NAME <= …` arrow.
    let peri_body = body_of("for_each_peripheral");
    let mut peripherals: Vec<String> = peri_body
        .split("<=")
        .filter_map(|chunk| {
            let tok = chunk.trim_end().rsplit(' ').next()?.trim();
            (!tok.is_empty()
                && tok
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                && tok.chars().next()?.is_ascii_uppercase())
            .then(|| tok.to_owned())
        })
        .filter(|p| !(p.starts_with("GPIO") && p[4..].chars().all(|c| c.is_ascii_digit())))
        .collect();
    peripherals.sort();
    peripherals.dedup();

    Ok(EspChip {
        id,
        name,
        arch,
        dram_bytes,
        gpios,
        uarts,
        i2c,
        spi,
        adc,
        peripherals,
    })
}

// ──────────────────────────────────────────────────────────────────────────────
// Finding the vendor files
// ──────────────────────────────────────────────────────────────────────────────

/// The RISC-V chips this IDE offers, in the vendor's own naming.
///
/// Xtensa parts (`esp32`, `esp32s2`, `esp32s3`) are deliberately absent: they
/// need Espressif's rustc fork — see [`Arch`].
pub const RISCV_CHIPS: [&str; 6] = [
    "esp32c2", "esp32c3", "esp32c5", "esp32c6", "esp32c61", "esp32h2",
];

/// Where `esp-metadata-generated` unpacked itself, if any ESP project has ever
/// been built on this machine.
///
/// Development-only, and it shows: the path carries cargo's registry hash and
/// the crate version, both of which are cargo's business rather than ours. That
/// is tolerable precisely because nothing at runtime calls this — the chip
/// definitions it produces are committed. It would not be tolerable in shipping
/// code, where the crate might simply not be there.
pub fn vendor_dir() -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| PathBuf::from(h).join(".cargo"))
        })?;
    let src = home.join("registry").join("src");
    // Newest version wins, so a refreshed toolchain is picked up by rerunning
    // the generator rather than by editing a path.
    let mut found: Vec<PathBuf> = std::fs::read_dir(&src)
        .ok()?
        .flatten()
        .flat_map(|reg| {
            std::fs::read_dir(reg.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("esp-metadata-generated-"))
        })
        .collect();
    found.sort();
    found.pop().map(|p| p.join("src"))
}

/// Read and parse one chip from a vendor directory.
pub fn load(dir: &std::path::Path, chip: &str) -> Result<EspChip, String> {
    let path = dir.join(format!("_generated_{chip}.rs"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    parse(&text)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed `_generated_*.rs`: one of each macro, formatted the way the
    /// real crate is — wrapped mid-list, with the `all(…)` aggregate present.
    const FIXTURE: &str = r#"
macro_rules! chip {
    () => {
        "esp32c3"
    };
}
macro_rules! chip_pretty {
    () => {
        "ESP32-C3"
    };
}
// riscv is named once, over here, in the interrupt plumbing.
macro_rules! memory_range {
    ("DRAM") => {
        0x3FC80000..0x3FCE0000
    };
    (size as str, "DRAM") => {
        "393216"
    };
}
macro_rules! for_each_uart {
    ($($pattern:tt => $code:tt;)*) => {
        macro_rules! _for_each_inner_uart { $(($pattern) => $code;)* ($other : tt) => {}
        } _for_each_inner_uart!((0, UART0, Uart0, U0RXD, U0TXD, U0CTS, U0RTS));
        _for_each_inner_uart!((1, UART1, Uart1, U1RXD, U1TXD, U1CTS,
        U1RTS)); _for_each_inner_uart!((all(0, UART0, Uart0, U0RXD, U0TXD, U0CTS,
        U0RTS), (1, UART1, Uart1, U1RXD, U1TXD, U1CTS, U1RTS)));
    };
}
macro_rules! for_each_i2c_master {
    ($($pattern:tt => $code:tt;)*) => {
        macro_rules! _for_each_inner_i2c_master { $(($pattern) => $code;)* ($other : tt)
        => {} } _for_each_inner_i2c_master!((0, I2C0, I2cExt0, I2CEXT0_SCL,
        I2CEXT0_SDA)); _for_each_inner_i2c_master!((all(0, I2C0, I2cExt0, I2CEXT0_SCL,
        I2CEXT0_SDA)));
    };
}
macro_rules! for_each_spi_master {
    ($($pattern:tt => $code:tt;)*) => {
        macro_rules! _for_each_inner_spi_master { $(($pattern) => $code;)* ($other : tt)
        => {} } _for_each_inner_spi_master!((SPI2, Spi2, FSPICLK[FSPICS0, FSPICS1] [FSPID,
        FSPIQ, FSPIWP, FSPIHD], true));
        _for_each_inner_spi_master!((all(SPI2, Spi2, FSPICLK[FSPICS0] [FSPID], true)));
    };
}
macro_rules! for_each_gpio {
    ($($pattern:tt => $code:tt;)*) => {
        macro_rules! _for_each_inner_gpio { $(($pattern) => $code;)* ($other : tt) => {}
        } _for_each_inner_gpio!((0, GPIO0() () ([Input] [Output])));
        _for_each_inner_gpio!((1, GPIO1() () ([Input] [Output])));
        _for_each_inner_gpio!((4, GPIO4(_0 => MTMS _2 => FSPIHD) (_2 => FSPIHD) ([Input]
        [Output])));
    };
}
macro_rules! for_each_analog_function {
    ($($pattern:tt => $code:tt;)*) => {
        macro_rules! _for_each_inner_analog_function { $(($pattern) => $code;)* ($other :
        tt) => {} } _for_each_inner_analog_function!((ADC1_CH0, GPIO0));
        _for_each_inner_analog_function!((ADC1_CH1, GPIO1));
        _for_each_inner_analog_function!((USB_DM, GPIO18));
        _for_each_inner_analog_function!(((ADC1_CH0, ADCn_CHm, 1, 0), GPIO0));
    };
}
macro_rules! for_each_peripheral {
    ($($pattern:tt => $code:tt;)*) => {
        macro_rules! _for_each_inner_peripheral { $(($pattern) => $code;)* ($other : tt)
        => {} } _for_each_inner_peripheral!((@ peri_type #[doc =
        "GPIO0 peripheral singleton"] GPIO0 <= virtual()));
        _for_each_inner_peripheral!((@ peri_type #[doc =
        "This pin is a strapping pin, it determines how the chip boots (see below, etc)."]
        GPIO2 <= virtual())); _for_each_inner_peripheral!((@ peri_type #[doc =
        "UART0 peripheral singleton"] UART0 <= UART0(UART0 : Uart0)));
        _for_each_inner_peripheral!((@ peri_type #[doc = "LEDC"] LEDC <= LEDC()));
    };
}
"#;

    fn c3() -> EspChip {
        parse(FIXTURE).expect("the fixture parses")
    }

    #[test]
    fn the_chip_identifies_itself() {
        let c = c3();
        assert_eq!(c.id, "esp32c3");
        assert_eq!(c.name, "ESP32-C3");
        assert_eq!(c.arch, Arch::RiscV);
        assert_eq!(c.dram_bytes, 393_216, "384 KiB of on-die SRAM");
    }

    /// The whole reason this is not a two-line `split`.
    #[test]
    fn the_aggregate_invocation_is_not_another_instance() {
        let c = c3();
        assert_eq!(c.uarts.len(), 2, "not 3: {:?}", c.uarts);
        assert_eq!(c.i2c.len(), 1, "not 2: {:?}", c.i2c);
        assert_eq!(c.spi.len(), 1, "not 2: {:?}", c.spi);
    }

    #[test]
    fn a_uart_keeps_all_four_signals() {
        let u = &c3().uarts[0];
        assert_eq!(u.id, 0);
        assert_eq!(u.instance, "UART0");
        assert_eq!((u.rx.as_str(), u.tx.as_str()), ("U0RXD", "U0TXD"));
        assert_eq!((u.cts.as_str(), u.rts.as_str()), ("U0CTS", "U0RTS"));
    }

    #[test]
    fn i2c_signals_are_read_in_the_vendor_order() {
        let i = &c3().i2c[0];
        assert_eq!(
            (i.scl.as_str(), i.sda.as_str()),
            ("I2CEXT0_SCL", "I2CEXT0_SDA")
        );
    }

    /// Two bracket groups in one field, and the second is the one that says
    /// which way the data flows.
    #[test]
    fn spi_splits_its_chip_selects_from_its_data_lines() {
        let s = &c3().spi[0];
        assert_eq!(s.instance, "SPI2");
        assert_eq!(s.clk, "FSPICLK");
        assert_eq!(s.cs, ["FSPICS0", "FSPICS1"]);
        assert_eq!(s.data, ["FSPID", "FSPIQ", "FSPIWP", "FSPIHD"]);
        // D is data OUT and Q is data IN — swapping them wires a project
        // backwards, and the names give no hint which way round they go.
        assert_eq!(s.mosi(), Some("FSPID"));
        assert_eq!(s.miso(), Some("FSPIQ"));
    }

    /// A GPIO line carries its alternate functions inline, unseparated by
    /// commas, so a naive split would read `GPIO4(_0 => MTMS …)` as a number.
    #[test]
    fn gpio_numbers_survive_their_alternate_functions() {
        assert_eq!(c3().gpios, [0, 1, 4]);
    }

    /// USB_DM is an analog function too, and it is not an ADC channel.
    #[test]
    fn only_adc_channels_are_taken_from_the_analog_list() {
        let c = c3();
        assert_eq!(
            c.adc,
            [("ADC1_CH0".to_owned(), 0), ("ADC1_CH1".to_owned(), 1)]
        );
    }

    /// Peripheral doc comments contain commas, parentheses and full stops, so
    /// these names cannot come from a tuple split.
    #[test]
    fn peripherals_are_read_past_their_doc_comments() {
        let p = c3().peripherals;
        assert!(p.contains(&"UART0".to_owned()), "{p:?}");
        assert!(p.contains(&"LEDC".to_owned()), "{p:?}");
        // A pad is not a peripheral anyone shops for.
        assert!(!p.iter().any(|n| n.starts_with("GPIO")), "{p:?}");
    }

    #[test]
    fn a_file_that_is_not_metadata_is_refused() {
        assert!(parse("fn main() {}").is_err());
    }

    #[test]
    fn targets_follow_the_core_not_the_family() {
        // C2/C3 are imc; everything later gained atomics.
        assert_eq!(
            Arch::RiscV.target("esp32c3"),
            Some("riscv32imc-unknown-none-elf")
        );
        assert_eq!(
            Arch::RiscV.target("esp32c6"),
            Some("riscv32imac-unknown-none-elf")
        );
        assert_eq!(Arch::Xtensa.target("esp32"), None, "not a rustup target");
    }

    /// Against the real vendor files. Ignored: needs an ESP project to have been
    /// built on this machine, so that the crate is in the cargo registry.
    ///
    /// `cargo test -- --ignored the_real_metadata_parses --nocapture`
    #[test]
    #[ignore]
    fn the_real_metadata_parses() {
        let Some(dir) = vendor_dir() else {
            eprintln!("esp-metadata-generated not in the cargo registry — skipping");
            return;
        };
        println!("reading {}", dir.display());
        for chip in RISCV_CHIPS {
            let c = load(&dir, chip).unwrap_or_else(|e| panic!("{chip}: {e}"));
            println!(
                "{:<9} {:<10} {:>3} GPIO  {:>3} KiB SRAM  {} UART  {} I2C  {} SPI  {:>2} ADC  {:>2} peri",
                c.id,
                c.name,
                c.gpios.len(),
                c.dram_bytes / 1024,
                c.uarts.len(),
                c.i2c.len(),
                c.spi.len(),
                c.adc.len(),
                c.peripherals.len(),
            );
            assert_eq!(c.id, chip);
            assert_eq!(c.arch, Arch::RiscV, "{chip} is meant to be RISC-V");
            assert!(!c.gpios.is_empty(), "{chip} has no GPIOs");
            assert!(c.dram_bytes > 100_000, "{chip} SRAM looks wrong");
            assert!(!c.uarts.is_empty(), "{chip} has no UART");
            assert!(!c.spi.is_empty(), "{chip} has no SPI master");
            assert!(!c.i2c.is_empty(), "{chip} has no I2C master");
            // The aggregate trap again, this time against real files: no chip
            // in this family has more than three of any of these.
            assert!(c.uarts.len() <= 3, "{chip}: {} UARTs?", c.uarts.len());
            assert!(c.i2c.len() <= 3, "{chip}: {} I2Cs?", c.i2c.len());
            assert!(c.spi.len() <= 3, "{chip}: {} SPIs?", c.spi.len());
            // Every SPI master must resolve both data directions, or the
            // generated project would be wired one-way.
            for s in &c.spi {
                assert!(s.mosi().is_some() && s.miso().is_some(), "{chip}: {s:?}");
            }
            // ADC pads must be real GPIOs.
            for (ch, pad) in &c.adc {
                assert!(c.gpios.contains(pad), "{chip}: {ch} on absent GPIO{pad}");
            }
            // Strictly increasing and unique - but NOT contiguous, which is
            // the point of checking. A definition must lay out the numbers the
            // vendor gives, not `0..n`.
            assert!(
                c.gpios.windows(2).all(|w| w[0] < w[1]),
                "{chip}: {:?}",
                c.gpios
            );
        }

        // Two of the six skip a block of GPIO numbers - they are wired to the
        // internal SPI flash and never reach a pad. Asserted rather than
        // described, because the obvious assumption is that GPIOs run 0..n, and
        // laying these out that way would put every pin past the gap under the
        // wrong number.
        for (chip, gap) in [("esp32c5", 15..=22u8), ("esp32h2", 15..=21u8)] {
            let c = load(&dir, chip).unwrap();
            for n in gap.clone() {
                assert!(!c.gpios.contains(&n), "{chip} gained GPIO{n}");
            }
            assert!(
                c.gpios.contains(&(gap.end() + 1)),
                "{chip} lost the pins after the gap"
            );
        }
        for chip in ["esp32c2", "esp32c3", "esp32c6", "esp32c61"] {
            let c = load(&dir, chip).unwrap();
            let want: Vec<u8> = (0..c.gpios.len() as u8).collect();
            assert_eq!(c.gpios, want, "{chip} was contiguous — recheck");
        }

        // The C61's metadata carries NO analog functions and no ADC peripheral,
        // though its datasheet describes a SAR ADC. That is the vendor data
        // being incomplete for a new part, not the chip lacking one - recorded
        // here so the missing ADC pins in its generated definition read as a
        // known gap rather than a bug in the reader.
        let c61 = load(&dir, "esp32c61").unwrap();
        assert!(c61.adc.is_empty(), "esp32c61 gained an ADC — regenerate");
        assert!(
            !c61.peripherals.iter().any(|p| p.starts_with("ADC")),
            "esp32c61 gained an ADC peripheral — regenerate"
        );

        // …and the Xtensa parts must be readable AND correctly refused a target.
        for chip in ["esp32", "esp32s2", "esp32s3"] {
            let c = load(&dir, chip).unwrap_or_else(|e| panic!("{chip}: {e}"));
            assert_eq!(c.arch, Arch::Xtensa, "{chip}");
            assert_eq!(c.arch.target(chip), None);
        }
    }
}
