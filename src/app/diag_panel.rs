//! Bottom diagnostics panel — tabbed: Cargo Check | rust-analyzer | Flash | Tools.
//!
//! Orchestrator only: renders the tab-header buttons (with status badges) and
//! dispatches to the per-tab render functions in `super::tabs`.

use super::BuildPanelTab;
use super::tabs::{
    show_activity_tab, show_cargo_tab, show_clippy_tab, show_debug_tab, show_dfu_tab, show_git_tab,
    show_ra_tab, show_rtt_tab, show_serial_tab, show_terminal_tab, show_tools_tab,
};
use crate::activity::ActivityLog;
use crate::build::BuildState;
use crate::dfu::{self, DfuState};
use crate::espflash::EspFlashState;
use crate::lsp::{self, LspStatus};
use crate::openocd::OpenOcdState;
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use crate::required_tools;
use crate::serial::SerialMonitor;
use crate::terminal::TerminalConsole;
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
    // Panel reduced to this tab bar: the header still renders, the content
    // below it doesn't. Toggled by the caret button right of "More".
    collapsed: &mut bool,
    // Set when ANY tab button is clicked — including one that was already
    // selected, so the caller can reopen a collapsed panel on any click.
    tab_clicked: &mut bool,
    cargo_sel: &mut Option<usize>,
    lsp_sel: &mut Option<usize>,
    // Diagnostic-row click target: `(rel_path, 1-based line, band colour)`; the
    // editor opens the file, scrolls to the line, and tints it.
    nav: &mut Option<(String, usize, egui::Color32)>,
    // Git tab: console state + the saved project dir; buttons set `git_op`
    // (the `clippy_run` signal pattern — the caller spawns the worker).
    git: &mut crate::git::GitConsole,
    project_dir: Option<&std::path::Path>,
    git_op: &mut Option<crate::git::GitOp>,
    // `(git path, 1-based line)` of an added diff row the user clicked to open
    // in the editor; the caller maps the path to a `ProjectFileId`, selects it
    // and scrolls to the line.
    git_open: &mut Option<(String, usize)>,
    // `(git path, hunk row index)` when the user clicks a hunk's revert button
    // in the diff view; the caller reverses just that hunk (Phase B).
    git_revert_hunk: &mut Option<(String, usize)>,
    // Git History view (read-only): a selected commit / one of its files.
    git_commit_load: &mut Option<String>,
    git_commit_file_load: &mut Option<(String, String)>,
    // `(git path, is_untracked)` when the user clicks a file's discard button;
    // the caller confirms then restores/deletes the whole file (Phase A).
    git_discard: &mut Option<(String, bool)>,
    // True when the user clicks "Discard all"; the caller confirms then resets
    // the whole tree to HEAD (Phase C).
    git_discard_all: &mut bool,
    // Flash-tab Programmer-row buttons: set `flash_scan`/`flash_go` on click;
    // `can_flash` = a buildable chip config exists (gates the Flash button).
    flash_scan: &mut bool,
    flash_go: &mut bool,
    can_flash: bool,
    // Cargo-tab Build button (moved off the top toolbar): set on click; the
    // caller runs `start_build`. Gated like Flash, on the same chip config.
    build_go: &mut bool,
    // Cargo-tab Size button: Flash/RAM usage measurement (state + signal; the
    // caller runs `start_size_measure`).
    size_state: &Arc<Mutex<crate::size::SizeState>>,
    size_go: &mut bool,
    // Flash-tab Size button — same measurement, but the caller keeps the Flash
    // tab in front instead of switching to Cargo.
    size_flash_go: &mut bool,
    // RTT tab: console + Run/Attach signal (caller runs `start_rtt`) + the
    // probe-rs chip name shown in the tab.
    rtt: &mut crate::rtt::RttConsole,
    rtt_go: &mut Option<crate::rtt::RttMode>,
    rtt_chip: &str,
    // Debug tab: session + Start signal (caller runs `start_debug`).
    debugger: &mut crate::debugger::Debugger,
    debug_go: &mut bool,
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
                *tab_clicked = true;
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
            let label = format!("Analyzer{badge}");
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
                *tab_clicked = true;
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
                *tab_clicked = true;
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
                *tab_clicked = true;
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
                *tab_clicked = true;
            }
        }

        // RTT / defmt tab button (streaming badge while a session is live).
        {
            let active = *tab == BuildPanelTab::Rtt;
            let (badge, col) = match rtt.phase() {
                crate::rtt::RttPhase::Streaming => (
                    format!(" {}", ph::BROADCAST),
                    egui::Color32::from_rgb(80, 200, 100),
                ),
                crate::rtt::RttPhase::Building => (" …".to_owned(), egui::Color32::GRAY),
                crate::rtt::RttPhase::Error(_) => (
                    format!(" {}", ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                ),
                crate::rtt::RttPhase::Idle => (String::new(), egui::Color32::DARK_GRAY),
            };
            let label = format!("{} RTT{badge}", ph::BROADCAST);
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Rtt;
                *tab_clicked = true;
            }
        }

        // Debug tab button (state badge while a session is live).
        {
            let active = *tab == BuildPanelTab::Debug;
            use crate::debugger::DebugPhase;
            let (badge, col) = match debugger.phase() {
                DebugPhase::Stopped(_) => (
                    format!(" {}", ph::PAUSE),
                    egui::Color32::from_rgb(230, 180, 60),
                ),
                DebugPhase::Running => (
                    format!(" {}", ph::PLAY),
                    egui::Color32::from_rgb(80, 200, 100),
                ),
                DebugPhase::Building | DebugPhase::Launching => {
                    (" …".to_owned(), egui::Color32::GRAY)
                }
                DebugPhase::Error(_) => (
                    format!(" {}", ph::X_CIRCLE),
                    egui::Color32::from_rgb(220, 80, 70),
                ),
                DebugPhase::Idle => (String::new(), egui::Color32::DARK_GRAY),
            };
            let label = format!("{} Debug{badge}", ph::BUG);
            let btn = ui.add(
                egui::Button::new(egui::RichText::new(&label).size(11.0).color(if active {
                    egui::Color32::WHITE
                } else {
                    col
                }))
                .frame(active),
            );
            if btn.clicked() {
                *tab = BuildPanelTab::Debug;
                *tab_clicked = true;
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
                *tab_clicked = true;
            }
        }

        // (The F12 "Definition" tab moved to the MCU Configurator on
        // 2026-07-10 — see `AppIde::show_definition_tab`.)

        // ── "More" dropdown (right-aligned) — groups the auxiliary panels
        //    Terminal / Activity / Tools so the main tab bar stays compact. ──
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Collapse / expand toggle. Added FIRST because this layout is
            // right-to-left — the first widget sits furthest right, i.e. to the
            // right of the "More" dropdown below.
            let (icon, tip) = if *collapsed {
                (
                    ph::CARET_DOUBLE_UP,
                    "Expand the panel — show the selected tab's content again.\n\
                     Clicking any tab above expands it too.",
                )
            } else {
                (
                    ph::CARET_DOWN,
                    "Collapse the panel — keep only this tab bar and give the \
                     space back to the editor.\n\
                     The bar stays visible; click any tab to reopen.",
                )
            };
            if ui
                .button(
                    egui::RichText::new(icon)
                        .size(12.0)
                        .color(egui::Color32::from_rgb(160, 185, 215)),
                )
                .on_hover_text(tip)
                .clicked()
            {
                *collapsed = !*collapsed;
            }

            let grouped = matches!(
                *tab,
                BuildPanelTab::Terminal | BuildPanelTab::Activity | BuildPanelTab::RequiredTools
            );
            let running = terminal.is_running();
            let acts = activity.lock().unwrap().actions.len();
            let (missing, tools_busy) = {
                let ts = tools_state.lock().unwrap();
                (ts.missing_installable_count(), ts.any_busy())
            };
            // Label shows the active grouped tab (so it's obvious which is
            // selected), else "More"; a caret hints at the dropdown.
            let name = match *tab {
                BuildPanelTab::Terminal => "Terminal",
                BuildPanelTab::Activity => "Activity",
                BuildPanelTab::RequiredTools => "Tools",
                _ => "More",
            };
            // A badge when a grouped panel wants attention while NOT selected.
            let attention = (running && *tab != BuildPanelTab::Terminal)
                || (missing > 0 && *tab != BuildPanelTab::RequiredTools);
            let col = if grouped {
                egui::Color32::WHITE
            } else if attention {
                egui::Color32::from_rgb(230, 160, 50)
            } else {
                egui::Color32::from_rgb(160, 185, 215)
            };
            let hint = if running {
                " …".to_owned()
            } else if missing > 0 {
                format!(" {missing} {}", ph::WARNING)
            } else {
                String::new()
            };
            ui.menu_button(
                egui::RichText::new(format!("{name}{hint} {}", ph::CARET_DOWN))
                    .size(11.0)
                    .color(col),
                |ui| {
                    let term_badge = if running { " …" } else { "" };
                    if ui
                        .selectable_label(
                            *tab == BuildPanelTab::Terminal,
                            format!("{} Terminal{term_badge}", ph::TERMINAL_WINDOW),
                        )
                        .clicked()
                    {
                        *tab = BuildPanelTab::Terminal;
                        *tab_clicked = true;
                        ui.close();
                    }
                    let act_badge = if acts > 0 {
                        format!(" {acts}")
                    } else {
                        String::new()
                    };
                    if ui
                        .selectable_label(
                            *tab == BuildPanelTab::Activity,
                            format!("{} Activity{act_badge}", ph::TIMER),
                        )
                        .clicked()
                    {
                        *tab = BuildPanelTab::Activity;
                        *tab_clicked = true;
                        ui.close();
                    }
                    let tool_badge = if tools_busy {
                        " …".to_owned()
                    } else if missing > 0 {
                        format!(" {missing} {}", ph::WARNING)
                    } else {
                        String::new()
                    };
                    if ui
                        .selectable_label(
                            *tab == BuildPanelTab::RequiredTools,
                            format!("{} Tools{tool_badge}", ph::WRENCH),
                        )
                        .clicked()
                    {
                        *tab = BuildPanelTab::RequiredTools;
                        *tab_clicked = true;
                        ui.close();
                    }
                },
            );
        });
    });

    // Collapsed: the tab bar above is the whole panel — stop before the
    // content. (The caller sizes the panel to just this row.)
    if *collapsed {
        return;
    }

    ui.separator();

    // ── Tab content ───────────────────────────────────────────────────────────
    match tab {
        BuildPanelTab::Cargo => {
            let clippy_running = clippy_state.lock().unwrap().is_building();
            show_cargo_tab(
                ui,
                ctx,
                build_state,
                cargo_sel,
                nav,
                build_go,
                can_flash, // same gate: a buildable chip config exists
                clippy_running,
                size_state,
                size_go,
            );
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
                flash_scan,
                flash_go,
                can_flash,
                size_state,
                size_flash_go,
            );
        }
        BuildPanelTab::Rtt => {
            show_rtt_tab(ui, rtt, rtt_go, can_flash, rtt_chip);
        }
        BuildPanelTab::Debug => {
            show_debug_tab(ui, debugger, debug_go, can_flash, rtt_chip);
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
            show_git_tab(
                ui,
                git,
                project_dir,
                git_op,
                git_open,
                git_revert_hunk,
                git_commit_load,
                git_commit_file_load,
                git_discard,
                git_discard_all,
            );
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
    }
}
