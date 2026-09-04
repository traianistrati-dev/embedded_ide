//! MCU business logic — partner assignment, state management, pin lookups.

use super::model::Mcu;
use crate::panels::mcu_module::modules::autowire;
use crate::panels::mcu_module::pins::logic::pin::Pin;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

// ── Peripheral pin groups ─────────────────────────────────────────────────────
// Defines which functions must be co-selected / co-deselected as a group.
// Selecting any member of a group auto-assigns the rest to the nearest
// available Unset pin; deselecting one member removes the whole group.

pub fn partner_functions(func: &PinFunction) -> Vec<PinFunction> {
    match func {
        // USART — basic full-duplex pair
        PinFunction::UsartTx(n) => vec![PinFunction::UsartRx(*n)],
        PinFunction::UsartRx(n) => vec![PinFunction::UsartTx(*n)],
        // USART — hardware flow-control pair (optional, separate from TX/RX)
        PinFunction::UsartCts(n) => vec![PinFunction::UsartRts(*n)],
        PinFunction::UsartRts(n) => vec![PinFunction::UsartCts(*n)],
        // LPUART — a peripheral of its own, paired exactly like the USART, so
        // assigning one half by hand completes the module the same way.
        PinFunction::LpuartTx(n) => vec![PinFunction::LpuartRx(*n)],
        PinFunction::LpuartRx(n) => vec![PinFunction::LpuartTx(*n)],
        PinFunction::LpuartCts(n) => vec![PinFunction::LpuartRts(*n)],
        PinFunction::LpuartRts(n) => vec![PinFunction::LpuartCts(*n)],
        // SPI — three-wire bus (NSS is optional, not auto-assigned)
        PinFunction::SpiSck(n) => vec![PinFunction::SpiMiso(*n), PinFunction::SpiMosi(*n)],
        PinFunction::SpiMiso(n) => vec![PinFunction::SpiSck(*n), PinFunction::SpiMosi(*n)],
        PinFunction::SpiMosi(n) => vec![PinFunction::SpiSck(*n), PinFunction::SpiMiso(*n)],
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
        PinFunction::SwdIo => vec![PinFunction::SwdClk],
        PinFunction::SwdClk => vec![PinFunction::SwdIo],
        // GPIO, ADC, Timer, MCO, SpiNss, UsartCk — no automatic partners
        _ => vec![],
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

impl Mcu {
    /// The watchdog `init(...)` lines followed by the Custom-module ones.
    ///
    /// One string because every backend already threads a single "extra
    /// inits" slot through `make_generated_section`; adding a parameter to
    /// each of the nine call sites would have been churn for no gain.
    /// Watchdogs come FIRST - one that is meant to catch a hang during
    /// start-up is worth arming before the code that might hang.
    pub fn watchdog_and_custom_inits(&self) -> String {
        format!(
            "{}{}",
            crate::panels::mcu_module::codegen::watchdog_gen::init_lines(
                &self.watchdog,
                &self.family,
            ),
            self.custom_module_inits(),
        )
    }
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
            GraphClock, layout::stm32f1_layout, stm32f1_graph,
        };
        use crate::panels::mcu_module::clock::{ClockConfig, ClockLimits, Stm32f1Clock};
        // Only the STM32F1 family has a built-in clock graph; others get `None`
        // (a definition's `ClockDef` overrides this in `build_mcu`).
        let clock = match family.as_str() {
            "stm32f1" => ClockConfig::Graph(GraphClock {
                graph: stm32f1_graph(&Stm32f1Clock::default()),
                layout: stm32f1_layout(&ClockLimits::default()),
                bindings: Default::default(),
            }),
            _ => ClockConfig::None,
        };
        // The tree as built here IS the default — `build_mcu` re-captures after
        // overriding `clock` from the definition.
        // A family with no RCC recipe cannot have its clock generated, so its
        // block is hand-written from the start.
        let clock_manual = !crate::panels::mcu_module::codegen::rcc::generates_clock_code(&family);
        let clock_defaults = match &clock {
            ClockConfig::Graph(gc) => Some(gc.graph.clone()),
            ClockConfig::None => None,
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
            // Edge-packaged by default; a ball-grid chip fills this in
            // afterwards (see `McuDefinition::build_mcu`).
            grid: None,
            dma: None,
            irq_vectors: Vec::new(),
            // A bare chip until the definition says otherwise.
            board_chip: None,
            usart_ip: None,
            sdmmc_ip: None,
            selected_pin: None,
            pin_search: String::new(),
            show_info: None,
            fn_scroll_offset: 0.0,
            clock,
            clock_limits: ClockLimits::default(),
            clock_presets: Vec::new(),
            clock_defaults,
            clock_manual,
            modules: Vec::new(),
            runtime: crate::panels::mcu_module::mcu::model::Runtime::default(),
            gpio_api: crate::panels::mcu_module::modules::ApiStyle::default(),
            pending_runtime: crate::panels::mcu_module::mcu::model::Runtime::default(),
            pending_gpio_api: crate::panels::mcu_module::modules::ApiStyle::default(),
            pending_module_styles: std::collections::BTreeMap::new(),
            pending_apply_confirm: false,
            config_regen_forced: false,
            auto_build: crate::panels::mcu_module::mcu::model::AutoBuild::default(),
            strict_lints: false,
            debug_build: false,
            expand_module: None,
            module_undo: Vec::new(),
            module_remove_confirm: None,
            pin_goto: None,
            module_goto: None,
            selected_module: None,
            collapse_modules: false,
            rotated: false,
            io_pin_pos: std::collections::BTreeMap::new(),
            groups: Vec::new(),
            watchdog: Default::default(),
            comp: Default::default(),
        }
    }

    /// A 4-sided (QFP-style) package — pins on 3 or 4 edges. Rotation makes it a
    /// 45° diamond; a 2-sided (DIP) package rotates 90° instead. See
    /// [`crate::panels::mcu_module::mcu::gui::rotate`].
    pub fn is_quad_package(&self) -> bool {
        [
            &self.top_pins,
            &self.bottom_pins,
            &self.left_pins,
            &self.right_pins,
        ]
        .iter()
        .filter(|v| !v.is_empty())
        .count()
            >= 3
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
    /// Why a peripheral this chip HAS is still not offered.
    ///
    /// [`supports_module`](Self::supports_module) answers from the pins, which
    /// is the right question almost always: no pins, no module. But a pin is
    /// only offered when something can be GENERATED for it, so a peripheral the
    /// silicon has and the HAL cannot drive disappears from the palette with no
    /// explanation — and someone holding the datasheet is left wondering.
    ///
    /// This returns the sentence to show instead. The palette keeps the entry
    /// visible and disabled, the same as it does for an exhausted instance.
    pub fn hardware_only_reason(
        &self,
        kind: crate::panels::mcu_module::modules::ModuleKind,
    ) -> Option<&'static str> {
        use crate::panels::mcu_module::modules::ModuleKind;
        match kind {
            // The S2 and S3 carry the touch sensors — their pads are in
            // Espressif's own pin tables — but esp-hal builds `touch` only for
            // the original ESP32, and does not even expose `peripherals::TOUCH`
            // on the other two. There is nothing to hand a constructor.
            ModuleKind::GenericInterfaceTouch
                if matches!(self.family.as_str(), "esp32s2" | "esp32s3") =>
            {
                Some(
                    "This chip HAS capacitive touch, but esp-hal builds no touch driver \
                     for it - only for the original ESP32. Nothing could be generated, so \
                     the pads are not offered either. An external touch controller over \
                     I2C works today.",
                )
            }
            _ => None,
        }
    }

    pub fn supports_module(&self, kind: crate::panels::mcu_module::modules::ModuleKind) -> bool {
        use crate::panels::mcu_module::modules::autowire;
        // A custom module needs no particular peripheral — any chip can host it.
        if kind.is_custom() {
            return true;
        }
        // Support is derived from the PINS below, which is right for every
        // peripheral whose init the backend can actually write. USB is the
        // exception: the D-/D+ pins exist on chips whose backend generates no
        // USB code at all, so the module was addable and produced nothing but
        // two stray dependencies. Only the family can answer that.
        if kind == crate::panels::mcu_module::modules::ModuleKind::GenericInterfaceUsb
            && !crate::panels::mcu_module::codegen::family::usb_supported(&self.family)
        {
            return false;
        }
        let (required, optional) = kind.signals();
        autowire::pick_pins(
            self,
            &Default::default(),
            &Default::default(),
            required,
            optional,
        )
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
        // Custom modules claim no peripheral, so you can always add another.
        if kind.is_custom() {
            return true;
        }
        let (required, _optional) = kind.signals();
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
        // `any_wiring`, not `pick_pins`: this only asks WHETHER, and the
        // ranking `pick_pins` does to answer WHICH is the whole cost. The
        // palette asks this for every entry on every frame its menu is open -
        // 119 ms of one on an ESP32-S3, now 0.35 ms.
        //
        // The optional signals are not consulted, and cannot change the answer:
        // each is added only `if let Some(pin)`, so it can extend a wiring but
        // never prevent one (`an_optional_signal_cannot_make_a_wiring_fail`).
        autowire::any_wiring(self, &used, &used_instances, required)
    }

    /// Add a virtual module (_USART / _SPI / _I2C) and auto-wire it to
    /// compatible MCU pins, setting those pins' functions. Returns `false` (and
    /// adds nothing) when the chip has no free pins for the module's interface.
    pub fn add_module(&mut self, kind: crate::panels::mcu_module::modules::ModuleKind) -> bool {
        use crate::panels::mcu_module::modules::autowire;

        // CAN and USB are single-instance and their pin functions carry no
        // index, so the instance-exclusion guard below can't stop a 2nd module
        // from grabbing the alternate pins — refuse it here.
        if kind.is_single_instance() && self.modules.iter().any(|m| m.kind == kind) {
            return false;
        }

        // A CUSTOM module wires nothing: it is created empty and the user adds
        // pins in its config panel, so the auto-wiring path below doesn't apply.
        if kind.is_custom() {
            use crate::panels::mcu_module::modules::VirtualModule;
            let inst = (self
                .modules
                .iter()
                .filter(|m| m.kind.is_custom())
                .map(|m| m.instance())
                .max()
                .unwrap_or(0))
                + 1;
            let id = self.free_module_id("custom");
            self.modules.push(VirtualModule {
                id,
                kind,
                name: format!("Custom{inst}"),
                pos: (0.0, 0.0),
                config: kind.default_config(inst),
                connections: Vec::new(),
            });
            return true;
        }

        let (required, optional) = kind.signals();

        // Pins already wired to an existing module are off-limits.
        let used: std::collections::HashSet<usize> = self
            .modules
            .iter()
            .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
            .collect();
        // Peripheral instances already hosting a module of THIS kind are off-limits
        // too — so a 2nd "+_SPI" advances to SPI2 instead of re-picking SPI1 on
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

        self.add_module_wired(inst, &chosen)
    }

    /// Commit a wiring the caller already decided on.
    ///
    /// The tail of [`add_module`](Self::add_module), split out so the two ways
    /// of adding a module - autowire's pick and the user's own choice in the
    /// palette dialog - end at exactly the same three lines. A second copy is
    /// how the two would drift into disagreeing about what a module IS.
    ///
    /// The module itself is created (with its default config) by
    /// `reconcile_modules`, the single source of truth mirroring pin
    /// assignments; nothing here builds a `VirtualModule`, because a
    /// hand-built one would be duplicated on the next reconcile.
    ///
    /// The whole set is written BEFORE the reconcile, and `apply_pin_function`
    /// is deliberately not used: it would run `auto_assign_partners` per pad
    /// and move the pins the caller just chose.
    /// Returns `false` and changes NOTHING when the wiring is not one the chip
    /// can form - every pad must be able to carry its signal AND be free (or
    /// already carrying exactly it). Checked whole, before anything is written,
    /// so a half-applied wiring cannot exist.
    ///
    /// The dialog enumerates from `autowire::eligible` and so cannot normally
    /// produce a bad set - but it holds its choice across frames, and the chip
    /// moves underneath it: a pad free when the dialog opened can be taken by
    /// the time it is confirmed. Without this the confirm would overwrite that
    /// pad, and whichever module owned it would lose a connection - or, if it
    /// owned only that one, be dropped by `reconcile_modules` with its config.
    pub fn add_module_wired(
        &mut self,
        inst: u8,
        chosen: &[(crate::panels::mcu_module::modules::ModuleSignal, usize)],
    ) -> bool {
        if chosen.is_empty() {
            return false;
        }
        let formable = chosen.iter().all(|(sig, pin)| {
            let want = sig.pin_function(inst);
            self.find_pin(*pin).is_some_and(|p| {
                !p.reserved
                    && p.available_functions.contains(&want)
                    && (p.selected_function == PinFunction::Unset || p.selected_function == want)
            })
        });
        if !formable {
            return false;
        }
        for (sig, pin) in chosen {
            if let Some(p) = self.find_pin_mut(*pin) {
                p.selected_function = sig.pin_function(inst);
            }
        }
        self.reconcile_modules();
        true
    }

    /// The group a pad belongs to, if any.
    ///
    /// A pad is in at most one: `join_group` takes it out of whatever held it
    /// first, because "this pad is part of the radar AND part of the display"
    /// is not a thing a schematic can show, and a pad with two accent colours
    /// would just look broken.
    /// Only a LIVE group answers — the same predicate persistence, the generated
    /// comment and the canvas mats use. A row the roster is still filling in has
    /// no name yet, and a pad tick or a box bar for it would mark a device that
    /// exists nowhere else.
    pub fn group_of_pin(
        &self,
        pin: usize,
    ) -> Option<&crate::panels::mcu_module::mcu_config::PinGroup> {
        self.groups
            .iter()
            .find(|g| g.is_live() && g.pins.contains(&pin))
    }

    /// The group a MODULE belongs to: the one holding any of its pads.
    ///
    /// Derived rather than stored, which is what lets a group survive
    /// `reconcile_modules` deleting and re-creating the module under a new id.
    pub fn group_of_module(
        &self,
        m: &crate::panels::mcu_module::modules::VirtualModule,
    ) -> Option<&crate::panels::mcu_module::mcu_config::PinGroup> {
        m.connections
            .iter()
            .find_map(|c| self.group_of_pin(c.mcu_pin))
    }

    /// Put `pin` in the group called `name`, creating it if it is new.
    ///
    /// An empty name is how a pad leaves its group - the same field does both,
    /// so there is no second gesture to find.
    pub fn join_group(&mut self, pin: usize, name: &str) {
        // A group that gave this pad up and has nothing left is finished. Only
        // THOSE are dropped: a device the user has just created and not filled
        // yet is empty too, and deleting it out from under them the moment they
        // group something else would be indistinguishable from a bug.
        let mut emptied: Vec<usize> = Vec::new();
        for (i, g) in self.groups.iter_mut().enumerate() {
            if g.pins.remove(&pin) && g.pins.is_empty() {
                emptied.push(i);
            }
        }
        // STORED verbatim, so it matches a name the roster is still editing
        // (see `rename_group`) - but two names are the SAME NAME when they
        // differ only in padding. Storage and identity have to part company
        // here: `mcu.config` trims on the way out and `group_color` hashes the
        // trimmed name, so "radar " and "radar" would draw one colour, save as
        // one line, and come back after a reload as two devices with literally
        // the same name.
        if !name.trim().is_empty() {
            match self.groups.iter_mut().find(|g| g.name.trim() == name.trim()) {
                Some(g) => {
                    g.pins.insert(pin);
                }
                // Pushed, so the indices collected above stay valid.
                None => self
                    .groups
                    .push(crate::panels::mcu_module::mcu_config::PinGroup {
                        name: name.to_owned(),
                        pins: std::iter::once(pin).collect(),
                    }),
            }
        }
        for i in emptied.into_iter().rev() {
            // Still empty: moving a pad WITHIN its own group empties it here and
            // fills it again above, and that group must survive.
            if self.groups[i].pins.is_empty() {
                self.groups.remove(i);
            }
        }
    }

    /// Start an empty device, named by the roster.
    ///
    /// It holds nothing yet, so it is not written to `mcu.config` and does not
    /// reach the generated comment - both skip empty groups. It exists to be
    /// filled in the next gesture.
    pub fn new_group(&mut self, name: String) {
        self.groups
            .push(crate::panels::mcu_module::mcu_config::PinGroup {
                name,
                pins: Default::default(),
            });
    }

    /// Set group `idx`'s name without ever merging.
    ///
    /// What the roster calls while its field still has FOCUS. Committing a merge
    /// on every keystroke destroyed devices in passing: typing "disp" out to
    /// "display2" passes through "display", and if another device answered to
    /// that, the two were merged at that keystroke and the rest of the word
    /// landed on whatever row had shifted into the slot. A name is only a
    /// decision once the user leaves the field.
    ///
    /// Two devices may briefly share a name this way. Nothing is lost by it: the
    /// canvas draws them as one mat for those frames, and `rename_group` folds
    /// them together the moment the field is left.
    pub fn set_group_name(&mut self, idx: usize, name: &str) {
        if let Some(g) = self.groups.get_mut(idx) {
            g.name = name.to_owned();
        }
    }

    /// Rename group `idx`, merging onto a name already taken.
    ///
    /// Renaming onto a name already taken MERGES the two: `join_group` finds a
    /// group by name, so leaving duplicates behind would mean two rows on the
    /// roster, one colour between them, and only one of them ever receiving a
    /// pad. Merging is the reading that matches what the user typed.
    ///
    /// Called when the roster's field is LEFT, never while it is being typed in
    /// — see [`set_group_name`](Self::set_group_name).
    pub fn rename_group(&mut self, idx: usize, name: &str) {
        // Stored EXACTLY as typed. Trimming here made a space impossible to
        // type: the roster re-seeds its text field from the stored name every
        // frame, so "mw " came back as "mw" and the next keystroke produced
        // "mwr". Whitespace is normalised where it belongs - on the way into
        // `mcu.config` - and an all-whitespace name still counts as no name.
        let name = name.to_owned();
        if idx >= self.groups.len() {
            return;
        }
        // ANOTHER group answering to this name - found by scanning past `idx`
        // rather than by `position(..).filter(!= idx)`, which returns the FIRST
        // match and so answers "none" whenever `idx` is itself that first match.
        // Two rows can transiently share a name (a name being typed is stored
        // without merging), and that is exactly the case the merge is owed.
        //
        // Deliberately NOT skipped when the name is unchanged: the roster defers
        // every merge until the field is left, at which point the text has long
        // since stopped changing and this is the only call that will make it.
        let other = self
            .groups
            .iter()
            .enumerate()
            .find(|(k, g)| {
                *k != idx && g.name.trim() == name.trim() && !name.trim().is_empty()
            })
            .map(|(k, _)| k);
        match other {
            Some(other) => {
                let moved = std::mem::take(&mut self.groups[idx].pins);
                self.groups[other].pins.extend(moved);
                self.groups.remove(idx);
            }
            None => {
                if self.groups[idx].name != name {
                    self.groups[idx].name = name;
                }
            }
        }
    }

    /// Put every pad of `m` in `name` at once - the gesture the panel offers on
    /// a module, since grouping "the UART" means its pads, not one of them.
    pub fn join_group_module(
        &mut self,
        m: &crate::panels::mcu_module::modules::VirtualModule,
        name: &str,
    ) {
        let pins: Vec<usize> = m.connections.iter().map(|c| c.mcu_pin).collect();
        for p in pins {
            self.join_group(p, name);
        }
    }

    /// Move one pin's function to ANOTHER pad, as a single operation.
    ///
    /// Deliberately NOT `apply_pin_function` twice, because neither order works:
    ///
    /// * clearing the old pad first runs `deselect_partners`, which takes the
    ///   whole bus with it - a USART TX drags its RX to `Unset`, the module
    ///   loses every connection, and `reconcile_modules` then hands the
    ///   re-created one a fresh `default_config`, so the baud rate the user set
    ///   is gone;
    /// * setting the new pad first leaves TWO pads carrying the same function,
    ///   and `reconcile_modules` does not de-duplicate by signal - the module
    ///   grows a second row for one wire.
    ///
    /// `auto_assign_partners` must not run here either: the partners are
    /// already placed, and re-picking them would move pads the user did not ask
    /// about. So the whole edit is one write-pair followed by one reconcile,
    /// which is also how [`add_module`](Self::add_module) commits.
    ///
    /// Returns `false` and changes nothing when the destination cannot carry
    /// the function - the caller offers only pads that can, but the check is
    /// here so the model cannot be driven into a state the panel forbids.
    pub fn move_pin_function(&mut self, from: usize, to: usize) -> bool {
        if from == to {
            return false;
        }
        let Some(func) = self.find_pin(from).map(|p| p.selected_function.clone()) else {
            return false;
        };
        if func == PinFunction::Unset {
            return false;
        }
        let reachable = self.find_pin(to).is_some_and(|p| {
            !p.reserved
                && p.available_functions.contains(&func)
                && (p.selected_function == PinFunction::Unset || p.selected_function == func)
        });
        if !reachable {
            return false;
        }
        if let Some(p) = self.find_pin_mut(from) {
            p.selected_function = PinFunction::Unset;
            // An armed edge belonged to the pad as an INPUT; the pad is now
            // unassigned, and a stale edge would arm a pin nothing drives.
            p.irq = None;
        }
        if let Some(p) = self.find_pin_mut(to) {
            p.selected_function = func;
        }
        // A group is a set of PAD numbers, so a signal that changes pad drops
        // out of its device unless the set is rewritten here. This is the only
        // place in the app where a signal moves between pads, which is why it
        // is also the only place that has to know.
        //
        // The destination is not necessarily device-free: a pad keeps its
        // device when its function goes away (removing a module resets its pads
        // to `Unset` but leaves them grouped), and the move only requires the
        // pad to be function-free. So `to` is taken out of whatever held it
        // before it is handed `from`'s device - otherwise one pad sat in two
        // devices at once and `group_of_pin` answered by Vec order.
        if let Some(mine) = self.groups.iter().position(|g| g.pins.contains(&from)) {
            let mut emptied: Vec<usize> = Vec::new();
            for (k, g) in self.groups.iter_mut().enumerate() {
                if g.pins.remove(&to) && g.pins.is_empty() && k != mine {
                    emptied.push(k);
                }
            }
            self.groups[mine].pins.remove(&from);
            self.groups[mine].pins.insert(to);
            // Only a device this move emptied disappears, the same rule
            // `join_group` follows.
            for k in emptied.into_iter().rev() {
                if self.groups[k].pins.is_empty() {
                    self.groups.remove(k);
                }
            }
        }
        self.reconcile_modules();
        true
    }

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

    // ── Virtual-module undo (Ctrl+Z on the Pins tab) ──────────────────────────

    /// Cap on the module undo stack — a Ctrl+Z safety net, not full history.
    const MODULE_UNDO_CAP: usize = 30;

    /// Snapshot the modules + pin state BEFORE an explicit add/remove, so Ctrl+Z
    /// (or the Undo button) can revert it. `label` is shown on the Undo hover.
    pub fn push_module_undo(&mut self, label: String) {
        use crate::panels::mcu_module::mcu::model::ModuleUndo;
        let pins = self
            .iter_all_pins()
            .map(|p| {
                (
                    p.number,
                    p.selected_function.clone(),
                    p.custom_label.clone(),
                )
            })
            .collect();
        self.module_undo.push(ModuleUndo {
            modules: self.modules.clone(),
            pins,
            label,
        });
        if self.module_undo.len() > Self::MODULE_UNDO_CAP {
            self.module_undo.remove(0);
        }
    }

    /// Drop the most recent snapshot WITHOUT applying it — used when a snapshotted
    /// action turned out to be a no-op (e.g. an add that found no free pins).
    pub fn discard_last_module_undo(&mut self) {
        self.module_undo.pop();
    }

    /// Revert the last add/remove: restore its snapshot (pins + modules). Returns
    /// the undone action's label, or `None` when the stack is empty.
    pub fn undo_modules(&mut self) -> Option<String> {
        let snap = self.module_undo.pop()?;
        for (num, func, label) in &snap.pins {
            if let Some(p) = self.find_pin_mut(*num) {
                p.selected_function = func.clone();
                p.custom_label = label.clone();
            }
        }
        self.modules = snap.modules;
        self.module_remove_confirm = None;
        Some(snap.label)
    }

    pub fn can_undo_modules(&self) -> bool {
        !self.module_undo.is_empty()
    }

    pub fn last_module_undo_label(&self) -> Option<&str> {
        self.module_undo.last().map(|u| u.label.as_str())
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
                ClockConfig::Graph(gc) => graph_to_stm32f1(&gc.for_codegen()),
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
        // Strict-lints preference (`@strict`) — workflow setting like @autobuild.
        let strict = mcu_config::strict_section(self.strict_lints);
        if !strict.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&strict);
        }
        // Debug-friendly release profile (`@debugbuild`) — workflow setting.
        let debug_build = mcu_config::debug_build_section(self.debug_build);
        if !debug_build.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&debug_build);
        }
        // Hand-written clock (`@clockmanual`) — this one is NOT a view
        // preference: it decides whether the generated clock block is replaced
        // or preserved, so it has to travel with the project's config.
        let clock_manual = mcu_config::clock_manual_section(self.clock_manual);
        if !clock_manual.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&clock_manual);
        }
        // Watchdogs (`@watchdog`) — codegen input, like `@clockmanual`: it
        // decides whether the watchdog config files exist at all.
        let wdg = mcu_config::watchdog_section(&self.watchdog);
        let comp = mcu_config::comp_section(&self.comp);
        if !wdg.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&wdg);
        }
        // Comparators (`@comp`) — codegen input for the same reason.
        if !comp.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&comp);
        }
        // Diagram rotation (`@rotation`) — view preference, same append pattern.
        let rotation = mcu_config::rotation_section(self.rotated);
        if !rotation.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&rotation);
        }
        // Manual in/out field positions (`@iopins`) — view preference.
        let groups = mcu_config::groups_section(&self.groups);
        if !groups.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&groups);
        }
        let iopins = mcu_config::iopins_section(&self.io_pin_pos);
        if !iopins.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&iopins);
        }
        // Interrupt edges (`@irq`). Unlike the two sections above this is NOT a
        // view preference: it changes the generated code on the RTIC runtime.
        let irqs: std::collections::BTreeMap<usize, _> = self
            .iter_all_pins()
            .filter_map(|p| p.irq.map(|e| (p.number, e)))
            .collect();
        let irq = mcu_config::irq_section(&irqs);
        if !irq.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&irq);
        }
        // GPIO drive/pull modes (`@iomode`) — also CODE, not a view preference:
        // it picks which `into_*` / `Pull::*` the binding is generated with.
        let modes: std::collections::BTreeMap<usize, _> = self
            .iter_all_pins()
            .filter_map(|p| p.io_mode.map(|m| (p.number, m)))
            .collect();
        let iomode = mcu_config::iomode_section(&modes);
        if !iomode.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&iomode);
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
        // Strict-lints preference (`@strict`) — missing restores the default OFF.
        self.strict_lints = mcu_config::parse_strict(text);
        // Debug-friendly release profile (`@debugbuild`) — missing = OFF.
        self.debug_build = mcu_config::parse_debug_build(text);
        // Diagram rotation (`@rotation`) — missing restores the default (0°).
        self.rotated = mcu_config::parse_rotation(text);
        // Hand-written clock (`@clockmanual`). A MISSING section keeps whatever
        // the chip's family decided (manual when it has no RCC recipe), so a
        // project saved before this existed still gets the right behaviour.
        if let Some(manual) = mcu_config::parse_clock_manual(text) {
            self.clock_manual = manual;
        }
        // Manual in/out field positions (`@iopins`) — missing = all auto-placed.
        self.io_pin_pos = mcu_config::parse_iopins(text);
        self.groups = mcu_config::parse_groups(text);
        self.watchdog = mcu_config::parse_watchdog(text);
        self.comp = mcu_config::parse_comp(text);
        // Interrupt edges (`@irq`) — a missing section means every input is
        // polled, which is the pre-RTIC behaviour of every existing project.
        let irqs = mcu_config::parse_irq(text);
        // GPIO modes (`@iomode`) — a missing section means every pin is on the
        // backend default (floating in / push-pull out), i.e. what every project
        // generated before the mode was selectable.
        let modes = mcu_config::parse_iomode(text);
        for pin in self.iter_all_pins_mut() {
            pin.irq = irqs.get(&pin.number).copied();
            pin.io_mode = modes.get(&pin.number).copied();
        }
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
        // The runtime / api-style just changed → the config templates (whole
        // `init()`, not just the consts) must be regenerated in full next frame.
        self.config_regen_forced = true;
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
                "Runtime: {} -> {}",
                self.runtime.as_token(),
                self.pending_runtime.as_token()
            ));
        }
        if self.pending_gpio_api != self.gpio_api {
            out.push(format!(
                "GPIO In/Out: {:?} -> {:?}",
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
                out.push(format!("{name} init: {cur_api:?} -> {api:?}"));
            }
            if asyncm != cur_async
                && !matches!(
                    m.config,
                    crate::panels::mcu_module::modules::ModuleConfig::Usart(_)
                )
            {
                let lbl = |x: AsyncBusMode| match x {
                    AsyncBusMode::Blocking => "Blocking",
                    AsyncBusMode::AsyncDma => "Async-DMA",
                };
                out.push(format!(
                    "{name} async: {} -> {}",
                    lbl(cur_async),
                    lbl(asyncm)
                ));
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
            // ESP spells both ends differently: esp-rtos drives the executor and
            // the blocking entry is esp-hal's, not cortex-m-rt's.
            let esp = crate::panels::mcu_module::codegen::family::async_is_esp(&self.family);
            let entry = match (self.pending_is_async(), esp) {
                (true, true) => "#[esp_rtos::main] async fn main(Spawner)",
                (true, false) => "#[embassy_executor::main] async fn main(Spawner)",
                (false, true) => "#[esp_hal::main] fn main() -> !",
                (false, false) => "#[entry] fn main() -> !",
            };
            out.push(format!("main.rs entry -> {entry}"));
        } else {
            out.push("~ main.rs regenerated (pin bindings)".to_string());
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
                (Some(_), None) => out.push(format!("- src/pins/configs/{name}  (removed)")),
                (Some(b), Some(a)) if b != a => {
                    out.push(format!("~ src/pins/configs/{name}  (regenerated)"))
                }
                _ => {}
            }
        }

        // 4. Cargo.toml deps follow the choices (embassy / embedded-io / nb / …);
        //    the exact set is applied by `init_frame` after Apply.
        out.push("~ Cargo.toml dependencies updated to match".to_string());
        out
    }

    /// Restores the clock-tree configuration parsed from a saved `main.rs`
    /// (`// @clock` marker). The saved config is expanded to graph node states
    /// and adopted by id — F103-shaped graphs restore fully; other-family
    /// graphs (no matching ids) are an intentional no-op.
    pub fn apply_saved_clock(&mut self, clock: crate::panels::mcu_module::clock::Stm32f1Clock) {
        use crate::panels::mcu_module::clock::ClockConfig;
        use crate::panels::mcu_module::clock::graph::stm32f1_graph;
        if let ClockConfig::Graph(gc) = &mut self.clock {
            gc.graph.adopt_states(&stm32f1_graph(&clock));
        }
    }

    /// Snapshots the current clock tree as the "factory" configuration for
    /// [`reset_clock`](Self::reset_clock). Call right after installing the
    /// definition's clock and BEFORE any saved `@clock` state is adopted.
    pub fn capture_clock_defaults(&mut self) {
        use crate::panels::mcu_module::clock::ClockConfig;
        self.clock_defaults = match &self.clock {
            ClockConfig::Graph(gc) => Some(gc.graph.clone()),
            ClockConfig::None => None,
        };
    }

    /// Is the clock tree still exactly as the chip definition shipped it?
    /// `true` when there is nothing to reset (including chips with no clock).
    pub fn clock_is_default(&self) -> bool {
        use crate::panels::mcu_module::clock::ClockConfig;
        match (&self.clock, &self.clock_defaults) {
            (ClockConfig::Graph(gc), Some(def)) => gc.graph.states_match(def),
            _ => true,
        }
    }

    /// Restores the chip's default clock configuration (node states only — the
    /// diagram layout is cosmetic and stays put). Returns `true` if anything
    /// actually changed, so the caller can regenerate `main.rs`.
    pub fn reset_clock(&mut self) -> bool {
        use crate::panels::mcu_module::clock::ClockConfig;
        if self.clock_is_default() {
            return false;
        }
        // Cloned so the defaults stay borrowable independently of `self.clock`.
        let Some(defaults) = self.clock_defaults.clone() else {
            return false;
        };
        let ClockConfig::Graph(gc) = &mut self.clock else {
            return false;
        };
        gc.graph.adopt_states(&defaults);
        true
    }

    /// Resets all non-reserved pins to Unset and clears selection/info state.
    /// How many pins "Reset pins" would actually clear.
    ///
    /// The header button asks before wiping, and this is what makes the question
    /// worth asking: it names the loss, and it is 0 exactly when the button has
    /// nothing to do.
    pub fn configured_pin_count(&self) -> usize {
        self.iter_all_pins()
            .filter(|p| !p.reserved && p.selected_function != PinFunction::Unset)
            .count()
    }

    pub fn reset_all_pins(&mut self) {
        for pin in self.iter_all_pins_mut() {
            if !pin.reserved {
                pin.selected_function = PinFunction::Unset;
            }
        }
        self.selected_pin = None;
        self.show_info = None;
    }

    /// Whether the package carries pins INSIDE the body (a ball grid) rather
    /// than only around its edges.
    ///
    /// The body is shared real estate: an edge package has it empty and can put
    /// the chip name there, a grid package has it full of pads. Anything drawn in
    /// the middle has to ask this first.
    pub fn has_inner_pins(&self) -> bool {
        self.grid.as_ref().is_some_and(|g| !g.cells.is_empty())
    }

    /// Pin numbers matching the toolbar search box. Three ways to hit, because
    /// three different labels are printed on the diagram:
    /// * the pin NAME — case-insensitive substring (`pa5`, `osc`, `ph1`);
    /// * the package DESIGNATOR of a ball — same, case-insensitive substring
    ///   (`n13`, `m1`), so a BGA can be searched by the label under the ball;
    /// * the pin NUMBER — EXACT (`13` finds pin 13, not 13/1/31).
    ///
    /// Substring for the two text labels and exact for the number on purpose: a
    /// name/designator is what the user half-remembers off the package, a number
    /// is something they read precisely.
    ///
    /// The designator matters more than it looks on a ball-grid part: there the
    /// number is our own ordinal and is never drawn — the designator IS what the
    /// user sees under the ball, so searching "N13" has to work.
    pub fn pin_search_hits(&self) -> std::collections::HashSet<usize> {
        let q = self.pin_search.trim().to_ascii_lowercase();
        if q.is_empty() {
            return std::collections::HashSet::new();
        }
        let mut hits: std::collections::HashSet<usize> = self
            .iter_all_pins()
            .filter(|p| p.name.to_ascii_lowercase().contains(&q) || p.number.to_string() == q)
            .map(|p| p.number)
            .collect();
        for cell in self.grid.iter().flat_map(|g| g.cells.iter()) {
            if cell.designator().to_ascii_lowercase().contains(&q) {
                hits.insert(cell.pin.number);
            }
        }
        hits
    }

    /// The set the diagram highlights, or `None` when nothing should be dimmed.
    ///
    /// `None` covers both "no search" and "search matches nothing": fading the
    /// WHOLE chip while the user is still typing a prefix that hasn't matched yet
    /// would be noise, not feedback.
    pub fn pin_search_highlight(&self) -> Option<std::collections::HashSet<usize>> {
        let hits = self.pin_search_hits();
        (!hits.is_empty()).then_some(hits)
    }

    /// Iterator over every pin (all four sides), immutable.
    pub fn iter_all_pins(&self) -> impl Iterator<Item = &Pin> {
        self.top_pins
            .iter()
            .chain(self.bottom_pins.iter())
            .chain(self.left_pins.iter())
            .chain(self.right_pins.iter())
            // Ball-grid pads are pins like any other — chaining them HERE is
            // what lets autowire, codegen and persistence stay layout-blind.
            .chain(
                self.grid
                    .iter()
                    .flat_map(|g| g.cells.iter().map(|c| &c.pin)),
            )
    }

    /// Iterator over every pin (all four sides), mutable.
    pub fn iter_all_pins_mut(&mut self) -> impl Iterator<Item = &mut Pin> {
        self.top_pins
            .iter_mut()
            .chain(self.bottom_pins.iter_mut())
            .chain(self.left_pins.iter_mut())
            .chain(self.right_pins.iter_mut())
            .chain(
                self.grid
                    .iter_mut()
                    .flat_map(|g| g.cells.iter_mut().map(|c| &mut c.pin)),
            )
    }

    /// Auto-assigns partner functions when `source_pin` receives `func`: the
    /// MISO/MOSI that go with an SCK, the RX that goes with a TX.
    ///
    /// The pins come from the same scoring a whole module's wiring goes through
    /// ([`autowire::pick_partners`]) — which is what keeps the peripheral on ONE
    /// pad group. Picking the first available pin instead (what this did until
    /// 2026-08-12) answered PA5 SCK with PB4/PB5, mixing the F1 SPI1 default set
    /// with its remap set: a combination one AFIO bit cannot express, that no
    /// `stm32f1xx_hal::spi::Pins` impl accepts, and that therefore generated a
    /// project which could not compile.
    pub fn auto_assign_partners(&mut self, source_pin: usize, func: &PinFunction) {
        let picks = autowire::pick_partners(self, source_pin, func);
        for (partner, num) in picks {
            if let Some(pin) = self.find_pin_mut(num) {
                pin.selected_function = partner;
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

    /// Ask the editor to jump to the line that defines pin `pin_num`'s variable.
    /// A no-op for a pin with no function — it has no generated binding yet, and
    /// silently doing nothing is better than scrolling somewhere arbitrary.
    /// Consumed by `AppIde` (the panel owns the editor, the MCU doesn't).
    pub fn request_pin_goto(&mut self, pin_num: usize) {
        let configured = self
            .find_pin(pin_num)
            .is_some_and(|p| p.selected_function != PinFunction::Unset);
        if configured {
            self.pin_goto = Some(pin_num);
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

        // A pad re-pointed at another channel of the SAME timer keeps what the
        // user set for it. Done HERE, before `reconcile_modules` rebuilds the
        // wires, because this is the one door both entrances use — the canvas's
        // function list and the module panel's channel picker.
        self.carry_pwm_channel(&old_func, &func);

        // A pin re-purposed away from USART must drop any virtual-module wire.
        self.reconcile_modules();

        Some(changed)
    }

    /// Move a channel's duty (and its shape) when a pad changes which channel
    /// of the same timer it drives.
    ///
    /// Only when the OLD channel is left with no pad at all: two pads on one
    /// timer swapping channels between them must not have one of them drag the
    /// other's duty away.
    fn carry_pwm_channel(&mut self, old: &PinFunction, new: &PinFunction) {
        use crate::panels::mcu_module::modules::ModuleConfig;
        let (
            PinFunction::TimerPwm {
                timer: t_old,
                channel: from,
            },
            PinFunction::TimerPwm {
                timer: t_new,
                channel: to,
            },
        ) = (old, new)
        else {
            return;
        };
        if t_old != t_new || from == to {
            return;
        }
        let (timer, from, to) = (*t_old, *from, *to);
        // Still driven from somewhere else? Then its duty is not orphaned and
        // moving it would steal a live setting.
        let still_used = self.iter_all_pins().any(|p| {
            p.selected_function
                == PinFunction::TimerPwm {
                    timer,
                    channel: from,
                }
        });
        if still_used {
            return;
        }
        for m in &mut self.modules {
            if let ModuleConfig::Timer(cfg) = &mut m.config
                && cfg.instance == timer
            {
                cfg.move_channel(from, to);
            }
        }
    }

    /// Drop each module connection whose pin no longer carries the matching
    /// USART function — so re-purposing a pin disconnects the _USART from it
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
            Connection, ModuleKind, ModuleSignal, VirtualModule, module_signal_of,
        };
        use std::collections::BTreeMap;

        let mut wanted: BTreeMap<(ModuleKind, u8), Vec<(ModuleSignal, usize)>> = BTreeMap::new();
        for p in self.iter_all_pins() {
            if let Some((kind, inst, sig)) = module_signal_of(&p.selected_function) {
                wanted
                    .entry((kind, inst))
                    .or_default()
                    .push((sig, p.number));
            }
        }

        // Drop modules whose peripheral no longer has any assigned pins — but
        // NEVER a Custom one: it is authored by the user, not derived from the
        // pins, so only an explicit Remove takes it away.
        self.modules
            .retain(|m| m.kind.is_custom() || wanted.contains_key(&(m.kind, m.instance())));

        // A custom module's wires mirror its own pin list (which the config
        // panel edits), so rebuild them here — the canvas then draws them with
        // the same machinery as every peripheral module.
        for m in self.modules.iter_mut().filter(|m| m.kind.is_custom()) {
            let pins: Vec<usize> = match &m.config {
                crate::panels::mcu_module::modules::ModuleConfig::Custom(c) => c.pins.clone(),
                _ => Vec::new(),
            };
            m.connections = pins
                .into_iter()
                .map(|mcu_pin| Connection {
                    signal: ModuleSignal::CustomPin,
                    mcu_pin,
                })
                .collect();
        }

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
                let id = self.free_module_id(&kind.short().to_ascii_lowercase());
                self.modules.push(VirtualModule {
                    id,
                    kind,
                    name: format!("{}{inst}", kind.short()),
                    pos: (0.0, 0.0),
                    config: kind.default_config(inst),
                    connections,
                });
            }
        }
    }

    /// A module id nothing else is using.
    ///
    /// # Why `len() + 1` was not one
    ///
    /// Both id factories used to be `format!("{base}_{}", self.modules.len() + 1)`
    /// — which is "one more than however many modules happen to exist", not "the
    /// next free number". Remove a module and add another and the new one takes
    /// an id that is still taken:
    ///
    /// ```text
    /// wire USART0 + USART1   -> usart_1, usart_2
    /// unwire USART0          -> usart_2
    /// wire USART0 again      -> usart_2, usart_2      <- both, same id
    /// ```
    ///
    /// The id is not decoration. It keys the list's `CollapsingState`, so two
    /// modules sharing one open together and cannot be opened apart; it is the
    /// `push_id` namespace for the whole config grid, so every widget inside
    /// both collides and egui paints its ID-clash banner over the panel; and it
    /// names the `mod <id>` block `ensure_module_models` writes into main.rs.
    ///
    /// Starts at the old number and walks up, so an id that was free stays the
    /// id it always was — only a collision moves.
    fn free_module_id(&self, base: &str) -> String {
        let mut n = self.modules.len() + 1;
        loop {
            let id = format!("{base}_{n}");
            if !self.modules.iter().any(|m| m.id == id) {
                return id;
            }
            n += 1;
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
mod module_id_tests {
    use crate::panels::mcu_module::modules::ModuleKind;
    use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

    fn ids(mcu: &super::Mcu) -> Vec<String> {
        mcu.modules.iter().map(|m| m.id.clone()).collect()
    }

    fn assert_distinct(mcu: &super::Mcu, when: &str) {
        let got = ids(mcu);
        let mut u = got.clone();
        u.sort();
        u.dedup();
        assert_eq!(
            u.len(),
            got.len(),
            "{when}: ids must be distinct, got {got:?}"
        );
    }

    /// Removing a module and adding another must not hand out an id that is
    /// still in use.
    ///
    /// The id was built from `modules.len() + 1` — "one more than however many
    /// exist", not "the next free one" — so this exact sequence produced TWO
    /// modules called `usart_2`. The id keys the list's `CollapsingState` and is
    /// the `push_id` namespace for the config grid, so the pair opened and
    /// closed together and egui painted its ID-clash banner over every widget in
    /// both of their grids.
    #[test]
    fn a_re_wired_module_does_not_reuse_a_live_id() {
        let mut mcu = crate::panels::mcu_module::builtins::builtin_for("esp32c3")
            .expect("a bundled ESP32-C3")
            .build_mcu();

        let mut wired = 0;
        for inst in [0u8, 1] {
            for f in [PinFunction::UsartTx(inst), PinFunction::UsartRx(inst)] {
                let free = mcu
                    .iter_all_pins()
                    .find(|p| {
                        p.selected_function == PinFunction::Unset
                            && p.available_functions.contains(&f)
                    })
                    .map(|p| p.number);
                if let Some(n) = free {
                    mcu.apply_pin_function(n, f);
                    wired += 1;
                }
            }
        }
        assert_eq!(wired, 4, "the C3 has two USARTs to wire");
        assert_eq!(mcu.modules.len(), 2);
        assert_distinct(&mcu, "freshly wired");

        // Move USART0 off its pads and back on — what a user does when the
        // board wants the peripheral somewhere else.
        let pads: Vec<usize> = mcu
            .iter_all_pins()
            .filter(|p| {
                matches!(
                    p.selected_function,
                    PinFunction::UsartTx(0) | PinFunction::UsartRx(0)
                )
            })
            .map(|p| p.number)
            .collect();
        for n in &pads {
            mcu.apply_pin_function(*n, PinFunction::Unset);
        }
        assert_eq!(mcu.modules.len(), 1, "USART0's module went with its pads");

        for n in &pads {
            let f = mcu
                .find_pin(*n)
                .unwrap()
                .available_functions
                .iter()
                .find(|f| matches!(f, PinFunction::UsartTx(0) | PinFunction::UsartRx(0)))
                .cloned()
                .expect("the pad still offers USART0");
            mcu.apply_pin_function(*n, f);
        }
        assert_eq!(mcu.modules.len(), 2, "and came back");
        assert_distinct(&mcu, "after a re-wire");
    }

    /// The Custom palette had the same defect, from the same expression.
    #[test]
    fn removing_a_custom_module_does_not_free_a_live_id() {
        let mut mcu = crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx();
        for _ in 0..3 {
            assert!(mcu.add_module(ModuleKind::Custom));
        }
        assert_distinct(&mcu, "three customs");

        // Drop the FIRST, so the count no longer matches the highest number.
        let first = mcu.modules[0].id.clone();
        mcu.modules.retain(|m| m.id != first);
        assert!(mcu.add_module(ModuleKind::Custom));
        assert_distinct(&mcu, "after removing one and adding another");
    }

    /// The numbering a project already has must not move: only a collision
    /// does. Otherwise a `mod <id>` data-model block already written into
    /// main.rs would be orphaned by a rename it never asked for.
    #[test]
    fn an_uncontested_id_keeps_the_number_it_always_had() {
        let mut mcu = crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        assert_eq!(mcu.modules[0].id, "usart_1");
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        assert_eq!(mcu.modules[1].id, "spi_2");
    }
}

#[cfg(test)]
mod reset_pins_tests {
    use crate::panels::mcu_module::create_stm32f103c8tx;
    use crate::panels::mcu_module::modules::ModuleKind;
    use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

    /// The count the header's confirm names, and the reset it confirms.
    ///
    /// Reserved pins (VDD/VSS/NRST) never count: they carry no function to
    /// clear, so including them would inflate the number the question shows.
    #[test]
    fn the_count_is_what_reset_actually_clears() {
        let mut mcu = create_stm32f103c8tx();
        assert_eq!(mcu.configured_pin_count(), 0, "a fresh chip has none");

        mcu.apply_pin_function(10, PinFunction::GpioOutput);
        mcu.apply_pin_function(11, PinFunction::GpioInput);
        assert_eq!(mcu.configured_pin_count(), 2);

        // A module wires several pins at once — all of them count.
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        let with_module = mcu.configured_pin_count();
        assert!(with_module > 2, "the USART's pins count too: {with_module}");

        mcu.reset_all_pins();
        assert_eq!(mcu.configured_pin_count(), 0);
        assert!(
            mcu.iter_all_pins()
                .all(|p| p.reserved || p.selected_function == PinFunction::Unset)
        );
        // And the modules go with the pins they were wired to — which is the
        // part of the loss the confirm has to warn about.
        mcu.reconcile_modules();
        assert!(mcu.modules.is_empty(), "{:?}", mcu.modules.len());
    }
}

#[cfg(test)]
mod pin_search_tests {
    use crate::panels::mcu_module::create_stm32f103c8tx;

    #[test]
    fn name_matches_any_part_case_insensitively() {
        let mut mcu = create_stm32f103c8tx();
        mcu.pin_search = "pb1".to_owned();
        let hits = mcu.pin_search_hits();
        // PB1 and PB10..PB15 — a substring, which is what a half-remembered
        // name needs.
        let names: Vec<String> = mcu
            .iter_all_pins()
            .filter(|p| hits.contains(&p.number))
            .map(|p| p.name.clone())
            .collect();
        assert!(names.contains(&"PB1".to_owned()), "{names:?}");
        assert!(names.contains(&"PB12".to_owned()), "{names:?}");
        assert!(!names.iter().any(|n| n.starts_with("PA")), "{names:?}");

        // Case doesn't matter.
        mcu.pin_search = "PB1".to_owned();
        assert_eq!(mcu.pin_search_hits(), hits);
    }

    /// On a ball-grid package the pin NUMBER is our own ordinal and is never
    /// drawn — the designator under the ball is what the user reads, so it has
    /// to be searchable (the reported bug: "N13" found nothing).
    #[test]
    fn ball_designator_is_searchable() {
        use crate::panels::mcu_module::mcu::model::{GridCell, PinGrid};
        use crate::panels::mcu_module::pins::logic::pin::Pin;

        let mut mcu = create_stm32f103c8tx();
        // Row 11 = "M", row 12 = "N" (JEDEC skips I/O/Q/S/X/Z); col 12 = "13".
        mcu.grid = Some(PinGrid {
            rows: 13,
            cols: 13,
            cells: vec![
                GridCell {
                    row: 12,
                    col: 12,
                    pin: Pin::new(900, "PH12"),
                },
                GridCell {
                    row: 11,
                    col: 11,
                    pin: Pin::new(901, "PH11"),
                },
            ],
        });
        mcu.pin_search = "N13".to_owned();
        let hits = mcu.pin_search_hits();
        assert!(hits.contains(&900), "designator N13 -> {hits:?}");
        assert!(!hits.contains(&901), "M12 must stay dimmed: {hits:?}");
        // Lower case works the same, and the NAME still matches on its own.
        mcu.pin_search = "n13".to_owned();
        assert!(mcu.pin_search_hits().contains(&900));
        mcu.pin_search = "ph12".to_owned();
        assert!(mcu.pin_search_hits().contains(&900));
    }

    /// A number is EXACT: "13" is pin 13, not 13 + 1 + 31.
    #[test]
    fn number_matches_exactly() {
        let mut mcu = create_stm32f103c8tx();
        mcu.pin_search = "13".to_owned();
        let hits = mcu.pin_search_hits();
        assert!(hits.contains(&13));
        assert!(!hits.contains(&1) && !hits.contains(&31), "{hits:?}");
    }

    /// Empty box, or a query nothing matches → NOTHING is dimmed. Fading the
    /// whole chip while the user is mid-word would be noise.
    #[test]
    fn no_query_and_no_match_both_dim_nothing() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.pin_search_highlight().is_none());
        mcu.pin_search = "   ".to_owned();
        assert!(mcu.pin_search_highlight().is_none());
        mcu.pin_search = "zzz".to_owned();
        assert!(mcu.pin_search_hits().is_empty());
        assert!(mcu.pin_search_highlight().is_none());
        mcu.pin_search = "pa5".to_owned();
        assert!(mcu.pin_search_highlight().is_some());
    }
}

