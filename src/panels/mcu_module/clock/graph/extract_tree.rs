//! Generic node/edge clock extraction — the SECOND pass (Phase 6).
//!
//! [`super::extract`] models the clock SPINE with a purpose-built contract
//! (sources → PLL → SYSCLK → AHB → APB). That shape is why an AI-imported clock
//! can never contain the rest of a datasheet figure: the kernel muxes, RTC,
//! IWDG, MCO and the low-speed branches simply have nowhere to go in it,
//! whatever the prompt says.
//!
//! This module adds the general contract — a flat list of typed NODES, each
//! naming its INPUTS — and merges it onto a graph that already exists. Two
//! passes, not one big prompt:
//!
//! 1. the spine, with its dedicated contract and SYSCLK self-check (unchanged);
//! 2. the branches, extracted here and merged over the result.
//!
//! The two design choices that make pass 1 robust are kept and generalised:
//! - **inputs are referenced BY NAME**, never by index, so a datasheet's mux
//!   listing order cannot be got wrong;
//! - **the reply carries numeric checks** (`node` = `hz`) which are verified on
//!   the MERGED graph, so a branch that computes the wrong frequency is rejected
//!   before it can touch the user's tree.
//!
//! Merging is additive and the base always wins: an existing node keeps its
//! kind, state and limit. So re-running the pass cannot silently re-tune a tree
//! the user has already configured.

use serde::{Deserialize, Deserializer};

use super::eval::evaluate;
use super::model::{ClockGraph, Edge, LimitKey, Node, NodeKind, NodeState};

// ── The AI's JSON contract ───────────────────────────────────────────────────

fn de_u32_any<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    // Models sometimes emit a number as a string; accept both (same as pass 1).
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        U(u64),
        S(String),
    }
    Ok(match N::deserialize(d)? {
        N::U(u) => u as u32,
        N::S(s) => s.trim().parse().unwrap_or(0),
    })
}

fn de_u32_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u32>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        U(u64),
        S(String),
    }
    Ok(Vec::<N>::deserialize(d)?
        .into_iter()
        .map(|n| match n {
            N::U(u) => u as u32,
            N::S(s) => s.trim().parse().unwrap_or(0),
        })
        .collect())
}

/// One element of the clock tree, in the datasheet's own words.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedNode {
    /// The datasheet's name (`"LSE"`, `"USART1SEL"`, `"to USART1 kernel"`).
    pub name: String,
    /// One of: `source`, `mux`, `divider`, `fixed_div`, `choice`, `multiplier`,
    /// `gate`, `timer_mul`, `tap`, `output`.
    pub kind: String,
    /// What feeds it, BY NAME, in the datasheet's input order (muxes need the
    /// order; everything else uses just the first).
    pub inputs: Vec<String>,

    // Per-kind parameters. Only the ones the kind needs are read.
    /// `source`: nominal frequency in Hz.
    #[serde(deserialize_with = "de_u32_any")]
    pub hz: u32,
    /// `source`: can it be switched off?
    pub gated: bool,
    /// `divider`: the selectable divisors.
    #[serde(deserialize_with = "de_u32_vec")]
    pub divisors: Vec<u32>,
    /// `fixed_div`: the constant divisor.
    #[serde(deserialize_with = "de_u32_any")]
    pub by: u32,
    /// `choice`: ratios as `"n/d"` strings (`["1/1", "2/3"]`).
    pub ratios: Vec<String>,
    /// `multiplier`: inclusive range.
    #[serde(deserialize_with = "de_u32_any")]
    pub min: u32,
    #[serde(deserialize_with = "de_u32_any")]
    pub max: u32,
    /// `timer_mul`: the prescaler node it follows, by name.
    pub prescaler: String,
    /// Optional datasheet ceiling for this clock, in Hz (0 = none).
    #[serde(deserialize_with = "de_u32_any")]
    pub limit_hz: u32,
    /// The reset / documented default selection: for a `mux` the input NAME,
    /// for a `divider`/`choice` the chosen divisor (or `"n/d"`), for a
    /// `multiplier` the value, for a `gate` `"on"`/`"off"`. Empty = first
    /// option.
    pub select: String,
}

