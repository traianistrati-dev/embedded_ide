//! The `Id` a `ScrollArea` keeps its state under, for code that reads or
//! writes that state from OUTSIDE the scroll area (caret-follow, go-to-line,
//! fold correction, the Flow list's reveal).

use eframe::egui;

/// The `Id` under which a `ScrollArea` shown on `ui` with `.id_salt(salt)`
/// stores its `scroll_area::State`.
///
/// This mirrors egui's own derivation, and must keep mirroring it: egui 0.35
/// changed it (`IdSalt`, #8184). The old spelling still compiles and still
/// returns an `Id` - just one nobody stores anything under, so `State::load`
/// answers `None` and every scroll written through it is silently dropped.
/// The test below renders a real `ScrollArea` and compares, so the next change
/// to egui's derivation fails here instead of in the editor.
pub fn scroll_area_id(ui: &egui::Ui, salt: impl std::hash::Hash) -> egui::Id {
    ui.make_persistent_id(egui::Id::new(salt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_id_egui_stores_the_state_under() {
        let ctx = egui::Context::default();
        let mut seen = None;
        for _ in 0..2 {
            let _ = crate::headless::run_ui(&ctx, Default::default(), |ui| {
                let salt = format!("{}_outer_scroll", "main");
                let out = egui::ScrollArea::vertical()
                    .id_salt(&salt)
                    .show(ui, |ui| ui.label("x"));
                seen = Some((out.id, scroll_area_id(ui, &salt)));
            });
        }
        let (real, ours) = seen.expect("the frame ran");
        assert_eq!(ours, real);
        assert!(
            egui::scroll_area::State::load(&ctx, ours).is_some(),
            "the state is really there"
        );
    }
}
