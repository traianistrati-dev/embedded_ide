//! Project tree GUI — file browser with create/rename/delete operations.

use crate::app::ProjectFileId;
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
use crate::{build, lsp};
use eframe::egui;
use egui_phosphor::regular as ph;
use std::collections::BTreeMap;

/// Tree node representing a file or folder.
#[derive(Clone, Debug)]
enum TreeNode {
    File(usize), // index into user_src_files
    Folder(BTreeMap<String, TreeNode>),
}

/// Drag-and-drop payload: the tree item being dragged onto a folder (drop
/// target). `Send + Sync + 'static` as egui requires.
#[derive(Clone)]
enum DraggedItem {
    /// A user file, by its `user_src_files` index.
    File(usize),
    /// A folder, by its `src/`-relative path (moves with all its contents).
    Folder(String),
}

/// The last path segment of a `src/`-relative path (the bare file/folder name).
fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// If `path` is an auto-generated folder that must NOT be moved, return a short
/// explanation; otherwise `None`. `pins/` and `pins/configs/` are recreated
/// every frame by the pin/peripheral sync, so moving them breaks codegen.
fn generated_folder_reason(path: &str) -> Option<&'static str> {
    match path {
        "pins" => Some("the `pins/` folder is auto-generated from your pin configuration"),
        "pins/configs" => Some("`pins/configs/` is auto-generated from the Virtual Modules (USART/SPI/I2C)"),
        _ => None,
    }
}

/// If `path` (relative to `src/`) is an auto-generated file that must NOT be
/// moved, return a short human explanation; otherwise `None`. These are rebuilt
/// each frame from the MCU / pin configuration (see
/// `ProjectTreeState::sync_pin_files` / `sync_config_files`), so moving one
/// would be silently undone or would break the generated module tree.
fn generated_file_reason(path: &str) -> Option<&'static str> {
    if path == "pins/mod.rs" || path == "pins/configs/mod.rs" {
        return Some("it's an auto-generated module file (rebuilt from your pin / peripheral configuration)");
    }
    if path.starts_with("pins/configs/") {
        return Some("it's an auto-generated peripheral init file — edit it via the MCU Configurator (Virtual Modules)");
    }
    // Generated pin files sit directly under pins/ as `pin<…>.rs`.
    if let Some(fname) = path.strip_prefix("pins/") {
        if !fname.contains('/') && fname.starts_with("pin") && fname.ends_with(".rs") {
            return Some("it's an auto-generated pin file (rebuilt from your pin configuration)");
        }
    }
    None
}

const TREE_NOTICE_ID: &str = "__tree_move_notice__";

/// Show a transient amber banner (`msg`) at the top of the tree for a few
/// seconds — used to explain why a drag-drop move was refused. Stored in egui
/// temp memory (with an expiry time) so it survives across frames without a
/// dedicated state field.
fn set_tree_notice(ctx: &egui::Context, msg: String) {
    let expiry = ctx.input(|i| i.time) + 6.0;
    ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new(TREE_NOTICE_ID), (msg, expiry)));
}

fn show_tree_notice(ui: &mut egui::Ui) {
    let id = egui::Id::new(TREE_NOTICE_ID);
    let Some((msg, expiry)) = ui.memory(|m| m.data.get_temp::<(String, f64)>(id)) else {
        return;
    };
    if ui.input(|i| i.time) >= expiry {
        ui.memory_mut(|m| m.data.remove::<(String, f64)>(id));
        return;
    }
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(70, 45, 20))
        .inner_margin(egui::Margin::same(5))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(ph::WARNING)
                        .size(12.0)
                        .color(egui::Color32::from_rgb(240, 190, 90)),
                );
                ui.label(
                    egui::RichText::new(msg)
                        .size(10.5)
                        .color(egui::Color32::from_rgb(235, 215, 165)),
                );
            });
        });
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(250));
}

/// Apply a drag-drop move of `item` into `target_folder` (`""` = src/ root),
/// dispatching to the file or folder mover. Shared guard: refuse dropping into
/// the auto-managed `pins/configs/` (files there are pruned by the sync).
fn apply_move(
    ui: &egui::Ui,
    item: &DraggedItem,
    target_folder: &str,
    user_src_files: &mut Vec<(String, String)>,
    user_src_folders: &mut Vec<String>,
    workspace_dir: &std::path::Path,
    save_needed: &mut bool,
) {
    if target_folder == "pins/configs" || target_folder.starts_with("pins/configs/") {
        set_tree_notice(
            ui.ctx(),
            "Can't move into `pins/configs/` — it's auto-managed by the MCU Configurator.".to_string(),
        );
        return;
    }
    match item {
        DraggedItem::File(idx) => {
            apply_file_move(ui, *idx, target_folder, user_src_files, workspace_dir, save_needed)
        }
        DraggedItem::Folder(src) => apply_folder_move(
            ui,
            src,
            target_folder,
            user_src_files,
            user_src_folders,
            workspace_dir,
            save_needed,
        ),
    }
}

