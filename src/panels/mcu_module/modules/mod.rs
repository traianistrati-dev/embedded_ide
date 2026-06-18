//! Virtual electronic modules attached to the MCU (Phase 1: model + auto-wire).
//!
//! - [`model`]    — the module data types (kinds, signals, config, connections).
//! - [`autowire`] — pick compatible MCU pins for a new module.
//!
//! Modules live on the [`Mcu`](crate::panels::mcu_module::mcu::Mcu) and are added
//! via `Mcu::add_module`, which auto-wires them to USART pins and sets those pins'
//! functions (the "auto-connect" behaviour).

pub mod autowire;
pub mod model;
pub mod persist;

pub use model::{
    module_signal_of, Connection, I2cModuleConfig, ModuleConfig, ModuleKind, ModuleSignal, Parity,
    SpiModuleConfig, StopBits, UsartModuleConfig, VirtualModule,
};

use std::collections::BTreeMap;

/// USART module configs keyed by peripheral instance — consumed by codegen to
/// drive the generated USART init (baud rate, parity, stop bits).
pub fn usart_configs(modules: &[VirtualModule]) -> BTreeMap<u8, UsartModuleConfig> {
    let mut map = BTreeMap::new();
    for m in modules {
        if let ModuleConfig::Usart(c) = &m.config {
            map.insert(c.instance, c.clone());
        }
    }
    map
}

/// SPI module configs keyed by peripheral instance (mode + clock for codegen).
pub fn spi_configs(modules: &[VirtualModule]) -> BTreeMap<u8, SpiModuleConfig> {
    let mut map = BTreeMap::new();
    for m in modules {
        if let ModuleConfig::Spi(c) = &m.config {
            map.insert(c.instance, c.clone());
        }
    }
    map
}

