//! Editable form model for authoring a new [`McuDefinition`] in the UI.
//!
//! This is the PURE half (no egui): the field buffers, validation, and the
//! `McuForm ⇄ McuDefinition` conversions. The dialog in `app::mcu_form_dialog`
//! renders it and, on Save, writes the RON to the user `mcus/` folder and
//! merges it into the live registry — the same path an imported `.ron` takes,
//! so a form-authored chip is indistinguishable from an imported one.
//!
//! Scope: a chip in an ALREADY-SUPPORTED family (STM32F1 / ESP32-C3) is pure
//! data and fully authorable here. A brand-new family still needs a codegen
//! `FamilyBackend` in code — the form warns when the family is unknown.

use serde::{Deserialize, Serialize};

use super::mcu_catalog::ToolchainKind;
use super::mcu_def::{ClockDef, McuDefinition, PinDef, PinLayout, ProjectDef};
use super::pins::logic::pin_function::PinFunction;

/// The four sides. This is the STORAGE order of `McuForm::pins` (and of
/// `PinLayout` in the definition) — do not reorder it.
pub const SIDES: [&str; 4] = ["Top", "Bottom", "Left", "Right"];

/// The order the GUI presents the sides in: **Left → Bottom → Right → Top**.
/// That mirrors QFP/QFN numbering (pin 1 sits at the top of the left side and
/// the count runs counter-clockwise), so reading the editors top-to-bottom
/// follows the pin numbers — the same walk [`crate::panels::mcu_module::stm32_pin_data`]
/// uses when it distributes an imported pinout. Values are indices into
/// [`SIDES`] / `McuForm::pins`; the storage order above is unchanged.
pub const SIDE_DISPLAY_ORDER: [usize; 4] = [2, 1, 3, 0];

/// One editable pin row (data form of [`PinDef`]). Numbers and functions are
/// STRINGS so a half-typed value never snaps back: the number stays as typed,
/// and functions are a space/comma list of tokens ([`parse_functions`]) — far
/// lighter to edit than a per-pin multi-select of a dozen parameterized
/// variants. `in out usart1_tx spi2_sck i2c1_scl adc1_5 tim2_1 swdio` etc.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PinRow {
    pub number: String,
    pub name: String,
    pub reserved: bool,
    pub functions: String,
    /// UI-only provenance flag: `true` for a row that an AI datasheet import
    /// created, so the pin editor can tag it as "review me". Never persisted
    /// (dropped by [`McuForm::to_definition`]).
    pub imported: bool,
    /// `(signal, alternate-function index)` captured from the vendor GPIO IP
    /// file. Carried through the form untouched — it is data ABOUT the chip, not
    /// something to author by hand — and written to `PinDef::af`.
    pub af: Vec<(String, u8)>,
    /// `(function token, GPIO)` for tokens contributed by a GPIO bonded to the
    /// same package pin - see
    /// [`PinDef::fn_owner`](crate::panels::mcu_module::mcu_def::PinDef::fn_owner).
    /// Carried through the form untouched, like [`af`](Self::af): it is data
    /// about the package, not something to author by hand.
    pub fn_owner: Vec<(String, String)>,
}

/// The clock model offered by the form. A full graph editor is out of scope,
/// so the choices are the built-in family models plus "none"; importing a
/// `.ron` remains the way to carry a hand-authored [`ClockDef::Graph`] (the
/// form PRESERVES such a graph — see [`McuForm::imported_clock`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClockChoice {
    None,
    Stm32f1,
    Esp32c3,
    /// STM32WBA tree (data-driven graph — ships the 100 MHz PLL preset).
    Stm32wba,
    /// STM32F4 tree (data-driven graph — ships the 100 MHz HSI→PLL preset).
    Stm32f4,
    /// STM32F2 tree. The same topology as [`ClockChoice::Stm32f4`] (one embassy
    /// RCC module covers F2/F4/F7), kept SEPARATE because the PLLN window is
    /// not the same: F2's PAC has `MUL192..=MUL432` where F4 has `MUL2..`, so an
    /// N the F4 accepts can name a variant the F2 does not have. Also its own
    /// ceilings — 120 MHz HCLK, 30/60 MHz APB — versus F4's 100/50/100.
    Stm32f2,
    /// STM32G4 tree (data-driven graph — ships the 150 MHz HSI→PLL preset). No
    /// hand-authored layout: the diagram is auto-generated from the topology.
    Stm32g4,
    /// STM32G0 tree (data-driven graph — ships the 64 MHz HSI→PLL preset,
    /// single APB bus). Auto-generated layout.
    Stm32g0,
    /// STM32L4 tree (data-driven graph — ships the 80 MHz HSI→PLL preset; MSI
    /// shown but HSI-PLL codegen). Auto-generated layout.
    Stm32l4,
}

impl ClockChoice {
    pub const ALL: [ClockChoice; 8] = [
        ClockChoice::None,
        ClockChoice::Stm32f1,
        ClockChoice::Esp32c3,
        ClockChoice::Stm32wba,
        ClockChoice::Stm32f4,
        ClockChoice::Stm32g4,
        ClockChoice::Stm32g0,
        ClockChoice::Stm32l4,
    ];
    /// The clock tree a chip FAMILY defaults to — so an imported chip (XML or
    /// AI datasheet) whose family has a modelled tree gets a working Clock tab
    /// and real RCC codegen without the user picking one by hand. `None` for
    /// families with no tree yet (the reset-default clock still compiles).
    pub fn for_family(family: &str) -> ClockChoice {
        match family {
            "stm32f1" => ClockChoice::Stm32f1,
            "stm32wba" => ClockChoice::Stm32wba,
            // F2/F4/F7 share embassy's f247 RCC, but NOT the PLLN window or the
            // clock ceilings — see `ClockChoice::Stm32f2`.
            "stm32f2" => ClockChoice::Stm32f2,
            "stm32f4" | "stm32f7" => ClockChoice::Stm32f4,
            "stm32g4" => ClockChoice::Stm32g4,
            "stm32g0" => ClockChoice::Stm32g0,
            "stm32l4" => ClockChoice::Stm32l4,
            "esp32c3" => ClockChoice::Esp32c3,
            _ => ClockChoice::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ClockChoice::None => "None",
            ClockChoice::Stm32f1 => "STM32F1 tree",
            ClockChoice::Esp32c3 => "ESP32-C3 tree",
            ClockChoice::Stm32wba => "STM32WBA tree",
            ClockChoice::Stm32f4 => "STM32F4/F7 tree",
            ClockChoice::Stm32f2 => "STM32F2 tree",
            ClockChoice::Stm32g4 => "STM32G4 tree",
            ClockChoice::Stm32g0 => "STM32G0 tree",
            ClockChoice::Stm32l4 => "STM32L4 tree",
        }
    }
    /// Public so a definition can fall back to its family's tree when it
    /// declares no clock of its own (see `McuDefinition::effective_clock`).
    pub fn to_def(self) -> ClockDef {
        use crate::panels::mcu_module::clock::graph::{
            GraphClock, stm32f2_graph, stm32f2_layout, stm32f4_graph, stm32f4_layout,
            stm32g0_graph, stm32g4_graph, stm32l4_graph, stm32wba_graph, stm32wba_layout,
        };
        match self {
            ClockChoice::None => ClockDef::None,
            ClockChoice::Stm32f1 => ClockDef::Stm32f1(Default::default()),
            ClockChoice::Esp32c3 => ClockDef::Esp32c3,
            ClockChoice::Stm32wba => ClockDef::Graph(GraphClock {
                graph: stm32wba_graph(),
                layout: stm32wba_layout(),
                bindings: Default::default(),
            }),
            ClockChoice::Stm32f4 => ClockDef::Graph(GraphClock {
                graph: stm32f4_graph(),
                layout: stm32f4_layout(),
                bindings: Default::default(),
            }),
            ClockChoice::Stm32f2 => ClockDef::Graph(GraphClock {
                graph: stm32f2_graph(),
                layout: stm32f2_layout(),
                bindings: Default::default(),
            }),
            // Empty layout on purpose — `auto_layout` draws the diagram from the
            // graph topology, so a new family needs no hand-tuned positions.
            ClockChoice::Stm32g4 => ClockDef::Graph(GraphClock {
                graph: stm32g4_graph(),
                layout: Default::default(),
                bindings: Default::default(),
            }),
            ClockChoice::Stm32g0 => ClockDef::Graph(GraphClock {
                graph: stm32g0_graph(),
                layout: Default::default(),
                bindings: Default::default(),
            }),
            ClockChoice::Stm32l4 => ClockDef::Graph(GraphClock {
                graph: stm32l4_graph(),
                layout: Default::default(),
                bindings: Default::default(),
            }),
        }
    }
    fn from_def(d: &ClockDef) -> ClockChoice {
        use crate::panels::mcu_module::clock::graph::{
            is_f4_graph, is_g0_graph, is_g4_graph, is_l4_graph, is_wba_graph,
        };
        match d {
            ClockDef::Stm32f1(_) => ClockChoice::Stm32f1,
            ClockDef::Esp32c3 => ClockChoice::Esp32c3,
            ClockDef::Graph(gc) if is_wba_graph(&gc.graph) => ClockChoice::Stm32wba,
            ClockDef::Graph(gc) if is_f4_graph(&gc.graph) => ClockChoice::Stm32f4,
            ClockDef::Graph(gc) if is_g4_graph(&gc.graph) => ClockChoice::Stm32g4,
            ClockDef::Graph(gc) if is_g0_graph(&gc.graph) => ClockChoice::Stm32g0,
            ClockDef::Graph(gc) if is_l4_graph(&gc.graph) => ClockChoice::Stm32l4,
            // A foreign graph maps to None here but is PRESERVED via
            // `McuForm::imported_clock`; plain none stays none.
            ClockDef::Graph(_) | ClockDef::None => ClockChoice::None,
        }
    }
}

