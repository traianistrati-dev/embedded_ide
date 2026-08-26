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
    /// The Rust target triple for a chip, or `None` if there is not one.
    ///
    /// The Xtensa triples are real and `rustc --print target-list` knows them —
    /// but see [`Arch::needs_esp_toolchain`]: knowing the name is not the same
    /// as being able to build it.
    pub fn target(self, chip: &str) -> Option<&'static str> {
        match self {
            // The C2/C3 cores have no atomics extension; everything later does.
            Arch::RiscV => Some(match chip {
                "esp32c2" | "esp32c3" => "riscv32imc-unknown-none-elf",
                _ => "riscv32imac-unknown-none-elf",
            }),
            Arch::Xtensa => match chip {
                "esp32" => Some("xtensa-esp32-none-elf"),
                "esp32s2" => Some("xtensa-esp32s2-none-elf"),
                "esp32s3" => Some("xtensa-esp32s3-none-elf"),
                _ => None,
            },
        }
    }

    /// Whether building for this architecture needs Espressif's fork of rustc.
    ///
    /// Xtensa does, and stock Rust cannot substitute for it. `rustc` ships the
    /// target *definitions* — `xtensa-esp32-none-elf` is in `--print
    /// target-list` — but no `core` for them, and building one with nightly and
    /// `-Z build-std` fails in LLVM before it reaches any user code:
    ///
    /// ```text
    /// error: data-layout for target `xtensa-esp32-none-elf`,
    ///   `e-m:e-p:32:32-v1:8:8-i64:64-i128:128-n32`, differs from LLVM target's
    ///   `xtensa-none-elf` default layout, `e-m:e-p:32:32-i8:8:32-…`
    /// ```
    ///
    /// The `esp` toolchain (installed by `espup`) carries a patched LLVM, and is
    /// invoked as `cargo +esp`. Nothing in this IDE passes that yet — cargo is
    /// launched from eleven places, none of which take a toolchain — which is
    /// why the Xtensa parts are not offered in the chip picker.
    pub fn needs_esp_toolchain(self) -> bool {
        self == Arch::Xtensa
    }
}

/// One pad, and the directions it supports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Gpio {
    pub number: u8,
    pub input: bool,
    pub output: bool,
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

