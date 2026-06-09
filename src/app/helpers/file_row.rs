// //! File row rendering for project tree — displays project files with diagnostic indicators.

// use eframe::egui;
// use egui_phosphor::regular as ph;
// use crate::app::ProjectFileId;
// use crate::build;
// use crate::lsp;

// /// Render a single file row for fixed project files (main.rs, build.rs, Cargo.toml).
// ///
// /// Displays the filename, selection highlight, and diagnostic indicators (errors/warnings).
// pub fn file_row(
//     ui: &mut egui::Ui,
//     indent: f32,
//     name: &str,
//     id: ProjectFileId,
//     selected: &mut ProjectFileId,
//     build_result: Option<&build::BuildResult>,
//     lsp: Option<&lsp::LspState>,
// ) {
//     let dim = egui::Color32::from_rgb(140, 150, 165);
//     let hi = egui::Color32::from_rgb(100, 180, 255);
//     let normal = egui::Color32::from_rgb(200, 205, 215);

//     ui.horizontal(|ui| {
//         ui.add_space(indent);
//         let is_sel = *selected == id;
//         let color = if is_sel { hi } else { normal };
//         let icon_color = if is_sel {
//             egui::Color32::from_rgb(180, 210, 255)
//         } else {
//             egui::Color32::from_rgb(160, 170, 190)
//         };
//         ui.label(egui::RichText::new(ph::FILE).size(11.5).color(icon_color));
//         let resp = ui.add(
//             egui::Label::new(
//                 egui::RichText::new(name)
//                     .size(11.5)
//                     .monospace()
//                     .color(color),
//             )
//             .sense(egui::Sense::click()),
//         );
//         if resp.clicked() {
//             *selected = id;
//         }
//         if resp.hovered() && !is_sel {
//             let r = resp.rect;
//             ui.painter().line_segment(
//                 [r.left_bottom(), r.right_bottom()],
//                 egui::Stroke::new(1.0, dim),
//             );
//         }
//         if let Some(cargo_path) = id.cargo_path() {
//             let cargo_err = build_result.map_or(false, |r| r.has_errors_in(cargo_path));
//             let cargo_warn = build_result.map_or(false, |r| r.has_warnings_in(cargo_path));
//             let lsp_err = lsp.map_or(false, |l| l.error_count_for(cargo_path) > 0);
//             let lsp_warn = lsp.map_or(false, |l| l.warning_count_for(cargo_path) > 0);
//             let has_err = cargo_err || lsp_err;
//             let has_warn = cargo_warn || lsp_warn;
//             let (dot, dot_color) = if has_err {
//                 (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
//             } else if has_warn {
//                 (ph::WARNING, egui::Color32::from_rgb(220, 180, 50))
//             } else {
//                 ("", egui::Color32::TRANSPARENT)
//             };
//             if !dot.is_empty() {
//                 ui.label(egui::RichText::new(dot).size(10.0).color(dot_color));
//             }
//         }
//     });
// }

// /// Render a file row for user-created source files (with delete button).
// ///
// /// Displays the filename with a context menu for renaming or deleting the file.
// pub fn user_file_row(
//     ui: &mut egui::Ui,
//     indent: f32,
//     name: &str,
//     idx: usize,
//     selected: &mut ProjectFileId,
//     to_delete: &mut Option<usize>,
//     renaming: &mut Option<(usize, String)>,
//     do_rename: &mut Option<usize>,
//     cancel_rename: &mut bool,
// ) {
//     let hi = egui::Color32::from_rgb(100, 180, 255);
//     let normal = egui::Color32::from_rgb(200, 205, 215);
//     let id = ProjectFileId::UserFile(idx);
//     let is_renaming = renaming.as_ref().map(|(i, _)| *i == idx).unwrap_or(false);

//     // ── Inline rename mode ────────────────────────────────────────────────
//     if is_renaming {
//         ui.horizontal(|ui| {
//             ui.add_space(indent);
//             ui.label(
//                 egui::RichText::new(ph::FILE)
//                     .size(11.5)
//                     .color(egui::Color32::from_rgb(180, 210, 255)),
//             );
//             if let Some((_, new_name)) = renaming.as_mut() {
//                 let fid = egui::Id::new(("__rename_file__", idx));
//                 let resp = ui
//                     .add(egui::TextEdit::singleline(new_name).desired_width(ui.available_width()));
//                 if ui.memory(|m| m.data.get_temp::<bool>(fid).unwrap_or(true)) {
//                     resp.request_focus();
//                     ui.memory_mut(|m| m.data.insert_temp(fid, false));
//                 }
//                 let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
//                 let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
//                 if enter {
//                     *do_rename = Some(idx);
//                 } else if esc || resp.lost_focus() {
//                     *cancel_rename = true;
//                 }
//             }
//         });
//         return;
//     }

//     // ── Normal display mode ───────────────────────────────────────────────
//     ui.horizontal(|ui| {
//         ui.add_space(indent);
//         let is_sel = *selected == id;
//         let color = if is_sel { hi } else { normal };
//         let icon_color = if is_sel {
//             egui::Color32::from_rgb(180, 210, 255)
//         } else {
//             egui::Color32::from_rgb(160, 170, 190)
//         };
//         ui.label(egui::RichText::new(ph::FILE).size(11.5).color(icon_color));
//         let resp = ui.add(
//             egui::Label::new(
//                 egui::RichText::new(name)
//                     .size(11.5)
//                     .monospace()
//                     .color(color),
//             )
//             .sense(egui::Sense::click()),
//         );
//         if resp.clicked() {
//             *selected = id;
//         }
//         resp.context_menu(|ui| {
//             if ui
//                 .button(egui::RichText::new(format!("{} Rename", ph::PENCIL_SIMPLE)).size(11.5))
//                 .clicked()
//             {
//                 *renaming = Some((idx, name.to_string()));
//                 ui.close();
//             }
//             ui.separator();
//             if ui
//                 .button(
//                     egui::RichText::new(format!("{} Delete", ph::TRASH))
//                         .size(11.5)
//                         .color(egui::Color32::from_rgb(220, 80, 60)),
//                 )
//                 .clicked()
//             {
//                 *to_delete = Some(idx);
//                 ui.close();
//             }
//         });
//     });
// }