/// All editable fields of a new / cloned MCU definition.
#[derive(Clone, Debug, PartialEq)]
pub struct McuForm {
    // Identity
    pub id: String,
    pub display_name: String,
    pub family: String,
    pub cpu: String,
    pub package: String,
    /// Datasheet maximum core frequency in MHz. `None` when the vendor file
    /// states none — shown as nothing, never as a family guess.
    pub max_mhz: Option<u32>,
    /// The chip's DMA channels, carried through untouched: imported from the
    /// vendor database, not authorable here (see [`super::mcu_def::DmaDef`]).
    /// Editing a chip in this form must not silently drop them.
    pub dma: Option<super::mcu_def::DmaDef>,
    /// The chip's interrupt vectors, carried through untouched like `dma`.
    pub irq_vectors: Vec<String>,
    /// The chip's USART IP version, carried verbatim from the vendor data —
    /// the IDE never edits it, it only decides whether swap/invert are offered.
    pub usart_ip: Option<String>,
    pub sdmmc_ip: Option<String>,
    // Toolchain + target
    pub toolchain: ToolchainKind,
    pub target: String,
    // Memory (RustEmbedded only — ESP owns its layout)
    pub flash_origin: String,
    pub flash_size: String,
    pub ram_origin: String,
    pub ram_size: String,
    pub memory_comment: String,
    // Probe / flash + dependency line
    pub probe_chip: String,
    pub hal_dep: String,
    // Clock model
    pub clock: ClockChoice,
    /// A hand-imported [`ClockDef::Graph`] the form cannot re-author: carried
    /// through Edit → Save verbatim while the choice stays `None`, so editing
    /// an imported chip never silently drops its clock tree.
    pub imported_clock: Option<ClockDef>,
    // Pins, per side
    pub pins: [Vec<PinRow>; 4],
    /// A ball grid (WLCSP / BGA) the form cannot re-author yet: carried through
    /// Edit -> Save verbatim, for the same reason as `imported_clock`. Editing a
    /// grid chip must not silently drop its balls.
    pub grid: Option<crate::panels::mcu_module::mcu_def::PinGridDef>,
    /// True when the form was opened to EDIT/clone an existing chip (so the
    /// dialog can warn before an id collision silently overrides a built-in).
    pub editing: bool,
}

impl Default for McuForm {
    fn default() -> Self {
        Self::blank()
    }
}

impl McuForm {
    /// A blank STM32-flavoured starting point (the most common authoring case).
    pub fn blank() -> Self {
        Self {
            grid: None,
            id: String::new(),
            display_name: String::new(),
            family: "stm32f1".into(),
            cpu: "Cortex-M3".into(),
            package: String::new(),
            max_mhz: None,
            dma: None,
            irq_vectors: Vec::new(),
            usart_ip: None,
            sdmmc_ip: None,
            toolchain: ToolchainKind::RustEmbedded,
            target: "thumbv7m-none-eabi".into(),
            flash_origin: "0x08000000".into(),
            flash_size: "64K".into(),
            ram_origin: "0x20000000".into(),
            ram_size: "20K".into(),
            memory_comment: String::new(),
            probe_chip: String::new(),
            hal_dep: "stm32f1xx-hal = { version = \"0.10\", features = [\"rt\"] }".into(),
            clock: ClockChoice::Stm32f1,
            imported_clock: None,
            pins: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            editing: false,
        }
    }

    /// Fill family / CPU / toolchain / target deterministically from the chip
    /// NAME (display_name, else id) — the "Auto-fill from name" button. Also
    /// seeds probe_chip when empty. No-op for a non-STM32 / unrecognised name;
    /// returns true when it recognised the name. See [`super::mcu_identity`].
    pub fn auto_fill_identity(&mut self) -> bool {
        let name = if !self.display_name.trim().is_empty() {
            self.display_name.trim().to_string()
        } else {
            self.id.trim().to_string()
        };
        let Some((family, cpu, toolchain, target)) = super::mcu_identity::identity_from_name(&name)
        else {
            return false;
        };
        self.family = family;
        self.cpu = cpu.to_string();
        self.toolchain = toolchain;
        self.target = target.to_string();
        // The HAL/PAC dependency line, too — same derivation the XML importer
        // uses. Without this, "Auto-fill" (and the AI import that calls it) left
        // a non-F1 STM32 on the blank form's `stm32f1xx-hal` default, so the
        // generated project wouldn't compile until the line was hand-edited.
        self.hal_dep = super::stm32_pin_data::hal_dep_for_name(&self.family, &name);
        if self.probe_chip.trim().is_empty() {
            self.probe_chip = name;
        }
        true
    }

    /// Move pin `idx` from side `from` to the END of side `to`, keeping the row
    /// intact. The pin NUMBER is untouched — which side a pin is drawn on is
    /// layout, not identity. `false` (no-op) for a same-side move or any
    /// out-of-range index/side.
    pub fn move_pin(&mut self, from: usize, idx: usize, to: usize) -> bool {
        if from == to || from >= self.pins.len() || to >= self.pins.len() {
            return false;
        }
        if idx >= self.pins[from].len() {
            return false;
        }
        let row = self.pins[from].remove(idx);
        self.pins[to].push(row);
        true
    }