/// One DMA channel of the chip, and the engine that owns it.
///
/// Two shapes hide behind one macro, and the difference decides whether a
/// channel can be handed out freely:
///
/// * **GDMA** (`AHB_GDMA`, every RISC-V part and the S3) — a pool. `DMA_CH0`
///   serves any peripheral that asks, so the allocator just takes a free one.
/// * **Per-peripheral DMA** (the original ESP32 and the S2) — `DMA_SPI2` is
///   wired to SPI2 and to nothing else. Handing it to an I2S would not compile.
///
/// Which it is comes from [`DmaChannel::compatible`], not from the vendor's bare
/// `((shared))` / `((split))` marker: the marker is one token that could change
/// spelling, while the compatibility lists ARE the answer and are checkable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DmaChannel {
    /// `AHB_GDMA`, `SPI_DMA`, `I2S_DMA`, `CRYPTO_DMA`, `COPY_DMA`.
    pub engine: String,
    /// The `esp_hal::peripherals` singleton — `DMA_CH0`, `DMA_SPI2`.
    pub name: String,
    /// Every peripheral this channel may serve, as the vendor's
    /// `compatible = [...]` states it: `[SPI2, UHCI0, I2S0, AES, SHA,
    /// APB_SARADC, PARL_IO]` on an ESP32-C5, `[SPI2]` alone on an ESP32's
    /// `DMA_SPI2`. Empty is real too — the S2's `DMA_COPY` serves nothing that
    /// has a driver.
    ///
    /// This is the same list the C5 datasheet gives in prose ("shared by
    /// peripherals with the GDMA feature, consisting of SPI2, UHCI0, I2S, AES,
    /// SHA, ADC, and PARLIO"), which is how it was checked.
    pub compatible: Vec<String>,
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
    /// Every GPIO the chip has, with what it can do.
    ///
    /// Not every pad can drive: an ESP32's GPIO34..39 are input-only, and the
    /// vendor says so with an EMPTY second capability group —
    /// `GPIO34() () ([Input] [])`. Reading the number and ignoring the brackets
    /// is how a generated project offers `Output::new(peripherals.GPIO34)` and
    /// fails to compile on `PeripheralOutput`.
    pub gpios: Vec<Gpio>,
    pub uarts: Vec<Uart>,
    pub i2c: Vec<I2cMaster>,
    pub spi: Vec<SpiMaster>,
    /// `("ADC1_CH0", 0)` — the analog channel and the GPIO carrying it. Unlike
    /// every other signal here, an ADC channel is bonded to ONE pad: the GPIO
    /// matrix routes digital signals, not analog ones.
    pub adc: Vec<(String, u8)>,
    /// Every peripheral singleton the chip has, GPIOs excluded.
    pub peripherals: Vec<String>,
    /// The chip's DMA channels. Empty for a part with no DMA at all.
    pub dma: Vec<DmaChannel>,
    /// True when the channels are a POOL any peripheral may draw from (GDMA),
    /// false when each is bolted to one peripheral.
    ///
    /// DERIVED, not read: every channel offering the same non-empty set of
    /// peripherals is what "pool" means. Not a detail — it is exactly the
    /// question [`DmaDef::mux`] asks, and getting it backwards would let the
    /// allocator hand an ESP32's `DMA_SPI2` to an I2S.
    ///
    /// [`DmaDef::mux`]: crate::panels::mcu_module::mcu_def::DmaDef::mux
    pub dma_shared: bool,
    /// Every output the PLL can be SET to, in Hz, lowest first.
    ///
    /// Two sources, because the vendor uses two shapes and they do not overlap:
    ///
    /// * `impl PllClkConfig { fn value() }` — a selectable PLL. The ESP32, S2,
    ///   S3 and C3 run theirs at 320 **or** 480 MHz.
    /// * a `pub fn pll*_frequency() -> u32 { 480000000 }` literal — a fixed one.
    ///   The C2, C5, C6, C61 and H2 have no enum at all.
    ///
    /// The enum wins where both exist, and the difference is not cosmetic: the
    /// Xtensa parts' only literal constants are TAPS (`pll_f160m_clk`), so
    /// reading those gave 160 MHz for a PLL that runs at 480 — and 160 divides
    /// into none of the 80/160/240 those chips offer, so they were refused a
    /// clock tree entirely.
    ///
    /// The old, single-value shape.
    ///
    /// NOT in the clock-tree TYPES — those only name the nodes. The frequencies
    /// live in `implement_peripheral_clocks`, as generated functions whose whole
    /// body is a literal:
    ///
    /// ```text
    /// pub fn pll_clk_frequency(clocks: &mut ClockTree) -> u32 { 480000000 }
    /// ```
    ///
    /// The largest such `pll*` constant is the PLL proper — the others
    /// (`pll_f80m`, `pll_f160m`) are its taps.
    pub pll_hz: Vec<u32>,
    /// The crystal frequencies this part accepts, in Hz.
    ///
    /// Usually one; an ESP32-C2 takes either 26 or 40 MHz. From
    /// `impl XtalClkConfig { fn value() }`, which unlike most of the clock tree
    /// DOES state its numbers in the types.
    pub xtal_hz: Vec<u32>,
    /// `("USB_DM", 18)` — the analog list's non-ADC entries.
    ///
    /// Kept apart from [`EspChip::adc`] because they answer a different
    /// question, but read from the SAME macro: the vendor files USB pads under
    /// "analog functions", and a reader that keeps only the `ADC*` entries
    /// silently loses the two pads that can carry USB at all.
    pub analog: Vec<(String, u8)>,
}

impl EspChip {
    /// The `(D-, D+)` pads, when this chip has USB.
    ///
    /// Matched on the SUFFIX, not the whole name: esp-metadata 0.4 called these
    /// `USB_DM`/`USB_DP` and 0.5 calls them `USJ_DM`/`USJ_DP` (USB Serial/JTAG).
    /// A reader pinned to the old spelling kept working and silently dropped
    /// the only two pads that can carry USB — which the ESP32-C3 yardstick
    /// caught, and nothing else would have.
    pub fn usb_pads(&self) -> Option<(u8, u8)> {
        let find = |suffix: &str| {
            self.analog
                .iter()
                .find(|(n, _)| n.ends_with(suffix))
                .map(|(_, pad)| *pad)
        };
        Some((find("_DM")?, find("_DP")?))
    }
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

