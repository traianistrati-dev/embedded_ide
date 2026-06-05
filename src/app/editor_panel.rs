//! Center-left "Code Editor" panel.
//!
//! Owns: the toolbar (Copy / Build / Scan / Flash buttons), the code editor
//! widget itself, the embedded bottom diagnostics panel, the LSP completion
//! popup, and the inline-diagnostic overlays.  It also writes the edited text
//! back into `generated_code` (main.rs) or the matching user source file.
//!
//! Implemented as one inherent method on `AppIde`; it consumes the
//! `project_files` snapshot (nothing after this panel needs it).

use super::AppIde;
use super::diag_panel::show_diag_panel;
use super::{BuildPanelTab, ProjectFileId};
use crate::build::{self, BuildState};
use crate::dfu::{self, DfuState};
use crate::editor::gui::show_diagnostics_overlay;
use crate::editor::gui::text_pos::{
    diags_for_file, lsp_completion_prefix, lsp_cursor_pos, lsp_kind_icon, selected_file_rel_path,
    lsp_word_start,
};
use crate::espflash::{self, EspFlashState};
use crate::lsp;
use crate::openocd::{self, OpenOcdState};
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use crate::panels::mcu_module::project_gen::{self, ProjectFiles};
use eframe::egui;
use egui_code_editor::{CodeEditor, ColorTheme};
use egui_phosphor::regular as ph;
use std::sync::Arc;

