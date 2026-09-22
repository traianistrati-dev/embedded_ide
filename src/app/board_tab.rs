//! The Board tab's side of the app: which system is open, putting chips into
//! it, and the canvas.
//!
//! The system is the PARENT folder of a chip project, when that folder has a
//! `system.config` - so opening any chip of a system opens its Board too, and
//! nothing about a chip project changes by being in one. Opening a project
//! that belongs to no system leaves the Board as it was.
//!
//! A chip gets into a system two ways:
//! * **New chip** runs the ordinary New Project dialog; once a chip is picked
//!   there, the project is saved straight INTO the system (no folder dialog)
//!   and listed in `system.config`.
//! * **Add existing project** copies a saved project into the system folder
//!   (files only - no `target/`, no `.git`), or just lists one already there.
//!
//! `system.config` is shared state - another window of the same system, or a
//! hand edit, can change it at any time - so every write re-reads it and
//! applies only this window's own change ([`model::carry_positions`]).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use eframe::egui;
use egui_phosphor::regular as ph;

use super::AppIde;
use crate::panels::board::model::{self, SystemConfig};
use crate::panels::board::snapshot::{self, ChipView};
use crate::panels::board::{gui, layout};

/// Everything the Board tab keeps between frames.
pub(crate) struct BoardState {
    /// The open system's folder, without a `\\?\` prefix.
    pub root: Option<PathBuf>,
    pub config: SystemConfig,
    /// One view per chip, in `config` order, read from disk. The chip open in
    /// this window is rebuilt from the live `Mcu` every frame instead.
    pub views: Vec<ChipView>,
    /// The project in this window is a "New chip" with its chip picked: its
    /// first Save goes into `root` and lists it there.
    pub adding_chip: bool,
    /// "New chip" passed the unsaved-changes gate and the New Project dialog
    /// is on its way; the dialog's OK turns it into `adding_chip`. Not
    /// before: until then the OLD project is still loaded, and a Save of it
    /// must not go into the system.
    pub new_chip_intent: bool,
    /// Clicked on the Board, acted on next frame at the same gates the
    /// toolbar's New / Open go through - the canvas draws after them.
    pub new_chip_request: bool,
    pub open_request: Option<PathBuf>,
    /// The last thing to tell the user: text, and whether it is an error.
    pub notice: Option<(String, bool)>,
    /// The canvas view, and whether the user has zoomed or panned it (until
    /// then it follows the frames).
    pub scene_rect: egui::Rect,
    pub view_adjusted: bool,
    /// Frame number the tab last drew on. A gap means it was hidden, and the
    /// system is read again: another window may have changed it meanwhile.
    pub last_frame: u64,
    /// Frames dragged here and not written yet (folder names, lower-cased).
    pub moved: BTreeSet<String>,
}

impl Default for BoardState {
    fn default() -> Self {
        Self {
            root: None,
            config: SystemConfig::default(),
            views: Vec::new(),
            adding_chip: false,
            new_chip_intent: false,
            new_chip_request: false,
            open_request: None,
            notice: None,
            scene_rect: egui::Rect::NOTHING,
            view_adjusted: false,
            last_frame: 0,
            moved: BTreeSet::new(),
        }
    }
}

/// How listing a folder as a chip went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Listed {
    Added,
    Already,
    /// A name `system.config` cannot hold (see [`model::listable`]).
    BadName,
}

/// Why a folder cannot be added as a chip, or `None` when it can.
fn not_a_chip(src: &Path, root: &Path) -> Option<&'static str> {
    if root.starts_with(src) {
        Some("That folder holds the system itself.")
    } else if model::is_system_root(src) {
        Some("That folder is a system, not a chip project.")
    } else if !is_project(src) {
        Some("Not a chip project - it has no Cargo.toml or src/main.rs.")
    } else {
        None
    }
}

fn is_project(dir: &Path) -> bool {
    dir.join("Cargo.toml").is_file() || dir.join("src").join("main.rs").is_file()
}

const BAD_NAME: &str = "system.config cannot list a folder whose name contains `=` or starts with \
                        `#` or `@` - rename the folder first.";

impl AppIde {
    fn board_notice(&mut self, text: impl Into<String>, error: bool) {
        self.board.notice = Some((text.into(), error));
    }

