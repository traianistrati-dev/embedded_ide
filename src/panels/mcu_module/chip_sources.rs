//! Where importable STM32 chips come from, and what each source can deliver.
//!
//! The IDE already converts a vendor chip XML into a definition
//! ([`convert_xml`](super::stm32_pin_data::convert_xml)) and a CubeMX clock tree
//! into a graph ([`cubemx`](super::clock::graph::cubemx)). What was missing is
//! the step before both: FINDING the chip. Until now that meant a file dialog
//! and knowing that `STM32F103C8Tx` lives in a file called
//! `STM32F103C(8-B)Tx.xml`.
//!
//! This module is the searchable catalogue that removes that step. It imports
//! nothing and never touches the registry — it answers only "which chips can
//! this machine offer me, and which file holds each one".
//!
//! # The two sources are not interchangeable
//!
//! | | chips | pin data | clock tree |
//! |---|---|---|---|
//! | a CubeMX installation | ~2800 | yes | **yes** |
//! | an `STM32_open_pin_data` checkout | ~2200 | yes | **no** |
//!
//! ST publishes the per-chip files openly, but NOT `db/plugins/clock` — those
//! ship only inside CubeMX. So an open-pin-data checkout can populate pins and
//! nothing else, which [`ChipSource::has_clock`] exists to say OUT LOUD, before
//! an import, rather than leaving the user to discover a chip with no clock.
//!
//! # `RefName` vs `Name`
//!
//! One file covers several orderable parts: `STM32F103C(8-B)Tx.xml` is both
//! `STM32F103C8Tx` and `STM32F103CBTx`. So the thing a user searches for
//! ([`ChipEntry::ref_name`]) is never quite the thing that gets opened
//! ([`ChipEntry::file`]), and the catalogue has to carry both.

use std::path::{Path, PathBuf};

use super::stm32_pin_data::expand_variants;

/// Which vendor data set a source is — they differ in what they can deliver,
/// not just in where they sit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceKind {
    /// A STM32CubeMX installation's `db` directory: chips AND clock trees.
    CubeMxDb,
    /// An `STM32_open_pin_data` checkout: chips only.
    OpenPinData,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            SourceKind::CubeMxDb => "STM32CubeMX",
            SourceKind::OpenPinData => "STM32_open_pin_data",
        }
    }
}

/// One place chips can be imported from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChipSource {
    pub kind: SourceKind,
    /// The directory holding the per-chip XMLs (`…/db/mcu`, `…/open_pin_data/mcu`).
    pub chips: PathBuf,
    /// The CubeMX `db` root, when this source has one. The clock import needs
    /// it — it reads `db/plugins/clock` and `db/mcu/IP` from here — so its
    /// presence IS the answer to "can this source give me a clock tree".
    pub db: Option<PathBuf>,
}

impl ChipSource {
    /// Can an import from this source populate the Clock tab?
    pub fn has_clock(&self) -> bool {
        self.db.is_some()
    }

    /// The file holding `entry`, which is named after the RANGE, not the part.
    pub fn chip_file(&self, entry: &ChipEntry) -> PathBuf {
        self.chips.join(format!("{}.xml", entry.file))
    }
}

/// One orderable part, as the catalogue knows it.
///
/// Everything past `file` is display metadata for the search list, and is EMPTY
/// when the source has no index to read it from — a listing-derived entry knows
/// the part number and nothing else. The authoritative values always come from
/// the chip XML at import time; these only have to be good enough to pick a row.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ChipEntry {
    /// The orderable part number, e.g. `STM32F103C8Tx` — what a user types.
    pub ref_name: String,
    /// The XML file stem, e.g. `STM32F103C(8-B)Tx` — what gets opened.
    pub file: String,
    pub package: String,
    pub core: String,
    pub mhz: u32,
    pub flash_kb: u32,
    pub ram_kb: u32,
    pub io: u32,
    /// The IDE's family key, e.g. `stm32f1`.
    ///
    /// Lowercased verbatim from the vendor data, exactly as `convert_xml` does
    /// it, so the family shown here is the family the imported chip actually
    /// gets. That includes ST's oddity `STM32L4+`, which stays `stm32l4+`
    /// rather than being quietly folded into `stm32l4` — prettying it up here
    /// would make the catalogue disagree with the import.
    pub family: String,
}

