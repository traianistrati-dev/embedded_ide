//! Cargo build diagnostics tab.
use eframe::egui;
use egui_phosphor::regular as ph;
use std::sync::{Arc, Mutex};
use crate::build::{self, BuildState};

pub fn show_cargo_tab(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    build_state: &Arc<Mutex<BuildState>>,
    selected_diagnostic: &mut Option<usize>,
    // Set to `(rel_path, 1-based line)` when a row is expanded, so the editor
    // opens that file and scrolls to the line.
    nav: &mut Option<(String, usize)>,
) {
    let state = build_state.lock().unwrap().clone();
    let workspace = std::env::temp_dir().join("embedded_ide_0_check");

    // ── Status bar ────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let (icon, text, color) = match &state {
            BuildState::Idle => return,
            BuildState::Building => (
                ph::HAMMER,
                "Building…".to_owned(),
                egui::Color32::from_rgb(180, 180, 180),
            ),
            BuildState::Failed(msg) => {
                let first = msg.lines().next().unwrap_or(msg);
                // Suppress the [DISK_FULL] prefix from the one-liner badge
                let first = first.strip_prefix("[DISK_FULL] ").unwrap_or(first);
                (
                    ph::X_CIRCLE,
                    format!("Build failed: {}", first),
                    egui::Color32::from_rgb(230, 90, 80),
                )
            }
            BuildState::Done(r) if r.error_count() > 0 => (
                ph::X_CIRCLE,
                format!(
                    "{} error{}{}",
                    r.error_count(),
                    if r.error_count() == 1 { "" } else { "s" },
                    if r.warning_count() > 0 {
                        format!(
                            ",  {} warning{}",
                            r.warning_count(),
                            if r.warning_count() == 1 { "" } else { "s" }
                        )
                    } else {
                        String::new()
                    }
                ),
                egui::Color32::from_rgb(230, 90, 80),
            ),
            BuildState::Done(r) if r.warning_count() > 0 => (
                ph::WARNING,
                format!(
                    "{} warning{}",
                    r.warning_count(),
                    if r.warning_count() == 1 { "" } else { "s" }
                ),
                egui::Color32::from_rgb(230, 190, 50),
            ),
            BuildState::Done(_) => (
                ph::CHECK_CIRCLE,
                "Build succeeded — no errors".to_owned(),
                egui::Color32::from_rgb(80, 200, 100),
            ),
        };

        ui.label(egui::RichText::new(icon).size(13.0).color(color));
        ui.label(egui::RichText::new(text).size(12.0).color(color).strong());

        // Right-side buttons: Clean target/ | Clear
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Clear result button
            if ui
                .add(egui::Button::new(
                    egui::RichText::new(format!("{} Clear", ph::X))
                        .size(10.0)
                        .color(egui::Color32::GRAY),
                ))
                .clicked()
            {
                *build_state.lock().unwrap() = BuildState::Idle;
                *selected_diagnostic = None;
            }

            ui.add_space(4.0);

            // "Clean target/" — deletes cached LLVM/cargo build artefacts to
            // recover disk space.  Shown always; especially helpful after a
            // disk-full build failure.
            let is_building = matches!(state, BuildState::Building);
            if ui
                .add_enabled(
                    !is_building,
                    egui::Button::new(
                        egui::RichText::new(format!("{} Clean target/", ph::TRASH))
                            .size(10.0)
                            .color(egui::Color32::from_rgb(200, 160, 80)),
                    ),
                )
                .on_hover_text(
                    "Run `cargo clean` — deletes the target/ directory to free disk space.\n\
                     Crates cached in ~/.cargo are NOT removed; only rebuilt files are re-compiled.",
                )
                .clicked()
            {
                build::start_clean(workspace.clone(), Arc::clone(build_state), ctx.clone());
                *selected_diagnostic = None;
            }
        });
    });

    let BuildState::Done(result) = &state else {
        // For Building/Failed we've shown what we can
        if let BuildState::Failed(msg) = &state {
            ui.separator();

            // ── Special disk-full banner ──────────────────────────────────────
            let is_disk_full = msg.starts_with("[DISK_FULL]");
            if is_disk_full {
                // Orange warning box with an inline "Clean target/" button
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(60, 45, 10))
                    .inner_margin(egui::Margin::same(8))
                    .corner_radius(egui::CornerRadius::same(4))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{} Disk full", ph::WARNING))
                                    .size(12.0)
                                    .color(egui::Color32::from_rgb(250, 190, 60))
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new(
                                    " — the build target/ directory ran out of space.",
                                )
                                .size(11.0)
                                .color(egui::Color32::from_rgb(230, 210, 140)),
                            );
                        });
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(
                                "ESP32 / RISC-V builds produce several GB of LLVM artefacts \
                                 on the first run.  Click the button to delete the target/ \
                                 folder and free that space — crates cached in ~/.cargo are \
                                 NOT removed so the next build only re-compiles changed files.",
                            )
                            .size(10.5)
                            .color(egui::Color32::from_rgb(200, 190, 150)),
                        );
                        ui.add_space(6.0);
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new(format!(
                                    "{} Clean target/  (free space)",
                                    ph::TRASH
                                ))
                                .size(11.0)
                                .color(egui::Color32::from_rgb(255, 210, 80)),
                            ))
                            .clicked()
                        {
                            build::start_clean(
                                workspace.clone(),
                                Arc::clone(build_state),
                                ctx.clone(),
                            );
                            *selected_diagnostic = None;
                        }
                    });
                ui.add_space(4.0);
            }

            // Full error text in a scroll area
            egui::ScrollArea::vertical()
                .id_salt("build_failed_scroll")
                .show(ui, |ui| {
                    // Strip the [DISK_FULL] marker before display
                    let display_msg = msg.strip_prefix("[DISK_FULL] ").unwrap_or(msg.as_str());
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(display_msg)
                                .size(11.0)
                                .monospace()
                                .color(egui::Color32::from_rgb(230, 90, 80)),
                        )
                        .wrap(),
                    );
                });
        }
        return;
    };

    if result.diagnostics.is_empty() {
        return;
    }

    ui.separator();

    // ── Compact diagnostic list ───────────────────────────────────────────────
    let sel = *selected_diagnostic;

    // If something is selected, split the panel: list on top, detail below
    let list_height = if sel.is_some() {
        ui.available_height() * 0.45
    } else {
        ui.available_height()
    };

    egui::ScrollArea::vertical()
        .id_salt("build_diag_list")
        .max_height(list_height)
        .show(ui, |ui| {
            for (i, diag) in result.diagnostics.iter().enumerate() {
                let is_sel = sel == Some(i);

                let (level_icon, level_color) = if diag.is_error() {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                } else {
                    (ph::WARNING, egui::Color32::from_rgb(210, 170, 40))
                };

                let location = match (diag.file.as_deref(), diag.line) {
                    (Some(f), Some(l)) => format!("{f}:{l}"),
                    (Some(f), None) => f.to_owned(),
                    _ => String::new(),
                };

                let row_bg = if is_sel {
                    egui::Color32::from_rgba_premultiplied(60, 80, 110, 180)
                } else {
                    egui::Color32::TRANSPARENT
                };

                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 18.0),
                    egui::Sense::click(),
                );

                if ui.is_rect_visible(rect) {
                    let painter = ui.painter();
                    painter.rect_filled(rect, 2.0, row_bg);

                    // painter.text() returns the Rect of the rendered text,
                    // letting us advance x without needing &mut Fonts.
                    let cy = rect.center().y;
                    let mut x = rect.left() + 4.0;

                    // Level icon
                    let r = painter.text(
                        egui::pos2(x, cy),
                        egui::Align2::LEFT_CENTER,
                        level_icon,
                        egui::FontId::proportional(11.0),
                        level_color,
                    );
                    x = r.right() + 4.0;

                    // File:line location
                    if !location.is_empty() {
                        let r = painter.text(
                            egui::pos2(x, cy),
                            egui::Align2::LEFT_CENTER,
                            &location,
                            egui::FontId::monospace(10.5),
                            egui::Color32::from_rgb(120, 160, 200),
                        );
                        x = r.right() + 6.0;
                    }

                    // Error code [E0308]
                    if let Some(code) = &diag.code {
                        let r = painter.text(
                            egui::pos2(x, cy),
                            egui::Align2::LEFT_CENTER,
                            format!("[{code}]"),
                            egui::FontId::monospace(10.0),
                            egui::Color32::from_rgb(150, 130, 80),
                        );
                        x = r.right() + 6.0;
                    }

                    // Message text
                    let msg_color = if is_sel {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(210, 210, 220)
                    };
                    painter.text(
                        egui::pos2(x, cy),
                        egui::Align2::LEFT_CENTER,
                        &diag.message,
                        egui::FontId::proportional(11.0),
                        msg_color,
                    );
                }

                if resp.clicked() {
                    let now_selected = !is_sel;
                    *selected_diagnostic = if now_selected { Some(i) } else { None };
                    // On expand, ask the editor to open this file and scroll to
                    // the diagnostic line (resolved in `diag_embed`).
                    if now_selected {
                        if let (Some(file), Some(line)) = (&diag.file, diag.line) {
                            *nav = Some((file.clone(), line as usize));
                        }
                    }
                }
            }
        });

    // ── Detail view for selected diagnostic ───────────────────────────────────
    if let Some(idx) = sel {
        if let Some(diag) = result.diagnostics.get(idx) {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("build_diag_detail")
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&diag.rendered)
                                .size(11.0)
                                .monospace()
                                .color(egui::Color32::from_rgb(220, 215, 200)),
                        )
                        .wrap(),
                    );
                });
        }
    }
}

