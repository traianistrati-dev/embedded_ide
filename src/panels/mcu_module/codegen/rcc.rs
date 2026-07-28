//! Data-driven RCC clock-block emitter (Layer 2).
//!
//! `f4.rs` and `wba.rs` emitted almost the same `embassy_stm32::Config` block —
//! ~85% identical text, differing only in a handful of family quirks (which PLL
//! output, the HSE syntax, the extra APB bus, the voltage-scale line). This
//! captures those quirks as a [`RccDescriptor`] and emits from ONE generic
//! function, so adding a family is a descriptor, not another copy of the string
//! builder.
//!
//! The GRAPH → values step stays per-family (`graph_to_f4` / `graph_to_wba`):
//! it is already identical across families (same node ids, same readers), and
//! keeping it lets both funnel through [`RccValues`] here. Correctness is pinned
//! by the existing `f4`/`wba` codegen tests — this must reproduce their exact
//! output byte-for-byte.

use super::super::clock::graph::model::{ClockGraph, NodeKind, NodeState};
use super::super::clock::model::ClockConfig;

/// embassy's reset default (HSI, everything /1) — the clock line for a family
/// with no RCC recipe, or whose graph selections happen to equal the reset.
const EMBASSY_RESET_INIT: &str =
    "    let p = embassy_stm32::init(Default::default()); // reset clock (HSI). \
     Set embassy_stm32::Config for RCC if needed.\n";

/// The RCC recipe (how to READ the graph + how to EMIT the config) for an STM32
/// `family` key, if one exists. **This is the data that replaced the old
/// `is_f4_graph` / `is_wba_graph` topology sniffing**: the chip's FAMILY selects
/// the codegen — which is physically true (every STM32F4 shares the F4 RCC) —
/// so an unusual graph shape can't mis-route it and a new family is just one arm
/// here plus its [`ReadSpec`] / [`RccDescriptor`] constructors.
pub fn rcc_recipe(family: &str) -> Option<(ReadSpec, RccDescriptor)> {
    match family {
        // F2 / F4 / F7 share embassy's `rcc/f247.rs` (same Config/Pll/Sysclk),
        // so they share ONE recipe — families on a common RCC module cost only
        // an extra pattern here. (Their N range differs but the shipped graph's
        // 50–432 window is valid for all three; verified vs metapac rcc_f2/f7.)
        "stm32f2" | "stm32f4" | "stm32f7" => Some((ReadSpec::f4(), RccDescriptor::f4())),
        "stm32g0" => Some((ReadSpec::g0(), RccDescriptor::g0())),
        "stm32g4" => Some((ReadSpec::g4(), RccDescriptor::g4())),
        "stm32l4" => Some((ReadSpec::l4(), RccDescriptor::l4())),
        "stm32wba" => Some((ReadSpec::wba(), RccDescriptor::wba())),
        _ => None,
    }
}

/// Emit the RCC clock block for an embassy STM32 `family` from its Clock-tab
/// graph. Families with no recipe — or a non-graph / reset-equivalent clock —
/// get embassy's reset default. Replaces the per-family `f4::clock_block` /
/// `wba::clock_block` and the `stm_clock_block` sniffing with one dispatch.
pub fn graph_clock_block(family: &str, clock: &ClockConfig) -> String {
    let Some((spec, desc)) = rcc_recipe(family) else {
        return EMBASSY_RESET_INIT.to_string();
    };
    let values = match clock {
        ClockConfig::Graph(gc) => read_rcc_values(&gc.graph, &spec),
        _ => spec.reset.clone(),
    };
    // The reset shortcut only applies when the chip's HW default equals this
    // reset (HSI). L4/L5/U5 default to MSI, so a reset-equivalent graph must
    // still emit an explicit HSI block, not `init(Default::default())`.
    if spec.reset_is_hw_default && values == spec.reset {
        return EMBASSY_RESET_INIT.to_string();
    }
    emit_rcc_block(&desc, &values)
}

/// The clock selections, family-neutral — what the generic emitter consumes and
/// the generic reader ([`read_rcc_values`]) produces.
#[derive(Clone, Debug, PartialEq)]
pub struct RccValues {
    /// `true` = PLL drives SYSCLK; `false` with `hse_on` = HSE; else HSI.
    pub sys: SysSource,
    pub hse_on: bool,
    pub hse_hz: u32,
    pub pll_src_hse: bool,
    pub pll_m: u32,
    pub pll_n: u32,
    /// The single PLL output divider in use (F4 = P, WBA = R).
    pub pll_out: u32,
    pub ahb: u32,
    /// SYSCLK produced, in Hz — for the header comment.
    pub sysclk_hz: u32,
    /// `(embassy field suffix, divisor)` per APB bus, in emit order.
    pub apb: Vec<(&'static str, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SysSource {
    Hsi,
    Hse,
    Pll,
}

/// How a family writes the HSE oscillator into `config.rcc.hse`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HseStyle {
    /// `Hse { freq: Hertz(hz), mode: HseMode::Oscillator }` — the crystal
    /// frequency is a field (F4).
    Freq,
    /// `Hse { prescaler: HsePrescaler::DIV1 }` — a fixed crystal, no freq field
    /// (WBA's 32 MHz radio crystal).
    FixedPrescaler,
}

