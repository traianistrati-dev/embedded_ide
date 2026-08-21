//! Filtering the chip catalogue by what a part actually *has*.
//!
//! [`chip_search`](super::chip_search) answers "which of these did the user
//! mean"; this answers the other half of picking a chip — "which of these could
//! I even use". They compose: the filter narrows, the ranking orders.
//!
//! # Everything here comes from `families.xml`
//!
//! CubeMX's index carries memory, speed, pin count and a `<Peripheral Type
//! MaxOccurs>` line per peripheral. That is the whole data source, which sets
//! two hard limits worth stating up front rather than discovering later:
//!
//! * **A source with no index knows nothing.** An open-pin-data checkout ships
//!   part numbers only. Such a row cannot pass *or* fail a flash filter, so it
//!   gets [`Verdict::Unknown`] and the UI counts it out loud. Reading "no data"
//!   as "zero" would silently delete a whole source from the list.
//! * **`MaxOccurs` is not always an instance count.** For `USART` it is — an
//!   F411 says 3 and has USART1/2/6. For `ADC 12-bit` it is CHANNELS: the same
//!   F411 says 16 and has exactly one ADC. So ADC is offered as presence plus a
//!   resolution, never as a number, and the counted facets below are only the
//!   types where the number means what a person would assume.
//!
//! # Why the tiers
//!
//! [`TIER1`] is the short list the IDE can generate code for, where "how many"
//! is the question people ask. Everything else in the vendor data is offered as
//! a yes/no, derived from the catalogue rather than hard-coded — see
//! [`presence_facets`] — so a CubeMX update that adds a peripheral adds a filter
//! for it without a code change.

use super::chip_sources::ChipEntry;
use std::collections::{BTreeMap, BTreeSet};

// ──────────────────────────────────────────────────────────────────────────────
// Facet tables
// ──────────────────────────────────────────────────────────────────────────────

/// A Tier-1 facet: a peripheral you filter by COUNT.
pub struct CountFacet {
    /// Stable key — what the filter map is keyed on.
    pub id: &'static str,
    pub label: &'static str,
    /// The vendor type(s) that make it up, SUMMED.
    ///
    /// Only `Timers` has more than one: ST splits them into `Timer 16-bit` and
    /// `Timer 32-bit`, and nobody shops for one width in particular — a G431
    /// has nine of the first and one of the second, and the useful answer is
    /// "ten".
    pub types: &'static [&'static str],
    /// The largest value in the catalogue, so a spinner cannot be dragged into
    /// a range that matches nothing.
    pub max: u16,
}

/// The peripherals worth a number, with the maxima measured across all 2781
/// catalogued parts.
pub const TIER1: &[CountFacet] = &[
    CountFacet {
        id: "usart",
        label: "USART",
        types: &["USART"],
        max: 8,
    },
    CountFacet {
        id: "uart",
        label: "UART",
        types: &["UART"],
        max: 6,
    },
    CountFacet {
        id: "lpuart",
        label: "LPUART",
        types: &["LPUART"],
        max: 3,
    },
    CountFacet {
        id: "spi",
        label: "SPI",
        types: &["SPI"],
        max: 8,
    },
    CountFacet {
        id: "i2c",
        label: "I2C",
        types: &["I2C"],
        max: 8,
    },
    CountFacet {
        id: "timers",
        label: "Timers",
        types: &["Timer 16-bit", "Timer 32-bit"],
        max: 21,
    },
    CountFacet {
        id: "can",
        label: "CAN",
        types: &["CAN"],
        max: 3,
    },
    CountFacet {
        id: "fdcan",
        label: "FDCAN",
        types: &["FDCAN"],
        max: 3,
    },
    CountFacet {
        id: "comp",
        label: "COMP",
        types: &["COMP"],
        max: 7,
    },
    CountFacet {
        id: "dac",
        label: "DAC",
        types: &["DAC 12-bit"],
        max: 3,
    },
];

/// The vendor types that mean "this part can do USB".
///
/// Five spellings for one question. Someone looking for a USB-capable chip does
/// not yet know whether the one they want calls it `USB Device` or `USB OTG_FS`
/// — that is precisely what they are trying to find out.
pub const USB_TYPES: &[&str] = &[
    "USB Device",
    "USB OTG_FS",
    "USB OTG_HS",
    "USB DRD_FS",
    "USBH_HS",
];

/// The ADC resolutions the vendor data distinguishes.
///
/// Only these two exist in the whole catalogue — there is no `ADC 10-bit` type,
/// however many STM32s have one. 69 parts carry both, so this is an OR, not a
/// choice between two.
pub const ADC_TYPES: [&str; 2] = ["ADC 12-bit", "ADC 16-bit"];

/// Every vendor type already spoken for by a named facet.
fn claimed() -> BTreeSet<&'static str> {
    TIER1
        .iter()
        .flat_map(|f| f.types.iter().copied())
        .chain(USB_TYPES.iter().copied())
        .chain(ADC_TYPES)
        .collect()
}

