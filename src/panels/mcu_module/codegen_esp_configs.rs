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
//! pub fn init<'d>(…) -> Uart<'d, Blocking> { … }
//! pub fn init_async<'d>(…) -> Uart<'d, Async> { … }   ← Async runtime only
//! // ── Using UART0 … ──               ← commented example, matches the runtime
//! ```
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
//! Real-compile verified on ESP32-C3 / esp-hal 1.1, both runtimes.

use super::codegen_esp::EspRuntime;
use super::modules::{
    AsyncBusMode, DacModuleConfig, I2cModuleConfig, I2sDirection, I2sFormat, I2sModuleConfig,
    I2sStandard, McpwmModuleConfig, Parity, ParlIoModuleConfig, PcntModuleConfig, RmtModuleConfig,
    SpiModuleConfig, StopBits, TimerModuleConfig, UsartModuleConfig,
};
use std::collections::BTreeMap;

/// Marker pair bounding the auto-updated half of a config file. Identical to the
/// STM32 templates so `sync_config_files` splices both the same way.
const GEN_BEGIN: &str = "// <<< GENERATED>>>";
const GEN_END: &str = "// <<< GENERATED END >>>";

/// `use` items, then the marker block around `consts`, then the user's `body`,
/// then a commented usage `example` at the very end of the file.
///
/// The example is COMMENTED on purpose: it names a handle (`_uart0`) that only
/// exists in `main.rs`, so as code it would not compile here — it is there to be
/// read and copied into the loop. It also tracks the runtime: a Runtime /
/// Init-API Apply rewrites config files wholesale (`sync_config_files(force)`),
/// so switching Blocking ⇄ Async replaces the example along with the `init` it
/// describes.
fn file(uses: &str, consts: &str, body: &str, example: &str) -> String {
    format!(
        "{uses}\n\
         {GEN_BEGIN}\n\
         // Peripheral config (from the Virtual Module) — auto-updated; edit in the module.\n\
         {consts}{GEN_END}\n\
         \n\
         {body}\
         \n\
         {example}"
    )
}

/// Frame a usage snippet as a comment block. Each line gets `// ` so the whole
/// thing is inert until the user copies it out.
fn example_block(title: &str, lines: &[String]) -> String {
    let mut s = format!("// ── {title} ──\n");
    for l in lines {
        if l.is_empty() {
            s.push_str("//\n");
        } else {
            s.push_str(&format!("// {l}\n"));
        }
    }
    s
}

/// Pick the example for the runtime and substitute the handle name.
fn example_for(
    title: &str,
    handle: &str,
    blocking: &[&str],
    asyncs: &[&str],
    rt: EspRuntime,
) -> String {
    let src = if rt == EspRuntime::Async {
        asyncs
    } else {
        blocking
    };
    let lines: Vec<String> = src.iter().map(|l| l.replace("{H}", handle)).collect();
    example_block(
        &format!(
            "{title} ({})",
            if rt == EspRuntime::Async {
                "async"
            } else {
                "blocking"
            }
        ),
        &lines,
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

/// The `init_async` twin of a blocking `init`, emitted ONLY on the Async
/// runtime — an EXTRA function, never a replacement: one bus can stay blocking
/// while another awaits, and dropping `init` would break code already calling it.
///
/// esp-hal builds both the same way; `.into_async()` at the end swaps the mode
/// type (`Uart<'d, Blocking>` → `Uart<'d, Async>`) and with it the method set
/// (`write` → `write_async`, …). No DMA channels, no extra parameters and no new
/// dependency — `Async` is esp-hal's own marker. Verified under `#[esp_rtos::main]`.
fn async_twin(doc: &str, params: &str, ty: &str, ctor: &str, chain: &str) -> String {
    format!(
        "\n\
         /// {doc}\n\
         ///\n\
         /// Same construction as `init`, then `.into_async()` — the methods become\n\
         /// `*_async` and `.await`-able on the embassy executor.\n\
         pub fn init_async<'d>(\n\
         {params}) -> {ty}<'d, Async> {{\n\
         {ctor}\
         {chain}\x20       .into_async()\n\
         }}\n"
    )
}

/// The `use esp_hal::…;` mode import — both markers are needed once `init_async`
/// exists beside `init`.
fn mode_import(rt: EspRuntime) -> &'static str {
    if rt == EspRuntime::Async {
        "use esp_hal::{Async, Blocking};\n"
    } else {
        "use esp_hal::Blocking;\n"
    }
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

fn uart_file(n: u8, sigs: &[&str], cfg: Option<&UsartModuleConfig>, rt: EspRuntime) -> String {
    let consts = format!(
        "const BAUDRATE: u32 = {};\n\
         const DATA_BITS: DataBits = DataBits::{};\n\
         const PARITY: Parity = Parity::{};\n\
         const STOP_BITS: StopBits = StopBits::{};\n",
        cfg.map_or(115_200, |c| c.baud_rate),
        data_bits_variant(cfg.map_or(8, |c| c.data_bits)),
        parity_variant(cfg.map_or(Parity::None, |c| c.parity)),
        stop_bits_variant(cfg.map_or(StopBits::One, |c| c.stop_bits)),
    );
    let params = format!(
        "    uart: impl Instance + 'd,\n{}",
        params_for(
            sigs,
            &[
                ("rx", "impl PeripheralInput<'d>"),
                ("tx", "impl PeripheralOutput<'d>"),
            ],
        )
    );
    let chain = chain_for(sigs, &[("rx", "with_rx"), ("tx", "with_tx")]);
    let ctor = "    let config = Config::default()\n\
                \x20       .with_baudrate(BAUDRATE)\n\
                \x20       .with_data_bits(DATA_BITS)\n\
                \x20       .with_parity(PARITY)\n\
                \x20       .with_stop_bits(STOP_BITS);\n\
                \x20   // `unwrap`: these values come from the Virtual Module's UI, which\n\
                \x20   // range-limits them — a failure here is a bug in the generator,\n\
                \x20   // not a runtime condition the firmware could recover from.\n\
                \x20   Uart::new(uart, config)\n\
                \x20       .unwrap()\n";
    let mut body = format!(
        "/// UART{n} — blocking driver.\n\
         ///\n\
         /// Generic over the pins so this file never names a GPIO: `main.rs`\n\
         /// passes the ones wired on the Pins canvas.\n\
         pub fn init<'d>(\n\
         {params}) -> Uart<'d, Blocking> {{\n\
         {ctor}\
         {chain}}}\n"
    );
    if rt == EspRuntime::Async {
        body.push_str(&async_twin(
            &format!("UART{n} — async driver."),
            &params,
            "Uart",
            ctor,
            &chain,
        ));
    }
    let example = example_for(
        &format!("Using UART{n}"),
        &format!("_uart{n}"),
        &[
            "In main.rs, after the init above:",
            "",
            "    // Send",
            "    {H}.write(b\"hello\\r\\n\").ok();",
            "    {H}.flush().ok();",
            "",
            "    // Receive what has already arrived (never blocks)",
            "    let mut buf = [0u8; 32];",
            "    if let Ok(n) = {H}.read_buffered(&mut buf) {",
            "        // buf[..n] holds the bytes",
            "    }",
            "",
            "    // Or block until at least one byte shows up",
            "    let n = {H}.read(&mut buf).unwrap();",
        ],
        &[
            "main.rs calls `init_async`, so the handle is a `Uart<'_, Async>`:",
            "",
            "    // Send — yields to the executor instead of spinning",
            "    {H}.write_async(b\"hello\\r\\n\").await.ok();",
            "    {H}.flush_async().await.ok();",
            "",
            "    // Receive whatever arrives (returns once there is >= 1 byte)",
            "    let mut buf = [0u8; 32];",
            "    let n = {H}.read_async(&mut buf).await.unwrap_or(0);",
            "",
            "    // ...or exactly this many",
            "    {H}.read_exact_async(&mut buf).await.ok();",
            "",
            "    // Time out a read — the reason for being on an executor at all:",
            "    use embassy_time::{with_timeout, Duration};",
            "    let read = {H}.read_async(&mut buf);",
            "    match with_timeout(Duration::from_millis(500), read).await {",
            "        Ok(Ok(n)) => { /* n bytes */ }",
            "        Ok(Err(_)) => { /* UART error */ }",
            "        Err(_) => { /* timed out */ }",
            "    }",
            "",
            "    // `init` is still there if you want this bus blocking instead.",
        ],
        rt,
    );
    file(
        &format!(
            "{}\
             use esp_hal::gpio::interconnect::{{PeripheralInput, PeripheralOutput}};\n\
             use esp_hal::uart::{{Config, DataBits, Instance, Parity, StopBits, Uart}};\n",
            mode_import(rt)
        ),
        &consts,
        &body,
        &example,
    )
}

// ── SPI (slave) ─────────────────────────────────────────────────────────────

/// One SPI instance as the SLAVE end of the bus.
///
/// # A different driver, not a flag
///
/// `esp_hal::spi::slave::Spi` is its own type in its own module. It takes no
/// frequency — the master supplies the clock — and its pin directions are the
/// mirror of the master's: SCK, MOSI and CS come IN, MISO goes out.
///
/// # DMA is not optional here
///
/// esp-hal's slave "can only be used with DMA": there is no CPU path and no
/// blocking transfer, because the master decides when bytes move. So this file
/// always takes a channel, and a project with none left gets no slave at all.
///
/// # No async twin
///
/// The driver has no `into_async`. Waiting is done on the TRANSFER — `wait()`
/// or `is_done()` — which is the same on either runtime.
fn spi_slave_file(n: u8, sigs: &[&str], cfg: Option<&SpiModuleConfig>, rt: EspRuntime) -> String {
    let mode = cfg.map_or(1, |c| c.mode).min(3);
    let consts = format!(
        "const MODE: Mode = Mode::_{mode}; // CPOL/CPHA - the MASTER's choice\n\
         // Both directions move at once on a full-duplex bus, so one size.\n\
         const BUFFER_BYTES: usize = 4096;\n"
    );
    // The mirror image of the master's bounds: only MISO is driven.
    let params = format!(
        "    spi: impl Instance + 'd,\n\
         {}\
         \x20   dma: impl DmaChannelFor<AnySpi<'d>>,\n",
        params_for(
            sigs,
            &[
                ("sck", "impl PeripheralInput<'d>"),
                ("mosi", "impl PeripheralInput<'d>"),
                ("miso", "impl PeripheralOutput<'d>"),
                ("cs", "impl PeripheralInput<'d>"),
            ],
        )
    );
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
        "/// SPI{n} — the SLAVE end, on DMA.\n\
         ///\n\
         /// No frequency: the master clocks this bus. Nothing moves until the\n\
         /// master asserts CS, which is why there is no blocking transfer.\n\
         ///\n\
         /// The BUFFERS come back with the driver: unlike the master's, this\n\
         /// driver takes them per transfer rather than holding them, so they\n\
         /// have to live somewhere the caller can reach.\n\
         pub fn init<'d>(\n\
         {params}) -> (SpiDma<'d, Blocking>, DmaRxBuf, DmaTxBuf) {{\n\
         \x20   let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) =\n\
         \x20       dma_buffers!(BUFFER_BYTES);\n\
         \x20   let dma_rx = DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();\n\
         \x20   let dma_tx = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();\n\
         \x20   let spi = Spi::new(spi, MODE)\n\
         {chain}\x20       .with_dma(dma);\n\
         \x20   (spi, dma_rx, dma_tx)\n\
         }}\n"
    );

    let example = example_block(
        &format!("Using SPI{n} (slave)"),
        &[
            "The master drives it. `transfer` takes BOTH buffers and hands them".to_owned(),
            "back with the driver when the master has clocked the bytes through:".to_owned(),
            String::new(),
            format!("    let (spi, rx, mut tx) = _spi{n};   // as generated above"),
            "    tx.as_mut_slice().fill(0xA5);".to_owned(),
            "    let transfer = spi.transfer(8, rx, 8, tx).unwrap();".to_owned(),
            "    let (spi, (rx, tx)) = transfer.wait();".to_owned(),
            String::new(),
            "// Nothing happens at all until the master asserts CS.".to_owned(),
        ],
    );

    let _ = rt; // the slave driver has no async twin - see the note above.
    file(
        "use esp_hal::Blocking;\n\
         use esp_hal::dma::{DmaChannelFor, DmaRxBuf, DmaTxBuf};\n\
         use esp_hal::dma_buffers;\n\
         use esp_hal::gpio::interconnect::{PeripheralInput, PeripheralOutput};\n\
         use esp_hal::spi::Mode;\n\
         use esp_hal::spi::slave::dma::SpiDma;\n\
         use esp_hal::spi::slave::{AnySpi, Instance, Spi};\n",
        &consts,
        &body,
        &example,
    )
}

// ── SPI ──────────────────────────────────────────────────────────────────────

fn spi_file(n: u8, sigs: &[&str], cfg: Option<&SpiModuleConfig>, rt: EspRuntime) -> String {
    if cfg.is_some_and(|c| c.role.is_slave()) {
        return spi_slave_file(n, sigs, cfg, rt);
    }
    let mut consts = format!(
        "const FREQUENCY_HZ: u32 = {};\nconst MODE: Mode = Mode::_{}; // CPOL/CPHA, 0..=3\n",
        cfg.map_or(1_000_000, |c| c.clock_hz),
        cfg.map_or(0, |c| c.mode).min(3),
    );
    let params = format!(
        "    spi: impl Instance + 'd,\n{}",
        params_for(
            sigs,
            &[
                ("sck", "impl PeripheralOutput<'d>"),
                ("mosi", "impl PeripheralOutput<'d>"),
                ("miso", "impl PeripheralInput<'d>"),
                ("cs", "impl PeripheralOutput<'d>"),
            ],
        )
    );
    let chain = chain_for(
        sigs,
        &[
            ("sck", "with_sck"),
            ("mosi", "with_mosi"),
            ("miso", "with_miso"),
            ("cs", "with_cs"),
        ],
    );
    // Only meaningful on the DMA path, and only emitted there: an unused
    // constant in a generated file is a warning in the user's project.
    if rt == EspRuntime::Async && cfg.is_some_and(|c| c.async_mode == AsyncBusMode::AsyncDma) {
        consts.push_str(
            "const DMA_BUFFER_BYTES: usize = 4096; // per direction, static-backed
",
        );
    }
    let ctor = "    let config = Config::default()\n\
                \x20       .with_frequency(Rate::from_hz(FREQUENCY_HZ))\n\
                \x20       .with_mode(MODE);\n\
                \x20   Spi::new(spi, config)\n\
                \x20       .unwrap()\n";
    let mut body = format!(
        "/// SPI{n} master — blocking driver.\n\
         ///\n\
         /// Takes exactly the lines wired on the Pins canvas: no MISO wired means\n\
         /// no `miso` parameter here.\n\
         pub fn init<'d>(\n\
         {params}) -> Spi<'d, Blocking> {{\n\
         {ctor}\
         {chain}}}\n"
    );
    let on_dma =
        rt == EspRuntime::Async && cfg.is_some_and(|c| c.async_mode == AsyncBusMode::AsyncDma);
    if rt == EspRuntime::Async {
        if on_dma {
            body.push_str(&spi_dma_twin(n, &params, ctor, &chain));
        } else {
            body.push_str(&async_twin(
                &format!("SPI{n} master — async driver."),
                &params,
                "Spi",
                ctor,
                &chain,
            ));
        }
    }
    let example = example_for(
        &format!("Using SPI{n}"),
        &format!("_spi{n}"),
        &[
            "In main.rs, after the init above:",
            "",
            "    // Write only",
            "    {H}.write(&[0x9F]).ok();",
            "",
            "    // Read only",
            "    let mut rx = [0u8; 3];",
            "    {H}.read(&mut rx).ok();",
            "",
            "    // Full duplex: `buf` is sent, and is overwritten by what comes back",
            "    let mut buf = [0x9F, 0x00, 0x00, 0x00];",
            "    {H}.transfer(&mut buf).ok();",
            "",
            "    // CS is NOT toggled for you unless it was wired on the canvas —",
            "    // drive it yourself around a transaction if you kept it as a GPIO.",
        ],
        &[
            "main.rs calls `init_async`, so the handle is a `Spi<'_, Async>`.",
            "esp-hal's async SPI surface is narrower than the blocking one: it is",
            "full-duplex in place, plus a flush. Use `init` for write-only/read-only.",
            "",
            "    // `buf` is sent, then overwritten by the reply",
            "    let mut buf = [0x9F, 0x00, 0x00, 0x00];",
            "    {H}.transfer_in_place_async(&mut buf).await.ok();",
            "    {H}.flush_async().await.ok();",
            "",
            "    // CS is NOT toggled for you unless it was wired on the canvas.",
        ],
        rt,
    );
    file(
        &format!(
            "{}\
             {}\
             use esp_hal::gpio::interconnect::{{PeripheralInput, PeripheralOutput}};\n\
             use esp_hal::spi::Mode;\n\
             use esp_hal::spi::master::{{Config, Instance, Spi}};\n\
             use esp_hal::time::Rate;\n",
            mode_import(rt),
            if on_dma {
                "use esp_hal::dma::{DmaChannelFor, DmaRxBuf, DmaTxBuf};\n\
                 use esp_hal::dma_buffers;\n\
                 use esp_hal::spi::master::{AnySpi, SpiDmaBus};\n"
            } else {
                ""
            },
        ),
        &consts,
        &body,
        &example,
    )
}

/// The DMA twin of `init_async`: the same bus, moved by the GDMA instead of by
/// the CPU.
///
/// # Why this is a separate shape and not a flag on [`async_twin`]
///
/// `.into_async()` alone gives an `Spi<'_, Async>` that still copies every byte
/// through the CPU. Going to DMA changes the RETURN TYPE — `SpiDmaBus` — and
/// needs two owned descriptor buffers built before the bus exists, so there is
/// no line to append: the whole body differs.
///
/// The buffers are `static`-backed by `dma_buffers!`, which is why the size is a
/// constant here rather than a parameter: they must outlive the transfer, and a
/// caller-supplied slice could not.
fn spi_dma_twin(n: u8, params: &str, ctor: &str, chain: &str) -> String {
    format!(
        "\n\
         /// SPI{n} master — async driver on DMA.\n\
         ///\n\
         /// Same construction as `init`, then `.with_dma()` and a pair of DMA\n\
         /// buffers. The GDMA moves the bytes, so a transfer costs the CPU one\n\
         /// `.await` rather than one interrupt per word.\n\
         ///\n\
         /// The channel comes from main.rs. On this chip any free channel serves\n\
         /// any peripheral, so which one you get is the IDE's choice — see the\n\
         /// DMA card in the Configuration tab, or pin one by hand in the SPI\n\
         /// module.\n\
         pub fn init_async<'d>(\n\
         {params}\x20   dma: impl DmaChannelFor<AnySpi<'d>>,\n\
         ) -> SpiDmaBus<'d, Async> {{\n\
         \x20   let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) =\n\
         \x20       dma_buffers!(DMA_BUFFER_BYTES);\n\
         \x20   let dma_rx = DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();\n\
         \x20   let dma_tx = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();\n\
         {ctor}\
         {chain}\x20       .with_dma(dma)\n\
         \x20       .with_buffers(dma_rx, dma_tx)\n\
         \x20       .into_async()\n\
         }}\n"
    )
}

// ── I2S ──────────────────────────────────────────────────────────────────────

/// esp-hal's `Config` constructor for the standard the module asked for.
///
/// `LsbFirst` has no constructor at all: esp-hal builds Philips, MSB-first and
/// the two PCM sync widths, and nothing else. The UI does not offer it on an
/// ESP — see `I2sStandard::options` — so reaching the fallback here means the
/// project carries a setting made for another chip, and Philips is the one to
/// land on rather than silently emitting something that will not compile.
fn i2s_standard_ctor(std: I2sStandard) -> &'static str {
    match std {
        I2sStandard::Philips | I2sStandard::LsbFirst => "new_tdm_philips",
        I2sStandard::MsbFirst => "new_tdm_msb",
        I2sStandard::PcmShortSync => "new_tdm_pcm_short",
        I2sStandard::PcmLongSync => "new_tdm_pcm_long",
    }
}