/// How the HSE appears in the human-readable header comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HseLabel {
    /// `HSE {mhz} MHz` (F4 — any crystal).
    WithMhz,
    /// A fixed string, e.g. `HSE32` (WBA).
    Fixed(&'static str),
}

/// Everything family-specific about emitting the RCC block. The ~15 fields that
/// differ between F4 and WBA — a new family is one of these.
#[derive(Clone, Debug, PartialEq)]
pub struct RccDescriptor {
    /// The `config.rcc.<field>` that holds the PLL: `pll` (F4) / `pll1` (WBA).
    pub pll_field: &'static str,
    /// `true`: PLL source is a nested `source:` line inside the `Pll { … }`
    /// struct (WBA). `false`: a separate `config.rcc.pll_src` line (F4).
    pub pll_source_nested: bool,
    /// embassy divider TYPE for the used PLL output: `PllPDiv` (F4) / `PllDiv`
    /// (WBA).
    pub pll_out_div_type: &'static str,
    /// Which `Pll { … }` field the used output fills: `divp` (F4) / `divr`
    /// (WBA). The other two outputs are emitted as `None`.
    pub pll_out_field: &'static str,
    /// `Sysclk::<variant>` when SYSCLK is the PLL: `PLL1_P` (F4) / `PLL1_R`
    /// (WBA).
    pub sys_pll_variant: &'static str,
    /// Emit a `frac: None` line in the `Pll { … }` (WBA yes, F4 no).
    pub pll_has_frac: bool,
    /// HSE write style.
    pub hse_style: HseStyle,
    /// Emit `config.rcc.voltage_scale = rcc::VoltageScale::RANGE1;` whenever
    /// SYSCLK is not plain HSI (WBA's PLL panics in range 2). Empty = never.
    pub voltage_scale_non_hsi: Option<&'static str>,
    /// Header-comment label for the HSI source (always `HSI16` today).
    pub hsi_label: &'static str,
    /// Header-comment label for the HSE source.
    pub hse_label: HseLabel,
    /// The `via <X>` suffix in the PLL description: `PLLP` (F4) / `PLL1R` (WBA).
    pub pll_desc_via: &'static str,
    /// Emit `config.rcc.hsi = true;` when HSI drives SYSCLK or the PLL. Needed
    /// by families whose `Config::default()` leaves HSI OFF (L4/L5/U5:
    /// `hsi: bool = false`); false where HSI is on by default (F4/G4/G0) or
    /// unused (WBA fixed HSE).
    pub hsi_needs_enable: bool,
}

impl RccDescriptor {
    /// STM32F4: separate `pll_src`, P output, no voltage scale, APB1/2.
    pub fn f4() -> Self {
        Self {
            pll_field: "pll",
            pll_source_nested: false,
            pll_out_div_type: "PllPDiv",
            pll_out_field: "divp",
            sys_pll_variant: "PLL1_P",
            pll_has_frac: false,
            hse_style: HseStyle::Freq,
            voltage_scale_non_hsi: None,
            hsi_label: "HSI16",
            hse_label: HseLabel::WithMhz,
            pll_desc_via: "PLLP",
            hsi_needs_enable: false,
        }
    }

    /// STM32G4: `pll` with nested source (no frac), R output, a real HSE
    /// crystal, APB1/2. Verified against embassy-stm32 v0.4.0 `src/rcc/g4.rs`:
    /// `Config { hsi: true (default), hse, sys: Sysclk, pll: Option<Pll>,
    /// ahb_pre, apb1_pre, apb2_pre, boost }`; `Pll { source, prediv, mul, divp,
    /// divq, divr }`; SYSCLK-from-PLL variant `Sysclk::PLL1_R`. No `voltage_
    /// scale` field (G4 uses `boost`, which defaults false — fine ≤150 MHz, the
    /// shipped preset's ceiling); embassy sets flash latency itself.
    pub fn g4() -> Self {
        Self {
            pll_field: "pll",
            pll_source_nested: true,
            pll_out_div_type: "PllRDiv",
            pll_out_field: "divr",
            sys_pll_variant: "PLL1_R",
            pll_has_frac: false,
            hse_style: HseStyle::Freq,
            voltage_scale_non_hsi: None,
            hsi_label: "HSI16",
            hse_label: HseLabel::WithMhz,
            pll_desc_via: "PLLR",
            hsi_needs_enable: false,
        }
    }

    /// STM32L4: same emit shape as G4 (nested source, R output, real HSE, no
    /// frac, no voltage_scale line) EXCEPT `hsi_needs_enable` — L4's
    /// `Config::default()` leaves `hsi: false`, so an HSI-sourced clock must
    /// switch it on explicitly. Verified against embassy `l.rs` + metapac
    /// `rcc_l4` (Sysclk::PLL1_R, Plln MUL8..127, Pllr {2,4,6,8}).
    pub fn l4() -> Self {
        Self { hsi_needs_enable: true, ..Self::g4() }
    }

