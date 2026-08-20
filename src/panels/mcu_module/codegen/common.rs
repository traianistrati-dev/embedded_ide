// ── Section markers ───────────────────────────────────────────────────────────
//
// The GEN_BEGIN … GEN_END block is auto-generated and replaced whenever the
// pin configuration changes.  It includes the HAL use items, any peripheral
// helper functions, the #[entry] attribute, and the opening of fn main().
// The block is intentionally left open — USER_TAIL closes main() with the
// user-editable loop body, which is preserved across every regen.

pub const GEN_BEGIN: &str = "// <<< GENERATED BEGIN — do not edit between these markers >>>";
pub const GEN_END: &str = "// <<< GENERATED END >>>";

// ── MCU identity marker ───────────────────────────────────────────────────────
//
// Written into the invariant file header (above GEN_BEGIN, so it survives every
// re-splice). Lets a reopened project restore the *exact* chip it was created
// with — including user-imported chips that share a HAL crate with a built-in
// (e.g. an imported "esp32c3-graph" vs the built-in "esp32c3"), which the
// Cargo.toml `hal_dep` sniff alone cannot tell apart.

pub const MCU_ID_MARKER: &str = "// embedded-ide:mcu=";

/// The header line that records the MCU id, or an empty string when the id is
/// unknown (so older/unidentified projects emit nothing).
pub fn mcu_id_marker_line(id: &str) -> String {
    if id.is_empty() {
        String::new()
    } else {
        format!("{MCU_ID_MARKER}{id}\n")
    }
}

/// Extract the MCU id recorded by [`mcu_id_marker_line`], if present.
pub fn parse_mcu_id(source: &str) -> Option<String> {
    source.lines().find_map(|l| {
        l.trim()
            .strip_prefix(MCU_ID_MARKER)
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty())
    })
}

// ── Hand-written clock block ──────────────────────────────────────────────────
//
// The clock setup sits INSIDE the generated section, so it is normally replaced
// on every regeneration like everything else there. A chip whose family has no
// RCC recipe cannot have that setup generated at all, though — the tree in the
// Clock tab has nothing to be turned into. For those the block is written by
// hand, and these markers carve out the one region of the generated section that
// survives a regen.
//
// They are emitted ONLY in manual mode, so every project that lets the IDE
// generate its clock keeps exactly the output it had.

pub const CLOCK_BEGIN: &str = "    // <<< CLOCK BEGIN — hand-written, kept across regeneration >>>";
pub const CLOCK_END: &str = "    // <<< CLOCK END >>>";

/// The hand-written clock region of `source`, markers included.
pub fn clock_region(source: &str) -> Option<&str> {
    let begin = source.find(CLOCK_BEGIN)?;
    let end = source[begin..].find(CLOCK_END)? + begin + CLOCK_END.len();
    Some(&source[begin..end])
}

/// Carry the user's hand-written clock block from `existing` into `section`.
///
/// Only in manual mode, and only when both sides actually have the region:
/// - not manual → `section` is returned untouched, so generated projects are
///   byte-for-byte what they were;
/// - manual but the old file has no region → the freshly generated block stays,
///   which is exactly the seed the user then edits;
/// - manual and both have one → the user's text wins.
pub fn keep_manual_clock(existing: &str, section: String, manual: bool) -> String {
    if !manual {
        return section;
    }
    let (Some(old), Some(new)) = (clock_region(existing), clock_region(&section)) else {
        return section;
    };
    section.replace(new, old)
}

// ── User tail — closes fn main() ─────────────────────────────────────────────
//
// Written once on first generation; the loop body is user-editable and is
// preserved across every pin-configuration change.

pub const USER_TAIL: &str = "    loop {\n        // Your main loop code here.\n    }\n}\n";

// ── Strict-lints exemption for generated code ─────────────────────────────────
//
// When the MCU System "Strict lints" toggle is on, the project Cargo.toml gets a
// `[lints.clippy]` deny profile (see `project_gen::ensure_strict_lints`). The
// GENERATED code (main's init, the peripheral `configs/*.rs`) uses `unwrap()`,
// `as`, indexing, … idiomatically, so it is exempted with `#[allow]` — leaving
// only the USER's own code (their modules, and main's loop below the GEN block —
// no, the whole entry fn is exempt since its init bindings must stay in scope)
// under the strict lints.

