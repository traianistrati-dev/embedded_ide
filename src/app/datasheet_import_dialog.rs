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
use crate::panels::mcu_module::datasheet_import::{self as ds, Extraction, Source};
use crate::panels::mcu_module::mcu_form::McuForm;
use eframe::egui;
use egui_phosphor::regular as ph;

/// Background extraction job state, shared with the worker thread.
enum ImportJob {
    Running,
    Done(Result<Extraction, String>),
}

/// A PDF the user picked (kept in memory until Extract or Remove).
struct PdfPick {
    name: String,
    bytes: Vec<u8>,
}

/// Apply maximize / restore to a dialog window.
///
/// Maximized → the window is pinned to (almost) the whole screen via
/// `fixed_rect`. Restored → the caller's normal size + centre anchor. The only
/// window control is the custom maximize/restore button — no collapse triangle.
pub(super) fn window_frame(
    ctx: &egui::Context,
    title: impl Into<egui::WidgetText>,
    maximized: bool,
    // Force the window back to `default_w × default_h`, centred, for ONE frame.
    // Needed because egui PERSISTS a window's rect in area memory, so
    // `default_size` alone is ignored once a rect is stored — both after a
    // maximize→restore and, crucially, when REOPENING a dialog that was left
    // maximized (the "opens huge" bug). A one-frame `fixed_rect` overrides the
    // stored rect, then we release to resizable.
    force_default_size: bool,
    default_w: f32,
    default_h: f32,
    anchor_y: f32,
) -> egui::Window<'static> {
    // `collapsible(false)` — no collapse triangle (the user removed it); the
    // custom maximize/restore button is the only window control.
    let win = egui::Window::new(title).collapsible(false).resizable(true);
    if maximized {
        // fixed_rect also fixes position, so don't add an anchor here.
        win.fixed_rect(ctx.content_rect().shrink(12.0))
    } else if force_default_size {
        let screen = ctx.content_rect();
        let size = egui::vec2(default_w, default_h);
        let centre = screen.center() + egui::vec2(0.0, anchor_y);
        win.fixed_rect(egui::Rect::from_center_size(centre, size))
    } else {
        win.default_width(default_w)
            .default_height(default_h)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, anchor_y])
    }
}

/// A maximize/restore toggle button, right-aligned — put it on the window's
/// first row. Flips `maximized`.
///
/// The row height is ALLOCATED explicitly. A bare
/// `with_layout(right_to_left, …)` at the top of a window grabs the whole
/// remaining HEIGHT as its cross-axis, so the button ended up vertically
/// centred in a huge empty band with all content pushed to the bottom (the
/// reported bug). `allocate_ui_with_layout` with a one-row height pins it.
pub(super) fn maximize_button(ui: &mut egui::Ui, maximized: &mut bool) {
    let row_h = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), row_h),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            let (icon, tip) = if *maximized {
                (ph::ARROWS_IN, "Restore window")
            } else {
                (ph::ARROWS_OUT, "Maximize window")
            };
            if ui
                .add(egui::Button::new(egui::RichText::new(icon).size(13.0)).frame(false))
                .on_hover_text(tip)
                .clicked()
            {
                *maximized = !*maximized;
            }
        },
    );
}

