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
        }
    }

    // ── Virtual modules ───────────────────────────────────────────────────────

    /// Add a virtual module and auto-wire it to compatible MCU pins, setting
    /// those pins' functions. Returns `false` (and adds nothing) when the chip
    /// has no free pins for the module's interface.
    pub fn add_module(
        &mut self,
        kind: crate::panels::mcu_module::modules::ModuleKind,
    ) -> bool {
        use crate::panels::mcu_module::modules::ModuleKind;
        match kind {
            ModuleKind::GenericInterfaceUsart => self.add_usart_module(),
        }
    }

    /// Wire a GI_USART module to a free USART TX/RX pin pair.
    fn add_usart_module(&mut self) -> bool {
        use crate::panels::mcu_module::modules::{
            autowire, Connection, ModuleConfig, ModuleKind, ModuleSignal, UsartModuleConfig,
            VirtualModule,
        };
        // Pins already wired to an existing module are off-limits.
        let used: std::collections::HashSet<usize> = self
            .modules
            .iter()
            .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
            .collect();

        let Some((n, tx_pin, rx_pin)) = autowire::pick_usart_pins(self, &used) else {
            return false;
        };

        if let Some(p) = self.find_pin_mut(tx_pin) {
            p.selected_function = PinFunction::UsartTx(n);
        }
        if let Some(p) = self.find_pin_mut(rx_pin) {
            p.selected_function = PinFunction::UsartRx(n);
        }

        let idx = self.modules.len() + 1;
        self.modules.push(VirtualModule {
            id: format!("gi_usart_{idx}"),
            kind: ModuleKind::GenericInterfaceUsart,
            name: format!("GI_USART{n}"),
            pos: (0.0, 0.0),
            config: ModuleConfig::Usart(UsartModuleConfig::new(n)),
            connections: vec![
                Connection { signal: ModuleSignal::Tx, mcu_pin: tx_pin },
                Connection { signal: ModuleSignal::Rx, mcu_pin: rx_pin },
            ],
        });
        true
    }

    /// Remove a module by id. Does not clear the pins it had wired (they keep
    /// their USART functions; the user can change them in the Pins tab).
    pub fn remove_module(&mut self, id: &str) {
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
    pub fn reconcile_modules(&mut self) {
        use crate::panels::mcu_module::modules::ModuleSignal;
        let funcs: std::collections::HashMap<usize, PinFunction> = self
            .iter_all_pins()
            .map(|p| (p.number, p.selected_function.clone()))
            .collect();
        for m in &mut self.modules {
            let inst = m.usart_instance();
            m.connections.retain(|conn| {
                let want = match conn.signal {
                    ModuleSignal::Tx => PinFunction::UsartTx(inst),
                    ModuleSignal::Rx => PinFunction::UsartRx(inst),
                };
                funcs.get(&conn.mcu_pin) == Some(&want)
            });
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
