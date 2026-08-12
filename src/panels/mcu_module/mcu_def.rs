//! Data-driven MCU definition (Phase 1 foundation).
//!
//! A serializable description of a chip — pins, clock, project params — that can
//! be loaded from a RON file.  [`McuDefinition::build_mcu`] turns it into the
//! runtime [`Mcu`] the configurator draws.
//!
//! This runs **in parallel** with the existing hardcoded factories
//! (`mock_mcu.rs`, `mock_esp32c3.rs`); later phases make the definition the
//! single source of truth and add the registry + family backends.

use serde::{Deserialize, Serialize};

use super::clock::graph::GraphClock;
use super::clock::{ClockConfig, ClockLimits, ClockPreset, Stm32f1Clock};
use super::mcu::Mcu;
use super::mcu_catalog::ToolchainKind;
use super::pins::logic::pin::Pin;
use super::pins::logic::pin_function::PinFunction;

/// One pin in a chip definition — the data form of [`Pin`] without runtime state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PinDef {
    pub number: usize,
    pub name: String,
    #[serde(default)]
    pub reserved: bool,
    /// Complete list of selectable functions (e.g. `[GpioInput, GpioOutput, …]`).
    /// Empty for reserved pins (VDD, VSS, …).
    #[serde(default)]
    pub functions: Vec<PinFunction>,
}

impl PinDef {
    /// Extract a `PinDef` from a runtime [`Pin`] — used by round-trip tests and
    /// to export the current hardcoded factories to RON.
    pub fn from_pin(p: &Pin) -> Self {
        Self {
            number: p.number,
            name: p.name.clone(),
            reserved: p.reserved,
            functions: p.available_functions.clone(),
        }
    }

    /// Build a runtime [`Pin`] (the selected function starts `Unset`).
    pub fn to_pin(&self) -> Pin {
        Pin {
            number: self.number,
            name: self.name.clone(),
            reserved: self.reserved,
            available_functions: self.functions.clone(),
            selected_function: PinFunction::Unset,
            custom_label: String::new(),
            irq: None,
        }
    }
}

/// The four physical sides of the chip, drawn around the package.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct PinLayout {
    #[serde(default)]
    pub top: Vec<PinDef>,
    #[serde(default)]
    pub bottom: Vec<PinDef>,
    #[serde(default)]
    pub left: Vec<PinDef>,
    #[serde(default)]
    pub right: Vec<PinDef>,
}

/// Project-generation parameters: target triple, HAL dependency line, memory
/// layout and probe/flash chip — everything `project_gen` needs to emit a
/// buildable Cargo project. Consumed alongside the chip's `ToolchainKind`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectDef {
    pub pkg_name: String,
    pub target: String,
    #[serde(default)]
    pub flash_origin: String,
    #[serde(default)]
    pub flash_size: String,
    #[serde(default)]
    pub ram_origin: String,
    #[serde(default)]
    pub ram_size: String,
    pub hal_dep: String,
    pub probe_chip: String,
    #[serde(default)]
    pub memory_comment: String,
}

/// Clock model + defaults for this chip, keyed per clock family.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClockDef {
    /// Compact STM32F1 authoring form — upgraded to a full graph at load.
    Stm32f1(Stm32f1Clock),
    /// Built-in ESP32-C3 clock graph (topology + diagram are code).
    Esp32c3,
    /// Data-driven clock tree + diagram, importable for any family.
    Graph(GraphClock),
    /// No modelled clock tree yet (STM8, …).
    None,
}

impl ClockDef {
    /// Build the runtime clock. The graph is the only runtime model, so the
    /// compact family forms are upgraded to a full graph here (topology +
    /// diagram from the family's `*_graph` / `*_layout`; `limits` only feeds the
    /// STM32F1 HSE-range label).
    ///
    /// Public because the AI clock importer needs the same upgrade: its second
    /// pass merges onto whatever tree the form currently holds, family template
    /// included.
    pub fn to_config(&self, limits: &ClockLimits) -> ClockConfig {
        use super::clock::graph::{
            GraphClock, esp32c3_graph, esp32c3_layout, layout::stm32f1_layout, stm32f1_graph,
        };
        match self {
            ClockDef::Stm32f1(c) => ClockConfig::Graph(GraphClock {
                graph: stm32f1_graph(c),
                layout: stm32f1_layout(limits),
            }),
            ClockDef::Esp32c3 => ClockConfig::Graph(GraphClock {
                graph: esp32c3_graph(),
                layout: esp32c3_layout(),
            }),
            ClockDef::Graph(g) => ClockConfig::Graph(g.clone()),
            ClockDef::None => ClockConfig::None,
        }
    }