    /// Open the system at `root`: read its config and its chips.
    pub(super) fn board_open_system(&mut self, root: PathBuf) {
        self.board_flush_positions();
        let root = model::plain(&root);
        match model::load(&root) {
            Ok(cfg) => {
                self.board.root = Some(root);
                self.board.config = cfg;
                self.board.moved.clear();
                self.board.view_adjusted = false;
                self.board_refresh();
            }
            Err(e) => self.board_notice(e, true),
        }
    }

    /// Stop showing the system. Nothing on disk changes.
    fn board_close_system(&mut self) {
        self.board_flush_positions();
        let scene_rect = self.board.scene_rect;
        self.board = BoardState {
            scene_rect,
            ..BoardState::default()
        };
    }

    /// Read every chip again from disk, and give each frame without a spot the
    /// one it is drawn at - so it stays there while another frame is dragged.
    fn board_refresh(&mut self) {
        let Some(root) = self.board.root.clone() else {
            self.board.views.clear();
            return;
        };
        self.board.views = self
            .board
            .config
            .chips
            .iter()
            .map(|c| snapshot::read_chip(&root, &c.dir, &self.mcu_registry))
            .collect();
        let spots: Vec<(Option<(f32, f32)>, egui::Vec2)> = self
            .board
            .config
            .chips
            .iter()
            .zip(gui::sizes(&self.board.views))
            .map(|(c, s)| (c.pos, s))
            .collect();
        let at = layout::frame_positions(&spots);
        for (c, p) in self.board.config.chips.iter_mut().zip(at) {
            c.pos.get_or_insert((p.x, p.y));
        }
    }

    /// Re-read the system from disk (writing this window's moves first).
    fn board_sync(&mut self) {
        if !self.board.moved.is_empty() {
            self.board_edit_config(|_| false);
            return;
        }
        if let Some(root) = self.board.root.clone()
            && let Ok(cfg) = model::load(&root)
        {
            self.board.config = cfg;
        }
        self.board_refresh();
    }

    /// Change `system.config`: re-read it, carry this window's frame positions
    /// over, apply `edit`, write it back when anything changed, re-read the
    /// chips. Returns what `edit` returned.
    fn board_edit_config(&mut self, edit: impl FnOnce(&mut SystemConfig) -> bool) -> bool {
        let Some(root) = self.board.root.clone() else {
            return false;
        };
        let mut cfg = model::load(&root).unwrap_or_else(|_| self.board.config.clone());
        let carried = model::carry_positions(&mut cfg, &self.board.config, &self.board.moved);
        let changed = edit(&mut cfg);
        self.board.config = cfg;
        self.board.moved.clear();
        if (carried || changed)
            && let Err(e) = model::save(&root, &self.board.config)
        {
            self.board_notice(e, true);
        }
        self.board_refresh();
        changed
    }

    /// Write dragged frames down, if any were dragged.
    fn board_flush_positions(&mut self) {
        if !self.board.moved.is_empty() {
            self.board_edit_config(|_| false);
        }
    }

    /// List `dir` (a folder directly under the root) as a chip, to the right
    /// of the others.
    fn board_register(&mut self, dir: &str) -> Listed {
        if !model::listable(dir) {
            return Listed::BadName;
        }
        let sizes: HashMap<String, egui::Vec2> = self
            .board
            .views
            .iter()
            .zip(gui::sizes(&self.board.views))
            .map(|(v, s)| (v.dir.to_ascii_lowercase(), s))
            .collect();
        let added = self.board_edit_config(|cfg| {
            let mut spots: Vec<(Option<(f32, f32)>, egui::Vec2)> = cfg
                .chips
                .iter()
                .map(|c| {
                    let size = sizes.get(&c.dir.to_ascii_lowercase()).copied();
                    (c.pos, size.unwrap_or(egui::vec2(layout::FRAME_W, 0.0)))
                })
                .collect();
            spots.push((None, egui::vec2(layout::FRAME_W, 0.0)));
            let at = layout::frame_positions(&spots)[spots.len() - 1];
            cfg.add(dir, Some((at.x, at.y)))
        });
        if added {
            Listed::Added
        } else {
            Listed::Already
        }
    }

