//! Persist virtual modules to a one-line `// @modules <ron>` marker in
//! `main.rs`, parsed back on project open — a lossless round-trip of the full
//! module state (config + connections + data models), mirroring the `@clock`
//! marker. RON serialises to a single line (newlines in data models are
//! escaped), so it fits in a comment.

use super::{ModuleConfig, VirtualModule};

/// Marker prefix for the modules comment line.
pub const MODULES_TAG: &str = "// @modules";

/// The `// @modules <ron>` line for `modules`, or `None` when there are none.
pub fn marker_line(modules: &[VirtualModule]) -> Option<String> {
    if modules.is_empty() {
        return None;
    }
    let ron = ron::to_string(modules).ok()?;
    Some(format!("{MODULES_TAG} {ron}"))
}

/// Parse the `// @modules …` line from a saved `main.rs` back into modules.
/// Returns an empty vec when the marker is absent or unparseable.
pub fn parse_from_source(source: &str) -> Vec<VirtualModule> {
    source
        .lines()
        .find_map(|l| {
            let rest = l.trim_start().strip_prefix(MODULES_TAG)?;
            let mut modules = ron::from_str::<Vec<VirtualModule>>(rest.trim_start()).ok()?;
            // The one place a saved project re-enters the model, so the one
            // place old field shapes are brought forward.
            for m in &mut modules {
                if let ModuleConfig::Timer(cfg) = &mut m.config {
                    cfg.migrate_duty();
                }
            }
            Some(modules)
        })
        .unwrap_or_default()
}

/// Refresh the `@modules` marker in `code` so it reflects `modules` (placed just
/// after the header's first line). Removes the marker when there are no modules.
pub fn with_marker(code: &str, modules: &[VirtualModule]) -> String {
    let stripped = strip_marker(code);
    match marker_line(modules) {
        Some(line) => insert_after_first_line(&stripped, &line),
        None => stripped,
    }
}

fn strip_marker(code: &str) -> String {
    let Some(start) = code.find(MODULES_TAG) else {
        return code.to_owned();
    };
    let end = code[start..]
        .find('\n')
        .map(|i| start + i + 1)
        .unwrap_or(code.len());
    let mut s = String::with_capacity(code.len());
    s.push_str(&code[..start]);
    s.push_str(&code[end..]);
    s
}