/// The collapsible "Prompt" section shared by both AI import dialogs: a
/// READ-ONLY view of the base prompt (so the user sees the contract they must
/// not repeat) plus an editable, persisted supplementary field.
///
/// The base is read-only ON PURPOSE — it carries the JSON shape and the
/// extraction rules; letting the user delete those would silently degrade
/// quality. Additions can only help (or, at worst, confuse — the JSON schema
/// still enforces the shape).
pub(super) fn prompt_section(
    ui: &mut egui::Ui,
    id: &str,
    open: &mut bool,
    base: &str,
    extra: &mut String,
) {
    let caret = if *open { ph::CARET_DOWN } else { ph::CARET_RIGHT };
    if ui
        .add(
            egui::Button::new(
                egui::RichText::new(format!("{caret} Prompt"))
                    .size(11.0)
                    .color(egui::Color32::from_gray(170)),
            )
            .frame(false),
        )
        .on_hover_text("View the base prompt and add your own extra instructions")
        .clicked()
    {
        *open = !*open;
    }
    if !*open {
        return;
    }
    // Full window width: the fields sit in a vertical-only `Resize` (drag the
    // bottom edge to grow the HEIGHT), and their WIDTH is pinned to the row's
    // available width. `Resize` can't track width itself — `default_size` is
    // read only on first creation — so the non-resizable x axis follows the
    // inner content, and the content is sized to `w` explicitly.
    let w = ui.available_width();

    ui.label(
        egui::RichText::new("Additional instructions (saved, appended to the base prompt):")
            .size(10.5)
            .color(egui::Color32::from_gray(150)),
    );
    egui::Resize::default()
        .id_salt(format!("{id}_extra_resize"))
        .resizable([false, true]) // height only
        .default_size(egui::vec2(w, 44.0))
        .min_height(28.0)
        .show(ui, |ui| {
            ui.add_sized(
                egui::vec2(w, ui.available_height()),
                egui::TextEdit::multiline(extra)
                    .id_salt(format!("{id}_extra"))
                    .desired_width(w)
                    .hint_text("e.g. prefer the non-SMPS variant · the PLL table is on page 200"),
            );
        });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new("Base prompt (read-only — carries the required JSON shape and rules):")
            .size(10.0)
            .color(egui::Color32::from_gray(130)),
    );
    egui::Resize::default()
        .id_salt(format!("{id}_base_resize"))
        .resizable([false, true]) // height only
        .default_size(egui::vec2(w, 120.0))
        .min_height(48.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(format!("{id}_base"))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Read-only but selectable (the `&str` buffer can't be
                    // edited), so the base prompt stays copyable. Natural
                    // (content) height so the ScrollArea scrolls the full text;
                    // width pinned to the row.
                    let mut base_ref = base;
                    ui.add(
                        egui::TextEdit::multiline(&mut base_ref)
                            .id_salt(format!("{id}_base_view"))
                            .desired_width(w)
                            .font(egui::TextStyle::Monospace),
                    );
                });
        });
}

/// Cache entry labels (newest first), each with its size — for the list.
fn cache_entry_labels() -> Vec<String> {
    ds::cache_entries()
        .into_iter()
        .map(|e| format!("{}  ({})", e.label, human_size(e.bytes)))
        .collect()
}

/// Compact human-readable byte size for the cache row.
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Session-only state for the import sub-dialog.
pub(crate) struct DatasheetImport {
    /// Which AI backend to use. Remembered across sessions in the user config
    /// folder, since it also selects which stored key applies.
    provider: ds::Provider,
    api_key: String,
    show_key: bool,
    model: String,
    text: String,
    /// A chosen datasheet PDF — when set, it is extracted instead of the text.
    pdf: Option<PdfPick>,
    /// Copied from the form when opened — biases the model only.
    family_hint: String,
    /// The target package. AUTHORITATIVE (it picks which pin-number column the
    /// model reads), editable here and REQUIRED before extracting; written back
    /// to the form on apply.
    package: String,
    /// Ignore the on-disk cache and force a fresh (billed) API call.
    force_reextract: bool,
    /// Whether the last applied extraction came from the cache (free).
    last_from_cache: bool,
    /// `(entries, bytes)` on disk — refreshed on open, after an extraction and
    /// after clearing, so the row never re-scans the folder per frame.
    cache_stats: (usize, u64),
    /// Human labels of the cached extractions, newest first — shown in a
    /// collapsible list. Refreshed with `cache_stats`.
    cache_entries: Vec<String>,
    cache_list_open: bool,
    cache_note: Option<String>,
    /// Persisted supplementary prompt appended to the base extraction prompt.
    extra_prompt: String,
    /// Whether the collapsible "Prompt" section is expanded.
    prompt_open: bool,
    /// Window maximized (fills the screen) vs normal size.
    maximized: bool,
    /// Previous frame's `maximized`, to detect the restore transition.
    prev_maximized: bool,
    /// `false` until the window has rendered once — the first frame forces the
    /// default size, so a dialog left maximized last time reopens normal.
    shown_once: bool,
    job: Option<Arc<Mutex<ImportJob>>>,
    report: Option<ds::ApplyReport>,
    error: Option<String>,
    key_note: Option<String>,
}

