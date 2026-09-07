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

/// Paint one full-width translucent band over `line` (1-based) of the editor.
///
/// The band lands OVER the text, so `color` must stay translucent — an opaque
/// fill hides the very line it is pointing at.
///
/// Standalone (rather than only inside [`show_diagnostics_overlay`]) so a
/// highlight can be drawn on a file rust-analyzer doesn't track, or while the
/// inline-errors toggle is off — neither has anything to do with wanting to see
/// where a jump landed.
pub fn show_line_band(
    ui: &egui::Ui,
    galley_pos: egui::Pos2,
    text_clip_rect: egui::Rect,
    galley: &egui::text::Galley,
    display_code: &str,
    line: u32,
    color: egui::Color32,
) {
    let total_chars = display_code.chars().count();
    let ci = lsp_pos_to_char_idx(display_code, line, 1).min(total_chars);
    let loc = galley.pos_from_cursor(egui::text::CCursor::new(ci));
    let y_top = galley_pos.y + loc.min.y;
    let y_bot = galley_pos.y + loc.max.y;
    if y_bot < text_clip_rect.top() || y_top > text_clip_rect.bottom() {
        return; // scrolled out of view
    }
    ui.painter().with_clip_rect(text_clip_rect).rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(text_clip_rect.left(), y_top),
            egui::pos2(text_clip_rect.right(), y_bot),
        ),
        0.0,
        color,
    );
}

/// The inline message's font. Monospace, which is what lets one glyph's advance
/// size any message (see `char_w` in the overlay).
fn msg_font() -> egui::FontId {
    egui::FontId::monospace(10.5)
}

/// Gap kept between a line's "N refs" pill and the inline message after it.
const PILL_GAP: f32 = 10.0;

/// Where the inline message starts: after this line's pill when there is one,
/// and never before the position it would have taken on its own.
///
/// `max`, not "pill_right + gap", because a pill can be narrower than the
/// message's own 16 px indent — moving the message LEFT to hug a short pill
/// would be a second bug wearing the first one's clothes.
pub(crate) fn inline_message_x(base_x: f32, pill_right: Option<f32>) -> f32 {
    match pill_right {
        Some(right) => base_x.max(right + PILL_GAP),
        None => base_x,
    }
}

