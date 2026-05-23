use super::mcu::Mcu;
use super::pin_module::pin::Pin;
use super::pin_module::pin_function::PinFunction;
use std::collections::{BTreeMap, BTreeSet};

// ── Public entry point ────────────────────────────────────────────────────────

impl Mcu {
    /// Generates a complete `src/main.rs` for `stm32f1xx-hal` from the
    /// current pin configuration. Called each frame by `AppIde`.
    pub fn generate_code(&self) -> String {
        let all: Vec<&Pin> = self
            .top_pins
            .iter()
            .chain(self.bottom_pins.iter())
            .chain(self.left_pins.iter())
            .chain(self.right_pins.iter())
            .collect();
        generate_hal_code(&self.name, &all)
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

struct PinMeta {
    port: char,
    pin_num: u8,
    var: String,       // "pa0", "pb15"
    port_var: String,  // "gpioa", "gpiob"
    crx: &'static str, // "crl" (0-7) | "crh" (8-15)
}

fn parse_pin(name: &str) -> Option<PinMeta> {
    let bytes = name.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'P' {
        return None;
    }
    let port = bytes[1] as char;
    if !port.is_ascii_uppercase() {
        return None;
    }
    let pin_num: u8 = name[2..].parse().ok()?;
    let lc = port.to_ascii_lowercase();
    Some(PinMeta {
        port,
        pin_num,
        var: format!("p{}{}", lc, pin_num),
        port_var: format!("gpio{}", lc),
        crx: if pin_num < 8 { "crl" } else { "crh" },
    })
}

/// Returns the `.into_xxx(&mut port.crx)` expression for a pin.
fn into_expr(func: &PinFunction, pv: &str, crx: &str) -> String {
    match func {
        PinFunction::GpioInput => format!("into_floating_input(&mut {pv}.{crx})"),
        PinFunction::GpioOutput => format!("into_push_pull_output(&mut {pv}.{crx})"),
        PinFunction::AdcChannel { .. } => format!("into_analog(&mut {pv}.{crx})"),
        PinFunction::TimerPwm { .. } => format!("into_alternate_push_pull(&mut {pv}.{crx})"),
        PinFunction::UsartTx(_) | PinFunction::UsartCk(_) => {
            format!("into_alternate_push_pull(&mut {pv}.{crx})")
        }
        PinFunction::UsartRx(_) | PinFunction::UsartCts(_) => {
            format!("into_floating_input(&mut {pv}.{crx})")
        }
        PinFunction::UsartRts(_) => format!("into_push_pull_output(&mut {pv}.{crx})"),
        PinFunction::SpiSck(_) | PinFunction::SpiMosi(_) => {
            format!("into_alternate_push_pull(&mut {pv}.{crx})")
        }
        PinFunction::SpiNss(_) => format!("into_push_pull_output(&mut {pv}.{crx})"),
        PinFunction::SpiMiso(_) => format!("into_floating_input(&mut {pv}.{crx})"),
        PinFunction::I2cScl(_) | PinFunction::I2cSda(_) => {
            format!("into_alternate_open_drain(&mut {pv}.{crx})")
        }
        PinFunction::Mco => format!("into_alternate_push_pull(&mut {pv}.{crx})"),
        PinFunction::CanTx => format!("into_alternate_push_pull(&mut {pv}.{crx})"),
        PinFunction::CanRx => format!("into_floating_input(&mut {pv}.{crx})"),
        PinFunction::UsbDm | PinFunction::UsbDp => {
            "// USB — configured automatically by the USB peripheral".to_owned()
        }
        PinFunction::SwdIo | PinFunction::SwdClk => {
            "// SWD — active by default, no config needed".to_owned()
        }
        PinFunction::Unset => unreachable!(),
    }
}

fn is_comment_expr(expr: &str) -> bool {
    expr.trim_start().starts_with("//")
}

// ── Code generator ────────────────────────────────────────────────────────────

fn generate_hal_code(mcu_name: &str, all_pins: &[&Pin]) -> String {
    let configured: Vec<(&Pin, PinMeta)> = all_pins
        .iter()
        .filter(|p| !p.reserved && p.selected_function != PinFunction::Unset)
        .filter_map(|p| parse_pin(&p.name).map(|m| (*p, m)))
        .collect();

    if configured.is_empty() {
        return default_template(mcu_name);
    }

    // ── Ports used ───────────────────────────────────────────────────────────
    let mut ports_used: BTreeSet<char> = BTreeSet::new();
    for (_, meta) in &configured {
        ports_used.insert(meta.port);
    }

    // ── HAL imports ──────────────────────────────────────────────────────────
    let mut use_items: Vec<&str> = vec!["pac", "prelude::*"];

    let has_serial = configured.iter().any(|(p, _)| {
        matches!(
            p.selected_function,
            PinFunction::UsartTx(_) | PinFunction::UsartRx(_)
        )
    });
    let has_spi = configured.iter().any(|(p, _)| {
        matches!(
            p.selected_function,
            PinFunction::SpiSck(_) | PinFunction::SpiMosi(_)
        )
    });
    let has_i2c = configured.iter().any(|(p, _)| {
        matches!(
            p.selected_function,
            PinFunction::I2cScl(_) | PinFunction::I2cSda(_)
        )
    });
    let has_adc = configured
        .iter()
        .any(|(p, _)| matches!(p.selected_function, PinFunction::AdcChannel { .. }));
    let has_timer = configured
        .iter()
        .any(|(p, _)| matches!(p.selected_function, PinFunction::TimerPwm { .. }));
    let needs_afio = has_serial || has_spi || has_i2c || has_timer;

    if has_serial {
        use_items.push("serial::{Config, Serial}");
    }
    if has_spi {
        use_items.push("spi::{Mode, Phase, Polarity, Spi}");
    }
    if has_i2c {
        use_items.push("i2c::{BlockingI2c, Mode as I2cMode}");
    }
    if has_adc {
        use_items.push("adc");
    }
    if has_timer {
        use_items.push("timer::Timer");
    }

    // deduplicate while preserving order
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let use_items: Vec<&str> = use_items.into_iter().filter(|s| seen.insert(s)).collect();

    let use_block = use_items
        .iter()
        .map(|s| format!("    {s},"))
        .collect::<Vec<_>>()
        .join("\n");

    // ── Pin declaration lines grouped by port ────────────────────────────────
    let mut port_groups: BTreeMap<char, Vec<String>> = BTreeMap::new();

    for (pin, meta) in &configured {
        let expr = into_expr(&pin.selected_function, &meta.port_var, meta.crx);
        let comment = pin.selected_function.label();
        let line = if is_comment_expr(&expr) {
            format!(
                "    // {name}: {expr_inner}",
                name = pin.name,
                expr_inner = expr.trim_start_matches("//").trim()
            )
        } else {
            format!(
                "    let {var} = {pv}.{var}.{expr}; // {comment}",
                var = meta.var,
                pv = meta.port_var,
                expr = expr,
                comment = comment
            )
        };
        port_groups.entry(meta.port).or_default().push(line);
    }

    let mut pin_section = String::new();
    for (port, lines) in &port_groups {
        pin_section.push_str(&format!("    // ── Port {port} ──\n"));
        for l in lines {
            pin_section.push_str(l);
            pin_section.push('\n');
        }
        pin_section.push('\n');
    }

    // ── Peripheral blocks ────────────────────────────────────────────────────
    let mut periph_section = String::new();

    // ADC
    if has_adc {
        periph_section.push_str("    // ── ADC ──\n");
        periph_section.push_str("    let mut adc1 = adc::Adc::adc1(dp.ADC1, clocks);\n");
        for (_, meta) in configured
            .iter()
            .filter(|(p, _)| matches!(p.selected_function, PinFunction::AdcChannel { .. }))
        {
            periph_section.push_str(&format!(
                "    // Example read: let val: u16 = adc1.read(&mut {var}).unwrap();\n",
                var = meta.var
            ));
        }
        periph_section.push('\n');
    }

    // Timers
    if has_timer {
        periph_section.push_str("    // ── Timers (PWM) ──\n");
        periph_section.push_str("    // Configure timers as needed, e.g.:\n");
        periph_section.push_str(
            "    // let pwm = Timer::tim2(dp.TIM2, &clocks).pwm_hz::<Tim2NoRemap, _, _>(\n",
        );
        periph_section.push_str("    //     (pa0, pa1), &mut afio.mapr, 1.kHz());\n\n");
    }

    // USART instances
    for n in 1u8..=3 {
        let tx = configured
            .iter()
            .find(|(p, _)| p.selected_function == PinFunction::UsartTx(n));
        let rx = configured
            .iter()
            .find(|(p, _)| p.selected_function == PinFunction::UsartRx(n));
        if tx.is_none() && rx.is_none() {
            continue;
        }

        let tx_var = tx
            .map(|(_, m)| m.var.clone())
            .unwrap_or_else(|| format!("_tx{n}"));
        let rx_var = rx
            .map(|(_, m)| m.var.clone())
            .unwrap_or_else(|| format!("_rx{n}"));

        periph_section.push_str(&format!("    // ── USART{n} ──\n"));
        periph_section.push_str(&format!(
            r#"    let serial{n} = Serial::new(
        dp.USART{n},
        ({tx_var}, {rx_var}),
        &mut afio.mapr,
        Config::default().baudrate(115_200.bps()),
        &clocks,
    );
    let (mut tx{n}, mut rx{n}) = serial{n}.split();

"#
        ));
    }

    // SPI instances
    for n in 1u8..=2 {
        let sck = configured
            .iter()
            .find(|(p, _)| p.selected_function == PinFunction::SpiSck(n));
        let miso = configured
            .iter()
            .find(|(p, _)| p.selected_function == PinFunction::SpiMiso(n));
        let mosi = configured
            .iter()
            .find(|(p, _)| p.selected_function == PinFunction::SpiMosi(n));
        if sck.is_none() && mosi.is_none() {
            continue;
        }

        let sck_v = sck
            .map(|(_, m)| m.var.clone())
            .unwrap_or_else(|| format!("_sck{n}"));
        let miso_v = miso
            .map(|(_, m)| m.var.clone())
            .unwrap_or_else(|| format!("_miso{n}"));
        let mosi_v = mosi
            .map(|(_, m)| m.var.clone())
            .unwrap_or_else(|| format!("_mosi{n}"));

        periph_section.push_str(&format!("    // ── SPI{n} ──\n"));
        periph_section.push_str(&format!(
            r#"    let spi{n} = Spi::spi{n}(
        dp.SPI{n},
        ({sck_v}, {miso_v}, {mosi_v}),
        &mut afio.mapr,
        Mode {{
            polarity: Polarity::IdleLow,
            phase: Phase::CaptureOnFirstTransition,
        }},
        1.MHz(),
        &clocks,
    );

"#
        ));
    }

