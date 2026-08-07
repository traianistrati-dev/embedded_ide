//! AI clock-tree extraction (Layer 3) — the PURE core.
//!
//! An AI reads a datasheet (text or the Figure-6 clock-tree diagram) and
//! returns a flat, NAME-based description of the clock SPINE — sources → PLL →
//! SYSCLK → AHB → APB. This turns that description into a validated
//! [`GraphClock`], the same shape Layer 1 imports.
//!
//! Two design choices from the analysis make this robust:
//! * **Mux inputs are matched by NAME, never by index.** A datasheet lists a
//!   mux's inputs but rarely pins down the register order, so the extraction
//!   references sources by name (`"HSE"`, `"PLL1R"`) and this resolves them —
//!   input order can't be got wrong because it is never used.
//! * **A numeric self-check.** The extraction also carries the datasheet's
//!   documented DEFAULT selection and the SYSCLK it should produce; after
//!   building the graph we set those selections, evaluate, and require the
//!   computed SYSCLK to match. A wrong edge or a mis-read divider set makes the
//!   number disagree, so a broken extraction is caught before it is imported.
//!
//! Network + UI wiring (a datasheet dialog like the pin importer's) reuses the
//! existing provider machinery and lives elsewhere; this module is pure and
//! fully tested.

use serde::{Deserialize, Deserializer};

use super::config::GraphClock;
use super::eval::evaluate;
use super::model::{ClockGraph, Edge, Node, NodeKind, NodeState};

// ── The AI's JSON contract ───────────────────────────────────────────────────

fn de_u32_any<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    // Models sometimes emit a number as a string; accept both.
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

/// One oscillator / clock source.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedSource {
    /// The name the datasheet uses (`HSI16`, `HSE`, `LSE`, …).
    pub name: String,
    /// Its frequency in Hz. A crystal has one; an RC has its nominal.
    #[serde(deserialize_with = "de_u32_any")]
    pub hz: u32,
    /// `true` if it can be turned off (most oscillators).
    pub gated: bool,
}

/// The PLL chain `source → /M → ×N → /out`, modelled with a single output leg
/// (the one that feeds SYSCLK — R on WBA, P on F4).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedPll {
    /// The PLL input mux inputs, BY NAME (must match `sources[].name`).
    pub source_options: Vec<String>,
    /// The `/M` predivider option set.
    pub m_divisors: Vec<u32>,
    /// The `×N` multiplier range.
    #[serde(deserialize_with = "de_u32_any")]
    pub n_min: u32,
    #[serde(deserialize_with = "de_u32_any")]
    pub n_max: u32,
    /// The modelled output `/out` option set.
    pub output_divisors: Vec<u32>,
    /// The name of that output as SYSCLK references it (`PLL1R`, `PLLP`, …).
    pub output_name: String,
}

/// One APB bus divider + its delivered clock.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedBus {
    pub name: String,
    pub divisors: Vec<u32>,
}

/// The documented reset/default configuration, used ONLY for the self-check.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedDefault {
    /// The SYSCLK source selected by default (a name in `sysclk_sources`).
    pub sysclk_source: String,
    pub pll_source: String,
    #[serde(deserialize_with = "de_u32_any")]
    pub pll_m: u32,
    #[serde(deserialize_with = "de_u32_any")]
    pub pll_n: u32,
    #[serde(deserialize_with = "de_u32_any")]
    pub pll_out: u32,
    /// The SYSCLK the datasheet says this default produces — the number the
    /// self-check must reproduce.
    #[serde(deserialize_with = "de_u32_any")]
    pub sysclk_hz: u32,
}

/// The whole extracted clock spine.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExtractedClock {
    pub sources: Vec<ExtractedSource>,
    /// `None`/absent when the part has no PLL.
    pub pll: Option<ExtractedPll>,
    /// SYSCLK mux inputs BY NAME — source names and/or the PLL `output_name`.
    pub sysclk_sources: Vec<String>,
    #[serde(deserialize_with = "de_u32_any")]
    pub sysclk_max_hz: u32,
    pub ahb_divisors: Vec<u32>,
    pub apb: Vec<ExtractedBus>,
    pub default: ExtractedDefault,
}

// ── The AI request: prompt + JSON schema ─────────────────────────────────────