    /// `board_register` plus the notice that says how it went.
    fn board_register_and_say(&mut self, dir: &str) {
        match self.board_register(dir) {
            Listed::Added => self.board_notice(format!("Added {dir} to the system."), false),
            Listed::Already => self.board_notice(format!("{dir} is already in the system."), false),
            Listed::BadName => self.board_notice(BAD_NAME, true),
        }
    }

    /// The folder of the chip open in this window, when it sits in the system.
    fn board_open_chip_dir(&self) -> Option<String> {
        let root = self.board.root.as_deref()?;
        let dir = model::plain(self.project_dir.as_deref()?);
        model::same_dir(dir.parent()?, root)
            .then(|| dir.file_name()?.to_str().map(str::to_owned))
            .flatten()
    }

    /// A project was opened: show its system, if it is in one. Called at the
    /// end of `load_project_from_dir`.
    pub(super) fn board_follow_project(&mut self, project_dir: &Path) {
        self.board.adding_chip = false;
        self.board.new_chip_intent = false;
        if let Some(root) = model::system_root_of(project_dir) {
            if self
                .board
                .root
                .as_deref()
                .is_some_and(|r| model::same_dir(r, &root))
            {
                // Same system: re-read it, the open may be a branch switch.
                self.board_sync();
            } else {
                self.board_open_system(root);
            }
        }
    }

    /// Where a "New chip" project is created on its first Save: the system
    /// root, without asking. `None` for any other new project.
    pub(super) fn board_new_chip_parent(&self) -> Option<PathBuf> {
        if self.board.adding_chip && self.project_dir.is_none() {
            self.board.root.clone()
        } else {
            None
        }
    }

    /// A Save just gave a new project its folder. If it was a "New chip", list
    /// it in the system.
    pub(super) fn board_register_saved_chip(&mut self) {
        if !std::mem::take(&mut self.board.adding_chip) {
            return;
        }
        if let Some(dir) = self.board_open_chip_dir() {
            self.board_register_and_say(&dir);
        }
    }

    fn board_new_system(&mut self) {
        let Some(folder) = rfd::FileDialog::new()
            .set_title("New system - choose or create an empty folder for it")
            .pick_folder()
        else {
            return;
        };
        let folder = model::plain(&folder);
        if model::is_system_root(&folder) {
            self.board_open_system(folder);
            self.board_notice("That folder already is a system - opened it.", false);
            return;
        }
        if folder.join("Cargo.toml").is_file() {
            self.board_notice(
                "That folder is a chip project. A system keeps its chips in subfolders - pick \
                 or create an empty folder.",
                true,
            );
            return;
        }
        if let Err(e) = model::save(&folder, &SystemConfig::default()) {
            self.board_notice(e, true);
            return;
        }
        self.board_open_system(folder.clone());
        let name = folder
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("system");
        self.board_notice(format!("Created the system {name}."), false);
        // The chip open right now already lives in that folder: it is the
        // system's first chip, not something to add by hand.
        if let Some(dir) = self.board_open_chip_dir() {
            self.board_register_and_say(&dir);
        }
    }

    fn board_open_system_dialog(&mut self) {
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Open system - pick the folder that holds system.config")
            .pick_folder()
        else {
            return;
        };
        if model::is_system_root(&folder) {
            self.board_open_system(folder);
        } else if let Some(root) = model::system_root_of(&folder) {
            // A chip folder: its system is the one meant.
            self.board_open_system(root);
        } else {
            self.board_notice(
                format!(
                    "No {} in that folder, or in the one above it.",
                    model::FILE_NAME
                ),
                true,
            );
        }
    }

