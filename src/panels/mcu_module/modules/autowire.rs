//! Auto-wiring: pick compatible MCU pins for a new module.
//!
//! Finds a peripheral instance whose required signals all map to distinct
//! free/assigned pins (optional signals added when available), and — among every
//! such wiring — picks the one that reads best on the diagram: pins the user
//! already configured, then pins that stay on one GPIO port and one side of the
//! chip, close together.
//!
//! That ranking is the point. A first-fit search (the earlier version) returned
//! whatever the pin order happened to produce, which on an STM32F103 meant
//! SPI1 with SCK/MISO on the remap pins (PB3/PB4, top edge) and MOSI on the
//! default one (PA7, bottom edge) whenever PB5 was already taken — a wire across
//! the whole chip, and a pin set the hardware cannot even form (the F1 SPI1
//! remap bit moves all four signals together). SPI2 sat free on PB12..PB15 the
//! whole time. Scoring the candidates picks SPI2 there and keeps the scattered
//! set as the last resort.

use std::collections::{HashMap, HashSet};

use super::ModuleSignal;
use crate::panels::mcu_module::mcu::model::Mcu;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

/// Candidate pins the AUTOMATIC search considers per signal, in chip order.
///
/// A bound on the search, not on the chip - and it is deliberately left at 8
/// even though a GPIO-matrix part offers far more (an ESP32-C3 has 21 pads for
/// `UsartTx(0)`). Raising it alone would make the search WORSE, not better:
/// [`MAX_COMBOS`] is 512 = 8x8x8, so a three-signal bus is explored
/// EXHAUSTIVELY inside this window today, while 21 candidates against the same
/// budget would let `walk` finish only the first pad of the first signal and
/// call that the ranking. Raising both turns a per-frame call
/// (`can_add_module` runs this for every palette entry, every frame) into
/// something 18x larger.
///
/// The pads outside the window are not lost - they are simply not the
/// AUTOMATIC choice. [`eligible_for`] is uncapped, so every picker in the UI
/// offers all of them; that is where "the pad I actually want" belongs.
const MAX_PER_SIGNAL: usize = 8;
/// Upper bound on the wirings explored per instance.
///
/// A backstop rather than a working limit: measured over the bundled chips, the
/// worst case (an ESP32-S3 I2S) walks 672 combinations summed over all eighteen
/// instances, and no chip reaches this cap on even one of them.
const MAX_COMBOS: usize = 512;
/// Highest peripheral instance number tried.
///
/// Buses stop at 3-6, but a TIMER module's "instance" is the timer number and
/// STM32s go up to TIM17. Scanning the gaps is free: an instance the chip does
/// not have yields an empty candidate list and is skipped before any work.
const MAX_INSTANCE: u8 = 17;

/// How good one candidate wiring is — **lower is better on every field**, and
/// the fields are compared in declaration order (that is what `derive(Ord)`
/// gives us), so this is the ranking, top priority first:
///
/// 1. `unassigned` — pins the user already set to this exact function are kept;
///    this is what the old two-pass `require_assigned` search expressed, only
///    finer (it can now prefer 3-of-4 reused over 0-of-4).
/// 2. `ports` — signals spread over two GPIO ports usually means mixing a
///    peripheral's default pins with its remap pins, which on STM32F1 is not a
///    legal combination at all.
/// 3. `sides` — pins on opposite edges of the chip mean wires across the body.
/// 4. `spread` — how far apart the pins sit, so a compact block wins.
/// 5. `board_use` — a pad the board already spends on something else (its own
///    NAME says so). Second-to-last on purpose: it settles TIES and must never
///    outrank the port/side rules, which stand in for what the silicon can form.
/// 6. `instance` — pure tie-break, keeping the old "lowest instance" order.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Score {
    unassigned: usize,
    ports: usize,
    sides: usize,
    spread: usize,
    board_use: usize,
    instance: u8,
}

/// Whether a pin's own NAME says the board already spends it on something.
///
/// The definitions annotate exactly those pads: `GP25 (on-board LED)`,
/// `GP24 (VBUS sense)`, `PB3 (JTDO-TRACESWO)`. Ranked next-to-last on purpose -
/// it must never override the port/side rules, which stand in for what the
/// silicon can actually form. It only settles TIES, and that is where it
/// matters: on a Pico the second "+ USART" scored dead level across four
/// wirings and took GP24/GP25 - the VBUS sense and the on-board LED - purely
/// because those two pads are declared first in the chip file.
fn is_board_used(name: &str) -> bool {
    name.contains('(')
}

/// The GPIO port a pin name belongs to — the leading letters (`PB5` → `PB`,
/// `PC15-OSC32_OUT` → `PC`). Chips whose pins aren't named that way (`GPIO2`)
/// simply end up in one bucket, which costs nothing: the criterion then just
/// doesn't discriminate.
fn port_of(name: &str) -> &str {
    let end = name
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(name.len());
    &name[..end]
}

/// pin number → which side of the chip it sits on.
fn side_map(mcu: &Mcu) -> HashMap<usize, u8> {
    let mut m = HashMap::new();
    for (side, pins) in [
        (0u8, &mcu.top_pins),
        (1, &mcu.bottom_pins),
        (2, &mcu.left_pins),
        (3, &mcu.right_pins),
    ] {
        for p in pins {
            m.insert(p.number, side);
        }
    }
    m
}

/// EVERY pin that could carry `want`: free (or already set to exactly this
/// function) and not taken by another module or an earlier signal.
///
/// Uncapped, and `pub(crate)` for that reason: this is what the pin pickers in
/// the panel enumerate, and a picker that offered 8 of a C3's 21 pads would
/// look exactly as arbitrary as the automatic choice the user is complaining
/// about. The search applies [`MAX_PER_SIGNAL`] itself, where the budget it has
/// to live within is known.
pub(crate) fn eligible_for(mcu: &Mcu, taken: &HashSet<usize>, want: &PinFunction) -> Vec<usize> {
    mcu.iter_all_pins()
        .filter(|p| !p.reserved && !taken.contains(&p.number))
        .filter(|p| {
            p.available_functions.contains(want)
                && (p.selected_function == *want || p.selected_function == PinFunction::Unset)
        })
        .map(|p| p.number)
        .collect()
}

/// [`eligible_for`], addressed the way a module names its signals.
pub(crate) fn eligible(
    mcu: &Mcu,
    taken: &HashSet<usize>,
    sig: ModuleSignal,
    inst: u8,
) -> Vec<usize> {
    eligible_for(mcu, taken, &sig.pin_function(inst))
}

/// [`eligible`], trimmed to what the automatic search can afford to explore.
///
/// Stops AT the cap rather than collecting the chip and truncating. `pick_pins`
/// asks this thousands of times per instance, for every palette entry, on every
/// frame the menu is open - building a forty-element Vec each time to keep
/// eight of it was measurably a third of that frame.
fn eligible_capped(mcu: &Mcu, taken: &HashSet<usize>, sig: ModuleSignal, inst: u8) -> Vec<usize> {
    eligible_for_limited(mcu, taken, &sig.pin_function(inst), MAX_PER_SIGNAL)
}

