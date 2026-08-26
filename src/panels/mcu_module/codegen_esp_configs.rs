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
    AsyncBusMode, CanModuleConfig, DacModuleConfig, I2cModuleConfig, I2sDirection, I2sFormat,
    I2sModuleConfig, I2sStandard, LcdCamMode, LcdCamModuleConfig, McpwmModuleConfig, Parity,
    ParlIoModuleConfig, PcntModuleConfig, RmtModuleConfig, SpiModuleConfig, StopBits,
    TimerModuleConfig, TouchModuleConfig, UsartModuleConfig, UsbModuleConfig,
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

/// The parallel port: the sending half, the receiving half, or BOTH.
///
/// # One constructor, one channel, two halves
///
/// `ParlIo::new` consumes the peripheral and takes a SINGLE DMA channel, which
/// it splits into a tx and an rx half itself. That is the one thing that
/// differs from LCD_CAM, whose halves want a channel each — and it is why this
/// file has one `init` rather than two.
///
/// # The halves never share a wire
///
/// The GPIO matrix has separate `PARL_TX_*` and `PARL_RX_*` signals, so the two
/// halves have their own pads, their own width, their own frequency. The
/// constants are prefixed for that reason.
///
/// # Buffers come back with the drivers
///
/// The descriptors go into the driver and the buffer is what each transfer
/// moves, so `init` hands both back for every half it builds.
fn parl_io_file(
    tx: Option<(&ParlIoModuleConfig, bool)>,
    rx: Option<(&ParlIoModuleConfig, bool)>,
    rt: EspRuntime,
) -> String {
    // `(prefix, config, has a valid pad, is the receiving half)`, sending
    // first — the order `main.rs` passes the pads in.
    let halves: Vec<(&str, &ParlIoModuleConfig, bool, bool)> = [
        tx.map(|(c, v)| ("TX", c, v, false)),
        rx.map(|(c, v)| ("RX", c, v, true)),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut consts = String::new();
    let mut params =
        String::from("    parl_io: PARL_IO<'d>,\n    dma: impl DmaChannelFor<PARL_IO<'d>>,\n");
    let mut rets: Vec<String> = Vec::new();
    let mut gives: Vec<String> = Vec::new();
    let mut descs: Vec<String> = Vec::new();

    for (prefix, c, has_valid, is_rx) in &halves {
        let lanes = c.width.lanes();
        // At sixteen bits the valid signal IS one of the data lines.
        let valid = *has_valid && lanes < 16;
        let lower = prefix.to_lowercase();
        let pad = if *is_rx { "r" } else { "" };
        consts.push_str(&format!(
            "const {prefix}_FREQUENCY_HZ: u32 = {};\n\
             const {prefix}_BUFFER_BYTES: usize = {};\n",
            c.freq_hz, c.buffer_bytes,
        ));
        let pad_bound = if *is_rx {
            "impl PeripheralInput<'d>"
        } else {
            "impl PeripheralOutput<'d>"
        };
        for lane in 0..lanes {
            params.push_str(&format!("    {pad}d{lane}: {pad_bound},\n"));
        }
        params.push_str(&format!(
            "    {pad}clk: impl Peripheral{}<'d>,\n",
            if *is_rx { "Input" } else { "Output" }
        ));
        if valid {
            params.push_str(&format!(
                "    {pad}valid: impl Peripheral{}<'d>,\n",
                if *is_rx { "Input" } else { "Output" }
            ));
        }

        let data_args: Vec<String> = (0..lanes).map(|l| format!("{pad}d{l}")).collect();
        descs.push(format!(
            "\x20   let {lower}_pins = {}::new({});\n",
            c.width.esp_hal(!*is_rx),
            data_args.join(", "),
        ));
        if valid {
            descs.push(format!(
                "\x20   let {lower}_pins = {}PinConfigWithValidPin::new({lower}_pins, {pad}valid);\n",
                if *is_rx { "Rx" } else { "Tx" },
            ));
        }
        // `ClkInPin` is a TxClkPin — a transmitter clocked from OUTSIDE.
        // The receiver wants `RxClkInPin`, which also names the edge it
        // samples on. Sampling on the wrong one reads the bus
        // mid-transition: garbage that looks like noise, not an error.
        descs.push(if *is_rx {
            format!(
                "\x20   let {lower}_clk = RxClkInPin::new({pad}clk, SampleEdge::{});\n",
                c.sample_edge.esp_hal(),
            )
        } else {
            format!("\x20   let {lower}_clk = ClkOutPin::new({pad}clk);\n")
        });

        let (buffers, sizes, buf_ty) = if *is_rx {
            (
                format!("({lower}_buffer, {lower}_desc, _, _)"),
                format!("{prefix}_BUFFER_BYTES, 0"),
                "DmaRxBuf",
            )
        } else {
            (
                format!("(_, _, {lower}_buffer, {lower}_desc)"),
                format!("0, {prefix}_BUFFER_BYTES"),
                "DmaTxBuf",
            )
        };
        descs.push(format!(
            "\x20   let {buffers} = dma_buffers!({sizes});\n\
             \x20   let {lower}_buf = {buf_ty}::new({lower}_desc, {lower}_buffer).unwrap();\n",
        ));

        rets.push(format!(
            "ParlIo{}<'d, {{DM}}>",
            if *is_rx { "Rx" } else { "Tx" }
        ));
        rets.push(buf_ty.to_owned());
        gives.push(format!("{lower}_driver"));
        gives.push(format!("{lower}_buf"));
    }

    let body_for = |name: &str, mode: &str, into_async: &str| {
        let mut build = String::new();
        for (prefix, c, has_valid, is_rx) in &halves {
            let lower = prefix.to_lowercase();
            let valid = *has_valid && c.width.lanes() < 16;
            let _ = valid;
            // The receiver needs a timeout or a frame never ends; the
            // transmitter has no such knob.
            let extra = if *is_rx {
                "\n\x20       .with_timeout_ticks(0xfff)"
            } else {
                ""
            };
            build.push_str(&format!(
                "\x20   let {lower}_config = {}Config::default()\n\
                 \x20       .with_frequency(Rate::from_hz({prefix}_FREQUENCY_HZ))\n\
                 \x20       .with_bit_order(BitPackOrder::{}){extra};\n",
                if *is_rx { "Rx" } else { "Tx" },
                c.bit_order.esp_hal(),
            ));
        }
        let mut take = String::new();
        for (prefix, _, _, is_rx) in &halves {
            let lower = prefix.to_lowercase();
            take.push_str(&format!(
                "\x20   let {lower}_driver = port.{}.with_config({lower}_pins, {lower}_clk, {lower}_config).unwrap();\n",
                if *is_rx { "rx" } else { "tx" },
            ));
        }
        format!(
            "pub fn {name}<'d>(\n\
             {params}) -> ({}) {{\n\
             {}{build}\
             \x20   let port = ParlIo::new(parl_io, dma).unwrap(){into_async};\n\
             {take}\
             \x20   ({})\n\
             }}\n",
            rets.join(", ").replace("{DM}", mode),
            descs.concat(),
            gives.join(", "),
        )
    };

    let mut body = format!(
        "/// The parallel port — {}.\n\
         ///\n\
         /// Returns each driver AND its DMA buffer: the descriptors go into the\n\
         /// driver, the buffer is what each transfer moves.\n\
         {}",
        halves
            .iter()
            .map(|(_, c, _, is_rx)| format!(
                "{} {} lines at {} Hz",
                if *is_rx { "receiving" } else { "transmitting" },
                c.width.lanes(),
                c.freq_hz
            ))
            .collect::<Vec<_>>()
            .join(" + "),
        body_for("init", "Blocking", ""),
    );
    if rt == EspRuntime::Async {
        body.push_str(&format!(
            "\n/// The same, async.\n{}",
            body_for("init_async", "Async", ".into_async()"),
        ));
    }

    // The handles `main.rs` binds, so the snippet is pasteable rather than a
    // sketch with a `…` in it. A `write`/`read` takes the buffer BY VALUE and
    // the transfer hands it back, which is why they are reassigned.
    let sending = halves.iter().any(|(_, _, _, is_rx)| !*is_rx);
    let (handle, buf, verb, len) = if sending {
        ("_parl", "_parl_buf", "write", "_parl_buf.len()")
    } else {
        (
            "_parl_rx",
            "_parl_rx_buf",
            "read",
            "Some(_parl_rx_buf.len())",
        )
    };
    let fill = if sending {
        format!("    {buf}.as_mut_slice().fill(0xAA);\n")
    } else {
        String::new()
    };
    let sync = vec![
        if sending {
            "Fill the buffer, hand it over, and take it back when done:".to_owned()
        } else {
            "Hand the buffer over and take it back full:".to_owned()
        },
        String::new(),
        fill.trim_end().to_owned(),
        format!("    let t = {handle}.{verb}({len}, {buf}).unwrap();"),
        "    let (result, p, b) = t.wait();".to_owned(),
        format!("    ({handle}, {buf}) = (p, b);"),
        "    result.ok();".to_owned(),
        String::new(),
        "// With BOTH halves built, each has its own handle and its own buffer -".to_owned(),
        "// they share only the peripheral and the ONE DMA channel it splits.".to_owned(),
    ];
    let asyn = [
        "The same, awaited:".to_owned(),
        String::new(),
        format!("    let mut t = {handle}.{verb}({len}, {buf}).unwrap();"),
        "    t.wait_for_done().await;".to_owned(),
        "    let (result, p, b) = t.wait();".to_owned(),
        format!("    ({handle}, {buf}) = (p, b);"),
        "    result.ok();".to_owned(),
    ];
    let sync: Vec<&str> = sync.iter().map(String::as_str).collect();
    let asyn: Vec<&str> = asyn.iter().map(String::as_str).collect();
    let example = example_for("Using the parallel port", handle, &sync, &asyn, rt);

    let mut uses = vec![
        // The mode markers name the return type. `Async` only appears when the
        // async twin is emitted, so it is added with that.
        "use esp_hal::Blocking;".to_owned(),
        "use esp_hal::dma::DmaChannelFor;".to_owned(),
        "use esp_hal::dma_buffers;".to_owned(),
        "use esp_hal::peripherals::PARL_IO;".to_owned(),
        "use esp_hal::time::Rate;".to_owned(),
    ];
    let mut items: Vec<String> = vec!["BitPackOrder".to_owned(), "ParlIo".to_owned()];
    for (_, c, has_valid, is_rx) in &halves {
        let valid = *has_valid && c.width.lanes() < 16;
        items.push(c.width.esp_hal(!*is_rx).to_owned());
        if *is_rx {
            items.extend([
                "RxClkInPin".to_owned(),
                "SampleEdge".to_owned(),
                "ParlIoRx".to_owned(),
                "RxConfig".to_owned(),
            ]);
            uses.push("use esp_hal::dma::DmaRxBuf;".to_owned());
            if valid {
                items.push("RxPinConfigWithValidPin".to_owned());
            }
        } else {
            items.extend([
                "ClkOutPin".to_owned(),
                "ParlIoTx".to_owned(),
                "TxConfig".to_owned(),
            ]);
            uses.push("use esp_hal::dma::DmaTxBuf;".to_owned());
            if valid {
                items.push("TxPinConfigWithValidPin".to_owned());
            }
        }
    }
    let ins = halves.iter().any(|(_, _, _, is_rx)| *is_rx);
    let outs = halves.iter().any(|(_, _, _, is_rx)| !*is_rx);
    uses.push(format!(
        "use esp_hal::gpio::interconnect::{};",
        match (ins, outs) {
            (true, true) => "{PeripheralInput, PeripheralOutput}".to_owned(),
            (true, false) => "PeripheralInput".to_owned(),
            _ => "PeripheralOutput".to_owned(),
        }
    ));
    if rt == EspRuntime::Async {
        uses.push("use esp_hal::Async;".to_owned());
    }
    items.sort();
    items.dedup();
    uses.push(format!("use esp_hal::parl_io::{{{}}};", items.join(", ")));
    uses.sort();
    uses.dedup();

    file(&format!("{}\n", uses.join("\n")), &consts, &body, &example)
}

// ── MCPWM ───────────────────────────────────────────────────────────────────

/// One MCPWM unit: its timers, its operators, and the pads they drive.
///
/// # A timer per operator
///
/// The unit has THREE timers and three operators, and `Operator::set_timer`
/// takes the timer index as a const parameter — any operator can be pointed at
/// any timer. That is what puts two motors at two frequencies on one unit, and
/// it is why the frequency and the period are per TIMER here rather than per
/// unit. Only the timers something actually runs on are started.
///
/// # The timers come back
///
/// A `Timer` owns the guard that holds the MCPWM clock on; a `PwmPin` does not.
/// Returning only the pins compiles and silently kills every output, so `init`
/// hands the started timers back and `main.rs` keeps them.
///
/// # `with_pin_a` consumes the operator
///
/// An operator whose two outputs are both wired has to build them together with
/// `with_pins`; reaching for `with_pin_a` and then `with_pin_b` does not
/// compile, because the first call moved the operator.
fn mcpwm_file(
    unit: u8,
    outputs: &[(u8, bool)],
    cfg: Option<&McpwmModuleConfig>,
    source_mhz: u32,
) -> String {
    let d = McpwmModuleConfig::new(unit);
    let c = cfg.unwrap_or(&d);
    let peri = format!("MCPWM{unit}");

    let mut ops: Vec<u8> = outputs.iter().map(|(op, _)| *op).collect();
    ops.sort_unstable();
    ops.dedup();
    let timers = c.timers_used(&ops);

    let mut consts = String::new();
    for t in &timers {
        consts.push_str(&format!(
            "const FREQUENCY_HZ_T{t}: u32 = {};\n\
             // Timer {t} counts 0..=PERIOD_T{t}, so a duty lands on one of PERIOD_T{t}+1 steps. Public: main.rs sets duty in terms of it.\n\
             pub const PERIOD_T{t}: u16 = {};\n",
            c.timer_freq_hz(*t),
            c.timer_period(*t),
        ));
    }
    for (op, b) in outputs {
        consts.push_str(&format!(
            "const TIMESTAMP_OP{op}{}: u16 = {}; // {:.2} % of timer {}\n",
            if *b { "B" } else { "A" },
            c.timestamp_of(*op, *b),
            f64::from(c.duty_x100_of(*op, *b)) / 100.0,
            c.timer_of(*op),
        ));
    }

    let name = |op: u8, b: bool| format!("op{op}{}", if b { "b" } else { "a" });
    let params: String = outputs
        .iter()
        .map(|(op, b)| format!("    {}: impl PeripheralOutput<'d>,\n", name(*op, *b)))
        .collect();
    // The const parameter is IS_A, so it is the NEGATION of "this is the B
    // output" — inverting it silently swaps which pad each handle drives.
    let ret: String = timers
        .iter()
        .map(|t| format!("Timer<{t}, {peri}<'d>>, "))
        .chain(
            outputs
                .iter()
                .map(|(op, b)| format!("PwmPin<'d, {peri}<'d>, {op}, {}>, ", !b)),
        )
        .collect::<String>()
        .trim_end_matches(", ")
        .to_owned();

    // Each operator is pointed at ITS timer once, however many of its two
    // outputs are wired.
    let links: String = ops
        .iter()
        .map(|op| {
            format!(
                "\x20   mcpwm.operator{op}.set_timer(&mcpwm.timer{});\n",
                c.timer_of(*op)
            )
        })
        .collect();

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

    // One config and one `start` per timer IN USE. Starting an unused one would
    // hold the peripheral clock for an output that does not exist.
    let starts: String = timers
        .iter()
        .map(|t| {
            format!(
                "\x20   let timer_cfg_t{t} = clock_cfg\n\
                 \x20       .timer_clock_with_frequency(\n\
                 \x20           PERIOD_T{t},\n\
                 \x20           PwmWorkingMode::Increase,\n\
                 \x20           Rate::from_hz(FREQUENCY_HZ_T{t}),\n\
                 \x20       )\n\
                 \x20       .unwrap();\n\
                 \x20   mcpwm.timer{t}.start(timer_cfg_t{t});\n"
            )
        })
        .collect();

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
    let handles: String = timers
        .iter()
        .map(|t| format!("mcpwm.timer{t}, "))
        .chain(outputs.iter().map(|(op, b)| format!("{}, ", name(*op, *b))))
        .collect::<String>()
        .trim_end_matches(", ")
        .to_owned();

    let body = format!(
        "/// MCPWM{unit} — motor-control PWM on {} timer{}.\n\
         ///\n\
         /// Returns the TIMERS as well as the pins, and `main.rs` keeps them: a\n\
         /// timer owns the guard that holds the MCPWM clock on. Drop one and\n\
         /// every output on it goes quiet.\n\
         pub fn init<'d>(\n\
         \x20   mcpwm: {peri}<'d>,\n\
         {params}) -> ({ret}) {{\n\
         \x20   let clock_cfg = PeripheralClockConfig::with_frequency(Rate::from_mhz({source_mhz}))\n\
         \x20       .unwrap();\n\
         \x20   let mut mcpwm = McPwm::new(mcpwm, clock_cfg);\n\
         {links}{pins}\n\
         {starts}{stamps}\n\
         \x20   ({handles})\n\
         }}\n",
        timers.len(),
        if timers.len() == 1 { "" } else { "s" },
    );

    let first = outputs.first().copied().unwrap_or((0, false));
    let ft = c.timer_of(first.0);
    let example = example_block(
        &format!("Using MCPWM{unit}"),
        &[
            format!(
                "Duty is a TIMESTAMP: the count the output flips at, out of {}.",
                format!("PERIOD_T{ft} + 1")
            ),
            String::new(),
            format!(
                // The handle main.rs binds, which carries the unit in its name.
                "    _mcpwm{unit}_{}.set_timestamp(pins::configs::mcpwm{unit}::PERIOD_T{ft} / 2);   // 50 %",
                name(first.0, first.1)
            ),
            String::new(),
            "// Each timer has its own PERIOD, so a duty computed against one".to_owned(),
            "// means nothing on another. The constant is named for its timer.".to_owned(),
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

/// The OTG full-speed controller — a USB device of the project's own design.
///
/// # Not the same peripheral as Serial/JTAG
///
/// They share the pads and nothing else. Serial/JTAG is a fixed console with
/// Espressif's identity; this is a controller you hang a `usb-device` stack on,
/// which is why the VID, PID and product string mean something here and mean
/// nothing there. Only the S2 and S3 have it.
///
/// # DP comes first
///
/// `Usb::new` takes D+ BEFORE D-, the opposite of how the pads are usually
/// listed. Swapping them compiles and enumerates nothing.
///
/// # The allocator cannot come back with its classes
///
/// `UsbBus::new` hands back a `UsbBusAllocator` that everything else BORROWS —
/// the serial port and the device both hold a reference to it. A function
/// cannot return a value and a borrow of that value, so this one returns the
/// allocator alone and `main.rs` builds the rest around it, in one scope.
fn usb_otg_file(cfg: Option<&UsbModuleConfig>) -> String {
    let d = UsbModuleConfig::new(1);
    let c = cfg.unwrap_or(&d);
    let consts = format!(
        "// The host sees these. `0x16c0:0x27dd` is pid.codes' test pair - fine on\n\
         // a bench, not for anything shipped.\n\
         pub const VID: u16 = 0x{:04x};\n\
         pub const PID: u16 = 0x{:04x};\n\
         pub const PRODUCT: &str = {:?};\n\
         // Endpoint scratch, in 32-bit words. 256 is enough for a CDC serial\n\
         // port; a composite device with more endpoints needs more.\n\
         const EP_WORDS: usize = 256;\n",
        c.vid, c.pid, c.product,
    );

    let body = "/// The OTG bus, ready for `usb-device` classes.\n\
         ///\n\
         /// D+ FIRST: `Usb::new` takes them in that order. The wrong way round\n\
         /// still compiles and enumerates nothing.\n\
         pub fn init<'d>(\n\
         \x20   usb0: USB0<'d>,\n\
         \x20   dp: impl UsbDp + 'd,\n\
         \x20   dm: impl UsbDm + 'd,\n\
         ) -> UsbBusAllocator<UsbBus<Usb<'d>>> {\n\
         \x20   // The endpoint scratch has to be `'static`, and `init` can only\n\
         \x20   // run once because it takes the USB0 singleton BY VALUE - which\n\
         \x20   // is what makes this handout sound.\n\
         \x20   static mut EP_MEMORY: [u32; EP_WORDS] = [0; EP_WORDS];\n\
         \x20   // SAFETY: see above - one call, one borrow, no other name for it.\n\
         \x20   let ep = unsafe { &mut *(&raw mut EP_MEMORY) };\n\
         \x20   UsbBus::new(Usb::new(usb0, dp, dm), ep)\n\
         }\n"
    .to_owned();

    let example = example_block(
        "Using the USB OTG port",
        &[
            "`main.rs` already builds the device around this bus. Poll it in".to_owned(),
            "your loop - nothing enumerates until you do:".to_owned(),
            String::new(),
            "    if _usb_dev.poll(&mut [&mut _usb_serial]) {".to_owned(),
            "        let mut buf = [0u8; 64];".to_owned(),
            "        if let Ok(n) = _usb_serial.read(&mut buf) {".to_owned(),
            "            _usb_serial.write(&buf[..n]).ok();".to_owned(),
            "        }".to_owned(),
            "    }".to_owned(),
            String::new(),
            "// `poll` must be called often - faster than once a millisecond, or".to_owned(),
            "// the host gives up on the device.".to_owned(),
        ],
    );

    file(
        "use esp_hal::otg_fs::{Usb, UsbBus, UsbDm, UsbDp};\n\
         use esp_hal::peripherals::USB0;\n\
         use usb_device::bus::UsbBusAllocator;\n",
        &consts,
        &body,
        &example,
    )
}

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
fn usb_file(rt: EspRuntime, cfg: Option<&UsbModuleConfig>) -> String {
    if cfg.is_some_and(|c| c.role.is_otg()) {
        return usb_otg_file(cfg);
    }
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

// ── LCD_CAM ─────────────────────────────────────────────────────────────

/// The parallel video port, in whichever of its three shapes the module picked.
///
/// # One peripheral, three drivers
///
/// esp-hal splits LCD_CAM into `lcd::i8080`, `lcd::dpi` and `cam`. They share
/// `LcdCam::new`, which hands back two halves — `.lcd` and `.cam` — and each
/// driver consumes one of them. This file builds exactly one, because the
/// module has exactly one mode.
///
/// # DMA is not optional
///
/// All three take a channel in their only constructor, so a project with none
/// left gets no video port at all rather than a slower one. The LCD half wants
/// a TX channel and the camera an RX channel; the same pool serves both.
///
/// # Async is on the PERIPHERAL, not the driver
///
/// `into_async` sits on `LcdCam`, before either half is taken. That is why it
/// appears in the middle of `init` rather than at the end of the chain, and why
/// the camera's type does not change with it: `Camera` has no mode parameter,
/// only its transfer has an `await`.
/// What one LCD_CAM half needs, so the arms below read side by side instead of
/// as a five-place tuple.
struct LcdCamShape<'a> {
    /// The control pads, as `(name, bound)` — the data lanes are appended after.
    ctl_params: Vec<(&'a str, &'a str)>,
    /// The same pads as `(name, setter)`.
    ctl_chain: Vec<(&'a str, &'a str)>,
    /// Which half of `LcdCam` this driver consumes: `lcd` or `cam`.
    half: &'a str,
    /// The return type, written out.
    ty: String,
    /// The esp-hal module under `lcd_cam::` that holds it.
    driver_mod: &'a str,
    /// The data pads' prefix. The two halves need DIFFERENT names in one
    /// signature, because both can appear in it.
    data_prefix: &'a str,
    /// The bound every data pad takes.
    data_dir: &'a str,
    /// `Tx` or `Rx` — which kind of DMA channel this half asks for.
    dma_dir: &'a str,
}

/// The shape of one half, given its mode.
fn lcd_cam_shape<'a>(mode: LcdCamMode, dm: &str) -> LcdCamShape<'a> {
    match mode {
        LcdCamMode::I8080 => LcdCamShape {
            ctl_params: vec![
                ("dc", "impl PeripheralOutput<'d>"),
                ("wr", "impl PeripheralOutput<'d>"),
                ("cs", "impl PeripheralOutput<'d>"),
            ],
            ctl_chain: vec![("dc", "with_dc"), ("wr", "with_wrx"), ("cs", "with_cs")],
            half: "lcd",
            ty: format!("I8080<'d, {dm}>"),
            driver_mod: "lcd::i8080",
            data_prefix: "d",
            data_dir: "impl PeripheralOutput<'d>",
            dma_dir: "Tx",
        },
        LcdCamMode::Dpi => LcdCamShape {
            ctl_params: vec![
                ("vsync", "impl PeripheralOutput<'d>"),
                ("hsync", "impl PeripheralOutput<'d>"),
                ("de", "impl PeripheralOutput<'d>"),
                ("pclk", "impl PeripheralOutput<'d>"),
            ],
            ctl_chain: vec![
                ("vsync", "with_vsync"),
                ("hsync", "with_hsync"),
                ("de", "with_de"),
                ("pclk", "with_pclk"),
            ],
            half: "lcd",
            ty: format!("Dpi<'d, {dm}>"),
            driver_mod: "lcd::dpi",
            data_prefix: "d",
            data_dir: "impl PeripheralOutput<'d>",
            dma_dir: "Tx",
        },
        // The camera READS: every pad is an input except the master clock,
        // which is the one thing this chip gives the sensor. Its control pads
        // are prefixed too, so a display's `pclk` and a sensor's never collide.
        LcdCamMode::Camera => LcdCamShape {
            ctl_params: vec![
                ("mclk", "impl PeripheralOutput<'d>"),
                ("cam_pclk", "impl PeripheralInput<'d>"),
                ("cam_vsync", "impl PeripheralInput<'d>"),
                ("cam_hsync", "impl PeripheralInput<'d>"),
                ("href", "impl PeripheralInput<'d>"),
            ],
            ctl_chain: vec![
                ("mclk", "with_master_clock"),
                ("cam_pclk", "with_pixel_clock"),
                ("cam_vsync", "with_vsync"),
                ("cam_hsync", "with_hsync"),
                ("href", "with_h_enable"),
            ],
            half: "cam",
            // `Camera` carries no driver mode at all.
            ty: "Camera<'d>".to_owned(),
            driver_mod: "cam",
            data_prefix: "cd",
            data_dir: "impl PeripheralInput<'d>",
            dma_dir: "Rx",
        },
    }
}

/// The parallel video port: the display half, the camera half, or BOTH.
///
/// # One constructor, two halves
///
/// `LcdCam::new` consumes the peripheral once and hands back `.lcd` and `.cam`
/// together. They then run independently — an ESP32-S3 driving a display while
/// reading a sensor is what the block is for — so this file has ONE `init` that
/// builds whichever halves the canvas wired, rather than two that would each
/// want the same peripheral.
///
/// # Two channels when both are live
///
/// The display half takes a TX channel and the camera an RX one. Neither has a
/// non-DMA form, so a project short of channels gets no video port at all
/// rather than a slower one.
///
/// # Async is on the PERIPHERAL
///
/// `into_async` sits on `LcdCam`, before either half is taken. That is why it
/// appears in the middle of `init` rather than at the end of a chain, and why
/// the camera's type does not change with it: `Camera` has no mode parameter.
fn lcd_cam_file(
    lcd_sigs: &[&str],
    lcd_cfg: Option<&LcdCamModuleConfig>,
    cam_sigs: &[&str],
    cam_cfg: Option<&LcdCamModuleConfig>,
    rt: EspRuntime,
) -> String {
    let asyn = rt == EspRuntime::Async;
    let dm = if asyn { "Async" } else { "Blocking" };
    let into_async = if asyn { ".into_async()" } else { "" };

    // `(prefix, shape, config, the signal names wired)` per half, display first
    // — the order `main.rs` passes them in.
    let mut halves: Vec<(&str, LcdCamShape, &LcdCamModuleConfig, &[&str])> = Vec::new();
    if !lcd_sigs.is_empty() {
        let c = lcd_cfg.expect("a wired display half has a module");
        halves.push(("LCD", lcd_cam_shape(c.mode, dm), c, lcd_sigs));
    }
    if !cam_sigs.is_empty() {
        let c = cam_cfg.expect("a wired camera half has a module");
        halves.push(("CAM", lcd_cam_shape(LcdCamMode::Camera, dm), c, cam_sigs));
    }

    let mut consts = String::new();
    let mut params = String::new();
    let mut dma_params = String::new();
    let mut body_lines = String::new();
    let mut rets: Vec<String> = Vec::new();
    let mut handles: Vec<String> = Vec::new();
    let mut driver_mods: Vec<String> = Vec::new();
    let mut dma_dirs: Vec<&str> = Vec::new();

    for (prefix, shape, c, sigs) in &halves {
        let lower = prefix.to_lowercase();
        consts.push_str(&lcd_cam_consts(prefix, c));
        dma_params.push_str(&format!(
            "\x20   dma_{lower}: impl {}ChannelFor<LCD_CAM<'d>>,\n",
            shape.dma_dir
        ));
        dma_dirs.push(shape.dma_dir);
        driver_mods.push(shape.driver_mod.to_owned());

        let lanes = usize::from(c.width.min(16)).max(8);
        let data: Vec<(String, String)> = (0..lanes)
            .map(|n| (format!("{}{n}", shape.data_prefix), format!("with_data{n}")))
            .collect();
        let mut param_spec: Vec<(&str, &str)> = shape.ctl_params.clone();
        let mut chain_spec: Vec<(&str, &str)> = shape.ctl_chain.clone();
        for (name, method) in &data {
            param_spec.push((name.as_str(), shape.data_dir));
            chain_spec.push((name.as_str(), method.as_str()));
        }
        params.push_str(&params_for(sigs, &param_spec));

        body_lines.push_str(&lcd_cam_config_expr(prefix, c));
        // The chain must END the statement, not sit above a lone semicolon.
        let mut chain = chain_for(sigs, &chain_spec);
        if chain.ends_with('\n') {
            chain.pop();
        }
        chain.push_str(";\n");
        body_lines.push_str(&format!(
            "\x20   let {lower} = {}::new(lcd_cam.{}, dma_{lower}, {lower}_config)\n\
             \x20       .unwrap()\n\
             {chain}",
            c.mode.driver(),
            shape.half,
        ));
        rets.push(shape.ty.clone());
        handles.push(lower);
    }

    // A single half is returned bare; two come back as a pair.
    let ret = if rets.len() == 1 {
        rets[0].clone()
    } else {
        format!("({})", rets.join(", "))
    };
    let give = if handles.len() == 1 {
        handles[0].clone()
    } else {
        format!("({})", handles.join(", "))
    };

    let body = format!(
        "/// The video port: {}.\n\
         ///\n\
         /// Every pad here is routed through the GPIO matrix, so which one it\n\
         /// is was decided on the canvas rather than by the silicon.\n\
         pub fn {fname}<'d>(\n\
         \x20   lcd_cam: LCD_CAM<'d>,\n\
         {dma_params}{params}) -> {ret} {{\n\
         \x20   let lcd_cam = LcdCam::new(lcd_cam){into_async};\n\
         {body_lines}\
         \x20   {give}\n\
         }}\n",
        halves
            .iter()
            .map(|(_, _, c, _)| c.mode.label())
            .collect::<Vec<_>>()
            .join(" + "),
        fname = if asyn { "init_async" } else { "init" },
    );

    let example = lcd_cam_example(&halves, rt);

    // Which imports a file needs is decided by what the code below actually
    // says, not by the modes: a camera with no master clock binds no output,
    // a display never binds an input, and `Camera` carries no mode marker.
    let mut traits = Vec::new();
    if params.contains("PeripheralInput") {
        traits.push("PeripheralInput");
    }
    if params.contains("PeripheralOutput") {
        traits.push("PeripheralOutput");
    }
    let mode_use = if ret.contains(dm) {
        format!("use esp_hal::{dm};\n")
    } else {
        String::new()
    };
    let mut dma_use: Vec<String> = dma_dirs
        .iter()
        .map(|d| format!("use esp_hal::dma::{d}ChannelFor;\n"))
        .collect();
    dma_use.sort();
    dma_use.dedup();

    let mut driver_use: Vec<String> = halves
        .iter()
        .map(|(_, shape, c, _)| {
            format!(
                "use esp_hal::lcd_cam::{}::{{Config as {}Config, {}{}}};\n",
                shape.driver_mod,
                c.mode.driver(),
                c.mode.driver(),
                if c.mode == LcdCamMode::Dpi {
                    ", Format, FrameTiming"
                } else {
                    ""
                },
            )
        })
        .collect();
    driver_use.sort();
    driver_use.dedup();
    let _ = driver_mods;

    file(
        &format!(
            "{mode_use}{}\
             use esp_hal::gpio::interconnect::{};\n\
             use esp_hal::lcd_cam::LcdCam;\n\
             {}\
             use esp_hal::peripherals::LCD_CAM;\n\
             use esp_hal::time::Rate;\n",
            dma_use.join(""),
            if traits.len() == 1 {
                traits[0].to_owned()
            } else {
                format!("{{{}}}", traits.join(", "))
            },
            driver_use.join(""),
        ),
        &consts,
        &body,
        &example,
    )
}

/// The editable constants for ONE half, namespaced by its prefix.
///
/// `LCD_` and `CAM_`: both halves can be in one file, and both have a
/// frequency and a width, so unprefixed names would collide.
///
/// The RGB timings are constants rather than literals buried in a builder
/// chain: they come off a panel datasheet, they are the numbers most likely to
/// need one more nudge, and a rolling picture is what a wrong one looks like.
fn lcd_cam_consts(prefix: &str, cfg: &LcdCamModuleConfig) -> String {
    let mut s = format!(
        "const {prefix}_FREQUENCY: Rate = Rate::from_hz({});\n",
        cfg.clock_hz.max(1)
    );
    if cfg.mode == LcdCamMode::Dpi {
        s.push_str(&format!(
            "// Straight off the panel's datasheet. Total = active + blanking.\n\
             const {prefix}_H_ACTIVE: usize = {};\n\
             const {prefix}_V_ACTIVE: usize = {};\n\
             const {prefix}_H_TOTAL: usize = {};\n\
             const {prefix}_V_TOTAL: usize = {};\n\
             const {prefix}_H_FRONT_PORCH: usize = {};\n\
             const {prefix}_V_FRONT_PORCH: usize = {};\n\
             const {prefix}_HSYNC_WIDTH: usize = {};\n\
             const {prefix}_VSYNC_WIDTH: usize = {};\n",
            cfg.h_active,
            cfg.v_active,
            cfg.h_total,
            cfg.v_total,
            cfg.h_front_porch,
            cfg.v_front_porch,
            cfg.hsync_width,
            cfg.vsync_width,
        ));
    }
    if cfg.mode != LcdCamMode::I8080 {
        s.push_str(&format!(
            "const {prefix}_TWO_BYTE_MODE: bool = {};\n",
            cfg.width >= 16
        ));
    }
    s
}

/// The `let <half>_config = ...;` line, which differs enough per mode to be
/// worth its own function rather than three branches inside the body format.
fn lcd_cam_config_expr(prefix: &str, cfg: &LcdCamModuleConfig) -> String {
    let lower = prefix.to_lowercase();
    let ty = format!("{}Config", cfg.mode.driver());
    match cfg.mode {
        // The i8080 driver takes its width from how many data pads are bound,
        // so there is nothing to set here beyond the clock.
        LcdCamMode::I8080 => format!(
            "\x20   let {lower}_config = {ty}::default().with_frequency({prefix}_FREQUENCY);\n"
        ),
        LcdCamMode::Camera => format!(
            "\x20   let {lower}_config = {ty}::default()\n\
             \x20       .with_frequency({prefix}_FREQUENCY)\n\
             \x20       .with_enable_2byte_mode({prefix}_TWO_BYTE_MODE);\n"
        ),
        LcdCamMode::Dpi => format!(
            "\x20   let {lower}_config = {ty}::default()\n\
             \x20       .with_frequency({prefix}_FREQUENCY)\n\
             \x20       .with_format(Format {{\n\
             \x20           enable_2byte_mode: {prefix}_TWO_BYTE_MODE,\n\
             \x20           ..Default::default()\n\
             \x20       }})\n\
             \x20       .with_timing(FrameTiming {{\n\
             \x20           horizontal_active_width: {prefix}_H_ACTIVE,\n\
             \x20           horizontal_total_width: {prefix}_H_TOTAL,\n\
             \x20           horizontal_blank_front_porch: {prefix}_H_FRONT_PORCH,\n\
             \x20           vertical_active_height: {prefix}_V_ACTIVE,\n\
             \x20           vertical_total_height: {prefix}_V_TOTAL,\n\
             \x20           vertical_blank_front_porch: {prefix}_V_FRONT_PORCH,\n\
             \x20           hsync_width: {prefix}_HSYNC_WIDTH,\n\
             \x20           vsync_width: {prefix}_VSYNC_WIDTH,\n\
             \x20           hsync_position: 0,\n\
             \x20       }});\n"
        ),
    }
}

/// What to do with each handle. The three modes move data three different ways,
/// and none of them looks like a bus.
///
/// Two things here are not symmetric, and both come from esp-hal rather than
/// from choice: every `send`/`receive` reports failure as a TUPLE that carries
/// the driver and the buffer back (so `.unwrap()` alone will not compile), and
/// `wait_for_done` exists on the i8080 transfer ALONE — the RGB and camera
/// transfers have `is_done`, `stop` and a blocking `wait`, on either runtime.
fn lcd_cam_example(
    halves: &[(&str, LcdCamShape, &LcdCamModuleConfig, &[&str])],
    rt: EspRuntime,
) -> String {
    let asyn = rt == EspRuntime::Async;
    let mut lines: Vec<String> = Vec::new();
    for (i, (_, _, c, _)) in halves.iter().enumerate() {
        if i > 0 {
            lines.push(String::new());
            lines.push("// …and the other half, at the same time:".to_owned());
            lines.push(String::new());
        }
        match c.mode {
            LcdCamMode::I8080 => {
                lines.push("A command, then its pixels, out of a DMA buffer:".to_owned());
                lines.push(String::new());
                lines.push("    use esp_hal::dma_tx_buffer;".to_owned());
                lines.push("    let mut buf = dma_tx_buffer!(32768).unwrap();".to_owned());
                lines.push("    buf.fill(&[0x55; 32]);".to_owned());
                lines.push(
                    "    // 0x3A is MIPI's COLMOD; the 0 is the dummy-cycle count.".to_owned(),
                );
                lines.push(
                    "    let mut t = _lcd.send(0x3Au8, 0, buf).map_err(|e| e.0).unwrap();"
                        .to_owned(),
                );
                if asyn {
                    lines.push("    t.wait_for_done().await;".to_owned());
                }
                lines.push("    let (result, _lcd, _buf) = t.wait();".to_owned());
                lines.push("    result.unwrap();".to_owned());
            }
            LcdCamMode::Dpi => {
                lines.push("An RGB panel is fed forever, so the buffer LOOPS:".to_owned());
                lines.push(String::new());
                lines.push("    use esp_hal::dma_loop_buffer;".to_owned());
                lines.push("    let mut buf = dma_loop_buffer!(32);".to_owned());
                lines.push("    buf.fill(0xFF);   // a solid frame".to_owned());
                lines
                    .push("    let t = _lcd.send(true, buf).map_err(|e| e.0).unwrap();".to_owned());
                lines.push("    let (_lcd, _buf) = t.stop();".to_owned());
                lines.push(
                    "    // `true` loops it: stopping the transfer blanks the screen.".to_owned(),
                );
            }
            LcdCamMode::Camera => {
                lines
                    .push("The sensor streams; this side supplies somewhere to put it:".to_owned());
                lines.push(String::new());
                lines.push("    use esp_hal::dma_rx_stream_buffer;".to_owned());
                lines.push("    let buf = dma_rx_stream_buffer!(20 * 1000, 1000);".to_owned());
                lines.push("    let t = _cam.receive(buf).map_err(|e| e.0).unwrap();".to_owned());
                lines.push("    while !t.is_done() {}".to_owned());
                lines.push("    let (_cam, _buf) = t.stop();".to_owned());
                lines.push("    // No `await` even on async: esp-hal gives the camera".to_owned());
                lines.push("    // transfer `is_done` and `stop`, nothing to wait on.".to_owned());
            }
        }
    }
    let title = if halves.len() > 1 {
        "Using LCD_CAM (both halves)".to_owned()
    } else {
        format!(
            "Using LCD_CAM ({})",
            halves
                .first()
                .map(|(_, _, c, _)| c.mode.label())
                .unwrap_or("")
        )
    };
    example_block(&title, &lines)
}

// ── Touch ───────────────────────────────────────────────────────────────

/// The capacitive touch controller and the pads wired to it.
///
/// # It hands the controller back too
///
/// `TouchPad::new` takes a `&Touch` and does not keep it — but the `Touch` owns
/// the peripheral singleton, and dropping it would put the controller away
/// while the pads are still being read. So `init` returns it alongside them,
/// for the same reason the MCPWM one returns its timer.
///
/// # The pads are generic
///
/// A `TouchPad`'s type names its GPIO, so ten wired pads would be ten different
/// concrete types to write out. One type parameter each says the same thing and
/// lets `main.rs` pass whatever pads the canvas chose.
///
/// # Async is continuous-only
///
/// esp-hal has `Touch<Continuous, Async>` and no one-shot equivalent: waiting
/// for a touch needs something measuring while you wait. A one-shot module on
/// the async runtime therefore gets `init` alone, and this file says so.
fn touch_file(pads: &[u8], cfg: &TouchModuleConfig, rt: EspRuntime) -> String {
    let consts = format!(
        "const THRESHOLD_MODE: ThresholdMode = ThresholdMode::{};\n\
         const MEASUREMENT_DURATION: u16 = {};\n\
         {}// The count that means \"touched\". There is no right value: read your\n\
         // own pad untouched and take a margin off it.\n\
         pub const THRESHOLD: u16 = {};\n\
         // What the driver is handed. `None` anywhere here means esp-hal's own\n\
         // default for that field.\n\
         const CONFIG: TouchConfig = TouchConfig {{\n\
         \x20   threshold_mode: Some(THRESHOLD_MODE),\n\
         \x20   measurement_duration: Some(MEASUREMENT_DURATION),\n\
         \x20   sleep_cycles: {},\n\
         }};\n",
        cfg.threshold_mode.token(),
        cfg.measurement_duration.max(1),
        if cfg.scan.is_continuous() {
            format!("const SLEEP_CYCLES: u16 = {};\n", cfg.sleep_cycles.max(1))
        } else {
            String::new()
        },
        cfg.threshold.max(1),
        // The sleep timer only exists in continuous mode.
        if cfg.scan.is_continuous() {
            "Some(SLEEP_CYCLES)"
        } else {
            "None"
        },
    );

    // One type parameter and one argument per wired pad, in channel order.
    let generics: String = pads
        .iter()
        .map(|n| format!("P{n}: TouchPin + 'd, "))
        .collect();
    let params: String = pads
        .iter()
        .map(|n| format!("\x20   pad{n}: P{n},\n"))
        .collect();
    let marker = cfg.scan.marker();

    let build = |fname: &str, dm: &str, ctor: &str, extra_arg: &str, extra_param: &str| {
        let ret: String = pads
            .iter()
            .map(|n| format!(", TouchPad<P{n}, {marker}, {dm}>"))
            .collect();
        let lets: String = pads
            .iter()
            .map(|n| format!("\x20   let touch{n} = TouchPad::new(pad{n}, &touch);\n"))
            .collect();
        let names: String = pads.iter().map(|n| format!(", touch{n}")).collect();
        format!(
            "pub fn {fname}<'d, {generics}>(\n\
             \x20   touch: TOUCH<'d>,\n\
             {extra_param}{params}) -> (Touch<'d, {marker}, {dm}>{ret}) {{\n\
             \x20   let touch = Touch::{ctor}(touch{extra_arg}, Some(CONFIG));\n\
             {lets}\
             \x20   (touch{names})\n\
             }}\n"
        )
    };

    let mut body = format!(
        "/// The touch controller, and one handle per pad.\n\
         ///\n\
         /// The CONTROLLER comes back with the pads on purpose: it owns the\n\
         /// peripheral, and dropping it would stop the pads reading.\n\
         {}",
        build("init", "Blocking", cfg.scan.ctor(), "", ""),
    );
    if rt == EspRuntime::Async {
        if cfg.scan.is_continuous() {
            body.push_str(&format!(
                "\n/// The same, async: the pads gain `wait_for_touch(THRESHOLD).await`.\n\
                 ///\n\
                 /// Takes the RTC because the driver hangs its interrupt off it.\n\
                 /// Anything else that installs an RTC handler breaks this.\n\
                 {}",
                build(
                    "init_async",
                    "Async",
                    "async_mode",
                    ", rtc",
                    "\x20   rtc: &mut Rtc<'_>,\n",
                ),
            ));
        } else {
            body.push_str(
                "\n// No `init_async`: esp-hal has `Touch<Continuous, Async>` and no\n\
                 // one-shot twin - waiting for a touch needs something measuring\n\
                 // while you wait. Switch the module to Continuous for that.\n",
            );
        }
    }

    let first = pads.first().copied().unwrap_or(0);
    let mut lines = vec!["A reading is a 16-bit COUNT, not a yes/no:".to_owned()];
    lines.push(String::new());
    if cfg.scan.is_continuous() && rt == EspRuntime::Async {
        lines.push(format!(
            "    _touch{first}.wait_for_touch(pins::configs::touch::THRESHOLD).await;"
        ));
        lines.push("    // No `read()` here: esp-hal puts it on the BLOCKING pad".to_owned());
        lines.push("    // only, so an async pad waits rather than polls.".to_owned());
    } else {
        if !cfg.scan.is_continuous() {
            lines.push(format!(
                "    _touch{first}.start_measurement();   // one-shot: nothing without this"
            ));
        }
        lines.push(format!("    let n = _touch{first}.read();"));
        lines.push(
            "    let _touched = n < pins::configs::touch::THRESHOLD;   // see THRESHOLD_MODE"
                .to_owned(),
        );
    }
    lines.push(String::new());
    lines.push("// Calibrate against YOUR pad: read it untouched, take a margin".to_owned());
    lines.push("// off, and put that in the module.".to_owned());
    let example = example_block("Using touch", &lines);

    let rtc_use = if rt == EspRuntime::Async && cfg.scan.is_continuous() {
        "use esp_hal::rtc_cntl::Rtc;\n"
    } else {
        ""
    };
    let async_use = if rt == EspRuntime::Async && cfg.scan.is_continuous() {
        "use esp_hal::Async;\n"
    } else {
        ""
    };
    file(
        &format!(
            "use esp_hal::Blocking;\n\
             {async_use}\
             use esp_hal::gpio::TouchPin;\n\
             use esp_hal::peripherals::TOUCH;\n\
             {rtc_use}\
             use esp_hal::touch::{{{marker}, ThresholdMode, Touch, TouchConfig, TouchPad}};\n"
        ),
        &consts,
        &body,
        &example,
    )
}

// ── TWAI (CAN) ──────────────────────────────────────────────────────────

/// The TWAI controller - Espressif's name for CAN 2.0.
///
/// # Two steps, not one
///
/// `TwaiConfiguration::new` does not give you a driver: it gives you a
/// CONFIGURATION, which `start()` then turns into a `Twai`. Everything that can
/// only be set while the controller is off the bus - the acceptance filter
/// above all - happens in between, which is why `init` is two statements.
///
/// # RX comes first
///
/// The constructor takes `(rx, tx)` in that order, the opposite of how the pads
/// are usually spoken about. Swapping them compiles perfectly and produces a
/// node that never hears anything, so both ends name their parameters.
///
/// # Only four bit rates
///
/// esp-hal ships timings for 125k, 250k, 500k and 1M. `BaudRate::Custom` takes
/// a five-field `TimingConfig` computed against the APB clock - real work, and
/// wrong in a way that shows up only as bus errors, so the module offers the
/// four presets and leaves the rest to whoever edits this file.
fn twai_file(n: u8, cfg: Option<&CanModuleConfig>, rt: EspRuntime) -> String {
    let d = CanModuleConfig::new(1);
    let c = cfg.unwrap_or(&d);
    let baud = match c.bitrate {
        125_000 => "B125K",
        250_000 => "B250K",
        1_000_000 => "B1000K",
        // 500k is the default, and the fall-back for anything the presets do
        // not name - see the note above.
        _ => "B500K",
    };
    let consts = format!(
        "const BAUD: BaudRate = BaudRate::{baud};\n\
         const MODE: TwaiMode = TwaiMode::{};\n",
        c.mode.esp_token(),
    );

    // Two boards wired pad-to-pad have no transceiver to hold the recessive
    // level, and esp-hal has a separate constructor that accounts for it.
    let (ctor, ctor_note) = if c.transceiver {
        (
            "TwaiConfiguration::new",
            "    // A transceiver sits between these pads and the bus.\n",
        )
    } else {
        (
            "TwaiConfiguration::new_no_transceiver",
            "    // No transceiver: two boards wired TX-to-RX directly.\n",
        )
    };
    // `into_async` sits on the CONFIGURATION, so it goes in before `start`.
    let (fname, dm, tail) = match rt {
        EspRuntime::Async => ("init_async", "Async", ".into_async()"),
        EspRuntime::Blocking => ("init", "Blocking", ""),
    };

    let body = format!(
        "/// TWAI{n} - CAN 2.0 on two pads.\n\
         ///\n\
         /// RX FIRST: the constructor takes the receiving pad before the\n\
         /// transmitting one. The wrong way round still compiles.\n\
         ///\n\
         /// The filter is left at accept-all, which is what `new` installs. To\n\
         /// narrow it, call `cfg.set_filter(..)` below before `start()` - it\n\
         /// cannot be changed once the controller is on the bus.\n\
         pub fn {fname}<'d>(\n\
         \x20   twai: impl Instance + 'd,\n\
         \x20   rx: impl PeripheralInput<'d>,\n\
         \x20   tx: impl PeripheralOutput<'d>,\n\
         ) -> Twai<'d, {dm}> {{\n\
         {ctor_note}\
         \x20   let cfg = {ctor}(twai, rx, tx, BAUD, MODE){tail};\n\
         \x20   cfg.start()\n\
         }}\n"
    );

    let traffic = match rt {
        EspRuntime::Async => vec![
            format!("    _twai{n}.transmit_async(&frame).await.unwrap();"),
            format!("    let _got = _twai{n}.receive_async().await.unwrap();"),
        ],
        EspRuntime::Blocking => vec![
            "    // Both are `nb`: Err(WouldBlock) rather than a wait.".to_owned(),
            format!("    while _twai{n}.transmit(&frame).is_err() {{}}"),
            format!("    if let Ok(_got) = _twai{n}.receive() {{ /* a frame arrived */ }}"),
        ],
    };
    let mut lines = vec![
        "A frame carries an id and up to 8 bytes:".to_owned(),
        String::new(),
        "    use esp_hal::twai::{EspTwaiFrame, StandardId};".to_owned(),
        "    let id = StandardId::new(0x123).unwrap();".to_owned(),
        "    let frame = EspTwaiFrame::new(id, &[1, 2, 3]).unwrap();".to_owned(),
    ];
    lines.extend(traffic);
    lines.push(String::new());
    lines.push("// Nothing leaves the controller until another node acknowledges".to_owned());
    lines.push("// it - one board alone on the bus needs Self-test mode instead.".to_owned());
    let example = example_block(&format!("Using TWAI{n}"), &lines);

    file(
        &format!(
            "use esp_hal::{dm};\n\
             use esp_hal::gpio::interconnect::{{PeripheralInput, PeripheralOutput}};\n\
             use esp_hal::twai::{{BaudRate, Instance, Twai, TwaiConfiguration, TwaiMode}};\n"
        ),
        &consts,
        &body,
        &example,
    )
}

// ── PCNT ────────────────────────────────────────────────────────────────────

/// One PCNT unit: its limits, its filter, and what each edge means on each of
/// its two channels.
///
/// # It hands the unit back
///
/// Unlike the buses, there is no driver object to keep — the unit IS the
/// counter, and `main.rs` reads `.counter` off it. So `init` takes the unit,
/// configures it, and returns it.
///
/// # Two channels, one counter
///
/// A unit has `channel0` and `channel1`, each with its own edge pad, its own
/// optional control pad, and its own answer to what an edge means at each
/// control level. They add into the SAME counter, which is what makes a
/// quadrature encoder possible: one channel per phase, with opposite rules.
fn pcnt_file(
    n: u8,
    chans: &[(u8, bool)],
    cfg: Option<&PcntModuleConfig>,
    rt: EspRuntime,
) -> String {
    let d = PcntModuleConfig::new(n);
    let c = cfg.unwrap_or(&d);
    let any_ctrl = chans.iter().any(|(_, has_ctrl)| *has_ctrl);
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

    // One edge parameter per wired channel, each followed by its control pad
    // when there is one — the order `main.rs` passes them in.
    let params: String = chans
        .iter()
        .map(|(ch, has_ctrl)| {
            let mut s = format!("\x20   edge{ch}: impl PeripheralInput<'d>,\n");
            if *has_ctrl {
                s.push_str(&format!("\x20   ctrl{ch}: impl PeripheralInput<'d>,\n"));
            }
            s
        })
        .collect();

    let mut steps = String::from(
        "\x20   unit.set_low_limit(Some(LOW_LIMIT)).unwrap();\n\
         \x20   unit.set_high_limit(Some(HIGH_LIMIT)).unwrap();\n",
    );
    if c.filter > 0 {
        steps.push_str("\x20   unit.set_filter(Some(FILTER)).unwrap();\n");
    }
    steps.push_str("\x20   unit.clear();\n");

    for (ch, has_ctrl) in chans {
        let k = c.channel(*ch);
        steps.push_str(&format!(
            "\n\x20   let channel{ch} = &unit.channel{ch};\n\
             \x20   channel{ch}.set_edge_signal(edge{ch});\n"
        ));
        if *has_ctrl {
            steps.push_str(&format!("\x20   channel{ch}.set_ctrl_signal(ctrl{ch});\n"));
        }
        // The vendor's own argument order, and it is NOT the obvious one:
        // `set_input_mode` takes the FALLING edge first.
        steps.push_str(&format!(
            "\x20   channel{ch}.set_input_mode(EdgeMode::{}, EdgeMode::{}); // (falling, rising)\n",
            k.neg_edge.esp_hal(),
            k.pos_edge.esp_hal(),
        ));
        if *has_ctrl {
            steps.push_str(&format!(
                "\x20   channel{ch}.set_ctrl_mode(CtrlMode::{}, CtrlMode::{}); // (low, high)\n",
                k.ctrl_low.esp_hal(),
                k.ctrl_high.esp_hal(),
            ));
        }
    }

    let body = format!(
        "/// PCNT unit {n} — a hardware pulse counter on {} channel{}.\n\
         ///\n\
         /// `main.rs` builds the one `Pcnt` and lends this unit in; it comes back\n\
         /// configured, and the count is read off `.counter`. Both channels add\n\
         /// into that ONE counter.\n\
         pub fn init<'d, const U: usize>(\n\
         \x20   unit: Unit<'d, U>,\n\
         {params}) -> Unit<'d, U> {{\n\
         {steps}\n\x20   unit\n\
         }}\n",
        chans.len(),
        if chans.len() == 1 { "" } else { "s" },
    );

    let example = example_for(
        &format!("Using PCNT unit {n}"),
        &format!("_pcnt{n}"),
        &[
            "The counter runs on its own; read it whenever you like:",
            "",
            "    let _count = {H}.counter.get();",
            "",
            "    // Start again from zero - `clear` is on the UNIT, so it",
            "    // resets what BOTH channels have added.",
            "    {H}.clear();",
            "",
            "// Reaching a limit CLEARS the counter and raises an event, so a",
            "// total wider than 16 bits is accumulated by listening for it.",
        ],
        &[
            "The counter runs on its own; read it between .awaits:",
            "",
            "    let _count = {H}.counter.get();",
            "    {H}.clear();",
        ],
        rt,
    );

    file(
        &format!(
            "use esp_hal::gpio::interconnect::PeripheralInput;\n\
             use esp_hal::pcnt::channel::{{{}}};\n\
             use esp_hal::pcnt::unit::Unit;\n",
            if any_ctrl {
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
    // `(unit, its wired channels as (channel, has a control pad))` — a unit has
    // two, and which of them are wired decides the generated signature.
    pcnt: &[(u8, Vec<(u8, bool)>)],
    pcnt_cfg: &BTreeMap<u8, PcntModuleConfig>,
    // True when the USB pads are wired AND this chip's esp-hal has the driver
    // — see `codegen_esp::has_usb_serial_jtag`.
    usb: bool,
    // The USB module, whose ROLE decides which of the two controllers the pads
    // are routed to — see `modules::UsbRole`.
    usb_cfg: Option<&UsbModuleConfig>,
    // The touch channels wired, and the module that configures them. Empty
    // means no touch controller on this canvas.
    touch: &[u8],
    touch_cfg: Option<&TouchModuleConfig>,
    // True when BOTH TWAI pads are wired and the chip has the driver — a CAN
    // node with one wire is not a node, so a half-wired bus emits nothing.
    twai: bool,
    can_cfg: Option<&CanModuleConfig>,
    // `(unit, its wired outputs as (operator, is B))`.
    mcpwm: &[(u8, Vec<(u8, bool)>)],
    mcpwm_cfg: &BTreeMap<u8, McpwmModuleConfig>,
    // The MCPWM peripheral clock, in MHz. 40 everywhere but the H2, which is 32.
    mcpwm_source_mhz: u32,
    // `Some(has a valid pad)` when the SENDING half of the parallel port is
    // wired, and the module that configures it.
    parl_io: &Option<bool>,
    parl_io_cfg: Option<&ParlIoModuleConfig>,
    // The RECEIVING half, wired and configured on its own. Both halves run at
    // once, off one peripheral and one DMA channel.
    parl_rx: &Option<bool>,
    parl_rx_cfg: Option<&ParlIoModuleConfig>,
    // The DISPLAY half's signal names wired, and the module that gives them
    // meaning. Empty means no display on this canvas.
    lcd_cam: &[String],
    lcd_cam_cfg: Option<&LcdCamModuleConfig>,
    // The CAMERA half, wired and configured on its own: the two halves run at
    // the same time, so one file has to build whichever of them exist.
    cam: &[String],
    cam_cfg: Option<&LcdCamModuleConfig>,
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
    for (n, chans) in pcnt {
        out.push((
            format!("pcnt{n}.rs"),
            pcnt_file(*n, chans, pcnt_cfg.get(n), rt),
        ));
    }
    if usb {
        out.push(("usb.rs".to_owned(), usb_file(rt, usb_cfg)));
    }
    // One file for both halves: `LcdCam::new` consumes the peripheral once, so
    // two independent `init`s could not exist even if they read better.
    if !lcd_cam.is_empty() || !cam.is_empty() {
        let lcd_default = LcdCamModuleConfig::new(0);
        let cam_default = LcdCamModuleConfig::new_camera();
        let lcd_sigs: Vec<&str> = lcd_cam.iter().map(String::as_str).collect();
        let cam_sigs: Vec<&str> = cam.iter().map(String::as_str).collect();
        out.push((
            "lcd_cam.rs".to_owned(),
            lcd_cam_file(
                &lcd_sigs,
                Some(lcd_cam_cfg.unwrap_or(&lcd_default)),
                &cam_sigs,
                Some(cam_cfg.unwrap_or(&cam_default)),
                rt,
            ),
        ));
    }
    // TWAI0 alone: `PinFunction::CanTx`/`CanRx` carry no instance number, so
    // the C6's second controller has no way to be wired on the canvas.
    if !touch.is_empty() {
        let d = TouchModuleConfig::new(0);
        out.push((
            "touch.rs".to_owned(),
            touch_file(touch, touch_cfg.unwrap_or(&d), rt),
        ));
    }
    if twai {
        out.push(("twai0.rs".to_owned(), twai_file(0, can_cfg, rt)));
    }
    if !dac.is_empty() {
        out.push(("dac.rs".to_owned(), dac_file(dac, dac_cfg, rt)));
    }
    if parl_io.is_some() || parl_rx.is_some() {
        let d = ParlIoModuleConfig::new(0);
        let dr = ParlIoModuleConfig::new_rx();
        out.push((
            "parl_io.rs".to_owned(),
            parl_io_file(
                parl_io.map(|v| (parl_io_cfg.unwrap_or(&d), v)),
                parl_rx.map(|v| (parl_rx_cfg.unwrap_or(&dr), v)),
                rt,
            ),
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
            // Unit 1: channel 0 with a control pad, channel 1 without.
            &[(1, vec![(0, true), (1, false)])],
            &BTreeMap::new(),
            true,
            // The USB module left at its defaults — Serial/JTAG, not OTG.
            None,
            // Two touch pads wired, module at its defaults.
            &[0, 5],
            None,
            // TWAI wired, with the module left at its defaults.
            true,
            None,
            &[(0, vec![(0, false), (0, true)])],
            &BTreeMap::new(),
            40,
            &Some(false),
            None,
            // …and the receiving half, unwired: one file, one half.
            &None,
            None,
            // An 8-bit i8080 display: DC, WR and D0..D7 wired.
            &[
                "dc".to_owned(),
                "wr".to_owned(),
                "d0".to_owned(),
                "d1".to_owned(),
                "d2".to_owned(),
                "d3".to_owned(),
                "d4".to_owned(),
                "d5".to_owned(),
                "d6".to_owned(),
                "d7".to_owned(),
            ],
            None,
            // …and a camera on the other half at the same time, which is the
            // arrangement this peripheral exists for.
            &["cd0".to_owned(), "cd1".to_owned(), "href".to_owned()],
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
                "lcd_cam.rs",
                "touch.rs",
                "twai0.rs",
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