/// The strict-profile clippy lints as `#[allow]`-able names. Matches the deny
/// list in `project_gen::STRICT_LINTS_BLOCK`.
const STRICT_LINT_LIST: &str = "clippy::pedantic, clippy::nursery, \
     clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, \
     clippy::arithmetic_side_effects, clippy::unreachable, clippy::unimplemented, \
     clippy::unchecked_time_subtraction, clippy::todo, clippy::string_slice, \
     clippy::panic_in_result_fn, clippy::panic, clippy::exit, clippy::as_conversions";

/// When `strict`, put `#[allow(<strict lints>)]` on the generated entry fn so
/// its init (`take().unwrap()`, `as` casts, …) doesn't flood clippy. Inserted
/// just before `#[entry]` / `#[embassy_executor::main]` — inside the GEN block,
/// so it's rebuilt on every regeneration (no accumulation). The whole `main` is
/// exempt: its init bindings must stay in scope for the user's loop, so the loop
/// can't be split off; the user's real code lives in their own modules, which
/// stay fully linted. No-op when `strict` is off or no entry attr is found.
pub fn strict_main_exemption(code: String, strict: bool) -> String {
    if !strict {
        return code;
    }
    for entry in [
        "#[entry]",
        "#[embassy_executor::main]",
        "#[esp_hal::main]",
        "#[main]",
    ] {
        let mut offset = 0;
        for line in code.split_inclusive('\n') {
            if line.trim() == entry {
                let attr = format!("#[allow({STRICT_LINT_LIST})]\n");
                let mut out = String::with_capacity(code.len() + attr.len());
                out.push_str(&code[..offset]);
                out.push_str(&attr);
                out.push_str(&code[offset..]);
                return out;
            }
            offset += line.len();
        }
    }
    code
}

/// When `strict`, put a module-level `#![allow(<strict lints>)]` right after a
/// config file's `// <<< GENERATED>>>` marker (before the first `const`), so the
/// whole generated peripheral module is exempt. Inside the marker block, so
/// `sync_config_files` re-splices it in/out on toggle. No-op otherwise.
pub fn strict_config_exemption(body: String, strict: bool) -> String {
    const MARK: &str = "// <<< GENERATED>>>";
    if !strict {
        return body;
    }
    let Some(pos) = body.find(MARK) else {
        return body;
    };
    let after = pos + MARK.len();
    let insert_at = body[after..]
        .find('\n')
        .map(|n| after + n + 1)
        .unwrap_or(after);
    let attr = format!("#![allow({STRICT_LINT_LIST})]\n");
    let mut out = String::with_capacity(body.len() + attr.len());
    out.push_str(&body[..insert_at]);
    out.push_str(&attr);
    out.push_str(&body[insert_at..]);
    out
}

// ── Virtual-module data models ────────────────────────────────────────────────

use super::super::modules::VirtualModule;

fn indent_block(s: &str) -> String {
    s.lines()
        .map(|l| {
            if l.trim().is_empty() {
                "\n".to_owned()
            } else {
                format!("    {l}\n")
            }
        })
        .collect()
}

/// Append each module's RX/TX data model as an inline `mod <id> { … }` at the end
/// of `main.rs` (family-agnostic). Additive: a module already present (matched by
/// `mod <id>`) is left untouched, so edits survive every regeneration — and a
/// module with an empty data model emits nothing. The module's id is a valid Rust
/// identifier (e.g. `_usart_1`), so its types are reachable as `_usart_1::…`.
pub fn ensure_module_models(mut file: String, modules: &[VirtualModule]) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for m in modules {
        let (rx, tx) = (m.config.rx_model(), m.config.tx_model());
        if rx.trim().is_empty() && tx.trim().is_empty() {
            continue;
        }
        if file.contains(&format!("mod {} ", m.id)) || file.contains(&format!("mod {}{{", m.id)) {
            continue;
        }
        let mut body = String::new();
        if !rx.trim().is_empty() {
            body.push_str("    // ── RX data model ──\n");
            body.push_str(&indent_block(rx));
        }
        if !tx.trim().is_empty() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str("    // ── TX data model ──\n");
            body.push_str(&indent_block(tx));
        }
        blocks.push(format!(
            "\n// Data model for {} (editable — kept across regeneration)\nmod {} {{\n{body}}}\n",
            m.name, m.id,
        ));
    }
    if blocks.is_empty() {
        return file;
    }
    if !file.ends_with('\n') {
        file.push('\n');
    }
    for b in blocks {
        file.push_str(&b);
    }
    file
}

// ── Variable name suffix ──────────────────────────────────────────────────────

use super::super::pins::logic::pin_function::PinFunction;