/// [`eligible_for`] that stops after `limit` pads.
fn eligible_for_limited(
    mcu: &Mcu,
    taken: &HashSet<usize>,
    want: &PinFunction,
    limit: usize,
) -> Vec<usize> {
    mcu.iter_all_pins()
        .filter(|p| !p.reserved && !taken.contains(&p.number))
        .filter(|p| {
            p.available_functions.contains(want)
                && (p.selected_function == *want || p.selected_function == PinFunction::Unset)
        })
        .map(|p| p.number)
        .take(limit)
        .collect()
}

/// What ranking needs to know about one pad.
struct PinFact<'a> {
    /// The GPIO port letters, as [`port_of`] reads them.
    port: &'a str,
    /// Which edge of the chip, or `None` for a ball in the grid.
    side: Option<u8>,
    /// The pad's own name says the board already spends it - see
    /// [`is_board_used`].
    board_used: bool,
    /// What the pad currently carries.
    func: &'a PinFunction,
}

/// The chip, read ONCE per search.
///
/// Ranking used to ask `Mcu::find_pin` for every pad of every candidate wiring,
/// and that is a LINEAR SCAN over the whole chip through a five-way iterator
/// chain (`iter_all_pins`) - twenty-four thousand calls and two hundred
/// thousand pin visits for the worst bundled case, an ESP32-S3 I2S.
///
/// Worth removing, but it is NOT where the time went: measured, substituting a
/// map for `find_pin` and changing nothing else is worth about 8%. The search
/// spent its time re-deriving the optional signals' candidate lists inside the
/// combination loop, and re-scoring the whole set for every candidate of them -
/// see [`ScoreAcc`] and the `opt_pads` hoist. Said plainly because the wrong
/// attribution would send the next reader at the wrong thing.
struct PinFacts<'a> {
    by_num: HashMap<usize, PinFact<'a>>,
}

impl<'a> PinFacts<'a> {
    fn of(mcu: &'a Mcu) -> Self {
        let sides = side_map(mcu);
        Self {
            by_num: mcu
                .iter_all_pins()
                .map(|p| {
                    (
                        p.number,
                        PinFact {
                            port: port_of(&p.name),
                            side: sides.get(&p.number).copied(),
                            board_used: is_board_used(&p.name),
                            func: &p.selected_function,
                        },
                    )
                })
                .collect(),
        }
    }
}

/// A [`Score`] under construction, so a set can be ranked once and then asked
/// what ONE more pad would make of it.
///
/// Every term folds: three are sums, one is a bitmask union, two are a running
/// min and max, and the port count is a set whose SIZE is all that is read. So
/// "the score of these pads plus that one" needs no second pass over the pads -
/// which is what the optional-signal loop was doing, once per candidate, five
/// hundred combinations deep.
#[derive(Clone)]
struct ScoreAcc<'a> {
    unassigned: usize,
    ports: Vec<&'a str>,
    side_bits: u8,
    board_use: usize,
    lo: usize,
    hi: usize,
}

impl<'a> ScoreAcc<'a> {
    fn new() -> Self {
        Self {
            unassigned: 0,
            ports: Vec::new(),
            side_bits: 0,
            board_use: 0,
            lo: usize::MAX,
            hi: 0,
        }
    }

    fn add(&mut self, facts: &'a PinFacts<'a>, want: &PinFunction, num: usize) {
        if let Some(f) = facts.by_num.get(&num) {
            if *f.func != *want {
                self.unassigned += 1;
            }
            if !self.ports.contains(&f.port) {
                self.ports.push(f.port);
            }
            if f.board_used {
                self.board_use += 1;
            }
            if let Some(s) = f.side {
                self.side_bits |= 1 << s;
            }
        }
        self.lo = self.lo.min(num);
        self.hi = self.hi.max(num);
    }

    fn finish(&self, inst: u8) -> Score {
        Score {
            unassigned: self.unassigned,
            ports: self.ports.len(),
            sides: self.side_bits.count_ones() as usize,
            spread: self.hi.saturating_sub(self.lo),
            board_use: self.board_use,
            instance: inst,
        }
    }

    /// What this set WOULD score with one more pad on it - without touching the
    /// accumulator, and without a second pass or an allocation.
    fn with(&self, facts: &PinFacts<'_>, inst: u8, want: &PinFunction, num: usize) -> Score {
        let mut unassigned = self.unassigned;
        let mut ports = self.ports.len();
        let mut side_bits = self.side_bits;
        let mut board_use = self.board_use;
        if let Some(f) = facts.by_num.get(&num) {
            if *f.func != *want {
                unassigned += 1;
            }
            if !self.ports.contains(&f.port) {
                ports += 1;
            }
            if f.board_used {
                board_use += 1;
            }
            if let Some(s) = f.side {
                side_bits |= 1 << s;
            }
        }
        Score {
            unassigned,
            ports,
            sides: side_bits.count_ones() as usize,
            spread: self.hi.max(num).saturating_sub(self.lo.min(num)),
            board_use,
            instance: inst,
        }
    }
}

/// Rank a candidate set, from the facts rather than from the chip.
///
/// Takes the pads as an ITERATOR so a caller naming them by module signal does
/// not have to materialise a `Vec` of `(PinFunction, usize)` per candidate -
/// which, at one allocation per wiring per instance, was the second cost after
/// `find_pin`.
///
/// `ports` and `sides` are counted without a `HashSet` each: a wiring has at
/// most a handful of pads, so a four-slot linear scan beats hashing, and a side
/// is one of four values, which is a bitmask.
fn score_pads(
    facts: &PinFacts,
    inst: u8,
    pads: impl Iterator<Item = (PinFunction, usize)>,
) -> Score {
    let mut acc = ScoreAcc::new();
    for (want, num) in pads {
        acc.add(facts, &want, num);
    }
    acc.finish(inst)
}

/// Rank a candidate set of `(function the pin is wanted for, pin number)`.
///
/// It is keyed on the FUNCTION rather than on a module signal so the same
/// ranking serves both callers: a whole module's wiring, and the partners of a
/// single pin the user assigned by hand.
fn score(facts: &PinFacts, inst: u8, chosen: &[(PinFunction, usize)]) -> Score {
    score_pads(facts, inst, chosen.iter().cloned())
}

/// All distinct-pin combinations of `lists` (one pin per required signal), up to
/// `cap`. The lists are tiny (a signal has two or three candidate pins), so a
/// plain backtracking walk is both exhaustive and cheap — and unlike the greedy
/// first-fit it can't paint itself into a corner by handing signal 1 the only
/// pin signal 3 could have used.
fn walk(
    lists: &[Vec<usize>],
    idx: usize,
    taken: &mut Vec<usize>,
    out: &mut Vec<Vec<usize>>,
    cap: usize,
) {
    if out.len() >= cap {
        return;
    }
    if idx == lists.len() {
        out.push(taken.clone());
        return;
    }
    for &pin in &lists[idx] {
        if taken.contains(&pin) {
            continue;
        }
        taken.push(pin);
        walk(lists, idx + 1, taken, out, cap);
        taken.pop();
        if out.len() >= cap {
            return;
        }
    }
}

