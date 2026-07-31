//! MCU business logic — partner assignment, state management, pin lookups.

use super::model::Mcu;
use crate::panels::mcu_module::pins::logic::pin::Pin;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

// ── Peripheral pin groups ─────────────────────────────────────────────────────
// Defines which functions must be co-selected / co-deselected as a group.
// Selecting any member of a group auto-assigns the rest to the nearest
// available Unset pin; deselecting one member removes the whole group.

pub fn partner_functions(func: &PinFunction) -> Vec<PinFunction> {
    match func {
        // USART — basic full-duplex pair
        PinFunction::UsartTx(n)  => vec![PinFunction::UsartRx(*n)],
        PinFunction::UsartRx(n)  => vec![PinFunction::UsartTx(*n)],
        // USART — hardware flow-control pair (optional, separate from TX/RX)
        PinFunction::UsartCts(n) => vec![PinFunction::UsartRts(*n)],
        PinFunction::UsartRts(n) => vec![PinFunction::UsartCts(*n)],
        // SPI — three-wire bus (NSS is optional, not auto-assigned)
        PinFunction::SpiSck(n)  => vec![PinFunction::SpiMiso(*n), PinFunction::SpiMosi(*n)],
        PinFunction::SpiMiso(n) => vec![PinFunction::SpiSck(*n),  PinFunction::SpiMosi(*n)],
        PinFunction::SpiMosi(n) => vec![PinFunction::SpiSck(*n),  PinFunction::SpiMiso(*n)],
        // I²C — two-wire bus
        PinFunction::I2cScl(n) => vec![PinFunction::I2cSda(*n)],
        PinFunction::I2cSda(n) => vec![PinFunction::I2cScl(*n)],
        // CAN — differential pair
        PinFunction::CanRx => vec![PinFunction::CanTx],
        PinFunction::CanTx => vec![PinFunction::CanRx],
        // USB — differential pair
        PinFunction::UsbDm => vec![PinFunction::UsbDp],
        PinFunction::UsbDp => vec![PinFunction::UsbDm],
        // SWD — two-wire debug
        PinFunction::SwdIo  => vec![PinFunction::SwdClk],
        PinFunction::SwdClk => vec![PinFunction::SwdIo],
        // GPIO, ADC, Timer, MCO, SpiNss, UsartCk — no automatic partners
        _ => vec![],
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

impl Mcu {
    /// Create a new MCU with the given configuration.
    ///
    /// `family` is the codegen backend key (e.g. "stm32f1", "esp32c3"); see
    /// [`FamilyBackend`](crate::panels::mcu_module::codegen::family::FamilyBackend).
    pub fn new(
        name: String,
        family: String,
        toolchain: crate::panels::mcu_module::mcu_catalog::ToolchainKind,
        top_pins: Vec<Pin>,
        bottom_pins: Vec<Pin>,
        left_pins: Vec<Pin>,
        right_pins: Vec<Pin>,
    ) -> Self {
        use crate::panels::mcu_module::clock::graph::{
            layout::stm32f1_layout, stm32f1_graph, GraphClock,
        };
        use crate::panels::mcu_module::clock::{ClockConfig, ClockLimits, Stm32f1Clock};
        // Only the STM32F1 family has a built-in clock graph; others get `None`
        // (a definition's `ClockDef` overrides this in `build_mcu`).
        let clock = match family.as_str() {
            "stm32f1" => ClockConfig::Graph(GraphClock {
                graph: stm32f1_graph(&Stm32f1Clock::default()),
                layout: stm32f1_layout(&ClockLimits::default()),
            }),
            _ => ClockConfig::None,
        };
        Self {
            id: String::new(),
            name,
            family,
            toolchain,
            top_pins,
            bottom_pins,
            left_pins,
            right_pins,
            selected_pin: None,
            show_info: None,
            fn_scroll_offset: 0.0,
            clock,
            clock_limits: ClockLimits::default(),
            clock_presets: Vec::new(),
            modules: Vec::new(),
            runtime: crate::panels::mcu_module::mcu::model::Runtime::default(),
            gpio_api: crate::panels::mcu_module::modules::ApiStyle::default(),
            pending_runtime: crate::panels::mcu_module::mcu::model::Runtime::default(),
            pending_gpio_api: crate::panels::mcu_module::modules::ApiStyle::default(),
            pending_module_styles: std::collections::BTreeMap::new(),
            pending_apply_confirm: false,
            auto_build: crate::panels::mcu_module::mcu::model::AutoBuild::default(),
            expand_module: None,
        }
    }

    // ── Virtual modules ───────────────────────────────────────────────────────

    /// Does this CHIP have the pins to host `kind` at all? A dry run of the
    /// real auto-wiring against a pristine chip (nothing wired yet), so it
    /// answers with exactly the logic [`add_module`](Self::add_module) uses —
    /// including the subtle part, that one peripheral INSTANCE must offer all
    /// the required signals (it isn't enough that some pin can TX and some
    /// unrelated pin can RX).
    ///
    /// Static: independent of what's currently wired. Use it to hide kinds the
    /// chip simply doesn't have.
    pub fn supports_module(&self, kind: crate::panels::mcu_module::modules::ModuleKind) -> bool {
        use crate::panels::mcu_module::modules::autowire;
        let (required, optional) = kind.signals();
        autowire::pick_pins(self, &Default::default(), &Default::default(), required, optional)
            .is_some()
    }

    /// Could another `kind` be added RIGHT NOW — i.e. are there still free pins
    /// and a free instance? Dynamic: changes as modules/pins are edited. A kind
    /// that is [`supports_module`](Self::supports_module) but not this is
    /// *exhausted*, which the palette shows disabled-with-a-reason rather than
    /// hiding (a button that silently vanishes is worse than one that explains).
    pub fn can_add_module(&self, kind: crate::panels::mcu_module::modules::ModuleKind) -> bool {
        use crate::panels::mcu_module::modules::autowire;
        if kind.is_single_instance() && self.modules.iter().any(|m| m.kind == kind) {
            return false;
        }
        let (required, optional) = kind.signals();
        let used: std::collections::HashSet<usize> = self
            .modules
            .iter()
            .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
            .collect();
        let used_instances: std::collections::HashSet<u8> = self
            .modules
            .iter()
            .filter(|m| m.kind == kind)
            .map(|m| m.instance())
            .collect();
        autowire::pick_pins(self, &used, &used_instances, required, optional).is_some()
    }

    /// Add a virtual module (GI_USART / GI_SPI / GI_I2C) and auto-wire it to
    /// compatible MCU pins, setting those pins' functions. Returns `false` (and
    /// adds nothing) when the chip has no free pins for the module's interface.
    pub fn add_module(
        &mut self,
        kind: crate::panels::mcu_module::modules::ModuleKind,
    ) -> bool {
        use crate::panels::mcu_module::modules::autowire;

        // CAN and USB are single-instance and their pin functions carry no
        // index, so the instance-exclusion guard below can't stop a 2nd module
        // from grabbing the alternate pins — refuse it here.
        if kind.is_single_instance() && self.modules.iter().any(|m| m.kind == kind) {
            return false;
        }

        let (required, optional) = kind.signals();

        // Pins already wired to an existing module are off-limits.
        let used: std::collections::HashSet<usize> = self
            .modules
            .iter()
            .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
            .collect();
        // Peripheral instances already hosting a module of THIS kind are off-limits
        // too — so a 2nd "+GI_SPI" advances to SPI2 instead of re-picking SPI1 on
        // its alternate pin set (which `reconcile_modules` would then merge in).
        let used_instances: std::collections::HashSet<u8> = self
            .modules
            .iter()
            .filter(|m| m.kind == kind)
            .map(|m| m.instance())
            .collect();

        let Some((inst, chosen)) =
            autowire::pick_pins(self, &used, &used_instances, required, optional)
        else {
            return false;
        };

        // Assign the picked pins; the module itself is created (with default
        // config) by `reconcile_modules`, the single source of truth that mirrors
        // pin assignments — so the palette and the Peripherals tab behave the same.
        for (sig, pin) in &chosen {
            if let Some(p) = self.find_pin_mut(*pin) {
                p.selected_function = sig.pin_function(inst);
            }
        }
        self.reconcile_modules();
        true
    }

    /// Remove a module by id, resetting the pins it was wired to back to `Unset`
    /// (so removing a GI_USART/SPI/I2C frees its pins, mirroring "unplugging" the
    /// device).
    pub fn remove_module(&mut self, id: &str) {
        let pins: Vec<usize> = self
            .modules
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.connections.iter().map(|c| c.mcu_pin).collect())
            .unwrap_or_default();
        for pin in pins {
            if let Some(p) = self.find_pin_mut(pin) {
                p.selected_function = PinFunction::Unset;
            }
        }
        self.modules.retain(|m| m.id != id);
    }

    /// Returns `(number, name, selected_function)` for every non-reserved pin.
    /// Used by the IDE to sync the `pins/` source-file directory.
    pub fn all_pin_functions(&self) -> Vec<(usize, String, PinFunction)> {
        self.iter_all_pins()
            .filter(|p| !p.reserved)
            .map(|p| (p.number, p.name.clone(), p.selected_function.clone()))
            .collect()
    }

    /// Restores pin assignments parsed from `src/main.rs` by
    /// `codegen::parse_main_rs()`.
    ///
    /// - Resets all pins to `Unset` first (clean slate).
    /// - Sets each named pin to the given `PinFunction`.
    /// - Pins not found in this MCU layout (wrong name) are silently skipped.
    /// - Reserved pins are never overwritten.
    /// - Does NOT trigger auto-partner assignment — the saved state already
    ///   contains every pin individually.
    pub fn apply_saved_pins(&mut self, pins: &[(String, PinFunction)]) {
        self.reset_all_pins();
        for (name, func) in pins {
            let num = self
                .iter_all_pins()
                .find(|p| p.name == *name && !p.reserved)
                .map(|p| p.number);
            if let Some(num) = num {
                if let Some(pin) = self.find_pin_mut(num) {
                    pin.selected_function = func.clone();
                }
            }
        }
    }

    /// Restores the per-pin user labels parsed from a saved `src/main.rs` by
    /// `codegen::parse_pin_labels()` (the `_<label>` suffix on a binding name).
    /// Apply this *after* [`apply_saved_pins`], since clearing a pin to `Unset`
    /// drops its label. Pins not in this layout (wrong name) are skipped.
    pub fn apply_saved_pin_labels(&mut self, labels: &[(String, String)]) {
        for (name, label) in labels {
            let num = self
                .iter_all_pins()
                .find(|p| p.name == *name && !p.reserved)
                .map(|p| p.number);
            if let Some(num) = num {
                if let Some(pin) = self.find_pin_mut(num) {
                    pin.custom_label = label.clone();
                }
            }
        }
    }

    /// The `mcu.config` text for this chip — its virtual modules (`@modules`)
    /// and, for the STM32F1 family, the clock-tree config (`@clock`). Written to
    /// the project root on save; empty when there is nothing to persist.
    pub fn mcu_config_text(&self) -> String {
        use crate::panels::mcu_module::clock::graph::graph_to_stm32f1;
        use crate::panels::mcu_module::clock::{ClockConfig, Stm32f1Clock};
        use crate::panels::mcu_module::mcu_config;
        let clock = if self.family == "stm32f1" {
            Some(match &self.clock {
                ClockConfig::Graph(gc) => graph_to_stm32f1(&gc.graph),
                _ => Stm32f1Clock::default(),
            })
        } else {
            None
        };
        let mut s =
            mcu_config::serialize(&self.modules, clock.as_ref(), self.runtime, self.gpio_api);
        // Auto-build preference lives in its own `@autobuild` section (workflow
        // setting, not codegen config), appended here.
        let ab = mcu_config::autobuild_section(self.auto_build);
        if !ab.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&ab);
        }
        s
    }

    /// Restore virtual modules + clock from an `mcu.config` file on project open.
    /// Apply *after* `apply_saved_pins` (which derives default modules from the
    /// pins) so the saved per-module config (labels, baud, …) wins.
    pub fn apply_mcu_config(&mut self, text: &str) {
        use crate::panels::mcu_module::mcu_config;
        let (modules, clock) = mcu_config::parse(text);
        if !modules.is_empty() {
            self.modules = modules;
        }
        if let Some(c) = clock {
            self.apply_saved_clock(c);
        }
        // Runtime lives in its own `@runtime` section; a missing one (any
        // pre-async project) restores the default Blocking.
        self.runtime = mcu_config::parse_runtime(text);
        // GPIO api (`@gpio`) — missing restores the default Portable (io.rs bridge).
        self.gpio_api = mcu_config::parse_gpio_api(text);
        // Auto-build preference (`@autobuild`) — missing restores the default Check.
        self.auto_build = mcu_config::parse_autobuild(text);
        // A freshly loaded project has NO staged edits: pending == applied.
        self.sync_pending_style();
    }

    // ── Staged codegen-style choices (System-tab "Apply") ─────────────────────

    /// Reset the staged (`pending_*`) choices to the currently APPLIED ones — so
    /// nothing shows as dirty. Called on project load and right after an Apply.
    pub fn sync_pending_style(&mut self) {
        self.pending_runtime = self.runtime;
        self.pending_gpio_api = self.gpio_api;
        self.pending_module_styles = self
            .modules
            .iter()
            .map(|m| (m.id.clone(), module_style(&m.config)))
            .collect();
        self.pending_apply_confirm = false;
    }

    /// Commit the staged choices into the applied fields — the runtime, GPIO api
    /// and each module's `api_style`/`async_mode`. This changes the state hash, so
    /// the next `init_frame` regenerates `main.rs`, the config files and the deps.
    pub fn apply_pending_style(&mut self) {
        self.runtime = self.pending_runtime;
        self.gpio_api = self.pending_gpio_api;
        let pending = std::mem::take(&mut self.pending_module_styles);
        for m in &mut self.modules {
            if let Some(&(api, async_mode)) = pending.get(&m.id) {
                set_module_style(&mut m.config, api, async_mode);
            }
        }
        self.pending_module_styles = pending;
        self.pending_apply_confirm = false;
    }

    /// Whether any staged choice differs from the applied one (drives the Apply
    /// button's enabled state).
    pub fn style_dirty(&self) -> bool {
        if self.pending_runtime != self.runtime || self.pending_gpio_api != self.gpio_api {
            return true;
        }
        self.modules.iter().any(|m| {
            self.pending_module_styles
                .get(&m.id)
                .is_some_and(|&staged| staged != module_style(&m.config))
        })
    }

    /// Human-readable lines of what an Apply would change (for the confirm
    /// prompt). Empty when nothing is staged.
    pub fn style_diff_summary(&self) -> Vec<String> {
        use crate::panels::mcu_module::modules::AsyncBusMode;
        let mut out = Vec::new();
        if self.pending_runtime != self.runtime {
            out.push(format!(
                "Runtime: {} → {}",
                self.runtime.as_token(),
                self.pending_runtime.as_token()
            ));
        }
        if self.pending_gpio_api != self.gpio_api {
            out.push(format!(
                "GPIO In/Out: {:?} → {:?}",
                self.gpio_api, self.pending_gpio_api
            ));
        }
        for m in &self.modules {
            let Some(&(api, asyncm)) = self.pending_module_styles.get(&m.id) else {
                continue;
            };
            let (cur_api, cur_async) = module_style(&m.config);
            let name = crate::panels::mcu_module::mcu::gui::modules::module_base_name(m);
            if api != cur_api {
                out.push(format!("{name} init: {cur_api:?} → {api:?}"));
            }
            if asyncm != cur_async && !matches!(
                m.config,
                crate::panels::mcu_module::modules::ModuleConfig::Usart(_)
            ) {
                let lbl = |x: AsyncBusMode| match x {
                    AsyncBusMode::Blocking => "Blocking",
                    AsyncBusMode::AsyncDma => "Async-DMA",
                };
                out.push(format!("{name} async: {} → {}", lbl(cur_async), lbl(asyncm)));
            }
        }
        out
    }

    /// The FULL list of concrete modifications an Apply would produce — the
    /// staged choices ([`style_diff_summary`](Self::style_diff_summary)) PLUS
    /// their effects: the `main.rs` entry-point change, and every
    /// `src/pins/configs/*.rs` file that would be added / removed / regenerated
    /// (dry-run: apply the pending choices to a clone and diff its
    /// `config_files()`). Shown in the Apply-confirm prompt. Empty when nothing is
    /// staged.
    pub fn apply_change_list(&self) -> Vec<String> {
        if !self.style_dirty() {
            return Vec::new();
        }
        // 1. The choices the user made.
        let mut out: Vec<String> = self
            .style_diff_summary()
            .into_iter()
            .map(|l| format!("• {l}"))
            .collect();

        // 2. Entry-point change (only when async-ness flips; Blocking↔Native
        //    share `#[entry] fn main() -> !`).
        if self.pending_is_async() != self.is_async() {
            let entry = if self.pending_is_async() {
                "#[embassy_executor::main] async fn main(Spawner)"
            } else {
                "#[entry] fn main() -> !"
            };
            out.push(format!("↻ main.rs entry → {entry}"));
        } else {
            out.push("↻ main.rs regenerated (pin bindings)".to_string());
        }

        // 3. Config-file adds / removes / regenerations — a dry-run of the regen.
        let mut preview = self.clone();
        preview.apply_pending_style();
        let before = self.config_files();
        let after = preview.config_files();
        let body_of = |v: &[(String, String)], n: &str| {
            v.iter().find(|(f, _)| f == n).map(|(_, b)| b.clone())
        };
        let names: std::collections::BTreeSet<String> = before
            .iter()
            .chain(after.iter())
            .map(|(n, _)| n.clone())
            .collect();
        for name in names {
            match (body_of(&before, &name), body_of(&after, &name)) {
                (None, Some(_)) => out.push(format!("+ src/pins/configs/{name}  (new)")),
                (Some(_), None) => out.push(format!("− src/pins/configs/{name}  (removed)")),
                (Some(b), Some(a)) if b != a => {
                    out.push(format!("↻ src/pins/configs/{name}  (regenerated)"))
                }
                _ => {}
            }
        }

        // 4. Cargo.toml deps follow the choices (embassy / embedded-io / nb / …);
        //    the exact set is applied by `init_frame` after Apply.
        out.push("↻ Cargo.toml dependencies updated to match".to_string());
        out
    }

    /// Restores the clock-tree configuration parsed from a saved `main.rs`
    /// (`// @clock` marker). The saved config is expanded to graph node states
    /// and adopted by id — F103-shaped graphs restore fully; other-family
    /// graphs (no matching ids) are an intentional no-op.
    pub fn apply_saved_clock(&mut self, clock: crate::panels::mcu_module::clock::Stm32f1Clock) {
        use crate::panels::mcu_module::clock::graph::stm32f1_graph;
        use crate::panels::mcu_module::clock::ClockConfig;
        if let ClockConfig::Graph(gc) = &mut self.clock {
            gc.graph.adopt_states(&stm32f1_graph(&clock));
        }
    }

    /// Resets all non-reserved pins to Unset and clears selection/info state.
    pub fn reset_all_pins(&mut self) {
        for pin in self.iter_all_pins_mut() {
            if !pin.reserved {
                pin.selected_function = PinFunction::Unset;
            }
        }
        self.selected_pin = None;
        self.show_info = None;
    }

    /// Iterator over every pin (all four sides), immutable.
    pub fn iter_all_pins(&self) -> impl Iterator<Item = &Pin> {
        self.top_pins
            .iter()
            .chain(self.bottom_pins.iter())
            .chain(self.left_pins.iter())
            .chain(self.right_pins.iter())
    }

    /// Iterator over every pin (all four sides), mutable.
    pub fn iter_all_pins_mut(&mut self) -> impl Iterator<Item = &mut Pin> {
        self.top_pins
            .iter_mut()
            .chain(self.bottom_pins.iter_mut())
            .chain(self.left_pins.iter_mut())
            .chain(self.right_pins.iter_mut())
    }

    /// Auto-assigns partner functions when `source_pin` receives `func`.
    /// For each partner function defined by `partner_functions()`, finds the
    /// first Unset pin (other than `source_pin`) that lists it as available
    /// and assigns it automatically.
    pub fn auto_assign_partners(&mut self, source_pin: usize, func: &PinFunction) {
        for partner in partner_functions(func) {
            // Resolve the target pin number before any mutable borrow
            let target = self
                .iter_all_pins()
                .find(|p| {
                    p.number != source_pin
                        && p.selected_function == PinFunction::Unset
                        && p.available_functions.contains(&partner)
                })
                .map(|p| p.number);

            if let Some(num) = target {
                if let Some(pin) = self.find_pin_mut(num) {
                    pin.selected_function = partner;
                }
            }
        }
    }

    /// Removes the partner functions of `old_func` from whichever pins
    /// currently hold them (called when `source_pin` is deselected).
    pub fn deselect_partners(&mut self, source_pin: usize, old_func: &PinFunction) {
        for partner in partner_functions(old_func) {
            let target = self
                .iter_all_pins()
                .find(|p| p.number != source_pin && p.selected_function == partner)
                .map(|p| p.number);

            if let Some(num) = target {
                if let Some(pin) = self.find_pin_mut(num) {
                    pin.selected_function = PinFunction::Unset;
                }
            }
        }
    }

    /// Assign `func` to pin `pin_num`, applying the same side effects as a
    /// click on the Pins tab: auto-assign partner functions (or deselect them
    /// when clearing), and close any open info popup.
    ///
    /// Returns the `(number, name, func)` change tuple so code-sync callers can
    /// regenerate the `pins/` files; `None` if `pin_num` doesn't exist.
    pub fn apply_pin_function(
        &mut self,
        pin_num: usize,
        func: PinFunction,
    ) -> Option<(usize, String, PinFunction)> {
        let old_func = self.find_pin(pin_num)?.selected_function.clone();

        let changed = {
            let pin = self.find_pin_mut(pin_num)?;
            pin.selected_function = func.clone();
            // Clearing a pin also clears its user label, so a freed pin starts
            // clean if it's reassigned later.
            if func == PinFunction::Unset {
                pin.custom_label.clear();
            }
            (pin.number, pin.name.clone(), func.clone())
        };
        self.show_info = None;

        if func == PinFunction::Unset {
            self.deselect_partners(pin_num, &old_func);
        } else {
            self.auto_assign_partners(pin_num, &func);
        }

        // A pin re-purposed away from USART must drop any virtual-module wire.
        self.reconcile_modules();

        Some(changed)
    }

    /// Drop each module connection whose pin no longer carries the matching
    /// USART function — so re-purposing a pin disconnects the GI_USART from it
    /// (the module stays, just unwired). Idempotent.
    /// Make the virtual modules mirror the pin assignments: a peripheral
    /// instance with any assigned USART/SPI/I2C signal pin gets (or keeps) a
    /// module wired to exactly those pins; an instance with no assigned pins
    /// loses its module. So selecting peripheral pins in the Peripherals tab
    /// auto-adds the matching module, and clearing them removes it. Existing
    /// modules keep their config (only connections are re-synced); newly created
    /// ones get the default config. Idempotent; never mutates pins.
    pub fn reconcile_modules(&mut self) {
        use crate::panels::mcu_module::modules::{
            module_signal_of, Connection, ModuleKind, ModuleSignal, VirtualModule,
        };
        use std::collections::BTreeMap;

        let mut wanted: BTreeMap<(ModuleKind, u8), Vec<(ModuleSignal, usize)>> = BTreeMap::new();
        for p in self.iter_all_pins() {
            if let Some((kind, inst, sig)) = module_signal_of(&p.selected_function) {
                wanted.entry((kind, inst)).or_default().push((sig, p.number));
            }
        }

        // Drop modules whose peripheral no longer has any assigned pins.
        self.modules
            .retain(|m| wanted.contains_key(&(m.kind, m.instance())));

        // Ensure a module per wanted peripheral and re-sync its connections.
        for ((kind, inst), mut conns) in wanted {
            conns.sort_by_key(|(s, _)| *s);
            let connections: Vec<Connection> = conns
                .into_iter()
                .map(|(signal, mcu_pin)| Connection { signal, mcu_pin })
                .collect();

            if let Some(pos) = self
                .modules
                .iter()
                .position(|m| m.kind == kind && m.instance() == inst)
            {
                self.modules[pos].connections = connections;
            } else {
                let idx = self.modules.len() + 1;
                self.modules.push(VirtualModule {
                    id: format!("{}_{idx}", kind.short().to_ascii_lowercase()),
                    kind,
                    name: format!("{}{inst}", kind.short()),
                    pos: (0.0, 0.0),
                    config: kind.default_config(inst),
                    connections,
                });
            }
        }
    }

    /// Finds a pin by number (immutable)
    pub fn find_pin(&self, number: usize) -> Option<&Pin> {
        self.iter_all_pins().find(|p| p.number == number)
    }

    /// Finds a pin by number (mutable)
    pub fn find_pin_mut(&mut self, number: usize) -> Option<&mut Pin> {
        self.iter_all_pins_mut().find(|p| p.number == number)
    }
}