/// The Tier-2 facets: everything else the catalogue mentions, as `(type, how
/// many parts have it)`, most common first.
///
/// Derived rather than listed, so this stays correct across CubeMX updates —
/// and so it can never offer a filter that matches nothing on this machine.
pub fn presence_facets<'a>(
    entries: impl IntoIterator<Item = &'a ChipEntry>,
) -> Vec<(String, usize)> {
    let claimed = claimed();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for e in entries {
        for (ty, _) in &e.peripherals {
            if !claimed.contains(ty.as_str()) {
                *seen.entry(ty.as_str()).or_default() += 1;
            }
        }
    }
    let mut out: Vec<(String, usize)> = seen.into_iter().map(|(t, n)| (t.to_owned(), n)).collect();
    // Commonest first, then alphabetical — the long tail is 80 entries and the
    // ones worth scrolling past are the rare ones.
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// A package name split into the two questions people actually ask of it:
/// `("LQFP", Some(48))`.
///
/// 133 distinct names in the catalogue, but they are one shape: an alphabetic
/// TYPE, a pin count, and sometimes a suffix. A flat list of 133 buttons would
/// be unusable; "LQFP" and "48 pins" are two short answers that compose.
///
/// The suffixes are the whole reason this is a function with tests rather than a
/// `split_at`. Real names in the data: `LQFP48_N`, `UFQFPN48_SMPS_USB`,
/// `TFBGA225 HEXA SMPS`, `VFBGA169GP`, `SO8N`, and `UFBGA176+25` — where the
/// pin count to filter on is the FIRST number, not the sum.
pub fn split_package(name: &str) -> (&str, Option<u32>) {
    let ty_len = name
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(name.len());
    let (ty, rest) = name.split_at(ty_len);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (ty, digits.parse().ok())
}

/// The package TYPES the catalogue contains, commonest first.
///
/// Derived like [`presence_facets`], and for the same reason: the set is the
/// vendor's, not ours.
pub fn package_types<'a>(entries: impl IntoIterator<Item = &'a ChipEntry>) -> Vec<(String, usize)> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for e in entries {
        let (ty, _) = split_package(&e.package);
        if !ty.is_empty() {
            *seen.entry(ty).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = seen.into_iter().map(|(t, n)| (t.to_owned(), n)).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

// ──────────────────────────────────────────────────────────────────────────────
// What a row is known to be
// ──────────────────────────────────────────────────────────────────────────────

/// One catalogue row, as the filter needs to see it.
///
/// Every numeric field is an `Option` because "the source did not say" is a
/// third answer, distinct from any value it could have given.
#[derive(Default, Clone, Debug)]
pub struct Metrics<'a> {
    pub flash_kb: Option<u32>,
    pub ram_kb: Option<u32>,
    pub mhz: Option<u32>,
    pub io: Option<u32>,
    pub family: &'a str,
    pub cores: &'a [String],
    /// As the vendor spells it, e.g. `LQFP48` — empty when unknown. Split by
    /// [`split_package`] rather than matched whole.
    pub package: &'a str,
    /// `None` when the source has no index — NOT an empty peripheral list.
    pub peripherals: Option<&'a [(String, u16)]>,
}

impl<'a> Metrics<'a> {
    /// The metrics of a catalogued part.
    ///
    /// The numbers pass straight through: [`ChipEntry`] already distinguishes
    /// "the index did not say" from "the index said zero", per field. It used to
    /// be decided here, from whether the entry knew any peripherals — a proxy
    /// that was right for a listing-only source and wrong for the 764 indexed
    /// parts that state no frequency.
    pub fn of(e: &'a ChipEntry) -> Self {
        let indexed = e.knows_peripherals();
        Self {
            flash_kb: e.flash_kb,
            ram_kb: e.ram_kb,
            mhz: e.mhz,
            io: e.io,
            family: &e.family,
            cores: &e.cores,
            package: &e.package,
            peripherals: indexed.then_some(e.peripherals.as_slice()),
        }
    }

    fn count_of(&self, types: &[&str]) -> Option<u16> {
        let ps = self.peripherals?;
        Some(
            types
                .iter()
                .map(|t| {
                    ps.iter()
                        .find(|(k, _)| k == t)
                        .map(|(_, n)| *n)
                        .unwrap_or(0)
                })
                .sum(),
        )
    }

    fn has_any(&self, types: &[&str]) -> Option<bool> {
        let ps = self.peripherals?;
        Some(types.iter().any(|t| ps.iter().any(|(k, _)| k == t)))
    }
}

/// The same facts as [`Metrics`], owned.
///
/// A search row is not always a catalogue row: a chip already in the registry
/// has no [`ChipEntry`] behind it, and one that was imported from a vendor file
/// should still be judged on that file's numbers. Owning them lets the search
/// hand a registry row the metrics of the disk row it shadows — see
/// `chip_search::search`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RowMetrics {
    pub flash_kb: Option<u32>,
    pub ram_kb: Option<u32>,
    pub mhz: Option<u32>,
    pub io: Option<u32>,
    pub cores: Vec<String>,
    pub package: String,
    /// `None` when nothing indexed this part.
    pub peripherals: Option<Vec<(String, u16)>>,
}

impl RowMetrics {
    pub fn of(e: &ChipEntry) -> Self {
        let m = Metrics::of(e);
        Self {
            flash_kb: m.flash_kb,
            ram_kb: m.ram_kb,
            mhz: m.mhz,
            io: m.io,
            cores: e.cores.clone(),
            package: e.package.clone(),
            peripherals: m.peripherals.map(<[_]>::to_vec),
        }
    }

    /// Whether this carries anything at all — a registry row with no vendor
    /// file behind it and no `memory.x` sizes carries nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn view<'a>(&'a self, family: &'a str) -> Metrics<'a> {
        Metrics {
            flash_kb: self.flash_kb,
            ram_kb: self.ram_kb,
            mhz: self.mhz,
            io: self.io,
            family,
            cores: &self.cores,
            package: &self.package,
            peripherals: self.peripherals.as_deref(),
        }
    }
}

/// A `memory.x` size (`"128K"`) as kilobytes.
///
/// The only memory figure a registry entry has: [`ProjectDef`] stores what goes
/// into the linker script, not what the vendor said. Returns `None` for the
/// empty string an ESP32 definition carries, which must stay "unknown" rather
/// than becoming a zero that fails every range.
///
/// [`ProjectDef`]: super::mcu_def::ProjectDef
pub fn parse_memory_kb(s: &str) -> Option<u32> {
    let t = s.trim();
    let (num, mul) = match t.chars().last()? {
        'K' | 'k' => (&t[..t.len() - 1], 1),
        'M' | 'm' => (&t[..t.len() - 1], 1024),
        // Bare bytes, hex or decimal — rare, but a hand-edited definition can
        // hold either, and rounding DOWN keeps a range check honest.
        _ => {
            let v = t
                .strip_prefix("0x")
                .or_else(|| t.strip_prefix("0X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| t.parse::<u32>().ok())?;
            return Some(v / 1024);
        }
    };
    num.trim().parse::<u32>().ok().map(|v| v * mul)
}

/// What the filter concluded about a row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Pass,
    /// Ruled out on a facet the row HAS data for.
    Fail,
    /// Ruled out only because the row has no data on an active facet. Kept
    /// apart so the UI can say how many rows it is hiding for that reason
    /// rather than leaving a source to vanish without explanation.
    Unknown,
}

// ──────────────────────────────────────────────────────────────────────────────
// The filter
// ──────────────────────────────────────────────────────────────────────────────

/// The span each range slider covers, measured from the catalogue.
///
/// Derived rather than fixed so a machine with only an F1 install does not get
/// a flash slider whose top half reaches parts it has never heard of. Doubles
/// as the "inactive" marker: a range equal to its bounds excludes nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bounds {
    pub flash_kb: (u32, u32),
    pub ram_kb: (u32, u32),
    pub mhz: (u32, u32),
    pub io: (u32, u32),
    /// Pins on the PACKAGE, which is not [`Bounds::io`]: an F103C8 is an LQFP48
    /// with 37 of those pins usable. The footprint is what you pick a package
    /// for; the I/O count is what you pick a part for.
    pub pins: (u32, u32),
}