    /// Move pin `idx` by `delta` positions within its own side (−1 = earlier,
    /// +1 = later). Order along a side IS the physical position, so this is how
    /// a pin gets placed after being moved across. `false` (no-op) at the ends.
    pub fn reorder_pin(&mut self, side: usize, idx: usize, delta: isize) -> bool {
        let Some(rows) = self.pins.get_mut(side) else {
            return false;
        };
        if idx >= rows.len() {
            return false;
        }
        let target = idx as isize + delta;
        if target < 0 || target as usize >= rows.len() {
            return false;
        }
        let row = rows.remove(idx);
        rows.insert(target as usize, row);
        true
    }

    /// Seed the form from an existing definition (the "Clone / Edit" path).
    pub fn from_definition(def: &McuDefinition) -> Self {
        let side = |ds: &[PinDef]| -> Vec<PinRow> {
            ds.iter()
                .map(|d| PinRow {
                    number: d.number.to_string(),
                    name: d.name.clone(),
                    reserved: d.reserved,
                    functions: functions_to_string(&d.functions),
                    imported: false,
                    af: d.af.clone(),
                    fn_owner: Vec::new(),
                })
                .collect()
        };
        Self {
            grid: def.pins.grid.clone(),
            id: def.id.clone(),
            display_name: def.display_name.clone(),
            family: def.family.clone(),
            cpu: def.cpu.clone(),
            package: def.package.clone(),
            max_mhz: def.max_mhz,
            dma: def.dma.clone(),
            irq_vectors: def.irq_vectors.clone(),
            usart_ip: def.usart_ip.clone(),
            sdmmc_ip: def.sdmmc_ip.clone(),
            toolchain: def.toolchain.clone(),
            target: def.project.target.clone(),
            flash_origin: def.project.flash_origin.clone(),
            flash_size: def.project.flash_size.clone(),
            ram_origin: def.project.ram_origin.clone(),
            ram_size: def.project.ram_size.clone(),
            memory_comment: def.project.memory_comment.clone(),
            probe_chip: def.project.probe_chip.clone(),
            hal_dep: def.project.hal_dep.clone(),
            clock: ClockChoice::from_def(&def.clock),
            imported_clock: match (&def.clock, ClockChoice::from_def(&def.clock)) {
                // A graph the form can't re-author (not the WBA one).
                (ClockDef::Graph(_), ClockChoice::None) => Some(def.clock.clone()),
                _ => None,
            },
            pins: [
                side(&def.pins.top),
                side(&def.pins.bottom),
                side(&def.pins.left),
                side(&def.pins.right),
            ],
            editing: true,
        }
    }

    /// Collected blocking errors — empty means [`to_definition`] will succeed.
    /// Order matches the form top-to-bottom so the first message points at the
    /// first offending field.
    pub fn errors(&self) -> Vec<String> {
        let mut e = Vec::new();
        if !is_valid_id(&self.id) {
            e.push(
                "Id must be non-empty and use only a–z, 0–9 and _ (it becomes the \
                 file name and the registry key)."
                    .into(),
            );
        }
        if self.display_name.trim().is_empty() {
            e.push("Display name is required.".into());
        }
        if self.family.trim().is_empty() {
            e.push("Family is required (selects the codegen + clock backend).".into());
        }
        if self.target.trim().is_empty() {
            e.push("Target triple is required (e.g. thumbv7m-none-eabi).".into());
        }
        // Memory + probe only matter for the ARM / probe-rs toolchain; ESP owns
        // its own memory layout and flashes over serial.
        if self.toolchain == ToolchainKind::RustEmbedded {
            if self.probe_chip.trim().is_empty() {
                e.push("Probe chip is required for the ARM toolchain (used by probe-rs).".into());
            }
            for (label, v) in [
                ("Flash origin", &self.flash_origin),
                ("Flash size", &self.flash_size),
                ("RAM origin", &self.ram_origin),
                ("RAM size", &self.ram_size),
            ] {
                if parse_ld_number(v).is_none() {
                    e.push(format!(
                        "{label} ('{v}') is not a valid value — use hex (0x…), \
                         decimal, or a K/M suffix (e.g. 64K)."
                    ));
                }
            }
        }
        // Pin numbers, when present, must be positive and unique across sides;
        // every function token must be recognised.
        let mut seen = std::collections::HashSet::new();
        for row in self.pins.iter().flatten() {
            let n = row.number.trim();
            if n.is_empty() && row.name.trim().is_empty() && row.functions.trim().is_empty() {
                continue; // a wholly blank scratch row is ignored, not an error
            }
            match n.parse::<usize>() {
                Ok(num) if num >= 1 => {
                    if !seen.insert(num) {
                        e.push(format!("Pin number {num} is used more than once."));
                    }
                }
                _ => e.push(format!(
                    "Pin '{}' has an invalid number ('{n}') — use a positive integer.",
                    row.name.trim()
                )),
            }
            for bad in unknown_function_tokens(&row.functions) {
                e.push(format!(
                    "Pin '{}' has an unknown function token '{bad}'.",
                    row.name.trim()
                ));
            }
        }
        e
    }

