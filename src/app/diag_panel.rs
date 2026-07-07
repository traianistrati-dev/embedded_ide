//! Bottom diagnostics panel — tabbed: Cargo Check | rust-analyzer | Flash | Tools.
//!
//! Orchestrator only: renders the tab-header buttons (with status badges) and
//! dispatches to the per-tab render functions in `super::tabs`.

use super::tabs::{
    show_activity_tab, show_cargo_tab, show_clippy_tab, show_dfu_tab, show_git_tab, show_ra_tab,
    show_serial_tab, show_terminal_tab, show_tools_tab,
};
use super::BuildPanelTab;
use crate::activity::ActivityLog;
use crate::build::BuildState;
use crate::terminal::TerminalConsole;
use crate::dfu::{self, DfuState};
use crate::espflash::EspFlashState;
use crate::lsp::{self, LspStatus};
use crate::openocd::OpenOcdState;
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use crate::required_tools;
use crate::serial::SerialMonitor;
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
    serial: &mut SerialMonitor,
    terminal: &mut TerminalConsole,
    activity: &Arc<Mutex<ActivityLog>>,
    clippy_state: &Arc<Mutex<BuildState>>,
    clippy_sel: &mut Option<usize>,
    // Set true when the user presses "Run clippy" (the caller starts the run).
    clippy_run: &mut bool,
    // Set to `Some(i)` to apply diagnostic `i`'s fix; `clippy_apply_all` applies
    // every machine-applicable suggestion. The caller performs the edits.
    clippy_apply_one: &mut Option<usize>,
    clippy_apply_all: &mut bool,
    // Set to `Some(i)` to project-wide-rename diagnostic `i`'s symbol (RA rename).
    clippy_apply_rename: &mut Option<usize>,
    // Byte ranges of main.rs's GENERATED block — "Fix" is disabled for fixes there.
    clippy_gen_ranges: &[(usize, usize)],
    toolchain: &ToolchainKind,
    tab: &mut BuildPanelTab,
    cargo_sel: &mut Option<usize>,
    lsp_sel: &mut Option<usize>,
    // Diagnostic-row click target: `(rel_path, 1-based line, band colour)`; the
    // editor opens the file, scrolls to the line, and tints it.
    nav: &mut Option<(String, usize, egui::Color32)>,
    // `definition`: the F12 snippet (header, code, highlight-line-index); the
    // "Definition" tab is shown only when this is Some. `definition_close`: set
    // true when the user closes it.
    definition: Option<(&str, &str, usize)>,
    definition_close: &mut bool,
    // One-shot flag: scroll the Definition view to the highlighted line on the
    // first render after a new F12 snippet loads.
    def_scroll_pending: &mut bool,
    // Git tab: console state + the saved project dir; buttons set `git_op`
    // (the `clippy_run` signal pattern — the caller spawns the worker).
    git: &mut crate::git::GitConsole,
    project_dir: Option<&std::path::Path>,
    git_op: &mut Option<crate::git::GitOp>,
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

        // Clippy tab button
        {
            let active = *tab == BuildPanelTab::Clippy;
            let cs = clippy_state.lock().unwrap();
            let (badge, col) = match &*cs {
                BuildState::Building => (" …".to_owned(), egui::Color32::GRAY),
                BuildState::Done(r) if !r.diagnostics.is_empty() => (
                    format!(" {} {}", r.diagnostics.len(), ph::LIGHTBULB),
                    egui::Color32::from_rgb(210, 170, 40),
                ),
                BuildState::Done(_) => (
                    format!(" {}", ph::CHECK_CIRCLE),
                    egui::Color32::from_rgb(80, 200, 100),
                ),
                BuildState::Failed(_) => (
                    format!(" {}", ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                ),
                BuildState::Idle => (String::new(), egui::Color32::DARK_GRAY),
            };
            drop(cs);
            let label = format!("{} Clippy{badge}", ph::SPARKLE);
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Clippy;
            }
        }

        ui.separator();

        // Terminal tab button (streaming command console).
        {
            let active = *tab == BuildPanelTab::Terminal;
            let running = terminal.is_running();
            let badge = if running { " …".to_owned() } else { String::new() };
            let col = if running {
                egui::Color32::GRAY
            } else {
                egui::Color32::from_rgb(150, 180, 210)
            };
            let label = format!("{} Terminal{badge}", ph::TERMINAL_WINDOW);
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Terminal;
            }
        }

        ui.separator();

        // Activity tab button (timing breakdown).
        {
            let active = *tab == BuildPanelTab::Activity;
            let n = activity.lock().unwrap().actions.len();
            let badge = if n > 0 { format!(" {n}") } else { String::new() };
            let label = format!("{} Activity{badge}", ph::TIMER);
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::from_rgb(160, 185, 215)
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Activity;
            }
        }

        ui.separator();

        // Git tab button (commit/push/pull in the project directory).
        {
            let active = *tab == BuildPanelTab::Git;
            let (busy, n) = {
                let st = git.state.lock().unwrap();
                (st.busy.is_some(), st.status.changes.len())
            };
            let badge = if busy {
                " …".to_owned()
            } else if n > 0 {
                format!(" {n}")
            } else {
                String::new()
            };
            let col = if busy {
                egui::Color32::from_rgb(220, 180, 70)
            } else {
                egui::Color32::GRAY
            };
            let btn = ui.add(
                egui::Button::new(
                    egui::RichText::new(format!("{} Git{badge}", ph::GIT_BRANCH))
                        .size(11.5)
                        .color(col),
                )
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Git;
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

        // Serial monitor tab button
        {
            let active = *tab == BuildPanelTab::Serial;
            let (badge, col) = if serial.is_connected() {
                (
                    format!(" {}", ph::PLUGS_CONNECTED),
                    egui::Color32::from_rgb(80, 200, 100),
                )
            } else {
                (String::new(), egui::Color32::DARK_GRAY)
            };
            let label = format!("{} Serial{badge}", ph::TERMINAL);
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Serial;
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
            show_cargo_tab(ui, ctx, build_state, cargo_sel, nav);
        }
        BuildPanelTab::RustAnalyzer => {
            show_ra_tab(ui, lsp_state, lsp_sel, nav);
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
        BuildPanelTab::Serial => {
            show_serial_tab(ui, serial, ctx);
        }
        BuildPanelTab::Terminal => {
            show_terminal_tab(ui, terminal, ctx);
        }
        BuildPanelTab::Activity => {
            show_activity_tab(ui, activity);
        }
        BuildPanelTab::Git => {
            show_git_tab(ui, git, project_dir, git_op);
        }
        BuildPanelTab::Clippy => {
            let build_busy = build_state.lock().unwrap().is_building();
            show_clippy_tab(
                ui,
                clippy_state,
                build_busy,
                clippy_sel,
                nav,
                clippy_run,
                clippy_apply_one,
                clippy_apply_all,
                clippy_apply_rename,
                clippy_gen_ranges,
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
                // The whole file is shown (so the user can scroll above and
                // below the target). Rows are virtualized (`show_rows` renders
                // only the visible ones), and the target line is scrolled near
                // the top once on open. The def line is drawn coloured so it
                // stands out from the surrounding (white) code.
                let lines: Vec<&str> = code.lines().collect();
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
                // Match the spacing show_rows will use, so its offset math lines
                // up with the rendered rows.
                ui.spacing_mut().item_spacing.y = 1.0;
                let pitch = row_h + ui.spacing().item_spacing.y;
                let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
                if *def_scroll_pending {
                    // Target near the top (2 lines of context above), then free.
                    let off = highlight.saturating_sub(2) as f32 * pitch;
                    area = area.vertical_scroll_offset(off);
                    *def_scroll_pending = false;
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