impl AppIde {
    /// Render the central-left code editor panel (toolbar + editor + diagnostics).
    pub(super) fn show_editor_panel(
        &mut self,
        ui: &mut egui::Ui,
        project_files: Option<ProjectFiles>,
    ) {
        // ── Compute editor content AFTER the project tree ─────────────────────
        // IMPORTANT: display_code must be computed AFTER the project tree panel
        // so that self.selected_file reflects any click the user just made.
        // Computing it before the tree caused a write-back bug: when the user
        // clicked a user file, self.selected_file was already updated by the
        // click handler, but display_code still held the OLD file's content.
        // The write-back then wrongly stored the old content into the new file.
        let mut display_code: String = if let ProjectFileId::UserFile(i) = self.selected_file {
            self.project_tree
                .user_src_files
                .get(i)
                .map(|(_, c)| c.clone())
                .unwrap_or_default()
        } else if self.selected_file == ProjectFileId::MainRs {
            // Always read from self.generated_code — not from the project_files
            // snapshot built at the start of this frame.  The snapshot is stale
            // whenever load_project_from_dir() runs in the same frame (Open
            // Project), which would otherwise show the previous project's code
            // and then immediately overwrite generated_code via the write-back.
            self.generated_code.clone()
        } else {
            match &project_files {
                Some(files) => self.selected_file.content(files).to_owned(),
                None => self.generated_code.clone(),
            }
        };
        let display_syntax = self.selected_file.syntax();

        // ── Panel 2: Code Editor ──────────────────────────────────────────────
        let editor_width = ui.available_width() * 0.5;
        egui::Panel::left("code_editor")
            .resizable(true)
            .default_size(editor_width)
            .show_inside(ui, |ui| {
                // Header row
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
                                    display_code.clone(),
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
                            if let Some(config) = self.selected_mcu_type.project_config() {
                                let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
                                let code = self.generated_code.clone();
                                match project_gen::write_project(
                                    &build_dir,
                                    &config,
                                    &code,
                                    &self.project_tree.user_src_files,
                                ) {
                                    Ok(()) => {
                                        self.selected_diagnostic = None;
                                        self.build_tab = BuildPanelTab::Cargo;
                                        build::start_build(
                                            build_dir,
                                            config.target.to_string(),
                                            Arc::clone(&self.build_state),
                                            self.egui_ctx.clone(),
                                        );
                                    }
                                    Err(e) => {
                                        *self.build_state.lock().unwrap() = BuildState::Failed(
                                            format!("Could not write project to temp dir: {e}"),
                                        );
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
                        let device_ok = matches!(*dfu_guard, DfuState::DeviceFound(_));
                        drop(dfu_guard);

                        let ocd_busy = self.openocd_state.lock().unwrap().is_busy();
                        let esp_busy = self.espflash_state.lock().unwrap().is_busy();
                        let any_busy = dfu_busy || ocd_busy || esp_busy;

                        // Determine toolchain of the selected chip
                        let chip_toolchain = self.selected_mcu_type.toolchain();

                        // Determine if the selected programmer supports SWD flashing
                        let (is_swd_capable, sel_interface_cfg) = {
                            let progs = self.dfu_programmers.lock().unwrap();
                            let kind = progs
                                .get(&self.dfu_sel_programmer)
                                .map(|p| p.kind.clone())
                                .unwrap_or("".to_string());
                            let swd = matches!(kind.as_str(), "ST-Link" | "J-Link" | "CMSIS-DAP");
                            let cfg = openocd::interface_cfg_for_kind(&kind).to_string();
                            (swd, cfg)
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
                                            openocd::start_flash(
                                                build_dir,
                                                config.target.to_string(),
                                                config.pkg_name.to_string(),
                                                sel_interface_cfg.clone(),
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
                                        egui::RichText::new(format!(
                                            "{} Flash ESP32",
                                            ph::LIGHTNING
                                        ))
                                        .size(11.0)
                                        .color(
                                            if flash_esp_enabled {
                                                egui::Color32::from_rgb(220, 140, 60)
                                            } else {
                                                egui::Color32::GRAY
                                            },
                                        ),
                                    ),
                                );
                                if flash_esp_btn.clicked() {
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
                                                config.target.to_string(),
                                                config.probe_chip.to_string(),
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

                ui.separator();

                // ── Diagnostics panel (bottom, manually resizable) ────────────
                {
                    let cargo_has = !matches!(*self.build_state.lock().unwrap(), BuildState::Idle);
                    let lsp_active = self.lsp_state.lock().unwrap().status.is_active();
                    let dfu_active = !matches!(*self.dfu_state.lock().unwrap(), DfuState::Idle)
                        || !matches!(*self.openocd_state.lock().unwrap(), OpenOcdState::Idle)
                        || !matches!(*self.espflash_state.lock().unwrap(), EspFlashState::Idle)
                        || !self.dfu_log.lock().unwrap().is_empty();
                    let show_panel = cargo_has || lsp_active || dfu_active;

                    if show_panel {
                        const HANDLE_H: f32 = 6.0;
                        const MIN_H: f32 = 56.0;

                        // Keep height in valid range for current window size.
                        let max_h = (ui.available_height() - 60.0).max(MIN_H);
                        self.diag_panel_height = self.diag_panel_height.clamp(MIN_H, max_h);

                        // TopBottomPanel::bottom takes space from the bottom
                        // of the remaining area before the editor is laid out.
                        // exact_height gives us full control — no egui-internal
                        // default_height that would reset on show/hide.
                        egui::TopBottomPanel::bottom("diag_panel")
                            .exact_height(self.diag_panel_height + HANDLE_H)
                            .show_inside(ui, |ui| {
                                // ── Drag handle (top edge of panel) ───────
                                let (handle_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), HANDLE_H),
                                    egui::Sense::hover(),
                                );
                                let drag_resp = ui.interact(
                                    handle_rect,
                                    egui::Id::new("diag_panel_resize"),
                                    egui::Sense::drag(),
                                );

                                let mid_y = handle_rect.center().y;
                                let line_color = if drag_resp.hovered() || drag_resp.dragged() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                                    egui::Color32::from_rgb(100, 140, 200)
                                } else {
                                    egui::Color32::from_gray(65)
                                };

                                // Line + three grip dots
                                ui.painter().hline(
                                    handle_rect.x_range(),
                                    mid_y,
                                    egui::Stroke::new(1.5, line_color),
                                );
                                for dx in [-6.0_f32, 0.0, 6.0] {
                                    ui.painter().circle_filled(
                                        egui::pos2(handle_rect.center().x + dx, mid_y),
                                        1.5,
                                        line_color,
                                    );
                                }

                                if drag_resp.dragged() {
                                    // Dragging up → negative delta.y → panel grows
                                    self.diag_panel_height = (self.diag_panel_height
                                        - drag_resp.drag_delta().y)
                                        .clamp(MIN_H, max_h);
                                }

                                // ── Content ────────────────────────────────
                                show_diag_panel(
                                    ui,
                                    &self.egui_ctx,
                                    &self.build_state,
                                    &self.lsp_state,
                                    &self.dfu_state,
                                    &self.dfu_log,
                                    &self.dfu_programmers,
                                    &mut self.dfu_sel_programmer,
                                    &mut self.dfu_flash_addr,
                                    &self.openocd_state,
                                    &mut self.openocd_target_cfg,
                                    &self.espflash_state,
                                    &mut self.espflash_port,
                                    &self.tools_state,
                                    &self.selected_mcu_type.toolchain(),
                                    &mut self.build_tab,
                                    &mut self.selected_diagnostic,
                                    &mut self.lsp_selected_diagnostic,
                                    &mut self.selected_file,
                                );
                            });
                    }
                }

                // Use a unique id per file so egui's TextEditState (galley,
                // cursor, undo stack) is never shared between files.
                // A fixed id caused the editor to keep the previous file's
                // rendered galley when switching to a new file.
                let editor_id: String = match &self.selected_file {
                    ProjectFileId::UserFile(i) => {
                        let path = self
                            .project_tree
                            .user_src_files
                            .get(*i)
                            .map(|(p, _)| p.as_str())
                            .unwrap_or("?");
                        format!("code_editor:user:{path}")
                    }
                    ProjectFileId::MainRs => "code_editor:main_rs".into(),
                    ProjectFileId::CargoToml => "code_editor:cargo_toml".into(),
                    ProjectFileId::CargoConfig => "code_editor:cargo_config".into(),
                    ProjectFileId::MemoryX => "code_editor:memory_x".into(),
                    ProjectFileId::BuildRs => "code_editor:build_rs".into(),
                    ProjectFileId::GitIgnore => "code_editor:gitignore".into(),
                };

                // ── LSP completion: pre-editor key consumption ───────────────
                // Consume navigation / accept keys BEFORE show_with_completer
                // so the built-in Completer never sees them when our popup is open.
                //
                // Mouse clicks on popup items set `completion_pending_insert` last
                // frame; apply them here so the same accept path is used for both
                // keyboard and mouse.
                let mut lsp_accepted: Option<String> = self.completion_pending_insert.take();
                if lsp_accepted.is_some() {
                    self.completion_open = false;
                }

                if self.completion_open {
                    let has_items = !self.completion_filtered_items.is_empty();
                    if has_items {
                        let count = self.completion_filtered_items.len();
                        ui.input_mut(|inp| {
                            if inp.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                                self.completion_open = false;
                            } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                                // Clamp against the FILTERED count so selection never
                                // goes out of the visible list.
                                self.completion_sel =
                                    (self.completion_sel + 1).min(count.saturating_sub(1));
                            } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                                self.completion_sel = self.completion_sel.saturating_sub(1);
                            } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                                || inp.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                            {
                                // Use the filtered list — guaranteed same items as shown.
                                let sel = self.completion_sel.min(count.saturating_sub(1));
                                if let Some(item) = self.completion_filtered_items.get(sel) {
                                    lsp_accepted = Some(item.insert_text.clone());
                                }
                                self.completion_open = false;
                            }
                        });
                    }
                }
                // Detect Ctrl+Space BEFORE the editor so egui doesn't pass it
                // to the TextEdit as a literal character.
                let ctrl_space_pressed =
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Space));

                let editor_resp = CodeEditor::default()
                    .id_source(editor_id)
                    .with_rows(50)
                    .with_fontsize(13.0)
                    .with_theme(ColorTheme::GRUVBOX)
                    .with_numlines(true)
                    .show_with_completer(
                        ui,
                        &mut display_code,
                        &display_syntax,
                        &mut self.completer,
                    );

                // ── Write user edits back ────────────────────────────────────
                // display_code is a local clone; persist changes here.
                if let ProjectFileId::UserFile(i) = self.selected_file {
                    if let Some(entry) = self.project_tree.user_src_files.get_mut(i) {
                        if display_code != entry.1 {
                            entry.1 = display_code.clone();
                            // Auto-save to workspace so LSP and build see the change
                            let workspace = std::env::temp_dir().join("embedded_ide_0_check");
                            let dest = workspace.join("src").join(&entry.0);
                            if let Some(parent) = dest.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::write(&dest, entry.1.as_bytes());
                        }
                    }
                } else if self.selected_file == ProjectFileId::MainRs
                    && display_code != self.generated_code
                {
                    self.generated_code = display_code.clone();
                }

                // ── LSP completion: post-editor apply + trigger + popup ───────
                let cursor_char_idx = editor_resp
                    .state
                    .cursor
                    .char_range()
                    .map(|r| r.primary.index);

                // Apply accepted completion: replace [word_start..cursor] with insert_text
                if let Some(insert_text) = lsp_accepted {
                    if let Some(cur_idx) = cursor_char_idx {
                        let word_start = lsp_word_start(&display_code, cur_idx);
                        let chars: Vec<char> = display_code.chars().collect();
                        let before: String = chars[..word_start].iter().collect();
                        let after: String = chars[cur_idx..].iter().collect();
                        display_code = format!("{}{}{}", before, insert_text, after);
                        // Persist the change so the write-back below picks it up
                        // (the write-back already happened above; redo it for this file)
                        if let ProjectFileId::UserFile(i) = self.selected_file {
                            if let Some(entry) = self.project_tree.user_src_files.get_mut(i) {
                                entry.1 = display_code.clone();
                                let workspace = std::env::temp_dir().join("embedded_ide_0_check");
                                let dest = workspace.join("src").join(&entry.0);
                                let _ = std::fs::write(&dest, entry.1.as_bytes());
                            }
                        } else if self.selected_file == ProjectFileId::MainRs {
                            self.generated_code = display_code.clone();
                        }
                    }
                }

                // Trigger detection
                // LSP completions are available for any .rs file open in RA.
                let lsp_file_tracked = matches!(
                    self.selected_file,
                    ProjectFileId::MainRs | ProjectFileId::UserFile(_)
                );
                // Compute the relative path for the currently edited file.
                // Used for all LSP requests (did_change, request_completion, etc.)
                let current_rel_path: Option<String> =
                    selected_file_rel_path(&self.selected_file, &self.project_tree.user_src_files);
                {
                    let lsp_ready = lsp_file_tracked
                        && current_rel_path.is_some()
                        && matches!(self.lsp_state.lock().unwrap().status, lsp::LspStatus::Ready);
                    if lsp_ready {
                        let rel = current_rel_path.as_deref().unwrap_or("src/main.rs");
                        // Manual Ctrl+Space
                        if ctrl_space_pressed {
                            if let Some(idx) = cursor_char_idx {
                                let (line, col) = lsp_cursor_pos(&display_code, idx);
                                // Sync the latest editor text to RA BEFORE the
                                // completion request — the frame's did_change (sent
                                // at the top of update()) used last frame's code.
                                {
                                    let mut lsp = self.lsp_state.lock().unwrap();
                                    lsp.did_change(rel, &display_code);
                                    lsp.request_completion(rel, line, col, None);
                                }
                                self.completion_trigger_idx = idx;
                                self.completion_sel = 0;
                                self.completion_open = true;
                            }
                        }

                        // Auto-trigger on `.`  (method / field access)
                        let dot_trigger = editor_resp.response.changed()
                            && cursor_char_idx
                                .map(|idx| {
                                    let chars: Vec<char> = display_code.chars().collect();
                                    idx > 0 && chars.get(idx - 1) == Some(&'.')
                                })
                                .unwrap_or(false);
                        if dot_trigger && !ctrl_space_pressed {
                            if let Some(idx) = cursor_char_idx {
                                let (line, col) = lsp_cursor_pos(&display_code, idx);
                                {
                                    let mut lsp = self.lsp_state.lock().unwrap();
                                    lsp.did_change(rel, &display_code);
                                    lsp.request_completion(rel, line, col, Some('.'));
                                }
                                self.completion_trigger_idx = idx;
                                self.completion_sel = 0;
                                self.completion_open = true;
                            }
                        }

                        // Auto-trigger on `::` (Rust path separator)
                        let colon_trigger = !dot_trigger
                            && !ctrl_space_pressed
                            && editor_resp.response.changed()
                            && cursor_char_idx
                                .map(|idx| {
                                    let chars: Vec<char> = display_code.chars().collect();
                                    idx >= 2
                                        && chars.get(idx - 1) == Some(&':')
                                        && chars.get(idx - 2) == Some(&':')
                                })
                                .unwrap_or(false);
                        if colon_trigger {
                            if let Some(idx) = cursor_char_idx {
                                let (line, col) = lsp_cursor_pos(&display_code, idx);
                                {
                                    let mut lsp = self.lsp_state.lock().unwrap();
                                    lsp.did_change(rel, &display_code);
                                    lsp.request_completion(rel, line, col, Some(':'));
                                }
                                self.completion_trigger_idx = idx;
                                self.completion_sel = 0;
                                self.completion_open = true;
                            }
                        }
                    }

                    // Close popup if cursor moved back past the trigger point,
                    // or too far ahead (user navigated away from the trigger word).
                    if self.completion_open {
                        if let Some(idx) = cursor_char_idx {
                            let cursor = idx as isize;
                            let trigger = self.completion_trigger_idx as isize;
                            let delta = cursor - trigger;
                            // delta < 0  → user deleted back past trigger point
                            // delta > 80 → user moved far forward (switched context)
                            if delta < 0 || delta > 80 {
                                self.completion_open = false;
                            }
                        }
                    }
                }

                // ── LSP completion popup ───────────────────────────────────────
                if self.completion_open {
                    let all_items = self.lsp_state.lock().unwrap().completion_items.clone();

                    if !all_items.is_empty() {
                        // ── Live prefix filtering ────────────────────────────────
                        // Compute what the user has typed since the trigger point.
                        let prefix = cursor_char_idx
                            .map(|cur| {
                                lsp_completion_prefix(
                                    &display_code,
                                    self.completion_trigger_idx,
                                    cur,
                                )
                            })
                            .unwrap_or_default();

                        let filtered: Vec<lsp::CompletionItem> = if prefix.is_empty() {
                            all_items
                        } else {
                            let pl = prefix.to_lowercase();
                            all_items
                                .into_iter()
                                .filter(|it| it.label.to_lowercase().starts_with(&pl))
                                .collect()
                        };

                        // Persist filtered list so next frame's key handlers see
                        // exactly the same items the user sees right now.
                        self.completion_filtered_items = filtered.clone();

                        if filtered.is_empty() {
                            // Nothing matches the current prefix — hide the popup.
                            self.completion_open = false;
                        } else {
                            // Clamp selection into the visible filtered range.
                            self.completion_sel = self.completion_sel.min(filtered.len() - 1);
                            let sel = self.completion_sel;

                            // ── Popup screen position ────────────────────────────
                            let popup_pos = if let Some(char_range) =
                                editor_resp.state.cursor.char_range()
                            {
                                let cursor_idx = char_range.primary.index;
                                let text_char_count = editor_resp.galley.job.text.chars().count();
                                let clamped = cursor_idx.min(text_char_count.saturating_sub(1));
                                let cursor_local = editor_resp
                                    .galley
                                    .pos_from_cursor(egui::text::CCursor::new(clamped));
                                let offset = egui::vec2(0.0, cursor_local.height() + 2.0);
                                editor_resp.response.rect.left_top()
                                    + cursor_local.min.to_vec2()
                                    + offset
                            } else {
                                editor_resp.response.rect.left_top()
                            };

                            // ── Render popup ─────────────────────────────────────
                            // `interactable` defaults to true → mouse clicks work.
                            egui::Area::new(egui::Id::new("lsp_completion_popup"))
                                .fixed_pos(popup_pos)
                                .order(egui::Order::Foreground)
                                .show(ui.ctx(), |ui| {
                                    egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                        ui.set_min_width(440.0);
                                        ui.set_max_width(440.0);

                                        egui::ScrollArea::vertical()
                                            .max_height(300.0)
                                            .auto_shrink([false, true])
                                            .show(ui, |ui| {
                                                for (i, item) in filtered.iter().enumerate() {
                                                    let selected = i == sel;

                                                    let fg = if selected {
                                                        egui::Color32::WHITE
                                                    } else {
                                                        egui::Color32::from_rgb(200, 210, 230)
                                                    };
                                                    let sel_bg =
                                                        egui::Color32::from_rgb(40, 90, 160);
                                                    let hover_bg =
                                                        egui::Color32::from_rgb(50, 60, 80);
                                                    let detail_fg = if selected {
                                                        egui::Color32::from_rgb(160, 195, 255)
                                                    } else {
                                                        egui::Color32::from_rgb(110, 130, 155)
                                                    };

                                                    // Allocate the full row width for hit-testing.
                                                    let row_h = 19.0;
                                                    let avail_w = ui.available_width();
                                                    let (rect, row_resp) = ui.allocate_exact_size(
                                                        egui::vec2(avail_w, row_h),
                                                        egui::Sense::click(),
                                                    );

                                                    // Background (selected / hovered).
                                                    if selected {
                                                        ui.painter().rect_filled(rect, 2.0, sel_bg);
                                                    } else if row_resp.hovered() {
                                                        ui.painter()
                                                            .rect_filled(rect, 2.0, hover_bg);
                                                    }

                                                    let painter = ui.painter();
                                                    let icon = lsp_kind_icon(item.kind);
                                                    let label = format!("{} {}", icon, item.label);

                                                    // Icon + label — left-aligned.
                                                    painter.text(
                                                        rect.left_center() + egui::vec2(4.0, 0.0),
                                                        egui::Align2::LEFT_CENTER,
                                                        &label,
                                                        egui::FontId::monospace(12.0),
                                                        fg,
                                                    );

                                                    // Detail (type signature) — right-aligned,
                                                    // smaller and dimmer, truncated if needed.
                                                    if !item.detail.is_empty() {
                                                        let det = {
                                                            let chars: Vec<char> =
                                                                item.detail.chars().collect();
                                                            if chars.len() > 38 {
                                                                format!(
                                                                    "{}…",
                                                                    chars[..35]
                                                                        .iter()
                                                                        .collect::<String>()
                                                                )
                                                            } else {
                                                                item.detail.clone()
                                                            }
                                                        };
                                                        painter.text(
                                                            rect.right_center()
                                                                - egui::vec2(4.0, 0.0),
                                                            egui::Align2::RIGHT_CENTER,
                                                            det,
                                                            egui::FontId::monospace(10.5),
                                                            detail_fg,
                                                        );
                                                    }

                                                    // Mouse click → deferred insert.
                                                    if row_resp.clicked() {
                                                        self.completion_pending_insert =
                                                            Some(item.insert_text.clone());
                                                        self.completion_open = false;
                                                    }

                                                    // Scroll selected item into view.
                                                    if selected {
                                                        row_resp.scroll_to_me(None);
                                                    }

                                                    // Hover tooltip: documentation first,
                                                    // then full detail as fallback.
                                                    if !item.documentation.is_empty() {
                                                        row_resp.on_hover_text(
                                                            egui::RichText::new(
                                                                &item.documentation,
                                                            )
                                                            .size(11.5),
                                                        );
                                                    } else if !item.detail.is_empty() {
                                                        row_resp.on_hover_text(
                                                            egui::RichText::new(&item.detail)
                                                                .monospace()
                                                                .size(11.0),
                                                        );
                                                    }
                                                }
                                            }); // ScrollArea
                                    }); // Frame
                                }); // Area
                        }
                    }
                    // all_items is empty: either RA hasn't responded yet, or
                    // it responded with no completions / an error.
                    else if lsp_file_tracked {
                        let (resp_received, timed_out) = {
                            let lsp = self.lsp_state.lock().unwrap();
                            let received = lsp.completion_response_received;
                            let timeout = lsp
                                .completion_request_sent_at
                                .map(|t| t.elapsed().as_secs() > 6)
                                .unwrap_or(false);
                            (received, timeout)
                        };

                        if resp_received || timed_out {
                            // RA answered (empty) or request is stale — close popup.
                            self.completion_open = false;
                        } else {
                            // Still waiting — show a small spinner popup.
                            let popup_pos = cursor_char_idx.and_then(|_| {
                                editor_resp.state.cursor.char_range().map(|cr| {
                                    let clamped = cr.primary.index.min(
                                        editor_resp
                                            .galley
                                            .job
                                            .text
                                            .chars()
                                            .count()
                                            .saturating_sub(1),
                                    );
                                    let local = editor_resp
                                        .galley
                                        .pos_from_cursor(egui::text::CCursor::new(clamped));
                                    editor_resp.response.rect.left_top()
                                        + local.min.to_vec2()
                                        + egui::vec2(0.0, local.height() + 2.0)
                                })
                            });
                            if let Some(pos) = popup_pos {
                                egui::Area::new(egui::Id::new("lsp_completion_loading"))
                                    .fixed_pos(pos)
                                    .order(egui::Order::Foreground)
                                    .show(ui.ctx(), |ui| {
                                        egui::Frame::popup(&ui.ctx().global_style()).show(
                                            ui,
                                            |ui| {
                                                ui.add_space(2.0);
                                                ui.horizontal(|ui| {
                                                    ui.spinner();
                                                    ui.label(
                                                        egui::RichText::new("  rust-analyzer…")
                                                            .size(11.5)
                                                            .color(egui::Color32::from_rgb(
                                                                160, 175, 200,
                                                            )),
                                                    );
                                                });
                                                ui.add_space(2.0);
                                            },
                                        );
                                    });
                                ui.ctx().request_repaint();
                            }
                        }
                    }
                }

                // ── Diagnostic overlays ───────────────────────────────────────
                if lsp_file_tracked {
                    let diags: Vec<lsp::LspDiagnostic> = current_rel_path
                        .as_deref()
                        .map(|rel| {
                            let lsp = self.lsp_state.lock().unwrap();
                            diags_for_file(&lsp.diagnostics, rel)
                        })
                        .unwrap_or_default();

                    show_diagnostics_overlay(
                        ui,
                        editor_resp.galley_pos,
                        editor_resp.text_clip_rect,
                        &editor_resp.galley,
                        &diags,
                        &display_code,
                    );
                }
            });
    }
}
