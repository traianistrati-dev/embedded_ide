//! Bottom diagnostics panel — tabbed: Cargo Check | rust-analyzer | Flash | Tools.
//!
//! Orchestrator only: renders the tab-header buttons (with status badges) and
//! dispatches to the per-tab render functions in `super::tabs`.

use super::tabs::{show_cargo_tab, show_dfu_tab, show_ra_tab, show_tools_tab};
use super::{BuildPanelTab, ProjectFileId};
use crate::build::BuildState;
use crate::dfu::{self, DfuState};
use crate::espflash::EspFlashState;
use crate::lsp::{self, LspStatus};
use crate::openocd::OpenOcdState;
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use crate::required_tools;
use eframe::egui;
use egui_phosphor::regular as ph;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub(super) fn show_diag_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    build_state: &Arc<Mutex<BuildState>>,
    lsp_state: &Arc<Mutex<lsp::LspState>>,
    dfu_state: &Arc<Mutex<DfuState>>,
    dfu_log: &Arc<Mutex<Vec<String>>>,
    dfu_programmers: &Arc<Mutex<HashMap<String, dfu::ProgrammerInfo>>>,
    dfu_sel_programmer: &mut String,
    dfu_flash_addr: &mut String,
    openocd_state: &Arc<Mutex<OpenOcdState>>,
    openocd_target_cfg: &mut String,
    espflash_state: &Arc<Mutex<EspFlashState>>,
    espflash_port: &mut String,
    tools_state: &Arc<Mutex<required_tools::ToolsState>>,
    toolchain: &ToolchainKind,
    tab: &mut BuildPanelTab,
    cargo_sel: &mut Option<usize>,
    lsp_sel: &mut Option<usize>,
    selected_file: &mut ProjectFileId,
    // `definition`: the F12 snippet (header, code, highlight-line-index); the
    // "Definition" tab is shown only when this is Some. `definition_close`: set
    // true when the user closes it.
    definition: Option<(&str, &str, usize)>,
    definition_close: &mut bool,
) {
    // ── Tab header ────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        // Cargo tab button
        {
            let st = build_state.lock().unwrap();
            let (badge, col) = match &*st {
                BuildState::Done(r) if r.error_count() > 0 => (
                    format!(" {} {}", r.error_count(), ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                ),
                BuildState::Done(r) if r.warning_count() > 0 => (
                    format!(" {} {}", r.warning_count(), ph::WARNING),
                    egui::Color32::from_rgb(210, 170, 40),
                ),
                BuildState::Done(r) if r.success => (
                    format!(" {}", ph::CHECK_CIRCLE),
                    egui::Color32::from_rgb(80, 200, 100),
                ),
                BuildState::Building => (" …".to_owned(), egui::Color32::GRAY),
                _ => (String::new(), egui::Color32::GRAY),
            };
            let label = format!("{} Cargo Check{badge}", ph::HAMMER);
            let active = *tab == BuildPanelTab::Cargo;
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Cargo;
            }
        }

        ui.separator();

        // RA tab button
        {
            let lsp = lsp_state.lock().unwrap();
            let (badge, col) = match &lsp.status {
                LspStatus::Starting | LspStatus::Indexing => {
                    (" …".to_owned(), egui::Color32::from_rgb(180, 180, 80))
                }
                LspStatus::Ready if lsp.total_errors() > 0 => (
                    format!(" {} {}", lsp.total_errors(), ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                ),
                LspStatus::Ready if lsp.total_warnings() > 0 => (
                    format!(" {} {}", lsp.total_warnings(), ph::WARNING),
                    egui::Color32::from_rgb(210, 170, 40),
                ),
                LspStatus::Ready => (
                    format!(" {}", ph::CHECK_CIRCLE),
                    egui::Color32::from_rgb(80, 200, 100),
                ),
                LspStatus::Failed(_) => (
                    format!(" {}", ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                ),
                _ => (String::new(), egui::Color32::DARK_GRAY),
            };
            let label = format!("rust-analyzer{badge}");
            let active = *tab == BuildPanelTab::RustAnalyzer;
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::RustAnalyzer;
            }
        }

        ui.separator();

        // Flash tab button — badge reflects whichever flash operation is active
        {
            let dfu = dfu_state.lock().unwrap();
            let ocd = openocd_state.lock().unwrap();
            let esp = espflash_state.lock().unwrap();
            let any_busy = dfu.is_busy() || ocd.is_busy() || esp.is_busy();
            let any_success = matches!(*dfu, DfuState::Success)
                || matches!(*ocd, OpenOcdState::Success)
                || matches!(*esp, EspFlashState::Success);
            let any_error = matches!(*dfu, DfuState::Error(_))
                || matches!(*ocd, OpenOcdState::Error(_))
                || matches!(*esp, EspFlashState::Error(_));
            let (badge, col) = if any_busy {
                if matches!(*dfu, DfuState::Flashing)
                    || matches!(*ocd, OpenOcdState::Flashing)
                    || matches!(*esp, EspFlashState::Flashing)
                {
                    (" …".to_owned(), egui::Color32::from_rgb(100, 180, 255))
                } else {
                    (" …".to_owned(), egui::Color32::from_rgb(220, 180, 60))
                }
            } else if any_success {
                (
                    format!(" {}", ph::CHECK_CIRCLE),
                    egui::Color32::from_rgb(80, 200, 100),
                )
            } else if any_error {
                (
                    format!(" {}", ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                )
            } else {
                (String::new(), egui::Color32::DARK_GRAY)
            };
            drop(esp);
            drop(ocd);
            drop(dfu);
            let label = format!("{} Flash{badge}", ph::LIGHTNING);
            let active = *tab == BuildPanelTab::Dfu;
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Dfu;
            }
        }

        ui.separator();

        // Required Tools tab button
        {
            let ts = tools_state.lock().unwrap();
            let missing = ts.missing_installable_count();
            let any_busy = ts.any_busy();
            drop(ts);
            let (badge, col) = if any_busy {
                (" …".to_owned(), egui::Color32::from_rgb(180, 180, 80))
            } else if missing > 0 {
                (
                    format!(" {} {}", missing, ph::WARNING),
                    egui::Color32::from_rgb(230, 160, 50),
                )
            } else {
                (String::new(), egui::Color32::DARK_GRAY)
            };
            let label = format!("{} Tools{badge}", ph::WRENCH);
            let active = *tab == BuildPanelTab::RequiredTools;
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::RequiredTools;
            }
        }

        // Definition tab (F12) — only present while there is a definition to show.
        if definition.is_some() {
            ui.separator();
            let active = *tab == BuildPanelTab::Definition;
            let btn = ui.add(
                egui::Button::new(egui::RichText::new("Definition").size(11.0).color(
                    if active {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(120, 180, 240)
                    },
                ))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Definition;
            }
            if ui
                .add(egui::Button::new(egui::RichText::new(ph::X).size(10.0)).frame(false))
                .on_hover_text("Close definition")
                .clicked()
            {
                *definition_close = true;
            }
        }
    });

    ui.separator();

    // ── Tab content ───────────────────────────────────────────────────────────
    match tab {
        BuildPanelTab::Cargo => {
            show_cargo_tab(ui, ctx, build_state, cargo_sel, selected_file);
        }
        BuildPanelTab::RustAnalyzer => {
            show_ra_tab(ui, lsp_state, lsp_sel, selected_file);
        }
        BuildPanelTab::Dfu => {
            show_dfu_tab(
                ui,
                dfu_state,
                dfu_log,
                dfu_programmers,
                dfu_sel_programmer,
                dfu_flash_addr,
                openocd_state,
                openocd_target_cfg,
                espflash_state,
                espflash_port,
                toolchain,
            );
        }
        BuildPanelTab::RequiredTools => {
            show_tools_tab(ui, tools_state, ctx);
        }
        BuildPanelTab::Definition => {
            if let Some((header, code, highlight)) = definition {
                ui.label(
                    egui::RichText::new(header)
                        .size(11.0)
                        .monospace()
                        .color(egui::Color32::from_rgb(150, 190, 240)),
                );
                ui.separator();
                // The definition line is drawn coloured with a subtle highlight
                // band so it stands out from the surrounding (white) code.
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for (i, line) in code.lines().enumerate() {
                        let shown = if line.is_empty() { " " } else { line };
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
                                egui::Label::new(
                                    egui::RichText::new(shown).monospace().size(12.0),
                                )
                                .selectable(true),
                            );
                        }
                    }
                });
            } else {
                ui.label(
                    egui::RichText::new("No definition.")
                        .color(egui::Color32::GRAY),
                );
            }
        }
    }
}
