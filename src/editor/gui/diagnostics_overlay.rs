//! Inline diagnostic visualization — wavy underlines, error messages, tooltips.

use eframe::egui;
use egui_phosphor::regular as ph;
use crate::lsp::LspDiagnostic;
use crate::editor::gui::text_pos::{draw_wavy_underline, lsp_line_end_char_idx, lsp_pos_to_char_idx};

/// Draw inline diagnostics (wavy underlines, inline messages, hover tooltips)
/// for the currently visible code in the editor.
///
/// Called after rendering the code editor but before closing the UI panel.
/// The rustc error-index URL for a compiler error code like `E0599`, or `None`
/// for lint names (e.g. `unused_variables`) which have no such page.
fn rustc_error_doc_url(code: &str) -> Option<String> {
    let digits = code.strip_prefix('E')?;
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        Some(format!("https://doc.rust-lang.org/error_codes/{code}.html"))
    } else {
        None
    }
}

pub fn show_diagnostics_overlay(
    ui: &mut egui::Ui,
    galley_pos: egui::Pos2,
    text_clip_rect: egui::Rect,
    galley: &egui::text::Galley,
    diags: &[LspDiagnostic],
    display_code: &str,
    // `copy_requested`: true when Ctrl+C was pressed this frame — the hovered
    // diagnostic copies its message to the clipboard.
    copy_requested: bool,
) {
    if diags.is_empty() {
        return;
    }

    let total_chars = display_code.chars().count();

    // Painter clipped to editor bounds
    let gp = galley_pos;
    let clip = text_clip_rect;
    let painter = ui.painter().with_clip_rect(clip);

    // ── Per-diagnostic: underline + inline message + tooltip ──────────────
    for (di, diag) in diags.iter().enumerate() {
        let start_ci = lsp_pos_to_char_idx(&display_code, diag.line, diag.col)
            .min(total_chars);
        let end_ci_raw = lsp_pos_to_char_idx(&display_code, diag.end_line, diag.end_col)
            .min(total_chars);
        let end_ci = if end_ci_raw <= start_ci {
            (start_ci + 1).min(total_chars)
        } else {
            end_ci_raw
        };

        // Galley-local positions
        let loc_s = galley.pos_from_cursor(egui::text::CCursor::new(start_ci));
        let loc_e = galley.pos_from_cursor(egui::text::CCursor::new(end_ci));

        // Screen coordinates
        let sx = gp.x + loc_s.min.x;
        let sy_top = gp.y + loc_s.min.y;
        let sy_bot = gp.y + loc_s.max.y;
        let line_h = loc_s.height().max(1.0);
        let sy_mid = (sy_top + sy_bot) * 0.5;

        // Skip lines scrolled out of the visible editor — otherwise the squiggle,
        // inline message, and hover region would land below the editor in the
        // bottom diagnostics panel (the painter clip hides the drawing, but the
        // hover interaction must be skipped too).
        if sy_bot < clip.top() || sy_top > clip.bottom() {
            continue;
        }

        // Same-line check
        let same_line = (loc_s.min.y - loc_e.min.y).abs() < line_h * 0.5;
        let ex = if same_line {
            gp.x + loc_e.min.x
        } else {
            gp.x + galley.rect.width()
        };
        if ex <= sx + 1.0 {
            continue;
        }

        // Severity colours
        let (ul_color, bg_color, msg_color) = match diag.severity {
            crate::lsp::DiagSeverity::Error => (
                egui::Color32::from_rgb(220, 65, 55),
                egui::Color32::from_rgba_unmultiplied(210, 55, 45, 22),
                egui::Color32::from_rgb(200, 80, 70),
            ),
            crate::lsp::DiagSeverity::Warning => (
                egui::Color32::from_rgb(210, 165, 35),
                egui::Color32::from_rgba_unmultiplied(200, 160, 30, 14),
                egui::Color32::from_rgb(190, 150, 40),
            ),
            crate::lsp::DiagSeverity::Info => (
                egui::Color32::from_rgb(80, 140, 215),
                egui::Color32::TRANSPARENT,
                egui::Color32::from_rgb(100, 150, 210),
            ),
            crate::lsp::DiagSeverity::Hint => (
                egui::Color32::from_rgb(100, 160, 110),
                egui::Color32::TRANSPARENT,
                egui::Color32::from_rgb(110, 150, 110),
            ),
        };

        // Background tint
        if bg_color.a() > 0 {
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(sx, sy_top), egui::pos2(ex, sy_bot)),
                0.0,
                bg_color,
            );
        }

        // Wavy underline
        draw_wavy_underline(&painter, sx, ex, sy_bot, ul_color);

        // ── Inline message at end of line ─────────────────────────────────
        let eol_ci = lsp_line_end_char_idx(&display_code, diag.line)
            .min(total_chars);
        let loc_eol = galley
            .pos_from_cursor(egui::text::CCursor::new(eol_ci));
        let same_row_eol = (loc_s.min.y - loc_eol.min.y).abs() < line_h * 0.5;
        if same_row_eol {
            let msg_x = gp.x + loc_eol.min.x + 16.0;
            // First line only, then cap length — a multi-line message rendered
            // raw would draw extra rows and overlap the code below it.
            let headline = diag.headline();
            let short_msg: String = headline.chars().take(72).collect();
            let short_msg = if headline.chars().count() > 72 || diag.has_more_lines() {
                format!("{short_msg}…")
            } else {
                short_msg
            };
            painter.text(
                egui::pos2(msg_x, sy_mid),
                egui::Align2::LEFT_CENTER,
                &short_msg,
                egui::FontId::monospace(10.5),
                msg_color,
            );
        }

        // ── Hover tooltip (full message + docs link) ──────────────────────
        let hover_rect =
            egui::Rect::from_min_max(egui::pos2(sx, sy_top), egui::pos2(ex, sy_bot + 3.0));
        let hover = ui.interact(
            hover_rect,
            egui::Id::new("inline_diag").with(di),
            egui::Sense::hover(),
        );

        // Ctrl+C while hovering copies the message + the error code (overwrites
        // any selection the editor copied earlier this frame, so the error wins).
        if hover.hovered() && copy_requested {
            let text = match &diag.code {
                Some(c) => format!("{} [{c}]", diag.message),
                None => diag.message.clone(),
            };
            ui.ctx().copy_text(text);
        }

        let icon = match diag.severity {
            crate::lsp::DiagSeverity::Error => ph::X_CIRCLE,
            crate::lsp::DiagSeverity::Warning => ph::WARNING,
            crate::lsp::DiagSeverity::Info => ph::INFO,
            crate::lsp::DiagSeverity::Hint => ph::DOT_OUTLINE,
        };
        let msg = format!("{icon}  {}", diag.message);
        let code = diag.code.clone();
        // Interactive tooltip — the user can move into it to click the docs link.
        hover.on_hover_ui(|ui: &mut egui::Ui| {
            ui.set_max_width(420.0);
            ui.label(egui::RichText::new(&msg).size(12.0));
            if let Some(c) = &code {
                match rustc_error_doc_url(c) {
                    // Clickable link → opens the rust error index in the browser.
                    Some(url) => {
                        ui.hyperlink_to(
                            egui::RichText::new(format!("[{c}]  open docs ↗"))
                                .size(10.5)
                                .color(egui::Color32::from_rgb(110, 165, 240)),
                            url,
                        );
                    }
                    None => {
                        ui.label(
                            egui::RichText::new(format!("[{c}]"))
                                .size(10.5)
                                .color(egui::Color32::from_rgb(140, 150, 170)),
                        );
                    }
                }
            }
            ui.label(
                egui::RichText::new("Ctrl+C to copy")
                    .size(9.0)
                    .color(egui::Color32::from_rgb(110, 120, 140)),
            );
        });
    }
}
