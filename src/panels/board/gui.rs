//! The Board canvas: one frame per chip, dragged by the user, with its modules
//! and devices arranged inside by [`super::layout`].
//!
//! Drawn inside an `egui::Scene`, so every coordinate here is a scene
//! coordinate - including `drag_delta`, which egui already scales by the zoom.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use egui_phosphor::regular as ph;

use super::layout::{self, FrameLayout, PAD, PILL_H};
use super::snapshot::ChipView;
use crate::panels::mcu_module::mcu::gui::modules::module_color;

/// One frame to draw.
pub struct Frame<'a> {
    pub view: &'a ChipView,
    /// Top-left, in scene coordinates.
    pub pos: Pos2,
    /// The chip open in this window.
    pub active: bool,
}

/// What the user did on the canvas this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A frame is being dragged: its new top-left.
    Moved { dir: String, pos: (f32, f32) },
    /// A drag ended - the moment to write the positions down.
    DragEnded,
    /// Open this chip in this window.
    Open(String),
    /// Take this chip out of the system (its folder stays).
    Remove(String),
}

const FRAME_FILL: Color32 = Color32::from_rgb(30, 34, 42);
const FRAME_STROKE: Color32 = Color32::from_gray(78);
const ACTIVE_STROKE: Color32 = Color32::from_rgb(110, 170, 240);
const DEVICE_STROKE: Color32 = Color32::from_rgb(150, 138, 230);
const PROBLEM: Color32 = Color32::from_rgb(230, 170, 80);

/// Each frame's size, for placing the ones without a spot. The same layout
/// the frames are drawn with, so a placed frame and its neighbour agree on
/// how tall it is.
pub fn sizes(views: &[ChipView]) -> Vec<Vec2> {
    views.iter().map(|v| layout::frame_layout(v).size).collect()
}