/// Choose `(instance, [(signal, pin_number), …])` for a new module. `required`
/// signals must all be satisfiable on the same instance with distinct pins;
/// `optional` ones (e.g. SPI NSS) are added when a pin is available. Pins in
/// `used` (wired to another module) are skipped, and peripheral instances in
/// `used_instances` (already hosting a module of this kind) are skipped entirely
/// — otherwise a 2nd "+_SPI" would re-pick SPI1 on its alternate pins instead of
/// moving to SPI2. Every instance is tried and the best-[`Score`]d wiring wins.
pub fn pick_pins(
    mcu: &Mcu,
    used: &HashSet<usize>,
    used_instances: &HashSet<u8>,
    required: &[ModuleSignal],
    optional: &[ModuleSignal],
) -> Option<(u8, Vec<(ModuleSignal, usize)>)> {
    let facts = PinFacts::of(mcu);
    let mut best: Option<(Score, u8, Vec<(ModuleSignal, usize)>)> = None;
    // Reused across combinations instead of reallocated per candidate.
    let mut chosen: Vec<(ModuleSignal, usize)> = Vec::new();

    for inst in 0u8..=MAX_INSTANCE {
        if used_instances.contains(&inst) {
            continue;
        }
        // Built with an early exit rather than `map().collect()`: an instance
        // the chip does not have fails on its FIRST signal, and there are
        // eighteen of those for every two or three real ones. Collecting all
        // four lists before looking at any of them scanned the chip three times
        // over for nothing, on most iterations of this loop.
        let Some(lists) = signal_lists(mcu, used, required, inst) else {
            continue; // this instance can't satisfy some required signal
        };

        // The pads each OPTIONAL signal could take, minus only what other
        // modules hold - hoisted out of the combination loop, which used to
        // re-scan the whole chip for every one of them.
        //
        // UNCAPPED on purpose, and the cap re-applied per combination below:
        // the search's `take(MAX_PER_SIGNAL)` has to happen AFTER this
        // combination's own pads are excluded, or a candidate list would come
        // back one short instead of reaching for the next pad. That is the one
        // detail that makes this hoist give the same answer as the scan.
        // Bounded, and the bound is a proof rather than a guess: the loop
        // below drops only pads already in `chosen` before taking
        // `MAX_PER_SIGNAL`, and `chosen` never holds more than one pad per
        // signal - so no candidate past this many can ever be reached, and
        // truncating here cannot change the answer.
        //
        // The optionals have to be counted too, not just the required pads:
        // by the time the SECOND optional signal is placed, `chosen` already
        // holds the first one. Leaving them out made the list one short in
        // exactly that case, which is what the differential test caught.
        //
        // Without the bound an ESP32-S3 walked forty-odd pads per optional
        // signal per combination, five hundred combinations deep per instance.
        let opt_cap = MAX_PER_SIGNAL + required.len() + optional.len();
        let opt_pads: Vec<Vec<usize>> = optional
            .iter()
            .map(|&sig| {
                let mut v = eligible(mcu, used, sig, inst);
                v.truncate(opt_cap);
                v
            })
            .collect();

        let mut combos: Vec<Vec<usize>> = Vec::new();
        walk(&lists, 0, &mut Vec::new(), &mut combos, MAX_COMBOS);

        for combo in combos {
            chosen.clear();
            chosen.extend(required.iter().copied().zip(combo.iter().copied()));
            // The required pads, ranked ONCE. Each optional candidate then asks
            // what it would make of this, instead of the whole set being folded
            // again per candidate.
            let mut acc = ScoreAcc::new();
            for &(sig, num) in chosen.iter() {
                acc.add(&facts, &sig.pin_function(inst), num);
            }
            // An optional signal takes whichever of its pins keeps the whole set
            // best — an NSS on the far side of the chip would otherwise undo the
            // compactness the required pins were chosen for.
            for (i, &sig) in optional.iter().enumerate() {
                // `chosen` holds the combination plus the optionals already
                // placed, and it is at most a handful of pads - so a linear
                // scan of it beats the `HashSet` clone the old code made per
                // combination.
                let pick = opt_pads[i]
                    .iter()
                    .copied()
                    .filter(|n| !chosen.iter().any(|&(_, c)| c == *n))
                    .take(MAX_PER_SIGNAL)
                    .min_by_key(|&num| acc.with(&facts, inst, &sig.pin_function(inst), num));
                if let Some(num) = pick {
                    acc.add(&facts, &sig.pin_function(inst), num);
                    chosen.push((sig, num));
                }
            }

            let sc = acc.finish(inst);
            let better = match &best {
                None => true,
                Some((bs, _, _)) => sc < *bs,
            };
            if better {
                best = Some((sc, inst, chosen.clone()));
            }
        }
    }

    best.map(|(_, inst, chosen)| (inst, chosen))
}

/// Which peripheral instances could host this module at all.
///
/// The instance loop of [`pick_pins`] with the scoring stripped out: an
/// instance qualifies when every REQUIRED signal has at least one pad left. The
/// palette dialog needs the list rather than the winner, because choosing the
/// instance is half of "put it where I want it" - autowire ranks the instance
/// LAST, so a compact wiring on USART3 beats a scattered one on USART1 and the
/// user never sees that USART1 was possible.
///
/// Uncapped, like [`eligible_for`]: this answers what the chip can do.
pub fn instances_for(
    mcu: &Mcu,
    used: &HashSet<usize>,
    used_instances: &HashSet<u8>,
    required: &[ModuleSignal],
) -> Vec<u8> {
    (0u8..=MAX_INSTANCE)
        .filter(|inst| {
            !used_instances.contains(inst)
                && !required.is_empty()
                && required
                    .iter()
                    .all(|&sig| !eligible(mcu, used, sig, *inst).is_empty())
        })
        .collect()
}

/// Every required signal's candidate pads for one instance, or `None` as soon
/// as one of them has nowhere to go.
fn signal_lists(
    mcu: &Mcu,
    used: &HashSet<usize>,
    required: &[ModuleSignal],
    inst: u8,
) -> Option<Vec<Vec<usize>>> {
    let mut lists = Vec::with_capacity(required.len());
    for &sig in required {
        let l = eligible_capped(mcu, used, sig, inst);
        if l.is_empty() {
            return None;
        }
        lists.push(l);
    }
    Some(lists)
}

/// Whether ANY wiring exists - the question [`Mcu::can_add_module`] asks.
///
/// The same search as [`pick_pins`] and deliberately not a second copy of its
/// rules: same instance loop, same `eligible_capped` candidate lists, same
/// `walk`. What it drops is everything that exists only to RANK - it stops at
/// the first complete assignment instead of enumerating up to `MAX_COMBOS` of
/// them per instance, scores none of them, and never builds the side map.
///
/// Exactly equivalent to `pick_pins(..).is_some()`, and asserted so against
/// every bundled chip in `the_two_searches_agree`: `pick_pins` yields `Some`
/// iff some instance produced at least one combination, and an optional signal
/// can only ever be ADDED to a set, never make it fail.
///
/// Worth its own function because the palette asks this for every entry it
/// draws, on every frame the menu is open, and the ranking it was throwing away
/// cost 119 ms of that frame on an ESP32-S3.
pub fn any_wiring(
    mcu: &Mcu,
    used: &HashSet<usize>,
    used_instances: &HashSet<u8>,
    required: &[ModuleSignal],
) -> bool {
    for inst in 0u8..=MAX_INSTANCE {
        if used_instances.contains(&inst) {
            continue;
        }
        let Some(lists) = signal_lists(mcu, used, required, inst) else {
            continue;
        };
        let mut found: Vec<Vec<usize>> = Vec::new();
        walk(&lists, 0, &mut Vec::new(), &mut found, 1);
        if !found.is_empty() {
            return true;
        }
    }
    false
}