/// System prompt asking the model to extract the clock SPINE — deliberately
/// NOT the peripheral kernel muxes (out of scope), and TEXT-first (the RM's
/// prose + tables carry the whole spine; the Figure-6 diagram only confirms).
pub fn build_clock_prompt() -> String {
    "You are a clock-tree extraction assistant for an embedded-Rust IDE.\n\
     From the microcontroller datasheet, extract ONLY the main clock SPINE and \
     return a SINGLE JSON object, no prose, no markdown, no code fences.\n\
     \n\
     The spine is: oscillator SOURCES → the PLL chain (source mux → /M → ×N → \
     one output /divider) → the SYSCLK mux → the AHB (HCLK) divider → the APB \
     bus dividers. STOP THERE. Do NOT model the peripheral kernel clocks \
     (USART/SPI/I2C/ADC/RNG/SAI…) — they are out of scope.\n\
     \n\
     Read from the TEXT and the tables first (the reference manual describes \
     the whole spine in prose: \"PLLM ranges 1..8\", \"the PLL input frequency \
     must be 4–16 MHz\", …). A clock-tree FIGURE, if present, only confirms it. \
     The PLL input (ref) and VCO frequency windows are in the electrical / PLL \
     characteristics TABLES — include them if stated.\n\
     \n\
     Reference every mux input BY NAME (never by position): a source's name, or \
     the PLL output_name. Names must match exactly across the object.\n\
     \n\
     Also give the datasheet's DEFAULT / max-performance configuration and the \
     SYSCLK it produces — it is used to verify the extraction.\n\
     \n\
     JSON shape:\n\
     {\n\
     \x20 \"sources\": [ { \"name\": \"HSE\", \"hz\": 32000000, \"gated\": true } ],\n\
     \x20 \"pll\": {                         // omit if the part has no PLL\n\
     \x20   \"source_options\": [\"HSI16\",\"HSE\"],\n\
     \x20   \"m_divisors\": [1,2,3,4,5,6,7,8],\n\
     \x20   \"n_min\": 4, \"n_max\": 512,\n\
     \x20   \"output_divisors\": [1,2,3,4,5,6,7,8],\n\
     \x20   \"output_name\": \"PLL1R\"         // how SYSCLK refers to this output\n\
     \x20 },\n\
     \x20 \"sysclk_sources\": [\"HSI16\",\"HSE\",\"PLL1R\"],\n\
     \x20 \"sysclk_max_hz\": 100000000,\n\
     \x20 \"ahb_divisors\": [1,2,4,8,16,64,128,256,512],\n\
     \x20 \"apb\": [ { \"name\": \"APB1\", \"divisors\": [1,2,4,8,16] } ],\n\
     \x20 \"default\": {\n\
     \x20   \"sysclk_source\": \"PLL1R\", \"pll_source\": \"HSE\",\n\
     \x20   \"pll_m\": 2, \"pll_n\": 25, \"pll_out\": 4,\n\
     \x20   \"sysclk_hz\": 100000000\n\
     \x20 }\n\
     }\n\
     \n\
     Rules: use Hz (not MHz) for every frequency; copy divider/multiplier option \
     sets exactly; if there is no PLL omit \"pll\" and set default.pll_* to 0; \
     output the JSON object only."
        .to_string()
}