    /// STM32G0: identical emit shape to G4 (nested source, R output, real HSE,
    /// no frac, no voltage_scale — G0 uses `voltage_range`, default Range1 is
    /// fine ≤64 MHz). The only difference from G4 is the single APB bus, which
    /// lives in [`ReadSpec::g0`], not here. Verified against embassy `g0.rs`.
    pub fn g0() -> Self {
        Self::g4() // same descriptor; the bus count is a ReadSpec concern
    }

    /// STM32WBA: `pll1` with nested source + frac, R output, RANGE1 off-HSI,
    /// fixed 32 MHz HSE, APB1/2/7.
    pub fn wba() -> Self {
        Self {
            pll_field: "pll1",
            pll_source_nested: true,
            pll_out_div_type: "PllDiv",
            pll_out_field: "divr",
            sys_pll_variant: "PLL1_R",
            pll_has_frac: true,
            hse_style: HseStyle::FixedPrescaler,
            voltage_scale_non_hsi: Some("RANGE1"),
            hsi_label: "HSI16",
            hse_label: HseLabel::Fixed("HSE32"),
            pll_desc_via: "PLL1R",
            hsi_needs_enable: false,
        }
    }
}

/// How to read a family's selections out of the clock graph.
///
/// The graph→values logic was duplicated verbatim between `graph_to_f4` and
/// `graph_to_wba` — same node ids, same `index_of`/`divisor_of` readers. The
/// only differences are these fields, so a family's reader is now data.
pub struct ReadSpec {
    /// HSI frequency in Hz — 16 MHz on both F4 and WBA (`HSI16`).
    pub hsi_hz: u32,
    /// `Some(hz)` for a fixed crystal (WBA's 32 MHz radio HSE, read from a
    /// register-less node); `None` reads the live frequency from the `hse`
    /// source node (F4).
    pub hse_fixed_hz: Option<u32>,
    /// The single PLL-output divider node id: `pllp` (F4) / `pllr` (WBA).
    pub pll_out_node: &'static str,
    /// `(embassy field suffix, graph node id)` per APB bus, in emit order.
    pub apb: &'static [(&'static str, &'static str)],
    /// The all-default baseline: values with everything at reset. Read as the
    /// fallback for any node the graph is missing, and compared against the
    /// result to decide the `embassy_stm32::init(Default::default())` path.
    pub reset: RccValues,
    /// `true` when `Config::default()` equals `reset` (F4/G4/G0/WBA: HSI) — then
    /// a reset-equivalent graph emits `init(Default::default())`. `false` for
    /// L4/L5/U5, whose HW default is MSI, not HSI: a reset graph must still emit
    /// an explicit HSI block, so the shortcut is skipped.
    pub reset_is_hw_default: bool,
}

impl ReadSpec {
    pub fn f4() -> Self {
        Self {
            reset_is_hw_default: true,
            hsi_hz: 16_000_000,
            hse_fixed_hz: None,
            pll_out_node: "pllp",
            apb: &[("apb1_pre", "apb1"), ("apb2_pre", "apb2")],
            reset: RccValues {
                sys: SysSource::Hsi,
                hse_on: false,
                hse_hz: 8_000_000,
                pll_src_hse: false,
                pll_m: 8,
                pll_n: 100,
                pll_out: 2,
                ahb: 1,
                sysclk_hz: 16_000_000,
                apb: vec![("apb1_pre", 1), ("apb2_pre", 1)],
            },
        }
    }

    /// G4: HSI16, a live HSE crystal (read from the `hse` node), PLL output on
    /// `pllr`, APB1/2. `reset` mirrors the shipped graph's default PLL
    /// selections (M=4, N=75, R=2) so a fully-reset graph reads back as reset.
    pub fn g4() -> Self {
        Self {
            reset_is_hw_default: true,
            hsi_hz: 16_000_000,
            hse_fixed_hz: None,
            pll_out_node: "pllr",
            apb: &[("apb1_pre", "apb1"), ("apb2_pre", "apb2")],
            reset: RccValues {
                sys: SysSource::Hsi,
                hse_on: false,
                hse_hz: 8_000_000,
                pll_src_hse: false,
                pll_m: 4,
                pll_n: 75,
                pll_out: 2,
                ahb: 1,
                sysclk_hz: 16_000_000,
                apb: vec![("apb1_pre", 1), ("apb2_pre", 1)],
            },
        }
    }

    /// G0: HSI16, a live HSE crystal, PLL output on `pllr`, and a SINGLE APB
    /// bus (G0 has no APB2). `reset` mirrors the shipped 64 MHz preset's PLL
    /// selections (M=1, N=8, R=2).
    pub fn g0() -> Self {
        Self {
            reset_is_hw_default: true,
            hsi_hz: 16_000_000,
            hse_fixed_hz: None,
            pll_out_node: "pllr",
            apb: &[("apb1_pre", "apb1")],
            reset: RccValues {
                sys: SysSource::Hsi,
                hse_on: false,
                hse_hz: 8_000_000,
                pll_src_hse: false,
                pll_m: 1,
                pll_n: 8,
                pll_out: 2,
                ahb: 1,
                sysclk_hz: 16_000_000,
                apb: vec![("apb1_pre", 1)],
            },
        }
    }

