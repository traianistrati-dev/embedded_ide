//! Bottom diagnostics panel embedded inside the code editor.
//!
//! Shows the resizable Cargo/RA/Flash/Tools panel when any of those have
//! activity.  Pure self-state; independent of the edited text.

use crate::app::AppIde;
use crate::app::diag_panel::show_diag_panel;
use crate::build::BuildState;
use crate::dfu::DfuState;
use crate::espflash::EspFlashState;
use crate::openocd::OpenOcdState;
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use eframe::egui;

impl AppIde {
    /// Render the embedded bottom diagnostics panel (drag handle + content).
    pub(super) fn show_editor_diag_panel(&mut self, ui: &mut egui::Ui) {
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
            egui::Panel::bottom("diag_panel")
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
                        &toolchain,
                        &mut self.build_tab,
                        &mut self.selected_diagnostic,
                        &mut self.lsp_selected_diagnostic,
                        &mut self.selected_file,
                    );
                });
        }
    }
}
