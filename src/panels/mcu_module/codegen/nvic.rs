//! The chip's interrupt vector names, and which peripheral each one serves.
//!
//! `bind_interrupts!` is keyed by VECTOR, not by peripheral, and ST does not
//! give every peripheral its own. The generated code used to assume it did:
//!
//! * `I2C1_EV` + `I2C1_ER` — true on F0/F1/F3/F4/F7/G4/H7…, but an STM32G0,
//!   C0, L0 or U0 has ONE `I2C1` vector carrying both, and the split names do
//!   not exist. `bind_interrupts!` then fails with "cannot find type `I2C1_EV`
//!   in module `$crate::interrupt::typelevel`", in generated code the user
//!   cannot fix by editing.
//! * `USART3` — an STM32G0 routes USART3, USART4 and LPUART1 through a single
//!   `USART3_4_LPUART1` vector.
//!
//! Both are answered by the same `NVIC-<ver>_Modes.xml` the DMA channels come
//! from (see [`super::dma_data`]), so the vector list is captured at import and
//! stored on the chip. Without it — a built-in chip, or one imported before
//! this existed — every function here answers `None` and the caller keeps the
//! split/plain names, which is what the families that dominate the registry
//! actually use.

/// Every interrupt vector the chip has, by name: `I2C1_EV`, `USART3_4_LPUART1`,
/// `DMA1_Channel2_3`. Order and spelling are the vendor's.
pub fn vectors(nvic_xml: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for chunk in nvic_xml.split("Value=\"").skip(1) {
        // `Value="I2C1_IRQn:Y,…"` - anything before the first `"` that is not a
        // vector name has no `_IRQn` and is skipped by the emptiness check.
        if let Some(name) = chunk.split("_IRQn").next()
            && !name.is_empty()
            && !name.contains('"')
            && !v.iter().any(|x| x == name)
        {
            v.push(name.to_owned());
        }
    }
    v
}

/// The `(peripheral, instance)` pairs a vector name covers.
///
/// A vector is an `_`-separated list where a bare number continues the previous
/// peripheral: `USART3_4_LPUART1` is USART3, USART4 and LPUART1; `I2C2_3` is
/// I2C2 and I2C3. Segments that are not `<letters><digits>` (`EV`, `ER`, `OVR`,
/// `DAC`) carry no instance and are skipped — they are what distinguishes two
/// vectors of the same peripheral, not what selects it.
fn covered(vector: &str) -> Vec<(&str, u8)> {
    let mut out = Vec::new();
    let mut base: Option<&str> = None;
    for seg in vector.split('_') {
        let digits = seg.len() - seg.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            // `EV`, `ER`, `OVR`, `DAC`: a qualifier, not a selector. It also
            // ends any range, so `TIM6_DAC_LPTIM1` cannot read as TIM6+TIM1.
            base = None;
            continue;
        }
        let (name, num) = seg.split_at(seg.len() - digits);
        let Ok(n) = num.parse::<u8>() else {
            base = None;
            continue;
        };
        if name.is_empty() {
            // A bare number continues the peripheral named before it.
            if let Some(b) = base {
                out.push((b, n));
            }
        } else {
            base = Some(name);
            out.push((name, n));
        }
    }
    out
}

/// Does `vector` serve `periph` instance `n`?
fn serves(vector: &str, periph: &str, n: u8) -> bool {
    covered(vector).iter().any(|(p, i)| *p == periph && *i == n)
}

/// The vector serving `periph{n}`, e.g. `usart_style("USART", 3)` on an STM32G0
/// is `USART3_4_LPUART1` rather than `USART3`.
///
/// `None` when the chip carries no vector list, or when nothing in it matches —
/// the caller then keeps the plain `{periph}{n}` name.
pub fn vector_for<'a>(vectors: &'a [String], periph: &str, n: u8) -> Option<&'a str> {
    vectors
        .iter()
        .find(|v| serves(v, periph, n))
        .map(String::as_str)
}

/// How a chip delivers one I2C peripheral's interrupts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum I2cIrqs {
    /// Two vectors, one per handler — the majority of families.
    Split { ev: String, er: String },
    /// One vector carrying both handlers (G0, C0, L0, U0, WL…).
    Combined(String),
}

