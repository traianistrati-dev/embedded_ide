//! Code-editor toolbar (header row): Copy, Build, Scan USB, and the
//! toolchain-specific Flash buttons (SWD / ESP32), plus the live status label.
//!
//! One inherent method on AppIde; reads the displayed code (for Copy) and the
//! project-files snapshot (to gate the Build/Flash buttons), and fires the
//! background build/flash operations as side effects.

use crate::app::{AppIde, BuildPanelTab, ProjectFileId};
use crate::build::{self, BuildState};
use crate::dfu::{self, DfuState};
use crate::espflash::{self, EspFlashState};
use crate::openocd::{self, OpenOcdState};
use crate::panels::mcu_module::project_gen::{self, ProjectFiles};
use eframe::egui;
use egui_phosphor::regular as ph;
use std::sync::Arc;

impl AppIde {
    /// Scan connected USB programmers (DFU / ST-Link / J-Link / CMSIS-DAP /
    /// USB-serial) and open the Flash tab. Fired from the Flash tab's Scan
    /// button (was a top-toolbar button before 2026-07-08).
    pub(crate) fn scan_usb(&mut self) {
        self.build_tab = BuildPanelTab::Dfu;
        self.dfu_sel_programmer = String::new();
        dfu::detect_dfu(
            Arc::clone(&self.dfu_state),
            Arc::clone(&self.dfu_log),
            Arc::clone(&self.dfu_programmers),
            self.egui_ctx.clone(),
        );
    }

    /// Build `--release` and flash over SWD via OpenOCD, using the selected
    /// programmer's interface/adapter. No-op without a buildable chip config.
    pub(crate) fn flash_swd(&mut self) {
        let Some((project, _tc)) = self.selected_build_cfg() else {
            return;
        };
        let (kind, vid_pid) = self
            .dfu_programmers
            .lock()
            .unwrap()
            .get(&self.dfu_sel_programmer)
            .map(|p| (p.kind.clone(), p.vid_pid.clone()))
            .unwrap_or_default();
        let interface_cfg = openocd::interface_cfg_for_kind(&kind).to_string();
        let adapter = openocd::adapter_select_cmd(&kind, &vid_pid);
        let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
        if project_gen::write_project(
            &build_dir,
            &self.current_project_files(),
            &self.project_tree.user_src_files,
            &self.mcu_config_text(),
        )
        .is_ok()
        {
            self.build_tab = BuildPanelTab::Dfu;
            openocd::start_flash(
                build_dir,
                project.target.clone(),
                project.pkg_name.clone(),
                interface_cfg,
                adapter,
                self.openocd_target_cfg.clone(),
                Arc::clone(&self.openocd_state),
                Arc::clone(&self.dfu_log),
                self.egui_ctx.clone(),
            );
        }
    }

    /// Build `--release` and flash an ESP32 via espflash, over the selected
    /// programmer's serial port. No-op without a buildable chip config.
    pub(crate) fn flash_esp(&mut self) {
        let Some((project, _tc)) = self.selected_build_cfg() else {
            return;
        };
        let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
        if project_gen::write_project(
            &build_dir,
            &self.current_project_files(),
            &self.project_tree.user_src_files,
            &self.mcu_config_text(),
        )
        .is_ok()
        {
            self.build_tab = BuildPanelTab::Dfu;
            let port = self
                .dfu_programmers
                .lock()
                .unwrap()
                .get(&self.dfu_sel_programmer)
                .map(|p| p.port.clone())
                .unwrap_or_default();
            espflash::start_flash(
                build_dir,
                project.target.clone(),
                project.probe_chip.clone(),
                port,
                Arc::clone(&self.espflash_state),
                Arc::clone(&self.dfu_log),
                self.egui_ctx.clone(),
            );
        }
    }

