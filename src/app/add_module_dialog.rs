//! "Choose pins…" — the second door out of the `+ Add module` palette.
//!
//! The palette's plain entries stay one click: autowire picks a wiring and
//! commits it. That is right most of the time and wrong in a way the IDE cannot
//! detect, because the ranking is a heuristic over geometry
//! ([`autowire::Score`](crate::panels::mcu_module::modules::autowire)) and the
//! board's own reasons for wanting a particular pad — a header already routed,
//! a level shifter, a pad kept free for a probe — are nowhere in the model.
//!
//! So this is not a replacement for the automatic pick, it is an override of
//! it: the dialog opens SEEDED with exactly what autowire would have chosen, so
//! confirming without touching anything reproduces the one-click behaviour.
//!
//! Everything it offers comes from the chip
//! ([`autowire::eligible`](crate::panels::mcu_module::modules::autowire::eligible),
//! uncapped), not from the search's candidate window — a menu that showed 8 of
//! an ESP32-C3's 21 UART pads would look exactly as arbitrary as the pick the
//! user opened this to correct.

use eframe::egui;
use std::collections::HashSet;

use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::modules::{ModuleKind, ModuleSignal, autowire};

/// What the dialog is editing, kept on `AppIde` across frames.
#[derive(Clone, Debug)]
pub struct AddModulePick {
    pub kind: ModuleKind,
    /// The peripheral instance. Autowire ranks this LAST, so a compact wiring
    /// on USART3 can win while USART1 was free the whole time — which is one of
    /// the ways "it landed somewhere else" happens.
    pub instance: u8,
    /// `(signal, chosen pad)` for the required signals — always present.
    pub required: Vec<(ModuleSignal, usize)>,
    /// `(signal, chosen pad or none)` for the optional ones. An SPI NSS the
    /// board drives in software is a real choice, so "none" is offered rather
    /// than the pad being forced.
    pub optional: Vec<(ModuleSignal, Option<usize>)>,
}

/// What the caller should do after drawing one frame of the dialog.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PickOutcome {
    /// Still open.
    Pending,
    Cancelled,
    /// Commit through `Mcu::add_module_wired`.
    Confirmed,
}

impl AddModulePick {
    /// Seed from what autowire would have picked, so Enter == the one-click
    /// path. `None` when the chip cannot host this module at all — the palette
    /// already greys those out, but a chip can fill up between frames.
    pub fn seed(mcu: &Mcu, kind: ModuleKind) -> Option<Self> {
        // A Custom module wires nothing: `add_module` creates it empty and its
        // pads are added in its own config panel. There is no wiring to seed,
        // and a dialog with no rows would have an Add button that did nothing.
        if kind.is_custom() {
            return None;
        }
        let (required, optional) = kind.signals();
        let used: HashSet<usize> = mcu
            .modules
            .iter()
            .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
            .collect();
        let used_instances: HashSet<u8> = mcu
            .modules
            .iter()
            .filter(|m| m.kind == kind)
            .map(|m| m.instance())
            .collect();
        let (inst, chosen) = autowire::pick_pins(mcu, &used, &used_instances, required, optional)?;
        Some(Self {
            kind,
            instance: inst,
            required: required
                .iter()
                .map(|&sig| {
                    let pad = chosen
                        .iter()
                        .find(|(s, _)| *s == sig)
                        .map(|(_, n)| *n)
                        .unwrap_or_default();
                    (sig, pad)
                })
                .collect(),
            optional: optional
                .iter()
                .map(|&sig| (sig, chosen.iter().find(|(s, _)| *s == sig).map(|(_, n)| *n)))
                .collect(),
        })
    }

    /// The wiring as `add_module_wired` wants it.
    pub fn wiring(&self) -> Vec<(ModuleSignal, usize)> {
        self.required
            .iter()
            .copied()
            .chain(self.optional.iter().filter_map(|&(s, n)| n.map(|n| (s, n))))
            .collect()
    }

