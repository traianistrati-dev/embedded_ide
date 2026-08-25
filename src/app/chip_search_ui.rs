//! The chip search field in the New Project dialog.
//!
//! Picking a chip used to mean one of two things: choosing from the handful the
//! IDE already knew, or knowing that `STM32F103C8Tx` lives in a file called
//! `STM32F103C(8-B)Tx.xml` and finding it in a file dialog. This is the third
//! way — type the part number.
//!
//! The ranking lives in [`chip_search`](crate::panels::mcu_module::chip_search)
//! and the catalogue in
//! [`chip_sources`](crate::panels::mcu_module::chip_sources); this module is the
//! field, the list and the thread that keeps indexing off the frame.
//!
//! # Why indexing is off-thread
//!
//! Cataloguing a CubeMX installation is ~2800 parts out of a 4.8 MB index, which
//! measured 563 ms in a debug build. Doing that on the frame that opens the
//! dialog would be half a second of frozen window, so it runs on a worker and
//! the field says "indexing…" until it lands.

use eframe::egui;
use egui_phosphor::regular as ph;

use super::chip_filter_ui::{self, Facets};
use crate::panels::mcu_module::chip_filter::{self, ChipFilter};
use crate::panels::mcu_module::chip_search::{self, Catalogue, Origin, RegistryRow};
use crate::panels::mcu_module::chip_sources;

/// How many matches the list shows at once. The catalogue is thousands of parts
/// long; past a screenful, more rows are not more useful — a narrower query is.
const MAX_ROWS: usize = 40;

/// The search field's state, owned by the app.
#[derive(Default)]
pub(super) struct ChipSearchState {
    pub query: String,
    /// The DRAFT the Filters panel edits. Nothing acts on it until Apply.
    pub filter: ChipFilter,
    /// What the list is actually showing.
    ///
    /// Separate from `filter` because a slider fires a change on every frame it
    /// is dragged, and each change is a fresh search over several thousand
    /// parts. Committing on a button turns a drag from hundreds of searches
    /// into one.
    applied: ChipFilter,
    /// The last search, with the inputs that produced it.
    ///
    /// `search` runs from the frame callback, so without this it re-runs at the
    /// refresh rate even when nothing has changed — an Apply button alone would
    /// have stopped the filter from CHANGING every frame while leaving it
    /// re-evaluated every frame.
    cached: Option<(String, ChipFilter, usize, chip_search::Results)>,
    /// What the catalogue offers to filter by — derived once it lands.
    facets: Facets,
    catalogue: Option<Catalogue>,
    /// The worker's channel while indexing is in flight.
    pending: Option<std::sync::mpsc::Receiver<Catalogue>>,
    /// Result of the last source change, shown under the list.
    pub note: String,
}

impl ChipSearchState {
    /// Kick off indexing once, and adopt the result when it arrives.
    fn poll(&mut self, ctx: &egui::Context) {
        if self.catalogue.is_some() {
            return;
        }
        match &self.pending {
            None => {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    // A closed channel just means the dialog went away.
                    let _ = tx.send(Catalogue::build(chip_sources::all_sources()));
                });
                self.pending = Some(rx);
                ctx.request_repaint();
            }
            Some(rx) => match rx.try_recv() {
                Ok(c) => {
                    // The dialog can be drawn before indexing finishes, so the
                    // sliders start on placeholder bounds. Re-spanning them here
                    // is what stops an untouched filter from reading as active
                    // the moment the real catalogue arrives.
                    self.facets = Facets::of(&c);
                    self.filter.rebound(self.facets.bounds);
                    self.applied.rebound(self.facets.bounds);
                    self.cached = None;
                    self.catalogue = Some(c);
                    self.pending = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                // The worker died; stop waiting on it rather than spin forever.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.pending = None,
            },
        }
    }

    /// Re-index — after the set of sources changes.
    fn reload(&mut self) {
        self.catalogue = None;
        self.pending = None;
        // The facets describe the OLD set of sources; keeping them would offer
        // filters for peripherals no remaining source mentions.
        self.facets = Facets::default();
        self.cached = None;
    }
}

/// What a click on the list asked for, resolved after the borrows end.
enum Action {
    /// A chip the registry already has.
    Select(String),
    /// A vendor file to import, the source it came from (which decides whether
    /// the clock tree comes with it), and the part that was actually asked for —
    /// a range file yields several chips, and the importer selects the LAST,
    /// which is rarely the one that was clicked.
    Import {
        path: std::path::PathBuf,
        source: chip_sources::ChipSource,
        part: String,
    },
}

