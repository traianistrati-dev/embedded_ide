//! Cargo build diagnostics tab.
use crate::build::{self, BuildState};
use crate::size::{MemUsage, SizeState};
use eframe::egui;
use egui_phosphor::regular as ph;
use std::sync::{Arc, Mutex};

pub fn show_cargo_tab(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    build_state: &Arc<Mutex<BuildState>>,
    selected_diagnostic: &mut Option<usize>,
    // Set to `(rel_path, 1-based line, highlight-band colour)` when a row is
    // expanded, so the editor opens that file, scrolls to the line, and tints it.
    nav: &mut Option<(String, usize, egui::Color32)>,
    // Build button (moved here from the top toolbar on 2026-07-10): set on
    // click, handled by AppIde after the panel (`start_build`). `can_build` =
    // a buildable chip config exists; `clippy_running` serializes with Clippy
    // (they share the same target/ directory).
    build_go: &mut bool,
    can_build: bool,
    clippy_running: bool,
    // Flash/RAM usage measurement (the Size button; caller runs
    // `start_size_measure` — same signal pattern as `build_go`).
    size_state: &Arc<Mutex<SizeState>>,
    size_go: &mut bool,
) {
    let state = build_state.lock().unwrap().clone();
    let size_snapshot = size_state.lock().unwrap().clone();
    let workspace = std::env::temp_dir().join("embedded_ide_0_check");

    // ── Status bar ────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        // Idle shows no status text but MUST still render the buttons row —
        // the first build of a session starts from here now.
        let status = match &state {
            BuildState::Idle => None,
            BuildState::Building => Some((
                ph::HAMMER,
                "Building…".to_owned(),
                egui::Color32::from_rgb(180, 180, 180),
            )),
            BuildState::Failed(msg) => {
                let first = msg.lines().next().unwrap_or(msg);
                // Suppress the [DISK_FULL] prefix from the one-liner badge
                let first = crate::failure_hint::strip(first);
                Some((
                    ph::X_CIRCLE,
                    format!("Build failed: {}", first),
                    egui::Color32::from_rgb(230, 90, 80),
                ))
            }
            BuildState::Done(r) if r.error_count() > 0 => Some((
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
            )),
            BuildState::Done(r) if r.warning_count() > 0 => Some((
                ph::WARNING,
                format!(
                    "{} warning{}",
                    r.warning_count(),
                    if r.warning_count() == 1 { "" } else { "s" }
                ),
                egui::Color32::from_rgb(230, 190, 50),
            )),
            BuildState::Done(_) => Some((
                ph::CHECK_CIRCLE,
                "Build succeeded — no errors".to_owned(),
                egui::Color32::from_rgb(80, 200, 100),
            )),
        };
        if let Some((icon, text, color)) = &status {
            ui.label(egui::RichText::new(*icon).size(13.0).color(*color));
            ui.label(egui::RichText::new(text).size(12.0).color(*color).strong());
        }

        // Right-side buttons (RTL: first added = rightmost): Clear | Clean
        // target/ | Build — so Build sits to their LEFT, per the user's ask.
        let is_building = matches!(state, BuildState::Building);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if status.is_some() {
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
                // recover disk space.  Especially helpful after a disk-full
                // build failure.
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

                ui.add_space(4.0);
            }

            // ── Build button (moved from the top toolbar) ─────────────────
            let build_label = if is_building {
                let dots = match (ctx.cumulative_frame_nr() / 15) % 3 {
                    0 => ".",
                    1 => "..",
                    _ => "...",
                };
                format!("Building{dots}")
            } else {
                format!("{} Build", ph::HAMMER)
            };
            let build_enabled = !is_building && !clippy_running && can_build;
            let build_btn = ui.add_enabled(
                build_enabled,
                egui::Button::new(egui::RichText::new(&build_label).size(10.0).color(
                    if build_enabled {
                        egui::Color32::from_rgb(100, 220, 100)
                    } else {
                        egui::Color32::GRAY
                    },
                )),
            );
            if build_btn.clicked() {
                *build_go = true;
            }
            build_btn.on_hover_text(
                "Run `cargo check` on the generated project.\n\
                 Requires the Rust toolchain + thumbv7m-none-eabi target:\n\
                 rustup target add thumbv7m-none-eabi",
            );

            ui.add_space(4.0);

            // ── Size button — measure Flash/RAM usage ─────────────────────
            let size_busy = size_snapshot.is_busy();
            let size_label = if size_busy {
                "Measuring…".to_owned()
            } else {
                format!("{} Size", ph::RULER)
            };
            let size_enabled = !size_busy && !is_building && !clippy_running && can_build;
            let size_btn = ui.add_enabled(
                size_enabled,
                egui::Button::new(egui::RichText::new(&size_label).size(10.0).color(
                    if size_enabled {
                        egui::Color32::from_rgb(120, 170, 210)
                    } else {
                        egui::Color32::GRAY
                    },
                )),
            );
            if size_btn.clicked() {
                *size_go = true;
            }
            size_btn.on_hover_text(
                "Measure Flash/RAM usage: `cargo build --release`, then read \
                 the ELF section sizes against the memory.x limits.\n\
                 RAM = static usage (.data + .bss) — stack and heap come on top.",
            );

            // Keep UI refreshing while build runs (drives the dot animation).
            if is_building || size_busy {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
            }
        });
    });

    render_size_row(ui, &size_snapshot);

    let BuildState::Done(result) = &state else {
        // For Building/Failed we've shown what we can
        if let BuildState::Failed(msg) = &state {
            ui.separator();

            // ── Known-cause card (disk full, missing MSVC toolchain, …) ───────
            // One shared renderer for every `[TAG]`ged failure — see
            // `crate::failure_hint`; the disk-full case adds its own recovery
            // button, the tool-related ones get "Open Tools" for free.
            let is_disk_full = msg.starts_with("[DISK_FULL]");
            let shown = crate::failure_hint::show_card(ui, msg, |ui| {
                if is_disk_full
                    && ui
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
                    build::start_clean(workspace.clone(), Arc::clone(build_state), ctx.clone());
                    *selected_diagnostic = None;
                }
            });
            if shown {
                ui.add_space(4.0);
            }

            // Full error text in a scroll area
            egui::ScrollArea::vertical()
                .id_salt("build_failed_scroll")
                .show(ui, |ui| {
                    // Marker already explained by the card above.
                    let display_msg = crate::failure_hint::strip(msg.as_str());
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

    render_diagnostics(ui, result, selected_diagnostic, nav);
}

/// The Flash/RAM usage strip under the status bar: two labelled bars (with
/// percentages when memory.x limits are known) + a section breakdown on hover.
/// Hidden while nothing was measured yet.
///
/// `pub(super)` because the Flash tab renders the same row (its measurement
/// runs automatically after every flash) — one renderer, one look.
pub(super) fn render_size_row(ui: &mut egui::Ui, state: &SizeState) {
    match state {
        SizeState::Idle => {}
        SizeState::Building => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} Measuring Flash/RAM… (cargo build --release)",
                        ph::RULER
                    ))
                    .size(10.5)
                    .color(egui::Color32::from_rgb(180, 180, 180)),
                );
            });
        }
        SizeState::Failed(msg) => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} Size: {}",
                        ph::X_CIRCLE,
                        msg.lines().next().unwrap_or("failed")
                    ))
                    .size(10.5)
                    .color(egui::Color32::from_rgb(230, 90, 80)),
                )
                .on_hover_text(msg);
            });
        }
        SizeState::Done(u) => {
            ui.horizontal(|ui| {
                usage_bar(
                    ui,
                    "Flash",
                    u.flash_used,
                    u.limits.flash.map(|r| r.length),
                    u,
                    true,
                );
                ui.add_space(12.0);
                usage_bar(
                    ui,
                    "RAM",
                    u.ram_used,
                    u.limits.ram.map(|r| r.length),
                    u,
                    false,
                );
                if u.limits.ram.is_none() {
                    ui.label(
                        egui::RichText::new("(no memory.x — sizes only)")
                            .size(9.5)
                            .color(egui::Color32::from_gray(110)),
                    )
                    .on_hover_text(
                        "This toolchain has no project memory.x (esp-hal owns the \
                         memory layout), so total capacity is unknown — the raw \
                         static sizes are still exact.",
                    );
                }
            });
        }
    }
}