/// The `<type>` half of a generated binding name `<pin>_<type>`, e.g.
/// `out` / `in` / `i2c1_sda` / `spi2_sck` / `usart1_tx` / `adc1_in0`. So a
/// PC13 output binds as `pc13_out`, a PB9 I2C1 SDA as `pb9_i2c1_sda`.
pub fn var_suffix(func: &PinFunction) -> String {
    match func {
        PinFunction::GpioOutput => "out".into(),
        PinFunction::GpioInput => "in".into(),
        PinFunction::GpioAnalog => "analog".into(),
        PinFunction::AdcChannel { adc, channel } => format!("adc{adc}_in{channel}"),
        PinFunction::TimerPwm { timer, channel } => format!("tim{timer}_ch{channel}"),
        PinFunction::UsartTx(n) => format!("usart{n}_tx"),
        PinFunction::UsartRx(n) => format!("usart{n}_rx"),
        PinFunction::UsartCts(n) => format!("usart{n}_cts"),
        PinFunction::UsartRts(n) => format!("usart{n}_rts"),
        PinFunction::UsartCk(n) => format!("usart{n}_ck"),
        PinFunction::LpuartTx(n) => format!("lpuart{n}_tx"),
        PinFunction::LpuartRx(n) => format!("lpuart{n}_rx"),
        PinFunction::LpuartCts(n) => format!("lpuart{n}_cts"),
        PinFunction::LpuartRts(n) => format!("lpuart{n}_rts"),
        PinFunction::SpiSck(n) => format!("spi{n}_sck"),
        PinFunction::SpiMosi(n) => format!("spi{n}_mosi"),
        PinFunction::SpiMiso(n) => format!("spi{n}_miso"),
        PinFunction::SpiNss(n) => format!("spi{n}_nss"),
        PinFunction::SpiRdy(n) => format!("spi{n}_rdy"),
        PinFunction::I2cScl(n) => format!("i2c{n}_scl"),
        PinFunction::I2cSda(n) => format!("i2c{n}_sda"),
        PinFunction::UsbDm => "usb_dm".into(),
        PinFunction::UsbDp => "usb_dp".into(),
        PinFunction::CanRx => "can_rx".into(),
        PinFunction::CanTx => "can_tx".into(),
        PinFunction::SwdIo => "swd_io".into(),
        PinFunction::SwdClk => "swd_clk".into(),
        PinFunction::Mco => "mco".into(),
        // Generic AF: the signal name lowercased is already a valid Rust
        // identifier fragment (`SAI1_SD_A` → `sai1_sd_a`); `-` (as in
        // `JTDO-TRACESWO`) becomes `_`.
        PinFunction::Other(name) => name.to_ascii_lowercase().replace('-', "_"),
        PinFunction::Unset => "unset".into(),
    }
}

// ── Custom label → binding name ───────────────────────────────────────────────

/// Sanitize a user-typed pin label into a Rust-identifier fragment: lowercase
/// ASCII alphanumerics kept, every other run collapsed to a single `_`, with
/// leading/trailing `_` trimmed. Returns "" when nothing usable remains.
///
/// e.g. `"Status LED"` → `status_led`, `"  D7! "` → `d7`.
/// A hundredths-of-a-percent duty as a plain percentage: `750` -> `"7.5"`,
/// `7525` -> `"75.25"`, `10_000` -> `"100"`.
///
/// Trailing zeros are trimmed, because a generated comment reading "75.00 %"
/// only adds noise to the common case.
pub fn duty_percent_str(x100: u16) -> String {
    let whole = x100 / 100;
    match x100 % 100 {
        0 => format!("{whole}"),
        r if r % 10 == 0 => format!("{whole}.{}", r / 10),
        r => format!("{whole}.{r:02}"),
    }
}

pub fn sanitize_label(label: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            pending_sep = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_sep = true;
        }
    }
    out
}

/// Full generated binding name: `<base>_<type>` with the user's sanitized label
/// appended as `_<label>` when present. So a `pc13` output (base `pc13`) labelled
/// "led" binds as `pc13_out_led`; with no label it stays `pc13_out`.
pub fn pin_binding(base_var: &str, func: &PinFunction, custom_label: &str) -> String {
    let mut s = format!("{}_{}", base_var, var_suffix(func));
    let extra = sanitize_label(custom_label);
    if !extra.is_empty() {
        s.push('_');
        s.push_str(&extra);
    }
    s
}

/// One per-pin `let` binding found inside a generated `main.rs` GEN block.
pub struct GenBinding<'a> {
    /// 1-based line number in the FULL source (not just the block).
    pub line: usize,
    /// The binding variable, e.g. `pc13_out_led`.
    pub var: &'a str,
    /// MCU pin the binding belongs to, e.g. `PC13` / `GPIO20`.
    pub pin_name: String,
    /// The trimmed source line.
    pub text: &'a str,
}