    /// Non-blocking advisories (shown amber; do not prevent Save).
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        // Warn exactly when no codegen backend claims this family — so a family
        // handled by the generic STM32 (embassy) backend never flags.
        if crate::panels::mcu_module::codegen::family::backend_for(self.family.trim()).is_none() {
            w.push(format!(
                "Family '{}' has no codegen backend yet — the chip loads and its \
                 pins/clock show, but configuring a peripheral won't generate init \
                 code until a backend is added.",
                self.family.trim()
            ));
        }
        // A ball-grid chip legitimately has NO edge pins — its pads are in the
        // grid, which the form carries but does not edit.
        if self.pins.iter().all(|s| s.is_empty()) && self.grid.is_none() {
            w.push("No pins defined — the Pins canvas will be empty.".into());
        }
        if let Some(g) = &self.grid {
            w.push(format!(
                "{} ball(s) on a {}x{} grid. The form edits edge pins only, so the                  grid is carried through unchanged — edit it in the .ron.",
                g.cells.len(),
                g.rows,
                g.cols
            ));
        }
        if self.package.trim().is_empty() {
            w.push(
                "Package is empty — set it (e.g. UFQFPN48 / LQFP64) before importing pins from a \
                 datasheet, so the right pin-count column is read."
                    .into(),
            );
        }
        w
    }

    /// The clock the form currently resolves to — an imported graph (kept while
    /// the family dropdown is "None"), otherwise the chosen family's tree.
    ///
    /// Shared by `to_definition` (what gets saved) and the dialog's "Export
    /// clock" button (what gets written to a `.ron`), so the two never diverge.
    pub fn effective_clock(&self) -> ClockDef {
        match (&self.imported_clock, self.clock) {
            // The preserved imported graph, unless the user actively switched
            // to a family model.
            (Some(g), ClockChoice::None) => g.clone(),
            _ => self.clock.to_def(),
        }
    }

    /// Attach a hand/AI-authored clock graph (from a `.ron` import). Stored as
    /// `imported_clock` with the family dropdown reset to None, so
    /// `effective_clock` returns it — the exact path a foreign graph already
    /// travels when a chip `.ron` is loaded.
    pub fn set_imported_clock(&mut self, gc: crate::panels::mcu_module::clock::graph::GraphClock) {
        self.imported_clock = Some(ClockDef::Graph(gc));
        self.clock = ClockChoice::None;
    }

    /// Build the [`McuDefinition`]. Call only when [`errors`] is empty; blank
    /// scratch pin rows are dropped and numbers are parsed here.
    pub fn to_definition(&self) -> McuDefinition {
        let side = |rows: &[PinRow]| -> Vec<PinDef> {
            rows.iter()
                .filter(|r| {
                    !(r.number.trim().is_empty()
                        && r.name.trim().is_empty()
                        && r.functions.trim().is_empty())
                })
                .map(|r| PinDef {
                    number: r.number.trim().parse().unwrap_or(0),
                    name: r.name.trim().to_string(),
                    reserved: r.reserved,
                    functions: parse_functions(&r.functions),
                    af: r.af.clone(),
                    fn_owner: owners_to_functions(&r.fn_owner),
                })
                .collect()
        };
        McuDefinition {
            id: self.id.trim().to_string(),
            display_name: self.display_name.trim().to_string(),
            family: self.family.trim().to_string(),
            package: self.package.trim().to_string(),
            max_mhz: self.max_mhz,
            // Not authored in the form: the form writes a linker script, and
            // `ram_size` is where it puts the figure.
            sram_kb: None,
            dma: self.dma.clone(),
            irq_vectors: self.irq_vectors.clone(),
            usart_ip: self.usart_ip.clone(),
            sdmmc_ip: self.sdmmc_ip.clone(),
            cpu: self.cpu.trim().to_string(),
            toolchain: self.toolchain.clone(),
            project: ProjectDef {
                pkg_name: self.id.trim().to_string(),
                target: self.target.trim().to_string(),
                flash_origin: self.flash_origin.trim().to_string(),
                flash_size: self.flash_size.trim().to_string(),
                ram_origin: self.ram_origin.trim().to_string(),
                ram_size: self.ram_size.trim().to_string(),
                hal_dep: self.hal_dep.trim().to_string(),
                probe_chip: self.probe_chip.trim().to_string(),
                memory_comment: self.memory_comment.trim().to_string(),
            },
            pins: PinLayout {
                top: side(&self.pins[0]),
                bottom: side(&self.pins[1]),
                left: side(&self.pins[2]),
                right: side(&self.pins[3]),
                // The form edits the four sides only; a ball grid is authored in
                // the `.ron` for now (the grid editor is a later phase). Editing
                // a grid chip here would silently drop its balls, which is why
                // `mcu_form_dialog` refuses to open one — see `from_definition`.
                grid: self.grid.clone(),
            },
            clock: self.effective_clock(),
            // Each graph family ships its own ceilings so its preset isn't
            // flagged against the F103 defaults. (F4's real per-chip ceiling is
            // set by the XML converter; this is the F411-class default.)
            clock_limits: match self.clock {
                ClockChoice::Stm32wba => crate::panels::mcu_module::clock::graph::stm32wba_limits(),
                ClockChoice::Stm32f4 => {
                    crate::panels::mcu_module::clock::graph::stm32f4_limits_default()
                }
                _ => Default::default(),
            },
            clock_presets: Vec::new(),
        }
    }

    /// Serialize the built definition to pretty RON (what gets written to disk).
    pub fn to_ron(&self) -> Result<String, String> {
        ron::ser::to_string_pretty(&self.to_definition(), ron::ser::PrettyConfig::default())
            .map_err(|e| format!("RON serialize error: {e}"))
    }
}

/// A valid registry id / file stem: non-empty, ASCII `a–z 0–9 _` only.
pub fn is_valid_id(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Parse an ld-style number: hex (`0x…`), decimal, optional `K`/`M` suffix.
/// `None` on anything else. (Mirrors the private helper in `crate::size`; kept
/// here so the form has no dependency on that module.)
pub fn parse_ld_number(tok: &str) -> Option<u64> {
    let tok = tok.trim();
    if tok.is_empty() {
        return None;
    }
    let (body, mult) = match tok.chars().last() {
        Some('K') | Some('k') => (&tok[..tok.len() - 1], 1024u64),
        Some('M') | Some('m') => (&tok[..tok.len() - 1], 1024 * 1024),
        _ => (tok, 1),
    };
    let body = body.trim();
    let value = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()?
    } else {
        body.parse::<u64>().ok()?
    };
    Some(value * mult)
}

/// Fill one side with `count` sequential general-purpose pins named `prefix{n}`
/// (e.g. `PA0…`), each offering GPIO in/out — a fast start for a fresh chip.
pub fn gpio_bank(prefix: &str, start_number: usize, count: usize) -> Vec<PinRow> {
    (0..count)
        .map(|i| PinRow {
            number: (start_number + i).to_string(),
            name: format!("{prefix}{i}"),
            reserved: false,
            functions: "in out".to_string(),
            imported: false,
            af: Vec::new(),
            fn_owner: Vec::new(),
        })
        .collect()
}

/// The function-token cheatsheet shown under the pin editor.
pub const FUNCTION_TOKEN_HELP: &str = "in out · usart{n}_tx/rx/cts/rts/ck · \
    lpuart{n}_tx/rx/cts/rts · spi{n}_nss/sck/miso/mosi/rdy · i2c{n}_scl/sda · \
    adc{a}_{ch} · tim{t}_{ch} · swdio swclk · usb_dm usb_dp · can_rx can_tx · mco · \
    af:{signal} for anything else (e.g. af:sai1_sd_a, af:fmc_a0)";

/// Resolve `(token, gpio)` pairs into `(function, gpio)`.
///
/// A token the parser does not recognise is DROPPED rather than defaulted: an
/// owner attached to the wrong function would send generated code at the wrong
/// GPIO, which is worse than losing the override and falling back to the pin's
/// own name.
pub fn owners_to_functions(owners: &[(String, String)]) -> Vec<(PinFunction, String)> {
    owners
        .iter()
        .filter_map(|(tok, gpio)| {
            parse_functions(tok)
                .into_iter()
                .next()
                .map(|f| (f, gpio.clone()))
        })
        .collect()
}

/// Parse a space/comma-separated function token list into [`PinFunction`]s.
/// Unrecognised tokens are skipped here (validation lists them separately).
pub fn parse_functions(s: &str) -> Vec<PinFunction> {
    s.split([' ', ',', '\t', '\n'])
        .filter(|t| !t.is_empty())
        .filter_map(token_to_function)
        .collect()
}

/// Tokens that don't map to any [`PinFunction`] — surfaced as validation
/// errors so a typo (`uart1_tx`) is caught before Save.
pub fn unknown_function_tokens(s: &str) -> Vec<String> {
    s.split([' ', ',', '\t', '\n'])
        .filter(|t| !t.is_empty())
        .filter(|t| token_to_function(t).is_none())
        .map(str::to_string)
        .collect()
}

