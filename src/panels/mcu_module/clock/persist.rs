//! Exact serialise / parse of a [`Stm32f1Clock`] to a one-line comment.
//!
//! The generated `rcc.cfgr` chain expresses *target frequencies*, from which the
//! original mux/prescaler choices can't be recovered unambiguously.  So we also
//! emit a compact, machine-readable marker line inside the GEN block and parse
//! it back on project open — giving an exact, lossless round-trip of the Clock
//! tab state (mirroring how pins are restored from `main.rs`).

use super::model::{Mco, PllSrc, RtcSrc, Stm32f1Clock, SysclkSrc, SystickSrc, UsbPre};

/// Marker prefix for the clock config comment line.
pub const CLOCK_TAG: &str = "// @clock";

/// The ordered `key=value` fields of a clock config (shared by the one-line
/// `// @clock` comment and the multi-line `mcu.config` block).
fn fields(c: &Stm32f1Clock) -> Vec<(&'static str, String)> {
    let src = match c.sysclk_src {
        SysclkSrc::Hsi => "hsi",
        SysclkSrc::Hse => "hse",
        SysclkSrc::Pll => "pll",
    };
    let pll = match c.pll_src {
        PllSrc::HsiDiv2 => "hsi2",
        PllSrc::Hse => "hse",
        PllSrc::HseDiv2 => "hse2",
    };
    let usb = match c.usb_pre {
        UsbPre::Div1_5 => "1_5",
        UsbPre::Div1 => "1",
    };
    let mco = match c.mco {
        Mco::None => "none",
        Mco::Sysclk => "sysclk",
        Mco::Hsi => "hsi",
        Mco::Hse => "hse",
        Mco::PllDiv2 => "pll2",
    };
    let rtc = match c.rtc_src {
        RtcSrc::None => "none",
        RtcSrc::HseDiv128 => "hse128",
        RtcSrc::Lse => "lse",
        RtcSrc::Lsi => "lsi",
    };
    let systick = match c.systick_src {
        SystickSrc::HclkDiv8 => "hclk8",
        SystickSrc::Hclk => "hclk",
    };
    vec![
        ("hse", c.hse_hz.to_string()),
        ("hse_on", if c.hse_enabled { "1" } else { "0" }.to_string()),
        ("src", src.to_string()),
        ("pll", pll.to_string()),
        ("mul", c.pll_mul.to_string()),
        ("ahb", c.ahb_pre.to_string()),
        ("apb1", c.apb1_pre.to_string()),
        ("apb2", c.apb2_pre.to_string()),
        ("adc", c.adc_pre.to_string()),
        ("usb", usb.to_string()),
        ("mco", mco.to_string()),
        ("rtc", rtc.to_string()),
        ("systick", systick.to_string()),
        ("css", if c.css_on { "1" } else { "0" }.to_string()),
    ]
}

/// Serialise to a single comment line, e.g.
/// `// @clock hse=8000000 hse_on=1 src=pll pll=hse mul=9 ahb=1 apb1=2 apb2=1 adc=6 usb=1_5 mco=none`
pub fn to_comment(c: &Stm32f1Clock) -> String {
    let body = fields(c)
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{CLOCK_TAG} {body}")
}