    /// Pads this dialog has already spent, so no two rows offer the same one.
    fn spoken_for(&self, except: ModuleSignal) -> HashSet<usize> {
        self.required
            .iter()
            .filter(|(s, _)| *s != except)
            .map(|(_, n)| *n)
            .chain(
                self.optional
                    .iter()
                    .filter(|(s, _)| *s != except)
                    .filter_map(|(_, n)| *n),
            )
            .collect()
    }

    /// Whether `pad` can still carry `sig` on `inst` - i.e. whether the row is
    /// still true. Re-asked every frame: this dialog outlives the state it was
    /// seeded from.
    fn still_free(
        mcu: &Mcu,
        used: &HashSet<usize>,
        sig: ModuleSignal,
        inst: u8,
        pad: usize,
    ) -> bool {
        let want = sig.pin_function(inst);
        !used.contains(&pad)
            && mcu.find_pin(pad).is_some_and(|p| {
                !p.reserved
                    && p.available_functions.contains(&want)
                    && (p.selected_function == crate::panels::mcu_module::pins::PinFunction::Unset
                        || p.selected_function == want)
            })
    }

    /// Move any row whose pad has been taken since the dialog opened onto a pad
    /// that is still free, and blank an optional one that has nowhere left.
    fn drop_taken_pads(&mut self, mcu: &Mcu, used: &HashSet<usize>) {
        for i in 0..self.required.len() {
            let (sig, pad) = self.required[i];
            if Self::still_free(mcu, used, sig, self.instance, pad) {
                continue;
            }
            let mut taken = self.spoken_for(sig);
            taken.extend(used.iter().copied());
            if let Some(free) = autowire::eligible(mcu, &taken, sig, self.instance).first() {
                self.required[i].1 = *free;
            }
        }
        for i in 0..self.optional.len() {
            let (sig, pad) = self.optional[i];
            let Some(pad) = pad else { continue };
            if Self::still_free(mcu, used, sig, self.instance, pad) {
                continue;
            }
            let mut taken = self.spoken_for(sig);
            taken.extend(used.iter().copied());
            self.optional[i].1 = autowire::eligible(mcu, &taken, sig, self.instance)
                .first()
                .copied();
        }
    }

    /// Every required row still points at a pad the chip can wire.
    ///
    /// False when the chip filled up under an open dialog and nothing was left
    /// to move a row to - `Add` is disabled rather than committing a wiring
    /// that would steal a pad from the module that took it.
    pub fn is_formable(&self, mcu: &Mcu, used: &HashSet<usize>) -> bool {
        // The PERIPHERAL can be taken while this is open too, not just the
        // pads - and that failure is quieter: `add_module_wired` would succeed,
        // `reconcile_modules` would fold the new pads into the module that
        // already holds the instance, and the user would get no new module and
        // a second TX row on their old one.
        let instance_taken = self
            .modules_of_this_kind(mcu)
            .any(|inst| inst == self.instance);
        !self.required.is_empty()
            && !instance_taken
            && self
                .required
                .iter()
                .all(|&(sig, pad)| Self::still_free(mcu, used, sig, self.instance, pad))
    }

    /// Instances of this module kind the chip already hosts.
    fn modules_of_this_kind<'a>(&'a self, mcu: &'a Mcu) -> impl Iterator<Item = u8> + 'a {
        mcu.modules
            .iter()
            .filter(move |m| m.kind == self.kind)
            .map(|m| m.instance())
    }

    /// Move to a still-free peripheral when the seeded one was taken.
    ///
    /// The pads follow, because the pads of USART1 are not the pads of USART3.
    /// When nothing is free `is_formable` says so and `Add` is refused.
    fn drop_taken_instance(&mut self, mcu: &Mcu, used: &HashSet<usize>) {
        if !self.modules_of_this_kind(mcu).any(|i| i == self.instance) {
            return;
        }
        let taken: HashSet<u8> = self.modules_of_this_kind(mcu).collect();
        let (required, _) = self.kind.signals();
        if let Some(free) = autowire::instances_for(mcu, used, &taken, required).first() {
            self.instance = *free;
            self.reseed_for_instance(mcu, used);
        }
    }

