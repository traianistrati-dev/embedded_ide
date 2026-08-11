//! Required Tools tab.
use crate::panels::mcu_module::ToolchainKind;
use crate::required_tools;
use eframe::egui;
use egui_phosphor::regular as ph;
use std::sync::{Arc, Mutex};

/// Write the generated udev rules next to the IDE's other per-user files and
/// report the install command in the tools log.
///
/// The command goes to the LOG rather than a dialog on purpose: the log is
/// selectable text that survives the rest of the session, so the user can come
/// back to it after plugging the probe in. The clipboard gets it too, because
/// retyping a path is where this goes wrong.
fn stage_udev_rules(tools_state: &Arc<Mutex<required_tools::ToolsState>>, ctx: &egui::Context) {
    let mut s = tools_state.lock().unwrap();
    match crate::udev::write_staged_rules() {
        Ok(path) => {
            let cmd = crate::udev::install_command(&path);
            s.push_log_public(format!("[OK] rules written to {}", path.display()));
            s.push_log_public("  Install them with (needs root):".to_string());
            s.push_log_public(format!("  {cmd}"));
            s.push_log_public(
                "  Then UNPLUG AND REPLUG the probe — udev applies rules at plug time.".to_string(),
            );
            ctx.copy_text(cmd);
            s.push_log_public("  (command copied to the clipboard)".to_string());
        }
        Err(e) => s.push_log_public(format!("[X] could not write the rules file: {e}")),
    }
    ctx.request_repaint();
}