/// Move user file `idx` into `target_folder`: refuse auto-generated files (with
/// notice), no-op when already there, refuse on name collision. Renames on disk
/// and updates the in-memory path.
fn apply_file_move(
    ui: &egui::Ui,
    idx: usize,
    target_folder: &str,
    user_src_files: &mut [(String, String)],
    workspace_dir: &std::path::Path,
    save_needed: &mut bool,
) {
    let Some((old_path, _)) = user_src_files.get(idx) else {
        return;
    };
    let old_path = old_path.clone();
    let fname = base_name(&old_path).to_string();

    if let Some(reason) = generated_file_reason(&old_path) {
        set_tree_notice(ui.ctx(), format!("Can't move `{fname}` — {reason}."));
        return;
    }

    let new_path = if target_folder.is_empty() {
        fname.clone()
    } else {
        format!("{target_folder}/{fname}")
    };
    if new_path == old_path {
        return; // dropped into its current folder — nothing to do
    }
    if user_src_files.iter().any(|(p, _)| p == &new_path) {
        set_tree_notice(
            ui.ctx(),
            format!("`{fname}` already exists in that folder — rename one first."),
        );
        return;
    }

    let old_dest = workspace_dir.join("src").join(&old_path);
    let new_dest = workspace_dir.join("src").join(&new_path);
    if let Some(parent) = new_dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::rename(&old_dest, &new_dest);
    user_src_files[idx].0 = new_path;
    *save_needed = true;
}

/// Move folder `src` (and everything under it) into `target_folder`: refuse
/// auto-generated folders, refuse moving a folder into itself/a descendant,
/// no-op when already there, refuse on collision. Renames on disk and rewrites
/// every affected folder + file path (mirrors the folder-rename logic).
fn apply_folder_move(
    ui: &egui::Ui,
    src: &str,
    target_folder: &str,
    user_src_files: &mut [(String, String)],
    user_src_folders: &mut [String],
    workspace_dir: &std::path::Path,
    save_needed: &mut bool,
) {
    let name = base_name(src).to_string();

    if let Some(reason) = generated_folder_reason(src) {
        set_tree_notice(ui.ctx(), format!("Can't move `{name}/` — {reason}."));
        return;
    }
    // Can't drop a folder into itself or one of its own descendants.
    if target_folder == src || target_folder.starts_with(&format!("{src}/")) {
        set_tree_notice(ui.ctx(), "Can't move a folder into itself.".to_string());
        return;
    }

    let new_path = if target_folder.is_empty() {
        name.clone()
    } else {
        format!("{target_folder}/{name}")
    };
    if new_path == *src {
        return; // already in that folder
    }
    let collides = user_src_folders.iter().any(|f| f == &new_path)
        || user_src_files.iter().any(|(p, _)| p == &new_path);
    if collides {
        set_tree_notice(
            ui.ctx(),
            format!("`{name}/` already exists in that folder — rename one first."),
        );
        return;
    }

    let old_dest = workspace_dir.join("src").join(src);
    let new_dest = workspace_dir.join("src").join(&new_path);
    if let Some(parent) = new_dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::rename(&old_dest, &new_dest);

    let old_prefix = format!("{src}/");
    for f in user_src_folders.iter_mut() {
        if *f == src {
            *f = new_path.clone();
        } else if let Some(rest) = f.strip_prefix(&old_prefix) {
            *f = format!("{new_path}/{rest}");
        }
    }
    for (p, _) in user_src_files.iter_mut() {
        if let Some(rest) = p.strip_prefix(&old_prefix) {
            *p = format!("{new_path}/{rest}");
        }
    }
    *save_needed = true;
}

/// egui id for "focus the inline new-item input on its first frame" (file / folder).
fn inline_focus_id(is_folder: bool) -> egui::Id {
    egui::Id::new(if is_folder {
        "__inline_new_folder_focus__"
    } else {
        "__inline_new_file_focus__"
    })
}

/// Arm the inline new-item input for `parent` (`""` = src/ root): set the
/// pending name + parent state and request focus next frame. Called from the
/// "New File" / "New Folder" context-menu entries.
fn begin_inline_new(
    ui: &egui::Ui,
    is_folder: bool,
    parent: &str,
    name_state: &mut Option<String>,
    parent_state: &mut Option<String>,
) {
    *name_state = Some(String::new());
    *parent_state = Some(parent.to_string());
    ui.memory_mut(|m| m.data.insert_temp(inline_focus_id(is_folder), true));
}

