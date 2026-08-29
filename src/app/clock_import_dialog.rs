//! "Extract clock tree from datasheet (AI)" sub-dialog of the New MCU form
//! (clock roadmap Layer 3).
//!
//! Reuses the pin importer's provider machinery (`Provider`, key storage, PDF
//! handling, `call_ai_clock`) but produces a `GraphClock`: the pure conversion
//! + numeric self-check live in `clock::graph::extract`, so a reply that
//! doesn't compute the datasheet's SYSCLK is rejected before it reaches the
//! form. On success it attaches the graph via `McuForm::set_imported_clock`.
//!
//! **Two passes** (Phase 6): "Extract spine" is the original call; "Extract
//! branches" then asks for everything the spine contract cannot express (the
//! low-speed paths, MCO, the peripheral kernel selectors) and MERGES it onto the
//! tree already in the form — see `clock::graph::extract_tree`. Splitting it
//! keeps the working spine banked instead of putting it behind one
//! all-or-nothing reply, and each pass carries its own numeric self-check.

use std::sync::{Arc, Mutex};

use super::AppIde;
use crate::panels::mcu_module::clock::graph::GraphClock;
use crate::panels::mcu_module::clock::graph::extract_tree::MergeReport;
use crate::panels::mcu_module::datasheet_import::{self as ds, Source};
use crate::panels::mcu_module::mcu_form::McuForm;
use eframe::egui;
use egui_phosphor::regular as ph;

/// What either pass produced. The two calls answer with different shapes; the
/// dialog only cares about the resulting tree, whether it was cached, and — for
/// the second pass — what the merge did.
struct Outcome {
    clock: GraphClock,
    from_cache: bool,
    /// `Some` for the branch pass: nodes added / kept / wired / skipped.
    report: Option<MergeReport>,
}

enum ClockJob {
    Running,
    Done(Result<Outcome, String>),
}

/// Which extraction the worker is running.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// The clock spine, with its own contract and SYSCLK self-check.
    Spine,
    /// Everything else, merged onto the spine already in the form.
    Branches,
}

struct PdfPick {
    name: String,
    bytes: Vec<u8>,
}

/// Session-only state for the clock-extraction sub-dialog.
pub(crate) struct ClockImport {
    provider: ds::Provider,
    api_key: String,
    show_key: bool,
    model: String,
    text: String,
    pdf: Option<PdfPick>,
    force_reextract: bool,
    /// Persisted supplementary prompt appended to the base clock prompt.
    extra_prompt: String,
    prompt_open: bool,
    maximized: bool,
    prev_maximized: bool,
    shown_once: bool,
    job: Option<Arc<Mutex<ClockJob>>>,
    error: Option<String>,
    /// Green confirmation after a successful extraction (kept until the dialog
    /// closes) so the user sees it worked before the diagram updates.
    note: Option<String>,
    key_note: Option<String>,
}

impl ClockImport {
    fn new() -> Self {
        let provider = ds::load_last_provider();
        Self {
            provider,
            api_key: ds::load_api_key(provider),
            show_key: false,
            model: provider.default_model().to_string(),
            text: String::new(),
            pdf: None,
            force_reextract: false,
            extra_prompt: ds::load_extra_prompt(ds::PromptSlot::Clock),
            prompt_open: false,
            maximized: false,
            prev_maximized: false,
            shown_once: false,
            job: None,
            error: None,
            note: None,
            key_note: None,
        }
    }
}

impl AppIde {
    /// Open the clock-extraction sub-dialog.
    pub(crate) fn open_clock_import(&mut self) {
        self.clock_import = Some(ClockImport::new());
    }