impl Default for Bounds {
    /// The real spans of a full CubeMX catalogue, for when there is nothing to
    /// measure yet.
    fn default() -> Self {
        Self {
            flash_kb: (0, 4096),
            ram_kb: (0, 4200),
            mhz: (0, 600),
            io: (0, 200),
            pins: (0, 448),
        }
    }
}

/// A span nothing has voted on yet — grown into shape by `Bounds::of`.
const EMPTY: (u32, u32) = (u32::MAX, 0);

impl Bounds {
    /// Measure the spans across an indexed catalogue.
    ///
    /// Listing-only entries are skipped: their zeroes are absence of data, and
    /// letting them pull every lower bound to zero would make every slider
    /// start somewhere no part actually lives.
    pub fn of<'a>(entries: impl IntoIterator<Item = &'a ChipEntry>) -> Self {
        let mut b: Option<Bounds> = None;
        for e in entries.into_iter().filter(|e| e.knows_peripherals()) {
            // Each field votes only if the part states it. Anything else and
            // one silent part would pull a span down to a value nothing has -
            // which is exactly how a slider ends up starting at 0 MHz.
            let acc = b.get_or_insert(Bounds {
                flash_kb: EMPTY,
                ram_kb: EMPTY,
                mhz: EMPTY,
                io: EMPTY,
                pins: EMPTY,
            });
            let grow = |r: &mut (u32, u32), v: Option<u32>| {
                if let Some(v) = v {
                    r.0 = r.0.min(v);
                    r.1 = r.1.max(v);
                }
            };
            grow(&mut acc.flash_kb, e.flash_kb);
            grow(&mut acc.ram_kb, e.ram_kb);
            grow(&mut acc.mhz, e.mhz);
            grow(&mut acc.io, e.io);
            grow(&mut acc.pins, split_package(&e.package).1);
        }
        // A field no part stated is left as an EMPTY span rather than an
        // inverted one; the UI checks for that before drawing a slider.
        if let Some(acc) = &mut b {
            for r in [
                &mut acc.flash_kb,
                &mut acc.ram_kb,
                &mut acc.mhz,
                &mut acc.io,
                &mut acc.pins,
            ] {
                if r.0 > r.1 {
                    *r = (0, 0);
                }
            }
        }
        b.unwrap_or_default()
    }
}

/// Which ADC a part must have.
///
/// Presence and resolution rather than a count, because the vendor's ADC number
/// is channels — see the module docs.
#[derive(Default, Clone, PartialEq, Eq, Debug)]
pub struct AdcFilter {
    /// Must have an ADC of some kind.
    pub required: bool,
    pub res_12: bool,
    pub res_16: bool,
}

impl AdcFilter {
    pub fn is_active(&self) -> bool {
        self.required || self.res_12 || self.res_16
    }

    /// The types that satisfy it — all of them when only presence is asked for.
    fn wanted(&self) -> Vec<&'static str> {
        match (self.res_12, self.res_16) {
            (false, false) => ADC_TYPES.to_vec(),
            (a, b) => ADC_TYPES
                .iter()
                .copied()
                .zip([a, b])
                .filter_map(|(t, on)| on.then_some(t))
                .collect(),
        }
    }
}

/// Everything the user narrowed the catalogue by.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChipFilter {
    /// What the ranges below are measured against — see [`Bounds`].
    pub bounds: Bounds,
    pub flash_kb: (u32, u32),
    pub ram_kb: (u32, u32),
    pub mhz: (u32, u32),
    pub io: (u32, u32),
    /// Pins on the package — see [`Bounds::pins`].
    pub pins: (u32, u32),
    /// Package TYPES (`LQFP`, `UFBGA`); empty asks nothing. Types rather than
    /// full names, so one tick covers LQFP32 through LQFP208 — see
    /// [`split_package`].
    pub packages: BTreeSet<String>,
    /// Minimum count per [`TIER1`] id. An id at 0, or absent, asks nothing.
    pub counts: BTreeMap<&'static str, u16>,
    pub adc: AdcFilter,
    pub usb: bool,
    /// Tier-2 vendor types that must be present.
    pub present: BTreeSet<String>,
    /// Family keys (`stm32g4`); empty asks nothing.
    pub families: BTreeSet<String>,
    /// Core names as the vendor spells them (`Arm Cortex-M4`).
    pub cores: BTreeSet<String>,
}

impl Default for ChipFilter {
    fn default() -> Self {
        Self::new(Bounds::default())
    }
}

impl ChipFilter {
    /// An empty filter spanning `bounds` — one that excludes nothing.
    pub fn new(bounds: Bounds) -> Self {
        Self {
            bounds,
            flash_kb: bounds.flash_kb,
            ram_kb: bounds.ram_kb,
            mhz: bounds.mhz,
            io: bounds.io,
            pins: bounds.pins,
            packages: BTreeSet::new(),
            counts: BTreeMap::new(),
            adc: AdcFilter::default(),
            usb: false,
            present: BTreeSet::new(),
            families: BTreeSet::new(),
            cores: BTreeSet::new(),
        }
    }

    /// Re-span the ranges when the catalogue lands, keeping the filter empty.
    ///
    /// Called once, after indexing: the dialog can be drawn before the worker
    /// finishes, and a filter built against the placeholder bounds would then
    /// read as active on every range.
    pub fn rebound(&mut self, bounds: Bounds) {
        if self.bounds != bounds && !self.is_active() {
            *self = Self::new(bounds);
        } else {
            self.bounds = bounds;
        }
    }

    /// How many facets are narrowing the list.
    ///
    /// Shown on the collapsed header. Without it, a filter left on behind a
    /// closed twisty turns into "No chip matches that" for a part that is
    /// plainly there, with nothing on screen to explain it.
    pub fn active_count(&self) -> usize {
        let ranges = [
            (self.flash_kb, self.bounds.flash_kb),
            (self.ram_kb, self.bounds.ram_kb),
            (self.mhz, self.bounds.mhz),
            (self.io, self.bounds.io),
            (self.pins, self.bounds.pins),
        ];
        ranges.iter().filter(|(v, b)| v != b).count()
            + usize::from(!self.packages.is_empty())
            + self.counts.values().filter(|n| **n > 0).count()
            + usize::from(self.adc.is_active())
            + usize::from(self.usb)
            + self.present.len()
            + usize::from(!self.families.is_empty())
            + usize::from(!self.cores.is_empty())
    }