    /// Re-seed every row after the instance changed — the pads of USART1 are
    /// not the pads of USART3, so keeping the old numbers would leave the
    /// dialog showing a wiring the chip cannot form.
    fn reseed_for_instance(&mut self, mcu: &Mcu, used: &HashSet<usize>) {
        for i in 0..self.required.len() {
            let sig = self.required[i].0;
            let mut taken = self.spoken_for(sig);
            taken.extend(used.iter().copied());
            if let Some(first) = autowire::eligible(mcu, &taken, sig, self.instance).first() {
                self.required[i].1 = *first;
            }
        }
        for i in 0..self.optional.len() {
            let sig = self.optional[i].0;
            let mut taken = self.spoken_for(sig);
            taken.extend(used.iter().copied());
            self.optional[i].1 = autowire::eligible(mcu, &taken, sig, self.instance)
                .first()
                .copied();
        }
    }
}

/// Whether `pick` still describes the chip it was seeded from.
///
/// The dialog is drawn from the Pins tab's own block, so switching tabs simply
/// stops drawing it - and it would come back later, holding pad numbers and an
/// instance from whatever chip was loaded then. Cheaper and more honest to
/// cancel it than to try to keep it alive across a project load.
pub fn belongs_to(pick: &AddModulePick, mcu: &Mcu) -> bool {
    pick.required
        .iter()
        .all(|&(_, pad)| mcu.find_pin(pad).is_some())
}

/// Draw one frame of the dialog. Mutates `pick` in place; the caller commits.
pub fn show(ui: &mut egui::Ui, mcu: &Mcu, pick: &mut AddModulePick) -> PickOutcome {
    let used: HashSet<usize> = mcu
        .modules
        .iter()
        .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
        .collect();
    let used_instances: HashSet<u8> = mcu
        .modules
        .iter()
        .filter(|m| m.kind == pick.kind)
        .map(|m| m.instance())
        .collect();
    let (required, _) = pick.kind.signals();
    let instances = autowire::instances_for(mcu, &used, &used_instances, required);
    // The chip moves while this is open - it is not modal, and the palette
    // behind it still adds modules. A pad that was free when the dialog was
    // seeded can be someone else's by now; keeping it would commit a wiring
    // that overwrites their pad, and a module left with no connections is
    // dropped by `reconcile_modules` along with everything configured on it.
    pick.drop_taken_instance(mcu, &used);
    pick.drop_taken_pads(mcu, &used);

    let name = |n: usize| {
        mcu.find_pin(n)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| format!("pin{n}"))
    };

    let mut outcome = PickOutcome::Pending;
    let mut new_instance: Option<u8> = None;

    egui::Window::new(format!("Add {}", pick.kind.short()))
        .id(egui::Id::new("add_module_pick"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "Seeded with the wiring the automatic pick would have used - confirm to take it.",
                )
                .size(11.0)
                .color(egui::Color32::from_gray(150)),
            );
            ui.add_space(6.0);

            egui::Grid::new("add_module_pick_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    // The peripheral, when the chip has more than one free.
                    ui.label("Peripheral");
                    if instances.len() > 1 {
                        egui::ComboBox::from_id_salt("add_module_instance")
                            .selected_text(format!("{}{}", pick.kind.short(), pick.instance))
                            .show_ui(ui, |ui| {
                                for i in &instances {
                                    if ui
                                        .selectable_label(
                                            *i == pick.instance,
                                            format!("{}{}", pick.kind.short(), i),
                                        )
                                        .clicked()
                                    {
                                        new_instance = Some(*i);
                                    }
                                }
                            });
                    } else {
                        ui.label(
                            egui::RichText::new(format!("{}{}", pick.kind.short(), pick.instance))
                                .color(egui::Color32::from_gray(170)),
                        );
                    }
                    ui.end_row();

                    for i in 0..pick.required.len() {
                        let sig = pick.required[i].0;
                        let mut taken = pick.spoken_for(sig);
                        taken.extend(used.iter().copied());
                        let pads = autowire::eligible(mcu, &taken, sig, pick.instance);
                        ui.label(sig.label());
                        pad_combo(ui, &mut pick.required[i].1, &pads, &name, sig.label());
                        ui.end_row();
                    }

                    for i in 0..pick.optional.len() {
                        let sig = pick.optional[i].0;
                        let mut taken = pick.spoken_for(sig);
                        taken.extend(used.iter().copied());
                        let pads = autowire::eligible(mcu, &taken, sig, pick.instance);
                        ui.label(
                            egui::RichText::new(format!("{} (optional)", sig.label()))
                                .color(egui::Color32::from_gray(170)),
                        );
                        optional_pad_combo(ui, &mut pick.optional[i].1, &pads, &name, sig.label());
                        ui.end_row();
                    }
                });

            // The one rule the pin model cannot express, said rather than
            // enforced: on an STM32F1 a peripheral's pads move TOGETHER (one
            // AFIO remap bit), so a set drawn from two ports is a wiring the
            // silicon has no way to form. `autowire` only ever expressed this
            // as a score, so nothing downstream will catch it either.
            if let Some(warning) = mixed_group_warning(mcu, pick) {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(warning)
                        .size(10.5)
                        .color(egui::Color32::from_rgb(220, 160, 70)),
                );
            }

            // The chip can fill up while this is open, and then there is no
            // wiring left to commit. Refusing here rather than at the model is
            // the only place that can SAY why.
            let formable = pick.is_formable(mcu, &used);
            if !formable {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "This peripheral or its pads were taken while the dialog was open, and nothing free is left to move to. Close this and free one first.",
                    )
                    .size(10.5)
                    .color(egui::Color32::from_rgb(220, 160, 70)),
                );
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(formable, egui::Button::new("Add"))
                    .clicked()
                {
                    outcome = PickOutcome::Confirmed;
                }
                if ui.button("Cancel").clicked() {
                    outcome = PickOutcome::Cancelled;
                }
            });
            ui.add_space(2.0);
        });

    if let Some(i) = new_instance {
        pick.instance = i;
        pick.reseed_for_instance(mcu, &used);
    }
    outcome
}