fn insert_after_first_line(code: &str, line: &str) -> String {
    match code.find('\n') {
        Some(i) => format!("{}{}\n{}", &code[..=i], line, &code[i + 1..]),
        None => format!("{code}\n{line}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::modules::{
        Connection, ModuleConfig, ModuleKind, ModuleSignal, TimerModuleConfig, UsartModuleConfig,
    };

    /// A project saved before duty was stored in hundredths keeps its duty:
    /// the whole-percent map is folded in on the way back, so 75 % stays 75 %
    /// instead of collapsing to 0.75 %.
    #[test]
    fn a_pre_hundredths_duty_is_migrated_on_load() {
        let mut cfg = TimerModuleConfig::new(2);
        cfg.freq_hz = 20_000;
        // Exactly what an older version wrote: the legacy field, nothing else.
        cfg.duty.insert(3, 75);
        let old = VirtualModule {
            id: "_pwm_2".into(),
            kind: ModuleKind::GenericInterfaceTimer,
            name: "PWM2".into(),
            pos: (0.0, 0.0),
            config: ModuleConfig::Timer(cfg),
            connections: vec![Connection {
                signal: ModuleSignal::PwmCh3,
                mcu_pin: 12,
            }],
        };

        let back = parse_from_source(&marker_line(&[old]).unwrap());
        let ModuleConfig::Timer(cfg) = &back[0].config else {
            panic!("expected a timer module");
        };
        assert_eq!(cfg.duty_x100_of(3), 7_500, "75 % is 7500 hundredths");
        assert_eq!(cfg.duty_percent_of(3), 75.0);
        assert!(
            cfg.duty.is_empty(),
            "the legacy map is consumed, not carried forward"
        );

        // …and saving it again writes only the new field, so the next load has
        // nothing left to migrate.
        let again = parse_from_source(&marker_line(&back).unwrap());
        let ModuleConfig::Timer(cfg) = &again[0].config else {
            panic!("expected a timer module");
        };
        assert_eq!(cfg.duty_x100_of(3), 7_500);
    }

    /// The per-channel output shape and the counter mode survive the
    /// `@modules` round-trip — they live in the saved project, not in the UI.
    #[test]
    fn output_settings_round_trip() {
        use crate::panels::mcu_module::modules::{
            PwmChannelConfig, PwmCounting, PwmMode, PwmOutput, PwmPolarity,
        };

        let mut cfg = TimerModuleConfig::new(1);
        cfg.counting = PwmCounting::CenterUpInterrupts;
        cfg.dead_time = 40;
        cfg.set_channel(
            2,
            PwmChannelConfig {
                output: PwmOutput::OpenDrain,
                polarity: PwmPolarity::ActiveLow,
                mode: PwmMode::Mode2,
            },
        );
        let m = VirtualModule {
            id: "_pwm_1".into(),
            kind: ModuleKind::GenericInterfaceTimer,
            name: "PWM1".into(),
            pos: (0.0, 0.0),
            config: ModuleConfig::Timer(cfg),
            connections: vec![],
        };

        let back = parse_from_source(&marker_line(&[m]).unwrap());
        let ModuleConfig::Timer(cfg) = &back[0].config else {
            panic!("expected a timer module");
        };
        assert_eq!(cfg.counting, PwmCounting::CenterUpInterrupts);
        assert_eq!(cfg.dead_time, 40);
        assert_eq!(cfg.channel_of(2).output, PwmOutput::OpenDrain);
        assert_eq!(cfg.channel_of(2).polarity, PwmPolarity::ActiveLow);
        assert_eq!(cfg.channel_of(2).mode, PwmMode::Mode2);
        // A channel nobody touched reads as every default.
        assert_eq!(cfg.channel_of(1), PwmChannelConfig::default());
    }

    /// A value the new field already carries wins: a half-migrated config
    /// cannot be dragged back to the coarser number.
    #[test]
    fn migration_never_overwrites_a_hundredths_value() {
        let mut cfg = TimerModuleConfig::new(2);
        cfg.set_duty_x100(3, 750);
        cfg.duty.insert(3, 75);
        cfg.migrate_duty();
        assert_eq!(cfg.duty_x100_of(3), 750, "7.5 % survives, not 75 %");
    }

    fn sample() -> VirtualModule {
        let mut cfg = UsartModuleConfig::new(1);
        cfg.baud_rate = 9600;
        cfg.rx_model = "pub struct R {\n    pub t: f32,\n}".into(); // multi-line on purpose
        VirtualModule {
            id: "_usart_1".into(),
            kind: ModuleKind::GenericInterfaceUsart,
            name: "USART1".into(),
            pos: (12.0, 34.0),
            config: ModuleConfig::Usart(cfg),
            connections: vec![
                Connection {
                    signal: ModuleSignal::Tx,
                    mcu_pin: 30,
                },
                Connection {
                    signal: ModuleSignal::Rx,
                    mcu_pin: 31,
                },
            ],
        }
    }

    #[test]
    fn marker_round_trips_through_source() {
        let modules = vec![sample()];
        let line = marker_line(&modules).unwrap();
        assert!(line.starts_with(MODULES_TAG));
        assert!(!line.contains('\n'), "marker must be a single line");

        let source = format!("// Auto-generated\n{line}\n#![no_std]\n");
        let parsed = parse_from_source(&source);
        assert_eq!(parsed, modules, "lossless round-trip");
    }

    #[test]
    fn with_marker_inserts_and_is_idempotent() {
        let modules = vec![sample()];
        let base = "// Auto-generated by Embedded IDE\n// MCU: x\n#![no_std]\n".to_owned();
        let once = with_marker(&base, &modules);
        assert!(once.contains(MODULES_TAG));
        // Re-applying keeps exactly one marker (no growth).
        let twice = with_marker(&once, &modules);
        assert_eq!(twice.matches(MODULES_TAG).count(), 1);
        assert_eq!(once, twice);
    }

    #[test]
    fn with_marker_removes_when_empty() {
        let with = with_marker("// hdr\n#![no_std]\n", &[sample()]);
        let without = with_marker(&with, &[]);
        assert!(!without.contains(MODULES_TAG));
        assert_eq!(without, "// hdr\n#![no_std]\n");
    }
}
