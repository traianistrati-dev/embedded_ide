//! Deterministic clock-tree import from an STM32CubeMX installation (Phase 7).
//!
//! **Verified against a real install** (`STM32CubeMX/db`, 67 family files): the
//! clock tree ST draws in CubeMX is shipped as data, and it carries everything
//! this IDE's model needs — including the node coordinates, so the imported
//! diagram is ST's own layout rather than a computed one.
//!
//! Two files are joined, exactly like the pin importer joins `mcu/*.xml` with
//! its IP files:
//!
//! - `db/plugins/clock/STM32<FAM>.xml` — the TOPOLOGY. Each `<Element id type
//!   refParameter x y>` is a node; each `<Input signalId from refValue/>` is a
//!   wire from another element, and `refValue` names the register value that
//!   selects it on a mux — so the input INDEX comes from the register, not from
//!   the order the file happens to list them in.
//! - `db/mcu/IP/RCC-STM32<FAM>_rcc_*_Modes.xml` — the VALUES. `refParameter`
//!   points at a `<RefParameter>` there, which holds either a `<PossibleValue
//!   Comment="4" Value="RCC_SYSCLK_DIV4"/>` list (divider options, mux inputs)
//!   or a `Min`/`Max` integer range (PLL multipliers), plus the reset default.
//!
//! `<Condition Expression="STM32WBAx4|STM32WBAx5"/>` gates elements per chip
//! variant, so the same family file serves several parts. [`Variant`] evaluates
//! those expressions against the tokens a chip defines; an element whose
//! condition is false is left out rather than imported as a dead node.
//!
//! This is the deterministic counterpart to [`super::extract_tree`]: where an
//! ST-supported chip is concerned, this is the accurate path and the AI passes
//! are the fallback for everything else.

use std::collections::BTreeMap;

use super::layout::NodeBox;
use super::model::{ClockGraph, Edge, LimitKey, Node, NodeKind, NodeState};

// ── RCC parameters (the value half) ──────────────────────────────────────────

/// One `<RefParameter>`: either a list of selectable values or an integer range.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Param {
    /// `(Value, Comment)` pairs in file order — `Value` is the register constant
    /// an `<Input refValue=…>` matches, `Comment` the human number (`"4"`).
    pub values: Vec<(String, String)>,
    pub min: Option<u32>,
    pub max: Option<u32>,
    pub default: String,
}

impl Param {
    /// The `Comment`s parsed as divisors — CubeMX writes the numeric factor
    /// there (`Comment="4"` for `RCC_SYSCLK_DIV4`).
    fn divisors(&self) -> Vec<u32> {
        self.values
            .iter()
            .filter_map(|(_, c)| c.trim().parse::<u32>().ok())
            .filter(|d| *d > 0)
            .collect()
    }

    /// Index of the reset default within `values`.
    fn default_index(&self) -> usize {
        self.values
            .iter()
            .position(|(v, _)| *v == self.default)
            .unwrap_or(0)
    }
}

/// Every `<RefParameter>` in an RCC IP file, by name.
pub type RccParams = BTreeMap<String, Param>;

/// Parse `db/mcu/IP/RCC-*_Modes.xml`.
pub fn parse_rcc_params(xml: &str) -> Result<RccParams, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("RCC XML parse error: {e}"))?;
    let mut out = RccParams::new();
    for p in doc.descendants().filter(|n| n.has_tag_name("RefParameter")) {
        let Some(name) = p.attribute("Name") else {
            continue;
        };
        out.insert(
            name.to_owned(),
            Param {
                values: p
                    .children()
                    .filter(|c| c.has_tag_name("PossibleValue"))
                    .filter_map(|c| {
                        Some((
                            c.attribute("Value")?.to_owned(),
                            c.attribute("Comment").unwrap_or_default().to_owned(),
                        ))
                    })
                    .collect(),
                min: p.attribute("Min").and_then(|v| v.trim().parse().ok()),
                max: p.attribute("Max").and_then(|v| v.trim().parse().ok()),
                default: p.attribute("DefaultValue").unwrap_or_default().to_owned(),
            },
        );
    }
    if out.is_empty() {
        return Err("no <RefParameter> found — is this an RCC *_Modes.xml?".into());
    }
    Ok(out)
}

