//! Function selection panel — scrollable list of pin functions with buttons.

use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
use eframe::egui;

/// State returned from panel rendering.
pub struct PanelState {
    pub new_function: Option<(usize, PinFunction)>,
    pub toggle_info: Option<PinFunction>,
}

/// Render the header (pin name/number) and separator.
pub fn draw_header(
    painter: &egui::Painter,
    chip_rect: egui::Rect,
    sep_y: &mut f32,
    num: usize,
    pin_name: &str,
) {
    let header_pos = chip_rect.center_top() + egui::vec2(0.0, 14.0);
    painter.text(
        header_pos,
        egui::Align2::CENTER_CENTER,
        format!("Pin {}  ·  {}", num, pin_name),
        egui::FontId::proportional(13.0),
        egui::Color32::WHITE,
    );

    *sep_y = header_pos.y + 14.0;
    painter.line_segment(
        [
            egui::pos2(chip_rect.left() + 8.0, *sep_y),
            egui::pos2(chip_rect.right() - 8.0, *sep_y),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 100, 120)),
    );
}

/// Render the function button list with scrolling and interaction detection.
pub fn render_function_buttons(
    painter: &egui::Painter,
    list_painter: &egui::Painter,
    chip_rect: egui::Rect,
    funcs: &[PinFunction],
    selected_func: &PinFunction,
    pin_num: usize,
    fn_scroll_offset: f32,
    show_info: &Option<PinFunction>,
    ui: &mut egui::Ui,
) -> PanelState {
    let info_btn_w = 22.0;
    let gap = 4.0;
    let btn_h = 28.0;
    let item_h = btn_h + 6.0;
    let btn_x = chip_rect.left() + 12.0;
    let sep_y = chip_rect.top() + 50.0; // approx header height
    let content_top = sep_y + 12.0;
    let content_bottom = chip_rect.bottom() - 8.0;
    let sb_w = 4.0;
    let sb_gap = 3.0;
    let btn_w = chip_rect.width() - 24.0 - info_btn_w - gap - sb_w - sb_gap;

    let mut state = PanelState {
        new_function: None,
        toggle_info: None,
    };

    let mut btn_y = content_top - fn_scroll_offset;

    for (i, func) in funcs.iter().enumerate() {
        let btn_rect =
            egui::Rect::from_min_size(egui::pos2(btn_x, btn_y), egui::vec2(btn_w, btn_h));
        let info_rect = egui::Rect::from_min_size(
            egui::pos2(btn_x + btn_w + gap, btn_y),
            egui::vec2(info_btn_w, btn_h),
        );

        let visible: bool = btn_rect.bottom() > content_top && btn_rect.top() < content_bottom;

        let is_sel: bool = func == selected_func;
        let bg: egui::Color32 = if is_sel {
            func.color()
        } else {
            egui::Color32::from_rgb(65, 65, 80)
        };

        list_painter.rect_filled(btn_rect, 5.0, bg);
        list_painter.text(
            btn_rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{}  —  {}", func.short_label(), func.label()),
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );

        // ⓘ button
        let info_open = show_info.as_ref() == Some(func);
        let info_bg: egui::Color32 = if info_open {
            egui::Color32::from_rgb(80, 120, 200)
        } else {
            egui::Color32::from_rgb(55, 55, 75)
        };
        list_painter.rect_filled(info_rect, 5.0, info_bg);
        let ic = info_rect.center();
        let ir = 7.5_f32;
        list_painter.circle_stroke(ic, ir, egui::Stroke::new(1.5, egui::Color32::WHITE));
        list_painter.circle_filled(egui::pos2(ic.x, ic.y - 2.5), 1.3, egui::Color32::WHITE);
        list_painter.line_segment(
            [egui::pos2(ic.x, ic.y - 0.5), egui::pos2(ic.x, ic.y + 4.0)],
            egui::Stroke::new(1.8, egui::Color32::WHITE),
        );

        // Hover / click
        if visible {
            let btn_response = ui.interact(
                btn_rect,
                ui.id().with(("fn_btn", pin_num, i)),
                egui::Sense::click(),
            );
            if btn_response.hovered() {
                list_painter.rect_stroke(
                    btn_rect,
                    5.0,
                    egui::Stroke::new(1.5, egui::Color32::WHITE),
                    egui::StrokeKind::Middle,
                );
            }
            if btn_response.clicked() {
                let next = if func == selected_func {
                    PinFunction::Unset
                } else {
                    func.clone()
                };
                state.new_function = Some((pin_num, next));
            }

            let info_response = ui.interact(
                info_rect,
                ui.id().with(("info_btn", pin_num, i)),
                egui::Sense::click(),
            );
            if info_response.hovered() {
                list_painter.rect_stroke(
                    info_rect,
                    5.0,
                    egui::Stroke::new(1.5, egui::Color32::WHITE),
                    egui::StrokeKind::Middle,
                );
            }
            if info_response.clicked() {
                state.toggle_info = Some(func.clone());
            }
        }

        btn_y += item_h;
    }

    state
}

/// Render the scrollbar track and thumb.
pub fn draw_scrollbar(
    painter: &egui::Painter,
    chip_rect: egui::Rect,
    max_scroll: f32,
    fn_scroll_offset: f32,
    content_top: f32,
    available_h: f32,
    total_h: f32,
) {
    if max_scroll > 0.0 {
        let sb_w = 4.0;
        let sb_x = chip_rect.right() - sb_w - 2.0;
        let track_h = available_h;
        let thumb_h = ((available_h / total_h) * track_h).max(16.0);
        let thumb_top = content_top + (fn_scroll_offset / max_scroll) * (track_h - thumb_h);
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(sb_x, thumb_top), egui::vec2(sb_w, thumb_h)),
            sb_w / 2.0,
            egui::Color32::from_rgba_premultiplied(180, 180, 210, 140),
        );
    }
}
