//! ESP32-C3 clock tree as a data-driven graph + diagram.
//!
//! Demonstrates a SECOND family shipping its own importable clock: a completely
//! different topology from STM32F1, expressed with the same generic node kinds
//! and rendered by the same `draw_static_diagram`. Frequencies follow the
//! ESP32-C3 TRM defaults — SPLL 480 MHz, CPU 160 MHz, APB 80 MHz,
//! RTC_FAST ≈ 17.5 MHz, RTC_SLOW ≈ 136 kHz.

use super::layout::{BlockDef, ClockLayout, LabelDef, OutputDef, TagDef, ValueSrc, Widget};
use super::model::{ClockGraph, Edge, Node, NodeKind, NodeState};

fn n(id: &str, kind: NodeKind, state: NodeState) -> Node {
    Node {
        id: id.into(),
        kind,
        state,
        limit: None,
    }
}
fn e(from: &str, to: &str) -> Edge {
    Edge {
        from: from.into(),
        to: to.into(),
        input: 0,
    }
}
fn e_in(from: &str, to: &str, input: usize) -> Edge {
    Edge {
        from: from.into(),
        to: to.into(),
        input,
    }
}

/// The ESP32-C3 clock graph (default selections).
pub fn esp32c3_graph() -> ClockGraph {
    let src = |hz: u32| NodeState::Source { enabled: true, hz };

    let nodes = vec![
        // ── Oscillators ──
        n(
            "xtal",
            NodeKind::Source {
                min_hz: 40_000_000,
                max_hz: 40_000_000,
                gated: false,
            },
            src(40_000_000),
        ),
        n(
            "rc_fast",
            NodeKind::Source {
                min_hz: 17_500_000,
                max_hz: 17_500_000,
                gated: false,
            },
            src(17_500_000),
        ),
        n(
            "rc_slow",
            NodeKind::Source {
                min_hz: 136_000,
                max_hz: 136_000,
                gated: false,
            },
            src(136_000),
        ),
        n(
            "xtal32k",
            NodeKind::Source {
                min_hz: 32_768,
                max_hz: 32_768,
                gated: true,
            },
            src(32_768),
        ),
        n(
            "rc32k",
            NodeKind::Source {
                min_hz: 32_000,
                max_hz: 32_000,
                gated: false,
            },
            src(32_000),
        ),
        // ── SPLL (480 MHz) ──
        n(
            "pll",
            NodeKind::Multiplier { min: 12, max: 12 },
            NodeState::Value(12),
        ),
        // ── CPU clock: PLL ÷{3,6} → mux{xtal, pll_div, rc_fast} ──
        n(
            "cpu_div",
            NodeKind::Divider {
                options: vec![3, 6],
            },
            NodeState::Index(0),
        ), // 480/3 = 160
        n("cpu", NodeKind::Mux { inputs: 3 }, NodeState::Index(1)), // PLL path
        // ── APB (PLL/6 = 80 MHz) ──
        n("apb", NodeKind::FixedDiv { by: 6 }, NodeState::Fixed),
        // ── RTC fast/slow ──
        n("xtal_d2", NodeKind::FixedDiv { by: 2 }, NodeState::Fixed), // 20 MHz
        n("rtc_fast", NodeKind::Mux { inputs: 2 }, NodeState::Index(1)), // RC_FAST default
        n("rtc_slow", NodeKind::Mux { inputs: 3 }, NodeState::Index(0)), // RC_SLOW default
    ];

    let edges = vec![
        e("xtal", "pll"),
        e("pll", "cpu_div"),
        e_in("xtal", "cpu", 0),
        e_in("cpu_div", "cpu", 1),
        e_in("rc_fast", "cpu", 2),
        e("pll", "apb"),
        e("xtal", "xtal_d2"),
        e_in("xtal_d2", "rtc_fast", 0),
        e_in("rc_fast", "rtc_fast", 1),
        e_in("rc_slow", "rtc_slow", 0),
        e_in("xtal32k", "rtc_slow", 1),
        e_in("rc32k", "rtc_slow", 2),
    ];

    ClockGraph { nodes, edges }
}

