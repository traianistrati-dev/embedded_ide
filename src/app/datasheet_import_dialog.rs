//! "Import from datasheet (AI)" sub-dialog of the New MCU form (Phase 1).
//!
//! The user pastes a pin / alternate-function table copied from a datasheet;
//! we hand it to Claude (on a background thread) and patch the result into the
//! open [`McuForm`] for review. Nothing is auto-saved — the extraction only
//! FILLS the form; the human still reviews it and clicks Save.
//!
//! The pure prompt/JSON/patch logic lives in
//! `panels::mcu_module::datasheet_import`; this file is just the egui window and
//! the background-job plumbing.

use std::sync::{Arc, Mutex};

use super::AppIde;
use crate::panels::mcu_module::datasheet_import::{self as ds, ExtractedChip, Source};
use crate::panels::mcu_module::mcu_form::McuForm;
use eframe::egui;
use egui_phosphor::regular as ph;

/// Background extraction job state, shared with the worker thread.
enum ImportJob {
    Running,
    Done(Result<ExtractedChip, String>),
}

/// A PDF the user picked (kept in memory until Extract or Remove).
struct PdfPick {
    name: String,
    bytes: Vec<u8>,
}

/// Session-only state for the import sub-dialog.
pub(crate) struct DatasheetImport {
    api_key: String,
    show_key: bool,
    model: String,
    text: String,
    /// A chosen datasheet PDF — when set, it is extracted instead of the text.
    pdf: Option<PdfPick>,
    /// Copied from the form when opened, so the worker has hints to bias with.
    family_hint: String,
    package_hint: String,
    job: Option<Arc<Mutex<ImportJob>>>,
    report: Option<ds::ApplyReport>,
    error: Option<String>,
    key_note: Option<String>,
}

impl DatasheetImport {
    fn new(form: &McuForm) -> Self {
        Self {
            api_key: ds::load_api_key(),
            show_key: false,
            model: ds::DEFAULT_MODEL.to_string(),
            text: String::new(),
            pdf: None,
            family_hint: form.family.clone(),
            package_hint: form.package.clone(),
            job: None,
            report: None,
            error: None,
            key_note: None,
        }
    }
}

impl AppIde {
    /// Open the import sub-dialog, seeding hints from the current form.
    pub(crate) fn open_datasheet_import(&mut self, form: &McuForm) {
        self.datasheet_import = Some(DatasheetImport::new(form));
    }

