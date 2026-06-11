//! STM32F1 clock tree expressed as a generic [`ClockGraph`] (Phase 1 bridge).
//!
//! [`stm32f1_graph`] maps the typed [`Stm32f1Clock`] config onto the data-driven
//! graph. Its only job in Phase 1 is to *prove* the generic evaluator reproduces
//! the hardcoded `compute::frequencies` exactly (the equivalence tests below).
//! Later phases move this topology into the chip's `.ron` and retire the typed
//! struct; for now both run side by side.

use super::model::{ClockGraph, Edge, LimitKey, Node, NodeKind, NodeState};
use crate::panels::mcu_module::clock::model::{
    Mco, PllSrc, RtcSrc, Stm32f1Clock, SysclkSrc, SystickSrc, UsbPre, ADC_PRESCALERS,
    AHB_PRESCALERS, APB_PRESCALERS, HSE_MAX_HZ, HSE_MIN_HZ, HSI_HZ, PLL_MUL_MAX, PLL_MUL_MIN,
};

fn n(id: &str, kind: NodeKind, state: NodeState) -> Node {
    Node { id: id.into(), kind, state, limit: None }
}
fn n_lim(id: &str, kind: NodeKind, state: NodeState, limit: LimitKey) -> Node {
    Node { id: id.into(), kind, state, limit: Some(limit) }
}
fn e(from: &str, to: &str) -> Edge {
    Edge { from: from.into(), to: to.into(), input: 0 }
}
fn e_in(from: &str, to: &str, input: usize) -> Edge {
    Edge { from: from.into(), to: to.into(), input }
}

