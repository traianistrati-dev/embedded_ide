//! The startup project picker.
//!
//! Shown when the window has nothing to open and something to choose from:
//! the preference says "always ask", or the project this instance remembered
//! is already open in another window. Which project a window opened used to be
//! decided by launch order — this is the screen that lets the user decide
//! instead.
//!
//! Same `egui::Modal` shape as the loading overlay, and for the same reason:
//! there is no usable app behind it yet, so blocking input is honest rather
//! than restrictive. The two are mutually exclusive by construction — nothing
//! is loading while the picker is up.

use super::AppIde;
use crate::app::helpers::forget_button::forget_button;
use crate::startup::StartupMode;
use eframe::egui;
use egui_phosphor::regular as ph;

/// How often the rows are rebuilt while the picker sits open. Reading the
/// recent file and probing every project's claim is filesystem work; doing it
/// per frame would be ~900 file opens a second. Two seconds is fast enough that
/// closing the other window makes its project selectable while you look at it.
const REFRESH: std::time::Duration = std::time::Duration::from_secs(2);

/// One row: a recent project plus whether another window has it right now.
struct Row {
    path: String,
    label: String,
    taken: bool,
}

/// In-flight picker state. `None` on `AppIde` means no picker.
pub(super) struct StartupPicker {
    /// What became of the project this instance had last: the primary action
    /// when it is free, the reason this screen is up when it is not.
    pub last: crate::startup::LastProject,
    /// Live copy of the preference, so the checkbox reflects edits made here.
    pub mode: StartupMode,
    rows: Vec<Row>,
    refreshed_at: std::time::Instant,
}

impl StartupPicker {
    pub(super) fn new(last: crate::startup::LastProject, mode: StartupMode) -> Self {
        Self {
            last,
            mode,
            rows: build_rows(),
            refreshed_at: std::time::Instant::now(),
        }
    }

    /// Rebuild the rows now, whatever the clock says — for a change this
    /// screen itself made, where waiting out [`REFRESH`] would look broken.
    fn force_refresh(&mut self) {
        self.rows = build_rows();
        self.refreshed_at = std::time::Instant::now();
    }

    /// Rebuild the rows if they are stale.
    fn refresh_if_due(&mut self) {
        if self.refreshed_at.elapsed() >= REFRESH {
            self.rows = build_rows();
            self.refreshed_at = std::time::Instant::now();
        }
    }
}

/// Folder leaf of a path — how a project is named everywhere in the UI.
fn leaf(dir: &std::path::Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string())
}

/// Read the recent list and probe each entry's claim — the only filesystem work
/// this screen does, kept to once every [`REFRESH`].
fn build_rows() -> Vec<Row> {
    crate::recent::load()
        .into_iter()
        .map(|entry| {
            let taken = matches!(
                crate::workspace::claim_project(std::path::Path::new(&entry.path)),
                crate::workspace::ProjectClaim::Busy
            );
            let label = match &entry.mcu_id {
                Some(id) => format!("{}   ({id})", entry.name),
                None => entry.name.clone(),
            };
            Row {
                path: entry.path,
                label,
                taken,
            }
        })
        .collect()
}

/// What the user picked this frame.
enum Choice {
    Open(std::path::PathBuf),
    Browse,
    New,
    Empty,
}

