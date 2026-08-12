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

/// Candidate pins considered per signal (chip order). A signal with more
/// alternatives than this is vanishingly rare; the cap just bounds the search.
const MAX_PER_SIGNAL: usize = 8;
/// Upper bound on the wirings explored per instance.
const MAX_COMBOS: usize = 512;

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
/// 5. `instance` — pure tie-break, keeping the old "lowest instance" order.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Score {
    unassigned: usize,
    ports: usize,
    sides: usize,
    spread: usize,
    instance: u8,
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

/// Every pin that could carry `sig` on `inst`: free (or already set to exactly
/// this function) and not taken by another module or an earlier signal.
fn eligible(
    mcu: &Mcu,
    taken: &HashSet<usize>,
    sig: ModuleSignal,
    inst: u8,
) -> Vec<usize> {
    let want = sig.pin_function(inst);
    mcu.iter_all_pins()
        .filter(|p| !p.reserved && !taken.contains(&p.number))
        .filter(|p| {
            p.available_functions.contains(&want)
                && (p.selected_function == want || p.selected_function == PinFunction::Unset)
        })
        .map(|p| p.number)
        .take(MAX_PER_SIGNAL)
        .collect()
}

fn score(
    mcu: &Mcu,
    inst: u8,
    chosen: &[(ModuleSignal, usize)],
    sides: &HashMap<usize, u8>,
) -> Score {
    let mut unassigned = 0;
    let mut ports: HashSet<&str> = HashSet::new();
    let mut side_set: HashSet<u8> = HashSet::new();
    let (mut lo, mut hi) = (usize::MAX, 0usize);
    for &(sig, num) in chosen {
        if let Some(pin) = mcu.find_pin(num) {
            if pin.selected_function != sig.pin_function(inst) {
                unassigned += 1;
            }
            ports.insert(port_of(&pin.name));
        }
        if let Some(&s) = sides.get(&num) {
            side_set.insert(s);
        }
        lo = lo.min(num);
        hi = hi.max(num);
    }
    Score {
        unassigned,
        ports: ports.len(),
        sides: side_set.len(),
        spread: hi.saturating_sub(lo),
        instance: inst,
    }
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
    let sides = side_map(mcu);
    let mut best: Option<(Score, u8, Vec<(ModuleSignal, usize)>)> = None;

    for inst in 0u8..=3 {
        if used_instances.contains(&inst) {
            continue;
        }
        let lists: Vec<Vec<usize>> = required
            .iter()
            .map(|&sig| eligible(mcu, used, sig, inst))
            .collect();
        if lists.iter().any(|l| l.is_empty()) {
            continue; // this instance can't satisfy some required signal
        }

        let mut combos: Vec<Vec<usize>> = Vec::new();
        walk(&lists, 0, &mut Vec::new(), &mut combos, MAX_COMBOS);

        for combo in combos {
            let mut chosen: Vec<(ModuleSignal, usize)> =
                required.iter().copied().zip(combo.iter().copied()).collect();
            let mut taken: HashSet<usize> = used.clone();
            taken.extend(combo.iter().copied());
            // An optional signal takes whichever of its pins keeps the whole set
            // best — an NSS on the far side of the chip would otherwise undo the
            // compactness the required pins were chosen for.
            for &sig in optional {
                let pick = eligible(mcu, &taken, sig, inst)
                    .into_iter()
                    .min_by_key(|&num| {
                        let mut trial = chosen.clone();
                        trial.push((sig, num));
                        score(mcu, inst, &trial, &sides)
                    });
                if let Some(num) = pick {
                    taken.insert(num);
                    chosen.push((sig, num));
                }
            }

            let sc = score(mcu, inst, &chosen, &sides);
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
