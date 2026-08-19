//! Watchdog configuration (IWDG + WWDG) — the Configuration tab's model.
//!
//! # Why this is not a grid of register fields
//!
//! CubeMX exposes the registers: prescaler, window value, free-running
//! downcounter. embassy exposes DURATIONS —
//! `WindowWatchdog::new(peri, timeout_us, window_us)` — and derives the
//! registers itself, at runtime, from the live PCLK1. There is no API that
//! accepts register values, so the IDE's surface has to be time.
//!
//! # What that costs, and what this module buys back
//!
//! Time is the honest surface but it is not self-validating: a duration the
//! hardware cannot express is not a compile error, it is a **panic at boot** —
//! `assert!(window_us < timeout_us)`, or the `unwrap!` behind "WWDG: timeout_us
//! is out of range for all prescalers". Neither shows up in `cargo check`.
//!
//! So this module reproduces embassy's arithmetic exactly (integer division and
//! all) to answer, before any code is generated: what range can this chip
//! actually reach, and is the value the user typed inside it.
//!
//! # Three facts that are per-family, and were checked rather than assumed
//!
//! * **LSI** — the IWDG's clock — is 40 kHz on F0/F1/F3 and 32 kHz everywhere
//!   else (embassy `rcc/bd.rs`).
//! * **IWDG prescaler** tops out at 256, except on `iwdg_v3` (H5/U5/WBA) where
//!   it reaches 1024.
//! * **WWDG prescaler** tops out at 8 on `wwdg_v1` and 128 on `wwdg_v2`. v1
//!   covers F1/F2/F3/F4/F7 **and L4**, which is the one that would have been
//!   guessed wrong.

use serde::{Deserialize, Serialize};

/// The IWDG counter is 12-bit; its maximum reload value (embassy `MAX_RL`).
const MAX_RL: u32 = 0xFFF;
/// The WWDG counter divides PCLK1 by 4096 before the prescaler.
const WWDG_DIV: u64 = 4096;

/// What one chip family's watchdog hardware can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchdogLimits {
    /// The IWDG's clock. NOT configurable — that is the point of the
    /// "independent" watchdog, and why its range never moves with the Clock tab.
    pub lsi_hz: u32,
    /// Largest IWDG prescaler divisor.
    pub iwdg_max_prescaler: u32,
    /// Largest WWDG prescaler multiplier.
    pub wwdg_max_prescaler: u64,
    /// `false` on families whose HAL has no window watchdog at all — see
    /// [`wwdg_supported`].
    pub wwdg_available: bool,
}

/// Does this family's HAL expose a WWDG driver?
///
/// `stm32f1xx-hal` 0.10 — the HAL the F1 blocking and RTIC backends use — has
/// only `IndependentWatchdog`. Every embassy family has both. So on F1 the
/// Configuration group is HALF live, which is exactly why the WWDG card is shown
/// disabled with the reason rather than hidden: a control that silently vanishes
/// on one chip is harder to understand than one that explains itself.
pub fn wwdg_supported(family: &str) -> bool {
    family != "stm32f1"
}

/// Is watchdog code actually GENERATED for this family yet?
///
/// Separate from [`wwdg_supported`], which is about the HAL. This is about
/// the IDE: only the two embassy STM32 backends (blocking and async) call
/// `watchdog_gen`, so on F1, WBA and RTIC the tab would accept a setting and
/// produce nothing. A control that silently does nothing is worse than one
/// that says it is not wired yet, so the tab asks this first.
pub fn codegen_supported(family: &str) -> bool {
    family.starts_with("stm32") && family != "stm32f1" && family != "stm32wba"
}

