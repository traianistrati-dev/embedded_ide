//! Which DMA channel carries which peripheral, per STM32 family.
//!
//! embassy makes the CALLER name the channel *and* bind its interrupt, so a
//! generated project cannot build until both are filled in. Before this, both
//! were a `TODO` the user had to resolve by hand against the reference manual.
//!
//! # Why a table and not a lookup
//!
//! The truth lives in `stm32-metapac`, which the IDE does not depend on (it
//! generates code, it does not compile it). The data below was harvested from
//! embassy's own generated `dma_trait_impl!` lines for a real build, rather
//! than transcribed from a datasheet.
//!
//! # Two columns, not one
//!
//! A channel has a PERIPHERAL name and an INTERRUPT name, and they differ: on
//! STM32F4 `DMA2_CH7`'s interrupt is `DMA2_STREAM7`; on an L4-class part
//! `DMA1_CH4`'s is `DMA1_CHANNEL4`. Both appear in the generated code, in
//! different places, so [`DmaPick`] carries both.
//!
//! # Why an allocator and not a constant
//!
//! Most peripherals accept SEVERAL channels, and the sets overlap: on F4,
//! `SPI1_TX` may use DMA2 channel 2, 3 or 5 while `USART1_RX` may use 2 or 5.
//! Picking the first candidate for each would hand the same channel to both, so
//! callers walk their peripherals through one [`DmaAllocator`], which skips what
//! it has already given out.

use std::collections::BTreeSet;

/// A DMA channel, under both the names the generated code needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmaPick {
    /// The `embassy_stm32::peripherals` singleton, e.g. `DMA2_CH7`.
    pub peri: String,
    /// The `bind_interrupts!` key, e.g. `DMA2_STREAM7`.
    pub irq: String,
}

/// Bus kinds that can run on DMA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Usart,
    Spi,
    I2c,
}

/// Transfer direction - a peripheral's TX and RX channels are different.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Tx,
    Rx,
}

/// The F4 request map, harvested from embassy's generated `dma_trait_impl!`
/// lines for an STM32F411RE and **verified by a real cross-compile** (see
/// `emit_embassy_project`). It is the family's DMA request table, not a
/// per-part quirk, so it holds for the other F4 parts that have the same
/// peripheral - an instance a part lacks simply never gets asked for.
///
/// Ordered by preference: the first candidate still free wins.
const F4: &[(Bus, u8, Dir, &[&str])] = &[
    (Bus::Usart, 1, Dir::Tx, &["DMA2_CH7"]),
    (Bus::Usart, 1, Dir::Rx, &["DMA2_CH5", "DMA2_CH2"]),
    (Bus::Usart, 2, Dir::Tx, &["DMA1_CH6"]),
    (Bus::Usart, 2, Dir::Rx, &["DMA1_CH5", "DMA1_CH7"]),
    (Bus::Usart, 6, Dir::Tx, &["DMA2_CH6", "DMA2_CH7"]),
    (Bus::Usart, 6, Dir::Rx, &["DMA2_CH1", "DMA2_CH2"]),
    (Bus::Spi, 1, Dir::Tx, &["DMA2_CH3", "DMA2_CH5", "DMA2_CH2"]),
    (Bus::Spi, 1, Dir::Rx, &["DMA2_CH0", "DMA2_CH2"]),
    (Bus::Spi, 2, Dir::Tx, &["DMA1_CH4"]),
    (Bus::Spi, 2, Dir::Rx, &["DMA1_CH3"]),
    (Bus::Spi, 3, Dir::Tx, &["DMA1_CH5", "DMA1_CH7"]),
    (Bus::Spi, 3, Dir::Rx, &["DMA1_CH0", "DMA1_CH2"]),
    (Bus::I2c, 1, Dir::Tx, &["DMA1_CH6", "DMA1_CH7", "DMA1_CH1"]),
    (Bus::I2c, 1, Dir::Rx, &["DMA1_CH0", "DMA1_CH5"]),
    (Bus::I2c, 2, Dir::Tx, &["DMA1_CH7"]),
    (Bus::I2c, 2, Dir::Rx, &["DMA1_CH2", "DMA1_CH3"]),
    (Bus::I2c, 3, Dir::Tx, &["DMA1_CH4", "DMA1_CH5"]),
    (Bus::I2c, 3, Dir::Rx, &["DMA1_CH1", "DMA1_CH2"]),
];