/// A required signal's pad: never empty, because the instance was only offered
/// when every required signal had somewhere to go.
fn pad_combo(
    ui: &mut egui::Ui,
    current: &mut usize,
    pads: &[usize],
    name: &dyn Fn(usize) -> String,
    salt: &str,
) {
    // The pad in `current` is held by this row, so `eligible` (which skips
    // taken pads) does not list it - put it back at the front or the combo
    // would show a value it cannot re-select.
    let mut all: Vec<usize> = pads.to_vec();
    if !all.contains(current) {
        all.insert(0, *current);
    }
    egui::ComboBox::from_id_salt(("add_module_pad", salt))
        .selected_text(name(*current))
        .show_ui(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for n in &all {
                        if ui.selectable_label(*n == *current, name(*n)).clicked() {
                            *current = *n;
                        }
                    }
                });
        });
}

fn optional_pad_combo(
    ui: &mut egui::Ui,
    current: &mut Option<usize>,
    pads: &[usize],
    name: &dyn Fn(usize) -> String,
    salt: &str,
) {
    let mut all: Vec<usize> = pads.to_vec();
    if let Some(c) = *current
        && !all.contains(&c)
    {
        all.insert(0, c);
    }
    egui::ComboBox::from_id_salt(("add_module_pad_opt", salt))
        .selected_text(current.map_or_else(|| "- none -".to_owned(), name))
        .show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), "- none -").clicked() {
                *current = None;
            }
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
                    for n in &all {
                        if ui
                            .selectable_label(*current == Some(*n), name(*n))
                            .clicked()
                        {
                            *current = Some(*n);
                        }
                    }
                });
        });
}

/// The GPIO port a pin name belongs to, the way `autowire` reads it.
fn port_of(name: &str) -> &str {
    let end = name
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(name.len());
    &name[..end]
}