/// The strict JSON schema mirroring [`ExtractedClock`], for structured output.
///
/// `additional_properties` emits `additionalProperties: false` at every object
/// level — Anthropic and OpenAI-strict REQUIRE it, Gemini's `responseSchema`
/// subset REJECTS it (same split as the pin schema).
pub fn clock_extraction_schema(additional_properties: bool) -> serde_json::Value {
    let u32s = serde_json::json!({ "type": "array", "items": { "type": "integer" } });
    let names = serde_json::json!({ "type": "array", "items": { "type": "string" } });
    let mut root = serde_json::json!({
        "type": "object",
        "properties": {
            "sources": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "hz": { "type": "integer" },
                        "gated": { "type": "boolean" },
                    },
                    "required": ["name", "hz", "gated"],
                },
            },
            "pll": {
                "type": "object",
                "properties": {
                    "source_options": names,
                    "m_divisors": u32s,
                    "n_min": { "type": "integer" },
                    "n_max": { "type": "integer" },
                    "output_divisors": u32s,
                    "output_name": { "type": "string" },
                },
                "required": ["source_options", "m_divisors", "n_min", "n_max",
                             "output_divisors", "output_name"],
            },
            "sysclk_sources": names,
            "sysclk_max_hz": { "type": "integer" },
            "ahb_divisors": u32s,
            "apb": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "divisors": u32s,
                    },
                    "required": ["name", "divisors"],
                },
            },
            "default": {
                "type": "object",
                "properties": {
                    "sysclk_source": { "type": "string" },
                    "pll_source": { "type": "string" },
                    "pll_m": { "type": "integer" },
                    "pll_n": { "type": "integer" },
                    "pll_out": { "type": "integer" },
                    "sysclk_hz": { "type": "integer" },
                },
                "required": ["sysclk_source", "pll_source", "pll_m", "pll_n",
                             "pll_out", "sysclk_hz"],
            },
        },
        "required": ["sources", "sysclk_sources", "sysclk_max_hz",
                     "ahb_divisors", "apb", "default"],
    });

    if additional_properties {
        root["additionalProperties"] = serde_json::json!(false);
        for path in [
            "/properties/sources/items",
            "/properties/pll",
            "/properties/apb/items",
            "/properties/default",
        ] {
            if let Some(obj) = root.pointer_mut(path) {
                obj["additionalProperties"] = serde_json::json!(false);
            }
        }
    }
    root
}

/// Parse the model's JSON reply into an [`ExtractedClock`], tolerating prose or
/// code fences around it (same lenient extraction the pin importer uses).
pub fn parse_clock_reply(model_text: &str) -> Result<ExtractedClock, String> {
    let json = crate::panels::mcu_module::datasheet_import::extract_json_object(model_text)?;
    serde_json::from_str(json).map_err(|e| format!("clock JSON did not match the schema: {e}"))
}

// ── Conversion to a validated graph ──────────────────────────────────────────

