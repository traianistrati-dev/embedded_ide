// ── Section markers ───────────────────────────────────────────────────────────
//
// The GEN_BEGIN … GEN_END block is auto-generated and replaced whenever the
// pin configuration changes.  It includes the HAL use items, any peripheral
// helper functions, the #[entry] attribute, and the opening of fn main().
// The block is intentionally left open — USER_TAIL closes main() with the
// user-editable loop body, which is preserved across every regen.

pub const GEN_BEGIN: &str = "// <<< GENERATED BEGIN — do not edit between these markers >>>";
pub const GEN_END: &str = "// <<< GENERATED END >>>";

// ── User tail — closes fn main() ─────────────────────────────────────────────
//
// Written once on first generation; the loop body is user-editable and is
// preserved across every pin-configuration change.

pub const USER_TAIL: &str = "    loop {\n        // Your main loop code here.\n    }\n}\n";

// ── Pin state parser ──────────────────────────────────────────────────────────

use super::super::pins::logic::pin_function::PinFunction;

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

        // ── STM32: "let p{port}{num} = ..." ──────────────────────────────────
        // trimmed = "let pc13 = &mut gpioc.pc13.into_push_pull_output(…); // …"
        if trimmed.starts_with("let p") {
            let after_let = &trimmed["let ".len()..]; // "pc13 = …"
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
            let pin_num_str = &var[2..];
            if pin_num_str.is_empty() || !pin_num_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }

            // "pc13" → "PC13"
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
        //       .enable_pin(peripherals.GPIO0, Attenuation::Db11);
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

            // Separate ADC suffix ("_adc") from the numeric pin number
            let (pin_num_str, is_adc) = match gpio_rest.strip_suffix("_adc") {
                Some(num) => (num, true),
                None      => (gpio_rest, false),
            };
            if pin_num_str.is_empty() || !pin_num_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }

            let pin_name = format!("GPIO{pin_num_str}"); // "GPIO2", "GPIO0"

            let Some(comment_pos) = trimmed.rfind("// ") else {
                continue;
            };
            // Strip trailing ';' — can appear on single-method init lines
            let label = trimmed[comment_pos + 3..].trim().trim_end_matches(';').trim();

            if is_adc {
                // label = "ADC1  IN0"  →  matches PinFunction::from_label
                if let Some(func) = PinFunction::from_label(label) {
                    result.push((pin_name, func));
                }
            } else {
                match label {
                    "GPIO Output" => result.push((pin_name, PinFunction::GpioOutput)),
                    "GPIO Input"  => result.push((pin_name, PinFunction::GpioInput)),
                    other => {
                        if let Some(func) = PinFunction::from_label(other) {
                            result.push((pin_name, func));
                        }
                    }
                }
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
            let label = trimmed[comment_pos + 3..].trim().trim_end_matches(';').trim();

            if let Some(func) = PinFunction::from_label(label) {
                result.push((pin_name, func));
            }
            continue;
        }
    }

    result
}
