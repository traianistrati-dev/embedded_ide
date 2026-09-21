//! The Notes section of a Virtual Module: free text, a documentation link and
//! an image - and the list of notes whose module is gone.
//!
//! The data and every rule about it live in
//! [`crate::panels::mcu_module::modules::notes`]; this file only draws it and
//! turns image files into textures.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use eframe::egui;
use egui_phosphor::regular as ph;

use crate::panels::mcu_module::mcu::Mcu;
use crate::panels::mcu_module::modules::notes;
use crate::panels::mcu_module::modules::{ModuleKind, ModuleNotes, NotesKey};

/// Longest side of a decoded image, in pixels.
///
/// Decoded ONCE, downscaled to this, and kept as a texture: a phone photo is
/// 4000 px and tens of MB of RGBA, and egui asserts on a texture past the GPU's
/// max side. The panel never shows more than 480 px of it.
const THUMB_PX: u32 = 512;

/// Decode PNG / JPEG / BMP bytes into a texture-ready image no larger than
/// [`THUMB_PX`] on either side.
///
/// A file that is not an image is an `Err`, never a panic: the bytes come from
/// the user's disk, and the path from a `mcu.config` someone else may have
/// written.
pub fn decode_thumbnail(bytes: &[u8]) -> Result<egui::ColorImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("not a readable image: {e}"))?;
    let img = if img.width() > THUMB_PX || img.height() > THUMB_PX {
        img.thumbnail(THUMB_PX, THUMB_PX)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

/// One load result per FILE, success or failure.
///
/// A failure is cached as well, so a missing or broken image is read once and
/// not on every frame it is on screen.
pub struct FileCache<T> {
    map: HashMap<PathBuf, Result<T, String>>,
}

impl<T> Default for FileCache<T> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl<T: Clone> FileCache<T> {
    /// The result for `path`, running `load` only the first time.
    pub fn get_with(
        &mut self,
        path: &Path,
        load: impl FnOnce(&Path) -> Result<T, String>,
    ) -> Result<T, String> {
        self.map
            .entry(path.to_path_buf())
            .or_insert_with(|| load(path))
            .clone()
    }

    /// Forget everything - on project open and New Project, where the same
    /// relative path names a different file.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Store a result already in hand - a thumbnail just decoded to check an
    /// image before copying it, so it is not decoded a second time to be drawn.
    pub fn put(&mut self, path: &Path, value: Result<T, String>) {
        self.map.insert(path.to_path_buf(), value);
    }
}

/// Textures for the note images, keyed by absolute path - never by module id,
/// which is handed out again (see `Mcu::module_notes`).
pub type NoteImages = FileCache<egui::TextureHandle>;

impl FileCache<egui::TextureHandle> {
    /// The texture for `path`, decoded on first use.
    ///
    /// Read through [`notes::read_image_file`], the same gate a PICKED file goes
    /// through: an image extension, a regular file, at most 20 MB. The path came
    /// out of `mcu.config`, which may have been written on another machine, and
    /// this runs on the UI thread - a bare `fs::read` of a 4 GB file, or of a
    /// link to a device that never ends, would hang the whole window.
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
    ) -> Result<egui::TextureHandle, String> {
        self.get_with(path, |p| {
            let (_, bytes) = notes::read_image_file(p)?;
            let img = decode_thumbnail(&bytes)?;
            Ok(ctx.load_texture(
                format!("module-note:{}", p.display()),
                img,
                egui::TextureOptions::LINEAR,
            ))
        })
    }
}

/// What the image row of a module can show.
#[derive(Debug, PartialEq, Eq)]
pub enum ImageRow {
    /// The project has no folder yet, so there is nowhere to copy an image to.
    NeedsSave,
    /// No image attached.
    Empty,
    /// The stored path is not one this IDE will open - it points outside the
    /// project.
    Bad(String),
    /// A path inside the project. The file may still turn out to be missing.
    Show(PathBuf),
}

