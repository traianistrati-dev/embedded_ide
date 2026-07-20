//! Project tree GUI — file browser with create/rename/delete operations.

use crate::app::ProjectFileId;
use crate::project_tree::logic::SRC_ROOT;
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

/// How long the primary button must be held STILL on an item before its drag
/// arms (the payload is set). Holding still is required because moving the
/// pointer early cancels egui's press-on-widget tracking — which is exactly
/// what makes a quick click/drag gesture NOT start a move.
const DRAG_HOLD_SECS: f64 = 0.4;

/// Hover affordance for an interactive tree row (file / folder header): a
/// subtle tint over the row plus the pointing-hand cursor — makes it visible
/// WHERE a click / right-click menu / hold-to-drag actually works. The cursor
/// then progresses hand → grab (while the button is held, see the call sites)
/// → grabbing/no-drop (once the drag arms; set by the payload block at the end
/// of the tree, which runs last and therefore wins).
fn row_hover_feedback(ui: &egui::Ui, rect: egui::Rect, contains_pointer: bool) {
    if contains_pointer {
        ui.painter().rect_filled(
            rect.expand2(egui::vec2(2.0, 0.0)),
            3.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

/// Long-press-to-drag gate, fully stateless: `true` once the PRIMARY button has
/// been held on `resp` for ≥ [`DRAG_HOLD_SECS`] (press-start time comes from
/// egui's own pointer state — no temp-memory bookkeeping that could go stale).
/// A normal click, a right-click (secondary → `primary_down()` is false), or a
/// context-menu interaction never arms, so dragging can't hijack those — the
/// caller only sets the DnD payload when this returns `true`.
fn drag_armed(ui: &egui::Ui, resp: &egui::Response) -> bool {
    if !resp.is_pointer_button_down_on() {
        return false;
    }
    ui.input(|i| {
        i.pointer.primary_down()
            && i.pointer
                .press_start_time()
                .is_some_and(|t| i.time - t >= DRAG_HOLD_SECS)
    })
}

/// If `path` is an auto-generated folder that must NOT be moved, return a short
/// explanation; otherwise `None`. `pins/` and `pins/configs/` are recreated
/// every frame by the pin/peripheral sync, so moving them breaks codegen.
fn generated_folder_reason(path: &str) -> Option<&'static str> {
    match path {
        "src/pins" => Some("the `pins/` folder is auto-generated from your pin configuration"),
        "src/pins/configs" => Some("`pins/configs/` is auto-generated from the Virtual Modules (USART/SPI/I2C)"),
        _ => None,
    }
}

/// If `path` (relative to the PROJECT ROOT) is an auto-generated file that must
/// NOT be moved, return a short human explanation; otherwise `None`. These are
/// rebuilt each frame from the MCU / pin configuration (see
/// `ProjectTreeState::sync_pin_files` / `sync_config_files`), so moving one
/// would be silently undone or would break the generated module tree.
///
/// Only the firmware's `src/` has generated files — a library crate's paths
/// never match, which is exactly why extracted libraries carry no restrictions.
pub(crate) fn generated_file_reason(path: &str) -> Option<&'static str> {
    if path == "src/pins/mod.rs" || path == "src/pins/configs/mod.rs" {
        return Some("it's an auto-generated module file (rebuilt from your pin / peripheral configuration)");
    }
    if path.starts_with("src/pins/configs/") {
        return Some("it's an auto-generated peripheral init file — edit it via the MCU Configurator (Virtual Modules)");
    }
    // Generated pin files sit directly under src/pins/ as `pin<…>.rs`.
    if let Some(fname) = path.strip_prefix("src/pins/") {
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
    if target_folder == "src/pins/configs" || target_folder.starts_with("src/pins/configs/") {
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

    let old_dest = workspace_dir.join(&old_path);
    let new_dest = workspace_dir.join(&new_path);
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
    // Released over its own header (e.g. an in-place hold that never left the
    // row) — just a no-op, not worth a warning banner.
    if target_folder == src {
        return;
    }
    // Can't drop a folder into one of its own descendants.
    if target_folder.starts_with(&format!("{src}/")) {
        set_tree_notice(ui.ctx(), "Can't move a folder into its own subfolder.".to_string());
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

    let old_dest = workspace_dir.join(src);
    let new_dest = workspace_dir.join(&new_path);
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
    // Only treat a focus-loss as "cancel" AFTER the input has actually held
    // keyboard focus for a frame — on the frame it appears (before
    // `request_focus` takes effect) a stray interaction elsewhere would fire
    // `lost_focus()` and instantly close it (the "flickers open then vanishes"
    // bug).
    let had_focus_id = focus_id.with("had_focus");
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
            let had_focus =
                ui.memory(|m| m.data.get_temp::<bool>(had_focus_id).unwrap_or(false));
            if resp.has_focus() {
                ui.memory_mut(|m| m.data.insert_temp(had_focus_id, true));
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                create = true;
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape))
                || (had_focus && resp.lost_focus())
            {
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
                    let dest = workspace_dir.join(&full);
                    let _ = std::fs::create_dir_all(&dest);
                    user_src_folders.push(full);
                    *save_needed = true;
                } else {
                    let dest = workspace_dir.join(&full);
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
        ui.memory_mut(|m| m.data.remove::<bool>(had_focus_id));
    } else if cancel {
        *name_state = None;
        *parent_state = None;
        ui.memory_mut(|m| m.data.remove::<bool>(had_focus_id));
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
    // Folder the user asked to turn into a sibling library crate (project-root
    // relative); the caller opens the Extract dialog.
    extract_folder: &mut Option<String>,
    // Directory names of the `[workspace] members` — the extracted library
    // crates, each rendered as its own collapsible section below the project.
    lib_crates: &[String],
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

    // Collected during the tree render below; an item dragged onto a folder sets
    // `(dragged_item, target_folder_rel_to_src)` — applied after the tree closure
    // so it doesn't clash with the `&mut` borrows used for rendering.
    let mut move_request: Option<(DraggedItem, String)> = None;
    // File-row signals, shared by the `src/` section AND every library-crate
    // section below it, applied once after all of them have rendered — the
    // indices they carry are only valid while `user_src_files` is unchanged.
    let mut to_delete: Option<usize> = None;
    let mut to_duplicate: Option<usize> = None;
    let mut do_rename_file: Option<usize> = None;
    let mut cancel_rename_file = false;

    // Paths are project-root-relative, so the full tree has `src` and every
    // library crate as top-level nodes. Built once (owned — it does not borrow
    // `user_src_files`); each section renders its own subtree.
    let full_tree = build_tree(user_src_files, user_src_folders);
    let subtree_of = |name: &str| match full_tree.get(name) {
        Some(TreeNode::Folder(children)) => children.clone(),
        _ => BTreeMap::new(),
    };

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

        // Inline "new file / new folder" input at the src/ root, rendered right
        // under main.rs where the item will be added.
        inline_new_item(
            ui, 8.0, SRC_ROOT, false, new_src_name, new_file_parent_folder,
            user_src_files, user_src_folders, selected, workspace_dir, save_needed,
        );
        inline_new_item(
            ui, 8.0, SRC_ROOT, true, new_src_folder_name, new_folder_parent_folder,
            user_src_files, user_src_folders, selected, workspace_dir, save_needed,
        );

        // This header IS `src/`, so it renders that subtree; library crates get
        // their own sections further down.
        let tree = subtree_of(SRC_ROOT);

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
            &mut to_duplicate,
            renaming_folder,
            workspace_dir,
            save_needed,
            new_src_name,
            new_src_folder_name,
            new_file_parent_folder,
            new_folder_parent_folder,
            SRC_ROOT, // this header IS src/, so children hang off it
            &mut move_request,
            extract_folder,
        );

    });

    // Hover: tint + pointing hand — the src/ header is a drop target and hosts
    // the New File / New Folder context menu.
    row_hover_feedback(
        ui,
        src_ch.header_response.rect,
        src_ch.header_response.contains_pointer(),
    );

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
        move_request = Some(((*p).clone(), SRC_ROOT.to_owned()));
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
        // Git actions live in the bottom "Git" tab (commit/push/pull), not
        // here — kept out of the tree menu on purpose (moved 2026-07-07).
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

    // ── Library crates ───────────────────────────────────────────────────────
    // Crates extracted out of the project (see `extract_crate`) live NEXT TO
    // src/ and are listed as `[workspace] members`. They render exactly like
    // the project's own files — delete / rename / new file / new folder all
    // work — because no generated-path guard can match a path outside `src/`.
    let libs: Vec<String> = lib_crates
        .iter()
        .filter(|c| full_tree.contains_key(c.as_str()))
        .cloned()
        .collect();
    if !libs.is_empty() {
        ui.add_space(6.0);
        ui.separator();
        ui.label(
            egui::RichText::new("LIBRARIES")
                .size(9.0)
                .color(egui::Color32::from_gray(110)),
        );
    }
    for lib in &libs {
        let id = ui.make_persistent_id(("lib_crate_section", lib.as_str()));
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
        let open = state.is_open();

        // Custom header so the caret sits on the RIGHT (`mw_radar ^`).
        // `CollapsingState::show_header` would paint egui's own arrow on the
        // left; and in a right_to_left layout the FIRST widget added is the
        // rightmost, so the caret goes in before anything else.
        let mut toggle = false;
        let header = ui.horizontal(|ui| {
            let name_resp = ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{} {lib}", ph::PACKAGE))
                        .size(11.5)
                        .monospace()
                        .strong()
                        .color(egui::Color32::from_rgb(140, 190, 145)),
                )
                .selectable(false)
                .sense(egui::Sense::click()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let icon = if open { ph::CARET_DOWN } else { ph::CARET_DOUBLE_UP };
                if ui
                    .add(egui::Button::new(egui::RichText::new(icon).size(11.0)).frame(false))
                    .on_hover_text(if open {
                        "Collapse this library"
                    } else {
                        "Expand this library"
                    })
                    .clicked()
                {
                    toggle = true;
                }
            });
            name_resp
        });
        if toggle || header.inner.clicked() {
            state.set_open(!open);
        }
        row_hover_feedback(ui, header.response.rect, header.response.contains_pointer());

        state.show_body_indented(&header.response, ui, |ui| {
            inline_new_item(
                ui, 8.0, lib, false, new_src_name, new_file_parent_folder,
                user_src_files, user_src_folders, selected, workspace_dir, save_needed,
            );
            inline_new_item(
                ui, 8.0, lib, true, new_src_folder_name, new_folder_parent_folder,
                user_src_files, user_src_folders, selected, workspace_dir, save_needed,
            );
            let tree = subtree_of(lib);
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
                &mut to_duplicate,
                renaming_folder,
                workspace_dir,
                save_needed,
                new_src_name,
                new_src_folder_name,
                new_file_parent_folder,
                new_folder_parent_folder,
                lib,
                &mut move_request,
                extract_folder,
            );
        });
        state.store(ui.ctx());

        header.response.context_menu(|ui| {
            if ui
                .button(egui::RichText::new(format!("{} New File", ph::FILE_PLUS)).size(11.5))
                .clicked()
            {
                begin_inline_new(ui, false, lib, new_src_name, new_file_parent_folder);
                *new_src_folder_name = None;
                *new_folder_parent_folder = None;
                ui.close();
            }
            if ui
                .button(egui::RichText::new(format!("{} New Folder", ph::FOLDER_PLUS)).size(11.5))
                .clicked()
            {
                begin_inline_new(ui, true, lib, new_src_folder_name, new_folder_parent_folder);
                *new_src_name = None;
                *new_file_parent_folder = None;
                ui.close();
            }
        });
    }

    // ── Apply the file-row signals ───────────────────────────────────────────
    // Once, after EVERY section: the indices are positions in `user_src_files`,
    // and mutating it mid-render would invalidate the ones a later section
    // produced.

    // Duplication — copy under the next free `<stem>_<n>` name in the same
    // folder, and select the new file so it is obvious what happened.
    if let Some(idx) = to_duplicate {
        let (src_path, content) = user_src_files[idx].clone();
        let new_path = crate::project_tree::logic::duplicate_path(&src_path, |cand| {
            user_src_files.iter().any(|(p, _)| p == cand)
        });
        let dest = workspace_dir.join(&new_path);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&dest, &content);
        user_src_files.push((new_path, content));
        *selected = ProjectFileId::UserFile(user_src_files.len() - 1);
        *save_needed = true;
    }

    // Deletion
    if let Some(idx) = to_delete {
        if *selected == ProjectFileId::UserFile(idx) {
            *selected = ProjectFileId::MainRs;
        }
        let dest = workspace_dir.join(&user_src_files[idx].0);
        let _ = std::fs::remove_file(&dest);
        user_src_files.remove(idx);
        *save_needed = true;
    }

    // Rename
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
                    let old_dest = workspace_dir.join(&old_path);
                    let new_dest = workspace_dir.join(&new_path);
                    let _ = std::fs::rename(&old_dest, &new_dest);
                    user_src_files[confirm_idx].0 = new_path;
                    *save_needed = true;
                }
            }
        }
    } else if cancel_rename_file {
        *renaming_file = None;
    }

    // While a tree item is being dragged (payload armed via the hold gate), give
    // the cursor the drag icon — `Grabbing`, or `NoDrop` when the item is
    // auto-generated (can't be moved) — and float a name preview by the pointer.
    // Drawn ONCE, payload-driven (a per-widget preview would vanish once the
    // pointer leaves the source row), and at the END of the tree: egui's
    // last-write-wins cursor means running after every row overrides any hover
    // cursor a row set (a selectable Label's I-beam used to mask this for files).
    // Reading the payload here also picks up same-frame arming (no 1-frame lag).
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
        let label = match &*payload {
            DraggedItem::File(idx) => user_src_files
                .get(*idx)
                .map(|(p, _)| format!("{} {}", ph::FILE, base_name(p))),
            DraggedItem::Folder(p) => Some(format!("{} {}/", ph::FOLDER, base_name(p))),
        };
        if let (Some(label), Some(pos)) = (label, ui.ctx().pointer_interact_pos()) {
            egui::Area::new(egui::Id::new("__tree_drag_preview__"))
                .fixed_pos(pos + egui::vec2(12.0, 4.0))
                .order(egui::Order::Tooltip)
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        // `.extend()`: never wrap — near the panel edge the name
                        // otherwise breaks into a one-char-per-line column.
                        ui.add(egui::Label::new(egui::RichText::new(label).size(11.0)).extend());
                    });
                });
        }
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
    to_duplicate: &mut Option<usize>,
    renaming_folder: &mut Option<(String, String)>,
    workspace_dir: &std::path::Path,
    save_needed: &mut bool,
    new_src_name: &mut Option<String>,
    new_src_folder_name: &mut Option<String>,
    new_file_parent_folder: &mut Option<String>,
    new_folder_parent_folder: &mut Option<String>,
    parent_path: &str,
    move_request: &mut Option<(DraggedItem, String)>,
    extract_folder: &mut Option<String>,
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
                let can_duplicate = generated_file_reason(full_path).is_none();
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
                    to_duplicate,
                    can_duplicate,
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
                                let old_dest = workspace_dir.join(&folder_path);
                                let new_dest = workspace_dir.join(&new_path);
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
                            to_duplicate,
                            renaming_folder,
                            workspace_dir,
                            save_needed,
                            new_src_name,
                            new_src_folder_name,
                            new_file_parent_folder,
                            new_folder_parent_folder,
                            &folder_path,
                            move_request,
                            extract_folder,
                        );
                    });

                    // Drag SOURCE: hold the primary button STILL on the header
                    // for ~0.4s to arm a move of the whole folder, then drag to a
                    // target. The payload is set directly on the header response —
                    // NO separate `ui.interact` overlay: an overlay competed with
                    // the header for pointer interactions, which broke the
                    // right-click context menu (New/Rename/Delete) and stole focus
                    // from the inline new-item input. A plain click (short) still
                    // toggles the header; a right-click (secondary) never arms.
                    // Skipped while an inline edit is open (see `editing` above).
                    if !editing && drag_armed(ui, &ch.header_response) {
                        egui::DragAndDrop::set_payload(
                            ui.ctx(),
                            DraggedItem::Folder(folder_path.clone()),
                        );
                    }

                    // Hover: tint the header + pointing hand (click toggles,
                    // right-click menu, hold-to-drag all work here); grab while
                    // the button is held (pre-arm feedback). The end-of-tree
                    // payload block upgrades to grabbing/no-drop once armed.
                    row_hover_feedback(
                        ui,
                        ch.header_response.rect,
                        ch.header_response.contains_pointer(),
                    );
                    if !editing
                        && ch.header_response.is_pointer_button_down_on()
                        && ui.input(|i| i.pointer.primary_down())
                    {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
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
                        // `pins/` and `pins/configs/` are rebuilt every frame by
                        // the pin/peripheral sync — renaming or deleting them
                        // would be undone, or would break codegen. Same guard
                        // that already refuses moving them.
                        if let Some(reason) = generated_folder_reason(&folder_path) {
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} Read-only — {reason}",
                                    ph::LOCK_SIMPLE
                                ))
                                .size(10.5)
                                .color(egui::Color32::from_rgb(210, 170, 90)),
                            );
                            return;
                        }
                        ui.separator();
                        // Turn this folder into a sibling crate you can publish.
                        if ui
                            .button(
                                egui::RichText::new(format!(
                                    "{} Extract to library crate…",
                                    ph::PACKAGE
                                ))
                                .size(11.5),
                            )
                            .on_hover_text(
                                "Move this folder into its own Cargo crate next to src/, \
                                 wire it up as a workspace member + path dependency, and \
                                 give it a publishable Cargo.toml.",
                            )
                            .clicked()
                        {
                            *extract_folder = Some(folder_path.clone());
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
                                let dest = workspace_dir.join(&user_src_files[i].0);
                                let _ = std::fs::remove_file(&dest);
                            }
                            // Drop the entries directly instead of relying on fs-watcher polling.
                            user_src_files.retain(|(p, _)| !p.starts_with(&prefix));
                            user_src_folders.retain(|f| f != &folder_path);
                            let dest = workspace_dir.join(&folder_path);
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

    let row = ui.horizontal(|ui| {
        ui.add_space(indent);
        let is_sel = *selected == id;
        let color = if is_sel { hi } else { fixed };
        ui.label(egui::RichText::new(ph::FILE).size(11.5).color(color));
        // `selectable(false)`: keeps the label from forcing the text (I-beam)
        // hover cursor, which would override the drag cursor while an item is
        // dragged across this row (see `user_file_row`).
        let resp = ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .size(11.5)
                    .monospace()
                    .strong()
                    .color(color),
            )
            .selectable(false)
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
    // Hover: tint + pointing hand (the row is clickable).
    row_hover_feedback(ui, row.response.rect, row.response.contains_pointer());
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
    // Set by the Duplicate menu entry; the caller copies the file (it owns the
    // file list). Offered only for files that are safe to rename/delete — a
    // copy inside `pins/` would be pruned by the next pin sync.
    to_duplicate: &mut Option<usize>,
    can_duplicate: bool,
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

    let row = ui.horizontal(|ui| {
        ui.add_space(indent);
        let is_sel = *selected == id;
        let color = if is_sel { hi } else { normal };
        ui.label(egui::RichText::new(ph::FILE).size(11.5).color(color));
        // Plain click sense: click selects, right-click opens the menu. A move
        // arms only via the hold gate (`drag_armed`) — hold the primary button
        // still ~0.4s, then drag; the payload is set directly (no drag sense, so
        // nothing competes with clicks). The floating preview + drag cursor are
        // drawn globally at the end of the tree while a payload exists.
        // `selectable(false)`: a selectable Label sets the text (I-beam) hover
        // cursor AFTER our global drag-cursor call, overriding it — which is why
        // files showed no drag cursor while folder headers (plain buttons) did.
        let resp = ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .size(11.5)
                    .monospace()
                    .color(color),
            )
            .selectable(false)
            .sense(egui::Sense::click()),
        );
        // Holding the primary button on the row (pre-arm) — reported to the
        // caller so the cursor can show "grab" while the 0.4s hold elapses.
        let held =
            resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
        if drag_armed(ui, &resp) {
            egui::DragAndDrop::set_payload(ui.ctx(), DraggedItem::File(idx));
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
            if can_duplicate
                && ui
                    .button(egui::RichText::new(format!("{} Duplicate", ph::COPY)).size(11.5))
                    .on_hover_text("Copy this file next to it as <name>_1")
                    .clicked()
            {
                *to_duplicate = Some(idx);
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
        held
    });
    // Hover: tint the whole row + pointing-hand cursor (click / menu / drag all
    // work here); while the button is held, grab — then the end-of-tree payload
    // block upgrades it to grabbing/no-drop once the drag arms.
    row_hover_feedback(ui, row.response.rect, row.response.contains_pointer());
    if row.inner {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
}
