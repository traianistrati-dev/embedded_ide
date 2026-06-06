//! Code generation for MCU projects — routes to toolchain-specific generators.
//!
//! Handles:
//! - STM32 (Rust Embedded HAL) via `stm32` module
//! - ESP32-C3 (esp-hal) via parent `codegen_esp` module
//! - STM8 (SDCC C) — not yet implemented

pub mod common;
pub mod stm32;

use super::mcu::Mcu;
use super::mcu_catalog::ToolchainKind;
use super::pins::logic::pin::Pin;

// Re-export public API for backward compatibility
pub use common::{parse_main_rs, GEN_BEGIN, GEN_END, USER_TAIL};

// ── Public API on Mcu ─────────────────────────────────────────────────────────

impl Mcu {
    /// Build a brand-new `src/main.rs` (called when the MCU type is first
    /// selected or reset).  Dispatches on `self.toolchain`.
    pub fn fresh_main_rs(&self) -> String {
        match self.toolchain {
            ToolchainKind::RustEmbedded => {
                let all = self.all_pins();
                let gen_ = stm32::make_generated_section(&self.name, &all, &self.clock);
                format!(
                    "{header}{gen_}\n{tail}",
                    header = stm32::invariant_header(&self.name),
                    tail = USER_TAIL,
                )
            }
            ToolchainKind::EspRust => {
                let all = self.all_pins();
                super::codegen_esp::fresh_esp32c3_main_rs(&all)
            }
            ToolchainKind::SdccC => {
                // STM8 — not yet implemented
                String::new()
            }
        }
    }

    /// Update `existing` in-place: replace only the generated section
    /// (between the markers), preserving the user-editable parts.
    ///
    /// For toolchains without a GEN block (EspRust, SdccC) the existing
    /// file is returned unchanged — pin state is not reflected in code yet.
    pub fn update_main_rs(&self, existing: &str) -> String {
        match self.toolchain {
            ToolchainKind::RustEmbedded => {
                let all = self.all_pins();
                let new_section = stm32::make_generated_section(&self.name, &all, &self.clock);
                stm32::splice_section(existing, &new_section, &self.name)
            }
            ToolchainKind::EspRust => {
                let all = self.all_pins();
                super::codegen_esp::update_esp32c3_main_rs(existing, &all)
            }
            // SdccC — not yet implemented; return file unchanged.
            ToolchainKind::SdccC => existing.to_owned(),
        }
    }

    /// Kept for any remaining call sites — delegates to `fresh_main_rs`.
    #[allow(dead_code)]
    pub fn generate_code(&self) -> String {
        self.fresh_main_rs()
    }

