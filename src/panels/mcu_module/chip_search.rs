//! Searching the chip catalogue — the ranking, not the rendering.
//!
//! [`chip_sources`](super::chip_sources) answers "which chips exist on this
//! machine"; this answers "which of them did the user mean". Kept apart from the
//! UI because the interesting part — what ranks above what, and which of two
//! copies of the same part wins — is logic worth testing, and a `TextEdit` is
//! not.
//!
//! # One list, two kinds of hit
//!
//! A chip the IDE already knows and a chip sitting in a vendor file are the same
//! thing to a person typing `F103C8`, so they share one list. They are not the
//! same thing to the IDE — one is selectable now, the other has to be imported
//! first — which is what [`Origin`] carries.
//!
//! # The searchable form
//!
//! Nobody types `STM32`. Ranking therefore matches against the part number both
//! with and without that prefix, so `f103c8` finds `STM32F103C8Tx` and still
//! ranks it below an exact hit. Keys are lowercased ONCE at build time: the
//! search runs over several thousand entries on every keystroke, and lowercasing
//! them per frame is the difference between free and noticeable.

use super::chip_sources::{ChipEntry, ChipSource};

/// Where a hit can be acted on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Origin {
    /// Already a definition in the registry — selecting it is immediate.
    Registry { id: String },
    /// A vendor file on disk. Needs an import before it can be selected.
    Disk {
        /// Index into [`Catalogue::sources`].
        source: usize,
        /// The XML file stem — see [`ChipEntry::file`], which is not the part.
        file: String,
        /// Whether that source can also deliver the clock tree.
        has_clock: bool,
    },
}

impl Origin {
    pub fn is_registry(&self) -> bool {
        matches!(self, Origin::Registry { .. })
    }
}

/// One row of the search list.
///
/// Owned rather than borrowed: a query returns at most a screenful, so the
/// copies are trivial, and the alternative — lifetimes tying every row back to
/// the catalogue — would spread through the UI for nothing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hit {
    /// The part number, as the vendor spells it.
    pub name: String,
    pub family: String,
    /// Package, memory and speed, pre-formatted; empty when the source has no
    /// index to read them from.
    pub detail: String,
    pub origin: Origin,
    /// Lower is better — see [`rank`].
    pub rank: u8,
}

/// A registry entry, as the search needs to see it: `(id, display name, family)`.
///
/// A tuple slice rather than the real `McuDefinition`, so the ranking can be
/// tested without building chip definitions.
pub type RegistryRow<'a> = (&'a str, &'a str, &'a str);

/// Every catalogued chip, ready to search.
pub struct Catalogue {
    pub sources: Vec<ChipSource>,
    /// One flat list rather than a list per source: a search crosses all of
    /// them anyway, and flattening keeps the source index on the row where the
    /// de-duplication needs it.
    rows: Vec<Indexed>,
    /// Sources that failed to index, as `(source index, reason)` — surfaced in
    /// the UI instead of silently shrinking the catalogue.
    pub errors: Vec<(usize, String)>,
}

struct Indexed {
    source: usize,
    entry: ChipEntry,
    /// `stm32f103c8tx` — lowercased once.
    key: String,
    /// The same without the `stm32` prefix, for the way people actually type.
    short: String,
}

impl Catalogue {
    /// Index every source. Failures are recorded, not propagated: one
    /// unreadable folder must not empty the whole list.
    pub fn build(sources: Vec<ChipSource>) -> Self {
        let mut rows = Vec::new();
        let mut errors = Vec::new();
        for (ix, src) in sources.iter().enumerate() {
            match super::chip_sources::index(src) {
                Ok(entries) => rows.extend(entries.into_iter().map(|entry| {
                    let key = entry.ref_name.to_ascii_lowercase();
                    let short = key.strip_prefix("stm32").unwrap_or(&key).to_owned();
                    Indexed {
                        source: ix,
                        entry,
                        key,
                        short,
                    }
                })),
                Err(e) => errors.push((ix, e)),
            }
        }
        Self {
            sources,
            rows,
            errors,
        }
    }

