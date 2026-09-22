//! `system.config` — the file that makes a folder of chip projects a system.
//!
//! It lives at the system root, next to the chip folders, and lists them:
//!
//! ```text
//! # RustOnChip system - the chip projects in this folder and where each sits on the Board tab.
//! @chips
//! stm32_main=40,60
//! esp32_radio=420,60
//! ```
//!
//! One chip per line: the FOLDER name (the chip's identity in the system), then
//! the top-left of its frame on the canvas. A line without a position is a chip
//! not placed yet; the canvas finds it a spot.
//!
//! `@view` holds `detailed` when the Board shows every pin of a link (absent =
//! abstract). `@links` holds one link per line, as RON:
//!
//! ```text
//! @links
//! (a:(chip:"stm32_main",kind:GenericInterfaceUsart,instance:1),b:(chip:"esp32_radio",kind:GenericInterfaceUsart,instance:0))
//! ```
//!
//! ONE LINE PER LINK, like `@modulenotes` in `mcu.config`: a line that does not
//! parse loses only itself. An end names a module by (kind, instance) - the one
//! identity a Virtual Module keeps across `reconcile_modules`, which re-mints
//! module ids.
//!
//! Sections this build does not know are kept verbatim, so a file written by a
//! newer build survives being saved by this one.

use std::path::{Path, PathBuf};

use crate::panels::mcu_module::modules::ModuleKind;

/// File name at the system root.
pub const FILE_NAME: &str = "system.config";

const CHIPS_HEADER: &str = "@chips";
const VIEW_HEADER: &str = "@view";
const LINKS_HEADER: &str = "@links";

/// One end of a link: a Virtual Module on a chip of the system.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LinkEnd {
    /// The chip's folder.
    pub chip: String,
    pub kind: ModuleKind,
    pub instance: u8,
    /// A custom module's name when the link was made. Its instance number is
    /// handed out again after a remove, so the number alone could quietly
    /// re-bind the link to a different module; with the name, a module that
    /// is not the one linked shows the link broken instead. Empty for a
    /// peripheral, whose (kind, instance) is the peripheral itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl LinkEnd {
    /// The same module, whatever the case of the folder name.
    pub fn same_as(&self, other: &LinkEnd) -> bool {
        self.chip.eq_ignore_ascii_case(&other.chip)
            && self.kind == other.kind
            && self.instance == other.instance
    }
}

/// Two modules on two chips, wired together.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Link {
    pub a: LinkEnd,
    pub b: LinkEnd,
}

impl Link {
    /// The same pair of modules, in either order.
    pub fn same_as(&self, other: &Link) -> bool {
        (self.a.same_as(&other.a) && self.b.same_as(&other.b))
            || (self.a.same_as(&other.b) && self.b.same_as(&other.a))
    }

    /// Whether one of its ends is on chip `dir`.
    pub fn touches(&self, dir: &str) -> bool {
        self.a.chip.eq_ignore_ascii_case(dir) || self.b.chip.eq_ignore_ascii_case(dir)
    }
}

/// How much of each link the Board draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// One line per link, between the two modules.
    #[default]
    Abstract,
    /// One wire per pin, with the pads named at the frame edges.
    Detailed,
}

/// One chip project in the system.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemChip {
    /// The chip's folder, directly under the system root.
    pub dir: String,
    /// Top-left of its frame on the Board canvas; `None` = not placed yet.
    pub pos: Option<(f32, f32)>,
}

/// The whole `system.config`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemConfig {
    pub chips: Vec<SystemChip>,
    pub links: Vec<Link>,
    pub view: View,
    /// Every section this build does not read, header lines included, exactly
    /// as it was in the file.
    pub unknown: String,
}