impl DatasheetImport {
    fn new(form: &McuForm) -> Self {
        let provider = ds::load_last_provider();
        Self {
            provider,
            api_key: ds::load_api_key(provider),
            show_key: false,
            model: provider.default_model().to_string(),
            text: String::new(),
            pdf: None,
            family_hint: form.family.clone(),
            package: form.package.clone(),
            force_reextract: false,
            last_from_cache: false,
            cache_stats: ds::cache_stats(),
            cache_entries: cache_entry_labels(),
            cache_list_open: false,
            cache_note: None,
            extra_prompt: ds::load_extra_prompt(ds::PromptSlot::Pins),
            prompt_open: false,
            maximized: false,
            prev_maximized: false,
            shown_once: false,
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
                    Some(Ok(ex)) => {
                        // The user's package is authoritative (it drove which
                        // column was read) — write it to the form before
                        // applying, so the pin-count cross-check uses it.
                        form.package = di.package.trim().to_string();
                        di.last_from_cache = ex.from_cache;
                        di.report = Some(ds::apply_to_form(&ex.chip, form));
                        di.error = None;
                        // A fresh extraction just wrote a new cache entry.
                        di.cache_stats = ds::cache_stats();
                        di.cache_entries = cache_entry_labels();
                        di.cache_note = None;
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
        let mut do_clear_cache = false;

        let force_default = !di.shown_once || (di.prev_maximized && !di.maximized);
        di.prev_maximized = di.maximized;
        di.shown_once = true;
        window_frame(
            ui.ctx(),
            format!("{} Import from datasheet (AI)", ph::SPARKLE),
            di.maximized,
            force_default,
            560.0,
            540.0,
            30.0,
        )
            .show(ui.ctx(), |ui| {
                maximize_button(ui, &mut di.maximized);
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

                // ── Provider ─────────────────────────────────────────────────
                // Switching providers swaps in that provider's own key and
                // default model — carrying either across would just produce an
                // auth failure with a confusing message.
                ui.horizontal(|ui| {
                    ui.label("AI provider:");
                    let before = di.provider;
                    egui::ComboBox::from_id_salt("ds_provider")
                        .selected_text(di.provider.label())
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            for p in ds::Provider::ALL {
                                ui.selectable_value(&mut di.provider, p, p.label());
                            }
                        });
                    if di.provider != before {
                        di.api_key = ds::load_api_key(di.provider);
                        di.model = di.provider.default_model().to_string();
                        di.key_note = None;
                        ds::save_last_provider(di.provider);
                    }
                    ui.label(
                        egui::RichText::new("all three read the PDF natively")
                            .size(10.0)
                            .color(egui::Color32::from_gray(140))
                            .italics(),
                    )
                    .on_hover_text(
                        "Only providers that accept a PDF directly are offered. A pin \
                         table is a 2D layout — backends that can only be fed text \
                         extracted locally scramble the columns and return confident, \
                         wrong pinouts.",
                    );
                });

                // ── API key ──────────────────────────────────────────────────
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(format!("{} API key:", di.provider.label()));
                    ui.add(
                        egui::TextEdit::singleline(&mut di.api_key)
                            .password(!di.show_key)
                            .desired_width(260.0),
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
                    egui::RichText::new(format!(
                        "Stored per provider in your user config folder; the {} \
                         environment variable overrides it. Never written to the project.",
                        di.provider.env_var()
                    ))
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
                        egui::RichText::new(di.provider.console_url()).size(10.0),
                        di.provider.console_url(),
                    )
                    .on_hover_text(
                        "Opens the provider's console. Sign in, create a key, copy it \
                         (usually shown once) and paste it above.",
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
                    .on_hover_text(di.provider.model_hint());
                    if !di.family_hint.trim().is_empty() {
                        ui.label(
                            egui::RichText::new(format!("family: {}", di.family_hint.trim()))
                                .size(10.0)
                                .color(egui::Color32::from_gray(140)),
                        );
                    }
                });

                // ── Package (REQUIRED — picks the pin-number column) ─────────
                ui.horizontal(|ui| {
                    ui.label("Package *:");
                    ui.add(
                        egui::TextEdit::singleline(&mut di.package)
                            .desired_width(140.0)
                            .hint_text("UFQFPN48"),
                    )
                    .on_hover_text(
                        "Must match a package name in the datasheet EXACTLY.\n\
                         The datasheet lists one column / pinout figure per package \
                         (UFQFPN32, WLCSP41, UFQFPN48, UFQFPN48 SMPS, UFBGA59…) — \
                         this tells the model which one to read.\n\n\
                         Variants matter: \"UFQFPN48\" and \"UFQFPN48 SMPS\" are \
                         DIFFERENT pinouts (both figures just say \"UFQFPN48\" inside \
                         the chip outline). Type the full variant name, e.g. copy it \
                         from the figure title.",
                    );
                    if di.package.trim().is_empty() {
                        ui.label(
                            egui::RichText::new("required — without it the wrong column gets read")
                                .size(10.0)
                                .color(egui::Color32::from_rgb(230, 170, 70)),
                        );
                    }
                });

                // ── Prompt (base read-only + supplementary) ──────────────────
                let base = ds::build_prompt(di.family_hint.trim(), di.package.trim());
                prompt_section(
                    ui,
                    "ds_pin_prompt",
                    &mut di.prompt_open,
                    &base,
                    &mut di.extra_prompt,
                );

                // ── Cache stats + clear ──────────────────────────────────────
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (n, bytes) = di.cache_stats;
                    ui.label(
                        egui::RichText::new(format!(
                            "Cache: {n} extraction(s) · {}",
                            human_size(bytes)
                        ))
                        .size(10.0)
                        .color(egui::Color32::from_gray(140)),
                    );
                    if ui
                        .add_enabled(
                            n > 0,
                            egui::Button::new(egui::RichText::new("Clear").size(10.0)).small(),
                        )
                        .on_hover_text(
                            "Delete every cached extraction. Re-importing the same datasheet \
                             will call (and bill) the API again.",
                        )
                        .on_disabled_hover_text("nothing cached yet")
                        .clicked()
                    {
                        do_clear_cache = true;
                    }
                    if let Some(note) = &di.cache_note {
                        ui.label(
                            egui::RichText::new(note)
                                .size(10.0)
                                .color(egui::Color32::from_gray(150)),
                        );
                    }
                });
                // ── Cached-file names (collapsible) ──────────────────────────
                // The reply files are hash-named, so this lists the human label
                // (chip · package · provider/model · source) written beside
                // each one, newest first.
                if !di.cache_entries.is_empty() {
                    let caret = if di.cache_list_open { ph::CARET_DOWN } else { ph::CARET_RIGHT };
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "{caret} cached datasheets ({})",
                                    di.cache_entries.len()
                                ))
                                .size(10.0)
                                .color(egui::Color32::from_gray(150)),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        di.cache_list_open = !di.cache_list_open;
                    }
                    if di.cache_list_open {
                        egui::ScrollArea::vertical()
                            .id_salt("ds_cache_list")
                            .max_height(120.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for label in &di.cache_entries {
                                    ui.label(
                                        egui::RichText::new(format!("• {label}"))
                                            .size(10.0)
                                            .color(egui::Color32::from_gray(165)),
                                    );
                                }
                            });
                    }
                }

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
                    let can_extract = !running
                        && has_input
                        && !di.api_key.trim().is_empty()
                        && !di.package.trim().is_empty();
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
                        .on_disabled_hover_text(
                            "needs an API key, a Package, and either pasted text or a PDF",
                        )
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
                    ui.checkbox(&mut di.force_reextract, "re-extract")
                        .on_hover_text(
                            "Off: an identical document + package + model reuses the cached \
                             result — instant and free.\nOn: force a fresh (billed) API call, \
                             e.g. after editing the pasted text.",
                        );
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
                            "{} Applied{} — {} pin(s). Review the form and fix \
                             anything flagged before Save.",
                            ph::CHECK_CIRCLE,
                            if di.last_from_cache {
                                " from cache (no API call)"
                            } else {
                                ""
                            },
                            rep.pins_added
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
                                        egui::RichText::new(format!("{} {n}", ph::DOT))
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
            di.key_note = Some(match ds::save_api_key(di.provider, &di.api_key) {
                Ok(()) => format!("{} Saved to the config folder.", ph::CHECK_CIRCLE),
                Err(e) => format!("{} Could not save key: {e}", ph::X_CIRCLE),
            });
        }
        if dismiss_report {
            di.report = None;
        }
        if do_clear_cache {
            di.cache_note = Some(match ds::clear_cache() {
                Ok(n) => format!("{} cleared {n}", ph::CHECK_CIRCLE),
                Err(e) => format!("{} {e}", ph::X_CIRCLE),
            });
            di.cache_stats = ds::cache_stats();
            di.cache_entries = cache_entry_labels();
        }
        if do_extract && di.job.is_none() {
            let shared = Arc::new(Mutex::new(ImportJob::Running));
            di.job = Some(shared.clone());
            di.error = None;
            di.report = None;
            // Persist the supplementary prompt so it survives across sessions.
            ds::save_extra_prompt(ds::PromptSlot::Pins, &di.extra_prompt);
            let (provider, key, model, family, package, extra) = (
                di.provider,
                di.api_key.clone(),
                di.model.clone(),
                di.family_hint.clone(),
                di.package.trim().to_string(),
                di.extra_prompt.clone(),
            );
            let source = match &di.pdf {
                Some(pdf) => Source::Pdf(pdf.bytes.clone()),
                None => Source::Text(di.text.clone()),
            };
            let use_cache = !di.force_reextract;
            std::thread::spawn(move || {
                let res = ds::call_ai(
                    provider, &key, &model, &family, &package, &extra, &source, use_cache,
                );
                *shared.lock().unwrap() = ImportJob::Done(res);
            });
            ui.ctx().request_repaint();
        }

        if keep_open {
            self.datasheet_import = Some(di);
        }
    }
}
