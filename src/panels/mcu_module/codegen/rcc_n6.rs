//! STM32N6 clock code generation.
//!
//! N6 gets its own emitter rather than a [`super::rcc::RccDescriptor`] because
//! it does not fit that shape at all. The descriptor model describes ONE PLL
//! with one output divider feeding SYSCLK; embassy's N6 `Config` has four PLLs,
//! twenty independent "IC" dividers, separate `cpu` and `sys` clocks, and five
//! APB prescalers. Bending the model around that would distort what serves the
//! other seven families for the sake of one.
//!
//! **This covers a SUBSET on purpose**: CPU, SYSCLK, PLL1 and the buses — the
//! part that decides whether the chip runs at the speed you asked for. The
//! other three PLLs and eighteen ICs feed peripherals, keep their reset values,
//! and are left to the hand-written block. What is emitted is complete and
//! correct; it is not everything the silicon can do.
//!
//! Every node id below was read out of the vendor's own tree, not guessed —
//! see `dump_clock_tree_nodes`, which prints them.

use super::super::clock::graph::model::{ClockGraph, NodeKind, NodeState};

/// The `Config` fields this emitter fills, read from the graph.
struct N6 {
    /// Index into `PLL1Source`: HSI / MSI / HSE / I2S_CKIN.
    pll_src: usize,
    divm: u32,
    divn: u32,
    frac: u32,
    divp1: u32,
    divp2: u32,
    ic1_div: u32,
    ic2_div: u32,
    /// Index into `SYSAClkSource` / `SYSBClkSource`: HSI / MSI / HSE / IC.
    cpu: usize,
    sys: usize,
    ahb: u32,
    apb: [u32; 4],
}

/// `Pllsel` has a fourth input the others do not, and its name is not `I2S`.
const PLL_SOURCE: [&str; 4] = ["HSI", "MSI", "HSE", "I2S_CKIN"];

fn read(g: &ClockGraph) -> Option<N6> {
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
    let value_of = |id: &str| match g.node(id).map(|n| &n.state) {
        Some(NodeState::Value(v)) => Some(*v),
        _ => None,
    };

    // The two that identify the tree. Without them this is not an N6 graph and
    // the caller must fall back rather than emit nonsense.
    let cpu = index_of("SYSAClkSource")?;
    let sys = index_of("SYSBClkSource")?;

    Some(N6 {
        pll_src: index_of("PLL1Source").unwrap_or(0),
        divm: divisor_of("FREFDIV1").unwrap_or(1),
        divn: value_of("FBDIV1").unwrap_or(25),
        // The fractional register is a plain multiplier node here; 0 is "no
        // fraction", which is what the reset value means.
        frac: value_of("PLL1FRACV").unwrap_or(0),
        divp1: divisor_of("POSTDIV1_1").unwrap_or(1),
        divp2: divisor_of("POSTDIV2_1").unwrap_or(1),
        ic1_div: divisor_of("IC1Div").unwrap_or(1),
        ic2_div: divisor_of("IC2Div").unwrap_or(1),
        cpu,
        sys,
        ahb: divisor_of("HPREDiv").unwrap_or(2),
        apb: [
            divisor_of("APB1DIV").unwrap_or(1),
            divisor_of("APB2DIV").unwrap_or(1),
            divisor_of("APB4DIV").unwrap_or(1),
            divisor_of("APB5DIV").unwrap_or(1),
        ],
    })
}

/// The `rcc` block for an N6, or `None` when this is not an N6 tree.
pub fn block(g: &ClockGraph) -> Option<String> {
    let n = read(g)?;
    let src = PLL_SOURCE.get(n.pll_src).copied().unwrap_or("HSI");
    // CPU and SYS reach a PLL only THROUGH IC1 and IC2 — `CpuClk` has no PLL
    // variant at all. So selecting the PLL for either means configuring its IC,
    // which is why those two of the twenty are in this subset.
    let cpu = ["Hsi", "Msi", "Hse", "Ic1"].get(n.cpu).copied().unwrap_or("Hsi");
    let sys = ["Hsi", "Msi", "Hse", "Ic2"].get(n.sys).copied().unwrap_or("Hsi");

    let mut s = String::new();
    s.push_str("    use embassy_stm32::rcc::*;\n");
    s.push_str("    let mut config = embassy_stm32::Config::default();\n");
    s.push_str("    config.rcc.pll1 = Some(Pll::Oscillator {\n");
    s.push_str(&format!("        source: Pllsel::{src},\n"));
    s.push_str(&format!("        divm: Plldivm::DIV{},\n", n.divm));
    s.push_str(&format!("        divn: {},\n", n.divn));
    s.push_str(&format!("        fractional: {},\n", n.frac));
    s.push_str(&format!("        divp1: Pllpdiv::DIV{},\n", n.divp1));
    s.push_str(&format!("        divp2: Pllpdiv::DIV{},\n", n.divp2));
    s.push_str("    });\n");
    // The tree draws IC1/IC2 as taps fed by all four PLLs rather than as a mux,
    // so which PLL feeds them is not a choice it records. PLL1 is the one this
    // emitter configures, and the only one it can honestly name.
    s.push_str(&format!(
        "    config.rcc.ic1 = Some(IcConfig {{ source: Icsel::PLL1, divider: Icint::DIV{} }});\n",
        n.ic1_div
    ));
    s.push_str(&format!(
        "    config.rcc.ic2 = Some(IcConfig {{ source: Icsel::PLL1, divider: Icint::DIV{} }});\n",
        n.ic2_div
    ));
    s.push_str(&format!("    config.rcc.cpu = CpuClk::{cpu};\n"));
    s.push_str(&format!("    config.rcc.sys = SysClk::{sys};\n"));
    s.push_str(&format!("    config.rcc.ahb = AhbPrescaler::DIV{};\n", n.ahb));
    for (i, field) in ["apb1", "apb2", "apb4", "apb5"].iter().enumerate() {
        s.push_str(&format!(
            "    config.rcc.{field} = ApbPrescaler::DIV{};\n",
            n.apb[i]
        ));
    }
    s.push_str("    let p = embassy_stm32::init(config);\n");
    Some(s)
}