/// Render the inline "new file / new folder" name input as a tree row (at
/// `indent`) while its pending state targets `parent`. Enter creates the item
/// (in memory + on disk) and clears the state; Esc / focus-loss cancels. Only
/// one input is active at a time (single `name_state` Option). No-op when the
/// pending state doesn't target this `parent`.
#[allow(clippy::too_many_arguments)]
fn inline_new_item(
    ui: &mut egui::Ui,
    indent: f32,
    parent: &str,
    is_folder: bool,
    name_state: &mut Option<String>,
    parent_state: &mut Option<String>,
    user_src_files: &mut Vec<(String, String)>,
    user_src_folders: &mut Vec<String>,
    selected: &mut ProjectFileId,
    workspace_dir: &std::path::Path,
    save_needed: &mut bool,
) {
    if parent_state.as_deref() != Some(parent) || name_state.is_none() {
        return;
    }
    let mut create = false;
    let mut cancel = false;
    let focus_id = inline_focus_id(is_folder);
    ui.horizontal(|ui| {
        ui.add_space(indent);
        let icon = if is_folder { ph::FOLDER } else { ph::FILE };
        ui.label(
            egui::RichText::new(icon)
                .size(11.5)
                .color(egui::Color32::from_rgb(150, 180, 240)),
        );
        if let Some(name) = name_state.as_mut() {
            let resp = ui.add(
                egui::TextEdit::singleline(name)
                    .desired_width(ui.available_width())
                    .hint_text(if is_folder { "new folder" } else { "new_file.rs" }),
            );
            if ui.memory(|m| m.data.get_temp::<bool>(focus_id).unwrap_or(true)) {
                resp.request_focus();
                ui.memory_mut(|m| m.data.insert_temp(focus_id, false));
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                create = true;
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) || resp.lost_focus() {
                cancel = true;
            }
        }
    });

    if create {
        if let Some(name) = name_state.take() {
            let clean = name.trim().to_string();
            if !clean.is_empty() {
                let full = if parent.is_empty() {
                    clean.clone()
                } else {
                    format!("{parent}/{clean}")
                };
                let collides = user_src_folders.iter().any(|f| f == &full)
                    || user_src_files.iter().any(|(p, _)| p == &full);
                if collides {
                    set_tree_notice(ui.ctx(), format!("`{clean}` already exists here."));
                } else if is_folder {
                    let dest = workspace_dir.join("src").join(&full);
                    let _ = std::fs::create_dir_all(&dest);
                    user_src_folders.push(full);
                    *save_needed = true;
                } else {
                    let dest = workspace_dir.join("src").join(&full);
                    if let Some(p) = dest.parent() {
                        let _ = std::fs::create_dir_all(p);
                    }
                    let _ = std::fs::write(&dest, "// New file\n");
                    user_src_files.push((full, "// New file\n".to_string()));
                    *selected = ProjectFileId::UserFile(user_src_files.len() - 1);
                    *save_needed = true;
                }
            }
        }
        *parent_state = None;
    } else if cancel {
        *name_state = None;
        *parent_state = None;
    }
}

/// Build a hierarchical tree from user_src_files and user_src_folders.
fn build_tree(
    user_src_files: &[(String, String)],
    user_src_folders: &[String],
) -> BTreeMap<String, TreeNode> {
    let mut root: BTreeMap<String, TreeNode> = BTreeMap::new();

    // First, ensure all folders exist in the tree
    for folder in user_src_folders {
        insert_folder_path(&mut root, folder);
    }

    // Then, insert all files
    for (i, (path, _)) in user_src_files.iter().enumerate() {
        let parts: Vec<&str> = path.split('/').collect();
        if parts.is_empty() {
            continue;
        }
        insert_file_path(&mut root, &parts, i);
    }

    root
}

/// Helper: insert a folder path into the tree (recursive).
fn insert_folder_path(root: &mut BTreeMap<String, TreeNode>, path: &str) {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    insert_folder_path_recursive(root, &parts);
}

fn insert_folder_path_recursive(current: &mut BTreeMap<String, TreeNode>, parts: &[&str]) {
    if parts.is_empty() {
        return;
    }
    let part = parts[0];
    let rest = &parts[1..];
    let node = current
        .entry(part.to_string())
        .or_insert_with(|| TreeNode::Folder(BTreeMap::new()));
    if let TreeNode::Folder(children) = node {
        insert_folder_path_recursive(children, rest);
    }
}

/// Helper: insert a file path into the tree (recursive).
fn insert_file_path(root: &mut BTreeMap<String, TreeNode>, parts: &[&str], file_idx: usize) {
    if parts.is_empty() {
        return;
    }
    insert_file_path_recursive(root, parts, file_idx);
}

fn insert_file_path_recursive(
    current: &mut BTreeMap<String, TreeNode>,
    parts: &[&str],
    file_idx: usize,
) {
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        // Last part is the filename
        current.insert(parts[0].to_string(), TreeNode::File(file_idx));
    } else {
        // Navigate deeper
        let part = parts[0];
        let rest = &parts[1..];
        let node = current
            .entry(part.to_string())
            .or_insert_with(|| TreeNode::Folder(BTreeMap::new()));
        if let TreeNode::Folder(children) = node {
            insert_file_path_recursive(children, rest, file_idx);
        }
    }
}

