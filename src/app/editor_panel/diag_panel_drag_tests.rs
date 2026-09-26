//! The bottom panel never lets the editor above it spill over its top edge.
//!
//! egui 0.36 clamps a `Panel::bottom` whose content came out taller than its
//! `exact_size` by moving the panel's TOP edge down, not by clipping the
//! bottom. That edge is what the editor above is laid out against, and the
//! editor is drawn after the panel, so the extra height became editor: its
//! last lines and its scrollbar were painted across the handle and the tab
//! row, over the More and collapse buttons. Dragging the panel open from its
//! collapsed bar did exactly that - the handle flips the flag mid-frame, and
//! the tab content was laid out inside the collapsed bar's height.

use crate::app::{AppIde, BuildPanelTab};
use eframe::egui;

struct Bench {
    ctx: egui::Context,
    app: AppIde,
    pass: u64,
    /// What one frame looked like.
    seen: Seen,
}

#[derive(Default, Clone, Copy, Debug)]
struct Seen {
    /// The top edge the panel reports.
    top: f32,
    /// The top of the panel's own drag handle.
    handle_top: f32,
    /// The bottom of the space left above the panel - the editor's.
    editor_bottom: f32,
}

impl Bench {
    fn new(collapsed: bool) -> Self {
        let ctx = egui::Context::default();
        let mut app = AppIde::new(
            &eframe::CreationContext::_new_kittest(ctx.clone()),
            None,
            None,
        );
        app.diag_collapsed = collapsed;
        app.diag_panel_height = 200.0;
        let mut b = Self {
            ctx,
            app,
            pass: 0,
            seen: Seen::default(),
        };
        for _ in 0..3 {
            b.step(vec![]);
        }
        b
    }

    fn step(&mut self, events: Vec<egui::Event>) {
        self.pass += 1;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 900.0),
            )),
            time: Some(self.pass as f64 / 30.0),
            predicted_dt: 1.0 / 30.0,
            events,
            ..Default::default()
        };
        let (app, mut seen) = (&mut self.app, None);
        let _ = crate::headless::run_ui(&self.ctx, input, |ui| {
            let mut rewritten = false;
            let top = app.show_editor_diag_panel(ui, &mut rewritten);
            let handle = ui
                .ctx()
                .read_response(egui::Id::new("diag_panel_resize"))
                .expect("the panel drew its handle");
            seen = Some(Seen {
                top: top.expect("the panel reports its top edge"),
                handle_top: handle.rect.top(),
                editor_bottom: ui.available_rect_before_wrap().bottom(),
            });
        });
        self.seen = seen.expect("the frame ran");
    }

    /// The editor ends where the panel begins, above its handle.
    fn assert_editor_stays_above(&self, what: &str) {
        let s = self.seen;
        assert!(
            s.top <= s.handle_top && s.editor_bottom <= s.handle_top,
            "{what}: the editor reaches past the panel's handle: {s:?}"
        );
    }

    /// Press the handle, then move the pointer by `dy` in `steps` frames,
    /// checking every frame; releases at the end.
    fn drag_handle(&mut self, dy: f32, steps: usize) {
        let start = egui::pos2(800.0, self.seen.handle_top + 3.0);
        self.step(vec![egui::Event::PointerMoved(start), button(start, true)]);
        self.assert_editor_stays_above("press");
        let mut at = start;
        for k in 0..steps {
            at.y += dy / steps as f32;
            self.step(vec![egui::Event::PointerMoved(at)]);
            self.assert_editor_stays_above(&format!("drag frame {k}"));
        }
        self.step(vec![button(at, false)]);
        self.assert_editor_stays_above("release");
    }
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[test]
fn dragging_the_handle_up_grows_the_panel() {
    let mut b = Bench::new(false);
    let before = b.app.diag_panel_height;
    b.drag_handle(-40.0, 4);
    let grown = b.app.diag_panel_height - before;
    assert!(
        (30.0..=50.0).contains(&grown),
        "the panel followed the 40 pt drag: grew {grown}"
    );
}

/// The user's report: pulling the collapsed bar open painted the editor over
/// the tab row while the drag lasted.
#[test]
fn dragging_the_collapsed_bar_open_keeps_the_editor_above_it() {
    let mut b = Bench::new(true);
    b.assert_editor_stays_above("collapsed at rest");
    b.drag_handle(-60.0, 6);
    assert!(!b.app.diag_collapsed, "the drag opened the panel");
}

/// Shrinking to the smallest size - and on, into a collapse - never lets the
/// editor through either.
#[test]
fn dragging_the_panel_down_to_a_collapse_keeps_the_editor_above_it() {
    let mut b = Bench::new(false);
    b.drag_handle(400.0, 20);
    assert!(
        b.app.diag_collapsed,
        "pulled past its smallest size, it collapsed"
    );
}

/// Every tab's content at the panel's smallest height: whatever a tab asks
/// for, the panel keeps its top edge.
#[test]
fn every_tab_at_the_smallest_height_keeps_the_editor_above_it() {
    let tabs = [
        BuildPanelTab::RustAnalyzer,
        BuildPanelTab::Cargo,
        BuildPanelTab::Dfu,
        BuildPanelTab::Rtt,
        BuildPanelTab::Debug,
        BuildPanelTab::Serial,
        BuildPanelTab::Clippy,
        BuildPanelTab::Profile,
        BuildPanelTab::Terminal,
        BuildPanelTab::Activity,
        BuildPanelTab::Git,
        BuildPanelTab::RequiredTools,
    ];
    let mut b = Bench::new(false);
    for tab in tabs {
        b.app.build_tab = tab;
        b.app.diag_panel_height = 0.0; // clamped up to the smallest height
        for _ in 0..3 {
            b.step(vec![]);
        }
        b.assert_editor_stays_above(&format!("{tab:?}"));
    }
}