/// A frequency the merged tree must produce — the numeric proof that the branch
/// was read correctly.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedCheck {
    /// Node name, as used in `nodes[].name`.
    pub node: String,
    #[serde(deserialize_with = "de_u32_any")]
    pub hz: u32,
}

/// A branch (or a whole tree) as a flat node list.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedTree {
    pub nodes: Vec<ExtractedNode>,
    pub checks: Vec<ExtractedCheck>,
}

/// Parse a model reply into [`ExtractedTree`] (lenient about surrounding prose).
pub fn parse_tree_reply(model_text: &str) -> Result<ExtractedTree, String> {
    let json = crate::panels::mcu_module::datasheet_import::extract_json_object(model_text)?;
    serde_json::from_str(json)
        .map_err(|e| format!("clock-branch JSON did not match the schema: {e}"))
}

// ── Conversion ───────────────────────────────────────────────────────────────

/// A node id: lowercase, non-alphanumerics collapsed to `_` — the same rule
/// pass 1 uses, so the two passes agree on what `"PLL1R"` is called.
pub fn slug(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    let mut prev_us = false;
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
            prev_us = false;
        } else if !prev_us && !s.is_empty() {
            s.push('_');
            prev_us = true;
        }
    }
    s.trim_end_matches('_').to_string()
}

/// Turn one extracted node into a graph node. `Err` names the offending field,
/// so a malformed reply is reported rather than silently dropped.
fn to_node(ex: &ExtractedNode) -> Result<Node, String> {
    let id = slug(&ex.name);
    if id.is_empty() {
        return Err("a node has an empty name".into());
    }
    let bad = |what: &str| format!("`{}` is a {} with {what}", ex.name, ex.kind);

    let (kind, state) = match ex.kind.trim().to_ascii_lowercase().as_str() {
        "source" => {
            if ex.hz == 0 {
                return Err(bad("no frequency"));
            }
            (
                NodeKind::Source {
                    min_hz: ex.hz,
                    max_hz: ex.hz,
                    gated: ex.gated,
                },
                NodeState::Source {
                    enabled: true,
                    hz: ex.hz,
                },
            )
        }
        "mux" => {
            if ex.inputs.is_empty() {
                return Err(bad("no inputs"));
            }
            // Selection BY NAME — never by the order the datasheet happens to
            // list the inputs in.
            let pick = ex
                .inputs
                .iter()
                .position(|i| slug(i) == slug(&ex.select))
                .unwrap_or(0);
            (
                NodeKind::Mux {
                    inputs: ex.inputs.len(),
                },
                NodeState::Index(pick),
            )
        }
        "divider" => {
            let options: Vec<u32> = ex.divisors.iter().copied().filter(|d| *d > 0).collect();
            if options.is_empty() {
                return Err(bad("no divisors"));
            }
            let pick = ex
                .select
                .trim()
                .parse::<u32>()
                .ok()
                .and_then(|v| options.iter().position(|o| *o == v))
                .unwrap_or(0);
            (NodeKind::Divider { options }, NodeState::Index(pick))
        }
        "fixed_div" | "fixeddiv" => {
            if ex.by == 0 {
                return Err(bad("a zero divisor"));
            }
            (NodeKind::FixedDiv { by: ex.by }, NodeState::Fixed)
        }
        "choice" => {
            let ratios: Vec<(u32, u32)> = ex
                .ratios
                .iter()
                .filter_map(|r| {
                    let (n, d) = r.split_once('/')?;
                    Some((n.trim().parse().ok()?, d.trim().parse::<u32>().ok()?))
                })
                .filter(|(_, d)| *d > 0)
                .collect();
            if ratios.is_empty() {
                return Err(bad("no usable ratios"));
            }
            let pick = ratios
                .iter()
                .position(|(n, d)| format!("{n}/{d}") == ex.select.trim())
                .unwrap_or(0);
            (NodeKind::Choice { ratios }, NodeState::Index(pick))
        }
        "multiplier" => {
            if ex.max < ex.min || ex.max == 0 {
                return Err(bad("an empty range"));
            }
            let v = ex.select.trim().parse::<u32>().unwrap_or(ex.min);
            (
                NodeKind::Multiplier {
                    min: ex.min,
                    max: ex.max,
                },
                NodeState::Value(v.clamp(ex.min, ex.max)),
            )
        }
        "gate" | "en" => {
            let off = ex.select.trim().eq_ignore_ascii_case("off");
            (
                NodeKind::Gate,
                if off {
                    NodeState::Unset
                } else {
                    NodeState::Fixed
                },
            )
        }
        "timer_mul" | "timermul" => {
            if ex.prescaler.trim().is_empty() {
                return Err(bad("no prescaler"));
            }
            (
                NodeKind::TimerMul {
                    prescaler: slug(&ex.prescaler),
                },
                NodeState::Fixed,
            )
        }
        "tap" => (NodeKind::Tap, NodeState::Fixed),
        "output" | "sink" => (NodeKind::Output, NodeState::Fixed),
        other => return Err(format!("`{}` has unknown kind `{other}`", ex.name)),
    };

    Ok(Node {
        id,
        kind,
        state,
        limit: (ex.limit_hz > 0).then_some(LimitKey::Hz(ex.limit_hz)),
    })
}