    pub fn is_active(&self) -> bool {
        self.active_count() > 0
    }

    /// Whether any active facet needs peripheral data to answer.
    fn needs_peripherals(&self) -> bool {
        self.counts.values().any(|n| *n > 0)
            || self.adc.is_active()
            || self.usb
            || !self.present.is_empty()
    }

    /// Judge one row.
    ///
    /// A definite `Fail` outranks an `Unknown`: a part that is too small is
    /// excluded for a reason the user can see, whatever else the source failed
    /// to say about it, and counting it among the "hidden, no data" rows would
    /// overstate how much the catalogue is missing.
    pub fn matches(&self, m: &Metrics) -> Verdict {
        let mut unknown = false;

        let mut range = |want: (u32, u32), bound: (u32, u32), got: Option<u32>| -> bool {
            if want == bound {
                return true; // inactive
            }
            match got {
                Some(v) => (want.0..=want.1).contains(&v),
                None => {
                    unknown = true;
                    true
                }
            }
        };
        // The package is one string answering two facets, so it is split once
        // and fed to both.
        let (pkg_ty, pkg_pins) = split_package(m.package);
        let ok = range(self.flash_kb, self.bounds.flash_kb, m.flash_kb)
            && range(self.ram_kb, self.bounds.ram_kb, m.ram_kb)
            && range(self.mhz, self.bounds.mhz, m.mhz)
            && range(self.io, self.bounds.io, m.io)
            && range(self.pins, self.bounds.pins, pkg_pins);
        if !ok {
            return Verdict::Fail;
        }

        if !self.packages.is_empty() {
            if pkg_ty.is_empty() {
                unknown = true;
            } else if !self.packages.contains(pkg_ty) {
                return Verdict::Fail;
            }
        }

        if !self.families.is_empty() && !self.families.contains(m.family) {
            return Verdict::Fail;
        }

        if !self.cores.is_empty() {
            if m.cores.is_empty() {
                unknown = true;
            } else if !m.cores.iter().any(|c| self.cores.contains(c)) {
                return Verdict::Fail;
            }
        }

        if self.needs_peripherals() {
            if m.peripherals.is_none() {
                unknown = true;
            } else {
                for f in TIER1 {
                    let want = self.counts.get(f.id).copied().unwrap_or(0);
                    if want > 0 && m.count_of(f.types).unwrap_or(0) < want {
                        return Verdict::Fail;
                    }
                }
                if self.adc.is_active() && m.has_any(&self.adc.wanted()) != Some(true) {
                    return Verdict::Fail;
                }
                if self.usb && m.has_any(USB_TYPES) != Some(true) {
                    return Verdict::Fail;
                }
                for ty in &self.present {
                    if m.has_any(&[ty.as_str()]) != Some(true) {
                        return Verdict::Fail;
                    }
                }
            }
        }

        if unknown {
            Verdict::Unknown
        } else {
            Verdict::Pass
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn part(name: &str, flash: u32, ram: u32, ps: &[(&str, u16)]) -> ChipEntry {
        packaged("LQFP48", name, flash, ram, ps)
    }

    fn packaged(pkg: &str, name: &str, flash: u32, ram: u32, ps: &[(&str, u16)]) -> ChipEntry {
        ChipEntry {
            ref_name: name.into(),
            file: name.into(),
            family: "stm32g4".into(),
            package: pkg.into(),
            cores: vec!["Arm Cortex-M4".into()],
            mhz: Some(170),
            flash_kb: Some(flash),
            ram_kb: Some(ram),
            io: Some(38),
            peripherals: {
                let mut v: Vec<(String, u16)> =
                    ps.iter().map(|(t, n)| ((*t).to_owned(), *n)).collect();
                v.sort();
                v
            },
        }
    }

    /// A row from a source with no index: a part number and nothing else.
    fn listed(name: &str) -> ChipEntry {
        ChipEntry {
            ref_name: name.into(),
            file: name.into(),
            family: "stm32g4".into(),
            ..Default::default()
        }
    }

    fn g431() -> ChipEntry {
        part(
            "STM32G431CBTx",
            128,
            32,
            &[
                ("USART", 3),
                ("LPUART", 1),
                ("SPI", 3),
                ("I2C", 3),
                ("Timer 16-bit", 9),
                ("Timer 32-bit", 1),
                ("ADC 12-bit", 17),
                ("COMP", 4),
                ("DAC 12-bit", 2),
                ("FDCAN", 1),
                ("USB Device", 1),
                ("CORDIC", 1),
            ],
        )
    }

    fn f411() -> ChipEntry {
        part(
            "STM32F411RETx",
            512,
            128,
            &[
                ("USART", 3),
                ("SPI", 5),
                ("I2C", 3),
                ("Timer 16-bit", 6),
                ("Timer 32-bit", 2),
                ("ADC 12-bit", 16),
                ("USB OTG_FS", 1),
                ("SDIO", 1),
            ],
        )
    }

    fn bounds() -> Bounds {
        Bounds::of(&[g431(), f411()])
    }

    fn verdict(f: &ChipFilter, e: &ChipEntry) -> Verdict {
        f.matches(&Metrics::of(e))
    }

    #[test]
    fn an_empty_filter_excludes_nothing() {
        let f = ChipFilter::new(bounds());
        assert_eq!(f.active_count(), 0);
        assert!(!f.is_active());
        assert_eq!(verdict(&f, &g431()), Verdict::Pass);
        assert_eq!(verdict(&f, &f411()), Verdict::Pass);
        // Even a row that knows nothing: with nothing asked, nothing is unknown.
        assert_eq!(verdict(&f, &listed("STM32H543CETx")), Verdict::Pass);
    }

    #[test]
    fn a_flash_range_keeps_only_what_fits() {
        let mut f = ChipFilter::new(bounds());
        f.flash_kb = (256, 4096);
        assert_eq!(f.active_count(), 1);
        assert_eq!(verdict(&f, &g431()), Verdict::Fail, "128K is below 256K");
        assert_eq!(verdict(&f, &f411()), Verdict::Pass);
    }

    /// The reason [`Verdict`] has three arms rather than two.
    #[test]
    fn a_row_with_no_data_is_unknown_not_failed() {
        let mut f = ChipFilter::new(bounds());
        f.flash_kb = (256, 4096);
        assert_eq!(verdict(&f, &listed("STM32H543CETx")), Verdict::Unknown);
    }

    /// Zero flash is a real value for an MP1, and must not read as "no data".
    #[test]
    fn a_genuine_zero_is_not_missing_data() {
        let mp1 = part("STM32MP157AAAx", 0, 512, &[("USART", 4), ("SPI", 6)]);
        let mut f = ChipFilter::new(Bounds {
            flash_kb: (0, 4096),
            ..bounds()
        });
        f.flash_kb = (64, 4096);
        assert_eq!(
            verdict(&f, &mp1),
            Verdict::Fail,
            "it HAS a flash size, and that size is not in range"
        );
    }

    /// The bug this field pair was split for: a quarter of the catalogue — the
    /// whole STM32C0 series among it — states no frequency, and used to be
    /// ruled out as "too slow" on a question the vendor never answered.
    #[test]
    fn a_part_that_states_no_frequency_is_unknown_not_slow() {
        let mut c0 = part("STM32C051C6Tx", 32, 12, &[("USART", 2)]);
        c0.mhz = None;

        let mut f = ChipFilter::new(Bounds {
            mhz: (24, 600),
            ..bounds()
        });
        f.mhz = (48, 600);
        assert_eq!(verdict(&f, &c0), Verdict::Unknown);
        // …while a part that DOES state one is judged on it.
        assert_eq!(verdict(&f, &g431()), Verdict::Pass, "170 MHz");
        let slow = part("STM32F103C8Tx", 64, 20, &[("USART", 3)]);
        let mut slow = slow;
        slow.mhz = Some(72);
        assert_eq!(verdict(&f, &slow), Verdict::Pass);
        slow.mhz = Some(24);
        assert_eq!(verdict(&f, &slow), Verdict::Fail, "genuinely too slow");
    }

    /// The other half of the same distinction, in the other direction.
    #[test]
    fn zero_flash_still_fails_while_absent_flash_is_unknown() {
        let mut mp1 = part("STM32MP157AAAx", 0, 708, &[("USART", 4)]);
        mp1.flash_kb = Some(0);
        let mut silent = mp1.clone();
        silent.flash_kb = None;

        let mut f = ChipFilter::new(Bounds {
            flash_kb: (0, 4096),
            ..bounds()
        });
        f.flash_kb = (64, 4096);
        assert_eq!(verdict(&f, &mp1), Verdict::Fail, "it HAS no flash");
        assert_eq!(verdict(&f, &silent), Verdict::Unknown, "we do not know");
    }

    /// A span must not be dragged to a value no part has.
    #[test]
    fn a_silent_field_does_not_vote_on_the_bounds() {
        let mut c0 = part("STM32C051C6Tx", 32, 12, &[("USART", 2)]);
        c0.mhz = None;
        let b = Bounds::of(&[c0, g431()]);
        assert_eq!(b.mhz, (170, 170), "not (0, 170)");
        assert_eq!(b.flash_kb, (32, 128), "both stated their flash");
    }

    #[test]
    fn timers_add_both_widths_together() {
        let mut f = ChipFilter::new(bounds());
        f.counts.insert("timers", 10);
        assert_eq!(verdict(&f, &g431()), Verdict::Pass, "9 + 1");
        assert_eq!(verdict(&f, &f411()), Verdict::Fail, "6 + 2");
    }

    #[test]
    fn a_count_is_a_minimum() {
        let mut f = ChipFilter::new(bounds());
        f.counts.insert("spi", 4);
        assert_eq!(verdict(&f, &g431()), Verdict::Fail, "3 SPI");
        assert_eq!(verdict(&f, &f411()), Verdict::Pass, "5 SPI");
    }

    /// A part with none of a peripheral fails a count, rather than being exempt.
    #[test]
    fn a_missing_peripheral_fails_a_count() {
        let mut f = ChipFilter::new(bounds());
        f.counts.insert("lpuart", 1);
        assert_eq!(verdict(&f, &g431()), Verdict::Pass);
        assert_eq!(verdict(&f, &f411()), Verdict::Fail, "F411 has no LPUART");
    }

    /// Five vendor spellings, one question.
    #[test]
    fn usb_matches_whatever_the_vendor_calls_it() {
        let mut f = ChipFilter::new(bounds());
        f.usb = true;
        assert_eq!(verdict(&f, &g431()), Verdict::Pass, "USB Device");
        assert_eq!(verdict(&f, &f411()), Verdict::Pass, "USB OTG_FS");
        let none = part("STM32F103C8Tx", 64, 20, &[("USART", 3)]);
        assert_eq!(verdict(&f, &none), Verdict::Fail);
    }

    #[test]
    fn adc_is_presence_and_resolution_never_a_count() {
        let h743 = part("STM32H743ZITx", 2048, 1024, &[("ADC 16-bit", 28)]);

        let mut any = ChipFilter::new(bounds());
        any.adc.required = true;
        assert_eq!(verdict(&any, &g431()), Verdict::Pass);
        assert_eq!(verdict(&any, &h743), Verdict::Pass);

        let mut only16 = ChipFilter::new(bounds());
        only16.adc.res_16 = true;
        assert_eq!(verdict(&only16, &g431()), Verdict::Fail, "12-bit only");
        assert_eq!(verdict(&only16, &h743), Verdict::Pass);
    }

    /// 69 catalogued parts carry both resolutions, so ticking both is an OR.
    #[test]
    fn two_resolutions_are_an_or_not_an_and() {
        let mut f = ChipFilter::new(bounds());
        f.adc.res_12 = true;
        f.adc.res_16 = true;
        assert_eq!(verdict(&f, &g431()), Verdict::Pass, "12-bit alone suffices");
        let both = part(
            "STM32H7S3L8Hx",
            64,
            620,
            &[("ADC 12-bit", 8), ("ADC 16-bit", 20)],
        );
        assert_eq!(verdict(&f, &both), Verdict::Pass);
    }

    #[test]
    fn a_definite_fail_outranks_a_missing_field() {
        // Active on both a range the row cannot answer and a family it can.
        let mut f = ChipFilter::new(bounds());
        f.flash_kb = (256, 4096);
        f.families.insert("stm32f4".to_owned());
        assert_eq!(
            verdict(&f, &listed("STM32G431CBTx")),
            Verdict::Fail,
            "the family is known and wrong — not merely unanswerable"
        );
    }

    /// Dual-core parts must match on EITHER core.
    #[test]
    fn a_core_filter_reads_every_core() {
        let mut h7 = part("STM32H747XIHx", 2048, 1024, &[("USART", 4)]);
        h7.cores = vec!["Arm Cortex-M7".into(), "Arm Cortex-M4".into()];

        let mut f = ChipFilter::new(bounds());
        f.cores.insert("Arm Cortex-M4".to_owned());
        assert_eq!(verdict(&f, &h7), Verdict::Pass, "its SECOND core");
        assert_eq!(verdict(&f, &g431()), Verdict::Pass);

        let mut m33 = ChipFilter::new(bounds());
        m33.cores.insert("Arm Cortex-M33".to_owned());
        assert_eq!(verdict(&m33, &h7), Verdict::Fail);
    }

    #[test]
    fn tier_two_is_whatever_is_left_over() {
        let facets = presence_facets(&[g431(), f411()]);
        let names: Vec<&str> = facets.iter().map(|(t, _)| t.as_str()).collect();
        assert!(names.contains(&"CORDIC"));
        assert!(names.contains(&"SDIO"));
        for claimed in ["USART", "SPI", "Timer 16-bit", "ADC 12-bit", "USB Device"] {
            assert!(
                !names.contains(&claimed),
                "{claimed} already has a named facet"
            );
        }
    }

    #[test]
    fn bounds_ignore_rows_that_know_nothing() {
        let b = Bounds::of(&[g431(), f411(), listed("STM32H543CETx")]);
        assert_eq!(b.flash_kb, (128, 512), "not (0, 512)");
        assert_eq!(b.ram_kb, (32, 128));
    }

    /// Re-spanning must not turn an untouched filter into an active one.
    #[test]
    fn rebound_keeps_an_empty_filter_empty() {
        let mut f = ChipFilter::default();
        f.rebound(bounds());
        assert!(!f.is_active());
        assert_eq!(f.flash_kb, bounds().flash_kb);
    }

    /// …and must not silently discard one the user has already set.
    #[test]
    fn rebound_keeps_a_set_filter() {
        let mut f = ChipFilter::default();
        f.counts.insert("spi", 4);
        f.rebound(bounds());
        assert_eq!(f.counts.get("spi"), Some(&4));
        assert_eq!(f.bounds, bounds());
    }

    #[test]
    fn memory_x_sizes_parse_back_to_kilobytes() {
        assert_eq!(parse_memory_kb("128K"), Some(128));
        assert_eq!(parse_memory_kb(" 20K "), Some(20));
        assert_eq!(parse_memory_kb("2M"), Some(2048));
        assert_eq!(parse_memory_kb("0x10000"), Some(64));
        assert_eq!(parse_memory_kb("65536"), Some(64));
        // An ESP definition leaves these blank; that is not zero.
        assert_eq!(parse_memory_kb(""), None);
        assert_eq!(parse_memory_kb("lots"), None);
    }

    #[test]
    fn owned_metrics_judge_the_same_as_borrowed_ones() {
        let e = g431();
        let owned = RowMetrics::of(&e);
        let mut f = ChipFilter::new(bounds());
        f.counts.insert("timers", 10);
        assert_eq!(f.matches(&owned.view(&e.family)), Verdict::Pass);
        assert_eq!(f.matches(&Metrics::of(&e)), Verdict::Pass);
        assert!(!owned.is_empty());
        assert!(RowMetrics::default().is_empty());
    }

    /// What ONE SEARCH of the chip picker costs. Ignored: needs real data.
    ///
    /// This used to be a frame budget: the search ran on every frame, and
    /// browsing cost ~100 ms of it, which is what a dragged slider felt like.
    /// Two things changed that - the dedup is a map instead of a linear scan of
    /// the growing hit list, and a row is judged before it is built - and the
    /// UI now caches the result and commits the filter on a button. So this is
    /// the cost of a KEYSTROKE or a CLICK, not of a frame.
    ///
    /// The numbers still matter: typing runs one of these per character.
    ///
    /// `cargo test -- --ignored searching_the_real_catalogue_is_cheap --nocapture`
    #[test]
    #[ignore]
    fn searching_the_real_catalogue_is_cheap() {
        use super::super::chip_search::Catalogue;
        use super::super::chip_sources;

        let cat = Catalogue::build(chip_sources::all_sources());
        if cat.is_empty() {
            eprintln!("no chip data on this machine - skipping");
            return;
        }
        let bounds = Bounds::of(cat.entries());
        let time = |label: &str, q: &str, f: &ChipFilter| {
            // One warm-up, then the median of five.
            let _ = cat.search(q, &[], f, 40);
            let mut runs: Vec<u128> = (0..5)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let r = cat.search(q, &[], f, 40);
                    let us = t.elapsed().as_micros();
                    std::hint::black_box(r);
                    us
                })
                .collect();
            runs.sort_unstable();
            println!("{label:<34} {:>7} us (median of 5)", runs[2]);
            runs[2]
        };

        let idle = ChipFilter::new(bounds);
        time("empty query, no filter", "", &idle);
        time("typing 'f1'", "f1", &idle);

        let mut broad = ChipFilter::new(bounds);
        broad.counts.insert("usart", 1);
        let browse = time("browse: >=1 USART (worst case)", "", &broad);

        let mut narrow = ChipFilter::new(bounds);
        narrow.families.insert("stm32g4".to_owned());
        narrow.counts.insert("spi", 3);
        let narrow_us = time("browse: G4 + >=3 SPI", "", &narrow);

        // Typing is the one path with no button in front of it, so it is the
        // one with a real budget.
        let typing = time("typing 'stm32g4'", "stm32g4", &idle);
        assert!(typing < 16_000, "a keystroke costs {typing} us");
        assert!(
            narrow_us < 16_000,
            "applying a narrow filter costs {narrow_us} us"
        );
        // The worst case is every part passing, where the work is building the
        // rows rather than judging them. Behind a button, that is a click.
        assert!(
            browse < 200_000,
            "applying a broad filter costs {browse} us"
        );
    }