impl SystemConfig {
    /// Read the file's text. Never fails: a line it cannot read is skipped, and
    /// a chip whose position does not parse keeps its place in the list without
    /// one - losing a chip over a typo in two numbers would be the worse error.
    pub fn parse(text: &str) -> Self {
        let mut cfg = Self::default();
        let mut section: Option<&str> = None;
        // A file saved by Notepad or PowerShell 5.1 starts with a BOM, which
        // `trim` keeps: `@chips` on the first line would not be a header, and
        // every chip under it would be read as preamble - and dropped on the
        // next save.
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        for raw in text.lines() {
            let line = raw.trim();
            if line.starts_with('@') {
                section = Some(line);
                if ![CHIPS_HEADER, VIEW_HEADER, LINKS_HEADER].contains(&line) {
                    push_line(&mut cfg.unknown, raw);
                }
                continue;
            }
            match section {
                Some(VIEW_HEADER) => {
                    if line.eq_ignore_ascii_case("detailed") {
                        cfg.view = View::Detailed;
                    }
                }
                Some(LINKS_HEADER) => {
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    // A line that does not parse is dropped, alone.
                    if let Ok(link) = ron::from_str::<Link>(line)
                        && !cfg.links.iter().any(|l| l.same_as(&link))
                    {
                        cfg.links.push(link);
                    }
                }
                Some(CHIPS_HEADER) => {
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let (dir, pos) = match line.split_once('=') {
                        Some((dir, pos)) => (dir.trim(), parse_pos(pos)),
                        None => (line, None),
                    };
                    if !dir.is_empty() && !cfg.contains(dir) {
                        cfg.chips.push(SystemChip {
                            dir: dir.to_owned(),
                            pos,
                        });
                    }
                }
                Some(_) => push_line(&mut cfg.unknown, raw),
                // Before the first section: the header comment, rewritten on
                // every save.
                None => {}
            }
        }
        cfg
    }

    /// The file's text. Positions are written as whole points: a drag never
    /// needs more, and fractions would make every move a noisy diff.
    pub fn serialize(&self) -> String {
        let mut out = format!(
            "# {} system - the chip projects in this folder and where each sits on the Board tab.\n",
            crate::names::APP_DISPLAY_NAME
        );
        out.push_str(CHIPS_HEADER);
        out.push('\n');
        for c in &self.chips {
            match c.pos {
                Some((x, y)) => out.push_str(&format!("{}={:.0},{:.0}\n", c.dir, x, y)),
                None => {
                    out.push_str(&c.dir);
                    out.push('\n');
                }
            }
        }
        if self.view == View::Detailed {
            out.push_str(VIEW_HEADER);
            out.push_str("\ndetailed\n");
        }
        if !self.links.is_empty() {
            out.push_str(LINKS_HEADER);
            out.push('\n');
            for l in &self.links {
                // `ron` escapes every string, so the line is always one line and
                // always starts with `(`: never a `@` section or a `#` comment.
                if let Ok(text) = ron::to_string(l) {
                    out.push_str(&text);
                    out.push('\n');
                }
            }
        }
        if !self.unknown.is_empty() {
            out.push_str(&self.unknown);
        }
        out
    }

    /// Whether `dir` is already a chip of this system. Case-insensitive: two
    /// entries differing only in case are one folder on Windows.
    pub fn contains(&self, dir: &str) -> bool {
        self.chips.iter().any(|c| c.dir.eq_ignore_ascii_case(dir))
    }

    /// Add a chip. `false` when it is already there, or when its name is one
    /// the file cannot hold (see [`listable`]) - nothing changes then.
    pub fn add(&mut self, dir: &str, pos: Option<(f32, f32)>) -> bool {
        if !listable(dir) || self.contains(dir) {
            return false;
        }
        self.chips.push(SystemChip {
            dir: dir.trim().to_owned(),
            pos,
        });
        true
    }

    /// Take a chip out of the system, with its links. Its folder is not
    /// touched.
    pub fn remove(&mut self, dir: &str) -> bool {
        let before = self.chips.len();
        self.chips.retain(|c| !c.dir.eq_ignore_ascii_case(dir));
        self.links.retain(|l| !l.touches(dir));
        self.chips.len() != before
    }

    /// Add a link. `false` when that pair is already linked, or when both ends
    /// are on one chip - a link is between chips.
    pub fn add_link(&mut self, link: Link) -> bool {
        if link.a.chip.eq_ignore_ascii_case(&link.b.chip)
            || self.links.iter().any(|l| l.same_as(&link))
        {
            return false;
        }
        self.links.push(link);
        true
    }

