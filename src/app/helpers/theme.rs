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
    vis.window_stroke = egui::Stroke::new(1.0_f32, sep);
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
    vis.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, sep);
    vis.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, fg0);
    vis.widgets.noninteractive.corner_radius = cr4;

    // ── Widgets: inactive ────────────────────────────────────────────────────
    vis.widgets.inactive.bg_fill = bg2;
    vis.widgets.inactive.weak_bg_fill = bg2;
    vis.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, sep);
    vis.widgets.inactive.fg_stroke = egui::Stroke::new(1.5_f32, fg0);
    vis.widgets.inactive.corner_radius = cr4;

    // ── Widgets: hovered ─────────────────────────────────────────────────────
    vis.widgets.hovered.bg_fill = bg3;
    vis.widgets.hovered.weak_bg_fill = bg3;
    vis.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(0x60, 0x6a, 0x80));
    vis.widgets.hovered.fg_stroke = egui::Stroke::new(1.5_f32, fg1);
    vis.widgets.hovered.corner_radius = cr4;

    // ── Widgets: active (pressed) ────────────────────────────────────────────
    vis.widgets.active.bg_fill = bg4;
    vis.widgets.active.weak_bg_fill = bg4;
    vis.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, acc);
    vis.widgets.active.fg_stroke = egui::Stroke::new(2.0_f32, fg1);
    vis.widgets.active.corner_radius = cr4;

    // ── Widgets: open (combo-box open state) ─────────────────────────────────
    vis.widgets.open.bg_fill = bg4;
    vis.widgets.open.weak_bg_fill = bg4;
    vis.widgets.open.bg_stroke = egui::Stroke::new(1.0_f32, acc);
    vis.widgets.open.fg_stroke = egui::Stroke::new(1.5_f32, fg1);
    vis.widgets.open.corner_radius = cr4;

    // ── Text / selection ─────────────────────────────────────────────────────
    vis.override_text_color = Some(fg0);
    vis.selection.bg_fill = acc_bg;
    vis.selection.stroke = egui::Stroke::new(1.0_f32, acc);

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

#[cfg(test)]
mod bar_width_probe {
    use super::apply_dark_theme;
    use eframe::egui;
    use egui_phosphor::regular as ph;

    fn ctx_with_app_style() -> egui::Context {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        ctx.set_fonts(fonts);
        apply_dark_theme(&ctx);
        // Fonts/style settle after a warm-up frame.
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }

    /// Lay the Virtual-modules bar out exactly as `mcu_panel.rs` does, with and
    /// without the two controls this diff adds, and report the widths.
    fn measure(screen_w: f32, with_devices: bool, can_undo: bool) -> (f32, f32) {
        let ctx = ctx_with_app_style();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(screen_w, 900.0),
        ));
        let avail_in_bar = std::cell::Cell::new(0.0_f32);
        let row_w = std::cell::Cell::new(0.0_f32);
        let _ = ctx.run(input, |ctx| {
            // The MCU zone is a CentralPanel...
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    // ...holding the bottom vmodules Panel.
                    egui::Panel::bottom("vmodules_panel")
                        .resizable(false)
                        .show_separator_line(false)
                        .show_inside(ui, |ui| {
                            let r = ui.horizontal(|ui| {
                                avail_in_bar.set(ui.available_width());
                                // 1. collapse caret
                                let _ = ui.button(
                                    egui::RichText::new(ph::CARET_DOWN)
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(160, 185, 215)),
                                );
                                // 2. "Virtual modules:"
                                ui.label(
                                    egui::RichText::new("Virtual modules:")
                                        .size(12.0)
                                        .color(egui::Color32::from_rgb(150, 150, 160)),
                                );
                                // 3. "+ Add module v" menu button
                                ui.menu_button(
                                    egui::RichText::new(format!(
                                        "{} Add module {}",
                                        ph::PLUS,
                                        ph::CARET_DOWN
                                    ))
                                    .size(11.0),
                                    |_ui| {},
                                );
                                if with_devices {
                                    // 4/5/6: the diff's additions
                                    ui.add_space(6.0);
                                    ui.label(
                                        egui::RichText::new("Devices:")
                                            .size(12.0)
                                            .color(egui::Color32::from_rgb(150, 150, 160)),
                                    );
                                    let _ = ui.button(
                                        egui::RichText::new(format!("{} Device", ph::PLUS))
                                            .size(11.0),
                                    );
                                }
                                if can_undo {
                                    ui.separator();
                                    let _ = ui.button(
                                        egui::RichText::new(format!(
                                            "{} Undo",
                                            ph::ARROW_COUNTER_CLOCKWISE
                                        ))
                                        .size(11.0),
                                    );
                                }
                            });
                            row_w.set(r.response.rect.width());
                        });
                });
            });
        });
        (avail_in_bar.get(), row_w.get())
    }

    #[test]
    fn probe() {
        for w in [420.0_f32, 600.0, 2000.0] {
            for (tag, dev) in [("before(no Devices)", false), ("after(+Devices)", true)] {
                let (avail, row) = measure(w, dev, true);
                println!(
                    "screen_w={w:>6.0}  {tag:<20} avail_in_bar={avail:>7.2}  row_used={row:>7.2}  overflow={:>7.2}",
                    row - avail
                );
            }
        }
        // Also: no-undo case
        let (a, r) = measure(420.0, true, false);
        println!("screen_w=   420  after, can_undo=FALSE  avail={a:.2} row={r:.2} overflow={:.2}", r - a);
    }
}