    // `(34, GPIO34() () ([Input] []))` — the number, then the pad's signals,
    // then the capability groups. Only the LAST parenthesised group matters
    // here, and an empty bracket inside it is the whole point.
    let gpio_body = body_of("for_each_gpio");
    let mut gpios: Vec<Gpio> = instances(&gpio_body, "gpio")
        .iter()
        .filter_map(|t| {
            let f = fields(t);
            let number = f.first()?.parse::<u8>().ok()?;
            let caps = f.get(1).map(String::as_str).unwrap_or("");
            let caps = &caps[caps.rfind('(').map_or(0, |i| i + 1)..];
            Some(Gpio {
                number,
                input: caps.contains("[Input]"),
                output: caps.contains("[Output]"),
            })
        })
        .collect();
    gpios.sort_unstable_by_key(|g| g.number);
    gpios.dedup_by_key(|g| g.number);
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
    let analog_all: Vec<(String, u8)> = instances(&analog_body, "analog_function")
        .iter()
        .filter_map(|t| {
            let f = fields(t);
            if f.len() < 2 {
                return None;
            }
            let pad = f[1].strip_prefix("GPIO")?.parse::<u8>().ok()?;
            Some((f[0].clone(), pad))
        })
        .collect();
    let adc: Vec<(String, u8)> = analog_all
        .iter()
        .filter(|(n, _)| n.starts_with("ADC"))
        .cloned()
        .collect();
    let analog: Vec<(String, u8)> = analog_all
        .into_iter()
        .filter(|(n, _)| !n.starts_with("ADC"))
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

    // DMA. The macro emits each channel FOUR times — a bare name, an
    // `any_channel` alias, the full entry, and the whole list again under a
    // `shared(...)` / `split(...)` heading. Only the full entry carries
    // `compatible =`, and only it starts with the engine's quoted name, so
    // those two conditions together pick each channel exactly once.
    let dma_body = body_of("for_each_dma_channel");
    let dma: Vec<DmaChannel> = instances(&dma_body, "dma_channel")
        .iter()
        .filter(|t| t.starts_with('"') && t.contains("compatible ="))
        .filter_map(|t| {
            let f = fields(t);
            let engine = f.first()?.trim_matches('"').to_owned();
            let name = f.get(1)?.trim().to_owned();
            let compatible = f
                .iter()
                .find_map(|x| x.trim().strip_prefix("compatible ="))
                .map(bracket_list)
                .unwrap_or_default();
            Some(DmaChannel {
                engine,
                name,
                compatible,
            })
        })
        .collect();
    // A pool is "every channel serves the same peripherals, and more than one
    // of them". An ESP32's four channels each name a single different one, so
    // this is false there — which is the whole difference.
    //
    // Deliberately NOT "more than one channel": the ESP32-C2's GDMA has exactly
    // one, and it still serves either of two peripherals. Counting channels
    // would have described that single pool channel as bolted to a peripheral.
    let dma_shared = dma.first().is_some_and(|first| first.compatible.len() > 1)
        && dma.iter().all(|c| c.compatible == dma[0].compatible);

    // `pub fn <node>_frequency(…) -> u32 { 480000000 }`, keeping only the
    // bodies that ARE a literal: the rest are computed from the live
    // configuration and cannot be read as a constant.
    //
    // The ARGUMENT LIST is deliberately not matched. esp-metadata 0.4 wrote
    // `_frequency(clocks: &mut ClockTree)` and 0.5 writes `_frequency()`; a
    // reader that pinned the old signature kept parsing, kept producing chip
    // definitions, and silently dropped every clock tree — which is exactly what
    // happened, and why `the_real_metadata_parses` now asserts a PLL is found.
    let flat = src.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut pll_hz: Vec<u32> = Vec::new();
    // `impl PllClkConfig { pub fn value(&self) -> u32 { match self {
    //  PllClkConfig::_320 => 320000000, PllClkConfig::_480 => 480000000, } } }`
    if let Some(at) = flat.find("impl PllClkConfig { pub fn value(&self) -> u32 { match self {")
        && let Some(end) = flat[at..].find("} }")
    {
        for arm in flat[at..at + end].split("PllClkConfig::_").skip(1) {
            if let Some((_, hz)) = arm.split_once("=> ")
                && let Ok(v) = hz
                    .trim_end_matches(',')
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .parse()
            {
                pll_hz.push(v);
            }
        }
    }
    let mut fixed: Option<u32> = None;
    for chunk in flat.split("pub fn ").skip(1) {
        let Some((name, rest)) = chunk.split_once("_frequency(") else {
            continue;
        };
        if !name.starts_with("pll") || name.contains(' ') {
            continue;
        }
        let Some((_args, body)) = rest.split_once(") -> u32 { ") else {
            continue;
        };
        if let Ok(hz) = body.split(' ').next().unwrap_or("").parse::<u32>() {
            fixed = Some(fixed.map_or(hz, |cur: u32| cur.max(hz)));
        }
    }
    // Only when the enum said nothing: a literal beside a selectable PLL is one
    // of its taps, not the PLL.
    if pll_hz.is_empty() {
        pll_hz.extend(fixed);
    }
    pll_hz.sort_unstable();
    pll_hz.dedup();

