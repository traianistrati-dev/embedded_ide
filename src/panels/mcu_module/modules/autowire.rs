//! Auto-wiring: pick compatible MCU pins for a new module.
//!
//! For a USART module, find a peripheral instance that has both a TX-capable and
//! a distinct RX-capable pin, preferring pins the user already assigned to that
//! USART, then free pins — and never reusing a pin already taken by another
//! module.

use std::collections::{BTreeMap, HashSet};

use crate::panels::mcu_module::mcu::model::Mcu;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

/// Candidate pins per USART instance: `(pin_number, already_assigned_to_role)`.
type Candidates = BTreeMap<u8, Vec<(usize, bool)>>;

/// Choose `(instance, tx_pin, rx_pin)` for a new USART module, skipping any pin
/// in `used_pins` (already wired to another module). Returns `None` when no
/// instance has a free/assigned TX + distinct RX.
pub fn pick_usart_pins(mcu: &Mcu, used_pins: &HashSet<usize>) -> Option<(u8, usize, usize)> {
    let mut tx: Candidates = BTreeMap::new();
    let mut rx: Candidates = BTreeMap::new();

    for pin in mcu.iter_all_pins().filter(|p| !p.reserved) {
        if used_pins.contains(&pin.number) {
            continue;
        }
        let free = pin.selected_function == PinFunction::Unset;
        for f in &pin.available_functions {
            match f {
                PinFunction::UsartTx(n) => {
                    let assigned = pin.selected_function == PinFunction::UsartTx(*n);
                    if free || assigned {
                        tx.entry(*n).or_default().push((pin.number, assigned));
                    }
                }
                PinFunction::UsartRx(n) => {
                    let assigned = pin.selected_function == PinFunction::UsartRx(*n);
                    if free || assigned {
                        rx.entry(*n).or_default().push((pin.number, assigned));
                    }
                }
                _ => {}
            }
        }
    }

    // First try instances where the user already wired TX *and* RX (reuse their
    // setup); otherwise the lowest instance with a free TX + distinct RX.
    choose(&tx, &rx, true).or_else(|| choose(&tx, &rx, false))
}

fn choose(tx: &Candidates, rx: &Candidates, require_assigned: bool) -> Option<(u8, usize, usize)> {
    for (&n, txs) in tx {
        let Some(rxs) = rx.get(&n) else {
            continue;
        };
        let keep = |(_, a): &&(usize, bool)| !require_assigned || *a;
        for &(t, _) in txs.iter().filter(keep) {
            if let Some(&(r, _)) = rxs.iter().filter(keep).find(|(p, _)| *p != t) {
                return Some((n, t, r));
            }
        }
    }
    None
}