/// One token → one [`PinFunction`]. Case-insensitive; the inverse of
/// [`function_to_token`].
fn token_to_function(tok: &str) -> Option<PinFunction> {
    let t = tok.trim().to_ascii_lowercase();
    // `af:<name>` — a generic alternate function the IDE doesn't model
    // natively (SAI / FMC / DCMI / …). Explicit prefix so a TYPO still gets
    // flagged by `unknown_function_tokens` instead of silently becoming one.
    if let Some(name) = t.strip_prefix("af:") {
        let name = name.trim();
        return (!name.is_empty()).then(|| PinFunction::Other(name.to_ascii_uppercase()));
    }
    // Fixed tokens first.
    let simple = match t.as_str() {
        "in" | "gpioinput" => Some(PinFunction::GpioInput),
        "out" | "gpiooutput" => Some(PinFunction::GpioOutput),
        "analog" | "gpioanalog" => Some(PinFunction::GpioAnalog),
        "swdio" => Some(PinFunction::SwdIo),
        "swclk" => Some(PinFunction::SwdClk),
        "usb_dm" => Some(PinFunction::UsbDm),
        "usb_dp" => Some(PinFunction::UsbDp),
        "can_rx" => Some(PinFunction::CanRx),
        "can_tx" => Some(PinFunction::CanTx),
        "mco" => Some(PinFunction::Mco),
        "qspi_clk" => Some(PinFunction::QspiClk),
        _ => None,
    };
    if simple.is_some() {
        return simple;
    }
    // `xspi_p1_io12` → XSPI port 1 data line 12. Same shape as the OCTOSPI
    // tokens below, and lifted for the same reason.
    if let Some(rest) = t.strip_prefix("xspi_") {
        let (p, role) = rest.split_once('_')?;
        let port = p.strip_prefix('p')?.parse().ok()?;
        return match role {
            "clk" => Some(PinFunction::XspiClk { port }),
            _ => {
                if let Some(cs) = role.strip_prefix("ncs").and_then(|c| c.parse().ok()) {
                    return (cs == 1 || cs == 2).then_some(PinFunction::XspiNcs { port, cs });
                }
                if let Some(index) = role.strip_prefix("dqs").and_then(|c| c.parse().ok()) {
                    return (index < 2).then_some(PinFunction::XspiDqs { port, index });
                }
                role.strip_prefix("io")
                    .and_then(|l| l.parse().ok())
                    .filter(|l| *l < 16)
                    .map(|lane| PinFunction::XspiIo { port, lane })
            }
        };
    }
    // `ospi_p1_io3` → OCTOSPI port 1 data line 3. Lifted for the same reason as
    // the QUADSPI tokens below: what varies is the PORT, not an instance.
    if let Some(rest) = t.strip_prefix("ospi_") {
        let (p, role) = rest.split_once('_')?;
        let port = p.strip_prefix('p')?.parse().ok()?;
        return match role {
            "clk" => Some(PinFunction::OspiClk { port }),
            "ncs" => Some(PinFunction::OspiNcs { port }),
            "dqs" => Some(PinFunction::OspiDqs { port }),
            _ => role
                .strip_prefix("io")
                .and_then(|l| l.parse().ok())
                .filter(|l| *l < 8)
                .map(|lane| PinFunction::OspiIo { port, lane }),
        };
    }
    // `qspi_b1_io2` → QUADSPI bank 1 data line 2. Lifted ABOVE the instance
    // split below, which needs a peripheral number: the chip has at most one
    // QUADSPI, and what varies is the BANK.
    if let Some(rest) = t.strip_prefix("qspi_") {
        let (b, role) = rest.split_once('_')?;
        let bank = b.strip_prefix('b')?.parse().ok()?;
        return match role {
            "ncs" => Some(PinFunction::QspiNcs { bank }),
            _ => role
                .strip_prefix("io")
                .and_then(|l| l.parse().ok())
                .filter(|l| *l < 4)
                .map(|lane| PinFunction::QspiIo { bank, lane }),
        };
    }
    // Parameterized `<peripheral><n>_<role>` and `<kind><a>_<b>`.
    let (head, tail) = t.split_once('_')?;
    // The instance number is the head's TRAILING digit run — NOT the first
    // digit: `i2c1` has a `2` inside the peripheral word, so `usart1` → 1 and
    // `i2c1` → 1 both need the suffix, not `find`.
    let split = head.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (word, n_str) = head.split_at(split);
    let n: u8 = n_str.parse().ok()?;
    match word {
        "usart" => match tail {
            "tx" => Some(PinFunction::UsartTx(n)),
            "rx" => Some(PinFunction::UsartRx(n)),
            "cts" => Some(PinFunction::UsartCts(n)),
            // `rts_de` is the same physical pin as RTS (it doubles as the RS485
            // driver-enable), so both spellings land on RTS.
            "rts" | "rts_de" => Some(PinFunction::UsartRts(n)),
            "ck" => Some(PinFunction::UsartCk(n)),
            _ => None,
        },
        "lpuart" => match tail {
            "tx" => Some(PinFunction::LpuartTx(n)),
            "rx" => Some(PinFunction::LpuartRx(n)),
            "cts" => Some(PinFunction::LpuartCts(n)),
            "rts" | "rts_de" => Some(PinFunction::LpuartRts(n)),
            _ => None,
        },
        "spi" => match tail {
            "nss" => Some(PinFunction::SpiNss(n)),
            "sck" => Some(PinFunction::SpiSck(n)),
            "miso" => Some(PinFunction::SpiMiso(n)),
            "mosi" => Some(PinFunction::SpiMosi(n)),
            "rdy" => Some(PinFunction::SpiRdy(n)),
            _ => None,
        },
        // `sdmmc1_d3` → SDMMC1 data line 3; `sdmmc0_ck` is the un-numbered SDIO.
        // `hspi1_io3` → HSPI1 data line 3. Instance-numbered, so it goes
        // through the ordinary split — unlike the OCTOSPI's and XSPI's, which
        // are named after an IO-manager port.
        "hspi" => match tail {
            "clk" => Some(PinFunction::HspiClk { unit: n }),
            "ncs" => Some(PinFunction::HspiNcs { unit: n }),
            _ => {
                if let Some(index) = tail.strip_prefix("dqs").and_then(|c| c.parse().ok()) {
                    return (index < 2).then_some(PinFunction::HspiDqs { unit: n, index });
                }
                tail.strip_prefix("io")
                    .and_then(|l| l.parse().ok())
                    .filter(|l| *l < 16)
                    .map(|lane| PinFunction::HspiIo { unit: n, lane })
            }
        },
        "sdmmc" => match tail {
            "ck" => Some(PinFunction::SdmmcCk { unit: n }),
            "cmd" => Some(PinFunction::SdmmcCmd { unit: n }),
            _ => tail
                .strip_prefix('d')
                .and_then(|l| l.parse().ok())
                .filter(|l| *l < 8)
                .map(|lane| PinFunction::SdmmcD { unit: n, lane }),
        },
        // `sai1_a_sck` → SAI1 sub-block A bit clock.
        "sai" => {
            let (letter, role) = tail.split_once('_')?;
            let block = match letter {
                "a" => 1u8,
                "b" => 2,
                _ => return None,
            };
            let sai = n;
            match role {
                "sck" => Some(PinFunction::SaiSck { sai, block }),
                "sd" => Some(PinFunction::SaiSd { sai, block }),
                "fs" => Some(PinFunction::SaiFs { sai, block }),
                "mclk" => Some(PinFunction::SaiMclk { sai, block }),
                _ => None,
            }
        }
        // `dac1_out2` → DAC1 channel 2.
        "dac" => tail
            .strip_prefix("out")
            .and_then(|c| c.parse().ok())
            .map(|channel| PinFunction::DacOut { dac: n, channel }),
        "i2s" => match tail {
            "ck" => Some(PinFunction::I2sCk(n)),
            "ws" => Some(PinFunction::I2sWs(n)),
            "sd" => Some(PinFunction::I2sSd(n)),
            "mck" => Some(PinFunction::I2sMck(n)),
            _ => None,
        },
        "i2c" => match tail {
            "scl" => Some(PinFunction::I2cScl(n)),
            "sda" => Some(PinFunction::I2cSda(n)),
            _ => None,
        },
        // `adc1_5` → ADC1 channel 5; `tim2_1` → TIM2 CH1.
        "adc" => tail.parse().ok().map(|ch| PinFunction::AdcChannel {
            adc: n,
            channel: ch,
        }),
        // `tim1_2` → TIM1 CH2; the `n` suffix is the complementary output,
        // `tim1_2n` → TIM1 CH2N; `tim1_bkin1` / `tim1_bkin2` are the break inputs.
        "tim" if tail.starts_with("bkin") => tail
            .strip_prefix("bkin")
            .and_then(|i| i.parse::<u8>().ok())
            .filter(|i| *i == 1 || *i == 2)
            .map(|input| PinFunction::TimerBreak { timer: n, input }),
        "tim" => match tail.strip_suffix('n') {
            Some(ch) => ch.parse().ok().map(|ch| PinFunction::TimerPwmN {
                timer: n,
                channel: ch,
            }),
            None => tail.parse().ok().map(|ch| PinFunction::TimerPwm {
                timer: n,
                channel: ch,
            }),
        },
        _ => None,
    }
}

