//! Central "MCU Configurator" panel — chip label, tab bar (Pins / Peripherals
//! / Clock / System) and the active tab's content.
//!
//! Inherent method on [`AppIde`]; the Pins tab draws the chip and, on any pin
//! change, re-syncs the generated `pins/` files.

use super::tabs::show_peripherals_tab;
use super::{AppIde, McuTab};
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
                    let label = egui::RichText::new(tab.label())
                        .size(13.0)
                        .color(if is_active {
                            egui::Color32::WHITE
                        } else if tab == McuTab::Definition {
                            egui::Color32::from_rgb(120, 180, 240)
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

            ui.separator();

            // Tab content
            match self.active_tab {
                McuTab::Pins => {
                    // ── Virtual-module palette + list, in a scrollable strip
                    //    BELOW the chip. Add a module (auto-wires to compatible
                    //    pins), rename it, edit its config, or remove it. Add/
                    //    remove change pin functions, so re-sync pins/ after.
                    let mut modules_changed = false;
                    egui::TopBottomPanel::bottom("vmodules_panel")
                        .resizable(true)
                        .default_height(190.0)
                        .show_inside(ui, |ui| {
                            let Some(mcu) = &mut self.mcu else { return };
                            use crate::panels::mcu_module::mcu::gui::modules as mod_gui;
                            use crate::panels::mcu_module::modules::ModuleKind;

                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Virtual modules:")
                                        .size(12.0)
                                        .color(egui::Color32::from_rgb(150, 150, 160)),
                                );
                                for (kind, hover) in [
                                    (
                                        ModuleKind::GenericInterfaceUsart,
                                        "Add a virtual USART device and auto-wire it to a free USART TX/RX pin pair",
                                    ),
                                    (
                                        ModuleKind::GenericInterfaceSpi,
                                        "Add a virtual SPI device and auto-wire it to free SPI SCK/MOSI/MISO(/NSS) pins",
                                    ),
                                    (
                                        ModuleKind::GenericInterfaceI2c,
                                        "Add a virtual I2C device and auto-wire it to a free I2C SCL/SDA pin pair",
                                    ),
                                    (
                                        ModuleKind::GenericInterfaceCan,
                                        "Add a virtual CAN device and auto-wire it to the CAN RX/TX pins (needs the bxcan crate)",
                                    ),
                                    (
                                        ModuleKind::GenericInterfaceUsb,
                                        "Add a virtual USB device and auto-wire it to the USB D-/D+ pins (PA11/PA12)",
                                    ),
                                ] {
                                    if ui
                                        .button(format!("{} {}", ph::PLUS, kind.short()))
                                        .on_hover_text(hover)
                                        .clicked()
                                        && mcu.add_module(kind)
                                    {
                                        modules_changed = true;
                                    }
                                }
                            });

                            // Id of a module clicked on the canvas last frame →
                            // TOGGLE its list entry this frame (expand if closed,
                            // collapse if open), then it's user-controlled again.
                            let to_open = mcu.expand_module.take();

                            if !mcu.modules.is_empty() {
                                let pin_names: std::collections::HashMap<usize, String> = mcu
                                    .iter_all_pins()
                                    .map(|p| (p.number, p.name.clone()))
                                    .collect();
                                let mut remove_id: Option<String> = None;
                                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                                    for m in &mut mcu.modules {
                                        let title = mod_gui::module_title(m);
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
                                        if toggle {
                                            state.toggle(ui);
                                        }
                                        state
                                            .show_header(ui, |ui| {
                                                ui.label(egui::RichText::new(title).strong());
                                            })
                                            .body(|ui| {
                                                // Rename field — appended to the generated
                                                // variable name(s); also shown in the title.
                                                ui.horizontal(|ui| {
                                                    ui.label("Name:");
                                                    ui.add(
                                                        egui::TextEdit::singleline(
                                                            m.config.custom_label_mut(),
                                                        )
                                                        .hint_text("variable name")
                                                        .desired_width(160.0),
                                                    );
                                                });
                                                mod_gui::module_config_ui(ui, m, &pin_names);
                                                ui.add_space(4.0);
                                                if ui
                                                    .button(format!("{} Remove module", ph::TRASH))
                                                    .clicked()
                                                {
                                                    remove_id = Some(m.id.clone());
                                                }
                                            });
                                    }
                                });
                                if let Some(id) = remove_id {
                                    mcu.remove_module(&id);
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

                    // Diagram fills the remaining (top) area.
                    // Computed before borrowing `self.mcu` mutably below.
                    let chip_label = self.selected_label();
                    let pin_changed = match &mut self.mcu {
                        Some(mcu) => {
                            // AUTO-ZOOM: the chip + virtual modules are drawn at
                            // their natural fixed size inside an `egui::Scene`
                            // whose rect is fed each frame from LAST frame's
                            // content bounds — so the canvas always rescales to
                            // fit the panel (window resizes, panel drags, new
                            // modules). Drag-pan is disabled and the rect is
                            // overwritten every frame, so no manual pan/zoom
                            // sticks: it is a pure fit-to-view. Capped at 1.0 —
                            // a large panel shows the chip at 100%, centered,
                            // never blown up.
                            let mut scene_rect = self.mcu_scene_bounds;
                            let mut content_bounds = egui::Rect::NOTHING;
                            let inner = egui::Scene::new()
                                .zoom_range(0.05..=1.0)
                                .drag_pan_buttons(egui::DragPanButtons::empty())
                                .show(ui, &mut scene_rect, |ui| {
                                    let r = mcu.draw(ui);
                                    content_bounds = ui.min_rect();
                                    r
                                })
                                .inner;
                            self.mcu_scene_bounds = content_bounds;
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
                        let _changed = mcu.draw_clock_tab(ui);
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
                McuTab::System => {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  System configuration — coming soon",
                                ph::GEAR
                            ))
                            .size(16.0)
                            .color(egui::Color32::GRAY),
                        );
                    });
                }
                // Module-relationship diagram — chip-agnostic (works with no
                // MCU selected), so it doesn't gate on `self.mcu`.
                McuTab::Structure => self.show_structure_tab(ui),
                McuTab::Definition => self.show_definition_tab(ui),
            }
        });
    }

    /// The F12 "Go to definition" snippet (external / crate / std files) —
    /// moved here from the bottom diagnostics panel on 2026-07-10. The whole
    /// file is shown (scrollable above and below the target); rows are
    /// virtualized and the target line is scrolled near the top once on open,
    /// drawn coloured so it stands out from the surrounding code.
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