// ── Merge ────────────────────────────────────────────────────────────────────

/// What a merge did — shown to the user before they accept it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    /// Node ids added to the tree.
    pub added: Vec<String>,
    /// Ids already present: the existing node was KEPT untouched.
    pub kept: Vec<String>,
    /// Wires added, as `from -> to`.
    pub wired: Vec<String>,
    /// Wires the extraction asked for that could not be made, with the reason.
    pub skipped: Vec<String>,
}

impl MergeReport {
    /// One-line summary for the dialog.
    pub fn summary(&self) -> String {
        format!(
            "{} node(s) added, {} kept, {} wire(s) added, {} skipped",
            self.added.len(),
            self.kept.len(),
            self.wired.len(),
            self.skipped.len()
        )
    }
}

/// Merge an extracted branch into `base`, returning the new graph.
///
/// Additive and base-wins: a node that already exists keeps its kind, state and
/// limit (so a re-run cannot re-tune a configured tree), and only genuinely new
/// wires are added. Wires whose endpoints are unknown, that already exist, or
/// that would close a loop are skipped and reported rather than applied.
///
/// The extraction's `checks` are verified on the MERGED graph — a branch that
/// does not produce the frequency the datasheet states is rejected whole, so a
/// bad second pass can never corrupt a good spine.
pub fn merge_tree(
    base: &ClockGraph,
    ex: &ExtractedTree,
) -> Result<(ClockGraph, MergeReport), String> {
    if ex.nodes.is_empty() {
        return Err("the extraction has no nodes".into());
    }
    let mut graph = base.clone();
    let mut report = MergeReport::default();

    // 1. Nodes.
    for n in &ex.nodes {
        let node = to_node(n)?;
        if graph.nodes.iter().any(|o| o.id == node.id) {
            report.kept.push(node.id);
        } else {
            report.added.push(node.id.clone());
            graph.nodes.push(node);
        }
    }

    // 2. Wires, in the extraction's declared input order.
    for n in &ex.nodes {
        let to = slug(&n.name);
        for (input, from_name) in n.inputs.iter().enumerate() {
            let from = slug(from_name);
            if from.is_empty() {
                continue;
            }
            if !graph.nodes.iter().any(|o| o.id == from) {
                report
                    .skipped
                    .push(format!("{from} -> {to} (no node `{from}`)"));
                continue;
            }
            if graph.edges.iter().any(|e| e.from == from && e.to == to) {
                continue; // already wired — nothing to do, nothing to report
            }
            if reaches(&graph, &to, &from) {
                report
                    .skipped
                    .push(format!("{from} -> {to} (would create a loop)"));
                continue;
            }
            // An existing single-input node keeps the wire it has; only a mux
            // accepts an additional one.
            let target_is_mux = graph
                .nodes
                .iter()
                .any(|o| o.id == to && matches!(o.kind, NodeKind::Mux { .. }));
            let occupied = graph.edges.iter().any(|e| e.to == to);
            if occupied && !target_is_mux {
                report
                    .skipped
                    .push(format!("{from} -> {to} (already has an input)"));
                continue;
            }
            graph.edges.push(Edge {
                from: from.clone(),
                to: to.clone(),
                input: if target_is_mux { input } else { 0 },
            });
            report.wired.push(format!("{from} -> {to}"));
        }
    }

    check_frequencies(&graph, &ex.checks)?;
    Ok((graph, report))
}