    // `impl XtalClkConfig { pub fn value(&self) -> u32 { match self {
    //  XtalClkConfig::_40 => 40000000, } } }`
    let mut xtal_hz: Vec<u32> = Vec::new();
    if let Some(at) = flat.find("impl XtalClkConfig { pub fn value(&self) -> u32 { match self {")
        && let Some(end) = flat[at..].find("} }")
    {
        for arm in flat[at..at + end].split("XtalClkConfig::_").skip(1) {
            if let Some((_, hz)) = arm.split_once("=> ")
                && let Ok(v) = hz
                    .trim_end_matches(',')
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .parse()
            {
                xtal_hz.push(v);
            }
        }
    }
    xtal_hz.sort_unstable();
    xtal_hz.dedup();

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
        dma,
        dma_shared,
        analog,
        pll_hz,
        xtal_hz,
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
        assert_eq!(
            c3().gpios.iter().map(|g| g.number).collect::<Vec<_>>(),
            [0, 1, 4]
        );
        assert!(c3().gpios.iter().all(|g| g.input && g.output));
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

    /// The C5's GDMA, checked against the sentence its datasheet writes in prose.
    ///
    /// Section 4.1.1.4: "The GDMA has six independent channels, three transmit
    /// channels and three receive channels. These channels are shared by
    /// peripherals with the GDMA feature, consisting of SPI2, UHCI0, I2S, AES,
    /// SHA, ADC, and PARLIO." Three channels, each with a TX and an RX half,
    /// and one compatibility list of seven — which is what this asserts, under
    /// the vendor's own spellings (`APB_SARADC` for the ADC, `PARL_IO`).
    #[test]
    #[ignore]
    fn the_c5_gdma_matches_its_datasheet() {
        let c5 = load(&vendor_dir().expect("esp-metadata"), "esp32c5").expect("parses");
        assert_eq!(
            c5.dma.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["DMA_CH0", "DMA_CH1", "DMA_CH2"],
        );
        assert!(c5.dma.iter().all(|c| c.engine == "AHB_GDMA"));
        assert_eq!(
            c5.dma[0].compatible,
            [
                "SPI2",
                "UHCI0",
                "I2S0",
                "AES",
                "SHA",
                "APB_SARADC",
                "PARL_IO"
            ],
        );
        assert!(c5.dma_shared, "GDMA is a pool");
    }

    /// The two ESP32s whose DMA is NOT a pool must not be read as one.
    ///
    /// `DMA_SPI2` is wired to SPI2 and to nothing else. Were `dma_shared` true
    /// here, the allocator would happily hand it to an I2S and the project
    /// would not compile.
    #[test]
    #[ignore]
    fn per_peripheral_dma_is_not_a_pool() {
        let dir = vendor_dir().expect("esp-metadata");
        for id in ["esp32", "esp32s2"] {
            let c = load(&dir, id).expect("parses");
            assert!(!c.dma_shared, "{id}: bolted channels read as a pool");
            let spi2 = c
                .dma
                .iter()
                .find(|d| d.name == "DMA_SPI2")
                .unwrap_or_else(|| panic!("{id} has no DMA_SPI2"));
            assert_eq!(spi2.compatible, ["SPI2"], "{id}");
        }
        // The S2 has a channel that serves nothing with a driver; an empty
        // list is real data, not a parse failure.
        let s2 = load(&dir, "esp32s2").unwrap();
        let copy = s2.dma.iter().find(|d| d.name == "DMA_COPY").unwrap();
        assert!(copy.compatible.is_empty());
    }