/// Scan the GEN block of a generated `main.rs` for its per-pin `let` bindings.
///
/// Covers both shapes the backends emit — `let [mut] pXY… = …` (STM32 blocking
/// and embassy) and `let [mut] gpioNN… = …` (ESP) — and, importantly, `let mut`
/// as well as plain `let`: embassy and ESP bind every OUTPUT as `let mut`.
///
/// This is the single place that knows what a generated binding line looks like;
/// [`parse_pin_labels`] and [`find_pin_binding_line`] both read through it, so a
/// lookup can't drift away from the generator.
pub fn gen_let_bindings(source: &str) -> Vec<GenBinding<'_>> {
    let (Some(begin_pos), Some(end_pos)) = (source.find(GEN_BEGIN), source.find(GEN_END)) else {
        return vec![];
    };
    if begin_pos >= end_pos {
        return vec![];
    }
    // Lines fully before the block — the offset that turns a block-local index
    // into an absolute 1-based line number.
    let base_line = source[..begin_pos].lines().count();

    let mut out = Vec::new();
    for (i, line) in source[begin_pos..end_pos].lines().enumerate() {
        let trimmed = line.trim();
        let after_let = match trimmed.strip_prefix("let mut ") {
            Some(r) => r,
            None => match trimmed.strip_prefix("let ") {
                Some(r) => r,
                None => continue,
            },
        };
        let Some(eq_pos) = after_let.find(" =") else {
            continue;
        };
        let var = after_let[..eq_pos].trim();

        // `pXY…` → port letter + number; `gpioNN…` → number.
        let pin_name = if let Some(rest) = var.strip_prefix("gpio") {
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if num.is_empty() {
                continue;
            }
            format!("GPIO{num}")
        } else if var.len() >= 3 && var.starts_with('p') {
            let port_lc = match var.chars().nth(1) {
                Some(c) if c.is_ascii_lowercase() => c,
                _ => continue,
            };
            let num: String = var[2..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if num.is_empty() {
                continue;
            }
            format!("P{}{}", port_lc.to_ascii_uppercase(), num)
        } else {
            continue;
        };

        out.push(GenBinding {
            line: base_line + i + 1,
            var,
            pin_name,
            text: trimmed,
        });
    }
    out
}

/// Line (1-based) + binding name of a pin's `let` in the generated GEN block —
/// what "jump to where this pin is defined" lands on. `None` when the pin has no
/// binding of its own (unconfigured, or consumed inline by a peripheral).
pub fn find_pin_binding_line(source: &str, pin_name: &str) -> Option<(usize, String)> {
    gen_let_bindings(source)
        .into_iter()
        .find(|b| b.pin_name.eq_ignore_ascii_case(pin_name))
        .map(|b| (b.line, b.var.to_owned()))
}

/// Fallback for a pin with no `let` of its own: the first GEN-block line that
/// mentions it as a whole token — `p.PB6`, `peripherals.GPIO20`, `gpiob.pb6`.
/// ESP hands bus pins straight to their driver (`.with_rx(peripherals.GPIO20)`),
/// so that call IS the definition site.
pub fn find_pin_mention_line(source: &str, pin_name: &str) -> Option<usize> {
    let (Some(begin_pos), Some(end_pos)) = (source.find(GEN_BEGIN), source.find(GEN_END)) else {
        return None;
    };
    if begin_pos >= end_pos {
        return None;
    }
    let base_line = source[..begin_pos].lines().count();
    let upper = pin_name.to_ascii_uppercase();
    let lower = pin_name.to_ascii_lowercase();
    // Whole-token match, so `GPIO2` never lights up on `GPIO20` and the bare
    // `pb6` never matches the binding `pb6_out` (that one is case 1's job).
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let hit = |line: &str, needle: &str| {
        let mut from = 0;
        while let Some(rel) = line[from..].find(needle) {
            let s = from + rel;
            let e = s + needle.len();
            let before_ok = s == 0 || !line[..s].ends_with(is_word);
            let after_ok = e >= line.len() || !line[e..].starts_with(is_word);
            if before_ok && after_ok {
                return true;
            }
            from = s + 1;
        }
        false
    };
    source[begin_pos..end_pos]
        .lines()
        .enumerate()
        .find(|(_, l)| hit(l, &upper) || hit(l, &lower))
        .map(|(i, _)| base_line + i + 1)
}

