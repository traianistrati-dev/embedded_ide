//! Bridge that lets the existing diagram / freq-table consume the **generic
//! graph evaluator** instead of the hardcoded `compute::frequencies` (Phase 3).
//!
//! `graph_frequencies(c)` builds the STM32F1 graph from the live config,
//! evaluates it, and packs the result into the legacy [`ClockFrequencies`]
//! struct the UI already reads. By Phase-2 equivalence this is identical to
//! `compute::frequencies`, so the diagram is now driven end-to-end by the
//! data-model evaluator with no visible change.

use std::collections::BTreeMap;

use super::eval::evaluate;
use super::layout::ValueSrc;
use super::stm32f1::stm32f1_graph;
use crate::panels::mcu_module::clock::compute::ClockFrequencies;
use crate::panels::mcu_module::clock::model::Stm32f1Clock;

/// Evaluate the clock graph for `c` and pack it into [`ClockFrequencies`].
pub fn graph_frequencies(c: &Stm32f1Clock) -> ClockFrequencies {
    let out = evaluate(&stm32f1_graph(c));
    let g = |id: &str| out.get(id).copied().unwrap_or(0);
    ClockFrequencies {
        hsi: g("hsi"),
        hse: g("hse"),
        pll_input: g("pllsrc"),
        pllclk: g("pllclk"),
        sysclk: g("sysclk"),
        hclk: g("hclk"),
        pclk1: g("pclk1"),
        pclk2: g("pclk2"),
        tim_apb1: g("tim_apb1"),
        tim_apb2: g("tim_apb2"),
        adcclk: g("adcclk"),
        usbclk: g("usbclk"),
        // `ClockFrequencies::systick` keeps the compute.rs convention (HCLK/8);
        // the diagram's SysTick box uses the faithful mux-aware value separately.
        systick: g("hclk") / 8,
        flitfclk: g("flitfclk"),
    }
}

/// The fixed graph node id a named [`ValueSrc`] maps to (F103 conveniences).
/// `Node`/`Fixed` carry their own target/constant, so they return `None` here.
pub fn value_node_id(src: &ValueSrc) -> Option<&'static str> {
    Some(match src {
        ValueSrc::Hclk => "hclk",
        ValueSrc::Sysclk => "sysclk",
        ValueSrc::Pclk1 => "pclk1",
        ValueSrc::Pclk2 => "pclk2",
        ValueSrc::Pclk1Tim => "tim_apb1",
        ValueSrc::Pclk2Tim => "tim_apb2",
        ValueSrc::Adc => "adcclk",
        ValueSrc::Usb => "usbclk",
        ValueSrc::Pllclk => "pllclk",
        ValueSrc::Flitf => "flitfclk",
        ValueSrc::Systick => "systick",
        ValueSrc::Rtc => "rtc",
        ValueSrc::Mco => "mco",
        ValueSrc::Node(_) | ValueSrc::Fixed(_) => return None,
    })
}

/// Resolve a layout [`ValueSrc`] to a frequency (Hz) directly from a graph's
/// evaluated node map — the data-driven counterpart of the typed `value_of` in
/// `gui/diagram.rs`. Lets a generic renderer fill every box/tag from the graph
/// alone (no `Stm32f1Clock` needed), for any chip.
pub fn value_from_graph(src: &ValueSrc, freqs: &BTreeMap<String, u32>) -> u32 {
    match src {
        ValueSrc::Fixed(v) => *v,
        ValueSrc::Node(id) => freqs.get(id).copied().unwrap_or(0),
        other => value_node_id(other)
            .and_then(|id| freqs.get(id).copied())
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::clock::compute::frequencies;
    use crate::panels::mcu_module::clock::model::{PllSrc, Stm32f1Clock, SysclkSrc};

    fn same(c: &Stm32f1Clock) {
        assert_eq!(graph_frequencies(c), frequencies(c));
    }

    #[test]
    fn graph_frequencies_match_compute() {
        same(&Stm32f1Clock::default());
        let mut c = Stm32f1Clock::default();
        c.sysclk_src = SysclkSrc::Hsi;
        same(&c);
        c = Stm32f1Clock::default();
        c.pll_src = PllSrc::HsiDiv2;
        c.pll_mul = 16;
        same(&c);
        c = Stm32f1Clock::default();
        c.ahb_pre = 2;
        c.apb1_pre = 4;
        c.adc_pre = 8;
        same(&c);
    }

    #[test]
    fn value_from_graph_resolves_every_box() {
        let freqs = evaluate(&stm32f1_graph(&Stm32f1Clock::default()));
        assert_eq!(value_from_graph(&ValueSrc::Hclk, &freqs), 72_000_000);
        assert_eq!(value_from_graph(&ValueSrc::Pclk1, &freqs), 36_000_000);
        assert_eq!(value_from_graph(&ValueSrc::Adc, &freqs), 12_000_000);
        assert_eq!(value_from_graph(&ValueSrc::Usb, &freqs), 48_000_000);
        assert_eq!(value_from_graph(&ValueSrc::Flitf, &freqs), 8_000_000);
        assert_eq!(value_from_graph(&ValueSrc::Systick, &freqs), 9_000_000);
        assert_eq!(value_from_graph(&ValueSrc::Fixed(40_000), &freqs), 40_000);
        // Default config: rtc_src = LSI (40 kHz), mco = disabled (0).
        assert_eq!(value_from_graph(&ValueSrc::Rtc, &freqs), 40_000);
        assert_eq!(value_from_graph(&ValueSrc::Mco, &freqs), 0);
        // Node(id) resolves an arbitrary node directly.
        assert_eq!(value_from_graph(&ValueSrc::Node("sysclk".into()), &freqs), 72_000_000);
        assert_eq!(value_from_graph(&ValueSrc::Node("nope".into()), &freqs), 0);
    }
}