impl AppIde {
    /// Render the picker and act on the choice. No-op when none is up.
    pub(super) fn show_startup_picker(&mut self, ui: &mut egui::Ui) {
        let Some(state) = &mut self.startup_picker else {
            return;
        };
        state.refresh_if_due();
        let state = &*state;
        let mut mode = state.mode;
        let mut choice: Option<Choice> = None;
        // Deferred like `choice`: the loop below borrows `state.rows`, and
        // dropping an entry has to rebuild them.
        let mut forget: Option<String> = None;
        // Taken out of the picker for the frame: `state` is reborrowed
        // immutably for the loop, so the armed slot cannot live behind it.
        let mut armed = std::mem::take(&mut self.recent_forget_confirm);
        // The last project, split into the two things the UI does with it.
        let (resume, blocked) = match &state.last {
            crate::startup::LastProject::Available(dir) => (Some(dir.clone()), None),
            crate::startup::LastProject::OpenElsewhere(dir) => (None, Some(leaf(dir))),
            crate::startup::LastProject::None => (None, None),
        };

        egui::Modal::new(egui::Id::new("startup_picker"))
            .backdrop_color(egui::Color32::from_black_alpha(242))
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(28, 30, 34))
                    .inner_margin(18.0)
                    .corner_radius(6.0),
            )
            .show(ui.ctx(), |ui| {
                ui.set_width(560.0);
                ui.label(
                    egui::RichText::new("Choose a project")
                        .size(17.0)
                        .strong()
                        .color(egui::Color32::from_gray(235)),
                );
                if let Some(name) = &blocked {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  \"{name}\" is open in another window, so this one didn't \
                             reopen it.",
                            ph::WARNING
                        ))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(230, 190, 120)),
                    );
                }
                ui.add_space(10.0);

                // ── Continue with the last project ────────────────────────────
                // The one-keypress way through, which is what lets the picker be
                // the DEFAULT without taxing a single-window session.
                if let Some(dir) = &resume {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "{}  Continue with \"{}\"",
                                    ph::ARROW_FAT_RIGHT,
                                    leaf(dir)
                                ))
                                .size(13.0)
                                .strong(),
                            )
                            .min_size(egui::vec2(ui.available_width(), 30.0)),
                        )
                        .on_hover_text("Enter")
                        .clicked()
                    {
                        choice = Some(Choice::Open(dir.clone()));
                    }
                    ui.add_space(12.0);
                }

                // ── Recent projects ───────────────────────────────────────────
                if state.rows.is_empty() {
                    ui.label(
                        egui::RichText::new("No recent projects yet.")
                            .size(11.0)
                            .italics()
                            .color(egui::Color32::from_gray(150)),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for row in &state.rows {
                                ui.horizontal(|ui| {
                                    // Forget FIRST: laid out before the project
                                    // button, every one starts at the same x.
                                    // After it they stepped across the panel,
                                    // because the project button's width is a
                                    // minimum a long name grows past.
                                    if forget_button(ui, &row.path, &mut armed) {
                                        forget = Some(row.path.clone());
                                    }
                                    // A project another window holds can't be opened
                                    // here — say so on the row instead of letting the
                                    // click land and the conflict banner explain later.
                                    let resp = ui.add_enabled(
                                        !row.taken,
                                        egui::Button::new(
                                            egui::RichText::new(&row.label).size(12.5),
                                        )
                                        .min_size(egui::vec2(ui.available_width(), 0.0)),
                                    );
                                    if resp.on_hover_text(&row.path).clicked() {
                                        choice =
                                            Some(Choice::Open(std::path::PathBuf::from(&row.path)));
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(if row.taken {
                                        format!("   {}  open in another window", ph::LOCK_SIMPLE)
                                    } else {
                                        format!("   {}", row.path)
                                    })
                                    .size(9.5)
                                    .color(
                                        egui::Color32::from_gray(if row.taken { 150 } else { 115 }),
                                    ),
                                );
                                ui.add_space(4.0);
                            }
                        });
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{} Open folder…", ph::FOLDER_OPEN))
                        .clicked()
                    {
                        choice = Some(Choice::Browse);
                    }
                    if ui
                        .button(format!("{} New project", ph::NOTE_PENCIL))
                        .clicked()
                    {
                        choice = Some(Choice::New);
                    }
                    if ui.button("Start empty").clicked() {
                        choice = Some(Choice::Empty);
                    }
                });
                ui.add_space(10.0);

                // The setting belongs here, where the user is thinking about it
                // — it needs no other home.
                let mut ask = mode == StartupMode::AlwaysAsk;
                if ui
                    .checkbox(&mut ask, "Ask which project to open at every start")
                    .on_hover_text(
                        "Off: a window reopens the project it had last, unless another \
                         window already has it open",
                    )
                    .changed()
                {
                    mode = if ask {
                        StartupMode::AlwaysAsk
                    } else {
                        StartupMode::ReopenLast
                    };
                    crate::startup::save_mode(mode);
                }
                ui.label(
                    egui::RichText::new(
                        "Tip: start the IDE with a folder path to open it directly — \
                         embedded_ide_0 <project folder>. One shortcut per project.",
                    )
                    .size(9.5)
                    .italics()
                    .color(egui::Color32::from_gray(120)),
                );
            });

        // Enter takes the primary action, Escape starts empty — the same pair
        // every other modal in the app uses. Nothing else has focus behind a
        // modal, so reading the keys without consuming them is safe.
        ui.ctx().input(|i| {
            if i.key_pressed(egui::Key::Enter) {
                if let Some(dir) = &resume {
                    choice = Some(Choice::Open(dir.clone()));
                }
            }
            if i.key_pressed(egui::Key::Escape) {
                choice = Some(Choice::Empty);
            }
        });

        // Keep the (possibly toggled) preference visible in the checkbox.
        if let Some(s) = &mut self.startup_picker {
            s.mode = mode;
        }

        // Dropping an entry rebuilds the rows NOW rather than waiting for the
        // next `REFRESH`: two seconds of a row still sitting there reads as a
        // click that did nothing.
        // Back into `self`, so the arm survives to the next frame.
        self.recent_forget_confirm = armed;
        if let Some(path) = forget {
            crate::recent::forget(std::path::Path::new(&path));
            if let Some(s) = &mut self.startup_picker {
                s.force_refresh();
            }
        }

        // ── Act ───────────────────────────────────────────────────────────────
        // The picker closes FIRST in every arm: "New project" opens an ordinary
        // `egui::Window`, which renders BELOW this modal and would be
        // unclickable underneath it.
        match choice {
            Some(Choice::Open(dir)) => {
                self.startup_picker = None;
                self.load_project_from_dir(&dir);
                self.workspace_write_requested = true;
            }
            Some(Choice::Browse) => {
                if let Some(dir) = rfd::FileDialog::new()
                    .set_title("Open Embedded IDE Project — pick the project root folder")
                    .pick_folder()
                {
                    self.startup_picker = None;
                    self.load_project_from_dir(&dir);
                    self.workspace_write_requested = true;
                }
                // Cancelling the folder picker leaves this screen up — the user
                // still has to choose something.
            }
            Some(Choice::New) => {
                self.startup_picker = None;
                self.begin_new_project();
            }
            Some(Choice::Empty) => self.startup_picker = None,
            None => {}
        }
    }
}