    /// Render the sub-dialog; on success attaches the graph to `form`. Held
    /// mutably by the caller, like the pin importer. No-op while closed.
    pub(super) fn show_clock_import(&mut self, ui: &egui::Ui, form: &mut McuForm) {
        let Some(mut di) = self.clock_import.take() else {
            return;
        };
        let mut keep_open = true;

        // Poll the worker.
        if let Some(job) = &di.job {
            let done = matches!(&*job.lock().unwrap(), ClockJob::Done(_));
            if done {
                let result = {
                    let mut g = job.lock().unwrap();
                    match std::mem::replace(&mut *g, ClockJob::Running) {
                        ClockJob::Done(r) => Some(r),
                        ClockJob::Running => None,
                    }
                };
                di.job = None;
                match result {
                    Some(Ok(ex)) => {
                        let sysclk =
                            crate::panels::mcu_module::clock::graph::evaluate(&ex.clock.graph)
                                .get("sysclk")
                                .copied()
                                .unwrap_or(0);
                        let cached = if ex.from_cache { " · from cache" } else { "" };
                        di.note = Some(match &ex.report {
                            // Second pass: what matters is what the merge did.
                            Some(r) => format!(
                                "{} Branches merged: {}{}. Review them in the Clock tab.",
                                ph::CHECK_CIRCLE,
                                r.summary(),
                                cached
                            ),
                            None => format!(
                                "{} Clock imported ({} MHz SYSCLK){}. Review it in the Clock tab.",
                                ph::CHECK_CIRCLE,
                                sysclk / 1_000_000,
                                cached
                            ),
                        });
                        // Wires the datasheet asked for but the merge refused —
                        // worth showing, they are where a re-run should look.
                        if let Some(r) = &ex.report
                            && !r.skipped.is_empty()
                        {
                            di.note.get_or_insert_default().push_str(&format!(
                                "
Skipped: {}",
                                r.skipped.join("; ")
                            ));
                        }
                        form.set_imported_clock(ex.clock);
                        di.error = None;
                    }
                    Some(Err(e)) => {
                        di.error = Some(e);
                        di.note = None;
                    }
                    None => {}
                }
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        let running = di.job.is_some();

        let mut do_extract: Option<Pass> = None;
        let mut do_save_key = false;

        // The tree the second pass would merge onto — whatever the form holds
        // right now (an AI spine, a `.ron` import, or a family template; the
        // compact family forms are upgraded to a graph the same way loading a
        // chip does).
        let base_clock: Option<GraphClock> = {
            use crate::panels::mcu_module::clock::{ClockConfig, ClockLimits};
            match form.effective_clock().to_config(&ClockLimits::default()) {
                ClockConfig::Graph(g) => Some(g),
                ClockConfig::None => None,
            }
        };

        let force_default = !di.shown_once || (di.prev_maximized && !di.maximized);
        di.prev_maximized = di.maximized;
        di.shown_once = true;
        super::datasheet_import_dialog::window_frame(
            ui.ctx(),
            format!("{} Extract clock tree from datasheet (AI)", ph::SPARKLE),
            di.maximized,
            force_default,
            520.0,
            440.0,
            40.0,
        )
        .show(ui.ctx(), |ui| {
            super::datasheet_import_dialog::maximize_button(ui, &mut di.maximized);
            ui.label(
                egui::RichText::new(
                    "Two passes. SPINE (sources -> PLL -> SYSCLK -> AHB -> APB) builds the tree; \
                         BRANCHES then adds the low-speed paths, MCO and the peripheral kernel \
                         clocks on top of it. Each pass is verified against frequencies the \
                         datasheet itself states before it is accepted.",
                )
                .size(11.0)
                .color(egui::Color32::from_gray(160)),
            );
            ui.add_space(6.0);

            // ── Provider ─────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label("AI provider:");
                let before = di.provider;
                egui::ComboBox::from_id_salt("clk_provider")
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
            });

            // ── API key ──────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label(format!("{} API key:", di.provider.label()));
                ui.add(
                    egui::TextEdit::singleline(&mut di.api_key)
                        .password(!di.show_key)
                        .desired_width(240.0),
                );
                // Whitespace only, never case — see the same call in the pin
                // importer's key field for why lowercasing a key is a trap.
                di.api_key.retain(|c| !c.is_whitespace());
                ui.checkbox(&mut di.show_key, "show");
                if ui.button("Save key").clicked() {
                    do_save_key = true;
                }
            });
            if let Some(n) = &di.key_note {
                ui.label(egui::RichText::new(n).size(10.5));
            }

            // ── Model ────────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label("Model:");
                ui.add(egui::TextEdit::singleline(&mut di.model).desired_width(200.0))
                    .on_hover_text(di.provider.model_hint());
                // Lowercased as typed — see the same call in the pin importer's
                // model field for why in-place ASCII folding is the safe form.
                di.model.make_ascii_lowercase();
            });

            // ── Prompt (base read-only + supplementary) ──────────────
            let base = crate::panels::mcu_module::clock::graph::extract::build_clock_prompt();
            super::datasheet_import_dialog::prompt_section(
                ui,
                "clk_prompt",
                &mut di.prompt_open,
                &base,
                &mut di.extra_prompt,
            );

            ui.add_space(4.0);
            ui.separator();

            // ── Source: PDF or pasted text ───────────────────────────
            ui.horizontal(|ui| {
                if ui.button(format!("{} Choose PDF…", ph::FILE_PDF)).clicked() {
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
                            "{}  ({} KB)",
                            pdf.name,
                            pdf.bytes.len() / 1024
                        ))
                        .size(10.5)
                        .color(egui::Color32::from_rgb(150, 200, 150)),
                    );
                    if ui.button(ph::X).on_hover_text("Remove the PDF").clicked() {
                        di.pdf = None;
                    }
                }
            });
            if di.pdf.is_none() {
                ui.label(
                    egui::RichText::new(
                        "…or paste the clock-tree section (the RCC / clock text + the PLL and \
                             prescaler tables):",
                    )
                    .size(10.5)
                    .color(egui::Color32::from_gray(150)),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut di.text)
                        .desired_rows(6)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace),
                );
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut di.force_reextract, "re-extract")
                    .on_hover_text("Ignore the cache and call (and bill) the API again.");
                let has_source = di.pdf.is_some() || !di.text.trim().is_empty();
                let can = !running && !di.api_key.trim().is_empty() && has_source;
                if ui
                    .add_enabled(can, egui::Button::new(format!("{} Extract spine", ph::SPARKLE)))
                    .on_hover_text("Pass 1: sources -> PLL -> SYSCLK -> AHB -> APB")
                    .on_disabled_hover_text("needs an API key and a PDF or pasted text")
                    .clicked()
                {
                    do_extract = Some(Pass::Spine);
                }
                // Pass 2 needs something to merge ONTO, so it only lights up
                // once a tree exists.
                if ui
                    .add_enabled(
                        can && base_clock.is_some(),
                        egui::Button::new(format!("{} Extract branches", ph::SPARKLE)),
                    )
                    .on_hover_text(
                        "Pass 2: the low-speed paths, MCO and the peripheral kernel clocks, merged onto the tree you already have",
                    )
                    .on_disabled_hover_text("extract (or import) the spine first")
                    .clicked()
                {
                    do_extract = Some(Pass::Branches);
                }
                if running {
                    crate::app::helpers::spinner::throttled_spinner(ui, 12.0);
                    ui.label(
                        egui::RichText::new(" extracting…")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(220, 180, 70)),
                    );
                }
            });

            if let Some(note) = &di.note {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(note)
                        .size(11.0)
                        .color(egui::Color32::from_rgb(150, 200, 150)),
                );
            }
            if let Some(err) = &di.error {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(err)
                        .size(11.0)
                        .color(egui::Color32::from_rgb(220, 120, 100)),
                );
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Close").clicked() {
                    keep_open = false;
                }
            });
        });

        // ── Deferred side effects ────────────────────────────────────────────
        if do_save_key {
            di.key_note = Some(match ds::save_api_key(di.provider, &di.api_key) {
                Ok(()) => format!("{} Saved.", ph::CHECK_CIRCLE),
                Err(e) => format!("{} {e}", ph::X_CIRCLE),
            });
        }
        if let Some(pass) = do_extract
            && di.job.is_none()
        {
            let shared = Arc::new(Mutex::new(ClockJob::Running));
            di.job = Some(shared.clone());
            di.error = None;
            di.note = None;
            ds::save_extra_prompt(ds::PromptSlot::Clock, &di.extra_prompt);
            let (provider, key, model, extra) = (
                di.provider,
                di.api_key.clone(),
                di.model.clone(),
                di.extra_prompt.clone(),
            );
            let source = match &di.pdf {
                Some(pdf) => Source::Pdf(pdf.bytes.clone()),
                None => Source::Text(di.text.clone()),
            };
            let use_cache = !di.force_reextract;
            let ctx = ui.ctx().clone();
            let base = base_clock.clone();
            std::thread::spawn(move || {
                let res = match pass {
                    Pass::Spine => ds::call_ai_clock(
                        provider, &key, &model, &extra, &source, use_cache,
                    )
                    .map(|ex| Outcome {
                        clock: ex.clock,
                        from_cache: ex.from_cache,
                        report: None,
                    }),
                    // `base` is Some — the button is disabled otherwise.
                    Pass::Branches => match base {
                        Some(base) => ds::call_ai_clock_branches(
                            provider, &key, &model, &extra, &source, &base, use_cache,
                        )
                        .map(|ex| Outcome {
                            clock: ex.clock,
                            from_cache: ex.from_cache,
                            report: Some(ex.report),
                        }),
                        None => Err("No clock tree to merge into.".to_string()),
                    },
                };
                *shared.lock().unwrap() = ClockJob::Done(res);
                ctx.request_repaint();
            });
        }

        if keep_open {
            self.clock_import = Some(di);
        }
    }
}