#[cfg(test)]
mod iomode_persist_tests {
    use crate::panels::mcu_module::create_stm32f103c8tx;
    use crate::panels::mcu_module::pins::logic::pin::GpioMode;
    use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

    /// A chosen GPIO mode survives save → load through `mcu.config` `@iomode`.
    #[test]
    fn gpio_modes_round_trip_through_mcu_config() {
        let mut mcu = create_stm32f103c8tx();
        mcu.apply_pin_function(10, PinFunction::GpioOutput);
        mcu.apply_pin_function(11, PinFunction::GpioInput);
        mcu.find_pin_mut(10).unwrap().io_mode = Some(GpioMode::OpenDrain);
        mcu.find_pin_mut(11).unwrap().io_mode = Some(GpioMode::PullDown);

        let text = mcu.mcu_config_text();
        assert!(text.contains("@iomode"), "{text}");

        let mut reloaded = create_stm32f103c8tx();
        reloaded.apply_pin_function(10, PinFunction::GpioOutput);
        reloaded.apply_pin_function(11, PinFunction::GpioInput);
        reloaded.apply_mcu_config(&text);
        assert_eq!(
            reloaded.find_pin(10).unwrap().io_mode,
            Some(GpioMode::OpenDrain)
        );
        assert_eq!(
            reloaded.find_pin(11).unwrap().io_mode,
            Some(GpioMode::PullDown)
        );
    }

