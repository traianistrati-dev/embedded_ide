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

use super::super::watchdog::WatchdogSettings;

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

/// The `pins/configs/*.rs` files the watchdog settings call for.
///
/// `family` decides what is even possible: `stm32f1xx-hal` has no window
/// watchdog, so a WWDG configured before a chip change would otherwise generate
/// a file that cannot compile. Dropping it here rather than in the UI means the
/// invariant holds however the settings got into the model.
pub fn config_files(w: &WatchdogSettings, family: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(i) = w.iwdg {
        out.push((
            "iwdg.rs".to_owned(),
            IWDG_TMPL.replace("{TIMEOUT}", &i.timeout_us.to_string()),
        ));
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
    let mut s = String::new();
    if w.iwdg.is_some() {
        s.push_str("    // Configured, NOT started - call unleash() when ready.\n");
        s.push_str("    let mut _iwdg = pins::configs::iwdg::init(p.IWDG);\n");
    }
    if w.wwdg.is_some() && super::super::watchdog::wwdg_supported(family) {
        s.push_str("    // Running from this line on; pet() too early also resets.\n");
        s.push_str("    let mut _wwdg = pins::configs::wwdg::init(p.WWDG);\n");
    }
    if s.is_empty() {
        return s;
    }
    format!("\n    // ── Watchdogs ──\n{s}")
}

#[cfg(test)]
mod tests {
    use super::super::super::watchdog::{IwdgConfig, WatchdogSettings, WwdgConfig};
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