/// Verify the extraction's own numbers against the graph — the guard that makes
/// an AI reply an accelerator rather than an authority. 1% tolerance, because a
/// datasheet quotes rounded MHz while the graph divides integers.
pub fn check_frequencies(graph: &ClockGraph, checks: &[ExtractedCheck]) -> Result<(), String> {
    let freqs = evaluate(graph);
    for c in checks {
        if c.hz == 0 {
            continue;
        }
        let id = slug(&c.node);
        let Some(&got) = freqs.get(&id) else {
            return Err(format!(
                "the extraction says `{}` should be {} Hz, but there is no such node",
                c.node, c.hz
            ));
        };
        let tol = (c.hz / 100).max(1);
        if got.abs_diff(c.hz) > tol {
            return Err(format!(
                "`{}` computes {} Hz but the datasheet says {} Hz — the branch was misread, \
                 nothing was merged",
                c.node, got, c.hz
            ));
        }
    }
    Ok(())
}

/// Can `from` reach `to`? Keeps the merge acyclic.
fn reaches(graph: &ClockGraph, from: &str, to: &str) -> bool {
    let mut seen: Vec<&str> = vec![from];
    let mut stack: Vec<&str> = vec![from];
    while let Some(cur) = stack.pop() {
        if cur == to {
            return true;
        }
        for e in graph.edges.iter().filter(|e| e.from == cur) {
            if !seen.contains(&e.to.as_str()) {
                seen.push(&e.to);
                stack.push(&e.to);
            }
        }
    }
    false
}

// ── The AI request: prompt + JSON schema ─────────────────────────────────────

/// System prompt for the SECOND pass: everything the spine pass left out,
/// wired onto the nodes that already exist.
///
/// `existing` are the node ids already in the tree; naming them is what lets the
/// model attach branches instead of re-describing the spine. With an empty list
/// the same prompt asks for the whole tree.
pub fn build_tree_prompt(existing: &[String]) -> String {
    let known = if existing.is_empty() {
        "The tree is EMPTY — extract everything you can see.".to_string()
    } else {
        format!(
            "These nodes ALREADY EXIST — attach your branches to them by name and do NOT \
             re-describe them:\n{}\n",
            existing.join(", ")
        )
    };
    format!(
        "You are a clock-tree extraction assistant for an embedded-Rust IDE.\n\
         The main spine (sources -> PLL -> SYSCLK -> AHB -> APB) is already modelled. \
         Extract the REST of the datasheet's clock tree: the low-speed branches \
         (LSE/LSI -> RTC, IWDG, LSCO), the MCO output, and the peripheral KERNEL \
         clock selectors (USART/LPUART/SPI/I2C/LPTIM/ADC/RNG/SAI...).\n\
         \n\
         {known}\n\
         Return a SINGLE JSON object, no prose, no markdown, no code fences:\n\
         {{\n\
         \x20 \"nodes\": [\n\
         \x20   {{ \"name\": \"LSE\", \"kind\": \"source\", \"hz\": 32768, \"gated\": true, \"inputs\": [] }},\n\
         \x20   {{ \"name\": \"USART1SEL\", \"kind\": \"mux\",\n\
         \x20     \"inputs\": [\"PCLK2\", \"SYSCLK\", \"HSI16\", \"LSE\"], \"select\": \"PCLK2\" }},\n\
         \x20   {{ \"name\": \"to USART1 kernel\", \"kind\": \"output\", \"inputs\": [\"USART1SEL\"] }}\n\
         \x20 ],\n\
         \x20 \"checks\": [ {{ \"node\": \"to USART1 kernel\", \"hz\": 100000000 }} ]\n\
         }}\n\
         \n\
         kind is one of: source, mux, divider, fixed_div, choice, multiplier, gate, \
         timer_mul, tap, output.\n\
         Per-kind fields: source -> hz, gated; divider -> divisors [..]; fixed_div -> by; \
         choice -> ratios [\"2/3\"]; multiplier -> min, max; timer_mul -> prescaler (a node \
         name); output/tap/gate/mux -> nothing extra. Any node may carry limit_hz (its \
         datasheet maximum).\n\
         \n\
         Rules:\n\
         - reference inputs BY NAME, in the order the datasheet lists them (the mux \
           selection bits follow that order);\n\
         - \"select\" is the RESET/default choice: an input name for a mux, the divisor for a \
           divider, the value for a multiplier, \"on\"/\"off\" for a gate;\n\
         - use Hz, never MHz;\n\
         - in \"checks\", state a few frequencies the tree must produce in its default \
           configuration — they are verified, and a wrong branch is rejected;\n\
         - output the JSON object only."
    )
}