/// ONE side of [`render_size_row`] — for a layout that stacks Flash over RAM
/// instead of placing them side by side (the Flash tab puts each next to its own
/// programmer row). Same states as the full row, so a measurement in progress or
/// a failure still says so rather than showing a stale bar.
pub(super) fn render_size_bar(ui: &mut egui::Ui, state: &SizeState, flash: bool) {
    let label = if flash { "Flash" } else { "RAM" };
    let muted = |ui: &mut egui::Ui, text: String, color: egui::Color32| {
        ui.label(
            egui::RichText::new(format!("{label}  {text}"))
                .size(10.5)
                .color(color),
        )
    };
    match state {
        SizeState::Idle => {
            muted(ui, "—".to_owned(), egui::Color32::from_gray(110))
                .on_hover_text("Not measured yet — press Size (it also runs after every flash).");
        }
        SizeState::Building => {
            muted(ui, "measuring…".to_owned(), egui::Color32::from_gray(170));
        }
        SizeState::Failed(msg) => {
            // The reason belongs on ONE line, not repeated under both bars.
            if flash {
                ui.label(
                    egui::RichText::new(format!(
                        "{} Size: {}",
                        ph::X_CIRCLE,
                        msg.lines().next().unwrap_or("failed")
                    ))
                    .size(10.5)
                    .color(egui::Color32::from_rgb(230, 90, 80)),
                )
                .on_hover_text(msg);
            } else {
                muted(ui, "—".to_owned(), egui::Color32::from_gray(110));
            }
        }
        SizeState::Done(u) => {
            let (used, limit) = if flash {
                (u.flash_used, u.limits.flash.map(|r| r.length))
            } else {
                (u.ram_used, u.limits.ram.map(|r| r.length))
            };
            usage_bar(ui, label, used, limit, u, flash);
        }
    }
}

