//! Hover-to-evaluate: while a debug session is halted, hovering an identifier
//! in the editor shows its value in a floating tooltip. The value comes from a
//! DAP `evaluate` (`context:"hover"`) against the selected frame — see
//! [`crate::debugger::Debugger::hover_eval`]. The request is async, so the
//! tooltip appears on the frame the reply lands (the reader repaints).

use crate::app::AppIde;
use crate::debugger::DebugPhase;
use eframe::egui;

impl AppIde {
    /// Resolve the identifier under the editor pointer, ask the target to
    /// evaluate it, and draw the result as a tooltip. A no-session / not-halted
    /// / not-on-an-identifier hover clears any pending evaluation. Call after the
    /// editor renders (it reads `editor_resp`'s galley).
    pub(super) fn paint_debug_hover(
        &self,
        ui: &egui::Ui,
        editor_resp: &egui::text_edit::TextEditOutput,
        display_code: &str,
    ) {
        // Only while halted on a frame.
        if !matches!(self.debugger.phase(), DebugPhase::Stopped(_)) {
            self.debugger.clear_hover();
            return;
        }
        // Pointer must be over the editor and not mid-drag (text selection).
        let Some(pos) = editor_resp.response.hover_pos() else {
            self.debugger.clear_hover();
            return;
        };
        if ui.input(|i| i.pointer.primary_down()) {
            return;
        }

        // Pointer position → char index → identifier (same mapping the
        // double-click word-select uses).
        let ccursor = editor_resp
            .galley
            .cursor_from_pos(pos - editor_resp.galley_pos);
        let expr = super::rename::identifier_at(display_code, ccursor.index);
        if expr.trim().is_empty() {
            self.debugger.clear_hover();
            return;
        }

        // Fire (debounced inside) + render whatever value we currently hold for
        // this exact expression.
        self.debugger.hover_eval(expr.clone());
        let tip = {
            let st = self.debugger.state.lock().unwrap();
            st.hover
                .as_ref()
                .filter(|h| h.expr == expr)
                .and_then(|h| h.value.as_ref().map(|v| (v.clone(), h.ty.clone())))
        };
        let Some((value, ty)) = tip else {
            return; // still awaiting the reply → no tooltip yet
        };

        // Floating tooltip just below-right of the pointer.
        egui::Area::new(egui::Id::new("debug_hover_eval_tip"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + egui::vec2(12.0, 18.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("{expr} = {value}"))
                            .monospace()
                            .size(11.0),
                    );
                    if let Some(t) = ty {
                        ui.label(
                            egui::RichText::new(t)
                                .size(10.0)
                                .color(egui::Color32::from_gray(150)),
                        );
                    }
                });
            });
    }
}