    fn board_add_existing(&mut self) {
        let Some(root) = self.board.root.clone() else {
            return;
        };
        let Some(src) = rfd::FileDialog::new()
            .set_title("Add existing project - pick a chip project folder")
            .pick_folder()
        else {
            return;
        };
        let src = model::plain(&src);
        if let Some(why) = not_a_chip(&src, &root) {
            self.board_notice(why, true);
            return;
        }
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("chip")
            .to_owned();
        // Already in the system folder: list it, copy nothing.
        if src.parent().is_some_and(|p| model::same_dir(p, &root)) {
            self.board_register_and_say(&name);
            return;
        }
        // A folder of our own making: `new_project_dir` sanitises the name
        // and never returns one that exists.
        let dest = super::project_io::new_project_dir(&root, &name, |p| p.exists());
        match super::clone_project_dialog::copy_tree(&src, &dest) {
            Ok(n) => {
                let dir = dest
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_owned();
                if self.board_register(&dir) == Listed::Added {
                    self.board_notice(
                        format!(
                            "Copied {n} files from {} into {dir} - the saved files; its target/ \
                             and .git were left behind. The original is unchanged.",
                            src.display()
                        ),
                        false,
                    );
                }
            }
            Err(e) => {
                // Half a project would sit in the system folder unlisted, and
                // push the next attempt to `<name>_1`. It did not exist before
                // this copy, so nothing of the user's goes with it.
                let _ = std::fs::remove_dir_all(&dest);
                self.board_notice(format!("Copy into {} failed: {e}", dest.display()), true);
            }
        }
    }

    fn board_remove(&mut self, dir: &str) {
        if self.board_edit_config(|cfg| cfg.remove(dir)) {
            self.board_notice(
                format!("Removed {dir} from the system. Its folder and files are still there."),
                false,
            );
        }
    }

    /// Open a chip of the system in this window - through the same gate as
    /// Open Recent - unless its folder is gone or holds no project: opening
    /// that would keep the previous chip's pins under the missing folder's
    /// name, and the next Save would write them there.
    fn board_open_chip(&mut self, root: &Path, dir: &str, ctx: &egui::Context) {
        let path = root.join(dir);
        if !path.is_dir() {
            self.board_notice(
                format!("{dir} is not there any more - moved or deleted?"),
                true,
            );
            self.board_refresh();
        } else if !is_project(&path) {
            self.board_notice(
                format!("{dir} is not a chip project - it has no Cargo.toml or src/main.rs."),
                true,
            );
        } else {
            self.board.open_request = Some(path);
            ctx.request_repaint();
        }
    }

    /// The Board tab.
    pub(super) fn show_board_tab(&mut self, ui: &mut egui::Ui) {
        // Hidden until now: read the system again - another window, a git
        // checkout or a hand edit may have changed it. A repeated number is
        // not a gap: egui runs a frame twice when a widget asks it to.
        let frame = ui.ctx().cumulative_frame_nr();
        let shown_again = frame != self.board.last_frame && frame != self.board.last_frame + 1;
        if shown_again && self.board.root.is_some() {
            self.board_sync();
        }
        self.board.last_frame = frame;

        self.board_toolbar(ui);
        ui.separator();

        let Some(root) = self.board.root.clone() else {
            empty_note(
                ui,
                "No system open",
                "A system is a folder of chip projects shown together here. New system… makes \
                 one; opening a chip project that sits in a system shows that system.",
            );
            return;
        };
        if self.board.config.chips.is_empty() {
            empty_note(
                ui,
                "No chips yet",
                "New chip creates a project inside this system. Add existing project… copies a \
                 saved project in.",
            );
            return;
        }
        self.board_canvas(ui, &root);
    }

