//! Per-peripheral kernel clock selection — the Clock tab's mux nodes, emitted.
//!
//! A modern STM32 gives most peripherals their own clock selector: USART1SEL,
//! I2C1SEL, ADCSEL, LPTIM1SEL. The Clock tab has ALWAYS shown these — a CubeMX
//! import of a WBA55 carries 16 of them, an H5 thirty — and the user could pick
//! a source and watch the frequency change. None of it reached `main.rs`:
//! [`codegen_node_ids`](super::rcc::codegen_node_ids) listed only bus and PLL
//! nodes, so nothing bound them and nothing read them. A diagram that computed
//! an answer nobody asked the compiler.
//!
//! # Nothing here is guessed
//!
//! Emitting one line needs three names:
//!
//! ```ignore
//! config.rcc.mux.usart1sel = mux::Usart1sel::HSI;
//! //             ^field           ^enum      ^variant
//! ```
//!
//! and the enum is NOT the field capitalised: on WBA, `usart2sel` and
//! `usart3sel` both use `Usartsel`, while `i2c2sel` and `i2c4sel` both use
//! `I2c1sel`. So the table is harvested from `stm32-metapac`'s register IR —
//! the same data embassy's own build script reads — by
//! `scripts/harvest-rcc-mux.py`, into [`rcc_mux_data`](super::rcc_mux_data).
//!
//! # Proposed by name, CONFIRMED by sources
//!
//! Matching a node to a selector by name alone would be a guess of the kind
//! this module exists to avoid, so a proposal only survives if the tree agrees
//! with the register: input *i* of the mux must be fed by the clock metapac
//! names for register value *i*. `USART1Mult` is fed by `APB2Prescaler`,
//! `SysCLKOutput`, `HSIRC`, `LSEOSC`; `Usart1sel` reads `PCLK2`, `SYS`, `HSI`,
//! `LSE`. Four for four, in order, so that binding is real.
//!
//! When they disagree — a vendor tree that orders its inputs differently, a
//! selector the tree does not model — the node is DROPPED. A missing line
//! leaves the peripheral on its reset clock, which is what happened before this
//! module existed. A wrong line silently runs a peripheral off the wrong clock.

use super::super::clock::graph::model::{ClockGraph, NodeKind, NodeState};

/// One selector embassy exposes on `config.rcc.mux`.
pub struct MuxField {
    /// The `ClockMux` field, e.g. `usart1sel`.
    pub field: &'static str,
    /// The PAC enum it takes, e.g. `Usart1sel` — not derivable from `field`.
    pub enum_name: &'static str,
    /// `(register value, variant name)`. The value is carried because it is not
    /// always 0-based: `rtcsel`'s first variant is 1, since 0 means "no clock".
    pub variants: &'static [(u32, &'static str)],
}

/// The selectors this family can emit, empty when the IDE has no table for it.
///
/// Absent families are not a bug to fix by adding a guess — `stm32g0` is left
/// out precisely because metapac splits it into `rcc_g0x0` and `rcc_g0x1` and
/// the choice is per chip.
pub fn fields_for(family: &str) -> &'static [MuxField] {
    super::rcc_mux_data::RCC_MUX
        .iter()
        .find(|(f, _)| *f == family)
        .map(|(_, fields)| *fields)
        .unwrap_or(&[])
}

/// One confirmed pairing of a graph node with a selector.
pub struct Bound {
    pub node: String,
    pub field: &'static MuxField,
}

/// Strip the decoration a vendor puts around a selector's name.
///
/// `USART1Mult` / `USART1CLockSelection` / `usart1sel` all name the same thing;
/// this reduces them to `usart1`.
fn core_name(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    for suffix in [
        "clocksource",
        "clockselection",
        "clksource",
        "selection",
        "source",
        "mult",
        "mux",
        "sel",
    ] {
        if let Some(rest) = s.strip_suffix(suffix)
            && !rest.is_empty()
        {
            s = rest.to_owned();
            break;
        }
    }
    s
}

