//! Generated code for the Configuration tab's watchdogs (IWDG + WWDG).
//!
//! Two peripherals with **opposite lifecycles**, which is the thing the
//! generated code has to make obvious or people will assume they match:
//!
//! * `IndependentWatchdog::new` only CONFIGURES. Nothing bites until
//!   `.unleash()`, so the binding is `mut` and the comment says where to start
//!   it.
//! * `WindowWatchdog::new` STARTS IMMEDIATELY and can never be stopped, and a
//!   `pet()` inside the closed window resets the chip just as surely as one that
//!   comes too late.
//!
//! Neither takes a pin, so unlike every other `pins/configs/*.rs` these are
//! driven by a tab, not by the Pins canvas.

use super::super::watchdog::{EspWdtConfig, WatchdogSettings};

/// `src/pins/configs/iwdg.rs` — the STM32F1, whose HAL is not embassy.
///
/// Three differences from [`IWDG_TMPL`], all visible at the call site: it takes
/// the raw PAC peripheral, its period is in MILLISECONDS, and the reload method
/// is `feed` rather than `pet`.
const IWDG_TMPL_F1: &str = r#"// <<< GENERATED>>>
// Watchdog config (from the Configuration tab) - auto-updated; edit it there.
// MILLISECONDS: stm32f1xx-hal takes `MilliSeconds`, not microseconds.
const TIMEOUT_MS: u32 = {TIMEOUT_MS};
// <<< GENERATED END >>>

// Everything below is editable - your changes are preserved on regeneration.
//
// The INDEPENDENT watchdog runs off the LSI (40 kHz on this family), so its
// period does not move when you change the system clock. `new` only wraps the
// peripheral: nothing resets the chip until `start()` is called.
use stm32f1xx_hal::pac::IWDG;
use stm32f1xx_hal::time::MilliSeconds;
use stm32f1xx_hal::watchdog::IndependentWatchdog;

/// Handle type, so it can be stored in a struct or an RTIC resource.
pub type Handle = IndependentWatchdog;

/// Wrap the IWDG. It is NOT running yet - call `start(period())` when your
/// start-up is far enough along that you can keep feeding it.
pub fn init(iwdg: IWDG) -> Handle {
    IndependentWatchdog::new(iwdg)
}

/// The configured period, in the unit this HAL wants.
pub fn period() -> MilliSeconds {
    MilliSeconds::from_ticks(TIMEOUT_MS)
}

// -- Using the IWDG --
//
//     let mut wdg = pins::configs::iwdg::init(dp.IWDG);
//     wdg.start(pins::configs::iwdg::period());  // starts biting here
//     loop {
//         wdg.feed();                            // at least every TIMEOUT_MS
//     }
"#;

/// `src/pins/configs/iwdg.rs` — embassy families.
const IWDG_TMPL: &str = r#"// <<< GENERATED>>>
// Watchdog config (from the Configuration tab) - auto-updated; edit it there.
const TIMEOUT_US: u32 = {TIMEOUT};
// <<< GENERATED END >>>

// Everything below is editable - your changes are preserved on regeneration.
//
// The INDEPENDENT watchdog runs off the LSI, so its period does not move when
// you change the system clock. `new` only configures it: nothing resets the
// chip until `unleash()` is called.
use embassy_stm32::peripherals::IWDG;
use embassy_stm32::wdg::IndependentWatchdog;
use embassy_stm32::Peri;

/// Handle type, so it can be stored in a struct or an RTIC resource.
pub type Handle = IndependentWatchdog<'static, IWDG>;

/// Configure the IWDG. It is NOT running yet - call `unleash()` when your
/// start-up is far enough along that you can keep petting it.
pub fn init(iwdg: Peri<'static, IWDG>) -> Handle {
    IndependentWatchdog::new(iwdg, TIMEOUT_US)
}

// -- Using the IWDG --
//
//     let mut wdg = pins::configs::iwdg::init(p.IWDG);
//     wdg.unleash();                 // from here on it will reset the chip
//     loop {
//         wdg.pet();                 // at least once every TIMEOUT_US
//     }
"#;

/// `src/pins/configs/wwdg.rs` — embassy families only (see `wwdg_supported`).
const WWDG_TMPL: &str = r#"// <<< GENERATED>>>
// Watchdog config (from the Configuration tab) - auto-updated; edit it there.
const TIMEOUT_US: u32 = {TIMEOUT};
// The CLOSED window: petting during it resets the chip. 0 = no restriction.
const WINDOW_US: u32 = {WINDOW};
// <<< GENERATED END >>>

