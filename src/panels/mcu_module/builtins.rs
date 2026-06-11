//! Built-in MCU definitions, bundled into the binary via `include_str!`.
//!
//! These RON files (`assets/mcus/*.ron`) are the single source of truth for the
//! shipped chips.  Regenerate them from the factories with:
//!   `cargo test generate_builtin_ron -- --ignored`
//!
//! Later phases add a runtime folder scan for user-imported definitions.

use super::mcu_def::McuDefinition;

const STM32F103C8T6_RON: &str = include_str!("../../../assets/mcus/stm32f103c8t6.ron");
const ESP32C3_RON: &str = include_str!("../../../assets/mcus/esp32c3.ron");

/// Raw `(id, ron-text)` for every bundled chip.
const BUILTINS: &[(&str, &str)] = &[
    ("stm32f103c8t6", STM32F103C8T6_RON),
    ("esp32c3", ESP32C3_RON),
];

/// Parse all bundled built-in MCU definitions (bad files are skipped + logged).
pub fn builtin_definitions() -> Vec<McuDefinition> {
    BUILTINS
        .iter()
        .filter_map(|(id, ron)| match ron::from_str::<McuDefinition>(ron) {
            Ok(d) => Some(d),
            Err(e) => {
                eprintln!("⚠ failed to parse built-in MCU '{id}': {e}");
                None
            }
        })
        .collect()
}