/// Warn when the chosen pads span two GPIO ports on a family whose remap moves
/// a peripheral's pins as a group. Only STM32F1: elsewhere each pad has its own
/// alternate-function mux and a mixed set is perfectly legal — and on RP/ESP
/// every pin is in one "port" anyway, so the test would never fire.
fn mixed_group_warning(mcu: &Mcu, pick: &AddModulePick) -> Option<String> {
    if mcu.family != "stm32f1" {
        return None;
    }
    let ports: std::collections::BTreeSet<String> = pick
        .wiring()
        .iter()
        .filter_map(|(_, n)| mcu.find_pin(*n))
        .map(|p| port_of(&p.name).to_owned())
        .collect();
    if ports.len() < 2 {
        return None;
    }
    Some(format!(
        "{} These pads sit on {} - on an STM32F1 one AFIO bit remaps a peripheral's pins TOGETHER, so a mixed set is usually not a wiring the chip can form.",
        egui_phosphor::regular::WARNING,
        ports.into_iter().collect::<Vec<_>>().join(" and ")
    ))
}

#[cfg(test)]
mod choosing_the_pins {
    use super::*;
    use crate::panels::mcu_module::builtins::builtin_definitions;

    fn chip(id: &str) -> Mcu {
        builtin_definitions()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("built-in {id}"))
            .build_mcu()
    }

    fn wired_pads(mcu: &Mcu) -> Vec<usize> {
        let mut v: Vec<usize> = mcu
            .modules
            .iter()
            .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
            .collect();
        v.sort_unstable();
        v
    }

    /// Confirming the dialog without touching a row IS the one-click add.
    ///
    /// The whole design rests on this: the override is seeded from the same
    /// `pick_pins` the plain entry runs, so choosing "Choose pins..." can never
    /// cost the user the good default. If the two ever diverge, the dialog has
    /// become a second wiring policy.
    #[test]
    fn the_seed_is_exactly_what_one_click_would_have_done() {
        for id in ["rp2040_pico", "stm32f103c8t6", "esp32c3"] {
            for kind in [
                ModuleKind::GenericInterfaceUsart,
                ModuleKind::GenericInterfaceSpi,
                ModuleKind::GenericInterfaceI2c,
            ] {
                let mut one_click = chip(id);
                if !one_click.add_module(kind) {
                    continue;
                }
                let mut via_dialog = chip(id);
                let pick = AddModulePick::seed(&via_dialog, kind)
                    .unwrap_or_else(|| panic!("{id}/{kind:?}: seeded"));
                via_dialog.add_module_wired(pick.instance, &pick.wiring());

                assert_eq!(
                    wired_pads(&one_click),
                    wired_pads(&via_dialog),
                    "{id}/{kind:?}: same pads"
                );
                assert_eq!(
                    one_click.modules.len(),
                    via_dialog.modules.len(),
                    "{id}/{kind:?}: same modules"
                );
                assert_eq!(
                    one_click.modules[0].instance(),
                    via_dialog.modules[0].instance(),
                    "{id}/{kind:?}: same peripheral"
                );
            }
        }
    }

    /// The pads the user may pick are the CHIP's, not the search window's.
    #[test]
    fn a_row_offers_every_pad_the_chip_can_route() {
        let mcu = chip("esp32c3");
        let pick = AddModulePick::seed(&mcu, ModuleKind::GenericInterfaceUsart).expect("seeded");
        let sig = pick.required[0].0;
        let mut taken = pick.spoken_for(sig);
        taken.extend(std::iter::once(pick.required[0].1));
        let pads = autowire::eligible(&mcu, &HashSet::new(), sig, pick.instance);
        assert!(
            pads.len() > 8,
            "a C3 routes this signal to more than the search window: {}",
            pads.len()
        );
    }

    /// A wiring the user actually changed lands where they put it.
    #[test]
    fn a_hand_picked_pad_is_the_one_that_gets_wired() {
        let mut mcu = chip("rp2040_pico");
        let mut pick =
            AddModulePick::seed(&mcu, ModuleKind::GenericInterfaceUsart).expect("seeded");
        let sig = pick.required[0].0;
        let seeded = pick.required[0].1;
        let elsewhere = *autowire::eligible(&mcu, &HashSet::new(), sig, pick.instance)
            .iter()
            .find(|n| **n != seeded)
            .expect("a Pico UART signal reaches several pads");
        pick.required[0].1 = elsewhere;

        mcu.add_module_wired(pick.instance, &pick.wiring());
        assert!(
            wired_pads(&mcu).contains(&elsewhere),
            "the chosen pad is wired"
        );
        assert!(!wired_pads(&mcu).contains(&seeded), "the seeded one is not");
    }

    /// An optional signal set to "none" simply is not wired.
    #[test]
    fn an_optional_signal_can_be_left_out() {
        let mut mcu = chip("stm32f103c8t6");
        let mut pick = AddModulePick::seed(&mcu, ModuleKind::GenericInterfaceSpi).expect("seeded");
        if pick.optional.is_empty() {
            return; // this chip's SPI has no optional signal to drop
        }
        let with = pick.wiring().len();
        for o in &mut pick.optional {
            o.1 = None;
        }
        let without = pick.wiring().len();
        assert!(without < with, "dropping NSS drops a wire");
        mcu.add_module_wired(pick.instance, &pick.wiring());
        assert_eq!(mcu.modules[0].connections.len(), without);
    }

    /// The instance is a real choice, and on some chips a wider one than the
    /// automatic pick's - which ranks the instance LAST.
    #[test]
    fn more_than_one_peripheral_is_offered_where_the_chip_has_them() {
        let mcu = chip("stm32f103c8t6");
        let (required, _) = ModuleKind::GenericInterfaceUsart.signals();
        let insts = autowire::instances_for(&mcu, &HashSet::new(), &HashSet::new(), required);
        assert!(insts.len() > 1, "the F103 has three USARTs: {insts:?}");
    }
}

