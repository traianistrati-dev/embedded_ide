//! Required Tools tab.
use eframe::egui;
use egui_phosphor::regular as ph;
use std::sync::{Arc, Mutex};
use crate::required_tools;
use crate::panels::mcu_module::ToolchainKind;

pub fn show_tools_tab(
    ui: &mut egui::Ui,
    tools_state: &Arc<Mutex<required_tools::ToolsState>>,
    ctx: &egui::Context,
) {
    use required_tools::ToolStatus;

    // Snapshot all state before rendering (avoids holding the lock during draw)
    let (rows, any_busy, missing_count, log) = {
        let s = tools_state.lock().unwrap();
        (
            s.snapshot(),
            s.any_busy(),
            s.missing_installable_count(),
            s.log.clone(),
        )
    };

    // ── Toolbar ───────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !any_busy,
                egui::Button::new(
                    egui::RichText::new(format!("{} Check All", ph::MAGNIFYING_GLASS)).size(11.0),
                ),
            )
            .on_hover_text("Check every tool / target in the list")
            .clicked()
        {
            required_tools::start_check_all(Arc::clone(tools_state), ctx.clone());
        }

        let install_label = if missing_count > 0 {
            format!("Install Missing ({})", missing_count)
        } else {
            "Install Missing".to_string()
        };
        if ui
            .add_enabled(
                !any_busy && missing_count > 0,
                egui::Button::new(egui::RichText::new(install_label).size(11.0)),
            )
            .on_hover_text("Auto-install every missing tool that supports it")
            .clicked()
        {
            required_tools::start_install_missing(Arc::clone(tools_state), ctx.clone());
        }
    });

    ui.add_space(2.0);
    ui.separator();

    // ── Tools grid ────────────────────────────────────────────────────────────
    let available_h = ui.available_height();
    // Reserve ~30 % of the panel for the log area (min 60 px, max 110 px)
    let log_h = (available_h * 0.30).clamp(60.0, 110.0);
    let grid_h = (available_h - log_h - 20.0).max(40.0);

    egui::ScrollArea::vertical()
        .id_salt("tools_grid_scroll")
        .max_height(grid_h)
        .show(ui, |ui| {
            egui::Grid::new("tools_grid")
                .num_columns(5)
                .striped(true)
                .min_col_width(50.0)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    // Header row
                    let hdr = |text: &str| {
                        egui::RichText::new(text)
                            .strong()
                            .size(10.5)
                            .color(egui::Color32::from_rgb(150, 160, 180))
                    };
                    ui.label(hdr("Tool"));
                    ui.label(hdr("For"));
                    ui.label(hdr("Status"));
                    ui.label(hdr("Version"));
                    ui.label(hdr("Action"));
                    ui.end_row();

                    for (idx, row) in rows.iter().enumerate() {
                        // ── Tool name + description tooltip ────────────────
                        ui.label(egui::RichText::new(row.name).monospace().size(10.5))
                            .on_hover_text(row.description);

                        // ── Toolchain column ──────────────────────────────
                        let tc_label = match &row.toolchain {
                            None => "All",
                            Some(ToolchainKind::RustEmbedded) => "STM32",
                            Some(ToolchainKind::EspRust) => "ESP32-C3",
                            Some(ToolchainKind::SdccC) => "SDCC",
                        };
                        ui.label(
                            egui::RichText::new(tc_label)
                                .size(10.5)
                                .color(egui::Color32::GRAY),
                        );

                        // ── Status badge ───────────────────────────────────
                        ui.label(
                            egui::RichText::new(row.status.label())
                                .size(10.5)
                                .color(row.status.color()),
                        );

                        // ── Version string ─────────────────────────────────
                        let ver = match &row.status {
                            ToolStatus::Ok(v) => v.as_str(),
                            _ => "—",
                        };
                        ui.label(
                            egui::RichText::new(ver)
                                .monospace()
                                .size(10.0)
                                .color(egui::Color32::from_rgb(140, 150, 170)),
                        );

                        // ── Action buttons ─────────────────────────────────
                        let busy = row.status.is_busy();
                        ui.horizontal(|ui| {
                            // Check button — always shown, disabled while busy
                            if ui
                                .add_enabled(
                                    !busy,
                                    egui::Button::new(
                                        egui::RichText::new(ph::MAGNIFYING_GLASS).size(11.0),
                                    )
                                    .small(),
                                )
                                .on_hover_text("Re-check this tool")
                                .clicked()
                            {
                                required_tools::start_check(
                                    idx,
                                    Arc::clone(tools_state),
                                    ctx.clone(),
                                );
                            }

                            // Install button — only when missing/failed AND auto-installable
                            if row.can_auto_install
                                && matches!(row.status, ToolStatus::Missing | ToolStatus::Failed(_))
                            {
                                if ui
                                    .add_enabled(
                                        !busy,
                                        egui::Button::new(
                                            egui::RichText::new("Install").size(10.5),
                                        )
                                        .small(),
                                    )
                                    .on_hover_text("Auto-install this tool")
                                    .clicked()
                                {
                                    required_tools::start_install(
                                        idx,
                                        Arc::clone(tools_state),
                                        ctx.clone(),
                                    );
                                }
                            }

                            // Manual URL link — for tools without auto-install
                            if !row.can_auto_install
                                && matches!(
                                    row.status,
                                    ToolStatus::Missing
                                        | ToolStatus::Unknown
                                        | ToolStatus::Failed(_)
                                )
                            {
                                ui.hyperlink_to(
                                    egui::RichText::new("Get…")
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(100, 160, 220)),
                                    row.manual_url,
                                )
                                .on_hover_text(row.manual_url);
                            }
                        });

                        ui.end_row();
                    }
                });
        });

    // ── Log area ──────────────────────────────────────────────────────────────
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("tools_log_scroll")
        .stick_to_bottom(true)
        .max_height(log_h)
        .show(ui, |ui| {
            if log.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "Click \"Check All\" to verify your toolchain setup.\n\
                         Use \"Install\" buttons to auto-install missing components.",
                    )
                    .size(10.5)
                    .color(egui::Color32::GRAY),
                );
                return;
            }
            for line in &log {
                let color = if line.starts_with("  [OK]") || line.starts_with("[OK]") {
                    egui::Color32::from_rgb(80, 200, 100)
                } else if line.starts_with("  [X]") || line.starts_with("[X]") {
                    egui::Color32::from_rgb(220, 80, 70)
                } else if line.starts_with(">") {
                    egui::Color32::from_rgb(100, 180, 255)
                } else {
                    egui::Color32::from_rgb(175, 180, 192)
                };
                ui.label(
                    egui::RichText::new(line.as_str())
                        .monospace()
                        .size(10.0)
                        .color(color),
                );
            }
        });
}
