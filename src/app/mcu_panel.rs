//! Central "MCU Configurator" panel — chip label, tab bar (Pins / Peripherals
//! / Clock / System) and the active tab's content.
//!
//! Inherent method on [`AppIde`]; the Pins tab draws the chip and, on any pin
//! change, re-syncs the generated `pins/` files.

use super::tabs::show_peripherals_tab;
use super::{AppIde, McuTab, ProjectFileId};
use eframe::egui;
use egui_phosphor::regular as ph;

impl AppIde {
    /// Render the central MCU configurator panel.
    ///
    /// Two-level navigation (2026-07-10): a GROUP row — "MCU" (chip config)
    /// and "Project" (chip-agnostic) — then the active group's own tab row:
    /// MCU → Pins / Peripherals / Clock / System; Project → Structure /
    /// Definition. Clicking a group returns to its last-used tab; the chip
    /// header row (Chip label + Reset pins) shows only for the MCU group.
    pub(super) fn show_mcu_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            // The Definition tab (F12 snippet) exists only while a snippet is
            // loaded — auto-leave it the moment the snippet clears.
            if self.definition_view.is_none() && self.active_tab == McuTab::Definition {
                self.active_tab = self.definition_return_tab;
            }
            // The second editor only renders while ITS tab is active. Without
            // this, both of its flags would keep their last value forever:
            // `reference_was_focused` would go on stealing the main editor's
            // shortcuts, and a completion it owns would keep swallowing
            // Enter/Escape for a popup nobody can see.
            if self.active_tab != McuTab::Reference || self.reference_file.is_none() {
                self.reference_was_focused = false;
                self.reference_ctrl_space = false;
                if self.completion_owner == crate::app::EditorSlot::Reference {
                    self.completion_open = false;
                    self.completion_note = None;
                }
            }
            // Track each group's last-used tab so group clicks restore it.
            if self.active_tab.is_project_group() {
                self.project_group_last = self.active_tab;
            } else {
                self.mcu_group_last = self.active_tab;
            }
            let project_active = self.active_tab.is_project_group();

