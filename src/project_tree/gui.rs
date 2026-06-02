//! Project tree GUI — file browser with create/rename/delete operations.

use std::collections::BTreeMap;
use eframe::egui;
use egui_phosphor::regular as ph;
use crate::app::ProjectFileId;
use crate::{build, lsp};
use crate::panels::mcu_module::mcu_catalog::ToolchainKind;

/// Tree node representing a file or folder.
#[derive(Clone, Debug)]
enum TreeNode {
    File(usize),  // index into user_src_files
    Folder(BTreeMap<String, TreeNode>),
}

/// Build a hierarchical tree from user_src_files and user_src_folders.
fn build_tree(user_src_files: &[(String, String)], user_src_folders: &[String]) -> BTreeMap<String, TreeNode> {
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

/// Helper: insert a folder path into the tree.
fn insert_folder_path(root: &mut BTreeMap<String, TreeNode>, path: &str) {
    let parts: Vec<&str> = path.split('/').collect();
    let mut current = root;
    for part in parts {
        let node = current
            .entry(part.to_string())
            .or_insert_with(|| TreeNode::Folder(BTreeMap::new()));
        if let TreeNode::Folder(children) = node {
            current = children;
        } else {
            break; // Parent is a file, can't go deeper
        }
    }
}

/// Helper: insert a file path into the tree.
fn insert_file_path(root: &mut BTreeMap<String, TreeNode>, parts: &[&str], file_idx: usize) {
    if parts.is_empty() {
        return;
    }
    let mut current = root;
    for part in &parts[..parts.len() - 1] {
        let node = current
            .entry(part.to_string())
            .or_insert_with(|| TreeNode::Folder(BTreeMap::new()));
        if let TreeNode::Folder(children) = node {
            current = children;
        } else {
            return; // Parent is a file, can't go deeper
        }
    }
    let file_name = parts[parts.len() - 1].to_string();
    current.insert(file_name, TreeNode::File(file_idx));
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
    let default_tree_folder_color = egui::Color32::from_rgb(100, 105, 115);

    ui.label(
        egui::RichText::new(format!("package: {pkg_name}"))
            .size(12.0)
            .strong()
            .color(egui::Color32::DARK_RED),
    );
    ui.add_space(2.0);

    // .cargo/
    egui::CollapsingHeader::new(
        egui::RichText::new(".cargo/")
            .size(11.5)
            .monospace()
            .color(default_tree_folder_color),
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

    // src/
    let src_ch = egui::CollapsingHeader::new(
        egui::RichText::new("src/")
            .size(11.5)
            .monospace()
            .color(default_tree_folder_color),
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
            "src",  // parent path at root is "src"
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
                    if new_path != old_path && !user_src_files.iter().any(|(p, _)| p == &new_path)
                    {
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

    src_ch.header_response.context_menu(|ui| {
        if ui
            .button(egui::RichText::new(format!("{} New File", ph::FILE_PLUS)).size(11.5))
            .clicked()
        {
            *new_src_name = Some(String::new());
            *new_file_parent_folder = Some("src".to_string());
            ui.close();
        }
        if ui
            .button(egui::RichText::new(format!("{} New Folder", ph::FOLDER_PLUS)).size(11.5))
            .clicked()
        {
            *new_src_folder_name = Some(String::new());
            *new_folder_parent_folder = Some("src".to_string());
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
    user_src_files: &[(String, String)],
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
) {
    let default_tree_folder_color = egui::Color32::from_rgb(100, 105, 115);

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
                let is_renaming = renaming_folder
                    .as_ref()
                    .map(|(f, _)| f == name)
                    .unwrap_or(false);

                if is_renaming {
                    let should_cancel = if let Some((_, new_name)) = renaming_folder.as_mut() {
                        let fid = egui::Id::new(("__rename_folder__", name.as_str()));
                        let mut cancel = false;
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
                            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                            cancel = enter || esc || resp.lost_focus();
                        });
                        cancel
                    } else {
                        false
                    };
                    if should_cancel {
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
                        if children.is_empty() {
                            ui.label(
                                egui::RichText::new("  (empty)")
                                    .size(10.0)
                                    .color(egui::Color32::from_gray(95)),
                            );
                        }
                        let folder_path = if parent_path == "src" {
                            name.clone()
                        } else {
                            format!("{parent_path}/{name}")
                        };
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
                        );
                    });

                    let folder_path = if parent_path == "src" {
                        name.clone()
                    } else {
                        format!("{parent_path}/{name}")
                    };
                    ch.header_response.context_menu(|ui| {
                        if ui
                            .button(
                                egui::RichText::new(format!("{} New File", ph::FILE_PLUS)).size(11.5),
                            )
                            .clicked()
                        {
                            *new_src_name = Some(String::new());
                            *new_file_parent_folder = Some(folder_path.clone());
                            ui.close();
                        }
                        if ui
                            .button(
                                egui::RichText::new(format!("{} New Folder", ph::FOLDER_PLUS)).size(11.5),
                            )
                            .clicked()
                        {
                            *new_src_folder_name = Some(String::new());
                            *new_folder_parent_folder = Some(folder_path.clone());
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .button(
                                egui::RichText::new(format!("{} Rename", ph::PENCIL_SIMPLE)).size(11.5),
                            )
                            .clicked()
                        {
                            *renaming_folder = Some((name.clone(), name.clone()));
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
                            for i in to_rm.into_iter().rev() {
                                let dest = workspace_dir.join(&user_src_files[i].0);
                                let _ = std::fs::remove_file(&dest);
                            }
                            // Remove from tracking (these will be auto-removed by polling)
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
    let normal = egui::Color32::from_rgb(200, 205, 215);

    ui.horizontal(|ui| {
        ui.add_space(indent);
        let is_sel = *selected == id;
        let color = if is_sel { hi } else { normal };
        ui.label(egui::RichText::new(ph::FILE).size(11.5).color(color));
        let resp = ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .size(11.5)
                    .monospace()
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
                ui.label(egui::RichText::new(ph::X_CIRCLE).size(10.0).color(
                    egui::Color32::from_rgb(220, 80, 70),
                ));
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
                let resp = ui
                    .add(egui::TextEdit::singleline(new_name).desired_width(ui.available_width()));
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
        let resp = ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .size(11.5)
                    .monospace()
                    .color(color),
            )
            .sense(egui::Sense::click()),
        );
        if resp.clicked() {
            *selected = id;
        }
        resp.context_menu(|ui| {
            if ui
                .button(egui::RichText::new(format!("{} Delete", ph::TRASH)).size(11.5))
                .clicked()
            {
                *to_delete = Some(idx);
                ui.close();
            }
        });
    });
}