/// Where to GET chip data, for a machine that does not have it yet.
///
/// The three links are not interchangeable, and the difference is the whole
/// reason this is spelled out rather than being one "download" link: ST
/// publishes the per-chip files openly, but the clock trees
/// (`db/plugins/clock`) ship only inside CubeMX. Someone who clones the open
/// repo expecting a Clock tab gets pins and a blank diagram.
///
/// Collapsed once a source with clock trees is present, because at that point
/// this is answered — and open otherwise, because at that point it is the most
/// useful thing on the screen.
fn source_links(ui: &mut egui::Ui, have_clock: bool) {
    if !have_clock {
        ui.label(
            egui::RichText::new(format!(
                "{}  Nothing here can supply a clock tree — chips will import with pins only.",
                ph::WARNING
            ))
            .size(10.5)
            .color(egui::Color32::from_rgb(225, 185, 60)),
        );
    }
    egui::CollapsingHeader::new(
        egui::RichText::new("Where to get chip data")
            .size(10.5)
            .color(egui::Color32::GRAY),
    )
    .id_salt("chip_data_links")
    .default_open(!have_clock)
    .show(ui, |ui| {
        let note = |ui: &mut egui::Ui, text: &str| {
            ui.label(
                egui::RichText::new(text)
                    .size(10.0)
                    .color(egui::Color32::from_gray(140)),
            );
        };

        ui.hyperlink_to(
            egui::RichText::new(format!("{} STM32CubeMX", ph::CPU)).size(11.0),
            "https://www.st.com/en/development-tools/stm32cubemx.html",
        );
        note(
            ui,
            "The only official source of clock trees. Install it, then add its `db` folder \
             above — nothing else needs to be run.",
        );
        ui.add_space(3.0);

        ui.hyperlink_to(
            egui::RichText::new(format!("{} STM32_open_pin_data", ph::GIT_BRANCH)).size(11.0),
            "https://github.com/STMicroelectronics/STM32_open_pin_data",
        );
        note(
            ui,
            "ST's open chip data — pins, packages and memory, no clock trees. A git clone; \
             add its `mcu` folder above.",
        );
        ui.add_space(3.0);

        ui.hyperlink_to(
            egui::RichText::new(format!("{} esden/stm32cube-database", ph::GIT_BRANCH)).size(11.0),
            "https://github.com/esden/stm32cube-database",
        );
        note(
            ui,
            "A community mirror of the CubeMX database, clock trees included — for a machine \
             with no CubeMX. It is an OLDER snapshot: the newest parts and RCC revisions are \
             missing, so prefer a real installation where you have one.",
        );
    });
}