/// Every source this machine offers without being told where to look.
///
/// Only CubeMX can be found automatically — it installs to known paths. An
/// open-pin-data checkout is a git clone that could be anywhere, so it arrives
/// through [`from_path`] instead.
pub fn detect() -> Vec<ChipSource> {
    super::clock::graph::cubemx::default_db_dir()
        .into_iter()
        .filter_map(|db| from_path(&db))
        .collect()
}

/// Classify a folder the user picked, if it is a usable source.
///
/// Deliberately forgiving about WHICH folder: people point at the CubeMX
/// install root, at its `db`, at `db/mcu`, at a repo checkout or at the `mcu`
/// inside it — all five are the same intent, and refusing four of them would be
/// a puzzle, not a validation.
pub fn from_path(path: &Path) -> Option<ChipSource> {
    let cube = |db: PathBuf| ChipSource {
        kind: SourceKind::CubeMxDb,
        chips: db.join("mcu"),
        db: Some(db),
    };
    // The CubeMX `db` itself, or an install root holding one. `plugins/clock`
    // is the discriminator: it is what open-pin-data does not have.
    for db in [path.to_path_buf(), path.join("db")] {
        if db.join("plugins").join("clock").is_dir() && has_chip_xml(&db.join("mcu")) {
            return Some(cube(db));
        }
    }
    // Pointed straight at `db/mcu` — climb one level to find the db root, so the
    // clock half is not lost just because the user picked the deeper folder.
    if let Some(parent) = path.parent()
        && parent.join("plugins").join("clock").is_dir()
        && has_chip_xml(path)
    {
        return Some(cube(parent.to_path_buf()));
    }
    // Chip XMLs with no clock plugins: an open-pin-data checkout, or its `mcu`.
    for chips in [path.join("mcu"), path.to_path_buf()] {
        if has_chip_xml(&chips) {
            return Some(ChipSource {
                kind: SourceKind::OpenPinData,
                chips,
                db: None,
            });
        }
    }
    None
}

/// Does this directory hold chip XMLs? Stops at the first one — the answer is
/// needed for a handful of candidate paths per pick, over directories with a
/// couple of thousand entries.
fn has_chip_xml(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| is_chip_xml(&e.file_name()))
}

fn is_chip_xml(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    name.starts_with("STM32") && name.ends_with(".xml")
}

/// The catalogue of a source.
///
/// Prefers the vendor's own index — CubeMX ships `db/mcu/families.xml`, one
/// 4.8 MB file naming every part with its package, core, memory and pin count.
/// Reading it is a single parse instead of ~2000 file opens, and it is the only
/// way to get that metadata at all. A source without one (open-pin-data) falls
/// back to the directory listing, which yields part numbers and nothing else.
pub fn index(src: &ChipSource) -> Result<Vec<ChipEntry>, String> {
    let families = src.chips.join("families.xml");
    if families.is_file() {
        let xml = std::fs::read_to_string(&families)
            .map_err(|e| format!("could not read {}: {e}", families.display()))?;
        return parse_families(&xml);
    }
    index_from_listing(&src.chips)
}