/// The lowercase sub-block letter used inside a SAI token.
fn sai_letter(block: u8) -> &'static str {
    if block == 1 { "a" } else { "b" }
}

/// One [`PinFunction`] → its canonical token (inverse of [`token_to_function`]).
fn function_to_token(f: &PinFunction) -> Option<String> {
    Some(match f {
        PinFunction::Unset => return None,
        PinFunction::GpioInput => "in".into(),
        PinFunction::GpioOutput => "out".into(),
        PinFunction::GpioAnalog => "analog".into(),
        PinFunction::SwdIo => "swdio".into(),
        PinFunction::SwdClk => "swclk".into(),
        PinFunction::UsbDm => "usb_dm".into(),
        PinFunction::UsbDp => "usb_dp".into(),
        PinFunction::CanRx => "can_rx".into(),
        PinFunction::CanTx => "can_tx".into(),
        PinFunction::Mco => "mco".into(),
        PinFunction::UsartTx(n) => format!("usart{n}_tx"),
        PinFunction::UsartRx(n) => format!("usart{n}_rx"),
        PinFunction::UsartCts(n) => format!("usart{n}_cts"),
        PinFunction::UsartRts(n) => format!("usart{n}_rts"),
        PinFunction::UsartCk(n) => format!("usart{n}_ck"),
        PinFunction::LpuartTx(n) => format!("lpuart{n}_tx"),
        PinFunction::LpuartRx(n) => format!("lpuart{n}_rx"),
        PinFunction::LpuartCts(n) => format!("lpuart{n}_cts"),
        PinFunction::LpuartRts(n) => format!("lpuart{n}_rts"),
        PinFunction::SpiNss(n) => format!("spi{n}_nss"),
        PinFunction::SpiSck(n) => format!("spi{n}_sck"),
        PinFunction::SpiMiso(n) => format!("spi{n}_miso"),
        PinFunction::SpiMosi(n) => format!("spi{n}_mosi"),
        PinFunction::SpiRdy(n) => format!("spi{n}_rdy"),
        PinFunction::DacOut { dac, channel } => format!("dac{dac}_out{channel}"),
        PinFunction::HspiClk { unit } => format!("hspi{unit}_clk"),
        PinFunction::HspiNcs { unit } => format!("hspi{unit}_ncs"),
        PinFunction::HspiDqs { unit, index } => format!("hspi{unit}_dqs{index}"),
        PinFunction::HspiIo { unit, lane } => format!("hspi{unit}_io{lane}"),
        PinFunction::XspiClk { port } => format!("xspi_p{port}_clk"),
        PinFunction::XspiNcs { port, cs } => format!("xspi_p{port}_ncs{cs}"),
        PinFunction::XspiDqs { port, index } => format!("xspi_p{port}_dqs{index}"),
        PinFunction::XspiIo { port, lane } => format!("xspi_p{port}_io{lane}"),
        PinFunction::OspiClk { port } => format!("ospi_p{port}_clk"),
        PinFunction::OspiNcs { port } => format!("ospi_p{port}_ncs"),
        PinFunction::OspiDqs { port } => format!("ospi_p{port}_dqs"),
        PinFunction::OspiIo { port, lane } => format!("ospi_p{port}_io{lane}"),
        PinFunction::QspiClk => "qspi_clk".into(),
        PinFunction::QspiNcs { bank } => format!("qspi_b{bank}_ncs"),
        PinFunction::QspiIo { bank, lane } => format!("qspi_b{bank}_io{lane}"),
        PinFunction::SdmmcCk { unit } => format!("sdmmc{unit}_ck"),
        PinFunction::SdmmcCmd { unit } => format!("sdmmc{unit}_cmd"),
        PinFunction::SdmmcD { unit, lane } => format!("sdmmc{unit}_d{lane}"),
        PinFunction::SaiSck { sai, block } => format!("sai{sai}_{}_sck", sai_letter(*block)),
        PinFunction::SaiSd { sai, block } => format!("sai{sai}_{}_sd", sai_letter(*block)),
        PinFunction::SaiFs { sai, block } => format!("sai{sai}_{}_fs", sai_letter(*block)),
        PinFunction::SaiMclk { sai, block } => format!("sai{sai}_{}_mclk", sai_letter(*block)),
        PinFunction::I2sCk(n) => format!("i2s{n}_ck"),
        PinFunction::I2sWs(n) => format!("i2s{n}_ws"),
        PinFunction::I2sSd(n) => format!("i2s{n}_sd"),
        PinFunction::I2sMck(n) => format!("i2s{n}_mck"),
        PinFunction::I2cScl(n) => format!("i2c{n}_scl"),
        PinFunction::I2cSda(n) => format!("i2c{n}_sda"),
        PinFunction::AdcChannel { adc, channel } => format!("adc{adc}_{channel}"),
        PinFunction::TimerPwm { timer, channel } => format!("tim{timer}_{channel}"),
        PinFunction::TimerPwmN { timer, channel } => format!("tim{timer}_{channel}n"),
        PinFunction::TimerBreak { timer, input } => format!("tim{timer}_bkin{input}"),
        PinFunction::Other(name) => format!("af:{}", name.to_ascii_lowercase()),
    })
}

