//! STM32WBA family codegen — `embassy-stm32` in BLOCKING style.
//!
//! Why embassy and not a classic `stm32wbaxx-hal`: there is no mature blocking
//! HAL crate for the STM32WBA series, but `embassy-stm32` genuinely supports it
//! and its GPIO API (`Output::new` / `Input::new`) is small and stable. Used
//! WITHOUT the async executor — `embassy_stm32::init()` hands back the
//! peripheral singletons and the `Output`/`Input` wrappers work fine under
//! `cortex_m_rt::entry`, so the generated project fits the same base
//! `Cargo.toml` (cortex-m-rt + panic-halt + the `hal_dep` line) every other
//! `RustEmbedded` family uses.
//!
//! Scope of v1: GPIO input/output pins get real, compiling init; bus / analog
//! pins (USART/SPI/I2C/ADC/…) are bound to their raw `embassy_stm32`
//! peripheral singleton (`let pa9 = p.PA9;`) with the role in the comment —
//! ready to hand to `embassy_stm32::usart::Uart::new(...)` etc. Every line
//! round-trips through [`super::parse_main_rs`], so the Pins canvas restores on
//! reopen. Full peripheral-driver generation (like the STM32F1 config files)
//! is a later step.

use super::super::clock::graph::{graph_to_wba, is_wba_graph, WbaClock, WbaSys};
use super::super::clock::model::ClockConfig;
use super::embassy_common;
use crate::panels::mcu_module::pins::logic::pin::Pin;

// The header + splice are the generic embassy shape (shared with every STM32
// family); re-exported so `family.rs` and the tests keep their `wba::…` paths.
pub use embassy_common::{invariant_header, splice_section};

/// The Clock-tab selections mapped onto `embassy_stm32::Config` (RCC).
/// The reset state (HSI16, everything /1) stays `Default::default()`; anything
/// else emits an explicit config block. Facts verified against embassy-stm32
/// v0.4.0 `src/rcc/wba.rs`: the field is `pll1`, sysclk variant `PLL1_R`, and
/// **the PLL is unavailable in voltage range 2** (embassy panics) — RANGE1 is
/// emitted for any PLL use; HSE-32 as sysclk also exceeds range 2's window.
fn clock_block(clock: &ClockConfig) -> String {
    let c: WbaClock = match clock {
        ClockConfig::Graph(gc) if is_wba_graph(&gc.graph) => graph_to_wba(&gc.graph),
        _ => WbaClock::default(),
    };
    if c == WbaClock::default() {
        return "    let p = embassy_stm32::init(Default::default()); // HSI16, all buses /1\n"
            .to_string();
    }

    let mhz = c.sysclk_hz() / 1_000_000;
    let sys_desc = match c.sys {
        WbaSys::Hsi => "HSI16".to_string(),
        WbaSys::Hse => "HSE32".to_string(),
        WbaSys::Pll => format!(
            "{} /{} x{} /{} via PLL1R",
            if c.pll_src_hse { "HSE32" } else { "HSI16" },
            c.pll_m,
            c.pll_n,
            c.pll_r
        ),
    };
    let mut b = String::new();
    b.push_str(&format!(
        "    // Clock (from the Clock tab): SYSCLK {mhz} MHz ({sys_desc}) · \
         AHB /{} · APB1 /{} APB2 /{} APB7 /{}\n",
        c.ahb, c.apb1, c.apb2, c.apb7
    ));
    b.push_str("    let mut config = embassy_stm32::Config::default();\n");
    b.push_str("    {\n        use embassy_stm32::rcc;\n");
    let hse_used = c.sys == WbaSys::Hse || (c.sys == WbaSys::Pll && c.pll_src_hse);
    if hse_used {
        b.push_str(
            "        config.rcc.hse = Some(rcc::Hse { prescaler: rcc::HsePrescaler::DIV1 });\n",
        );
    }
    if c.sys == WbaSys::Pll {
        b.push_str(&format!(
            "        config.rcc.pll1 = Some(rcc::Pll {{\n\
             \x20           source: rcc::PllSource::{src},\n\
             \x20           prediv: rcc::PllPreDiv::DIV{m},\n\
             \x20           mul: rcc::PllMul::MUL{n},\n\
             \x20           divp: None,\n\
             \x20           divq: None,\n\
             \x20           divr: Some(rcc::PllDiv::DIV{r}),\n\
             \x20           frac: None,\n\
             \x20       }});\n",
            src = if c.pll_src_hse { "HSE" } else { "HSI" },
            m = c.pll_m,
            n = c.pll_n,
            r = c.pll_r,
        ));
    }
    let sys = match c.sys {
        WbaSys::Hsi => "HSI",
        WbaSys::Hse => "HSE",
        WbaSys::Pll => "PLL1_R",
    };
    b.push_str(&format!("        config.rcc.sys = rcc::Sysclk::{sys};\n"));
    // Range 1 is REQUIRED for the PLL (embassy panics in range 2) and for any
    // sysclk beyond range 2's window (HSE 32 MHz). Plain HSI16 keeps range 2.
    if c.sys != WbaSys::Hsi {
        b.push_str("        config.rcc.voltage_scale = rcc::VoltageScale::RANGE1;\n");
    }
    b.push_str(&format!(
        "        config.rcc.ahb_pre = rcc::AHBPrescaler::DIV{};\n",
        c.ahb
    ));
    for (field, v) in [("apb1_pre", c.apb1), ("apb2_pre", c.apb2), ("apb7_pre", c.apb7)] {
        b.push_str(&format!(
            "        config.rcc.{field} = rcc::APBPrescaler::DIV{v};\n"
        ));
    }
    b.push_str("    }\n    let p = embassy_stm32::init(config);\n");
    b
}

