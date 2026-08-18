//! Proposing the codegen bindings of an imported clock tree.
//!
//! Code generation addresses nodes by fixed ids (`sw`, `ahb`, `pllm`, …); an
//! imported tree uses the vendor's names (`SysClkSource`, `AHBPrescaler`,
//! `PLLM`). [`GraphClock::bindings`](super::GraphClock::bindings) records which
//! node carries which id, and this proposes that map so the user confirms a
//! filled-in table instead of building one from nothing.
//!
//! The matching is deliberately conservative — a WRONG binding is worse than a
//! missing one, because it silently generates the wrong clock setup while
//! looking correct. So a candidate must both **normalise to the same word** and
//! **be of a plausible KIND** (a mux id never binds to a divider), and an id
//! that matches nothing is simply left out for the user to fill in.

use std::collections::BTreeMap;

use super::model::{ClockGraph, NodeKind};

/// Vendor spellings for the ids code generation uses, beyond what normalising
/// alone catches. Left = canonical codegen id, right = the words a vendor file
/// is likely to use for that node.
///
/// Kept as data rather than per-family tables: the same words recur across ST's
/// families (`SysClkSource` is the SYSCLK mux on F1, F2, F4, WBA and H5 alike),
/// and a name that means something else on some future family fails the KIND
/// check below rather than binding wrongly.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("sw", &["sysclksource", "sysclkmux", "systemclockmux"]),
    ("ahb", &["ahbprescaler", "hpre", "ahbdivider"]),
    ("apb1", &["apb1prescaler", "ppre1"]),
    ("apb2", &["apb2prescaler", "ppre2"]),
    ("apb7", &["apb7prescaler", "ppre7"]),
    ("adc", &["adcprescaler", "adcdivider"]),
    ("usb", &["usbprescaler", "usbdivider"]),
    ("hse", &["hseosc", "hseoscillator"]),
    ("pllsrc", &["pllsource", "pllclocksource"]),
    ("pllmul", &["pllmul", "pllmultiplicator"]),
    ("pllxtpre", &["hsedivpll", "pllxtpre"]),
    // The PLL pre-divider. ST calls it PREDIV on F0/F1/F3 and spells it
    // `HSEPLLsourceDevisor` in the CubeMX trees (its own typo) — without these
    // an imported F3 left `pllm` unbound and generated the reset /1 whatever the
    // diagram showed.
    (
        "pllm",
        &[
            "pllm",
            "plldivm",
            "prediv",
            "pllprediv",
            "hsepllsourcedevisor",
            "hsepllsourcedivisor",
        ],
    ),
    ("plln", &["plln", "pllmul"]),
    ("pllp", &["pllp", "pll1p"]),
    ("pllr", &["pllr", "pll1r"]),
    ("rtc", &["rtcclksource", "rtcsource", "rtcmux"]),
    ("mco", &["mcomult", "mco1mult", "mcomux"]),
    (
        "systick",
        &["cortexprescaler", "timsyspresc", "systickprescaler"],
    ),
    ("cpu", &["cpuclk", "cpuclock"]),
];

/// Collapse a name to comparable letters: lowercase, digits and separators
/// dropped where they carry no meaning (`"SysClkSource"` -> `"sysclksource"`).
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Could a node of this kind plausibly answer for `canonical`?
///
/// This is the guard that keeps a plausible-looking name from binding to the
/// wrong thing: `plln` must be a multiplier or a divider-shaped node, `sw` and
/// `pllsrc` must be muxes, and a prescaler id must not land on an oscillator.
fn kind_fits(canonical: &str, kind: &NodeKind) -> bool {
    let is_mux = matches!(kind, NodeKind::Mux { .. });
    let is_div = matches!(
        kind,
        NodeKind::Divider { .. } | NodeKind::FixedDiv { .. } | NodeKind::Choice { .. }
    );
    let is_mul = matches!(kind, NodeKind::Multiplier { .. });
    let is_src = matches!(kind, NodeKind::Source { .. });

    match canonical {
        "sw" | "pllsrc" | "rtc" | "mco" => is_mux,
        "hse" => is_src,
        "plln" | "pllmul" => is_mul || is_div,
        "ahb" | "apb1" | "apb2" | "apb7" | "adc" | "usb" | "pllm" | "pllp" | "pllr"
        | "pllxtpre" | "systick" => is_div,
        // `cpu` and anything unknown: a tap/output is what carries a frequency.
        _ => !is_src,
    }
}

/// Score a candidate node for `canonical`, higher is better; `None` = no match.
fn score(canonical: &str, node_id: &str, kind: &NodeKind) -> Option<u32> {
    if !kind_fits(canonical, kind) {
        return None;
    }
    let (c, n) = (norm(canonical), norm(node_id));
    if n == c {
        return Some(100); // the graph already uses the canonical name
    }
    if let Some((_, words)) = SYNONYMS.iter().find(|(k, _)| *k == canonical)
        && words.iter().any(|w| norm(w) == n)
    {
        return Some(90);
    }
    // A containment match is weaker and only counted for ids long enough that
    // it cannot be a coincidence (`"sw"` inside `"PWRSWitch"` is not a match).
    if c.len() >= 3 && n.contains(&c) {
        return Some(50);
    }
    None
}

