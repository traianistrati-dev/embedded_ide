//! Clippy tab — runs `cargo clippy` on demand and lists its improvement
//! suggestions. Each row is clickable (→ navigate to the code) and, when clippy
//! offers a machine-applicable fix, carries a "Fix" button; an "Apply all"
//! button applies every fix at once. Serialized with Build (the "Run clippy"
//! button is disabled while a Cargo Check build runs, and vice-versa).

use crate::build::BuildState;
use eframe::egui;
use egui_phosphor::regular as ph;
use std::sync::{Arc, Mutex};

/// True when a fix span `[start, end)` in `file` lands inside main.rs's GENERATED
/// block (`gen_ranges`) — that code is owned by the MCU Configurator, so the fix
/// can't be applied (it would be reverted) and its button is disabled.
fn fix_locked(file: &str, start: usize, end: usize, gen_ranges: &[(usize, usize)]) -> bool {
    file == "src/main.rs" && gen_ranges.iter().any(|&(b, e)| start < e && end > b)
}

/// Out-params: `run_clicked` = press "Run clippy"; `apply_one` = apply the
/// suggestion on diagnostic index N; `apply_all` = apply every suggestion. The
/// caller (`diag_embed`) performs the run / edits. `build_busy` greys the run
/// button while a Cargo Check build is going.
#[allow(clippy::too_many_arguments)]
pub fn show_clippy_tab(
    ui: &mut egui::Ui,
    clippy_state: &Arc<Mutex<BuildState>>,
    build_busy: bool,
    selected: &mut Option<usize>,
    nav: &mut Option<(String, usize, egui::Color32)>,
    run_clicked: &mut bool,
    apply_one: &mut Option<usize>,
    apply_all: &mut bool,
    // Set to `Some(i)` to project-wide-rename diagnostic `i`'s symbol (RA rename).
    apply_rename: &mut Option<usize>,
    // Byte ranges of main.rs's GENERATED block. A fix whose span lands inside one
    // is "locked" — its "Fix" button is shown but disabled (the MCU Configurator
    // owns that code and would overwrite a hand-applied fix).
    gen_ranges: &[(usize, usize)],
) {
    let state = clippy_state.lock().unwrap().clone();
    let clippy_busy = state.is_building();
    // "Apply all" runs every actionable suggestion — splice "Fix"es AND project-
    // wide "Rename"s. It only counts ones that are actually applicable (a fix/
    // rename locked inside main.rs's GENERATED block is skipped), so the button
    // never appears when it would do nothing.
    let any_action = matches!(&state, BuildState::Done(r) if r.diagnostics.iter().any(|d| {
        d.fixes.iter().any(|e| !fix_locked(&e.file, e.start, e.end, gen_ranges))
            || d.rename.as_ref().is_some_and(|rn| !fix_locked(&rn.file, rn.byte, rn.byte + 1, gen_ranges))
    }));

    // ── Control + status row ────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let enabled = !clippy_busy && !build_busy;
        let run = ui
            .add_enabled(
                enabled,
                egui::Button::new(
                    egui::RichText::new(format!("{} Run clippy", ph::SPARKLE))
                        .size(11.0)
                        .color(if enabled {
                            egui::Color32::from_rgb(150, 200, 120)
                        } else {
                            egui::Color32::GRAY
                        }),
                ),
            )
            .on_hover_text(
                "Run `cargo clippy` for improvement suggestions.\n\
                 Disabled while a Build (cargo check) is running.",
            );
        if run.clicked() {
            *run_clicked = true;
        }

        // Apply-all — every actionable suggestion: splice fixes + project-wide
        // renames (run one-by-one). Shown only when something can be applied.
        if any_action
            && ui
                .add(egui::Button::new(
                    egui::RichText::new(format!("{} Apply all", ph::MAGIC_WAND))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(120, 190, 230)),
                ))
                .on_hover_text(
                    "Apply every fix and rename clippy suggests (renames run one-by-one).",
                )
                .clicked()
        {
            *apply_all = true;
        }

        let (icon, text, color) = match &state {
            BuildState::Idle => (
                ph::LIGHTBULB,
                "Run clippy for code-improvement suggestions".to_owned(),
                egui::Color32::GRAY,
            ),
            BuildState::Building => (
                ph::HAMMER,
                "Running clippy…".to_owned(),
                egui::Color32::from_rgb(180, 180, 180),
            ),
            BuildState::Failed(msg) => {
                let first = msg.lines().next().unwrap_or(msg);
                let first = crate::failure_hint::strip(first);
                (
                    ph::X_CIRCLE,
                    format!("clippy failed: {first}"),
                    egui::Color32::from_rgb(230, 90, 80),
                )
            }
            BuildState::Done(r) if !r.diagnostics.is_empty() => (
                ph::WARNING,
                format!(
                    "{} suggestion{}",
                    r.diagnostics.len(),
                    if r.diagnostics.len() == 1 { "" } else { "s" }
                ),
                egui::Color32::from_rgb(230, 190, 50),
            ),
            BuildState::Done(_) => (
                ph::CHECK_CIRCLE,
                "No suggestions — clean".to_owned(),
                egui::Color32::from_rgb(80, 200, 100),
            ),
        };
        ui.add_space(6.0);
        ui.label(egui::RichText::new(icon).size(13.0).color(color));
        ui.label(egui::RichText::new(text).size(12.0).color(color));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(
                    egui::RichText::new(format!("{} Clear", ph::X))
                        .size(10.0)
                        .color(egui::Color32::GRAY),
                ))
                .clicked()
            {
                *clippy_state.lock().unwrap() = BuildState::Idle;
                *selected = None;
            }
        });
    });

    let result = match &state {
        BuildState::Failed(msg) => {
            ui.separator();
            let display = crate::failure_hint::strip(msg);
            egui::ScrollArea::vertical()
                .id_salt("clippy_failed_scroll")
                .show(ui, |ui| {
                    // Known cause (missing clippy / MSVC toolchain …) → shared
                    // card; otherwise the raw message.
                    if crate::failure_hint::show_card(ui, msg, |_| {}) {
                        return;
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(display)
                                .size(11.0)
                                .monospace()
                                .color(egui::Color32::from_rgb(230, 150, 90)),
                        )
                        .wrap(),
                    );
                });
            return;
        }
        BuildState::Done(r) if !r.diagnostics.is_empty() => r,
        _ => return,
    };

    ui.separator();

    // ── Suggestion list: [Fix] + colourised row (→ navigate) ────────────────────
    // Each row mirrors the rust-analyzer tab's colour scheme: severity icon,
    // file:line (blue), [lint] code (olive), message — painted segment by segment
    // so each keeps its own colour. A per-row "Fix" widget sits to the left.
    let sel = *selected;
    let list_height = if sel.is_some() {
        ui.available_height() * 0.45
    } else {
        ui.available_height()
    };
    egui::ScrollArea::vertical()
        .id_salt("clippy_list")
        .max_height(list_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Rows that have an action button (Fix or Rename) come first; pure-
            // advice rows (dead_code, too_many_arguments, …) sink to the bottom.
            // Sort an index list (stable → keeps file:line order within a group) so
            // selection / apply still use the real diagnostic index.
            let mut order: Vec<usize> = (0..result.diagnostics.len()).collect();
            order.sort_by_key(|&i| {
                let d = &result.diagnostics[i];
                !(d.has_fix() || d.rename.is_some()) // has-button (false) sorts first
            });
            for i in order {
                let diag = &result.diagnostics[i];
                let is_sel = sel == Some(i);
                ui.horizontal(|ui| {
                    // Fixed-width action column so the colourised text below lines
                    // up across every row. A fix is "locked" when it touches
                    // main.rs's GENERATED block: that code is owned by the MCU
                    // Configurator, so the button is shown but disabled (applying it
                    // would be reverted / unsafe).
                    let locked =
                        diag.fixes
                            .iter()
                            .any(|e| fix_locked(&e.file, e.start, e.end, gen_ranges))
                            || diag.rename.as_ref().is_some_and(|r| {
                                fix_locked(&r.file, r.byte, r.byte + 1, gen_ranges)
                            });
                    ui.allocate_ui_with_layout(
                        egui::vec2(64.0, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            // Machine-applicable splice → "Fix" (wand); otherwise a
                            // naming-convention rename → "Rename" (project-wide, RA).
                            if diag.has_fix() {
                                let btn = egui::Button::new(
                                    egui::RichText::new(format!("{} Fix", ph::MAGIC_WAND))
                                        .size(10.0)
                                        .color(if locked {
                                            egui::Color32::from_gray(110)
                                        } else {
                                            egui::Color32::from_rgb(130, 200, 140)
                                        }),
                                );
                                let resp = ui.add_enabled(!locked, btn).on_hover_text(if locked {
                                    "Autogenerated code — edit it via the MCU Configurator, \
                                     not here"
                                } else {
                                    "Apply this suggestion"
                                });
                                if resp.clicked() {
                                    *apply_one = Some(i);
                                }
                            } else if let Some(rn) = &diag.rename {
                                let btn = egui::Button::new(
                                    egui::RichText::new(format!("{} Rename", ph::NOTE_PENCIL))
                                        .size(10.0)
                                        .color(if locked {
                                            egui::Color32::from_gray(110)
                                        } else {
                                            egui::Color32::from_rgb(150, 180, 230)
                                        }),
                                );
                                let tip = if locked {
                                    "Autogenerated code — edit it via the MCU Configurator, \
                                     not here"
                                        .to_owned()
                                } else {
                                    format!(
                                        "Rename to `{}` everywhere (project-wide, like Ctrl+R)",
                                        rn.new_name
                                    )
                                };
                                let resp = ui.add_enabled(!locked, btn).on_hover_text(tip);
                                if resp.clicked() {
                                    *apply_rename = Some(i);
                                }
                            }
                        },
                    );

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
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 18.0),
                        egui::Sense::click(),
                    );
                    let row_bg = crate::app::diag_row_bg(is_sel, resp.hovered());
                    if ui.is_rect_visible(rect) {
                        let painter = ui.painter();
                        painter.rect_filled(rect, 2.0, row_bg);
                        let cy = rect.center().y;
                        let mut x = rect.left() + 4.0;

                        let r = painter.text(
                            egui::pos2(x, cy),
                            egui::Align2::LEFT_CENTER,
                            level_icon,
                            egui::FontId::proportional(11.0),
                            level_color,
                        );
                        x = r.right() + 4.0;

                        if !location.is_empty() {
                            const LOC_COLOR: egui::Color32 = egui::Color32::from_rgb(120, 160, 200);
                            let r = painter.text(
                                egui::pos2(x, cy),
                                egui::Align2::LEFT_CENTER,
                                &location,
                                egui::FontId::monospace(10.5),
                                LOC_COLOR,
                            );
                            crate::app::diag_row_link_hint(painter, &resp, r, LOC_COLOR);
                            x = r.right() + 6.0;
                        }

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
                        let now = !is_sel;
                        *selected = if now { Some(i) } else { None };
                        if now {
                            if let (Some(file), Some(line)) = (&diag.file, diag.line) {
                                let sev = if diag.is_error() {
                                    crate::lsp::DiagSeverity::Error
                                } else {
                                    crate::lsp::DiagSeverity::Warning
                                };
                                *nav = Some((
                                    file.clone(),
                                    line as usize,
                                    crate::app::diag_highlight_color(sev),
                                ));
                            }
                        }
                    }
                });
            }
        });

    // ── Detail of the selected suggestion ───────────────────────────────────────
    if let Some(idx) = sel {
        if let Some(diag) = result.diagnostics.get(idx) {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("clippy_detail")
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