    /// A project that never touched a mode writes NO section at all, so its
    /// `mcu.config` is byte-identical to what older versions produced.
    #[test]
    fn untouched_modes_write_no_section() {
        let mut mcu = create_stm32f103c8tx();
        mcu.apply_pin_function(10, PinFunction::GpioOutput);
        assert!(!mcu.mcu_config_text().contains("@iomode"));
    }
}

#[cfg(test)]
mod module_support_tests {
    use crate::panels::mcu_module::modules::ModuleKind;
    use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
    use crate::panels::mcu_module::{create_esp32c3, create_stm32f103c8tx};

    /// The S2 and S3 have the touch sensors and esp-hal has no driver for them,
    /// so the palette keeps the entry and says why instead of dropping it.
    ///
    /// The distinction that matters: a chip with no touch AT ALL gets no
    /// sentence, because there is nothing to explain — the module is simply not
    /// its. Claiming otherwise would be worse than silence.
    #[test]
    fn a_peripheral_without_a_driver_says_so_instead_of_vanishing() {
        let touch = ModuleKind::GenericInterfaceTouch;
        for chip in ["esp32s2", "esp32s3"] {
            let mcu = crate::panels::mcu_module::builtins::builtin_for(chip)
                .unwrap()
                .build_mcu();
            assert!(!mcu.supports_module(touch), "{chip}: no pads, so no module");
            let why = mcu
                .hardware_only_reason(touch)
                .unwrap_or_else(|| panic!("{chip} should explain itself"));
            assert!(why.contains("esp-hal"), "{chip}: names the real limit");
        }

        // The original ESP32 has the driver, so it is offered outright.
        let esp32 = crate::panels::mcu_module::builtins::builtin_for("esp32")
            .unwrap()
            .build_mcu();
        assert!(esp32.supports_module(touch));
        assert!(esp32.hardware_only_reason(touch).is_none());

        // …and a chip with no touch silicon says nothing at all.
        for chip in ["esp32c6", "stm32f103c8t6"] {
            let mcu = crate::panels::mcu_module::builtins::builtin_for(chip)
                .unwrap()
                .build_mcu();
            assert!(
                mcu.hardware_only_reason(touch).is_none(),
                "{chip} has no touch to explain"
            );
        }

        // Nothing else claims to be hardware-only anywhere, so the palette is
        // unchanged for every kind but this one.
        for kind in ModuleKind::ALL {
            if kind == touch {
                continue;
            }
            for chip in ["esp32", "esp32s2", "esp32s3", "esp32c6", "stm32f103c8t6"] {
                let mcu = crate::panels::mcu_module::builtins::builtin_for(chip)
                    .unwrap()
                    .build_mcu();
                assert!(
                    mcu.hardware_only_reason(kind).is_none(),
                    "{chip} vs {}",
                    kind.short()
                );
            }
        }
    }