/// Format a function list back into an editable token string (space-joined).
pub fn functions_to_string(fns: &[PinFunction]) -> String {
    fns.iter()
        .filter_map(function_to_token)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::builtins::builtin_for;

    #[test]
    fn ld_number_parses_hex_dec_and_suffixes() {
        assert_eq!(parse_ld_number("0x08000000"), Some(0x0800_0000));
        assert_eq!(parse_ld_number("64K"), Some(64 * 1024));
        assert_eq!(parse_ld_number("1M"), Some(1024 * 1024));
        assert_eq!(parse_ld_number("131072"), Some(131072));
        assert_eq!(parse_ld_number(""), None);
        assert_eq!(parse_ld_number("garbage"), None);
    }

    #[test]
    fn id_validation() {
        assert!(is_valid_id("stm32f103rb"));
        assert!(is_valid_id("esp32_c6"));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("STM32")); // uppercase
        assert!(!is_valid_id("a b")); // space
        assert!(!is_valid_id("a-b")); // dash
    }

    #[test]
    fn blank_form_reports_the_missing_essentials() {
        let f = McuForm::blank();
        let errs = f.errors();
        // id, display_name, probe_chip empty on a blank ARM form.
        assert!(errs.iter().any(|e| e.contains("Id must")));
        assert!(errs.iter().any(|e| e.contains("Display name")));
        assert!(errs.iter().any(|e| e.contains("Probe chip")));
    }

    #[test]
    fn a_minimal_valid_form_builds_and_round_trips() {
        let mut f = McuForm::blank();
        f.id = "stm32f103rb".into();
        f.display_name = "STM32F103RB".into();
        f.probe_chip = "STM32F103RB".into();
        f.pins[2] = gpio_bank("PA", 1, 4);
        assert!(f.errors().is_empty(), "{:?}", f.errors());

        let def = f.to_definition();
        assert_eq!(def.id, "stm32f103rb");
        assert_eq!(def.pins.left.len(), 4);
        assert_eq!(def.pins.left[0].name, "PA0");
        // Builds into a runtime Mcu.
        assert!(def.build_mcu().iter_all_pins().count() >= 4);

        // The RON it writes parses back to an equal definition.
        let ron = f.to_ron().unwrap();
        let parsed: McuDefinition = ron::from_str(&ron).unwrap();
        assert_eq!(parsed, def);
    }

    #[test]
    fn duplicate_and_bad_pin_numbers_are_errors() {
        let mut f = McuForm::blank();
        f.id = "x".into();
        f.display_name = "X".into();
        f.probe_chip = "X".into();
        f.pins[0] = vec![
            PinRow {
                number: "1".into(),
                name: "PA0".into(),
                ..Default::default()
            },
            PinRow {
                number: "1".into(),
                name: "PA1".into(),
                ..Default::default()
            },
            PinRow {
                number: "abc".into(),
                name: "PA2".into(),
                ..Default::default()
            },
        ];
        let errs = f.errors();
        assert!(errs.iter().any(|e| e.contains("used more than once")));
        assert!(errs.iter().any(|e| e.contains("invalid number")));
    }

    #[test]
    fn function_tokens_round_trip_and_reject_typos() {
        let src = "in out usart1_tx spi2_sck i2c1_scl adc1_5 tim2_1 swdio usb_dp can_tx mco";
        let fns = parse_functions(src);
        assert_eq!(fns.len(), 11);
        assert_eq!(fns[2], PinFunction::UsartTx(1));
        assert_eq!(fns[5], PinFunction::AdcChannel { adc: 1, channel: 5 });
        assert_eq!(
            fns[6],
            PinFunction::TimerPwm {
                timer: 2,
                channel: 1
            }
        );
        // Round-trips through the canonical string form.
        assert_eq!(parse_functions(&functions_to_string(&fns)), fns);
        // Commas and case are accepted; a typo is reported, others still parse.
        assert_eq!(parse_functions("IN, USART2_RX").len(), 2);
        assert_eq!(
            unknown_function_tokens("in uart1_tx spi9_bad out"),
            vec!["uart1_tx", "spi9_bad"]
        );
        // A bad token on a pin row surfaces as a validation error.
        let mut f = McuForm::blank();
        f.id = "x".into();
        f.display_name = "X".into();
        f.probe_chip = "X".into();
        f.pins[0] = vec![PinRow {
            number: "1".into(),
            name: "PA0".into(),
            reserved: false,
            functions: "in wat".into(),
            imported: false,
            af: Vec::new(),
            fn_owner: Vec::new(),
        }];
        assert!(
            f.errors()
                .iter()
                .any(|e| e.contains("unknown function token 'wat'"))
        );
    }

    #[test]
    fn esp_form_skips_memory_and_probe_requirements() {
        let mut f = McuForm::blank();
        f.id = "esp32c6".into();
        f.display_name = "ESP32-C6".into();
        f.family = "esp32c3".into(); // reuse the backed family for the test
        f.toolchain = ToolchainKind::EspRust;
        f.target = "riscv32imac-unknown-none-elf".into();
        f.clock = ClockChoice::Esp32c3;
        // No probe_chip, no memory — must still be valid for ESP.
        f.probe_chip.clear();
        f.flash_origin.clear();
        assert!(f.errors().is_empty(), "{:?}", f.errors());
    }

    #[test]
    fn from_definition_seeds_every_field() {
        let def = builtin_for("stm32f103c8t6").unwrap();
        let f = McuForm::from_definition(&def);
        assert_eq!(f.id, def.id);
        assert!(f.editing);
        // Round-trip through the form preserves the definition (clock graphs
        // collapse to their choice, but stm32f1 defaults reconstruct equal).
        let rebuilt = f.to_definition();
        assert_eq!(rebuilt.id, def.id);
        assert_eq!(rebuilt.pins, def.pins);
        assert_eq!(rebuilt.project, def.project);
    }

    /// The WBA clock choice: the def carries the WBA graph + the WBA ceilings,
    /// Edit detects it back, and a FOREIGN imported graph survives Edit→Save.
    #[test]
    fn wba_clock_choice_round_trips_and_foreign_graphs_survive() {
        use crate::panels::mcu_module::clock::graph::{GraphClock, is_wba_graph};

        let mut f = McuForm::blank();
        f.id = "stm32wba55cg".into();
        f.display_name = "STM32WBA55CG".into();
        f.family = "stm32wba".into();
        f.probe_chip = "STM32WBA55CGUx".into();
        f.target = "thumbv8m.main-none-eabihf".into();
        f.clock = ClockChoice::Stm32wba;
        let def = f.to_definition();
        match &def.clock {
            ClockDef::Graph(gc) => assert!(is_wba_graph(&gc.graph)),
            other => panic!("expected WBA graph, got {other:?}"),
        }
        assert_eq!(def.clock_limits.sysclk_max, 100_000_000);
        // Edit path detects the WBA tree again.
        assert_eq!(McuForm::from_definition(&def).clock, ClockChoice::Stm32wba);

        // A hand-imported foreign graph: choice shows None but the graph is
        // preserved verbatim through Edit → Save.
        let mut imported = def.clone();
        imported.clock = ClockDef::Graph(GraphClock {
            graph: crate::panels::mcu_module::clock::graph::ClockGraph {
                nodes: Vec::new(),
                edges: Vec::new(),
            },
            layout: Default::default(),
            bindings: Default::default(),
        }); // an empty graph — not the WBA tree
        let edited = McuForm::from_definition(&imported);
        assert_eq!(edited.clock, ClockChoice::None);
        assert_eq!(edited.to_definition().clock, imported.clock);
    }

    /// The STM32F4 clock choice drives the whole chain: the def carries the F4
    /// graph, the generated main.rs carries the embassy F4 RCC config, and Edit
    /// detects the choice back.
    #[test]
    fn stm32f4_clock_choice_reaches_generated_code() {
        use crate::panels::mcu_module::clock::graph::is_f4_graph;

        let mut f = McuForm::blank();
        f.id = "stm32f411re".into();
        f.display_name = "STM32F411RE".into();
        f.family = "stm32f4".into();
        f.probe_chip = "STM32F411RE".into();
        f.target = "thumbv7em-none-eabihf".into();
        f.clock = ClockChoice::Stm32f4;

        let def = f.to_definition();
        match &def.clock {
            ClockDef::Graph(gc) => assert!(is_f4_graph(&gc.graph)),
            other => panic!("expected F4 graph, got {other:?}"),
        }
        assert_eq!(def.clock_limits.sysclk_max, 100_000_000);

        // Full chain: build the Mcu and generate main.rs.
        let code = def.build_mcu().fresh_main_rs();
        assert!(
            code.contains("config.rcc.sys = rcc::Sysclk::PLL1_P;"),
            "{code}"
        );
        assert!(code.contains("SYSCLK 100 MHz"), "{code}");
        // Edit round-trips the choice.
        assert_eq!(McuForm::from_definition(&def).clock, ClockChoice::Stm32f4);
    }

    /// LPUART / SPI-RDY are first-class tokens now, and `rts_de` is accepted as
    /// a spelling of `rts` (same physical pin — RS485 driver enable).
    #[test]
    fn lpuart_spi_rdy_and_rts_de_tokens() {
        let fns = parse_functions("lpuart1_tx lpuart1_rx lpuart2_cts lpuart1_rts spi3_rdy");
        assert_eq!(
            fns,
            vec![
                PinFunction::LpuartTx(1),
                PinFunction::LpuartRx(1),
                PinFunction::LpuartCts(2),
                PinFunction::LpuartRts(1),
                PinFunction::SpiRdy(3),
            ]
        );
        // Canonical round-trip.
        assert_eq!(parse_functions(&functions_to_string(&fns)), fns);
        // `rts_de` is an accepted alias for `rts` on both peripherals.
        assert_eq!(
            parse_functions("usart2_rts_de"),
            vec![PinFunction::UsartRts(2)]
        );
        assert_eq!(
            parse_functions("lpuart1_rts_de"),
            vec![PinFunction::LpuartRts(1)]
        );
        // None of them are flagged as unknown any more.
        assert!(
            unknown_function_tokens("lpuart1_tx lpuart1_rts_de spi1_rdy usart2_rts_de").is_empty()
        );
        // The cheatsheet advertises them.
        assert!(FUNCTION_TOKEN_HELP.contains("lpuart"));
        assert!(FUNCTION_TOKEN_HELP.contains("rdy"));
    }

    /// `af:<signal>` carries anything the IDE doesn't model natively, so an
    /// import never loses a pin function — while typos are STILL flagged
    /// (that's why the prefix is explicit rather than a catch-all).
    #[test]
    fn generic_af_tokens_round_trip_and_typos_still_flagged() {
        let fns = parse_functions("in out af:sai1_sd_a af:fmc_a0 af:tim1_ch1n");
        assert_eq!(
            fns,
            vec![
                PinFunction::GpioInput,
                PinFunction::GpioOutput,
                PinFunction::Other("SAI1_SD_A".into()),
                PinFunction::Other("FMC_A0".into()),
                PinFunction::Other("TIM1_CH1N".into()),
            ]
        );
        // Canonical round-trip through the token string.
        assert_eq!(
            functions_to_string(&fns),
            "in out af:sai1_sd_a af:fmc_a0 af:tim1_ch1n"
        );
        assert_eq!(parse_functions(&functions_to_string(&fns)), fns);
        // Generic AFs are never "unknown"…
        assert!(unknown_function_tokens("af:dcmi_d3 af:eth_mdio").is_empty());
        // …but a real typo still is (the prefix keeps validation honest).
        assert_eq!(
            unknown_function_tokens("uart1_tx spi9_bad af:"),
            vec!["uart1_tx", "spi9_bad", "af:"]
        );
        // The label round-trips through codegen comments too.
        let f = PinFunction::Other("SAI1_SD_A".into());
        assert_eq!(f.label(), "SAI1_SD_A");
        assert_eq!(PinFunction::from_label(&f.label()), Some(f));
    }

    /// Moving a pin across sides and positioning it within a side — the pin
    /// keeps its number (the side is layout, not identity).
    #[test]
    fn move_and_reorder_pins() {
        let mut f = McuForm::blank();
        // pins = [top, bottom, left, right]
        f.pins[1] = gpio_bank("PB", 1, 3); // bottom: PB0 PB1 PB2
        f.pins[3] = gpio_bank("PC", 10, 1); // right:  PC0

        // Bottom → Right (the user's case): appended at the end, number kept.
        assert!(f.move_pin(1, 1, 3)); // PB1
        assert_eq!(names(&f.pins[1]), vec!["PB0", "PB2"]);
        assert_eq!(names(&f.pins[3]), vec!["PC0", "PB1"]);
        assert_eq!(f.pins[3][1].number, "2", "package number is untouched");

        // Position it within the side.
        assert!(f.reorder_pin(3, 1, -1));
        assert_eq!(names(&f.pins[3]), vec!["PB1", "PC0"]);
        // Clamped at the ends — no wrap-around, no panic.
        assert!(!f.reorder_pin(3, 0, -1));
        assert!(!f.reorder_pin(3, 1, 1));
        assert_eq!(names(&f.pins[3]), vec!["PB1", "PC0"]);

        // Guards: same side, out-of-range index, out-of-range side.
        assert!(!f.move_pin(1, 0, 1));
        assert!(!f.move_pin(1, 99, 3));
        assert!(!f.move_pin(1, 0, 9));
        assert!(!f.reorder_pin(9, 0, 1));
        assert!(!f.reorder_pin(1, 99, 1));
    }

    fn names(rows: &[PinRow]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    /// A chip's family picks its clock tree, and that tree round-trips to a
    /// graph whose codegen the family dispatch recognises. Guards the "imported
    /// chip gets a working clock automatically" path (recommendation b).
    #[test]
    fn for_family_maps_to_a_dispatchable_clock_tree() {
        use crate::panels::mcu_module::clock::graph::{is_g0_graph, is_g4_graph};
        use crate::panels::mcu_module::mcu_def::ClockDef;

        assert_eq!(ClockChoice::for_family("stm32g4"), ClockChoice::Stm32g4);
        assert_eq!(ClockChoice::for_family("stm32g0"), ClockChoice::Stm32g0);
        // The f247 families share one TOPOLOGY, but the F2 gets its own choice:
        // its PLLN window and clock ceilings differ, and mapping it onto the F4
        // tree is what generated an uncompilable `PllMul::MUL144`.
        assert_eq!(ClockChoice::for_family("stm32f7"), ClockChoice::Stm32f4);
        assert_eq!(ClockChoice::for_family("stm32f2"), ClockChoice::Stm32f2);
        // A family with no tree yet → None (reset-default clock, still compiles).
        assert_eq!(ClockChoice::for_family("stm32h7"), ClockChoice::None);

        // End-to-end: a G4 choice builds a graph the codegen recognises as G4.
        match ClockChoice::Stm32g4.to_def() {
            ClockDef::Graph(gc) => assert!(is_g4_graph(&gc.graph) && !is_g0_graph(&gc.graph)),
            _ => panic!("G4 choice must build a graph clock"),
        }
    }

    /// The GUI order must be a real permutation of the four sides — a typo
    /// would silently hide one editor and show another twice.
    #[test]
    fn side_display_order_is_left_bottom_right_top() {
        let shown: Vec<&str> = SIDE_DISPLAY_ORDER.iter().map(|&i| SIDES[i]).collect();
        assert_eq!(shown, vec!["Left", "Bottom", "Right", "Top"]);
        let mut sorted = SIDE_DISPLAY_ORDER;
        sorted.sort_unstable();
        assert_eq!(sorted, [0, 1, 2, 3], "must cover every side exactly once");
    }

    #[test]
    fn auto_fill_identity_from_name_sets_family_cpu_target() {
        let mut f = McuForm::blank();
        f.display_name = "STM32WBA55CG".into();
        f.family.clear();
        f.cpu.clear();
        f.target.clear();
        f.probe_chip.clear();
        assert!(f.auto_fill_identity());
        assert_eq!(f.family, "stm32wba");
        assert_eq!(f.cpu, "Cortex-M33");
        assert_eq!(f.target, "thumbv8m.main-none-eabihf");
        assert_eq!(f.toolchain, ToolchainKind::RustEmbedded);
        assert_eq!(f.probe_chip, "STM32WBA55CG"); // seeded because it was empty
        // An unrecognised name changes nothing.
        let mut g = McuForm::blank();
        g.display_name = "ESP32-C3".into();
        assert!(!g.auto_fill_identity());
    }

    #[test]
    fn empty_package_warns() {
        let mut f = McuForm::blank();
        f.package.clear();
        assert!(f.warnings().iter().any(|w| w.contains("Package is empty")));
        f.package = "UFQFPN48".into();
        assert!(!f.warnings().iter().any(|w| w.contains("Package is empty")));
    }

    #[test]
    fn unknown_family_warns_but_does_not_block() {
        let mut f = McuForm::blank();
        f.id = "rp2040".into();
        f.display_name = "RP2040".into();
        f.probe_chip = "RP2040".into();
        f.family = "rp2040".into();
        assert!(f.errors().is_empty());
        assert!(
            f.warnings()
                .iter()
                .any(|w| w.contains("no codegen backend"))
        );
    }
}