/// Display the project tree panel (left side of the IDE).
pub fn show_project_tree(
    ui: &mut egui::Ui,
    pkg_name: &str,
    toolchain: &ToolchainKind,
    selected: &mut ProjectFileId,
    build_result: Option<&build::BuildResult>,
    lsp_state: Option<&lsp::LspState>,
    user_src_files: &mut Vec<(String, String)>,
    user_src_folders: &mut Vec<String>,
    new_src_name: &mut Option<String>,
    new_src_folder_name: &mut Option<String>,
    new_file_parent_folder: &mut Option<String>,
    new_folder_parent_folder: &mut Option<String>,
    new_file_in_folder: &mut Option<(String, String)>,
    renaming_file: &mut Option<(usize, String)>,
    renaming_folder: &mut Option<(String, String)>,
    workspace_dir: &std::path::Path,
    save_needed: &mut bool,
) {
    ui.label(
        egui::RichText::new(format!("package: {pkg_name}"))
            .size(12.0)
            .strong()
            .color(egui::Color32::LIGHT_YELLOW),
    );
    // Transient "can't move" banner from a refused drag-drop (auto-cleared).
    show_tree_notice(ui);
    ui.add_space(2.0);

    // While a tree item is being dragged, give the cursor the drag icon —
    // `Grabbing`, or `NoDrop` when the item is auto-generated (can't be moved).
    if let Some(payload) = egui::DragAndDrop::payload::<DraggedItem>(ui.ctx()) {
        let blocked = match &*payload {
            DraggedItem::File(idx) => user_src_files
                .get(*idx)
                .is_some_and(|(p, _)| generated_file_reason(p).is_some()),
            DraggedItem::Folder(p) => generated_folder_reason(p).is_some(),
        };
        ui.ctx().set_cursor_icon(if blocked {
            egui::CursorIcon::NoDrop
        } else {
            egui::CursorIcon::Grabbing
        });
    }

    // Collected during the tree render below; an item dragged onto a folder sets
    // `(dragged_item, target_folder_rel_to_src)` — applied after the tree closure
    // so it doesn't clash with the `&mut` borrows used for rendering.
    let mut move_request: Option<(DraggedItem, String)> = None;

    // .cargo/  — fixed folder, no context menu → dark-red + bold.
    egui::CollapsingHeader::new(
        egui::RichText::new(".cargo/")
            .size(11.5)
            .monospace()
            .strong()
            .color(egui::Color32::from_rgb(100, 50, 50)),
    )
    .default_open(true)
    .show(ui, |ui| {
        file_row(
            ui,
            8.0,
            "config.toml",
            ProjectFileId::CargoConfig,
            selected,
            build_result,
            lsp_state,
        );
    });

    // src/  — fixed root, cannot be renamed/deleted → muted-red + bold.
    let src_ch = egui::CollapsingHeader::new(
        egui::RichText::new("src/")
            .size(11.5)
            .monospace()
            .strong()
            .color(egui::Color32::from_rgb(200, 100, 100)),
    )
    .default_open(true)
    .show(ui, |ui| {
        file_row(
            ui,
            8.0,
            "main.rs",
            ProjectFileId::MainRs,
            selected,
            build_result,
            lsp_state,
        );

        // Inline "new file / new folder" input at the src/ root ("" parent),
        // rendered right under main.rs where the item will be added.
        inline_new_item(
            ui, 8.0, "", false, new_src_name, new_file_parent_folder,
            user_src_files, user_src_folders, selected, workspace_dir, save_needed,
        );
        inline_new_item(
            ui, 8.0, "", true, new_src_folder_name, new_folder_parent_folder,
            user_src_files, user_src_folders, selected, workspace_dir, save_needed,
        );

        // Build hierarchical tree from files and folders
        let tree = build_tree(user_src_files, user_src_folders);

        // Track deletions and renames to apply after rendering
        let mut to_delete: Option<usize> = None;
        let mut do_rename_file: Option<usize> = None;
        let mut cancel_rename_file = false;

        // Recursively render the tree
        render_tree_node(
            ui,
            &tree,
            user_src_files,
            user_src_folders,
            selected,
            8.0,
            renaming_file,
            &mut do_rename_file,
            &mut cancel_rename_file,
            &mut to_delete,
            renaming_folder,
            workspace_dir,
            save_needed,
            new_src_name,
            new_src_folder_name,
            new_file_parent_folder,
            new_folder_parent_folder,
            "", // parent path at root is empty (relative to src/)
            &mut move_request,
        );

        // Apply file deletion
        if let Some(idx) = to_delete {
            if *selected == ProjectFileId::UserFile(idx) {
                *selected = ProjectFileId::MainRs;
            }
            let dest = workspace_dir.join("src").join(&user_src_files[idx].0);
            let _ = std::fs::remove_file(&dest);
            user_src_files.remove(idx);
            *save_needed = true;
        }

        // Apply file rename
        if let Some(confirm_idx) = do_rename_file {
            if let Some((_, new_name)) = renaming_file.take() {
                let old_path = user_src_files[confirm_idx].0.clone();
                let clean = new_name.trim().to_string();
                if !clean.is_empty() {
                    let new_path = if let Some(slash) = old_path.rfind('/') {
                        format!("{}/{clean}", &old_path[..slash])
                    } else {
                        clean
                    };
                    if new_path != old_path && !user_src_files.iter().any(|(p, _)| p == &new_path) {
                        let old_dest = workspace_dir.join("src").join(&old_path);
                        let new_dest = workspace_dir.join("src").join(&new_path);
                        let _ = std::fs::rename(&old_dest, &new_dest);
                        user_src_files[confirm_idx].0 = new_path;
                        *save_needed = true;
                    }
                }
            }
        } else if cancel_rename_file {
            *renaming_file = None;
        }
    });

    // Dropping a dragged item on the `src/` header moves it to the src/ root.
    if src_ch.header_response.dnd_hover_payload::<DraggedItem>().is_some() {
        ui.painter().rect_stroke(
            src_ch.header_response.rect,
            3.0,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 170, 240)),
            egui::StrokeKind::Inside,
        );
    }
    if let Some(p) = src_ch.header_response.dnd_release_payload::<DraggedItem>() {
        move_request = Some(((*p).clone(), String::new()));
    }

    // Apply a drag-drop move now that the tree closure's borrows have ended.
    if let Some((item, target)) = move_request.take() {
        apply_move(
            ui,
            &item,
            &target,
            user_src_files,
            user_src_folders,
            workspace_dir,
            save_needed,
        );
    }

    src_ch.header_response.context_menu(|ui| {
        if ui
            .button(egui::RichText::new(format!("{} New File", ph::FILE_PLUS)).size(11.5))
            .clicked()
        {
            begin_inline_new(ui, false, "", new_src_name, new_file_parent_folder);
            *new_src_folder_name = None;
            *new_folder_parent_folder = None;
            ui.close();
        }
        if ui
            .button(egui::RichText::new(format!("{} New Folder", ph::FOLDER_PLUS)).size(11.5))
            .clicked()
        {
            begin_inline_new(ui, true, "", new_src_folder_name, new_folder_parent_folder);
            *new_src_name = None;
            *new_file_parent_folder = None;
            ui.close();
        }
    });

    ui.add_space(2.0);
    file_row(
        ui,
        4.0,
        ".gitignore",
        ProjectFileId::GitIgnore,
        selected,
        build_result,
        lsp_state,
    );
    if *toolchain == ToolchainKind::RustEmbedded {
        file_row(
            ui,
            4.0,
            "build.rs",
            ProjectFileId::BuildRs,
            selected,
            build_result,
            lsp_state,
        );
    }
    file_row(
        ui,
        4.0,
        "Cargo.toml",
        ProjectFileId::CargoToml,
        selected,
        build_result,
        lsp_state,
    );
    if *toolchain == ToolchainKind::RustEmbedded {
        file_row(
            ui,
            4.0,
            "memory.x",
            ProjectFileId::MemoryX,
            selected,
            build_result,
            lsp_state,
        );
    }
}