            // ── Level 1: group selector ────────────────────────────────────
            ui.horizontal(|ui| {
                for (label, is_project) in [("MCU", false), ("Project", true)] {
                    let active = project_active == is_project;
                    let text = egui::RichText::new(label).size(14.0).strong().color(
                        if active {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::from_rgb(150, 150, 160)
                        },
                    );
                    if ui.selectable_label(active, text).clicked() && !active {
                        self.active_tab = if is_project {
                            // A remembered Definition tab needs its snippet.
                            if self.project_group_last == McuTab::Definition
                                && self.definition_view.is_none()
                            {
                                McuTab::Structure
                            } else {
                                self.project_group_last
                            }
                        } else {
                            self.mcu_group_last
                        };
                    }
                }
                // Reset pins — a chip operation, shown with the MCU group only.
                if !project_active {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let reset_btn = ui
                            .add(egui::Button::new(
                                egui::RichText::new(format!(
                                    "{} Reset pins",
                                    ph::ARROW_COUNTER_CLOCKWISE
                                ))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(220, 100, 80)),
                            ))
                            .on_hover_text("Clear all pin function selections");
                        if reset_btn.clicked() {
                            if let Some(mcu) = &mut self.mcu {
                                mcu.reset_all_pins();
                            }
                        }
                    });
                }
            });

            // Chip label — MCU group only (read-only; selection happens in the
            // "New Project" popup). The Project group doesn't involve the chip.
            if !project_active {
                ui.horizontal(|ui| {
                    ui.label("Chip:");
                    ui.label(
                        egui::RichText::new(self.selected_label())
                            .strong()
                            .color(egui::Color32::LIGHT_BLUE),
                    );
                    ui.label(
                        egui::RichText::new(format!("·  {}", self.selected_family()))
                            .color(egui::Color32::GRAY)
                            .size(11.0),
                    );
                });
            }

            ui.separator();

            // ── Level 2: the active group's tab row ────────────────────────
            ui.horizontal(|ui| {
                let mut tabs: Vec<McuTab> = if project_active {
                    let mut t = vec![McuTab::Structure];
                    if self.definition_view.is_some() {
                        t.push(McuTab::Definition);
                    }
                    if self.reference_file.is_some() {
                        t.push(McuTab::Reference);
                    }
                    t
                } else {
                    vec![
                        McuTab::Pins,
                        McuTab::Peripherals,
                        McuTab::Clock,
                        McuTab::System,
                    ]
                };
                for tab in tabs.drain(..) {
                    let is_active = self.active_tab == tab;
                    // The second editor is named after its FILE — "Reference"
                    // says nothing about which one is open.
                    let text = if tab == McuTab::Reference {
                        self.reference_file
                            .as_deref()
                            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
                            .unwrap_or_else(|| tab.label().to_string())
                    } else {
                        tab.label().to_string()
                    };
                    let label = egui::RichText::new(text)
                        .size(13.0)
                        .color(if is_active {
                            egui::Color32::WHITE
                        } else if tab == McuTab::Definition {
                            egui::Color32::from_rgb(120, 180, 240)
                        } else if tab == McuTab::Reference {
                            egui::Color32::from_rgb(150, 200, 150)
                        } else {
                            egui::Color32::from_rgb(160, 160, 170)
                        });
                    if ui.selectable_label(is_active, label).clicked() {
                        self.active_tab = tab;
                    }
                }
                // Close button for the transient Definition tab.
                if project_active
                    && self.definition_view.is_some()
                    && ui
                        .add(egui::Button::new(egui::RichText::new(ph::X).size(10.0)).frame(false))
                        .on_hover_text("Close definition")
                        .clicked()
                {
                    self.definition_view = None;
                    if self.active_tab == McuTab::Definition {
                        self.active_tab = self.definition_return_tab;
                    }
                }
                // Same for the Reference tab.
                if project_active
                    && self.reference_file.is_some()
                    && self.active_tab == McuTab::Reference
                    && ui
                        .add(egui::Button::new(egui::RichText::new(ph::X).size(10.0)).frame(false))
                        .on_hover_text("Close the reference file")
                        .clicked()
                {
                    self.reference_file = None;
                    self.active_tab = McuTab::Structure;
                }
            });

            ui.separator();

            // Tab content
            match self.active_tab {
                McuTab::Pins => {
                    // ── Virtual-module palette + list, in a scrollable strip
                    //    BELOW the chip. Add a module (auto-wires to compatible
                    //    pins), rename it, edit its config, or remove it. Add/
                    //    remove change pin functions, so re-sync pins/ after.
                    let mut modules_changed = false;
                    // Set when the Rotate toggle is clicked → re-fit the Pins
                    // canvas so the re-oriented chip isn't left off-screen.
                    let mut rotate_toggled = false;
                    egui::TopBottomPanel::bottom("vmodules_panel")
                        .resizable(true)
                        .default_height(190.0)
                        .show_inside(ui, |ui| {
                            let Some(mcu) = &mut self.mcu else { return };
                            use crate::panels::mcu_module::mcu::gui::modules as mod_gui;
                            use crate::panels::mcu_module::modules::ModuleKind;

                            // A per-module "Init API" change (Portable | Native)
                            // below STAGES a codegen choice exactly like the
                            // System-tab Runtime cards — so show the SAME Apply /
                            // Discard prompt HERE. Without it the staged change was
                            // only visible on the System tab, leaving the user
                            // unsure whether the Init-API switch took effect.
                            Self::runtime_apply_bar(ui, mcu);

                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Virtual modules:")
                                        .size(12.0)
                                        .color(egui::Color32::from_rgb(150, 150, 160)),
                                );
                                // Only the kinds THIS chip's pins can host are
                                // offered (derived from the pins, so a new chip
                                // needs no per-family list). A supported-but-
                                // exhausted kind stays visible but disabled with
                                // the reason — a button that silently vanishes
                                // is more confusing than one that explains.
                                let mut any_supported = false;
                                for kind in ModuleKind::ALL {
                                    if !mcu.supports_module(kind) {
                                        continue; // this chip has no such pins
                                    }
                                    any_supported = true;
                                    let can_add = mcu.can_add_module(kind);
                                    let hover = match kind {
                                        ModuleKind::GenericInterfaceUsart => "Add a virtual USART device and auto-wire it to a free USART TX/RX pin pair",
                                        ModuleKind::GenericInterfaceSpi => "Add a virtual SPI device and auto-wire it to free SPI SCK/MOSI/MISO(/NSS) pins",
                                        ModuleKind::GenericInterfaceI2c => "Add a virtual I2C device and auto-wire it to a free I2C SCL/SDA pin pair",
                                        ModuleKind::GenericInterfaceCan => "Add a virtual CAN device and auto-wire it to the CAN RX/TX pins (needs the bxcan crate)",
                                        ModuleKind::GenericInterfaceUsb => "Add a virtual USB device and auto-wire it to the USB D-/D+ pins",
                                        ModuleKind::Custom => "Add a CUSTOM module — nothing is auto-wired: name it, add the pins you want, and a `configs/custom_<name>.rs` struct is generated for them",
                                    };
                                    // Colour the button's TEXT with the peripheral's
                                    // colour ONLY when a module of this kind is already
                                    // on the chip — a glance shows what's wired; the
                                    // rest stay neutral. No background fill (it would
                                    // hurt text clarity), matching the list names below.
                                    let added = mcu.modules.iter().any(|m| m.kind == kind);
                                    let label = format!("{} {}", ph::PLUS, kind.short());
                                    let text = if added {
                                        egui::RichText::new(label).color(mod_gui::module_color(kind, 1))
                                    } else {
                                        egui::RichText::new(label)
                                    };
                                    if ui
                                        .add_enabled(can_add, egui::Button::new(text))
                                        .on_hover_text(hover)
                                        .on_disabled_hover_text(if kind.is_single_instance() {
                                            "this chip has only one such peripheral and it's already used"
                                        } else {
                                            "every instance of this peripheral is already wired to a module — remove one to free it"
                                        })
                                        .clicked()
                                    {
                                        // Snapshot for Ctrl+Z BEFORE the add; drop it
                                        // again if the add found no free pins.
                                        mcu.push_module_undo(format!("Add {}", kind.short()));
                                        if mcu.add_module(kind) {
                                            modules_changed = true;
                                        } else {
                                            mcu.discard_last_module_undo();
                                        }
                                    }
                                }
                                // Undo the last module add/remove (also Ctrl+Z).
                                if mcu.can_undo_modules() {
                                    ui.separator();
                                    let hover = format!(
                                        "Undo: {}  (Ctrl+Z)",
                                        mcu.last_module_undo_label().unwrap_or("last change")
                                    );
                                    if ui
                                        .button(
                                            egui::RichText::new(format!(
                                                "{} Undo",
                                                ph::ARROW_COUNTER_CLOCKWISE
                                            ))
                                            .size(11.0),
                                        )
                                        .on_hover_text(hover)
                                        .clicked()
                                    {
                                        mcu.undo_modules();
                                        modules_changed = true;
                                    }
                                }
                                // View-only diagram rotation (persisted in
                                // mcu.config). A 4-sided chip toggles a 45°
                                // diamond, a 2-sided one 90° — to line pins &
                                // modules up horizontally. See mcu/gui/rotate.rs.
                                ui.separator();
                                let rot_hint = if mcu.is_quad_package() {
                                    "Rotate the chip 45° into a diamond — helps line up pins & modules. Toggle off to reset."
                                } else {
                                    "Rotate the chip 90° (vertical / horizontal) — helps line up pins & modules."
                                };
                                if ui
                                    .selectable_label(
                                        mcu.rotated,
                                        egui::RichText::new(format!(
                                            "{} Rotate",
                                            ph::ARROW_CLOCKWISE
                                        ))
                                        .size(11.0),
                                    )
                                    .on_hover_text(rot_hint)
                                    .clicked()
                                {
                                    mcu.rotated = !mcu.rotated;
                                    rotate_toggled = true;
                                    // ANY orientation change → clean auto-layout:
                                    // drop the manual drag positions (they don't
                                    // transfer between 0° / 90° / diamond) so the
                                    // modules + in/out fields snap beside their
                                    // pins for the new orientation.
                                    for m in &mut mcu.modules {
                                        m.pos = (0.0, 0.0);
                                    }
                                    mcu.io_pin_pos.clear();
                                }
                                if !any_supported {
                                    ui.label(
                                        egui::RichText::new(
                                            "this chip's pins offer no USART / SPI / I2C / CAN / USB interface",
                                        )
                                        .size(11.0)
                                        .italics()
                                        .color(egui::Color32::from_gray(130)),
                                    );
                                }
                            });

                            // Ctrl+Z reverts the last module add/remove — but only
                            // when NO text field has focus, so the editor's own undo
                            // still works while the user is typing.
                            let undo_z = ui.memory(|m| m.focused().is_none())
                                && ui.input_mut(|i| {
                                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)
                                });
                            if undo_z && mcu.can_undo_modules() {
                                mcu.undo_modules();
                                modules_changed = true;
                            }

                            // Id of a module clicked on the canvas last frame →
                            // TOGGLE its list entry this frame (expand if closed,
                            // collapse if open), then it's user-controlled again.
                            let to_open = mcu.expand_module.take();
                            // Empty-canvas click last frame → close EVERY entry
                            // (the canvas cleared its selection at the same time).
                            let collapse_all = std::mem::take(&mut mcu.collapse_modules);
                            // The module selectors follow the STAGED runtime (what
                            // the user is about to Apply), not the applied one.
                            let is_async = mcu.pending_is_async();
                            let is_native = mcu.pending_is_native();

                            if !mcu.modules.is_empty() {
                                use crate::panels::mcu_module::mcu::logic::module_style;
                                use crate::panels::mcu_module::modules::{ApiStyle, AsyncBusMode};
                                let pin_names: std::collections::HashMap<usize, String> = mcu
                                    .iter_all_pins()
                                    .map(|p| (p.number, p.name.clone()))
                                    .collect();
                                // Per-pin fingerprint (function + label) so a
                                // Custom module's Update button also lights up
                                // when a pin is renamed or flipped In->Out —
                                // both change the generated field names.
                                // Pins a Custom module must NOT offer — only FREE
                                // pins can be claimed:
                                //  • reserved (power / NRST — no user code),
                                //  • already assigned a function (a peripheral
                                //    pin is moved into its own `init(…)`, and a
                                //    configured GPIO already belongs somewhere),
                                //  • already claimed by another virtual module.
                                // Add the pin first, then give it its function —
                                // its rename field lives inside the module box.
                                let pin_blocked: std::collections::HashSet<usize> = mcu
                                    .iter_all_pins()
                                    .filter(|p| {
                                        p.reserved
                                            || p.selected_function
                                                != crate::panels::mcu_module::pins::logic::pin_function::PinFunction::Unset
                                    })
                                    .map(|p| p.number)
                                    .chain(mcu.modules.iter().flat_map(|other| {
                                        other.connections.iter().map(|c| c.mcu_pin)
                                    }))
                                    .collect();
                                // Editable copy of the per-pin labels: a Custom
                                // module's pin rows edit them here, mirroring the
                                // field inside its box on the canvas. Written back
                                // after the loop (which borrows `mcu.modules`).
                                let mut pin_labels: std::collections::HashMap<usize, String> = mcu
                                    .iter_all_pins()
                                    .map(|p| (p.number, p.custom_label.clone()))
                                    .collect();
                                let pin_labels_before = pin_labels.clone();
                                // Selectable functions per pin — a Custom module's
                                // pin buttons open this list, mirroring the picker
                                // inside the chip.
                                let pin_funcs: std::collections::HashMap<
                                    usize,
                                    Vec<crate::panels::mcu_module::pins::logic::pin_function::PinFunction>,
                                > = mcu
                                    .iter_all_pins()
                                    .filter(|p| !p.reserved)
                                    .map(|p| (p.number, p.available_functions.clone()))
                                    .collect();
                                // Set when the user picks one; applied after the
                                // loop (which holds `mcu.modules` mutably).
                                let pin_funcs_current: std::collections::HashMap<
                                    usize,
                                    crate::panels::mcu_module::pins::logic::pin_function::PinFunction,
                                > = mcu
                                    .iter_all_pins()
                                    .map(|p| (p.number, p.selected_function.clone()))
                                    .collect();
                                let mut pin_fn_choice: Option<(
                                    usize,
                                    crate::panels::mcu_module::pins::logic::pin_function::PinFunction,
                                )> = None;
                                let pin_sigs: std::collections::HashMap<usize, String> = mcu
                                    .iter_all_pins()
                                    .map(|p| {
                                        (
                                            p.number,
                                            format!("{:?}|{}", p.selected_function, p.custom_label),
                                        )
                                    })
                                    .collect();
                                // Removal is confirmed inline (it resets the module's
                                // pins). `confirm_id` = the module currently showing
                                // the confirm; the loop can't touch `mcu` while it
                                // borrows `mcu.modules`, so it signals via locals
                                // applied after the loop.
                                let confirm_id = mcu.module_remove_confirm.clone();
                                let mut remove_id: Option<String> = None; // confirmed → remove
                                let mut arm_confirm: Option<String> = None; // show the confirm
                                let mut cancel_confirm = false;
                                // Pull the staged per-module styles into a LOCAL map so
                                // the config panels can edit them without borrowing
                                // `mcu` while `mcu.modules` is iterated. Seeded from the
                                // current staged value, else the module's applied style.
                                let mut local_pending: std::collections::BTreeMap<
                                    String,
                                    (ApiStyle, AsyncBusMode),
                                > = mcu
                                    .modules
                                    .iter()
                                    .map(|m| {
                                        let cur = mcu
                                            .pending_module_styles
                                            .get(&m.id)
                                            .copied()
                                            .unwrap_or_else(|| module_style(&m.config));
                                        (m.id.clone(), cur)
                                    })
                                    .collect();
                                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                                    for m in &mut mcu.modules {
                                        let title = mod_gui::module_title(m);
                                        // The entry's name carries the module's
                                        // peripheral colour + a 10%-opacity tint
                                        // behind it, matching its box on the canvas.
                                        let mod_color = mod_gui::module_color(m.kind, m.instance());
                                        let toggle = to_open.as_deref() == Some(m.id.as_str());
                                        // Drive the section via CollapsingState so a canvas
                                        // click can TOGGLE (not just force-open) the entry.
                                        let cs_id =
                                            ui.make_persistent_id(("vmod_hdr", m.id.as_str()));
                                        let mut state =
                                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                                ui.ctx(),
                                                cs_id,
                                                false,
                                            );
                                        // "Collapse everything" outranks the
                                        // per-module toggle: both can't be true
                                        // in one frame (a canvas click either hit
                                        // a box or the background), but if they
                                        // ever were, closing is the safe answer.
                                        if collapse_all {
                                            state.set_open(false);
                                        } else if toggle {
                                            state.toggle(ui);
                                        }
                                        state
                                            .show_header(ui, |ui| {
                                                let bg = egui::Color32::from_rgba_unmultiplied(
                                                    mod_color.r(),
                                                    mod_color.g(),
                                                    mod_color.b(),
                                                    26, // ~10% opacity
                                                );
                                                egui::Frame::new()
                                                    .fill(bg)
                                                    .inner_margin(egui::Margin::symmetric(6, 2))
                                                    .corner_radius(egui::CornerRadius::same(4))
                                                    .show(ui, |ui| {
                                                        ui.label(
                                                            egui::RichText::new(title)
                                                                .strong()
                                                                .color(mod_color),
                                                        );
                                                    });
                                            })
                                            .body(|ui| {
                                                // Rename field — appended to the generated
                                                // variable name(s); also shown in the title.
                                                ui.horizontal(|ui| {
                                                    // Custom modules label their
                                                    // fields in BOLD (the user's
                                                    // ask) so the hand-authored
                                                    // panel reads apart from the
                                                    // peripheral ones.
                                                    // Fixed-width label there, so
                                                    // this field and the Struct
                                                    // one below it (a different
                                                    // container) line up.
                                                    if m.kind.is_custom() {
                                                        mod_gui::custom_field_label(ui, "Name:");
                                                    } else {
                                                        ui.label("Name:");
                                                    }
                                                    ui.add(
                                                        egui::TextEdit::singleline(
                                                            m.config.custom_label_mut(),
                                                        )
                                                        .hint_text("variable name")
                                                        .desired_width(mod_gui::CUSTOM_FIELD_W),
                                                    );
                                                });
                                                let pending = local_pending
                                                    .get_mut(&m.id)
                                                    .expect("pending entry seeded above");
                                                mod_gui::module_config_ui(
                                                    ui, m, &pin_names, &pin_sigs, &pin_blocked,
                                                    &mut pin_labels, &pin_funcs_current,
                                                    &pin_funcs, &mut pin_fn_choice, is_async,
                                                    is_native, pending,
                                                );
                                                ui.add_space(4.0);
                                                if confirm_id.as_deref() == Some(m.id.as_str()) {
                                                    // Armed → inline confirm (removing
                                                    // resets this module's pins).
                                                    ui.horizontal(|ui| {
                                                        ui.label(
                                                            egui::RichText::new(format!(
                                                                "{} Remove this module & free its pins?",
                                                                ph::WARNING
                                                            ))
                                                            .size(11.5)
                                                            .color(egui::Color32::from_rgb(220, 180, 90)),
                                                        );
                                                        if ui
                                                            .button(
                                                                egui::RichText::new(format!(
                                                                    "{} Remove",
                                                                    ph::TRASH
                                                                ))
                                                                .color(egui::Color32::from_rgb(220, 80, 80)),
                                                            )
                                                            .clicked()
                                                        {
                                                            remove_id = Some(m.id.clone());
                                                        }
                                                        if ui.button("Cancel").clicked() {
                                                            cancel_confirm = true;
                                                        }
                                                    });
                                                } else if ui
                                                    // Red TEXT signals the destructive
                                                    // action; the fill stays default.
                                                    .button(
                                                        egui::RichText::new(format!(
                                                            "{} Remove module",
                                                            ph::TRASH
                                                        ))
                                                        .color(egui::Color32::from_rgb(220, 80, 80)),
                                                    )
                                                    .clicked()
                                                {
                                                    arm_confirm = Some(m.id.clone());
                                                }
                                            });
                                    }
                                });
                                // Write the edited staged styles back onto the MCU.
                                mcu.pending_module_styles = local_pending;
                                // A function picked from a Custom module's pin
                                // button — same path as clicking the pin on the
                                // chip, so partners/labels stay consistent.
                                if let Some((num, func)) = pin_fn_choice {
                                    mcu.apply_pin_function(num, func);
                                    modules_changed = true;
                                }
                                // Pin labels a Custom module's rows edited (the
                                // mirror of the field inside its canvas box) —
                                // applied now that `mcu.modules` is free again.
                                for (num, label) in &pin_labels {
                                    if pin_labels_before.get(num) != Some(label) {
                                        if let Some(p) = mcu.find_pin_mut(*num) {
                                            p.custom_label = label.clone();
                                        }
                                    }
                                }
                                // Apply the inline remove-confirm signals.
                                if let Some(id) = arm_confirm {
                                    mcu.module_remove_confirm = Some(id);
                                }
                                if cancel_confirm {
                                    mcu.module_remove_confirm = None;
                                }
                                if let Some(id) = remove_id {
                                    // Snapshot for Ctrl+Z, then remove + free pins.
                                    let title = mcu
                                        .modules
                                        .iter()
                                        .find(|m| m.id == id)
                                        .map(mod_gui::module_title)
                                        .unwrap_or_else(|| "module".to_owned());
                                    mcu.push_module_undo(format!("Remove {title}"));
                                    mcu.remove_module(&id);
                                    mcu.module_remove_confirm = None;
                                    modules_changed = true;
                                }
                            }
                        });

                    if modules_changed {
                        if let Some(mcu) = &self.mcu {
                            let all_pins = mcu.all_pin_functions();
                            self.project_tree.sync_pin_files(&all_pins);
                        }
                    }
                    // Rotating re-orients the whole chip, so drop any manual
                    // zoom/pan and let the canvas auto-fit the new layout.
                    if rotate_toggled {
                        self.mcu_view_adjusted = false;
                    }

                    // Diagram fills the remaining (top) area.
                    // Computed before borrowing `self.mcu` mutably below.
                    let chip_label = self.selected_label();
                    let pin_changed = match &mut self.mcu {
                        Some(mcu) => {
                            // The chip + virtual modules are drawn at their
                            // natural fixed size inside an `egui::Scene` (egui's
                            // pan/zoom container). DEFAULT = auto-fit: the scene
                            // rect is refilled from last frame's content bounds,
                            // so the canvas rescales to fit the panel (window /
                            // panel resizes, added modules). The user can then
                            // take over the view exactly like the Structure tab —
                            // mouse wheel = zoom to the cursor, Ctrl+± = zoom,
                            // drag / middle-drag = pan, up to 4×; that latches
                            // `mcu_view_adjusted` and the view PERSISTS. Ctrl+0
                            // re-fits.
                            // `avail`/`outer` = the rect the Scene will fill —
                            // `outer` maps the cursor into scene space (for the
                            // wheel zoom), `avail` keeps AUTO-fit capped at 100%.
                            let avail = ui.available_size_before_wrap();
                            let outer = ui.available_rect_before_wrap();
                            let mut scene_rect = self.mcu_scene_bounds;
                            let mut content_bounds = egui::Rect::NOTHING;

                            // Structure-style plain mouse-wheel zoom, anchored at
                            // the pointer. egui's Scene PANS on plain scroll, so
                            // we intercept the wheel and turn it into a cursor
                            // zoom (Ctrl+scroll is left to the Scene, which also
                            // zooms to the pointer). We replicate the Scene's own
                            // fit transform (scene→screen) to map the cursor into
                            // scene space, zoom about it, then consume the scroll
                            // so the Scene does not also pan.
                            let fit = |scene: egui::Rect| -> egui::emath::TSTransform {
                                let scale =
                                    (outer.size() / scene.size()).min_elem().clamp(0.05, 4.0);
                                egui::emath::TSTransform::from_translation(
                                    outer.center().to_vec2() - scale * scene.center().to_vec2(),
                                ) * egui::emath::TSTransform::from_scaling(scale)
                            };
                            let ptr = ui.input(|i| i.pointer.hover_pos());
                            let (scroll_y, ctrl) =
                                ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.command));
                            // Over the pin-function list inside the chip the wheel
                            // means SCROLL THE LIST, not zoom. That list is painted
                            // inside the Scene and can't take the wheel itself (we
                            // intercept it here, before the Scene), so this is where
                            // it is fed — and consumed, so neither the zoom below
                            // nor the Scene's own pan sees it. The rect is last
                            // frame's, in screen coords, which is exact unless the
                            // view moved in between.
                            let on_fn_list = ptr.is_some_and(|p| self.mcu_fn_list_rect.contains(p));
                            if on_fn_list && scroll_y != 0.0 && !ctrl {
                                mcu.fn_scroll_offset -= scroll_y;
                                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                            }
                            if let Some(ptr) = ptr
                                && scroll_y != 0.0
                                && !ctrl
                                && !on_fn_list
                                && outer.contains(ptr)
                                && scene_rect.is_finite()
                                && scene_rect.size() != egui::Vec2::ZERO
                            {
                                let to_global = fit(scene_rect);
                                // Clamp so the effective scale stays in range — no
                                // scene-rect drift past the zoom limits.
                                let cur = to_global.scaling;
                                let z = ((scroll_y * 0.002).exp() * cur).clamp(0.05, 4.0) / cur;
                                let p = to_global.inverse() * ptr;
                                let new = to_global
                                    * egui::emath::TSTransform::from_translation(p.to_vec2())
                                    * egui::emath::TSTransform::from_scaling(z)
                                    * egui::emath::TSTransform::from_translation(-p.to_vec2());
                                let new_rect = new.inverse() * outer;
                                if new_rect.is_finite() && new_rect.size() != egui::Vec2::ZERO {
                                    scene_rect = new_rect;
                                    self.mcu_scene_bounds = new_rect;
                                    self.mcu_view_adjusted = true;
                                }
                                // Consume so the Scene doesn't pan with it too.
                                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                            }
                            let scene = egui::Scene::new()
                                .zoom_range(0.05..=4.0)
                                .drag_pan_buttons(
                                    egui::DragPanButtons::PRIMARY | egui::DragPanButtons::MIDDLE,
                                )
                                .show(ui, &mut scene_rect, |ui| {
                                    let r = mcu.draw(ui);
                                    content_bounds = ui.min_rect();
                                    r
                                });
                            let (inner, fn_rect) = scene.inner;
                            // Screen rect of the function list — next frame's wheel
                            // uses it to tell "scroll the list" from "zoom".
                            self.mcu_fn_list_rect = fn_rect;

                            // Scene marks its response changed on the pan/zoom it
                            // still owns (drag-pan + Ctrl+scroll zoom) and writes
                            // the new view back into `scene_rect` — latch it.
                            if scene.response.changed() {
                                self.mcu_view_adjusted = true;
                                self.mcu_scene_bounds = scene_rect;
                            }

                            // Ctrl+± zoom (around the view centre) + Ctrl+0
                            // reset — consumed ONLY while the pointer is over the
                            // canvas, so the editor keeps its own Ctrl+±. egui's
                            // global keyboard-zoom is disabled, so the Scene
                            // never sees these keys itself.
                            if scene.response.contains_pointer() {
                                let (reset, zoom_f) = ui.input_mut(|i| {
                                    let cmd = egui::Modifiers::COMMAND;
                                    if i.consume_key(cmd, egui::Key::Num0) {
                                        (true, None)
                                    } else if i.consume_key(cmd, egui::Key::Plus)
                                        || i.consume_key(cmd, egui::Key::Equals)
                                    {
                                        (false, Some(1.0 / 1.2)) // smaller rect = zoom IN
                                    } else if i.consume_key(cmd, egui::Key::Minus) {
                                        (false, Some(1.2)) // larger rect = zoom OUT
                                    } else {
                                        (false, None)
                                    }
                                });
                                if reset {
                                    self.mcu_view_adjusted = false;
                                } else if let Some(f) = zoom_f {
                                    // `scene_rect` post-show is the actual current
                                    // view (Scene wrote it, or it equals what we
                                    // fed) — zoom relative to it, no jump.
                                    let base = scene_rect;
                                    if base.is_finite()
                                        && base.size() != egui::Vec2::ZERO
                                        && avail.x > 0.0
                                        && avail.y > 0.0
                                    {
                                        // `scene_rect` maps onto `avail`, so bound
                                        // the implied scale to ~0.2×..5× (the Scene
                                        // also hard-clamps the displayed zoom to
                                        // its own range) so repeated presses can't
                                        // drift the view out of reach.
                                        let min = avail * 0.2;
                                        let max = avail * 5.0;
                                        let s = base.size() * f;
                                        let s = egui::vec2(
                                            s.x.clamp(min.x, max.x),
                                            s.y.clamp(min.y, max.y),
                                        );
                                        self.mcu_scene_bounds =
                                            egui::Rect::from_center_size(base.center(), s);
                                        self.mcu_view_adjusted = true;
                                    }
                                }
                            }

                            // Keep auto-fitting until the user takes over. Pad
                            // the fed rect to at least the panel size so a panel
                            // larger than the chip fits at 100% (centered, not
                            // blown up); a smaller panel still shrinks to fit.
                            if !self.mcu_view_adjusted {
                                self.mcu_scene_bounds = if content_bounds.is_finite() {
                                    egui::Rect::from_center_size(
                                        content_bounds.center(),
                                        egui::vec2(
                                            content_bounds.width().max(avail.x),
                                            content_bounds.height().max(avail.y),
                                        ),
                                    )
                                } else {
                                    content_bounds
                                };
                            }

                            inner
                        }
                        None => {
                            ui.centered_and_justified(|ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{}  {}  —  support coming soon",
                                        ph::GEAR, chip_label
                                    ))
                                    .size(18.0)
                                    .color(egui::Color32::GRAY),
                                );
                            });
                            None
                        }
                    };

                    // Any pin change (configure OR deselect) triggers a full
                    // sync: files for unconfigured pins are removed, files for
                    // configured pins are created/updated, and mod.rs is rebuilt.
                    if pin_changed.is_some() {
                        if let Some(mcu) = &self.mcu {
                            let all_pins = mcu.all_pin_functions();
                            self.project_tree.sync_pin_files(&all_pins);
                        }
                    }
                }
                McuTab::Peripherals => {
                    // Assigning a function here mutates the MCU just like the
                    // Pins tab, so re-sync the generated pins/ files on change.
                    let changed = show_peripherals_tab(ui, &mut self.mcu);
                    if changed.is_some() {
                        if let Some(mcu) = &self.mcu {
                            let all_pins = mcu.all_pin_functions();
                            self.project_tree.sync_pin_files(&all_pins);
                        }
                    }
                }
                McuTab::Clock => match &mut self.mcu {
                    Some(mcu) => {
                        // The Clock tab owns its layout (fixed 3-zone footer +
                        // scrollable diagram), so no outer ScrollArea here.
                        // Mutating mcu.clock is enough — `init_frame`
                        // regenerates main.rs from MCU state each frame.
                        // `clock_overrides` is a disjoint field, so it can be
                        // borrowed alongside `self.mcu`; edit mode writes the
                        // dragged positions straight into it.
                        let out =
                            mcu.draw_clock_tab(ui, &mut self.clock_overrides, &mut self.clock_note);
                        if out.save_to_definition {
                            self.save_clock_to_definition();
                        }
                    }
                    None => {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  Clock configuration — coming soon",
                                    ph::CLOCK
                                ))
                                .size(16.0)
                                .color(egui::Color32::GRAY),
                            );
                        });
                    }
                },
                McuTab::System => match &mut self.mcu {
                    Some(mcu) => Self::show_system_tab(ui, mcu),
                    None => {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  System configuration — select an MCU first",
                                    ph::GEAR
                                ))
                                .size(16.0)
                                .color(egui::Color32::GRAY),
                            );
                        });
                    }
                },
                // Module-relationship diagram — chip-agnostic (works with no
                // MCU selected), so it doesn't gate on `self.mcu`.
                McuTab::Structure => self.show_structure_tab(ui),
                McuTab::Definition => self.show_definition_tab(ui),
                McuTab::Reference => self.show_reference_tab(ui),
            }
        });
    }

    /// The "System" tab — project-level settings that aren't tied to a single
    /// pin. Currently the **Runtime** selector (Blocking bare-metal vs. embassy
    /// Async), which re-targets code generation and the embassy deps. Takes the
    /// MCU by `&mut` (not `&mut self`) so the caller can hand it the already
    /// borrowed `self.mcu` without a second borrow of `self`.
    fn show_system_tab(ui: &mut egui::Ui, mcu: &mut crate::panels::mcu_module::mcu::Mcu) {
        use crate::panels::mcu_module::codegen::family;
        use crate::panels::mcu_module::mcu::Runtime;

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(10.0);
            ui.heading(format!("{}  Runtime", ph::GEAR));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "How the firmware executes. Changing this regenerates the \
                     entry point in main.rs and the runtime dependencies.",
                )
                .color(egui::Color32::GRAY),
            );
            ui.add_space(10.0);

            let async_ok = family::async_supported(&mcu.family);
            let native_ok = family::native_supported(&mcu.family);

            // The cards edit the STAGED choice (`pending_*`); nothing regenerates
            // until "Apply" commits it (see the Apply bar below).
            Self::runtime_apply_bar(ui, mcu);

            // ── Blocking ─────────────────────────────────────────────────────
            let blocking_sel = mcu.pending_runtime == Runtime::Blocking;
            if runtime_card(
                ui,
                blocking_sel,
                true,
                "Blocking (bare-metal, portable)",
                "#[entry] fn main() -> !  ·  portable blocking drivers \
                 (embedded-io / embedded-hal 1.0); per-module Native opt-in",
            )
            .clicked()
            {
                mcu.pending_runtime = Runtime::Blocking;
            }
            runtime_details(
                ui,
                "rt_details_blocking",
                &[
                    ("On Apply:", "Regenerates main.rs with the #[entry] entry, rewrites every \
                                   src/pins/configs/*.rs to the portable init, (re)writes src/pins/io.rs, \
                                   and reconciles Cargo.toml — then triggers a build."),
                    ("Entry:", "#[cortex_m_rt::entry] fn main() -> ! — classic bare-metal, runs forever."),
                    ("Drivers:", "Each USART/SPI/I2C initialises in src/pins/configs/*.rs and is exposed \
                                  through STANDARD portable traits — embedded-io (serial) + embedded-hal 1.0 \
                                  (SpiBus / I2c / OutputPin). App code generic over those traits ports across \
                                  chips/HALs unchanged."),
                    ("Virtual modules:", "Each USART/SPI/I2C module keeps its Init API: Portable | Native (HAL \
                                          type) selector on the Pins tab. Portable = the trait wrapper (its .0 \
                                          still hands back the raw HAL object); Native = the concrete HAL type \
                                          for that one module only. SPI/I2C are always blocking here."),
                    ("GPIO / io.rs:", "GPIO API = Portable (System tab) -> src/pins/io.rs wraps the pins as \
                                       DigitalOut / DigitalIn + a Delay over embedded-hal 1.0. GPIO API = Native \
                                       -> raw HAL pins and io.rs is not emitted."),
                    ("Cargo.toml:", "Adds embedded-io (serial) + embedded-hal 1.0 (SPI/I2C/GPIO) + the \
                                     embedded-hal-0-2 bridge alias; nb is added only while a Native-mode module \
                                     needs it. The embassy-* async crates are removed."),
                    ("Applies to:", "Every chip — this is the default runtime."),
                ],
                "let mut _serial1 = pins::configs::usart1::init(...);\n\
                 fn app<S: embedded_io::Read + embedded_io::Write>(s: &mut S) { /* portable */ }",
            );
            ui.add_space(6.0);

            // ── Native (concrete HAL) ────────────────────────────────────────
            let native_sel = mcu.pending_runtime == Runtime::Native;
            let native_resp = runtime_card(
                ui,
                native_sel,
                native_ok,
                "Native (bare-metal, HAL types)",
                "#[entry] fn main() -> !  ·  concrete HAL types everywhere \
                 (Serial / Spi / BlockingI2c) — no portable bridges",
            );
            if native_resp.clicked() && native_ok {
                mcu.pending_runtime = Runtime::Native;
            }
            runtime_details(
                ui,
                "rt_details_native",
                &[
                    ("On Apply:", "Regenerates main.rs, rewrites every src/pins/configs/*.rs to the concrete \
                                   init, drops src/pins/io.rs (raw GPIO), and reconciles Cargo.toml — then builds."),
                    ("Entry:", "#[entry] fn main() -> ! — the same bare-metal entry as Blocking."),
                    ("Drivers:", "init returns the CONCRETE stm32f1xx-hal types: Serial split into (Tx, Rx), \
                                  Spi<…>, BlockingI2c<…>. No portable bridges, no extra trait crates, full HAL \
                                  features."),
                    ("Virtual modules:", "Init API is FORCED to Native (HAL type) for every USART/SPI/I2C and \
                                          shown locked on the Pins tab — project-wide, nothing to pick per module."),
                    ("GPIO / io.rs:", "GPIO is raw concrete HAL pins — src/pins/io.rs (the DigitalOut / DigitalIn \
                                       / Delay wrappers) is NOT generated. The System-tab GPIO selector is subsumed."),
                    ("Cargo.toml:", "Adds nb (the concrete Tx / Rx are nb-based) and keeps the concrete \
                                     stm32f1xx-hal; the embassy-* async crates are removed."),
                    ("Applies to:", "STM32F1 only (the family with concrete-HAL templates). Greyed on other \
                                     families, whose blocking HAL types are already concrete."),
                ],
                "let (mut _tx1, mut _rx1) = pins::configs::usart1::init(...);\n\
                 // use _tx1 with writeln!(), _rx1 with .read()",
            );
            ui.add_space(6.0);

            // ── Async (embassy) ──────────────────────────────────────────────
            let async_sel = mcu.pending_runtime == Runtime::Async;
            let async_resp = runtime_card(
                ui,
                async_sel,
                async_ok,
                "Async (embassy)",
                "#[embassy_executor::main] async fn main(Spawner)  ·  \
                 .await-able drivers on embassy-stm32",
            );
            if async_resp.clicked() && async_ok {
                mcu.pending_runtime = Runtime::Async;
            }
            runtime_details(
                ui,
                "rt_details_async",
                &[
                    ("On Apply:", "Regenerates main.rs with the embassy entry + Spawner, rewrites every \
                                   src/pins/configs/*.rs to the async init, toggles the embassy Cargo.toml deps — \
                                   then builds."),
                    ("Entry:", "#[embassy_executor::main] async fn main(Spawner) — the embassy executor drives \
                                the task; use .await inside the loop."),
                    ("Drivers:", "embedded-io-async (USART via BufferedUart, StaticCell ring buffers) + \
                                  embedded-hal-async (SPI/I2C), initialised in src/pins/configs/*.rs."),
                    ("Virtual modules:", "USART is always BufferedUart. Each SPI/I2C module gets a Blocking | \
                                          Async-DMA selector on the Pins tab (the Portable/Native Init API is not \
                                          used under Async)."),
                    ("Async-DMA:", "embassy async SPI/I2C need DMA channels the IDE can't choose -> main.rs gets \
                                    a TODO line to fill (it won't compile until you set channels valid for your chip)."),
                    ("Cargo.toml:", "Adds embassy-executor + embassy-time and toggles the time-driver-any feature \
                                     on embassy-stm32. Async USART adds embedded-io-async + static_cell; SPI/I2C \
                                     add embedded-hal (blocking) / embedded-hal-async (async-DMA). Leaving Async \
                                     removes these again."),
                    ("Applies to:", "STM32F4/G0/G4/L4/H7/WBA/… (embassy families). NOT STM32F1 (on stm32f1xx-hal) \
                                     or ESP yet."),
                ],
                "let mut _serial1 = pins::configs::usart1::init(p.USART1, p.PA10, p.PA9);\n\
                 _serial1.write_all(b\"hi\").await.ok();",
            );

            ui.add_space(10.0);
            if !async_ok && !native_ok {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  {} only supports Blocking here (Native = STM32F1 concrete HAL; \
                         Async = embassy on STM32F4/G0/G4/L4/H7/…).",
                        ph::WARNING,
                        mcu.family,
                    ))
                    .color(egui::Color32::from_rgb(210, 170, 90)),
                );
            } else if native_sel {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  All USART/SPI/I2C peripherals now expose the concrete \
                         stm32f1xx-hal types (the per-module Portable/Native selector is \
                         subsumed). USART `init` returns the split (Tx, Rx).",
                        ph::INFO,
                    ))
                    .color(egui::Color32::from_rgb(120, 170, 220)),
                );
            } else if async_sel {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  main.rs now runs on the embassy executor — add \
                         `embassy_time::Timer::after_millis(..).await` in the loop.",
                        ph::INFO,
                    ))
                    .color(egui::Color32::from_rgb(120, 170, 220)),
                );
            }

            // ── GPIO In/Out bridge (io.rs) ───────────────────────────────────
            use crate::panels::mcu_module::modules::ApiStyle;
            ui.add_space(16.0);
            ui.separator();
            ui.add_space(10.0);
            ui.heading(format!("{}  GPIO In/Out", ph::GEAR));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "How GPIO In/Out pins bind in main.rs (STM32F1 blocking path).",
                )
                .color(egui::Color32::GRAY),
            );
            ui.add_space(8.0);

            // Only the STM32F1 backend has the io.rs bridge; on the (staged)
            // Native runtime GPIO is forced raw regardless. Edits the STAGED
            // `pending_gpio_api`.
            let gpio_ok = native_ok && mcu.pending_runtime != Runtime::Native;
            let portable_sel = mcu.pending_gpio_api == ApiStyle::Portable;
            if runtime_card(
                ui,
                portable_sel && native_ok,
                gpio_ok,
                "Portable (embedded-hal bridge)",
                "let pa0_out = &mut pins::configs::io::DigitalOut(...)  ·  \
                 generates io.rs (embedded-hal 1.0 OutputPin/InputPin/DelayNs)",
            )
            .clicked()
                && gpio_ok
            {
                mcu.pending_gpio_api = ApiStyle::Portable;
            }
            ui.add_space(6.0);
            let native_gpio_sel = mcu.pending_gpio_api == ApiStyle::Native;
            if runtime_card(
                ui,
                native_gpio_sel && native_ok,
                gpio_ok,
                "Native (raw HAL pin)",
                "let pa0_out = &mut gpioa.pa0.into_push_pull_output(...)  ·  \
                 no io.rs, no embedded-hal dep for GPIO",
            )
            .clicked()
                && gpio_ok
            {
                mcu.pending_gpio_api = ApiStyle::Native;
            }

            ui.add_space(8.0);
            if !native_ok {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  The io.rs bridge is an STM32F1 feature. {} binds GPIO via \
                         its own HAL (embassy `Output`/`Input` on non-F1).",
                        ph::WARNING,
                        mcu.family,
                    ))
                    .color(egui::Color32::from_rgb(210, 170, 90)),
                );
            } else if mcu.pending_runtime == Runtime::Native {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  The Native runtime already binds all GPIO raw — this choice \
                         applies on the Blocking runtime.",
                        ph::INFO,
                    ))
                    .color(egui::Color32::from_rgb(120, 170, 220)),
                );
            }

            // ── Build on Save ────────────────────────────────────────────────
            // A workflow preference (NOT a codegen choice) — applied immediately,
            // no staged Apply.
            use crate::panels::mcu_module::mcu::AutoBuild;
            ui.add_space(16.0);
            ui.separator();
            ui.add_space(10.0);
            ui.heading(format!("{}  Build on Save", ph::GEAR));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "When a library changes in Cargo.toml (yours or codegen-added), Save can \
                     run a build automatically so the new deps resolve + compile.",
                )
                .color(egui::Color32::GRAY),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("On dependency change:");
                egui::ComboBox::from_id_salt("auto_build_on_save")
                    .selected_text(match mcu.auto_build {
                        AutoBuild::Off => "Off",
                        AutoBuild::Check => "cargo check",
                        AutoBuild::Release => "cargo build --release",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut mcu.auto_build, AutoBuild::Off, "Off")
                            .on_hover_text("Never auto-build — run Build / Check manually.");
                        ui.selectable_value(&mut mcu.auto_build, AutoBuild::Check, "cargo check")
                            .on_hover_text("Fast: resolves new deps + catches errors (default).");
                        ui.selectable_value(
                            &mut mcu.auto_build,
                            AutoBuild::Release,
                            "cargo build --release",
                        )
                        .on_hover_text("Full optimized build — slower, but exactly what gets flashed.");
                    });
            });

            // ── Strict lints (Clippy) ────────────────────────────────────────
            ui.add_space(16.0);
            ui.separator();
            ui.add_space(10.0);
            ui.heading(format!("{}  Strict lints (Clippy)", ph::SHIELD_CHECK));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "Add a strict `[lints.clippy]` deny profile (pedantic + nursery + no \
                     unwrap / panic / indexing / arithmetic / as-cast) to Cargo.toml. Only \
                     affects the Clippy tab — check / build / flash are unaffected.",
                )
                .color(egui::Color32::GRAY),
            );
            ui.add_space(8.0);
            if ui
                .checkbox(&mut mcu.strict_lints, "Deny panics & risky ops on my code")
                .on_hover_text(
                    "The generated code (main's init + src/pins/configs/*.rs) is auto-exempted \
                     with #[allow], so only YOUR modules are held to the strict profile. \
                     Toggling regenerates Cargo.toml + the exemptions.",
                )
                .changed()
            {
                // Codegen reads `strict_lints` (hashed) → regeneration next frame.
            }
        });
    }

    /// The staged-changes banner shown at the top of the System tab: nothing
    /// regenerates until the user **Applies** the pending Runtime / GPIO /
    /// per-module choices. Nothing is drawn when there's no staged change.
    fn runtime_apply_bar(ui: &mut egui::Ui, mcu: &mut crate::panels::mcu_module::mcu::Mcu) {
        if !mcu.style_dirty() {
            return;
        }
        let diff = mcu.style_diff_summary();
        egui::Frame::new()
            .inner_margin(egui::Margin::same(10))
            .corner_radius(egui::CornerRadius::same(6))
            .fill(egui::Color32::from_rgb(48, 44, 30))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(150, 130, 70)))
            .show(ui, |ui| {
                if !mcu.pending_apply_confirm {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  {} staged change{} — not applied yet.",
                                ph::WARNING,
                                diff.len(),
                                if diff.len() == 1 { "" } else { "s" },
                            ))
                            .color(egui::Color32::from_rgb(230, 200, 120)),
                        );
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(egui::RichText::new(format!("{}  Apply — regenerate", ph::CHECK)).strong())
                            .on_hover_text("Commit these choices and regenerate main.rs, the config files and Cargo.toml deps")
                            .clicked()
                        {
                            mcu.pending_apply_confirm = true;
                        }
                        if ui.button("Discard").on_hover_text("Revert the staged choices to what's applied").clicked() {
                            mcu.sync_pending_style();
                        }
                    });
                } else {
                    ui.label(
                        egui::RichText::new("Apply these changes? Everything below will be regenerated:")
                            .strong()
                            .color(egui::Color32::from_gray(225)),
                    );
                    ui.add_space(4.0);
                    // The FULL list: the staged choices + their concrete effects
                    // (entry point, config files added/removed/regenerated, deps).
                    let changes = mcu.apply_change_list();
                    egui::ScrollArea::vertical()
                        .max_height(190.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for line in &changes {
                                let (color, mono) = if line.starts_with('•') {
                                    (egui::Color32::from_gray(220), false)
                                } else if line.starts_with('+') {
                                    (egui::Color32::from_rgb(140, 200, 140), true)
                                } else if line.starts_with('−') {
                                    (egui::Color32::from_rgb(210, 150, 150), true)
                                } else {
                                    (egui::Color32::from_gray(185), true)
                                };
                                let mut txt = egui::RichText::new(format!("   {line}")).size(11.5).color(color);
                                if mono {
                                    txt = txt.monospace();
                                }
                                ui.label(txt);
                            }
                        });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  Edits inside a config file may be lost if its template changes (e.g. blocking -> async).",
                            ph::WARNING,
                        ))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(210, 170, 90)),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(egui::RichText::new(format!("{}  Confirm & apply", ph::CHECK)).strong())
                            .clicked()
                        {
                            mcu.apply_pending_style();
                        }
                        if ui.button("Cancel").clicked() {
                            mcu.pending_apply_confirm = false;
                        }
                    });
                }
            });
        ui.add_space(12.0);
    }

    /// The F12 "Go to definition" snippet (external / crate / std files) —
    /// moved here from the bottom diagnostics panel on 2026-07-10. The whole
    /// file is shown (scrollable above and below the target); rows are
    /// virtualized and the target line is scrolled near the top once on open,
    /// drawn coloured so it stands out from the surrounding code.
    /// The second editor: another project file, EDITABLE, beside the main one.
    ///
    /// Level 1 of the two-editor design — plain editing (type, select, undo)
    /// with syntax highlighting, but deliberately NO language features here:
    /// completion, rename, code actions, inline diagnostics and multi-cursor
    /// all read singleton state on `AppIde` that belongs to the main editor, so
    /// wiring them to a second view means splitting ~300 references into a
    /// per-editor struct. That refactor is Level 2 and is not needed for the
    /// case this serves: change something small in another file while looking
    /// at this one.
    ///
    /// Undo and the caret come free and correctly separated: egui keys
    /// `TextEditState` by widget id, and the id here is derived from the file
    /// path (same rule as the main editor).
    fn show_reference_tab(&mut self, ui: &mut egui::Ui) {
        let Some(path) = self.reference_file.clone() else {
            ui.label(egui::RichText::new("No file open.").color(egui::Color32::GRAY));
            return;
        };
        let Some((id, mut code)) = self.reference_content(&path) else {
            // Deleted or renamed while it was open here.
            ui.label(
                egui::RichText::new(format!("{path} is no longer in the project."))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(220, 170, 90)),
            );
            return;
        };

        // The same file in BOTH editors: the widget ids differ (`reference_…`
        // vs `code_editor:…`), so egui keeps two independent carets and undo
        // stacks over ONE buffer. Typing in either then makes the other's view
        // and undo history stale, and both write back to the same slot in the
        // same frame. Editing is refused rather than left to that race —
        // "Open in editor" is the way to edit it.
        let clash = self.selected_file == id;

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&path)
                    .size(11.0)
                    .monospace()
                    .color(egui::Color32::from_rgb(150, 200, 150)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("Open in editor").size(10.5))
                            .frame(false),
                    )
                    .on_hover_text("Switch the MAIN editor to this file")
                    .clicked()
                {
                    self.selected_file = id;
                }
                if clash {
                    ui.label(
                        egui::RichText::new("read-only — already open in the main editor")
                            .size(10.0)
                            .color(egui::Color32::from_rgb(220, 180, 90)),
                    );
                }
            });
        });
        ui.separator();

        let font_size = self.editor_font_size;
        let rows = ((ui.available_height() / (font_size * 1.3)).floor() as usize).max(6);
        // Distinct id space from the main editor, and per FILE so the caret and
        // undo stack follow the file rather than the slot.
        let editor_id = format!("reference_editor:{path}");
        let before = code.clone();

        if clash {
            // Read-only: render the same highlighter through a disabled editor
            // so it still looks like code.
            ui.add_enabled_ui(false, |ui| {
                crate::editor::gui::code_editor::show_rust_editor_plain(
                    ui, &mut code, font_size, rows, &editor_id,
                );
            });
            // Disabled widgets can't hold focus; make sure a stale `true` from
            // an earlier frame doesn't keep stealing the main editor's keys.
            self.reference_was_focused = false;
            return;
        }

        let clip = ui.clip_rect();
        let out = crate::editor::gui::code_editor::show_rust_editor_plain(
            ui, &mut code, font_size, rows, &editor_id,
        );
        // Focus is read AFTER rendering and used by the main editor's
        // keyboard-scope gate NEXT frame — that panel runs earlier, so
        // last-frame focus is the only thing available to it.
        self.reference_was_focused = out.response.has_focus();

        // Write-back BEFORE completion runs: `handle_editor_completion` may
        // apply an accepted item, and it persists through `owner_file` itself.
        // Mirrors the main editor's rule — persist to the file the view was
        // BUILT for, never to whatever is selected now.
        if code != before {
            self.apply_reference_edit(id, code.clone());
        }

        // Completion for this editor. Everything else the handler can do
        // (rename, F12, diagnostics, type hints) is skipped by the slot — see
        // its `slot` parameter doc.
        let ctrl_space = std::mem::take(&mut self.reference_ctrl_space);
        // Claim a deferred accept meant for THIS editor. Both routes land here:
        // a mouse click in the popup (which renders from this call) and a
        // keyboard accept, which `mod.rs` consumes before either editor draws
        // and parks here rather than applying to the main editor's buffer.
        let accepted = if self.completion_owner == crate::app::EditorSlot::Reference {
            self.completion_pending_insert.take()
        } else {
            None
        };
        if accepted.is_some() {
            self.completion_open = false;
        }
        self.handle_editor_completion(
            ui,
            &out,
            clip,
            code,
            accepted,
            ctrl_space,
            false,
            false,
            false,
            false,
            None,
            None,
            Vec::new(),
            crate::app::EditorSlot::Reference,
            id,
        );
    }

    /// Persist an edit made in the second editor.
    ///
    /// Only real project buffers are written; `main.rs` is refused because the
    /// MCU Configurator regenerates it from the pin model, so an edit made here
    /// outside the editor's own guarded path could be silently overwritten.
    fn apply_reference_edit(&mut self, id: ProjectFileId, code: String) {
        match id {
            ProjectFileId::UserFile(i) => {
                if let Some(e) = self.project_tree.user_src_files.get_mut(i) {
                    e.1 = code;
                }
            }
            ProjectFileId::CargoToml => self.cargo_toml = code,
            ProjectFileId::CargoConfig => self.cargo_config = code,
            ProjectFileId::MemoryX => self.memory_x = code,
            ProjectFileId::BuildRs => self.build_rs = code,
            ProjectFileId::GitIgnore => self.gitignore = code,
            // main.rs is owned by codegen — edit it in the main editor.
            ProjectFileId::MainRs => {}
        }
        self.cached_project_files = None;
    }

    /// Resolve the reference file's path to `(id, content)`, or `None` when it
    /// is no longer in the project (deleted or renamed since it was opened).
    fn reference_content(&self, path: &str) -> Option<(ProjectFileId, String)> {
        match path {
            "src/main.rs" => Some((ProjectFileId::MainRs, self.generated_code.clone())),
            "Cargo.toml" => Some((ProjectFileId::CargoToml, self.cargo_toml.clone())),
            ".cargo/config.toml" => Some((ProjectFileId::CargoConfig, self.cargo_config.clone())),
            "memory.x" => Some((ProjectFileId::MemoryX, self.memory_x.clone())),
            "build.rs" => Some((ProjectFileId::BuildRs, self.build_rs.clone())),
            ".gitignore" => Some((ProjectFileId::GitIgnore, self.gitignore.clone())),
            _ => self
                .project_tree
                .user_src_files
                .iter()
                .position(|(p, _)| p == path)
                .map(|i| {
                    (
                        ProjectFileId::UserFile(i),
                        self.project_tree.user_src_files[i].1.clone(),
                    )
                }),
        }
    }

    fn show_definition_tab(&mut self, ui: &mut egui::Ui) {
        let Some(def) = &self.definition_view else {
            ui.label(egui::RichText::new("No definition.").color(egui::Color32::GRAY));
            return;
        };
        ui.label(
            egui::RichText::new(&def.header)
                .size(11.0)
                .monospace()
                .color(egui::Color32::from_rgb(150, 190, 240)),
        );
        ui.separator();
        let lines: Vec<&str> = def.code.lines().collect();
        let highlight = def.highlight;
        // Height of one monospace-12 line (matches the rows below).
        let row_h = ui
            .painter()
            .layout_no_wrap(
                "X".to_owned(),
                egui::FontId::monospace(12.0),
                egui::Color32::WHITE,
            )
            .size()
            .y;
        // Match the spacing show_rows will use, so its offset math lines up
        // with the rendered rows.
        ui.spacing_mut().item_spacing.y = 1.0;
        let pitch = row_h + ui.spacing().item_spacing.y;
        let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
        if self.def_scroll_pending {
            // Target near the top (2 lines of context above), then free.
            let off = highlight.saturating_sub(2) as f32 * pitch;
            area = area.vertical_scroll_offset(off);
            self.def_scroll_pending = false;
        }
        area.show_rows(ui, row_h, lines.len(), |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for i in range {
                let shown = if lines[i].is_empty() { " " } else { lines[i] };
                if i == highlight {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(shown)
                                .monospace()
                                .size(12.0)
                                .strong()
                                .color(egui::Color32::from_rgb(255, 214, 90))
                                .background_color(egui::Color32::from_rgb(64, 58, 30)),
                        )
                        .selectable(true),
                    );
                } else {
                    ui.add(
                        egui::Label::new(egui::RichText::new(shown).monospace().size(12.0))
                            .selectable(true),
                    );
                }
            }
        });
    }
}

