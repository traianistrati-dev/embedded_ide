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

/// Every pin that could carry `want`: free (or already set to exactly this
/// function) and not taken by another module or an earlier signal.
fn eligible_for(mcu: &Mcu, taken: &HashSet<usize>, want: &PinFunction) -> Vec<usize> {
    mcu.iter_all_pins()
        .filter(|p| !p.reserved && !taken.contains(&p.number))
        .filter(|p| {
            p.available_functions.contains(want)
                && (p.selected_function == *want || p.selected_function == PinFunction::Unset)
        })
        .map(|p| p.number)
        .take(MAX_PER_SIGNAL)
        .collect()
}

/// [`eligible_for`], addressed the way a module names its signals.
fn eligible(mcu: &Mcu, taken: &HashSet<usize>, sig: ModuleSignal, inst: u8) -> Vec<usize> {
    eligible_for(mcu, taken, &sig.pin_function(inst))
}

/// Rank a candidate set of `(function the pin is wanted for, pin number)`.
///
/// It is keyed on the FUNCTION rather than on a module signal so the same
/// ranking serves both callers: a whole module's wiring, and the partners of a
/// single pin the user assigned by hand.
fn score(
    mcu: &Mcu,
    inst: u8,
    chosen: &[(PinFunction, usize)],
    sides: &HashMap<usize, u8>,
) -> Score {
    let mut unassigned = 0;
    let mut ports: HashSet<&str> = HashSet::new();
    let mut side_set: HashSet<u8> = HashSet::new();
    let (mut lo, mut hi) = (usize::MAX, 0usize);
    for (want, num) in chosen {
        if let Some(pin) = mcu.find_pin(*num) {
            if pin.selected_function != *want {
                unassigned += 1;
            }
            ports.insert(port_of(&pin.name));
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
        instance: inst,
    }
}

/// `score` for a set named by module signals.
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

    for inst in 0u8..=MAX_INSTANCE {
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
                let pick = eligible(mcu, &taken, sig, inst)
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
    let sides = side_map(mcu);
    let mut taken: HashSet<usize> = HashSet::new();
    taken.insert(source_pin);

    let lists: Vec<Vec<usize>> = partners
        .iter()
        .map(|want| eligible_for(mcu, &taken, want))
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
            score(mcu, 0, &with_source, &sides)
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
