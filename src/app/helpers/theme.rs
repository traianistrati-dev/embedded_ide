//! Dark IDE theme configuration.

use eframe::egui;

/// Apply a consistent dark IDE theme to the entire application.
///
/// Palette (One Dark / VSCode Dark+):
///   bg0  #1e2127  — darkest  (extreme bg, gutter)
///   bg1  #21252b  — panel / window fill
///   bg2  #282c34  — widget normal fill
///   bg3  #2c313a  — widget hovered
///   bg4  #3e4451  — widget active / selected
///   fg0  #abb2bf  — primary text
///   fg1  #d0d7e0  — strong / heading text
///   acc  #528bff  — selection / focus accent
///   sep  #3a3f4b  — borders / separators
pub fn apply_dark_theme(ctx: &egui::Context) {
    use egui::Color32;

    // Pin theme to Dark so an OS appearance change can't flip it to light.
    ctx.set_theme(egui::Theme::Dark);

    // ── Start from egui's built-in dark base ─────────────────────────────────
    let mut vis = egui::Visuals::dark();

    // ── Palette ──────────────────────────────────────────────────────────────
    let bg0 = Color32::from_rgb(0x1e, 0x21, 0x27);
    let bg1 = Color32::from_rgb(0x21, 0x25, 0x2b);
    let bg2 = Color32::from_rgb(0x28, 0x2c, 0x34);
    let bg3 = Color32::from_rgb(0x2c, 0x31, 0x3a);
    let bg4 = Color32::from_rgb(0x3e, 0x44, 0x51);
    let sep = Color32::from_rgb(0x3a, 0x3f, 0x4b);
    let fg0 = Color32::from_rgb(0xab, 0xb2, 0xbf);
    let fg1 = Color32::from_rgb(0xd0, 0xd7, 0xe0);
    let acc = Color32::from_rgb(0x52, 0x8b, 0xff);
    let acc_bg = Color32::from_rgba_unmultiplied(0x52, 0x8b, 0xff, 50);
    let cr4 = egui::CornerRadius::same(4); // CornerRadius uses u8

    // ── Backgrounds ──────────────────────────────────────────────────────────
    vis.window_fill = bg1;
    vis.panel_fill = bg1;
    vis.faint_bg_color = bg0;
    vis.extreme_bg_color = bg0;
    vis.code_bg_color = Color32::from_rgb(0x1a, 0x1d, 0x23);

    // ── Window chrome ────────────────────────────────────────────────────────
    vis.window_stroke = egui::Stroke::new(1.0, sep);
    vis.window_corner_radius = egui::CornerRadius::same(6);
    vis.menu_corner_radius = egui::CornerRadius::same(5);
    vis.window_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(130),
    };
    vis.popup_shadow = egui::Shadow {
        offset: [0, 2],
        blur: 8,
        spread: 0,
        color: Color32::from_black_alpha(100),
    };

    // ── Widgets: noninteractive ───────────────────────────────────────────────
    vis.widgets.noninteractive.bg_fill = bg1;
    vis.widgets.noninteractive.weak_bg_fill = bg1;
    vis.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, sep);
    vis.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, fg0);
    vis.widgets.noninteractive.corner_radius = cr4;

    // ── Widgets: inactive ────────────────────────────────────────────────────
    vis.widgets.inactive.bg_fill = bg2;
    vis.widgets.inactive.weak_bg_fill = bg2;
    vis.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, sep);
    vis.widgets.inactive.fg_stroke = egui::Stroke::new(1.5, fg0);
    vis.widgets.inactive.corner_radius = cr4;

    // ── Widgets: hovered ─────────────────────────────────────────────────────
    vis.widgets.hovered.bg_fill = bg3;
    vis.widgets.hovered.weak_bg_fill = bg3;
    vis.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x60, 0x6a, 0x80));
    vis.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, fg1);
    vis.widgets.hovered.corner_radius = cr4;

    // ── Widgets: active (pressed) ────────────────────────────────────────────
    vis.widgets.active.bg_fill = bg4;
    vis.widgets.active.weak_bg_fill = bg4;
    vis.widgets.active.bg_stroke = egui::Stroke::new(1.0, acc);
    vis.widgets.active.fg_stroke = egui::Stroke::new(2.0, fg1);
    vis.widgets.active.corner_radius = cr4;

    // ── Widgets: open (combo-box open state) ─────────────────────────────────
    vis.widgets.open.bg_fill = bg4;
    vis.widgets.open.weak_bg_fill = bg4;
    vis.widgets.open.bg_stroke = egui::Stroke::new(1.0, acc);
    vis.widgets.open.fg_stroke = egui::Stroke::new(1.5, fg1);
    vis.widgets.open.corner_radius = cr4;

    // ── Text / selection ─────────────────────────────────────────────────────
    vis.override_text_color = Some(fg0);
    vis.selection.bg_fill = acc_bg;
    vis.selection.stroke = egui::Stroke::new(1.0, acc);

    // ── Misc ─────────────────────────────────────────────────────────────────
    vis.hyperlink_color = Color32::from_rgb(0x61, 0xaf, 0xef);
    vis.warn_fg_color = Color32::from_rgb(0xe5, 0xc0, 0x7b);
    vis.error_fg_color = Color32::from_rgb(0xe0, 0x6c, 0x75);

    ctx.set_visuals(vis);

    // ── Global style tweaks ──────────────────────────────────────────────────
    let mut style = (*ctx.global_style()).clone();

    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    style.spacing.window_margin = egui::Margin::same(8);
    style.spacing.indent = 14.0;

    use egui::TextStyle;
    style.text_styles.insert(
        TextStyle::Heading,
        egui::FontId::new(14.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Body,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Button,
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        egui::FontId::new(13.0, egui::FontFamily::Monospace),
    );

    ctx.set_global_style(style);
}