    fn board_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| match self.board.root.clone() {
            Some(root) => {
                let name = root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("system");
                ui.label(
                    egui::RichText::new(format!("{}  {name}", ph::CIRCUITRY))
                        .strong()
                        .color(egui::Color32::LIGHT_BLUE),
                )
                .on_hover_text(root.display().to_string());
                ui.add_space(8.0);
                let busy = self.save_in_progress.is_some();
                if ui
                    .add_enabled(!busy, egui::Button::new(format!("{}  New chip", ph::PLUS)))
                    .on_hover_text("Pick a chip in New Project - it is created inside this system")
                    .clicked()
                {
                    self.board.new_chip_request = true;
                    ui.ctx().request_repaint();
                }
                if ui
                    .button(format!("{}  Add existing project…", ph::FOLDER_PLUS))
                    .on_hover_text("Copy a saved chip project into this system")
                    .clicked()
                {
                    self.board_add_existing();
                }
                if ui
                    .button(ph::ARROW_CLOCKWISE)
                    .on_hover_text("Read the system and its chips again from disk")
                    .clicked()
                {
                    self.board_sync();
                }
                if ui
                    .button(ph::CORNERS_OUT)
                    .on_hover_text("Fit the view to the chips")
                    .clicked()
                {
                    self.board.view_adjusted = false;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(format!("{}  Close system", ph::X))
                        .on_hover_text("Stop showing this system. Nothing on disk changes.")
                        .clicked()
                    {
                        self.board_close_system();
                    }
                });
            }
            None => {
                ui.label(egui::RichText::new("No system open").color(egui::Color32::GRAY));
                ui.add_space(8.0);
                if ui.button(format!("{}  New system…", ph::PLUS)).clicked() {
                    self.board_new_system();
                }
                if ui
                    .button(format!("{}  Open system…", ph::FOLDER_OPEN))
                    .clicked()
                {
                    self.board_open_system_dialog();
                }
            }
        });

        // Each row puts its button FIRST and lets its text wrap after it: a
        // long line would otherwise push the button off the panel.
        if let Some(dir) = self.board_open_chip_dir()
            && !self.board.config.contains(&dir)
        {
            ui.horizontal(|ui| {
                if ui.small_button("Add it").clicked() {
                    self.board_register_and_say(&dir);
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{}  The open project {dir} is in this folder but not in the system.",
                            ph::INFO
                        ))
                        .size(11.5)
                        .color(egui::Color32::from_rgb(220, 180, 90)),
                    )
                    .wrap(),
                );
            });
        }
        let picking = self.confirm_new_project && self.board.new_chip_intent;
        if picking || (self.board.adding_chip && self.project_dir.is_none()) {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{}  New chip: the project is saved inside this system as soon as a chip \
                         is picked.",
                        ph::INFO
                    ))
                    .size(11.5)
                    .color(egui::Color32::from_rgb(150, 200, 240)),
                )
                .wrap(),
            );
        }
        if let Some((text, error)) = self.board.notice.clone() {
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new(ph::X).size(10.0)).frame(false))
                    .on_hover_text("Dismiss")
                    .clicked()
                {
                    self.board.notice = None;
                }
                let (icon, color) = if error {
                    (ph::WARNING, egui::Color32::from_rgb(230, 120, 90))
                } else {
                    (ph::INFO, egui::Color32::from_gray(170))
                };
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("{icon}  {text}"))
                            .size(11.5)
                            .color(color),
                    )
                    .wrap(),
                );
            });
        }
    }

    fn board_canvas(&mut self, ui: &mut egui::Ui, root: &Path) {
        // The open chip shows what it is NOW, saved or not.
        let open = self.board_open_chip_dir();
        let mut views = self.board.views.clone();
        if let (Some(dir), Some(mcu)) = (&open, &self.mcu) {
            let chip = self.selected_label();
            for v in views.iter_mut().filter(|v| v.dir.eq_ignore_ascii_case(dir)) {
                *v = ChipView::from_parts(
                    &v.dir,
                    &chip,
                    Some(mcu.runtime),
                    &mcu.modules,
                    &mcu.groups,
                );
            }
        }
        let sizes = gui::sizes(&views);
        let spots: Vec<(Option<(f32, f32)>, egui::Vec2)> = self
            .board
            .config
            .chips
            .iter()
            .zip(&sizes)
            .map(|(c, s)| (c.pos, *s))
            .collect();
        let positions = layout::frame_positions(&spots);
        let frames: Vec<gui::Frame<'_>> = views
            .iter()
            .zip(positions)
            .map(|(v, pos)| gui::Frame {
                view: v,
                pos,
                active: open
                    .as_deref()
                    .is_some_and(|d| v.dir.eq_ignore_ascii_case(d)),
            })
            .collect();

        let outer = ui.available_rect_before_wrap();
        let avail = ui.available_size_before_wrap();
        let mut scene_rect = self.board.scene_rect;
        // Plain wheel zooms at the pointer, as on the Pins and Structure tabs;
        // the Scene itself would pan with it. Only when this canvas is the
        // topmost thing under the pointer: a window or popup over it scrolls
        // its own content, and the wheel is held back from the Scene (which
        // would pan by it) and handed back afterwards.
        let hover = ui.input(|i| i.pointer.hover_pos());
        let ptr = hover.filter(|_| ui.rect_contains_pointer(outer));
        let held = if ptr.is_none() && hover.is_some_and(|p| outer.contains(p)) {
            ui.input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta))
        } else {
            egui::Vec2::ZERO
        };
        let (scroll_y, ctrl) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.command));
        if let Some(ptr) = ptr
            && scroll_y != 0.0
            && !ctrl
            && scene_rect.is_finite()
            && scene_rect.size() != egui::Vec2::ZERO
        {
            let scale = (outer.size() / scene_rect.size())
                .min_elem()
                .clamp(0.1, 3.0);
            let to_global = egui::emath::TSTransform::from_translation(
                outer.center().to_vec2() - scale * scene_rect.center().to_vec2(),
            ) * egui::emath::TSTransform::from_scaling(scale);
            let z = ((scroll_y * 0.002).exp() * scale).clamp(0.1, 3.0) / scale;
            let p = to_global.inverse() * ptr;
            let new = to_global
                * egui::emath::TSTransform::from_translation(p.to_vec2())
                * egui::emath::TSTransform::from_scaling(z)
                * egui::emath::TSTransform::from_translation(-p.to_vec2());
            let new_rect = new.inverse() * outer;
            if new_rect.is_finite() && new_rect.size() != egui::Vec2::ZERO {
                scene_rect = new_rect;
                self.board.view_adjusted = true;
            }
            ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
        let scene = egui::Scene::new()
            .zoom_range(0.1..=3.0)
            .drag_pan_buttons(egui::DragPanButtons::PRIMARY | egui::DragPanButtons::MIDDLE)
            .show(ui, &mut scene_rect, |ui| gui::draw(ui, &frames));
        if held != egui::Vec2::ZERO {
            ui.input_mut(|i| i.smooth_scroll_delta += held);
        }
        if scene.response.changed() {
            self.board.view_adjusted = true;
        }
        let (events, bounds) = scene.inner;
        self.board.scene_rect = if self.board.view_adjusted || !bounds.is_finite() {
            scene_rect
        } else {
            let want = bounds.expand(24.0);
            egui::Rect::from_center_size(
                want.center(),
                egui::vec2(want.width().max(avail.x), want.height().max(avail.y)),
            )
        };

        for ev in events {
            match ev {
                gui::Event::Moved { dir, pos } => {
                    self.board.config.set_pos(&dir, pos);
                    self.board.moved.insert(dir.to_ascii_lowercase());
                    // Hold the view still: still fitting to the frames, it
                    // would slide away under the frame being dragged.
                    self.board.view_adjusted = true;
                }
                gui::Event::DragEnded => self.board_flush_positions(),
                gui::Event::Open(dir) => self.board_open_chip(root, &dir, ui.ctx()),
                gui::Event::Remove(dir) => self.board_remove(&dir),
            }
        }
    }
}

