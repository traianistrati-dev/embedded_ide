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
    I2cModuleConfig, Parity, SpiModuleConfig, StopBits, TimerModuleConfig, UsartModuleConfig,
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

// ── SPI ──────────────────────────────────────────────────────────────────────

fn spi_file(n: u8, sigs: &[&str], cfg: Option<&SpiModuleConfig>, rt: EspRuntime) -> String {
    let consts = format!(
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
    if rt == EspRuntime::Async {
        body.push_str(&async_twin(
            &format!("SPI{n} master — async driver."),
            &params,
            "Spi",
            ctor,
            &chain,
        ));
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
             use esp_hal::gpio::interconnect::{{PeripheralInput, PeripheralOutput}};\n\
             use esp_hal::spi::Mode;\n\
             use esp_hal::spi::master::{{Config, Instance, Spi}};\n\
             use esp_hal::time::Rate;\n",
            mode_import(rt)
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
    usart_cfg: &BTreeMap<u8, UsartModuleConfig>,
    spi_cfg: &BTreeMap<u8, SpiModuleConfig>,
    i2c_cfg: &BTreeMap<u8, I2cModuleConfig>,
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
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[(0, vec![(1, 2_000)])],
            &BTreeMap::new(),
            EspRuntime::Blocking,
        );
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["uart1.rs", "spi2.rs", "i2c0.rs", "pwm0.rs"]);
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
