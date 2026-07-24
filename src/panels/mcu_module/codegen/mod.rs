//! Code generation for MCU projects — routes to a per-**family** backend.
//!
//! [`Mcu::fresh_main_rs`] / [`Mcu::update_main_rs`] dispatch on `self.family`
//! via the [`family`] module's [`FamilyBackend`](family::FamilyBackend)
//! registry. Each backend wraps the HAL-specific generator:
//! - "stm32f1" → `stm32` module (Rust Embedded HAL)
//! - "esp32c3" → parent `codegen_esp` module (esp-hal)
//! - unknown families (e.g. STM8) → no generated code yet.

pub mod common;
pub mod embassy_common;
pub mod family;
pub mod rcc;
pub mod stm32;
pub mod wba;

use super::mcu::Mcu;

// Re-export public API for backward compatibility
pub use common::{
    mcu_id_marker_line, parse_main_rs, parse_mcu_id, parse_pin_labels, pin_binding, sanitize_label,
    var_suffix, GEN_BEGIN, GEN_END, MCU_ID_MARKER, USER_TAIL,
};

// ── Public API on Mcu ─────────────────────────────────────────────────────────

impl Mcu {
    /// Build a brand-new `src/main.rs` (called when the MCU type is first
    /// selected or reset). Dispatches on `self.family`; families without a
    /// registered backend produce an empty file.
    pub fn fresh_main_rs(&self) -> String {
        let code = family::backend_for(&self.family)
            .map(|b| b.fresh_main_rs(self))
            .unwrap_or_default();
        // Module/clock state is persisted out-of-source in `mcu.config`
        // (see `Mcu::mcu_config_text`), not as comment markers in main.rs.
        common::ensure_module_models(code, &self.modules)
    }

    /// Update `existing` in-place: replace only the generated section
    /// (between the markers), preserving the user-editable parts.
    ///
    /// Families without a GEN block (ESP32-C3 has its own scheme; STM8 has
    /// no backend) return the existing file unchanged.
    pub fn update_main_rs(&self, existing: &str) -> String {
        let code = family::backend_for(&self.family)
            .map(|b| b.update_main_rs(self, existing))
            .unwrap_or_else(|| existing.to_owned());
        // Module/clock state is persisted in `mcu.config`, not in main.rs.
        common::ensure_module_models(code, &self.modules)
    }

    /// Per-peripheral init module bodies for `src/pins/configs/` — `(file_name,
    /// generated_body)`. Empty for families without separate config files.
    pub fn config_files(&self) -> Vec<(String, String)> {
        family::backend_for(&self.family)
            .map(|b| b.config_files(self))
            .unwrap_or_default()
    }