/// Parse CubeMX's `families.xml` into one entry per orderable part.
///
/// Kept as a pure string function so it is testable without a CubeMX install.
pub fn parse_families(xml: &str) -> Result<Vec<ChipEntry>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("XML parse error: {e}"))?;
    let mut out = Vec::new();
    // Matched by LOCAL name throughout, like the other vendor-XML readers here:
    // ST has changed namespace declarations between releases before.
    for family in doc.descendants().filter(|n| n.has_tag_name("Family")) {
        let key = family
            .attribute("Name")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        for mcu in family.descendants().filter(|n| n.has_tag_name("Mcu")) {
            let Some(ref_name) = mcu.attribute("RefName").map(str::trim) else {
                continue;
            };
            if ref_name.is_empty() {
                continue;
            }
            let text = |tag: &str| -> Option<String> {
                mcu.children()
                    .find(|c| c.has_tag_name(tag))
                    .and_then(|c| c.text())
                    .map(|t| t.trim().to_owned())
            };
            let num = |tag: &str| -> u32 {
                text(tag)
                    .and_then(|t| t.split('.').next().unwrap_or("").parse().ok())
                    .unwrap_or(0)
            };
            out.push(ChipEntry {
                ref_name: ref_name.to_owned(),
                // `Name` is the file; it falls back to the part number, because
                // a row whose file we cannot name would open nothing.
                file: mcu
                    .attribute("Name")
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .unwrap_or(ref_name)
                    .to_owned(),
                package: mcu
                    .attribute("PackageName")
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
                core: text("Core").unwrap_or_default(),
                mhz: num("Frequency"),
                flash_kb: num("Flash"),
                ram_kb: num("Ram"),
                io: num("IONb"),
                family: key.clone(),
            });
        }
    }
    if out.is_empty() {
        return Err("no <Mcu> entries found — is this a CubeMX families.xml?".into());
    }
    Ok(out)
}

/// The fallback catalogue: every `STM32*.xml` in a directory, expanded into the
/// parts it covers.
///
/// `STM32F103C(8-B)Tx.xml` becomes two entries pointing at the one file — the
/// same expansion the importer applies, so what the list offers is exactly what
/// an import can produce.
fn index_from_listing(dir: &Path) -> Result<Vec<ChipEntry>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("could not read {}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name();
        if !is_chip_xml(&name) {
            continue;
        }
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".xml")) else {
            continue;
        };
        for (ref_name, _) in expand_variants(stem) {
            out.push(ChipEntry {
                family: family_of(&ref_name),
                ref_name,
                file: stem.to_owned(),
                ..Default::default()
            });
        }
    }
    if out.is_empty() {
        return Err(format!("no STM32*.xml files in {}", dir.display()));
    }
    out.sort_by(|a, b| a.ref_name.cmp(&b.ref_name));
    Ok(out)
}