// Everything below is editable - your changes are preserved on regeneration.
//
// The WINDOW watchdog counts down from PCLK1, so its period moves with the
// Clock tab - the values above were checked against the clock configured there.
//
// Two things to know before using it:
//   * `new` STARTS it. There is no way to stop it short of a reset.
//   * petting too EARLY resets the chip, exactly like petting too late. That is
//     what the closed window means.
use embassy_stm32::peripherals::WWDG;
use embassy_stm32::wdg::WindowWatchdog;
use embassy_stm32::Peri;

/// Handle type, so it can be stored in a struct or an RTIC resource.
pub type Handle = WindowWatchdog<'static, WWDG>;

/// Configure AND START the window watchdog.
pub fn init(wwdg: Peri<'static, WWDG>) -> Handle {
    WindowWatchdog::new(wwdg, TIMEOUT_US, WINDOW_US)
}

// -- Using the WWDG --
//
//     let mut wdg = pins::configs::wwdg::init(p.WWDG);   // already running
//     loop {
//         // …work that takes at least WINDOW_US…
//         wdg.pet();
//     }
"#;

/// `src/pins/configs/rwdt.rs` — the ESP's RTC watchdog.
///
/// # Why `init` hands back the whole `Rtc`
///
/// `Rwdt` is a field of it. Moving that field out would drop the `Rtc` — and
/// with it the `LPWR` handle the rest of the low-power API needs — for the sake
/// of a shorter type. Returning the `Rtc` costs one `.rwdt.` at each call and
/// leaves sleep and the RTC clock reachable from the same binding.
const RWDT_TMPL: &str = r#"// <<< GENERATED>>>
// Watchdog config (from the Configuration tab) - auto-updated; edit it there.
const TIMEOUT_US: u64 = {TIMEOUT};
// <<< GENERATED END >>>

// Everything below is editable - your changes are preserved on regeneration.
//
// The RTC watchdog lives in the RTC power domain and counts on the RTC SLOW
// clock, an RC oscillator esp-hal calibrates at boot. So its period does not
// move with the CPU clock - but it is not crystal-accurate either, and the same
// code will not time out at exactly the same instant on two boards.
//
// `esp_hal::init()` DISABLES this watchdog on the way in, so nothing is armed
// until you call `enable()`.
use esp_hal::peripherals::LPWR;
use esp_hal::rtc_cntl::{Rtc, RwdtStage};
use esp_hal::time::Duration;

/// Handle type, so it can be stored in a struct or a task's state.
pub type Handle = Rtc<'static>;

/// Configure the RTC watchdog. It is NOT running yet - call `rwdt.enable()`
/// when your start-up is far enough along to keep feeding it.
pub fn init(lpwr: LPWR<'static>) -> Handle {
    let mut rtc = Rtc::new(lpwr);
    rtc.rwdt
        .set_timeout(RwdtStage::Stage0, Duration::from_micros(TIMEOUT_US));
    rtc
}

// -- Using the RWDT --
//
//     let mut rtc = pins::configs::rwdt::init(peripherals.LPWR);
//     rtc.rwdt.enable();             // from here on it will reset the chip
//     loop {
//         rtc.rwdt.feed();           // at least once every TIMEOUT_US
//     }
"#;

/// `src/pins/configs/mwdt{N}.rs` — a timer group's watchdog.
///
/// # Why `init` takes nothing
///
/// `Wdt<TG>` is a `PhantomData` marker: it owns no register state, and esp-hal
/// exposes `Wdt::new()` with no argument. esp-hal's own `init()` builds one
/// exactly this way — `Wdt::<TIMG0<'static>>::new().disable()` — to switch the
/// boot watchdogs off.
///
/// That matters on the ASYNC runtime, where `TimerGroup::new(peripherals.TIMG0)`
/// has already consumed the peripheral for the scheduler's timer. Taking the
/// peripheral here would not compile there; taking nothing works on both, and
/// the timer half and the watchdog half do not share a register.
const MWDT_TMPL: &str = r#"// <<< GENERATED>>>
// Watchdog config (from the Configuration tab) - auto-updated; edit it there.
const TIMEOUT_US: u64 = {TIMEOUT};
// <<< GENERATED END >>>

// Everything below is editable - your changes are preserved on regeneration.
//
// Timer group {N}'s watchdog. It counts on the APB clock, but the period above
// still means what it says: `set_timeout` reads the live clock and works out
// the prescaler itself, so changing the Clock tab does not stretch it.
//
// `esp_hal::init()` DISABLES this watchdog on the way in, so nothing is armed
// until you call `enable()`. Note that `enable()` also REWRITES the stage
// actions - stage 0 resets the system, stages 1..3 off - so call it BEFORE any
// `set_stage_action` of your own, not after.
use esp_hal::peripherals::TIMG{N};
use esp_hal::time::Duration;
use esp_hal::timer::timg::{MwdtStage, Wdt};