/// Recursively render tree nodes (files and folders).
#[allow(clippy::too_many_arguments)]
fn render_tree_node(
    ui: &mut egui::Ui,
    tree: &BTreeMap<String, TreeNode>,
    user_src_files: &mut Vec<(String, String)>,
    user_src_folders: &mut Vec<String>,
    selected: &mut ProjectFileId,
    indent: f32,
    renaming_file: &mut Option<(usize, String)>,
    do_rename_file: &mut Option<usize>,
    cancel_rename_file: &mut bool,
    to_delete: &mut Option<usize>,
    renaming_folder: &mut Option<(String, String)>,
    workspace_dir: &std::path::Path,
    save_needed: &mut bool,
    new_src_name: &mut Option<String>,
    new_src_folder_name: &mut Option<String>,
    new_file_parent_folder: &mut Option<String>,
    new_folder_parent_folder: &mut Option<String>,
    parent_path: &str,
    move_request: &mut Option<(DraggedItem, String)>,
) {
    let default_tree_folder_color = egui::Color32::from_rgb(100, 105, 115);
    // While any inline edit is active (new file/folder input or a rename), don't
    // arm the folder drag-source overlay: its `Sense::drag()` interaction on the
    // header steals the pointer/focus from the just-opened input, so the input
    // flickers open and immediately cancels (reported as "it tries to move the
    // folder"). Dragging isn't meaningful mid-edit anyway.
    let editing = new_src_name.is_some()
        || new_src_folder_name.is_some()
        || renaming_file.is_some()
        || renaming_folder.is_some();

    for (name, node) in tree {
        match node {
            TreeNode::File(idx) => {
                let full_path = &user_src_files[*idx].0;
                let file_name = full_path.split('/').last().unwrap_or(full_path).to_string();
                user_file_row(
                    ui,
                    indent,
                    &file_name,
                    *idx,
                    selected,
                    to_delete,
                    renaming_file,
                    do_rename_file,
                    cancel_rename_file,
                );
            }
            TreeNode::Folder(children) => {
                let folder_path = if parent_path.is_empty() {
                    name.clone()
                } else {
                    format!("{parent_path}/{name}")
                };
                let is_renaming = renaming_folder
                    .as_ref()
                    .map(|(f, _)| f == &folder_path)
                    .unwrap_or(false);

                if is_renaming {
                    let mut do_apply = false;
                    let mut do_cancel = false;
                    if let Some((_, new_name)) = renaming_folder.as_mut() {
                        let fid = egui::Id::new(("__rename_folder__", folder_path.as_str()));
                        ui.horizontal(|ui| {
                            ui.add_space(indent);
                            ui.label(
                                egui::RichText::new(ph::FOLDER)
                                    .size(11.5)
                                    .color(egui::Color32::from_rgb(200, 165, 70)),
                            );
                            let resp = ui.add(
                                egui::TextEdit::singleline(new_name)
                                    .desired_width(ui.available_width()),
                            );
                            if ui.memory(|m| m.data.get_temp::<bool>(fid).unwrap_or(true)) {
                                resp.request_focus();
                                ui.memory_mut(|m| m.data.insert_temp(fid, false));
                            }
                            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                do_apply = true;
                            } else if ui.input(|i| i.key_pressed(egui::Key::Escape))
                                || resp.lost_focus()
                            {
                                do_cancel = true;
                            }
                        });
                    }
                    if do_apply {
                        if let Some((_, new_name)) = renaming_folder.take() {
                            let clean = new_name.trim().to_string();
                            let new_path = if parent_path.is_empty() {
                                clean.clone()
                            } else {
                                format!("{parent_path}/{clean}")
                            };
                            let collides = user_src_folders.iter().any(|f| f == &new_path)
                                || user_src_files.iter().any(|(p, _)| p == &new_path);
                            if !clean.is_empty() && new_path != folder_path && !collides {
                                // Best-effort rename on disk (live workspace).
                                let old_dest = workspace_dir.join("src").join(&folder_path);
                                let new_dest = workspace_dir.join("src").join(&new_path);
                                let _ = std::fs::rename(&old_dest, &new_dest);
                                // Update the folder itself and any nested folders.
                                let old_prefix = format!("{folder_path}/");
                                for f in user_src_folders.iter_mut() {
                                    if *f == folder_path {
                                        *f = new_path.clone();
                                    } else if let Some(rest) = f.strip_prefix(&old_prefix) {
                                        *f = format!("{new_path}/{rest}");
                                    }
                                }
                                // Update every file under the renamed folder.
                                for (p, _) in user_src_files.iter_mut() {
                                    if let Some(rest) = p.strip_prefix(&old_prefix) {
                                        *p = format!("{new_path}/{rest}");
                                    }
                                }
                                *save_needed = true;
                            }
                        }
                    } else if do_cancel {
                        *renaming_folder = None;
                    }
                } else {
                    let ch = egui::CollapsingHeader::new(
                        egui::RichText::new(format!("{name}/"))
                            .size(11.5)
                            .monospace()
                            .color(default_tree_folder_color),
                    )
                    .default_open(true)
                    .show(ui, |ui| {
                        // Inline "new file / new folder" input as the first child
                        // of this folder (shown right where the item is added).
                        inline_new_item(
                            ui,
                            indent + 8.0,
                            &folder_path,
                            false,
                            new_src_name,
                            new_file_parent_folder,
                            user_src_files,
                            user_src_folders,
                            selected,
                            workspace_dir,
                            save_needed,
                        );
                        inline_new_item(
                            ui,
                            indent + 8.0,
                            &folder_path,
                            true,
                            new_src_folder_name,
                            new_folder_parent_folder,
                            user_src_files,
                            user_src_folders,
                            selected,
                            workspace_dir,
                            save_needed,
                        );
                        if children.is_empty()
                            && new_file_parent_folder.as_deref() != Some(folder_path.as_str())
                            && new_folder_parent_folder.as_deref() != Some(folder_path.as_str())
                        {
                            ui.label(
                                egui::RichText::new("  (empty)")
                                    .size(10.0)
                                    .color(egui::Color32::from_gray(95)),
                            );
                        }
                        render_tree_node(
                            ui,
                            children,
                            user_src_files,
                            user_src_folders,
                            selected,
                            indent + 8.0,
                            renaming_file,
                            do_rename_file,
                            cancel_rename_file,
                            to_delete,
                            renaming_folder,
                            workspace_dir,
                            save_needed,
                            new_src_name,
                            new_src_folder_name,
                            new_file_parent_folder,
                            new_folder_parent_folder,
                            &folder_path,
                            move_request,
                        );
                    });

                    // Drag SOURCE: a separate drag-sensing overlay on the header
                    // rect so the whole folder can be moved. `Sense::drag()` only
                    // (not click) so a plain click still toggles the collapsing
                    // header underneath; only a press-and-move starts a drag.
                    // Skipped while editing (see `editing` above).
                    if !editing {
                        let folder_drag = ui.interact(
                            ch.header_response.rect,
                            ch.header_response.id.with("__folder_drag__"),
                            egui::Sense::drag(),
                        );
                        folder_drag.dnd_set_drag_payload(DraggedItem::Folder(folder_path.clone()));
                        if folder_drag.dragged() {
                            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                                egui::Area::new(egui::Id::new("__tree_drag_preview__"))
                                    .fixed_pos(pos + egui::vec2(12.0, 4.0))
                                    .order(egui::Order::Tooltip)
                                    .interactable(false)
                                    .show(ui.ctx(), |ui| {
                                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} {name}/",
                                                    ph::FOLDER
                                                ))
                                                .size(11.0),
                                            );
                                        });
                                    });
                            }
                        }
                    }

                    // Drop TARGET: dragging an item onto this folder header moves
                    // it here. Highlight the header while an item hovers over it.
                    if ch.header_response.dnd_hover_payload::<DraggedItem>().is_some() {
                        ui.painter().rect_stroke(
                            ch.header_response.rect,
                            3.0,
                            egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 170, 240)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if let Some(p) = ch.header_response.dnd_release_payload::<DraggedItem>() {
                        *move_request = Some(((*p).clone(), folder_path.clone()));
                    }

                    ch.header_response.context_menu(|ui| {
                        if ui
                            .button(
                                egui::RichText::new(format!("{} New File", ph::FILE_PLUS))
                                    .size(11.5),
                            )
                            .clicked()
                        {
                            begin_inline_new(
                                ui,
                                false,
                                &folder_path,
                                new_src_name,
                                new_file_parent_folder,
                            );
                            // Cancel any pending folder input so only one shows.
                            *new_src_folder_name = None;
                            *new_folder_parent_folder = None;
                            ui.close();
                        }
                        if ui
                            .button(
                                egui::RichText::new(format!("{} New Folder", ph::FOLDER_PLUS))
                                    .size(11.5),
                            )
                            .clicked()
                        {
                            begin_inline_new(
                                ui,
                                true,
                                &folder_path,
                                new_src_folder_name,
                                new_folder_parent_folder,
                            );
                            *new_src_name = None;
                            *new_file_parent_folder = None;
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button(
                                egui::RichText::new(format!("{} Rename", ph::PENCIL_SIMPLE))
                                    .size(11.5),
                            )
                            .clicked()
                        {
                            *renaming_folder = Some((folder_path.clone(), name.clone()));
                            // Reset the focus flag so the edit box grabs focus next frame.
                            let fid = egui::Id::new(("__rename_folder__", folder_path.as_str()));
                            ui.memory_mut(|m| m.data.insert_temp(fid, true));
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button(
                                egui::RichText::new(format!("{} Delete", ph::TRASH))
                                    .size(11.5)
                                    .color(egui::Color32::from_rgb(220, 80, 60)),
                            )
                            .clicked()
                        {
                            // Delete folder and all contents
                            let prefix = format!("{folder_path}/");
                            let to_rm: Vec<usize> = user_src_files
                                .iter()
                                .enumerate()
                                .filter(|(_, (p, _))| p.starts_with(&prefix))
                                .map(|(i, _)| i)
                                .collect();
                            // If the selected file lives under this folder it's about
                            // to be removed; fall back to main.rs before indices shift.
                            if let ProjectFileId::UserFile(sel) = *selected {
                                if to_rm.contains(&sel) {
                                    *selected = ProjectFileId::MainRs;
                                }
                            }
                            for i in to_rm.into_iter().rev() {
                                // user_src_files paths are relative to src/.
                                let dest = workspace_dir.join("src").join(&user_src_files[i].0);
                                let _ = std::fs::remove_file(&dest);
                            }
                            // Drop the entries directly instead of relying on fs-watcher polling.
                            user_src_files.retain(|(p, _)| !p.starts_with(&prefix));
                            user_src_folders.retain(|f| f != &folder_path);
                            let dest = workspace_dir.join("src").join(&folder_path);
                            let _ = std::fs::remove_dir_all(&dest);
                            *save_needed = true;
                            ui.close();
                        }
                    });
                }
            }
        }
    }
}

