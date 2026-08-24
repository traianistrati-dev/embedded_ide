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
    /// What an MCU tab shows when there is no [`Mcu`](crate::panels::mcu_module::mcu::model::Mcu)
    /// to draw.
    ///
    /// Two different situations hide behind that one `None`, and they need
    /// opposite answers:
    ///
    /// * **No chip chosen at all** — a fresh New Project. Offer the picker: it
    ///   is the only way forward, and this is where the user is already looking.
    /// * **A chip whose definition exists but has no runtime support yet.**
    ///   Nothing to pick; say so.
    ///
    /// The old text said "coming soon" for both, which named a chip that wasn't
    /// selected and offered no way out of an empty project.
    fn show_no_mcu_notice(&mut self, ui: &mut egui::Ui, what: &str) {
        let chip = self.selected_label();
        ui.add_space((ui.available_height() * 0.3).min(160.0));
        ui.vertical_centered(|ui| {
            if !chip.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  {chip}  —  {what} not supported yet",
                        ph::GEAR
                    ))
                    .size(16.0)
                    .color(egui::Color32::GRAY),
                );
                return;
            }
            ui.label(
                egui::RichText::new(format!("{}  No MCU selected", ph::CPU))
                    .size(17.0)
                    .color(egui::Color32::from_rgb(150, 158, 172)),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "{what} needs a chip — the project's code is generated from it."
                ))
                .size(12.0)
                .color(egui::Color32::from_gray(120)),
            );
            ui.add_space(12.0);
            if ui
                .button(egui::RichText::new(format!("{}  Select a chip…", ph::CPU)).size(13.0))
                .on_hover_text("Choose the MCU this project targets")
                .clicked()
            {
                // The same dialog New Project uses — it IS the chip picker.
                // Safe to reach for here: the project is empty by definition,
                // so its "this clears everything" has nothing to clear.
                self.begin_new_project();
                // That dialog is rendered EARLIER in the frame than this panel,
                // so it first appears next frame — guarantee there is one.
                ui.ctx().request_repaint();
            }
        });
    }

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
            // Reference belongs to NEITHER group — recording it would make a
            // later "MCU" click land on a file instead of the chip.
            let reference_active = self.active_tab == McuTab::Reference;
            if self.active_tab.is_project_group() {
                self.project_group_last = self.active_tab;
            } else if !reference_active {
                self.mcu_group_last = self.active_tab;
            }
            let project_active = self.active_tab.is_project_group();
            // The chip-only chrome (Reset pins, the chip label) must not come
            // back just because Reference left the Project group.
            let mcu_active = !project_active && !reference_active;

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
                // The open reference file, as a THIRD top-level entry — right
                // of Project, level with the code editor it is read beside.
                if let Some(path) = self.reference_file.clone() {
                    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                    let text = egui::RichText::new(name).size(14.0).strong().color(
                        if reference_active {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::from_rgb(150, 200, 150)
                        },
                    );
                    if ui.selectable_label(reference_active, text).clicked() {
                        self.active_tab = McuTab::Reference;
                    }
                    if ui
                        .add(egui::Button::new(egui::RichText::new(ph::X).size(10.0)).frame(false))
                        .on_hover_text("Close the reference file")
                        .clicked()
                    {
                        self.reference_file = None;
                        if reference_active {
                            self.active_tab = self.project_group_last;
                        }
                    }
                }
                // Reset pins — a chip operation, shown with the MCU group only.
                //
                // It ASKS first, like removing a single module does, because it
                // is the widest destructive action in the app: every pin function
                // goes, and with the pins go the Virtual Modules that were wired
                // to them (`reconcile_modules` drops a peripheral module whose
                // pins are gone). The confirm names the count, so the question
                // carries the size of the loss instead of just repeating itself.
                if mcu_active {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let configured =
                            self.mcu.as_ref().map_or(0, |m| m.configured_pin_count());
                        if self.reset_pins_confirm {
                            // Right-to-left: Cancel is added first, so it lands
                            // on the OUTSIDE — the safe choice is the one nearest
                            // the edge, away from where the pointer already is.
                            if ui.button("Cancel").clicked() {
                                self.reset_pins_confirm = false;
                            }
                            if ui
                                .button(
                                    egui::RichText::new(format!(
                                        "{} Reset {configured} pin{}",
                                        ph::ARROW_COUNTER_CLOCKWISE,
                                        if configured == 1 { "" } else { "s" }
                                    ))
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(220, 100, 80)),
                                )
                                .clicked()
                            {
                                if let Some(mcu) = &mut self.mcu {
                                    mcu.reset_all_pins();
                                }
                                self.reset_pins_confirm = false;
                            }
                            ui.label(
                                egui::RichText::new("also removes the Virtual Modules")
                                    .size(10.5)
                                    .color(egui::Color32::from_rgb(220, 180, 90)),
                            );
                        } else {
                            // Nothing configured → nothing to ask about, and a
                            // live button would only invite a pointless confirm.
                            let btn = ui.add_enabled(
                                configured > 0,
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{} Reset pins",
                                        ph::ARROW_COUNTER_CLOCKWISE
                                    ))
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(220, 100, 80)),
                                ),
                            );
                            let btn = if configured > 0 {
                                btn.on_hover_text(
                                    "Clear every pin function on this chip (asks first)",
                                )
                            } else {
                                btn.on_disabled_hover_text("No pin is configured")
                            };
                            if btn.clicked() {
                                self.reset_pins_confirm = true;
                            }
                        }
                    });
                } else {
                    // Left the MCU group with the question open: disarm, so it
                    // is not still waiting when the user comes back much later.
                    self.reset_pins_confirm = false;
                }
            });

            // Chip label — MCU group only (read-only; selection happens in the
            // "New Project" popup). The Project group doesn't involve the chip.
            if mcu_active {
                ui.horizontal(|ui| {
                    ui.label("Chip:");
                    ui.label(
                        egui::RichText::new(self.selected_label())
                            .strong()
                            .color(egui::Color32::LIGHT_BLUE),
                    );
                    let facts = self.chip_facts();
                    if !facts.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("·  {}", facts.join("  ·  ")))
                                .color(egui::Color32::GRAY)
                                .size(11.0),
                        );
                    }
                });
            }

            ui.separator();

            // ── Level 2: the active group's tab row ────────────────────────
            // Skipped entirely for Reference: it is a top-level entry with no
            // sub-tabs, and falling through to the `else` below would show the
            // MCU row (Pins/Clock/…) over a file that has nothing to do with it.
            if !reference_active {
            ui.horizontal(|ui| {
                let mut tabs: Vec<McuTab> = if project_active {
                    let mut t = vec![McuTab::Structure];
                    if self.definition_view.is_some() {
                        t.push(McuTab::Definition);
                    }
                    t
                } else {
                    vec![
                        McuTab::Pins,
                        McuTab::Peripherals,
                        McuTab::Configuration,
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
            });
            }

            ui.separator();

            // Set by a tab that has no MCU to draw, and rendered AFTER the
            // match: those arms sit inside `match &mut self.mcu`, whose borrow
            // is live across them, so the notice (which takes `&mut self`)
            // cannot be drawn from inside one. Nothing else is drawn there
            // either, so it lands in the same place on screen.
            let mut no_mcu: Option<&'static str> = None;

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
                    // The panel keeps a real MINIMUM of one bar, so the border
                    // is always draggable and the diagram can always be given
                    // the space back. Making room for a freshly opened config is
                    // a one-shot RESIZE further down instead of a floor here: a
                    // floor cannot be dragged past, and this panel sits on top of
                    // the chip.
                    let mut needed_h = 0.0_f32;
                    // Height the module LIST needs to show every row.
                    let mut list_h = 0.0_f32;
                    // Set when the caret button (not the handle) expands the
                    // panel — that is the gesture that means "show me all of it".
                    let mut expand_clicked = false;
                    // A module added from the palette: its config is opened, and
                    // a collapsed panel opens with it.
                    let mut open_after_add: Option<String> = None;
                    let mut open_sig = 0_u64;
                    // The panel hugs its content (an egui bottom panel stores
                    // the CONTENT's rect, not the size it was given), so the body
                    // height is ours to set — and ours to offer a drag handle for.
                    // Two ceilings, on purpose. The DRAG may go nearly to the
                    // top — it is a deliberate act, and someone configuring six
                    // modules wants the room. Growing BY ITSELF stops at 45 %:
                    // a panel that takes the whole zone the moment you open a
                    // config has stopped being a panel.
                    let body_cap = (ui.available_height() - 90.0).max(120.0);
                    let auto_cap = (ui.available_height() * 0.45).min(body_cap);
                    let mut body_h = self.vmod_body_h.min(body_cap);
                    let mut collapsed = self.vmod_collapsed;
                    // The handle below IS this panel's top border, so egui's
                    // own separator line would be a second bar right above it —
                    // which is exactly what it looked like.
                    // Collapsed, it hugs its content on purpose. Forcing it to
                    // the OTHER panel's exact box lined the two outlines up but
                    // left dead space under these buttons, which pushed the
                    // buttons off the line — and the buttons are what you see.
                    // Both bars are bottom-anchored, so equal content is what
                    // puts equal rows on one line.
                    egui::TopBottomPanel::bottom("vmodules_panel")
                        // NOT `resizable`: the handle below does the resizing,
                        // and egui's own can only cap a content-sized panel.
                        .resizable(false)
                        .show_separator_line(false)
                        .show_inside(ui, |ui| {
                            // ── The panel's top border, as a real handle ──
                            // Drag it up to make room, down to give it back to
                            // the diagram. Drawn before anything else so it sits
                            // on the panel's top edge, where the pointer expects
                            // the boundary to be.
                            {
                                /// Smallest body worth showing, and how far past
                                /// it the handle must be pulled DOWN before the
                                /// panel collapses itself — same pair as the
                                /// panel under the editor, so both borders
                                /// behave identically.
                                const MIN_BODY_H: f32 = 60.0;
                                const COLLAPSE_SLACK: f32 = 14.0;
                                let (hid, hrect) =
                                    ui.allocate_space(egui::vec2(ui.available_width(), 6.0));
                                let h = ui.interact(hrect, hid, egui::Sense::drag());
                                if h.dragged() {
                                    let dy = h.drag_delta().y;
                                    if collapsed {
                                        // Only upward opens it: a collapsed bar
                                        // has nowhere to shrink to. The caret
                                        // button flips on its own — it renders
                                        // straight from this flag.
                                        if dy < 0.0 {
                                            collapsed = false;
                                            body_h =
                                                (MIN_BODY_H - dy).clamp(MIN_BODY_H, body_cap);
                                        }
                                    } else {
                                        let want = body_h - dy;
                                        // Pulled past the smallest useful body:
                                        // finish the gesture by collapsing
                                        // rather than jamming against the floor.
                                        // The slack keeps a drag that merely
                                        // BOTTOMS OUT from snapping it shut.
                                        if want < MIN_BODY_H - COLLAPSE_SLACK {
                                            collapsed = true;
                                        }
                                        body_h = want.clamp(MIN_BODY_H, body_cap);
                                    }
                                }
                                let live = h.hovered() || h.dragged();
                                if live {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                                }
                                ui.painter().hline(
                                    hrect.x_range(),
                                    hrect.center().y,
                                    egui::Stroke::new(
                                        if live { 2.0 } else { 1.0 },
                                        if live {
                                            egui::Color32::from_rgb(120, 160, 210)
                                        } else {
                                            egui::Color32::from_gray(70)
                                        },
                                    ),
                                );
                            }
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

                            if !collapsed {
                                ui.add_space(4.0);
                            }
                            ui.horizontal(|ui| {
                                // Disclosure for the WHOLE panel, left of its
                                // name. Collapsed it keeps only this bar, which
                                // is the difference between a panel that can be
                                // put away and one that permanently costs the
                                // diagram a third of its height.
                                // Same pair as the panel under the editor
                                // (`diag_panel`): double-up to reopen, plain
                                // caret-down to put away. One collapse control,
                                // one shape — a right-caret reads as "unfold
                                // sideways", which is not what this does.
                                let (icon, tip) = if collapsed {
                                    (
                                        ph::CARET_DOUBLE_UP,
                                        "Expand the Virtual-modules panel.",
                                    )
                                } else {
                                    (
                                        ph::CARET_DOWN,
                                        "Collapse the panel to this bar and give the height back \
                                         to the diagram. Drag the top border to resize it instead.",
                                    )
                                };
                                if ui
                                    .button(
                                        egui::RichText::new(icon)
                                            .size(11.0)
                                            .color(egui::Color32::from_rgb(160, 185, 215)),
                                    )
                                    .on_hover_text(tip)
                                    .clicked()
                                {
                                    collapsed = !collapsed;
                                    expand_clicked = !collapsed;
                                }
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
                                        ModuleKind::GenericInterfaceLpuart => "Add a virtual LPUART device and auto-wire it to a free LPUART TX/RX pin pair (a peripheral of its own, not a USART instance)",
                                        ModuleKind::GenericInterfaceSpi => "Add a virtual SPI device and auto-wire it to free SPI SCK/MOSI/MISO(/NSS) pins",
                                        ModuleKind::GenericInterfaceI2c => "Add a virtual I2C device and auto-wire it to a free I2C SCL/SDA pin pair",
                                        ModuleKind::GenericInterfaceHspi => "Add external memory on the HSPI (high-end U5): it auto-wires the single-line width, and the octal one joins by assigning IO2-IO7 and DQS0, which that call requires",
                                        ModuleKind::GenericInterfaceXspi => "Add an external flash or RAM on an XSPI port (H7RS / N6): it auto-wires the two-line minimum on NCS1, and the wider modes join by assigning IO2-IO15 on the canvas",
                                        ModuleKind::GenericInterfaceOspi => "Add an external flash or RAM on an OCTOSPI port: it auto-wires the two-line minimum (CLK, NCS, IO0-IO1), and the quad and octal modes join by assigning IO2-IO7 on the canvas",
                                        ModuleKind::GenericInterfaceQspi => "Add an external-flash module on the QUADSPI: it auto-wires bank 1 whole (CLK, NCS, IO0-IO3), and bank 2 joins the same module when you assign its pads on the canvas",
                                        ModuleKind::GenericInterfaceSdmmc => "Add an SD card / eMMC module: it auto-wires CK, CMD and D0, and the wider buses join by assigning D1-D3 (4-bit) or D1-D7 (8-bit) on the canvas",
                                        ModuleKind::GenericInterfaceSai => "Add a SAI module for one audio unit: it auto-wires sub-block A (SCK/SD/FS), and sub-block B joins the same module when you assign its pads on the canvas",
                                        ModuleKind::GenericInterfaceDac => "Add a DAC module for one peripheral: it takes a free OUT pad now, and the other channel of the SAME DAC joins it when you assign it on the canvas",
                                        ModuleKind::GenericInterfaceI2s => "Add a virtual I2S audio device and auto-wire it to free I2S CK/WS/SD pins (MCK stays free unless you assign it). I2Sn runs on SPIn, so the two cannot both be built",
                                        ModuleKind::GenericInterfaceTimer => "Add a PWM module for one TIMER: it takes a free channel now, and any other channel of the SAME timer you assign on the canvas joins it (they share the frequency)",
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
                                    // 11, like every other button on this bar.
                                    // Without an explicit size they took the
                                    // global `TextStyle::Button` (12.5) and were
                                    // visibly bigger than the rest of the row.
                                    let text = if added {
                                        egui::RichText::new(label)
                                            .size(11.0)
                                            .color(mod_gui::module_color(kind, 1))
                                    } else {
                                        egui::RichText::new(label).size(11.0)
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
                                            // Adding from a collapsed bar means
                                            // "I want to set this up" — so the
                                            // panel opens with the new module's
                                            // config already unfolded, instead
                                            // of leaving the click looking like
                                            // it did nothing.
                                            open_after_add =
                                                mcu.modules.last().map(|m| m.id.clone());
                                            collapsed = false;
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

                                // Find a pin on the diagram: matches stay bright,
                                // every other pin fades to half opacity. Purely a
                                // view filter — nothing is selected or changed.
                                ui.separator();
                                ui.label(
                                    egui::RichText::new(ph::MAGNIFYING_GLASS)
                                        .size(12.0)
                                        .color(egui::Color32::from_gray(160)),
                                );
                                ui.add(
                                    egui::TextEdit::singleline(&mut mcu.pin_search)
                                        .hint_text("find pin")
                                        .desired_width(96.0)
                                        .font(egui::FontId::proportional(11.0)),
                                )
                                .on_hover_text(
                                    "Highlight pins by NAME (any part: pa5, osc, pb) or by \
                                     NUMBER (exact: 13). Everything else fades to 30%.",
                                );
                                if !mcu.pin_search.trim().is_empty() {
                                    // Match count doubles as the "nothing found"
                                    // signal — the diagram deliberately does NOT
                                    // fade at all when a query matches nothing.
                                    let n = mcu.pin_search_hits().len();
                                    let (txt, col) = if n == 0 {
                                        ("no match".to_owned(), egui::Color32::from_rgb(220, 160, 70))
                                    } else {
                                        (format!("{n} pin{}", if n == 1 { "" } else { "s" }),
                                         egui::Color32::from_gray(150))
                                    };
                                    ui.label(egui::RichText::new(txt).size(10.5).color(col));
                                    if ui
                                        .button(egui::RichText::new(ph::X).size(11.0))
                                        .on_hover_text("Clear the search")
                                        .clicked()
                                    {
                                        mcu.pin_search.clear();
                                    }
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
                            // Clicking a module box on the canvas has to REACH
                            // something: with the panel put away, the request
                            // would be taken here and quietly dropped, and the
                            // click would look broken.
                            if to_open.is_some() {
                                collapsed = false;
                            }
                            // Empty-canvas click last frame → close EVERY entry
                            // (the canvas cleared its selection at the same time).
                            let collapse_all = std::mem::take(&mut mcu.collapse_modules);
                            // The module selectors follow the STAGED runtime (what
                            // the user is about to Apply), not the applied one.
                            //
                            // ESP is excluded on purpose: the embassy Blocking |
                            // Async-DMA row belongs to the embassy bus drivers in
                            // src/pins/configs/*.rs, which the esp-rtos backend
                            // does not emit — its USART/SPI/I2C stay the blocking
                            // esp-hal ones inline in main.rs, same as ESP
                            // blocking. Offering the row there would promise a
                            // driver the generator never writes.
                            let is_async = mcu.pending_is_async()
                                && !crate::panels::mcu_module::codegen::family::async_is_esp(
                                    &mcu.family,
                                );
                            let is_native = mcu.pending_is_native();

                            if !mcu.modules.is_empty() && !collapsed {
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
                                // Cloned once per frame: the picker needs the
                                // chip's channels while `mcu.modules` is
                                // borrowed mutably below.
                                let chip_dma = mcu.dma.clone();
                                let family = mcu.family.clone();
                                // Read before `mcu.modules` is borrowed mutably.
                                let usart_line_extras =
                                    crate::panels::mcu_module::stm32_pin_data::usart_has_swap_invert(
                                        mcu.usart_ip.as_deref(),
                                    );
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
                                // ── Two columns: the list on the LEFT, the
                                //    open modules' configs on the RIGHT, one
                                //    under another.
                                //
                                // One column could not hold both: a USART's
                                // config is a dozen rows, so an expanded module
                                // pushed every other one off the bottom of the
                                // panel and the list stopped being a list.
                                //
                                // The open/closed bit still lives in a
                                // `CollapsingState`, but keyed off the MODULE ID
                                // rather than the Ui: the header and the body are
                                // in different Uis now, so a Ui-derived id would
                                // no longer find the state the other one stored.
                                let cs_id =
                                    |id: &str| egui::Id::new(("vmod_hdr", id));
                                let list_w = (ui.available_width() * 0.32).clamp(160.0, 260.0);
                                // Filled by the left column, read by the right —
                                // the two loops borrow `mcu.modules` in turn.
                                let mut open_ids: Vec<String> = Vec::new();
                                let mut clicked: Option<String> = None;
                                // Everything the panel spends ABOVE the columns
                                // — the Apply bar, the palette row, the spacing.
                                // Measured, not guessed: a new button in that
                                // toolbar would otherwise start clipping the
                                // bottom of every config.

                                // The body is allocated at OUR height, so the
                                // panel that hugs it ends up exactly as tall as
                                // the handle says.
                                let body = egui::vec2(ui.available_width(), body_h);
                                ui.allocate_ui(body, |ui| {
                                ui.horizontal_top(|ui| {
                                    // ── left: the list ──
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(list_w, ui.available_height()),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            let list_out = egui::ScrollArea::vertical()
                                                .id_salt("vmod_list")
                                                .auto_shrink([false, false])
                                                .show(ui, |ui| {
                                                    for m in &mcu.modules {
                                                        let mut st =
                                                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                                                ui.ctx(),
                                                                cs_id(&m.id),
                                                                false,
                                                            );
                                                        // "Collapse everything"
                                                        // outranks the per-module
                                                        // toggle: both can't be
                                                        // true in one frame, but
                                                        // if they ever were,
                                                        // closing is the safe
                                                        // answer.
                                                        if collapse_all {
                                                            st.set_open(false);
                                                        } else if let Some(t) =
                                                            to_open.as_deref()
                                                        {
                                                            // A click on the CANVAS
                                                            // means "show me this
                                                            // one": every other
                                                            // config closes, so the
                                                            // right column holds
                                                            // exactly the module
                                                            // whose box was hit.
                                                            // Several at once is a
                                                            // deliberate act, and
                                                            // the list is where you
                                                            // do it.
                                                            if t == m.id {
                                                                let now = st.is_open();
                                                                st.set_open(!now);
                                                            } else {
                                                                st.set_open(false);
                                                            }
                                                        }
                                                        let open = st.is_open();
                                                        st.store(ui.ctx());
                                                        if open {
                                                            open_ids.push(m.id.clone());
                                                        }

                                                        let title = mod_gui::module_title(m);
                                                        let c = mod_gui::module_color(
                                                            m.kind,
                                                            m.instance(),
                                                        );
                                                        // Same 10 %-opacity tint as
                                                        // the module's box on the
                                                        // canvas; the OPEN one is
                                                        // brighter, so the list says
                                                        // which config is on the right.
                                                        let bg = egui::Color32::from_rgba_unmultiplied(
                                                            c.r(),
                                                            c.g(),
                                                            c.b(),
                                                            if open { 56 } else { 26 },
                                                        );
                                                        let resp = egui::Frame::new()
                                                            .fill(bg)
                                                            .inner_margin(egui::Margin::symmetric(6, 3))
                                                            .corner_radius(egui::CornerRadius::same(4))
                                                            .show(ui, |ui| {
                                                                ui.set_width(ui.available_width());
                                                                ui.horizontal(|ui| {
                                                                    ui.label(
                                                                        egui::RichText::new(if open {
                                                                            ph::CARET_DOWN
                                                                        } else {
                                                                            ph::CARET_RIGHT
                                                                        })
                                                                        .size(11.0)
                                                                        .color(c),
                                                                    );
                                                                    ui.label(
                                                                        egui::RichText::new(title)
                                                                            .strong()
                                                                            .color(c),
                                                                    );
                                                                });
                                                            })
                                                            .response
                                                            .interact(egui::Sense::click());
                                                        if resp
                                                            .on_hover_cursor(
                                                                egui::CursorIcon::PointingHand,
                                                            )
                                                            .clicked()
                                                        {
                                                            clicked = Some(m.id.clone());
                                                        }
                                                        ui.add_space(2.0);
                                                    }
                                                });
                                            // What "show me the whole list"
                                            // costs — the caret button opens the
                                            // panel to exactly this.
                                            list_h = list_out.content_size.y + 8.0;
                                        },
                                    );
                                    ui.separator();

                                    // ── right: one config block per open module ──
                                    //
                                    // The explicit `top_down` is load-bearing: a
                                    // `ScrollArea` inherits its parent's layout,
                                    // and the parent here is a HORIZONTAL one —
                                    // so without it the title and the Name row
                                    // laid out side by side and the config's grid
                                    // had nowhere left to go.
                                    let cfg_size =
                                        egui::vec2(ui.available_width(), ui.available_height());
                                    let out = ui
                                        .allocate_ui_with_layout(
                                            cfg_size,
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                egui::ScrollArea::vertical()
                                                    .id_salt("vmod_cfg")
                                                    .auto_shrink([false, false])
                                                    .show(ui, |ui| {
                                            if open_ids.is_empty() {
                                                ui.add_space(6.0);
                                                ui.label(
                                                    egui::RichText::new(
                                                        "Click a module on the left to configure it here.",
                                                    )
                                                    .size(11.0)
                                                    .color(egui::Color32::from_gray(130)),
                                                );
                                                return;
                                            }
                                            for m in &mut mcu.modules {
                                                if !open_ids.iter().any(|id| id == &m.id) {
                                                    continue;
                                                }
                                                let title = mod_gui::module_title(m);
                                                let mod_color =
                                                    mod_gui::module_color(m.kind, m.instance());
                                                let bg = egui::Color32::from_rgba_unmultiplied(
                                                    mod_color.r(),
                                                    mod_color.g(),
                                                    mod_color.b(),
                                                    26,
                                                );
                                                // The title rides along with the
                                                // config: with several open at
                                                // once, an unlabelled block would
                                                // not say whose it is.
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
                                                // One id namespace per module.
                                                // `module_config_ui` builds a
                                                // `Grid::new("module_cfg")` and
                                                // salt-less combo boxes, so two
                                                // configs drawn in the SAME frame
                                                // — which is what the right column
                                                // does — collide on every one of
                                                // them, and egui paints an
                                                // "ID clash" banner across the
                                                // panel.
                                                // Owned: the closure needs `m`
                                                // mutably, so the salt cannot
                                                // borrow out of it.
                                                let salt = m.id.clone();
                                                ui.push_id(salt, |ui| {
                                                    mod_gui::module_config_ui(
                                                        ui, m, &pin_names, &pin_sigs,
                                                        &pin_blocked, &mut pin_labels,
                                                        &pin_funcs_current, &pin_funcs,
                                                        &mut pin_fn_choice, is_async, is_native,
                                                        &family, pending, chip_dma.as_ref(),
                                                        usart_line_extras,
                                                    );
                                                });
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
                                                ui.add_space(10.0);
                                            }
                                                    })
                                            },
                                        )
                                        .inner;
                                    // + a row of slack so the last button is not
                                    // flush against the panel edge.
                                    needed_h = out.content_size.y + 16.0;
                                });
                                });

                                // Which modules are open, as one number, so the
                                // panel can be resized ONCE when the set changes.
                                open_sig = open_ids.iter().fold(0u64, |h, id| {
                                    id.bytes().fold(h.wrapping_mul(31).wrapping_add(1), |h, b| {
                                        h.wrapping_mul(131).wrapping_add(u64::from(b))
                                    })
                                });
                                // Applied after both loops — each borrowed
                                // `mcu.modules`, and the toggle only touches
                                // egui's own state.
                                if let Some(id) = clicked {
                                    let mut st =
                                        egui::collapsing_header::CollapsingState::load_with_default_open(
                                            ui.ctx(),
                                            cs_id(&id),
                                            false,
                                        );
                                    let now = st.is_open();
                                    st.set_open(!now);
                                    st.store(ui.ctx());
                                }
                                // A module added this frame is not in the list
                                // yet (it was built after the loop ran), so its
                                // config is opened here and shows next frame —
                                // by which time `open_sig` has changed and the
                                // panel has already grown to fit it.
                                if let Some(id) = &open_after_add {
                                    let mut st =
                                        egui::collapsing_header::CollapsingState::load_with_default_open(
                                            ui.ctx(),
                                            cs_id(id),
                                            false,
                                        );
                                    st.set_open(true);
                                    st.store(ui.ctx());
                                    ui.ctx().request_repaint();
                                }
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

                    // One frame late by construction (the height of a config is
                    // only known once it has been laid out), so a change has to
                    // repaint — otherwise the panel settles on the next mouse
                    // move instead of now.
                    if (self.vmod_needed_h - needed_h).abs() > 0.5 {
                        self.vmod_needed_h = needed_h;
                        ui.ctx().request_repaint();
                    }
                    // Make room ONCE, when the set of open configs changes —
                    // never every frame, or the handle could not be dragged back
                    // down. Growing only: nobody wants the panel they just
                    // widened snapping shut because the next config is shorter.
                    if self.vmod_open_sig != open_sig && !collapsed {
                        body_h = body_h.max(self.vmod_needed_h.min(auto_cap));
                        ui.ctx().request_repaint();
                    }
                    if (self.vmod_list_h - list_h).abs() > 0.5 && list_h > 0.0 {
                        self.vmod_list_h = list_h;
                    }
                    // Expanded with the caret button: open tall enough for the
                    // whole module list, and close every config on the way —
                    // the gesture means "show me what modules there are", not
                    // "restore the six-row form I was last editing". Uses the
                    // height measured on a PREVIOUS frame (this one was laid out
                    // collapsed, so the list has no size yet); `body_cap` still
                    // has the last word, since the diagram above needs to live.
                    if expand_clicked {
                        body_h = self.vmod_list_h.clamp(60.0, body_cap);
                        if let Some(mcu) = &mut self.mcu {
                            mcu.collapse_modules = true;
                        }
                        ui.ctx().request_repaint();
                    }
                    self.vmod_open_sig = open_sig;
                    self.vmod_collapsed = collapsed;
                    self.vmod_body_h = body_h;

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
                            no_mcu = Some("Pin configuration");
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
                McuTab::Peripherals if self.mcu.is_none() => {
                    // Checked here rather than letting `show_peripherals_tab`
                    // draw its own note, so every tab says the same thing and
                    // offers the same way out. Its guard stays as defence for
                    // any other caller.
                    no_mcu = Some("Peripheral configuration");
                }
                McuTab::Peripherals => {
                    // Assigning a function here mutates the MCU just like the
                    // Pins tab, so re-sync the generated pins/ files on change.
                    let changed = show_peripherals_tab(ui, &mut self.mcu, &mut self.peripheral_query);
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
                        // `clock_ui` is a disjoint field, so it can be borrowed
                        // alongside `self.mcu`; the tab writes the dragged
                        // positions and its note straight into it.
                        let out = mcu.draw_clock_tab(ui, &mut self.clock_ui);
                        if out.save_to_definition {
                            self.save_clock_to_definition();
                        }
                    }
                    None => no_mcu = Some("Clock configuration"),
                },
                McuTab::Configuration if self.mcu.is_none() => {
                    no_mcu = Some("Peripheral configuration");
                }
                McuTab::Configuration => self.show_configuration_tab(ui),
                McuTab::System => match &mut self.mcu {
                    Some(mcu) => Self::show_system_tab(ui, mcu),
                    None => no_mcu = Some("System configuration"),
                },
                // Module-relationship diagram — chip-agnostic (works with no
                // MCU selected), so it doesn't gate on `self.mcu`.
                McuTab::Structure => self.show_structure_tab(ui),
                McuTab::Definition => self.show_definition_tab(ui),
                McuTab::Reference => self.show_reference_tab(ui),
            }

            if let Some(what) = no_mcu {
                self.show_no_mcu_notice(ui, what);
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
            // What this chip cannot do, said where the project actually starts.
            //
            // The import report already says it — but the import report is not
            // where most chips are met. One arrives from a shared `.ron`, from
            // the recent list, or inside a project someone else made, and then
            // nothing ever explained why the clock in `main.rs` is a commented
            // skeleton and the DMA a `TODO`. The verdict now travels with the
            // chip instead of with the moment it was imported.
            let gaps = crate::app::dialogs::local_chip_gaps(
                crate::panels::mcu_module::codegen::rcc::generates_clock_code_for(
                    &mcu.family,
                    &mcu.clock,
                ),
                mcu.dma.as_ref().map_or(0, |d| d.channels.len()),
            );
            if !gaps.is_empty() {
                ui.add_space(10.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  {} is missing {} of what codegen needs",
                            ph::WARNING,
                            mcu.name,
                            if gaps.len() == 1 { "one piece" } else { "pieces" }
                        ))
                        .size(12.0)
                        .color(egui::Color32::from_rgb(230, 190, 90)),
                    );
                    for g in &gaps {
                        ui.label(
                            egui::RichText::new(format!("    {}  {g}", ph::ARROW_ELBOW_DOWN_RIGHT))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(200, 170, 100)),
                        );
                    }
                    // Not a refusal. The rest of the chip works, and half a
                    // project is worth more than none — the point is that you
                    // find out now rather than from generated code.
                    ui.label(
                        egui::RichText::new(
                            "    Everything else still generates; these parts come out as comments or TODOs.",
                        )
                        .size(10.5)
                        .color(egui::Color32::GRAY),
                    );
                });
            }

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
            let rtic_ok = family::rtic_supported(&mcu.family);

            // The cards edit the STAGED choice (`pending_*`); nothing regenerates
            // until "Apply" commits it (see the Apply bar below).
            Self::runtime_apply_bar(ui, mcu);

            // ── Blocking ─────────────────────────────────────────────────────
            let blocking_sel = mcu.pending_runtime == Runtime::Blocking;
            // The subtitle names the HAL this CHIP will use — see
            // `family::blocking_hal_note`. A fixed sentence described the F1 path
            // only, which made embassy-stm32 appearing in a Blocking project on
            // any other STM32 look like the choice had been ignored.
            let blocking_note = format!(
                "#[entry] fn main() -> !  ·  {}",
                family::blocking_hal_note(&mcu.family)
            );
            if runtime_card(
                ui,
                blocking_sel,
                true,
                "Blocking (bare-metal)",
                &blocking_note,
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
            let native_why = family::native_unavailable_reason(&mcu.family);
            let mut native_resp = runtime_card(
                ui,
                native_sel,
                native_ok,
                "Native (bare-metal, HAL types)",
                "#[entry] fn main() -> !  ·  concrete HAL types everywhere \
                 (Serial / Spi / BlockingI2c) — no portable bridges",
            );
            if let Some(why) = &native_why {
                native_resp = native_resp.on_hover_text(why);
            }
            if native_resp.clicked() && native_ok {
                mcu.pending_runtime = Runtime::Native;
            }
            // The reason a card is greyed belongs BESIDE it, not three clicks
            // deep in "Details" — a disabled control with no explanation is the
            // question this answers.
            disabled_reason(ui, native_why.as_deref());
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
            // Two different async stacks behind one card: embassy-stm32 on ARM,
            // esp-rtos (same embassy executor, ESP scheduler) on the ESP32-C3.
            let esp_async = family::async_is_esp(&mcu.family);
            let async_sel = mcu.pending_runtime == Runtime::Async;
            let async_why = family::async_unavailable_reason(&mcu.family);
            let mut async_resp = runtime_card(
                ui,
                async_sel,
                async_ok,
                if esp_async {
                    "Async (embassy on esp-rtos)"
                } else {
                    "Async (embassy)"
                },
                if esp_async {
                    "#[esp_rtos::main] async fn main(Spawner)  ·  \
                     embassy executor scheduled by esp-rtos"
                } else {
                    "#[embassy_executor::main] async fn main(Spawner)  ·  \
                     .await-able drivers on embassy-stm32"
                },
            );
            if let Some(why) = &async_why {
                async_resp = async_resp.on_hover_text(why);
            }
            if async_resp.clicked() && async_ok {
                mcu.pending_runtime = Runtime::Async;
            }
            // Same rule as the Native and RTIC cards: the reason a card is
            // greyed belongs beside it, not in "Details" the user cannot open.
            disabled_reason(ui, async_why.as_deref());
            let esp_async_details: &[(&str, &str)] = &[
                ("On Apply:", "Regenerates main.rs with the esp-rtos entry + Spawner and adds the async \
                               Cargo.toml deps — then builds. The pin bindings themselves do NOT change: \
                               esp-hal's Output/Input/Uart/Spi/I2c are the same types in both runtimes."),
                ("Entry:", "#[esp_rtos::main] async fn main(_spawner: Spawner) — esp_rtos::start(...) hands \
                            TIMG0 + software interrupt 0 to the scheduler, then embassy_time::Timer and \
                            .await work in the loop."),
                ("TIMG0:", "The scheduler takes peripherals.TIMG0, so your own code cannot also claim it. \
                            That is the cost of having embassy-time on this chip."),
                ("Why esp-rtos:", "NOT esp-hal-embassy: that crate needs esp-hal's private __esp_hal_embassy \
                                   feature, dropped in esp-hal 1.1 (the version this template pins), so cargo \
                                   cannot even resolve it. esp-rtos is its replacement, same maintainers."),
                ("Cargo.toml:", "Adds esp-rtos (chip + embassy features), embassy-executor 0.10 and \
                                 embassy-time 0.5. Leaving Async removes them again."),
                ("Not yet:", "USART/SPI/I2C stay the blocking esp-hal drivers written inline in main.rs — \
                              ESP has no src/pins/configs/*.rs, so there is no async bus driver to select."),
                ("Applies to:", "ESP32-C3 (riscv32imc). The STM32 async path is embassy-stm32 instead."),
            ];
            runtime_details(
                ui,
                "rt_details_async",
                if esp_async {
                    esp_async_details
                } else {
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
                    ("Applies to:", "STM32F4/G0/G4/L4/H7/WBA/… (embassy families). NOT STM32F1 (on stm32f1xx-hal). \
                                     ESP32-C3 has its own async path (esp-rtos)."),
                    ]
                },
                if esp_async {
                    "let timg0 = TimerGroup::new(peripherals.TIMG0);\n\
                     esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);\n\
                     // then: embassy_time::Timer::after_millis(500).await;"
                } else {
                    "let mut _serial1 = pins::configs::usart1::init(p.USART1, p.PA10, p.PA9);\n\
                     _serial1.write_all(b\"hi\").await.ok();"
                },
            );

            let rtic_sel = mcu.pending_runtime == Runtime::Rtic;
            let rtic_why = family::rtic_unavailable_reason(&mcu.family);
            let mut rtic_resp = runtime_card(
                ui,
                rtic_sel,
                rtic_ok,
                "RTIC 2",
                "#[rtic::app] with Shared / Local / init / idle  ·                   one hardware task per interrupt pin",
            );
            if let Some(why) = &rtic_why {
                rtic_resp = rtic_resp.on_hover_text(why);
            }
            if rtic_resp.clicked() && rtic_ok {
                mcu.pending_runtime = Runtime::Rtic;
            }
            disabled_reason(ui, rtic_why.as_deref());
            runtime_details(
                ui,
                "rt_details_rtic",
                &[
                    ("On Apply:", "Regenerates main.rs as an #[rtic::app] module. The init sequence is                                    UNCHANGED - same clocks, same pin bindings, same pins/configs/*::init calls -                                    it just moves into #[init]. The config files themselves are untouched."),
                    ("Entry:", "RTIC owns main. #[init] runs once and returns (Shared, Local); #[idle] is the                                 background loop (wfi)."),
                    ("Interrupts:", "A GPIO input with an Edge set on the Pins canvas becomes a                                      #[task(binds = EXTIn)]. RTIC enables the vector itself, so the generated                                      code never calls NVIC::unmask."),
                    ("Shared EXTI vectors:", "STM32 gives lines 5-9 and 10-15 ONE vector each, so pins on them                                               share a single task that branches on check_interrupt(). Two tasks                                               on one vector would not compile."),
                    ("Cargo.toml:", "Adds rtic (backend feature picked from the chip's Rust target: thumbv6 /                                      thumbv7 / thumbv8) and rtic-monotonics with cortex-m-systick. cortex-m                                      already carries critical-section-single-core. Leaving RTIC removes them."),
                    ("Applies to:", "STM32F1 only for now - the interrupt tasks are written against                                      stm32f1xx-hal's ExtiPin trait."),
                ],
                "#[task(binds = EXTI0, local = [pa0_in_button])]
                 fn exti0(cx: exti0::Context) {
                     cx.local.pa0_in_button.clear_interrupt_pending_bit();
                 }",
            );

            ui.add_space(10.0);
            // The old catch-all summary here ("only Blocking is supported…") is
            // gone: every unavailable card now carries its OWN reason, and that
            // sentence hardcoded a family list ("STM32F4/G0/G4/L4/H7/…") that
            // goes stale the moment a family is added.
            if native_sel {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  All USART/SPI/I2C peripherals now expose the concrete \
                         stm32f1xx-hal types (the per-module Portable/Native selector is \
                         subsumed). USART `init` returns the split (Tx, Rx).",
                        ph::INFO,
                    ))
                    .color(egui::Color32::from_rgb(120, 170, 220)),
                );
            } else if async_sel && async_ok {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  main.rs now runs on the embassy executor — add \
                         `embassy_time::Timer::after_millis(..).await` in the loop.",
                        ph::INFO,
                    ))
                    .color(egui::Color32::from_rgb(120, 170, 220)),
                );
            } else if async_sel {
                // Async SELECTED on a family that has no async backend. The card
                // itself cannot be clicked there, but `mcu.config` is a file the
                // user can edit, and `@runtime Async` on an F1 project lands
                // here. Codegen falls back to the blocking backend (see
                // `Mcu::is_async`), so promising the executor would be a lie
                // about code that does not exist.
                ui.label(
                    egui::RichText::new(format!(
                        "{}  Async is selected but INERT on this chip — main.rs is still \
                         the blocking project, and the dependencies with it. Pick a \
                         runtime above, or see the reason on the card.",
                        ph::WARNING,
                    ))
                    .color(egui::Color32::from_rgb(220, 160, 70)),
                );
            }

            // ── GPIO In/Out bridge (io.rs) ───────────────────────────────────
            // Shown ONLY where the choice exists. The `io.rs` bridge is an F1
            // feature; elsewhere GPIO binds straight to the HAL's own types, and
            // rendering the section greyed out on those chips was dead UI that
            // read like a misconfiguration — its own subtitle said "STM32F1".
            use crate::panels::mcu_module::modules::ApiStyle;
            if family::gpio_bridge_supported(&mcu.family) {
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
            // The "this is an F1 feature" warning is gone with the section it
            // explained — a chip that never shows the choice needs no excuse.
            if mcu.pending_runtime == Runtime::Native {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  The Native runtime already binds all GPIO raw — this choice \
                         applies on the Blocking runtime.",
                        ph::INFO,
                    ))
                    .color(egui::Color32::from_rgb(120, 170, 220)),
                );
            }
            } // end: only families with the io.rs bridge

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
/// The one-line "why is this greyed out" note under a disabled runtime card.
/// `None` — the card is enabled — draws nothing at all.
///
/// Both cards stay VISIBLE when unavailable rather than being hidden: the
/// question they answer ("why can't I pick RTIC on this chip?") can only be
/// asked about something you can see.
fn disabled_reason(ui: &mut egui::Ui, why: Option<&str>) {
    let Some(why) = why else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(format!("{}  {why}", ph::INFO))
                .size(10.5)
                .color(egui::Color32::from_rgb(170, 150, 110)),
        );
    });
}

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
