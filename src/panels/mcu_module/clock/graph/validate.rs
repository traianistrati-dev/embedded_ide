//! Generic, data-driven clock-graph validation (Phase 2).
//!
//! The *ceiling* checks — "frequency X must stay ≤ datasheet maximum" — are pure
//! data: any node carrying a ceiling [`LimitKey`] is compared against the chip's
//! [`ClockLimits`]. This reproduces the bulk of the hardcoded `validate.rs`
//! errors (SYSCLK / PLL / HCLK / PCLK1 / PCLK2 / ADCCLK) for *any* graph.
//!
//! Footnote rules that are NOT simple ceilings — USB must equal exactly 48 MHz,
//! the HSI→PLL 64 MHz cap, the 1 µs-ADC APB2 set, "selected path needs HSE" —
//! stay family-specific (they don't generalise) and live in the per-family
//! validator / future `FamilyBackend`.

use std::collections::BTreeMap;

use super::model::{ClockGraph, LimitKey};
use crate::panels::mcu_module::clock::model::ClockLimits;

/// One ceiling violation found in the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overflow {
    /// Node whose output exceeded its ceiling (e.g. "pclk1").
    pub node: String,
    pub key: LimitKey,
    pub hz: u32,
    pub limit: u32,
}

/// The datasheet ceiling for a *ceiling-type* limit key. `UsbclkHz` is an exact
/// requirement (≈ a footnote), not a ceiling, so it returns `None` and is not
/// checked here.
pub fn ceiling_for(key: LimitKey, l: &ClockLimits) -> Option<u32> {
    match key {
        LimitKey::SysclkMax => Some(l.sysclk_max),
        LimitKey::HclkMax => Some(l.hclk_max),
        LimitKey::Pclk1Max => Some(l.pclk1_max),
        LimitKey::Pclk2Max => Some(l.pclk2_max),
        LimitKey::AdcclkMax => Some(l.adcclk_max),
        LimitKey::UsbclkHz => None,
        // A node-carried ceiling needs no lookup.
        LimitKey::Hz(hz) => Some(hz),
    }
}

/// Every node whose evaluated frequency exceeds its declared ceiling.
/// `freqs` is the output of [`super::eval::evaluate`].
pub fn over_limits(
    graph: &ClockGraph,
    limits: &ClockLimits,
    freqs: &BTreeMap<String, u32>,
) -> Vec<Overflow> {
    let mut out = Vec::new();
    for node in &graph.nodes {
        let Some(key) = node.limit else { continue };
        let Some(ceiling) = ceiling_for(key, limits) else {
            continue;
        };
        let Some(&hz) = freqs.get(&node.id) else {
            continue;
        };
        if hz > ceiling {
            out.push(Overflow {
                node: node.id.clone(),
                key,
                hz,
                limit: ceiling,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::eval::evaluate;
    use super::super::stm32f1::stm32f1_graph;
    use super::*;
    use crate::panels::mcu_module::clock::model::Stm32f1Clock;

    fn over(c: &Stm32f1Clock, l: &ClockLimits) -> Vec<String> {
        let g = stm32f1_graph(c);
        let f = evaluate(&g);
        let mut v: Vec<String> = over_limits(&g, l, &f).into_iter().map(|o| o.node).collect();
        v.sort();
        v
    }

    #[test]
    fn default_72mhz_is_within_limits() {
        assert!(over(&Stm32f1Clock::default(), &ClockLimits::default()).is_empty());
    }

    #[test]
    fn pclk1_overflow_is_flagged() {
        let mut c = Stm32f1Clock::default();
        c.apb1_pre = 1; // PCLK1 = 72 MHz > 36
        assert!(over(&c, &ClockLimits::default()).contains(&"pclk1".to_string()));
    }

    #[test]
    fn adcclk_overflow_is_flagged() {
        let mut c = Stm32f1Clock::default();
        c.adc_pre = 2; // 72/2 = 36 MHz > 14
        assert!(over(&c, &ClockLimits::default()).contains(&"adcclk".to_string()));
    }

    #[test]
    fn custom_24mhz_limits_flag_sysclk_chain() {
        let c = Stm32f1Clock::default(); // 72 MHz everywhere
        let lim = ClockLimits {
            sysclk_max: 24_000_000,
            hclk_max: 24_000_000,
            pclk1_max: 24_000_000,
            pclk2_max: 24_000_000,
            ..ClockLimits::default()
        };
        let flagged = over(&c, &lim);
        // SYSCLK + PLLCLK taps and the bus outputs all bust a 24 MHz cap.
        for id in ["sysclk", "pllclk", "hclk", "pclk2"] {
            assert!(flagged.contains(&id.to_string()), "expected {id} flagged");
        }
    }

    /// A node-carried ceiling needs no `ClockLimits` field — this is how clocks
    /// the fixed struct doesn't name (PCLK7, ADC4, SAI…) get validated.
    #[test]
    fn a_node_carried_hz_ceiling_is_checked() {
        use super::super::eval::evaluate;
        use super::super::model::{Edge, Node, NodeKind, NodeState};

        let graph = |limit_hz: u32| ClockGraph {
            nodes: vec![
                Node {
                    id: "src".into(),
                    kind: NodeKind::Source {
                        min_hz: 100_000_000,
                        max_hz: 100_000_000,
                        gated: false,
                    },
                    state: NodeState::Source {
                        enabled: true,
                        hz: 100_000_000,
                    },
                    limit: None,
                },
                Node {
                    id: "pclk7".into(),
                    kind: NodeKind::Output,
                    state: NodeState::Fixed,
                    limit: Some(LimitKey::Hz(limit_hz)),
                },
            ],
            edges: vec![Edge {
                from: "src".into(),
                to: "pclk7".into(),
                input: 0,
            }],
        };

        let flagged = |limit_hz: u32| {
            let g = graph(limit_hz);
            let f = evaluate(&g);
            over_limits(&g, &ClockLimits::default(), &f)
        };
        assert!(
            flagged(100_000_000).is_empty(),
            "exactly at the ceiling is OK"
        );
        let over = flagged(36_000_000);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].node, "pclk7");
        assert_eq!(over[0].limit, 36_000_000);
    }

    #[test]
    fn usbclk_is_not_a_ceiling_check() {
        // USB ≠ 48 is a footnote, not a ceiling — the generic validator ignores it.
        let mut c = Stm32f1Clock::default();
        c.usb_pre = crate::panels::mcu_module::clock::model::UsbPre::Div1; // 72 MHz
        assert!(!over(&c, &ClockLimits::default()).contains(&"usbclk".to_string()));
    }
}