/// Report the serial-access fix in the tools log + clipboard.
///
/// The re-login warning is not decoration: `usermod` changes the database, but a
/// running session keeps the group set it was started with, so the user runs the
/// command, sees no change, and concludes it didn't work.
fn show_serial_access_fix(
    tools_state: &Arc<Mutex<required_tools::ToolsState>>,
    ctx: &egui::Context,
) {
    let (group, cmd) = required_tools::serial_access_fix();
    let mut s = tools_state.lock().unwrap();
    s.push_log_public(format!("> Serial port is owned by group `{group}`."));
    s.push_log_public(format!("  {cmd}"));
    s.push_log_public("  Then LOG OUT and back in — a running session keeps its old groups.");
    ctx.copy_text(cmd);
    s.push_log_public("  (command copied to the clipboard)".to_string());
    ctx.request_repaint();
}

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

    // ── Grid on the left, log on the right ────────────────────────────────────
    // Side by side rather than stacked: the log is a running commentary on the
    // rows above it, and reading a 110 px strip while the list scrolls away was
    // the worse half of the deal.
    //
    // Both halves are child uis at FIXED rects (`new_child` + `set_clip_rect`),
    // the pattern from `debug_tab`'s pane row: a child's min_rect never feeds
    // back into the parent, so nothing here can re-widen the Code Editor side
    // panel this tab lives inside.
    let avail_h = ui.available_height();
    let total_w = ui.available_width();
    let gap = 8.0;
    // The split follows the TABLE's own width, measured last frame from the
    // scroll area's content size: the log then starts right where the rows end,
    // instead of after a fixed fraction that leaves a gap on a wide panel and
    // cuts the Action column off on a narrow one. Clamped both ways so neither
    // half can be squeezed out; one frame of lag on a resize, self-correcting.
    /// The log is not worth a column narrower than this.
    const MIN_LOG_W: f32 = 260.0;
    let measure_id = ui.id().with("tools_grid_width");
    let measured: f32 = ui.data(|d| d.get_temp(measure_id)).unwrap_or(0.0);
    let table_w = if measured > 0.0 {
        measured
    } else {
        total_w * 0.62
    };
    // Side by side ONLY while the table fits at its natural width AND the log
    // still gets a usable column. Otherwise the log goes back under the table:
    // squeezing the split instead would cut the Action column off and read as
    // the log covering the list.
    let side_by_side = total_w >= table_w + gap + MIN_LOG_W;
    let (row, _) = ui.allocate_exact_size(egui::vec2(total_w, avail_h), egui::Sense::hover());
    let (grid_rect, log_rect) = if side_by_side {
        let gw = table_w;
        (
            egui::Rect::from_min_size(row.min, egui::vec2(gw, avail_h)),
            egui::Rect::from_min_size(
                egui::pos2(row.left() + gw + gap, row.top()),
                egui::vec2(total_w - gw - gap, avail_h),
            ),
        )
    } else {
        let log_h = (avail_h * 0.32).clamp(70.0, 160.0);
        let grid_h = (avail_h - log_h - gap).max(40.0);
        (
            egui::Rect::from_min_size(row.min, egui::vec2(total_w, grid_h)),
            egui::Rect::from_min_size(
                egui::pos2(row.left(), row.top() + grid_h + gap),
                egui::vec2(total_w, log_h),
            ),
        )
    };
    let child = |ui: &mut egui::Ui, rect: egui::Rect| -> egui::Ui {
        let mut c = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        c.set_clip_rect(rect.intersect(ui.clip_rect()));
        c
    };
    let sep = ui.visuals().widgets.noninteractive.bg_stroke;
    if side_by_side {
        ui.painter()
            .vline(grid_rect.right() + gap * 0.5, row.y_range(), sep);
    } else {
        ui.painter()
            .hline(row.x_range(), grid_rect.bottom() + gap * 0.5, sep);
    }

    let grid_ui = &mut child(ui, grid_rect);
    // Scrolls BOTH ways: the five columns no longer have the whole panel, and a
    // grid that can't scroll sideways would demand the width instead.
    let grid_out = egui::ScrollArea::both()
        .id_salt("tools_grid_scroll")
        .max_height(grid_rect.height())
        .auto_shrink([false, false])
        .show(grid_ui, |ui| {
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

                        // ── Severity badge — answers "do I actually need this?"
                        ui.label(
                            egui::RichText::new(row.severity.label())
                                .size(10.0)
                                .color(row.severity.color()),
                        )
                        .on_hover_text(row.impact);

                        // ── Status badge ───────────────────────────────────
                        let status_hover = match &row.status {
                            ToolStatus::Outdated { found, min } => format!(
                                "Found {found}, but this IDE needs {min} or newer. \
                                 It may still work — update it with the button on the right.\n\n{}",
                                row.impact
                            ),
                            _ => row.impact.to_owned(),
                        };
                        ui.label(
                            egui::RichText::new(row.status.label())
                                .size(10.5)
                                .color(row.status.color()),
                        )
                        .on_hover_text(status_hover);

                        // ── Version string ─────────────────────────────────
                        let ver = match &row.status {
                            ToolStatus::Ok(v) => v.as_str(),
                            // Show what WAS found, so the gap to the minimum is
                            // visible without hovering.
                            ToolStatus::Outdated { found, .. } => found.as_str(),
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
                                && matches!(
                                    row.status,
                                    ToolStatus::Missing
                                        | ToolStatus::Failed(_)
                                        // An outdated tool offers the same
                                        // action — installing again upgrades it.
                                        | ToolStatus::Outdated { .. }
                                )
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

                            // The udev row can't be "installed" (writing to
                            // /etc needs root), but the IDE CAN generate the
                            // rules file from its own programmer catalogue and
                            // hand over the one command that installs it —
                            // which beats sending the user to a web page to
                            // assemble a list of VID:PIDs by hand.
                            if row.name == required_tools::UDEV_RULES_TOOL {
                                if ui
                                    .add_enabled(
                                        !busy,
                                        egui::Button::new(
                                            egui::RichText::new("Generate rules…").size(10.5),
                                        )
                                        .small(),
                                    )
                                    .on_hover_text(
                                        "Write a udev rules file covering every probe this IDE \
                                         knows, then show the sudo command that installs it",
                                    )
                                    .clicked()
                                {
                                    stage_udev_rules(tools_state, ctx);
                                }
                            }

                            // Serial access can't be "installed" either — the fix
                            // is a group change that needs root. What the IDE can
                            // do is name the RIGHT group: read off the device
                            // actually plugged in, so the command is correct on
                            // Arch (`uucp`) as well as Debian (`dialout`).
                            if row.name == required_tools::SERIAL_ACCESS_TOOL
                                && matches!(row.status, ToolStatus::Missing | ToolStatus::Failed(_))
                            {
                                if ui
                                    .add_enabled(
                                        !busy,
                                        egui::Button::new(
                                            egui::RichText::new("Fix access…").size(10.5),
                                        )
                                        .small(),
                                    )
                                    .on_hover_text(
                                        "Show (and copy) the command that grants this user \
                                         access to the serial port",
                                    )
                                    .clicked()
                                {
                                    show_serial_access_fix(tools_state, ctx);
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

    // What the table actually needed, for next frame's split (plus the room a
    // vertical scrollbar takes, so the measurement doesn't oscillate).
    ui.data_mut(|d| d.insert_temp(measure_id, grid_out.content_size.x + 18.0));

    // ── Log area — beside the table, or under it on a narrow panel ────────────
    let log_ui = &mut child(ui, log_rect);
    egui::ScrollArea::vertical()
        .id_salt("tools_log_scroll")
        .stick_to_bottom(true)
        .max_height(log_rect.height())
        .auto_shrink([false, false])
        .show(log_ui, |ui| {
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
                // Wrapped: the log is a narrow column now, and a clipped tail
                // is exactly where the reason for a failure tends to sit.
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(line.as_str())
                            .monospace()
                            .size(10.0)
                            .color(color),
                    )
                    .wrap(),
                );
            }
        });
}