/// One labelled usage bar: `Flash ▓▓░░ 34.2 KB / 64 KB · 53%`. Green under
/// 70%, amber under 90%, red above. Hover lists the matching ELF sections.
fn usage_bar(
    ui: &mut egui::Ui,
    label: &str,
    used: u64,
    limit: Option<u64>,
    usage: &MemUsage,
    flash: bool,
) {
    ui.label(
        egui::RichText::new(label)
            .size(10.5)
            .color(egui::Color32::from_gray(160)),
    );
    let pct = limit.map(|l| used as f32 / l.max(1) as f32);
    let color = match pct {
        Some(p) if p >= 0.9 => egui::Color32::from_rgb(230, 80, 60),
        Some(p) if p >= 0.7 => egui::Color32::from_rgb(230, 180, 60),
        _ => egui::Color32::from_rgb(80, 200, 100),
    };

    // The bar itself (only when a limit is known — no denominator, no bar).
    if let Some(p) = pct {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(120.0, 11.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 2.0, egui::Color32::from_gray(48));
        let mut fill = rect;
        fill.set_width(rect.width() * p.min(1.0));
        painter.rect_filled(fill, 2.0, color);
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
            egui::StrokeKind::Inside,
        );
        resp.on_hover_text(section_breakdown(usage, flash));
    }

    let text = match limit {
        Some(l) => format!(
            "{} / {} · {:.0}%",
            fmt_bytes(used),
            fmt_bytes(l),
            (used as f64 / l.max(1) as f64) * 100.0
        ),
        None => fmt_bytes(used),
    };
    ui.label(egui::RichText::new(text).size(10.5).monospace().color(
        if pct.is_some_and(|p| p >= 0.9) {
            color
        } else {
            egui::Color32::from_rgb(200, 205, 215)
        },
    ))
    .on_hover_text(section_breakdown(usage, flash));
}

/// Hover text: the ELF sections counted on this side (flash or RAM), largest
/// first, plus the static-RAM caveat.
fn section_breakdown(usage: &MemUsage, flash: bool) -> String {
    let mut rows: Vec<&crate::size::SectionUse> = usage
        .sections
        .iter()
        .filter(|s| if flash { s.in_flash } else { s.in_ram })
        .collect();
    rows.sort_by(|a, b| b.size.cmp(&a.size));
    let mut out = String::new();
    for s in rows {
        out.push_str(&format!("{:<16} {}\n", s.name, fmt_bytes(s.size)));
    }
    if out.is_empty() {
        out.push_str("(no section details)\n");
    }
    if flash {
        out.push_str("\nProgrammed image: vectors + code + constants + .data initializers.");
    } else {
        out.push_str("\nStatic RAM only (.data + .bss) — stack and heap come on top.");
    }
    out
}

/// `812 B`, `34.2 KB`, `1.25 MB`.
fn fmt_bytes(b: u64) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{:.2} MB", b as f64 / (1024.0 * 1024.0))
    }
}

/// Render a `cargo check`/`clippy` diagnostics list (clickable rows → navigate)
/// plus the rendered detail of the selected one. Shared by the Cargo Check and
/// Clippy tabs.
pub(crate) fn render_diagnostics(
    ui: &mut egui::Ui,
    result: &build::BuildResult,
    selected_diagnostic: &mut Option<usize>,
    nav: &mut Option<(String, usize, egui::Color32)>,
) {
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
                            let sev = if diag.is_error() {
                                crate::lsp::DiagSeverity::Error
                            } else {
                                crate::lsp::DiagSeverity::Warning
                            };
                            let color = crate::app::diag_highlight_color(sev);
                            *nav = Some((file.clone(), line as usize, color));
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