    fn all_pins(&self) -> Vec<&Pin> {
        self.top_pins
            .iter()
            .chain(self.bottom_pins.iter())
            .chain(self.left_pins.iter())
            .chain(self.right_pins.iter())
            .collect()
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

    // ── Helper functions ──────────────────────────────────────────────────────

    fn assert_contains_substring(haystack: &str, needle: &str) {
        assert!(
            haystack.contains(needle),
            "Expected substring not found: '{}'\n\nIn:\n{}",
            needle,
            haystack
        );
    }

    fn assert_not_contains_substring(haystack: &str, needle: &str) {
        assert!(
            !haystack.contains(needle),
            "Unexpected substring found: '{}'\n\nIn:\n{}",
            needle,
            haystack
        );
    }

    fn count_gen_sections(code: &str) -> usize {
        code.matches(GEN_BEGIN).count()
    }

    // ── Parse Main RS Tests (PUBLIC API) ──────────────────────────────────────

    #[test]
    fn test_parse_main_rs_stm32_gpio_output() {
        let input = "// <<< GENERATED BEGIN — do not edit between these markers >>>\nlet pa0 = &mut gpioa.pa0.into_push_pull_output(&mut gpioa.crl); // GPIO Output\n// <<< GENERATED END >>>";
        let parsed = parse_main_rs(input);

        assert!(parsed.iter().any(|(name, _)| name == "PA0"));
        assert!(parsed.iter().any(|(_, func)| *func == PinFunction::GpioOutput));
    }

    #[test]
    fn test_parse_main_rs_stm32_gpio_input() {
        let input = "// <<< GENERATED BEGIN — do not edit between these markers >>>\nlet pc13 = &mut gpioc.pc13.into_floating_input(&mut gpioc.crh); // GPIO Input\n// <<< GENERATED END >>>";
        let parsed = parse_main_rs(input);

        assert!(parsed.iter().any(|(name, _)| name == "PC13"));
        assert!(parsed.iter().any(|(_, func)| *func == PinFunction::GpioInput));
    }

    #[test]
    fn test_parse_main_rs_multiple_pins() {
        let input = "// <<< GENERATED BEGIN — do not edit between these markers >>>\nlet pa0 = &mut gpioa.pa0.into_push_pull_output(&mut gpioa.crl); // GPIO Output\nlet pc13 = &mut gpioc.pc13.into_floating_input(&mut gpioc.crh); // GPIO Input\n// <<< GENERATED END >>>";
        let parsed = parse_main_rs(input);

        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn test_parse_main_rs_no_markers() {
        let input = "let pa0 = &mut gpioa.pa0.into_push_pull_output(&mut gpioa.crl); // GPIO Output";
        let parsed = parse_main_rs(input);

        assert!(parsed.is_empty(), "Should not parse without markers");
    }

    #[test]
    fn test_parse_main_rs_empty_gen_section() {
        let input = "// <<< GENERATED BEGIN — do not edit between these markers >>>\n// <<< GENERATED END >>>";
        let parsed = parse_main_rs(input);

        assert!(parsed.is_empty());
    }

    #[test]
    fn test_parse_main_rs_adc_channel() {
        let input = "// <<< GENERATED BEGIN — do not edit between these markers >>>\nlet pa1 = &mut gpioa.pa1.into_analog(&mut gpioa.crl); // ADC1  IN1\n// <<< GENERATED END >>>";
        let parsed = parse_main_rs(input);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "PA1");
    }

    #[test]
    fn test_parse_main_rs_skips_port_split_lines() {
        let input = "// <<< GENERATED BEGIN — do not edit between these markers >>>\nlet mut gpioa = dp.GPIOA.split();\nlet pa0 = &mut gpioa.pa0.into_push_pull_output(&mut gpioa.crl); // GPIO Output\n// <<< GENERATED END >>>";
        let parsed = parse_main_rs(input);

        // Should only have one pin (PA0), not the port split
        assert_eq!(parsed.len(), 1);
    }

    // ── Marker Constants Tests ────────────────────────────────────────────────

    #[test]
    fn test_gen_begin_marker_constant_exists() {
        assert!(!GEN_BEGIN.is_empty());
        assert!(GEN_BEGIN.contains("GENERATED BEGIN"));
        assert!(GEN_BEGIN.contains("do not edit"));
    }

    #[test]
    fn test_gen_end_marker_constant_exists() {
        assert!(!GEN_END.is_empty());
        assert!(GEN_END.contains("GENERATED END"));
    }

    #[test]
    fn test_user_tail_constant_exists() {
        assert!(!USER_TAIL.is_empty());
        assert!(USER_TAIL.contains("loop"));
    }

    // ── Marker Count Tests ────────────────────────────────────────────────────

    #[test]
    fn test_count_gen_sections_single() {
        let code = "// <<< GENERATED BEGIN — do not edit between these markers >>>\ncode\n// <<< GENERATED END >>>";
        assert_eq!(count_gen_sections(code), 1);
    }

    #[test]
    fn test_count_gen_sections_multiple() {
        let code = "// <<< GENERATED BEGIN — do not edit between these markers >>>\ncode1\n// <<< GENERATED END >>>\n// <<< GENERATED BEGIN — do not edit between these markers >>>\ncode2\n// <<< GENERATED END >>>";
        assert_eq!(count_gen_sections(code), 2);
    }

    #[test]
    fn test_count_gen_sections_none() {
        let code = "// No markers here";
        assert_eq!(count_gen_sections(code), 0);
    }

    // ── Fresh Code Generation Tests (via Mcu::fresh_main_rs) ─────────────────

    #[test]
    fn test_fresh_main_rs_stm32_no_pins_has_markers() {
        use super::super::mock_mcu;

        let mcu = mock_mcu::create_stm32f103c8tx();
        let code = mcu.fresh_main_rs();

        assert_contains_substring(&code, GEN_BEGIN);
        assert_contains_substring(&code, GEN_END);
        assert_eq!(count_gen_sections(&code), 1);
    }

    #[test]
    fn test_fresh_main_rs_stm32_has_required_headers() {
        use super::super::mock_mcu;

        let mcu = mock_mcu::create_stm32f103c8tx();
        let code = mcu.fresh_main_rs();

        assert_contains_substring(&code, "#![no_std]");
        assert_contains_substring(&code, "#![no_main]");
        assert_contains_substring(&code, "use cortex_m_rt::entry;");
        assert_contains_substring(&code, "#[entry]");
        assert_contains_substring(&code, "fn main() -> !");
    }

    #[test]
    fn test_fresh_main_rs_stm32_ends_with_user_tail() {
        use super::super::mock_mcu;

        let mcu = mock_mcu::create_stm32f103c8tx();
        let code = mcu.fresh_main_rs();

        assert_contains_substring(&code, USER_TAIL);
        let trimmed_code = code.trim_end();
        let trimmed_tail = USER_TAIL.trim_end();
        assert!(trimmed_code.ends_with(trimmed_tail), "Code should end with USER_TAIL");
    }

    #[test]
    fn test_fresh_main_rs_stm32_contains_preamble_before_gen() {
        use super::super::mock_mcu;

        let mcu = mock_mcu::create_stm32f103c8tx();
        let code = mcu.fresh_main_rs();

        let gen_start = code.find(GEN_BEGIN).expect("GEN_BEGIN not found");
        let preamble = &code[..gen_start];

        // Should have some content before gen section
        assert!(preamble.len() > 0);
        assert!(preamble.contains("#!") || preamble.contains("use"));
    }

    // ── Update Code Generation Tests (via Mcu::update_main_rs) ───────────────

    #[test]
    fn test_update_main_rs_preserves_user_code() {
        use super::super::mock_mcu;

        let before = "// preamble\n// <<< GENERATED BEGIN — do not edit between these markers >>>\nOLD_GEN\n// <<< GENERATED END >>>\nloop { eprintln!(\"Custom\"); }";
        let mcu = mock_mcu::create_stm32f103c8tx();
        let after = mcu.update_main_rs(before);

        assert_contains_substring(&after, "eprintln!(\"Custom\")");
        assert_not_contains_substring(&after, "OLD_GEN");
    }

    #[test]
    fn test_update_main_rs_produces_valid_markers() {
        use super::super::mock_mcu;

        let before = "// <<< GENERATED BEGIN — do not edit between these markers >>>\nOLD\n// <<< GENERATED END >>>\nloop";
        let mcu = mock_mcu::create_stm32f103c8tx();
        let after = mcu.update_main_rs(before);

        assert_eq!(count_gen_sections(&after), 1);
        assert_contains_substring(&after, GEN_BEGIN);
        assert_contains_substring(&after, GEN_END);
    }

    #[test]
    fn test_update_main_rs_idempotent() {
        use super::super::mock_mcu;

        let original = "// <<< GENERATED BEGIN — do not edit between these markers >>>\nOLD\n// <<< GENERATED END >>>\nloop";
        let mcu = mock_mcu::create_stm32f103c8tx();

        let first = mcu.update_main_rs(original);
        let second = mcu.update_main_rs(&first);

        assert_eq!(first, second, "update_main_rs should be idempotent");
    }

    #[test]
    fn test_update_main_rs_multiple_iterations() {
        use super::super::mock_mcu;

        let original = "// <<< GENERATED BEGIN — do not edit between these markers >>>\nOLD\n// <<< GENERATED END >>>\nloop";
        let mcu = mock_mcu::create_stm32f103c8tx();

        let first = mcu.update_main_rs(original);
        let second = mcu.update_main_rs(&first);

        // Second update should be identical to first (idempotent)
        assert_eq!(first, second, "Second update should be identical to first");

        // Count markers to ensure they didn't multiply
        assert_eq!(first.matches(GEN_BEGIN).count(), 1, "Should have exactly 1 GEN_BEGIN");
        assert_eq!(second.matches(GEN_BEGIN).count(), 1, "Should have exactly 1 GEN_BEGIN");
    }

    #[test]
    fn test_update_main_rs_handles_multiline_user_code() {
        use super::super::mock_mcu;

        let before = "// <<< GENERATED BEGIN — do not edit between these markers >>>\nOLD\n// <<< GENERATED END >>>\nloop {\n    eprintln!(\"Line 1\");\n    eprintln!(\"Line 2\");\n}";
        let mcu = mock_mcu::create_stm32f103c8tx();
        let after = mcu.update_main_rs(before);

        assert_contains_substring(&after, "eprintln!(\"Line 1\")");
        assert_contains_substring(&after, "eprintln!(\"Line 2\")");
    }

    // ── Round-Trip Tests (Parse → Generate → Parse) ──────────────────────────

    #[test]
    fn test_round_trip_generated_then_parsed() {
        use super::super::mock_mcu;

        let mcu = mock_mcu::create_stm32f103c8tx();
        let code = mcu.fresh_main_rs();

        // Parse what we generated
        let parsed = parse_main_rs(&code);

        // Since we didn't configure pins, should be empty
        assert!(parsed.is_empty());
    }

    #[test]
    fn test_update_preserves_gen_section_integrity() {
        use super::super::mock_mcu;

        let before = "// <<< GENERATED BEGIN — do not edit between these markers >>>\nOLD\n// <<< GENERATED END >>>\nloop";
        let mcu = mock_mcu::create_stm32f103c8tx();
        let after = mcu.update_main_rs(before);

        // After update, should still have exactly one marker pair
        let begin_count = after.matches(GEN_BEGIN).count();
        let end_count = after.matches(GEN_END).count();

        assert_eq!(begin_count, 1);
        assert_eq!(end_count, 1);
    }

    // ── Clock codegen Tests ───────────────────────────────────────────────────

    #[test]
    fn test_default_clock_chain_matches_legacy() {
        use super::super::mock_mcu;

        let mcu = mock_mcu::create_stm32f103c8tx();
        let code = mcu.fresh_main_rs();

        // Default 72 MHz config must reproduce the original hardcoded chain…
        assert_contains_substring(&code, ".use_hse(8.MHz())");
        assert_contains_substring(&code, ".sysclk(72.MHz())");
        assert_contains_substring(&code, ".pclk1(36.MHz())");
        // …and omit knobs that match the HAL defaults (ahb/1, apb2/1, adc/6).
        assert_not_contains_substring(&code, ".hclk(");
        assert_not_contains_substring(&code, ".pclk2(");
        assert_not_contains_substring(&code, ".adcclk(");
    }

    #[test]
    fn test_modified_clock_emits_extra_knobs() {
        use super::super::clock::ClockConfig;
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        if let ClockConfig::Stm32f1(c) = &mut mcu.clock {
            c.ahb_pre = 2; // HCLK = 36 → emits .hclk(...)
            c.apb2_pre = 2; // PCLK2 = 18 → emits .pclk2(...)
        }
        let code = mcu.fresh_main_rs();

        assert_contains_substring(&code, ".hclk(");
        assert_contains_substring(&code, ".pclk2(");
    }

    #[test]
    fn test_clock_marker_roundtrips_through_codegen() {
        use super::super::clock::{persist, ClockConfig};
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        if let ClockConfig::Stm32f1(c) = &mut mcu.clock {
            c.pll_mul = 6;
            c.apb2_pre = 2;
            c.adc_pre = 8;
        }
        let code = mcu.fresh_main_rs();

        // Generated code carries the @clock marker, which parses back exactly.
        let parsed = persist::parse_from_source(&code).expect("clock marker present");
        if let ClockConfig::Stm32f1(orig) = &mcu.clock {
            assert_eq!(&parsed, orig);
        } else {
            panic!("expected Stm32f1 clock");
        }
    }

    #[test]
    fn test_hsi_preset_omits_use_hse() {
        use super::super::clock::model::{Stm32f1Clock, SysclkSrc};
        use super::super::clock::ClockConfig;
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        // HSI-only: no crystal → no `.use_hse(...)`, SYSCLK = 8 MHz.
        mcu.clock = ClockConfig::Stm32f1(Stm32f1Clock {
            hse_enabled: false,
            sysclk_src: SysclkSrc::Hsi,
            ..Stm32f1Clock::default()
        });
        let code = mcu.fresh_main_rs();

        assert_not_contains_substring(&code, ".use_hse(");
        assert_contains_substring(&code, ".sysclk(8.MHz())");
    }
}