/// Cases an adversarial pass over this file found, kept as the net for them.
#[cfg(test)]
mod the_chip_moves_under_an_open_dialog {
    use super::*;
    use crate::panels::mcu_module::builtins::builtin_definitions;
    use crate::panels::mcu_module::modules::ModuleConfig;

    fn chip(id: &str) -> Mcu {
        builtin_definitions()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("built-in {id}"))
            .build_mcu()
    }

    fn used_pads(mcu: &Mcu) -> HashSet<usize> {
        mcu.modules
            .iter()
            .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
            .collect()
    }

    /// The dialog is NOT modal: the palette behind it still adds modules, and
    /// on an RP2040 they compete for the same pads - GP0/GP1 are UART0 TX/RX
    /// and I2C0 SDA/SCL both. Confirming a dialog seeded before that used to
    /// overwrite the pads, leave the I2C module with no connections, and let
    /// `reconcile_modules` drop it with everything configured on it.
    #[test]
    fn a_module_added_while_it_was_open_is_not_deleted_by_confirming() {
        let mut mcu = chip("rp2040_pico");
        let mut pick =
            AddModulePick::seed(&mcu, ModuleKind::GenericInterfaceUsart).expect("seeded");

        assert!(mcu.add_module(ModuleKind::GenericInterfaceI2c));
        if let ModuleConfig::I2c(c) = &mut mcu.modules[0].config {
            c.custom_label = "display".into();
        }
        let i2c_id = mcu.modules[0].id.clone();

        // What the dialog does on its next frame, before anything is drawn.
        pick.drop_taken_pads(&mcu, &used_pads(&mcu));
        assert!(mcu.add_module_wired(pick.instance, &pick.wiring()));

        let i2c = mcu
            .modules
            .iter()
            .find(|m| m.id == i2c_id)
            .expect("the I2C module survived");
        match &i2c.config {
            ModuleConfig::I2c(c) => assert_eq!(c.custom_label, "display", "and its config"),
            other => panic!("still an I2C: {other:?}"),
        }
        assert!(
            mcu.modules
                .iter()
                .any(|m| m.kind == ModuleKind::GenericInterfaceUsart),
            "and the USART was still added, on pads of its own"
        );
    }

    /// When there is nowhere left to move a row to, `Add` is refused rather
    /// than committing a wiring that would steal a pad.
    #[test]
    fn a_wiring_with_nowhere_to_go_is_not_formable() {
        let mut mcu = chip("stm32f103c8t6");
        let pick = AddModulePick::seed(&mcu, ModuleKind::GenericInterfaceUsart).expect("seeded");
        // Take every pad the seeded wiring wants.
        for (_, pad) in &pick.wiring() {
            if let Some(p) = mcu.find_pin_mut(*pad) {
                p.selected_function = crate::panels::mcu_module::pins::PinFunction::GpioOutput;
            }
        }
        assert!(!pick.is_formable(&mcu, &used_pads(&mcu)));
        assert!(
            !mcu.add_module_wired(pick.instance, &pick.wiring()),
            "and the model refuses it too, whatever the panel does"
        );
    }

    /// A Custom module has no signals, so there is nothing to choose - the
    /// palette keeps it a one-click button. A dialog seeded for it would have
    /// had no rows and an Add that did nothing.
    #[test]
    fn a_custom_module_never_reaches_this_dialog() {
        let mut mcu = chip("rp2040_pico");
        assert!(AddModulePick::seed(&mcu, ModuleKind::Custom).is_none());
        assert!(
            mcu.add_module(ModuleKind::Custom),
            "one-click still makes it"
        );
        assert_eq!(mcu.modules.len(), 1);
    }
}