/// The `DataFormat` variant, for the two widths esp-hal and the IDE both have.
///
/// esp-hal's list is `Data{8,16,32}Channel{8,16,24,32}`-ish and the IDE's is the
/// STM32's four; the overlap is 16-in-16 and 32-in-32. The other two are not
/// offered on an ESP, so this fallback is the same "setting from another chip"
/// case as [`i2s_standard_ctor`].
fn i2s_data_format(fmt: I2sFormat) -> &'static str {
    match fmt {
        I2sFormat::Data32Channel32 | I2sFormat::Data24Channel32 => "Data32Channel32",
        _ => "Data16Channel16",
    }
}

/// One I2S instance as an esp-hal driver, in the direction the module asked for.
///
/// # It is a DMA peripheral, and only a DMA peripheral
///
/// Unlike the UART/SPI/I2C files beside it, there is no non-DMA form to fall
/// back to: `I2s::new` TAKES a channel. So this file always needs one, and
/// `main.rs` always passes one — a project with no channel left does not get a
/// half-working I2S, it gets none (see `dma_plan`).
///
/// # Why `init` hands back the buffer
///
/// `dma_buffers!` makes a `&'static mut [u8]` and a descriptor list. The
/// descriptors go into `.build()` and disappear into the driver; the BUFFER is
/// the one the caller passes to `write_dma_circular` on every transfer, so it
/// has to come back out. Returning it is what keeps the whole allocation in one
/// place instead of asking the user to declare a matching `static` by hand.
fn i2s_file(
    n: u8,
    sigs: &[&str],
    cfg: Option<&I2sModuleConfig>,
    rt: EspRuntime,
    // The ESP32 alone restricts MCLK to three pads, and says so in the TYPE:
    // its `with_mclk` takes `impl ClkPin`, not `impl PeripheralOutput`. The
    // wrong bound is a trait error in the USER's project, not here - which is
    // how it survived until a DAC pushed the harness onto an Xtensa part.
    esp32_mclk: bool,
) -> String {
    let tx = cfg.is_none_or(|c| c.direction == I2sDirection::Transmit);
    let (unit, data_method, data_bound) = if tx {
        ("i2s_tx", "with_dout", "impl PeripheralOutput<'d>")
    } else {
        ("i2s_rx", "with_din", "impl PeripheralInput<'d>")
    };
    let ty = if tx { "I2sTx" } else { "I2sRx" };
    let samples = cfg.map_or(256, |c| c.buffer_len) as usize;
    let consts = format!(
        "const SAMPLE_RATE_HZ: u32 = {};\n\
         // Samples are 32-bit at the widest, and the ring holds both channels.\n\
         const BUFFER_BYTES: usize = {} * 4 * 2;\n",
        cfg.map_or(48_000, |c| c.sample_rate_hz),
        samples,
    );
    // The signal names the canvas uses are the STM32's (ck/ws/sd/mck); esp-hal
    // calls the first two bclk/ws and the third dout or din by direction.
    let params = format!(
        "    i2s: impl Instance + 'd,\n\
         \x20   dma: impl DmaChannelFor<AnyI2s<'d>>,\n\
         {}",
        params_for(
            sigs,
            &[
                ("ck", "impl PeripheralOutput<'d>"),
                ("ws", "impl PeripheralOutput<'d>"),
                ("sd", data_bound),
                (
                    "mck",
                    if esp32_mclk {
                        "impl ClkPin<'d>"
                    } else {
                        "impl PeripheralOutput<'d>"
                    },
                ),
            ],
        )
    );
    let chain = chain_for(
        sigs,
        &[("ck", "with_bclk"), ("ws", "with_ws"), ("sd", data_method)],
    );
    // MCLK is set on the I2S itself, before it is split into its two halves —
    // it belongs to the block, not to a direction.
    let mclk = if sigs.contains(&"mck") {
        "\n\x20       .with_mclk(mck)"
    } else {
        ""
    };
    let (buffers, descriptors) = if tx {
        ("(_, _, buffer, descriptors)", "0, BUFFER_BYTES")
    } else {
        ("(buffer, descriptors, _, _)", "BUFFER_BYTES, 0")
    };
    let body_for = |name: &str, mode: &str, extra: &str| {
        format!(
            "pub fn {name}<'d>(\n\
             {params}) -> ({ty}<'d, {mode}>, &'static mut [u8]) {{\n\
             \x20   let {buffers} = dma_buffers!({descriptors});\n\
             \x20   let config = Config::{ctor}()\n\
             \x20       .with_sample_rate(Rate::from_hz(SAMPLE_RATE_HZ))\n\
             \x20       .with_data_format(DataFormat::{fmt})\n\
             \x20       .with_channels(Channels::STEREO);\n\
             \x20   let i2s = I2s::new(i2s, dma, config)\n\
             \x20       .unwrap(){mclk}{extra};\n\
             \x20   let driver = i2s\n\
             \x20       .{unit}\n\
             {chain}\x20       .build(descriptors);\n\
             \x20   (driver, buffer)\n\
             }}\n",
            ctor = i2s_standard_ctor(cfg.map_or(I2sStandard::Philips, |c| c.standard)),
            fmt = i2s_data_format(cfg.map_or(I2sFormat::Data16Channel16, |c| c.format)),
        )
    };
    let mut body = format!(
        "/// I2S{n} — blocking driver, {} on DMA.\n\
         ///\n\
         /// Returns the driver AND the ring buffer it moves: the descriptors go\n\
         /// into the driver, the buffer is what every transfer reads or writes.\n\
         {}",
        if tx { "transmitting" } else { "receiving" },
        body_for("init", "Blocking", ""),
    );
    if rt == EspRuntime::Async {
        body.push_str(&format!(
            "\n/// I2S{n} — async driver.\n\
             ///\n\
             /// Same construction, then `.into_async()`: the transfer methods\n\
             /// become `*_async` and `.await`-able on the executor.\n\
             {}",
            body_for("init_async", "Async", "\n\x20       .into_async()"),
        ));
    }
    let example = example_for(
        &format!("Using I2S{n}"),
        &format!("_i2s{n}"),
        &[
            "main.rs hands back the driver and its ring buffer:",
            "",
            "    let (mut {H}, buf) = …;   // as generated above",
            if tx {
                "    let mut transfer = {H}.write_dma_circular(buf).unwrap();"
            } else {
                "    let mut transfer = {H}.read_dma_circular(buf).unwrap();"
            },
            "    loop {",
            "        let n = transfer.available().unwrap();",
            if tx {
                "        if n > 0 { transfer.push(&samples[..n]).ok(); }"
            } else {
                "        if n > 0 { transfer.pop(&mut samples[..n]).ok(); }"
            },
            "    }",
        ],
        &[
            "main.rs calls `init_async`, so the transfer is .await-able:",
            "",
            "    let (mut {H}, buf) = …;   // as generated above",
            if tx {
                "    let mut transfer = {H}.write_dma_circular_async(buf).unwrap();"
            } else {
                "    let mut transfer = {H}.read_dma_circular_async(buf).unwrap();"
            },
            "    transfer.available().await.ok();",
            "",
            "    // `init` is still there if you want this one blocking instead.",
        ],
        rt,
    );
    file(
        &format!(
            "{}\
             use esp_hal::dma::DmaChannelFor;\n\
             use esp_hal::dma_buffers;\n\
             use esp_hal::gpio::interconnect::{{{pads}}};\n\
             use esp_hal::i2s::AnyI2s;\n\
             use esp_hal::i2s::master::{{Channels, {clk_pin}Config, DataFormat, I2s, {ty}, Instance}};\n\
             use esp_hal::time::Rate;\n",
            mode_import(rt),
            // Imported only where it is both needed and used.
            clk_pin = if esp32_mclk && sigs.contains(&"mck") {
                "ClkPin, "
            } else {
                ""
            },
            // Only the direction actually used: an unused import is a
            // warning in the user's project, and a generated file must
            // not raise one.
            pads = if tx {
                "PeripheralOutput"
            } else {
                "PeripheralInput, PeripheralOutput"
            },
        ),
        &consts,
        &body,
        &example,
    )
}