/// A selectable "card" for the Runtime picker: a framed, clickable block with a
/// bold title over a dimmed monospace subtitle. `selected` tints it with the
/// accent; `enabled == false` greys the text and swallows clicks (the disabled
/// Async option on families without an embassy backend). Returns the click
/// [`egui::Response`] so the caller decides what a click does.
fn runtime_card(
    ui: &mut egui::Ui,
    selected: bool,
    enabled: bool,
    title: &str,
    subtitle: &str,
) -> egui::Response {
    let accent = egui::Color32::from_rgb(90, 140, 210);
    let fill = if selected {
        egui::Color32::from_rgb(40, 55, 78)
    } else {
        egui::Color32::from_rgb(38, 38, 46)
    };
    let border = if selected {
        egui::Stroke::new(1.5, accent)
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 70, 82))
    };

    let inner = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 10))
        .corner_radius(egui::CornerRadius::same(6))
        .fill(fill)
        .stroke(border)
        .show(ui, |ui| {
            ui.set_width(ui.available_width().min(560.0));
            let title_col = if enabled {
                egui::Color32::from_gray(232)
            } else {
                egui::Color32::from_gray(120)
            };
            ui.label(
                egui::RichText::new(title)
                    .strong()
                    .size(14.0)
                    .color(title_col),
            );
            ui.label(
                egui::RichText::new(subtitle)
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_gray(if enabled { 155 } else { 100 })),
            );
        });

    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let resp = ui.interact(
        inner.response.rect,
        egui::Id::new(("runtime_card", title)),
        sense,
    );
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Style one word inside a Runtime-details body: crate/library names → yellow
/// bold, source-file names → orange bold, async/runtime keywords → orange-red
/// bold, peripheral names → bold (default colour); everything else stays plain.
/// Matching strips surrounding sentence punctuation but the returned text keeps
/// the original token verbatim.
fn detail_word(word: &str) -> egui::RichText {
    const SIZE: f32 = 11.5;
    let plain = egui::Color32::from_gray(165);
    // Trim only clear sentence punctuation for matching (keep '.', '/', '-', '_'
    // so file names / crate names / `.await` survive).
    let clean = word
        .trim_start_matches('(')
        .trim_end_matches([',', ';', ':', '.', ')']);

    // 1. Source-file names → orange bold (Cargo.toml, io.rs, main.rs, *.rs …).
    if clean.ends_with(".rs") || clean.ends_with(".toml") {
        return egui::RichText::new(word)
            .size(SIZE)
            .strong()
            .color(egui::Color32::from_rgb(240, 150, 40));
    }
    // 2. Crate / library names → yellow bold.
    const LIBS: &[&str] = &[
        "embedded-hal-0-2",
        "embedded-hal",
        "embedded-io",
        "embedded-io-async",
        "embedded-hal-async",
        "nb",
        "embassy-executor",
        "embassy-time",
        "embassy-stm32",
        "static_cell",
        "stm32f1xx-hal",
        "cortex-m-rt",
        "bxcan",
        "usb-device",
        "usbd-serial",
    ];
    if LIBS.contains(&clean) {
        return egui::RichText::new(word)
            .size(SIZE)
            .strong()
            .color(egui::Color32::from_rgb(235, 215, 70));
    }
    // 3. async / runtime keywords → orange-red bold.
    const KEYWORDS: &[&str] = &[
        "async",
        "await",
        ".await",
        "Async",
        "Async-DMA",
        "Spawner",
        "Blocking",
        "Native",
        "Portable",
        "#[entry]",
        "#[embassy_executor::main]",
        "#[cortex_m_rt::entry]",
    ];
    if KEYWORDS.contains(&clean) {
        return egui::RichText::new(word)
            .size(SIZE)
            .strong()
            .color(egui::Color32::from_rgb(255, 85, 50));
    }
    // 4. Peripheral names → bold (default colour). Slash-joined groups like
    // "USART/SPI/I2C" match via `contains`; case-sensitive so `BlockingI2c`
    // (lower-case c) or English "can" don't trip it.
    const PERIPH: &[&str] = &["USART", "SPI", "I2C", "CAN", "USB", "GPIO", "Tx", "Rx"];
    if PERIPH.iter().any(|p| clean.contains(p)) {
        return egui::RichText::new(word).size(SIZE).strong().color(plain);
    }

    egui::RichText::new(word).size(SIZE).color(plain)
}