/// `text` cut to what fits in `avail` pixels at `char_w` per character, or
/// `None` when so little room is left that a stub would say nothing.
///
/// Monospace, so the character count is exact rather than a guess. The cut keeps
/// a trailing `…`; the caller already uses that character to mean "there is
/// more", so a width-elided message reads as truncated and not as broken.
pub(crate) fn fit_to_width(text: &str, avail: f32, char_w: f32) -> Option<String> {
    if char_w <= 0.0 {
        return Some(text.to_owned());
    }
    // Below this the message is a couple of letters and an ellipsis, which is
    // noise on top of the code; the hover tooltip and the error list still carry
    // the full text.
    const MIN_CHARS: usize = 8;
    let fits = (avail / char_w).floor().max(0.0) as usize;
    if text.chars().count() <= fits {
        return Some(text.to_owned());
    }
    if fits < MIN_CHARS {
        return None;
    }
    let mut out: String = text.chars().take(fits - 1).collect();
    out.push('…');
    Some(out)
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
    // `pill_edges`: (1-BASED line, right edge in screen x) of every "N refs"
    // pill the usages overlay painted earlier in this same frame. The inline
    // message steps around them; see [`inline_message_x`].
    pill_edges: &[(u32, f32)],
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
        band(
            line,
            egui::Color32::from_rgba_unmultiplied(255, 214, 90, 32),
        );
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

    // One measurement for the whole pass. The message font is monospace, so a
    // single glyph's advance sizes every message; measuring inside the loop laid
    // out an "M" once per diagnostic per frame for the same answer.
    let char_w = painter
        .layout_no_wrap("M".to_owned(), msg_font(), egui::Color32::WHITE)
        .size()
        .x;

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
            // Start after this line's "N refs" pill when it has one. The pill
            // begins at end-of-line + 14 and the message at + 16, so before this
            // the message was painted straight through it.
            let pill_right = pill_edges
                .iter()
                .find(|(line, _)| *line == diag.line)
                .map(|(_, right)| *right);
            let msg_x = inline_message_x(gp.x + loc_eol.min.x + 16.0, pill_right);
            // First line only, then cap length — a multi-line message rendered
            // raw would draw extra rows and overlap the code below it.
            let headline = diag.headline();
            let short_msg: String = headline.chars().take(72).collect();
            let short_msg = if headline.chars().count() > 72 || diag.has_more_lines() {
                format!("{short_msg}…")
            } else {
                short_msg
            };
            // Fit what is left of the row, rather than running off the edge.
            //
            // The message never had a right-edge rule and was already cut mid-word
            // by the clip; stepping around the pill spends more of the same room,
            // so it now elides deliberately and keeps the ellipsis the 72-char cap
            // above already uses as the "there is more" signal.
            //
            // Deliberately NOT the clamp `show_inlay_hint` uses below: that one
            // right-ALIGNS its text against the clip edge, which is right for a
            // short ghost type and wrong here — it would drag a 450 px message
            // hundreds of pixels left, over the code of the line it annotates.
            if let Some(fitted) = fit_to_width(&short_msg, clip.right() - 4.0 - msg_x, char_w) {
                painter.text(
                    egui::pos2(msg_x, sy_mid),
                    egui::Align2::LEFT_CENTER,
                    &fitted,
                    msg_font(),
                    msg_color,
                );
            }
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
                            // `ARROW_SQUARE_OUT`, not a raw `↗`. Reading the
                            // bundled fonts' cmaps afterwards showed U+2197 is
                            // one of the ten arrows NotoEmoji does carry, so
                            // this one was probably rendering — the guard
                            // flagged it on the block rule, not on measured
                            // tofu. Kept as phosphor anyway: the standing rule
                            // is icons in UI text, whatever the font happens to
                            // cover today.
                            egui::RichText::new(format!(
                                "[{c}]  open docs {}",
                                egui_phosphor::regular::ARROW_SQUARE_OUT
                            ))
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

/// Draw a single inferred-type inlay hint as dim ghost text just past the END
/// of the line at `eol_idx` (the char index of that line's last character in
/// `display_code`). Positioned through the galley so it tracks scrolling, and
/// clipped to the visible editor area. Purely visual — the text is NOT part of
/// the document (accepting it is handled separately, by the caller's Tab
/// binding).
///
/// It sits after the line rather than inline after the binding name because an
/// overlay can't reflow the real text: an inline hint painted over the ` =
/// initializer …`, so it read as garbage (`parser:=Parser…`).
pub fn show_inlay_hint(
    ui: &egui::Ui,
    galley_pos: egui::Pos2,
    text_clip_rect: egui::Rect,
    galley: &egui::text::Galley,
    eol_idx: usize,
    label: &str,
    font_size: f32,
) {
    let painter = ui.painter().with_clip_rect(text_clip_rect);
    let loc = galley.pos_from_cursor(egui::text::CCursor::new(eol_idx));
    let y_top = galley_pos.y + loc.min.y;
    let y_bot = galley_pos.y + loc.max.y;
    // Skip when scrolled out of the visible editor.
    if y_bot < text_clip_rect.top() || y_top > text_clip_rect.bottom() {
        return;
    }
    // A gap past the line's end, mirroring the inline-diagnostic messages.
    // rust-analyzer's type-hint label already includes the leading `: `
    // (renderColons default); render it verbatim, dimmed like an editor hint.
    let font = egui::FontId::monospace(font_size);
    let color = egui::Color32::from_rgb(150, 165, 180);
    // Clamp X so the hint stays on-screen even when the line is long enough that
    // its end scrolls past the right edge — otherwise the hint would be clipped
    // and appear to be missing. Measure the label width and keep it inside the
    // visible editor, right-aligned against the edge when the line-end is far.
    let text_w = painter
        .layout_no_wrap(label.to_owned(), font.clone(), color)
        .size()
        .x;
    let eol_x = galley_pos.x + loc.max.x + 16.0;
    let max_x = (text_clip_rect.right() - text_w - 4.0).max(text_clip_rect.left());
    let x = eol_x.min(max_x);
    let y_mid = (y_top + y_bot) * 0.5;
    painter.text(
        egui::pos2(x, y_mid),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::{PILL_GAP, fit_to_width, inline_message_x};

    /// No pill on the line: the message keeps the position it always had. This
    /// is the common case — most lines carry no "N refs" indicator at all.
    #[test]
    fn a_line_without_a_pill_is_left_where_it_was() {
        assert_eq!(inline_message_x(300.0, None), 300.0);
    }

    /// The reported bug: the pill starts at end-of-line + 14 and the message at
    /// + 16, so the message was painted through it. It now clears the pill's
    /// real right edge by the requested gap.
    #[test]
    fn a_message_clears_the_pill_by_the_full_gap() {
        let pill_right = 352.0;
        let x = inline_message_x(300.0, Some(pill_right));
        assert_eq!(x, pill_right + PILL_GAP);
        assert!(x - pill_right >= 10.0, "at least 10px, as asked");
    }

    /// `max`, not `pill_right + gap` outright. A "1 ref" pill can end LEFT of
    /// where the message would have started on its own, and moving the message
    /// backwards to hug it would be a new bug wearing the old one's clothes.
    #[test]
    fn a_short_pill_never_drags_the_message_backwards() {
        assert_eq!(inline_message_x(500.0, Some(120.0)), 500.0);
    }

    #[test]
    fn a_message_that_fits_is_not_touched() {
        assert_eq!(
            fit_to_width("never used", 400.0, 6.0).as_deref(),
            Some("never used")
        );
    }

    /// Cut to the room that is left, keeping the ellipsis the 72-char cap
    /// already uses — so a width-elided message reads as truncated, not broken.
    #[test]
    fn a_message_too_wide_is_elided_to_what_fits() {
        let long = "fields `normal`, `night`, and `max` are never read";
        let out = fit_to_width(long, 60.0, 6.0).expect("10 chars is plenty of room");
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
        assert!(long.starts_with(&out[..out.len() - '…'.len_utf8()]));
    }

    /// A sliver of room says nothing worth the pixels; the hover tooltip and the
    /// error list still carry the whole message.
    #[test]
    fn too_little_room_draws_nothing_rather_than_a_stub() {
        assert_eq!(fit_to_width("mismatched types", 30.0, 6.0), None);
    }

    /// A degenerate font measurement must not divide by zero or silently blank
    /// every message in the editor.
    #[test]
    fn a_zero_width_measurement_falls_back_to_the_whole_text() {
        assert_eq!(
            fit_to_width("mismatched types", 100.0, 0.0).as_deref(),
            Some("mismatched types")
        );
    }

    /// Counted in CHARACTERS: rustc quotes identifiers, and a byte cut would
    /// panic in the middle of one the user named in Romanian.
    #[test]
    fn a_non_ascii_message_survives_the_cut() {
        let msg = "cannot find value `măsurători` in this scope";
        let out = fit_to_width(msg, 90.0, 6.0).expect("15 chars fit");
        assert_eq!(out.chars().count(), 15);
    }

    /// Negative room (the pill pushed the message past the clip edge) must be
    /// treated as no room, not as a huge one via a wrapped cast.
    #[test]
    fn no_room_at_all_is_not_mistaken_for_unlimited_room() {
        assert_eq!(fit_to_width("mismatched types", -200.0, 6.0), None);
    }
}
