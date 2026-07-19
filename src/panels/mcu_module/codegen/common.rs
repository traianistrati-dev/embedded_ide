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

// ── User tail — closes fn main() ─────────────────────────────────────────────
//
// Written once on first generation; the loop body is user-editable and is
// preserved across every pin-configuration change.

pub const USER_TAIL: &str = "    loop {\n        // Your main loop code here.\n    }\n}\n";

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
/// identifier (e.g. `gi_usart_1`), so its types are reachable as `gi_usart_1::…`.
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

/// Recover the user labels embedded in generated binding names, mirroring
/// [`parse_main_rs`]. Scans the GEN block's per-pin `let` bindings and, for each
/// one carrying a `<base>_<type>_<label>` suffix, returns `(pin_name, label)`.
/// Only GPIO/PWM bindings (the ones the rename field targets) can carry a label.
pub fn parse_pin_labels(source: &str) -> Vec<(String, String)> {
    let (Some(begin_pos), Some(end_pos)) = (source.find(GEN_BEGIN), source.find(GEN_END)) else {
        return vec![];
    };
    if begin_pos >= end_pos {
        return vec![];
    }

    let mut result = Vec::new();
    for line in source[begin_pos..end_pos].lines() {
        let trimmed = line.trim();

        // Identify the binding var + pin name for the two `let`-binding shapes.
        let (var, pin_name) = if trimmed.starts_with("let p") {
            let after_let = &trimmed["let ".len()..];
            let Some(eq_pos) = after_let.find(" =") else { continue };
            let var = after_let[..eq_pos].trim();
            if var.len() < 3 || !var.starts_with('p') {
                continue;
            }
            let port_lc = match var.chars().nth(1) {
                Some(c) if c.is_ascii_lowercase() => c,
                _ => continue,
            };
            let num: String = var[2..].chars().take_while(|c| c.is_ascii_digit()).collect();
            if num.is_empty() {
                continue;
            }
            (var, format!("P{}{}", port_lc.to_ascii_uppercase(), num))
        } else if trimmed.starts_with("let mut gpio") || trimmed.starts_with("let gpio") {
            let after_let = if trimmed.starts_with("let mut ") {
                &trimmed["let mut ".len()..]
            } else {
                &trimmed["let ".len()..]
            };
            let Some(eq_pos) = after_let.find(" =") else { continue };
            let var = after_let[..eq_pos].trim();
            let rest = match var.strip_prefix("gpio") {
                Some(r) if r.starts_with(|c: char| c.is_ascii_digit()) => r,
                _ => continue,
            };
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if num.is_empty() {
                continue;
            }
            (var, format!("GPIO{num}"))
        } else {
            continue;
        };

        // Function from the comment, then strip the `<base>_<type>` prefix; the
        // remaining `_<label>` (if any) is the user's custom name.
        let Some(comment_pos) = trimmed.rfind("// ") else { continue };
        let label_str = trimmed[comment_pos + 3..].trim().trim_end_matches(';').trim();
        let Some(func) = PinFunction::from_label(label_str) else { continue };

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

    for line in gen_block.lines() {
        let trimmed = line.trim();

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
            let pin_num_str: String = var[2..].chars().take_while(|c| c.is_ascii_digit()).collect();
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
            let pin_num_str: String = gpio_rest.chars().take_while(|c| c.is_ascii_digit()).collect();
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

            let Some(comment_pos) = trimmed.rfind("// ") else {
                continue;
            };
            // Strip trailing ';' — appears on the last method of a builder chain
            let label = trimmed[comment_pos + 3..]
                .trim()
                .trim_end_matches(';')
                .trim();

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
    use super::*;

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
        assert_eq!(pin_binding("pc13", &PinFunction::GpioOutput, ""), "pc13_out");
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
}