fn file_row(
    ui: &mut egui::Ui,
    indent: f32,
    name: &str,
    id: ProjectFileId,
    selected: &mut ProjectFileId,
    build_result: Option<&build::BuildResult>,
    lsp_state: Option<&lsp::LspState>,
) {
    let hi = egui::Color32::from_rgb(100, 180, 255);
    // Fixed project files have no Rename/Delete menu — mark them dark-red + bold.
    let fixed = egui::Color32::from_rgb(100, 50, 50);

    ui.horizontal(|ui| {
        ui.add_space(indent);
        let is_sel = *selected == id;
        let color = if is_sel { hi } else { fixed };
        ui.label(egui::RichText::new(ph::FILE).size(11.5).color(color));
        let resp = ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .size(11.5)
                    .monospace()
                    .strong()
                    .color(color),
            )
            .sense(egui::Sense::click()),
        );
        if resp.clicked() {
            *selected = id;
        }
        if let Some(cargo_path) = id.cargo_path() {
            let cargo_err = build_result.map_or(false, |r| r.has_errors_in(cargo_path));
            let lsp_err = lsp_state.map_or(false, |l| l.error_count_for(cargo_path) > 0);
            if cargo_err || lsp_err {
                ui.label(
                    egui::RichText::new(ph::X_CIRCLE)
                        .size(10.0)
                        .color(egui::Color32::from_rgb(220, 80, 70)),
                );
            }
        }
    });
}