/// Pins for the partner signals of a pin the user assigned BY HAND — the SPI
/// MISO/MOSI that go with an SCK, the USART RX that goes with a TX.
///
/// Same ranking as [`pick_pins`], with the user's own pin pinned into the set
/// and counted by it. That is what keeps a peripheral on ONE pad group: on an
/// STM32F1 SPI1 is either PA5/PA6/PA7 or the remapped PB3/PB4/PB5, never a mix
/// (one AFIO bit moves all of them), and a mixed set is a `ports` of 2 against
/// the matching set's 1 — so wiring PA5 draws in PA6/PA7, and wiring PB3 draws
/// in PB4/PB5. Where both groups sit on one port (I2C1's PB6/PB7 vs PB8/PB9)
/// `spread` separates them instead.
///
/// This is a ranking, not a hardware rule: the pin model has no notion of a
/// remap group (a definition only lists which functions a pin can carry), so
/// geometry stands in for it. It agrees with the F1 groups on every peripheral
/// this IDE generates; a chip whose alternate pads interleave would need real
/// group data.
///
/// The partners are chosen TOGETHER (see [`walk`]) rather than one after the
/// other, so the first one cannot take the only pin the second could have used.
pub fn pick_partners(
    mcu: &Mcu,
    source_pin: usize,
    func: &PinFunction,
) -> Vec<(PinFunction, usize)> {
    let partners = crate::panels::mcu_module::mcu::logic::partner_functions(func);
    if partners.is_empty() {
        return Vec::new();
    }
    let facts = PinFacts::of(mcu);
    let mut taken: HashSet<usize> = HashSet::new();
    taken.insert(source_pin);

    // CAPPED, like `pick_pins`: this is a search under the same `MAX_COMBOS`
    // budget, not an enumeration for a menu. Handing it the uncapped list makes
    // it WORSE - measured on the bundled chips, an esp32s3 wants 1892
    // combinations against a budget of 512, and `walk` fills that budget
    // depth-first from the first candidate of the first partner. The ranking
    // then covers only that corner, which is exactly the bias the exhaustive
    // search was written to remove.
    let lists: Vec<Vec<usize>> = partners
        .iter()
        .map(|want| eligible_for_limited(mcu, &taken, want, MAX_PER_SIGNAL))
        .collect();
    // A partner with nowhere to go is dropped, not a reason to wire nothing:
    // the old first-fit assigned what it could, and so does this.
    let present: Vec<usize> = (0..lists.len()).filter(|&i| !lists[i].is_empty()).collect();
    if present.is_empty() {
        return Vec::new();
    }
    let lists: Vec<Vec<usize>> = present.iter().map(|&i| lists[i].clone()).collect();

    let mut combos: Vec<Vec<usize>> = Vec::new();
    walk(&lists, 0, &mut Vec::new(), &mut combos, MAX_COMBOS);

    combos
        .into_iter()
        .map(|combo| {
            present
                .iter()
                .map(|&i| partners[i].clone())
                .zip(combo)
                .collect::<Vec<(PinFunction, usize)>>()
        })
        .min_by_key(|chosen| {
            let mut with_source = chosen.clone();
            with_source.push((func.clone(), source_pin));
            // The instance tie-break is meaningless here (the instance is the
            // user's, already fixed by `func`), so any constant will do.
            score(&facts, 0, &with_source)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;
    use crate::panels::mcu_module::modules::ModuleKind;

    /// Pin numbers a module ended up wired to, sorted.
    fn wired(mcu: &Mcu, idx: usize) -> Vec<usize> {
        let mut v: Vec<usize> = mcu.modules[idx]
            .connections
            .iter()
            .map(|c| c.mcu_pin)
            .collect();
        v.sort_unstable();
        v
    }

    /// LPUART is a peripheral of its OWN: on a chip carrying both, adding an
    /// LPUART module must not touch the USART pins, and the two coexist on
    /// instance 1. (The F103 has no LPUART, so the pins are grafted on here —
    /// which is also the smallest possible proof that support is pin-derived.)
    #[test]
    fn lpuart_wires_to_its_own_pins_beside_a_usart() {
        let mut mcu = create_stm32f103c8tx();
        assert!(
            !mcu.supports_module(ModuleKind::GenericInterfaceLpuart),
            "no LPUART pins yet"
        );
        // PB10/PB11 gain LPUART1 TX/RX, the way an imported G0 would have them.
        for (num, f) in [
            (21usize, PinFunction::LpuartTx(1)),
            (22usize, PinFunction::LpuartRx(1)),
        ] {
            mcu.find_pin_mut(num).unwrap().available_functions.push(f);
        }
        assert!(mcu.supports_module(ModuleKind::GenericInterfaceLpuart));

        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        assert!(mcu.add_module(ModuleKind::GenericInterfaceLpuart));
        assert_eq!(mcu.modules[1].kind, ModuleKind::GenericInterfaceLpuart);
        // Same instance number as USART1, different peripheral, different pins.
        assert_eq!(mcu.modules[0].instance(), 1);
        assert_eq!(mcu.modules[1].instance(), 1);
        assert_eq!(wired(&mcu, 1), vec![21, 22]);
        assert!(
            wired(&mcu, 0).iter().all(|n| ![21, 22].contains(n)),
            "the USART kept its own pins: {:?}",
            wired(&mcu, 0)
        );
        assert_eq!(
            mcu.find_pin(21).unwrap().selected_function,
            PinFunction::LpuartTx(1)
        );
    }

    /// Every LEDC channel gets its OWN name.
    ///
    /// `ModuleSignal` used to stop at `PwmCh4`, with a catch-all arm, so on an
    /// ESP — six or eight channels through the GPIO matrix — channels 0, 5, 6
    /// and 7 all came back as `CH4`. The pins were right, because the ESP
    /// generator reads them and not the module's wires; every panel row that
    /// reads the wires was wrong.
    #[test]
    fn every_timer_channel_has_its_own_signal() {
        use crate::panels::mcu_module::modules::{ModuleSignal, module_signal_of};

        for ch in 0u8..=7 {
            let (kind, timer, sig) = module_signal_of(&PinFunction::TimerPwm {
                timer: 3,
                channel: ch,
            })
            .expect("a PWM pad belongs to a timer module");
            assert_eq!(kind, ModuleKind::GenericInterfaceTimer);
            assert_eq!(timer, 3);
            assert_eq!(sig.label(), format!("CH{ch}"), "channel {ch}");
            // And back again — the reverse map is what the canvas uses to
            // assign a signal's pad, so a one-way name would strand it.
            assert_eq!(
                sig.pin_function(3),
                PinFunction::TimerPwm {
                    timer: 3,
                    channel: ch
                },
                "channel {ch} round-trips"
            );
        }

        // The order is the row order: `reconcile_modules` sorts by signal, so
        // CH0 must come before CH1 rather than after CH7.
        assert!(ModuleSignal::PwmCh0 < ModuleSignal::PwmCh1);
        assert!(ModuleSignal::PwmCh4 < ModuleSignal::PwmCh5);
    }

    /// Re-pointing a pad at another channel of the same timer keeps its duty.
    ///
    /// Everything a channel owns is keyed by its NUMBER, so without this the
    /// move reads as "CH1 disappeared, a fresh CH3 arrived at 0 %" — and a duty
    /// silently falling to zero is the kind of change nobody re-checks.
    ///
    /// On an ESP, deliberately: it is the part where a pad HAS a choice of
    /// channel, because LEDC reaches the pins through the GPIO matrix. An F1
    /// pad carries one channel per timer, so the same test there would take the
    /// early exit and prove nothing.
    #[test]
    fn a_pads_duty_follows_it_to_another_channel() {
        use crate::panels::mcu_module::modules::ModuleConfig;

        let mut mcu = crate::panels::mcu_module::builtins::builtin_for("esp32c3")
            .expect("a bundled ESP32-C3")
            .build_mcu();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceTimer));
        let timer = mcu.modules[0].instance();
        let pin = mcu.modules[0].connections[0].mcu_pin;
        let from = match mcu.find_pin(pin).unwrap().selected_function {
            PinFunction::TimerPwm { channel, .. } => channel,
            ref f => panic!("the seeded pad is a PWM channel, not {f:?}"),
        };

        let ModuleConfig::Timer(cfg) = &mut mcu.modules[0].config else {
            panic!("a timer module carries a timer config");
        };
        cfg.duty_x100.insert(from, 2_000); // 20 %

        // Another channel the SAME pad can drive.
        let to = mcu
            .find_pin(pin)
            .unwrap()
            .available_functions
            .iter()
            .filter_map(|f| match f {
                PinFunction::TimerPwm { timer: t, channel } if *t == timer && *channel != from => {
                    Some(*channel)
                }
                _ => None,
            })
            .min()
            .expect("an LEDC pad offers every channel");

        mcu.apply_pin_function(pin, PinFunction::TimerPwm { timer, channel: to });
        let ModuleConfig::Timer(cfg) = &mcu.modules[0].config else {
            panic!("still a timer config");
        };
        assert_eq!(cfg.duty_x100.get(&to), Some(&2_000), "the duty moved");
        assert!(
            cfg.duty_x100.get(&from).is_none(),
            "and did not stay behind"
        );

        // And the module now names the channel it really drives — the thing
        // the four-signal `ModuleSignal` used to get wrong.
        assert_eq!(
            mcu.modules[0].connections[0].signal.label(),
            format!("CH{to}")
        );
    }

    /// …but it never steals a duty from a channel that is still driven.
    #[test]
    fn a_live_channels_duty_is_left_alone() {
        use crate::panels::mcu_module::modules::{ModuleConfig, TimerModuleConfig};

        let mut cfg = TimerModuleConfig::new(3);
        cfg.duty_x100.insert(1, 2_000);
        cfg.duty_x100.insert(3, 500);
        // CH3 is already somebody's, so CH1 does not get to overwrite it.
        assert!(!cfg.move_channel(1, 3));
        assert_eq!(cfg.duty_x100.get(&1), Some(&2_000));
        assert_eq!(cfg.duty_x100.get(&3), Some(&500));

        // A free destination does take it.
        assert!(cfg.move_channel(1, 2));
        assert_eq!(cfg.duty_x100.get(&2), Some(&2_000));
        assert!(cfg.duty_x100.get(&1).is_none());

        let _ = ModuleConfig::Timer(cfg);
    }

    /// The PWM module is the TIMER: adding one takes a single channel, and a
    /// channel of the SAME timer assigned by hand later JOINS that module rather
    /// than making a second one. That fold-in is how channels 2..4 are added, so
    /// it is the design's load-bearing part.
    #[test]
    fn extra_channels_join_the_timers_module() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceTimer));
        let timer = mcu.modules[0].instance();
        assert_eq!(mcu.modules[0].connections.len(), 1, "one channel to start");

        // A second channel of the SAME timer, assigned on the canvas.
        let ch2 = PinFunction::TimerPwm { timer, channel: 2 };
        let pin = mcu
            .iter_all_pins()
            .find(|p| {
                p.selected_function == PinFunction::Unset && p.available_functions.contains(&ch2)
            })
            .map(|p| p.number)
            .expect("the mock has a second channel for this timer");
        mcu.apply_pin_function(pin, ch2);

        assert_eq!(
            mcu.modules.len(),
            1,
            "still ONE module: {:?}",
            mcu.modules.len()
        );
        assert_eq!(mcu.modules[0].connections.len(), 2, "both channels wired");
        assert!(mcu.modules[0].connections.iter().any(|c| c.mcu_pin == pin));
    }

    /// On a clean chip the first SPI takes SPI1 on its DEFAULT pins — one port,
    /// one side, and the lowest instance breaks the tie with SPI2.
    #[test]
    fn first_spi_takes_spi1_default_pins() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        assert_eq!(mcu.modules[0].instance(), 1);
        // PA4 (NSS) PA5 (SCK) PA6 (MISO) PA7 (MOSI)
        assert_eq!(wired(&mcu, 0), vec![14, 15, 16, 17]);
    }

    /// The reported case: PB5 (SPI1_MOSI on the remap set) is already a GPIO, so
    /// SPI1 can only be formed by mixing remap pins with the default MOSI on the
    /// opposite edge. SPI2 is free and compact — it must win.
    #[test]
    fn a_scattered_instance_loses_to_a_compact_one() {
        let mut mcu = create_stm32f103c8tx();
        // Take the whole SPI1 default block, leaving only the remap set.
        for num in [14, 15, 16, 17] {
            mcu.apply_pin_function(num, PinFunction::GpioOutput);
        }
        // …and knock out the remap MOSI, so SPI1 could only be built scattered.
        mcu.apply_pin_function(41, PinFunction::GpioInput); // PB5
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        assert_eq!(mcu.modules[0].instance(), 2, "SPI2, not a scattered SPI1");
        // PB12 (NSS) PB13 (SCK) PB14 (MISO) PB15 (MOSI)
        assert_eq!(wired(&mcu, 0), vec![25, 26, 27, 28]);
    }

    /// Two SPI modules: the instance guard still moves the second one on, and
    /// the leftover instance is wired even though its pins are the scattered
    /// ones — "last resort" means it IS still used.
    #[test]
    fn the_second_spi_moves_to_the_other_instance() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        assert_eq!(mcu.modules[0].instance(), 1);
        assert_eq!(mcu.modules[1].instance(), 2);
        assert_eq!(wired(&mcu, 1), vec![25, 26, 27, 28]);
    }

    /// Assign `func` to the pin NAMED `name`, the way a click on the Pins tab
    /// does — partners included.
    fn assign(mcu: &mut Mcu, name: &str, func: PinFunction) {
        let num = mcu
            .iter_all_pins()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no pin {name} on the mock chip"))
            .number;
        mcu.apply_pin_function(num, func);
    }

    /// The names carrying each of `funcs`, in that order. `None` for a function
    /// nothing holds, so a missing partner fails the assertion visibly.
    fn holders(mcu: &Mcu, funcs: &[PinFunction]) -> Vec<Option<String>> {
        funcs
            .iter()
            .map(|f| {
                mcu.iter_all_pins()
                    .find(|p| p.selected_function == *f)
                    .map(|p| p.name.clone())
            })
            .collect()
    }

    fn spi1(mcu: &Mcu) -> Vec<Option<String>> {
        holders(
            mcu,
            &[
                PinFunction::SpiSck(1),
                PinFunction::SpiMiso(1),
                PinFunction::SpiMosi(1),
            ],
        )
    }

    fn names(v: &[Option<String>]) -> Vec<&str> {
        v.iter().map(|o| o.as_deref().unwrap_or("<none>")).collect()
    }

    /// The reported bug: assigning ONE SPI1 signal by hand answered PA5 (the
    /// default SCK) with PB4/PB5 (the REMAP MISO/MOSI). One AFIO bit moves all
    /// of SPI1's pins together, so that set does not exist in hardware, no
    /// `spi::Pins` impl accepts it, and the generated project did not compile.
    /// Whichever group the user's own pin is in, the partners must follow it.
    #[test]
    fn a_hand_assigned_signal_keeps_its_partners_in_one_pad_group() {
        let mut mcu = create_stm32f103c8tx();
        assign(&mut mcu, "PA5", PinFunction::SpiSck(1));
        assert_eq!(
            names(&spi1(&mcu)),
            ["PA5", "PA6", "PA7"],
            "the DEFAULT set must stay whole"
        );

        let mut mcu = create_stm32f103c8tx();
        assign(&mut mcu, "PB3", PinFunction::SpiSck(1));
        assert_eq!(
            names(&spi1(&mcu)),
            ["PB3", "PB4", "PB5"],
            "the REMAP set must stay whole"
        );

        // Reached from a partner signal rather than the clock, it holds too.
        let mut mcu = create_stm32f103c8tx();
        assign(&mut mcu, "PA7", PinFunction::SpiMosi(1));
        assert_eq!(names(&spi1(&mcu)), ["PA5", "PA6", "PA7"]);
    }

    /// I2C1's two groups (PB6/PB7 and the PB8/PB9 remap) share a port, so the
    /// `ports` criterion cannot separate them — `spread` does.
    #[test]
    fn i2c1_partners_follow_the_group_the_user_picked() {
        let scl_sda = [PinFunction::I2cScl(1), PinFunction::I2cSda(1)];

        let mut mcu = create_stm32f103c8tx();
        assign(&mut mcu, "PB6", PinFunction::I2cScl(1));
        assert_eq!(names(&holders(&mcu, &scl_sda)), ["PB6", "PB7"]);

        let mut mcu = create_stm32f103c8tx();
        assign(&mut mcu, "PB8", PinFunction::I2cScl(1));
        assert_eq!(names(&holders(&mcu, &scl_sda)), ["PB8", "PB9"]);
    }

    /// USART1 is modelled with ONE pin set on this chip (PA9/PA10 — the remap to
    /// PB6/PB7 is not in the definition), so there is no group to get wrong;
    /// this pins down that the partner is still found.
    #[test]
    fn usart1_still_pairs_tx_with_rx() {
        let mut mcu = create_stm32f103c8tx();
        assign(&mut mcu, "PA9", PinFunction::UsartTx(1));
        assert_eq!(
            names(&holders(
                &mcu,
                &[PinFunction::UsartTx(1), PinFunction::UsartRx(1)]
            )),
            ["PA9", "PA10"]
        );
    }

    /// A partner already on the pin the user wants is REUSED, not duplicated.
    /// First-fit skipped any non-Unset pin, so it would hand the same function
    /// to a second pin — two SPI1 MISOs, and a `pins::configs` call naming only
    /// one of them.
    #[test]
    fn an_already_assigned_partner_is_not_duplicated() {
        let mut mcu = create_stm32f103c8tx();
        assign(&mut mcu, "PA6", PinFunction::SpiMiso(1));
        assign(&mut mcu, "PA5", PinFunction::SpiSck(1));
        assert_eq!(names(&spi1(&mcu)), ["PA5", "PA6", "PA7"]);
        assert_eq!(
            mcu.iter_all_pins()
                .filter(|p| p.selected_function == PinFunction::SpiMiso(1))
                .count(),
            1,
            "MISO must sit on exactly one pin"
        );
    }

    /// Pins the user already assigned to the exact function outrank everything
    /// else — that is the top criterion, so a set that reuses them beats a
    /// tighter set of untouched pins.
    #[test]
    fn already_assigned_pins_win_over_a_tighter_set() {
        let mut mcu = create_stm32f103c8tx();
        // Configure the SPI1 REMAP set by hand (2 ports, so it loses on every
        // geometric criterion to SPI2's PB12..PB15 block).
        mcu.apply_pin_function(39, PinFunction::SpiSck(1)); // PB3
        mcu.apply_pin_function(40, PinFunction::SpiMiso(1)); // PB4
        mcu.apply_pin_function(41, PinFunction::SpiMosi(1)); // PB5
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        assert_eq!(mcu.modules[0].instance(), 1);
        let w = wired(&mcu, 0);
        assert!(
            w.contains(&39) && w.contains(&40) && w.contains(&41),
            "the user's own pins are reused, got {w:?}"
        );
    }
}