    /// Against the real vendor data on this machine. Ignored: needs a CubeMX
    /// install.
    ///
    /// Substring assertions over a trimmed fixture cannot catch a `families.xml`
    /// whose shape differs from the one in the test — and every claim this
    /// module documents (ADC counts are channels, timers come in two types, some
    /// parts are dual-core) is a claim ABOUT that file.
    ///
    /// `cargo test -- --ignored the_real_catalogue_supports_the_filter`
    #[test]
    #[ignore]
    fn the_real_catalogue_supports_the_filter() {
        use super::super::chip_search::Catalogue;
        use super::super::chip_sources;

        let cat = Catalogue::build(chip_sources::all_sources());
        if cat.is_empty() {
            eprintln!("no chip data on this machine - skipping");
            return;
        }
        let indexed: Vec<&ChipEntry> = cat.entries().filter(|e| e.knows_peripherals()).collect();
        println!("{} parts, {} of them indexed", cat.len(), indexed.len());
        assert!(indexed.len() > 2000, "expected a full CubeMX catalogue");

        let find = |n: &str| {
            indexed
                .iter()
                .copied()
                .find(|e| e.ref_name == n)
                .unwrap_or_else(|| panic!("{n} not catalogued"))
        };

        // The documented trap, straight from the vendor file: the F411 has ONE
        // ADC and the index says 16, because that is a channel count.
        let f411 = find("STM32F411RETx");
        assert_eq!(f411.count_of("ADC 12-bit"), 16);
        assert_eq!(
            f411.count_of("USART"),
            3,
            "…while USART really is instances"
        );
        assert_eq!(f411.flash_kb, Some(512));

        // Two types, and adding them is the only useful reading.
        let g431 = find("STM32G431CBTx");
        assert_eq!(
            (g431.count_of("Timer 16-bit"), g431.count_of("Timer 32-bit")),
            (9, 1)
        );

        // Dual-core really is in there.
        let dual = indexed.iter().filter(|e| e.cores.len() > 1).count();
        assert!(
            dual > 100,
            "only {dual} dual-core parts - parser regressed?"
        );

        // The long tail exists and is worth a section of its own.
        let tail = presence_facets(cat.entries());
        println!("{} Tier-2 peripheral types", tail.len());
        assert!(tail.len() > 50, "only {} types", tail.len());

        // What the Core chips row actually offers, and how many parts each
        // covers - including the dual-core ones, which appear under BOTH.
        let mut by_core: BTreeMap<&str, usize> = BTreeMap::new();
        for e in &indexed {
            for c in &e.cores {
                *by_core.entry(c.as_str()).or_default() += 1;
            }
        }
        let mut ranked: Vec<_> = by_core.into_iter().collect();
        ranked.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        println!("cores offered: {ranked:?}");
        assert!(ranked.len() >= 8, "only {} cores", ranked.len());

        // A core filter must find the M4 half of a dual-core H7.
        let mut m4 = ChipFilter::new(Bounds::of(cat.entries()));
        m4.cores.insert("Arm Cortex-M4".to_owned());
        let dual_m4 = indexed
            .iter()
            .filter(|e| e.cores.len() > 1 && e.cores.contains(&"Arm Cortex-M4".to_owned()))
            .count();
        println!("dual-core parts whose SECOND core is an M4: {dual_m4}");
        assert!(dual_m4 > 0);
        let some_dual = indexed
            .iter()
            .find(|e| e.cores.len() > 1 && e.cores.contains(&"Arm Cortex-M4".to_owned()))
            .unwrap();
        assert_eq!(
            m4.matches(&Metrics::of(some_dual)),
            Verdict::Pass,
            "{} is {:?}",
            some_dual.ref_name,
            some_dual.cores
        );

        // The 764-part bug, measured against the real index rather than a
        // fixture: a frequency filter must set these aside, not rule them out.
        let silent = indexed.iter().filter(|e| e.mhz.is_none()).count();
        let zero_flash = indexed.iter().filter(|e| e.flash_kb == Some(0)).count();
        println!("parts stating no frequency: {silent}; stating 0 flash: {zero_flash}");
        assert!(silent > 500, "expected the C0-class gap, got {silent}");
        assert!(
            zero_flash > 100,
            "expected the MP1/N6 zeroes, got {zero_flash}"
        );

        let mut fast = ChipFilter::new(Bounds::of(cat.entries()));
        fast.mhz = (100, fast.bounds.mhz.1);
        let counted = indexed.iter().fold((0, 0, 0), |(p, f_, u), e| {
            match fast.matches(&Metrics::of(e)) {
                Verdict::Pass => (p + 1, f_, u),
                Verdict::Fail => (p, f_ + 1, u),
                Verdict::Unknown => (p, f_, u + 1),
            }
        });
        println!(
            ">=100 MHz -> {} pass, {} fail, {} unknown",
            counted.0, counted.1, counted.2
        );
        assert_eq!(
            counted.2, silent,
            "every silent part, and only those, must land in Unknown"
        );
        // And the span itself must start where a part actually lives.
        assert!(
            fast.bounds.mhz.0 >= 24,
            "the frequency slider starts at {} MHz",
            fast.bounds.mhz.0
        );

        // 133 vendor names, but only a handful of shapes - and every one of
        // them must split, or a package that fails to parse becomes a package
        // nothing can filter for.
        let pkgs = package_types(cat.entries());
        println!(
            "{} package types: {:?}",
            pkgs.len(),
            &pkgs[..pkgs.len().min(6)]
        );
        assert!(pkgs.len() >= 10 && pkgs.len() < 30, "{} types", pkgs.len());
        let unsplittable: Vec<&str> = indexed
            .iter()
            .map(|e| e.package.as_str())
            .filter(|p| !p.is_empty() && split_package(p).1.is_none())
            .collect();
        assert!(
            unsplittable.is_empty(),
            "packages with no readable pin count: {unsplittable:?}"
        );
        assert_eq!(split_package(&f411.package), ("LQFP", Some(64)));

        // And the whole point: browsing by capability, with no query at all.
        let bounds = Bounds::of(cat.entries());
        println!("bounds: {bounds:?}");
        let mut f = ChipFilter::new(bounds);
        f.families.insert("stm32g4".to_owned());
        f.counts.insert("spi", 3);
        f.flash_kb = (256, bounds.flash_kb.1);
        f.adc.required = true;
        f.packages.insert("LQFP".to_owned());
        f.pins = (32, 64);
        let r = cat.search("", &[], &f, 40);
        println!(
            "G4 / >=3 SPI / >=256K / ADC / LQFP32-64 -> {} matches, {} unknown",
            r.total, r.unknown
        );
        assert!(r.total > 0, "that combination matches nothing");
        for h in &r.hits {
            assert_eq!(h.family, "stm32g4");
            assert!(h.metrics.flash_kb.unwrap() >= 256, "{}", h.name);
            let (ty, pins) = split_package(&h.metrics.package);
            assert_eq!(ty, "LQFP", "{} is a {}", h.name, h.metrics.package);
            assert!((32..=64).contains(&pins.unwrap()), "{}", h.name);
        }
        // An empty query with NO filter must still return nothing.
        let none = cat.search("", &[], &ChipFilter::new(bounds), 40);
        assert!(none.hits.is_empty(), "browsing without a filter");

        // The Unknown path, on real data. Every CubeMX mirror ships an index,
        // so the only way to reach it is a source that does not - which is
        // exactly what an open-pin-data checkout is.
        //
        // `EIDE_PIN_DATA=F:\...\STM32_open_pin_data-master\mcu cargo test …`
        let Some(pin_data) = std::env::var_os("EIDE_PIN_DATA") else {
            println!("EIDE_PIN_DATA not set - the Unknown path stays unit-tested only");
            return;
        };
        let src =
            chip_sources::from_path(std::path::Path::new(&pin_data)).expect("not a chip source");
        let bare = Catalogue::build(vec![src]);
        assert!(
            bare.entries().all(|e| !e.knows_peripherals()),
            "a listing-only source must claim no peripheral data"
        );
        let mut only_big = ChipFilter::new(bounds);
        only_big.flash_kb = (256, bounds.flash_kb.1);
        let r = bare.search("stm32", &[], &only_big, 40);
        println!("pin-data alone -> {} shown, {} unknown", r.total, r.unknown);
        assert_eq!(r.total, 0, "nothing can be shown to fit");
        assert!(
            r.unknown > 1000,
            "…and the count says so instead of the source vanishing: {}",
            r.unknown
        );
    }