// ── Variant conditions ───────────────────────────────────────────────────────

/// The tokens a particular chip defines (`STM32WBAx5`, `SAI1_Exist`, …), used to
/// evaluate the `<Condition Expression>` gates.
///
/// Unknown tokens are FALSE: a family file mentions peripherals a given part may
/// not have, and importing a branch for hardware that isn't there is worse than
/// leaving it out (the user can always add it in the editor).
#[derive(Clone, Debug, Default)]
pub struct Variant {
    pub defines: Vec<String>,
}

impl Variant {
    pub fn new<I: IntoIterator<Item = S>, S: Into<String>>(defines: I) -> Self {
        Self {
            defines: defines.into_iter().map(Into::into).collect(),
        }
    }

    /// Evaluate a CubeMX condition: identifiers combined with `&`, `|`, `!` and
    /// parentheses. `&` binds tighter than `|`, as in C. An empty expression is
    /// true (the element is unconditional).
    pub fn eval(&self, expr: &str) -> bool {
        let expr = expr.trim();
        if expr.is_empty() {
            return true;
        }
        let toks: Vec<char> = expr.chars().collect();
        let mut pos = 0;
        self.or_expr(&toks, &mut pos)
    }

    fn or_expr(&self, t: &[char], i: &mut usize) -> bool {
        let mut v = self.and_expr(t, i);
        while self.peek(t, i) == Some('|') {
            *i += 1;
            v |= self.and_expr(t, i);
        }
        v
    }

    fn and_expr(&self, t: &[char], i: &mut usize) -> bool {
        let mut v = self.unary(t, i);
        while self.peek(t, i) == Some('&') {
            *i += 1;
            v &= self.unary(t, i);
        }
        v
    }

    fn unary(&self, t: &[char], i: &mut usize) -> bool {
        match self.peek(t, i) {
            Some('!') => {
                *i += 1;
                !self.unary(t, i)
            }
            Some('(') => {
                *i += 1;
                let v = self.or_expr(t, i);
                if self.peek(t, i) == Some(')') {
                    *i += 1;
                }
                v
            }
            _ => {
                let mut id = String::new();
                while *i < t.len() && (t[*i].is_alphanumeric() || t[*i] == '_' || t[*i] == '.') {
                    id.push(t[*i]);
                    *i += 1;
                }
                // A malformed expression yields an empty identifier; skip the
                // offending char so evaluation always terminates.
                if id.is_empty() && *i < t.len() {
                    *i += 1;
                    return false;
                }
                self.defines.iter().any(|d| *d == id)
            }
        }
    }

    /// Skip whitespace and report the next character — advancing `i` past the
    /// spaces, so the identifier reader that follows starts on real input.
    fn peek(&self, t: &[char], i: &mut usize) -> Option<char> {
        while *i < t.len() && t[*i].is_whitespace() {
            *i += 1;
        }
        t.get(*i).copied()
    }
}

// ── The clock tree (the topology half) ───────────────────────────────────────

/// One imported element, before it becomes a node.
struct RawElement<'a> {
    id: &'a str,
    kind: &'a str,
    param: Option<&'a str>,
    x: f32,
    y: f32,
    /// `(from, refValue)` per `<Input>`, in file order.
    inputs: Vec<(&'a str, Option<&'a str>)>,
}

