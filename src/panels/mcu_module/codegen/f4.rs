//! STM32F4 clock block — maps the Clock-tab graph onto `embassy_stm32::Config`
//! (RCC). Shares the rest of the codegen with every embassy STM32 family via
//! [`embassy_common`](super::embassy_common); only this RCC mapping is
//! F4-specific.
//!
//! API facts verified against embassy-stm32 v0.4.0 `src/rcc/f247.rs`: `Config`
//! fields `hse: Option<Hse>` / `pll_src: PllSource` / `pll: Option<Pll>` /
//! `sys: Sysclk` / `ahb_pre` / `apb1_pre` / `apb2_pre`; `Pll { prediv, mul,
//! divp, divq, divr }`; sysclk-from-PLL variant is `Sysclk::PLL1_P` (the PLL's
//! P output); `Hse { freq: Hertz, mode: HseMode }`.

use super::super::clock::graph::{graph_to_f4, is_f4_graph, F4Clock, F4Sys};
use super::super::clock::model::ClockConfig;

/// The reset state (HSI, everything /1) stays `Default::default()`; anything
/// else emits an explicit config block.
pub fn clock_block(clock: &ClockConfig) -> String {
    let c: F4Clock = match clock {
        ClockConfig::Graph(gc) if is_f4_graph(&gc.graph) => graph_to_f4(&gc.graph),
        _ => F4Clock::default(),
    };
    if c == F4Clock::default() {
        return "    let p = embassy_stm32::init(Default::default()); // HSI 16 MHz, all buses /1\n"
            .to_string();
    }

    let mhz = c.sysclk_hz() / 1_000_000;
    let hse_mhz = c.hse_hz / 1_000_000;
    let sys_desc = match c.sys {
        F4Sys::Hsi => "HSI16".to_string(),
        F4Sys::Hse => format!("HSE {hse_mhz} MHz"),
        F4Sys::Pll => format!(
            "{} /{} x{} /{} via PLLP",
            if c.pll_src_hse {
                format!("HSE {hse_mhz} MHz")
            } else {
                "HSI16".to_string()
            },
            c.pll_m,
            c.pll_n,
            c.pll_p
        ),
    };

    let mut b = String::new();
    b.push_str(&format!(
        "    // Clock (from the Clock tab): SYSCLK {mhz} MHz ({sys_desc}) · \
         AHB /{} · APB1 /{} APB2 /{}\n",
        c.ahb, c.apb1, c.apb2
    ));
    b.push_str("    let mut config = embassy_stm32::Config::default();\n");
    b.push_str("    {\n        use embassy_stm32::rcc;\n");

    let hse_used = c.sys == F4Sys::Hse || (c.sys == F4Sys::Pll && c.pll_src_hse);
    if hse_used {
        b.push_str(&format!(
            "        config.rcc.hse = Some(rcc::Hse {{ freq: embassy_stm32::time::Hertz({}), \
             mode: rcc::HseMode::Oscillator }});\n",
            c.hse_hz
        ));
    }
    if c.sys == F4Sys::Pll {
        b.push_str(&format!(
            "        config.rcc.pll_src = rcc::PllSource::{};\n",
            if c.pll_src_hse { "HSE" } else { "HSI" }
        ));
        b.push_str(&format!(
            "        config.rcc.pll = Some(rcc::Pll {{\n\
             \x20           prediv: rcc::PllPreDiv::DIV{m},\n\
             \x20           mul: rcc::PllMul::MUL{n},\n\
             \x20           divp: Some(rcc::PllPDiv::DIV{p}),\n\
             \x20           divq: None,\n\
             \x20           divr: None,\n\
             \x20       }});\n",
            m = c.pll_m,
            n = c.pll_n,
            p = c.pll_p,
        ));
    }
    let sys = match c.sys {
        F4Sys::Hsi => "HSI",
        F4Sys::Hse => "HSE",
        F4Sys::Pll => "PLL1_P",
    };
    b.push_str(&format!("        config.rcc.sys = rcc::Sysclk::{sys};\n"));
    b.push_str(&format!(
        "        config.rcc.ahb_pre = rcc::AHBPrescaler::DIV{};\n",
        c.ahb
    ));
    b.push_str(&format!(
        "        config.rcc.apb1_pre = rcc::APBPrescaler::DIV{};\n",
        c.apb1
    ));
    b.push_str(&format!(
        "        config.rcc.apb2_pre = rcc::APBPrescaler::DIV{};\n",
        c.apb2
    ));
    b.push_str("    }\n    let p = embassy_stm32::init(config);\n");
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::clock::graph::{stm32f4_graph, GraphClock};
    use crate::panels::mcu_module::clock::graph::model::NodeState;

    #[test]
    fn default_preset_maps_to_hsi_pll_100mhz() {
        let gc = GraphClock { graph: stm32f4_graph(), layout: Default::default() };
        let s = clock_block(&ClockConfig::Graph(gc));
        for needle in [
            "config.rcc.pll_src = rcc::PllSource::HSI;",
            "prediv: rcc::PllPreDiv::DIV8,",
            "mul: rcc::PllMul::MUL100,",
            "divp: Some(rcc::PllPDiv::DIV2),",
            "config.rcc.sys = rcc::Sysclk::PLL1_P;",
            "config.rcc.apb1_pre = rcc::APBPrescaler::DIV2;",
            "config.rcc.apb2_pre = rcc::APBPrescaler::DIV1;",
            "let p = embassy_stm32::init(config);",
            "SYSCLK 100 MHz (HSI16 /8 x100 /2 via PLLP)",
        ] {
            assert!(s.contains(needle), "missing: {needle}\n\n{s}");
        }
        assert!(!s.contains("config.rcc.hse"), "HSI preset needs no HSE");
    }

    #[test]
    fn hse_pll_emits_hse_block_and_source() {
        let mut g = stm32f4_graph();
        g.node_mut("hse").unwrap().state = NodeState::Source { enabled: true, hz: 25_000_000 };
        g.node_mut("pllsrc").unwrap().state = NodeState::Index(1); // HSE
        g.node_mut("pllm").unwrap().state = NodeState::Index(5); // /25
        g.node_mut("plln").unwrap().state = NodeState::Value(160);
        let gc = GraphClock { graph: g, layout: Default::default() };
        let s = clock_block(&ClockConfig::Graph(gc));
        assert!(s.contains("config.rcc.hse = Some(rcc::Hse { freq: embassy_stm32::time::Hertz(25000000), mode: rcc::HseMode::Oscillator });"));
        assert!(s.contains("config.rcc.pll_src = rcc::PllSource::HSE;"));
        assert!(s.contains("prediv: rcc::PllPreDiv::DIV25,"));
        assert!(s.contains("mul: rcc::PllMul::MUL160,"));
    }

    #[test]
    fn reset_state_falls_back_to_default_init() {
        assert!(clock_block(&ClockConfig::None).contains("embassy_stm32::init(Default::default())"));
    }
}