/// Draw every frame. Returns what the user did, and the union of the frames
/// (the content to fit the view to).
pub fn draw(ui: &mut egui::Ui, frames: &[Frame<'_>]) -> (Vec<Event>, Rect) {
    let mut events = Vec::new();
    let mut bounds = Rect::NOTHING;
    for f in frames {
        let l = layout::frame_layout(f.view);
        let rect = Rect::from_min_size(f.pos, l.size);
        bounds = bounds.union(rect);
        draw_frame(ui, f, &l, rect);
        interact(ui, f, rect, &mut events);
    }
    (events, bounds)
}

fn draw_frame(ui: &egui::Ui, f: &Frame<'_>, l: &FrameLayout, rect: Rect) {
    let painter = ui.painter();
    let origin = rect.min.to_vec2();
    let (stroke_w, stroke_c) = if f.active {
        (1.5_f32, ACTIVE_STROKE)
    } else {
        (1.0_f32, FRAME_STROKE)
    };
    painter.rect(
        rect,
        10.0,
        FRAME_FILL,
        Stroke::new(stroke_w, stroke_c),
        egui::StrokeKind::Inside,
    );

    // Header: the chip, then folder and runtime.
    let title = if f.view.chip.is_empty() {
        "Unknown chip"
    } else {
        f.view.chip.as_str()
    };
    let text_w = rect.width() - 2.0 * PAD - if f.active { 44.0 } else { 0.0 };
    let title = one_line(ui, title, 14.0, Color32::WHITE, text_w);
    painter.galley(rect.min + egui::vec2(PAD, 10.0), title, Color32::WHITE);
    let sub = one_line(
        ui,
        &f.view.subtitle(),
        11.0,
        Color32::GRAY,
        rect.width() - 2.0 * PAD,
    );
    painter.galley(rect.min + egui::vec2(PAD, 29.0), sub, Color32::GRAY);
    if f.active {
        painter.text(
            egui::pos2(rect.right() - PAD, rect.top() + 12.0),
            egui::Align2::RIGHT_TOP,
            "open",
            egui::FontId::proportional(11.0),
            ACTIVE_STROKE,
        );
    }

    // Wires first, so the pills sit on top of their ends.
    for (m, pts) in &l.wires {
        let color = module_color(f.view.modules[*m].kind, f.view.modules[*m].instance);
        let pts: Vec<Pos2> = pts.iter().map(|p| *p + origin).collect();
        painter.add(egui::Shape::line(pts, Stroke::new(1.5_f32, color)));
    }
    for (m, r) in f.view.modules.iter().zip(&l.modules) {
        let color = module_color(m.kind, m.instance);
        // Square corners for a custom module, as on the Pins tab: it is the
        // user's, not a peripheral's.
        let radius = if m.kind.is_custom() { 0.0 } else { 6.0 };
        pill(ui, r.translate(origin), &m.name, color, radius);
    }
    for (d, r) in f.view.devices.iter().zip(&l.devices) {
        pill(
            ui,
            r.translate(origin),
            &d.name,
            DEVICE_STROKE,
            PILL_H / 2.0,
        );
    }

    if let Some(note) = l.note {
        let (text, color) = match &f.view.problem {
            Some(p) => (format!("{}  {p}", ph::WARNING), PROBLEM),
            None => (
                "No modules or devices yet".to_owned(),
                Color32::from_gray(120),
            ),
        };
        let galley = one_line(ui, &text, 12.0, color, note.width());
        let at = note.translate(origin).left_center() - egui::vec2(0.0, galley.size().y / 2.0);
        painter.galley(at, galley, color);
    }
}

fn interact(ui: &egui::Ui, f: &Frame<'_>, rect: Rect, events: &mut Vec<Event>) {
    let dir = &f.view.dir;
    let resp = ui.interact(
        rect,
        egui::Id::new(("board_frame", dir)),
        Sense::click_and_drag(),
    );
    // The left button only: a middle or right drag (the canvas's pan button,
    // the menu button) must not carry a frame off.
    if resp.dragged_by(egui::PointerButton::Primary) {
        let to = rect.min + resp.drag_delta();
        events.push(Event::Moved {
            dir: dir.clone(),
            pos: (to.x, to.y),
        });
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        events.push(Event::DragEnded);
    }
    if resp.double_clicked() && !f.active {
        events.push(Event::Open(dir.clone()));
    }
    let resp = if f.active {
        resp.on_hover_text("The chip open in this window. Drag to move.")
    } else {
        resp.on_hover_text("Double-click to open this chip. Drag to move.")
    };
    resp.context_menu(|ui| {
        let open = ui.add_enabled(
            !f.active,
            egui::Button::new(format!("{}  Open in this window", ph::FOLDER_OPEN)),
        );
        if open.on_disabled_hover_text("Already open").clicked() {
            events.push(Event::Open(dir.clone()));
            ui.close();
        }
        ui.separator();
        if ui
            .button(format!("{}  Remove from system", ph::MINUS_CIRCLE))
            .on_hover_text("The Board forgets this chip. Its folder and files stay.")
            .clicked()
        {
            events.push(Event::Remove(dir.clone()));
            ui.close();
        }
    });
}

/// A module or device: a rounded box with its name.
fn pill(ui: &egui::Ui, r: Rect, name: &str, color: Color32, radius: f32) {
    let fill = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 46);
    ui.painter().rect(
        r,
        radius,
        fill,
        Stroke::new(1.0_f32, color),
        egui::StrokeKind::Inside,
    );
    let text = Color32::from_gray(232);
    let galley = one_line(ui, name, 12.0, text, r.width() - 12.0);
    let at = r.center() - galley.size() / 2.0;
    ui.painter().galley(at, galley, text);
}