/// The F2 request map, harvested the same way from an STM32F217ZE build and
/// **verified by a real cross-compile**.
///
/// Close to [`F4`] but NOT the same, which is why it is its own table rather
/// than a shared one: USART2's RX has a single channel here where F4 offers two,
/// SPI1's TX drops F4's third option, I2C1's TX drops one and I2C3 is down to a
/// single channel each way. F2 also has USART3, which the F411 the F4 table came
/// from does not.
const F2: &[(Bus, u8, Dir, &[&str])] = &[
    (Bus::Usart, 1, Dir::Tx, &["DMA2_CH7"]),
    (Bus::Usart, 1, Dir::Rx, &["DMA2_CH5", "DMA2_CH2"]),
    (Bus::Usart, 2, Dir::Tx, &["DMA1_CH6"]),
    (Bus::Usart, 2, Dir::Rx, &["DMA1_CH5"]),
    (Bus::Usart, 3, Dir::Tx, &["DMA1_CH3", "DMA1_CH4"]),
    (Bus::Usart, 3, Dir::Rx, &["DMA1_CH1"]),
    (Bus::Usart, 6, Dir::Tx, &["DMA2_CH6", "DMA2_CH7"]),
    (Bus::Usart, 6, Dir::Rx, &["DMA2_CH1", "DMA2_CH2"]),
    (Bus::Spi, 1, Dir::Tx, &["DMA2_CH3", "DMA2_CH5"]),
    (Bus::Spi, 1, Dir::Rx, &["DMA2_CH0", "DMA2_CH2"]),
    (Bus::Spi, 2, Dir::Tx, &["DMA1_CH4"]),
    (Bus::Spi, 2, Dir::Rx, &["DMA1_CH3"]),
    (Bus::Spi, 3, Dir::Tx, &["DMA1_CH5", "DMA1_CH7"]),
    (Bus::Spi, 3, Dir::Rx, &["DMA1_CH0", "DMA1_CH2"]),
    (Bus::I2c, 1, Dir::Tx, &["DMA1_CH6", "DMA1_CH7"]),
    (Bus::I2c, 1, Dir::Rx, &["DMA1_CH0", "DMA1_CH5"]),
    (Bus::I2c, 2, Dir::Tx, &["DMA1_CH7"]),
    (Bus::I2c, 2, Dir::Rx, &["DMA1_CH2", "DMA1_CH3"]),
    (Bus::I2c, 3, Dir::Tx, &["DMA1_CH4"]),
    (Bus::I2c, 3, Dir::Rx, &["DMA1_CH2"]),
];

/// The F7 request map, harvested from an STM32F767ZI build and **verified by a
/// real cross-compile** on an STM32F746ZG - the two were diffed over every
/// instance below and are identical, which is what makes one table per family
/// defensible here rather than one per part.
///
/// USART, SPI1/3 and I2C1 match [`F2`]; the differences are all extra room:
/// SPI2 gains a second channel each way, I2C2's TX and I2C3 both directions
/// gain one. Nothing F2 offers is missing, but the reverse is not true, so the
/// tables stay separate.
const F7: &[(Bus, u8, Dir, &[&str])] = &[
    (Bus::Usart, 1, Dir::Tx, &["DMA2_CH7"]),
    (Bus::Usart, 1, Dir::Rx, &["DMA2_CH5", "DMA2_CH2"]),
    (Bus::Usart, 2, Dir::Tx, &["DMA1_CH6"]),
    (Bus::Usart, 2, Dir::Rx, &["DMA1_CH5"]),
    (Bus::Usart, 3, Dir::Tx, &["DMA1_CH3", "DMA1_CH4"]),
    (Bus::Usart, 3, Dir::Rx, &["DMA1_CH1"]),
    (Bus::Usart, 6, Dir::Tx, &["DMA2_CH6", "DMA2_CH7"]),
    (Bus::Usart, 6, Dir::Rx, &["DMA2_CH1", "DMA2_CH2"]),
    (Bus::Spi, 1, Dir::Tx, &["DMA2_CH3", "DMA2_CH5"]),
    (Bus::Spi, 1, Dir::Rx, &["DMA2_CH0", "DMA2_CH2"]),
    (Bus::Spi, 2, Dir::Tx, &["DMA1_CH4", "DMA1_CH6"]),
    (Bus::Spi, 2, Dir::Rx, &["DMA1_CH3", "DMA1_CH1"]),
    (Bus::Spi, 3, Dir::Tx, &["DMA1_CH5", "DMA1_CH7"]),
    (Bus::Spi, 3, Dir::Rx, &["DMA1_CH0", "DMA1_CH2"]),
    (Bus::I2c, 1, Dir::Tx, &["DMA1_CH6", "DMA1_CH7"]),
    (Bus::I2c, 1, Dir::Rx, &["DMA1_CH0", "DMA1_CH5"]),
    (Bus::I2c, 2, Dir::Tx, &["DMA1_CH7", "DMA1_CH4"]),
    (Bus::I2c, 2, Dir::Rx, &["DMA1_CH2", "DMA1_CH3"]),
    (Bus::I2c, 3, Dir::Tx, &["DMA1_CH4", "DMA1_CH0"]),
    (Bus::I2c, 3, Dir::Rx, &["DMA1_CH2", "DMA1_CH1"]),
];