/// The watchdog limits for an IDE family key (`stm32f4`, `stm32g0`, …).
///
/// An unknown family gets the conservative common case (32 kHz LSI, /256, WWDG
/// /8): every value it then offers is valid on any STM32, so a chip this does
/// not know about is under-served rather than mis-served.
pub fn limits_for(family: &str) -> WatchdogLimits {
    WatchdogLimits {
        // 40 kHz only on the three oldest families.
        lsi_hz: match family {
            "stm32f0" | "stm32f1" | "stm32f3" => 40_000,
            _ => 32_000,
        },
        // `iwdg_v3` — H5, U5, WBA — reaches /1024.
        iwdg_max_prescaler: match family {
            "stm32h5" | "stm32u5" | "stm32wba" => 1024,
            _ => 256,
        },
        // `wwdg_v2` reaches /128. Note L4 is v1, unlike its neighbours.
        wwdg_max_prescaler: match family {
            "stm32g0" | "stm32g4" | "stm32c0" | "stm32h5" | "stm32u5" | "stm32wba" => 128,
            _ => 8,
        },
        wwdg_available: wwdg_supported(family),
    }
}

/// embassy's `get_timeout_us`, integer division included.
///
/// Reproduced rather than approximated: `lsi_hz / prescaler` truncates, and at
/// /1024 on a 32 kHz LSI that is 31 Hz, not 31.25 — a 1 % difference in the
/// advertised maximum. Showing a range the driver would reject is the failure
/// this whole module exists to avoid.
fn iwdg_timeout_us(lsi_hz: u32, prescaler: u32, reload: u32) -> u32 {
    let ticks_hz = (lsi_hz / prescaler).max(1);
    (1_000_000u64 * (reload as u64 + 1) / ticks_hz as u64).min(u32::MAX as u64) as u32
}

/// The IWDG timeouts this chip can express, in microseconds.
///
/// The floor is one tick at the smallest prescaler (/4); below it embassy's
/// `reload_value` computes `0 - 1` on a `u16` and panics. The ceiling is a full
/// 12-bit counter at the largest prescaler.
pub fn iwdg_range_us(l: &WatchdogLimits) -> (u32, u32) {
    (
        iwdg_timeout_us(l.lsi_hz, 4, 0),
        iwdg_timeout_us(l.lsi_hz, l.iwdg_max_prescaler, MAX_RL),
    )
}

/// The WWDG timeouts this chip can express at the given PCLK1, in microseconds.
///
/// `None` when PCLK1 is unknown (no clock model, or a graph that evaluates to
/// zero): a range computed from a guessed clock would be worse than none.
///
/// The floor is one counter tick — embassy rounds UP, so a shorter request is
/// silently stretched to it, which is not an error but is a surprise worth
/// showing. The ceiling is the 6-bit counter (64 ticks) at the top prescaler.
pub fn wwdg_range_us(l: &WatchdogLimits, pclk1_hz: u32) -> Option<(u32, u32)> {
    if pclk1_hz == 0 {
        return None;
    }
    let tick_us = WWDG_DIV * 1_000_000 / pclk1_hz as u64;
    let tick_us = tick_us.max(1);
    let max = (tick_us * 64 * l.wwdg_max_prescaler).min(u32::MAX as u64);
    Some((tick_us as u32, max as u32))
}

/// IWDG settings. `timeout_us` is the period; the watchdog resets the chip
/// unless petted within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IwdgConfig {
    pub timeout_us: u32,
}

/// WWDG settings.
///
/// `window_us` is the CLOSED window at the start of the period: petting during
/// it resets the chip just as surely as petting too late. `0` disables the
/// restriction, which is the safe default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WwdgConfig {
    pub timeout_us: u32,
    pub window_us: u32,
}

/// Both watchdogs, each `None` until switched on in the Configuration tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchdogSettings {
    pub iwdg: Option<IwdgConfig>,
    pub wwdg: Option<WwdgConfig>,
}

impl IwdgConfig {
    /// The default the Reset button restores: the LONGEST period the chip can
    /// express.
    ///
    /// Longest, not some middle value, because it is the least aggressive
    /// setting and the only one guaranteed to be in range on every chip — a
    /// "restore defaults" that produced a boot panic would be its own bug.
    pub fn default_for(l: &WatchdogLimits) -> Self {
        Self {
            timeout_us: iwdg_range_us(l).1,
        }
    }
}