/// Strict JSON schema mirroring [`ExtractedTree`].
///
/// `additional_properties` follows the same per-provider rule as the other
/// schemas (Anthropic/OpenAI require it, Gemini rejects it).
pub fn tree_extraction_schema(additional_properties: bool) -> serde_json::Value {
    let strings = serde_json::json!({ "type": "array", "items": { "type": "string" } });
    let mut node = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "kind": { "type": "string" },
            "inputs": strings,
            "hz": { "type": "integer" },
            "gated": { "type": "boolean" },
            "divisors": { "type": "array", "items": { "type": "integer" } },
            "by": { "type": "integer" },
            "ratios": strings,
            "min": { "type": "integer" },
            "max": { "type": "integer" },
            "prescaler": { "type": "string" },
            "limit_hz": { "type": "integer" },
            "select": { "type": "string" },
        },
        "required": ["name", "kind", "inputs"],
    });
    let mut check = serde_json::json!({
        "type": "object",
        "properties": { "node": { "type": "string" }, "hz": { "type": "integer" } },
        "required": ["node", "hz"],
    });
    if additional_properties {
        node["additionalProperties"] = serde_json::Value::Bool(false);
        check["additionalProperties"] = serde_json::Value::Bool(false);
    }
    let mut root = serde_json::json!({
        "type": "object",
        "properties": {
            "nodes": { "type": "array", "items": node },
            "checks": { "type": "array", "items": check },
        },
        "required": ["nodes", "checks"],
    });
    if additional_properties {
        root["additionalProperties"] = serde_json::Value::Bool(false);
    }
    root
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A three-node spine to merge branches onto: HSI16 -> ahb -> pclk2.
    fn spine() -> ClockGraph {
        ClockGraph {
            nodes: vec![
                Node {
                    id: "hsi16".into(),
                    kind: NodeKind::Source {
                        min_hz: 16_000_000,
                        max_hz: 16_000_000,
                        gated: false,
                    },
                    state: NodeState::Source {
                        enabled: true,
                        hz: 16_000_000,
                    },
                    limit: None,
                },
                Node {
                    id: "ahb".into(),
                    kind: NodeKind::Divider {
                        options: vec![1, 2, 4],
                    },
                    state: NodeState::Index(0),
                    limit: None,
                },
                Node {
                    id: "pclk2".into(),
                    kind: NodeKind::Output,
                    state: NodeState::Fixed,
                    limit: None,
                },
            ],
            edges: vec![
                Edge {
                    from: "hsi16".into(),
                    to: "ahb".into(),
                    input: 0,
                },
                Edge {
                    from: "ahb".into(),
                    to: "pclk2".into(),
                    input: 0,
                },
            ],
        }
    }

    fn reply(json: &str) -> ExtractedTree {
        parse_tree_reply(json).expect("parse")
    }

    /// The whole point: a kernel-clock branch lands on the existing spine.
    #[test]
    fn a_kernel_branch_merges_onto_the_spine() {
        let ex = reply(
            r#"{ "nodes": [
                 { "name": "LSE", "kind": "source", "hz": 32768, "gated": true, "inputs": [] },
                 { "name": "USART1SEL", "kind": "mux",
                   "inputs": ["PCLK2", "HSI16", "LSE"], "select": "PCLK2" },
                 { "name": "to USART1 kernel", "kind": "output", "inputs": ["USART1SEL"] }
               ],
               "checks": [ { "node": "to USART1 kernel", "hz": 16000000 } ] }"#,
        );
        let (merged, report) = merge_tree(&spine(), &ex).expect("merge");

        assert_eq!(report.added, ["lse", "usart1sel", "to_usart1_kernel"]);
        assert!(report.kept.is_empty());
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(evaluate(&merged)["to_usart1_kernel"], 16_000_000);

        // The mux really has three inputs, in the datasheet's order.
        let ins: Vec<(&str, usize)> = merged
            .edges
            .iter()
            .filter(|e| e.to == "usart1sel")
            .map(|e| (e.from.as_str(), e.input))
            .collect();
        assert_eq!(ins, [("pclk2", 0), ("hsi16", 1), ("lse", 2)]);
    }

    /// Selection is resolved BY NAME, so the datasheet's listing order cannot
    /// silently change which input is selected.
    #[test]
    fn the_default_selection_is_matched_by_name() {
        let ex = reply(
            r#"{ "nodes": [
                 { "name": "LSE", "kind": "source", "hz": 32768, "gated": true, "inputs": [] },
                 { "name": "RTCSEL", "kind": "mux", "inputs": ["HSI16", "LSE"], "select": "LSE" },
                 { "name": "to RTC", "kind": "output", "inputs": ["RTCSEL"] }
               ], "checks": [ { "node": "to RTC", "hz": 32768 } ] }"#,
        );
        let (merged, _) = merge_tree(&spine(), &ex).expect("merge");
        assert_eq!(evaluate(&merged)["to_rtc"], 32_768, "LSE, not HSI16");
    }

    /// A branch whose numbers don't add up is rejected WHOLE — the spine it was
    /// merging into must be left alone.
    #[test]
    fn a_wrong_frequency_rejects_the_whole_branch() {
        let ex = reply(
            r#"{ "nodes": [
                 { "name": "MCOPRE", "kind": "divider", "divisors": [1,2,4], "select": "2",
                   "inputs": ["AHB"] },
                 { "name": "MCO", "kind": "output", "inputs": ["MCOPRE"] }
               ], "checks": [ { "node": "MCO", "hz": 16000000 } ] }"#,
        );
        // AHB is 16 MHz and the divider is /2, so MCO is 8 MHz, not 16.
        let err = merge_tree(&spine(), &ex).unwrap_err();
        assert!(err.contains("8000000") && err.contains("misread"), "{err}");
    }

    /// The base always wins: an existing node is kept exactly as configured.
    #[test]
    fn an_existing_node_is_kept_not_retuned() {
        let mut base = spine();
        base.node_mut("ahb").unwrap().state = NodeState::Index(2); // user picked /4
        let ex = reply(
            r#"{ "nodes": [
                 { "name": "AHB", "kind": "divider", "divisors": [1,2,4,8,16], "select": "1",
                   "inputs": ["HSI16"] },
                 { "name": "to IWDG", "kind": "output", "inputs": ["AHB"] }
               ], "checks": [] }"#,
        );
        let (merged, report) = merge_tree(&base, &ex).expect("merge");
        assert_eq!(report.kept, ["ahb"]);
        assert_eq!(report.added, ["to_iwdg"]);
        let ahb = merged.node("ahb").unwrap();
        assert_eq!(ahb.state, NodeState::Index(2), "the user's /4 survived");
        assert!(
            matches!(&ahb.kind, NodeKind::Divider { options } if options.len() == 3),
            "and so did its option list"
        );
        assert_eq!(evaluate(&merged)["to_iwdg"], 4_000_000);
    }

    /// Wires that cannot be made are reported, not applied — including the one
    /// that would close a loop.
    #[test]
    fn impossible_wires_are_skipped_with_a_reason() {
        let ex = reply(
            r#"{ "nodes": [
                 { "name": "HSI16", "kind": "source", "hz": 16000000, "inputs": ["PCLK2"] },
                 { "name": "Orphan", "kind": "tap", "inputs": ["NoSuchNode"] }
               ], "checks": [] }"#,
        );
        let (merged, report) = merge_tree(&spine(), &ex).expect("merge");
        assert!(
            report.skipped.iter().any(|s| s.contains("loop")),
            "pclk2 -> hsi16 closes a loop: {report:?}"
        );
        assert!(
            report.skipped.iter().any(|s| s.contains("no node")),
            "{report:?}"
        );
        // Nothing bogus reached the graph.
        assert!(!merged.edges.iter().any(|e| e.to == "hsi16"));
    }

    #[test]
    fn a_malformed_node_is_reported_not_dropped() {
        let ex = reply(r#"{ "nodes": [ { "name": "X", "kind": "wat", "inputs": [] } ] }"#);
        assert!(
            merge_tree(&spine(), &ex)
                .unwrap_err()
                .contains("unknown kind")
        );

        let ex = reply(r#"{ "nodes": [ { "name": "Y", "kind": "source", "inputs": [] } ] }"#);
        assert!(
            merge_tree(&spine(), &ex)
                .unwrap_err()
                .contains("no frequency")
        );
    }

    /// Every kind in the contract converts — otherwise the prompt promises
    /// something the parser cannot take.
    #[test]
    fn every_documented_kind_converts() {
        let ex = reply(
            r#"{ "nodes": [
              { "name": "S",  "kind": "source", "hz": 8000000, "inputs": [] },
              { "name": "M",  "kind": "mux", "inputs": ["S"], "select": "S" },
              { "name": "D",  "kind": "divider", "divisors": [1,2], "select": "2", "inputs": ["M"] },
              { "name": "F",  "kind": "fixed_div", "by": 2, "inputs": ["D"] },
              { "name": "C",  "kind": "choice", "ratios": ["1/1","2/3"], "select": "2/3", "inputs": ["F"] },
              { "name": "X",  "kind": "multiplier", "min": 2, "max": 16, "select": "3", "inputs": ["C"] },
              { "name": "G",  "kind": "gate", "select": "on", "inputs": ["X"] },
              { "name": "T",  "kind": "tap", "inputs": ["G"], "limit_hz": 50000000 },
              { "name": "TM", "kind": "timer_mul", "prescaler": "D", "inputs": ["T"] },
              { "name": "O",  "kind": "output", "inputs": ["TM"] }
            ], "checks": [] }"#,
        );
        let (merged, report) = merge_tree(&spine(), &ex).expect("merge");
        assert_eq!(report.added.len(), 10);
        let f = evaluate(&merged);
        // 8M /2 /2 = 2M, ×2/3 = 1_333_333 (integer division, as the hardware
        // ratio nodes do), ×3 = 3_999_999; the timer rule then doubles it
        // because D divides by 2.
        assert_eq!(f["x"], 3_999_999);
        assert_eq!(f["o"], 7_999_998);
        assert_eq!(
            merged.node("t").unwrap().limit,
            Some(LimitKey::Hz(50_000_000))
        );
    }

    /// The prompt names the nodes that already exist — that is what makes it a
    /// second PASS rather than a second opinion.
    #[test]
    fn the_prompt_names_the_existing_nodes() {
        let p = build_tree_prompt(&["sysclk".into(), "pclk2".into()]);
        assert!(p.contains("ALREADY EXIST"));
        assert!(p.contains("sysclk, pclk2"));
        assert!(build_tree_prompt(&[]).contains("EMPTY"));
    }

    /// Gemini rejects `additionalProperties`; Anthropic/OpenAI-strict require
    /// it. Same split as the other two schemas.
    #[test]
    fn the_schema_follows_the_per_provider_split() {
        let strict = tree_extraction_schema(true);
        assert_eq!(strict["additionalProperties"], serde_json::json!(false));
        assert_eq!(
            strict["properties"]["nodes"]["items"]["additionalProperties"],
            serde_json::json!(false)
        );
        let gemini = tree_extraction_schema(false);
        assert!(gemini.get("additionalProperties").is_none());
        assert!(
            gemini["properties"]["nodes"]["items"]
                .get("additionalProperties")
                .is_none()
        );
    }
}