    /// Remove a link (in either orientation).
    pub fn remove_link(&mut self, link: &Link) -> bool {
        let before = self.links.len();
        self.links.retain(|l| !l.same_as(link));
        self.links.len() != before
    }

    /// Move a chip's frame. `false` for a chip that is not in the system.
    pub fn set_pos(&mut self, dir: &str, pos: (f32, f32)) -> bool {
        match self
            .chips
            .iter_mut()
            .find(|c| c.dir.eq_ignore_ascii_case(dir))
        {
            Some(c) => {
                c.pos = Some(pos);
                true
            }
            None => false,
        }
    }
}

/// Put this window's frame positions onto `disk`, a config just read from the
/// file, and say whether that changed anything.
///
/// The file is shared - another window of the same system, or a hand edit,
/// may have added or removed a chip since this window read it - so a write
/// never replaces it with this window's copy. It re-reads, then carries over
/// only what this window decided: the frames the user dragged here (`moved`,
/// folder names lower-cased), and a spot for every frame the file has none
/// for, so a frame never jumps once it has been shown. A chip removed on disk
/// stays removed.
pub fn carry_positions(
    disk: &mut SystemConfig,
    mine: &SystemConfig,
    moved: &std::collections::BTreeSet<String>,
) -> bool {
    let mut changed = false;
    for c in &mine.chips {
        let Some(pos) = c.pos else {
            continue;
        };
        let unplaced_on_disk = disk
            .chips
            .iter()
            .any(|d| d.dir.eq_ignore_ascii_case(&c.dir) && d.pos.is_none());
        if (moved.contains(&c.dir.to_ascii_lowercase()) || unplaced_on_disk)
            && disk.set_pos(&c.dir, pos)
        {
            changed = true;
        }
    }
    changed
}

/// Whether a folder name can be a line of `@chips` and read back as itself.
///
/// The file has no escaping, so three things cannot be in a name: `=` (it
/// would split the name from the position), and a leading `#` or `@` (the line
/// would read as a comment or a section). Folders this app creates never
/// have them; a folder named by hand can.
pub fn listable(dir: &str) -> bool {
    let d = dir.trim();
    !d.is_empty() && !d.contains('=') && !d.starts_with('#') && !d.starts_with('@')
}

/// `path` without the `\\?\` prefix `canonicalize` puts on Windows paths - the
/// form a folder picker returns. A project opened from the command line is
/// canonicalized; one picked in a dialog is not; both must name one folder.
/// A `\\?\UNC\` share path is left alone: stripping it would not give a path
/// that works.
pub fn plain(path: &Path) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Whether two paths name the same folder: [`plain`] forms, compared by
/// component, ignoring case on Windows.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    let (a, b) = (plain(a), plain(b));
    if cfg!(windows) {
        let key = |p: &Path| -> Vec<String> {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
                .collect()
        };
        key(&a) == key(&b)
    } else {
        a == b
    }
}

fn push_line(buf: &mut String, line: &str) {
    buf.push_str(line);
    buf.push('\n');
}

fn parse_pos(text: &str) -> Option<(f32, f32)> {
    let (x, y) = text.split_once(',')?;
    let x = x.trim().parse::<f32>().ok()?;
    let y = y.trim().parse::<f32>().ok()?;
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// Whether `dir` is a system root.
pub fn is_system_root(dir: &Path) -> bool {
    dir.join(FILE_NAME).is_file()
}

/// The system a chip project belongs to: its parent folder, when that holds a
/// `system.config`. A chip always sits DIRECTLY under the root, so no further
/// walk up is needed - and a deeper one would claim a project that only
/// happens to live somewhere below a system.
pub fn system_root_of(project_dir: &Path) -> Option<PathBuf> {
    let dir = plain(project_dir);
    let parent = dir.parent()?;
    is_system_root(parent).then(|| parent.to_path_buf())
}

/// Read a system's config. `Err` names the file, for the Board tab to show.
pub fn load(root: &Path) -> Result<SystemConfig, String> {
    let path = root.join(FILE_NAME);
    std::fs::read_to_string(&path)
        .map(|t| SystemConfig::parse(&t))
        .map_err(|e| format!("couldn't read {}: {e}", path.display()))
}

/// Write a system's config - to a temporary file first, then renamed over the
/// old one. A plain write truncates before it writes, and another window
/// reading in between would take the empty file for the system and write
/// it back without any chips.
pub fn save(root: &Path, cfg: &SystemConfig) -> Result<(), String> {
    let path = root.join(FILE_NAME);
    let tmp = root.join(format!("{FILE_NAME}.tmp"));
    std::fs::write(&tmp, cfg.serialize())
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("couldn't write {}: {e}", path.display())
        })
}

