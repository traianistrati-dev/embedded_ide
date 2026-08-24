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

use super::chip_filter::{ChipFilter, Metrics, RowMetrics, Verdict};
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
    /// The vendor file this part could be re-imported from, when it is
    /// ALREADY in the registry and still on disk.
    ///
    /// A registry hit shadows the disk one - re-offering an import as the
    /// primary action would be a worse answer to "which chip do I want".
    /// But the vendor data can have changed (a newer CubeMX, a fixed import),
    /// so the row keeps the location for a secondary "re-import" that
    /// overwrites the stored `.ron`.
    pub reimport: Option<Origin>,
    /// What [`ChipFilter`] judges this row on.
    ///
    /// Owned, because a registry row has no catalogue entry of its own: it
    /// inherits the numbers of the vendor file it shadows, and falls back to
    /// whatever its `memory.x` sizes say when there is no such file.
    pub metrics: RowMetrics,
}

/// A registry entry, as the search needs to see it.
///
/// A borrowed row rather than the real `McuDefinition`, so the ranking can be
/// tested without building chip definitions.
#[derive(Clone, Copy, Debug)]
pub struct RegistryRow<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub family: &'a str,
    /// From the definition's `memory.x` sizes — see
    /// [`chip_filter::parse_memory_kb`](super::chip_filter::parse_memory_kb).
    /// The only memory figure a registry entry holds, and absent on chips whose
    /// definition leaves them blank.
    pub flash_kb: Option<u32>,
    pub ram_kb: Option<u32>,
    /// As the definition records it, e.g. `LQFP48`; empty when it does not.
    pub package: &'a str,
}

/// A search, and what it had to leave out.
pub struct Results {
    pub hits: Vec<Hit>,
    /// How many rows matched in total, before `limit` truncated them.
    pub total: usize,
    /// Rows excluded ONLY because their source could not answer an active
    /// facet — an open-pin-data checkout ships part numbers and nothing else.
    ///
    /// Reported rather than absorbed: a filter that quietly deletes a whole
    /// source is indistinguishable from a filter that found nothing.
    pub unknown: usize,
}

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
    /// Indices into `rows`: one per DISTINCT part, the copy that knows the most
    /// about it. This is the set a search actually walks.
    ///
    /// Computed once here rather than per keystroke: the same part number
    /// appears in every source that carries it, and picking between them by
    /// hand — first one wins — silently preferred whichever source happened to
    /// be listed first over whichever one had the data.
    unified: Vec<usize>,
}