/// I2C module configs keyed by peripheral instance (clock for codegen).
pub fn i2c_configs(modules: &[VirtualModule]) -> BTreeMap<u8, I2cModuleConfig> {
    let mut map = BTreeMap::new();
    for m in modules {
        if let ModuleConfig::I2c(c) = &m.config {
            map.insert(c.instance, c.clone());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;
    use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;

    #[test]
    fn add_usart_module_wires_a_valid_pair() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        assert_eq!(mcu.modules.len(), 1);

        let m = &mcu.modules[0];
        let n = m.instance();
        let tx = m.pin_for(ModuleSignal::Tx).unwrap();
        let rx = m.pin_for(ModuleSignal::Rx).unwrap();
        assert_ne!(tx, rx, "TX and RX must be different pins");

        // Auto-connect set the pins to the matching USART functions.
        assert_eq!(
            mcu.find_pin(tx).unwrap().selected_function,
            PinFunction::UsartTx(n)
        );
        assert_eq!(
            mcu.find_pin(rx).unwrap().selected_function,
            PinFunction::UsartRx(n)
        );
    }

    #[test]
    fn two_modules_use_distinct_instances_and_pins() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        assert_eq!(mcu.modules.len(), 2);

        let n0 = mcu.modules[0].instance();
        let n1 = mcu.modules[1].instance();
        assert_ne!(n0, n1, "second module must pick a free USART instance");

        // No MCU pin is shared between the two modules.
        let pins0: Vec<usize> = mcu.modules[0]
            .connections
            .iter()
            .map(|c| c.mcu_pin)
            .collect();
        let pins1: Vec<usize> = mcu.modules[1]
            .connections
            .iter()
            .map(|c| c.mcu_pin)
            .collect();
        assert!(pins0.iter().all(|p| !pins1.contains(p)));
    }

    /// Removing a module resets the pins it was wired to back to `Unset`.
    #[test]
    fn remove_module_resets_pins() {
        let mut mcu = create_stm32f103c8tx();
        mcu.add_module(ModuleKind::GenericInterfaceUsart);
        let tx = mcu.modules[0].pin_for(ModuleSignal::Tx).unwrap();
        let rx = mcu.modules[0].pin_for(ModuleSignal::Rx).unwrap();
        let id = mcu.modules[0].id.clone();

        mcu.remove_module(&id);

        assert!(mcu.modules.is_empty());
        assert_eq!(
            mcu.find_pin(tx).unwrap().selected_function,
            PinFunction::Unset,
            "TX pin freed"
        );
        assert_eq!(
            mcu.find_pin(rx).unwrap().selected_function,
            PinFunction::Unset,
            "RX pin freed"
        );
    }

    /// The config constant lives in the regenerated GEN block, so editing the
    /// module config updates the constant value on the next regeneration.
    #[test]
    fn const_updates_with_module_config() {
        let mut mcu = create_stm32f103c8tx();
        mcu.add_module(ModuleKind::GenericInterfaceUsart);
        let code = mcu.fresh_main_rs();
        assert!(code.contains("const USART1_BAUDRATE: u32 = 115200;"), "default:\n{code}");

        if let ModuleConfig::Usart(cfg) = &mut mcu.modules[0].config {
            cfg.baud_rate = 9600;
        }
        let code = mcu.update_main_rs(&code);
        assert!(code.contains("const USART1_BAUDRATE: u32 = 9600;"), "updated:\n{code}");
        assert!(!code.contains("115200"), "old value gone");
    }

    /// The module's USART config drives the generated `init_usartN` helper.
    #[test]
    fn module_config_drives_stm32_usart_init() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        if let ModuleConfig::Usart(cfg) = &mut mcu.modules[0].config {
            cfg.baud_rate = 9600;
            cfg.parity = Parity::Even;
            cfg.stop_bits = StopBits::Two;
        }
        let code = mcu.fresh_main_rs();
        assert!(
            code.contains("const USART1_BAUDRATE: u32 = 9600;"),
            "baud constant:\n{code}"
        );
        assert!(
            code.contains(".baudrate(USART1_BAUDRATE.bps())"),
            "init references the constant"
        );
        assert!(code.contains(".parity_even()"), "parity in init");
        assert!(
            code.contains("serial::StopBits::STOP2"),
            "stop bits in init"
        );
    }

    /// The RX/TX data model is emitted as an inline `mod <id>` and not
    /// duplicated on re-generation.
    #[test]
    fn data_model_emitted_as_inline_mod_idempotently() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        if let ModuleConfig::Usart(cfg) = &mut mcu.modules[0].config {
            cfg.rx_model = "pub struct Reading { pub temp: f32 }".into();
        }
        let id = mcu.modules[0].id.clone();

        let code = mcu.fresh_main_rs();
        assert!(
            code.contains(&format!("mod {id} {{")),
            "inline mod:\n{code}"
        );
        assert!(code.contains("pub struct Reading"), "rx model body present");

        let again = mcu.update_main_rs(&code);
        assert_eq!(
            again.matches(&format!("mod {id} {{")).count(),
            1,
            "data-model mod must not be duplicated on regen"
        );
    }

    /// Modules survive the `mcu.config` serialize → parse round-trip, so a saved
    /// project restores them exactly. (They no longer live in main.rs.)
    #[test]
    fn modules_round_trip_through_mcu_config() {
        use crate::panels::mcu_module::mcu_config;
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        if let ModuleConfig::Usart(cfg) = &mut mcu.modules[0].config {
            cfg.baud_rate = 9600;
            cfg.rx_model = "pub struct R { pub t: f32 }".into();
        }
        assert!(!mcu.fresh_main_rs().contains("@modules"), "no marker in main.rs");
        let (parsed, _) = mcu_config::parse(&mcu.mcu_config_text());
        assert_eq!(parsed, mcu.modules);
    }

    /// An empty data model emits no module block.
    #[test]
    fn empty_data_model_emits_nothing() {
        let mut mcu = create_stm32f103c8tx();
        mcu.add_module(ModuleKind::GenericInterfaceUsart);
        assert!(!mcu.fresh_main_rs().contains("// Data model for"));
    }

    /// Re-purposing a wired pin away from USART disconnects that terminal; the
    /// module stays (with its other connection).
    #[test]
    fn repurposing_pin_disconnects_module() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        assert_eq!(mcu.modules[0].connections.len(), 2);
        let tx_pin = mcu.modules[0].pin_for(ModuleSignal::Tx).unwrap();

        mcu.apply_pin_function(tx_pin, PinFunction::GpioOutput);

        assert_eq!(mcu.modules[0].connections.len(), 1, "TX wire dropped");
        assert!(mcu.modules[0].pin_for(ModuleSignal::Tx).is_none());
        assert!(
            mcu.modules[0].pin_for(ModuleSignal::Rx).is_some(),
            "RX still wired"
        );
        assert!(!mcu.modules.is_empty(), "module stays (disconnected)");
    }

    #[test]
    fn add_spi_module_wires_pins() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        let m = &mcu.modules[0];
        let n = m.instance();
        let sck = m.pin_for(ModuleSignal::Sck).unwrap();
        assert!(m.pin_for(ModuleSignal::Mosi).is_some());
        assert!(m.pin_for(ModuleSignal::Miso).is_some());
        assert_eq!(
            mcu.find_pin(sck).unwrap().selected_function,
            PinFunction::SpiSck(n)
        );
    }

    /// The config constant is emitted once and not duplicated on regeneration.
    #[test]
    fn config_constant_not_duplicated_on_regen() {
        let mut mcu = create_stm32f103c8tx();
        mcu.add_module(ModuleKind::GenericInterfaceUsart);
        let code = mcu.fresh_main_rs();
        assert_eq!(code.matches("const USART1_BAUDRATE").count(), 1);
        let again = mcu.update_main_rs(&code);
        assert_eq!(
            again.matches("const USART1_BAUDRATE").count(),
            1,
            "constant must not be duplicated on regen"
        );
    }

    /// A module's custom label is appended to its generated handle variable(s)
    /// and survives the `@modules` round-trip (persisted in the config).
    #[test]
    fn module_label_appended_to_handle_vars() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        if let ModuleConfig::Usart(cfg) = &mut mcu.modules[0].config {
            cfg.custom_label = "IMU sensor".into();
        }
        let n = mcu.modules[0].instance();
        let code = mcu.fresh_main_rs();

        // USART handles carry the sanitized label: `_tx1_imu_sensor`.
        assert!(
            code.contains(&format!("_tx{n}_imu_sensor")),
            "tx handle labelled:\n{code}"
        );
        assert!(code.contains(&format!("_rx{n}_imu_sensor")), "rx handle labelled");

        // The label persists through the mcu.config round-trip (serde on config).
        let (parsed, _) = crate::panels::mcu_module::mcu_config::parse(&mcu.mcu_config_text());
        assert_eq!(parsed, mcu.modules);
    }

    /// An SPI module label lands on the `_spiN` handle.
    #[test]
    fn spi_module_label_on_handle() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        if let ModuleConfig::Spi(cfg) = &mut mcu.modules[0].config {
            cfg.custom_label = "flash".into();
        }
        let n = mcu.modules[0].instance();
        let code = mcu.fresh_main_rs();
        assert!(code.contains(&format!("let _spi{n}_flash =")), "spi handle:\n{code}");
    }

    /// Adding GI_SPI twice must advance to SPI2 — not re-pick SPI1 on its
    /// alternate (PA5/6/7) pins, which would merge into the first module.
    /// (Regression: 2nd "+GI_SPI" wrongly grabbed PA4/5/6/7 for SPI1.)
    #[test]
    fn second_spi_module_picks_spi2_not_spi1_again() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        assert_eq!(mcu.modules.len(), 2, "two distinct SPI modules");

        let mut instances: Vec<u8> = mcu.modules.iter().map(|m| m.instance()).collect();
        instances.sort();
        assert_eq!(instances, vec![1, 2], "instances must be SPI1 and SPI2");

        // No pin is shared between the two modules.
        let pins0: Vec<usize> =
            mcu.modules[0].connections.iter().map(|c| c.mcu_pin).collect();
        let pins1: Vec<usize> =
            mcu.modules[1].connections.iter().map(|c| c.mcu_pin).collect();
        assert!(pins0.iter().all(|p| !pins1.contains(p)), "no shared pins");

        // A 3rd add fails — the F103 has only SPI1/SPI2.
        assert!(!mcu.add_module(ModuleKind::GenericInterfaceSpi));
        assert_eq!(mcu.modules.len(), 2);
    }

    #[test]
    fn add_i2c_module_wires_pins() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceI2c));
        let m = &mcu.modules[0];
        let n = m.instance();
        let scl = m.pin_for(ModuleSignal::Scl).unwrap();
        let sda = m.pin_for(ModuleSignal::Sda).unwrap();
        assert_ne!(scl, sda);
        assert_eq!(
            mcu.find_pin(scl).unwrap().selected_function,
            PinFunction::I2cScl(n)
        );
        assert_eq!(
            mcu.find_pin(sda).unwrap().selected_function,
            PinFunction::I2cSda(n)
        );
    }

    #[test]
    fn spi_config_drives_codegen() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceSpi));
        if let ModuleConfig::Spi(cfg) = &mut mcu.modules[0].config {
            cfg.mode = 3;
            cfg.clock_hz = 4_000_000;
        }
        let code = mcu.fresh_main_rs();
        assert!(code.contains("Polarity::IdleHigh"), "mode 3 polarity:\n{code}");
        assert!(code.contains("Phase::CaptureOnSecondTransition"), "mode 3 phase");
        assert!(code.contains("const SPI1_CLOCK_KHZ: u32 = 4000;"), "spi clock const");
        assert!(code.contains("SPI1_CLOCK_KHZ.kHz()"), "init references the constant");
    }

    #[test]
    fn i2c_config_drives_codegen() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceI2c));
        if let ModuleConfig::I2c(cfg) = &mut mcu.modules[0].config {
            cfg.clock_hz = 400_000;
        }
        let code = mcu.fresh_main_rs();
        assert!(code.contains("I2cMode::Fast"), "fast mode:\n{code}");
        assert!(code.contains("const I2C1_CLOCK_KHZ: u32 = 400;"), "i2c clock const");
        assert!(code.contains("I2C1_CLOCK_KHZ.kHz()"), "init references the constant");
    }

    /// Assigning a peripheral signal pin (as the Peripherals tab does) auto-adds
    /// the matching virtual module; clearing the last pin removes it.
    #[test]
    fn assigning_peripheral_pin_adds_and_clearing_removes_module() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.modules.is_empty());

        let scl = mcu
            .iter_all_pins()
            .find(|p| p.available_functions.contains(&PinFunction::I2cScl(1)))
            .map(|p| p.number)
            .unwrap();

        // Assign I2C1 SCL → a GI_I2C module appears, wired to that pin.
        mcu.apply_pin_function(scl, PinFunction::I2cScl(1));
        assert_eq!(mcu.modules.len(), 1, "I2C module auto-added");
        assert_eq!(mcu.modules[0].kind, ModuleKind::GenericInterfaceI2c);
        assert_eq!(mcu.modules[0].instance(), 1);
        assert_eq!(mcu.modules[0].pin_for(ModuleSignal::Scl), Some(scl));

        // Clear every I2C pin → the module disappears.
        for n in mcu
            .iter_all_pins()
            .filter(|p| {
                matches!(
                    p.selected_function,
                    PinFunction::I2cScl(_) | PinFunction::I2cSda(_)
                )
            })
            .map(|p| p.number)
            .collect::<Vec<_>>()
        {
            mcu.apply_pin_function(n, PinFunction::Unset);
        }
        assert!(mcu.modules.is_empty(), "module removed once its pins are cleared");
    }

    /// A chip with no USART pins can't host a GI_USART module.
    #[test]
    fn add_fails_without_usart_pins() {
        use crate::panels::mcu_module::mcu::model::Mcu;
        use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
        use crate::panels::mcu_module::pins::logic::pin::Pin;
        let mut mcu = Mcu::new(
            "t".into(),
            "stm32f1".into(),
            ToolchainKind::RustEmbedded,
            vec![],
            vec![],
            vec![Pin::new(1, "PA0")], // GPIO only, no USART
            vec![],
        );
        assert!(!mcu.add_module(ModuleKind::GenericInterfaceUsart));
        assert!(mcu.modules.is_empty());
    }
}