    /// L4: HSI16, live HSE crystal, PLL output on `pllr`, APB1/2. `reset` mirrors
    /// the shipped 80 MHz preset (M=1, N=10, R=2); `reset_is_hw_default: false`
    /// because L4 boots on MSI, so a reset graph still emits an explicit HSI
    /// block rather than `init(Default::default())`.
    pub fn l4() -> Self {
        Self {
            reset_is_hw_default: false,
            hsi_hz: 16_000_000,
            hse_fixed_hz: None,
            pll_out_node: "pllr",
            apb: &[("apb1_pre", "apb1"), ("apb2_pre", "apb2")],
            reset: RccValues {
                sys: SysSource::Hsi,
                hse_on: false,
                hse_hz: 8_000_000,
                pll_src_hse: false,
                pll_m: 1,
                pll_n: 10,
                pll_out: 2,
                ahb: 1,
                sysclk_hz: 16_000_000,
                apb: vec![("apb1_pre", 1), ("apb2_pre", 1)],
            },
        }
    }

    pub fn wba() -> Self {
        Self {
            reset_is_hw_default: true,
            hsi_hz: 16_000_000,
            hse_fixed_hz: Some(32_000_000),
            pll_out_node: "pllr",
            apb: &[
                ("apb1_pre", "apb1"),
                ("apb2_pre", "apb2"),
                ("apb7_pre", "apb7"),
            ],
            reset: RccValues {
                sys: SysSource::Hsi,
                hse_on: false,
                hse_hz: 32_000_000,
                pll_src_hse: true,
                pll_m: 2,
                pll_n: 25,
                pll_out: 4,
                ahb: 1,
                sysclk_hz: 16_000_000,
                apb: vec![("apb1_pre", 1), ("apb2_pre", 1), ("apb7_pre", 1)],
            },
        }
    }
}

