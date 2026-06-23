//! Rust-Analyzer diagnostics tab.
use eframe::egui;
use egui_phosphor::regular as ph;
use std::sync::{Arc, Mutex};
use crate::lsp;

pub fn show_ra_tab(
    ui: &mut egui::Ui,
    lsp_state: &Arc<Mutex<lsp::LspState>>,
    selected: &mut Option<usize>,
    // Set to `(rel_path, 1-based line)` when a row is expanded, so the editor
    // opens that file and scrolls to the line.
    nav: &mut Option<(String, usize)>,
) {
    // Extract everything we need while holding the lock, then drop it
    // before we start drawing so there's no risk of a deadlock.
    let (status, total_err, total_warn, all_diags, failed_msg) = {
        let lsp = lsp_state.lock().unwrap();
        let failed_msg = if let lsp::LspStatus::Failed(ref m) = lsp.status {
            Some(m.clone())
        } else {
            None
        };
        // Flatten all diagnostics into (rel_path, LspDiagnostic) pairs. While a
        // re-check is pending (`flycheck_stale`), drop flycheck (cargo check)
        // diagnostics: rustc's line/cols are stale and the error may already be
        // fixed — they'd otherwise linger here until the next check finishes.
        // RA's own (native) diagnostics re-map per edit, so keep them.
        let stale = lsp.flycheck_stale();
        let mut flat: Vec<(String, lsp::LspDiagnostic)> = lsp
            .diagnostics
            .iter()
            .flat_map(|(path, diags)| diags.iter().map(move |d| (path.clone(), d.clone())))
            .filter(|(_, d)| d.source == "rust-analyzer" || !stale)
            .collect();
        // Errors first, then warnings
        flat.sort_by_key(|(_, d)| (d.severity != lsp::DiagSeverity::Error, d.line));
        (
            lsp.status.clone(),
            lsp.total_errors(),
            lsp.total_warnings(),
            flat,
            failed_msg,
        )
    };

    // ── Status bar ────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let (icon, text, color) = match &status {
            lsp::LspStatus::Stopped => (
                ph::PLUGS,
                "rust-analyzer not running".to_owned(),
                egui::Color32::DARK_GRAY,
            ),
            lsp::LspStatus::Starting => (
                ph::CIRCLE_NOTCH,
                "rust-analyzer starting…".to_owned(),
                egui::Color32::from_rgb(180, 180, 80),
            ),
            lsp::LspStatus::Indexing => (
                ph::CIRCLE_NOTCH,
                "Indexing project…".to_owned(),
                egui::Color32::from_rgb(180, 180, 80),
            ),
            lsp::LspStatus::Ready if total_err > 0 => (
                ph::X_CIRCLE,
                format!(
                    "{} error{}{}",
                    total_err,
                    if total_err == 1 { "" } else { "s" },
                    if total_warn > 0 {
                        format!(
                            ",  {} warning{}",
                            total_warn,
                            if total_warn == 1 { "" } else { "s" }
                        )
                    } else {
                        String::new()
                    }
                ),
                egui::Color32::from_rgb(230, 90, 80),
            ),
            lsp::LspStatus::Ready if total_warn > 0 => (
                ph::WARNING,
                format!(
                    "{} warning{}",
                    total_warn,
                    if total_warn == 1 { "" } else { "s" }
                ),
                egui::Color32::from_rgb(230, 190, 50),
            ),
            lsp::LspStatus::Ready => (
                ph::CHECK_CIRCLE,
                "No issues — rust-analyzer ready".to_owned(),
                egui::Color32::from_rgb(80, 200, 100),
            ),
            lsp::LspStatus::Failed(_) => (
                ph::X_CIRCLE,
                "rust-analyzer failed to start".to_owned(),
                egui::Color32::from_rgb(230, 90, 80),
            ),
        };

        ui.label(egui::RichText::new(icon).size(13.0).color(color));
        ui.label(egui::RichText::new(text).size(12.0).color(color).strong());
    });

    // Spinner repaint
    if matches!(status, lsp::LspStatus::Starting | lsp::LspStatus::Indexing) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    }

    // Failed detail
    if let Some(msg) = failed_msg {
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("ra_failed_scroll")
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&msg)
                            .size(11.0)
                            .monospace()
                            .color(egui::Color32::from_rgb(230, 90, 80)),
                    )
                    .wrap(),
                );
            });
        return;
    }

    if all_diags.is_empty() {
        return;
    }

    ui.separator();

    // ── Diagnostic list ───────────────────────────────────────────────────────
    let sel = *selected;
    let list_height = if sel.is_some() {
        ui.available_height() * 0.45
    } else {
        ui.available_height()
    };

    egui::ScrollArea::vertical()
        .id_salt("ra_diag_list")
        .max_height(list_height)
        .show(ui, |ui| {
            for (i, (path, diag)) in all_diags.iter().enumerate() {
                let is_sel = sel == Some(i);

                let (level_icon, level_color) = match diag.severity {
                    lsp::DiagSeverity::Error => {
                        (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                    }
                    lsp::DiagSeverity::Warning => {
                        (ph::WARNING, egui::Color32::from_rgb(210, 170, 40))
                    }
                    lsp::DiagSeverity::Info | lsp::DiagSeverity::Hint => {
                        (ph::INFO, egui::Color32::from_rgb(80, 140, 210))
                    }
                };

                let location = format!("{}:{}", path, diag.line);

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

                    let cy = rect.center().y;
                    let mut x = rect.left() + 4.0;

                    // Severity icon
                    let r = painter.text(
                        egui::pos2(x, cy),
                        egui::Align2::LEFT_CENTER,
                        level_icon,
                        egui::FontId::proportional(11.0),
                        level_color,
                    );
                    x = r.right() + 4.0;

                    // file:line
                    let r = painter.text(
                        egui::pos2(x, cy),
                        egui::Align2::LEFT_CENTER,
                        &location,
                        egui::FontId::monospace(10.5),
                        egui::Color32::from_rgb(120, 160, 200),
                    );
                    x = r.right() + 6.0;

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

                    // Message — first line only (the full body is shown in the
                    // detail view below when the row is expanded/selected).
                    let msg_color = if is_sel {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_rgb(210, 210, 220)
                    };
                    let headline = if diag.has_more_lines() {
                        format!("{}  …", diag.headline())
                    } else {
                        diag.headline().to_owned()
                    };
                    painter.text(
                        egui::pos2(x, cy),
                        egui::Align2::LEFT_CENTER,
                        headline,
                        egui::FontId::proportional(11.0),
                        msg_color,
                    );
                }

                if resp.clicked() {
                    let now_selected = !is_sel;
                    *selected = if now_selected { Some(i) } else { None };
                    // On expand, ask the editor to open this file and scroll to
                    // the diagnostic line (resolved in `diag_embed`).
                    if now_selected {
                        *nav = Some((path.clone(), diag.line as usize));
                    }
                }
            }
        });

    // ── Detail view ───────────────────────────────────────────────────────────
    if let Some(idx) = sel {
        if let Some((_, diag)) = all_diags.get(idx) {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("ra_diag_detail")
                .show(ui, |ui| {
                    // Header: severity + code + location
                    ui.horizontal(|ui| {
                        let (sev_icon, sev_col) = match diag.severity {
                            lsp::DiagSeverity::Error => {
                                (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                            }
                            lsp::DiagSeverity::Warning => {
                                (ph::WARNING, egui::Color32::from_rgb(210, 170, 40))
                            }
                            _ => (ph::INFO, egui::Color32::from_rgb(80, 140, 210)),
                        };
                        ui.label(egui::RichText::new(sev_icon).size(13.0).color(sev_col));
                        if let Some(code) = &diag.code {
                            ui.label(
                                egui::RichText::new(format!("[{code}]"))
                                    .size(11.0)
                                    .monospace()
                                    .color(egui::Color32::from_rgb(180, 155, 80)),
                            );
                        }
                        ui.label(
                            egui::RichText::new(format!("line {}  col {}", diag.line, diag.col))
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        );
                    });

                    // Full message body
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&diag.message)
                                .size(11.5)
                                .color(egui::Color32::from_rgb(220, 215, 200)),
                        )
                        .wrap(),
                    );
                });
        }
    }
}