    /// Both bundled chips genuinely expose all five interfaces, so a fresh
    /// palette offers everything.
    #[test]
    fn bundled_chips_support_every_kind_they_have_pins_for() {
        for mcu in [create_stm32f103c8tx(), create_esp32c3()] {
            for kind in ModuleKind::ALL {
                // The exceptions are the RULE working: the palette is derived
                // from the PINS, and a built-in that does not name those pads
                // does not offer the module. No built-in has an LPUART (the
                // F103 predates the peripheral, the ESP32-C3 has no such
                // thing), the F103 spells out no SAI, SD-card, external-memory
                // or DAC pads, and neither does the C3.
                //
                // I2S is the one that MOVED: the ESP32-C3 has an I2S block that
                // esp-hal drives, so its pads now carry the four I2S functions
                // and the module is offered. The F103's I2S rides on its SPI
                // block and its hand-written definition still does not name the
                // pads, so it stays out.
                let esp = mcu.name.starts_with("ESP32");
                let want = !matches!(
                    kind,
                    ModuleKind::GenericInterfaceLpuart
                        // PCNT and MCPWM are Espressif's, and neither built-in
                        // has either: no STM32 does, and the ESP32-C3 is one of
                        // the parts esp-hal gives neither driver to.
                        | ModuleKind::GenericInterfacePcnt
                        | ModuleKind::GenericInterfaceMcpwm
                        // PARL_IO likewise: only three ESP parts have one, and
                        // the C3 is not among them.
                        | ModuleKind::GenericInterfaceParlIo
                        // …and its receiving half, which is its own kind.
                        | ModuleKind::GenericInterfaceParlIoRx
                        // LCD_CAM is the ESP32-S3's alone, and neither built-in
                        // is one - so neither offers the pads.
                        | ModuleKind::GenericInterfaceLcdCam
                        // …and so is its camera half.
                        | ModuleKind::GenericInterfaceCamera
                        // Touch is the original ESP32's alone, for the same
                        // reason: esp-hal builds no driver for the C3.
                        | ModuleKind::GenericInterfaceTouch
                        | ModuleKind::GenericInterfaceSai
                        | ModuleKind::GenericInterfaceSdmmc
                        | ModuleKind::GenericInterfaceQspi
                        | ModuleKind::GenericInterfaceOspi
                        | ModuleKind::GenericInterfaceXspi
                        | ModuleKind::GenericInterfaceHspi
                        | ModuleKind::GenericInterfaceDac
                ) && (kind != ModuleKind::GenericInterfaceI2s || esp)
                    // RMT is Espressif's outright: no STM32 pin ever carries it,
                    // and the C3's do, so the palette offers it on one and not
                    // the other.
                    && (kind != ModuleKind::GenericInterfaceRmt || esp);
                assert_eq!(
                    mcu.supports_module(kind),
                    want,
                    "{} vs {}",
                    mcu.name,
                    kind.short()
                );
            }
        }
    }

