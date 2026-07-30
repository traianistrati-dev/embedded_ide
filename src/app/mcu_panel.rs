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
                                    };
                                    if ui
                                        .add_enabled(
                                            can_add,
                                            egui::Button::new(format!(
                                                "{} {}",
                                                ph::PLUS,
                                                kind.short()
                                            )),
                                        )
                                        .on_hover_text(hover)
                                        .on_disabled_hover_text(if kind.is_single_instance() {
                                            "this chip has only one such peripheral and it's already used"
                                        } else {
                                            "every instance of this peripheral is already wired to a module — remove one to free it"
                                        })
                                        .clicked()
                                        && mcu.add_module(kind)
                                    {
                                        modules_changed = true;
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

                            // Id of a module clicked on the canvas last frame →
                            // TOGGLE its list entry this frame (expand if closed,
                            // collapse if open), then it's user-controlled again.
                            let to_open = mcu.expand_module.take();
                            // Async runtime → SPI/I2C modules show the Blocking|
                            // Async-DMA selector instead of Portable|Native; Native
                            // runtime forces concrete HAL (per-module selector hidden).
                            let is_async = mcu.is_async();
                            let is_native = mcu.is_native();

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
                                                mod_gui::module_config_ui(ui, m, &pin_names, is_async, is_native);
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

            // ── Blocking ─────────────────────────────────────────────────────
            let blocking_sel = mcu.runtime == Runtime::Blocking;
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
                mcu.runtime = Runtime::Blocking;
            }
            runtime_details(
                ui,
                "rt_details_blocking",
                &[
                    ("Entry:", "#[cortex_m_rt::entry] fn main() -> ! — classic bare-metal, runs forever."),
                    ("Drivers:", "Each USART/SPI/I2C initialises in src/pins/configs/*.rs and is exposed \
                                  through STANDARD portable traits — embedded-io (serial) + embedded-hal 1.0 \
                                  (SpiBus / I2c / OutputPin). App code generic over those traits ports across \
                                  chips/HALs unchanged."),
                    ("Per module:", "Each bus module has a Portable | Native selector; the portable wrapper's \
                                     .0 still gives the raw HAL object back."),
                    ("Applies to:", "Every chip — this is the default runtime."),
                ],
                "let mut _serial1 = pins::configs::usart1::init(...);\n\
                 fn app<S: embedded_io::Read + embedded_io::Write>(s: &mut S) { /* portable */ }",
            );
            ui.add_space(6.0);

            // ── Native (concrete HAL) ────────────────────────────────────────
            let native_sel = mcu.runtime == Runtime::Native;
            let native_resp = runtime_card(
                ui,
                native_sel,
                native_ok,
                "Native (bare-metal, HAL types)",
                "#[entry] fn main() -> !  ·  concrete HAL types everywhere \
                 (Serial / Spi / BlockingI2c) — no portable bridges",
            );
            if native_resp.clicked() && native_ok {
                mcu.runtime = Runtime::Native;
            }
            runtime_details(
                ui,
                "rt_details_native",
                &[
                    ("Entry:", "#[entry] fn main() -> ! — the same bare-metal entry as Blocking."),
                    ("Drivers:", "init returns the CONCRETE stm32f1xx-hal types: Serial split into (Tx, Rx), \
                                  Spi<…>, BlockingI2c<…>. No portable bridges, no extra trait crates, full HAL \
                                  features."),
                    ("Scope:", "Project-wide — forces ALL USART/SPI/I2C to Native, so the per-module \
                                Portable/Native selector is hidden."),
                    ("Applies to:", "STM32F1 only (the family with concrete-HAL templates). Greyed on other \
                                     families, whose blocking HAL types are already concrete."),
                ],
                "let (mut _tx1, mut _rx1) = pins::configs::usart1::init(...);\n\
                 // use _tx1 with writeln!(), _rx1 with .read()",
            );
            ui.add_space(6.0);

            // ── Async (embassy) ──────────────────────────────────────────────
            let async_sel = mcu.runtime == Runtime::Async;
            let async_resp = runtime_card(
                ui,
                async_sel,
                async_ok,
                "Async (embassy)",
                "#[embassy_executor::main] async fn main(Spawner)  ·  \
                 .await-able drivers on embassy-stm32",
            );
            if async_resp.clicked() && async_ok {
                mcu.runtime = Runtime::Async;
            }
            runtime_details(
                ui,
                "rt_details_async",
                &[
                    ("Entry:", "#[embassy_executor::main] async fn main(Spawner) — the embassy executor drives \
                                the task; use .await inside the loop."),
                    ("Drivers:", "embedded-io-async (USART via BufferedUart) + embedded-hal-async (SPI/I2C). \
                                  Each SPI/I2C module has a Blocking | Async-DMA selector."),
                    ("Async-DMA:", "embassy async SPI/I2C need DMA channels the IDE can't choose → main.rs gets \
                                    a TODO line to fill (it won't compile until you set channels valid for your chip)."),
                    ("Deps:", "Adds embassy-executor + embassy-time + the HAL time-driver automatically."),
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
        });
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
                    ui,
                    &mut code,
                    font_size,
                    rows,
                    &editor_id,
                );
            });
            // Disabled widgets can't hold focus; make sure a stale `true` from
            // an earlier frame doesn't keep stealing the main editor's keys.
            self.reference_was_focused = false;
            return;
        }

        let clip = ui.clip_rect();
        let out = crate::editor::gui::code_editor::show_rust_editor_plain(
            ui,
            &mut code,
            font_size,
            rows,
            &editor_id,
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
            ".cargo/config.toml" => {
                Some((ProjectFileId::CargoConfig, self.cargo_config.clone()))
            }
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
            ui.label(egui::RichText::new(title).strong().size(14.0).color(title_col));
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
    let resp = ui.interact(inner.response.rect, egui::Id::new(("runtime_card", title)), sense);
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
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
                ui.label(
                    egui::RichText::new(*body)
                        .size(11.5)
                        .color(egui::Color32::from_gray(165)),
                );
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
