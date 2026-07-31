//! Auto-wiring: pick compatible MCU pins for a new module.
//!
//! Finds a peripheral instance whose required signals all map to distinct
//! free/assigned pins (optional signals added when available), preferring pins
//! the user already assigned, and never reusing a pin taken by another module.

use std::collections::HashSet;

use super::ModuleSignal;
use crate::panels::mcu_module::mcu::model::Mcu;
use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

/// Choose `(instance, [(signal, pin_number), …])` for a new module. `required`
/// signals must all be satisfiable on the same instance with distinct pins;
/// `optional` ones (e.g. SPI NSS) are added when a pin is available. Pins in
/// `used` (wired to another module) are skipped, and peripheral instances in
/// `used_instances` (already hosting a module of this kind) are skipped entirely
/// — otherwise a 2nd "+_SPI" would re-pick SPI1 on its alternate pins instead
/// of moving to SPI2. Tries instances 0..=3, preferring instances where the pins
/// are already assigned to the signal.
pub fn pick_pins(
    mcu: &Mcu,
    used: &HashSet<usize>,
    used_instances: &HashSet<u8>,
    required: &[ModuleSignal],
    optional: &[ModuleSignal],
) -> Option<(u8, Vec<(ModuleSignal, usize)>)> {
    for require_assigned in [true, false] {
        for inst in 0u8..=3 {
            if used_instances.contains(&inst) {
                continue;
            }
            if let Some(chosen) =
                try_instance(mcu, used, required, optional, inst, require_assigned)
            {
                return Some((inst, chosen));
            }
        }
    }
    None
}

fn try_instance(
    mcu: &Mcu,
    used: &HashSet<usize>,
    required: &[ModuleSignal],
    optional: &[ModuleSignal],
    inst: u8,
    require_assigned: bool,
) -> Option<Vec<(ModuleSignal, usize)>> {
    let find = |sig: ModuleSignal, taken: &HashSet<usize>| -> Option<usize> {
        let want = sig.pin_function(inst);
        mcu.iter_all_pins()
            .filter(|p| !p.reserved && !taken.contains(&p.number))
            .find(|p| {
                p.available_functions.contains(&want)
                    && (p.selected_function == want
                        || (!require_assigned && p.selected_function == PinFunction::Unset))
            })
            .map(|p| p.number)
    };

    let mut taken: HashSet<usize> = used.clone();
    let mut chosen: Vec<(ModuleSignal, usize)> = Vec::new();

    for &sig in required {
        let pin = find(sig, &taken)?;
        taken.insert(pin);
        chosen.push((sig, pin));
    }
    for &sig in optional {
        if let Some(pin) = find(sig, &taken) {
            taken.insert(pin);
            chosen.push((sig, pin));
        }
    }
    Some(chosen)
}