#[cfg(test)]
mod the_pads_the_search_may_reach {
    use super::*;
    use crate::panels::mcu_module::builtins::builtin_definitions;
    use crate::panels::mcu_module::modules::ModuleKind;

    fn chip(id: &str) -> Mcu {
        builtin_definitions()
            .into_iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("built-in {id}"))
            .build_mcu()
    }

    /// The enumeration a PICKER sees is the whole chip, not the search window.
    ///
    /// On a GPIO-matrix part the two numbers are far apart, and the gap is the
    /// user's complaint: a menu built on the search's list would offer 8 of the
    /// 21 pads that can carry the signal, and look every bit as arbitrary as
    /// the automatic pick.
    #[test]
    fn a_picker_is_offered_every_legal_pad_not_the_first_eight() {
        let mcu = chip("esp32c3");
        let none = HashSet::new();
        let all = eligible(&mcu, &none, ModuleSignal::Tx, 0);
        assert!(
            all.len() > MAX_PER_SIGNAL,
            "a C3 has more UART TX pads than the search window: {}",
            all.len()
        );
        // ...and the search still keeps to its budget, because MAX_COMBOS is
        // sized for it.
        assert_eq!(
            eligible_capped(&mcu, &none, ModuleSignal::Tx, 0).len(),
            MAX_PER_SIGNAL
        );
    }

    /// A pad the BOARD already spends is the last one to be taken.
    ///
    /// The Pico's definition annotates exactly four: the on-board LED, VBUS
    /// sense, VSYS sense and the SMPS mode pin. The second "+ USART" used to
    /// land on two of them - not because they scored better, but because they
    /// scored the SAME and are declared earlier in the chip file.
    #[test]
    fn a_board_pad_is_not_taken_while_a_plain_one_is_free() {
        let mut mcu = chip("rp2040_pico");
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        let annotated: Vec<String> = mcu
            .modules
            .iter()
            .flat_map(|m| m.connections.iter())
            .filter_map(|c| mcu.find_pin(c.mcu_pin))
            .map(|p| p.name.clone())
            .filter(|n| super::is_board_used(n))
            .collect();
        assert!(
            annotated.is_empty(),
            "no board-committed pad was taken: {annotated:?}"
        );
    }

    /// The term ranks BELOW the geometry, so it can only settle a tie - it must
    /// never pull a wiring onto two ports or two sides of the chip.
    #[test]
    fn the_board_term_never_outranks_the_hardware_terms() {
        let worse_geometry = Score {
            unassigned: 0,
            ports: 2,
            sides: 1,
            spread: 4,
            board_use: 0,
            instance: 0,
        };
        let board_pad_but_compact = Score {
            unassigned: 0,
            ports: 1,
            sides: 1,
            spread: 4,
            board_use: 2,
            instance: 0,
        };
        assert!(board_pad_but_compact < worse_geometry);
    }
}

