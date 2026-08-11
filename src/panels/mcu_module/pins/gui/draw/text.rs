//! Text rendering primitives for pins (vertical and horizontal).

use super::super::super::logic::pin::Pin;
use eframe::egui;

/// What a cut name ends with. ASCII on purpose — a real ellipsis renders as a
/// tofu box in the pin font.
const CUT: &str = "..";

/// Fit `name` into `max_w`, cutting the tail and appending [`CUT`] when it
/// doesn't fit (`PC15-OSC32_OUT` → `PC15-OSC..`).
///
/// The pin stub is a fixed length, but a chip's names are not: an ST `PC15-
/// OSC32_OUT` runs well past its own stub and over the neighbouring pins. The
/// full name is always readable in the function list's header, so cutting the
/// diagram label loses nothing.
///
/// The font is monospace, so one measurement of the whole string gives the
/// per-character width and the cut is a single division — no per-prefix probing.
pub fn fit(painter: &egui::Painter, name: &str, font: egui::FontId, max_w: f32) -> String {
    let chars = name.chars().count();
    if chars == 0 || max_w <= 0.0 {
        return String::new();
    }
    let full_w = painter
        .layout_no_wrap(name.to_owned(), font, egui::Color32::WHITE)
        .size()
        .x;
    if full_w <= max_w {
        return name.to_owned();
    }
    let char_w = full_w / chars as f32;
    let fits = (max_w / char_w).floor() as usize;
    // Room for the ".." — if not even that fits, show what little we can.
    let keep = fits.saturating_sub(CUT.len());
    if keep == 0 {
        return name.chars().take(fits).collect();
    }
    let mut s: String = name.chars().take(keep).collect();
    s.push_str(CUT);
    s
}

/// Render pin name vertically (rotated -90°) with custom color, cut to `max_len`
/// (the space along the pin, i.e. the stub's height).
pub fn draw_vertical_text_colored(
    pin: &Pin,
    painter: &egui::Painter,
    pos: egui::Pos2,
    color: egui::Color32,
    font_size: f32,
    max_len: f32,
) {
    let font = egui::FontId::monospace(font_size);
    let galley =
        painter.layout_no_wrap(fit(painter, &pin.name, font.clone(), max_len), font, color);

    let text_shape = egui::epaint::TextShape {
        pos,
        galley,
        underline: egui::epaint::Stroke::NONE,
        override_text_color: Some(color),
        angle: -std::f32::consts::FRAC_PI_2,
        fallback_color: color,
        opacity_factor: 1.0,
    };

    painter.add(egui::Shape::Text(text_shape));
}

/// Render pin name horizontally with custom color, cut to `max_len` (the space
/// along the pin, i.e. the stub's width).
pub fn draw_horizontal_text_colored(
    pin: &Pin,
    painter: &egui::Painter,
    pos: egui::Pos2,
    color: egui::Color32,
    font_size: f32,
    max_len: f32,
) {
    let font = egui::FontId::monospace(font_size);
    painter.text(
        pos,
        egui::Align2::LEFT_CENTER,
        fit(painter, &pin.name, font.clone(), max_len),
        font,
        color,
    );
}
