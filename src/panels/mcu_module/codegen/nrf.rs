//! Nordic nRF52 family — `nrf52833-hal` blocking, `embassy-nrf` async.
//!
//! Only the family predicate lives here so far: the definition
//! (`assets/mcus/nrf52833_microbit_v2.ron`) loads and draws, but no backend
//! is registered yet, so it generates no `main.rs`.
//!
//! Two facts about the silicon that the backend has to honor, recorded here
//! because this is where it will be written:
//!
//! - **Every signal routes to every pin.** There is no alternate-function
//!   table: a UARTE, SPIM, TWIM or PWM output is connected by writing the pin
//!   number into the peripheral's `PSEL` register. The definition still offers
//!   SPI and I2C only on the board's labeled pads, so autowire lands where
//!   accessories expect them.
//! - **SPIM0/SPIM1 share their peripheral IDs with TWIM0/TWIM1.** One block
//!   is either an SPI master or an I2C master, never both, which is why the
//!   definition offers SPI on SPIM2 and I2C on TWIM0/TWIM1.

/// Whether `family` is one of Nordic's nRF52 parts.
///
/// A prefix, not a list: the family key is the chip (`nrf52833`), the way it
/// is on the ESP parts, and every nRF52 shares the HAL shape. An nRF52840
/// definition then needs no change here.
pub fn is_nrf(family: &str) -> bool {
    family.starts_with("nrf52")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nrf52_parts_are_nrf_and_nothing_else_is() {
        assert!(is_nrf("nrf52833"));
        assert!(is_nrf("nrf52840"));
        for other in ["nrf51", "nrf5340", "rp2040", "stm32f4", "esp32c3", ""] {
            assert!(!is_nrf(other), "{other:?} matched");
        }
    }

    /// The BBC micro:bit v2 definition must parse, build, and keep the facts
    /// the rest of the IDE reads straight off it. Not a built-in yet, so no
    /// other test loads it - and a `.ron` nobody loads is documentation that
    /// rots.
    #[test]
    fn the_microbit_v2_definition_parses_and_builds() {
        use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
        use crate::panels::mcu_module::mcu_def::McuDefinition;

        const SRC: &str = include_str!("../../../../assets/mcus/nrf52833_microbit_v2.ron");
        let def: McuDefinition =
            ron::from_str(SRC).expect("the micro:bit v2 definition must parse");

        assert!(is_nrf(&def.family));
        assert_eq!(def.board_chip.as_deref(), Some("nRF52833"));
        assert_eq!(def.toolchain, ToolchainKind::RustEmbedded);
        let p = &def.project;
        assert_eq!(p.target, "thumbv7em-none-eabihf");
        assert_eq!(p.probe_chip, "nRF52833_xxAA");
        assert_eq!(
            (p.flash_origin.as_str(), p.flash_size.as_str()),
            ("0x00000000", "512K")
        );
        assert_eq!(
            (p.ram_origin.as_str(), p.ram_size.as_str()),
            ("0x20000000", "128K")
        );
        assert!(
            p.hal_dep_async
                .as_deref()
                .is_some_and(|l| l.contains("nfc-pins-as-gpio")),
            "pads 8 and 9 do not exist in embassy-nrf without it"
        );

        assert_eq!(def.pins.bottom.len(), 25, "25 edge-connector contacts");
        let mcu = def.build_mcu();
        assert_eq!(mcu.iter_all_pins().count(), 25 + 14);

        let mut numbers: Vec<usize> = mcu.iter_all_pins().map(|p| p.number).collect();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers.len(), 39, "pin numbers must be unique");
    }

    /// Every GPIO pad names its nRF port and pin first, and no GPIO is claimed
    /// twice. A duplicated `P0.xx` would generate two owners for one pin.
    #[test]
    fn every_gpio_pad_names_a_distinct_nrf_pin() {
        use crate::panels::mcu_module::mcu_def::McuDefinition;

        const SRC: &str = include_str!("../../../../assets/mcus/nrf52833_microbit_v2.ron");
        let def: McuDefinition = ron::from_str(SRC).unwrap();
        let mcu = def.build_mcu();

        let mut seen = std::collections::BTreeSet::new();
        for pin in mcu.iter_all_pins() {
            let head = pin.name.split_whitespace().next().unwrap_or("");
            let is_gpio = head.len() == 5
                && (head.starts_with("P0.") || head.starts_with("P1."))
                && head[3..].chars().all(|c| c.is_ascii_digit());
            if !is_gpio {
                assert!(
                    pin.reserved,
                    "{}: not a GPIO, so it must be reserved",
                    pin.name
                );
                continue;
            }
            assert!(seen.insert(head.to_owned()), "{head} appears twice");
        }
        assert_eq!(
            seen.len(),
            19 + 14,
            "19 GPIO contacts plus 14 on-board nets"
        );
    }

    /// Where autowire puts each bus on this board, stated as it is today.
    ///
    /// SPI lands on pads 13/14/15, the only pads that offer it. I2C and UART
    /// are different: the internal sensor bus and the USB-serial nets offer
    /// them too, and autowire picks those FIRST. For UART that is the default
    /// most users want (serial to the PC). For I2C it is right for the on-board
    /// motion sensor and wrong for an external device, which then needs the
    /// second pick - pads 19/20 - once the internal pads are taken.
    ///
    /// Open until on-board device modules exist to claim the internal nets
    /// explicitly; at that point a plain I2C module should prefer the edge.
    #[test]
    fn where_autowire_puts_each_bus_on_the_board() {
        use crate::panels::mcu_module::mcu_def::McuDefinition;
        use crate::panels::mcu_module::modules::ModuleSignal as S;
        use crate::panels::mcu_module::modules::autowire::pick_pins;
        use std::collections::HashSet;

        const SRC: &str = include_str!("../../../../assets/mcus/nrf52833_microbit_v2.ron");
        let def: McuDefinition = ron::from_str(SRC).unwrap();
        let mcu = def.build_mcu();
        let name_of = |n: usize| mcu.find_pin(n).map(|p| p.name.clone()).unwrap();
        let none = HashSet::new();

        let (inst, spi) = pick_pins(
            &mcu,
            &none,
            &HashSet::new(),
            &[S::Sck, S::Mosi, S::Miso],
            &[],
        )
        .expect("SPI must wire");
        assert_eq!(inst, 2, "SPIM2, the one that shares no ID with a TWIM");
        let spi: Vec<String> = spi.into_iter().map(|(_, n)| name_of(n)).collect();
        for want in ["pad 13", "pad 14", "pad 15"] {
            assert!(
                spi.iter().any(|n| n.contains(want)),
                "SPI missed {want}: {spi:?}"
            );
        }

        // First picks: the on-board nets.
        let (_, first) =
            pick_pins(&mcu, &none, &HashSet::new(), &[S::Scl, S::Sda], &[]).expect("I2C must wire");
        let first: Vec<String> = first.into_iter().map(|(_, n)| name_of(n)).collect();
        assert!(
            first.iter().all(|n| n.contains("INT_")),
            "I2C first pick: {first:?}"
        );
        let (_, uart) =
            pick_pins(&mcu, &none, &HashSet::new(), &[S::Tx, S::Rx], &[]).expect("UART must wire");
        let uart: Vec<String> = uart.into_iter().map(|(_, n)| name_of(n)).collect();
        for want in ["P0.06 (UART_TX)", "P1.08 (UART_RX)"] {
            assert!(uart.iter().any(|n| n == want), "UART first pick: {uart:?}");
        }

        // With the sensor bus taken, I2C moves to the labeled edge pads.
        let used: HashSet<usize> = mcu
            .iter_all_pins()
            .filter(|p| p.name.contains("INT_"))
            .map(|p| p.number)
            .collect();
        let (_, i2c) =
            pick_pins(&mcu, &used, &HashSet::new(), &[S::Scl, S::Sda], &[]).expect("I2C must wire");
        let i2c: Vec<String> = i2c.into_iter().map(|(_, n)| name_of(n)).collect();
        for want in ["pad 19", "pad 20"] {
            assert!(
                i2c.iter().any(|n| n.contains(want)),
                "I2C missed {want}: {i2c:?}"
            );
        }
    }

    /// HFCLK is 64 MHz from either source, and LFCLK is 32.768 kHz from either
    /// of the two the board can use. There is no LFXO node: the crystal is not
    /// fitted, and offering it would generate a `start_lfclk()` that never
    /// returns.
    #[test]
    fn the_clock_graph_delivers_64_mhz_and_32768_hz_from_every_source() {
        use crate::panels::mcu_module::clock::graph::eval::evaluate;
        use crate::panels::mcu_module::clock::graph::model::NodeState;
        use crate::panels::mcu_module::mcu_def::{ClockDef, McuDefinition};

        const SRC: &str = include_str!("../../../../assets/mcus/nrf52833_microbit_v2.ron");
        let def: McuDefinition = ron::from_str(SRC).unwrap();
        let ClockDef::Graph(gc) = def.effective_clock() else {
            panic!("the definition carries its own graph");
        };
        assert!(
            gc.graph.node("lfxo").is_none(),
            "no 32.768 kHz crystal on the board"
        );

        for hf in 0..2 {
            for lf in 0..2 {
                let mut g = gc.graph.clone();
                g.node_mut("hfclk_src").unwrap().state = NodeState::Index(hf);
                g.node_mut("lfclk_src").unwrap().state = NodeState::Index(lf);
                let hz = evaluate(&g);
                assert_eq!(hz["hfclk"], 64_000_000, "hfclk_src={hf}");
                assert_eq!(hz["lfclk"], 32_768, "hfclk_src={hf} lfclk_src={lf}");
            }
        }
    }
}
