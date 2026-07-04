//! Activity tab — a chronological, per-action timing breakdown of every Save /
//! Build / Flash / Clippy, so the user can SEE what runs and where the time
//! goes. Populated by [`crate::activity`]; newest action first, each showing its
//! phases with durations, the exact command line, and exit code.

use crate::activity::{fmt_dur, ActivityLog};
use eframe::egui;
use egui_phosphor::regular as ph;
use std::sync::{Arc, Mutex};

pub fn show_activity_tab(ui: &mut egui::Ui, activity: &Arc<Mutex<ActivityLog>>) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} Activity — timing per Save / Build / Flash", ph::TIMER))
                .size(11.0)
                .color(egui::Color32::from_rgb(160, 185, 215)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(egui::RichText::new(format!("{} Clear", ph::BROOM)).size(11.0))
                .clicked()
            {
                activity.lock().unwrap().clear();
            }
        });
    });
    ui.separator();

    let log = activity.lock().unwrap();
    if log.actions.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "No activity yet. Save, Build (Cargo Check), run Clippy, or Flash — \
                 each run's phases and durations appear here so you can see what's slow.",
            )
            .size(11.0)
            .color(egui::Color32::DARK_GRAY),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("activity_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, action) in log.actions.iter().enumerate() {
                // Header: "Flash (DFU)   total 41.0s   @123.4s"
                let slow = action.total.as_secs_f64() >= 1.0;
                let head = format!(
                    "{}  ·  total {}",
                    action.kind,
                    fmt_dur(action.total),
                );
                egui::CollapsingHeader::new(
                    egui::RichText::new(head)
                        .size(11.5)
                        .strong()
                        .color(if slow {
                            egui::Color32::from_rgb(220, 180, 60)
                        } else {
                            egui::Color32::from_rgb(150, 200, 130)
                        }),
                )
                .id_salt(("activity_action", i))
                .default_open(i == 0) // expand the newest
                .show(ui, |ui| {
                    for ph_ in &action.phases {
                        let exit_bad = matches!(ph_.exit, Some(c) if c != 0);
                        // "  ├ cargo check   2.41s   [exit 0]"
                        let mut job = egui::text::LayoutJob::default();
                        let dur_col = egui::Color32::from_rgb(120, 170, 230);
                        job.append(
                            &format!("  {}", ph_.label),
                            0.0,
                            fmt(egui::Color32::from_gray(210)),
                        );
                        job.append(
                            &format!("   {}", fmt_dur(ph_.dur)),
                            0.0,
                            fmt(dur_col),
                        );
                        if let Some(code) = ph_.exit {
                            job.append(
                                &format!("   [exit {code}]"),
                                0.0,
                                fmt(if exit_bad {
                                    egui::Color32::from_rgb(230, 100, 90)
                                } else {
                                    egui::Color32::from_rgb(120, 190, 120)
                                }),
                            );
                        }
                        ui.label(job);
                        // The exact command line, indented, dimmer.
                        if let Some(cmd) = &ph_.cmd {
                            ui.label(
                                egui::RichText::new(format!("      $ {cmd}"))
                                    .size(10.5)
                                    .monospace()
                                    .color(egui::Color32::from_gray(130)),
                            );
                        }
                    }
                });
            }
        });
}

fn fmt(color: egui::Color32) -> egui::TextFormat {
    egui::TextFormat {
        font_id: egui::FontId::monospace(11.0),
        color,
        ..Default::default()
    }
}