/// Import a CubeMX clock tree: the graph plus ST's own node positions.
///
/// `variant` decides which conditional elements belong to this chip. Elements
/// whose `refParameter` is missing from `rcc` still import — they simply get a
/// pass-through/default shape, which is better than dropping a branch.
pub fn parse_clock_tree(
    clock_xml: &str,
    rcc: &RccParams,
    variant: &Variant,
) -> Result<(ClockGraph, Vec<NodeBox>), String> {
    let doc =
        roxmltree::Document::parse(clock_xml).map_err(|e| format!("clock XML parse error: {e}"))?;

    // 1. Collect the elements this variant has.
    let mut raw: Vec<RawElement> = Vec::new();
    for el in doc.descendants().filter(|n| n.has_tag_name("Element")) {
        let keep = el
            .children()
            .filter(|c| c.has_tag_name("Condition"))
            .all(|c| variant.eval(c.attribute("Expression").unwrap_or_default()));
        if !keep {
            continue;
        }
        let (Some(id), Some(kind)) = (el.attribute("id"), el.attribute("type")) else {
            continue;
        };
        // The same id can appear twice under mutually exclusive conditions
        // (LSIRC on WBAx4 vs the rest); the first one this variant matches wins.
        if raw.iter().any(|r| r.id == id) {
            continue;
        }
        raw.push(RawElement {
            id,
            kind,
            param: el.attribute("refParameter"),
            x: el
                .attribute("x")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0),
            y: el
                .attribute("y")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0),
            inputs: el
                .children()
                .filter(|c| c.has_tag_name("Input"))
                .filter(|c| {
                    c.children()
                        .filter(|g| g.has_tag_name("Condition"))
                        .all(|g| variant.eval(g.attribute("Expression").unwrap_or_default()))
                })
                .filter_map(|c| Some((c.attribute("from")?, c.attribute("refValue"))))
                .collect(),
        });
    }
    if raw.is_empty() {
        return Err("no clock elements found — is this a db/plugins/clock file?".into());
    }

    // 2. Elements → nodes.
    let mut nodes = Vec::with_capacity(raw.len());
    let mut boxes = Vec::with_capacity(raw.len());
    for r in &raw {
        let p = r.param.and_then(|name| rcc.get(name));
        let (kind, state) = shape(r, p);
        nodes.push(Node {
            id: r.id.to_owned(),
            kind,
            state,
            limit: None,
        });
        boxes.push(NodeBox {
            node: r.id.to_owned(),
            x: r.x,
            y: r.y,
            w: 96.0,
            h: 26.0,
        });
    }

    // 3. Inputs → edges. A mux input's index comes from the REGISTER value that
    //    selects it, so a file listing them in a different order still wires up
    //    correctly; without a `refValue` we fall back to file order.
    let mut edges = Vec::new();
    for r in &raw {
        let p = r.param.and_then(|name| rcc.get(name));
        for (n, (from, ref_value)) in r.inputs.iter().enumerate() {
            if !raw.iter().any(|o| o.id == *from) {
                continue; // fed by an element this variant does not have
            }
            let input = match (r.kind, ref_value, p) {
                ("multiplexor", Some(rv), Some(p)) => {
                    p.values.iter().position(|(v, _)| v == rv).unwrap_or(n)
                }
                ("multiplexor", _, _) => n,
                _ => 0,
            };
            edges.push(Edge {
                from: (*from).to_owned(),
                to: r.id.to_owned(),
                input,
            });
        }
    }

    Ok((ClockGraph { nodes, edges }, boxes))
}