impl super::AppIde {
    /// Draw the search field, its results and the source list.
    pub(super) fn show_chip_search(&mut self, ui: &mut egui::Ui) {
        self.chip_search.poll(ui.ctx());

        let mut action: Option<Action> = None;
        let mut add_folder = false;
        // Deferred like `add_folder`: acting on it needs `&mut self`, which the
        // closure drawing the rows does not have.
        let mut remove_source: Option<std::path::PathBuf> = None;

        // Borrow the three fields apart, so the list can read the registry while
        // writing the selection.
        let Self {
            chip_search,
            mcu_registry,
            pending_mcu_id,
            ..
        } = self;

        ui.horizontal(|ui| {
            ui.label("Search:");
            // No debounce and nothing to invalidate: the list is recomputed from
            // the pre-lowercased keys every frame, which is a linear scan over a
            // few thousand short strings.
            ui.add(
                egui::TextEdit::singleline(&mut chip_search.query)
                    .hint_text("part number, e.g. F103C8")
                    .desired_width(200.0),
            );
            if !chip_search.query.is_empty()
                && ui
                    .small_button(ph::X)
                    .on_hover_text("Clear the search")
                    .clicked()
            {
                chip_search.query.clear();
            }
            if chip_search.catalogue.is_none() {
                ui.spinner();
                ui.label(
                    egui::RichText::new("indexing…")
                        .size(10.5)
                        .color(egui::Color32::GRAY),
                );
            }
        });

        // The fields apart: the filter panel WRITES `filter` while the search
        // READS `catalogue`, and they live in the same struct.
        let ChipSearchState {
            query,
            filter,
            applied,
            cached,
            facets,
            catalogue,
            note,
            ..
        } = chip_search;
        let Some(cat) = catalogue.as_ref() else {
            return;
        };

        chip_filter_ui::show_filters(ui, filter, applied, facets);

        // The registry, in the shape the ranking wants.
        let registry: Vec<RegistryRow> = mcu_registry
            .iter()
            .map(|d| RegistryRow {
                id: d.id.as_str(),
                name: d.display_name.as_str(),
                family: d.family.as_str(),
                // A registry entry carries no vendor row of its own; its
                // `memory.x` sizes are all it knows about itself. The search
                // upgrades these from the vendor file whenever one exists.
                flash_kb: chip_filter::parse_memory_kb(&d.project.flash_size),
                ram_kb: chip_filter::parse_memory_kb(&d.project.ram_size),
                package: d.package.as_str(),
            })
            .collect();

        // Re-run only when an input moved. The registry length is in the key
        // because an import adds a chip, and the list must show it at once.
        let fresh = cached
            .as_ref()
            .is_some_and(|(q, f, n, _)| q == query && f == applied && *n == registry.len());
        if !fresh {
            *cached = Some((
                query.clone(),
                applied.clone(),
                registry.len(),
                cat.search(query, &registry, applied, MAX_ROWS),
            ));
        }
        let found = &cached.as_ref().expect("just filled").3;
        let (hits, total) = (&found.hits, found.total);
        let filter = &*applied;

        // Typing is no longer the only way to ask a question: with a filter set,
        // an empty query means "show me everything that fits".
        if !query.trim().is_empty() || filter.is_active() {
            if hits.is_empty() {
                // WHICH of the two narrowed it to nothing, because they are
                // fixed differently: one by typing less, the other by a Clear
                // button the user may not have noticed is on.
                let why = match (query.trim().is_empty(), filter.active_count()) {
                    (_, 0) => "No chip matches that.".to_owned(),
                    (true, n) => format!("No chip matches those {n} filter(s)."),
                    (false, n) => format!("No chip matches that, with {n} filter(s) on."),
                };
                ui.label(
                    egui::RichText::new(format!("{}  {why}", ph::MAGNIFYING_GLASS))
                        .size(11.0)
                        .color(egui::Color32::GRAY),
                );
            } else {
                if total > hits.len() {
                    let how = if query.trim().is_empty() {
                        "narrow the filters or type a part number"
                    } else {
                        "keep typing to narrow it"
                    };
                    ui.label(
                        egui::RichText::new(format!("{} of {total} matches — {how}", hits.len()))
                            .size(10.5)
                            .color(egui::Color32::GRAY),
                    );
                }
                egui::ScrollArea::vertical()
                    .id_salt("chip_search_results")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        ui.set_min_width(500.0);
                        for hit in hits {
                            ui.horizontal(|ui| {
                                let known = hit.origin.is_registry();
                                let selected = match &hit.origin {
                                    Origin::Registry { id } => {
                                        pending_mcu_id.as_deref() == Some(id.as_str())
                                    }
                                    Origin::Disk { .. } => false,
                                };
                                // The part number is the button: one click does
                                // the obvious thing, whichever kind of row it is.
                                let label = if known {
                                    egui::RichText::new(&hit.name)
                                } else {
                                    egui::RichText::new(format!(
                                        "{}  {}",
                                        ph::DOWNLOAD_SIMPLE,
                                        hit.name
                                    ))
                                };
                                let btn = ui.add(
                                    egui::Button::new(label)
                                        .selected(selected)
                                        .min_size(egui::vec2(170.0, 0.0)),
                                );
                                let btn = if known {
                                    btn.on_hover_text("Use this chip")
                                } else {
                                    btn.on_hover_text(
                                        "Import this chip from its vendor file and select it",
                                    )
                                };
                                if btn.clicked() {
                                    action = Some(match &hit.origin {
                                        Origin::Registry { id } => Action::Select(id.clone()),
                                        Origin::Disk { source, file, .. } => {
                                            let src = &cat.sources[*source];
                                            Action::Import {
                                                path: src.chips.join(format!("{file}.xml")),
                                                source: src.clone(),
                                                part: hit.name.clone(),
                                            }
                                        }
                                    });
                                }
                                ui.label(
                                    egui::RichText::new(&hit.family)
                                        .size(10.5)
                                        .color(egui::Color32::from_rgb(150, 158, 172)),
                                );
                                if !hit.detail.is_empty() {
                                    ui.label(
                                        egui::RichText::new(&hit.detail)
                                            .size(10.5)
                                            .color(egui::Color32::GRAY),
                                    );
                                }
                                // What this row can actually deliver — said
                                // before the click, not discovered after it.
                                let (tag, color) = match &hit.origin {
                                    Origin::Registry { .. } => (
                                        "already added".to_string(),
                                        egui::Color32::from_rgb(120, 200, 120),
                                    ),
                                    Origin::Disk {
                                        has_clock: true, ..
                                    } => (
                                        "pins + clock".to_string(),
                                        egui::Color32::from_rgb(120, 190, 200),
                                    ),
                                    Origin::Disk {
                                        has_clock: false, ..
                                    } => (
                                        "pins only".to_string(),
                                        egui::Color32::from_rgb(225, 185, 60),
                                    ),
                                };
                                ui.label(egui::RichText::new(tag).size(10.0).color(color));
                                // A chip already in the registry can still be
                                // refreshed from its vendor file: the data may have
                                // moved on (a newer CubeMX) or the import itself may
                                // have been fixed since. Secondary, never the primary
                                // action - the row's job is still "pick this chip".
                                if let Some(Origin::Disk { source, file, .. }) = &hit.reimport {
                                    if ui
                                        .small_button(ph::ARROW_CLOCKWISE)
                                        .on_hover_text(
                                            "Re-import from the vendor file, overwriting the stored definition",
                                        )
                                        .clicked()
                                    {
                                        let src = &cat.sources[*source];
                                        action = Some(Action::Import {
                                            path: src.chips.join(format!("{file}.xml")),
                                            source: src.clone(),
                                            part: hit.name.clone(),
                                        });
                                    }
                                }
                            });
                        }
                    });
            }