#[cfg(test)]
mod the_peripheral_can_be_taken_too {
    use super::*;
    use crate::panels::mcu_module::builtins::builtin_definitions;

    fn chip(id: &str) -> Mcu {
        builtin_definitions()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("built-in {id}"))
            .build_mcu()
    }

    fn used_pads(mcu: &Mcu) -> HashSet<usize> {
        mcu.modules
            .iter()
            .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
            .collect()
    }

    /// Re-homing the PADS is not enough: the peripheral can go too.
    ///
    /// The quieter failure of the two. With the instance stale,
    /// `add_module_wired` succeeds and `reconcile_modules` folds the new pads
    /// into the module that already holds it - no new module appears, and the
    /// user's existing USART0 silently grows a second TX and a second RX.
    #[test]
    fn a_stale_instance_moves_to_a_free_one_instead_of_folding_in() {
        let mut mcu = chip("rp2040_pico");
        let mut pick =
            AddModulePick::seed(&mcu, ModuleKind::GenericInterfaceUsart).expect("seeded");
        let seeded_instance = pick.instance;

        // The palette behind the open dialog takes that very peripheral.
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        assert_eq!(mcu.modules.len(), 1);
        assert_eq!(mcu.modules[0].instance(), seeded_instance);

        pick.drop_taken_instance(&mcu, &used_pads(&mcu));
        pick.drop_taken_pads(&mcu, &used_pads(&mcu));

        if pick.is_formable(&mcu, &used_pads(&mcu)) {
            assert_ne!(pick.instance, seeded_instance, "it moved off the taken one");
            assert!(mcu.add_module_wired(pick.instance, &pick.wiring()));
            assert_eq!(mcu.modules.len(), 2, "a SECOND module, not a fatter first");
            for m in &mcu.modules {
                let tx = m
                    .connections
                    .iter()
                    .filter(|c| c.signal == ModuleSignal::Tx)
                    .count();
                assert!(tx <= 1, "one TX per module, got {tx}");
            }
        } else {
            // The chip had only one UART free: then Add is refused outright,
            // which is the other half of the same guard.
            assert!(!mcu.add_module_wired(pick.instance, &pick.wiring()));
        }
    }

    /// A dialog seeded on another chip does not survive a project switch.
    #[test]
    fn a_dialog_from_another_chip_is_not_kept() {
        let pico = chip("rp2040_pico");
        let pick = AddModulePick::seed(&pico, ModuleKind::GenericInterfaceUsart).expect("seeded");
        assert!(belongs_to(&pick, &pico));

        // A chip whose pin numbering does not cover the Pico's pads.
        let other = chip("stm32f103c8t6");
        let unknown = pick
            .required
            .iter()
            .any(|&(_, pad)| other.find_pin(pad).is_none());
        if unknown {
            assert!(!belongs_to(&pick, &other), "cancelled, not re-used");
        }
    }
}