/// Serialise to a multi-line `key=value` block (no comment prefix), for the
/// `mcu.config` file — one field per line:
/// `hse=8000000\nhse_on=0\nsrc=pll\n…`
pub fn to_config_block(c: &Stm32f1Clock) -> String {
    fields(c)
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse `key=value` tokens (whitespace- OR newline-separated) into a config.
/// Unknown / missing fields fall back to the default.
fn from_tokens(rest: &str) -> Stm32f1Clock {
    let mut c = Stm32f1Clock::default();
    for tok in rest.split_whitespace() {
        let Some((k, v)) = tok.split_once('=') else {
            continue;
        };
        match k {
            "hse" => {
                if let Ok(n) = v.parse() {
                    c.hse_hz = n;
                }
            }
            "hse_on" => c.hse_enabled = v == "1",
            "src" => {
                c.sysclk_src = match v {
                    "hsi" => SysclkSrc::Hsi,
                    "hse" => SysclkSrc::Hse,
                    _ => SysclkSrc::Pll,
                }
            }
            "pll" => {
                c.pll_src = match v {
                    "hsi2" => PllSrc::HsiDiv2,
                    "hse2" => PllSrc::HseDiv2,
                    _ => PllSrc::Hse,
                }
            }
            "mul" => {
                if let Ok(n) = v.parse() {
                    c.pll_mul = n;
                }
            }
            "ahb" => {
                if let Ok(n) = v.parse() {
                    c.ahb_pre = n;
                }
            }
            "apb1" => {
                if let Ok(n) = v.parse() {
                    c.apb1_pre = n;
                }
            }
            "apb2" => {
                if let Ok(n) = v.parse() {
                    c.apb2_pre = n;
                }
            }
            "adc" => {
                if let Ok(n) = v.parse() {
                    c.adc_pre = n;
                }
            }
            "usb" => {
                c.usb_pre = match v {
                    "1" => UsbPre::Div1,
                    _ => UsbPre::Div1_5,
                }
            }
            "mco" => {
                c.mco = match v {
                    "sysclk" => Mco::Sysclk,
                    "hsi" => Mco::Hsi,
                    "hse" => Mco::Hse,
                    "pll2" => Mco::PllDiv2,
                    _ => Mco::None,
                }
            }
            "rtc" => {
                c.rtc_src = match v {
                    "hse128" => RtcSrc::HseDiv128,
                    "lse" => RtcSrc::Lse,
                    "lsi" => RtcSrc::Lsi,
                    _ => RtcSrc::None,
                }
            }
            "systick" => {
                c.systick_src = match v {
                    "hclk" => SystickSrc::Hclk,
                    _ => SystickSrc::HclkDiv8,
                }
            }
            "css" => c.css_on = v == "1",
            _ => {}
        }
    }
    c
}

/// Parse a `// @clock …` comment line back into a config. Returns `None` if
/// `line` isn't a clock marker.
pub fn from_comment(line: &str) -> Option<Stm32f1Clock> {
    let rest = line.trim().strip_prefix(CLOCK_TAG)?;
    Some(from_tokens(rest))
}

/// Parse the multi-line `mcu.config` clock block (the body after `@clock`).
pub fn from_config_block(body: &str) -> Stm32f1Clock {
    from_tokens(body)
}

/// Scan an entire `main.rs` for the clock marker line and parse it.
pub fn parse_from_source(source: &str) -> Option<Stm32f1Clock> {
    source
        .lines()
        .find(|l| l.trim_start().starts_with(CLOCK_TAG))
        .and_then(from_comment)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_default() {
        let c = Stm32f1Clock::default();
        let line = to_comment(&c);
        assert_eq!(from_comment(&line), Some(c));
    }

    #[test]
    fn round_trips_custom() {
        let c = Stm32f1Clock {
            hse_hz: 12_000_000,
            hse_enabled: false,
            sysclk_src: SysclkSrc::Hsi,
            pll_src: PllSrc::HsiDiv2,
            pll_mul: 16,
            ahb_pre: 2,
            apb1_pre: 4,
            apb2_pre: 2,
            adc_pre: 8,
            usb_pre: UsbPre::Div1,
            mco: Mco::PllDiv2,
            rtc_src: RtcSrc::Lse,
            systick_src: SystickSrc::Hclk,
            css_on: true,
        };
        let line = to_comment(&c);
        assert_eq!(from_comment(&line), Some(c));
    }

    #[test]
    fn finds_marker_in_source() {
        let c = Stm32f1Clock::default();
        let src = format!("// header\n{}\nfn main() {{}}", to_comment(&c));
        assert_eq!(parse_from_source(&src), Some(c));
    }

    #[test]
    fn non_marker_returns_none() {
        assert_eq!(from_comment("let x = 5;"), None);
        assert_eq!(parse_from_source("no markers here"), None);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Any graph, per project — the `@clocknodes` block
// ──────────────────────────────────────────────────────────────────────────────
//
// Everything above this line speaks `Stm32f1Clock`, and the save path gated on
// `family == "stm32f1"` — so on every OTHER chip the Clock tab was a scratchpad:
// retune an H5's PLL, close the project, and the tree came back at the chip
// definition's defaults with nothing to say it had ever changed. The only way to
// keep an edit was "Save to chip", which writes the DEFINITION and so changes
// every project using that part.
//
// This block is family-neutral: it records node states by id, which is the one
// thing every graph has.
//
// Only the DELTA against the chip's factory tree is written. A project that has
// not touched its clock adds nothing to `mcu.config`, and a definition that
// later gains a better default is picked up rather than pinned over.

use super::graph::model::{ClockGraph, NodeState};

/// One node's state, as a self-describing token.
///
/// Tagged rather than bare, because `Index` and `Value` are both integers and
/// the node's kind is not something a config file should have to be right about
/// — a tree that changes shape must fail to match a node, not silently reinterpret
/// its number.
fn state_token(s: &NodeState) -> Option<String> {
    match s {
        // Nothing selectable: nothing to restore.
        NodeState::Fixed => None,
        NodeState::Unset => Some("u".into()),
        NodeState::Index(i) => Some(format!("i{i}")),
        NodeState::Value(v) => Some(format!("v{v}")),
        NodeState::Source { enabled, hz } => {
            Some(format!("s{}:{hz}", if *enabled { 1 } else { 0 }))
        }
    }
}

fn parse_state(tok: &str) -> Option<NodeState> {
    // Split off the tag CHARACTER, not at the second char's index: `u` carries
    // no payload and has no second char to split at.
    let mut chars = tok.chars();
    let tag = chars.next()?;
    let rest = chars.as_str();
    match tag {
        'u' => Some(NodeState::Unset),
        'i' => rest.parse().ok().map(NodeState::Index),
        'v' => rest.parse().ok().map(NodeState::Value),
        's' => {
            let (on, hz) = rest.split_once(':')?;
            Some(NodeState::Source {
                enabled: on == "1",
                hz: hz.parse().ok()?,
            })
        }
        _ => None,
    }
}

/// The project's clock edits: every node whose state differs from `defaults`.
///
/// Empty when the tree is untouched, so `mcu.config` stays byte-identical for
/// the common case.
pub fn nodes_to_block(graph: &ClockGraph, defaults: Option<&ClockGraph>) -> String {
    let mut out = Vec::new();
    for node in &graph.nodes {
        let factory = defaults.and_then(|d| d.nodes.iter().find(|n| n.id == node.id));
        // No defaults captured (a tree built in the editor this session) means
        // every state is a deviation — write them all rather than lose them.
        if factory.is_some_and(|f| f.state == node.state) {
            continue;
        }
        if let Some(tok) = state_token(&node.state) {
            out.push(format!("{}={tok}", node.id));
        }
    }
    out.join("\n")
}

/// Parse a `@clocknodes` body into `(node id, state)` pairs.
///
/// Unparseable lines are skipped, not fatal: a config written by a later version
/// must not cost the user the states it CAN restore.
pub fn nodes_from_block(body: &str) -> Vec<(String, NodeState)> {
    body.split_whitespace()
        .filter_map(|tok| {
            let (id, state) = tok.split_once('=')?;
            Some((id.to_owned(), parse_state(state)?))
        })
        .collect()
}

/// Apply saved states onto a graph, by id.
///
/// A node the graph no longer has is skipped — the chip definition may have been
/// re-imported with different names since, and a stale id is not a reason to
/// refuse the rest.
pub fn apply_nodes(graph: &mut ClockGraph, saved: &[(String, NodeState)]) -> usize {
    let mut applied = 0;
    for (id, state) in saved {
        if let Some(node) = graph.node_mut(id) {
            node.state = state.clone();
            applied += 1;
        }
    }
    applied
}

#[cfg(test)]
mod node_state_tests {
    use super::*;
    use crate::panels::mcu_module::clock::graph::minimal_graph;

    /// THE gap this closes: a non-F1 tree, retuned, surviving a save/load cycle.
    ///
    /// Before, `mcu.config` was written with `if self.family == "stm32f1"`, so a
    /// retuned H5 or WBA came back at the chip's defaults with nothing recording
    /// that the project had ever changed it.
    #[test]
    fn a_retuned_tree_round_trips() {
        let factory = minimal_graph();
        let mut project = minimal_graph();
        // Put the PLL on HSE, wind it up, and run SYSCLK off it.
        project.node_mut("pllsrc").unwrap().state = NodeState::Index(1);
        project.node_mut("plln").unwrap().state = NodeState::Value(100);
        project.node_mut("sw").unwrap().state = NodeState::Index(2);
        project.node_mut("hse").unwrap().state = NodeState::Source {
            enabled: true,
            hz: 12_000_000,
        };

        let block = nodes_to_block(&project, Some(&factory));
        // Only what CHANGED, so an untouched project adds nothing to the file.
        assert_eq!(block.lines().count(), 4, "{block}");
        assert!(block.contains("plln=v100"), "{block}");
        assert!(block.contains("hse=s1:12000000"), "{block}");

        // A fresh chip, as built from its definition, plus the saved delta.
        let mut reopened = minimal_graph();
        let applied = apply_nodes(&mut reopened, &nodes_from_block(&block));
        assert_eq!(applied, 4);
        assert_eq!(reopened.nodes, project.nodes, "the tree came back exactly");
    }

    /// An untouched clock must not touch the file at all.
    #[test]
    fn an_untouched_tree_writes_nothing() {
        let g = minimal_graph();
        assert!(nodes_to_block(&g, Some(&g)).is_empty());
        // …but with no defaults captured, everything is a deviation and is kept:
        // losing a tree built in the editor this session would be worse.
        assert!(!nodes_to_block(&g, None).is_empty());
    }

    /// Every selectable state survives its own encoding, and `Fixed` is dropped
    /// because there is nothing in it to restore.
    #[test]
    fn every_state_encodes_unambiguously() {
        let cases = [
            NodeState::Unset,
            NodeState::Index(0),
            NodeState::Index(7),
            NodeState::Value(129),
            NodeState::Source {
                enabled: false,
                hz: 32_768,
            },
            NodeState::Source {
                enabled: true,
                hz: 25_000_000,
            },
        ];
        for s in &cases {
            let tok = state_token(s).expect("selectable states encode");
            assert_eq!(parse_state(&tok).as_ref(), Some(s), "token {tok}");
        }
        assert!(state_token(&NodeState::Fixed).is_none());

        // `Index` and `Value` are both integers; the tag is what keeps them
        // apart, so a tree that changes shape cannot silently reinterpret one.
        assert_ne!(
            state_token(&NodeState::Index(5)),
            state_token(&NodeState::Value(5))
        );
    }

    /// A saved id the tree no longer has costs only that id.
    #[test]
    fn a_stale_id_does_not_sink_the_rest() {
        let mut g = minimal_graph();
        let saved = nodes_from_block("sw=i2 gone_node=i1 plln=v40");
        assert_eq!(saved.len(), 3, "it parses, even what will not apply");
        assert_eq!(apply_nodes(&mut g, &saved), 2, "the two that still exist");
        assert_eq!(g.node("sw").unwrap().state, NodeState::Index(2));
        assert_eq!(g.node("plln").unwrap().state, NodeState::Value(40));
    }

    /// Garbage in a hand-edited file loses that line, not the section.
    #[test]
    fn an_unreadable_token_is_skipped() {
        let saved = nodes_from_block("sw=i1 plln=nonsense noequals hse=s1:8000000");
        assert_eq!(
            saved.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            ["sw", "hse"]
        );
    }
}