            // Said out loud rather than absorbed. An open-pin-data checkout
            // ships part numbers and nothing else, so ANY filter hides all of
            // it - and a source vanishing without a word looks exactly like a
            // filter that found nothing.
            if found.unknown > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  {} more hidden: their source has no index, so nothing is known about their memory or peripherals.",
                        ph::INFO,
                        found.unknown
                    ))
                    .size(10.0)
                    .color(egui::Color32::from_rgb(150, 158, 172)),
                );
            }
        }

        // ── Sources ───────────────────────────────────────────────────────────
        // Collapsed by default: this is reference material (which vendor
        // folders are indexed, and where to get more), not something you
        // touch while picking a chip. It was three lines of paths between
        // the search results and the buttons under them.
        ui.add_space(4.0);
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("{} Sources", ph::DATABASE))
                .size(10.5)
                .color(egui::Color32::GRAY),
        )
        .id_salt("new_project_sources")
        .default_open(false)
        .show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} Sources:", ph::DATABASE))
                        .size(10.5)
                        .color(egui::Color32::GRAY),
                );
                if ui
                    .small_button("Add a folder…")
                    .on_hover_text(
                        "A STM32CubeMX installation (chips + clock trees) or an \
                         STM32_open_pin_data checkout (chips only)",
                    )
                    .clicked()
                {
                    add_folder = true;
                }
            });
            if cat.sources.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  No STM32CubeMX installation found — add a folder to search vendor data.",
                        ph::WARNING
                    ))
                    .size(10.0)
                    .color(egui::Color32::from_rgb(225, 185, 60)),
                );
            }
                // The number that actually answers "how many chips can I search":
            // not the sum of the rows above, because a part in two sources is
            // one part, and the copy that survives is the one that knows more.
            if cat.sources.len() > 1 {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  {} distinct parts in all sources together — searched and filtered as one set",
                        ph::FUNNEL,
                        cat.unified_len()
                    ))
                    .size(10.0)
                    .color(egui::Color32::from_rgb(150, 200, 160)),
                );
                ui.add_space(2.0);
            }
            for (ix, src) in cat.sources.iter().enumerate() {
                let (what, color) = if src.has_clock() {
                    ("pins + clock trees", egui::Color32::from_rgb(120, 190, 200))
                } else {
                    ("pins only", egui::Color32::from_rgb(225, 185, 60))
                };
                ui.horizontal(|ui| {
                    // Removal is only honest for a folder the user added: an
                    // auto-detected install would be found again next launch, so
                    // a button promising to remove it would be a lie.
                    if src.user_added {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(ph::X)
                                        .size(10.0)
                                        .color(egui::Color32::from_rgb(220, 110, 90)),
                                )
                                .small()
                                .frame(false),
                            )
                            .on_hover_text("Forget this folder")
                            .clicked()
                        {
                            remove_source = Some(src.chips.clone());
                        }
                    } else {
                        ui.add_enabled(
                            false,
                            egui::Button::new(egui::RichText::new(ph::X).size(10.0))
                                .small()
                                .frame(false),
                        )
                        .on_disabled_hover_text(
                            "Found automatically — uninstall it to stop it being used",
                        );
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{} — {} parts,",
                            src.kind.label(),
                            cat.count_of(ix)
                        ))
                        .size(10.0)
                        .color(egui::Color32::from_gray(140)),
                    );
                    ui.label(egui::RichText::new(what).size(10.0).color(color));
                });
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(src.chips.display().to_string())
                            .size(9.5)
                            .monospace()
                            .color(egui::Color32::from_gray(110)),
                    )
                    .truncate(),
                );
            }
            source_links(ui, cat.sources.iter().any(|s| s.has_clock()));
        });

        for (ix, err) in &cat.errors {
            let where_ = cat
                .sources
                .get(*ix)
                .map(|s| s.chips.display().to_string())
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(format!("{}  {where_}: {err}", ph::X_CIRCLE))
                    .size(10.0)
                    .color(egui::Color32::from_rgb(220, 120, 90)),
            );
        }
        if !note.is_empty() {
            ui.label(
                egui::RichText::new(&*note)
                    .size(10.5)
                    .color(egui::Color32::from_rgb(150, 200, 160)),
            );
        }

        // ── Deferred: both of these need `&mut self` ─────────────────────────
        if add_folder {
            self.add_chip_source();
        }
        if let Some(path) = remove_source {
            self.remove_chip_source(&path);
        }
        match action {
            Some(Action::Select(id)) => self.pending_mcu_id = Some(id),
            Some(Action::Import { path, source, part }) => {
                self.import_searched_chip(&path, &source, &part)
            }
            None => {}
        }
    }

    /// Ask for a folder, keep it if it is a usable source, and re-index.
    /// Forget a folder the user added.
    ///
    /// Only the remembered list is touched — nothing on disk is deleted, and an
    /// auto-detected install is unaffected because it was never in that list.
    fn remove_chip_source(&mut self, chips: &std::path::Path) {
        let before = chip_sources::saved_paths();
        // The saved path may be the folder the user PICKED (a CubeMX root),
        // while `chips` is the `db/mcu` inside it — so match on either being a
        // prefix of the other rather than on equality, which would silently
        // remove nothing.
        let after: Vec<std::path::PathBuf> = before
            .iter()
            .filter(|p| !(chips.starts_with(p.as_path()) || p.starts_with(chips)))
            .cloned()
            .collect();
        if after.len() == before.len() {
            self.chip_search.note = format!(
                "{}  {} is not one of the remembered folders",
                ph::WARNING,
                chips.display()
            );
            return;
        }
        self.chip_search.note = match chip_sources::save_paths(&after) {
            Ok(()) => format!("{}  Forgot {}", ph::CHECK, chips.display()),
            Err(e) => format!("{}  Could not forget that folder: {e}", ph::WARNING),
        };
        self.chip_search.reload();
    }

    fn add_chip_source(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Add a chip data folder")
            .pick_folder()
        else {
            return;
        };
        let Some(src) = chip_sources::from_path(&path) else {
            self.chip_search.note = format!(
                "{}  No STM32 chip XMLs in {} — pick a CubeMX `db` folder or an \
                 STM32_open_pin_data checkout.",
                ph::WARNING,
                path.display()
            );
            return;
        };
        let mut saved = chip_sources::saved_paths();
        if !saved.contains(&path) {
            saved.push(path.clone());
        }
        self.chip_search.note = match chip_sources::save_paths(&saved) {
            Ok(()) => format!(
                "{}  Added {} ({})",
                ph::CHECK,
                src.kind.label(),
                if src.has_clock() {
                    "pins + clock trees"
                } else {
                    "pins only"
                }
            ),
            Err(e) => format!("{}  Could not remember that folder: {e}", ph::WARNING),
        };
        self.chip_search.reload();
    }

    /// Import the file a search hit names — pins AND clock — then select the
    /// part that was CLICKED.
    ///
    /// The source is passed on rather than dropped: it is what says whether a
    /// clock tree can come with the pins, and it is the row the user chose, not
    /// a guess. The bulk importer selects the last chip it saved, which for a
    /// range file (`STM32F103C(8-B)Tx.xml` holds both the C8 and the CB) is the
    /// wrong one about half the time. It has no way to know better — it takes
    /// files, not part numbers — so the correction belongs here.
    fn import_searched_chip(
        &mut self,
        path: &std::path::Path,
        source: &chip_sources::ChipSource,
        part: &str,
    ) {
        self.import_stm32_pin_data_from(std::slice::from_ref(&path.to_path_buf()), Some(source));
        if let Some(def) = self
            .mcu_registry
            .iter()
            .find(|d| d.display_name.eq_ignore_ascii_case(part))
        {
            self.pending_mcu_id = Some(def.id.clone());
        }
    }
}
