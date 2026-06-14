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
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use crate::panels::mcu_module::project_gen::{self, ProjectFiles};
use eframe::egui;
use egui_phosphor::regular as ph;
use std::sync::Arc;

impl AppIde {
    /// Render the editor header toolbar.  `display_code` is the text shown in
    /// the editor (copied verbatim by the Copy button); `project_files` gates
    /// the Build/Flash buttons (disabled when the chip has no project config).
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

                let build_enabled = !is_building && project_files.is_some();
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
                        ) {
                            Ok(()) => {
                                self.selected_diagnostic = None;
                                self.build_tab = BuildPanelTab::Cargo;
                                build::start_build(
                                    build_dir,
                                    project.target.clone(),
                                    Arc::clone(&self.build_state),
                                    self.egui_ctx.clone(),
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

                // Determine toolchain of the selected chip (SdccC = no flash UI).
                let chip_toolchain = self.selected_toolchain().unwrap_or(ToolchainKind::SdccC);

                // Determine if the selected programmer supports SWD flashing
                let (is_swd_capable, sel_interface_cfg, sel_adapter) = {
                    let progs = self.dfu_programmers.lock().unwrap();
                    let (kind, vid_pid) = progs
                        .get(&self.dfu_sel_programmer)
                        .map(|p| (p.kind.clone(), p.vid_pid.clone()))
                        .unwrap_or_default();
                    let swd = matches!(kind.as_str(), "ST-Link" | "J-Link" | "CMSIS-DAP");
                    let cfg = openocd::interface_cfg_for_kind(&kind).to_string();
                    // Pin OpenOCD to the selected probe so the right one is used
                    // when several ST-Links are connected.
                    let adapter = openocd::adapter_select_cmd(&kind, &vid_pid);
                    (swd, cfg, adapter)
                };

                // Keep UI refreshing while any flash operation is running
                if any_busy {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(120));
                }

                // 🔍 Scan button — always visible (detects DFU, ST-Link, and serial)
                let scan_btn = ui.add_enabled(
                    !dfu_busy,
                    egui::Button::new(
                        egui::RichText::new(format!("{} Scan USB", ph::MAGNIFYING_GLASS))
                            .size(11.0),
                    ),
                );
                if scan_btn.clicked() {
                    self.build_tab = BuildPanelTab::Dfu;
                    self.dfu_sel_programmer = String::new();
                    dfu::detect_dfu(
                        Arc::clone(&self.dfu_state),
                        Arc::clone(&self.dfu_log),
                        Arc::clone(&self.dfu_programmers),
                        self.egui_ctx.clone(),
                    );
                }
                scan_btn.on_hover_text(
                    "Scan for connected USB programmers:\n\
                     • DFU bootloader (STM32 with BOOT0 = 1)\n\
                     • ST-Link / J-Link / CMSIS-DAP\n\
                     • USB-Serial (ESP32-C3, …)",
                );

                ui.add_space(2.0);

                // ── Toolchain-specific flash buttons ──────────────────
                match chip_toolchain {
                    ToolchainKind::RustEmbedded => {
                        // ⚡ Flash via USB (DFU)
                        /*
                        let flash_enabled =
                            device_ok && !any_busy && project_files.is_some();
                        let flash_btn = ui.add_enabled(
                            flash_enabled,
                            egui::Button::new(
                                egui::RichText::new(format!("{} Flash USB", ph::LIGHTNING))
                                    .size(11.0)
                                    .color(if flash_enabled {
                                        egui::Color32::from_rgb(100, 200, 255)
                                    } else {
                                        egui::Color32::GRAY
                                    }),
                            ),
                        );
                        if flash_btn.clicked() {
                            if let Some(config) = self.selected_mcu_type.project_config() {
                                let build_dir =
                                    std::env::temp_dir().join("embedded_ide_0_check");
                                let code = self.generated_code.clone();
                                if project_gen::write_project(
                                    &build_dir,
                                    &config,
                                    &code,
                                    &self.project_tree.user_src_files,
                                )
                                .is_ok()
                                {
                                    self.build_tab = BuildPanelTab::Dfu;
                                    dfu::start_flash(
                                        build_dir,
                                        config.target.to_string(),
                                        config.pkg_name.to_string(),
                                        self.dfu_flash_addr.clone(),
                                        Arc::clone(&self.dfu_state),
                                        Arc::clone(&self.dfu_log),
                                        self.egui_ctx.clone(),
                                    );
                                }
                            }
                        }
                        flash_btn.on_hover_text(
                            "Build with --release, convert to .bin, flash via dfu-util.\n\
                             Requires:\n\
                             • STM32 in DFU mode (BOOT0 = 1)\n\
                             • dfu-util in PATH\n\
                             • WinUSB driver (install via Zadig)\n\
                             • llvm-objcopy or arm-none-eabi-objcopy",
                        );

                        ui.add_space(2.0);
                        */

                        // 🔗 Flash via SWD (OpenOCD)
                        let flash_swd_enabled =
                            is_swd_capable && !any_busy && project_files.is_some();
                        let flash_swd_btn = ui.add_enabled(
                            flash_swd_enabled,
                            egui::Button::new(
                                egui::RichText::new(format!("{} Flash SWD", ph::LIGHTNING))
                                    .size(11.0)
                                    .color(if flash_swd_enabled {
                                        egui::Color32::from_rgb(255, 165, 50)
                                    } else {
                                        egui::Color32::GRAY
                                    }),
                            ),
                        );
                        if flash_swd_btn.clicked() {
                            if let Some((project, _toolchain)) = self.selected_build_cfg() {
                                let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
                                if project_gen::write_project(
                                    &build_dir,
                                    &self.current_project_files(),
                                    &self.project_tree.user_src_files,
                                )
                                .is_ok()
                                {
                                    self.build_tab = BuildPanelTab::Dfu;
                                    openocd::start_flash(
                                        build_dir,
                                        project.target.clone(),
                                        project.pkg_name.clone(),
                                        sel_interface_cfg.clone(),
                                        sel_adapter.clone(),
                                        self.openocd_target_cfg.clone(),
                                        Arc::clone(&self.openocd_state),
                                        Arc::clone(&self.dfu_log),
                                        self.egui_ctx.clone(),
                                    );
                                }
                            }
                        }
                        flash_swd_btn.on_hover_text(
                            "Build with --release, then program via SWD using OpenOCD.\n\
                             Requires:\n\
                             • OpenOCD in PATH  (winget install openocd)\n\
                             • ST-Link/J-Link/CMSIS-DAP driver installed\n\
                             • Target .cfg selected in the Flash tab\n\
                             • SWD wiring: SWDIO + SWCLK + GND",
                        );
                    }

                    ToolchainKind::EspRust => {
                        // 🔶 Flash ESP32
                        let flash_esp_enabled = !any_busy && project_files.is_some();
                        let flash_esp_btn = ui.add_enabled(
                            flash_esp_enabled,
                            egui::Button::new(
                                egui::RichText::new(format!("{} Flash ESP32", ph::LIGHTNING))
                                    .size(11.0)
                                    .color(if flash_esp_enabled {
                                        egui::Color32::from_rgb(220, 140, 60)
                                    } else {
                                        egui::Color32::GRAY
                                    }),
                            ),
                        );
                        if flash_esp_btn.clicked() {
                            if let Some((project, _toolchain)) = self.selected_build_cfg() {
                                let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
                                if project_gen::write_project(
                                    &build_dir,
                                    &self.current_project_files(),
                                    &self.project_tree.user_src_files,
                                )
                                .is_ok()
                                {
                                    self.build_tab = BuildPanelTab::Dfu;

                                    // Extract port from selected programmer
                                    let port = self
                                        .dfu_programmers
                                        .lock()
                                        .unwrap()
                                        .get(&self.dfu_sel_programmer)
                                        .map(|p| p.port.clone())
                                        .unwrap_or_default();

                                    // println!(
                                    //     "port: {},self.dfu_sel_programmer: {:?}, dfu_programmers: {:?}",
                                    //     port,
                                    //     self.dfu_sel_programmer,

                                    //     self.dfu_programmers.lock().unwrap()
                                    // );

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
                        }
                        flash_esp_btn.on_hover_text(
                            "Build with --release, then flash via espflash.\n\
                             Requires:\n\
                             • espflash in PATH  (cargo install espflash)\n\
                             • ESP32-C3 connected via USB\n\
                             • ESP32-C3 in download mode:\n\
                                 hold BOOT → press RESET → release BOOT\n\
                             • Target installed:\n\
                                 rustup target add riscv32imc-unknown-none-elf",
                        );
                    }

                    ToolchainKind::SdccC => {
                        // STM8 — on hold, no flash button
                    }
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