    /// The 133 vendor names are one shape plus 30 suffixes, and the suffixes
    /// are the reason this is parsed rather than matched whole. Every awkward
    /// case below is a real name from `families.xml`.
    #[test]
    fn a_package_name_splits_into_type_and_pins() {
        for (name, ty, pins) in [
            ("LQFP48", "LQFP", Some(48)),
            ("UFQFPN32", "UFQFPN", Some(32)),
            ("TFBGA361", "TFBGA", Some(361)),
            ("WLCSP49", "WLCSP", Some(49)),
            // Suffixes, in all four spellings ST uses.
            ("LQFP48_N", "LQFP", Some(48)),
            ("UFQFPN48_SMPS_USB", "UFQFPN", Some(48)),
            ("TFBGA225 HEXA SMPS", "TFBGA", Some(225)),
            ("VFBGA169GP", "VFBGA", Some(169)),
            ("SO8N", "SO", Some(8)),
            // The extra balls are not the footprint you order by.
            ("UFBGA176+25", "UFBGA", Some(176)),
            // A listing-only row has none.
            ("", "", None),
        ] {
            assert_eq!(split_package(name), (ty, pins), "for {name:?}");
        }
    }

    #[test]
    fn a_package_type_covers_every_size_of_it() {
        let small = packaged("LQFP48", "STM32F103C8Tx", 64, 20, &[("USART", 3)]);
        let big = packaged("LQFP144", "STM32F103ZETx", 512, 64, &[("USART", 5)]);
        let bga = packaged("UFBGA100", "STM32F411VCHx", 256, 128, &[("USART", 3)]);

        let mut f = ChipFilter::new(Bounds::of(&[small.clone(), big.clone(), bga.clone()]));
        f.packages.insert("LQFP".to_owned());
        assert_eq!(verdict(&f, &small), Verdict::Pass);
        assert_eq!(verdict(&f, &big), Verdict::Pass, "one tick, every size");
        assert_eq!(verdict(&f, &bga), Verdict::Fail);
        assert_eq!(f.active_count(), 1);
    }

