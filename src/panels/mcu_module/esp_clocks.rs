//! Which CPU frequencies an Espressif part can actually be set to.
//!
//! Small, and load-bearing. `esp_hal::Config::with_cpu_clock` takes a per-chip
//! `CpuClock` enum whose variants differ between parts — and a variant that does
//! not exist is a compile error in the user's project, not a warning here.
//!
//! # Where these come from
//!
//! `esp-hal/src/soc/<chip>/clocks.rs`, one `pub enum CpuClock` per chip. Not
//! from a datasheet: the number that matters is the one the generated code has
//! to name, and only the HAL decides that.
//!
//! ```text
//! esp32c2   80, 120
//! esp32c3   80, 160
//! esp32c6   80, 160
//! esp32h2   96          <- the reason this file exists
//! ```
//!
//! The ESP32-H2 has neither `_80MHz` nor `_160MHz`, which were the only two
//! names the generator could produce before. Its single option is `_96MHz`.

/// Selectable CPU frequencies in MHz, lowest first, or empty for a chip nothing
/// here knows about.
///
/// Every RISC-V part the IDE ships is here; the numbers come from `esp-hal`, so
/// a chip missing from this table is one whose CPU clock the generator cannot
/// name and which therefore falls back to `CpuClock::max()`.
pub fn cpu_options(chip: &str) -> &'static [u32] {
    match chip {
        "esp32c2" => &[80, 120],
        "esp32c3" | "esp32c6" | "esp32c61" => &[80, 160],
        "esp32c5" => &[80, 160, 240],
        "esp32h2" => &[96],
        // Xtensa. Same three everywhere, and unlike the RISC-V parts their
        // metadata states no PLL that divides into 240 MHz — so they get no
        // derived clock tree, only this ceiling. See `esp_gen::clock_graph`.
        "esp32" | "esp32s2" | "esp32s3" => &[80, 160, 240],
        _ => &[],
    }
}

/// The fastest this chip runs, for the System tab and the chip filter.
pub fn max_mhz(chip: &str) -> Option<u32> {
    cpu_options(chip).last().copied()
}

/// The `CpuClock` variant to name in generated code for a wanted frequency.
///
/// Picks the closest option the chip HAS rather than the nearest round number:
/// asking an H2 for 160 MHz has to produce `_96MHz`, because `_160MHz` is not a
/// variant of its enum and the project would not compile.
///
/// `None` when the chip is unknown, which is the caller's cue to fall back to
/// `CpuClock::max()` — always valid, whatever the part.
pub fn cpu_variant(chip: &str, wanted_mhz: u32) -> Option<String> {
    let opts = cpu_options(chip);
    if opts.is_empty() {
        return None;
    }
    let best = opts
        .iter()
        .min_by_key(|o| o.abs_diff(wanted_mhz))
        .copied()
        .expect("non-empty");
    Some(format!("esp_hal::clock::CpuClock::_{best}MHz"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_chip_has_options() {
        for chip in ["esp32c2", "esp32c3", "esp32c6", "esp32h2"] {
            assert!(!cpu_options(chip).is_empty(), "{chip}");
            assert!(max_mhz(chip).is_some(), "{chip}");
        }
        assert!(cpu_options("stm32f1").is_empty());
        assert_eq!(cpu_variant("stm32f1", 72), None);
    }

    #[test]
    fn options_are_sorted_so_the_last_is_the_maximum() {
        for chip in [
            "esp32c2", "esp32c3", "esp32c5", "esp32c6", "esp32c61", "esp32h2",
        ] {
            let o = cpu_options(chip);
            assert!(o.windows(2).all(|w| w[0] < w[1]), "{chip}: {o:?}");
        }
        assert_eq!(max_mhz("esp32c2"), Some(120));
        assert_eq!(max_mhz("esp32h2"), Some(96));
    }

    /// The bug this table exists to prevent.
    #[test]
    fn a_chip_is_never_given_a_variant_it_lacks() {
        // An H2 asked for the old hard-coded 160 must not be told "_160MHz".
        assert_eq!(
            cpu_variant("esp32h2", 160).as_deref(),
            Some("esp_hal::clock::CpuClock::_96MHz")
        );
        assert_eq!(
            cpu_variant("esp32h2", 80).as_deref(),
            Some("esp_hal::clock::CpuClock::_96MHz"),
            "one option means one answer"
        );
        // A C2 has 120, not 160.
        assert_eq!(
            cpu_variant("esp32c2", 160).as_deref(),
            Some("esp_hal::clock::CpuClock::_120MHz")
        );
        // And the C3 keeps behaving exactly as it did.
        assert_eq!(
            cpu_variant("esp32c3", 160).as_deref(),
            Some("esp_hal::clock::CpuClock::_160MHz")
        );
        assert_eq!(
            cpu_variant("esp32c3", 80).as_deref(),
            Some("esp_hal::clock::CpuClock::_80MHz")
        );
    }

    #[test]
    fn the_closest_option_wins_not_the_lower_one() {
        // 130 is nearer 160 than 80, even though it is below it.
        assert_eq!(
            cpu_variant("esp32c3", 130).as_deref(),
            Some("esp_hal::clock::CpuClock::_160MHz")
        );
        assert_eq!(
            cpu_variant("esp32c3", 110).as_deref(),
            Some("esp_hal::clock::CpuClock::_80MHz")
        );
    }

    /// Against the HAL these numbers were read from. Ignored: needs esp-hal in
    /// the cargo registry.
    ///
    /// `cargo test -- --ignored the_table_matches_esp_hal --nocapture`
    #[test]
    #[ignore]
    fn the_table_matches_esp_hal() {
        let Some(home) = std::env::var_os("CARGO_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .or_else(|| std::env::var_os("HOME"))
                    .map(|h| std::path::PathBuf::from(h).join(".cargo"))
            })
        else {
            return;
        };
        let src = home.join("registry").join("src");
        let Some(hal) = std::fs::read_dir(&src).ok().and_then(|regs| {
            let mut found: Vec<std::path::PathBuf> = regs
                .flatten()
                .flat_map(|r| std::fs::read_dir(r.path()).into_iter().flatten().flatten())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("esp-hal-1."))
                })
                .collect();
            found.sort();
            found.pop()
        }) else {
            eprintln!("no esp-hal in the cargo registry — skipping");
            return;
        };
        println!("checking against {}", hal.display());
        for chip in [
            "esp32c2", "esp32c3", "esp32c5", "esp32c6", "esp32c61", "esp32h2",
        ] {
            let path = hal.join("src").join("soc").join(chip).join("clocks.rs");
            let Ok(text) = std::fs::read_to_string(&path) else {
                eprintln!("{chip}: no clocks.rs — skipping");
                continue;
            };
            // `    _160MHz = 160,` — the enum's own numbering.
            let mut found: Vec<u32> = text
                .lines()
                .filter_map(|l| {
                    let t = l.trim();
                    let name = t.strip_prefix('_')?;
                    let mhz = name.split("MHz").next()?;
                    // Discriminants only: a doc comment has no `=`.
                    t.contains('=').then(|| mhz.parse::<u32>().ok())?
                })
                .collect();
            found.sort_unstable();
            found.dedup();
            println!("{chip:<9} esp-hal says {found:?}");
            assert_eq!(found, cpu_options(chip), "{chip} drifted from esp-hal");
        }
    }
}