/// Find a built-in definition by its `id` (e.g. "stm32f103c8t6").
pub fn builtin_for(id: &str) -> Option<McuDefinition> {
    BUILTINS
        .iter()
        .find(|(bid, _)| *bid == id)
        .and_then(|(_, ron)| ron::from_str::<McuDefinition>(ron).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::clock::ClockConfig;
    use crate::panels::mcu_module::mcu::Mcu;
    use crate::panels::mcu_module::mcu_def::{ClockDef, McuDefinition, PinDef, PinLayout, ProjectDef};
    use crate::panels::mcu_module::mock_esp32c3::create_esp32c3;
    use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;

    fn layout(m: &Mcu) -> PinLayout {
        PinLayout {
            top: m.top_pins.iter().map(PinDef::from_pin).collect(),
            bottom: m.bottom_pins.iter().map(PinDef::from_pin).collect(),
            left: m.left_pins.iter().map(PinDef::from_pin).collect(),
            right: m.right_pins.iter().map(PinDef::from_pin).collect(),
        }
    }

    fn def_of(id: &str, family: &str, package: &str, cpu: &str, m: &Mcu, project: ProjectDef) -> McuDefinition {
        let clock = match &m.clock {
            ClockConfig::Stm32f1(c) => ClockDef::Stm32f1(c.clone()),
            ClockConfig::Graph(g) => ClockDef::Graph(g.clone()),
            ClockConfig::None => ClockDef::None,
        };
        McuDefinition {
            id: id.into(),
            display_name: m.name.clone(),
            family: family.into(),
            package: package.into(),
            cpu: cpu.into(),
            toolchain: m.toolchain.clone(),
            project,
            pins: layout(m),
            clock,
            clock_limits: m.clock_limits,
            clock_presets: Vec::new(),
        }
    }

    /// One-shot: regenerate `assets/mcus/*.ron` from the factories.
    /// Run with:  `cargo test regenerate_builtin_ron -- --ignored`
    /// Project params are authored here (the single source for them).
    #[test]
    #[ignore]
    fn regenerate_builtin_ron() {
        use ron::ser::PrettyConfig;
        std::fs::create_dir_all("assets/mcus").unwrap();
        let pretty = PrettyConfig::default().struct_names(true);

        let stm = def_of(
            "stm32f103c8t6", "stm32f1", "LQFP48", "ARM Cortex-M3",
            &create_stm32f103c8tx(),
            ProjectDef {
                pkg_name: "stm32f103c8t6".into(),
                target: "thumbv7m-none-eabi".into(),
                flash_origin: "0x08000000".into(),
                flash_size: "64K".into(),
                ram_origin: "0x20000000".into(),
                ram_size: "20K".into(),
                hal_dep: r#"stm32f1xx-hal = { version = "0.10", features = ["stm32f103", "medium", "rt"] }"#.into(),
                probe_chip: "STM32F103C8".into(),
                memory_comment: "STM32F103C8T6  —  64 KiB Flash / 20 KiB RAM".into(),
            },
        );
        let esp = def_of(
            "esp32c3", "esp32c3", "QFN32", "RISC-V 32-bit",
            &create_esp32c3(),
            ProjectDef {
                pkg_name: "esp32c3".into(),
                target: "riscv32imc-unknown-none-elf".into(),
                flash_origin: String::new(),
                flash_size: String::new(),
                ram_origin: String::new(),
                ram_size: String::new(),
                hal_dep: r#"esp-hal = { version = "0.23", features = ["esp32c3"] }"#.into(),
                probe_chip: "esp32c3".into(),
                memory_comment: String::new(),
            },
        );

        for def in [&stm, &esp] {
            let ron = ron::ser::to_string_pretty(def, pretty.clone()).unwrap();
            std::fs::write(format!("assets/mcus/{}.ron", def.id), ron).unwrap();
        }
    }

    /// One-shot: (re)generate the **example** importable definition shipped in
    /// `assets/mcus/examples/stm32f103rb.ron` — a real STM32F103RBT6 in the
    /// LQFP64 package (64 pins). This is *not* a built-in; it demonstrates the
    /// runtime "Import MCU…" flow for a new chip inside the supported STM32F1
    /// family. Run with: `cargo test generate_stm32f103rb_example -- --ignored`
    #[test]
    #[ignore]
    fn generate_stm32f103rb_example() {
        use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
        use crate::panels::mcu_module::pins::logic::pin::Pin;
        use crate::panels::mcu_module::pins::logic::pin_function::PinFunction as F;
        use ron::ser::PrettyConfig;

        // STM32F103RBT6 LQFP64 pinout (datasheet DS5319). Functions mirror the
        // STM32F103xB die used by the built-in C8T6, plus the pins LQFP64 adds
        // (full PORTC + PD0..PD2).

        // ── LEFT — pins 1..16 (top→bottom) ──────────────────────────
        let left = vec![
            Pin::new_reserved(1, "VBAT"),
            Pin::new(2, "PC13"),
            Pin::new(3, "PC14"),
            Pin::new(4, "PC15"),
            Pin::new(5, "PD0"),
            Pin::new(6, "PD1"),
            Pin::new_reserved(7, "NRST"),
            Pin::new_with_analog(8, "PC0", 1, 10),
            Pin::new_with_analog(9, "PC1", 1, 11),
            Pin::new_with_analog(10, "PC2", 1, 12),
            Pin::new_with_analog(11, "PC3", 1, 13),
            Pin::new_reserved(12, "VSSA"),
            Pin::new_reserved(13, "VDDA"),
            Pin::new_with_analog(14, "PA0", 1, 0)
                .with_functions(vec![F::UsartCts(2), F::TimerPwm { timer: 2, channel: 1 }]),
            Pin::new_with_analog(15, "PA1", 1, 1)
                .with_functions(vec![F::UsartRts(2), F::TimerPwm { timer: 2, channel: 2 }]),
            Pin::new_with_analog(16, "PA2", 1, 2)
                .with_functions(vec![F::UsartTx(2), F::TimerPwm { timer: 2, channel: 3 }]),
        ];

        // ── BOTTOM — pins 17..32 (left→right) ───────────────────────
        let bottom = vec![
            Pin::new_with_analog(17, "PA3", 1, 3)
                .with_functions(vec![F::UsartRx(2), F::TimerPwm { timer: 2, channel: 4 }]),
            Pin::new_reserved(18, "VSS"),
            Pin::new_reserved(19, "VDD"),
            Pin::new_with_analog(20, "PA4", 1, 4)
                .with_functions(vec![F::SpiNss(1), F::UsartCk(2)]),
            Pin::new_with_analog(21, "PA5", 1, 5).with_functions(vec![F::SpiSck(1)]),
            Pin::new_with_analog(22, "PA6", 1, 6)
                .with_functions(vec![F::SpiMiso(1), F::TimerPwm { timer: 3, channel: 1 }]),
            Pin::new_with_analog(23, "PA7", 1, 7)
                .with_functions(vec![F::SpiMosi(1), F::TimerPwm { timer: 3, channel: 2 }]),
            Pin::new_with_analog(24, "PC4", 1, 14),
            Pin::new_with_analog(25, "PC5", 1, 15),
            Pin::new_with_analog(26, "PB0", 1, 8)
                .with_functions(vec![F::TimerPwm { timer: 3, channel: 3 }]),
            Pin::new_with_analog(27, "PB1", 1, 9)
                .with_functions(vec![F::TimerPwm { timer: 3, channel: 4 }]),
            Pin::new(28, "PB2"),
            Pin::new(29, "PB10").with_functions(vec![F::I2cScl(2), F::UsartTx(3)]),
            Pin::new(30, "PB11").with_functions(vec![F::I2cSda(2), F::UsartRx(3)]),
            Pin::new_reserved(31, "VSS"),
            Pin::new_reserved(32, "VDD"),
        ];

        // ── RIGHT — pins 48..33 (top→bottom) ────────────────────────
        let right = vec![
            Pin::new_reserved(48, "VDD"),
            Pin::new_reserved(47, "VSS"),
            Pin::new(46, "PA13").with_functions(vec![F::SwdIo]),
            Pin::new(45, "PA12").with_functions(vec![F::UsbDp, F::CanTx, F::UsartRts(1)]),
            Pin::new(44, "PA11").with_functions(vec![
                F::UsbDm,
                F::CanRx,
                F::UsartCts(1),
                F::TimerPwm { timer: 1, channel: 4 },
            ]),
            Pin::new(43, "PA10").with_functions(vec![F::UsartRx(1), F::TimerPwm { timer: 1, channel: 3 }]),
            Pin::new(42, "PA9").with_functions(vec![F::UsartTx(1), F::TimerPwm { timer: 1, channel: 2 }]),
            Pin::new(41, "PA8").with_functions(vec![
                F::Mco,
                F::UsartCk(1),
                F::TimerPwm { timer: 1, channel: 1 },
            ]),
            Pin::new(40, "PC9").with_functions(vec![F::TimerPwm { timer: 3, channel: 4 }]),
            Pin::new(39, "PC8").with_functions(vec![F::TimerPwm { timer: 3, channel: 3 }]),
            Pin::new(38, "PC7").with_functions(vec![F::TimerPwm { timer: 3, channel: 2 }]),
            Pin::new(37, "PC6").with_functions(vec![F::TimerPwm { timer: 3, channel: 1 }]),
            Pin::new(36, "PB15").with_functions(vec![F::SpiMosi(2)]),
            Pin::new(35, "PB14").with_functions(vec![F::SpiMiso(2), F::UsartRts(3)]),
            Pin::new(34, "PB13").with_functions(vec![F::SpiSck(2), F::UsartCts(3)]),
            Pin::new(33, "PB12").with_functions(vec![F::SpiNss(2), F::UsartCk(3), F::I2cScl(2)]),
        ];

        // ── TOP — pins 64..49 (left→right) ──────────────────────────
        let top = vec![
            Pin::new_reserved(64, "VDD"),
            Pin::new_reserved(63, "VSS"),
            Pin::new(62, "PB9").with_functions(vec![
                F::TimerPwm { timer: 4, channel: 4 },
                F::CanTx,
                F::I2cSda(1),
            ]),
            Pin::new(61, "PB8").with_functions(vec![
                F::TimerPwm { timer: 4, channel: 3 },
                F::CanRx,
                F::I2cScl(1),
            ]),
            Pin::new_reserved(60, "BOOT0"),
            Pin::new(59, "PB7").with_functions(vec![F::I2cSda(1), F::TimerPwm { timer: 4, channel: 2 }]),
            Pin::new(58, "PB6").with_functions(vec![F::I2cScl(1), F::TimerPwm { timer: 4, channel: 1 }]),
            Pin::new(57, "PB5").with_functions(vec![F::SpiMosi(1), F::TimerPwm { timer: 3, channel: 2 }]),
            Pin::new(56, "PB4").with_functions(vec![F::SpiMiso(1), F::TimerPwm { timer: 3, channel: 1 }]),
            Pin::new(55, "PB3").with_functions(vec![F::SpiSck(1), F::TimerPwm { timer: 2, channel: 2 }]),
            Pin::new(54, "PD2"),
            Pin::new(53, "PC12").with_functions(vec![F::UsartCk(3)]),
            Pin::new(52, "PC11").with_functions(vec![F::UsartRx(3)]),
            Pin::new(51, "PC10").with_functions(vec![F::UsartTx(3)]),
            Pin::new(50, "PA15").with_functions(vec![F::SpiNss(1), F::TimerPwm { timer: 2, channel: 1 }]),
            Pin::new(49, "PA14").with_functions(vec![F::SwdClk]),
        ];

        let m = Mcu::new(
            "STM32F103RBT6".to_owned(),
            "stm32f1".to_owned(),
            ToolchainKind::RustEmbedded,
            top,
            bottom,
            left,
            right,
        );

        let mut def = def_of(
            "stm32f103rb", "stm32f1", "LQFP64", "ARM Cortex-M3",
            &m,
            ProjectDef {
                pkg_name: "stm32f103rb".into(),
                target: "thumbv7m-none-eabi".into(),
                flash_origin: "0x08000000".into(),
                flash_size: "128K".into(),
                ram_origin: "0x20000000".into(),
                ram_size: "20K".into(),
                hal_dep: r#"stm32f1xx-hal = { version = "0.10", features = ["stm32f103", "medium", "rt"] }"#.into(),
                probe_chip: "STM32F103RB".into(),
                memory_comment: "STM32F103RBT6  —  128 KiB Flash / 20 KiB RAM (LQFP64)".into(),
            },
        );

        // Demonstrate a chip-specific preset in the importable format (when
        // `clock_presets` is non-empty it replaces the family defaults in the
        // Clock tab). 48 MHz keeps USB valid with the /1 prescaler.
        {
            use crate::panels::mcu_module::clock::model::{Stm32f1Clock, UsbPre};
            use crate::panels::mcu_module::mcu_def::ClockPresetDef;
            def.clock_presets = vec![
                ClockPresetDef {
                    name: "72 MHz (HSE 8 + PLL×9)".into(),
                    description: "Max performance. SYSCLK 72, PCLK1 36, USB 48 MHz.".into(),
                    config: ClockDef::Stm32f1(Stm32f1Clock::default()),
                },
                ClockPresetDef {
                    name: "48 MHz USB (HSE 8 + PLL×6)".into(),
                    description: "SYSCLK 48, PCLK1 24, USBCLK 48 via /1 prescaler.".into(),
                    config: ClockDef::Stm32f1(Stm32f1Clock {
                        pll_mul: 6,
                        adc_pre: 4, // 48/1/4 = 12 ≤ 14
                        usb_pre: UsbPre::Div1,
                        ..Stm32f1Clock::default()
                    }),
                },
            ];
        }

        std::fs::create_dir_all("assets/mcus/examples").unwrap();
        let ron = ron::ser::to_string_pretty(&def, PrettyConfig::default().struct_names(true)).unwrap();
        std::fs::write("assets/mcus/examples/stm32f103rb.ron", ron).unwrap();
    }

    /// The shipped example file must stay parseable and build into a 64-pin Mcu.
    /// Read at runtime (not `include_str!`) so the suite compiles even before the
    /// generator has produced the file.
    #[test]
    fn stm32f103rb_example_is_valid() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/mcus/examples/stm32f103rb.ron");
        let text = std::fs::read_to_string(path).expect("example file exists (run generator)");
        let def: McuDefinition = ron::from_str(&text).expect("example RON parses");
        assert_eq!(def.id, "stm32f103rb");
        assert_eq!(def.family, "stm32f1");
        let mcu = def.build_mcu();
        let total = mcu.top_pins.len()
            + mcu.bottom_pins.len()
            + mcu.left_pins.len()
            + mcu.right_pins.len();
        assert_eq!(total, 64, "LQFP64 must expose exactly 64 pins");
        // A family backend exists, so it generates real code.
        assert!(!mcu.fresh_main_rs().is_empty(), "stm32f1 example must generate code");
    }

    /// One-shot: write an example chip that uses a **data-driven `ClockDef::Graph`**
    /// (the F103 tree + diagram as data) so the generic graph clock tab can be
    /// exercised by importing it. Run with:
    /// `cargo test generate_graph_clock_example -- --ignored`
    #[test]
    #[ignore]
    fn generate_graph_clock_example() {
        use crate::panels::mcu_module::clock::graph::layout::stm32f1_layout;
        use crate::panels::mcu_module::clock::graph::{stm32f1_graph, GraphClock};
        use crate::panels::mcu_module::clock::model::{ClockLimits, Stm32f1Clock};
        use ron::ser::PrettyConfig;

        // Start from the built-in C8T6, swap the clock for an embedded graph.
        let mut def = builtin_for("stm32f103c8t6").expect("base def");
        def.id = "stm32f103c8t6-graph".to_owned();
        def.display_name = "STM32F103C8T6 (graph clock demo)".to_owned();
        def.clock = ClockDef::Graph(GraphClock {
            graph: stm32f1_graph(&Stm32f1Clock::default()),
            layout: stm32f1_layout(&ClockLimits::default()),
        });

        std::fs::create_dir_all("assets/mcus/examples").unwrap();
        let ron =
            ron::ser::to_string_pretty(&def, PrettyConfig::default().struct_names(true)).unwrap();
        std::fs::write("assets/mcus/examples/stm32f103_graphclock.ron", ron).unwrap();
    }

    /// The graph-clock example parses and builds into a `ClockConfig::Graph`
    /// carrying both the evaluatable graph and its diagram layout.
    #[test]
    fn graph_clock_example_is_valid() {
        use crate::panels::mcu_module::clock::ClockConfig;

        let path =
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/mcus/examples/stm32f103_graphclock.ron");
        let text = std::fs::read_to_string(path).expect("example exists (run generator)");
        let def: McuDefinition = ron::from_str(&text).expect("graph-clock example parses");
        assert_eq!(def.id, "stm32f103c8t6-graph");
        assert!(matches!(def.clock, ClockDef::Graph(_)));

        match def.build_mcu().clock {
            ClockConfig::Graph(gc) => {
                assert!(!gc.graph.nodes.is_empty(), "graph must carry nodes");
                assert!(!gc.layout.outputs.is_empty(), "layout must carry the diagram");
            }
            _ => panic!("expected a graph clock"),
        }
    }

    #[test]
    fn all_builtins_parse() {
        let defs = builtin_definitions();
        assert_eq!(defs.len(), BUILTINS.len(), "every bundled RON must parse");
    }

    #[test]
    fn builtins_build_into_mcus() {
        for (id, _) in BUILTINS {
            let def = builtin_for(id).unwrap_or_else(|| panic!("missing {id}"));
            let mcu = def.build_mcu();
            // A real chip has at least one configurable pin somewhere.
            let total = mcu.top_pins.len()
                + mcu.bottom_pins.len()
                + mcu.left_pins.len()
                + mcu.right_pins.len();
            assert!(total > 0, "{id} produced no pins");
        }
    }

    /// The bundled RON must build chips identical to the original factories.
    #[test]
    fn esp32c3_ron_matches_factory() {
        use crate::panels::mcu_module::mock_esp32c3::create_esp32c3;
        use crate::panels::mcu_module::pins::logic::pin::Pin;

        let factory = create_esp32c3();
        let built = builtin_for("esp32c3").unwrap().build_mcu();
        let same = |a: &[Pin], b: &[Pin]| {
            a.len() == b.len()
                && a.iter().zip(b).all(|(x, y)| {
                    x.number == y.number
                        && x.name == y.name
                        && x.reserved == y.reserved
                        && x.available_functions == y.available_functions
                })
        };
        assert!(same(&built.top_pins, &factory.top_pins));
        assert!(same(&built.bottom_pins, &factory.bottom_pins));
        assert!(same(&built.left_pins, &factory.left_pins));
        assert!(same(&built.right_pins, &factory.right_pins));
        assert_eq!(built.toolchain, factory.toolchain);
        assert_eq!(built.clock, factory.clock);
    }
}
