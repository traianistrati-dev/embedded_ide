//! Throttled activity spinner.
//!
//! `egui::Spinner` requests a repaint EVERY frame ("because it is animated"),
//! so any long-lived spinner — the status bar's Saving/Checking/Flashing, the
//! RA status strip, a running terminal command — kept the whole app repainting
//! at 60+ FPS for its entire duration. Each of those frames re-runs the full
//! editor pipeline, so a 20-second post-save check burned 20 seconds of
//! sustained CPU. This spinner draws the same arc but animates at ~10 FPS via
//! `request_repaint_after`, cutting that idle-animation cost ~6-fold.

use eframe::egui;

/// How often the spinner advances. 100 ms ≈ 10 FPS — plenty for a 13 px arc.
const STEP: std::time::Duration = std::time::Duration::from_millis(100);

/// Draw a spinner of `size` px that repaints at ~10 FPS instead of every
/// frame. Drop-in replacement for `ui.add(egui::Spinner::new().size(size))`.
pub(crate) fn throttled_spinner(ui: &mut egui::Ui, size: f32) {
    throttled_spinner_stroked(ui, size, 3.0);
}

/// Same spinner with an explicit arc thickness — a 3 px stroke looks like a
/// hairline on the big project-loading spinner.
pub(crate) fn throttled_spinner_stroked(ui: &mut egui::Ui, size: f32, thickness: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    ui.ctx().request_repaint_after(STEP);

    // Quantize the clock to the repaint cadence so the drawn frame is stable
    // between scheduled repaints (repaints triggered by OTHER events don't
    // advance the arc mid-step).
    let time = (ui.input(|i| i.time) / STEP.as_secs_f64()).floor() * STEP.as_secs_f64();

    // Same arc geometry as egui's Spinner.
    let color = ui.visuals().strong_text_color();
    let radius = size / 2.0 - 2.0;
    let n_points = 20;
    let start_angle = time * std::f64::consts::TAU;
    let end_angle = start_angle + 240f64.to_radians() * time.sin();
    let points: Vec<egui::Pos2> = (0..=n_points)
        .map(|i| {
            let angle = egui::lerp(start_angle..=end_angle, i as f64 / n_points as f64);
            let (sin, cos) = angle.sin_cos();
            rect.center() + radius * egui::vec2(cos as f32, sin as f32)
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(thickness, color),
    ));
}