/// Read the user's clock selections out of `g` per `spec` — the ONE reader that
/// replaced `graph_to_f4` / `graph_to_wba`. Missing/foreign nodes fall back to
/// `spec.reset`, so a non-matching graph degrades to the reset state.
pub fn read_rcc_values(g: &ClockGraph, spec: &ReadSpec) -> RccValues {
    let index_of = |id: &str| match g.node(id).map(|n| &n.state) {
        Some(NodeState::Index(i)) => Some(*i),
        _ => None,
    };
    let divisor_of = |id: &str| -> Option<u32> {
        let node = g.node(id)?;
        let NodeKind::Divider { options } = &node.kind else {
            return None;
        };
        let NodeState::Index(i) = node.state else {
            return None;
        };
        options.get(i).copied()
    };

    let r = &spec.reset;
    let mut hse_on = false;
    let mut hse_hz = spec.hse_fixed_hz.unwrap_or(r.hse_hz);
    if let Some(NodeState::Source { enabled, hz }) = g.node("hse").map(|n| &n.state) {
        hse_on = *enabled;
        if spec.hse_fixed_hz.is_none() {
            hse_hz = *hz;
        }
    }

    let sys = match index_of("sw") {
        Some(0) => SysSource::Hsi,
        Some(1) => SysSource::Hse,
        _ => SysSource::Pll,
    };
    // The pllsrc mux is 2-input (HSI=0, HSE=1): index != 0 → HSE; a missing
    // node keeps the family default. (F4 used `== Some(1)`, WBA `!= Some(0)`;
    // for the only real indices, 0 and 1, both agree with this.)
    let pll_src_hse = index_of("pllsrc")
        .map(|i| i != 0)
        .unwrap_or(r.pll_src_hse);

    let pll_m = divisor_of("pllm").unwrap_or(r.pll_m);
    let pll_n = match g.node("plln").map(|n| &n.state) {
        Some(NodeState::Value(v)) => *v,
        _ => r.pll_n,
    };
    let pll_out = divisor_of(spec.pll_out_node).unwrap_or(r.pll_out);
    let ahb = divisor_of("ahb").unwrap_or(1);
    let apb: Vec<(&'static str, u32)> = spec
        .apb
        .iter()
        .map(|(field, node)| (*field, divisor_of(node).unwrap_or(1)))
        .collect();

    // Sysclk on HSE / PLL-from-HSE implies the oscillator is in use.
    if sys == SysSource::Hse || (sys == SysSource::Pll && pll_src_hse) {
        hse_on = true;
    }

    let sysclk_hz = match sys {
        SysSource::Hsi => spec.hsi_hz,
        SysSource::Hse => hse_hz,
        SysSource::Pll => {
            let src = if pll_src_hse { hse_hz } else { spec.hsi_hz };
            (src / pll_m.max(1)) * pll_n / pll_out.max(1)
        }
    };

    RccValues {
        sys,
        hse_on,
        hse_hz,
        pll_src_hse,
        pll_m,
        pll_n,
        pll_out,
        ahb,
        sysclk_hz,
        apb,
    }
}

/// Emit the full clock block for `values` using the family's `desc`.
///
/// Byte-for-byte compatible with the previous hand-written `f4`/`wba` emitters —
/// their tests are the acceptance criteria. The caller decides the reset case
/// (all-default → `embassy_stm32::init(Default::default())`) before calling.
pub fn emit_rcc_block(desc: &RccDescriptor, v: &RccValues) -> String {
    let mhz = v.sysclk_hz / 1_000_000;
    let hse_mhz = v.hse_hz / 1_000_000;

    // Source description for the header comment.
    let hse_desc = match &desc.hse_label {
        HseLabel::WithMhz => format!("HSE {hse_mhz} MHz"),
        HseLabel::Fixed(s) => s.to_string(),
    };
    let sys_desc = match v.sys {
        SysSource::Hsi => desc.hsi_label.to_string(),
        SysSource::Hse => hse_desc.clone(),
        SysSource::Pll => format!(
            "{} /{} x{} /{} via {}",
            if v.pll_src_hse { hse_desc.clone() } else { desc.hsi_label.to_string() },
            v.pll_m,
            v.pll_n,
            v.pll_out,
            desc.pll_desc_via,
        ),
    };

    let apb_desc = v
        .apb
        .iter()
        .map(|(name, div)| format!("{} /{}", apb_human(name), div))
        .collect::<Vec<_>>()
        .join(" ");

    let mut b = String::new();
    b.push_str(&format!(
        "    // Clock (from the Clock tab): SYSCLK {mhz} MHz ({sys_desc}) · \
         AHB /{} · {apb_desc}\n",
        v.ahb
    ));
    b.push_str("    let mut config = embassy_stm32::Config::default();\n");
    b.push_str("    {\n        use embassy_stm32::rcc;\n");

    // ── HSE ─────────────────────────────────────────────────────────────
    let hse_used = v.sys == SysSource::Hse || (v.sys == SysSource::Pll && v.pll_src_hse);
    if hse_used {
        match desc.hse_style {
            HseStyle::Freq => b.push_str(&format!(
                "        config.rcc.hse = Some(rcc::Hse {{ freq: embassy_stm32::time::Hertz({}), \
                 mode: rcc::HseMode::Oscillator }});\n",
                v.hse_hz
            )),
            HseStyle::FixedPrescaler => b.push_str(
                "        config.rcc.hse = Some(rcc::Hse { prescaler: rcc::HsePrescaler::DIV1 });\n",
            ),
        }
    }

    // ── HSI enable (families whose default leaves HSI off, e.g. L4) ─────
    let hsi_used = v.sys == SysSource::Hsi || (v.sys == SysSource::Pll && !v.pll_src_hse);
    if desc.hsi_needs_enable && hsi_used {
        b.push_str("        config.rcc.hsi = true;\n");
    }

    // ── PLL ─────────────────────────────────────────────────────────────
    if v.sys == SysSource::Pll {
        let src = if v.pll_src_hse { "HSE" } else { "HSI" };
        if !desc.pll_source_nested {
            b.push_str(&format!("        config.rcc.pll_src = rcc::PllSource::{src};\n"));
        }
        b.push_str(&format!(
            "        config.rcc.{field} = Some(rcc::Pll {{\n",
            field = desc.pll_field
        ));
        if desc.pll_source_nested {
            b.push_str(&format!("            source: rcc::PllSource::{src},\n"));
        }
        b.push_str(&format!("            prediv: rcc::PllPreDiv::DIV{},\n", v.pll_m));
        b.push_str(&format!("            mul: rcc::PllMul::MUL{},\n", v.pll_n));
        // Three outputs; the one this family uses gets the value, the rest None.
        for out in ["divp", "divq", "divr"] {
            if out == desc.pll_out_field {
                b.push_str(&format!(
                    "            {out}: Some(rcc::{ty}::DIV{val}),\n",
                    ty = desc.pll_out_div_type,
                    val = v.pll_out,
                ));
            } else {
                b.push_str(&format!("            {out}: None,\n"));
            }
        }
        if desc.pll_has_frac {
            b.push_str("            frac: None,\n");
        }
        b.push_str("        });\n");
    }

    // ── SYSCLK source ───────────────────────────────────────────────────
    let sys = match v.sys {
        SysSource::Hsi => "HSI",
        SysSource::Hse => "HSE",
        SysSource::Pll => desc.sys_pll_variant,
    };
    b.push_str(&format!("        config.rcc.sys = rcc::Sysclk::{sys};\n"));

    // ── Voltage scale (family option) ───────────────────────────────────
    if let Some(range) = desc.voltage_scale_non_hsi {
        if v.sys != SysSource::Hsi {
            b.push_str(&format!(
                "        config.rcc.voltage_scale = rcc::VoltageScale::{range};\n"
            ));
        }
    }

    // ── Prescalers ──────────────────────────────────────────────────────
    b.push_str(&format!(
        "        config.rcc.ahb_pre = rcc::AHBPrescaler::DIV{};\n",
        v.ahb
    ));
    for (field, div) in &v.apb {
        b.push_str(&format!(
            "        config.rcc.{field} = rcc::APBPrescaler::DIV{div};\n"
        ));
    }
    b.push_str("    }\n    let p = embassy_stm32::init(config);\n");
    b
}

/// `apb1_pre` → `APB1` for the header comment.
fn apb_human(field: &str) -> String {
    field.trim_end_matches("_pre").to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f4_100mhz() -> RccValues {
        // The shipped F4 default: HSI /8 ×100 /2 → 100 MHz, APB1 /2.
        RccValues {
            sys: SysSource::Pll,
            hse_on: false,
            hse_hz: 8_000_000,
            pll_src_hse: false,
            pll_m: 8,
            pll_n: 100,
            pll_out: 2,
            ahb: 1,
            sysclk_hz: 100_000_000,
            apb: vec![("apb1_pre", 2), ("apb2_pre", 1)],
        }
    }

    #[test]
    fn f4_descriptor_reproduces_the_hand_written_output() {
        let s = emit_rcc_block(&RccDescriptor::f4(), &f4_100mhz());
        for needle in [
            "config.rcc.pll_src = rcc::PllSource::HSI;",
            "prediv: rcc::PllPreDiv::DIV8,",
            "mul: rcc::PllMul::MUL100,",
            "divp: Some(rcc::PllPDiv::DIV2),",
            "divq: None,",
            "divr: None,",
            "config.rcc.sys = rcc::Sysclk::PLL1_P;",
            "config.rcc.apb1_pre = rcc::APBPrescaler::DIV2;",
            "config.rcc.apb2_pre = rcc::APBPrescaler::DIV1;",
            "let p = embassy_stm32::init(config);",
            "SYSCLK 100 MHz (HSI16 /8 x100 /2 via PLLP)",
            "AHB /1 · APB1 /2 APB2 /1",
        ] {
            assert!(s.contains(needle), "missing: {needle}\n\n{s}");
        }
        // F4 has no voltage scale and no frac line.
        assert!(!s.contains("voltage_scale"));
        assert!(!s.contains("frac:"));
        assert!(!s.contains("config.rcc.hse"), "HSI preset needs no HSE");
    }

    #[test]
    fn wba_descriptor_reproduces_the_hand_written_output() {
        let v = RccValues {
            sys: SysSource::Pll,
            hse_on: true,
            hse_hz: 32_000_000,
            pll_src_hse: true,
            pll_m: 2,
            pll_n: 25,
            pll_out: 4,
            ahb: 1,
            sysclk_hz: 100_000_000,
            apb: vec![("apb1_pre", 1), ("apb2_pre", 1), ("apb7_pre", 1)],
        };
        let s = emit_rcc_block(&RccDescriptor::wba(), &v);
        for needle in [
            "config.rcc.hse = Some(rcc::Hse { prescaler: rcc::HsePrescaler::DIV1 });",
            "source: rcc::PllSource::HSE,",
            "prediv: rcc::PllPreDiv::DIV2,",
            "mul: rcc::PllMul::MUL25,",
            "divr: Some(rcc::PllDiv::DIV4),",
            "frac: None,",
            "config.rcc.sys = rcc::Sysclk::PLL1_R;",
            "config.rcc.voltage_scale = rcc::VoltageScale::RANGE1;",
            "config.rcc.apb7_pre = rcc::APBPrescaler::DIV1;",
            "SYSCLK 100 MHz (HSE32 /2 x25 /4 via PLL1R)",
            "AHB /1 · APB1 /1 APB2 /1 APB7 /1",
        ] {
            assert!(s.contains(needle), "missing: {needle}\n\n{s}");
        }
        // WBA nests the source and has no separate pll_src line.
        assert!(!s.contains("config.rcc.pll_src ="));
    }

    #[test]
    fn generic_reader_matches_the_shipped_f4_and_wba_presets() {
        use crate::panels::mcu_module::clock::graph::{stm32f4_graph, stm32wba_graph};
        // The shipped F4 default = HSI /8 ×100 /2 → 100 MHz SYSCLK, APB1 /2.
        let f4 = read_rcc_values(&stm32f4_graph(), &ReadSpec::f4());
        assert_eq!(f4.sys, SysSource::Pll);
        assert_eq!(f4.sysclk_hz, 100_000_000);
        assert_eq!((f4.pll_m, f4.pll_n, f4.pll_out), (8, 100, 2));
        assert!(!f4.pll_src_hse);
        assert_eq!(f4.apb, vec![("apb1_pre", 2), ("apb2_pre", 1)]);

        // The shipped WBA default = HSE32 /2 ×25 /4 → 100 MHz, all buses /1.
        let wba = read_rcc_values(&stm32wba_graph(), &ReadSpec::wba());
        assert_eq!(wba.sys, SysSource::Pll);
        assert_eq!(wba.sysclk_hz, 100_000_000);
        assert_eq!((wba.pll_m, wba.pll_n, wba.pll_out), (2, 25, 4));
        assert!(wba.pll_src_hse);
        assert_eq!(wba.hse_hz, 32_000_000);
        assert_eq!(wba.apb.len(), 3);
    }

    #[test]
    fn a_fully_reset_graph_reads_back_as_the_reset_baseline() {
        // HSI sysclk + EVERY prescaler /1 must equal `spec.reset` so codegen
        // emits `init(Default::default())`. The shipped 100 MHz preset ships
        // APB1 /2, so resetting `sw` alone is (correctly) NOT the reset state —
        // every divider has to go back to /1 (index 0).
        use crate::panels::mcu_module::clock::graph::{stm32f4_graph, NodeState};
        let mut g = stm32f4_graph();
        g.node_mut("sw").unwrap().state = NodeState::Index(0); // HSI
        g.node_mut("hse").unwrap().state =
            NodeState::Source { enabled: false, hz: 8_000_000 };
        for div in ["ahb", "apb1", "apb2"] {
            g.node_mut(div).unwrap().state = NodeState::Index(0); // /1
        }
        let spec = ReadSpec::f4();
        assert_eq!(read_rcc_values(&g, &spec), spec.reset);
    }

    #[test]
    fn family_dispatch_selects_the_recipe_without_topology_sniffing() {
        use crate::panels::mcu_module::clock::graph::{stm32f4_graph, stm32wba_graph, GraphClock};

        // F4 family + the shipped F4 graph → the F4 100 MHz RCC block.
        let f4 = GraphClock { graph: stm32f4_graph(), layout: Default::default() };
        let s = graph_clock_block("stm32f4", &ClockConfig::Graph(f4));
        assert!(s.contains("config.rcc.sys = rcc::Sysclk::PLL1_P;"), "{s}");
        assert!(s.contains("SYSCLK 100 MHz (HSI16 /8 x100 /2 via PLLP)"), "{s}");

        // An F4 HSE-PLL config emits the HSE oscillator block + HSE source
        // (was `f4::hse_pll_emits_hse_block_and_source`, now family-dispatched).
        use crate::panels::mcu_module::clock::graph::NodeState;
        let mut hse = stm32f4_graph();
        hse.node_mut("hse").unwrap().state = NodeState::Source { enabled: true, hz: 25_000_000 };
        hse.node_mut("pllsrc").unwrap().state = NodeState::Index(1); // HSE
        hse.node_mut("pllm").unwrap().state = NodeState::Index(5); // /25
        hse.node_mut("plln").unwrap().state = NodeState::Value(160);
        let s = graph_clock_block("stm32f4", &ClockConfig::Graph(GraphClock { graph: hse, layout: Default::default() }));
        assert!(s.contains("config.rcc.hse = Some(rcc::Hse { freq: embassy_stm32::time::Hertz(25000000), mode: rcc::HseMode::Oscillator });"), "{s}");
        assert!(s.contains("config.rcc.pll_src = rcc::PllSource::HSE;"), "{s}");
        assert!(s.contains("mul: rcc::PllMul::MUL160,"), "{s}");

        // WBA family + the shipped WBA graph → the WBA 100 MHz RCC block.
        let wba = GraphClock { graph: stm32wba_graph(), layout: Default::default() };
        let s = graph_clock_block("stm32wba", &ClockConfig::Graph(wba));
        assert!(s.contains("config.rcc.sys = rcc::Sysclk::PLL1_R;"), "{s}");
        assert!(s.contains("VoltageScale::RANGE1"), "{s}");

        // A family with no recipe (e.g. h7 until one lands) → embassy reset init,
        // regardless of what graph it carries.
        let g = GraphClock { graph: stm32f4_graph(), layout: Default::default() };
        assert!(graph_clock_block("stm32h7", &ClockConfig::Graph(g))
            .contains("embassy_stm32::init(Default::default())"));

        // Non-graph clock → reset init.
        assert!(graph_clock_block("stm32f4", &ClockConfig::None)
            .contains("embassy_stm32::init(Default::default())"));
    }

    #[test]
    fn g4_recipe_reads_the_preset_and_emits_valid_rcc() {
        use crate::panels::mcu_module::clock::graph::{stm32g4_graph, GraphClock};

        // The shipped G4 default = HSI16 /4 ×75 /2 → 150 MHz, all buses /1.
        let v = read_rcc_values(&stm32g4_graph(), &ReadSpec::g4());
        assert_eq!(v.sys, SysSource::Pll);
        assert_eq!(v.sysclk_hz, 150_000_000);
        assert_eq!((v.pll_m, v.pll_n, v.pll_out), (4, 75, 2));
        assert!(!v.pll_src_hse);
        assert_eq!(v.apb, vec![("apb1_pre", 1), ("apb2_pre", 1)]);

        // Emitted block: G4 nests the source, uses divr / PLL1_R, no frac, no
        // voltage_scale, and a real init(config).
        let s = graph_clock_block("stm32g4", &ClockConfig::Graph(GraphClock {
            graph: stm32g4_graph(),
            layout: Default::default(),
        }));
        for needle in [
            "source: rcc::PllSource::HSI,",
            "prediv: rcc::PllPreDiv::DIV4,",
            "mul: rcc::PllMul::MUL75,",
            "divr: Some(rcc::PllRDiv::DIV2),",
            "config.rcc.sys = rcc::Sysclk::PLL1_R;",
            "config.rcc.apb1_pre = rcc::APBPrescaler::DIV1;",
            "let p = embassy_stm32::init(config);",
            "SYSCLK 150 MHz (HSI16 /4 x75 /2 via PLLR)",
        ] {
            assert!(s.contains(needle), "missing: {needle}\n\n{s}");
        }
        // G4 has no separate pll_src line (nested), no frac, no voltage scale.
        assert!(!s.contains("config.rcc.pll_src ="));
        assert!(!s.contains("frac:"));
        assert!(!s.contains("voltage_scale"));
    }

    #[test]
    fn f2_and_f7_share_the_f4_recipe_byte_for_byte() {
        use crate::panels::mcu_module::clock::graph::{stm32f4_graph, GraphClock};
        let gc = || ClockConfig::Graph(GraphClock { graph: stm32f4_graph(), layout: Default::default() });
        // Same embassy rcc module (f247.rs) → identical emitted RCC block.
        let f4 = graph_clock_block("stm32f4", &gc());
        assert_eq!(graph_clock_block("stm32f7", &gc()), f4);
        assert_eq!(graph_clock_block("stm32f2", &gc()), f4);
        assert!(f4.contains("config.rcc.sys = rcc::Sysclk::PLL1_P;"));
    }

    #[test]
    fn l4_recipe_emits_hsi_enable_and_never_the_msi_reset_default() {
        use crate::panels::mcu_module::clock::graph::{stm32l4_graph, GraphClock, NodeState};

        // Shipped 80 MHz preset: HSI16 /1 ×10 /2 → 80 MHz.
        let s = graph_clock_block("stm32l4", &ClockConfig::Graph(GraphClock {
            graph: stm32l4_graph(),
            layout: Default::default(),
        }));
        for needle in [
            "config.rcc.hsi = true;", // L4 boots with HSI off — must switch it on
            "source: rcc::PllSource::HSI,",
            "mul: rcc::PllMul::MUL10,",
            "divr: Some(rcc::PllRDiv::DIV2),",
            "config.rcc.sys = rcc::Sysclk::PLL1_R;",
            "SYSCLK 80 MHz (HSI16 /1 x10 /2 via PLLR)",
        ] {
            assert!(s.contains(needle), "missing: {needle}\n\n{s}");
        }

        // A reset-equivalent L4 graph (HSI sysclk, all /1) must STILL emit an
        // explicit HSI block — NOT init(Default::default()), which on L4 is MSI.
        let mut g = stm32l4_graph();
        g.node_mut("sw").unwrap().state = NodeState::Index(0); // HSI direct
        let s2 = graph_clock_block("stm32l4", &ClockConfig::Graph(GraphClock { graph: g, layout: Default::default() }));
        assert!(s2.contains("config.rcc.hsi = true;"), "{s2}");
        assert!(s2.contains("config.rcc.sys = rcc::Sysclk::HSI;"), "{s2}");
        assert!(!s2.contains("init(Default::default())"), "L4 reset must be explicit, not MSI default\n\n{s2}");
    }

    #[test]
    fn g0_recipe_reads_the_preset_and_emits_a_single_apb_bus() {
        use crate::panels::mcu_module::clock::graph::{stm32g0_graph, GraphClock};

        // The shipped G0 default = HSI16 /1 ×8 /2 → 64 MHz, single APB /1.
        let v = read_rcc_values(&stm32g0_graph(), &ReadSpec::g0());
        assert_eq!(v.sys, SysSource::Pll);
        assert_eq!(v.sysclk_hz, 64_000_000);
        assert_eq!((v.pll_m, v.pll_n, v.pll_out), (1, 8, 2));
        assert_eq!(v.apb, vec![("apb1_pre", 1)]); // ONE bus — no apb2

        let s = graph_clock_block("stm32g0", &ClockConfig::Graph(GraphClock {
            graph: stm32g0_graph(),
            layout: Default::default(),
        }));
        for needle in [
            "source: rcc::PllSource::HSI,",
            "mul: rcc::PllMul::MUL8,",
            "divr: Some(rcc::PllRDiv::DIV2),",
            "config.rcc.sys = rcc::Sysclk::PLL1_R;",
            "config.rcc.apb1_pre = rcc::APBPrescaler::DIV1;",
            "SYSCLK 64 MHz (HSI16 /1 x8 /2 via PLLR)",
        ] {
            assert!(s.contains(needle), "missing: {needle}\n\n{s}");
        }
        // G0 has no APB2 bus.
        assert!(!s.contains("apb2_pre"), "G0 must not emit apb2_pre\n\n{s}");
    }

    #[test]
    fn hse_direct_sysclk_emits_no_pll_but_keeps_voltage_scale() {
        let v = RccValues {
            sys: SysSource::Hse,
            hse_on: true,
            hse_hz: 32_000_000,
            pll_src_hse: false,
            pll_m: 2,
            pll_n: 25,
            pll_out: 4,
            ahb: 1,
            sysclk_hz: 32_000_000,
            apb: vec![("apb1_pre", 1), ("apb2_pre", 1), ("apb7_pre", 1)],
        };
        let s = emit_rcc_block(&RccDescriptor::wba(), &v);
        assert!(s.contains("config.rcc.sys = rcc::Sysclk::HSE;"));
        assert!(!s.contains("config.rcc.pll1"));
        assert!(s.contains("VoltageScale::RANGE1"));
    }
}
