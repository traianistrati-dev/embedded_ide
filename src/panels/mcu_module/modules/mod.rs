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
    Connection, ModuleConfig, ModuleKind, ModuleSignal, Parity, StopBits, UsartModuleConfig,
    VirtualModule,
};

/// USART module configs keyed by peripheral instance — consumed by codegen to
/// drive the generated USART init (baud rate, parity, stop bits).
pub fn usart_configs(modules: &[VirtualModule]) -> std::collections::BTreeMap<u8, UsartModuleConfig> {
    let mut map = std::collections::BTreeMap::new();
    for m in modules {
        let ModuleConfig::Usart(c) = &m.config;
        map.insert(c.instance, c.clone());
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
        let n = m.usart_instance();
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

        let n0 = mcu.modules[0].usart_instance();
        let n1 = mcu.modules[1].usart_instance();
        assert_ne!(n0, n1, "second module must pick a free USART instance");

        // No MCU pin is shared between the two modules.
        let pins0: Vec<usize> = mcu.modules[0].connections.iter().map(|c| c.mcu_pin).collect();
        let pins1: Vec<usize> = mcu.modules[1].connections.iter().map(|c| c.mcu_pin).collect();
        assert!(pins0.iter().all(|p| !pins1.contains(p)));
    }

    #[test]
    fn remove_module_drops_it() {
        let mut mcu = create_stm32f103c8tx();
        mcu.add_module(ModuleKind::GenericInterfaceUsart);
        let id = mcu.modules[0].id.clone();
        mcu.remove_module(&id);
        assert!(mcu.modules.is_empty());
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
        assert!(code.contains(".baudrate(9600.bps())"), "baud rate in init:\n{code}");
        assert!(code.contains(".parity_even()"), "parity in init");
        assert!(code.contains("serial::StopBits::STOP2"), "stop bits in init");
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
        assert!(code.contains(&format!("mod {id} {{")), "inline mod:\n{code}");
        assert!(code.contains("pub struct Reading"), "rx model body present");

        let again = mcu.update_main_rs(&code);
        assert_eq!(
            again.matches(&format!("mod {id} {{")).count(),
            1,
            "data-model mod must not be duplicated on regen"
        );
    }

    /// Modules survive a full codegen → `@modules` marker → parse round-trip,
    /// so a saved project restores them exactly.
    #[test]
    fn modules_round_trip_through_generated_main_rs() {
        let mut mcu = create_stm32f103c8tx();
        assert!(mcu.add_module(ModuleKind::GenericInterfaceUsart));
        if let ModuleConfig::Usart(cfg) = &mut mcu.modules[0].config {
            cfg.baud_rate = 9600;
            cfg.rx_model = "pub struct R { pub t: f32 }".into();
        }
        let code = mcu.fresh_main_rs();
        assert_eq!(persist::parse_from_source(&code), mcu.modules);
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
        assert!(mcu.modules[0].pin_for(ModuleSignal::Rx).is_some(), "RX still wired");
        assert!(!mcu.modules.is_empty(), "module stays (disconnected)");
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