    /// Package pins and I/O pins are different numbers, and the filter must not
    /// quietly treat one as the other.
    #[test]
    fn package_pins_are_not_io_pins() {
        let c8 = packaged("LQFP48", "STM32F103C8Tx", 64, 20, &[("USART", 3)]);
        let b = Bounds::of(&[c8.clone()]);
        assert_eq!(b.pins, (48, 48));
        assert_eq!(b.io, (38, 38), "the fixture's IONb, not its package");
    }

    #[test]
    fn a_pin_range_keeps_only_what_fits() {
        let small = packaged("UFQFPN32", "STM32G031K8Tx", 64, 8, &[("USART", 2)]);
        let big = packaged("LQFP144", "STM32F103ZETx", 512, 64, &[("USART", 5)]);
        let mut f = ChipFilter::new(Bounds::of(&[small.clone(), big.clone()]));
        f.pins = (64, 448);
        assert_eq!(verdict(&f, &small), Verdict::Fail);
        assert_eq!(verdict(&f, &big), Verdict::Pass);
    }

    /// A row with no package cannot answer either facet.
    #[test]
    fn a_row_with_no_package_is_unknown() {
        let mut f = ChipFilter::new(bounds());
        f.packages.insert("LQFP".to_owned());
        assert_eq!(verdict(&f, &listed("STM32H543CETx")), Verdict::Unknown);
    }

    #[test]
    fn package_types_are_derived_and_ranked() {
        let rows = [
            packaged("LQFP48", "a", 64, 20, &[("SPI", 1)]),
            packaged("LQFP64", "b", 64, 20, &[("SPI", 1)]),
            packaged("UFBGA100", "c", 64, 20, &[("SPI", 1)]),
        ];
        assert_eq!(
            package_types(&rows),
            [("LQFP".to_owned(), 2), ("UFBGA".to_owned(), 1)],
            "commonest first"
        );
    }

    #[test]
    fn every_tier1_max_is_reachable() {
        // Guards the spinner ranges against a typo that would make a facet
        // unable to express a value the catalogue actually contains.
        for f in TIER1 {
            assert!(f.max > 0, "{} has no range", f.id);
            assert!(!f.types.is_empty(), "{} filters on nothing", f.id);
        }
        assert_eq!(TIER1.len(), 10);
    }
}
