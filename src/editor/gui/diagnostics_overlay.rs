//! Inline diagnostic visualization — wavy underlines, error messages, tooltips.

use crate::editor::gui::text_pos::{
    draw_wavy_underline, lsp_line_end_char_idx, lsp_pos_to_char_idx,
};
use crate::lsp::LspDiagnostic;
use eframe::egui;
use egui_phosphor::regular as ph;

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
    // `highlight`: (1-based line, band colour) of the diagnostic the user clicked
    // in the bottom panel — drawn as a translucent band (colour keyed by
    // severity: error red / warning yellow / info blue).
    highlight: Option<(u32, egui::Color32)>,
    // `def_line`: 1-based line of the F12 go-to-definition target (when it's in
    // this project file) — drawn with a translucent yellow band, like the
    // Definition tab.
    def_line: Option<u32>,
) {
    let total_chars = display_code.chars().count();

    // Painter clipped to editor bounds.
    let gp = galley_pos;
    let clip = text_clip_rect;
    let painter = ui.painter().with_clip_rect(clip);

    // ── Full-width line-highlight bands ───────────────────────────────────
    // Drawn before the diagnostics (so squiggles/messages render on top) AND
    // before the empty-diags return below (the def target may be a clean file).
    let band = |line: u32, color: egui::Color32| {
        let ci = lsp_pos_to_char_idx(display_code, line, 1).min(total_chars);
        let loc = galley.pos_from_cursor(egui::text::CCursor::new(ci));
        let y_top = gp.y + loc.min.y;
        let y_bot = gp.y + loc.max.y;
        if y_bot >= clip.top() && y_top <= clip.bottom() {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(clip.left(), y_top),
                    egui::pos2(clip.right(), y_bot),
                ),
                0.0,
                color,
            );
        }
    };
    // F12 definition line — translucent yellow (matches the Definition tab).
    if let Some(line) = def_line {
        band(line, egui::Color32::from_rgba_unmultiplied(255, 214, 90, 32));
    }
    // Clicked-diagnostic line — translucent band, colour keyed by severity.
    if let Some((line, color)) = highlight {
        band(line, color);
    }

    if diags.is_empty() {
        return;
    }

    // Lines that already drew an inline message — a line can carry several
    // diagnostics, but a second message would overlap the first, so show one.
    let mut msg_lines: Vec<u32> = Vec::new();

    // ── Per-diagnostic: underline + inline message + tooltip ──────────────
    for (di, diag) in diags.iter().enumerate() {
        let start_ci = lsp_pos_to_char_idx(&display_code, diag.line, diag.col).min(total_chars);
        let end_ci_raw =
            lsp_pos_to_char_idx(&display_code, diag.end_line, diag.end_col).min(total_chars);
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
        let eol_ci = lsp_line_end_char_idx(&display_code, diag.line).min(total_chars);
        let loc_eol = galley.pos_from_cursor(egui::text::CCursor::new(eol_ci));
        let same_row_eol = (loc_s.min.y - loc_eol.min.y).abs() < line_h * 0.5;
        // Only one inline message per line (a second would overlap the first).
        if same_row_eol && !msg_lines.contains(&diag.line) {
            msg_lines.push(diag.line);
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

/// Draw a single inferred-type inlay hint as dim ghost text at `insert_idx`
/// (the char index in `display_code` just after the binding name). Positioned
/// through the galley so it tracks scrolling, and clipped to the visible editor
/// area. Purely visual — the text is NOT part of the document (accepting it is
/// handled separately, by the caller's Tab binding).
pub fn show_inlay_hint(
    ui: &egui::Ui,
    galley_pos: egui::Pos2,
    text_clip_rect: egui::Rect,
    galley: &egui::text::Galley,
    insert_idx: usize,
    label: &str,
    font_size: f32,
) {
    let painter = ui.painter().with_clip_rect(text_clip_rect);
    let loc = galley.pos_from_cursor(egui::text::CCursor::new(insert_idx));
    let x = galley_pos.x + loc.min.x;
    let y_top = galley_pos.y + loc.min.y;
    let y_bot = galley_pos.y + loc.max.y;
    // Skip when scrolled out of the visible editor.
    if y_bot < text_clip_rect.top() || y_top > text_clip_rect.bottom() {
        return;
    }
    // rust-analyzer's type-hint label already includes the leading `: `
    // (renderColons default); render it verbatim, dimmed like an editor hint.
    let y_mid = (y_top + y_bot) * 0.5;
    painter.text(
        egui::pos2(x, y_mid),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(font_size),
        egui::Color32::from_rgb(120, 130, 140),
    );
}