// ── DAC ─────────────────────────────────────────────────────────────────────

/// The chip's DAC channels — an 8-bit level per pad.
///
/// # Two channels, two peripherals
///
/// esp-hal names them `DAC1` and `DAC2`, each with a `T::Pin` fixed by type:
/// `GPIO25`/`GPIO26` on the ESP32, `GPIO17`/`GPIO18` on the S2. So `init` takes
/// the pad, but passing a different one does not compile — which is the whole
/// reason the canvas offers `DacOut` on those two pads and nowhere else.
///
/// # Eight bits, not twelve
///
/// `Dac::write` takes a `u8`. The module stores its resting value the way the
/// STM32's DAC needs it — twelve bits — so it is scaled here rather than
/// clamped, or every value above 255 would land at full scale.
fn dac_file(channels: &[u8], cfg: Option<&DacModuleConfig>, rt: EspRuntime) -> String {
    let d = DacModuleConfig::new(1);
    let c = cfg.unwrap_or(&d);
    let mut consts = String::new();
    for ch in channels {
        consts.push_str(&format!(
            "// The level the pad holds once `init` returns.\n\
             const START_OUT{ch}: u8 = {};\n",
            esp_dac_value(c.value_of(*ch)),
        ));
    }

    let params: String = channels
        .iter()
        .map(|ch| {
            format!("    dac{ch}: DAC{ch}<'d>,\n    pin{ch}: <DAC{ch}<'d> as Instance>::Pin,\n")
        })
        .collect();
    let ret: String = channels
        .iter()
        .map(|ch| format!("Dac<'d, DAC{ch}<'d>>"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut steps = String::new();
    for ch in channels {
        steps.push_str(&format!(
            "\x20   let mut out{ch} = Dac::new(dac{ch}, pin{ch});\n\
             \x20   out{ch}.write(START_OUT{ch});\n"
        ));
    }
    let handles = channels
        .iter()
        .map(|ch| format!("out{ch}"))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = if channels.len() == 1 {
        ret
    } else {
        format!("({ret})")
    };
    let handles = if channels.len() == 1 {
        handles
    } else {
        format!("({handles})")
    };

    let body = format!(
        "/// The DAC — {} channel{}, eight bits each.\n\
         ///\n\
         /// Each pad is fixed in silicon; `Dac::new` takes no other.\n\
         pub fn init<'d>(\n\
         {params}) -> {ret} {{\n\
         {steps}\x20   {handles}\n\
         }}\n",
        channels.len(),
        if channels.len() == 1 { "" } else { "s" },
    );

    let first = channels.first().copied().unwrap_or(1);
    let example = example_block(
        "Using the DAC",
        &[
            "One byte in, one voltage out — 0 is ground, 255 is the supply:".to_owned(),
            String::new(),
            format!("    _dac_out{first}.write(128);   // half scale"),
            String::new(),
            "// There is no ramp and no buffer: the pad follows the last value".to_owned(),
            "// written, and holds it.".to_owned(),
        ],
    );

    let _ = rt; // the DAC has no async surface: `write` is a register store.
    file(
        &format!(
            "use esp_hal::analog::dac::{{Dac, Instance}};\n\
             use esp_hal::peripherals::{{{}}};\n",
            channels
                .iter()
                .map(|ch| format!("DAC{ch}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        &consts,
        &body,
        &example,
    )
}

/// The module's 12-bit resting value, as the eight bits an ESP DAC takes.
///
/// Scaled, not truncated: the field is shared with the STM32 path, where the
/// converter really is twelve bits, so a mid-scale 2048 has to come out as 128
/// rather than as the low byte of 2048 (which is 0).
fn esp_dac_value(v12: u16) -> u8 {
    (u32::from(v12.min(4095)) * 255 / 4095) as u8
}

// ── PARL_IO ─────────────────────────────────────────────────────────────────

/// The chip's parallel port, in the direction and width the module asked for.
///
/// # DMA only, like the I2S
///
/// `ParlIo::new` TAKES a channel: there is no other constructor, so a port with
/// no channel left is not generated at all rather than generated half-working.
///
/// # The buffer comes back
///
/// Same reason as the I2S: `dma_buffers!` makes the descriptors that disappear
/// into the driver and the BUFFER that every transfer reads or writes, so the
/// second has to be returned.
///
/// # The valid line
///
/// Espressif puts it on the sixteenth data line when the bus is sixteen wide,
/// and on a pad of its own when it is narrower — esp-hal says so by
/// implementing `NotContainsValidSignalPin` for every width but the widest.
/// So a wired VALID pad is used below 16 bits and ignored at 16, which the
/// module states rather than leaving to be discovered.
fn parl_io_file(cfg: Option<&ParlIoModuleConfig>, has_valid: bool, rt: EspRuntime) -> String {
    let d = ParlIoModuleConfig::new(0);
    let c = cfg.unwrap_or(&d);
    let tx = c.direction.is_tx();
    let lanes = c.width.lanes();
    // At sixteen bits the valid signal IS one of the data lines.
    let valid = has_valid && lanes < 16;

    let consts = format!(
        "const FREQUENCY_HZ: u32 = {};\n\
         const BUFFER_BYTES: usize = {};\n",
        c.freq_hz, c.buffer_bytes,
    );

    let pad_bound = if tx {
        "impl PeripheralOutput<'d>"
    } else {
        "impl PeripheralInput<'d>"
    };
    let mut params =
        format!("    parl_io: PARL_IO<'d>,\n    dma: impl DmaChannelFor<PARL_IO<'d>>,\n");
    for lane in 0..lanes {
        params.push_str(&format!("    d{lane}: {pad_bound},\n"));
    }
    params.push_str(&format!(
        "    clk: impl Peripheral{}<'d>,\n",
        if tx { "Output" } else { "Input" }
    ));
    if valid {
        params.push_str("    valid: impl PeripheralOutput<'d>,\n");
    }

    let data_args: Vec<String> = (0..lanes).map(|l| format!("d{l}")).collect();
    let pins_ty = c.width.esp_hal(tx);
    let mut build = format!(
        "\x20   let pins = {pins_ty}::new({});\n",
        data_args.join(", ")
    );
    if valid {
        build.push_str("\x20   let pins = TxPinConfigWithValidPin::new(pins, valid);\n");
    }
    build.push_str(&format!(
        "\x20   let clk_pin = Clk{}Pin::new(clk);\n",
        if tx { "Out" } else { "In" }
    ));

    let (buffers, sizes, buf_ty, half) = if tx {
        (
            "(_, _, buffer, descriptors)",
            "0, BUFFER_BYTES",
            "DmaTxBuf",
            "tx",
        )
    } else {
        (
            "(buffer, descriptors, _, _)",
            "BUFFER_BYTES, 0",
            "DmaRxBuf",
            "rx",
        )
    };
    let cfg_ty = if tx { "TxConfig" } else { "RxConfig" };
    let driver = if tx { "ParlIoTx" } else { "ParlIoRx" };
    // The receiver needs a timeout or a frame never ends; the transmitter has
    // no such knob.
    let extra_cfg = if tx {
        String::new()
    } else {
        "\n\x20       .with_timeout_ticks(0xfff)".to_owned()
    };

    let body_for = |name: &str, mode: &str, into_async: &str| {
        format!(
            "pub fn {name}<'d>(\n\
             {params}) -> ({driver}<'d, {mode}>, {buf_ty}) {{\n\
             \x20   let {buffers} = dma_buffers!({sizes});\n\
             \x20   let dma_buf = {buf_ty}::new(descriptors, buffer).unwrap();\n\
             {build}\
             \x20   let config = {cfg_ty}::default()\n\
             \x20       .with_frequency(Rate::from_hz(FREQUENCY_HZ))\n\
             \x20       .with_bit_order(BitPackOrder::{order})\
             {extra_cfg};\n\
             \x20   let port = ParlIo::new(parl_io, dma).unwrap(){into_async};\n\
             \x20   let driver = port.{half}.with_config(pins, clk_pin, config).unwrap();\n\
             \x20   (driver, dma_buf)\n\
             }}\n",
            order = c.bit_order.esp_hal(),
        )
    };

    let mut body = format!(
        "/// The parallel port — {} {} lines at {} Hz.\n\
         ///\n\
         /// Returns the driver AND its DMA buffer: the descriptors go into the\n\
         /// driver, the buffer is what each transfer moves.\n\
         {}",
        if tx { "transmitting" } else { "receiving" },
        lanes,
        c.freq_hz,
        body_for("init", "Blocking", ""),
    );
    if rt == EspRuntime::Async {
        body.push_str(&format!(
            "\n/// The same, async.\n{}",
            body_for("init_async", "Async", ".into_async()"),
        ));
    }

    // Every call below is the REAL one: `write`/`read` take a length and the
    // buffer and hand back a transfer, which gives both back when it ends.
    // `wait_for_done` is the only async-specific method — the rest is shared.
    let example = example_for(
        "Using the parallel port",
        "_parl",
        &if tx {
            vec![
                "Fill the buffer, hand it over, and take it back when done:",
                "",
                "    let (mut port, mut buf) = …;   // as generated above",
                "    buf.as_mut_slice().fill(0xAA);",
                "    let transfer = port.write(buf.len(), buf).unwrap();",
                "    let (result, p, b) = transfer.wait();",
                "    (port, buf) = (p, b);",
                "    result.ok();",
            ]
        } else {
            vec![
                "Hand the buffer over and take it back full:",
                "",
                "    let (mut port, mut buf) = …;   // as generated above",
                "    let transfer = port.read(Some(buf.len()), buf).unwrap();",
                "    let (result, p, b) = transfer.wait();",
                "    (port, buf) = (p, b);",
                "    result.ok();",
            ]
        },
        &if tx {
            vec![
                "`init_async` gives the transfer a `.wait_for_done()` that yields",
                "instead of spinning; everything else is the same:",
                "",
                "    let (mut port, mut buf) = …;   // as generated above",
                "    buf.as_mut_slice().fill(0xAA);",
                "    let mut transfer = port.write(buf.len(), buf).unwrap();",
                "    transfer.wait_for_done().await;",
                "    let (_, p, b) = transfer.wait();",
                "    (port, buf) = (p, b);",
            ]
        } else {
            vec![
                "`init_async` gives the transfer a `.wait_for_done()` that yields",
                "instead of spinning; everything else is the same:",
                "",
                "    let (mut port, mut buf) = …;   // as generated above",
                "    let mut transfer = port.read(Some(buf.len()), buf).unwrap();",
                "    transfer.wait_for_done().await;",
                "    let (_, p, b) = transfer.wait();",
                "    (port, buf) = (p, b);",
            ]
        },
        rt,
    );

    let mut uses = format!(
        "{}\
         use esp_hal::dma::{{DmaChannelFor, {buf_ty}}};\n\
         use esp_hal::dma_buffers;\n",
        mode_import(rt),
    );
    uses.push_str(&format!(
        "use esp_hal::gpio::interconnect::{};\n",
        if tx {
            "PeripheralOutput"
        } else {
            "{PeripheralInput, PeripheralOutput}"
        }
    ));
    uses.push_str(&format!(
        "use esp_hal::parl_io::{{BitPackOrder, Clk{}Pin, ParlIo, {driver}, {cfg_ty}, {pins_ty}{}}};\n\
         use esp_hal::peripherals::PARL_IO;\n\
         use esp_hal::time::Rate;\n",
        if tx { "Out" } else { "In" },
        if valid { ", TxPinConfigWithValidPin" } else { "" },
    ));

    file(&uses, &consts, &body, &example)
}

// ── MCPWM ───────────────────────────────────────────────────────────────────

/// One MCPWM unit: its timer, and a `PwmPin` per wired output.
///
/// # Why the TIMER comes back too
///
/// `Timer` owns the `PwmClockGuard` that keeps the MCPWM function clock
/// running; `PwmPin` does not — it holds only a `PeripheralGuard`. An `init`
/// that returned the pins alone would compile, and the clock would be switched
/// off the moment it returned, leaving every output silent. So the timer is
/// handed back as the first element of the tuple and `main.rs` binds it.
///
/// # One frequency, one timer
///
/// Every wired operator is pointed at timer 0. The module carries one
/// frequency, so three timers would be three names for one number — and a
/// three-phase inverter, which is what this peripheral is for, wants exactly
/// one anyway.
///
/// # No async twin
///
/// There is no `into_async` on `McPwm`: the duty is set by writing a register,
/// and there is nothing to await. `init` is the whole surface on either runtime.
fn mcpwm_file(
    unit: u8,
    outputs: &[(u8, bool)],
    cfg: Option<&McpwmModuleConfig>,
    source_mhz: u32,
) -> String {
    let d = McpwmModuleConfig::new(unit);
    let c = cfg.unwrap_or(&d);
    let peri = format!("MCPWM{unit}");

    let mut consts = format!(
        "const FREQUENCY_HZ: u32 = {};\n\
         // The timer counts 0..=PERIOD, so a duty lands on one of PERIOD+1 steps. Public: main.rs sets duty in terms of it.\n\
         pub const PERIOD: u16 = {};\n",
        c.freq_hz, c.period,
    );
    for (op, b) in outputs {
        consts.push_str(&format!(
            "const TIMESTAMP_OP{op}{}: u16 = {}; // {:.2} %\n",
            if *b { "B" } else { "A" },
            c.timestamp_of(*op, *b),
            f64::from(c.duty_x100_of(*op, *b)) / 100.0,
        ));
    }

    let name = |op: u8, b: bool| format!("op{op}{}", if b { "b" } else { "a" });
    let params: String = outputs
        .iter()
        .map(|(op, b)| format!("    {}: impl PeripheralOutput<'d>,\n", name(*op, *b)))
        .collect();
    // The const parameter is IS_A, so it is the NEGATION of "this is the B
    // output" — inverting it silently swaps which pad each handle drives.
    let ret: String = outputs
        .iter()
        .map(|(op, b)| format!(", PwmPin<'d, {peri}<'d>, {op}, {}>", !b))
        .collect();

    // Each operator is pointed at timer 0 once, however many of its two
    // outputs are wired.
    let mut ops: Vec<u8> = outputs.iter().map(|(op, _)| *op).collect();
    ops.sort_unstable();
    ops.dedup();
    let links: String = ops
        .iter()
        .map(|op| format!("\x20   mcpwm.operator{op}.set_timer(&mcpwm.timer0);\n"))
        .collect();

    // `with_pin_a` CONSUMES the operator, so an operator whose two outputs are
    // both wired has to build them together — `with_pins` is the constructor
    // that exists for exactly that, and reaching for `with_pin_a` twice does
    // not compile.
    let mut pins = String::new();
    for op in &ops {
        let a = outputs.contains(&(*op, false));
        let b = outputs.contains(&(*op, true));
        match (a, b) {
            (true, true) => pins.push_str(&format!(
                "\x20   let (mut {a_n}, mut {b_n}) = mcpwm.operator{op}.with_pins(\n\
                 \x20       {a_n},\n\
                 \x20       PwmPinConfig::UP_ACTIVE_HIGH,\n\
                 \x20       {b_n},\n\
                 \x20       PwmPinConfig::UP_ACTIVE_HIGH,\n\
                 \x20   );\n",
                a_n = name(*op, false),
                b_n = name(*op, true),
            )),
            _ => {
                let is_b = b;
                pins.push_str(&format!(
                    "\x20   let mut {n} = mcpwm\n\
                     \x20       .operator{op}\n\
                     \x20       .with_pin_{ab}({n}, PwmPinConfig::UP_ACTIVE_HIGH);\n",
                    n = name(*op, is_b),
                    ab = if is_b { "b" } else { "a" },
                ));
            }
        }
    }
    let stamps: String = outputs
        .iter()
        .map(|(op, b)| {
            format!(
                "\x20   {}.set_timestamp(TIMESTAMP_OP{op}{});\n",
                name(*op, *b),
                if *b { "B" } else { "A" },
            )
        })
        .collect();
    let handles: String = outputs
        .iter()
        .map(|(op, b)| format!(", {}", name(*op, *b)))
        .collect();

    let body = format!(
        "/// MCPWM{unit} — motor-control PWM.\n\
         ///\n\
         /// Returns the TIMER as well as the pins, and `main.rs` keeps it: the\n\
         /// timer owns the guard that holds the MCPWM clock on. Drop it and every\n\
         /// output here goes quiet.\n\
         pub fn init<'d>(\n\
         \x20   mcpwm: {peri}<'d>,\n\
         {params}) -> (Timer<0, {peri}<'d>>{ret}) {{\n\
         \x20   let clock_cfg = PeripheralClockConfig::with_frequency(Rate::from_mhz({source_mhz}))\n\
         \x20       .unwrap();\n\
         \x20   let mut mcpwm = McPwm::new(mcpwm, clock_cfg);\n\
         {links}{pins}\n\
         \x20   let timer_cfg = clock_cfg\n\
         \x20       .timer_clock_with_frequency(\n\
         \x20           PERIOD,\n\
         \x20           PwmWorkingMode::Increase,\n\
         \x20           Rate::from_hz(FREQUENCY_HZ),\n\
         \x20       )\n\
         \x20       .unwrap();\n\
         \x20   mcpwm.timer0.start(timer_cfg);\n\
         {stamps}\n\
         \x20   (mcpwm.timer0{handles})\n\
         }}\n"
    );

    let first = outputs
        .first()
        .map(|(op, b)| name(*op, *b))
        .unwrap_or_else(|| "op0a".into());
    let example = example_block(
        &format!("Using MCPWM{unit}"),
        &[
            "The duty is a TIMESTAMP: the counter value the output flips at,".into(),
            "so it runs 0..=PERIOD rather than 0..=100.".into(),
            String::new(),
            format!("    // Half power, whatever PERIOD is set to"),
            format!("    _mcpwm{unit}_{first}.set_timestamp((PERIOD + 1) / 2);"),
            String::new(),
            "// Keep the timer binding alive for as long as you want output:".into(),
            "// it holds the clock on, and dropping it stops every pin above.".into(),
        ],
    );

    file(
        &format!(
            "use esp_hal::gpio::interconnect::PeripheralOutput;\n\
             use esp_hal::mcpwm::operator::{{PwmPin, PwmPinConfig}};\n\
             use esp_hal::mcpwm::timer::{{PwmWorkingMode, Timer}};\n\
             use esp_hal::mcpwm::{{McPwm, PeripheralClockConfig}};\n\
             use esp_hal::peripherals::{peri};\n\
             use esp_hal::time::Rate;\n"
        ),
        &consts,
        &body,
        &example,
    )
}

// ── USB Serial/JTAG ─────────────────────────────────────────────────────────

/// The USB Serial/JTAG peripheral: a CDC serial port over the chip's own USB.
///
/// # No pins, and no VID/PID either
///
/// The two pads are fixed in silicon, so `UsbSerialJtag::new` takes the
/// peripheral and nothing else — this is the one config file here whose `init`
/// has no parameters at all.
///
/// The descriptors are fixed too: the device always enumerates as Espressif's
/// `303a:1001`, which is why the module's Vendor ID, Product ID and product
/// string are not offered on an ESP. They belong to the `usb-device` stack the
/// STM32 path builds, where they really are a choice.
///
/// # It is not the flashing port
///
/// A board with a USB-UART bridge chip exposes that instead; this is the port
/// the chip provides itself, on parts that have one. Both can be present, and
/// they are different devices to the host.
fn usb_file(rt: EspRuntime) -> String {
    let body_for = |name: &str, mode: &str, extra: &str| {
        format!(
            "pub fn {name}<'d>(usb: USB_DEVICE<'d>) -> UsbSerialJtag<'d, {mode}> {{\n\
             \x20   UsbSerialJtag::new(usb){extra}\n\
             }}\n",
        )
    };
    let mut body = format!(
        "/// The chip's own USB CDC serial port — blocking driver.\n\
         ///\n\
         /// Enumerates as Espressif's `303a:1001`. On a board that also has a\n\
         /// USB-UART bridge, this is the SECOND serial port the host sees.\n\
         {}",
        body_for("init", "Blocking", ""),
    );
    if rt == EspRuntime::Async {
        body.push_str(&format!(
            "\n/// The same, async.\n\
             ///\n\
             /// `.into_async()` makes the reads and writes `.await`-able on the\n\
             /// executor instead of spinning.\n\
             {}",
            body_for("init_async", "Async", "\n\x20       .into_async()"),
        ));
    }

    let example = example_for(
        "Using the USB serial port",
        "_usb",
        &[
            "Writing works on the handle, which implements `core::fmt::Write`:",
            "",
            "    use core::fmt::Write;",
            "    writeln!({H}, \"hello over USB\").ok();",
            "",
            "Reading needs the two halves apart, and `split` CONSUMES the handle:",
            "",
            "    let (mut rx, mut tx) = {H}.split();",
            "    let mut buf = [0u8; 64];",
            "    let n = rx.drain_rx_fifo(&mut buf);   // non-blocking",
            "    tx.write(&buf[..n]).ok();             // echo it back",
            "",
            "// Nothing is sent until a host opens the port.",
        ],
        &[
            "`init_async` swaps in the interrupt-driven handler. The .await-able",
            "surface is the embedded-io-async traits, which this project does not",
            "depend on yet — add `embedded-io-async` to Cargo.toml to use them.",
            "The calls below work on either driver:",
            "",
            "    use core::fmt::Write;",
            "    writeln!({H}, \"hello over USB\").ok();",
            "",
            "    let (mut rx, mut tx) = {H}.split();",
            "    let mut buf = [0u8; 64];",
            "    let n = rx.drain_rx_fifo(&mut buf);",
            "    tx.write(&buf[..n]).ok();",
        ],
        rt,
    );

    file(
        &format!(
            "{}\
             use esp_hal::peripherals::USB_DEVICE;\n\
             use esp_hal::usb_serial_jtag::UsbSerialJtag;\n",
            mode_import(rt)
        ),
        // Nothing to configure: the peripheral has no settings the driver
        // exposes, so the generated block is empty rather than decorative.
        "",
        &body,
        &example,
    )
}

// ── PCNT ────────────────────────────────────────────────────────────────────

/// One PCNT unit: its limits, its filter, and what each edge means.
///
/// # It hands the unit back
///
/// Unlike the buses, there is no driver object to keep — the unit IS the
/// handle, and `main.rs` reads the count off it with `.counter.get()`. So
/// `init` takes the unit, configures it, and returns it.
///
/// # Only channel 0
///
/// A unit has two channels and wiring both is how a quadrature encoder is
/// counted four times per cycle instead of twice. The module wires one; the
/// second is a signal to add, not a limit of the hardware.
fn pcnt_file(n: u8, has_ctrl: bool, cfg: Option<&PcntModuleConfig>, rt: EspRuntime) -> String {
    let d = PcntModuleConfig::new(n);
    let c = cfg.unwrap_or(&d);
    let consts = format!(
        "const LOW_LIMIT: i16 = {};\n\
         const HIGH_LIMIT: i16 = {};\n\
         {}",
        c.low_limit,
        c.high_limit,
        if c.filter > 0 {
            format!(
                "// Pulses shorter than this many APB clocks are ignored.\n\
                 const FILTER: u16 = {};\n",
                c.filter,
            )
        } else {
            String::new()
        },
    );

    let ctrl_param = if has_ctrl {
        "\x20   ctrl: impl PeripheralInput<'d>,\n"
    } else {
        ""
    };
    let mut steps = String::from(
        "\x20   unit.set_low_limit(Some(LOW_LIMIT)).unwrap();\n\
         \x20   unit.set_high_limit(Some(HIGH_LIMIT)).unwrap();\n",
    );
    if c.filter > 0 {
        steps.push_str("\x20   unit.set_filter(Some(FILTER)).unwrap();\n");
    }
    steps.push_str(
        "\x20   unit.clear();\n\
         \n\
         \x20   let channel = &unit.channel0;\n\
         \x20   channel.set_edge_signal(edge);\n",
    );
    if has_ctrl {
        steps.push_str("\x20   channel.set_ctrl_signal(ctrl);\n");
    }
    // The vendor's own argument order, and it is NOT the obvious one:
    // `set_input_mode` takes the FALLING edge first.
    steps.push_str(&format!(
        "\x20   channel.set_input_mode(EdgeMode::{}, EdgeMode::{}); // (falling, rising)\n",
        c.neg_edge.esp_hal(),
        c.pos_edge.esp_hal(),
    ));
    if has_ctrl {
        steps.push_str(&format!(
            "\x20   channel.set_ctrl_mode(CtrlMode::{}, CtrlMode::{}); // (low, high)\n",
            c.ctrl_low.esp_hal(),
            c.ctrl_high.esp_hal(),
        ));
    }

    let body = format!(
        "/// PCNT unit {n} — a hardware pulse counter.\n\
         ///\n\
         /// `main.rs` builds the one `Pcnt` and lends this unit in; it comes back\n\
         /// configured, and the count is read off `.counter`.\n\
         pub fn init<'d, const U: usize>(\n\
         \x20   unit: Unit<'d, U>,\n\
         \x20   edge: impl PeripheralInput<'d>,\n\
         {ctrl_param}) -> Unit<'d, U> {{\n\
         {steps}\x20   unit\n\
         }}\n"
    );

    let example = example_for(
        &format!("Using PCNT unit {n}"),
        &format!("_pcnt{n}"),
        &[
            "The counter runs on its own; read it whenever you like:",
            "",
            "    let count = {H}.counter.get();",
            "",
            "    // Start again from zero - `clear` is on the UNIT",
            "    {H}.clear();",
            "",
            "// Reaching a limit CLEARS the counter and raises an event, so a",
            "// total wider than 16 bits is accumulated by listening for it.",
        ],
        &[
            "The counter runs on its own; read it between .awaits:",
            "",
            "    let count = {H}.counter.get();",
            "    {H}.clear();",
        ],
        rt,
    );

    file(
        &format!(
            "use esp_hal::gpio::interconnect::PeripheralInput;\n\
             use esp_hal::pcnt::channel::{{{}}};\n\
             use esp_hal::pcnt::unit::Unit;\n",
            if has_ctrl {
                "CtrlMode, EdgeMode"
            } else {
                "EdgeMode"
            },
        ),
        &consts,
        &body,
        &example,
    )
}

// ── RMT ─────────────────────────────────────────────────────────────────────

/// One RMT channel, in the direction that channel has.
///
/// # Not a bus, so not shaped like one
///
/// The other files here build a peripheral that moves BYTES. An RMT channel
/// moves EDGES: you hand it `(level, ticks)` pairs and it clocks each one out.
/// So there is no baud rate and no data format — the two numbers that matter
/// are the tick length (the clock divider) and, if the far end is an IR
/// receiver, the carrier to modulate onto.
///
/// # The channel arrives from main.rs
///
/// `Rmt::new` takes the whole block and hands back a struct whose channels are
/// fields. `main.rs` builds it once and lends `rmt.channel0` here, the same way
/// it lends a LEDC timer to a PWM channel — which is why `init` takes a
/// `TxChannelCreator` rather than a `peripherals.*` singleton.
fn rmt_file(n: u8, cfg: Option<&RmtModuleConfig>, rt: EspRuntime, source_hz: u32) -> String {
    let tx = cfg.is_none_or(|c| c.direction.is_tx());
    let divider = cfg.map_or(1, |c| c.clk_divider).max(1);
    let carrier = cfg.is_some_and(|c| c.carrier);
    let carrier_hz = cfg.map_or(38_000, |c| c.carrier_hz).max(1);
    // The carrier is counted in the channel's own ticks, so the divider is
    // already in play. Split evenly: a 50% duty is what an IR receiver expects.
    let period = (source_hz / u32::from(divider) / carrier_hz).max(2);
    let (high, low) = (period / 2, period - period / 2);

    let mut consts = format!(
        "const CLK_DIVIDER: u8 = {divider}; // 1 tick = {} ns\n",
        (1_000_000_000f64 / (source_hz as f64 / f64::from(divider))).round() as u64,
    );
    if tx {
        consts.push_str(&format!(
            "const IDLE_HIGH: bool = {}; // where the pad rests between trains\n",
            cfg.is_some_and(|c| c.idle_high),
        ));
    } else {
        consts.push_str(&format!(
            "const IDLE_THRESHOLD: u16 = {}; // ticks of silence that end a frame\n",
            cfg.map_or(10_000, |c| c.idle_threshold),
        ));
    }
    if carrier {
        consts.push_str(&format!(
            "// Carrier {carrier_hz} Hz at 50%: source / CLK_DIVIDER / carrier, split in two.\n\
             const CARRIER_HIGH: u16 = {high};\n\
             const CARRIER_LOW: u16 = {low};\n",
        ));
    }

    let (ty, creator, ctor, pad_bound, pad_use) = if tx {
        (
            "Tx",
            "TxChannelCreator",
            "configure_tx",
            "impl PeripheralOutput<'d>",
            "PeripheralOutput",
        )
    } else {
        (
            "Rx",
            "RxChannelCreator",
            "configure_rx",
            "impl PeripheralInput<'d>",
            "PeripheralInput",
        )
    };
    let cfg_ty = if tx {
        "TxChannelConfig"
    } else {
        "RxChannelConfig"
    };
    let mut chain = String::from("\x20       .with_clk_divider(CLK_DIVIDER)\n");
    if tx {
        chain.push_str(
            "\x20       .with_idle_output(true)\n\
             \x20       .with_idle_output_level(if IDLE_HIGH { Level::High } else { Level::Low })\n",
        );
    } else {
        chain.push_str("\x20       .with_idle_threshold(IDLE_THRESHOLD)\n");
    }
    chain.push_str(&if carrier {
        "\x20       .with_carrier_modulation(true)\n\
         \x20       .with_carrier_high(CARRIER_HIGH)\n\
         \x20       .with_carrier_low(CARRIER_LOW)\n\
         \x20       .with_carrier_level(Level::High)\n"
            .to_owned()
    } else {
        "\x20       .with_carrier_modulation(false)\n".to_owned()
    });

    // The `;` belongs to the last builder call, not to a line of its own.
    let chain = chain.trim_end_matches('\n');

    let body_for = |name: &str, mode: &str| {
        format!(
            "pub fn {name}<'d>(\n\
             \x20   channel: impl {creator}<'d, {mode}>,\n\
             \x20   line: {pad_bound},\n\
             ) -> Channel<'d, {mode}, {ty}> {{\n\
             \x20   let config = {cfg_ty}::default()\n\
             {chain};\n\
             \x20   channel.{ctor}(&config).unwrap().with_pin(line)\n\
             }}\n"
        )
    };
    let mut body = format!(
        "/// RMT channel {n} — {}, blocking driver.\n\
         ///\n\
         /// `main.rs` builds the one `Rmt` and lends this channel in.\n\
         {}",
        if tx { "transmitting" } else { "receiving" },
        body_for("init", "Blocking"),
    );
    if rt == EspRuntime::Async {
        body.push_str(&format!(
            "\n/// RMT channel {n} — async driver.\n\
             ///\n\
             /// Identical, but built from the `Rmt` main.rs turned async: the\n\
             /// transmit and receive calls become `.await`-able.\n\
             {}",
            body_for("init_async", "Async"),
        ));
    }

    let example = example_for(
        &format!("Using RMT channel {n}"),
        &format!("_rmt{n}"),
        &if tx {
            vec![
                "A pulse train is a list of (level, ticks) pairs, ended by a zero:",
                "",
                "    use esp_hal::rmt::{PulseCode, TxChannel};",
                "    let data = [",
                "        PulseCode::new(Level::High, 200, Level::Low, 100),",
                "        PulseCode::empty(),   // the terminator",
                "    ];",
                "    let tx = {H}.transmit(&data).unwrap();",
                "    {H} = tx.wait().unwrap();",
            ]
        } else {
            vec![
                "Receiving fills a buffer with the pairs the line carried:",
                "",
                "    use esp_hal::rmt::RxChannel;",
                "    let mut data = [esp_hal::rmt::PulseCode::empty(); 48];",
                "    let rx = {H}.receive(&mut data).unwrap();",
                "    {H} = rx.wait().unwrap();",
            ]
        },
        &if tx {
            vec![
                "main.rs calls `init_async`, so the transmit is .await-able:",
                "",
                "    use esp_hal::rmt::{PulseCode, TxChannelAsync};",
                "    let data = [",
                "        PulseCode::new(Level::High, 200, Level::Low, 100),",
                "        PulseCode::empty(),",
                "    ];",
                "    {H}.transmit(&data).await.unwrap();",
            ]
        } else {
            vec![
                "main.rs calls `init_async`, so the receive is .await-able:",
                "",
                "    use esp_hal::rmt::RxChannelAsync;",
                "    let mut data = [esp_hal::rmt::PulseCode::empty(); 48];",
                "    {H}.receive(&mut data).await.unwrap();",
            ]
        },
        rt,
    );

    file(
        &format!(
            "{}\
             {level_use}\
             use esp_hal::gpio::interconnect::{pad_use};\n\
             use esp_hal::rmt::{{Channel, {ty}, {cfg_ty}, {creator}}};\n",
            mode_import(rt),
            // `Level` names the idle output and the carrier level, and a
            // receive channel with no carrier uses neither — importing it
            // there is an unused-import warning in the user's project.
            level_use = if tx || carrier {
                "use esp_hal::gpio::Level;
"
            } else {
                ""
            },
        ),
        &consts,
        &body,
        &example,
    )
}

// ── I2C ──────────────────────────────────────────────────────────────────────

fn i2c_file(n: u8, sigs: &[&str], cfg: Option<&I2cModuleConfig>, rt: EspRuntime) -> String {
    let consts = format!(
        "const FREQUENCY_HZ: u32 = {};\n\
         // 7-bit address of the device on this bus — for YOUR code, not for `init`:\n\
         // an esp-hal I2C master takes the address per transaction.\n\
         pub const DEVICE_ADDRESS: u8 = 0x{:02X};\n",
        cfg.map_or(100_000, |c| c.clock_hz),
        cfg.map_or(0, |c| c.address),
    );
    let bound = "impl PeripheralInput<'d> + PeripheralOutput<'d>";
    let params = format!(
        "    i2c: impl Instance + 'd,\n{}",
        params_for(sigs, &[("scl", bound), ("sda", bound)])
    );
    let chain = chain_for(sigs, &[("scl", "with_scl"), ("sda", "with_sda")]);
    let ctor = "    let config = Config::default().with_frequency(Rate::from_hz(FREQUENCY_HZ));\n\
                \x20   I2c::new(i2c, config)\n\
                \x20       .unwrap()\n";
    let mut body = format!(
        "/// I2C{n} master — blocking driver.\n\
         pub fn init<'d>(\n\
         {params}) -> I2c<'d, Blocking> {{\n\
         {ctor}\
         {chain}}}\n"
    );
    if rt == EspRuntime::Async {
        body.push_str(&async_twin(
            &format!("I2C{n} master — async driver."),
            &params,
            "I2c",
            ctor,
            &chain,
        ));
    }
    let example = example_for(
        &format!("Using I2C{n}"),
        &format!("_i2c{n}"),
        &[
            "In main.rs, after the init above:",
            "",
            "    use pins::configs::i2c{N}::DEVICE_ADDRESS;",
            "",
            "    // Write to a register",
            "    {H}.write(DEVICE_ADDRESS, &[0x10, 0x42]).ok();",
            "",
            "    // Read bytes",
            "    let mut rx = [0u8; 2];",
            "    {H}.read(DEVICE_ADDRESS, &mut rx).ok();",
            "",
            "    // Register read: write the address, then read WITHOUT releasing",
            "    // the bus (repeated START) — what most sensors expect.",
            "    {H}.write_read(DEVICE_ADDRESS, &[0x10], &mut rx).ok();",
        ],
        &[
            "main.rs calls `init_async`, so the handle is an `I2c<'_, Async>`:",
            "",
            "    use pins::configs::i2c{N}::DEVICE_ADDRESS;",
            "",
            "    {H}.write_async(DEVICE_ADDRESS, &[0x10, 0x42]).await.ok();",
            "",
            "    let mut rx = [0u8; 2];",
            "    {H}.read_async(DEVICE_ADDRESS, &mut rx).await.ok();",
            "",
            "    // Register read (repeated START) — what most sensors expect.",
            "    {H}.write_read_async(DEVICE_ADDRESS, &[0x10], &mut rx).await.ok();",
            "",
            "    // `init` is still there if you want this bus blocking instead.",
        ],
        rt,
    )
    .replace("{N}", &n.to_string());
    file(
        &format!(
            "{}\
             use esp_hal::gpio::interconnect::{{PeripheralInput, PeripheralOutput}};\n\
             use esp_hal::i2c::master::{{Config, I2c, Instance}};\n\
             use esp_hal::time::Rate;\n",
            mode_import(rt)
        ),
        &consts,
        &body,
        &example,
    )
}

/// The `(file_name, body)` pairs for every bus instance the pins wire, given
/// `(instance, wired signals)` from
/// [`super::codegen_esp::bus_instances`].
///
/// Keyed on what the CANVAS wires, not on the Virtual Modules: a bus can be
/// wired without a module (the module only carries the settings), and the
/// generated `init` then uses esp-hal's defaults. `rt` decides whether an
/// `init_async` twin joins each `init`.
#[allow(clippy::too_many_arguments)]
/// The LEDC source clock. Fixed at 80 MHz on the ESP32-C3 — `APB_CLK` does not
/// follow `CpuClock`, so this is a constant and not something read off the
/// clock config.
const LEDC_APB_HZ: u32 = 80_000_000;

/// The widest duty resolution `freq_hz` leaves room for, in bits.
///
/// Not a free choice: esp-hal computes `divisor = (apb << 8) / freq / 2^bits`
/// and REFUSES a divisor under 256, so `2^bits` may not exceed `apb / freq`. At
/// 20 kHz that is 4000, so 11 bits fit and 12 do not — and picking 12 anyway
/// makes `configure` return `Err(Divisor)` at boot rather than at build time.
/// Fourteen is the ceiling the ESP32-C3's enum offers.
pub fn ledc_duty_bits(freq_hz: u32) -> u32 {
    let ratio = LEDC_APB_HZ / freq_hz.max(1);
    if ratio == 0 {
        return 1;
    }
    (u32::BITS - ratio.leading_zeros() - 1).clamp(1, 14)
}

/// `src/pins/configs/pwm{n}.rs`: one LEDC timer and the channels wired to it.
///
/// Two functions, not one, and the hardware is the reason: a `Channel` BORROWS
/// its timer for as long as it lives (`Config { timer: &dyn TimerIFace }`), so
/// a single `init` returning both would be a self-referential struct. `timer()`
/// hands the timer back to `main.rs`, which owns it and lends it to `init()`.
fn pwm_file(n: u8, chans: &[(u8, u16)], cfg: Option<&TimerModuleConfig>) -> String {
    let freq = cfg.map_or(1_000, |c| c.freq_hz);
    let bits = ledc_duty_bits(freq);

    let mut consts = format!("const FREQUENCY_HZ: u32 = {freq};\n");
    // One `push_str` per line on purpose: rustfmt joins a `\`-continued literal
    // and keeps the SOURCE indentation, which drops the `//` off every line but
    // the first — inside a generated file that is a syntax error, not a typo.
    consts.push_str(
        "// Duty resolution. Tied to FREQUENCY_HZ: 2^bits must stay under
",
    );
    consts.push_str(
        "// 80_000_000 / FREQUENCY_HZ (the LEDC runs off the 80 MHz APB
",
    );
    consts.push_str(
        "// clock), or `configure` returns Err(Divisor). Change one, check
",
    );
    consts.push_str(
        "// the other.
",
    );
    consts.push_str(&format!(
        "const DUTY_RESOLUTION: timer::config::Duty = timer::config::Duty::Duty{bits}Bit;\n"
    ));
    // esp-hal takes duty in WHOLE percent, so a module set to 7.5 % cannot be
    // carried across as-is. Say so in the file rather than silently rounding.
    for (ch, x100) in chans {
        let pct = (*x100 as u32).div_ceil(100).min(100);
        consts.push_str(&format!("const DUTY_CH{ch}_PCT: u8 = {pct};"));
        if x100 % 100 != 0 {
            consts.push_str(&format!(
                " // {}.{:02} % rounded up — esp-hal's LEDC takes whole percent",
                x100 / 100,
                x100 % 100
            ));
        }
        consts.push('\n');
    }

    let mut params = String::new();
    let mut body = String::new();
    let mut rets = Vec::new();
    for (ch, _) in chans {
        params.push_str(&format!("    ch{ch}: impl PeripheralOutput<'d>,\n"));
        body.push_str(&format!(
            "    let mut ch{ch} = ledc.channel(channel::Number::Channel{ch}, ch{ch});\n\
             \x20   ch{ch}\n\
             \x20       .configure(channel::config::Config {{\n\
             \x20           timer,\n\
             \x20           duty_pct: DUTY_CH{ch}_PCT,\n\
             \x20           drive_mode: DriveMode::PushPull,\n\
             \x20       }})\n\
             \x20       .unwrap();\n"
        ));
        rets.push(format!("ch{ch}"));
    }
    // One channel is a bare value, not a 1-tuple — same rule the STM32 pin
    // tuples follow, and it keeps the common case readable.
    let (ret_ty, ret_expr) = if rets.len() == 1 {
        ("channel::Channel<'d, LowSpeed>".to_owned(), rets[0].clone())
    } else {
        (
            format!(
                "({})",
                vec!["channel::Channel<'d, LowSpeed>"; rets.len()].join(", ")
            ),
            format!("({})", rets.join(", ")),
        )
    };

    let list = chans
        .iter()
        .map(|(c, _)| format!("CH{c}"))
        .collect::<Vec<_>>()
        .join("+");
    let handle = format!("_pwm{n}");
    let mut func = format!(
        "/// The LEDC timer PWM{n} runs on. One frequency for every channel on it.\n\
         ///\n\
         /// `main.rs` keeps the value alive and lends it to `init` — the channels\n\
         /// hold a reference to it for as long as they exist.\n\
         pub fn timer<'d>(ledc: &Ledc<'d>) -> timer::Timer<'d, LowSpeed> {{\n\
         \x20   let mut t = ledc.timer::<LowSpeed>(timer::Number::Timer{n});\n\
         \x20   t.configure(timer::config::Config {{\n\
         \x20       duty: DUTY_RESOLUTION,\n\
         \x20       clock_source: timer::LSClockSource::APBClk,\n\
         \x20       frequency: Rate::from_hz(FREQUENCY_HZ),\n\
         \x20   }})\n\
         \x20   .unwrap();\n\
         \x20   t\n\
         }}\n\
         \n\
         /// PWM{n} {list} — the channels wired on the canvas, at their module duty.\n\
         pub fn init<'d>(\n\
         \x20   ledc: &Ledc<'d>,\n\
         \x20   timer: &'d timer::Timer<'d, LowSpeed>,\n\
         {params}) -> {ret_ty} {{\n\
         {body}\x20   {ret_expr}\n\
         }}\n"
    );

    let first = chans.first().map(|(c, _)| *c).unwrap_or(0);

    // The same duty trait the STM32 backends generate, so a call site reads the
    // same on any chip. TWO things about it differ, and esp-hal forces both:
    // `set_duty` takes `&self`, and it returns a `Result` \u{2014} a duty above what
    // DUTY_RESOLUTION allows is a real failure, and swallowing it inside a
    // generated helper would hide it.
    if !chans.is_empty() {
        func.push_str(
            "\n/// Hundredths of a percent into the whole percent esp-hal's LEDC takes,\n",
        );
        func.push_str("/// rounded UP and clamped \u{2014} the same rounding the `DUTY_CH*_PCT`\n");
        func.push_str("/// constants above already show.\n");
        func.push_str("fn whole_percent(x100: u32) -> u8 {\n");
        func.push_str("    x100.div_ceil(100).min(100) as u8\n");
        func.push_str("}\n\n");
        func.push_str(
            "/// Set a channel's duty in the same units the STM32 backends use \u{2014}\n",
        );
        func.push_str(
            "/// HUNDREDTHS of a percent \u{2014} so the call site reads the same on any chip.\n",
        );
        func.push_str("///\n");
        func.push_str(
            "/// The channel is part of the NAME rather than an argument, and the value\n",
        );
        func.push_str("/// is rounded UP to whole percent, because that is all the LEDC takes.\n");
        func.push_str(&format!(
            "pub trait DutyHandle {{\n    /// CH{first}, the lowest channel wired to PWM{n}.\n"
        ));
        func.push_str(&format!(
            "    fn set_duty_tim_{n}(&self, value: u32) -> Result<(), channel::Error>;\n"
        ));
        for (c, _) in chans {
            func.push_str(&format!("\n    /// CH{c}.\n"));
            func.push_str(&format!(
                "    fn set_duty_tim_{n}_ch{c}(&self, value: u32) -> Result<(), channel::Error>;\n"
            ));
        }
        func.push_str("}\n\n");
        func.push_str(&format!("impl<'d> DutyHandle for {ret_ty} {{\n"));
        func.push_str(&format!(
            "    fn set_duty_tim_{n}(&self, value: u32) -> Result<(), channel::Error> {{\n"
        ));
        func.push_str(&format!(
            "        self.set_duty_tim_{n}_ch{first}(value)\n    }}\n"
        ));
        for (i, (c, _)) in chans.iter().enumerate() {
            // One channel is a bare value, not a 1-tuple \u{2014} the same rule the
            // return type follows two dozen lines up.
            let this = if chans.len() == 1 {
                "self".to_owned()
            } else {
                format!("self.{i}")
            };
            func.push_str(&format!(
                "\n    fn set_duty_tim_{n}_ch{c}(&self, value: u32) -> Result<(), channel::Error> {{\n"
            ));
            func.push_str(&format!(
                "        {this}.set_duty(whole_percent(value))\n    }}\n"
            ));
        }
        func.push_str("}\n");
    }
    let mut usage = vec![
        "In main.rs, after the init above:".to_owned(),
        String::new(),
        "    // Duty is whole percent, 0..=100.".to_owned(),
    ];
    if chans.len() == 1 {
        usage.push("    {H}.set_duty(50).ok();".to_owned());
    } else {
        usage.push(format!("    let (ch{first}, ..) = &{{H}};"));
        usage.push(format!("    ch{first}.set_duty(50).ok();"));
    }
    usage.extend([
        String::new(),
        "    // Or let the hardware fade for you — no CPU involved:".to_owned(),
        if chans.len() == 1 {
            "    {H}.start_duty_fade(0, 100, 1000).ok();".to_owned()
        } else {
            format!("    ch{first}.start_duty_fade(0, 100, 1000).ok();")
        },
    ]);
    let usage: Vec<&str> = usage.iter().map(String::as_str).collect();
    // The LEDC has no async driver in esp-hal, so both runtimes read the same.
    let example = example_for(
        &format!("Using PWM{n}"),
        &handle,
        &usage,
        &usage,
        EspRuntime::Blocking,
    );

    file(
        "use esp_hal::gpio::DriveMode;\n\
         use esp_hal::gpio::interconnect::PeripheralOutput;\n\
         use esp_hal::ledc::channel::{self, ChannelIFace};\n\
         use esp_hal::ledc::timer::{self, TimerIFace};\n\
         use esp_hal::ledc::{Ledc, LowSpeed};\n\
         use esp_hal::time::Rate;\n",
        &consts,
        &func,
        &example,
    )
}

pub fn config_files(
    uart: &[(u8, Vec<&'static str>)],
    spi: &[(u8, Vec<&'static str>)],
    i2c: &[(u8, Vec<&'static str>)],
    i2s: &[(u8, Vec<&'static str>)],
    usart_cfg: &BTreeMap<u8, UsartModuleConfig>,
    spi_cfg: &BTreeMap<u8, SpiModuleConfig>,
    i2c_cfg: &BTreeMap<u8, I2cModuleConfig>,
    i2s_cfg: &BTreeMap<u8, I2sModuleConfig>,
    // Just the channel numbers: an RMT channel has one wire, so there is no
    // signal list to carry.
    rmt: &[u8],
    rmt_cfg: &BTreeMap<u8, RmtModuleConfig>,
    rmt_hz: u32,
    // `(unit, has a control pad)` — the second decides whether `init` takes a
    // ctrl argument at all.
    pcnt: &[(u8, bool)],
    pcnt_cfg: &BTreeMap<u8, PcntModuleConfig>,
    // True when the USB pads are wired AND this chip's esp-hal has the driver
    // — see `codegen_esp::has_usb_serial_jtag`.
    usb: bool,
    // `(unit, its wired outputs as (operator, is B))`.
    mcpwm: &[(u8, Vec<(u8, bool)>)],
    mcpwm_cfg: &BTreeMap<u8, McpwmModuleConfig>,
    // The MCPWM peripheral clock, in MHz. 40 everywhere but the H2, which is 32.
    mcpwm_source_mhz: u32,
    // `Some(has a valid pad)` when the parallel port is wired at all.
    parl_io: &Option<bool>,
    parl_io_cfg: Option<&ParlIoModuleConfig>,
    // The DAC channels wired, and the module they belong to.
    dac: &[u8],
    dac_cfg: Option<&DacModuleConfig>,
    // True on the ESP32 alone, whose I2S restricts MCLK to three pads and says
    // so in the type — see `i2s_file`.
    esp32_mclk: bool,
    // LEDC timer → the channels wired on it, with each channel duty in
    // hundredths of a percent.
    pwm: &[(u8, Vec<(u8, u16)>)],
    timer_cfg: &BTreeMap<u8, TimerModuleConfig>,
    rt: EspRuntime,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (n, sigs) in uart {
        out.push((
            format!("uart{n}.rs"),
            uart_file(*n, sigs, usart_cfg.get(n), rt),
        ));
    }
    for (n, sigs) in spi {
        out.push((format!("spi{n}.rs"), spi_file(*n, sigs, spi_cfg.get(n), rt)));
    }
    for (n, sigs) in i2c {
        out.push((format!("i2c{n}.rs"), i2c_file(*n, sigs, i2c_cfg.get(n), rt)));
    }
    for (n, sigs) in i2s {
        out.push((
            format!("i2s{n}.rs"),
            i2s_file(*n, sigs, i2s_cfg.get(n), rt, esp32_mclk),
        ));
    }
    for n in rmt {
        out.push((
            format!("rmt{n}.rs"),
            rmt_file(*n, rmt_cfg.get(n), rt, rmt_hz),
        ));
    }
    for (n, has_ctrl) in pcnt {
        out.push((
            format!("pcnt{n}.rs"),
            pcnt_file(*n, *has_ctrl, pcnt_cfg.get(n), rt),
        ));
    }
    if usb {
        out.push(("usb.rs".to_owned(), usb_file(rt)));
    }
    if !dac.is_empty() {
        out.push(("dac.rs".to_owned(), dac_file(dac, dac_cfg, rt)));
    }
    if let Some(has_valid) = parl_io {
        out.push((
            "parl_io.rs".to_owned(),
            parl_io_file(parl_io_cfg, *has_valid, rt),
        ));
    }
    for (unit, outputs) in mcpwm {
        out.push((
            format!("mcpwm{unit}.rs"),
            mcpwm_file(*unit, outputs, mcpwm_cfg.get(unit), mcpwm_source_mhz),
        ));
    }
    for (n, chans) in pwm {
        out.push((format!("pwm{n}.rs"), pwm_file(*n, chans, timer_cfg.get(n))));
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
        let f = uart_file(1, &["rx", "tx"], Some(&cfg), EspRuntime::Blocking);

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
        let f = uart_file(0, &["rx", "tx"], None, EspRuntime::Blocking);
        assert!(f.contains("const BAUDRATE: u32 = 115200;"), "{f}");
        assert!(f.contains("pub fn init"), "{f}");
    }

    /// `init` declares a parameter per WIRED line and nothing more — an unused
    /// parameter would warn in the user's build and misdescribe the peripheral.
    #[test]
    fn the_signature_mirrors_the_wiring() {
        let full = spi_file(
            2,
            &["sck", "mosi", "miso", "cs"],
            None,
            EspRuntime::Blocking,
        );
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
        let no_cs = spi_file(2, &["sck", "mosi", "miso"], None, EspRuntime::Blocking);
        assert!(!no_cs.contains("cs:"), "no cs parameter:\n{no_cs}");
        assert!(!no_cs.contains("with_cs"), "no cs link:\n{no_cs}");
        assert!(no_cs.contains("miso:"), "{no_cs}");

        // TX-only UART: no rx parameter, no `.with_rx`.
        let tx_only = uart_file(1, &["tx"], None, EspRuntime::Blocking);
        assert!(!tx_only.contains("rx:"), "{tx_only}");
        assert!(!tx_only.contains("with_rx"), "{tx_only}");
        assert!(tx_only.contains(".with_tx(tx)"), "{tx_only}");
    }

    /// On the Async runtime each file gains an `init_async` BESIDE `init` — an
    /// extra function, so one bus can await while another stays blocking, and
    /// code already calling `init` keeps working.
    #[test]
    fn async_runtime_adds_init_async_without_removing_init() {
        for (name, blocking, asyncs) in [
            (
                "uart",
                uart_file(0, &["rx", "tx"], None, EspRuntime::Blocking),
                uart_file(0, &["rx", "tx"], None, EspRuntime::Async),
            ),
            (
                "spi",
                spi_file(2, &["sck", "mosi"], None, EspRuntime::Blocking),
                spi_file(2, &["sck", "mosi"], None, EspRuntime::Async),
            ),
            (
                "i2c",
                i2c_file(0, &["scl", "sda"], None, EspRuntime::Blocking),
                i2c_file(0, &["scl", "sda"], None, EspRuntime::Async),
            ),
        ] {
            assert!(
                !blocking.contains("init_async"),
                "{name}: no async twin on the blocking runtime:\n{blocking}"
            );
            assert!(
                blocking.contains("use esp_hal::Blocking;"),
                "{name}: {blocking}"
            );

            assert!(asyncs.contains("pub fn init_async"), "{name}:\n{asyncs}");
            assert!(
                asyncs.contains("pub fn init<'d>"),
                "{name}: `init` SURVIVES — the twin is additive:\n{asyncs}"
            );
            assert!(asyncs.contains(".into_async()"), "{name}:\n{asyncs}");
            assert!(
                asyncs.contains("use esp_hal::{Async, Blocking};"),
                "{name}: both mode markers imported:\n{asyncs}"
            );
            // Both functions take the same parameters — only the tail differs.
            assert_eq!(
                asyncs.matches("impl Instance + 'd").count(),
                2,
                "{name}: same signature twice:\n{asyncs}"
            );
        }
    }

    /// The example follows the runtime: `.await` methods only where they exist.
    #[test]
    fn the_example_matches_the_runtime() {
        let blocking = i2c_file(0, &["scl", "sda"], None, EspRuntime::Blocking);
        assert!(blocking.contains("write_read(DEVICE_ADDRESS"), "{blocking}");
        assert!(!blocking.contains(".await"), "{blocking}");

        let asyncs = i2c_file(0, &["scl", "sda"], None, EspRuntime::Async);
        assert!(
            asyncs.contains("write_read_async(DEVICE_ADDRESS, &[0x10], &mut rx).await"),
            "{asyncs}"
        );

        // esp-hal's async SPI is transfer-in-place only — the example must not
        // promise a `write_async` that does not exist.
        let spi = spi_file(2, &["sck", "mosi"], None, EspRuntime::Async);
        assert!(spi.contains("transfer_in_place_async"), "{spi}");
        assert!(!spi.contains("{H}.write_async"), "{spi}");
    }

    /// The duty resolution is not a taste question: too many bits and the
    /// divisor falls under 256, which esp-hal refuses AT BOOT.
    #[test]
    fn the_duty_resolution_follows_the_frequency() {
        // 80 MHz / 20 kHz = 4000, so 2^11 fits and 2^12 does not.
        assert_eq!(ledc_duty_bits(20_000), 11);
        // Plenty of room at 1 kHz — capped at what the enum offers.
        assert_eq!(ledc_duty_bits(1_000), 14);
        assert_eq!(ledc_duty_bits(50), 14);
        // And no panic at the far end, where the ratio collapses to nothing.
        assert_eq!(ledc_duty_bits(40_000_000), 1);
        assert_eq!(ledc_duty_bits(100_000_000), 1);
        // Zero is nonsense, not a crash: it reads as "as slow as possible",
        // so it lands on the widest resolution rather than dividing by zero.
        assert_eq!(ledc_duty_bits(0), 14);
    }

    /// The LEDC's shape decides the file's: a `Channel` borrows its timer, so
    /// the timer cannot be built and returned in the same call.
    #[test]
    fn the_timer_and_the_channels_are_separate_functions() {
        let mut cfg = TimerModuleConfig::new(0);
        cfg.freq_hz = 20_000;
        cfg.set_duty_x100(1, 2_000);
        let f = pwm_file(0, &[(1, 2_000)], Some(&cfg));

        assert!(f.contains("const FREQUENCY_HZ: u32 = 20000;"), "{f}");
        assert!(
            f.contains(
                "const DUTY_RESOLUTION: timer::config::Duty = timer::config::Duty::Duty11Bit;"
            ),
            "{f}"
        );
        assert!(f.contains("const DUTY_CH1_PCT: u8 = 20;"), "{f}");
        assert!(
            f.contains("pub fn timer<'d>(ledc: &Ledc<'d>) -> timer::Timer<'d, LowSpeed>"),
            "{f}"
        );
        assert!(f.contains("timer::Number::Timer0"), "{f}");
        // The channel takes the timer BY REFERENCE, for its own lifetime.
        assert!(f.contains("timer: &'d timer::Timer<'d, LowSpeed>,"), "{f}");
        assert!(f.contains("channel::Number::Channel1"), "{f}");
        // One channel is a bare value, not a 1-tuple.
        assert!(f.contains(") -> channel::Channel<'d, LowSpeed> {"), "{f}");
    }

    /// esp-hal's LEDC takes WHOLE percent. A module set to 7.5 % cannot be
    /// carried across, so the file says what it did instead of pretending.
    #[test]
    fn a_fractional_duty_is_rounded_and_says_so() {
        let f = pwm_file(0, &[(0, 750)], None);
        assert!(f.contains("const DUTY_CH0_PCT: u8 = 8;"), "{f}");
        assert!(
            f.contains("7.50 % rounded up — esp-hal's LEDC takes whole percent"),
            "{f}"
        );
        // A whole percent gets no note — there is nothing to explain.
        let f = pwm_file(0, &[(0, 2_000)], None);
        assert!(f.contains("const DUTY_CH0_PCT: u8 = 20;\n"), "{f}");
        assert!(!f.contains("rounded up"), "{f}");
    }

    /// Several channels on one timer come back as a tuple, in channel order.
    #[test]
    fn several_channels_come_back_as_a_tuple() {
        let f = pwm_file(0, &[(0, 1_000), (1, 5_000), (2, 0)], None);
        assert!(
            f.contains(
                ") -> (channel::Channel<'d, LowSpeed>, channel::Channel<'d, LowSpeed>, \
                 channel::Channel<'d, LowSpeed>) {"
            ),
            "{f}"
        );
        assert!(f.contains("    (ch0, ch1, ch2)\n"), "{f}");
        for ch in 0..3 {
            assert!(f.contains(&format!("channel::Number::Channel{ch}")), "{f}");
        }
    }

    /// Every line of the generated file is a line: no `\`-continued literal has
    /// swallowed a comment marker on its way through rustfmt.
    #[test]
    fn no_generated_comment_lost_its_marker() {
        let f = pwm_file(0, &[(1, 2_000)], None);
        for line in f.lines() {
            assert!(
                !line.starts_with(' ') || line.starts_with("    ") || line.starts_with("     "),
                "stray indentation: {line:?}"
            );
            // A line that starts with whitespace then `//` inside the GENERATED
            // block is exactly the joined-literal bug.
            let t = line.trim_start();
            assert!(
                !(line != t && t.starts_with("// ") && t.contains("FREQUENCY_HZ")),
                "comment lost its column: {line:?}"
            );
        }
    }

    #[test]
    fn one_file_per_wired_instance() {
        let files = config_files(
            &[(1, vec!["rx", "tx"])],
            &[(2, vec!["sck", "mosi"])],
            &[(0, vec!["scl", "sda"])],
            &[(0, vec!["ck", "ws", "sd"])],
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[2],
            &BTreeMap::new(),
            80_000_000,
            &[(1, true)],
            &BTreeMap::new(),
            true,
            &[(0, vec![(0, false), (0, true)])],
            &BTreeMap::new(),
            40,
            &Some(false),
            None,
            &[1, 2],
            None,
            false,
            &[(0, vec![(1, 2_000)])],
            &BTreeMap::new(),
            EspRuntime::Blocking,
        );
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "uart1.rs",
                "spi2.rs",
                "i2c0.rs",
                "i2s0.rs",
                "rmt2.rs",
                "pcnt1.rs",
                "usb.rs",
                "dac.rs",
                "parl_io.rs",
                "mcpwm0.rs",
                "pwm0.rs"
            ]
        );
    }
}

#[cfg(test)]
mod esp_duty_handle_tests {
    use super::*;

    /// One channel is a bare `Channel`, not a 1-tuple, so the impl target and
    /// the method body both lose the index.
    #[test]
    fn a_single_channel_impls_on_the_bare_type() {
        let f = pwm_file(0, &[(2, 750)], None);
        assert!(
            f.contains("impl<'d> DutyHandle for channel::Channel<'d, LowSpeed> {"),
            "{f}"
        );
        assert!(
            f.contains("        self.set_duty(whole_percent(value))"),
            "{f}"
        );
        // CH2 is the only pad, so it is also what the bare method drives.
        assert!(f.contains("        self.set_duty_tim_0_ch2(value)"), "{f}");
        for ch in [0, 1, 3] {
            assert!(!f.contains(&format!("set_duty_tim_0_ch{ch}")), "{f}");
        }
    }

    /// The trap this test exists for: the TUPLE POSITION is not the channel
    /// number. CH0+CH2 wired means `self.0` drives CH0 and `self.1` drives CH2;
    /// writing `self.2` there would compile on a 3-channel timer and drive the
    /// wrong pad on this one.
    #[test]
    fn the_tuple_index_is_the_position_not_the_channel() {
        let f = pwm_file(0, &[(0, 1_000), (2, 5_000)], None);
        assert!(
            f.contains(
                "_ch0(&self, value: u32) -> Result<(), channel::Error> {\n        self.0.set_duty("
            ),
            "{f}"
        );
        assert!(
            f.contains(
                "_ch2(&self, value: u32) -> Result<(), channel::Error> {\n        self.1.set_duty("
            ),
            "{f}"
        );
        assert!(!f.contains("self.2."), "there is no third channel:\n{f}");
    }

    /// esp-hal takes WHOLE percent, so the trait rounds up rather than losing
    /// the hundredths silently, and hands the `Result` back rather than eating
    /// a duty the resolution cannot hold.
    #[test]
    fn the_duty_is_rounded_up_and_the_result_survives() {
        let f = pwm_file(0, &[(0, 750)], None);
        assert!(f.contains("    x100.div_ceil(100).min(100) as u8"), "{f}");
        assert!(
            f.contains("fn set_duty_tim_0(&self, value: u32) -> Result<(), channel::Error>;"),
            "{f}"
        );
    }
}