    /// Render the sub-dialog. Applies a finished extraction straight into
    /// `form`, so the caller must hold the form mutably. No-op while closed.
    pub(super) fn show_datasheet_import(&mut self, ui: &egui::Ui, form: &mut McuForm) {
        let Some(mut di) = self.datasheet_import.take() else {
            return;
        };
        let mut keep_open = true;

        // ── Poll the worker ──────────────────────────────────────────────────
        if let Some(job) = &di.job {
            let done = matches!(&*job.lock().unwrap(), ImportJob::Done(_));
            if done {
                let result = {
                    let mut guard = job.lock().unwrap();
                    match std::mem::replace(&mut *guard, ImportJob::Running) {
                        ImportJob::Done(r) => Some(r),
                        ImportJob::Running => None,
                    }
                };
                di.job = None;
                match result {
                    Some(Ok(chip)) => {
                        di.report = Some(ds::apply_to_form(&chip, form));
                        di.error = None;
                    }
                    Some(Err(e)) => {
                        di.error = Some(e);
                        di.report = None;
                    }
                    None => {}
                }
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        let running = di.job.is_some();

        let mut do_extract = false;
        let mut do_save_key = false;
        let mut dismiss_report = false;

        egui::Window::new(format!("{} Import from datasheet (AI)", ph::SPARKLE))
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .default_height(540.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 30.0])
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new(
                        "Paste a pin / alternate-function table from the datasheet. \
                         The AI fills the form for you to review — nothing is saved \
                         automatically.",
                    )
                    .size(11.0)
                    .color(egui::Color32::from_gray(160)),
                );
                ui.add_space(6.0);

                // ── API key ──────────────────────────────────────────────────
                ui.horizontal(|ui| {
                    ui.label("Anthropic API key:");
                    ui.add(
                        egui::TextEdit::singleline(&mut di.api_key)
                            .password(!di.show_key)
                            .desired_width(260.0)
                            .hint_text("sk-ant-…"),
                    );
                    ui.checkbox(&mut di.show_key, "show");
                    if ui
                        .button("Save key")
                        .on_hover_text("Store in the user config folder (never in the project)")
                        .clicked()
                    {
                        do_save_key = true;
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Stored in your user config folder; the ANTHROPIC_API_KEY \
                         environment variable overrides it. Never written to the project.",
                    )
                    .size(10.0)
                    .color(egui::Color32::from_gray(140))
                    .italics(),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    ui.label(
                        egui::RichText::new(format!("{} No key? Create one at", ph::KEY))
                            .size(10.0)
                            .color(egui::Color32::from_gray(140)),
                    );
                    ui.hyperlink_to(
                        egui::RichText::new("console.anthropic.com → Settings → API keys")
                            .size(10.0),
                        "https://console.anthropic.com/settings/keys",
                    )
                    .on_hover_text(
                        "Opens the Anthropic Console. Sign in, click \"Create Key\", \
                         copy the key (shown once) and paste it above.",
                    );
                    ui.label(
                        egui::RichText::new("(needs billing set up on the account).")
                            .size(10.0)
                            .color(egui::Color32::from_gray(140)),
                    );
                });
                if let Some(note) = &di.key_note {
                    ui.label(egui::RichText::new(note).size(10.5));
                }

                // ── Model ────────────────────────────────────────────────────
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Model:");
                    ui.add(
                        egui::TextEdit::singleline(&mut di.model).desired_width(200.0),
                    )
                    .on_hover_text("Anthropic model id — e.g. claude-opus-4-8");
                    if !di.family_hint.trim().is_empty() || !di.package_hint.trim().is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "hints: {} {}",
                                di.family_hint.trim(),
                                di.package_hint.trim()
                            ))
                            .size(10.0)
                            .color(egui::Color32::from_gray(140)),
                        );
                    }
                });

                // ── PDF picker ───────────────────────────────────────────────
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{} Choose PDF…", ph::FILE_PDF))
                        .on_hover_text("Send a datasheet PDF instead of pasted text")
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("PDF", &["pdf"])
                            .pick_file()
                        {
                            match std::fs::read(&path) {
                                Ok(bytes) => {
                                    let name = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| "datasheet.pdf".into());
                                    di.pdf = Some(PdfPick { name, bytes });
                                    di.error = None;
                                }
                                Err(e) => di.error = Some(format!("Could not read PDF: {e}")),
                            }
                        }
                    }
                    if let Some(pdf) = &di.pdf {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {} ({:.1} MB) — used instead of the text",
                                ph::CHECK_CIRCLE,
                                pdf.name,
                                pdf.bytes.len() as f64 / (1024.0 * 1024.0),
                            ))
                            .size(10.5)
                            .color(egui::Color32::from_rgb(120, 190, 120)),
                        );
                        if ui.button("Remove").clicked() {
                            di.pdf = None;
                        }
                    } else {
                        ui.label(
                            egui::RichText::new("or paste text below")
                                .size(10.0)
                                .color(egui::Color32::from_gray(140)),
                        );
                    }
                });

                // ── Paste area ───────────────────────────────────────────────
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Datasheet text:").size(11.0));
                let paste_enabled = di.pdf.is_none();
                egui::ScrollArea::vertical()
                    .id_salt("ds_paste")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        ui.add_enabled(
                            paste_enabled,
                            egui::TextEdit::multiline(&mut di.text)
                                .desired_rows(8)
                                .desired_width(f32::INFINITY)
                                .font(egui::TextStyle::Monospace)
                                .hint_text(
                                    "Paste the pin table / alternate-function list here…",
                                ),
                        );
                    });

                // ── Actions ──────────────────────────────────────────────────
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let has_input = di.pdf.is_some() || !di.text.trim().is_empty();
                    let can_extract = !running && has_input && !di.api_key.trim().is_empty();
                    if ui
                        .add_enabled(
                            can_extract,
                            egui::Button::new(
                                egui::RichText::new(format!("{} Extract", ph::MAGIC_WAND))
                                    .color(if can_extract {
                                        egui::Color32::from_rgb(150, 200, 255)
                                    } else {
                                        egui::Color32::GRAY
                                    }),
                            ),
                        )
                        .on_hover_text("Send the text to Claude and fill the form")
                        .clicked()
                    {
                        do_extract = true;
                    }
                    if running {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("extracting…")
                                .size(11.0)
                                .color(egui::Color32::from_gray(160)),
                        );
                    }
                    if ui.button("Close").clicked() {
                        keep_open = false;
                    }
                });

                if let Some(err) = &di.error {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("{} {err}", ph::X_CIRCLE))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(230, 110, 90)),
                    );
                }

                // ── Review report ────────────────────────────────────────────
                if let Some(rep) = &di.report {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!(
                            "{} Applied — {} pin(s) added. Review the form and fix \
                             anything flagged before Save.",
                            ph::CHECK_CIRCLE, rep.pins_added
                        ))
                        .size(11.5)
                        .strong()
                        .color(egui::Color32::from_rgb(120, 210, 120)),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("ds_report")
                        .max_height(150.0)
                        .show(ui, |ui| {
                            if !rep.patched.is_empty() {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Filled: {}",
                                        rep.patched.join(" · ")
                                    ))
                                    .size(10.5)
                                    .color(egui::Color32::from_gray(170)),
                                );
                            }
                            for w in &rep.warnings {
                                ui.label(
                                    egui::RichText::new(format!("{} {w}", ph::WARNING))
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(220, 170, 70)),
                                );
                            }
                            if !rep.raw_notes.is_empty() {
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new("Unmapped alternate functions (kept for you to place):")
                                        .size(10.5)
                                        .italics()
                                        .color(egui::Color32::from_gray(150)),
                                );
                                for n in &rep.raw_notes {
                                    ui.label(
                                        egui::RichText::new(format!("• {n}"))
                                            .size(10.0)
                                            .color(egui::Color32::from_gray(160)),
                                    );
                                }
                            }
                        });
                    if ui.button("Dismiss report").clicked() {
                        dismiss_report = true;
                    }
                }
            });

        // ── Deferred side effects ────────────────────────────────────────────
        if do_save_key {
            di.key_note = Some(match ds::save_api_key(&di.api_key) {
                Ok(()) => format!("{} Saved to the config folder.", ph::CHECK_CIRCLE),
                Err(e) => format!("{} Could not save key: {e}", ph::X_CIRCLE),
            });
        }
        if dismiss_report {
            di.report = None;
        }
        if do_extract && di.job.is_none() {
            let shared = Arc::new(Mutex::new(ImportJob::Running));
            di.job = Some(shared.clone());
            di.error = None;
            di.report = None;
            let (key, model, family, package) = (
                di.api_key.clone(),
                di.model.clone(),
                di.family_hint.clone(),
                di.package_hint.clone(),
            );
            let source = match &di.pdf {
                Some(pdf) => Source::Pdf(pdf.bytes.clone()),
                None => Source::Text(di.text.clone()),
            };
            std::thread::spawn(move || {
                let res = ds::call_claude(&key, &model, &family, &package, &source);
                *shared.lock().unwrap() = ImportJob::Done(res);
            });
            ui.ctx().request_repaint();
        }

        if keep_open {
            self.datasheet_import = Some(di);
        }
    }
}