/// Classify a module's image for drawing. Pure, so the rules are testable.
pub fn image_row(project_dir: Option<&Path>, rel: &str) -> ImageRow {
    let Some(root) = project_dir else {
        return ImageRow::NeedsSave;
    };
    if rel.trim().is_empty() {
        return ImageRow::Empty;
    }
    match notes::resolve_image(root, rel) {
        Ok(p) => ImageRow::Show(p),
        Err(e) => ImageRow::Bad(e),
    }
}

/// The name an orphan's row shows: what the module will be called when its
/// peripheral is wired again - the same `{short}{instance}` that
/// `reconcile_modules` gives a new module.
pub fn orphan_label(kind: ModuleKind, inst: u8) -> String {
    format!("{}{inst}", kind.short())
}

/// Largest thumbnail in the config column, and on hover.
const THUMB_SHOWN: egui::Vec2 = egui::vec2(220.0, 140.0);
const THUMB_HOVER: egui::Vec2 = egui::vec2(480.0, 360.0);

/// Muted text for hints under the fields.
const HINT: egui::Color32 = egui::Color32::from_gray(140);
/// Amber, the colour the panel already uses for a warning line.
const WARN: egui::Color32 = egui::Color32::from_rgb(220, 180, 90);

/// The Notes block under one module's settings.
///
/// `key` is the module's `(kind, instance)`. The notes are NOT the module's -
/// see `Mcu::module_notes` - which is why they are passed in as the map and a
/// key, and why the block sits outside the module's settings grid.
pub fn notes_section(
    ui: &mut egui::Ui,
    all: &mut BTreeMap<NotesKey, ModuleNotes>,
    key: NotesKey,
    project_dir: Option<&Path>,
    images: &mut NoteImages,
) {
    let has_any = all.get(&key).is_some_and(|n| !n.is_empty());
    egui::CollapsingHeader::new(egui::RichText::new(format!("{} Notes", ph::NOTE)).size(12.0))
        .id_salt(("vmod_notes", key.0, key.1))
        .default_open(has_any)
        .show(ui, |ui| {
            // An entry made just by opening the block is empty, and an empty
            // entry is neither saved nor listed - so this costs nothing.
            let n = all.entry(key).or_default();
            ui.add(
                egui::TextEdit::multiline(&mut n.text)
                    .hint_text("What is wired here: part number, supply, quirks")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(2.0);
            link_row(ui, n);
            ui.add_space(4.0);
            image_rows(ui, n, key, project_dir, images);
        });
}

/// The documentation link, and the button that opens it.
///
/// The button gets the URL AFTER it has been parsed - see
/// [`notes::openable_link`] - never the text as typed.
fn link_row(ui: &mut egui::Ui, n: &mut ModuleNotes) {
    let open = notes::openable_link(&n.link);
    ui.horizontal(|ui| {
        ui.label("Link");
        // The field takes what is left, minus room for the open button.
        let room = if open.is_some() { 28.0 } else { 0.0 };
        ui.add(
            egui::TextEdit::singleline(&mut n.link)
                .hint_text("https://… datasheet")
                .desired_width((ui.available_width() - room).max(60.0)),
        );
        if let Some(url) = &open {
            ui.hyperlink_to(ph::ARROW_SQUARE_OUT, url)
                .on_hover_text(format!("Open in the browser:\n{url}"));
        }
    });
    if open.is_none() && !n.link.trim().is_empty() {
        ui.label(
            egui::RichText::new("Not opened: only a valid http:// or https:// address is.")
                .size(11.0)
                .color(HINT),
        );
    }
}

/// The image: a button to attach one, or the thumbnail and what can be done
/// with it.
fn image_rows(
    ui: &mut egui::Ui,
    n: &mut ModuleNotes,
    key: NotesKey,
    project_dir: Option<&Path>,
    images: &mut NoteImages,
) {
    // The PROJECT is part of the id. egui's memory outlives a project switch,
    // and (kind, instance) alone would carry an error from one project's USART1
    // to the next one's.
    let err_id = egui::Id::new(("vmod_notes_err", project_dir, key.0, key.1));
    match image_row(project_dir, &n.image) {
        ImageRow::NeedsSave => {
            ui.add_enabled(
                false,
                egui::Button::new(format!("{} Choose image…", ph::IMAGE)),
            )
            .on_disabled_hover_text(
                "Save the project first: the image is copied into its folder, \
                 so it travels with the project.",
            );
        }
        ImageRow::Empty => {
            if choose_button(ui, "Choose image…") {
                if let Some(root) = project_dir {
                    pick_and_attach(ui, root, n, images, err_id);
                }
            }
        }
        ImageRow::Bad(e) => {
            ui.label(egui::RichText::new(format!("{} {e}", ph::WARNING)).color(WARN));
            if ui
                .button("Remove image")
                .on_hover_text("Clear the reference. No file is touched.")
                .clicked()
            {
                n.image.clear();
            }
        }
        ImageRow::Show(path) => {
            match images.texture(ui.ctx(), &path) {
                Ok(tex) => {
                    ui.add(egui::Image::from_texture(&tex).max_size(THUMB_SHOWN))
                        .on_hover_ui(|ui| {
                            ui.add(egui::Image::from_texture(&tex).max_size(THUMB_HOVER));
                        });
                }
                Err(e) => {
                    ui.label(
                        egui::RichText::new(format!("{} {e}\n{}", ph::WARNING, n.image))
                            .size(11.0)
                            .color(WARN),
                    );
                }
            }
            ui.horizontal(|ui| {
                if choose_button(ui, "Change…") {
                    if let Some(root) = project_dir {
                        pick_and_attach(ui, root, n, images, err_id);
                    }
                }
                if ui
                    .button(format!("{} Show in Explorer", ph::FOLDER_OPEN))
                    .clicked()
                {
                    if let Err(e) = crate::reveal::open(&path) {
                        ui.data_mut(|d| d.insert_temp(err_id, e));
                    }
                }
                if ui
                    .button(format!("{} Remove", ph::X))
                    .on_hover_text(
                        "Detach the image from these notes. The file stays in \
                         docs/modules/: the IDE never deletes an image.",
                    )
                    .clicked()
                {
                    n.image.clear();
                }
            });
        }
    }
    // The last thing that went wrong, until the next attempt succeeds.
    if let Some(e) = ui.data(|d| d.get_temp::<String>(err_id)) {
        ui.label(
            egui::RichText::new(format!("{} {e}", ph::WARNING))
                .size(11.0)
                .color(WARN),
        );
    }
}

fn choose_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.button(format!("{} {label}", ph::IMAGE))
        .on_hover_text("PNG, JPEG or BMP, up to 20 MB. It is COPIED into docs/modules/.")
        .clicked()
}