/// What one open-palette frame costs. `cargo test palette_frame_cost -- --ignored --nocapture`
#[cfg(test)]
mod palette_cost {
    use crate::panels::mcu_module::builtins::builtin_definitions;
    use crate::panels::mcu_module::modules::ModuleKind;
    use std::time::Instant;

    /// The palette redraws every frame while its menu is open, and asks about
    /// every kind the chip offers. This is the number that matters.
    #[test]
    #[ignore]
    fn palette_frame_cost() {
        for id in ["stm32f103c8t6", "rp2040_pico", "esp32c3", "esp32s3"] {
            let mcu = builtin_definitions()
                .into_iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("built-in {id}"))
                .build_mcu();
            let kinds: Vec<ModuleKind> = ModuleKind::ALL
                .into_iter()
                .filter(|k| mcu.supports_module(*k))
                .collect();

            let t = Instant::now();
            let mut n = 0;
            for k in &kinds {
                if mcu.can_add_module(*k) {
                    n += 1;
                }
            }
            let feasibility = t.elapsed();

            // The pad preview, for the WORST kind. A mean over the cheap ones
            // would flatter it: the frame is only as fast as the submenu the
            // user actually opened.
            let mut preview = std::time::Duration::ZERO;
            let mut worst = ModuleKind::Custom;
            for k in kinds.iter().filter(|k| !k.is_custom()) {
                let t = Instant::now();
                let _ = crate::panels::mcu_module::mcu::gui::modules::auto_wiring_summary(&mcu, *k);
                let e = t.elapsed();
                if e > preview {
                    preview = e;
                    worst = *k;
                }
            }

            println!(
                "FRAME {id:<16} kinds={:<3} addable={n:<3} feasibility={feasibility:?} \
                 worst preview={preview:?} ({worst:?})",
                kinds.len()
            );
        }
    }
}