/// The centred note an empty Board shows.
fn empty_note(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.add_space((ui.available_height() * 0.3).min(160.0));
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(format!("{}  {title}", ph::CIRCUITRY))
                .size(17.0)
                .color(egui::Color32::from_rgb(150, 158, 172)),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(body)
                .size(12.0)
                .color(egui::Color32::from_gray(120)),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("roc_board_tab_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Only a chip project may become a chip, and never the folder holding
    /// the system (the copy would recurse into itself).
    #[test]
    fn only_a_chip_project_can_be_added() {
        let base = scratch("add");
        let root = base.join("sys");
        std::fs::create_dir_all(&root).unwrap();
        model::save(&root, &SystemConfig::default()).unwrap();
        let chip = base.join("blinky");
        std::fs::create_dir_all(chip.join("src")).unwrap();
        std::fs::write(chip.join("src/main.rs"), "fn main() {}").unwrap();
        let notes = base.join("notes");
        std::fs::create_dir_all(&notes).unwrap();

        assert_eq!(not_a_chip(&chip, &root), None);
        assert!(
            not_a_chip(&notes, &root)
                .unwrap()
                .starts_with("Not a chip project")
        );
        assert!(
            not_a_chip(&base, &root)
                .unwrap()
                .contains("holds the system")
        );
        assert!(
            not_a_chip(&root, &root)
                .unwrap()
                .contains("holds the system")
        );
        let other = base.join("other_sys");
        std::fs::create_dir_all(&other).unwrap();
        model::save(&other, &SystemConfig::default()).unwrap();
        assert!(not_a_chip(&other, &root).unwrap().contains("is a system"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