    /// Render the editor header toolbar.  `display_code` is the text shown in
    /// the editor (copied verbatim by the Copy button); `project_files` gates
    /// the Build button (disabled when the chip has no project config).
    pub(super) fn show_editor_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        display_code: &str,
        project_files: &Option<ProjectFiles>,
    ) {
        ui.horizontal(|ui| {
            ui.heading("Code Editor");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Copy button — copies the currently displayed file
                let copy_ok = format!("{} Copied!", ph::CHECK);
                let copy_def = format!("{} Copy", ph::COPY);
                let copy_label: &str = if self.copy_flash > 0 {
                    &copy_ok
                } else {
                    &copy_def
                };
                let copy_btn = ui.add(egui::Button::new(
                    egui::RichText::new(copy_label).size(11.0),
                ));
                if copy_btn.clicked() {
                    ui.output_mut(|o| {
                        o.commands.push(egui::output::OutputCommand::CopyText(
                            display_code.to_owned(),
                        ));
                    });
                    self.copy_flash = 60;
                }

                ui.add_space(4.0);

                // ── Serial monitor button — opens the bottom "Serial" tab ──
                let serial_on = self.serial.is_connected();
                let serial_btn = ui.add(egui::Button::new(
                    egui::RichText::new(format!("{} Serial", ph::TERMINAL))
                        .size(11.0)
                        .color(if serial_on {
                            egui::Color32::from_rgb(80, 200, 100)
                        } else {
                            egui::Color32::from_rgb(150, 180, 220)
                        }),
                ));
                if serial_btn.clicked() {
                    self.build_tab = BuildPanelTab::Serial;
                    self.serial.refresh_ports();
                    // Seed the baud from the first GI_USART module (when idle).
                    let module_baud = self.mcu.as_ref().and_then(|mcu| {
                        mcu.modules.iter().find_map(|m| match &m.config {
                            crate::panels::mcu_module::modules::ModuleConfig::Usart(c) => {
                                Some(c.baud_rate)
                            }
                            _ => None,
                        })
                    });
                    if !serial_on {
                        if let Some(b) = module_baud {
                            self.serial.baud = b;
                        }
                    }
                }
                serial_btn.on_hover_text("Open the serial monitor (USART/UART console)");

                ui.add_space(4.0);

                // ── Terminal button — opens the bottom "Terminal" tab ──
                let term_on = self.terminal.is_running();
                let term_btn = ui.add(egui::Button::new(
                    egui::RichText::new(format!("{} Terminal", ph::TERMINAL_WINDOW))
                        .size(11.0)
                        .color(if term_on {
                            egui::Color32::from_rgb(220, 180, 60)
                        } else {
                            egui::Color32::from_rgb(150, 180, 210)
                        }),
                ));
                if term_btn.clicked() {
                    self.build_tab = BuildPanelTab::Terminal;
                }
                term_btn.on_hover_text(
                    "Open the terminal — run any command (PowerShell) in the project \
                     workspace, with live output",
                );

                ui.add_space(4.0);

                // ── Activity button — opens the bottom "Activity" (timing) tab ──
                let activity_btn = ui.add(egui::Button::new(
                    egui::RichText::new(format!("{} Activity", ph::TIMER))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(160, 185, 215)),
                ));
                if activity_btn.clicked() {
                    self.build_tab = BuildPanelTab::Activity;
                }
                activity_btn.on_hover_text(
                    "Open the Activity tab — per-Save/Build/Flash timing breakdown \
                     (see where the time goes)",
                );

                ui.add_space(4.0);

                // ── Clippy button — opens the bottom "Clippy" tab ──
                let clippy_running = self.clippy_state.lock().unwrap().is_building();
                let clippy_btn = ui.add(egui::Button::new(
                    egui::RichText::new(format!("{} Clippy", ph::SPARKLE))
                        .size(11.0)
                        .color(if clippy_running {
                            egui::Color32::from_rgb(180, 180, 180)
                        } else {
                            egui::Color32::from_rgb(150, 200, 120)
                        }),
                ));
                if clippy_btn.clicked() {
                    self.build_tab = BuildPanelTab::Clippy;
                }
                clippy_btn.on_hover_text("Open the Clippy tab (code-improvement suggestions)");

                ui.add_space(4.0);

                // ── Build button ──────────────────────────────────────
                let build_guard = self.build_state.lock().unwrap();
                let is_building = build_guard.is_building();

                // Animate trailing dots while building
                let build_label = if is_building {
                    let dots = match (ui.ctx().cumulative_frame_nr() / 15) % 3 {
                        0 => ".",
                        1 => "..",
                        _ => "...",
                    };
                    format!("Building{dots}")
                } else {
                    format!("{} Build", ph::HAMMER)
                };

                // Badge: error/warning/ok count shown to the left of the button
                let badge_text = match &*build_guard {
                    BuildState::Done(r) if r.error_count() > 0 => Some((
                        format!("{} {}", r.error_count(), ph::X_CIRCLE),
                        egui::Color32::from_rgb(230, 90, 80),
                    )),
                    BuildState::Done(r) if r.warning_count() > 0 => Some((
                        format!("{} {}", r.warning_count(), ph::WARNING),
                        egui::Color32::from_rgb(230, 190, 50),
                    )),
                    BuildState::Done(r) if r.success => Some((
                        format!("{}", ph::CHECK_CIRCLE),
                        egui::Color32::from_rgb(80, 200, 100),
                    )),
                    BuildState::Failed(_) => Some((
                        format!("{}", ph::X_CIRCLE),
                        egui::Color32::from_rgb(230, 90, 80),
                    )),
                    _ => None,
                };
                drop(build_guard);

                if let Some((badge, color)) = badge_text {
                    ui.label(egui::RichText::new(badge).size(11.0).color(color));
                }

                // Serialized with Clippy: don't allow a Build while clippy runs
                // (they share the same target/ directory).
                let clippy_running = self.clippy_state.lock().unwrap().is_building();
                let build_enabled = !is_building && !clippy_running && project_files.is_some();
                let build_btn = ui.add_enabled(
                    build_enabled,
                    egui::Button::new(egui::RichText::new(&build_label).size(11.0).color(
                        if build_enabled {
                            egui::Color32::from_rgb(100, 220, 100)
                        } else {
                            egui::Color32::GRAY
                        },
                    )),
                );

                if build_btn.clicked() {
                    if let Some((project, _toolchain)) = self.selected_build_cfg() {
                        let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
                        match project_gen::write_project(
                            &build_dir,
                            &self.current_project_files(),
                            &self.project_tree.user_src_files,
                            &self.mcu_config_text(),
                        ) {
                            Ok(()) => {
                                self.selected_diagnostic = None;
                                self.build_tab = BuildPanelTab::Cargo;
                                // Snapshot the compiled text so the "unused local
                                // variable" fade can tell later whether this run's
                                // diagnostics still match the live file.
                                self.snapshot_build_text();
                                build::start_build(
                                    build_dir,
                                    project.target.clone(),
                                    Arc::clone(&self.build_state),
                                    self.egui_ctx.clone(),
                                    Arc::clone(&self.activity),
                                );
                            }
                            Err(e) => {
                                *self.build_state.lock().unwrap() = BuildState::Failed(format!(
                                    "Could not write project to temp dir: {e}"
                                ));
                            }
                        }
                    }
                }
                build_btn.on_hover_text(
                    "Run `cargo check` on the generated project.\n\
                     Requires the Rust toolchain + thumbv7m-none-eabi target:\n\
                     rustup target add thumbv7m-none-eabi",
                );

                // Keep UI refreshing while build is running (drives dot animation)
                if is_building {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(120));
                }

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // ── USB DFU + SWD section ─────────────────────────────
                let dfu_guard = self.dfu_state.lock().unwrap();
                let dfu_busy = dfu_guard.is_busy();
                let dfu_label = dfu_guard.status_label().to_string();
                let dfu_color = dfu_guard.status_color();
                let dfu_detail = dfu_guard.detail();
                _ = matches!(*dfu_guard, DfuState::DeviceFound(_));
                drop(dfu_guard);

                let ocd_busy = self.openocd_state.lock().unwrap().is_busy();
                let esp_busy = self.espflash_state.lock().unwrap().is_busy();
                let any_busy = dfu_busy || ocd_busy || esp_busy;

                // Keep UI refreshing while any flash operation is running.
                // (Scan USB + Flash SWD/ESP32 buttons moved to the Flash tab's
                // Programmer row on 2026-07-08 — see `dfu_tab.rs`.)
                if any_busy {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(120));
                }

                ui.add_space(4.0);

                // Status label — shows the most active state
                let (show_label, show_color, show_detail) = {
                    let ocd = self.openocd_state.lock().unwrap();
                    let esp = self.espflash_state.lock().unwrap();
                    if !matches!(*ocd, OpenOcdState::Idle) {
                        (ocd.status_label().to_string(), ocd.status_color(), None)
                    } else if !matches!(*esp, EspFlashState::Idle) {
                        (esp.status_label().to_string(), esp.status_color(), None)
                    } else {
                        (dfu_label.clone(), dfu_color, dfu_detail)
                    }
                };
                let status_widget = ui.label(
                    egui::RichText::new(&show_label)
                        .size(10.5)
                        .color(show_color),
                );
                if let Some(detail) = show_detail {
                    status_widget.on_hover_text(detail);
                }

                ui.add_space(8.0);
                // Show which file is open
                let open_label = match self.selected_file {
                    ProjectFileId::UserFile(i) => self
                        .project_tree
                        .user_src_files
                        .get(i)
                        .map(|(name, _)| format!("src/{name}"))
                        .unwrap_or_else(|| "src/???".to_string()),
                    other => other.label().to_string(),
                };
                ui.label(
                    egui::RichText::new(&open_label)
                        .size(10.0)
                        .color(egui::Color32::from_rgb(120, 160, 200)),
                );
            });
        });
    }
}