/// The node kind + reset state for one element.
fn shape(r: &RawElement, p: Option<&Param>) -> (NodeKind, NodeState) {
    let default_int = || -> u32 {
        p.and_then(|p| p.default.trim().parse::<u32>().ok())
            .unwrap_or(0)
    };
    match r.kind {
        "fixedSource" | "variedSource" => {
            let hz = default_int();
            let (min, max) = match p {
                Some(p) => (p.min.unwrap_or(hz), p.max.unwrap_or(hz)),
                None => (hz, hz),
            };
            (
                NodeKind::Source {
                    min_hz: min,
                    max_hz: max.max(min),
                    gated: r.kind == "variedSource",
                },
                NodeState::Source {
                    enabled: true,
                    hz: hz.clamp(min, max.max(min)),
                },
            )
        }
        "multiplexor" => {
            let n = p.map(|p| p.values.len()).unwrap_or(r.inputs.len()).max(1);
            let sel = p.map(|p| p.default_index()).unwrap_or(0);
            (
                NodeKind::Mux { inputs: n },
                NodeState::Index(sel.min(n.saturating_sub(1))),
            )
        }
        "devisor" => match p {
            Some(p) if !p.divisors().is_empty() => {
                let options = p.divisors();
                // The default is a Value in the list; its position indexes the
                // parsed divisors only when every entry parsed, so re-find it by
                // the number the Comment carries.
                let want = p
                    .values
                    .iter()
                    .find(|(v, _)| *v == p.default)
                    .and_then(|(_, c)| c.trim().parse::<u32>().ok());
                let idx = want
                    .and_then(|w| options.iter().position(|o| *o == w))
                    .unwrap_or(0);
                (NodeKind::Divider { options }, NodeState::Index(idx))
            }
            // An integer-typed divider (Min/Max) — e.g. PLLM.
            Some(p) if p.max.is_some() => {
                let (lo, hi) = (p.min.unwrap_or(1).max(1), p.max.unwrap_or(1).max(1));
                let options: Vec<u32> = (lo..=hi).collect();
                let idx = default_int()
                    .checked_sub(lo)
                    .map(|d| d as usize)
                    .unwrap_or(0);
                (
                    NodeKind::Divider { options },
                    NodeState::Index(idx.min((hi - lo) as usize)),
                )
            }
            _ => (NodeKind::FixedDiv { by: 1 }, NodeState::Fixed),
        },
        "multiplicator" | "multiplicatorFrac" | "fractional" => match p {
            // A ×1/×2 style list (the APB timer rule) is a ratio choice.
            Some(p) if !p.values.is_empty() => {
                let ratios: Vec<(u32, u32)> =
                    p.divisors().into_iter().map(|n| (n, 1)).collect::<Vec<_>>();
                if ratios.is_empty() {
                    (NodeKind::Tap, NodeState::Fixed)
                } else {
                    let idx = p.default_index().min(ratios.len() - 1);
                    (NodeKind::Choice { ratios }, NodeState::Index(idx))
                }
            }
            Some(p) if p.max.is_some() => {
                let (lo, hi) = (p.min.unwrap_or(1).max(1), p.max.unwrap_or(1).max(1));
                (
                    NodeKind::Multiplier { min: lo, max: hi },
                    NodeState::Value(default_int().clamp(lo, hi)),
                )
            }
            _ => (NodeKind::Tap, NodeState::Fixed),
        },
        "output" | "activeOutput" => (NodeKind::Output, NodeState::Fixed),
        // Anything new ST adds still imports, as a pass-through.
        _ => (NodeKind::Tap, NodeState::Fixed),
    }
}

/// Attach a datasheet ceiling to a node, by id — the CubeMX files describe the
/// topology but not the maxima, which the IDE already carries per chip.
pub fn set_limit(graph: &mut ClockGraph, id: &str, hz: u32) {
    if let Some(n) = graph.node_mut(id) {
        n.limit = Some(LimitKey::Hz(hz));
    }
}

// ── Locating the two files ───────────────────────────────────────────────────

/// The RCC IP file that goes with a clock-tree file.
///
/// It cannot be derived by formatting a name: the version suffix varies
/// (`_rcc_v1_0_`, `_rcc_v1_2_`) and so does the separator — ST ships
/// `RCC-STM32WBA_rcc_v1_0_Modes.xml` next to `RCC-STM32F411-rcc_v1_0_Modes.xml`.
/// So it is FOUND by prefix, and the newest match wins.
pub fn find_rcc_file(db_dir: &std::path::Path, family: &str) -> Option<std::path::PathBuf> {
    let ip = db_dir.join("mcu").join("IP");
    let want = format!("rcc-{}", family.to_ascii_lowercase());
    let mut hits: Vec<std::path::PathBuf> = std::fs::read_dir(ip)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                return false;
            };
            let lower = name.to_ascii_lowercase();
            lower.ends_with("_modes.xml")
                && lower
                    .strip_prefix(&want)
                    .is_some_and(|rest| rest.starts_with('_') || rest.starts_with('-'))
        })
        .collect();
    hits.sort();
    hits.pop()
}

