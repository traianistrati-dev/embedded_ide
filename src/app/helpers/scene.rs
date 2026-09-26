//! `egui::Scene::show` that survives the first frame.
//!
//! Every canvas here (the pins, the Board, the clock diagram) starts with no
//! view yet: `Rect::NOTHING`, which `Scene` itself treats as "fit the content
//! after this frame". But on THAT frame it builds its transform from the empty
//! rect, and the scene's `Ui` ends up with a NaN min rect. egui 0.34 let that
//! through; egui 0.36 has `debug_assert!(!rect.any_nan())` in `create_widget`,
//! so the debug build - the one in daily use - panicked the first time any of
//! the three was drawn. The pins canvas is drawn as soon as a project with a
//! chip is restored, so the IDE crashed on start-up, over and over.

use eframe::egui;

/// [`egui::Scene::show`], given a finite view on a frame that has none yet.
///
/// A missing view (`Rect::NOTHING`, or any rect with no area) is replaced by
/// the available space for this one frame, then by the content's bounds - the
/// same fit `Scene` would have made from the empty rect, reached without the
/// NaN frame in between.
pub fn show<R>(
    scene: egui::Scene,
    ui: &mut egui::Ui,
    scene_rect: &mut egui::Rect,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let fresh = !has_area(*scene_rect);
    if fresh {
        let avail = ui.available_size_before_wrap();
        let size = if avail.is_finite() && avail.x > 0.0 && avail.y > 0.0 {
            avail
        } else {
            egui::vec2(100.0, 100.0)
        };
        *scene_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    }
    let mut content = egui::Rect::NOTHING;
    let shown = scene.show(ui, scene_rect, |ui| {
        let r = add_contents(ui);
        content = ui.min_rect();
        r
    });
    if fresh && has_area(content) {
        *scene_rect = content;
    }
    shown
}

/// A rect a `Scene` can build a transform from.
fn has_area(r: egui::Rect) -> bool {
    r.is_finite() && r.width() > 0.0 && r.height() > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame that crashed the debug build: a Scene with no view yet.
    #[test]
    fn a_scene_with_no_view_yet_draws_and_then_fits_its_content() {
        let ctx = egui::Context::default();
        let mut view = egui::Rect::NOTHING;
        let content = egui::Rect::from_min_size(egui::pos2(40.0, 30.0), egui::vec2(200.0, 80.0));
        for _ in 0..2 {
            let _ = crate::headless::run_ui(&ctx, Default::default(), |ui| {
                show(egui::Scene::new(), ui, &mut view, |ui| {
                    ui.put(content, egui::Label::new("pins"));
                });
            });
        }
        assert!(has_area(view), "a finite view: {view:?}");
        assert!(
            view.contains_rect(content.shrink(1.0)),
            "it fits the content: {view:?}"
        );
    }

    /// Every canvas goes through [`show`]: a `Scene` shown directly passes on
    /// egui 0.34, and panics a debug build on 0.36 the first time it is drawn.
    #[test]
    fn every_scene_is_shown_through_the_helper() {
        fn scan(dir: &std::path::Path, scenes: &mut Vec<String>, helped: &mut usize) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    scan(&p, scenes, helped);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs")
                    || p.ends_with("helpers/scene.rs")
                    || p.ends_with("helpers\\scene.rs")
                {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                for (i, line) in text.lines().enumerate() {
                    if line.trim_start().starts_with("//") {
                        continue;
                    }
                    if line.contains("Scene::new()") {
                        scenes.push(format!("{}:{}", p.display(), i + 1));
                    }
                    if line.contains("helpers::scene::show(") {
                        *helped += 1;
                    }
                }
            }
        }
        let (mut scenes, mut helped) = (Vec::new(), 0);
        scan(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut scenes,
            &mut helped,
        );
        assert!(!scenes.is_empty(), "the scanner found the canvases");
        assert_eq!(
            scenes.len(),
            helped,
            "show every Scene through crate::app::helpers::scene::show:\n{}",
            scenes.join("\n")
        );
    }
}