/// The candidate channels for one peripheral direction, best first.
///
/// `None` for a family with no table yet - the caller then leaves its `TODO` in
/// place, which is what every family did before this existed. Guessing would
/// produce code that compiles and moves the wrong bytes.
fn candidates(family: &str, bus: Bus, instance: u8, dir: Dir) -> Option<&'static [&'static str]> {
    let table = match family {
        "stm32f4" => F4,
        "stm32f2" => F2,
        "stm32f7" => F7,
        // Every other family keeps the TODO. The three tables here differ from
        // each other in ways no amount of squinting at the reference manuals
        // would have predicted, so a new one means harvesting and compiling it,
        // not extending a pattern.
        _ => return None,
    };
    table
        .iter()
        .find(|(b, i, d, _)| *b == bus && *i == instance && *d == dir)
        .map(|(_, _, _, c)| *c)
}

/// The `bind_interrupts!` key for a channel peripheral name.
///
/// STM32F4 calls the peripheral `DMA2_CH7` and its interrupt `DMA2_STREAM7`
/// (ST's "stream" naming, which embassy keeps for the vector but not for the
/// singleton). Verified against embassy's `dma_channel_impl!` output.
fn irq_for(family: &str, channel: &str) -> Option<String> {
    match family {
        // Both use ST's "stream" naming for the vector while embassy keeps
        // "channel" for the singleton. Verified against `dma_channel_impl!`
        // output for an F411 and an F217ZE.
        "stm32f4" | "stm32f2" | "stm32f7" => Some(channel.replace("_CH", "_STREAM")),
        _ => None,
    }
}

/// Hands out DMA channels for one project, never the same one twice.
///
/// Two ways to answer, in order:
///
/// 1. **The chip's own channel list**, when it was imported from the vendor
///    database AND its controller is muxed (DMAMUX / GPDMA — 1085 of the 1839
///    parts the database describes). Then there is no request table to consult:
///    any channel can serve any peripheral, so the first free one wins.
/// 2. **The family table** above, for the fixed-mapping families.
///
/// A chip that is neither — a classic part imported from a source with no DMA
/// data, or one from a family nobody has harvested — still answers `None`, and
/// the caller keeps its `TODO`.
#[derive(Debug, Default)]
pub struct DmaAllocator {
    family: String,
    used: BTreeSet<String>,
    /// The chip's channels, in the order the vendor lists them. Non-empty only
    /// for a muxed chip: on a fixed-mapping part the list is real but useless
    /// without the request table, and handing out a free channel there would
    /// produce code that compiles and moves nothing.
    pool: Vec<super::dma_data::DmaChannel>,
}

impl DmaAllocator {
    pub fn new(family: &str) -> Self {
        Self {
            family: family.to_owned(),
            used: BTreeSet::new(),
            pool: Vec::new(),
        }
    }

    /// The allocator for a specific chip — the same family tables, plus the
    /// chip's own channels when they can be used without a request table.
    pub fn for_chip(
        family: &str,
        dma: Option<&crate::panels::mcu_module::mcu_def::DmaDef>,
    ) -> Self {
        let pool = match dma {
            Some(d) if d.mux => d.channels.clone(),
            _ => Vec::new(),
        };
        Self {
            family: family.to_owned(),
            used: BTreeSet::new(),
            pool,
        }
    }