#[cfg(test)]
mod the_two_searches_agree {
    use super::{any_wiring, pick_pins};
    use crate::panels::mcu_module::builtins::builtin_definitions;
    use crate::panels::mcu_module::modules::ModuleKind;
    use std::collections::HashSet;

    /// `any_wiring` must answer exactly `pick_pins(..).is_some()`.
    ///
    /// The whole point of the cheap search is that it asks the SAME question,
    /// so the palette cannot grey out an entry the add would accept, or offer
    /// one the add would refuse. Driven to exhaustion on every bundled chip and
    /// every kind it supports, because the answers only get interesting once
    /// the pads start running out.
    #[test]
    fn the_cheap_search_answers_what_the_full_one_does() {
        let mut checked = 0usize;
        for d in builtin_definitions() {
            let mut mcu = d.build_mcu();
            for kind in ModuleKind::ALL {
                if kind.is_custom() || !mcu.supports_module(kind) {
                    continue;
                }
                let (required, optional) = kind.signals();
                // Past exhaustion on purpose: the last iterations are the ones
                // where one search could say yes and the other no.
                for _ in 0..6 {
                    let used: HashSet<usize> = mcu
                        .modules
                        .iter()
                        .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
                        .collect();
                    let used_instances: HashSet<u8> = mcu
                        .modules
                        .iter()
                        .filter(|m| m.kind == kind)
                        .map(|m| m.instance())
                        .collect();
                    let cheap = any_wiring(&mcu, &used, &used_instances, required);
                    let full =
                        pick_pins(&mcu, &used, &used_instances, required, optional).is_some();
                    assert_eq!(cheap, full, "{} {kind:?}", d.id);
                    checked += 1;
                    if !cheap {
                        break;
                    }
                    mcu.add_module(kind);
                }
            }
        }
        assert!(checked > 200, "the sweep really ran: {checked} cases");
    }

    /// An OPTIONAL signal can never turn a possible wiring into an impossible
    /// one - which is why the cheap search may ignore optionals entirely.
    #[test]
    fn an_optional_signal_cannot_make_a_wiring_fail() {
        for d in builtin_definitions() {
            let mcu = d.build_mcu();
            for kind in ModuleKind::ALL {
                if kind.is_custom() || !mcu.supports_module(kind) {
                    continue;
                }
                let (required, optional) = kind.signals();
                if optional.is_empty() {
                    continue;
                }
                let none = HashSet::new();
                let with = pick_pins(&mcu, &none, &HashSet::new(), required, optional).is_some();
                let without = pick_pins(&mcu, &none, &HashSet::new(), required, &[]).is_some();
                assert_eq!(with, without, "{} {kind:?}", d.id);
            }
        }
    }
}

/// The search EXACTLY as it stood before it was made fast, kept as the oracle.
///
/// `pick_pins` decides where every peripheral in every generated project goes,
/// so an optimisation of it has one acceptance criterion and it is not "the
/// tests still pass": it must return the SAME instance and the SAME pin list,
/// in the same order, for every input. A faster search that quietly prefers a
/// different pad would rewire every user's project on their next Save, and no
/// existing test would say a word.
///
/// So the old body lives on here, verbatim, and
/// `the_fast_search_returns_what_the_old_one_did` runs both over every bundled
/// chip, every module kind, and a spread of prefill states, comparing results
/// exactly. Delete this module only when `pick_pins` stops being load-bearing.
#[cfg(test)]
mod reference_search {
    use super::*;

