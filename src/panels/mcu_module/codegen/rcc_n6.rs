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
/// One PLL, read from its `FREFDIV`/`FBDIV`/`POSTDIV` triple.
#[derive(Clone, Copy, PartialEq)]
struct Pll {
    /// Index into `PLL<n>Source`: HSI / MSI / HSE / I2S_CKIN.
    src: usize,
    divm: u32,
    divn: u32,
    frac: u32,
    divp1: u32,
    divp2: u32,
}

/// One IC: which PLL feeds it, and by how much it divides.
#[derive(Clone, Copy, PartialEq)]
struct Ic {
    /// 0..=3 -> `Icsel::PLL1`..`PLL4`.
    pll: usize,
    div: u32,
}

/// The IC's reset state, and the shape of "nothing to say about it".
const IC_RESET: Ic = Ic { pll: 0, div: 1 };

struct N6 {
    pll: [Pll; 4],
    ic: [Ic; 20],
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

    // The four PLLs are the same shape, numbered. Their FBDIV ranges differ
    // (PLL1 is 10..=2500, PLL2 20..=500) but that bounds the editor, not this.
    let pll = std::array::from_fn(|i| {
        let n = i + 1;
        Pll {
            src: index_of(&format!("PLL{n}Source")).unwrap_or(0),
            divm: divisor_of(&format!("FREFDIV{n}")).unwrap_or(1),
            divn: value_of(&format!("FBDIV{n}")).unwrap_or(25),
            // A plain multiplier node here; its value IS the FRACN register.
            frac: value_of(&format!("PLL{n}FRACV")).unwrap_or(0),
            divp1: divisor_of(&format!("POSTDIV1_{n}")).unwrap_or(1),
            divp2: divisor_of(&format!("POSTDIV2_{n}")).unwrap_or(1),
        }
    });
    // Which PLL feeds each IC is a real selection, readable only since `xbar`
    // stopped being treated as a pass-through tap. Before that all twenty came
    // out fed by four PLLs at once, and this emitter had to assume PLL1.
    let ic = std::array::from_fn(|i| {
        let n = i + 1;
        Ic {
            pll: index_of(&format!("IC{n}")).unwrap_or(0),
            div: divisor_of(&format!("IC{n}Div")).unwrap_or(1),
        }
    });

    Some(N6 {
        pll,
        ic,
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
    // CPU and SYS reach a PLL only THROUGH IC1 and IC2 — `CpuClk` has no PLL
    // variant at all — so those two are always emitted, whatever they are set
    // to. Of the other eighteen, only the ones that have been moved off their
    // reset say anything: an N6 has twenty IC selectors, and printing all of
    // them at their defaults would bury the handful that were chosen.
    let mut ics: Vec<usize> = (0..20).filter(|&i| i < 2 || n.ic[i] != IC_RESET).collect();
    ics.sort_unstable();
    // A PLL is emitted when something CONSUMES it. Configuring PLL2..PLL4 with
    // nothing routed to them would be three blocks of dead configuration in
    // every project — the silicon would run them for no reason.
    let mut plls: Vec<usize> = ics.iter().map(|&i| n.ic[i].pll).collect();
    plls.push(0);
    plls.sort_unstable();
    plls.dedup();

    let mut s = String::new();
    s.push_str(
        "    use embassy_stm32::rcc::*;
",
    );
    s.push_str(
        "    let mut config = embassy_stm32::Config::default();
",
    );
    for &i in &plls {
        let p = n.pll[i];
        let src = PLL_SOURCE.get(p.src).copied().unwrap_or("HSI");
        s.push_str(&format!(
            "    config.rcc.pll{} = Some(Pll::Oscillator {{
",
            i + 1
        ));
        s.push_str(&format!(
            "        source: Pllsel::{src},
"
        ));
        s.push_str(&format!(
            "        divm: Plldivm::DIV{},
",
            p.divm
        ));
        s.push_str(&format!(
            "        divn: {},
",
            p.divn
        ));
        s.push_str(&format!(
            "        fractional: {},
",
            p.frac
        ));
        s.push_str(&format!(
            "        divp1: Pllpdiv::DIV{},
",
            p.divp1
        ));
        s.push_str(&format!(
            "        divp2: Pllpdiv::DIV{},
",
            p.divp2
        ));
        s.push_str(
            "    });
",
        );
    }
    for &i in &ics {
        let c = n.ic[i];
        s.push_str(&format!(
            "    config.rcc.ic{} = Some(IcConfig {{ source: Icsel::PLL{}, divider: Icint::DIV{} }});
",
            i + 1,
            c.pll + 1,
            c.div
        ));
    }
    let cpu = ["Hsi", "Msi", "Hse", "Ic1"]
        .get(n.cpu)
        .copied()
        .unwrap_or("Hsi");
    let sys = ["Hsi", "Msi", "Hse", "Ic2"]
        .get(n.sys)
        .copied()
        .unwrap_or("Hsi");
    s.push_str(&format!(
        "    config.rcc.cpu = CpuClk::{cpu};
"
    ));
    s.push_str(&format!(
        "    config.rcc.sys = SysClk::{sys};
"
    ));
    s.push_str(&format!(
        "    config.rcc.ahb = AhbPrescaler::DIV{};
",
        n.ahb
    ));
    for (i, field) in ["apb1", "apb2", "apb4", "apb5"].iter().enumerate() {
        s.push_str(&format!(
            "    config.rcc.{field} = ApbPrescaler::DIV{};
",
            n.apb[i]
        ));
    }
    s.push_str(
        "    let p = embassy_stm32::init(config);
",
    );
    Some(s)
}