/// The vector(s) for `I2C{n}`, or `None` when the chip's list does not say.
///
/// The split form is recognised by ST's own `_EV` / `_ER` suffixes; anything
/// else that serves the instance is a combined vector. A chip that has only one
/// of the two suffixed vectors is treated as unknown rather than guessed at.
pub fn i2c_irqs(vectors: &[String], n: u8) -> Option<I2cIrqs> {
    let mine: Vec<&str> = vectors
        .iter()
        .map(String::as_str)
        .filter(|v| serves(v, "I2C", n))
        .collect();
    let ev = mine.iter().find(|v| v.ends_with("_EV"));
    let er = mine.iter().find(|v| v.ends_with("_ER"));
    match (ev, er) {
        (Some(ev), Some(er)) => Some(I2cIrqs::Split {
            ev: (*ev).to_owned(),
            er: (*er).to_owned(),
        }),
        _ => mine
            .iter()
            .find(|v| !v.ends_with("_EV") && !v.ends_with("_ER"))
            .map(|v| I2cIrqs::Combined((*v).to_owned())),
    }
}

/// The chip's vector names, read from the `NVIC*-<ver>_Modes.xml` next to its
/// `.xml` — the import-time counterpart of [`super::dma_data::dma_def_for`],
/// with the same per-version cache (one NVIC table serves a whole family).
///
/// Empty when the file is not there, which is the case for every chip imported
/// from the public open-pin-data repo.
pub fn vectors_for(
    mcu_xml: &str,
    mcu_dir: Option<&std::path::Path>,
    cache: &mut std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    let Some(dir) = mcu_dir.map(|d| d.join("IP")) else {
        return Vec::new();
    };
    let Some((name, ver)) = super::dma_data::nvic_ip(mcu_xml) else {
        return Vec::new();
    };
    let key = format!("{name}-{ver}");
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }
    let list = super::dma_data::modes_file_names(&name, &ver)
        .into_iter()
        .find_map(|f| std::fs::read_to_string(dir.join(f)).ok())
        .map(|xml| vectors(&xml))
        .unwrap_or_default();
    cache.insert(key, list.clone());
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_vector_name_lists_every_instance_it_covers() {
        assert_eq!(
            covered("USART3_4_LPUART1"),
            [("USART", 3), ("USART", 4), ("LPUART", 1)]
        );
        assert_eq!(covered("I2C2_3"), [("I2C", 2), ("I2C", 3)]);
        assert_eq!(covered("I2C1_EV"), [("I2C", 1)], "EV selects nothing");
        assert_eq!(covered("SPI1"), [("SPI", 1)]);
        assert_eq!(covered("TIM6_DAC_LPTIM1"), [("TIM", 6), ("LPTIM", 1)]);
    }

    /// The STM32G0 shape — one vector for both halves of I2C1.
    #[test]
    fn a_combined_i2c_vector_is_recognised() {
        let g0 = v(&["I2C1", "I2C2_3", "USART1", "USART3_4_LPUART1"]);
        assert_eq!(i2c_irqs(&g0, 1), Some(I2cIrqs::Combined("I2C1".into())));
        assert_eq!(i2c_irqs(&g0, 3), Some(I2cIrqs::Combined("I2C2_3".into())));
        assert_eq!(i2c_irqs(&g0, 4), None, "no such instance");
    }

    /// …and the shape every other family has, which must keep working.
    #[test]
    fn a_split_i2c_vector_stays_split() {
        let f4 = v(&["I2C1_EV", "I2C1_ER", "I2C2_EV", "I2C2_ER", "USART1"]);
        assert_eq!(
            i2c_irqs(&f4, 2),
            Some(I2cIrqs::Split {
                ev: "I2C2_EV".into(),
                er: "I2C2_ER".into()
            })
        );
    }

    #[test]
    fn a_shared_usart_vector_is_found_by_instance() {
        let g0 = v(&["USART1", "USART2_LPUART2", "USART3_4_LPUART1"]);
        assert_eq!(vector_for(&g0, "USART", 1), Some("USART1"));
        assert_eq!(vector_for(&g0, "USART", 2), Some("USART2_LPUART2"));
        assert_eq!(vector_for(&g0, "USART", 4), Some("USART3_4_LPUART1"));
        assert_eq!(vector_for(&g0, "USART", 6), None);
        // An empty list is the "chip does not say" case, not a wrong answer.
        assert_eq!(vector_for(&[], "USART", 1), None);
    }

    /// `USART1` must not be answered by `USART10`, nor the other way round.
    #[test]
    fn instance_numbers_match_whole() {
        let h7 = v(&["USART10", "USART1"]);
        assert_eq!(vector_for(&h7, "USART", 1), Some("USART1"));
        assert_eq!(vector_for(&h7, "USART", 10), Some("USART10"));
    }
}