    // I2C instances
    for n in 1u8..=2 {
        let scl = configured
            .iter()
            .find(|(p, _)| p.selected_function == PinFunction::I2cScl(n));
        let sda = configured
            .iter()
            .find(|(p, _)| p.selected_function == PinFunction::I2cSda(n));
        if scl.is_none() || sda.is_none() {
            continue;
        }

        let scl_v = scl.unwrap().1.var.clone();
        let sda_v = sda.unwrap().1.var.clone();

        periph_section.push_str(&format!("    // ── I2C{n} ──\n"));
        periph_section.push_str(&format!(
            r#"    let i2c{n} = BlockingI2c::i2c{n}(
        dp.I2C{n},
        ({scl_v}, {sda_v}),
        &mut afio.mapr,
        I2cMode::Standard {{ frequency: 100_000.Hz() }},
        &clocks,
        1000, 10, 1000, 1000,
    );

"#
        ));
    }

    // CAN
    let can_rx = configured
        .iter()
        .find(|(p, _)| p.selected_function == PinFunction::CanRx);
    let can_tx = configured
        .iter()
        .find(|(p, _)| p.selected_function == PinFunction::CanTx);
    if can_rx.is_some() || can_tx.is_some() {
        let rx_v = can_rx
            .map(|(_, m)| m.var.clone())
            .unwrap_or("_can_rx".into());
        let tx_v = can_tx
            .map(|(_, m)| m.var.clone())
            .unwrap_or("_can_tx".into());
        periph_section.push_str("    // ── CAN ──\n");
        periph_section.push_str(&format!(
            r#"    // let can = Can::new(dp.CAN1, ({rx_v}, {tx_v}));
    // Requires bxcan crate: https://docs.rs/bxcan

"#
        ));
    }

    // ── Assemble output ──────────────────────────────────────────────────────
    let port_splits = ports_used
        .iter()
        .map(|p| {
            let lc = p.to_ascii_lowercase();
            format!("    let mut gpio{lc} = dp.GPIO{p}.split();")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let afio_line = if needs_afio {
        "    let mut afio = dp.AFIO.constrain();\n"
    } else {
        ""
    };

    format!(
        "// Auto-generated by Embedded IDE\n\
         // MCU: {mcu_name}\n\
         // HAL: stm32f1xx-hal\n\n\
         #![no_std]\n\
         #![no_main]\n\n\
         use panic_halt as _;\n\
         use cortex_m_rt::entry;\n\
         use stm32f1xx_hal::{{\n\
         {use_block}\n\
         }};\n\n\
         #[entry]\n\
         fn main() -> ! {{\n\
             let dp = pac::Peripherals::take().unwrap();\n\n\
             let mut flash = dp.FLASH.constrain();\n\
             let rcc = dp.RCC.constrain();\n\
         {afio_line}\
             let clocks = rcc.cfgr\n\
                 .use_hse(8.MHz())\n\
                 .sysclk(72.MHz())\n\
                 .pclk1(36.MHz())\n\
                 .freeze(&mut flash.acr);\n\n\
         {port_splits}\n\n\
         {pin_section}\
         {periph_section}\
             loop {{}}\n\
         }}\n"
    )
}

// ── Default template ──────────────────────────────────────────────────────────

fn default_template(mcu_name: &str) -> String {
    format!(
        "// Auto-generated by Embedded IDE\n\
         // MCU: {mcu_name}\n\
         // HAL: stm32f1xx-hal\n\
         //\n\
         // Select a pin in the MCU Configurator and assign a function\n\
         // to generate code here automatically.\n\n\
         #![no_std]\n\
         #![no_main]\n\n\
         use panic_halt as _;\n\
         use cortex_m_rt::entry;\n\
         use stm32f1xx_hal::{{pac, prelude::*}};\n\n\
         #[entry]\n\
         fn main() -> ! {{\n\
             let dp = pac::Peripherals::take().unwrap();\n\n\
             let mut flash = dp.FLASH.constrain();\n\
             let rcc = dp.RCC.constrain();\n\
             let clocks = rcc.cfgr\n\
                 .use_hse(8.MHz())\n\
                 .sysclk(72.MHz())\n\
                 .pclk1(36.MHz())\n\
                 .freeze(&mut flash.acr);\n\n\
             loop {{}}\n\
         }}\n"
    )
}
