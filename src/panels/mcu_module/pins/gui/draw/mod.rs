//! High-level pin rendering orchestration.
//! Coordinates text, shapes, layout, and interaction for pin visualization.

pub mod layout;
pub mod shapes;
pub mod text;

use super::super::logic::pin::Pin;
use super::listeners;
use eframe::egui;

/// A selected pin is called out in WHITE — text and border alike — so it reads
/// as the focused item from across the diagram.
const SELECTION_COLOR: egui::Color32 = egui::Color32::WHITE;
/// Its name is drawn 15 % larger than an idle pin's, and its border twice as
/// thick, on top of the colour. The SAME scale grows every text of a selected
/// element attached to the chip — a module box's title / summary / preview /
/// fields, and a single pin's rename field beside its arrow.
pub const SELECTED_TEXT_SCALE: f32 = 1.15;
const STROKE_W: f32 = 2.0;
const SELECTED_STROKE_W: f32 = STROKE_W * 2.0;

/// `(text colour, font size, border width)` for a pin in either state.
fn selection_style(pin: &Pin, is_selected: bool) -> (egui::Color32, f32, f32) {
    if is_selected {
        (
            SELECTION_COLOR,
            super::super::logic::pin::PIN_FONT_SIZE * SELECTED_TEXT_SCALE,
            SELECTED_STROKE_W,
        )
    } else {
        (
            pin.get_text_color(),
            super::super::logic::pin::PIN_FONT_SIZE,
            STROKE_W,
        )
    }
}

impl Pin {
    /// Draws pin on the right side. Returns (rect, clicked).
    pub fn draw_right(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        height: f32,
        width: f32,
        ui: Option<&egui::Ui>,
        is_selected: bool,
    ) -> (egui::Rect, bool) {
        let rect = layout::calc_rect_right(x, y, height, width);
        shapes::draw_rect_filled(painter, rect, 0.0, self.get_background_color());

        let clicked = ui.map_or(false, |ui| {
            listeners::listen_on_rect(self, painter, ui, rect, is_selected)
        });

        let (text_color, font_size, stroke_w) = selection_style(self, is_selected);
        text::draw_horizontal_text_colored(
            self,
            painter,
            layout::text_position_horizontal(rect),
            text_color,
            font_size,
            // The label runs along the stub from `left + 2`, so that is the room
            // it has before it spills past the pin.
            rect.width() - 4.0,
        );

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR, stroke_w);
        }

        (rect, clicked)
    }

    /// Draws the pin as a BALL of a grid package (WLCSP / BGA) — a filled circle
    /// inside the chip body, with the name across it. Returns `clicked`.
    ///
    /// Circles, not the edge stub's rectangle, because that is how every ballout
    /// drawing shows them, and because a pad under the die has no edge to stick
    /// out of. Interaction is the shared [`listeners::listen_on_rect`] contract
    /// (same id scheme, same hover/selected colours) with the fill re-drawn as a
    /// circle on top.
    pub fn draw_ball(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        ui: Option<&egui::Ui>,
        is_selected: bool,
    ) -> bool {
        let radius = rect.width().min(rect.height()) / 2.0;
        // The rect fill this paints is immediately covered by the circle below;
        // what we want from it is the interaction + the hover/selected colour.
        let clicked = ui.map_or(false, |ui| {
            listeners::listen_on_rect(self, painter, ui, rect, is_selected)
        });
        let hovered = ui.is_some_and(|ui| ui.rect_contains_pointer(rect) && !self.reserved);
        let fill = if is_selected {
            egui::Color32::from_rgb(60, 60, 80)
        } else if hovered {
            egui::Color32::DARK_GRAY
        } else {
            self.get_background_color()
        };
        // Repaint the body over the square the listener filled, so the ball
        // reads as round at every state.
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(45, 45, 55));
        painter.circle_filled(rect.center(), radius, fill);

        let (text_color, font_size, stroke_w) = selection_style(self, is_selected);
        let font = egui::FontId::monospace(font_size);
        let label = text::fit(painter, &self.name, font.clone(), radius * 2.0 - 4.0);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font,
            text_color,
        );
        if is_selected {
            painter.circle_stroke(
                rect.center(),
                radius,
                egui::Stroke::new(stroke_w, SELECTION_COLOR),
            );
        }
        clicked
    }

    /// Draws pin on the left side. Returns (rect, clicked).
    pub fn draw_left(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        height: f32,
        width: f32,
        ui: Option<&egui::Ui>,
        is_selected: bool,
    ) -> (egui::Rect, bool) {
        let rect = layout::calc_rect_left(x, y, height, width);
        shapes::draw_rect_filled(painter, rect, 0.0, self.get_background_color());

        let clicked = ui.map_or(false, |ui| {
            listeners::listen_on_rect(self, painter, ui, rect, is_selected)
        });

        let (text_color, font_size, stroke_w) = selection_style(self, is_selected);
        text::draw_horizontal_text_colored(
            self,
            painter,
            layout::text_position_horizontal(rect),
            text_color,
            font_size,
            // The label runs along the stub from `left + 2`, so that is the room
            // it has before it spills past the pin.
            rect.width() - 4.0,
        );

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR, stroke_w);
        }

        (rect, clicked)
    }

    /// Draws pin on the top side. Returns (rect, clicked).
    pub fn draw_top(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        height: f32,
        width: f32,
        ui: Option<&egui::Ui>,
        is_selected: bool,
    ) -> (egui::Rect, bool) {
        let rect = layout::calc_rect_top(x, y, height, width);
        shapes::draw_rect_filled(painter, rect, 0.0, self.get_background_color());

        let clicked = ui.map_or(false, |ui| {
            listeners::listen_on_rect(self, painter, ui, rect, is_selected)
        });

        let (text_color, font_size, stroke_w) = selection_style(self, is_selected);
        text::draw_vertical_text_colored(
            self,
            painter,
            layout::text_position_vertical(rect),
            text_color,
            font_size,
            // Rotated −90°, the label runs UP the stub from `bottom - 4`.
            rect.height() - 6.0,
        );

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR, stroke_w);
        }

        (rect, clicked)
    }

    /// Draws pin on the bottom side. Returns (rect, clicked).
    pub fn draw_bottom(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        height: f32,
        width: f32,
        ui: Option<&egui::Ui>,
        is_selected: bool,
    ) -> (egui::Rect, bool) {
        let rect = layout::calc_rect_bottom(x, y, height, width);
        shapes::draw_rect_filled(painter, rect, 0.0, self.get_background_color());

        let clicked = ui.map_or(false, |ui| {
            listeners::listen_on_rect(self, painter, ui, rect, is_selected)
        });

        let (text_color, font_size, stroke_w) = selection_style(self, is_selected);
        text::draw_vertical_text_colored(
            self,
            painter,
            layout::text_position_vertical(rect),
            text_color,
            font_size,
            // Rotated −90°, the label runs UP the stub from `bottom - 4`.
            rect.height() - 6.0,
        );

        if is_selected {
            shapes::draw_rect_stroke(painter, rect, 0.0, SELECTION_COLOR, stroke_w);
        }

        (rect, clicked)
    }
}