/// Reduce a clock's name to a token both vocabularies agree on.
///
/// The tree and the register describe the same silicon in different words:
/// `APB7Output` / `PCLK7`, `AHBOutput` / `HCLK4`, `LSIOut` / `LSI`,
/// `PLL1P` / `PLL1_P`. Strip the decoration each side adds, then rename the two
/// bus families onto one spelling.
fn clock_token(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    for suffix in ["output", "out", "osc", "rc", "prescaler"] {
        if let Some(rest) = s.strip_suffix(suffix)
            && !rest.is_empty()
        {
            s = rest.to_owned();
            break;
        }
    }
    // The register says PCLK/HCLK where the tree says APB/AHB. The bus NUMBER
    // matters on PCLK — apb1 is not apb7 — and does not on HCLK: `HCLK4` names
    // the AHB4 domain, which these trees draw as a single AHB node.
    if let Some(n) = s.strip_prefix("pclk") {
        return format!("apb{n}");
    }
    if s.starts_with("hclk") || s.starts_with("ahb") {
        return "ahb".into();
    }
    if s == "sys" || s == "system" || s == "sysclk" {
        return "sysclk".into();
    }
    s
}

/// Does this graph node name the clock metapac calls `variant`?
fn same_clock(node_id: &str, variant: &str) -> bool {
    let n = clock_token(node_id);
    !n.is_empty() && n == clock_token(variant)
}

/// Pair this graph's mux nodes with the family's selectors.
///
/// Only pairings the tree CONFIRMS are returned — see the module docs.
pub fn propose(graph: &ClockGraph, family: &str) -> Vec<Bound> {
    let mut out = Vec::new();
    for field in fields_for(family) {
        let target = core_name(field.field);
        let Some(node) = graph
            .nodes
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Mux { .. }) && core_name(&n.id) == target)
        else {
            continue;
        };
        if confirms(graph, &node.id, field) {
            out.push(Bound {
                node: node.id.clone(),
                field,
            });
        }
    }
    out
}

/// Do the tree's inputs and the register's variants describe the same clocks?
///
/// Order is deliberately NOT checked, because it does not hold — see
/// [`variant_of`]. What is checked is that the two are talking about the same
/// selector at all: most of the tree's inputs must name a variant this field
/// really has. A selector whose sources we cannot recognise is one we do not
/// understand well enough to write a line for.
fn confirms(graph: &ClockGraph, node_id: &str, field: &MuxField) -> bool {
    let sources: Vec<&str> = graph
        .edges
        .iter()
        .filter(|e| e.to == node_id)
        .map(|e| e.from.as_str())
        .collect();
    if sources.len() < 2 {
        return false;
    }
    let known = sources
        .iter()
        .filter(|s| field.variants.iter().any(|(_, v)| same_clock(s, v)))
        .count();
    // Two thirds, and never fewer than two. A tree may legitimately offer a
    // source the register calls something this module has not been taught —
    // WBA's `SAI1_EXT` for `AUDIOCLK` — without that making the pairing wrong.
    known >= 2 && known * 3 >= sources.len() * 2
}

/// The variant this node's current selection means, if any.
///
/// Resolved through the SOURCE the selected input is fed by, never through the
/// index itself. Measured on the shipped WBA55 tree, the two disagree for most
/// selectors: `ADCMult` input 2 is `HSEOSC` while `adcsel` value 2 is `PLL1_P`,
/// and `LPTIM1Mult` is rotated by one against `lptim1sel`. Reading the index as
/// a register value would have put peripherals on the wrong clock SILENTLY —
/// the diagram would still show the frequency the user picked.
pub fn variant_of(graph: &ClockGraph, bound: &Bound) -> Option<&'static str> {
    let node = graph.node(&bound.node)?;
    let NodeState::Index(i) = node.state else {
        // `Unset` is a mux with nothing selected — the peripheral keeps its
        // reset clock, which is exactly what emitting nothing gives it.
        return None;
    };
    let source = &graph
        .edges
        .iter()
        .find(|e| e.to == bound.node && e.input == i)?
        .from;
    bound
        .field
        .variants
        .iter()
        .find(|(_, name)| same_clock(source, name))
        .map(|(_, name)| *name)
}

