//! Activity tab — a chronological, per-action timing breakdown of every Save /
//! Build / Flash / Clippy, so the user can SEE what runs and where the time
//! goes. Populated by [`crate::activity`]; newest action first, each showing its
//! phases with durations, the exact command line, and exit code.

use crate::activity::{fmt_clock, fmt_dur, ActivityLog};
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
            // The bottom panel sits inside the EDITOR panel, and a side panel
            // STORES the rect its content measured (`PanelState`, panel.rs) as
            // next frame's starting width. Content wider than the panel
            // therefore ratchets it wider every frame up to `max_width` — the
            // editor kept growing and the MCU zone got squeezed beside it.
            //
            // Both halves are needed: pin the width, AND make text wrap rather
            // than extend, since the editor sets `wrap_mode = Extend` and that
            // would otherwise let a long command line measure past the pin.
            ui.set_max_width(ui.available_width());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            for (i, action) in log.actions.iter().enumerate() {
                // ── Group separator ──────────────────────────────────────
                // A rule between GROUPS, where a group is one user Save (all
                // its actions share a session id) or one standalone action.
                // Keying on the name was wrong: every action of a Save is
                // called "Save (…)", so one Ctrl+S drew three separate blocks.
                // `i > 0` FIRST: `i - 1` underflows on usize at the top of the
                // list. A standalone action (session `None`) always starts its
                // own group — two consecutive Builds are two groups, not one.
                let starts_group = i > 0
                    && (action.session.is_none()
                        || log.actions[i - 1].session != action.session);
                if starts_group {
                    ui.add_space(3.0);
                    // Idle gap between this action's end and the previous
                    // (newer) one's start — where the time actually went when
                    // nothing was logged.
                    let gap = log.actions[i - 1]
                        .started_at
                        .duration_since(action.ended_at)
                        .ok();
                    ui.horizontal(|ui| {
                        ui.add(egui::Separator::default().horizontal());
                        if let Some(g) = gap {
                            ui.label(
                                egui::RichText::new(format!("idle {}", fmt_dur(g)))
                                    .size(9.5)
                                    .color(egui::Color32::from_gray(110)),
                            );
                        }
                    });
                    ui.add_space(3.0);
                }

                // Header: "Save (project)  ·  total 41.0s  ·  12:03:44.120 → 12:03:44.180"
                let slow = action.total.as_secs_f64() >= 1.0;
                // `→` renders as a tofu box — the app's fonts only carry
                // phosphor's glyph range plus ASCII (see the glyph-font note).
                let head = format!(
                    "{}  ·  total {}  ·  {} {} {}",
                    action.kind,
                    fmt_dur(action.total),
                    fmt_clock(action.started_at),
                    ph::ARROW_RIGHT,
                    fmt_clock(action.ended_at),
                );
                egui::CollapsingHeader::new(
                    egui::RichText::new(head)
                        .size(11.5)
                        .strong()
                        .color(if action.aborted {
                            // A worker that died without finishing — this is
                            // what used to hang the status bar at "Saving…".
                            egui::Color32::from_rgb(235, 100, 90)
                        } else if slow {
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