/// The WBA generated section — the shared embassy shape with the WBA RCC clock
/// block spliced in.
pub fn make_generated_section(mcu_name: &str, pins: &[&Pin], clock: &ClockConfig) -> String {
    embassy_common::make_generated_section(mcu_name, pins, &clock_block(clock))
}

#[cfg(test)]
mod tests {
    use super::super::{parse_main_rs, USER_TAIL};
    use super::*;
    use crate::panels::mcu_module::pins::logic::pin::Pin;
    use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

    fn pin(name: &str, func: PinFunction) -> Pin {
        Pin {
            name: name.into(),
            number: 0,
            reserved: false,
            available_functions: vec![func.clone()],
            selected_function: func,
            custom_label: String::new(),
        }
    }

    /// A GPIO output + input + a bus pin generate compiling embassy code, and
    /// every configured pin round-trips through the shared parser.
    #[test]
    fn generates_and_round_trips_pins() {
        let pb5 = pin("PB5", PinFunction::GpioOutput);
        let pc13 = pin("PC13", PinFunction::GpioInput);
        let pa9 = pin("PA9", PinFunction::UsartTx(1));
        let refs: Vec<&Pin> = vec![&pb5, &pc13, &pa9];
        let section = make_generated_section("STM32WBA55CG", &refs, &ClockConfig::None);

        // Shape: gpio imports (both kinds), embassy init, one line per pin.
        assert!(section.contains("use embassy_stm32::gpio::{Input, Level, Output, Pull, Speed};"));
        assert!(section.contains("embassy_stm32::init(Default::default())"));
        assert!(section.contains(
            "let mut pb5_out = Output::new(p.PB5, Level::Low, Speed::Low); // GPIO Output"
        ));
        assert!(section.contains("let pc13_in = Input::new(p.PC13, Pull::None); // GPIO Input"));
        assert!(section.contains("let pa9_usart1_tx = p.PA9; // USART1  TX"));

        // Round-trip: the full file's GEN block parses back to the same pins,
        // INCLUDING the `let mut` output (the parser tweak this relies on).
        let file = format!(
            "{}{section}\n{USER_TAIL}",
            invariant_header("STM32WBA55CG", "stm32wba55cg")
        );
        let parsed = parse_main_rs(&file);
        assert!(parsed.contains(&("PB5".into(), PinFunction::GpioOutput)));
        assert!(parsed.contains(&("PC13".into(), PinFunction::GpioInput)));
        assert!(parsed.contains(&("PA9".into(), PinFunction::UsartTx(1))));
    }