/// Propose `canonical id -> node id` for the ids this chip's code generation
/// reads. Ids with no plausible node are omitted — the caller reports them.
///
/// A node is bound at most once: the best-scoring id wins it, so two ids cannot
/// silently claim the same node.
pub fn propose(canonical_ids: &[&str], graph: &ClockGraph) -> BTreeMap<String, String> {
    let mut ranked: Vec<(u32, &str, &str)> = Vec::new();
    for id in canonical_ids {
        for node in &graph.nodes {
            if let Some(s) = score(id, &node.id, &node.kind) {
                ranked.push((s, id, node.id.as_str()));
            }
        }
    }
    // Best first; ties resolved by name so the result is deterministic.
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)).then(a.2.cmp(b.2)));

    let mut out = BTreeMap::new();
    let mut taken: Vec<&str> = Vec::new();
    for (_, canonical, node) in ranked {
        if out.contains_key(canonical) || taken.contains(&node) {
            continue;
        }
        out.insert(canonical.to_owned(), node.to_owned());
        taken.push(node);
    }
    out
}

/// The ids code generation reads that the bindings do NOT resolve — what the
/// user still has to answer for.
pub fn unbound(canonical_ids: &[&str], bindings: &BTreeMap<String, String>) -> Vec<String> {
    canonical_ids
        .iter()
        .filter(|id| !bindings.contains_key(**id))
        .map(|id| (*id).to_owned())
        .collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::model::{Node, NodeState};
    use super::*;

    fn node(id: &str, kind: NodeKind) -> Node {
        Node {
            id: id.into(),
            kind,
            state: NodeState::Fixed,
            limit: None,
        }
    }
    fn mux(id: &str) -> Node {
        node(id, NodeKind::Mux { inputs: 2 })
    }
    fn div(id: &str) -> Node {
        node(
            id,
            NodeKind::Divider {
                options: vec![1, 2],
            },
        )
    }
    fn graph(nodes: Vec<Node>) -> ClockGraph {
        ClockGraph {
            nodes,
            edges: Vec::new(),
        }
    }

    /// A CubeMX-named tree binds to the ids the STM32F4 generator reads.
    #[test]
    fn a_cubemx_tree_binds_to_the_codegen_ids() {
        let g = graph(vec![
            node(
                "HSEOSC",
                NodeKind::Source {
                    min_hz: 8_000_000,
                    max_hz: 8_000_000,
                    gated: true,
                },
            ),
            mux("SysClkSource"),
            mux("PLLSource"),
            div("PLLM"),
            node("PLLN", NodeKind::Multiplier { min: 50, max: 432 }),
            div("PLLP"),
            div("AHBPrescaler"),
            div("APB1Prescaler"),
            div("APB2Prescaler"),
        ]);
        let ids = [
            "hse", "sw", "pllsrc", "pllm", "plln", "ahb", "pllp", "apb1", "apb2",
        ];
        let b = propose(&ids, &g);

        assert_eq!(b["sw"], "SysClkSource");
        assert_eq!(b["ahb"], "AHBPrescaler");
        assert_eq!(b["apb1"], "APB1Prescaler");
        assert_eq!(b["apb2"], "APB2Prescaler");
        assert_eq!(b["hse"], "HSEOSC");
        assert_eq!(b["pllsrc"], "PLLSource");
        assert_eq!(b["pllm"], "PLLM");
        assert_eq!(b["plln"], "PLLN");
        assert_eq!(b["pllp"], "PLLP");
        assert!(unbound(&ids, &b).is_empty(), "{:?}", unbound(&ids, &b));
    }

    /// A tree that already uses the canonical names needs no bindings at all —
    /// the proposal is the identity, so nothing changes for existing chips.
    #[test]
    fn a_native_tree_proposes_the_identity() {
        let g = graph(vec![mux("sw"), div("ahb"), div("apb1")]);
        let b = propose(&["sw", "ahb", "apb1"], &g);
        assert!(b.iter().all(|(k, v)| k == v), "{b:?}");
    }

    /// The KIND guard: a name that reads right but is the wrong sort of node
    /// must NOT bind — a wrong binding generates the wrong clock silently.
    #[test]
    fn a_plausible_name_of_the_wrong_kind_is_refused() {
        // `sw` wants a mux; this "SysClkSource" is a divider.
        let g = graph(vec![div("SysClkSource")]);
        assert!(propose(&["sw"], &g).is_empty());

        // `hse` wants an oscillator, not the divider that follows it.
        let g = graph(vec![div("HSEDivPLL")]);
        assert!(propose(&["hse"], &g).is_empty());
    }

    /// Two ids cannot claim the same node.
    #[test]
    fn one_node_answers_for_one_id() {
        let g = graph(vec![div("PLLM")]);
        let b = propose(&["pllm", "pllp"], &g);
        assert_eq!(b.len(), 1);
        assert_eq!(b["pllm"], "PLLM");
        assert_eq!(unbound(&["pllm", "pllp"], &b), ["pllp"]);
    }

    /// A short id must not match by containment — that is how `sw` would end up
    /// bound to something like a power switch.
    #[test]
    fn short_ids_do_not_match_by_containment() {
        let g = graph(vec![mux("PowerSWitchMux")]);
        assert!(propose(&["sw"], &g).is_empty());
    }

    /// What the user is asked to resolve is exactly what did not bind.
    #[test]
    fn unbound_lists_what_is_missing() {
        let g = graph(vec![mux("SysClkSource")]);
        let ids = ["sw", "ahb", "apb1"];
        let b = propose(&ids, &g);
        assert_eq!(unbound(&ids, &b), ["ahb", "apb1"]);
    }
}