#[cfg(test)]
mod module_support_tests {
    use crate::panels::mcu_module::modules::ModuleKind;
    use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
    use crate::panels::mcu_module::{create_esp32c3, create_stm32f103c8tx};

    /// Both bundled chips genuinely expose all five interfaces, so a fresh
    /// palette offers everything.
    #[test]
    fn bundled_chips_support_every_kind() {
        for mcu in [create_stm32f103c8tx(), create_esp32c3()] {
            for kind in ModuleKind::ALL {
                assert!(
                    mcu.supports_module(kind),
                    "{} should host {}",
                    mcu.name,
                    kind.short()
                );
            }
        }
    }

    /// Support is derived from the PINS: strip a peripheral's pins and its kind
    /// disappears from the palette — no per-family list to maintain.
    #[test]
    fn a_chip_without_the_pins_does_not_offer_the_kind() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.supports_module(ModuleKind::GenericInterfaceUsb));
        for p in mcu.iter_all_pins_mut() {
            p.available_functions
                .retain(|f| !matches!(f, PinFunction::UsbDm | PinFunction::UsbDp));
        }
        assert!(
            !mcu.supports_module(ModuleKind::GenericInterfaceUsb),
            "no USB pins → the kind is hidden"
        );
        // Untouched peripherals still show.
        assert!(mcu.supports_module(ModuleKind::GenericInterfaceUsart));
        assert!(mcu.supports_module(ModuleKind::GenericInterfaceI2c));
    }

    /// The subtle part the dry-run buys us: ONE peripheral instance must offer
    /// every required signal — it is NOT enough that some pin can TX and some
    /// unrelated pin can RX.
    #[test]
    fn required_signals_must_come_from_the_same_instance() {
        let mut mcu = create_stm32f103c8tx();
        for p in mcu.iter_all_pins_mut() {
            p.available_functions.retain(|f| match f {
                PinFunction::UsartTx(n) => *n == 1, // TX only on USART1
                PinFunction::UsartRx(n) => *n == 2, // RX only on USART2
                _ => true,
            });
        }
        assert!(
            !mcu.supports_module(ModuleKind::GenericInterfaceUsart),
            "TX on USART1 + RX on USART2 is not a usable pair"
        );
    }

    /// Exhausting a kind must flip only AVAILABILITY — it stays supported, so
    /// the palette keeps the button visible (disabled + reason) instead of
    /// silently dropping it.
    #[test]
    fn exhausting_a_kind_keeps_it_supported_but_unavailable() {
        let mut mcu = create_stm32f103c8tx();
        let kind = ModuleKind::GenericInterfaceUsb; // single-instance
        assert!(mcu.supports_module(kind) && mcu.can_add_module(kind));
        assert!(mcu.add_module(kind));
        assert!(mcu.supports_module(kind), "the chip still has the pins");
        assert!(!mcu.can_add_module(kind), "but none are free any more");
    }

    /// The palette can never lie: whatever `can_add_module` promises,
    /// `add_module` delivers — driven to exhaustion across every kind.
    #[test]
    fn can_add_module_always_agrees_with_add_module() {
        let mut mcu = create_stm32f103c8tx();
        for _ in 0..12 {
            for kind in ModuleKind::ALL {
                let promised = mcu.can_add_module(kind);
                let actual = mcu.add_module(kind);
                assert_eq!(
                    promised,
                    actual,
                    "{}: palette promised {promised} but add_module returned {actual}",
                    kind.short()
                );
            }
        }
    }
}