/// Build the STM32F103 clock graph with selections taken from `c`.
pub fn stm32f1_graph(c: &Stm32f1Clock) -> ClockGraph {
    let ahb: Vec<u32> = AHB_PRESCALERS.iter().map(|&x| x as u32).collect();
    let apb: Vec<u32> = APB_PRESCALERS.iter().map(|&x| x as u32).collect();
    let adc: Vec<u32> = ADC_PRESCALERS.iter().map(|&x| x as u32).collect();
    let idx = |opts: &[u32], v: u32| opts.iter().position(|&x| x == v).unwrap_or(0);

    // PLL source: the mux picks HSI/2 (0) or the HSE branch (1); the HSE branch's
    // PLLXTPRE choice does the /1 or /2.
    let pll_src_idx = if c.pll_src == PllSrc::HsiDiv2 { 0 } else { 1 };
    let pllxtpre_idx = if c.pll_src == PllSrc::HseDiv2 { 1 } else { 0 };
    let sw_idx = match c.sysclk_src {
        SysclkSrc::Hsi => 0,
        SysclkSrc::Hse => 1,
        SysclkSrc::Pll => 2,
    };
    let usb_idx = match c.usb_pre {
        UsbPre::Div1_5 => 0,
        UsbPre::Div1 => 1,
    };
    let systick_idx = match c.systick_src {
        SystickSrc::HclkDiv8 => 0,
        SystickSrc::Hclk => 1,
    };
    // RTC / MCO muxes can be disabled (→ Unset → 0).
    let rtc_state = match c.rtc_src {
        RtcSrc::None => NodeState::Unset,
        RtcSrc::HseDiv128 => NodeState::Index(0),
        RtcSrc::Lse => NodeState::Index(1),
        RtcSrc::Lsi => NodeState::Index(2),
    };
    let mco_state = match c.mco {
        Mco::None => NodeState::Unset,
        Mco::Sysclk => NodeState::Index(0),
        Mco::Hsi => NodeState::Index(1),
        Mco::Hse => NodeState::Index(2),
        Mco::PllDiv2 => NodeState::Index(3),
    };

    let nodes = vec![
        // ── Oscillators ──
        n("hsi", NodeKind::Source { min_hz: HSI_HZ, max_hz: HSI_HZ, gated: false },
          NodeState::Source { enabled: true, hz: HSI_HZ }),
        n("hse", NodeKind::Source { min_hz: HSE_MIN_HZ, max_hz: HSE_MAX_HZ, gated: true },
          NodeState::Source { enabled: c.hse_enabled, hz: c.hse_hz }),
        // ── PLL input chain ──
        n("hsi_div2", NodeKind::FixedDiv { by: 2 }, NodeState::Fixed),
        n("pllxtpre", NodeKind::Choice { ratios: vec![(1, 1), (1, 2)] }, NodeState::Index(pllxtpre_idx)),
        n("pllsrc", NodeKind::Mux { inputs: 2 }, NodeState::Index(pll_src_idx)),
        n("pllmul", NodeKind::Multiplier { min: PLL_MUL_MIN as u32, max: PLL_MUL_MAX as u32 },
          NodeState::Value(c.pll_mul as u32)),
        n_lim("pllclk", NodeKind::Tap, NodeState::Fixed, LimitKey::SysclkMax),
        // ── System clock + buses ──
        n("sw", NodeKind::Mux { inputs: 3 }, NodeState::Index(sw_idx)),
        n_lim("sysclk", NodeKind::Tap, NodeState::Fixed, LimitKey::SysclkMax),
        n("ahb", NodeKind::Divider { options: ahb.clone() }, NodeState::Index(idx(&ahb, c.ahb_pre as u32))),
        n_lim("hclk", NodeKind::Output, NodeState::Fixed, LimitKey::HclkMax),
        n("apb1", NodeKind::Divider { options: apb.clone() }, NodeState::Index(idx(&apb, c.apb1_pre as u32))),
        n_lim("pclk1", NodeKind::Output, NodeState::Fixed, LimitKey::Pclk1Max),
        n("apb2", NodeKind::Divider { options: apb.clone() }, NodeState::Index(idx(&apb, c.apb2_pre as u32))),
        n_lim("pclk2", NodeKind::Output, NodeState::Fixed, LimitKey::Pclk2Max),
        n("adc", NodeKind::Divider { options: adc.clone() }, NodeState::Index(idx(&adc, c.adc_pre as u32))),
        n_lim("adcclk", NodeKind::Output, NodeState::Fixed, LimitKey::AdcclkMax),
        // ── Timer clocks (×2 rule) ──
        n("tim_apb1", NodeKind::TimerMul { prescaler: "apb1".into() }, NodeState::Fixed),
        n("tim_apb2", NodeKind::TimerMul { prescaler: "apb2".into() }, NodeState::Fixed),
        // ── USB / SysTick / Flash ──
        n("usb", NodeKind::Choice { ratios: vec![(2, 3), (1, 1)] }, NodeState::Index(usb_idx)),
        n_lim("usbclk", NodeKind::Output, NodeState::Fixed, LimitKey::UsbclkHz),
        n("systick", NodeKind::Choice { ratios: vec![(1, 8), (1, 1)] }, NodeState::Index(systick_idx)),
        n("flitfclk", NodeKind::Tap, NodeState::Fixed),
        // ── Low-speed oscillators + RTC / MCO (diagram-only outputs) ──
        n("lse", NodeKind::Source { min_hz: 32_768, max_hz: 32_768, gated: true },
          NodeState::Source { enabled: true, hz: 32_768 }),
        n("lsi", NodeKind::Source { min_hz: 40_000, max_hz: 40_000, gated: false },
          NodeState::Source { enabled: true, hz: 40_000 }),
        n("hse_div128", NodeKind::FixedDiv { by: 128 }, NodeState::Fixed),
        n("pll_div2", NodeKind::FixedDiv { by: 2 }, NodeState::Fixed),
        n("rtc", NodeKind::Mux { inputs: 3 }, rtc_state),
        n("mco", NodeKind::Mux { inputs: 4 }, mco_state),
    ];

    let edges = vec![
        e("hsi", "hsi_div2"),
        e("hse", "pllxtpre"),
        e_in("hsi_div2", "pllsrc", 0),
        e_in("pllxtpre", "pllsrc", 1),
        e("pllsrc", "pllmul"),
        e("pllmul", "pllclk"),
        e_in("hsi", "sw", 0),
        e_in("hse", "sw", 1),
        e_in("pllclk", "sw", 2),
        e("sw", "sysclk"),
        e("sysclk", "ahb"),
        e("ahb", "hclk"),
        e("hclk", "apb1"),
        e("apb1", "pclk1"),
        e("hclk", "apb2"),
        e("apb2", "pclk2"),
        e("pclk2", "adc"),
        e("adc", "adcclk"),
        e("pclk1", "tim_apb1"),
        e("pclk2", "tim_apb2"),
        e("pllclk", "usb"),
        e("usb", "usbclk"),
        e("hclk", "systick"),
        e("hsi", "flitfclk"),
        // RTC mux: HSE/128, LSE, LSI.
        e("hse", "hse_div128"),
        e_in("hse_div128", "rtc", 0),
        e_in("lse", "rtc", 1),
        e_in("lsi", "rtc", 2),
        // MCO mux: SYSCLK, HSI, HSE, PLL/2.
        e("pllclk", "pll_div2"),
        e_in("sysclk", "mco", 0),
        e_in("hsi", "mco", 1),
        e_in("hse", "mco", 2),
        e_in("pll_div2", "mco", 3),
    ];

    ClockGraph { nodes, edges }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::eval::evaluate;
    use super::super::model::ClockGraph;
    use crate::panels::mcu_module::clock::compute::frequencies;
    use crate::panels::mcu_module::clock::model::{PllSrc, Stm32f1Clock, SysclkSrc, SystickSrc, UsbPre};

    /// The generic graph evaluator must reproduce `compute::frequencies` exactly.
    fn assert_equiv(c: &Stm32f1Clock) {
        let out = evaluate(&stm32f1_graph(c));
        let f = frequencies(c);
        let g = |id: &str| out.get(id).copied().unwrap_or(0);

        assert_eq!(g("pllsrc"), f.pll_input, "pll_input");
        assert_eq!(g("pllclk"), f.pllclk, "pllclk");
        assert_eq!(g("sysclk"), f.sysclk, "sysclk");
        assert_eq!(g("hclk"), f.hclk, "hclk");
        assert_eq!(g("pclk1"), f.pclk1, "pclk1");
        assert_eq!(g("pclk2"), f.pclk2, "pclk2");
        assert_eq!(g("adcclk"), f.adcclk, "adcclk");
        assert_eq!(g("usbclk"), f.usbclk, "usbclk");
        assert_eq!(g("tim_apb1"), f.tim_apb1, "tim_apb1");
        assert_eq!(g("tim_apb2"), f.tim_apb2, "tim_apb2");
        // `compute.rs` hardcodes `systick = hclk/8`; the graph (like the diagram's
        // `systick_hz`) honours the SysTick source mux, so check the faithful value.
        let expected_systick = match c.systick_src {
            SystickSrc::HclkDiv8 => f.hclk / 8,
            SystickSrc::Hclk => f.hclk,
        };
        assert_eq!(g("systick"), expected_systick, "systick");
        assert_eq!(g("flitfclk"), f.flitfclk, "flitfclk");
    }

    #[test]
    fn default_72mhz_blue_pill() {
        assert_equiv(&Stm32f1Clock::default());
    }

    /// The newly-added RTC / MCO nodes evaluate like the diagram's helpers.
    #[test]
    fn rtc_and_mco_nodes_evaluate_correctly() {
        use crate::panels::mcu_module::clock::model::{Mco, RtcSrc};

        let mut c = Stm32f1Clock::default(); // HSE 8 MHz · SYSCLK/PLLCLK 72 MHz
        for (src, expect) in [
            (RtcSrc::None, 0u32),
            (RtcSrc::HseDiv128, 8_000_000 / 128),
            (RtcSrc::Lse, 32_768),
            (RtcSrc::Lsi, 40_000),
        ] {
            c.rtc_src = src;
            let f = evaluate(&stm32f1_graph(&c));
            assert_eq!(f.get("rtc").copied().unwrap_or(0), expect, "rtc {src:?}");
        }

        c = Stm32f1Clock::default();
        for (mco, expect) in [
            (Mco::None, 0u32),
            (Mco::Sysclk, 72_000_000),
            (Mco::Hsi, 8_000_000),
            (Mco::Hse, 8_000_000),
            (Mco::PllDiv2, 36_000_000),
        ] {
            c.mco = mco;
            let f = evaluate(&stm32f1_graph(&c));
            assert_eq!(f.get("mco").copied().unwrap_or(0), expect, "mco {mco:?}");
        }
    }

    #[test]
    fn hsi_direct() {
        let mut c = Stm32f1Clock::default();
        c.sysclk_src = SysclkSrc::Hsi;
        assert_equiv(&c);
    }

    #[test]
    fn hsi_div2_pll_x16_64mhz() {
        let mut c = Stm32f1Clock::default();
        c.pll_src = PllSrc::HsiDiv2;
        c.pll_mul = 16;
        assert_equiv(&c);
    }

    #[test]
    fn hse_div2_branch() {
        let mut c = Stm32f1Clock::default();
        c.pll_src = PllSrc::HseDiv2;
        assert_equiv(&c);
    }

    #[test]
    fn hse_disabled_zeroes_pll_chain() {
        let mut c = Stm32f1Clock::default();
        c.hse_enabled = false;
        assert_equiv(&c);
    }

    #[test]
    fn modified_prescalers() {
        let mut c = Stm32f1Clock::default();
        c.ahb_pre = 2;
        c.apb1_pre = 4;
        c.apb2_pre = 2;
        c.adc_pre = 8;
        assert_equiv(&c);
    }

    #[test]
    fn usb_div1_and_systick_undivided() {
        let mut c = Stm32f1Clock::default();
        c.usb_pre = UsbPre::Div1;
        c.systick_src = SystickSrc::Hclk;
        assert_equiv(&c);
    }

    /// The graph is serializable (forward-looking: the topology will live in
    /// each chip's `.ron`).
    #[test]
    fn graph_round_trips_via_ron() {
        let g = stm32f1_graph(&Stm32f1Clock::default());
        let text = ron::to_string(&g).expect("serialize graph");
        let back: ClockGraph = ron::from_str(&text).expect("parse graph");
        assert_eq!(g, back, "RON round-trip must be lossless");
    }

    // ── Parallel verification across the whole config space ──────────────────

    /// Map a ceiling-violation node id to its clock name.
    fn ceiling_clock(node: &str) -> Option<&'static str> {
        match node {
            "sysclk" => Some("sysclk"),
            "pllclk" => Some("pllclk"),
            "hclk" => Some("hclk"),
            "pclk1" => Some("pclk1"),
            "pclk2" => Some("pclk2"),
            "adcclk" => Some("adcclk"),
            _ => None,
        }
    }

    /// Map a `validate.rs` ceiling-error message to its clock name. Footnote
    /// messages ("With HSI…", "Selected clock path…") return `None`.
    fn validate_clock(msg: &str) -> Option<&'static str> {
        if msg.starts_with("SYSCLK ") {
            Some("sysclk")
        } else if msg.starts_with("PLL output") {
            Some("pllclk")
        } else if msg.starts_with("HCLK") {
            Some("hclk")
        } else if msg.starts_with("PCLK1") {
            Some("pclk1")
        } else if msg.starts_with("PCLK2") {
            Some("pclk2")
        } else if msg.starts_with("ADCCLK") {
            Some("adcclk")
        } else {
            None
        }
    }

    /// The graph (frequencies + ceiling violations) must agree with the
    /// hardcoded `compute.rs` + `validate.rs` for every config in a broad sweep.
    #[test]
    fn graph_matches_compute_and_validate_across_config_space() {
        use super::super::validate::over_limits;
        use crate::panels::mcu_module::clock::model::ClockLimits;
        use crate::panels::mcu_module::clock::validate::{warnings, Severity};
        use std::collections::BTreeSet;

        let limits = ClockLimits::default();
        let mut checked = 0u32;

        for &src in &[SysclkSrc::Hsi, SysclkSrc::Hse, SysclkSrc::Pll] {
            for &pll in &[PllSrc::HsiDiv2, PllSrc::Hse, PllSrc::HseDiv2] {
                for &mul in &[2u8, 6, 9, 12, 16] {
                    for &ahb in &[1u16, 2] {
                        for &apb1 in &[1u8, 2, 4] {
                            for &apb2 in &[1u8, 2] {
                                for &adc in &[2u8, 4, 6, 8] {
                                    for &hse_on in &[true, false] {
                                        let c = Stm32f1Clock {
                                            sysclk_src: src,
                                            pll_src: pll,
                                            pll_mul: mul,
                                            ahb_pre: ahb,
                                            apb1_pre: apb1,
                                            apb2_pre: apb2,
                                            adc_pre: adc,
                                            hse_enabled: hse_on,
                                            ..Stm32f1Clock::default()
                                        };

                                        let g = stm32f1_graph(&c);
                                        let fg = evaluate(&g);
                                        let fr = frequencies(&c);
                                        let get = |id: &str| fg.get(id).copied().unwrap_or(0);

                                        // Frequencies must match exactly.
                                        assert_eq!(get("pllclk"), fr.pllclk, "pllclk {c:?}");
                                        assert_eq!(get("sysclk"), fr.sysclk, "sysclk {c:?}");
                                        assert_eq!(get("hclk"), fr.hclk, "hclk {c:?}");
                                        assert_eq!(get("pclk1"), fr.pclk1, "pclk1 {c:?}");
                                        assert_eq!(get("pclk2"), fr.pclk2, "pclk2 {c:?}");
                                        assert_eq!(get("adcclk"), fr.adcclk, "adcclk {c:?}");
                                        assert_eq!(get("usbclk"), fr.usbclk, "usbclk {c:?}");
                                        assert_eq!(get("tim_apb1"), fr.tim_apb1, "tim1 {c:?}");
                                        assert_eq!(get("tim_apb2"), fr.tim_apb2, "tim2 {c:?}");

                                        // Ceiling violations must match.
                                        let graph_over: BTreeSet<&str> = over_limits(&g, &limits, &fg)
                                            .iter()
                                            .filter_map(|o| ceiling_clock(&o.node))
                                            .collect();
                                        let real_over: BTreeSet<&str> = warnings(&c, &fr, &limits)
                                            .iter()
                                            .filter(|w| w.severity == Severity::Error)
                                            .filter_map(|w| validate_clock(&w.msg))
                                            .collect();
                                        assert_eq!(graph_over, real_over, "ceilings {c:?}");

                                        checked += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(checked > 1000, "sweep should cover a broad space (got {checked})");
    }
}