impl WwdgConfig {
    /// Longest period, window disabled — see [`IwdgConfig::default_for`].
    ///
    /// `None` when PCLK1 is unknown, because every WWDG duration is relative to
    /// it. The tab then offers no default rather than a fabricated one.
    pub fn default_for(l: &WatchdogLimits, pclk1_hz: u32) -> Option<Self> {
        let (_, max) = wwdg_range_us(l, pclk1_hz)?;
        Some(Self {
            timeout_us: max,
            window_us: 0,
        })
    }
}

/// Why a setting would not survive to run, or `None` when it is fine.
///
/// Every message names the value AND the bound, because "out of range" without
/// the range is a dead end for the person reading it.
pub fn iwdg_problem(cfg: &IwdgConfig, l: &WatchdogLimits) -> Option<String> {
    let (lo, hi) = iwdg_range_us(l);
    (!(lo..=hi).contains(&cfg.timeout_us)).then(|| {
        format!(
            "{} is outside this chip's IWDG range {}..={} us (LSI {} kHz, prescaler up to /{})",
            cfg.timeout_us,
            lo,
            hi,
            l.lsi_hz / 1000,
            l.iwdg_max_prescaler
        )
    })
}

/// The same for the WWDG, which has two ways to fail instead of one.
pub fn wwdg_problem(cfg: &WwdgConfig, l: &WatchdogLimits, pclk1_hz: u32) -> Option<String> {
    // Checked first: it is the one that holds no matter what the clock is, and
    // it is an `assert!` in the driver rather than an `unwrap!`.
    if cfg.window_us >= cfg.timeout_us {
        return Some(format!(
            "the closed window ({} us) must be shorter than the period ({} us)",
            cfg.window_us, cfg.timeout_us
        ));
    }
    let Some((lo, hi)) = wwdg_range_us(l, pclk1_hz) else {
        return Some(
            "PCLK1 is unknown, so the achievable range cannot be checked - set the Clock tab first"
                .to_owned(),
        );
    };
    (!(lo..=hi).contains(&cfg.timeout_us)).then(|| {
        format!(
            "{} is outside this chip's WWDG range {}..={} us at PCLK1 {} MHz (prescaler up to /{})",
            cfg.timeout_us,
            lo,
            hi,
            pclk1_hz / 1_000_000,
            l.wwdg_max_prescaler
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lsi_frequency_is_per_family() {
        // Getting this wrong scales every IWDG duration by 25 %.
        assert_eq!(limits_for("stm32f1").lsi_hz, 40_000);
        assert_eq!(limits_for("stm32f3").lsi_hz, 40_000);
        assert_eq!(limits_for("stm32f4").lsi_hz, 32_000);
        assert_eq!(limits_for("stm32g4").lsi_hz, 32_000);
    }

    #[test]
    fn l4_has_the_older_window_watchdog() {
        // The one a glance at the family list would have got wrong: L4 sits
        // among the v2 families but its WWDG is v1, so its longest period is
        // sixteen times shorter than G4's.
        assert_eq!(limits_for("stm32l4").wwdg_max_prescaler, 8);
        assert_eq!(limits_for("stm32g4").wwdg_max_prescaler, 128);
    }

    #[test]
    fn the_iwdg_range_matches_embassys_own_arithmetic() {
        // 32 kHz LSI, /256, full 12-bit reload: 1e6 * 4096 / (32000/256) = 32.768 s.
        let l = limits_for("stm32f4");
        assert_eq!(iwdg_range_us(&l), (125, 32_768_000));
        // F1 runs its LSI at 40 kHz, so the same hardware reaches less time --
        // and 40000/256 truncates to 156, not 156.25, so the answer is not the
        // 26_214_400 exact arithmetic gives. Mirroring the driver's integer
        // division is the whole point; a range one step too wide would hand the
        // `unwrap!` a value we had just called valid.
        let f1 = limits_for("stm32f1");
        assert_eq!(iwdg_range_us(&f1).1, 26_256_410);
    }

    #[test]
    fn the_v3_prescaler_uses_truncating_division_like_the_driver() {
        // 32000 / 1024 = 31.25, and embassy truncates to 31 - so the real
        // maximum is 132.1 s, not the 134.2 s exact arithmetic would suggest.
        // Advertising the larger number would put the driver's `unwrap!` one
        // step past the end of our own range.
        let l = limits_for("stm32u5");
        assert_eq!(l.iwdg_max_prescaler, 1024);
        assert_eq!(iwdg_range_us(&l).1, 132_129_032);
    }

    #[test]
    fn the_wwdg_range_moves_with_pclk1() {
        let l = limits_for("stm32f4");
        // 4096 / 50 MHz = 81.92 us per tick, truncated to 81; 64 ticks x /8.
        let (lo, hi) = wwdg_range_us(&l, 50_000_000).unwrap();
        assert_eq!(lo, 81);
        assert_eq!(hi, 81 * 64 * 8);
        // Halving the clock roughly doubles every duration - which is why a
        // stored value has to be re-checked when the Clock tab changes. Only
        // ROUGHLY: 4096/25 MHz truncates to 163, not 2 x 81, so the range does
        // not scale exactly and cannot be rescaled arithmetically when the
        // clock moves. It has to be recomputed.
        let (lo2, hi2) = wwdg_range_us(&l, 25_000_000).unwrap();
        assert_eq!(lo2, 163);
        assert_eq!(hi2, 163 * 64 * 8);
        assert!(lo2 > lo && lo2 != lo * 2, "truncation, not proportion");
    }

    #[test]
    fn an_unknown_clock_yields_no_range_rather_than_a_guess() {
        assert_eq!(wwdg_range_us(&limits_for("stm32f4"), 0), None);
        assert_eq!(WwdgConfig::default_for(&limits_for("stm32f4"), 0), None);
    }

    #[test]
    fn the_defaults_are_always_inside_the_range() {
        // The property that matters for the Reset button: on every family it
        // can be pressed, it must produce something the driver accepts.
        for fam in [
            "stm32f1",
            "stm32f2",
            "stm32f4",
            "stm32f7",
            "stm32g0",
            "stm32g4",
            "stm32l4",
            "stm32u5",
            "stm32wba",
            "stm32c0",
            "unknown-family",
        ] {
            let l = limits_for(fam);
            assert_eq!(
                iwdg_problem(&IwdgConfig::default_for(&l), &l),
                None,
                "{fam}"
            );
            for pclk1 in [8_000_000, 50_000_000, 170_000_000] {
                let Some(w) = WwdgConfig::default_for(&l, pclk1) else {
                    continue;
                };
                assert_eq!(wwdg_problem(&w, &l, pclk1), None, "{fam} @ {pclk1}");
            }
        }
    }

    #[test]
    fn the_window_must_be_shorter_than_the_period() {
        let l = limits_for("stm32f4");
        let bad = WwdgConfig {
            timeout_us: 1000,
            window_us: 1000,
        };
        // Equal is not allowed either - the driver asserts strictly less.
        let msg = wwdg_problem(&bad, &l, 50_000_000).expect("equal must be refused");
        assert!(msg.contains("shorter than"), "{msg}");
    }

    #[test]
    fn problems_name_the_bound_they_broke() {
        let l = limits_for("stm32f4");
        let msg = iwdg_problem(&IwdgConfig { timeout_us: 1 }, &l).expect("1 us is too short");
        assert!(
            msg.contains("125"),
            "the message must state the range: {msg}"
        );
    }

    #[test]
    fn f1_is_the_family_without_a_window_watchdog() {
        assert!(!wwdg_supported("stm32f1"));
        assert!(!limits_for("stm32f1").wwdg_available);
        assert!(wwdg_supported("stm32f4"));
        assert!(limits_for("stm32g0").wwdg_available);
    }

    #[test]
    fn the_codegen_gate_matches_the_backend_that_implements_it() {
        // `StmEmbassyBackend::handles` is the real rule; if it ever widens,
        // this fails rather than letting the tab go quietly dead.
        for fam in [
            "stm32f2", "stm32f4", "stm32f7", "stm32g0", "stm32g4", "stm32l4",
        ] {
            assert!(codegen_supported(fam), "{fam}");
        }
        for fam in ["stm32f1", "stm32wba", "esp32c3"] {
            assert!(!codegen_supported(fam), "{fam}");
        }
    }
}