// ── Bus-module style helpers (used by the staged-Apply flow) ──────────────────
use crate::panels::mcu_module::modules::{ApiStyle, AsyncBusMode, ModuleConfig};

/// The `(api_style, async_mode)` a bus module currently carries. USART has no
/// `async_mode` (its async form is always the embedded-io-async bridge), so it
/// reports `Blocking`; non-bus kinds report the defaults.
pub fn module_style(config: &ModuleConfig) -> (ApiStyle, AsyncBusMode) {
    match config {
        ModuleConfig::Usart(c) => (c.api_style, AsyncBusMode::Blocking),
        ModuleConfig::Spi(c) => (c.api_style, c.async_mode),
        ModuleConfig::I2c(c) => (c.api_style, c.async_mode),
        _ => (ApiStyle::Portable, AsyncBusMode::Blocking),
    }
}

/// Write a staged `(api_style, async_mode)` into a bus module's config.
fn set_module_style(config: &mut ModuleConfig, api: ApiStyle, async_mode: AsyncBusMode) {
    match config {
        ModuleConfig::Usart(c) => c.api_style = api,
        ModuleConfig::Spi(c) => {
            c.api_style = api;
            c.async_mode = async_mode;
        }
        ModuleConfig::I2c(c) => {
            c.api_style = api;
            c.async_mode = async_mode;
        }
        _ => {}
    }
}