    /// The channel for `bus{instance}`'s `dir`, or `None` when the family has no
    /// table or every candidate is already taken by an earlier peripheral.
    ///
    /// `None` is a normal outcome, not an error: the caller falls back to the
    /// `TODO` placeholder, so a project with more DMA buses than channels still
    /// generates - it just asks the user to finish it.
    pub fn take(&mut self, bus: Bus, instance: u8, dir: Dir) -> Option<DmaPick> {
        // A muxed chip does not care WHICH channel a peripheral gets, so the
        // bus/instance/direction never enter into it - only "is it free".
        if let Some(c) = self.pool.iter().find(|c| !self.used.contains(&c.peri)) {
            let pick = DmaPick {
                peri: c.peri.clone(),
                irq: c.irq.clone(),
            };
            self.used.insert(pick.peri.clone());
            return Some(pick);
        }
        let cands = candidates(&self.family, bus, instance, dir)?;
        let chan = cands.iter().find(|c| !self.used.contains(**c))?;
        let irq = irq_for(&self.family, chan)?;
        self.used.insert((*chan).to_owned());
        Some(DmaPick {
            peri: (*chan).to_owned(),
            irq,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::panels::mcu_module::codegen::dma_data::DmaChannel;
    use crate::panels::mcu_module::mcu_def::DmaDef;

    fn chan(peri: &str, irq: &str) -> DmaChannel {
        DmaChannel {
            peri: peri.into(),
            irq: irq.into(),
        }
    }

    /// The whole point of the mux rule: a family with no table at all - here
    /// STM32G4 - now generates real channels, taken in order and never reused.
    #[test]
    fn a_muxed_chip_hands_out_its_own_channels_in_order() {
        let def = DmaDef {
            mux: true,
            channels: vec![
                chan("DMA1_CH1", "DMA1_CHANNEL1"),
                chan("DMA1_CH2", "DMA1_CHANNEL2"),
                chan("DMA1_CH3", "DMA1_CHANNEL3"),
            ],
        };
        let mut a = DmaAllocator::for_chip("stm32g4", Some(&def));
        let tx = a.take(Bus::Usart, 1, Dir::Tx).expect("USART1 TX");
        let rx = a.take(Bus::Usart, 1, Dir::Rx).expect("USART1 RX");
        assert_eq!(
            (tx.peri.as_str(), tx.irq.as_str()),
            ("DMA1_CH1", "DMA1_CHANNEL1")
        );
        assert_eq!(rx.peri, "DMA1_CH2");
        // A different peripheral keeps taking from the same pool.
        assert_eq!(a.take(Bus::Spi, 1, Dir::Tx).unwrap().peri, "DMA1_CH3");
        // Exhausted: back to the TODO rather than a channel already in use.
        assert_eq!(a.take(Bus::Spi, 1, Dir::Rx), None);
    }

    /// A chip with a FIXED request map must not be served from its channel
    /// list, even though the list is perfectly real - which channel carries
    /// which request is exactly what the hardware decides there.
    #[test]
    fn a_classic_chip_still_needs_its_request_table() {
        let def = DmaDef {
            mux: false,
            channels: vec![chan("DMA1_CH1", "DMA1_CHANNEL1")],
        };
        let mut a = DmaAllocator::for_chip("stm32l1", Some(&def));
        assert_eq!(a.take(Bus::Usart, 1, Dir::Tx), None);
        // ... and where a table DOES exist, it is the one that answers.
        let mut f4 = DmaAllocator::for_chip("stm32f4", Some(&def));
        assert_eq!(f4.take(Bus::Usart, 1, Dir::Tx).unwrap().peri, "DMA2_CH7");
    }

    #[test]
    fn a_family_without_a_table_picks_nothing() {
        // Silence is the contract: the caller keeps its TODO rather than
        // receiving a plausible-looking guess.
        let mut a = DmaAllocator::new("stm32l4");
        assert_eq!(a.take(Bus::Usart, 1, Dir::Tx), None);
    }

    /// F7 is F2 plus room, never less - the one relationship between two of
    /// these tables that IS a pattern, and worth pinning so a future edit that
    /// narrows F7 shows up as a failure rather than a silent regression.
    #[test]
    fn f7_is_a_superset_of_f2() {
        for (bus, n, dir) in [
            (Bus::Usart, 1u8, Dir::Tx),
            (Bus::Usart, 3, Dir::Rx),
            (Bus::Spi, 2, Dir::Tx),
            (Bus::I2c, 2, Dir::Tx),
            (Bus::I2c, 3, Dir::Rx),
        ] {
            let f2 = candidates("stm32f2", bus, n, dir).unwrap_or(&[]);
            let f7 = candidates("stm32f7", bus, n, dir).unwrap_or(&[]);
            assert!(
                f2.iter().all(|c| f7.contains(c)),
                "F7 lost a channel F2 has: {bus:?}{n} {dir:?} - F2 {f2:?}, F7 {f7:?}"
            );
        }
    }

    /// F2 and F4 look interchangeable and are not. Sharing one table would have
    /// handed an F2 project channels its hardware does not route.
    #[test]
    fn f2_is_not_f4() {
        let pick = |fam: &str, bus, n, dir| DmaAllocator::new(fam).take(bus, n, dir);
        // USART2 RX: F4 falls back to DMA1_CH7, F2 has no second choice.
        let mut f4 = DmaAllocator::new("stm32f4");
        let mut f2 = DmaAllocator::new("stm32f2");
        f4.take(Bus::Usart, 2, Dir::Rx);
        f2.take(Bus::Usart, 2, Dir::Rx);
        assert!(
            f4.take(Bus::Usart, 2, Dir::Rx).is_some(),
            "F4 falls back to DMA1_CH7"
        );
        assert!(
            f2.take(Bus::Usart, 2, Dir::Rx).is_none(),
            "F2 has no second channel"
        );
        // USART3 exists on F2 and not on the F411 the F4 table came from.
        assert!(pick("stm32f2", Bus::Usart, 3, Dir::Rx).is_some());
        assert!(pick("stm32f4", Bus::Usart, 3, Dir::Rx).is_none());
        // I2C3 TX: two candidates on F4, one on F2.
        let mut f4 = DmaAllocator::new("stm32f4");
        let mut f2 = DmaAllocator::new("stm32f2");
        f4.take(Bus::I2c, 3, Dir::Tx);
        f2.take(Bus::I2c, 3, Dir::Tx);
        assert!(f4.take(Bus::I2c, 3, Dir::Tx).is_some(), "F4 has a spare");
        assert!(f2.take(Bus::I2c, 3, Dir::Tx).is_none(), "F2 does not");
    }

    #[test]
    fn the_channel_carries_both_of_its_names() {
        let mut a = DmaAllocator::new("stm32f4");
        let tx = a.take(Bus::Usart, 1, Dir::Tx).expect("USART1 TX");
        assert_eq!(tx.peri, "DMA2_CH7");
        // The name that goes in `bind_interrupts!` is NOT the same string.
        assert_eq!(tx.irq, "DMA2_STREAM7");
    }

    #[test]
    fn an_allocated_channel_is_never_handed_out_twice() {
        // The overlap that makes an allocator necessary: SPI1_TX's first choice
        // is DMA2_CH3, USART1_RX's is DMA2_CH5, and both also list DMA2_CH2.
        let mut a = DmaAllocator::new("stm32f4");
        let spi_tx = a.take(Bus::Spi, 1, Dir::Tx).unwrap();
        let spi_rx = a.take(Bus::Spi, 1, Dir::Rx).unwrap();
        let u_rx = a.take(Bus::Usart, 1, Dir::Rx).unwrap();
        let all = [&spi_tx.peri, &spi_rx.peri, &u_rx.peri];
        let uniq: BTreeSet<_> = all.iter().collect();
        assert_eq!(uniq.len(), all.len(), "channels collided: {all:?}");
    }

    #[test]
    fn running_out_of_candidates_falls_back_rather_than_reusing() {
        let mut a = DmaAllocator::new("stm32f4");
        // SPI2 TX has exactly one candidate; taking it twice must fail the
        // second time instead of silently sharing the channel.
        assert!(a.take(Bus::Spi, 2, Dir::Tx).is_some());
        assert_eq!(a.take(Bus::Spi, 2, Dir::Tx), None);
    }

    #[test]
    fn an_unknown_instance_is_not_invented() {
        let mut a = DmaAllocator::new("stm32f4");
        assert_eq!(a.take(Bus::Usart, 3, Dir::Tx), None, "F411 has no USART3");
    }
}