    /// Same clock family? Used to filter presets that don't fit the chip.
    fn same_family(&self, other: &ClockDef) -> bool {
        matches!(
            (self, other),
            (ClockDef::Stm32f1(_), ClockDef::Stm32f1(_))
                | (ClockDef::Esp32c3, ClockDef::Esp32c3)
                | (ClockDef::Graph(_), ClockDef::Graph(_))
                | (ClockDef::None, ClockDef::None)
        )
    }
}

/// One named clock preset shipped with a definition. `config` reuses the
/// family-tagged [`ClockDef`], so presets stay extensible per family; presets
/// whose family doesn't match the chip's `clock` are dropped at build time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClockPresetDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub config: ClockDef,
}

/// `skip_serializing_if` helper — omit `clock_limits` from generated RON when
/// it carries only the family defaults.
fn limits_are_default(l: &ClockLimits) -> bool {
    *l == ClockLimits::default()
}

/// A complete, importable MCU definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McuDefinition {
    /// Stable identifier, e.g. "stm32f103c8t6".
    pub id: String,
    /// Display name shown in the chip selector.
    pub display_name: String,
    /// Family / backend key (e.g. "stm32f1", "esp32c3") — selects the codegen
    /// + clock backend in later phases.
    pub family: String,
    #[serde(default)]
    pub package: String,
    #[serde(default)]
    pub cpu: String,
    pub toolchain: ToolchainKind,
    pub project: ProjectDef,
    #[serde(default)]
    pub pins: PinLayout,
    pub clock: ClockDef,
    /// Per-chip datasheet frequency ceilings. Omitted (or partial) fields fall
    /// back to the family defaults (STM32F103 values), so existing `.ron`
    /// files keep parsing unchanged. Example override in RON:
    /// `clock_limits: (sysclk_max: 24000000, hclk_max: 24000000)`.
    #[serde(default, skip_serializing_if = "limits_are_default")]
    pub clock_limits: ClockLimits,
    /// Chip-specific one-click presets for the Clock tab; when empty the
    /// family's built-in presets are shown instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clock_presets: Vec<ClockPresetDef>,
}