/// Ask for an image, check it, copy it in, and point the notes at the copy.
///
/// Decoded BEFORE it is copied, so a file that is not an image is refused
/// without leaving a copy behind. The decoded thumbnail goes straight into the
/// cache, so it is not decoded a second time to be drawn.
fn pick_and_attach(
    ui: &egui::Ui,
    root: &Path,
    n: &mut ModuleNotes,
    images: &mut NoteImages,
    err_id: egui::Id,
) {
    let Some(src) = rfd::FileDialog::new()
        .add_filter("Image", &notes::IMAGE_EXTENSIONS)
        .pick_file()
    else {
        return;
    };
    let attach = || -> Result<(String, egui::ColorImage), String> {
        let (name, bytes) = notes::read_image_file(&src)?;
        let img = decode_thumbnail(&bytes)?;
        let rel = notes::store_image(root, &name, &bytes)?;
        Ok((rel, img))
    };
    match attach() {
        Ok((rel, img)) => {
            if let Ok(path) = notes::resolve_image(root, &rel) {
                let tex = ui.ctx().load_texture(
                    format!("module-note:{}", path.display()),
                    img,
                    egui::TextureOptions::LINEAR,
                );
                images.put(&path, Ok(tex));
            }
            n.image = rel;
            ui.ctx().data_mut(|d| d.remove::<String>(err_id));
        }
        Err(e) => {
            ui.ctx().data_mut(|d| d.insert_temp(err_id, e));
        }
    }
}

