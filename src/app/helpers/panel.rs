//! A `Panel` of an exact size that keeps the edge its neighbours lean on.
//!
//! egui 0.36 clamps a panel whose content came out bigger than its
//! `exact_size` by moving the edge that faces the REST of the `Ui` - for a
//! bottom panel, its top - instead of letting the content spill past the far
//! edge the way 0.34 did. The space left for the neighbour is measured from
//! that edge, so the neighbour grew into the panel, and since it is drawn
//! after the panel it painted over it: the editor's last lines and scrollbar
//! across the diagnostics panel's handle and tab row, over the More and
//! collapse buttons.

use eframe::egui;

/// `panel.exact_size(size).show(ui, add_contents)`, except that content bigger
/// than `size` is clipped at the panel's far edge and never moves the edge
/// facing the rest of `ui`.
pub fn show_exact<R>(
    panel: egui::Panel,
    size: f32,
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    panel.exact_size(size).show(ui, |ui| {
        // The content gets the panel's box as its own Ui, which does NOT grow
        // the frame around it; the box itself is what the frame measures.
        let rect = ui.max_rect();
        let mut content = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let inner = add_contents(&mut content);
        ui.advance_cursor_after_rect(rect);
        inner
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: egui::Vec2 = egui::vec2(800.0, 600.0);

    /// A 40 pt bottom panel given 100 pt of content, and what is left above.
    fn overfull_bottom_panel(guarded: bool) -> (egui::Rect, f32) {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut seen = (egui::Rect::NOTHING, 0.0);
        for _ in 0..2 {
            let _ = crate::headless::run_ui(&ctx, input(), |ui| {
                let panel = egui::Panel::bottom("overfull");
                let tall = |ui: &mut egui::Ui| {
                    ui.allocate_exact_size(egui::vec2(10.0, 100.0), egui::Sense::hover());
                };
                let shown = if guarded {
                    show_exact(panel, 40.0, ui, tall)
                } else {
                    panel.exact_size(40.0).show(ui, tall)
                };
                seen = (
                    shown.response.rect,
                    ui.available_rect_before_wrap().bottom(),
                );
            });
        }
        seen
    }

    #[test]
    fn content_taller_than_the_panel_leaves_its_top_edge_where_it_was() {
        let (panel, above) = overfull_bottom_panel(true);
        assert_eq!(
            panel.top(),
            SCREEN.y - 40.0,
            "the panel's own box: {panel:?}"
        );
        assert!(
            above <= panel.top(),
            "the space above ends at the panel: {above} vs {panel:?}"
        );
    }

    /// What the helper is for: egui on its own moves the top edge down.
    #[test]
    fn egui_alone_moves_the_top_edge_of_an_overfull_bottom_panel() {
        let (panel, above) = overfull_bottom_panel(false);
        assert!(
            panel.top() > SCREEN.y - 40.0 && above > SCREEN.y - 40.0,
            "egui 0.36 no longer moves the edge - is the helper still needed? \
             {panel:?}, space above ends at {above}"
        );
    }

    /// Every exact-size panel goes through [`show_exact`]: a panel sized with
    /// `exact_size` directly is one tall row away from being painted over.
    #[test]
    fn every_exact_size_panel_is_shown_through_the_helper() {
        fn scan(dir: &std::path::Path, direct: &mut Vec<String>, helped: &mut usize) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    scan(&p, direct, helped);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs")
                    || p.ends_with("helpers/panel.rs")
                    || p.ends_with("helpers\\panel.rs")
                {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                for (i, line) in text.lines().enumerate() {
                    if line.trim_start().starts_with("//") {
                        continue;
                    }
                    if line.contains(".exact_size(") {
                        direct.push(format!("{}:{}", p.display(), i + 1));
                    }
                    if line.contains("helpers::panel::show_exact(") {
                        *helped += 1;
                    }
                }
            }
        }
        let (mut direct, mut helped) = (Vec::new(), 0);
        scan(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut direct,
            &mut helped,
        );
        assert!(helped > 0, "the scanner found the helper's callers");
        assert!(
            direct.is_empty(),
            "size these panels through crate::app::helpers::panel::show_exact:\n{}",
            direct.join("\n")
        );
    }
}
