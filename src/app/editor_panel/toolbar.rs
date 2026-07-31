//! Code-editor toolbar (header row): Copy, the Errors / Types toggles and the
//! live status label. (Scan USB + Flash moved to the Flash tab; Serial /
//! Terminal / Activity / Clippy shortcuts were removed; the Build button moved
//! into the Cargo Check tab on 2026-07-10.)
//!
//! Also hosts the `pub(crate)` action helpers on AppIde fired from the
//! bottom-panel tabs: `scan_usb`, `flash_swd`, `flash_esp` (Flash tab) and
//! `start_build` (Cargo tab).

use crate::app::{AppIde, BuildPanelTab, ProjectFileId};
use crate::build::{self, BuildState};
use crate::dfu::{self, DfuState};
use crate::espflash::{self, EspFlashState};
use crate::openocd::{self, OpenOcdState};
use crate::panels::mcu_module::project_gen;
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
            &self.structure_config_text(),
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
                std::sync::Arc::clone(&self.activity),
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
            &self.structure_config_text(),
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
                std::sync::Arc::clone(&self.activity),
            );
        }
    }

    /// Run `cargo check` on the generated project: write it to the check
    /// workspace, snapshot the compiled text (for the unused-local fade), then
    /// start the background build. No-op without a buildable chip config.
    /// Fired from the Cargo Check tab's Build button (was a top-toolbar button
    /// before 2026-07-10).
    pub(crate) fn start_build(&mut self, release: bool) {
        let Some((project, _toolchain)) = self.selected_build_cfg() else {
            return;
        };
        let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
        match project_gen::write_project(
            &build_dir,
            &self.current_project_files(),
            &self.project_tree.user_src_files,
            &self.mcu_config_text(),
            &self.structure_config_text(),
        ) {
            Ok(()) => {
                self.selected_diagnostic = None;
                self.build_tab = BuildPanelTab::Cargo;
                // Snapshot the compiled text so the "unused local variable"
                // fade can tell later whether this run's diagnostics still
                // match the live file.
                self.snapshot_build_text();
                build::start_build(
                    build_dir,
                    project.target.clone(),
                    Arc::clone(&self.build_state),
                    self.egui_ctx.clone(),
                    Arc::clone(&self.activity),
                    release,
                );
            }
            Err(e) => {
                *self.build_state.lock().unwrap() =
                    BuildState::Failed(format!("Could not write project to temp dir: {e}"));
            }
        }
    }

    /// Fire the Flash/RAM measurement once, when a flash that was running has
    /// just finished successfully. Called every frame from `AppIde::ui`.
    ///
    /// It runs AFTER the flash rather than alongside it on purpose: the flash
    /// pipelines build `--release` into the same workspace, and a second cargo
    /// there would just block on the target-dir lock. Afterwards the build is
    /// warm, so the measurement is near-instant.
    pub(crate) fn poll_flash_finished_size(&mut self) {
        let (dfu_busy, dfu_ok) = {
            let s = self.dfu_state.lock().unwrap();
            (s.is_busy(), matches!(*s, crate::dfu::DfuState::Success))
        };
        let (ocd_busy, ocd_ok) = {
            let s = self.openocd_state.lock().unwrap();
            (
                s.is_busy(),
                matches!(*s, crate::openocd::OpenOcdState::Success),
            )
        };
        let (esp_busy, esp_ok) = {
            let s = self.espflash_state.lock().unwrap();
            (
                s.is_busy(),
                matches!(*s, crate::espflash::EspFlashState::Success),
            )
        };
        let busy = dfu_busy || ocd_busy || esp_busy;
        let finished = self.flash_was_busy && !busy;
        self.flash_was_busy = busy;
        // Only on success — a failed flash usually means the build failed, and
        // measuring would just repeat the same cargo error in a second place.
        if finished && (dfu_ok || ocd_ok || esp_ok) {
            self.start_size_measure_quiet();
        }
    }

    /// Measure Flash/RAM usage from the Cargo tab's Size button — brings that
    /// tab to the front so the result is visible.
    pub(crate) fn start_size_measure(&mut self) {
        self.start_size_measure_inner(true);
    }

    /// Same measurement without switching tabs: the Flash tab's own Size button
    /// and the automatic run after each flash, both of which must leave the
    /// Flash tab in view (it renders its own copy of the usage row).
    pub(crate) fn start_size_measure_quiet(&mut self) {
        self.start_size_measure_inner(false);
    }

    /// Measure Flash/RAM usage: write the project, `cargo build --release`,
    /// then parse the ELF against the memory.x limits (see `crate::size`).
    /// No-op without a chip config.
    fn start_size_measure_inner(&mut self, focus_cargo_tab: bool) {
        let Some((project, _toolchain)) = self.selected_build_cfg() else {
            return;
        };
        let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
        match project_gen::write_project(
            &build_dir,
            &self.current_project_files(),
            &self.project_tree.user_src_files,
            &self.mcu_config_text(),
            &self.structure_config_text(),
        ) {
            Ok(()) => {
                if focus_cargo_tab {
                    self.build_tab = BuildPanelTab::Cargo;
                }
                crate::size::start_measure(
                    build_dir,
                    project.target.clone(),
                    self.memory_x.clone(),
                    Arc::clone(&self.size_state),
                    self.egui_ctx.clone(),
                    Arc::clone(&self.activity),
                );
            }
            Err(e) => {
                *self.size_state.lock().unwrap() =
                    crate::size::SizeState::Failed(format!("Could not write project: {e}"));
            }
        }
    }

    /// Start an RTT session: write the project, then hand off to the
    /// [`crate::rtt::RttConsole`] pipeline (build --release → probe-rs
    /// run/attach). Fired from the RTT tab's buttons. No-op without a chip.
    pub(crate) fn start_rtt(&mut self, mode: crate::rtt::RttMode) {
        let Some((project, _toolchain)) = self.selected_build_cfg() else {
            return;
        };
        let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
        match project_gen::write_project(
            &build_dir,
            &self.current_project_files(),
            &self.project_tree.user_src_files,
            &self.mcu_config_text(),
            &self.structure_config_text(),
        ) {
            Ok(()) => {
                self.build_tab = BuildPanelTab::Rtt;
                self.rtt.start(
                    mode,
                    build_dir,
                    project.target.clone(),
                    project.probe_chip.clone(),
                    self.egui_ctx.clone(),
                );
            }
            Err(e) => {
                *self.rtt.phase.lock().unwrap() =
                    crate::rtt::RttPhase::Error(format!("could not write project: {e}"));
            }
        }
    }

    /// Start a debug session: write the project, snapshot the breakpoints,
    /// then hand off to the [`crate::debugger::Debugger`] pipeline (build →
    /// probe-rs dap-server → flash + attach). No-op without a chip config.
    pub(crate) fn start_debug(&mut self) {
        let Some((project, _toolchain)) = self.selected_build_cfg() else {
            return;
        };
        let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
        match project_gen::write_project(
            &build_dir,
            &self.current_project_files(),
            &self.project_tree.user_src_files,
            &self.mcu_config_text(),
            &self.structure_config_text(),
        ) {
            Ok(()) => {
                self.build_tab = BuildPanelTab::Debug;
                let bps: std::collections::BTreeMap<String, Vec<u32>> = self
                    .breakpoints
                    .iter()
                    .map(|(k, v)| (k.clone(), v.iter().copied().collect()))
                    .collect();
                self.debugger.start(
                    build_dir,
                    project.target.clone(),
                    project.probe_chip.clone(),
                    bps,
                    self.egui_ctx.clone(),
                );
            }
            Err(e) => {
                self.debugger.state.lock().unwrap().phase =
                    crate::debugger::DebugPhase::Error(format!("could not write project: {e}"));
            }
        }
    }

    /// Render the editor header toolbar.  `display_code` is the text shown in
    /// the editor (copied verbatim by the Copy button).
    pub(super) fn show_editor_toolbar(&mut self, ui: &mut egui::Ui, display_code: &str) {
        ui.horizontal(|ui| {
            ui.heading("Code Editor");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // ── Collapse / expand the middle (MCU Configurator) zone ──
                // Hides Pins / Clock / Structure / … so the editor widens; the
                // Project tree on the far right always stays.
                // NOTE: this layout is RIGHT-TO-LEFT, so the first widget added
                // sits furthest right — this must come BEFORE Copy to appear to
                // its right.
                let collapsed = self.side_panels_collapsed;
                if ui
                    .selectable_label(
                        collapsed,
                        egui::RichText::new(if collapsed {
                            // format!("{} Panels", ph::ARROWS_OUT_SIMPLE)
                            ph::CARET_DOUBLE_LEFT
                        } else {
                            // format!("{} Panels", ph::ARROWS_IN_SIMPLE)
                            ph::CARET_RIGHT
                        })
                        .size(11.0)
                        .color(if collapsed {
                            egui::Color32::from_rgb(120, 190, 240)
                        } else {
                            egui::Color32::GRAY
                        }),
                    )
                    .on_hover_text(if collapsed {
                        "Show the MCU Configurator again (Pins / Clock / Structure …)"
                    } else {
                        "Hide the MCU Configurator (Pins / Clock / Structure …) so the editor widens — the Project tree stays"
                    })
                    .clicked()
                {
                    self.side_panels_collapsed = !collapsed;
                }

                ui.add_space(4.0);

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

                // (Serial / Terminal / Activity / Clippy shortcut buttons were
                // removed on 2026-07-08; the Build button moved into the Cargo
                // Check tab on 2026-07-10 — see `show_cargo_tab` / `start_build`.)

                // ── Inline-errors toggle ──────────────────────────────
                // Show/hide the in-editor RA/cargo diagnostic overlay
                // (squiggles + inline error text). The bottom-panel Cargo
                // Check / rust-analyzer tabs keep listing everything.
                let inline_btn = ui.selectable_label(
                    self.inline_errors_enabled,
                    egui::RichText::new(format!("{} Errors", ph::WARNING_OCTAGON))
                        .size(11.0)
                        .color(if self.inline_errors_enabled {
                            egui::Color32::from_rgb(230, 160, 60)
                        } else {
                            egui::Color32::GRAY
                        }),
                );
                if inline_btn.clicked() {
                    self.inline_errors_enabled = !self.inline_errors_enabled;
                }
                inline_btn.on_hover_text(if self.inline_errors_enabled {
                    "Inline errors: ON — squiggles and error messages are drawn in \
                     the editor.\nClick to hide them (they stay in the Cargo Check / \
                     rust-analyzer tabs)."
                } else {
                    "Inline errors: OFF — the editor overlay is hidden.\nClick to \
                     show squiggles and error messages inline again."
                });

                ui.add_space(4.0);

                // ── Inferred-type hint toggle ─────────────────────────
                // Show/hide the ghost type on the cursor's untyped `let` line
                // (Tab inserts it). OFF also disables the Tab accept.
                let types_btn = ui.selectable_label(
                    self.inlay_types_enabled,
                    egui::RichText::new(format!("{} Types", ph::TEXT_T))
                        .size(11.0)
                        .color(if self.inlay_types_enabled {
                            egui::Color32::from_rgb(120, 170, 210)
                        } else {
                            egui::Color32::GRAY
                        }),
                );
                if types_btn.clicked() {
                    self.inlay_types_enabled = !self.inlay_types_enabled;
                }
                types_btn.on_hover_text(if self.inlay_types_enabled {
                    "Inferred types: ON — the type of the `let` on the cursor line \
                     shows as ghost text; press Tab to insert it.\nClick to hide."
                } else {
                    "Inferred types: OFF — no ghost type is shown.\nClick to show \
                     the inferred type on the cursor's `let` line (Tab inserts it)."
                });

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
                        .map(|(name, _)| name.clone())
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