    /// How many parts are catalogued, across every source.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// How many parts a given source contributed.
    pub fn count_of(&self, source: usize) -> usize {
        self.rows.iter().filter(|r| r.source == source).count()
    }

    /// The best `limit` matches for `query`, plus how many there were in total.
    ///
    /// Registry chips are searched alongside the disk ones and always win when
    /// both describe the same part — a chip the IDE already has needs no import,
    /// and offering to import it again would be a worse answer to the same
    /// question. Between two disk sources, the one that can also deliver a clock
    /// tree wins, for the same reason: same part, more of it.
    ///
    /// An empty query returns nothing. The catalogue is thousands of parts long;
    /// a list that starts by offering all of them is a list nobody reads.
    pub fn search(&self, query: &str, registry: &[RegistryRow], limit: usize) -> (Vec<Hit>, usize) {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return (Vec::new(), 0);
        }
        let mut hits: Vec<Hit> = Vec::new();

        for (id, name, family) in registry {
            let key = name.to_ascii_lowercase();
            let short = key.strip_prefix("stm32").unwrap_or(&key);
            if let Some(rank) = rank(&key, short, &q) {
                hits.push(Hit {
                    name: (*name).to_owned(),
                    family: (*family).to_owned(),
                    detail: String::new(),
                    origin: Origin::Registry {
                        id: (*id).to_owned(),
                    },
                    rank,
                });
            }
        }

        for row in &self.rows {
            let Some(rank) = rank(&row.key, &row.short, &q) else {
                continue;
            };
            let has_clock = self.sources[row.source].has_clock();
            // Same part, already covered? Keep whichever offers more.
            if let Some(seen) = hits
                .iter_mut()
                .find(|h| h.name.eq_ignore_ascii_case(&row.entry.ref_name))
            {
                let better = match &seen.origin {
                    // Nothing beats a chip that is already here.
                    Origin::Registry { .. } => false,
                    Origin::Disk {
                        has_clock: seen_clock,
                        ..
                    } => has_clock && !seen_clock,
                };
                if better {
                    *seen = disk_hit(row, rank, has_clock);
                }
                continue;
            }
            hits.push(disk_hit(row, rank, has_clock));
        }

        let total = hits.len();
        // Rank first, then alphabetical, so the order is stable and the reason a
        // row is near the top is visible.
        hits.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.name.cmp(&b.name)));
        hits.truncate(limit);
        (hits, total)
    }
}

fn disk_hit(row: &Indexed, rank: u8, has_clock: bool) -> Hit {
    Hit {
        name: row.entry.ref_name.clone(),
        family: row.entry.family.clone(),
        detail: detail_of(&row.entry),
        origin: Origin::Disk {
            source: row.source,
            file: row.entry.file.clone(),
            has_clock,
        },
        rank,
    }
}

/// How well `key` answers `q`, or `None` for no match at all.
///
/// `short` is `key` without its `stm32` prefix. The two prefix ranks are
/// separate on purpose: someone who types the full prefix is being specific and
/// should not be outranked by a part that merely contains the same letters.
fn rank(key: &str, short: &str, q: &str) -> Option<u8> {
    if key == q || short == q {
        Some(0)
    } else if key.starts_with(q) {
        Some(1)
    } else if short.starts_with(q) {
        Some(2)
    } else if key.contains(q) {
        Some(3)
    } else {
        None
    }
}