    /// Every shipped chip has DMA, and it is read the same way for all of them.
    #[test]
    #[ignore]
    fn every_chip_reports_its_dma_channels() {
        let dir = vendor_dir().expect("esp-metadata");
        for id in RISCV_CHIPS.iter().chain(&["esp32", "esp32s2", "esp32s3"]) {
            let c = load(&dir, id).expect("parses");
            assert!(!c.dma.is_empty(), "{id}: no DMA channels read");
            assert!(
                c.dma.iter().all(|d| d.name.starts_with("DMA_")),
                "{id}: a channel is not a DMA_* singleton: {:?}",
                c.dma
            );
        }
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
        // Xtensa triples are real and rustc knows them; what they are NOT is
        // buildable with a stock toolchain.
        assert_eq!(Arch::Xtensa.target("esp32"), Some("xtensa-esp32-none-elf"));
        assert!(Arch::Xtensa.needs_esp_toolchain());
        assert!(!Arch::RiscV.needs_esp_toolchain());
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
            // The PLL is what a CPU-clock graph divides down from, so a chip
            // without one gets no clock tree rather than a wrong one.
            println!(
                "          PLL {:?} MHz",
                c.pll_hz.iter().map(|h| h / 1_000_000).collect::<Vec<_>>()
            );
            // Asserted, not just printed: losing the PLL costs every generated
            // chip its clock tree, and nothing else about the definition looks
            // any different when it happens.
            assert!(!c.pll_hz.is_empty(), "{chip}: no PLL frequency found");
            assert!(!c.xtal_hz.is_empty(), "{chip}: no crystal frequency found");
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
                assert!(
                    c.gpios.iter().any(|g| g.number == *pad),
                    "{chip}: {ch} on absent GPIO{pad}"
                );
            }
            // Strictly increasing and unique - but NOT contiguous, which is
            // the point of checking. A definition must lay out the numbers the
            // vendor gives, not `0..n`.
            assert!(
                c.gpios.windows(2).all(|w| w[0].number < w[1].number),
                "{chip}: {:?}",
                c.gpios
            );
        }

        // Some parts skip a block of GPIO numbers — wired to the internal SPI
        // flash, never reaching a pad. Asserted rather than described, because
        // the obvious assumption is that GPIOs run 0..n, and laying these out
        // that way would put every pin past the gap under the wrong number.
        //
        // These are per-VERSION facts, not laws: esp-metadata 0.4 showed a gap
        // at 15..22 on the C5 that 0.5 filled in. That is the point of pinning
        // them — the change was silent everywhere else.
        for (chip, gaps) in [
            ("esp32h2", &[15u8, 16, 17, 18, 19, 20, 21][..]),
            ("esp32", &[24, 28, 29, 30, 31][..]),
            ("esp32s2", &[22, 23, 24, 25][..]),
            ("esp32s3", &[22, 23, 24, 25][..]),
        ] {
            let c = load(&dir, chip).unwrap();
            for n in gaps {
                assert!(
                    !c.gpios.iter().any(|g| g.number == *n),
                    "{chip} gained GPIO{n}"
                );
            }
            assert!(
                c.gpios.iter().any(|g| g.number > *gaps.last().unwrap()),
                "{chip} lost the pins after the gap"
            );
        }
        for chip in ["esp32c2", "esp32c3", "esp32c5", "esp32c6", "esp32c61"] {
            let c = load(&dir, chip).unwrap();
            let want: Vec<u8> = (0..c.gpios.len() as u8).collect();
            let got: Vec<u8> = c.gpios.iter().map(|g| g.number).collect();
            assert_eq!(got, want, "{chip} grew a gap — recheck the layout");
        }

        // The ESP32 is the only part with pads that cannot drive. Its GPIO34..39
        // are input-only, and the vendor says so with an empty second capability
        // group rather than by omitting anything.
        let esp32 = load(&dir, "esp32").unwrap();
        for n in 34..=39u8 {
            let pad = esp32.gpios.iter().find(|g| g.number == n).unwrap();
            assert!(pad.input && !pad.output, "GPIO{n}: {pad:?}");
        }
        for chip in RISCV_CHIPS {
            let c = load(&dir, chip).unwrap();
            assert!(
                c.gpios.iter().all(|g| g.input && g.output),
                "{chip} grew a one-way pad"
            );
        }

        // The C61 carried no analog functions at all in esp-metadata 0.4, though
        // its datasheet describes a SAR ADC; 0.5 filled them in. Kept as an
        // assertion because the two readings differ, and a definition generated
        // against the older data silently lacked every ADC pin.
        let c61 = load(&dir, "esp32c61").unwrap();
        assert!(
            !c61.adc.is_empty(),
            "esp32c61 lost its ADC — is an older esp-metadata being read?"
        );

        // The Xtensa parts read the same way and name their own triples; what
        // sets them apart is needing Espressif's fork to build.
        for (chip, target) in [
            ("esp32", "xtensa-esp32-none-elf"),
            ("esp32s2", "xtensa-esp32s2-none-elf"),
            ("esp32s3", "xtensa-esp32s3-none-elf"),
        ] {
            let c = load(&dir, chip).unwrap_or_else(|e| panic!("{chip}: {e}"));
            assert_eq!(c.arch, Arch::Xtensa, "{chip}");
            assert_eq!(c.arch.target(chip), Some(target));
            assert!(c.arch.needs_esp_toolchain(), "{chip}");
        }
    }
}
