//! Right-click context menu for the code editor.
//!
//! Lists every editor command together with its keyboard shortcut. Clicking a
//! row returns an [`EditorAction`]; the caller (`mod.rs`) maps it onto the same
//! key flags the shortcut would set, so both paths share one implementation.

use eframe::egui;
use egui_phosphor::regular as ph;

/// A command chosen from the editor right-click menu.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum EditorAction {
    DeleteLine,
    DuplicateLine,
    Comment,
    MoveUp,
    MoveDown,
    Format,
    Rename,
    GoToDef,
    Completion,
    Copy,
    SelectAll,
}

/// Render the editor context menu. `is_rs` enables the rust-analyzer-backed
/// items (format / rename / go-to-definition); `is_cargo` relabels completion
/// for Cargo.toml. Returns the clicked action, if any.
pub(super) fn editor_menu(ui: &mut egui::Ui, is_rs: bool, is_cargo: bool) -> Option<EditorAction> {
    ui.set_min_width(248.0);
    let mut chosen: Option<EditorAction> = None;

    let mut item =
        |ui: &mut egui::Ui, icon: &str, label: &str, shortcut: &str, action: EditorAction| {
            let btn = egui::Button::new(egui::RichText::new(format!("{icon}  {label}")).size(12.0))
                .shortcut_text(egui::RichText::new(shortcut).size(11.0))
                .min_size(egui::vec2(ui.available_width(), 0.0));
            if ui.add(btn).clicked() {
                chosen = Some(action);
                ui.close();
            }
        };

    item(ui, ph::SCISSORS, "Delete line", "Ctrl+X", EditorAction::DeleteLine);
    item(ui, ph::COPY_SIMPLE, "Duplicate line", "Ctrl+D", EditorAction::DuplicateLine);
    item(ui, ph::CODE, "Toggle comment", "Ctrl+/", EditorAction::Comment);
    item(ui, ph::ARROW_UP, "Move line up", "Ctrl+Up", EditorAction::MoveUp);
    item(ui, ph::ARROW_DOWN, "Move line down", "Ctrl+Down", EditorAction::MoveDown);

    if is_rs {
        ui.separator();
        item(ui, ph::TEXT_INDENT, "Format code", "Ctrl+Shift+F", EditorAction::Format);
        item(ui, ph::PENCIL_SIMPLE, "Rename symbol", "Ctrl+R", EditorAction::Rename);
        item(ui, ph::ARROW_SQUARE_OUT, "Go to definition", "F12", EditorAction::GoToDef);
    }
    if is_rs || is_cargo {
        ui.separator();
        let label = if is_cargo { "Dependency completion" } else { "Code suggestions" };
        item(ui, ph::LIST_BULLETS, label, "Ctrl+Space", EditorAction::Completion);
    }

    ui.separator();
    item(ui, ph::COPY, "Copy", "Ctrl+C", EditorAction::Copy);
    item(ui, ph::SELECTION_ALL, "Select all", "Ctrl+A", EditorAction::SelectAll);

    chosen
}