/// A node id: lowercase, non-alphanumerics collapsed to `_`. Deterministic so
/// name references resolve consistently (`"PLL1R"` → `pll1r`).
fn slug(name: &str) -> String {
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

/// Build a [`GraphClock`] from an extraction, wiring muxes by NAME and seeding
/// every node with the datasheet's documented default selection.
///
/// Returns a readable error (not a dead graph) when a referenced name has no
/// matching node — the single most likely extraction mistake.
pub fn to_graph_clock(ex: &ExtractedClock) -> Result<GraphClock, String> {
    if ex.sources.is_empty() {
        return Err("the extraction has no clock sources".into());
    }
    let mut nodes: Vec<Node> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();

    let node = |id: &str, kind: NodeKind, state: NodeState| Node {
        id: id.into(),
        kind,
        state,
        limit: None,
    };
    let edge = |from: &str, to: &str, input: usize| Edge {
        from: from.into(),
        to: to.into(),
        input,
    };

    // Resolve a referenced name to a node id: a source, or the PLL output tap.
    let source_id = |name: &str| -> Option<String> {
        ex.sources
            .iter()
            .find(|s| slug(&s.name) == slug(name))
            .map(|s| slug(&s.name))
    };
    let pll_tap_id = ex.pll.as_ref().map(|p| slug(&p.output_name));
    let resolve = |name: &str| -> Option<String> {
        if let Some(id) = source_id(name) {
            return Some(id);
        }
        match &pll_tap_id {
            Some(t) if slug(name) == *t => Some(t.clone()),
            _ => None,
        }
    };

    // ── Sources ─────────────────────────────────────────────────────────
    for s in &ex.sources {
        nodes.push(node(
            &slug(&s.name),
            NodeKind::Source {
                min_hz: s.hz,
                max_hz: s.hz,
                gated: s.gated,
            },
            NodeState::Source {
                enabled: true,
                hz: s.hz,
            },
        ));
    }

    // ── PLL chain ───────────────────────────────────────────────────────
    if let Some(p) = &ex.pll {
        if p.source_options.is_empty() {
            return Err("the PLL has no source options".into());
        }
        nodes.push(node(
            "pllsrc",
            NodeKind::Mux {
                inputs: p.source_options.len(),
            },
            NodeState::Index(index_of_name(&p.source_options, &ex.default.pll_source)),
        ));
        for (i, opt) in p.source_options.iter().enumerate() {
            let from = source_id(opt)
                .ok_or_else(|| format!("PLL source \"{opt}\" is not one of the listed sources"))?;
            edges.push(edge(&from, "pllsrc", i));
        }
        nodes.push(node(
            "pllm",
            NodeKind::Divider {
                options: nonempty(&p.m_divisors, 1),
            },
            NodeState::Index(index_of_div(&p.m_divisors, ex.default.pll_m)),
        ));
        edges.push(edge("pllsrc", "pllm", 0));
        nodes.push(node(
            "plln",
            NodeKind::Multiplier {
                min: p.n_min.max(1),
                max: p.n_max.max(p.n_min.max(1)),
            },
            NodeState::Value(clamp_n(ex.default.pll_n, p)),
        ));
        edges.push(edge("pllm", "plln", 0));
        nodes.push(node(
            "pllout",
            NodeKind::Divider {
                options: nonempty(&p.output_divisors, 1),
            },
            NodeState::Index(index_of_div(&p.output_divisors, ex.default.pll_out)),
        ));
        edges.push(edge("plln", "pllout", 0));
        // The named tap SYSCLK references.
        nodes.push(node(&slug(&p.output_name), NodeKind::Tap, NodeState::Fixed));
        edges.push(edge("pllout", &slug(&p.output_name), 0));
    }

    // ── SYSCLK mux ──────────────────────────────────────────────────────
    if ex.sysclk_sources.is_empty() {
        return Err("the extraction has no SYSCLK sources".into());
    }
    nodes.push(node(
        "sw",
        NodeKind::Mux {
            inputs: ex.sysclk_sources.len(),
        },
        NodeState::Index(index_of_name(&ex.sysclk_sources, &ex.default.sysclk_source)),
    ));
    for (i, name) in ex.sysclk_sources.iter().enumerate() {
        let from = resolve(name)
            .ok_or_else(|| format!("SYSCLK source \"{name}\" matches no source or PLL output"))?;
        edges.push(edge(&from, "sw", i));
    }
    nodes.push(node("sysclk", NodeKind::Tap, NodeState::Fixed));
    edges.push(edge("sw", "sysclk", 0));

    // ── AHB / HCLK ──────────────────────────────────────────────────────
    nodes.push(node(
        "ahb",
        NodeKind::Divider {
            options: nonempty(&ex.ahb_divisors, 1),
        },
        NodeState::Index(0),
    ));
    edges.push(edge("sysclk", "ahb", 0));
    nodes.push(node("hclk", NodeKind::Output, NodeState::Fixed));
    edges.push(edge("ahb", "hclk", 0));

    // ── APB buses ───────────────────────────────────────────────────────
    for bus in &ex.apb {
        let div_id = slug(&bus.name);
        let out_id = format!("{}_out", div_id);
        nodes.push(node(
            &div_id,
            NodeKind::Divider {
                options: nonempty(&bus.divisors, 1),
            },
            NodeState::Index(0),
        ));
        edges.push(edge("ahb", &div_id, 0));
        nodes.push(node(&out_id, NodeKind::Output, NodeState::Fixed));
        edges.push(edge(&div_id, &out_id, 0));
    }

    let gc = GraphClock {
        graph: ClockGraph { nodes, edges },
        layout: Default::default(),
    };
    self_check(ex, &gc.graph)?;
    Ok(gc)
}

/// Evaluate the built graph at the documented default and require SYSCLK to
/// match. This is the guard that turns a plausible-but-wrong extraction (a
/// mis-wired mux, a mis-read divider set) into a rejected import instead of a
/// silently wrong clock tree.
pub fn self_check(ex: &ExtractedClock, graph: &ClockGraph) -> Result<(), String> {
    let want = ex.default.sysclk_hz;
    if want == 0 {
        // No documented default to check against — fall back to Layer 1's
        // "must not be all-zero" (the caller runs it via parse anyway).
        return Ok(());
    }
    let freqs = evaluate(graph);
    let got = freqs.get("sysclk").copied().unwrap_or(0);
    // Allow a 1% tolerance: integer division in the graph can differ from a
    // datasheet's rounded MHz figure by a few Hz.
    let tol = (want / 100).max(1);
    if got.abs_diff(want) > tol {
        return Err(format!(
            "self-check failed: the extracted tree computes SYSCLK = {got} Hz at the documented \
             default, but the datasheet says {want} Hz. The PLL factors or a mux wiring were \
             likely mis-read."
        ));
    }
    Ok(())
}

fn nonempty(v: &[u32], fallback: u32) -> Vec<u32> {
    if v.is_empty() {
        vec![fallback]
    } else {
        v.to_vec()
    }
}

fn index_of_name(options: &[String], want: &str) -> usize {
    options
        .iter()
        .position(|o| slug(o) == slug(want))
        .unwrap_or(0)
}

fn index_of_div(options: &[u32], want: u32) -> usize {
    options.iter().position(|&o| o == want).unwrap_or(0)
}

fn clamp_n(n: u32, p: &ExtractedPll) -> u32 {
    let lo = p.n_min.max(1);
    let hi = p.n_max.max(lo);
    n.clamp(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WBA-shaped extraction: HSE32 /2 ×25 /4 via PLL1R → 100 MHz.
    fn wba_like() -> ExtractedClock {
        ExtractedClock {
            sources: vec![
                ExtractedSource {
                    name: "HSI16".into(),
                    hz: 16_000_000,
                    gated: true,
                },
                ExtractedSource {
                    name: "HSE".into(),
                    hz: 32_000_000,
                    gated: true,
                },
            ],
            pll: Some(ExtractedPll {
                source_options: vec!["HSI16".into(), "HSE".into()],
                m_divisors: vec![1, 2, 3, 4, 5, 6, 7, 8],
                n_min: 4,
                n_max: 512,
                output_divisors: vec![1, 2, 3, 4, 5, 6, 7, 8],
                output_name: "PLL1R".into(),
            }),
            sysclk_sources: vec!["HSI16".into(), "HSE".into(), "PLL1R".into()],
            sysclk_max_hz: 100_000_000,
            ahb_divisors: vec![1, 2, 4, 8, 16, 64, 128, 256, 512],
            apb: vec![
                ExtractedBus {
                    name: "APB1".into(),
                    divisors: vec![1, 2, 4, 8, 16],
                },
                ExtractedBus {
                    name: "APB2".into(),
                    divisors: vec![1, 2, 4, 8, 16],
                },
                ExtractedBus {
                    name: "APB7".into(),
                    divisors: vec![1, 2, 4, 8, 16],
                },
            ],
            default: ExtractedDefault {
                sysclk_source: "PLL1R".into(),
                pll_source: "HSE".into(),
                pll_m: 2,
                pll_n: 25,
                pll_out: 4,
                sysclk_hz: 100_000_000,
            },
        }
    }

    #[test]
    fn builds_a_graph_that_self_checks_to_the_documented_sysclk() {
        let gc = to_graph_clock(&wba_like()).expect("valid extraction must convert");
        // The self-check ran inside to_graph_clock; confirm the number too.
        let f = evaluate(&gc.graph);
        assert_eq!(f.get("sysclk").copied(), Some(100_000_000));
    }

    #[test]
    fn mux_inputs_are_matched_by_name_not_order() {
        // Reverse the SYSCLK source order — the default still points at PLL1R by
        // NAME, so the result must be identical.
        let mut ex = wba_like();
        ex.sysclk_sources = vec!["PLL1R".into(), "HSE".into(), "HSI16".into()];
        let gc = to_graph_clock(&ex).expect("name mapping must ignore order");
        assert_eq!(
            evaluate(&gc.graph).get("sysclk").copied(),
            Some(100_000_000)
        );
    }

    #[test]
    fn a_wrong_pll_factor_is_caught_by_the_self_check() {
        // The datasheet default SAYS 100 MHz, but the extracted N is wrong.
        let mut ex = wba_like();
        ex.default.pll_n = 20; // → 32/2*20/4 = 80 MHz, not 100.
        let err = to_graph_clock(&ex).unwrap_err();
        assert!(err.contains("self-check failed"), "{err}");
        assert!(err.contains("80000000"), "{err}");
    }

    #[test]
    fn an_unresolvable_sysclk_source_name_is_a_readable_error() {
        let mut ex = wba_like();
        ex.sysclk_sources = vec!["HSI16".into(), "HSE".into(), "PLLXYZ".into()];
        ex.default.sysclk_source = "HSE".into(); // keep the default resolvable
        let err = to_graph_clock(&ex).unwrap_err();
        assert!(err.contains("PLLXYZ"), "{err}");
        assert!(err.contains("matches no source"), "{err}");
    }

    #[test]
    fn hse_direct_default_needs_no_pll_factors() {
        // A default that runs SYSCLK straight off HSE (32 MHz), PLL present but
        // unused — self-check must still pass.
        let mut ex = wba_like();
        ex.default = ExtractedDefault {
            sysclk_source: "HSE".into(),
            pll_source: "HSE".into(),
            pll_m: 0,
            pll_n: 0,
            pll_out: 0,
            sysclk_hz: 32_000_000,
        };
        let gc = to_graph_clock(&ex).expect("HSE-direct default must convert");
        assert_eq!(evaluate(&gc.graph).get("sysclk").copied(), Some(32_000_000));
    }

    #[test]
    fn a_full_json_reply_parses_and_self_checks() {
        // The end-to-end pure path: model text (with a code fence, as a model
        // often emits) → parse → convert → self-check.
        let reply = "```json\n{\
            \"sources\":[{\"name\":\"HSI16\",\"hz\":16000000,\"gated\":true},\
                        {\"name\":\"HSE\",\"hz\":32000000,\"gated\":true}],\
            \"pll\":{\"source_options\":[\"HSI16\",\"HSE\"],\"m_divisors\":[1,2,3,4],\
                     \"n_min\":4,\"n_max\":512,\"output_divisors\":[2,4,6,8],\"output_name\":\"PLL1R\"},\
            \"sysclk_sources\":[\"HSI16\",\"HSE\",\"PLL1R\"],\"sysclk_max_hz\":100000000,\
            \"ahb_divisors\":[1,2,4,8],\"apb\":[{\"name\":\"APB1\",\"divisors\":[1,2,4]}],\
            \"default\":{\"sysclk_source\":\"PLL1R\",\"pll_source\":\"HSE\",\
                         \"pll_m\":2,\"pll_n\":25,\"pll_out\":4,\"sysclk_hz\":100000000}\
            }\n```";
        let ex = parse_clock_reply(reply).expect("reply must parse");
        let gc = to_graph_clock(&ex).expect("must convert + self-check");
        assert_eq!(
            evaluate(&gc.graph).get("sysclk").copied(),
            Some(100_000_000)
        );
    }

    #[test]
    fn schema_and_prompt_name_the_key_fields() {
        let schema = clock_extraction_schema(true);
        assert_eq!(schema["properties"]["sources"]["type"], "array");
        assert!(
            schema["properties"]["default"]["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "sysclk_hz")
        );
        // Gemini's subset rejects additionalProperties; strict mode requires it.
        let strict = clock_extraction_schema(true);
        assert_eq!(strict["additionalProperties"], serde_json::json!(false));
        assert_eq!(
            strict["properties"]["default"]["additionalProperties"],
            serde_json::json!(false)
        );
        let gemini = clock_extraction_schema(false);
        assert!(!gemini.to_string().contains("additionalProperties"));

        let prompt = build_clock_prompt();
        assert!(prompt.contains("clock SPINE"));
        assert!(prompt.contains("BY NAME"));
        assert!(prompt.contains("out of scope")); // kernel muxes excluded
        assert!(prompt.contains("output_name"));
    }

    #[test]
    fn slug_is_deterministic_and_name_safe() {
        assert_eq!(slug("HSI16"), "hsi16");
        assert_eq!(slug("PLL1R"), "pll1r");
        assert_eq!(slug("PLL 1 R"), "pll_1_r");
        assert_eq!(slug("APB7"), "apb7");
    }

    #[test]
    fn round_trips_through_the_layer1_importer() {
        // The produced GraphClock must survive the .ron path Layer 1 uses.
        let gc = to_graph_clock(&wba_like()).unwrap();
        let text = super::super::import::export_clock_ron(&gc);
        let back = super::super::import::parse_clock_ron(&text).expect("Layer 1 must accept it");
        assert_eq!(
            evaluate(&back.graph).get("sysclk").copied(),
            Some(100_000_000)
        );
    }
}