    /// No configured pins → no gpio import, a placeholder comment, still valid.
    #[test]
    fn empty_config_omits_imports() {
        let section = make_generated_section("STM32WBA55CG", &[], &ClockConfig::None);
        assert!(!section.contains("use embassy_stm32::gpio"));
        assert!(section.contains("No pins configured yet"));
        assert!(section.contains("fn main() -> !"));
        // No WBA clock graph → embassy's own default init.
        assert!(section.contains("embassy_stm32::init(Default::default())"));
    }

    /// The Clock-tab graph maps onto the embassy RCC config: the default
    /// 100 MHz preset emits the exact PLL fields + RANGE1 (the PLL panics in
    /// range 2), and an HSI-reset graph falls back to `Default::default()`.
    #[test]
    fn clock_graph_maps_to_embassy_rcc_config() {
        use crate::panels::mcu_module::clock::graph::{stm32wba_graph, GraphClock};
        use crate::panels::mcu_module::clock::model::ClockConfig;

        // 100 MHz preset (the shipped default selections).
        let gc = GraphClock { graph: stm32wba_graph(), layout: Default::default() };
        let section = make_generated_section("WBA", &[], &ClockConfig::Graph(gc.clone()));
        for needle in [
            "config.rcc.hse = Some(rcc::Hse { prescaler: rcc::HsePrescaler::DIV1 });",
            "source: rcc::PllSource::HSE,",
            "prediv: rcc::PllPreDiv::DIV2,",
            "mul: rcc::PllMul::MUL25,",
            "divr: Some(rcc::PllDiv::DIV4),",
            "config.rcc.sys = rcc::Sysclk::PLL1_R;",
            "config.rcc.voltage_scale = rcc::VoltageScale::RANGE1;",
            "config.rcc.ahb_pre = rcc::AHBPrescaler::DIV1;",
            "config.rcc.apb7_pre = rcc::APBPrescaler::DIV1;",
            "let p = embassy_stm32::init(config);",
            "SYSCLK 100 MHz (HSE32 /2 x25 /4 via PLL1R)",
        ] {
            assert!(section.contains(needle), "missing: {needle}\n\n{section}");
        }

        // HSE directly as sysclk: no PLL block, but still RANGE1 + hse on.
        let mut gc2 = gc.clone();
        gc2.graph.node_mut("sw").unwrap().state =
            crate::panels::mcu_module::clock::graph::NodeState::Index(1);
        let s2 = make_generated_section("WBA", &[], &ClockConfig::Graph(gc2));
        assert!(s2.contains("config.rcc.sys = rcc::Sysclk::HSE;"));
        assert!(!s2.contains("config.rcc.pll1"));
        assert!(s2.contains("VoltageScale::RANGE1"));

        // Reset selections (HSI16, everything /1) → embassy's own default.
        let mut gc3 = gc;
        gc3.graph.node_mut("sw").unwrap().state =
            crate::panels::mcu_module::clock::graph::NodeState::Index(0);
        gc3.graph.node_mut("hse").unwrap().state =
            crate::panels::mcu_module::clock::graph::NodeState::Source {
                enabled: false,
                hz: 32_000_000,
            };
        let s3 = make_generated_section("WBA", &[], &ClockConfig::Graph(gc3));
        assert!(s3.contains("embassy_stm32::init(Default::default())"), "{s3}");
    }

    /// Splice replaces only the GEN block, keeping the user tail.
    #[test]
    fn splice_preserves_user_tail() {
        let v1 = format!(
            "{}{}\n{USER_TAIL}",
            invariant_header("X", "x"),
            make_generated_section("X", &[], &ClockConfig::None)
        );
        let edited = v1.replace(
            "// Your main loop code here.",
            "// Your main loop code here.\n        my_custom();",
        );
        let pb5 = pin("PB5", PinFunction::GpioOutput);
        let v2 = splice_section(
            &edited,
            &make_generated_section("X", &[&pb5], &ClockConfig::None),
            "X",
            "x",
        );
        assert!(v2.contains("my_custom();")); // user edit survived
        assert!(v2.contains("Output::new(p.PB5")); // new pin present
    }
}
