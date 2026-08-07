//! RA status bar — shows rust-analyzer state, diagnostics, and checking progress.

use crate::app::ProjectFileId;
use crate::editor::gui::text_pos::selected_file_rel_path;
use crate::lsp::{LspState, LspStatus};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Display the RA status bar above the code editor.
/// Shows whether RA is indexing, checking, or ready, along with error/warning counts.
pub fn show_ra_status_bar(
    ui: &mut egui::Ui,
    lsp_state: &std::sync::Arc<std::sync::Mutex<LspState>>,
    selected_file: &ProjectFileId,
    user_src_files: &[(String, String)],
) {
    let (ra_status, checking, errs, warns) = {
        let lsp = lsp_state.lock().unwrap();
        let rel = selected_file_rel_path(selected_file, user_src_files).unwrap_or_default();
        let errs = lsp
            .diagnostics
            .get(&rel)
            .map(|v| v.iter().filter(|d| d.severity.is_error()).count())
            .unwrap_or(0);
        let warns = lsp
            .diagnostics
            .get(&rel)
            .map(|v| v.iter().filter(|d| d.severity.is_warning()).count())
            .unwrap_or(0);
        (lsp.status.clone(), lsp.checking, errs, warns)
    };

    // Keep the spinner animating while RA is busy — at ~10 FPS, not every
    // frame (a per-frame request here kept the WHOLE app repainting for the
    // full duration of indexing and of every post-save cargo check).
    if checking || matches!(ra_status, LspStatus::Starting | LspStatus::Indexing) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }

    ui.horizontal(|ui| {
        ui.add_space(2.0);
        match &ra_status {
            LspStatus::Stopped => {
                ui.label(
                    egui::RichText::new(format!("{} RA oprit", ph::PLUGS))
                        .size(10.5)
                        .color(egui::Color32::from_gray(90)),
                );
            }
            LspStatus::Starting | LspStatus::Indexing => {
                crate::app::helpers::spinner::throttled_spinner(ui, 12.0);
                ui.label(
                    egui::RichText::new(if matches!(ra_status, LspStatus::Starting) {
                        " rust-analyzer: pornire…"
                    } else {
                        " rust-analyzer: indexare crate-uri…"
                    })
                    .size(10.5)
                    .color(egui::Color32::from_rgb(200, 190, 80)),
                );
            }
            LspStatus::Ready => {
                if checking {
                    crate::app::helpers::spinner::throttled_spinner(ui, 12.0);
                    ui.label(
                        egui::RichText::new(" verificare cargo…")
                            .size(10.5)
                            .color(egui::Color32::from_rgb(100, 160, 220)),
                    );
                    ui.label(
                        egui::RichText::new("  |")
                            .size(10.5)
                            .color(egui::Color32::from_gray(70)),
                    );
                }
                let (icon, color) = if errs > 0 {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                } else if warns > 0 {
                    (ph::WARNING, egui::Color32::from_rgb(210, 170, 40))
                } else {
                    (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100))
                };
                ui.label(
                    egui::RichText::new(format!("{icon} RA"))
                        .size(10.5)
                        .color(color),
                );
                if errs > 0 {
                    ui.label(
                        egui::RichText::new(format!("  {errs} erori"))
                            .size(10.5)
                            .color(egui::Color32::from_rgb(220, 80, 70))
                            .strong(),
                    );
                }
                if warns > 0 {
                    ui.label(
                        egui::RichText::new(format!("  {warns} atenționări"))
                            .size(10.5)
                            .color(egui::Color32::from_rgb(210, 170, 40)),
                    );
                }
                if errs == 0 && warns == 0 && !checking {
                    ui.label(
                        egui::RichText::new("  fără erori")
                            .size(10.5)
                            .color(egui::Color32::from_gray(130))
                            .italics(),
                    );
                }
            }
            LspStatus::Failed(msg) => {
                ui.label(
                    egui::RichText::new(format!(
                        "{} RA eșuat: {}",
                        ph::X_CIRCLE,
                        msg.lines().next().unwrap_or("eroare")
                    ))
                    .size(10.5)
                    .color(egui::Color32::from_rgb(220, 80, 70)),
                )
                .on_hover_text(msg.as_str());
            }
        }
    });
    ui.add_space(1.0);
}