/// A clean left→right diagram layout for the ESP32-C3 clock tree.
pub fn esp32c3_layout() -> ClockLayout {
    let blk = |x, y, w, h, label: &str, node: &str| BlockDef {
        x,
        y,
        w,
        h,
        label: label.to_owned(),
        node: (!node.is_empty()).then(|| node.to_owned()),
    };
    let out = |x, y, w, h, label: &str, node: &str| OutputDef {
        x,
        y,
        w,
        h,
        label: label.to_owned(),
        src: ValueSrc::Node(node.to_owned()),
        limit: None,
    };
    let tag = |x, y, name: &str, node: &str| TagDef {
        x,
        y,
        name: name.to_owned(),
        src: ValueSrc::Node(node.to_owned()),
        limit: None,
    };
    let lbl = |x, y, text: &str, node: &str| LabelDef {
        x,
        y,
        text: text.to_owned(),
        node: (!node.is_empty()).then(|| node.to_owned()),
    };
    // CubeMX-style trapezoid mux (many inputs → 1 output). Inputs are labelled
    // stubs (label, dy, mux index) so no long crossing wires are needed.
    let mux = |node: &str, x, y, w, h, inputs: Vec<(String, f32, NodeState)>| Widget::MuxRadios {
        node: node.to_owned(),
        x,
        y,
        w,
        h,
        flip: false,
        inputs,
    };
    let mi = |label: &str, dy: f32, i: usize| (label.to_owned(), dy, NodeState::Index(i));

    ClockLayout {
        // Hand-authored: the primitives below ARE the layout, so there are
        // no node boxes to derive them from.
        nodes: Vec::new(),
        blocks: vec![
            blk(28.0, 92.0, 120.0, 36.0, "XTAL\n40 MHz", "xtal"),
            blk(28.0, 300.0, 120.0, 36.0, "RC_FAST\n17.5 MHz", "rc_fast"),
            blk(28.0, 470.0, 120.0, 36.0, "RC_SLOW\n136 kHz", "rc_slow"),
            blk(28.0, 550.0, 120.0, 36.0, "XTAL32K\n32.768 kHz", "xtal32k"),
            blk(28.0, 630.0, 120.0, 36.0, "RC32K\n~32 kHz", "rc32k"),
            blk(220.0, 92.0, 120.0, 36.0, "SPLL\n×12 -> 480 MHz", "pll"),
            blk(220.0, 250.0, 56.0, 24.0, "/2", "xtal_d2"),
            blk(390.0, 200.0, 95.0, 28.0, "APB /6", "apb"),
        ],
        outputs: vec![
            out(720.0, 105.0, 210.0, 30.0, "CPU_CLK -> core", "cpu"),
            out(720.0, 200.0, 210.0, 30.0, "APB_CLK -> peripherals", "apb"),
            out(720.0, 303.0, 210.0, 30.0, "RTC_FAST_CLK", "rtc_fast"),
            out(720.0, 485.0, 210.0, 30.0, "RTC_SLOW_CLK", "rtc_slow"),
        ],
        tags: vec![
            tag(655.0, 120.0, "CPU", "cpu"),
            tag(505.0, 214.0, "APB", "apb"),
        ],
        labels_above: vec![
            lbl(220.0, 86.0, "PLL ×12", "pll"),
            lbl(390.0, 91.0, "CPU divider", "cpu_div"),
        ],
        // Mux names (CENTER_BOTTOM-anchored above each trapezoid).
        mux_titles: vec![
            lbl(582.0, 76.0, "CPU Mux", "cpu"),
            lbl(422.0, 284.0, "RTC Fast", "rtc_fast"),
            lbl(422.0, 452.0, "RTC Slow", "rtc_slow"),
        ],
        wires: vec![
            vec![(148.0, 110.0), (220.0, 110.0)], // xtal → pll
            vec![(340.0, 110.0), (390.0, 110.0)], // pll → cpu_div
            vec![(485.0, 110.0), (536.0, 110.0)], // cpu_div → cpu mux (PLL input)
            vec![(605.0, 120.0), (720.0, 120.0)], // cpu mux → CPU_CLK
            vec![(280.0, 128.0), (280.0, 214.0), (390.0, 214.0)], // pll → apb
            vec![(485.0, 214.0), (720.0, 214.0)], // apb → APB_CLK
            vec![(88.0, 128.0), (88.0, 262.0), (220.0, 262.0)], // xtal → /2
            vec![(445.0, 318.0), (720.0, 318.0)], // RTC_FAST mux → out
            vec![(445.0, 500.0), (720.0, 500.0)], // RTC_SLOW mux → out
        ],
        widgets: vec![
            Widget::Combo {
                node: "cpu_div".to_owned(),
                x: 390.0,
                y: 97.0,
                w: 95.0,
                options: vec![
                    ("/3 -> 160 MHz".to_owned(), NodeState::Index(0)),
                    ("/6 -> 80 MHz".to_owned(), NodeState::Index(1)),
                ],
            },
            mux(
                "cpu",
                560.0,
                84.0,
                45.0,
                72.0,
                vec![
                    mi("XTAL", 10.0, 0),
                    mi("PLL ÷N", 26.0, 1),
                    mi("RC_FAST", 52.0, 2),
                ],
            ),
            mux(
                "rtc_fast",
                400.0,
                292.0,
                45.0,
                52.0,
                vec![mi("XTAL/2", 14.0, 0), mi("RC_FAST", 38.0, 1)],
            ),
            mux(
                "rtc_slow",
                400.0,
                460.0,
                45.0,
                80.0,
                vec![
                    mi("RC_SLOW", 16.0, 0),
                    mi("XTAL32K", 40.0, 1),
                    mi("RC32K", 64.0, 2),
                ],
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::super::eval::evaluate;
    use super::*;

    #[test]
    fn esp32c3_default_frequencies() {
        let f = evaluate(&esp32c3_graph());
        let g = |id: &str| f.get(id).copied().unwrap_or(0);
        assert_eq!(g("pll"), 480_000_000, "SPLL");
        assert_eq!(g("cpu"), 160_000_000, "CPU 480/3");
        assert_eq!(g("apb"), 80_000_000, "APB 480/6");
        assert_eq!(g("rtc_fast"), 17_500_000, "RTC_FAST = RC_FAST");
        assert_eq!(g("rtc_slow"), 136_000, "RTC_SLOW = RC_SLOW");
        assert_eq!(g("xtal_d2"), 20_000_000, "XTAL/2");
    }

    #[test]
    fn cpu_div6_gives_80mhz() {
        let mut graph = esp32c3_graph();
        graph.node_mut("cpu_div").unwrap().state = NodeState::Index(1); // /6
        let f = evaluate(&graph);
        assert_eq!(f.get("cpu").copied().unwrap_or(0), 80_000_000);
    }

    #[test]
    fn layout_outputs_reference_real_nodes() {
        let g = esp32c3_graph();
        let lay = esp32c3_layout();
        for o in &lay.outputs {
            if let ValueSrc::Node(id) = &o.src {
                assert!(g.node(id).is_some(), "output references missing node {id}");
            }
        }
    }

    #[test]
    fn widgets_reference_real_nodes_with_options() {
        let g = esp32c3_graph();
        for w in &esp32c3_layout().widgets {
            let node = w.node_id();
            assert!(
                g.node(node).is_some(),
                "widget references missing node {node}"
            );
            if let Widget::Combo { options, .. } = w {
                assert!(!options.is_empty(), "widget {node} has no options");
            }
        }
    }

    /// Applying a widget option (the interactive editor's job) changes the
    /// evaluated frequency: picking XTAL as the CPU source → 40 MHz.
    #[test]
    fn selecting_cpu_source_xtal_gives_40mhz() {
        let mut g = esp32c3_graph();
        g.node_mut("cpu").unwrap().state = NodeState::Index(0); // XTAL
        assert_eq!(evaluate(&g).get("cpu").copied().unwrap_or(0), 40_000_000);
    }
}
