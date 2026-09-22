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
//! Sections this build does not know are kept verbatim, so a file written by a
//! newer build (with links, say) survives being saved by this one.

use std::path::{Path, PathBuf};

/// File name at the system root.
pub const FILE_NAME: &str = "system.config";

const CHIPS_HEADER: &str = "@chips";

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
                if line != CHIPS_HEADER {
                    push_line(&mut cfg.unknown, raw);
                }
                continue;
            }
            match section {
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

    /// Take a chip out of the system. Its folder is not touched.
    pub fn remove(&mut self, dir: &str) -> bool {
        let before = self.chips.len();
        self.chips.retain(|c| !c.dir.eq_ignore_ascii_case(dir));
        self.chips.len() != before
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

/// Write a system's config.
pub fn save(root: &Path, cfg: &SystemConfig) -> Result<(), String> {
    let path = root.join(FILE_NAME);
    std::fs::write(&path, cfg.serialize())
        .map_err(|e| format!("couldn't write {}: {e}", path.display()))
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
        let text =
            "# header\n@chips\na=1,2\n@links\nstm32_main.USART1 = esp32_radio.UART0\n  indented\n";
        let cfg = SystemConfig::parse(text);
        assert_eq!(cfg.chips.len(), 1);
        let out = cfg.serialize();
        assert!(
            out.ends_with("@links\nstm32_main.USART1 = esp32_radio.UART0\n  indented\n"),
            "{out}"
        );
        assert_eq!(SystemConfig::parse(&out), cfg);
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