/// Text cut to one line of `max_w`, ending in `…` when it does not fit - a
/// long device name must not run out of its box.
fn one_line(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    color: Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        egui::FontId::proportional(size),
        color,
    );
    job.wrap = egui::text::TextWrapping {
        max_width: max_w.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.fonts_mut(|f| f.layout_job(job))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(pass: usize, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0))),
            time: Some(pass as f64 / 60.0),
            events,
            ..Default::default()
        }
    }

    fn button(at: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn run(
        ctx: &egui::Context,
        pass: usize,
        events: Vec<egui::Event>,
        frames: &[Frame<'_>],
    ) -> Vec<Event> {
        let mut out = Vec::new();
        let _ = ctx.run_ui(input(pass, events), |ui| out = draw(ui, frames).0);
        out
    }

    /// A drag reports where the frame goes, and its end once - the moment the
    /// Board writes positions to disk.
    #[test]
    fn dragging_a_frame_moves_it_and_ends_once() {
        let view = ChipView::broken("stm32_main", "gone");
        let frames = [Frame {
            view: &view,
            pos: egui::pos2(100.0, 100.0),
            active: false,
        }];
        let ctx = egui::Context::default();
        let at = egui::pos2(150.0, 120.0);
        assert!(run(&ctx, 0, vec![egui::Event::PointerMoved(at)], &frames).is_empty());
        run(&ctx, 1, vec![button(at, true)], &frames);
        let mut last = None;
        let mut ended = 0;
        for step in 1..=4 {
            let p = at + egui::vec2(10.0, 7.5) * step as f32;
            for e in run(&ctx, 1 + step, vec![egui::Event::PointerMoved(p)], &frames) {
                match e {
                    Event::Moved { dir, pos } => {
                        assert_eq!(dir, "stm32_main");
                        last = Some(pos);
                    }
                    Event::DragEnded => ended += 1,
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        // The frames here never move, so each report is the frame's spot plus
        // that frame's pointer step.
        assert_eq!(last, Some((110.0, 107.5)));
        let end = run(
            &ctx,
            6,
            vec![button(at + egui::vec2(40.0, 30.0), false)],
            &frames,
        );
        ended += end.iter().filter(|e| **e == Event::DragEnded).count();
        assert_eq!(ended, 1);
        assert!(!end.iter().any(|e| matches!(e, Event::Open(_))));
    }

    /// Only the left button carries a frame: the middle one is the canvas's
    /// pan button, the right one opens the menu.
    #[test]
    fn only_a_left_drag_moves_a_frame() {
        let view = ChipView::broken("stm32_main", "gone");
        let frames = [Frame {
            view: &view,
            pos: egui::pos2(100.0, 100.0),
            active: false,
        }];
        for b in [egui::PointerButton::Middle, egui::PointerButton::Secondary] {
            let ctx = egui::Context::default();
            let at = egui::pos2(150.0, 120.0);
            let press = |pressed: bool, pos: Pos2| egui::Event::PointerButton {
                pos,
                button: b,
                pressed,
                modifiers: egui::Modifiers::default(),
            };
            let mut events = run(&ctx, 0, vec![egui::Event::PointerMoved(at)], &frames);
            events.extend(run(&ctx, 1, vec![press(true, at)], &frames));
            for step in 1..=4 {
                let p = at + egui::vec2(10.0, 7.5) * step as f32;
                events.extend(run(
                    &ctx,
                    1 + step,
                    vec![egui::Event::PointerMoved(p)],
                    &frames,
                ));
            }
            let end = at + egui::vec2(40.0, 30.0);
            events.extend(run(&ctx, 6, vec![press(false, end)], &frames));
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, Event::Moved { .. } | Event::DragEnded)),
                "{b:?}: {events:?}"
            );
        }
    }

    /// Double-clicking opens a chip - but not the one already open.
    #[test]
    fn a_double_click_opens_any_chip_but_the_open_one() {
        let view = ChipView::broken("esp32_radio", "gone");
        for active in [false, true] {
            let frames = [Frame {
                view: &view,
                pos: egui::pos2(100.0, 100.0),
                active,
            }];
            let ctx = egui::Context::default();
            let at = egui::pos2(150.0, 120.0);
            let mut events = Vec::new();
            run(&ctx, 0, vec![egui::Event::PointerMoved(at)], &frames);
            for (pass, pressed) in [(1, true), (2, false), (3, true), (4, false)] {
                events.extend(run(&ctx, pass, vec![button(at, pressed)], &frames));
            }
            let opened = events.contains(&Event::Open("esp32_radio".into()));
            assert_eq!(opened, !active, "active = {active}: {events:?}");
        }
    }
}