/// Which orphan row, in which project, has its Delete armed - one at a time.
type Armed = (Option<PathBuf>, NotesKey);

/// Whether the armed Delete still applies to THIS list.
///
/// egui's memory outlives both a project switch and the orphan itself, so a
/// flag keyed by (kind, instance) alone came back armed on the next orphan with
/// that key - another project's, or the same one after a re-wire and unwire -
/// and the first click on the trash icon's spot deleted without asking. Only an
/// arming made in this project, for a key that is an orphan right now, counts.
fn armed_here(
    armed: Option<&Armed>,
    project_dir: Option<&Path>,
    orphans: &[NotesKey],
) -> Option<NotesKey> {
    let (proj, key) = armed?;
    (proj.as_deref() == project_dir && orphans.contains(key)).then_some(*key)
}

/// "Notes without a module": notes whose module is not on the canvas right now.
///
/// They are kept on purpose - clearing a bus to move it must not lose what was
/// written about the device on it. `delete` is filled for the caller to apply,
/// because the list is drawn while `mcu` is borrowed.
pub fn orphan_rows(
    ui: &mut egui::Ui,
    mcu: &Mcu,
    project_dir: Option<&Path>,
    delete: &mut Option<NotesKey>,
) {
    let orphans = mcu.orphan_notes();
    let armed_id = egui::Id::new("vmod_orphan_delete_armed");
    let armed = armed_here(
        ui.data(|d| d.get_temp::<Armed>(armed_id)).as_ref(),
        project_dir,
        &orphans,
    );
    if armed.is_none() {
        ui.data_mut(|d| d.remove::<Armed>(armed_id));
    }
    if orphans.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.separator();
    ui.label(
        egui::RichText::new("Notes without a module")
            .size(11.0)
            .color(HINT),
    )
    .on_hover_text(
        "Notes whose module is not on the canvas. A peripheral's come back by \
         themselves when it is wired again; a removed Custom module is not \
         re-created, so its notes stay here until you undo the Remove or delete \
         them.",
    );
    for key in orphans {
        let Some(n) = mcu.module_notes.get(&key) else {
            continue;
        };
        ui.horizontal(|ui| {
            let row = ui.label(
                egui::RichText::new(format!("{} {}", ph::NOTE, orphan_label(key.0, key.1)))
                    .size(11.5),
            );
            let mut hover = n.preview().unwrap_or_default();
            if key.0.is_custom() {
                hover.push_str(
                    "\n\nA removed Custom module is not re-created: undo the Remove \
                     to get it back with these notes.",
                );
            }
            row.on_hover_text(hover);
            if armed == Some(key) {
                if ui
                    .small_button("Delete")
                    .on_hover_text("Forget these notes. An attached image file is kept.")
                    .clicked()
                {
                    *delete = Some(key);
                    ui.data_mut(|d| d.remove::<Armed>(armed_id));
                }
                if ui.small_button("Keep").clicked() {
                    ui.data_mut(|d| d.remove::<Armed>(armed_id));
                }
            } else if ui
                .small_button(ph::TRASH)
                .on_hover_text("Delete these notes")
                .clicked()
            {
                let arm: Armed = (project_dir.map(Path::to_path_buf), key);
                ui.data_mut(|d| d.insert_temp(armed_id, arm));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_and_a_jpeg_both_decode_within_the_thumbnail_size() {
        let png = include_bytes!("../../assets/icon.png");
        let img = decode_thumbnail(png).unwrap();
        assert!(img.size[0] <= THUMB_PX as usize && img.size[1] <= THUMB_PX as usize);

        // A JPEG larger than the cap, made here: it cannot even be ENCODED
        // without the `jpeg` feature, so this also proves Cargo.toml has it.
        let (w, h) = (1200u32, 800u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        let img = decode_thumbnail(&jpeg).unwrap();
        assert_eq!(img.size, [512, 341], "downscaled, aspect kept");
    }

    #[test]
    fn garbage_bytes_are_an_error_not_a_panic() {
        assert!(decode_thumbnail(b"not an image at all").is_err());
        assert!(decode_thumbnail(&[]).is_err());
    }

    /// A missing file must not be retried on every frame.
    #[test]
    fn the_cache_reads_a_missing_file_once() {
        let mut cache: FileCache<u32> = FileCache::default();
        let path = Path::new("missing/gps.jpg");
        let mut calls = 0;
        for _ in 0..3 {
            let got = cache.get_with(path, |_| {
                calls += 1;
                Err("image not found".into())
            });
            assert!(got.is_err());
        }
        assert_eq!(calls, 1);

        cache.put(path, Ok(7));
        assert_eq!(cache.get_with(path, |_| Ok(0)), Ok(7), "put replaces it");
        cache.clear();
        let _ = cache.get_with(path, |_| {
            calls += 1;
            Ok(1)
        });
        assert_eq!(calls, 2, "clear makes it read again");
    }

    #[test]
    fn the_image_button_waits_for_a_saved_project() {
        assert_eq!(image_row(None, "docs/modules/x.png"), ImageRow::NeedsSave);
        assert_eq!(image_row(None, ""), ImageRow::NeedsSave);
    }

    #[test]
    fn a_bad_path_is_shown_not_opened() {
        let root = Path::new("proj");
        assert!(matches!(
            image_row(Some(root), "../x.png"),
            ImageRow::Bad(_)
        ));
        assert_eq!(image_row(Some(root), "  "), ImageRow::Empty);
        assert_eq!(
            image_row(Some(root), "docs/modules/x.png"),
            ImageRow::Show(root.join("docs/modules/x.png"))
        );
    }

    #[test]
    fn an_orphan_is_named_like_the_module_it_would_become() {
        assert_eq!(orphan_label(ModuleKind::GenericInterfaceUsart, 2), "USART2");
    }

    /// An armed Delete counts only in the project it was armed in, and only
    /// while that key is still an orphan - otherwise the first click on the
    /// trash icon's spot deletes without asking.
    #[test]
    fn a_stale_armed_delete_does_not_carry_over() {
        let a = Path::new("projA");
        let b = Path::new("projB");
        let spi2 = (ModuleKind::GenericInterfaceSpi, 2);
        let armed: Armed = (Some(a.to_path_buf()), spi2);

        assert_eq!(armed_here(Some(&armed), Some(a), &[spi2]), Some(spi2));
        assert_eq!(
            armed_here(Some(&armed), Some(b), &[spi2]),
            None,
            "another project"
        );
        assert_eq!(
            armed_here(Some(&armed), Some(a), &[]),
            None,
            "re-wired meanwhile"
        );
        assert_eq!(armed_here(None, Some(a), &[spi2]), None);
    }

    /// The image gate reads the same way a picked file does, so a path from a
    /// foreign mcu.config cannot make the UI thread read something huge or
    /// something that is not an image.
    #[test]
    fn a_stored_path_is_read_through_the_picked_file_gate() {
        let dir = std::env::temp_dir().join(format!("eide_notes_gate_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, "x").unwrap();
        let mut cache: FileCache<u32> = FileCache::default();
        let got = cache.get_with(&txt, |p| notes::read_image_file(p).map(|_| 0));
        assert!(got.is_err(), "not an image extension");
        // Named like an image, so it is the file check that refuses it.
        let folder = dir.join("folder.png");
        std::fs::create_dir_all(&folder).unwrap();
        let got = cache.get_with(&folder, |p| notes::read_image_file(p).map(|_| 0));
        assert!(got.unwrap_err().contains("not a file"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