/// Recover the user labels embedded in generated binding names, mirroring
/// [`parse_main_rs`]. Scans the GEN block's per-pin `let` bindings and, for each
/// one carrying a `<base>_<type>_<label>` suffix, returns `(pin_name, label)`.
/// Only GPIO/PWM bindings (the ones the rename field targets) can carry a label.
pub fn parse_pin_labels(source: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for b in gen_let_bindings(source) {
        let (var, pin_name, trimmed) = (b.var, b.pin_name, b.text);

        // Function from the comment, then strip the `<base>_<type>` prefix; the
        // remaining `_<label>` (if any) is the user's custom name.
        let Some(comment_pos) = trimmed.rfind("// ") else {
            continue;
        };
        let label_str = trimmed[comment_pos + 3..]
            .trim()
            .trim_end_matches(';')
            .trim();
        let Some(func) = PinFunction::from_label(label_str) else {
            continue;
        };

        let needle = format!("_{}", var_suffix(&func));
        if let Some(pos) = var.find(&needle) {
            let after_suffix = &var[pos + needle.len()..];
            if let Some(label) = after_suffix.strip_prefix('_') {
                if !label.is_empty() {
                    result.push((pin_name, label.to_owned()));
                }
            }
        }
    }
    result
}

// ── Pin state parser ──────────────────────────────────────────────────────────

