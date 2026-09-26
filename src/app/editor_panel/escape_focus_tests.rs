//! Escape never ejects the caret from the code editor.
//!
//! In the code editor Escape means "dismiss the popup" or "drop the extra
//! carets", so the editor keeps its focus through it (`EDITOR_KEYS`). Both
//! editors are covered: the project's own Rust editor, and the stock
//! `egui_code_editor` one the config files (Cargo.toml, .gitignore) use, which
//! locks its focus without Escape and needs the filter set from outside. When
//! focus was lost, everything typed after the Escape was silently dropped.

use crate::app::{AppIde, EditorSlot, ProjectFileId};
use eframe::egui;

const RUST: &str = "fn main() {\n    let value = 1;\n}\n";
const TOML: &str = "[package]\nname = \"p\"\n\n[dependencies]\nse\n";

struct Editor {
    ctx: egui::Context,
    app: AppIde,
    idx: usize,
    pass: u64,
}

impl Editor {
    fn open(path: &str) -> Self {
        let ctx = egui::Context::default();
        let mut app = AppIde::new(
            &eframe::CreationContext::_new_kittest(ctx.clone()),
            None,
            None,
        );
        app.project_tree.user_src_files.clear();
        app.project_tree
            .user_src_files
            .push(("src/main.rs".into(), RUST.into()));
        app.project_tree
            .user_src_files
            .push(("Cargo.toml".into(), TOML.into()));
        let idx = app
            .project_tree
            .user_src_files
            .iter()
            .position(|(p, _)| p == path)
            .expect("the file is in the project");
        app.selected_file = ProjectFileId::UserFile(idx);
        let mut ed = Self {
            ctx,
            app,
            idx,
            pass: 0,
        };
        for _ in 0..4 {
            ed.step(vec![]);
        }
        ed
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
            focused: true,
            events,
            ..Default::default()
        };
        let (idx, app) = (self.idx, &mut self.app);
        let _ = crate::headless::run_ui(&self.ctx, input, |ui| {
            let file = ProjectFileId::UserFile(idx);
            let (path, code) = app.project_tree.user_src_files[idx].clone();
            let syntax = file.syntax(&path);
            let manifest = file.is_cargo_manifest(&path);
            app.show_code_view(
                ui,
                EditorSlot::Main,
                file,
                code,
                &syntax,
                manifest,
                path.ends_with(".rs"),
                None,
            );
        });
    }

    fn id(&self) -> egui::Id {
        self.app.ed.editor_widget_id.expect("the editor was drawn")
    }

    fn focused(&self) -> bool {
        self.ctx.memory(|m| m.focused()) == Some(self.id())
    }

    fn text(&self) -> String {
        self.app.project_tree.user_src_files[self.idx].1.clone()
    }

    /// Click into the editor and let egui settle its focus filter.
    fn click(&mut self) {
        let r = self.ctx.read_response(self.id()).expect("a response").rect;
        let p = r.left_top() + egui::vec2(60.0, 8.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.step(vec![egui::Event::PointerMoved(p), button(true)]);
        self.step(vec![button(false)]);
        for _ in 0..3 {
            self.step(vec![]);
        }
        assert!(self.focused(), "the click focused the editor");
    }

    fn press(&mut self, key: egui::Key) {
        let event = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        self.step(vec![event(true), event(false)]);
    }
}

fn escape_keeps_the_keyboard(path: &str) {
    let mut ed = Editor::open(path);
    ed.click();
    ed.press(egui::Key::Escape);
    for _ in 0..3 {
        ed.step(vec![]);
    }
    assert!(ed.focused(), "{path}: Escape took the focus away");
    ed.step(vec![egui::Event::Text("Q".into())]);
    ed.step(vec![]);
    assert!(
        ed.text().contains('Q'),
        "{path}: a key typed after Escape was lost"
    );
}

#[test]
fn escape_in_a_config_file_keeps_the_keyboard() {
    escape_keeps_the_keyboard("Cargo.toml");
}

#[test]
fn escape_in_a_rust_file_keeps_the_keyboard() {
    escape_keeps_the_keyboard("src/main.rs");
}
