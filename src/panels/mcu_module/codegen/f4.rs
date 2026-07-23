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

use super::super::clock::graph::is_f4_graph;
use super::super::clock::model::ClockConfig;

/// The reset state (HSI, everything /1) stays `Default::default()`; anything
/// else emits an explicit config block.
pub fn clock_block(clock: &ClockConfig) -> String {
    use super::rcc::{self, ReadSpec};
    let spec = ReadSpec::f4();
    // The graph→values read and the emit are BOTH generic (rcc.rs); only the
    // ReadSpec + RccDescriptor are F4-specific.
    let v = match clock {
        ClockConfig::Graph(gc) if is_f4_graph(&gc.graph) => rcc::read_rcc_values(&gc.graph, &spec),
        _ => spec.reset.clone(),
    };
    if v == spec.reset {
        return "    let p = embassy_stm32::init(Default::default()); // HSI 16 MHz, all buses /1\n"
            .to_string();
    }
    rcc::emit_rcc_block(&rcc::RccDescriptor::f4(), &v)
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