/// The family key for a part number, for sources with no index to state it.
///
/// A single series letter takes the digit after it (`STM32F103…` -> `stm32f1`),
/// a multi-letter one does not (`STM32WBA55…` -> `stm32wba`, not `stm32wba5`) —
/// which is what separates `stm32wb` from `stm32wba` from `stm32wl`.
///
/// A GUESS, and only ever a fallback: the import reads the real `Family`
/// attribute out of the chip XML.
pub fn family_of(ref_name: &str) -> String {
    let Some(rest) = ref_name
        .strip_prefix("STM32")
        .or_else(|| ref_name.strip_prefix("stm32"))
    else {
        return String::new();
    };
    let letters: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if letters.is_empty() {
        return String::new();
    }
    let mut key = format!("stm32{}", letters.to_ascii_lowercase());
    if letters.len() == 1
        && let Some(d) = rest[letters.len()..]
            .chars()
            .next()
            .filter(char::is_ascii_digit)
    {
        key.push(d);
    }
    key
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed families.xml: one range file covering two parts, one plain one.
    const FAMILIES: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<Families>
  <Family Name="STM32F1">
    <SubFamily Name="STM32F103">
      <Mcu Name="STM32F103C(8-B)Tx" PackageName="LQFP48" RefName="STM32F103C8Tx" RPN="STM32F103C8">
        <Core>Arm Cortex-M3</Core><Frequency>72</Frequency><Ram>20</Ram><IONb>37</IONb><Flash>64</Flash>
      </Mcu>
      <Mcu Name="STM32F103C(8-B)Tx" PackageName="LQFP48" RefName="STM32F103CBTx" RPN="STM32F103CB">
        <Core>Arm Cortex-M3</Core><Frequency>72</Frequency><Ram>20</Ram><IONb>37</IONb><Flash>128</Flash>
      </Mcu>
    </SubFamily>
  </Family>
  <Family Name="STM32WBA">
    <SubFamily Name="STM32WBA5x">
      <Mcu Name="STM32WBA55CGUx" PackageName="UFQFPN48" RefName="STM32WBA55CGUx" RPN="STM32WBA55CG">
        <Core>Arm Cortex-M33</Core><Frequency>100</Frequency><Ram>128</Ram><IONb>35</IONb><Flash>1024</Flash>
      </Mcu>
    </SubFamily>
  </Family>
</Families>"#;

    /// The catalogue's whole reason to exist: the part you search for and the
    /// file it lives in are different strings.
    #[test]
    fn one_file_yields_every_part_it_covers() {
        let all = parse_families(FAMILIES).unwrap();
        assert_eq!(all.len(), 3);

        let c8 = &all[0];
        assert_eq!(c8.ref_name, "STM32F103C8Tx");
        assert_eq!(c8.file, "STM32F103C(8-B)Tx", "the RANGE names the file");
        assert_eq!(c8.family, "stm32f1");
        assert_eq!((c8.mhz, c8.flash_kb, c8.ram_kb, c8.io), (72, 64, 20, 37));
        assert_eq!(c8.package, "LQFP48");
        assert_eq!(c8.core, "Arm Cortex-M3");

        // Same file, different part — and the flash that separates them.
        let cb = &all[1];
        assert_eq!(cb.ref_name, "STM32F103CBTx");
        assert_eq!(cb.file, c8.file);
        assert_eq!(cb.flash_kb, 128);

        assert_eq!(all[2].family, "stm32wba");
    }

    #[test]
    fn a_file_that_is_not_an_index_is_refused() {
        assert!(parse_families("<Families></Families>").is_err());
        assert!(parse_families("not xml at all <<<").is_err());
    }

    /// The multi-letter series are the whole difficulty here.
    #[test]
    fn the_family_guess_separates_the_w_series() {
        for (part, want) in [
            ("STM32F103C8Tx", "stm32f1"),
            ("STM32F411RETx", "stm32f4"),
            ("STM32H563ZITx", "stm32h5"),
            ("STM32U575ZITx", "stm32u5"),
            ("STM32C011D6Yx", "stm32c0"),
            ("STM32WB55RGVx", "stm32wb"),
            ("STM32WBA55CGUx", "stm32wba"),
            ("STM32WL55JCIx", "stm32wl"),
            ("STM32N657X0HxQ", "stm32n6"),
        ] {
            assert_eq!(family_of(part), want, "for {part}");
        }
        assert_eq!(family_of("ESP32C3"), "", "not an STM32");
    }

    /// A source is whichever of five plausible folders the user happened to
    /// pick — and picking the deeper one must not silently cost the clock half.
    #[test]
    fn any_reasonable_folder_is_accepted() {
        let root = std::env::temp_dir().join(format!("eide_srcs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cube = root.join("CubeMX");
        std::fs::create_dir_all(cube.join("db").join("mcu")).unwrap();
        std::fs::create_dir_all(cube.join("db").join("plugins").join("clock")).unwrap();
        std::fs::write(cube.join("db/mcu/STM32F103C(8-B)Tx.xml"), "<Mcu/>").unwrap();
        let repo = root.join("open_pin_data");
        std::fs::create_dir_all(repo.join("mcu")).unwrap();
        std::fs::write(repo.join("mcu/STM32H563ZITx.xml"), "<Mcu/>").unwrap();

        // Install root, `db`, and `db/mcu` all resolve to the same source.
        for p in [&cube, &cube.join("db"), &cube.join("db").join("mcu")] {
            let s = from_path(p).unwrap_or_else(|| panic!("{}", p.display()));
            assert_eq!(s.kind, SourceKind::CubeMxDb, "{}", p.display());
            assert_eq!(s.db.as_deref(), Some(cube.join("db").as_path()));
            assert!(s.has_clock(), "the clock half survives {}", p.display());
        }
        // The checkout and its `mcu` both resolve, and neither claims a clock.
        for p in [&repo, &repo.join("mcu")] {
            let s = from_path(p).unwrap();
            assert_eq!(s.kind, SourceKind::OpenPinData);
            assert!(!s.has_clock(), "open-pin-data ships no clock trees");
        }
        assert!(from_path(&root).is_none(), "a folder of folders is not one");

        // And the listing fallback expands the range file, both parts pointing
        // at the one file that exists on disk.
        let src = from_path(&cube.join("db")).unwrap();
        let listed = index_from_listing(&src.chips).unwrap();
        assert_eq!(
            listed.iter().map(|e| e.ref_name.as_str()).collect::<Vec<_>>(),
            ["STM32F103C8Tx", "STM32F103CBTx"]
        );
        assert!(
            src.chip_file(&listed[1]).is_file(),
            "the file it names exists"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Against the real vendor data on this machine. Ignored: it needs a CubeMX
    /// installation, which no CI has.
    ///
    /// `cargo test -- --ignored the_real_cubemx_db`
    #[test]
    #[ignore]
    fn the_real_cubemx_db_is_catalogued() {
        let srcs = detect();
        assert!(!srcs.is_empty(), "no CubeMX installation found");
        let src = &srcs[0];
        assert!(src.has_clock());

        let all = index(src).unwrap();
        println!("{} parts from {}", all.len(), src.chips.display());
        assert!(all.len() > 2000, "only {} parts", all.len());

        let f103 = all
            .iter()
            .find(|e| e.ref_name == "STM32F103C8Tx")
            .expect("the blue-pill part is in there");
        assert_eq!(f103.file, "STM32F103C(8-B)Tx");
        assert_eq!(f103.family, "stm32f1");
        assert_eq!(f103.mhz, 72);
        assert!(src.chip_file(f103).is_file(), "and its file is where we say");

        // Every entry must name a file that exists, or the search list offers
        // rows that cannot be imported.
        let missing: Vec<_> = all
            .iter()
            .filter(|e| !src.chip_file(e).is_file())
            .map(|e| e.ref_name.clone())
            .collect();
        println!(
            "{} of {} parts name a file that is not on disk{}",
            missing.len(),
            all.len(),
            match missing.first() {
                Some(first) => format!(" (e.g. {first})"),
                None => String::new(),
            }
        );
        assert!(
            missing.len() < all.len() / 20,
            "{} of {} parts name a missing file, e.g. {:?}",
            missing.len(),
            all.len(),
            &missing[..missing.len().min(5)]
        );
    }

    /// The other half, against a real `STM32_open_pin_data` checkout: no index
    /// file, so the whole catalogue comes out of the directory listing. Ignored
    /// and path-driven — set `EIDE_PIN_DATA` to a checkout to run it.
    ///
    /// `EIDE_PIN_DATA=/path/to/STM32_open_pin_data cargo test -- --ignored the_real_pin_data`
    #[test]
    #[ignore]
    fn the_real_pin_data_checkout_is_catalogued() {
        let Some(path) = std::env::var_os("EIDE_PIN_DATA") else {
            println!("EIDE_PIN_DATA not set — nothing to check");
            return;
        };
        let src = from_path(Path::new(&path)).expect("not a chip source");
        assert_eq!(src.kind, SourceKind::OpenPinData);
        assert!(!src.has_clock(), "this source ships no clock trees");

        let all = index(&src).unwrap();
        println!("{} parts from {}", all.len(), src.chips.display());
        assert!(all.len() > 2000, "only {} parts", all.len());

        // The range expansion is the point: this part exists only as a name
        // INSIDE a file called something else.
        let c8 = all
            .iter()
            .find(|e| e.ref_name == "STM32F103C8Tx")
            .expect("expanded out of its range file");
        assert_eq!(c8.file, "STM32F103C(8-B)Tx");
        assert!(src.chip_file(c8).is_file());
        assert_eq!(c8.family, "stm32f1", "guessed, with no index to say so");

        let missing = all.iter().filter(|e| !src.chip_file(e).is_file()).count();
        assert_eq!(missing, 0, "every listed part must open");
    }
}