/// The family key a clock-tree file stands for: `…/plugins/clock/STM32WBA.xml`
/// → `STM32WBA`. Used to find its RCC partner.
pub fn family_of(clock_path: &std::path::Path) -> Option<String> {
    clock_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_owned())
}

/// Import both halves from a CubeMX `db` directory: the family's clock tree and
/// the RCC parameters it references.
pub fn import_from_db(
    db_dir: &std::path::Path,
    family: &str,
    variant: &Variant,
) -> Result<(ClockGraph, Vec<NodeBox>), String> {
    let clock_path = db_dir
        .join("plugins")
        .join("clock")
        .join(format!("{family}.xml"));
    let clock = std::fs::read_to_string(&clock_path)
        .map_err(|e| format!("could not read {}: {e}", clock_path.display()))?;
    let rcc_path = find_rcc_file(db_dir, family)
        .ok_or_else(|| format!("no RCC IP file for {family} under {}", db_dir.display()))?;
    let rcc = std::fs::read_to_string(&rcc_path)
        .map_err(|e| format!("could not read {}: {e}", rcc_path.display()))?;
    parse_clock_tree(&clock, &parse_rcc_params(&rcc)?, variant)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::clock::graph::evaluate;

    const RCC: &str = r#"<IP>
        <RefParameter Name="HSI_VALUE" Type="integer" DefaultValue="16000000"/>
        <RefParameter Name="HSE_VALUE" Type="integer" DefaultValue="32000000" Min="32000000" Max="32000000"/>
        <RefParameter Name="SYSCLKSource" Type="list" DefaultValue="RCC_SYSCLKSOURCE_HSE">
            <PossibleValue Value="RCC_SYSCLKSOURCE_HSI" Comment="Hsi"/>
            <PossibleValue Value="RCC_SYSCLKSOURCE_HSE" Comment="Hse"/>
        </RefParameter>
        <RefParameter Name="AHBCLKDivider" Type="list" DefaultValue="RCC_SYSCLK_DIV2">
            <PossibleValue Value="RCC_SYSCLK_DIV1" Comment="1"/>
            <PossibleValue Value="RCC_SYSCLK_DIV2" Comment="2"/>
            <PossibleValue Value="RCC_SYSCLK_DIV4" Comment="4"/>
        </RefParameter>
        <RefParameter Name="PLLN" Type="integer" Min="4" Max="512" DefaultValue="25"/>
        <RefParameter Name="APB1TimCLKDivider" Type="list" DefaultValue="RCC_HCLK_DIV1">
            <PossibleValue Value="RCC_HCLK_DIV1" Comment="1"/>
            <PossibleValue Value="RCC_HCLK_DIV2" Comment="2"/>
        </RefParameter>
    </IP>"#;

    /// The shape of a real family file, cut down to what the parser reads.
    const CLOCK: &str = r#"<Clock>
        <Element id="HSIRC" type="fixedSource" refParameter="HSI_VALUE" x="295" y="485"/>
        <Element id="HSEOSC" type="variedSource" refParameter="HSE_VALUE" x="200" y="650"/>
        <Element id="OnlyOnBigParts" type="fixedSource" refParameter="HSI_VALUE" x="0" y="0">
            <Condition Expression="STM32WBAx5" Diagnostic=""/>
        </Element>
        <Element id="SysClkSource" type="multiplexor" refParameter="SYSCLKSource" x="502" y="427">
            <Input signalId="HSI" from="HSIRC" refValue="RCC_SYSCLKSOURCE_HSI"/>
            <Input signalId="HSE" from="HSEOSC" refValue="RCC_SYSCLKSOURCE_HSE"/>
        </Element>
        <Element id="AHBPrescaler" type="devisor" refParameter="AHBCLKDivider" x="900" y="427">
            <Input signalId="SYSCLK" from="SysClkSource"/>
        </Element>
        <Element id="HCLK" type="output" x="1100" y="427">
            <Input signalId="HCLK" from="AHBPrescaler"/>
        </Element>
    </Clock>"#;

    fn parse(defines: &[&str]) -> (ClockGraph, Vec<NodeBox>) {
        let rcc = parse_rcc_params(RCC).expect("rcc");
        parse_clock_tree(CLOCK, &rcc, &Variant::new(defines.to_vec())).expect("clock")
    }

    /// End to end: ST's data becomes a tree that evaluates to ST's numbers.
    #[test]
    fn a_cubemx_tree_imports_and_evaluates() {
        let (graph, boxes) = parse(&[]);
        // The conditional element is not on this variant.
        assert!(graph.node("OnlyOnBigParts").is_none());
        assert_eq!(graph.nodes.len(), 5);

        let f = evaluate(&graph);
        // HSE 32 MHz selected by default, AHB /2 by default.
        assert_eq!(f["SysClkSource"], 32_000_000);
        assert_eq!(f["HCLK"], 16_000_000);

        // ST's own coordinates come along, so the diagram is their layout.
        let hse = boxes.iter().find(|b| b.node == "HSEOSC").unwrap();
        assert_eq!((hse.x, hse.y), (200.0, 650.0));
    }

    /// A conditional element joins the tree when the chip defines its token.
    #[test]
    fn variant_conditions_select_the_elements() {
        let (graph, _) = parse(&["STM32WBAx5"]);
        assert!(graph.node("OnlyOnBigParts").is_some());
    }

    /// Mux inputs are indexed by the REGISTER value, not by file order — the
    /// same guarantee the AI path gets from matching names.
    #[test]
    fn mux_inputs_follow_the_register_value_not_the_file_order() {
        let clock = CLOCK.replace(
            r#"<Input signalId="HSI" from="HSIRC" refValue="RCC_SYSCLKSOURCE_HSI"/>
            <Input signalId="HSE" from="HSEOSC" refValue="RCC_SYSCLKSOURCE_HSE"/>"#,
            // Same two inputs, listed the other way round.
            r#"<Input signalId="HSE" from="HSEOSC" refValue="RCC_SYSCLKSOURCE_HSE"/>
            <Input signalId="HSI" from="HSIRC" refValue="RCC_SYSCLKSOURCE_HSI"/>"#,
        );
        let rcc = parse_rcc_params(RCC).unwrap();
        let (graph, _) = parse_clock_tree(&clock, &rcc, &Variant::default()).unwrap();
        let idx = |from: &str| {
            graph
                .edges
                .iter()
                .find(|e| e.from == from && e.to == "SysClkSource")
                .unwrap()
                .input
        };
        assert_eq!((idx("HSIRC"), idx("HSEOSC")), (0, 1), "register order wins");
        // And the default selection still resolves to HSE.
        assert_eq!(evaluate(&graph)["SysClkSource"], 32_000_000);
    }

    /// Divider option lists come from the `Comment` numbers, with the reset
    /// default selected.
    #[test]
    fn divider_options_and_defaults_come_from_the_rcc_file() {
        let (graph, _) = parse(&[]);
        let ahb = graph.node("AHBPrescaler").unwrap();
        let NodeKind::Divider { options } = &ahb.kind else {
            panic!("a divider");
        };
        assert_eq!(options, &[1, 2, 4]);
        assert_eq!(ahb.state, NodeState::Index(1), "RCC_SYSCLK_DIV2");
    }

    /// The two parameter shapes ST uses for multipliers: an integer range (PLLN)
    /// and a ×1/×2 list (the APB timer rule).
    #[test]
    fn multipliers_take_either_shape() {
        let rcc = parse_rcc_params(RCC).unwrap();
        let clock = r#"<Clock>
            <Element id="Src" type="fixedSource" refParameter="HSI_VALUE" x="0" y="0"/>
            <Element id="PLLN" type="multiplicator" refParameter="PLLN" x="1" y="1">
                <Input signalId="a" from="Src"/>
            </Element>
            <Element id="TimPrescalerAPB1" type="multiplicator" refParameter="APB1TimCLKDivider" x="2" y="2">
                <Input signalId="b" from="Src"/>
            </Element>
        </Clock>"#;
        let (graph, _) = parse_clock_tree(clock, &rcc, &Variant::default()).unwrap();
        assert!(matches!(
            graph.node("PLLN").unwrap().kind,
            NodeKind::Multiplier { min: 4, max: 512 }
        ));
        assert_eq!(graph.node("PLLN").unwrap().state, NodeState::Value(25));
        assert!(matches!(
            &graph.node("TimPrescalerAPB1").unwrap().kind,
            NodeKind::Choice { ratios } if ratios == &[(1, 1), (2, 1)]
        ));
    }

    /// The condition language: `&` binds tighter than `|`, `!` negates, and
    /// unknown tokens are false.
    #[test]
    fn condition_expressions_evaluate_like_c() {
        let v = Variant::new(vec!["A", "B"]);
        assert!(v.eval(""));
        assert!(v.eval("A"));
        assert!(!v.eval("C"));
        assert!(v.eval("A & B"));
        assert!(!v.eval("A & C"));
        assert!(v.eval("A | C"));
        assert!(v.eval("!C"));
        assert!(!v.eval("!A"));
        assert!(v.eval("!STM32WBAx4&!STM32WBAx5"), "the real WBA form");
        assert!(v.eval("(A|C) & B"));
        assert!(!v.eval("C | (A & !B)"));
    }

    #[test]
    fn a_wrong_file_is_reported() {
        assert!(
            parse_rcc_params("<Nope/>")
                .unwrap_err()
                .contains("RefParameter")
        );
        let rcc = parse_rcc_params(RCC).unwrap();
        assert!(
            parse_clock_tree("<Nope/>", &rcc, &Variant::default())
                .unwrap_err()
                .contains("no clock elements")
        );
        assert!(parse_rcc_params("not xml at all <<<").is_err());
    }

    /// Run against a REAL CubeMX install (ignored — it needs one):
    /// `cargo test -- --ignored cubemx_real_install --nocapture`
    #[test]
    #[ignore]
    fn cubemx_real_install() {
        let db =
            std::path::Path::new(r"C:\Program Files\STMicroelectronics\STM32Cube\STM32CubeMX\db");
        if !db.exists() {
            eprintln!("no CubeMX install at {} — skipping", db.display());
            return;
        }
        for fam in ["STM32WBA", "STM32F411", "STM32G0", "STM32L4"] {
            let variant = Variant::new(vec!["STM32WBAx5", "SAI1_Exist", "USART2_Exist"]);
            let (graph, boxes) = match import_from_db(db, fam, &variant) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{fam}: {e}");
                    continue;
                }
            };
            let freqs = evaluate(&graph);
            let live = freqs.values().filter(|hz| **hz > 0).count();
            eprintln!(
                "{fam}: {} nodes, {} edges, {} boxes, {live} nodes with a frequency",
                graph.nodes.len(),
                graph.edges.len(),
                boxes.len()
            );
            assert!(graph.nodes.len() > 20, "{fam} should be a full tree");
            assert!(live > 5, "{fam} should evaluate to real frequencies");
            assert!(
                boxes.iter().any(|b| b.x > 0.0 && b.y > 0.0),
                "{fam} should carry ST's coordinates"
            );
        }
    }
}