/// The one-line summary shown beside a part, from whatever the source knew.
fn detail_of(e: &ChipEntry) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !e.package.is_empty() {
        parts.push(e.package.clone());
    }
    if e.flash_kb > 0 {
        parts.push(format!("{}K flash", e.flash_kb));
    }
    if e.ram_kb > 0 {
        parts.push(format!("{}K RAM", e.ram_kb));
    }
    if e.mhz > 0 {
        parts.push(format!("{} MHz", e.mhz));
    }
    parts.join(" · ")
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::chip_sources::SourceKind;
    use super::*;
    use std::path::PathBuf;

    fn entry(ref_name: &str, family: &str) -> ChipEntry {
        ChipEntry {
            ref_name: ref_name.into(),
            file: ref_name.into(),
            family: family.into(),
            package: "LQFP48".into(),
            flash_kb: 64,
            ram_kb: 20,
            mhz: 72,
            ..Default::default()
        }
    }

    fn source(kind: SourceKind, clock: bool) -> ChipSource {
        ChipSource {
            kind,
            chips: PathBuf::from("chips"),
            db: clock.then(|| PathBuf::from("db")),
        }
    }

    /// Builds a catalogue directly, bypassing the filesystem.
    fn catalogue(sources: Vec<ChipSource>, rows: Vec<(usize, ChipEntry)>) -> Catalogue {
        Catalogue {
            sources,
            rows: rows
                .into_iter()
                .map(|(source, entry)| {
                    let key = entry.ref_name.to_ascii_lowercase();
                    let short = key.strip_prefix("stm32").unwrap_or(&key).to_owned();
                    Indexed {
                        source,
                        entry,
                        key,
                        short,
                    }
                })
                .collect(),
            errors: Vec::new(),
        }
    }

    /// Nobody types `STM32`, and the ranking has to survive that.
    #[test]
    fn the_prefix_is_optional_but_being_specific_still_pays() {
        let c = catalogue(
            vec![source(SourceKind::CubeMxDb, true)],
            vec![
                (0, entry("STM32F103C8Tx", "stm32f1")),
                (0, entry("STM32F103CBTx", "stm32f1")),
                (0, entry("STM32L4F103Zx", "stm32l4")), // contains, does not start
            ],
        );

        let (hits, total) = c.search("f103c8", &[], 10);
        assert_eq!(hits[0].name, "STM32F103C8Tx");
        assert_eq!(hits[0].rank, 2, "matched past the STM32 prefix");
        assert_eq!(total, 1);

        // Typing the whole thing is an exact hit.
        assert_eq!(c.search("stm32f103c8tx", &[], 10).0[0].rank, 0);
        // …and so is typing it without the prefix.
        assert_eq!(c.search("f103c8tx", &[], 10).0[0].rank, 0);

        // A part that merely CONTAINS the query sorts below one that starts with
        // it, however the user typed it.
        let (hits, _) = c.search("f103", &[], 10);
        assert_eq!(
            hits.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
            ["STM32F103C8Tx", "STM32F103CBTx", "STM32L4F103Zx"]
        );
        assert_eq!(hits[2].rank, 3, "the contains-match is last");
    }

    /// An empty query must not dump the catalogue.
    #[test]
    fn nothing_typed_means_nothing_offered() {
        let c = catalogue(
            vec![source(SourceKind::CubeMxDb, true)],
            vec![(0, entry("STM32F103C8Tx", "stm32f1"))],
        );
        assert_eq!(c.search("", &[], 10).0.len(), 0);
        assert_eq!(c.search("   ", &[], 10).0.len(), 0);
    }

    /// The same part in several places is ONE row — the richest one.
    #[test]
    fn duplicates_collapse_to_the_best_copy() {
        // Source 0 has no clock (open-pin-data), source 1 does (CubeMX).
        let c = catalogue(
            vec![
                source(SourceKind::OpenPinData, false),
                source(SourceKind::CubeMxDb, true),
            ],
            vec![
                (0, entry("STM32F103C8Tx", "stm32f1")),
                (1, entry("STM32F103C8Tx", "stm32f1")),
                (0, entry("STM32H563ZITx", "stm32h5")),
            ],
        );

        let (hits, _) = c.search("f103c8", &[], 10);
        assert_eq!(hits.len(), 1, "one part, one row: {hits:?}");
        assert_eq!(
            hits[0].origin,
            Origin::Disk {
                source: 1,
                file: "STM32F103C8Tx".into(),
                has_clock: true,
            },
            "the source that can also give a clock wins"
        );

        // Order of discovery must not change the winner.
        let flipped = catalogue(
            vec![
                source(SourceKind::CubeMxDb, true),
                source(SourceKind::OpenPinData, false),
            ],
            vec![
                (0, entry("STM32F103C8Tx", "stm32f1")),
                (1, entry("STM32F103C8Tx", "stm32f1")),
            ],
        );
        let (hits, _) = flipped.search("f103c8", &[], 10);
        assert_eq!(hits.len(), 1);
        assert!(matches!(hits[0].origin, Origin::Disk { source: 0, .. }));

        // A part only one source has is unaffected.
        assert_eq!(c.search("h563", &[], 10).0.len(), 1);
    }

    /// A chip the IDE already has needs no import, and says so.
    #[test]
    fn a_registry_chip_beats_its_own_vendor_file() {
        let c = catalogue(
            vec![source(SourceKind::CubeMxDb, true)],
            vec![(0, entry("STM32F103C8Tx", "stm32f1"))],
        );
        let reg = [("stm32f103c8tx", "STM32F103C8Tx", "stm32f1")];

        let (hits, _) = c.search("f103c8", &reg, 10);
        assert_eq!(hits.len(), 1, "not offered twice: {hits:?}");
        assert_eq!(
            hits[0].origin,
            Origin::Registry {
                id: "stm32f103c8tx".into()
            }
        );
        assert!(hits[0].origin.is_registry());

        // A registry chip with no vendor file still shows up.
        let reg = [("esp32c3", "ESP32-C3", "esp32c3")];
        let (hits, _) = c.search("esp32", &reg, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "ESP32-C3");
    }

    /// The list is capped, but the count tells the truth about what was cut.
    #[test]
    fn the_cap_is_visible() {
        let rows: Vec<_> = (0..40)
            .map(|i| (0, entry(&format!("STM32F103C{i:02}Tx"), "stm32f1")))
            .collect();
        let c = catalogue(vec![source(SourceKind::CubeMxDb, true)], rows);

        let (hits, total) = c.search("f103", &[], 10);
        assert_eq!(hits.len(), 10, "a screenful");
        assert_eq!(total, 40, "out of this many");
        assert_eq!(c.len(), 40);
        assert_eq!(c.count_of(0), 40);
    }

    /// Against every source this machine has. Ignored: needs real vendor data.
    ///
    /// Also times the build, because that cost lands on the frame that opens the
    /// dialog — if indexing were slow, the UI would have to move it off-thread.
    ///
    /// `cargo test -- --ignored the_real_catalogue`
    #[test]
    #[ignore]
    fn the_real_catalogue_answers_a_part_number() {
        use super::super::chip_sources;

        let started = std::time::Instant::now();
        let c = Catalogue::build(chip_sources::all_sources());
        let took = started.elapsed();
        println!(
            "indexed {} parts from {} source(s) in {took:?}",
            c.len(),
            c.sources.len()
        );
        for (ix, e) in &c.errors {
            println!("  source {ix} failed: {e}");
        }
        assert!(!c.is_empty(), "no chips found on this machine");

        let (hits, total) = c.search("f103c8", &[], 20);
        println!("f103c8 -> {total} hit(s): {:?}", hits.first());
        let blue_pill = hits
            .iter()
            .find(|h| h.name == "STM32F103C8Tx")
            .expect("the blue-pill part");
        assert_eq!(blue_pill.family, "stm32f1");
        assert_eq!(
            hits.iter().filter(|h| h.name == "STM32F103C8Tx").count(),
            1,
            "one row per part, even with several sources"
        );
    }

    /// Whatever the source knew, formatted; whatever it did not, absent.
    #[test]
    fn the_detail_line_skips_what_is_unknown() {
        assert_eq!(
            detail_of(&entry("STM32F103C8Tx", "stm32f1")),
            "LQFP48 · 64K flash · 20K RAM · 72 MHz"
        );
        // A listing-derived entry knows only the part number.
        let bare = ChipEntry {
            ref_name: "STM32H563ZITx".into(),
            ..Default::default()
        };
        assert_eq!(detail_of(&bare), "");
    }
}