fn user_file_row(
    ui: &mut egui::Ui,
    indent: f32,
    name: &str,
    idx: usize,
    selected: &mut ProjectFileId,
    to_delete: &mut Option<usize>,
    renaming: &mut Option<(usize, String)>,
    do_rename: &mut Option<usize>,
    cancel_rename: &mut bool,
) {
    let hi = egui::Color32::from_rgb(100, 180, 255);
    let normal = egui::Color32::from_rgb(200, 205, 215);
    let id = ProjectFileId::UserFile(idx);
    let is_renaming = renaming.as_ref().map(|(i, _)| *i == idx).unwrap_or(false);

    if is_renaming {
        ui.horizontal(|ui| {
            ui.add_space(indent);
            if let Some((_, new_name)) = renaming.as_mut() {
                let fid = egui::Id::new(("__rename_file__", idx));
                let resp = ui
                    .add(egui::TextEdit::singleline(new_name).desired_width(ui.available_width()));
                // Focus the field on the first frame it appears.
                if ui.memory(|m| m.data.get_temp::<bool>(fid).unwrap_or(true)) {
                    resp.request_focus();
                    ui.memory_mut(|m| m.data.insert_temp(fid, false));
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    *do_rename = Some(idx);
                } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) || resp.lost_focus() {
                    *cancel_rename = true;
                }
            }
        });
        return;
    }

    ui.horizontal(|ui| {
        ui.add_space(indent);
        let is_sel = *selected == id;
        let color = if is_sel { hi } else { normal };
        ui.label(egui::RichText::new(ph::FILE).size(11.5).color(color));
        // `click_and_drag`: a plain click still selects (below); a drag makes the
        // row a drag source so it can be dropped onto a folder to move it.
        let resp = ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .size(11.5)
                    .monospace()
                    .color(color),
            )
            .sense(egui::Sense::click_and_drag()),
        );
        resp.dnd_set_drag_payload(DraggedItem::File(idx));
        if resp.dragged() {
            // Floating preview following the cursor (the response-based DnD API
            // doesn't paint one itself, unlike `dnd_drag_source`).
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                egui::Area::new(egui::Id::new("__tree_drag_preview__"))
                    .fixed_pos(pos + egui::vec2(12.0, 4.0))
                    .order(egui::Order::Tooltip)
                    .interactable(false)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!("{} {name}", ph::FILE)).size(11.0),
                            );
                        });
                    });
            }
        }
        if resp.clicked() {
            *selected = id;
        }
        resp.context_menu(|ui| {
            if ui
                .button(egui::RichText::new(format!("{} Rename", ph::PENCIL_SIMPLE)).size(11.5))
                .clicked()
            {
                *renaming = Some((idx, name.to_string()));
                // Reset the focus flag so the edit box grabs focus next frame.
                let fid = egui::Id::new(("__rename_file__", idx));
                ui.memory_mut(|m| m.data.insert_temp(fid, true));
                ui.close();
            }
            ui.separator();
            if ui
                .button(
                    egui::RichText::new(format!("{} Delete", ph::TRASH))
                        .size(11.5)
                        .color(egui::Color32::from_rgb(220, 80, 60)),
                )
                .clicked()
            {
                *to_delete = Some(idx);
                ui.close();
            }
        });
    });
}