    /// The search window, FROZEN at what it was when this oracle was taken.
    ///
    /// Not `super::MAX_PER_SIGNAL`. An oracle that reads the same constant as
    /// the code it checks moves with it, and the one class of change it would
    /// then be blind to is the one that matters most here: narrowing the window
    /// re-wires real projects onto different pads, and both sides of the
    /// comparison would agree about it. That is not hypothetical - it happened
    /// to this very file, the constant went 8 -> 6, and the differential test
    /// stayed green.
    ///
    /// So changing `MAX_PER_SIGNAL` now BREAKS this test, and that is the
    /// intent: the window is part of the answer, and moving it is a decision to
    /// declare here, not a tuning knob to turn quietly.
    const REF_MAX_PER_SIGNAL: usize = 8;

    /// The oracle's own candidate list, on the frozen window.
    fn eligible_capped(
        mcu: &Mcu,
        taken: &HashSet<usize>,
        sig: ModuleSignal,
        inst: u8,
    ) -> Vec<usize> {
        eligible_for_limited(mcu, taken, &sig.pin_function(inst), REF_MAX_PER_SIGNAL)
    }

    fn score(
        mcu: &Mcu,
        inst: u8,
        chosen: &[(PinFunction, usize)],
        sides: &HashMap<usize, u8>,
    ) -> Score {
        let mut unassigned = 0;
        let mut ports: HashSet<&str> = HashSet::new();
        let mut side_set: HashSet<u8> = HashSet::new();
        let mut board_use = 0;
        let (mut lo, mut hi) = (usize::MAX, 0usize);
        for (want, num) in chosen {
            if let Some(pin) = mcu.find_pin(*num) {
                if pin.selected_function != *want {
                    unassigned += 1;
                }
                ports.insert(port_of(&pin.name));
                if is_board_used(&pin.name) {
                    board_use += 1;
                }
            }
            if let Some(&s) = sides.get(num) {
                side_set.insert(s);
            }
            lo = lo.min(*num);
            hi = hi.max(*num);
        }
        Score {
            unassigned,
            ports: ports.len(),
            sides: side_set.len(),
            spread: hi.saturating_sub(lo),
            board_use,
            instance: inst,
        }
    }

    fn score_signals(
        mcu: &Mcu,
        inst: u8,
        chosen: &[(ModuleSignal, usize)],
        sides: &HashMap<usize, u8>,
    ) -> Score {
        let by_fn: Vec<(PinFunction, usize)> = chosen
            .iter()
            .map(|&(sig, num)| (sig.pin_function(inst), num))
            .collect();
        score(mcu, inst, &by_fn, sides)
    }

    pub fn pick_pins(
        mcu: &Mcu,
        used: &HashSet<usize>,
        used_instances: &HashSet<u8>,
        required: &[ModuleSignal],
        optional: &[ModuleSignal],
    ) -> Option<(u8, Vec<(ModuleSignal, usize)>)> {
        let sides = side_map(mcu);
        let mut best: Option<(Score, u8, Vec<(ModuleSignal, usize)>)> = None;

        for inst in 0u8..=MAX_INSTANCE {
            if used_instances.contains(&inst) {
                continue;
            }
            let lists: Vec<Vec<usize>> = required
                .iter()
                .map(|&sig| eligible_capped(mcu, used, sig, inst))
                .collect();
            if lists.iter().any(|l| l.is_empty()) {
                continue; // this instance can't satisfy some required signal
            }

            let mut combos: Vec<Vec<usize>> = Vec::new();
            walk(&lists, 0, &mut Vec::new(), &mut combos, MAX_COMBOS);

            for combo in combos {
                let mut chosen: Vec<(ModuleSignal, usize)> = required
                    .iter()
                    .copied()
                    .zip(combo.iter().copied())
                    .collect();
                let mut taken: HashSet<usize> = used.clone();
                taken.extend(combo.iter().copied());
                // An optional signal takes whichever of its pins keeps the whole set
                // best — an NSS on the far side of the chip would otherwise undo the
                // compactness the required pins were chosen for.
                for &sig in optional {
                    let pick = eligible_capped(mcu, &taken, sig, inst)
                        .into_iter()
                        .min_by_key(|&num| {
                            let mut trial = chosen.clone();
                            trial.push((sig, num));
                            score_signals(mcu, inst, &trial, &sides)
                        });
                    if let Some(num) = pick {
                        taken.insert(num);
                        chosen.push((sig, num));
                    }
                }

                let sc = score_signals(mcu, inst, &chosen, &sides);
                let better = match &best {
                    None => true,
                    Some((bs, _, _)) => sc < *bs,
                };
                if better {
                    best = Some((sc, inst, chosen));
                }
            }
        }

        best.map(|(_, inst, chosen)| (inst, chosen))
    }
}

#[cfg(test)]
mod the_fast_search_matches_the_reference {
    use super::{pick_pins, reference_search};
    use crate::panels::mcu_module::builtins::builtin_definitions;
    use crate::panels::mcu_module::modules::ModuleKind;
    use crate::panels::mcu_module::pins::PinFunction;
    use std::collections::HashSet;

    /// Byte-for-byte the same answer, on every chip, kind and prefill state.
    ///
    /// The acceptance criterion for making `pick_pins` fast. Not "the tests
    /// still pass" - a search that quietly preferred a different pad would
    /// rewire every project on its next Save and nothing else would notice.
    #[test]
    fn the_fast_search_returns_what_the_old_one_did() {
        let mut cases = 0usize;
        for d in builtin_definitions() {
            for kind in ModuleKind::ALL {
                let base = d.build_mcu();
                if kind.is_custom() || !base.supports_module(kind) {
                    continue;
                }
                let (required, optional) = kind.signals();

                // A spread of states: empty, then with pads spoken for, then
                // with instances spoken for, then both - the corners where a
                // ranking can start to differ.
                for prefill in 0..5usize {
                    let mut mcu = base.clone();
                    // Park some pads on GPIO so the candidate lists shrink
                    // unevenly, which is what makes ties appear.
                    let victims: Vec<usize> = mcu
                        .iter_all_pins()
                        .filter(|p| !p.reserved)
                        .map(|p| p.number)
                        .step_by(3)
                        .take(prefill * 2)
                        .collect();
                    for n in victims {
                        if let Some(p) = mcu.find_pin_mut(n) {
                            p.selected_function = PinFunction::GpioOutput;
                        }
                    }
                    for _ in 0..prefill.min(2) {
                        mcu.add_module(kind);
                    }

                    let used: HashSet<usize> = mcu
                        .modules
                        .iter()
                        .flat_map(|m| m.connections.iter().map(|c| c.mcu_pin))
                        .collect();
                    let used_instances: HashSet<u8> = mcu
                        .modules
                        .iter()
                        .filter(|m| m.kind == kind)
                        .map(|m| m.instance())
                        .collect();

                    let fast = pick_pins(&mcu, &used, &used_instances, required, optional);
                    let slow = reference_search::pick_pins(
                        &mcu,
                        &used,
                        &used_instances,
                        required,
                        optional,
                    );
                    assert_eq!(
                        fast, slow,
                        "{} {kind:?} prefill={prefill}: the fast search diverged",
                        d.id
                    );
                    cases += 1;
                }
            }
        }
        assert!(cases > 300, "the sweep really ran: {cases} cases");
    }
}