/// Parses pin assignments from an existing `src/main.rs`.
///
/// Scans the GEN_BEGIN … GEN_END block for lines of the form:
/// ```text
///     let p{lc}{num} = [&mut ]{pv}.p{lc}{num}.{method}(…); // {label}
/// ```
/// Returns `(pin_name, PinFunction)` pairs (e.g. `("PC13", GpioOutput)`)
/// for every recognisable pin.  Unknown or comment-only lines are skipped.
///
/// Handles both STM32 format ("let pc13 = …") and ESP32 format ("let gpio2 = …").
///
/// The label may also sit on its OWN line, immediately above the code it
/// describes — the shape the ESP bus builders emit:
/// ```text
///     // USART0  RX
///     .with_rx(peripherals.GPIO20)
/// ```
/// They moved there because a trailing comment SWALLOWS the chain's terminating
/// `;`. Without this, every USART/SPI/I2C pin on an ESP project was lost on
/// reload: `apply_saved_pins` clears the diagram and re-applies only what parsed
/// here, so an unparsed pin comes back Unset and its Virtual Module unwired.
pub fn parse_main_rs(source: &str) -> Vec<(String, PinFunction)> {
    let Some(begin_pos) = source.find(GEN_BEGIN) else {
        return vec![];
    };
    let Some(end_pos) = source.find(GEN_END) else {
        return vec![];
    };
    if begin_pos >= end_pos {
        return vec![];
    }

    let gen_block = &source[begin_pos..end_pos];
    let mut result = Vec::new();
    // A comment-only line labels the line RIGHT below it (see the doc comment).
    // Deliberately one line of memory, not a running "last comment seen": a
    // section header like `// ── UART0 ──` must not leak onto a `.with_` line
    // three lines further down. Consumed (taken) by whatever line follows it,
    // matched or not.
    let mut pending_label: Option<String> = None;

    for line in gen_block.lines() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("//") {
            pending_label = Some(rest.trim().to_owned());
            continue;
        }
        let label_above = pending_label.take();

        // ── STM32: "let [mut ]p{port}{num} = ..." ────────────────────────────
        // trimmed = "let pc13 = &mut gpioc.pc13.into_push_pull_output(…); // …"
        // The `mut` form is emitted by the WBA (embassy) backend for outputs
        // (`let mut pb5 = Output::new(…)`) — strip it so both shapes parse.
        if trimmed.starts_with("let p") || trimmed.starts_with("let mut p") {
            let after_let = trimmed
                .strip_prefix("let mut ")
                .or_else(|| trimmed.strip_prefix("let "))
                .unwrap_or(trimmed); // "pc13 = …"
            let Some(eq_pos) = after_let.find(" =") else {
                continue;
            };
            let var = after_let[..eq_pos].trim(); // "pc13"

            // var must be p + ascii-lowercase-letter + one-or-more digits
            if var.len() < 3 || !var.starts_with('p') {
                continue;
            }
            let port_lc = match var.chars().nth(1) {
                Some(c) if c.is_ascii_lowercase() => c,
                _ => continue,
            };
            // Read the pin-number digits, stopping at the `_<type>` suffix (so
            // both `pc13` and `pc13_out` yield "13").
            let pin_num_str: String = var[2..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if pin_num_str.is_empty() {
                continue;
            }

            // "pc13" / "pc13_out" → "PC13"
            let pin_name = format!("P{}{}", port_lc.to_ascii_uppercase(), pin_num_str);

            let Some(comment_pos) = trimmed.rfind("// ") else {
                continue;
            };
            let label = trimmed[comment_pos + 3..].trim();

            if let Some(func) = PinFunction::from_label(label) {
                result.push((pin_name, func));
            }
            continue;
        }

        // ── ESP32 GPIO / ADC per-pin bindings ────────────────────────────────
        //   let mut gpio2 = Output::new(peripherals.GPIO2, Level::Low); // GPIO Output
        //   let gpio9 = Input::new(peripherals.GPIO9, Pull::None);       // GPIO Input
        //   let mut gpio0_adc = adc1_config                              // ADC1  IN0
        //       .enable_pin(peripherals.GPIO0, Attenuation::_11dB);
        //
        // STM32 port-split lines ("let mut gpioa = dp.GPIOA.split()") are also
        // caught by these guards, but they fail the "starts with digit" check below.
        if trimmed.starts_with("let mut gpio") || trimmed.starts_with("let gpio") {
            let after_let = if trimmed.starts_with("let mut ") {
                &trimmed["let mut ".len()..]
            } else {
                &trimmed["let ".len()..]
            };
            let Some(eq_pos) = after_let.find(" =") else {
                continue;
            };
            let var = after_let[..eq_pos].trim(); // "gpio2", "gpio9", "gpio0_adc"

            // Must be "gpio" + digit  →  filters out "gpioa"/"gpiob" port splits
            let gpio_rest = match var.strip_prefix("gpio") {
                Some(r) if r.starts_with(|c: char| c.is_ascii_digit()) => r,
                _ => continue,
            };

            // Read the pin-number digits, stopping at any `_<type>` suffix (so
            // `gpio2`, `gpio2_out`, `gpio0_adc1_in0` all yield the number).
            let pin_num_str: String = gpio_rest
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if pin_num_str.is_empty() {
                continue;
            }

            let pin_name = format!("GPIO{pin_num_str}"); // "GPIO2", "GPIO0"

            let Some(comment_pos) = trimmed.rfind("// ") else {
                continue;
            };
            // The function comes from the comment label (robust to the binding
            // name format). Strip a trailing ';' from single-method init lines.
            let label = trimmed[comment_pos + 3..]
                .trim()
                .trim_end_matches(';')
                .trim();
            if let Some(func) = PinFunction::from_label(label) {
                result.push((pin_name, func));
            }
            continue;
        }

        // ── ESP32 peripheral chain lines ──────────────────────────────────────
        //   .with_rx(peripherals.GPIO20)  // USART0  RX
        //   .with_tx(peripherals.GPIO21)  // USART0  TX;   ← ';' on last method
        //   .with_sck(peripherals.GPIO6)  // SPI2  SCK
        //   .with_scl(peripherals.GPIO10) // I2C0  SCL
        if trimmed.starts_with(".with_") {
            // Extract GPIO number from "peripherals.GPIO{N}"
            let Some(gpio_pos) = trimmed.find("peripherals.GPIO") else {
                continue;
            };
            let after_gpio = &trimmed[gpio_pos + "peripherals.GPIO".len()..];
            let num_str: String = after_gpio
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if num_str.is_empty() {
                continue;
            }
            let pin_name = format!("GPIO{num_str}");

            // Either shape: the label trailing the call (older projects, still
            // on disk) or sitting on the line above it (what is generated now).
            let trailing = trimmed
                .rfind("// ")
                .map(|p| trimmed[p + 3..].trim().trim_end_matches(';').trim());
            let Some(label) = trailing.or(label_above.as_deref()) else {
                continue;
            };

            if let Some(func) = PinFunction::from_label(label) {
                result.push((pin_name, func));
            }
            continue;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    /// The comment beside a generated duty reads as a person would write it.
    #[test]
    fn a_duty_reads_as_a_plain_percentage() {
        for (x100, want) in [
            (0u16, "0"),
            (750, "7.5"),  // the servo case whole percent could not express
            (7_500, "75"), // no trailing ".00" on the common case
            (7_550, "75.5"),
            (7_505, "75.05"),
            (10_000, "100"),
        ] {
            assert_eq!(duty_percent_str(x100), want, "{x100} hundredths");
        }
    }

    use super::*;

    #[test]
    fn strict_main_exemption_wraps_entry_only_when_strict() {
        let code =
            "// GEN\nuse foo;\n#[entry]\nfn main() -> ! {\n    let dp = take().unwrap();\n}\n";
        // Off → unchanged.
        assert_eq!(strict_main_exemption(code.to_string(), false), code);
        // On → an #[allow(...)] appears immediately before #[entry].
        let on = strict_main_exemption(code.to_string(), true);
        assert!(
            on.contains("#[allow(clippy::pedantic"),
            "allow added:\n{on}"
        );
        assert!(on.contains("clippy::unwrap_used"), "lints listed:\n{on}");
        let allow_pos = on.find("#[allow(").unwrap();
        let entry_pos = on.find("#[entry]").unwrap();
        assert!(allow_pos < entry_pos, "allow precedes entry:\n{on}");
        // Only one allow (no accumulation on a second pass over fresh codegen).
        assert_eq!(on.matches("#[allow(clippy::pedantic").count(), 1);
    }

    #[test]
    fn strict_main_exemption_handles_embassy_entry() {
        let code = "#[embassy_executor::main]\nasync fn main(s: Spawner) {}\n";
        let on = strict_main_exemption(code.to_string(), true);
        assert!(on.starts_with("#[allow(clippy::"), "allow first:\n{on}");
        assert!(on.contains("#[embassy_executor::main]"));
    }

    #[test]
    fn strict_config_exemption_inserts_module_allow_after_marker() {
        let body = "// <<< GENERATED>>>\nconst BAUDRATE: u32 = 115200;\n// <<< GENERATED END >>>\n\nuse foo;\n";
        assert_eq!(strict_config_exemption(body.to_string(), false), body);
        let on = strict_config_exemption(body.to_string(), true);
        // Module inner attribute, right after the marker, before the const.
        let attr = on.find("#![allow(clippy::").unwrap();
        let marker = on.find("// <<< GENERATED>>>").unwrap();
        let konst = on.find("const BAUDRATE").unwrap();
        assert!(
            marker < attr && attr < konst,
            "attr between marker and const:\n{on}"
        );
    }

    #[test]
    fn mcu_id_marker_round_trips() {
        let line = mcu_id_marker_line("esp32c3-graph");
        assert_eq!(line, "// embedded-ide:mcu=esp32c3-graph\n");
        // Embedded anywhere in a file, possibly indented, parse_mcu_id finds it.
        let src = format!("// Auto-generated\n{line}#![no_std]\n");
        assert_eq!(parse_mcu_id(&src).as_deref(), Some("esp32c3-graph"));
    }

    #[test]
    fn empty_id_emits_no_marker() {
        assert_eq!(mcu_id_marker_line(""), "");
        assert!(parse_mcu_id("// Auto-generated\n#![no_std]\n").is_none());
    }

    #[test]
    fn sanitize_label_makes_identifier_fragments() {
        assert_eq!(sanitize_label("led"), "led");
        assert_eq!(sanitize_label("Status LED"), "status_led");
        assert_eq!(sanitize_label("  D7! "), "d7");
        assert_eq!(sanitize_label("a--b__c"), "a_b_c");
        assert_eq!(sanitize_label(""), "");
        assert_eq!(sanitize_label("***"), "");
    }

    #[test]
    fn pin_binding_appends_sanitized_label() {
        // No label → plain `<base>_<type>`.
        assert_eq!(
            pin_binding("pc13", &PinFunction::GpioOutput, ""),
            "pc13_out"
        );
        // Label appended and sanitized.
        assert_eq!(
            pin_binding("pc13", &PinFunction::GpioOutput, "Status LED"),
            "pc13_out_status_led"
        );
        // ESP-style base + ADC suffix.
        assert_eq!(
            pin_binding("gpio0", &PinFunction::AdcChannel { adc: 1, channel: 0 }, ""),
            "gpio0_adc1_in0"
        );
    }

    #[test]
    fn parse_pin_labels_recovers_custom_names() {
        let src = format!(
            "{GEN_BEGIN}\n\
             let pc13_out_led = &mut gpioc.pc13.into_push_pull_output(&mut gpioc.crh); // GPIO Output\n\
             let pa1_in = &mut gpioa.pa1.into_floating_input(&mut gpioa.crl); // GPIO Input\n\
             let mut gpio2_out_relay = Output::new(peripherals.GPIO2, Level::Low); // GPIO Output\n\
             {GEN_END}"
        );
        let labels = parse_pin_labels(&src);
        // Pins with a label are recovered; the unlabelled one is absent.
        assert!(labels.contains(&("PC13".to_owned(), "led".to_owned())));
        assert!(labels.contains(&("GPIO2".to_owned(), "relay".to_owned())));
        assert!(!labels.iter().any(|(n, _)| n == "PA1"));
    }

    #[test]
    fn pin_binding_round_trips_through_parse_pin_labels() {
        let var = pin_binding("pc13", &PinFunction::GpioOutput, "My Pin 7");
        assert_eq!(var, "pc13_out_my_pin_7");
        let src = format!(
            "{GEN_BEGIN}\nlet {var} = &mut gpioc.pc13.into_push_pull_output(&mut gpioc.crh); // GPIO Output\n{GEN_END}"
        );
        assert_eq!(
            parse_pin_labels(&src),
            vec![("PC13".to_owned(), "my_pin_7".to_owned())]
        );
    }

    /// The shape each backend emits, so "jump to this pin" lands on the right
    /// line no matter which one generated main.rs. The `let mut` cases are the
    /// ones that matter: embassy and ESP bind every OUTPUT that way.
    #[test]
    fn find_pin_binding_line_covers_every_backend_shape() {
        // (source line inside the block, pin, expected binding)
        let cases: [(&str, &str, &str); 4] = [
            // STM32 blocking
            (
                "    let pc13_out_led = gpioc.pc13.into_push_pull_output(&mut gpioc.crh); // GPIO Output",
                "PC13",
                "pc13_out_led",
            ),
            // embassy output (`let mut`)
            (
                "    let mut pa5_out = Output::new(p.PA5, Level::Low, Speed::Low); // GPIO Output",
                "PA5",
                "pa5_out",
            ),
            // embassy input
            (
                "    let pb6_in = Input::new(p.PB6, Pull::None); // GPIO Input",
                "PB6",
                "pb6_in",
            ),
            // ESP output (`let mut`, gpioNN naming)
            (
                "    let mut gpio2_out = Output::new(peripherals.GPIO2, Level::High, OutputConfig::default()); // GPIO Output",
                "GPIO2",
                "gpio2_out",
            ),
        ];
        for (line, pin, binding) in cases {
            let src = format!("#![no_std]\nfn x() {{}}\n{GEN_BEGIN}\n{line}\n{GEN_END}\n");
            assert_eq!(
                find_pin_binding_line(&src, pin),
                // 1: #![no_std], 2: fn x, 3: GEN_BEGIN, 4: the binding
                Some((4, binding.to_owned())),
                "{pin} in `{line}`"
            );
        }
    }

    #[test]
    fn find_pin_binding_line_ignores_non_pin_lets() {
        let src = format!(
            "{GEN_BEGIN}\n\
             let peripherals = esp_hal::init(config);\n\
             let mut gpioc = dp.GPIOC.split();\n\
             let p = embassy_stm32::init(config);\n\
             let mut _adc1 = init_adc1(dp.ADC1, clocks);\n\
             let pc14_out = gpioc.pc14.into_push_pull_output(&mut gpioc.crh); // GPIO Output\n\
             {GEN_END}\n"
        );
        assert_eq!(gen_let_bindings(&src).len(), 1);
        assert_eq!(
            find_pin_binding_line(&src, "PC14"),
            Some((6, "pc14_out".to_owned()))
        );
        assert_eq!(find_pin_binding_line(&src, "PC13"), None);
    }

    /// A pin handed straight to its driver has no `let` of its own — the call
    /// that consumes it is the definition site.
    #[test]
    fn find_pin_mention_line_falls_back_to_the_consuming_call() {
        let src = format!(
            "{GEN_BEGIN}\n\
             let mut _uart1 = Uart::new(peripherals.UART1, cfg)\n\
             .with_rx(peripherals.GPIO20)\n\
             .with_tx(peripherals.GPIO21);\n\
             {GEN_END}\n"
        );
        assert_eq!(find_pin_binding_line(&src, "GPIO20"), None);
        assert_eq!(find_pin_mention_line(&src, "GPIO20"), Some(3));
        assert_eq!(find_pin_mention_line(&src, "GPIO21"), Some(4));
        // Whole-token only: GPIO2 must not light up on GPIO20 / GPIO21.
        assert_eq!(find_pin_mention_line(&src, "GPIO2"), None);
    }

    #[test]
    fn pin_lookups_ignore_code_outside_the_gen_block() {
        let src = format!(
            "{GEN_BEGIN}\n{GEN_END}\n\
             let pc13_out = something(); // GPIO Output\n\
             let x = p.PC13;\n"
        );
        assert_eq!(find_pin_binding_line(&src, "PC13"), None);
        assert_eq!(find_pin_mention_line(&src, "PC13"), None);
    }
}