impl McuDefinition {
    /// Build the runtime [`Mcu`] (pin diagram + clock) from this definition.
    pub fn build_mcu(&self) -> Mcu {
        let map = |defs: &[PinDef]| defs.iter().map(PinDef::to_pin).collect::<Vec<_>>();
        let mut mcu = Mcu::new(
            self.display_name.clone(),
            self.family.clone(),
            self.toolchain.clone(),
            map(&self.pins.top),
            map(&self.pins.bottom),
            map(&self.pins.left),
            map(&self.pins.right),
        );
        mcu.id = self.id.clone();
        mcu.clock = self.clock.to_config(&self.clock_limits);
        // The definition's tree is this chip's factory clock — snapshot it for
        // the Clock tab's "Reset" button before any saved state is applied.
        mcu.capture_clock_defaults();
        mcu.clock_limits = self.clock_limits;
        mcu.clock_presets = self
            .clock_presets
            .iter()
            .filter(|p| p.config.same_family(&self.clock))
            .filter_map(|p| match &p.config {
                ClockDef::Stm32f1(c) => Some(ClockPreset {
                    name: p.name.clone(),
                    description: p.description.clone(),
                    config: c.clone(),
                }),
                // `ClockPreset` is the Stm32f1 runtime preset; other families
                // carry presets in-graph, so they don't convert here.
                ClockDef::Esp32c3 | ClockDef::Graph(_) | ClockDef::None => None,
            })
            .collect();
        mcu
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::builtins::builtin_for;
    use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;

    fn stm_def() -> McuDefinition {
        builtin_for("stm32f103c8t6").expect("built-in stm32f103c8t6 definition")
    }

    #[test]
    fn ron_round_trips_definition() {
        let def = stm_def();
        let ron = ron::to_string(&def).expect("serialize to RON");
        let parsed: McuDefinition = ron::from_str(&ron).expect("parse RON");
        assert_eq!(def, parsed, "RON round-trip must be lossless");
    }

    #[test]
    fn pindef_round_trips_a_pin() {
        // A real pin carrying multiple peripheral functions.
        let pin = create_stm32f103c8tx()
            .top_pins
            .into_iter()
            .find(|p| !p.reserved && !p.available_functions.is_empty())
            .unwrap();
        let back = PinDef::from_pin(&pin).to_pin();
        assert_eq!(back.number, pin.number);
        assert_eq!(back.name, pin.name);
        assert_eq!(back.reserved, pin.reserved);
        assert_eq!(back.available_functions, pin.available_functions);
    }

    #[test]
    fn build_mcu_matches_factory() {
        let factory = create_stm32f103c8tx();
        let built = stm_def().build_mcu();

        let same = |a: &[Pin], b: &[Pin]| {
            a.len() == b.len()
                && a.iter().zip(b).all(|(x, y)| {
                    x.number == y.number
                        && x.name == y.name
                        && x.reserved == y.reserved
                        && x.available_functions == y.available_functions
                })
        };
        assert!(same(&built.top_pins, &factory.top_pins), "top pins differ");
        assert!(
            same(&built.bottom_pins, &factory.bottom_pins),
            "bottom pins differ"
        );
        assert!(
            same(&built.left_pins, &factory.left_pins),
            "left pins differ"
        );
        assert!(
            same(&built.right_pins, &factory.right_pins),
            "right pins differ"
        );
        assert_eq!(built.clock, factory.clock, "clock differs");
    }

    /// A definition without `clock_limits` / `clock_presets` (every existing
    /// `.ron`) must keep parsing and fall back to the family defaults.
    #[test]
    fn missing_clock_fields_fall_back_to_defaults() {
        let def = stm_def(); // bundled RON predates the new fields
        assert_eq!(def.clock_limits, ClockLimits::default());
        assert!(def.clock_presets.is_empty());

        let mcu = def.build_mcu();
        assert_eq!(mcu.clock_limits, ClockLimits::default());
        assert!(
            mcu.clock_presets.is_empty(),
            "empty -> family presets in the GUI"
        );
    }

    /// Custom limits and presets declared in a definition reach the runtime Mcu.
    #[test]
    fn clock_limits_and_presets_flow_into_mcu() {
        let mut def = stm_def();
        def.clock_limits = ClockLimits {
            sysclk_max: 24_000_000,
            hclk_max: 24_000_000,
            ..ClockLimits::default()
        };
        def.clock_presets = vec![ClockPresetDef {
            name: "24 MHz max".to_owned(),
            description: "Value-line style cap.".to_owned(),
            config: ClockDef::Stm32f1(Stm32f1Clock::default()),
        }];

        let mcu = def.build_mcu();
        assert_eq!(mcu.clock_limits.sysclk_max, 24_000_000);
        assert_eq!(mcu.clock_presets.len(), 1);
        assert_eq!(mcu.clock_presets[0].name, "24 MHz max");
    }

    /// Presets from a different clock family are dropped, not mis-applied.
    #[test]
    fn foreign_family_presets_are_filtered() {
        let mut def = stm_def(); // chip clock family: Stm32f1
        def.clock_presets = vec![
            ClockPresetDef {
                name: "fits".to_owned(),
                description: String::new(),
                config: ClockDef::Stm32f1(Stm32f1Clock::default()),
            },
            ClockPresetDef {
                name: "foreign".to_owned(),
                description: String::new(),
                config: ClockDef::None,
            },
        ];

        let mcu = def.build_mcu();
        assert_eq!(mcu.clock_presets.len(), 1);
        assert_eq!(mcu.clock_presets[0].name, "fits");
    }

    /// New fields survive a RON round-trip (and stay omitted when default).
    #[test]
    fn clock_fields_round_trip_in_ron() {
        let mut def = stm_def();
        def.clock_limits = ClockLimits {
            adcclk_max: 12_000_000,
            ..ClockLimits::default()
        };
        def.clock_presets = vec![ClockPresetDef {
            name: "p".to_owned(),
            description: "d".to_owned(),
            config: ClockDef::Stm32f1(Stm32f1Clock::default()),
        }];

        let ron = ron::to_string(&def).expect("serialize");
        let parsed: McuDefinition = ron::from_str(&ron).expect("parse");
        assert_eq!(parsed, def);

        // Default limits / empty presets are skipped in the output entirely.
        let plain = ron::to_string(&stm_def()).expect("serialize");
        assert!(!plain.contains("clock_limits"));
        assert!(!plain.contains("clock_presets"));
    }

    /// A chip can ship a data-driven `ClockDef::Graph`: it round-trips through
    /// RON and builds into a runtime `ClockConfig::Graph`.
    #[test]
    fn graph_clock_def_round_trips_and_builds() {
        use crate::panels::mcu_module::clock::ClockConfig;
        use crate::panels::mcu_module::clock::graph::layout::stm32f1_layout;
        use crate::panels::mcu_module::clock::graph::{GraphClock, stm32f1_graph};

        let gc = GraphClock {
            graph: stm32f1_graph(&Stm32f1Clock::default()),
            layout: stm32f1_layout(&ClockLimits::default()),
        };
        let mut def = stm_def();
        def.clock = ClockDef::Graph(gc);

        let mcu = def.build_mcu();
        assert!(
            matches!(mcu.clock, ClockConfig::Graph(_)),
            "build_mcu yields a graph clock"
        );

        let ron = ron::to_string(&def).expect("serialize def with graph clock");
        let back: McuDefinition = ron::from_str(&ron).expect("parse def with graph clock");
        assert_eq!(
            def, back,
            "definition with embedded graph + layout must round-trip"
        );
    }

    /// The Clock tab's "Reset" button: an edited tree returns to exactly the
    /// definition's factory states, and resetting twice is a no-op.
    #[test]
    fn reset_clock_restores_the_definition_default() {
        use crate::panels::mcu_module::clock::ClockConfig;
        use crate::panels::mcu_module::clock::graph::NodeState;

        let mut mcu = stm_def().build_mcu();
        assert!(mcu.clock_defaults.is_some(), "defaults captured at build");
        assert!(mcu.clock_is_default(), "a freshly built chip is pristine");
        assert!(!mcu.reset_clock(), "nothing to reset yet");

        // Edit the PLL multiplier the way the diagram widget does.
        let pristine = mcu.clock.clone();
        let ClockConfig::Graph(gc) = &mut mcu.clock else {
            panic!("stm32f103 has a graph clock");
        };
        let pll = gc.graph.node_mut("pllmul").expect("pllmul node");
        pll.state = NodeState::Value(4);

        assert!(!mcu.clock_is_default(), "edit is detected");
        assert!(mcu.reset_clock(), "reset reports the change");
        assert_eq!(mcu.clock, pristine, "tree is back to the factory config");
        assert!(!mcu.reset_clock(), "second reset is a no-op");
    }

    /// "Save to chip" writes the edited TOPOLOGY into the definition — the part
    /// `mcu.config` cannot carry (it round-trips node states, not the graph). A
    /// definition carrying an edited tree must survive the `.ron` round trip and
    /// rebuild into exactly that tree.
    #[test]
    fn an_edited_clock_tree_survives_the_definition_round_trip() {
        use crate::panels::mcu_module::clock::ClockConfig;
        use crate::panels::mcu_module::clock::graph::edit::{PaletteKind, add_node, connect};
        use crate::panels::mcu_module::clock::graph::{GraphClock, auto_layout};

        // Start from the chip's own tree and add a node, as the editor does.
        let mut mcu = stm_def().build_mcu();
        let ClockConfig::Graph(gc) = &mut mcu.clock else {
            panic!("graph clock");
        };
        let mut boxes = auto_layout(&gc.graph).nodes;
        let added = add_node(
            &mut gc.graph,
            &mut boxes,
            PaletteKind::Output,
            10.0,
            10.0,
            96.0,
            26.0,
        );
        connect(&mut gc.graph, "hclk", &added).expect("wire the new output");
        let edited = GraphClock {
            graph: gc.graph.clone(),
            layout: gc.layout.clone(),
        };

        let mut def = stm_def();
        def.clock = ClockDef::Graph(edited.clone());
        let text = ron::ser::to_string_pretty(&def, ron::ser::PrettyConfig::default())
            .expect("serialize the edited definition");
        let back: McuDefinition = ron::from_str(&text).expect("parse it back");
        assert_eq!(back, def, "the edited tree round-trips");

        let rebuilt = back.build_mcu();
        let ClockConfig::Graph(out) = &rebuilt.clock else {
            panic!("graph clock");
        };
        assert!(
            out.graph.node(&added).is_some(),
            "the node added in the editor is in the rebuilt chip"
        );
        assert!(
            out.graph.edges.iter().any(|e| e.to == added),
            "and so is its wire"
        );
        // The saved tree is the chip's factory config now, so Reset aims at it.
        assert!(rebuilt.clock_is_default());
    }

    /// A saved project's clock is adopted AFTER the snapshot, so Reset targets
    /// the chip default — not whatever the project was opened with.
    #[test]
    fn defaults_survive_a_restored_project_clock() {
        use crate::panels::mcu_module::clock::graph::{NodeState, stm32f1_graph};

        let mut mcu = stm_def().build_mcu();
        let pristine = mcu.clock.clone();
        mcu.apply_saved_clock(Stm32f1Clock {
            pll_mul: 4,
            ..Stm32f1Clock::default()
        });
        assert!(
            !mcu.clock_is_default(),
            "restored config differs from factory"
        );

        assert!(mcu.reset_clock());
        assert_eq!(mcu.clock, pristine);

        // Sanity: the snapshot really is the definition's tree, not a clone of
        // the saved one.
        let def_graph = stm32f1_graph(&Stm32f1Clock::default());
        let saved = mcu.clock_defaults.as_ref().unwrap();
        assert!(saved.states_match(&def_graph));
        assert_ne!(
            saved.node("pllmul").map(|n| n.state.clone()),
            Some(NodeState::Value(4))
        );
    }
}
