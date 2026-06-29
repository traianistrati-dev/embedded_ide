//! Bottom diagnostics panel embedded inside the code editor.
//!
//! Shows the resizable Cargo/RA/Flash/Tools panel when any of those have
//! activity.  Pure self-state; independent of the edited text.

use crate::app::AppIde;
use crate::app::BuildPanelTab;
use crate::app::diag_panel::show_diag_panel;
use crate::build::BuildState;
use crate::dfu::DfuState;
use crate::espflash::EspFlashState;
use crate::openocd::OpenOcdState;
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use eframe::egui;

impl AppIde {
    /// Render the embedded bottom diagnostics panel (drag handle + content).
    /// Returns the panel's top Y (= the bottom edge of the editor region above
    /// it), or `None` when the panel isn't shown — used to clip the inline
    /// diagnostic overlay so it can't bleed into this panel.
    /// `source_rewritten` is set to `true` when a Clippy "Fix" / "Apply all"
    /// applied edits to a source buffer (`generated_code` or a user file), so the
    /// caller can refresh its editor working-copy and avoid reverting them.
    pub(super) fn show_editor_diag_panel(
        &mut self,
        ui: &mut egui::Ui,
        source_rewritten: &mut bool,
    ) -> Option<f32> {
        let cargo_has = !matches!(*self.build_state.lock().unwrap(), BuildState::Idle);
        let lsp_active = self.lsp_state.lock().unwrap().status.is_active();
        let dfu_active = !matches!(*self.dfu_state.lock().unwrap(), DfuState::Idle)
            || !matches!(*self.openocd_state.lock().unwrap(), OpenOcdState::Idle)
            || !matches!(*self.espflash_state.lock().unwrap(), EspFlashState::Idle)
            || !self.dfu_log.lock().unwrap().is_empty();
        // The Serial tab is opened from the toolbar (`build_tab == Serial`) and
        // stays available while a port is connected.
        let serial_active =
            self.build_tab == BuildPanelTab::Serial || self.serial.is_connected();
        // The Clippy tab stays available while selected or while a run is going.
        let clippy_active = self.build_tab == BuildPanelTab::Clippy
            || matches!(*self.clippy_state.lock().unwrap(), BuildState::Building);
        let show_panel = cargo_has
            || lsp_active
            || dfu_active
            || serial_active
            || clippy_active
            || self.definition_view.is_some();

        if !show_panel {
            return None;
        }
        const HANDLE_H: f32 = 6.0;
        const MIN_H: f32 = 56.0;

        // Keep height in valid range for current window size.
        let max_h = (ui.available_height() - 60.0).max(MIN_H);
        self.diag_panel_height = self.diag_panel_height.clamp(MIN_H, max_h);

        // Panel::bottom takes space from the bottom of the remaining area
        // before the editor is laid out. exact_size gives us full control —
        // no egui-internal default that would reset on show/hide.
        let mut def_close = false;
        // Diagnostic-row click navigation: (rel_path, 1-based line, band colour).
        let mut nav: Option<(String, usize, egui::Color32)> = None;
        // Set by the Clippy tab's "Run clippy" button.
        let mut clippy_run = false;
        // Set by the Clippy tab's per-row "Fix" / "Apply all" buttons.
        let mut clippy_apply_one: Option<usize> = None;
        let mut clippy_apply_all = false;
        let panel = egui::Panel::bottom("diag_panel")
            .exact_size(self.diag_panel_height + HANDLE_H)
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
                        self.diag_panel_height =
                            (self.diag_panel_height - drag_resp.drag_delta().y).clamp(MIN_H, max_h);
                    }

                    // ── Content ────────────────────────────────
                    let toolchain = self.selected_toolchain().unwrap_or(ToolchainKind::SdccC);
                    let definition = self
                        .definition_view
                        .as_ref()
                        .map(|d| (d.header.as_str(), d.code.as_str(), d.highlight));
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
                        &mut self.serial,
                        &self.clippy_state,
                        &mut self.clippy_sel,
                        &mut clippy_run,
                        &mut clippy_apply_one,
                        &mut clippy_apply_all,
                        &toolchain,
                        &mut self.build_tab,
                        &mut self.selected_diagnostic,
                        &mut self.lsp_selected_diagnostic,
                        &mut nav,
                        definition,
                        &mut def_close,
                        &mut self.def_scroll_pending,
                    );
                });
        // "Run clippy" was clicked: write the project to the workspace and start
        // `cargo clippy` on a worker thread (serialized with Build).
        if clippy_run {
            self.start_clippy_run();
        }

        // "Fix" / "Apply all" was clicked: collect the relevant machine-applicable
        // span edits from the current clippy results and splice them into the
        // in-memory source.
        if clippy_apply_all || clippy_apply_one.is_some() {
            let edits: Vec<crate::build::SpanEdit> = {
                let cs = self.clippy_state.lock().unwrap();
                match &*cs {
                    BuildState::Done(r) if clippy_apply_all => {
                        r.diagnostics.iter().flat_map(|d| d.fixes.clone()).collect()
                    }
                    BuildState::Done(r) => clippy_apply_one
                        .and_then(|i| r.diagnostics.get(i))
                        .map(|d| d.fixes.clone())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                }
            };
            if !edits.is_empty() && self.apply_source_edits(&edits) > 0 {
                // Tell the editor to refresh its working copy from the rewritten
                // source, otherwise its stale clone is written back and reverts us.
                *source_rewritten = true;
                // The applied edits shift every later byte offset, so the current
                // results are now stale. Re-run clippy automatically so the list
                // refreshes with valid offsets (no need to press "Run clippy"
                // again). If a run can't be started, just clear the stale list.
                if !self.start_clippy_run() {
                    *self.clippy_state.lock().unwrap() = BuildState::Idle;
                    self.clippy_sel = None;
                }
            }
        }

        // Closing the Definition tab clears the snippet and switches away.
        if def_close {
            self.definition_view = None;
            if self.build_tab == BuildPanelTab::Definition {
                self.build_tab = BuildPanelTab::RustAnalyzer;
            }
        }
        // A diagnostic row was clicked: open its file (incl. user `src/` files)
        // and queue the scroll-to-line, applied once the editor shows that file.
        if let Some((path, line, color)) = nav {
            if let Some(id) =
                crate::app::resolve_diag_file(&path, &self.project_tree.user_src_files)
            {
                self.selected_file = id;
                self.pending_scroll_to_line = Some((id, line));
                self.highlighted_error_line = Some((id, line, color));
            }
        }
        Some(panel.response.rect.top())
    }

    /// Write the current project to the temp workspace and start `cargo clippy`
    /// on a worker thread. Returns `false` when there's no buildable chip config
    /// or the workspace write failed (so the caller can fall back). Clears the
    /// selected-row index since fresh results invalidate it.
    fn start_clippy_run(&mut self) -> bool {
        let Some((project, _tc)) = self.selected_build_cfg() else {
            return false;
        };
        let build_dir = std::env::temp_dir().join("embedded_ide_0_check");
        if crate::panels::mcu_module::project_gen::write_project(
            &build_dir,
            &self.current_project_files(),
            &self.project_tree.user_src_files,
            &self.mcu_config_text(),
        )
        .is_err()
        {
            return false;
        }
        self.clippy_sel = None;
        crate::build::start_clippy(
            build_dir,
            project.target.clone(),
            std::sync::Arc::clone(&self.clippy_state),
            self.egui_ctx.clone(),
        );
        true
    }
}