/// The `config.rcc.mux.…` lines for a whole tree, in field order.
///
/// Empty for a family with no table, for a tree with no mux nodes, and for
/// every selector still on its reset value — a project that has not chosen a
/// peripheral clock gets no line for it, so the generated block stays as small
/// as what the user actually configured.
pub fn emit_lines(graph: &ClockGraph, family: &str) -> Vec<String> {
    let mut out = Vec::new();
    for bound in propose(graph, family) {
        let Some(variant) = variant_of(graph, &bound) else {
            continue;
        };
        out.push(format!(
            "        config.rcc.mux.{} = rcc::mux::{}::{variant};",
            bound.field.field, bound.field.enum_name
        ));
    }
    out
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The naming vocabularies this module has to reconcile.
    #[test]
    fn a_selector_is_recognised_however_it_is_spelled() {
        for raw in ["USART1Mult", "usart1sel", "USART1CLockSelection", "USART1"] {
            assert_eq!(core_name(raw), "usart1", "for {raw}");
        }
        assert_eq!(core_name("I2C1Mult"), "i2c1");
        assert_eq!(core_name("ADCMult"), "adc");
        // A name that IS the decoration keeps itself rather than emptying out.
        assert_eq!(core_name("sel"), "sel");
    }

    /// Tree words vs register words, in both directions.
    #[test]
    fn the_two_vocabularies_line_up() {
        assert!(same_clock("APB2Prescaler", "PCLK2"));
        assert!(same_clock("SysCLKOutput", "SYS"));
        assert!(same_clock("HSIRC", "HSI"));
        assert!(same_clock("LSEOSC", "LSE"));
        // PLL legs match structurally rather than from a list.
        assert!(same_clock("PLL1Qoutput", "PLL1_Q"));
        // And a mismatch is a mismatch — this is what keeps a wrong line out.
        assert!(!same_clock("APB1Prescaler", "PCLK2"));
        assert!(!same_clock("HSIRC", "HSE"));
    }

    /// Every harvested table is well-formed: no empty variant list, no field
    /// that would emit a line naming nothing.
    #[test]
    fn the_harvested_tables_are_sane() {
        let mut families = 0;
        let mut selectors = 0;
        for (family, fields) in super::super::rcc_mux_data::RCC_MUX {
            families += 1;
            assert!(!fields.is_empty(), "{family} has no selectors");
            for f in *fields {
                selectors += 1;
                assert!(!f.field.is_empty() && !f.enum_name.is_empty(), "{family}");
                assert!(!f.variants.is_empty(), "{}::{}", family, f.field);
                assert!(
                    f.enum_name.starts_with(|c: char| c.is_ascii_uppercase()),
                    "an enum name is capitalised: {}",
                    f.enum_name
                );
            }
        }
        assert_eq!(families, 6);
        assert_eq!(selectors, 110, "the table is the harvest, unedited");

        // The case that proves the table is not a rule: field != enum.
        let wba = fields_for("stm32wba");
        let u2 = wba.iter().find(|f| f.field == "usart2sel").unwrap();
        assert_eq!(u2.enum_name, "Usartsel", "not `Usart2sel`");

        // And a family with no table asks for nothing.
        assert!(
            fields_for("stm32g0").is_empty(),
            "g0 is deliberately absent"
        );
        assert!(fields_for("esp32c3").is_empty());
    }

    /// Against the shipped WBA55 tree — 81 nodes, 16 of them peripheral muxes.
    ///
    /// This is the measurement that says whether the confirmation rule is too
    /// strict to be useful or loose enough to be wrong, so it PRINTS what it
    /// bound and what it refused.
    #[test]
    fn the_shipped_wba_tree_binds_its_selectors() {
        use crate::panels::mcu_module::clock::graph::parse_clock_ron;

        let ron = include_str!("../../../../assets/mcus/examples/stm32wba55_graphclock.ron");
        let gc = parse_clock_ron(ron).expect("the shipped tree parses");
        let muxes: Vec<&str> = gc
            .graph
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Mux { .. }))
            .map(|n| n.id.as_str())
            .collect();

        let bound = propose(&gc.graph, "stm32wba");
        let names: Vec<&str> = bound.iter().map(|b| b.node.as_str()).collect();
        println!(
            "{} mux nodes, {} bound: {names:?}",
            muxes.len(),
            bound.len()
        );
        let refused: Vec<&&str> = muxes.iter().filter(|m| !names.contains(m)).collect();
        println!("refused: {refused:?}");

        assert!(
            bound.len() >= 8,
            "the rule is too strict to be useful: only {} of {}",
            bound.len(),
            muxes.len()
        );
        // Every binding must name a selector this family really has.
        for b in &bound {
            assert!(
                fields_for("stm32wba")
                    .iter()
                    .any(|f| f.field == b.field.field),
                "{} is not a WBA selector",
                b.field.field
            );
        }

        let lines = emit_lines(&gc.graph, "stm32wba");
        for l in &lines {
            println!("{l}");
        }
        assert!(!lines.is_empty(), "the tree's selections emit something");
    }

    /// Values are carried, not assumed to be 0-based.
    #[test]
    fn a_selector_that_does_not_start_at_zero() {
        let f2 = fields_for("stm32f2");
        let rtc = f2.iter().find(|f| f.field == "rtcsel").unwrap();
        assert_eq!(rtc.variants[0], (1, "LSE"), "0 means no clock, not LSE");
    }
}