/// A collapsible "ⓘ Details" section under a Runtime card, explaining what that
/// runtime generates and how it applies. `points` are `(label, body)` rows; a
/// non-empty `example` is rendered as a monospace code block. The open/closed
/// state persists per `salt` via egui's own widget memory.
fn runtime_details(ui: &mut egui::Ui, salt: &str, points: &[(&str, &str)], example: &str) {
    egui::CollapsingHeader::new(
        egui::RichText::new(format!("{}  Details — how it works & applies", ph::INFO))
            .size(11.5)
            .color(egui::Color32::from_gray(160)),
    )
    .id_salt(salt)
    .show(ui, |ui| {
        for (label, body) in points {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    egui::RichText::new(*label)
                        .strong()
                        .size(11.5)
                        .color(egui::Color32::from_gray(215)),
                );
                // Emphasise keywords inline: crate names, file names, runtime/
                // async keywords and peripheral names each get their own colour.
                for word in body.split_whitespace() {
                    ui.label(detail_word(word));
                }
            });
            ui.add_space(4.0);
        }
        if !example.is_empty() {
            ui.add_space(2.0);
            egui::Frame::new()
                .inner_margin(egui::Margin::same(7))
                .corner_radius(egui::CornerRadius::same(4))
                .fill(egui::Color32::from_rgb(30, 30, 36))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(example)
                            .monospace()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(180, 200, 180)),
                    );
                });
        }
    });
}