/// Handle type, so it can be stored in a struct or a task's state.
pub type Handle = Wdt<TIMG{N}<'static>>;

/// Configure timer group {N}'s watchdog. It is NOT running yet - call
/// `enable()` when your start-up is far enough along to keep feeding it.
pub fn init() -> Handle {
    let mut wdt = Wdt::new();
    wdt.set_timeout(MwdtStage::Stage0, Duration::from_micros(TIMEOUT_US));
    wdt
}

// -- Using MWDT{N} --
//
//     let mut wdt = pins::configs::mwdt{N}::init();
//     wdt.enable();                  // from here on it will reset the chip
//     loop {
//         wdt.feed();                // at least once every TIMEOUT_US
//     }
"#;

/// Does this chip have the second timer group? The ESP32-C2 does not.
///
/// Asked HERE and not only in the UI so that a configuration carried over from
/// another chip cannot generate a file naming a `TIMG1` that does not exist.
fn has_mwdt1(chip: &str) -> bool {
    super::super::watchdog::esp_limits_for(chip).has_mwdt1
}

/// Every ESP watchdog that is switched on, as `(file stem, settings)`.
///
/// One list read by both [`esp_config_files`] and [`esp_init_lines`], so the
/// file that is written and the line that calls it cannot disagree about which
/// watchdogs exist.
fn esp_enabled(w: &WatchdogSettings, chip: &str) -> Vec<(String, EspWdtConfig)> {
    let mut out = Vec::new();
    if let Some(c) = w.rwdt {
        out.push(("rwdt".to_owned(), c));
    }
    for (n, cfg) in [(0u8, w.mwdt0), (1, w.mwdt1)] {
        if n == 1 && !has_mwdt1(chip) {
            continue;
        }
        if let Some(c) = cfg {
            out.push((format!("mwdt{n}"), c));
        }
    }
    out
}

/// The ESP half of [`config_files`].
fn esp_config_files(w: &WatchdogSettings, chip: &str) -> Vec<(String, String)> {
    esp_enabled(w, chip)
        .into_iter()
        .map(|(stem, c)| {
            let body = if stem == "rwdt" {
                RWDT_TMPL.replace("{TIMEOUT}", &c.timeout_us.to_string())
            } else {
                MWDT_TMPL
                    .replace("{TIMEOUT}", &c.timeout_us.to_string())
                    .replace("{N}", stem.trim_start_matches("mwdt"))
            };
            (format!("{stem}.rs"), body)
        })
        .collect()
}

/// The ESP half of [`init_lines`].
fn esp_init_lines(w: &WatchdogSettings, chip: &str) -> String {
    let mut s = String::new();
    for (stem, _) in esp_enabled(w, chip) {
        if stem == "rwdt" {
            s.push_str("    // Configured, NOT started - call rwdt.enable() when ready.\n");
            s.push_str("    let mut _rtc = pins::configs::rwdt::init(peripherals.LPWR);\n");
        } else {
            s.push_str("    // Configured, NOT started - call enable() when ready.\n");
            s.push_str(&format!(
                "    let mut _{stem} = pins::configs::{stem}::init();\n"
            ));
        }
    }
    s
}

/// The `pins/configs/*.rs` files the watchdog settings call for.
///
/// `family` decides what is even possible: `stm32f1xx-hal` has no window
/// watchdog, so a WWDG configured before a chip change would otherwise generate
/// a file that cannot compile. Dropping it here rather than in the UI means the
/// invariant holds however the settings got into the model.
pub fn config_files(w: &WatchdogSettings, family: &str) -> Vec<(String, String)> {
    if super::super::watchdog::is_esp(family) {
        return esp_config_files(w, family);
    }
    let mut out = Vec::new();
    if let Some(i) = w.iwdg {
        // The F1 HAL takes milliseconds, so the stored microseconds are
        // converted HERE rather than in the model: the tab keeps one unit for
        // both families, and only the generator knows what each HAL wants.
        // Truncating is right - rounding up could step past the range.
        let body = if family == "stm32f1" {
            IWDG_TMPL_F1.replace("{TIMEOUT_MS}", &(i.timeout_us / 1_000).to_string())
        } else {
            IWDG_TMPL.replace("{TIMEOUT}", &i.timeout_us.to_string())
        };
        out.push(("iwdg.rs".to_owned(), body));
    }
    if let Some(x) = w.wwdg {
        if super::super::watchdog::wwdg_supported(family) {
            out.push((
                "wwdg.rs".to_owned(),
                WWDG_TMPL
                    .replace("{TIMEOUT}", &x.timeout_us.to_string())
                    .replace("{WINDOW}", &x.window_us.to_string()),
            ));
        }
    }
    out
}