    /// Kept for any remaining call sites — delegates to `fresh_main_rs`.
    #[allow(dead_code)]
    pub fn generate_code(&self) -> String {
        self.fresh_main_rs()
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

    // ── Peripheral helper placement / ADC fix ────────────────────────────────

    #[test]
    fn test_adc_helper_takes_clocks_by_value_after_main() {
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        // PA0 (pin 10) supports ADC1 IN0.
        mcu.apply_pin_function(10, PinFunction::AdcChannel { adc: 1, channel: 0 });
        let code = mcu.fresh_main_rs();

        // Bug fix: the helper takes `Clocks` BY VALUE, not `&Clocks`.
        assert_contains_substring(
            &code,
            "fn init_adc1(adc1: pac::ADC1, clocks: Clocks) -> adc::Adc<pac::ADC1>",
        );
        assert_not_contains_substring(&code, "clocks: &Clocks)");
        // …and the call passes `clocks` by value (Clocks is Copy).
        assert_contains_substring(&code, "init_adc1(dp.ADC1, clocks)");

        // The helper is placed AFTER `fn main`, outside the GENERATED block.
        let main_pos = code.find("fn main()").unwrap();
        let helper_pos = code.find("fn init_adc1(").unwrap();
        let gen_end = code.find(GEN_END).unwrap();
        assert!(helper_pos > main_pos, "helper must come after fn main");
        assert!(helper_pos > gen_end, "helper must be outside the GEN block");
    }

    #[test]
    fn test_helper_added_on_update_and_idempotent() {
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        mcu.apply_pin_function(10, PinFunction::AdcChannel { adc: 1, channel: 0 });

        let blank = "// <<< GENERATED BEGIN — do not edit between these markers >>>\n\
                     OLD\n\
                     // <<< GENERATED END >>>\n    loop {}\n}\n";
        let once = mcu.update_main_rs(blank);
        assert_contains_substring(&once, "fn init_adc1(adc1: pac::ADC1, clocks: Clocks)");

        let twice = mcu.update_main_rs(&once);
        assert_eq!(once.matches("fn init_adc1(").count(), 1, "no duplicate helper");
        assert_eq!(twice.matches("fn init_adc1(").count(), 1, "still single helper");
        assert_eq!(once, twice, "update must be idempotent");
    }

    #[test]
    fn test_generic_helpers_not_duplicated_on_repeated_update() {
        use super::super::mock_mcu;

        // SPI1 (PA5 = pin 15 → SpiSck(1); partners auto-assign MISO/MOSI) and
        // I2C1 (PB6 = pin 42 → I2cScl(1); PB7 → I2cSda(1)) both emit GENERIC
        // helpers `fn init_spi1<PINS>(` / `fn init_i2c1<PINS>(`. These were the
        // ones that grew without bound.
        let mut mcu = mock_mcu::create_stm32f103c8tx();
        mcu.apply_pin_function(15, PinFunction::SpiSck(1));
        mcu.apply_pin_function(42, PinFunction::I2cScl(1));
        mcu.apply_pin_function(43, PinFunction::I2cSda(1));

        // Init now lives in `src/pins/configs/`; main.rs only calls into it.
        let mut code = mcu.fresh_main_rs();
        assert_contains_substring(&code, "pins::configs::spi1::init(");
        assert_contains_substring(&code, "pins::configs::i2c1::init(");

        // Re-run the per-frame regeneration many times — the calls aren't
        // duplicated in main.rs and each config file has one `pub fn init`.
        for _ in 0..10 {
            code = mcu.update_main_rs(&code);
        }
        assert_eq!(
            code.matches("pins::configs::spi1::init(").count(),
            1,
            "SPI call duplicated"
        );
        assert_eq!(
            code.matches("pins::configs::i2c1::init(").count(),
            1,
            "I2C call duplicated"
        );
        let cfgs = mcu.config_files();
        let spi1 = &cfgs.iter().find(|(n, _)| n == "spi1.rs").unwrap().1;
        let i2c1 = &cfgs.iter().find(|(n, _)| n == "i2c1.rs").unwrap().1;
        assert_eq!(spi1.matches("pub fn init").count(), 1);
        assert_eq!(i2c1.matches("pub fn init").count(), 1);
    }

    #[test]
    fn test_can_emits_config_file_and_call() {
        use super::super::mock_mcu;

        // CAN1 on PA11 (RX, pin 32) + PA12 (TX, pin 33). Codegen must emit a
        // `can1.rs` config module and a single call into it from main.rs, with
        // pins ordered (TX, RX) as bxcan/assign_pins expect.
        let mut mcu = mock_mcu::create_stm32f103c8tx();
        mcu.apply_pin_function(32, PinFunction::CanRx);
        mcu.apply_pin_function(33, PinFunction::CanTx);

        let mut code = mcu.fresh_main_rs();
        assert_contains_substring(&code, "pins::configs::can1::init(dp.CAN1,");
        assert_contains_substring(&code, "&mut afio);");
        // CAN needs AFIO constrained.
        assert_contains_substring(&code, "let mut afio = dp.AFIO.constrain();");

        // The call isn't duplicated across repeated per-frame regeneration.
        for _ in 0..10 {
            code = mcu.update_main_rs(&code);
        }
        assert_eq!(
            code.matches("pins::configs::can1::init(").count(),
            1,
            "CAN call duplicated"
        );

        // The config file carries the GENERATED constants + one editable `init`.
        let cfgs = mcu.config_files();
        let can1 = &cfgs.iter().find(|(n, _)| n == "can1.rs").unwrap().1;
        assert_contains_substring(can1, "const BITRATE: u32 =");
        assert_contains_substring(can1, "const BTR: u32 =");
        assert_eq!(can1.matches("pub fn init").count(), 1);
    }

    #[test]
    fn test_usb_emits_cdc_serial_init_and_uses() {
        use super::super::mock_mcu;

        // USB on PA11 (D-, pin 32) + PA12 (D+, pin 33). Codegen must emit the
        // CDC serial setup in main + the external-crate `use`s.
        let mut mcu = mock_mcu::create_stm32f103c8tx();
        mcu.apply_pin_function(32, PinFunction::UsbDm);
        mcu.apply_pin_function(33, PinFunction::UsbDp);

        let mut code = mcu.fresh_main_rs();
        assert_contains_substring(&code, "use usb_device::prelude::*;");
        assert_contains_substring(&code, "use usbd_serial::{SerialPort, USB_CLASS_CDC};");
        assert_contains_substring(&code, "let usb_bus = UsbBus::new(usb_periph);");
        assert_contains_substring(&code, "UsbVidPid(0x16c0, 0x27dd)");
        assert_contains_substring(&code, ".device_class(USB_CLASS_CDC)");
        // D+ reset trick + HAL usb import present.
        assert_contains_substring(&code, "usb_dp.set_low();");
        assert_contains_substring(&code, "usb::{Peripheral, UsbBus}");

        // The block isn't duplicated across repeated per-frame regeneration.
        for _ in 0..10 {
            code = mcu.update_main_rs(&code);
        }
        assert_eq!(
            code.matches("let usb_bus = UsbBus::new").count(),
            1,
            "USB init duplicated"
        );
    }

    #[test]
    fn test_user_edited_helper_is_preserved() {
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        mcu.apply_pin_function(10, PinFunction::AdcChannel { adc: 1, channel: 0 });

        // A file where the user customised the helper body.
        let edited = "// <<< GENERATED BEGIN — do not edit between these markers >>>\n\
                      OLD\n\
                      // <<< GENERATED END >>>\n    loop {}\n}\n\n\
                      fn init_adc1(adc1: pac::ADC1, clocks: Clocks) -> adc::Adc<pac::ADC1> {\n\
                          let mut a = adc::Adc::adc1(adc1, clocks);\n\
                          a.set_sample_time(adc::SampleTime::T_239);\n\
                          a\n}\n";
        let out = mcu.update_main_rs(edited);

        // The custom body survives and the helper is not duplicated.
        assert_contains_substring(&out, "set_sample_time");
        assert_eq!(out.matches("fn init_adc1(").count(), 1, "must not re-add helper");
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

    /// An imported `ClockConfig::Graph` drives the generated `rcc.cfgr` chain —
    /// not the hardcoded default — closing the data-driven loop.
    #[test]
    fn test_graph_clock_drives_generated_chain() {
        use super::super::clock::graph::layout::stm32f1_layout;
        use super::super::clock::graph::{stm32f1_graph, GraphClock};
        use super::super::clock::model::{Stm32f1Clock, SysclkSrc};
        use super::super::clock::{ClockConfig, ClockLimits};
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        // HSI-direct config carried as a graph → SYSCLK 8 MHz (not the default 72).
        let mut cfg = Stm32f1Clock::default();
        cfg.sysclk_src = SysclkSrc::Hsi;
        mcu.clock = ClockConfig::Graph(GraphClock {
            graph: stm32f1_graph(&cfg),
            layout: stm32f1_layout(&ClockLimits::default()),
        });

        let code = mcu.fresh_main_rs();
        assert_contains_substring(&code, ".sysclk(8.MHz())");
        assert_not_contains_substring(&code, ".sysclk(72.MHz())");
    }

    /// Build a graph clock from a typed config (the only runtime clock model).
    fn graph_clock(cfg: &super::super::clock::model::Stm32f1Clock) -> super::super::clock::ClockConfig {
        use super::super::clock::graph::{layout::stm32f1_layout, stm32f1_graph, GraphClock};
        use super::super::clock::{ClockConfig, ClockLimits};
        ClockConfig::Graph(GraphClock {
            graph: stm32f1_graph(cfg),
            layout: stm32f1_layout(&ClockLimits::default()),
        })
    }

    #[test]
    fn test_modified_clock_emits_extra_knobs() {
        use super::super::clock::model::Stm32f1Clock;
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        let mut cfg = Stm32f1Clock::default();
        cfg.ahb_pre = 2; // HCLK = 36 → emits .hclk(...)
        cfg.apb2_pre = 2; // PCLK2 = 18 → emits .pclk2(...)
        mcu.clock = graph_clock(&cfg);
        let code = mcu.fresh_main_rs();

        assert_contains_substring(&code, ".hclk(");
        assert_contains_substring(&code, ".pclk2(");
    }

    #[test]
    fn test_clock_marker_roundtrips_through_mcu_config() {
        use super::super::clock::graph::graph_to_stm32f1;
        use super::super::clock::model::Stm32f1Clock;
        use super::super::clock::ClockConfig;
        use super::super::mcu_config;
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        let mut cfg = Stm32f1Clock::default();
        cfg.pll_mul = 6;
        cfg.apb2_pre = 2;
        cfg.adc_pre = 8;
        mcu.clock = graph_clock(&cfg);

        // The clock is persisted in mcu.config (no longer a marker in main.rs),
        // and parses back to the graph's effective config.
        assert!(!mcu.fresh_main_rs().contains("@clock"), "no marker in main.rs");
        let (_, parsed) = mcu_config::parse(&mcu.mcu_config_text());
        let ClockConfig::Graph(gc) = &mcu.clock else { panic!("expected graph clock") };
        assert_eq!(parsed, Some(graph_to_stm32f1(&gc.graph)));
    }

    #[test]
    fn test_hsi_preset_omits_use_hse() {
        use super::super::clock::model::{Stm32f1Clock, SysclkSrc};
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        // HSI-only: no crystal → no `.use_hse(...)`, SYSCLK = 8 MHz.
        mcu.clock = graph_clock(&Stm32f1Clock {
            hse_enabled: false,
            sysclk_src: SysclkSrc::Hsi,
            ..Stm32f1Clock::default()
        });
        let code = mcu.fresh_main_rs();

        assert_not_contains_substring(&code, ".use_hse(");
        assert_contains_substring(&code, ".sysclk(8.MHz())");
    }

    // ── Variable naming: `<pin>_<type>` bindings ─────────────────────────────

    /// The generated `let` binding carries the type suffix (`pa0_out`), while
    /// the HAL field access stays the bare pin (`gpioa.pa0`).
    #[test]
    fn test_binding_uses_pin_type_format_gpio() {
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        // PA0 (pin 10) → GPIO output.
        mcu.apply_pin_function(10, PinFunction::GpioOutput);
        let code = mcu.fresh_main_rs();

        assert_contains_substring(&code, "let pa0_out = &mut gpioa.pa0.into_push_pull_output");
        // The bare-field form must NOT be used for the binding name.
        assert_not_contains_substring(&code, "let pa0 = &mut gpioa.pa0");
    }

    /// A `<pin>_<type>` binding still round-trips back through `parse_main_rs`.
    #[test]
    fn test_binding_pin_type_format_roundtrips() {
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        mcu.apply_pin_function(10, PinFunction::GpioOutput);
        let code = mcu.fresh_main_rs();

        let parsed = parse_main_rs(&code);
        assert!(parsed.iter().any(|(name, func)| name == "PA0"
            && *func == PinFunction::GpioOutput));
    }

    /// A user label on a pin is appended to the generated binding name
    /// (`pa0_out_led`) while the HAL field access stays bare (`gpioa.pa0`).
    #[test]
    fn test_custom_label_appended_to_binding() {
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        mcu.apply_pin_function(10, PinFunction::GpioOutput);
        mcu.find_pin_mut(10).unwrap().custom_label = "Status LED".into();
        let code = mcu.fresh_main_rs();

        assert_contains_substring(
            &code,
            "let pa0_out_status_led = &mut gpioa.pa0.into_push_pull_output",
        );

        // The label round-trips back into the pin's custom_label on reopen.
        let labels = parse_pin_labels(&code);
        assert!(labels.contains(&("PA0".to_owned(), "status_led".to_owned())));
    }

    /// Clearing a pin (Unset) drops its custom label, so a reassigned pin
    /// starts clean.
    #[test]
    fn test_unset_clears_custom_label() {
        use super::super::mock_mcu;

        let mut mcu = mock_mcu::create_stm32f103c8tx();
        mcu.apply_pin_function(10, PinFunction::GpioOutput);
        mcu.find_pin_mut(10).unwrap().custom_label = "led".into();
        mcu.apply_pin_function(10, PinFunction::Unset);

        assert!(mcu.find_pin(10).unwrap().custom_label.is_empty());
    }
}