    /// USB is the one kind gated by FAMILY as well as by pins: the D-/D+ pins
    /// exist on chips whose backend writes no USB code, where adding the module
    /// only ever produced two stray dependencies.
    #[test]
    fn usb_is_offered_only_where_the_backend_generates_it() {
        let mut mcu = create_stm32f103c8tx();
        assert!(
            mcu.supports_module(ModuleKind::GenericInterfaceUsb),
            "F1 generates the whole CDC device"
        );

        // Same chip, same pins, a family whose backend emits no USB code.
        for family in ["stm32f4", "stm32h5", "stm32wba"] {
            mcu.family = family.to_string();
            assert!(
                !mcu.supports_module(ModuleKind::GenericInterfaceUsb),
                "{family} must not offer USB"
            );
            // …and every other kind is unaffected by the gate.
            assert!(
                mcu.supports_module(ModuleKind::GenericInterfaceUsart),
                "{family} still offers USART"
            );
            assert!(mcu.supports_module(ModuleKind::GenericInterfaceSpi));
        }

        // ESP acknowledges its hardware-fixed USB peripheral in the generated
        // code, so the module still means something there.
        mcu.family = "esp32c3".into();
        assert!(mcu.supports_module(ModuleKind::GenericInterfaceUsb));
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
            "no USB pins -> the kind is hidden"
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

#[cfg(test)]
mod moving_a_signal_to_another_pad {
    use crate::panels::mcu_module::builtins::builtin_definitions;
    use crate::panels::mcu_module::modules::{ModuleConfig, ModuleKind, ModuleSignal};
    use crate::panels::mcu_module::pins::PinFunction;

    /// A Pico, because the bundled F103 models no remap pads at all - each of
    /// its three USART TX signals sits on exactly one pad, so there is nowhere
    /// to move to and the case cannot be expressed there. On an RP the same
    /// UART reaches four pads.
    fn usart_mcu() -> crate::panels::mcu_module::mcu::Mcu {
        let mut mcu = builtin_definitions()
            .into_iter()
            .find(|d| d.id == "rp2040_pico")
            .expect("built-in Pico")
            .build_mcu();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        mcu
    }

    fn pin_of(mcu: &crate::panels::mcu_module::mcu::Mcu, sig: ModuleSignal) -> usize {
        mcu.modules[0]
            .connections
            .iter()
            .find(|c| c.signal == sig)
            .map(|c| c.mcu_pin)
            .expect("signal is wired")
    }

    /// The module survives the move with its config, and the wire does not
    /// double.
    ///
    /// Both halves are the reason this is not `apply_pin_function` twice:
    /// clearing first drags the partner to `Unset` and the re-created module
    /// gets a fresh `default_config`; setting first leaves two pads on one
    /// function and `reconcile_modules` makes two connections out of them.
    #[test]
    fn the_module_and_its_settings_come_along() {
        let mut mcu = usart_mcu();
        // A setting worth losing, so the test can see it survive.
        if let ModuleConfig::Usart(c) = &mut mcu.modules[0].config {
            c.baud_rate = 9600;
        }
        let tx = pin_of(&mcu, ModuleSignal::Tx);
        let rx = pin_of(&mcu, ModuleSignal::Rx);
        let want = mcu.find_pin(tx).expect("tx pin").selected_function.clone();

        // Somewhere else the same TX can go.
        let dest = mcu
            .iter_all_pins()
            .find(|p| {
                p.number != tx
                    && !p.reserved
                    && p.available_functions.contains(&want)
                    && p.selected_function == PinFunction::Unset
            })
            .map(|p| p.number)
            .expect("the F103 remaps USART TX to a second pad");

        assert!(mcu.move_pin_function(tx, dest));

        assert_eq!(mcu.modules.len(), 1, "still one module");
        assert_eq!(pin_of(&mcu, ModuleSignal::Tx), dest, "TX moved");
        assert_eq!(pin_of(&mcu, ModuleSignal::Rx), rx, "RX did not");
        assert_eq!(
            mcu.modules[0]
                .connections
                .iter()
                .filter(|c| c.signal == ModuleSignal::Tx)
                .count(),
            1,
            "one wire, not two"
        );
        assert_eq!(
            mcu.find_pin(tx).expect("old pad").selected_function,
            PinFunction::Unset,
            "the old pad is free again"
        );
        match &mcu.modules[0].config {
            ModuleConfig::Usart(c) => assert_eq!(c.baud_rate, 9600, "the config survived"),
            other => panic!("still a USART: {other:?}"),
        }
    }

    /// A destination that cannot carry the signal changes nothing at all.
    #[test]
    fn an_impossible_move_is_refused_whole() {
        let mut mcu = usart_mcu();
        let tx = pin_of(&mcu, ModuleSignal::Tx);
        let before: Vec<(usize, PinFunction)> = mcu
            .iter_all_pins()
            .map(|p| (p.number, p.selected_function.clone()))
            .collect();
        // A pad that offers no USART TX at all.
        let want = mcu.find_pin(tx).expect("tx").selected_function.clone();
        let bad = mcu
            .iter_all_pins()
            .find(|p| !p.reserved && !p.available_functions.contains(&want))
            .map(|p| p.number)
            .expect("some pad cannot carry a USART TX");
        assert!(!mcu.move_pin_function(tx, bad));
        let after: Vec<(usize, PinFunction)> = mcu
            .iter_all_pins()
            .map(|p| (p.number, p.selected_function.clone()))
            .collect();
        assert_eq!(before, after, "nothing moved");
        assert!(
            !mcu.move_pin_function(tx, tx),
            "a move onto itself is a no-op"
        );
    }
}

#[cfg(test)]
mod a_wiring_is_committed_whole_or_not_at_all {
    use crate::panels::mcu_module::builtins::builtin_definitions;
    use crate::panels::mcu_module::modules::ModuleSignal;
    use crate::panels::mcu_module::pins::PinFunction;

    /// `add_module_wired` is public and the dialog holds its choice across
    /// frames, so the check belongs in the model rather than only in the panel:
    /// a pad that cannot carry the signal must not be written at all.
    #[test]
    fn a_pad_that_cannot_carry_the_signal_is_refused() {
        let mut mcu = builtin_definitions()
            .into_iter()
            .find(|d| d.id == "stm32f103c8t6")
            .expect("built-in F103")
            .build_mcu();
        let want = ModuleSignal::Tx.pin_function(0);
        let bad = mcu
            .iter_all_pins()
            .find(|p| !p.reserved && !p.available_functions.contains(&want))
            .map(|p| p.number)
            .expect("a pad with no USART0 TX");

        assert!(!mcu.add_module_wired(0, &[(ModuleSignal::Tx, bad)]));
        assert_eq!(
            mcu.find_pin(bad).expect("the pad").selected_function,
            PinFunction::Unset,
            "nothing was written"
        );
        assert!(mcu.modules.is_empty(), "and no module appeared");
    }

    /// Whole or not at all: one bad pad must not leave the good ones written.
    #[test]
    fn one_bad_pad_rolls_the_whole_set_back() {
        let mut mcu = builtin_definitions()
            .into_iter()
            .find(|d| d.id == "rp2040_pico")
            .expect("built-in Pico")
            .build_mcu();
        let tx_want = ModuleSignal::Tx.pin_function(0);
        let good = mcu
            .iter_all_pins()
            .find(|p| p.available_functions.contains(&tx_want))
            .map(|p| p.number)
            .expect("a UART0 TX pad");
        let rx_want = ModuleSignal::Rx.pin_function(0);
        let bad = mcu
            .iter_all_pins()
            .find(|p| !p.reserved && !p.available_functions.contains(&rx_want))
            .map(|p| p.number)
            .expect("a pad with no UART0 RX");

        assert!(!mcu.add_module_wired(0, &[(ModuleSignal::Tx, good), (ModuleSignal::Rx, bad)]));
        assert_eq!(
            mcu.find_pin(good).expect("the good pad").selected_function,
            PinFunction::Unset,
            "the pad that COULD have been written was not"
        );
    }
}

#[cfg(test)]
mod device_groups {
    use crate::panels::mcu_module::builtins::builtin_definitions;
    use crate::panels::mcu_module::mcu::Mcu;
    use crate::panels::mcu_module::mcu_config::PinGroup;
    use crate::panels::mcu_module::modules::{ModuleKind, ModuleSignal};
    use crate::panels::mcu_module::pins::PinFunction;

    fn pico() -> Mcu {
        builtin_definitions()
            .into_iter()
            .find(|d| d.id == "rp2040_pico")
            .expect("built-in Pico")
            .build_mcu()
    }

    /// Three free pads that can all host a plain output, so a move between two of
    /// them is legal on the silicon.
    fn three_output_pads(mcu: &Mcu) -> (usize, usize, usize) {
        let free: Vec<usize> = mcu
            .iter_all_pins()
            .filter(|p| {
                !p.reserved
                    && p.selected_function == PinFunction::Unset
                    && p.available_functions.contains(&PinFunction::GpioOutput)
            })
            .map(|p| p.number)
            .take(3)
            .collect();
        assert_eq!(free.len(), 3, "the Pico has three free GPIOs");
        (free[0], free[1], free[2])
    }

    fn named(mcu: &Mcu, name: &str) -> Option<Vec<usize>> {
        mcu.groups
            .iter()
            .find(|g| g.name == name)
            .map(|g| g.pins.iter().copied().collect())
    }

    /// A pad belongs to ONE device. Two accent colours on one stub would read as
    /// a drawing error, and `join_group` finds a group by name - so a pad in two
    /// of them would answer to whichever came first.
    #[test]
    fn a_pad_belongs_to_one_device_at_a_time() {
        let mut mcu = pico();
        mcu.join_group(7, "radar");
        mcu.join_group(8, "radar");
        mcu.join_group(7, "display");
        assert_eq!(named(&mcu, "radar"), Some(vec![8]));
        assert_eq!(named(&mcu, "display"), Some(vec![7]));
    }

    /// The empty name is how a pad leaves - the roster's × and the same call.
    /// The device it leaves behind disappears only when nothing is left in it.
    #[test]
    fn the_last_pad_out_takes_the_device_with_it() {
        let mut mcu = pico();
        mcu.join_group(7, "radar");
        mcu.join_group(8, "radar");
        mcu.join_group(7, "");
        assert_eq!(
            named(&mcu, "radar"),
            Some(vec![8]),
            "one pad left, still a device"
        );
        mcu.join_group(8, "");
        assert!(mcu.groups.is_empty(), "nothing left, no device");
    }

    /// A device created from the roster is empty until the next gesture fills
    /// it. Grouping something ELSE in between must not sweep it away - the row
    /// vanishing under the user's cursor is indistinguishable from a bug.
    #[test]
    fn a_device_with_nothing_in_it_yet_survives_the_next_grouping() {
        let mut mcu = pico();
        mcu.new_group("Device 2".into());
        mcu.join_group(7, "Device 1");
        assert!(
            mcu.groups.iter().any(|g| g.name == "Device 2"),
            "the unfilled device is still on the roster"
        );
    }

    /// Moving a pad WITHIN its own device empties the group in passing. It must
    /// not be collected as a casualty of that.
    #[test]
    fn regrouping_a_pad_into_its_own_device_keeps_it() {
        let mut mcu = pico();
        mcu.join_group(7, "radar");
        mcu.join_group(7, "radar");
        assert_eq!(named(&mcu, "radar"), Some(vec![7]));
    }

    /// Renaming onto a name already taken MERGES: `join_group` looks a group up
    /// by name, so two rows sharing one would draw one colour and only ever fill
    /// one of them.
    #[test]
    fn renaming_onto_a_taken_name_merges_the_two() {
        let mut mcu = pico();
        mcu.join_group(7, "radar");
        mcu.join_group(8, "sensor");
        mcu.rename_group(1, "radar");
        assert_eq!(mcu.groups.len(), 1);
        assert_eq!(named(&mcu, "radar"), Some(vec![7, 8]));
    }

    /// A group is a set of PAD NUMBERS, so a signal that changes pad would drop
    /// out of its device unless the move rewrites the set. `move_pin_function`
    /// is the only place in the app where a signal changes pad.
    #[test]
    fn a_device_follows_its_pad_across_a_move() {
        let mut mcu = pico();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        let tx = mcu.modules[0]
            .pin_for(ModuleSignal::Tx)
            .expect("the UART got a TX pad");
        let want = mcu
            .find_pin(tx)
            .expect("the TX pad")
            .selected_function
            .clone();
        let dest = mcu
            .iter_all_pins()
            .find(|p| {
                p.number != tx
                    && !p.reserved
                    && p.available_functions.contains(&want)
                    && p.selected_function == PinFunction::Unset
            })
            .map(|p| p.number)
            .expect("the RP maps the same TX to a second pad");

        mcu.join_group(tx, "radar");
        assert!(mcu.move_pin_function(tx, dest));
        assert_eq!(
            named(&mcu, "radar"),
            Some(vec![dest]),
            "the device followed the signal to its new pad"
        );
    }

    /// The pad tick, the box bar and the io bar all ask `group_of_pin`, so it has
    /// to answer with the same predicate everything else uses. A device whose name
    /// the user cleared draws no mat and writes no comment — it may not keep
    /// marking its pads either.
    #[test]
    fn a_nameless_device_marks_none_of_its_pads() {
        let mut mcu = pico();
        mcu.join_group(7, "radar");
        assert!(mcu.group_of_pin(7).is_some());
        mcu.rename_group(0, "  ");
        assert_eq!(mcu.groups.len(), 1, "still on the roster");
        assert!(
            mcu.group_of_pin(7).is_none(),
            "but nothing on the canvas answers for it"
        );
    }

    /// A pad added under a padded spelling of a device's name joins THAT device
    /// rather than starting a second one beside it.
    #[test]
    fn a_pad_joins_a_device_whose_name_differs_only_in_padding() {
        let mut mcu = pico();
        mcu.join_group(7, "mw radar");
        mcu.join_group(8, " mw radar ");
        assert_eq!(mcu.groups.len(), 1);
        assert_eq!(named(&mcu, "mw radar"), Some(vec![7, 8]));
    }

    /// Membership is derived for a MODULE, never stored: `reconcile_modules`
    /// deletes and re-creates a module under a fresh id on an ordinary edit, and
    /// a stored id would lose its device every time.
    #[test]
    fn a_module_is_in_the_device_holding_any_of_its_pads() {
        let mut mcu = pico();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        let m = mcu.modules[0].clone();
        assert!(mcu.group_of_module(&m).is_none());
        mcu.join_group(m.connections[0].mcu_pin, "radar");
        assert_eq!(
            mcu.group_of_module(&m).map(|g| g.name.as_str()),
            Some("radar")
        );
        // …and the roster's gesture puts the WHOLE bus in.
        mcu.join_group_module(&m, "radar");
        assert_eq!(
            named(&mcu, "radar").map(|v| v.len()),
            Some(m.connections.len())
        );
    }

    /// The whole point of keying by pad: a device outlives the module that
    /// carried it, because it never knew the module's id.
    #[test]
    fn a_device_outlives_the_module_id_it_was_grouped_through() {
        let mut mcu = pico();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        let m = mcu.modules[0].clone();
        mcu.join_group_module(&m, "radar");
        let before = m.id.clone();
        mcu.remove_module(&before);
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        // Same pads, and the device is still on them.
        let again = mcu.modules[0].clone();
        assert_eq!(
            mcu.group_of_module(&again).map(|g| g.name.as_str()),
            Some("radar"),
            "the re-created module is still part of the device"
        );
    }

    /// A pad keeps its device when its function goes away, so the destination of
    /// a move is not necessarily device-free. Handing it the mover's device
    /// without taking it out of its own left one pad in two devices at once, and
    /// `group_of_pin` then answered by Vec order.
    #[test]
    fn a_move_never_leaves_a_pad_in_two_devices() {
        let mut mcu = pico();
        let (from, to, spare) = three_output_pads(&mcu);
        mcu.apply_pin_function(from, PinFunction::GpioOutput);
        mcu.join_group(from, "radar");
        // The destination carries no function, but does carry a device.
        mcu.join_group(to, "display");
        mcu.join_group(spare, "display");

        assert!(mcu.move_pin_function(from, to));

        let holders: Vec<&str> = mcu
            .groups
            .iter()
            .filter(|g| g.pins.contains(&to))
            .map(|g| g.name.as_str())
            .collect();
        assert_eq!(holders, ["radar"], "the destination is in exactly one device");
        assert_eq!(named(&mcu, "radar"), Some(vec![to]));
        assert_eq!(
            named(&mcu, "display"),
            Some(vec![spare]),
            "and display kept the rest"
        );
    }

    /// The device the move empties disappears — but only that one.
    #[test]
    fn a_move_onto_a_devices_last_pad_retires_that_device() {
        let mut mcu = pico();
        let (from, to, _) = three_output_pads(&mcu);
        mcu.apply_pin_function(from, PinFunction::GpioOutput);
        mcu.join_group(from, "radar");
        mcu.join_group(to, "display");
        mcu.new_group("unfilled".into());

        assert!(mcu.move_pin_function(from, to));

        assert!(named(&mcu, "display").is_none(), "it had only that pad");
        assert_eq!(named(&mcu, "radar"), Some(vec![to]));
        assert!(
            mcu.groups.iter().any(|g| g.name == "unfilled"),
            "a device the user has not filled yet is not swept up"
        );
    }

    /// A device the user has not named is not a device yet: it stays on the
    /// roster and reaches neither `mcu.config` nor the generated comment. The
    /// three used to disagree, so an unnamed device was written into main.rs as
    /// a nameless `// : PA4, PA5` and then lost on the next save.
    #[test]
    fn an_unnamed_device_reaches_neither_the_file_nor_the_comment() {
        let mut mcu = pico();
        mcu.join_group(7, "radar");
        mcu.rename_group(0, "");
        assert_eq!(mcu.groups.len(), 1, "still on the roster");
        assert!(!mcu.groups[0].is_live());
        assert!(!mcu.mcu_config_text().contains("@groups"));
        assert_eq!(
            crate::panels::mcu_module::codegen::common::device_comment(&mcu),
            ""
        );
    }

    /// `mcu.config` is the only place a device is stored, so the section has to
    /// survive the app's own write-then-read.
    #[test]
    fn a_device_round_trips_through_mcu_config() {
        let mut mcu = pico();
        mcu.join_group(7, "mw radar");
        mcu.join_group(8, "mw radar");
        let text = mcu.mcu_config_text();
        let mut back = pico();
        back.apply_mcu_config(&text);
        assert_eq!(
            back.groups,
            vec![PinGroup {
                name: "mw radar".into(),
                pins: [7, 8].into_iter().collect(),
            }]
        );
    }
}