/// The `main.rs` lines that call the above, or "" when no watchdog is enabled.
///
/// Emitted BEFORE the custom-module inits: a watchdog that is meant to catch a
/// hang during start-up is worth arming before the code that might hang.
pub fn init_lines(w: &WatchdogSettings, family: &str) -> String {
    // The ESP's names, types and lifecycles share nothing with the STM32
    // pair's, so that branch REPLACES this one rather than adding to it. Only
    // the header below is common.
    let s = if super::super::watchdog::is_esp(family) {
        esp_init_lines(w, family)
    } else {
        stm32_init_lines(w, family)
    };
    if s.is_empty() {
        return s;
    }
    format!("\n    // ── Watchdogs ──\n{s}")
}

/// The STM32 half of [`init_lines`].
fn stm32_init_lines(w: &WatchdogSettings, family: &str) -> String {
    let mut s = String::new();
    if w.iwdg.is_some() {
        if family == "stm32f1" {
            s.push_str("    // Wrapped, NOT started - call start(period()) when ready.\n");
            s.push_str("    let mut _iwdg = pins::configs::iwdg::init(dp.IWDG);\n");
        } else {
            s.push_str("    // Configured, NOT started - call unleash() when ready.\n");
            s.push_str("    let mut _iwdg = pins::configs::iwdg::init(p.IWDG);\n");
        }
    }
    if w.wwdg.is_some() && super::super::watchdog::wwdg_supported(family) {
        s.push_str("    // Running from this line on; pet() too early also resets.\n");
        s.push_str("    let mut _wwdg = pins::configs::wwdg::init(p.WWDG);\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::super::super::watchdog::{EspWdtConfig, IwdgConfig, WatchdogSettings, WwdgConfig};
    use super::*;

    fn both() -> WatchdogSettings {
        WatchdogSettings {
            iwdg: Some(IwdgConfig {
                timeout_us: 32_768_000,
            }),
            wwdg: Some(WwdgConfig {
                timeout_us: 41_472,
                window_us: 1_000,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn the_configured_durations_reach_the_generated_file() {
        let files = config_files(&both(), "stm32f4");
        let iwdg = &files.iter().find(|(n, _)| n == "iwdg.rs").unwrap().1;
        assert!(iwdg.contains("const TIMEOUT_US: u32 = 32768000;"), "{iwdg}");
        let wwdg = &files.iter().find(|(n, _)| n == "wwdg.rs").unwrap().1;
        assert!(wwdg.contains("const TIMEOUT_US: u32 = 41472;"), "{wwdg}");
        assert!(wwdg.contains("const WINDOW_US: u32 = 1000;"), "{wwdg}");
    }

    #[test]
    fn f1_gets_the_iwdg_and_not_the_wwdg() {
        // The family check lives here, not only in the UI: a WWDG configured on
        // an F4 and then carried to an F1 by a chip change would otherwise
        // generate a file referencing a driver that does not exist.
        let files = config_files(&both(), "stm32f1");
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["iwdg.rs"]);
        let calls = init_lines(&both(), "stm32f1");
        assert!(calls.contains("iwdg::init"), "{calls}");
        assert!(!calls.contains("wwdg"), "{calls}");
    }

    #[test]
    fn the_f1_gets_its_own_hal_unit_and_names() {
        // One stored value, two different generated files: the F1 HAL wants
        // MILLISECONDS and the PAC singleton off `dp`; embassy wants
        // microseconds and a `Peri` off `p`. Getting the unit wrong here is a
        // factor of a thousand, silently.
        let w = WatchdogSettings {
            iwdg: Some(IwdgConfig {
                timeout_us: 26_214_000,
            }),
            ..Default::default()
        };
        let f1 = &config_files(&w, "stm32f1")[0].1;
        assert!(f1.contains("const TIMEOUT_MS: u32 = 26214;"), "{f1}");
        assert!(f1.contains("stm32f1xx_hal::watchdog"), "{f1}");
        assert!(!f1.contains("embassy"), "{f1}");
        assert!(init_lines(&w, "stm32f1").contains("init(dp.IWDG)"));

        let f4 = &config_files(&w, "stm32f4")[0].1;
        assert!(f4.contains("const TIMEOUT_US: u32 = 26214000;"), "{f4}");
        assert!(init_lines(&w, "stm32f4").contains("init(p.IWDG)"));
    }

    fn esp_all() -> WatchdogSettings {
        WatchdogSettings {
            rwdt: Some(EspWdtConfig {
                timeout_us: 2_000_000,
            }),
            mwdt0: Some(EspWdtConfig {
                timeout_us: 1_500_000,
            }),
            mwdt1: Some(EspWdtConfig {
                timeout_us: 500_000,
            }),
            ..Default::default()
        }
    }

    /// An ESP gets its OWN watchdogs, never the STM32 pair, and each duration
    /// lands in the file that calls it.
    #[test]
    fn an_esp_gets_the_rtc_and_timer_group_watchdogs() {
        let files = config_files(&esp_all(), "esp32c3");
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["rwdt.rs", "mwdt0.rs", "mwdt1.rs"]);

        let rwdt = &files[0].1;
        assert!(rwdt.contains("const TIMEOUT_US: u64 = 2000000;"), "{rwdt}");
        assert!(rwdt.contains("Rtc::new(lpwr)"), "{rwdt}");
        assert!(rwdt.contains("RwdtStage::Stage0"), "{rwdt}");
        // Never the STM32 vocabulary.
        assert!(!rwdt.contains("embassy"), "{rwdt}");
        assert!(!rwdt.contains("unleash"), "{rwdt}");

        // Each MWDT file names its OWN timer group — the number is in the
        // type, the module path and the example, so one missed substitution
        // would compile against the wrong peripheral.
        let mwdt1 = &files[2].1;
        assert!(mwdt1.contains("const TIMEOUT_US: u64 = 500000;"), "{mwdt1}");
        assert!(mwdt1.contains("Wdt<TIMG1<'static>>"), "{mwdt1}");
        assert!(mwdt1.contains("peripherals::TIMG1"), "{mwdt1}");
        assert!(!mwdt1.contains("TIMG0"), "{mwdt1}");

        let calls = init_lines(&esp_all(), "esp32c3");
        assert!(calls.contains("rwdt::init(peripherals.LPWR)"), "{calls}");
        // `init()` takes nothing — that is what lets it work on the async
        // runtime, where TIMG0 already belongs to the scheduler.
        assert!(
            calls.contains("_mwdt0 = pins::configs::mwdt0::init();"),
            "{calls}"
        );
        assert!(
            calls.contains("_mwdt1 = pins::configs::mwdt1::init();"),
            "{calls}"
        );
        assert!(!calls.contains("IWDG"), "{calls}");
    }

    /// The ESP32-C2 has one timer group, so MWDT1 is dropped HERE and not only
    /// in the UI — a setting carried over from another chip would otherwise
    /// generate a file naming a `TIMG1` that does not exist.
    #[test]
    fn the_c2_drops_the_second_timer_group() {
        let files = config_files(&esp_all(), "esp32c2");
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["rwdt.rs", "mwdt0.rs"]);
        let calls = init_lines(&esp_all(), "esp32c2");
        assert!(calls.contains("mwdt0"), "{calls}");
        assert!(!calls.contains("mwdt1"), "{calls}");
    }

    /// The two families do not leak into each other: an STM32 setting carried
    /// to an ESP generates nothing, and the reverse holds too.
    #[test]
    fn a_setting_from_the_other_family_generates_nothing() {
        assert!(config_files(&both(), "esp32c3").is_empty());
        assert_eq!(init_lines(&both(), "esp32c3"), "");
        assert!(config_files(&esp_all(), "stm32f4").is_empty());
        assert_eq!(init_lines(&esp_all(), "stm32f4"), "");
    }

    #[test]
    fn nothing_enabled_generates_nothing() {
        let none = WatchdogSettings::default();
        assert!(config_files(&none, "stm32f4").is_empty());
        assert_eq!(init_lines(&none, "stm32f4"), "");
    }

    #[test]
    fn the_two_lifecycles_are_spelled_out_where_they_bite() {
        // The asymmetry is invisible from the call site, so it has to be in the
        // text: one is armed later, the other is already running.
        let calls = init_lines(&both(), "stm32f4");
        assert!(calls.contains("NOT started"), "{calls}");
        assert!(calls.contains("Running from this line"), "{calls}");
        let files = config_files(&both(), "stm32f4");
        let wwdg = &files.iter().find(|(n, _)| n == "wwdg.rs").unwrap().1;
        assert!(wwdg.contains("no way to stop it"), "{wwdg}");
    }
}