/// One winner per part: the source that knows the most about it.
///
/// A clock tree outranks every other fact — it is the difference between a Clock
/// tab that works and one that says the chip has no clock — and beyond that it
/// is simply how many facts the source carries. Ties keep the earlier source,
/// so a re-index cannot reshuffle the catalogue for no reason.
fn pick_unified(rows: &[Indexed], sources: &[ChipSource]) -> Vec<usize> {
    let score = |r: &Indexed| (sources[r.source].has_clock(), r.entry.completeness());
    let mut best: std::collections::HashMap<String, usize> =
        std::collections::HashMap::with_capacity(rows.len());
    for (ix, row) in rows.iter().enumerate() {
        match best.get(&row.key) {
            Some(&cur) if score(&rows[cur]) >= score(row) => {}
            _ => {
                best.insert(row.key.clone(), ix);
            }
        }
    }
    let mut out: Vec<usize> = best.into_values().collect();
    out.sort_unstable();
    out
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
        // One winner per part: the source that knows the most about it. A clock
        // tree outranks every other fact — it is the difference between a Clock
        // tab that works and one that says the chip has no clock — and beyond
        // that it is simply how many facts the source carries.
        let unified = pick_unified(&rows, &sources);

        Self {
            sources,
            rows,
            errors,
            unified,
        }
    }

    /// How many DISTINCT parts the sources describe between them.
    ///
    /// Not the sum of the per-source counts: a part in three sources is one
    /// part. This is the number a search can actually return.
    pub fn unified_len(&self) -> usize {
        self.unified.len()
    }

    /// How many parts are catalogued, across every source.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Every catalogued part, for deriving the filter's ranges and facets.
    pub fn entries(&self) -> impl Iterator<Item = &ChipEntry> {
        self.rows.iter().map(|r| &r.entry)
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
    /// An empty query returns nothing — UNLESS a filter is narrowing the list.
    ///
    /// The catalogue is thousands of parts long and a list that starts by
    /// offering all of them is a list nobody reads. A filter is exactly what
    /// makes such a list readable, which is why it is the one thing that turns
    /// browsing on: "every G4 with three SPIs and 256K" is a question with a
    /// short answer. With no filter set, the behaviour is unchanged.
    pub fn search(
        &self,
        query: &str,
        registry: &[RegistryRow],
        filter: &ChipFilter,
        limit: usize,
    ) -> Results {
        let q = query.trim().to_ascii_lowercase();
        let browsing = q.is_empty();
        if browsing && !filter.is_active() {
            return Results {
                hits: Vec::new(),
                total: 0,
                unknown: 0,
            };
        }
        let active = filter.is_active();
        let mut hits: Vec<Hit> = Vec::new();

        for r in registry {
            let key = r.name.to_ascii_lowercase();
            let short = key.strip_prefix("stm32").unwrap_or(&key);
            // While browsing every row is equally relevant; the sort below is
            // alphabetical anyway.
            let Some(rank) = (if browsing {
                Some(0)
            } else {
                rank(&key, short, &q)
            }) else {
                continue;
            };
            hits.push(Hit {
                name: r.name.to_owned(),
                family: r.family.to_owned(),
                detail: String::new(),
                origin: Origin::Registry {
                    id: r.id.to_owned(),
                },
                rank,
                reimport: None,
                // Only what the definition itself holds. The dedup below
                // upgrades this the moment a vendor file for the same part
                // turns up, which is the usual case — the chip got here by
                // being imported from one.
                metrics: RowMetrics {
                    flash_kb: r.flash_kb,
                    ram_kb: r.ram_kb,
                    package: r.package.to_owned(),
                    ..Default::default()
                },
            });
        }

        let registry_count = hits.len();

        // Where each part already sits, keyed on the name that is ALREADY
        // lowercased on both sides. This replaced a linear scan of the growing
        // hit list, which browsing turned into ~15 million string comparisons
        // per FRAME - 100 ms in a release build, and the search runs every frame.
        let mut seen_at: std::collections::HashMap<String, usize> = hits
            .iter()
            .enumerate()
            .map(|(ix, h)| (h.name.to_ascii_lowercase(), ix))
            .collect();
        // Parts a source could not answer for. Held as keys rather than counted
        // on the spot: the SAME part often arrives again from an indexed source,
        // and a row that is on screen is not a row that was hidden.
        let mut silent: std::collections::HashSet<&str> = std::collections::HashSet::new();

        for row in self.unified.iter().map(|&ix| &self.rows[ix]) {
            let Some(rank) = (if browsing {
                Some(0)
            } else {
                rank(&row.key, &row.short, &q)
            }) else {
                continue;
            };
            let has_clock = self.sources[row.source].has_clock();
            // Same part, already covered? Keep whichever offers more.
            if let Some(&ix) = seen_at.get(row.key.as_str()) {
                let seen = &mut hits[ix];
                let better = match &seen.origin {
                    // Nothing beats a chip that is already here - but keep
                    // WHERE it came from, so the row can offer to refresh it.
                    Origin::Registry { .. } => {
                        // …and take its NUMBERS, which the registry entry does
                        // not carry. Without this a chip already imported would
                        // be excluded by every peripheral filter, which is the
                        // one chip the user certainly has.
                        if row.entry.knows_peripherals() {
                            let own = std::mem::take(&mut seen.metrics.package);
                            seen.metrics = RowMetrics::of(&row.entry);
                            if seen.metrics.package.is_empty() {
                                seen.metrics.package = own;
                            }
                        }
                        let keep = match &seen.reimport {
                            // Same rule as between two disk rows: more data wins.
                            Some(Origin::Disk {
                                has_clock: seen_clock,
                                ..
                            }) => has_clock && !seen_clock,
                            _ => true,
                        };
                        if keep {
                            seen.reimport = Some(disk_hit(row, rank, has_clock).origin);
                        }
                        false
                    }
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
            // Judged on the BORROWED entry, before a single string is cloned.
            // Building the hit first and dropping it afterwards meant ~80,000
            // wasted allocations per frame on a catalogue this size.
            if active {
                match filter.matches(&Metrics::of(&row.entry)) {
                    Verdict::Pass => {}
                    Verdict::Unknown => {
                        silent.insert(row.key.as_str());
                        continue;
                    }
                    Verdict::Fail => continue,
                }
            }
            seen_at.insert(row.key.clone(), hits.len());
            hits.push(disk_hit(row, rank, has_clock));
        }

        // The REGISTRY rows are the only ones still unjudged: they were built
        // before the vendor metrics they inherit above could be known. Disk rows
        // were judged on the way in.
        let mut unknown = 0usize;
        if active {
            let mut ix = 0usize;
            hits.retain(|h| {
                let keep = ix >= registry_count
                    || match filter.matches(&h.metrics.view(&h.family)) {
                        Verdict::Pass => true,
                        Verdict::Unknown => {
                            unknown += 1;
                            false
                        }
                        Verdict::Fail => false,
                    };
                ix += 1;
                keep
            });
        }
        // A part one source could not describe, but another could, was never
        // hidden - so it must not be counted as such.
        unknown += silent.iter().filter(|k| !seen_at.contains_key(**k)).count();

        let total = hits.len();
        // Rank first, then alphabetical, so the order is stable and the reason a
        // row is near the top is visible. Browsing gives every row rank 0, which
        // leaves the list plainly alphabetical.
        hits.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.name.cmp(&b.name)));
        hits.truncate(limit);
        Results {
            hits,
            total,
            unknown,
        }
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
        reimport: None,
        metrics: RowMetrics::of(&row.entry),
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
///
/// The core is in here because it is FILTERABLE: being able to narrow the list
/// to Cortex-M33 parts, and then not being told which core any row has, leaves
/// the filter with no visible effect. It is also the one field where a part can
/// legitimately give two answers - see [`ChipEntry::cores`].
fn detail_of(e: &ChipEntry) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !e.package.is_empty() {
        parts.push(e.package.clone());
    }
    if !e.cores.is_empty() {
        // "Arm Cortex-M4" is four words of which one is the answer, and a
        // dual-core part would otherwise spend half the line saying "Arm"
        // twice.
        parts.push(
            e.cores
                .iter()
                .map(|c| c.trim_start_matches("Arm ").trim_start_matches("Cortex-"))
                .collect::<Vec<_>>()
                .join("+"),
        );
    }
    // `Some(0)` is printed, because an MP1 with no internal flash is a fact
    // worth showing; `None` is skipped, because it is only our ignorance.
    if let Some(v) = e.flash_kb {
        parts.push(format!("{v}K flash"));
    }
    if let Some(v) = e.ram_kb {
        parts.push(format!("{v}K RAM"));
    }
    if let Some(v) = e.mhz {
        parts.push(format!("{v} MHz"));
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

    /// A registry row, as the definitions the IDE already has produce one.
    fn reg_row<'a>(id: &'a str, name: &'a str, family: &'a str) -> RegistryRow<'a> {
        RegistryRow {
            id,
            name,
            family,
            flash_kb: None,
            ram_kb: None,
            package: "",
        }
    }

    /// Search with no filter — the shape these tests were written against, and
    /// the behaviour that must not have changed.
    fn find(c: &Catalogue, q: &str, reg: &[RegistryRow], limit: usize) -> (Vec<Hit>, usize) {
        let r = c.search(q, reg, &ChipFilter::default(), limit);
        (r.hits, r.total)
    }

    fn entry(ref_name: &str, family: &str) -> ChipEntry {
        ChipEntry {
            ref_name: ref_name.into(),
            file: ref_name.into(),
            family: family.into(),
            package: "LQFP48".into(),
            cores: vec!["Arm Cortex-M3".into()],
            flash_kb: Some(64),
            ram_kb: Some(20),
            mhz: Some(72),
            ..Default::default()
        }
    }

    fn source(kind: SourceKind, clock: bool) -> ChipSource {
        ChipSource {
            user_added: false,
            kind,
            chips: PathBuf::from("chips"),
            db: clock.then(|| PathBuf::from("db")),
        }
    }

    /// Builds a catalogue directly, bypassing the filesystem.
    fn catalogue(sources: Vec<ChipSource>, rows: Vec<(usize, ChipEntry)>) -> Catalogue {
        let rows: Vec<Indexed> = rows
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
            .collect();
        // The same winner pass the real constructor runs — a test catalogue that
        // skipped it would search a set no user ever sees.
        let unified = pick_unified(&rows, &sources);
        Catalogue {
            sources,
            rows,
            errors: Vec::new(),
            unified,
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

        let (hits, total) = find(&c, "f103c8", &[], 10);
        assert_eq!(hits[0].name, "STM32F103C8Tx");
        assert_eq!(hits[0].rank, 2, "matched past the STM32 prefix");
        assert_eq!(total, 1);

        // Typing the whole thing is an exact hit.
        assert_eq!(find(&c, "stm32f103c8tx", &[], 10).0[0].rank, 0);
        // …and so is typing it without the prefix.
        assert_eq!(find(&c, "f103c8tx", &[], 10).0[0].rank, 0);

        // A part that merely CONTAINS the query sorts below one that starts with
        // it, however the user typed it.
        let (hits, _) = find(&c, "f103", &[], 10);
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
        assert_eq!(find(&c, "", &[], 10).0.len(), 0);
        assert_eq!(find(&c, "   ", &[], 10).0.len(), 0);
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

        let (hits, _) = find(&c, "f103c8", &[], 10);
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
        let (hits, _) = find(&flipped, "f103c8", &[], 10);
        assert_eq!(hits.len(), 1);
        assert!(matches!(hits[0].origin, Origin::Disk { source: 0, .. }));

        // A part only one source has is unaffected.
        assert_eq!(find(&c, "h563", &[], 10).0.len(), 1);
    }

    /// A chip the IDE already has needs no import, and says so.
    #[test]
    fn a_registry_chip_beats_its_own_vendor_file() {
        let c = catalogue(
            vec![source(SourceKind::CubeMxDb, true)],
            vec![(0, entry("STM32F103C8Tx", "stm32f1"))],
        );
        let reg = [reg_row("stm32f103c8tx", "STM32F103C8Tx", "stm32f1")];

        let (hits, _) = find(&c, "f103c8", &reg, 10);
        assert_eq!(hits.len(), 1, "not offered twice: {hits:?}");
        assert_eq!(
            hits[0].origin,
            Origin::Registry {
                id: "stm32f103c8tx".into()
            }
        );
        assert!(hits[0].origin.is_registry());

        // A registry chip with no vendor file still shows up.
        let reg = [reg_row("esp32c3", "ESP32-C3", "esp32c3")];
        let (hits, _) = find(&c, "esp32", &reg, 10);
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

        let (hits, total) = find(&c, "f103", &[], 10);
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

        let (hits, total) = find(&c, "f103c8", &[], 20);
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
            "LQFP48 · M3 · 64K flash · 20K RAM · 72 MHz"
        );

        // A dual-core part says both, in one field rather than two.
        let mut h7 = entry("STM32H747XIHx", "stm32h7");
        h7.cores = vec!["Arm Cortex-M7".into(), "Arm Cortex-M4".into()];
        assert!(detail_of(&h7).contains("M7+M4"), "got {:?}", detail_of(&h7));
        // A listing-derived entry knows only the part number.
        let bare = ChipEntry {
            ref_name: "STM32H563ZITx".into(),
            ..Default::default()
        };
        assert_eq!(detail_of(&bare), "");
    }

    /// The same part in two sources is ONE part, and the copy that survives is
    /// the one that knows more — not the one whose source was listed first.
    ///
    /// This is what two CubeMX folders on one machine actually look like: both
    /// carry the part, and picking by position threw away flash, RAM and the
    /// peripheral table roughly half the time.
    #[test]
    fn a_part_in_two_sources_is_one_part_and_keeps_the_better_copy() {
        let thin = ChipEntry {
            ref_name: "STM32WL30KBVx".into(),
            file: "STM32WL30KBVx".into(),
            family: "stm32wl3".into(),
            ..Default::default()
        };
        let rich = entry("STM32WL30KBVx", "stm32wl3");
        assert!(rich.completeness() > thin.completeness(), "the fixture is the point");

        // Thin source listed FIRST, so "first wins" would pick the wrong one.
        let cat = catalogue(
            vec![
                source(SourceKind::OpenPinData, false),
                source(SourceKind::CubeMxDb, true),
            ],
            vec![(0, thin), (1, rich)],
        );

        assert_eq!(cat.len(), 2, "both copies are still catalogued");
        assert_eq!(cat.unified_len(), 1, "but they are ONE part");

        let (hits, total) = find(&cat, "wl30", &[], 10);
        assert_eq!(total, 1, "and a search returns it once");
        assert_eq!(hits.len(), 1);
        // The surviving row is the one with the data.
        assert!(hits[0].detail.contains("64"), "kept the thin copy: {:?}", hits[0].detail);
    }

    /// A part that is BOTH in the registry and on disk keeps its disk
    /// location, so the row can offer to overwrite the stored definition.
    /// Before this it was thrown away and the only way to refresh a chip was
    /// to delete its `.ron` by hand.
    #[test]
    fn an_already_added_chip_remembers_where_to_re_import_from() {
        let cat = catalogue(
            vec![source(SourceKind::CubeMxDb, true)],
            vec![(0, entry("STM32F358CCTx", "stm32f3"))],
        );
        let registry = [reg_row("stm32f358cc", "STM32F358CCTx", "stm32f3")];
        let (hits, _) = find(&cat, "358cc", &registry, 10);
        let hit = hits
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("STM32F358CCTx"))
            .expect("the part is in both");
        // The registry still wins the row itself - selecting must not import.
        assert!(hit.origin.is_registry());
        // …but the vendor file is remembered.
        assert!(
            matches!(hit.reimport, Some(Origin::Disk { .. })),
            "no re-import offered: {:?}",
            hit.reimport
        );
    }

    /// A registry chip with no vendor file behind it offers nothing, rather
    /// than a button that would fail.
    #[test]
    fn a_registry_only_chip_offers_no_re_import() {
        let cat = catalogue(vec![source(SourceKind::CubeMxDb, true)], vec![]);
        let registry = [reg_row("mystery1", "MYSTERYCHIP1", "custom")];
        let (hits, _) = find(&cat, "mystery", &registry, 10);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].reimport.is_none());
    }
}
