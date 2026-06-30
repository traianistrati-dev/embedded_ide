//! `Ctrl+R` — rename a symbol project-wide via rust-analyzer's
//! `textDocument/rename`. The popup collects the new name; on submit the current
//! file is synced to RA, the rename is requested, and the returned cross-file
//! edits are applied in `AppIde::apply_rename_edits` (see `app.rs`).

use crate::app::AppIde;
use eframe::egui;

/// The identifier (variable / function / type / …) surrounding char index
/// `cursor` in `text`, or "" if the cursor isn't on one.
pub fn identifier_at(text: &str, cursor: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = cursor.min(chars.len());
    while start > 0 && is_id(chars[start - 1]) {
        start -= 1;
    }
    let mut end = cursor.min(chars.len());
    while end < chars.len() && is_id(chars[end]) {
        end += 1;
    }
    chars[start..end].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::identifier_at;

    #[test]
    fn finds_identifier_around_cursor() {
        let text = "let my_var = 1;";
        // cursor in the middle of `my_var`
        assert_eq!(identifier_at(text, 7), "my_var");
        // cursor at the start of `my_var`
        assert_eq!(identifier_at(text, 4), "my_var");
        // cursor right after `my_var` (on the space)
        assert_eq!(identifier_at(text, 10), "my_var");
    }

    #[test]
    fn empty_when_not_on_identifier() {
        // cursor between the space and `+`, no identifier char on either side
        assert_eq!(identifier_at("a + b", 2), "");
    }
}

impl AppIde {
    /// Render the rename input popup (shown while `rename_active`). On submit it
    /// sends the rename request to RA; on Esc / Cancel it just closes.
    pub(super) fn show_rename_popup(&mut self, ui: &mut egui::Ui) {
        if !self.rename_active {
            return;
        }
        let mut submit = false;
        let mut cancel = false;

        egui::Area::new(egui::Id::new("rename_popup"))
            .fixed_pos(self.rename_popup_pos)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(&ui.ctx().global_style()).show(ui, |ui| {
                    ui.set_min_width(200.0);
                    ui.label(
                        egui::RichText::new("Rename symbol (everywhere)")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(170, 180, 200)),
                    );
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.rename_input)
                            .desired_width(200.0)
                            .hint_text("new name"),
                    );
                    if self.rename_focus {
                        resp.request_focus();
                        self.rename_focus = false;
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            submit = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                        ui.label(
                            egui::RichText::new("Enter / Esc")
                                .size(9.0)
                                .color(egui::Color32::from_rgb(120, 130, 150)),
                        );
                    });
                });
            });

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }

        if cancel {
            self.rename_active = false;
            return;
        }
        if submit {
            self.rename_active = false;
            let new_name = self.rename_input.trim().to_owned();
            if new_name.is_empty() {
                return;
            }
            // Sync the current file to RA first (it may hold debounced-stale text),
            // then request the rename; the response is applied in init_frame.
            let rel = self.rename_rel.clone();
            let content = self.file_content_for(&rel);
            let mut lsp = self.lsp_state.lock().unwrap();
            lsp.did_change(&rel, &content, false);
            lsp.request_rename(&rel, self.rename_line, self.rename_char, &new_name);
            drop(lsp);
            self.rename_in_flight = true;
            self.egui_ctx.request_repaint();
        }
    }

    /// Current in-memory content of a workspace-relative file (`src/main.rs` or a
    /// user source file under `src/`).
    pub(super) fn file_content_for(&self, rel: &str) -> String {
        if rel == "src/main.rs" {
            self.generated_code.clone()
        } else if let Some(sub) = rel.strip_prefix("src/") {
            self.project_tree
                .user_src_files
                .iter()
                .find(|(p, _)| p == sub)
                .map(|(_, c)| c.clone())
                .unwrap_or_default()
        } else {
            String::new()
        }
    }
}