/// A chip folder was renamed: follow it, in the chip list and in every link
/// end. `false` when `old` is not a chip of the system.
pub fn rename_chip(cfg: &mut SystemConfig, old: &str, new: &str) -> bool {
    let Some(c) = cfg
        .chips
        .iter_mut()
        .find(|c| c.dir.eq_ignore_ascii_case(old))
    else {
        return false;
    };
    c.dir = new.to_owned();
    for l in &mut cfg.links {
        for end in [&mut l.a, &mut l.b] {
            if end.chip.eq_ignore_ascii_case(old) {
                end.chip = new.to_owned();
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_chips() -> SystemConfig {
        let mut c = SystemConfig::default();
        c.add("stm32_main", Some((40.0, 60.0)));
        c.add("esp32_radio", None);
        c
    }

    #[test]
    fn a_system_round_trips() {
        let cfg = two_chips();
        let text = cfg.serialize();
        assert!(
            text.contains("\n@chips\nstm32_main=40,60\nesp32_radio\n"),
            "{text}"
        );
        assert_eq!(SystemConfig::parse(&text), cfg);
        // Saving what was read changes nothing.
        assert_eq!(SystemConfig::parse(&text).serialize(), text);
    }

    /// A newer build's sections survive a save by this one.
    #[test]
    fn unknown_sections_are_kept_verbatim() {
        let text = "# header\n@chips\na=1,2\n@parts\nfpga = iCE40UP5K\n  indented\n";
        let cfg = SystemConfig::parse(text);
        assert_eq!(cfg.chips.len(), 1);
        let out = cfg.serialize();
        assert!(
            out.ends_with("@parts\nfpga = iCE40UP5K\n  indented\n"),
            "{out}"
        );
        assert_eq!(SystemConfig::parse(&out), cfg);
    }

    fn end(chip: &str, kind: ModuleKind, instance: u8) -> LinkEnd {
        LinkEnd {
            chip: chip.to_owned(),
            kind,
            instance,
            name: String::new(),
        }
    }

    fn uart_link() -> Link {
        Link {
            a: end("stm32_main", ModuleKind::GenericInterfaceUsart, 1),
            b: end("esp32_radio", ModuleKind::GenericInterfaceUsart, 0),
        }
    }

    /// Links and the view round-trip; each link is one line starting with `(`.
    #[test]
    fn links_and_the_view_round_trip() {
        let mut cfg = two_chips();
        assert!(cfg.add_link(uart_link()));
        assert!(cfg.add_link(Link {
            a: end("stm32_main", ModuleKind::Custom, 2),
            b: end("esp32_radio", ModuleKind::Custom, 0),
        }));
        cfg.view = View::Detailed;
        let text = cfg.serialize();
        assert!(text.contains("\n@view\ndetailed\n@links\n("), "{text}");
        let link_lines = text.lines().skip_while(|l| *l != "@links").skip(1);
        assert!(link_lines.clone().all(|l| l.starts_with('(')));
        assert_eq!(link_lines.count(), 2);
        assert_eq!(SystemConfig::parse(&text), cfg);
        // The default view writes nothing.
        cfg.view = View::Abstract;
        assert!(!cfg.serialize().contains("@view"));
    }

    /// A pair is one link whichever end comes first; a link inside one chip
    /// is refused; a line that does not parse costs only itself.
    #[test]
    fn a_link_is_a_pair_of_modules_on_two_chips() {
        let mut cfg = two_chips();
        assert!(cfg.add_link(uart_link()));
        let flipped = Link {
            a: uart_link().b,
            b: uart_link().a,
        };
        assert!(!cfg.add_link(flipped.clone()), "same pair, other order");
        assert!(!cfg.add_link(Link {
            a: end("STM32_MAIN", ModuleKind::GenericInterfaceSpi, 1),
            b: end("stm32_main", ModuleKind::GenericInterfaceSpi, 2),
        }));
        let text = format!("{}garbage\n", cfg.serialize());
        assert_eq!(SystemConfig::parse(&text).links, cfg.links);
        assert!(cfg.remove_link(&flipped));
        assert!(cfg.links.is_empty());
    }

    /// A renamed chip folder keeps its place and its links.
    #[test]
    fn a_renamed_chip_keeps_its_links() {
        let mut cfg = two_chips();
        cfg.add_link(uart_link());
        assert!(rename_chip(&mut cfg, "ESP32_RADIO", "radio_c3"));
        assert_eq!(cfg.chips[1].dir, "radio_c3");
        assert_eq!(cfg.chips[1].pos, None);
        assert_eq!(cfg.links[0].b.chip, "radio_c3");
        assert_eq!(cfg.links[0].a.chip, "stm32_main");
        assert!(!rename_chip(&mut cfg, "nope", "x"));
    }

    /// A custom module's name rides along; a peripheral's end writes none.
    #[test]
    fn only_a_custom_end_carries_a_name() {
        let mut cfg = two_chips();
        let mut custom = end("esp32_radio", ModuleKind::Custom, 2);
        custom.name = "irq_out".into();
        cfg.add_link(Link {
            a: end("stm32_main", ModuleKind::Custom, 0),
            b: custom,
        });
        let text = cfg.serialize();
        assert_eq!(text.matches("name:").count(), 1, "{text}");
        assert_eq!(SystemConfig::parse(&text), cfg);
    }

    /// The write goes through a temporary file and leaves none behind.
    #[test]
    fn a_save_leaves_no_temporary_file() {
        let base = std::env::temp_dir().join(format!("roc_board_save_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        save(&base, &two_chips()).unwrap();
        save(&base, &two_chips()).unwrap();
        assert!(!base.join(format!("{FILE_NAME}.tmp")).exists());
        assert_eq!(load(&base).unwrap(), two_chips());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Taking a chip out takes its links too - nothing is left pointing at it.
    #[test]
    fn removing_a_chip_removes_its_links() {
        let mut cfg = two_chips();
        cfg.add("pico", None);
        cfg.add_link(uart_link());
        cfg.add_link(Link {
            a: end("pico", ModuleKind::GenericInterfaceI2c, 0),
            b: end("stm32_main", ModuleKind::GenericInterfaceI2c, 1),
        });
        assert!(cfg.remove("Esp32_Radio"));
        assert_eq!(cfg.links.len(), 1);
        assert!(cfg.links[0].touches("pico"));
    }

    /// A position typo costs the position, never the chip.
    #[test]
    fn a_bad_position_keeps_the_chip() {
        let cfg = SystemConfig::parse("@chips\na=12,x\nb=NaN,3\nc=7\nd\n");
        let dirs: Vec<&str> = cfg.chips.iter().map(|c| c.dir.as_str()).collect();
        assert_eq!(dirs, ["a", "b", "c", "d"]);
        assert!(cfg.chips.iter().all(|c| c.pos.is_none()));
    }

    #[test]
    fn duplicates_blank_lines_and_comments_are_dropped() {
        let cfg = SystemConfig::parse("@chips\n\n# a note\na=1,1\nA=5,5\n  \nb\n");
        let dirs: Vec<&str> = cfg.chips.iter().map(|c| c.dir.as_str()).collect();
        assert_eq!(dirs, ["a", "b"]);
        assert_eq!(cfg.chips[0].pos, Some((1.0, 1.0)));
    }

    #[test]
    fn add_remove_and_move_ignore_case() {
        let mut cfg = two_chips();
        assert!(!cfg.add("STM32_MAIN", None), "already there");
        assert!(cfg.set_pos("ESP32_RADIO", (300.4, 10.6)));
        assert_eq!(cfg.chips[1].pos, Some((300.4, 10.6)));
        assert!(cfg.serialize().contains("esp32_radio=300,11\n"));
        assert!(!cfg.set_pos("nope", (0.0, 0.0)));
        assert!(cfg.remove("Stm32_Main"));
        assert!(!cfg.remove("stm32_main"));
        assert_eq!(cfg.chips.len(), 1);
        assert!(!cfg.add("  ", None));
    }

    /// Only the PARENT of a project makes it a chip of a system.
    #[test]
    fn a_chip_belongs_to_the_system_directly_above_it() {
        let base = std::env::temp_dir().join(format!("roc_board_model_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("sys");
        let chip = root.join("stm32_main");
        let deeper = chip.join("lib_a");
        std::fs::create_dir_all(&deeper).unwrap();
        assert_eq!(system_root_of(&chip), None);
        save(&root, &two_chips()).unwrap();
        assert!(is_system_root(&root));
        assert_eq!(system_root_of(&chip), Some(root.clone()));
        assert_eq!(system_root_of(&deeper), None);
        assert_eq!(load(&root).unwrap(), two_chips());
        let _ = std::fs::remove_dir_all(&base);
        assert!(load(&root).is_err());
    }

    /// A write carries this window's moves onto the file as it is NOW: a chip
    /// another window added survives, one it removed stays gone, and only the
    /// frames moved here get this window's position.
    #[test]
    fn positions_are_carried_onto_the_file_not_over_it() {
        let mut mine = SystemConfig::default();
        mine.add("a", Some((500.0, 40.0)));
        mine.add("b", Some((900.0, 40.0)));
        mine.add("gone", Some((1.0, 1.0)));
        let mut disk = SystemConfig::default();
        disk.add("a", Some((40.0, 40.0)));
        disk.add("b", None);
        disk.add("new_from_other_window", Some((7.0, 7.0)));
        let moved: std::collections::BTreeSet<String> = ["a".to_owned(), "gone".to_owned()].into();

        assert!(carry_positions(&mut disk, &mine, &moved));
        let got: Vec<(&str, Option<(f32, f32)>)> =
            disk.chips.iter().map(|c| (c.dir.as_str(), c.pos)).collect();
        assert_eq!(
            got,
            [
                ("a", Some((500.0, 40.0))),
                ("b", Some((900.0, 40.0))),
                ("new_from_other_window", Some((7.0, 7.0))),
            ]
        );
        // Nothing of this window's left to carry: no change, no write.
        assert!(!carry_positions(&mut disk, &mine, &Default::default()));
    }

    /// A BOM (Notepad, PowerShell 5.1) must not turn `@chips` into preamble.
    #[test]
    fn a_leading_bom_is_not_part_of_the_first_header() {
        let cfg = SystemConfig::parse("\u{FEFF}@chips\nstm32_main=1,2\n");
        assert_eq!(cfg.chips.len(), 1);
        assert_eq!(cfg.chips[0].dir, "stm32_main");
    }

    /// A name the file cannot write back is refused, not listed and lost.
    #[test]
    fn names_the_file_cannot_hold_are_refused() {
        let mut cfg = SystemConfig::default();
        for bad in ["a=b", "#notes", "@radio", "  ", ""] {
            assert!(!listable(bad), "{bad:?}");
            assert!(!cfg.add(bad, None), "{bad:?}");
        }
        for good in ["stm32_main", "ESP32-C3 radio", "a#b", "x@y", "pico.2"] {
            assert!(listable(good), "{good:?}");
            assert!(cfg.add(good, Some((1.0, 2.0))), "{good:?}");
        }
        assert_eq!(SystemConfig::parse(&cfg.serialize()), cfg);
    }

    /// The canonicalized form of a path and the picker's form are one folder.
    #[test]
    fn a_verbatim_path_is_the_same_folder() {
        assert_eq!(
            plain(Path::new(r"\\?\C:\sys\chip")),
            PathBuf::from(r"C:\sys\chip")
        );
        assert_eq!(
            plain(Path::new(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\?\UNC\server\share")
        );
        assert!(same_dir(
            Path::new(r"\\?\C:\sys\chip"),
            Path::new(r"C:\sys\chip")
        ));
        assert!(!same_dir(
            Path::new(r"C:\sys\chip"),
            Path::new(r"C:\sys\chip2")
        ));
        if cfg!(windows) {
            assert!(same_dir(
                Path::new(r"C:\Sys\Chip"),
                Path::new(r"c:\sys\chip\")
            ));
        }
    }
}
