//! Virtual electronic modules attached to the MCU on the Pins canvas.
//!
//! A module mimics a physical peripheral device (USART / SPI / I2C) and
//! auto-connects to compatible MCU pins, drawn as a simplified schematic next to
//! the chip. This is the data model; auto-wiring lives in [`super::autowire`].

use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
use crate::panels::mcu_module::pins::logic::pin_function::display::sdmmc_name;
use serde::{Deserialize, Serialize};

/// Kind of virtual module. New kinds are added here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ModuleKind {
    /// Generic device speaking over USART (TX/RX) — "USART".
    GenericInterfaceUsart,
    /// Generic device speaking over LPUART (TX/RX) — "LPUART".
    ///
    /// A peripheral of its own, NOT a USART instance: an STM32G0 has both
    /// USART1 and LPUART1, so the two must be able to coexist on instance 1.
    GenericInterfaceLpuart,
    /// Generic device on an SPI bus (SCK/MOSI/MISO/NSS) — "SPI".
    GenericInterfaceSpi,
    /// Generic device on an I2C bus (SCL/SDA) — "I2C".
    GenericInterfaceI2c,
    /// External memory on the HSPI controller — "HSPI".
    ///
    /// The high-speed controller at the top of the U5 line. Sixteen data pads
    /// in silicon, but embassy builds exactly two widths: single and octal.
    GenericInterfaceHspi,
    /// External flash or RAM on an XSPI port — "XSPI".
    ///
    /// OCTOSPI's successor, on the H7RS and N6. Same shape as the OCTOSPI
    /// module — the module is the PORT — with twice the data lines and a
    /// second chip select and strobe.
    GenericInterfaceXspi,
    /// External flash or RAM on an OCTOSPI port — "OSPI".
    ///
    /// The module is the PORT, because that is what the vendor names the pads
    /// after; the IO manager maps it to a controller, and the IDE takes the
    /// default 1:1 (port 1 to OCTOSPI1).
    GenericInterfaceOspi,
    /// External flash on the QUADSPI controller — "QSPI".
    ///
    /// One peripheral, up to two banks. Which banks are wired is which
    /// constructor embassy gets, so the width of the module is the wiring.
    GenericInterfaceQspi,
    /// SD card / eMMC on ONE SDMMC (or SDIO) controller — "SDMMC".
    ///
    /// The bus width is not a setting: wiring D0 alone, D0–D3 or D0–D7 IS the
    /// width, and each one is a different embassy constructor.
    GenericInterfaceSdmmc,
    /// Audio on ONE SAI unit — "SAI".
    ///
    /// The module is the UNIT, and its two sub-blocks A and B are its channels:
    /// same shape as the PWM module's timer and the DAC module's block. They are
    /// independent — a codec is usually transmit on one and receive on the other
    /// — but they are split off one peripheral, once, which is why they cannot
    /// be two modules.
    GenericInterfaceSai,
    /// Analog outputs of ONE DAC — "DAC".
    ///
    /// The module is the PERIPHERAL, not the channel, for the same reason the
    /// PWM module is the timer: DAC1 OUT1 and OUT2 are two pads of one block,
    /// and embassy builds them together or one alone, never twice.
    GenericInterfaceDac,
    /// Audio device on an I2S bus (CK/WS/SD, optional MCK) — "I2S".
    ///
    /// The instance is the SPI block the I2S runs on: I2S2 IS SPI2, so a
    /// `GI_SPI 2` and a `GI_I2S 2` describe the same silicon and only one of
    /// them can be built.
    GenericInterfaceI2s,
    /// A pulse train on one RMT channel — "RMT".
    ///
    /// Espressif only, and unlike every other kind here the instance is a
    /// CHANNEL, not a peripheral: a chip has one RMT block whose four (or
    /// eight) channels are independent, each with its own pin, divider and
    /// carrier. `Rmt::new` is called once in `main.rs` and lends each channel
    /// out, exactly as `Ledc::new` does for PWM.
    GenericInterfaceRmt,
    /// PWM outputs driven by ONE timer — "PWM".
    ///
    /// The module is the TIMER, not the channel: every channel of a timer shares
    /// its prescaler and reload value, so they physically share a frequency. One
    /// module per timer is what lets the UI state that once instead of inviting
    /// four contradictory answers.
    GenericInterfaceTimer,
    /// Generic device on a CAN bus (RX/TX) — "CAN".
    GenericInterfaceCan,
    /// USB full-speed device (D-/D+) — "USB".
    GenericInterfaceUsb,
    /// A user-authored module with an arbitrary list of pins — "Custom".
    ///
    /// Unlike every kind above, this one is NOT derived from the pin functions:
    /// the user creates it and picks its pins, so `reconcile_modules` must leave
    /// it alone (a peripheral module disappears when its pins go; a custom one
    /// only when the user removes it).
    Custom,
}

impl ModuleKind {
    /// Every kind, in palette order.
    pub const ALL: [ModuleKind; 17] = [
        ModuleKind::GenericInterfaceUsart,
        ModuleKind::GenericInterfaceLpuart,
        ModuleKind::GenericInterfaceSpi,
        ModuleKind::GenericInterfaceI2c,
        ModuleKind::GenericInterfaceI2s,
        ModuleKind::GenericInterfaceRmt,
        ModuleKind::GenericInterfaceSai,
        ModuleKind::GenericInterfaceSdmmc,
        ModuleKind::GenericInterfaceQspi,
        ModuleKind::GenericInterfaceOspi,
        ModuleKind::GenericInterfaceXspi,
        ModuleKind::GenericInterfaceHspi,
        ModuleKind::GenericInterfaceDac,
        ModuleKind::GenericInterfaceTimer,
        ModuleKind::GenericInterfaceCan,
        ModuleKind::GenericInterfaceUsb,
        ModuleKind::Custom,
    ];

    /// User-authored (not derived from the pin functions) — see
    /// [`ModuleKind::Custom`].
    pub fn is_custom(self) -> bool {
        matches!(self, ModuleKind::Custom)
    }

    /// The `(required, optional)` signals this kind needs to auto-wire.
    /// Single source of truth: `Mcu::add_module` wires from it, and the palette
    /// asks the SAME table whether a chip can host the kind — so what the UI
    /// offers can never drift from what actually succeeds.
    pub fn signals(self) -> (&'static [ModuleSignal], &'static [ModuleSignal]) {
        use ModuleSignal::*;
        match self {
            ModuleKind::GenericInterfaceUsart => (&[Tx, Rx], &[]),
            // Deliberately TX/RX only, like the USART: CTS/RTS as "optional"
            // would make the module eat four pins whenever flow-control pads
            // happen to be free, which is not what adding a serial device means.
            ModuleKind::GenericInterfaceLpuart => (&[LpTx, LpRx], &[]),
            // MISO is OPTIONAL, not absent: autowire still takes it when a
            // pad is free, but a bus without one is a transmitter rather than
            // a broken module — embassy's `new_txonly` is exactly that shape.
            ModuleKind::GenericInterfaceSpi => (&[Sck, Mosi], &[Miso, Nss]),
            ModuleKind::GenericInterfaceI2c => (&[Scl, Sda], &[]),
            // MCK is optional: only a codec that wants a master clock needs
            // the pad, and spending it by default would take a pin nobody
            // asked for.
            ModuleKind::GenericInterfaceI2s => (&[I2sCk, I2sWs, I2sSd], &[I2sMck]),
            // One wire, and it is required: an RMT channel with no pin
            // has nothing to clock a train onto.
            ModuleKind::GenericInterfaceRmt => (&[RmtLine], &[]),
            // One channel, like the PWM module: taking the second pad by
            // default would spend a pin on a DAC the user added for one.
            ModuleKind::GenericInterfaceDac => (&[DacOut1], &[]),
            // Sub-block A only: B is a second, independent audio stream and
            // taking its three pads by default would spend them on nothing.
            ModuleKind::GenericInterfaceSai => (&[SaiSckA, SaiSdA, SaiFsA], &[SaiMclkA]),
            // One data line: the 4- and 8-bit widths join by assigning the
            // other lanes, the same way a timer's channels do.
            ModuleKind::GenericInterfaceSdmmc => (&[SdCk, SdCmd, SdD0], &[]),
            // A whole bank: a quad flash needs all four data lines, so
            // taking fewer would auto-wire something that cannot work.
            ModuleKind::GenericInterfaceQspi => {
                (&[QsClk, QsB1Ncs, QsB1Io0, QsB1Io1, QsB1Io2, QsB1Io3], &[])
            }
            // The narrowest device embassy builds: two data lines. The
            // wider modes join by assigning IO2..IO7, so a module added
            // from the palette does not swallow eight pads.
            ModuleKind::GenericInterfaceOspi => (&[OsClk, OsNcs, OsIo0, OsIo1], &[]),
            // The same two-line minimum as the OCTOSPI, on NCS1.
            ModuleKind::GenericInterfaceXspi => (&[XsClk, XsNcs1, XsIo0, XsIo1], &[]),
            // The single-line width: the octal one joins by assigning
            // IO2..IO7 and the strobe, which it needs.
            ModuleKind::GenericInterfaceHspi => (&[HsClk, HsNcs, HsIo0, HsIo1], &[]),
            // ONE channel, and no optional ones: "optional" here means "take it
            // if the pad is free", which would spend all four of a timer's
            // channels on a module the user added to blink one LED. The other
            // channels join by assigning them on the canvas — `reconcile_modules`
            // folds them into this same module, since it keys on (kind, timer).
            ModuleKind::GenericInterfaceTimer => (&[PwmCh1], &[]),
            ModuleKind::GenericInterfaceCan => (&[CanRx, CanTx], &[]),
            ModuleKind::GenericInterfaceUsb => (&[UsbDm, UsbDp], &[]),
            // Nothing is auto-wired: the user picks the pins by hand.
            ModuleKind::Custom => (&[], &[]),
        }
    }

    /// `true` for peripherals that exist only once on the chip and whose pin
    /// functions carry no instance index, so the per-instance guard can't stop
    /// a second module from grabbing the alternate pins.
    pub fn is_single_instance(self) -> bool {
        matches!(
            self,
            ModuleKind::GenericInterfaceCan | ModuleKind::GenericInterfaceUsb
        )
    }

    /// Short tag used in the palette and as the default module name prefix.
    pub fn short(self) -> &'static str {
        match self {
            ModuleKind::GenericInterfaceUsart => "USART",
            ModuleKind::GenericInterfaceLpuart => "LPUART",
            ModuleKind::GenericInterfaceSpi => "SPI",
            ModuleKind::GenericInterfaceI2c => "I2C",
            ModuleKind::GenericInterfaceI2s => "I2S",
            ModuleKind::GenericInterfaceRmt => "RMT",
            ModuleKind::GenericInterfaceDac => "DAC",
            ModuleKind::GenericInterfaceSai => "SAI",
            ModuleKind::GenericInterfaceSdmmc => "SDMMC",
            ModuleKind::GenericInterfaceQspi => "QSPI",
            ModuleKind::GenericInterfaceOspi => "OSPI",
            ModuleKind::GenericInterfaceXspi => "XSPI",
            ModuleKind::GenericInterfaceHspi => "HSPI",
            ModuleKind::GenericInterfaceTimer => "PWM",
            ModuleKind::GenericInterfaceCan => "CAN",
            ModuleKind::GenericInterfaceUsb => "USB",
            ModuleKind::Custom => "Custom",
        }
    }

    /// Default config for this kind on `instance`.
    pub fn default_config(self, instance: u8) -> ModuleConfig {
        match self {
            ModuleKind::GenericInterfaceUsart => {
                ModuleConfig::Usart(UsartModuleConfig::new(instance))
            }
            // Same settings struct as the USART — baud / parity / stop bits /
            // DMA all mean exactly the same thing on an LPUART, and the variant
            // is what keeps the two peripherals apart.
            ModuleKind::GenericInterfaceLpuart => {
                ModuleConfig::Lpuart(UsartModuleConfig::new(instance))
            }
            ModuleKind::GenericInterfaceSpi => ModuleConfig::Spi(SpiModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceI2c => ModuleConfig::I2c(I2cModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceI2s => ModuleConfig::I2s(I2sModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceRmt => ModuleConfig::Rmt(RmtModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceDac => ModuleConfig::Dac(DacModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceSai => ModuleConfig::Sai(SaiModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceSdmmc => {
                ModuleConfig::Sdmmc(SdmmcModuleConfig::new(instance))
            }
            ModuleKind::GenericInterfaceQspi => ModuleConfig::Qspi(QspiModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceOspi => ModuleConfig::Ospi(OspiModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceXspi => ModuleConfig::Xspi(XspiModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceHspi => ModuleConfig::Hspi(HspiModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceTimer => {
                ModuleConfig::Timer(TimerModuleConfig::new(instance))
            }
            ModuleKind::GenericInterfaceCan => ModuleConfig::Can(CanModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceUsb => ModuleConfig::Usb(UsbModuleConfig::new(instance)),
            ModuleKind::Custom => ModuleConfig::Custom(CustomModuleConfig::new(instance)),
        }
    }
}

/// Map an assigned pin function to the module `(kind, instance, signal)` it
/// implies — the inverse of [`ModuleSignal::pin_function`]. `None` for functions
/// that don't belong to a module (GPIO, ADC, timer, USART CTS/RTS/CK, …).
pub fn module_signal_of(func: &PinFunction) -> Option<(ModuleKind, u8, ModuleSignal)> {
    use ModuleKind::*;
    use ModuleSignal::*;
    Some(match func {
        PinFunction::UsartTx(n) => (GenericInterfaceUsart, *n, Tx),
        PinFunction::UsartRx(n) => (GenericInterfaceUsart, *n, Rx),
        PinFunction::LpuartTx(n) => (GenericInterfaceLpuart, *n, LpTx),
        PinFunction::LpuartRx(n) => (GenericInterfaceLpuart, *n, LpRx),
        // Flow control joins the module that owns the instance. Before this,
        // a CTS/RTS pin assigned by hand belonged to nothing on the diagram.
        PinFunction::UsartCts(n) => (GenericInterfaceUsart, *n, Cts),
        PinFunction::UsartRts(n) => (GenericInterfaceUsart, *n, Rts),
        PinFunction::LpuartCts(n) => (GenericInterfaceLpuart, *n, LpCts),
        PinFunction::LpuartRts(n) => (GenericInterfaceLpuart, *n, LpRts),
        PinFunction::SpiSck(n) => (GenericInterfaceSpi, *n, Sck),
        PinFunction::SpiMosi(n) => (GenericInterfaceSpi, *n, Mosi),
        PinFunction::SpiMiso(n) => (GenericInterfaceSpi, *n, Miso),
        PinFunction::SpiNss(n) => (GenericInterfaceSpi, *n, Nss),
        // A PWM pin names its timer AND its channel, so the module it belongs to
        // is the timer and the wire is the channel.
        PinFunction::TimerPwm { timer, channel } => (
            GenericInterfaceTimer,
            *timer,
            match channel {
                1 => PwmCh1,
                2 => PwmCh2,
                3 => PwmCh3,
                _ => PwmCh4,
            },
        ),
        // The break pad joins the timer it protects.
        PinFunction::TimerBreak { timer, input } => (
            GenericInterfaceTimer,
            *timer,
            if *input == 1 { PwmBkin1 } else { PwmBkin2 },
        ),
        // The complementary pad joins the SAME module as its channel: one
        // timer, one module, however many pads it drives.
        PinFunction::TimerPwmN { timer, channel } => (
            GenericInterfaceTimer,
            *timer,
            match channel {
                1 => PwmCh1N,
                2 => PwmCh2N,
                3 => PwmCh3N,
                _ => PwmCh4N,
            },
        ),
        PinFunction::I2cScl(n) => (GenericInterfaceI2c, *n, Scl),
        PinFunction::I2cSda(n) => (GenericInterfaceI2c, *n, Sda),
        PinFunction::RmtChannel(n) => (GenericInterfaceRmt, *n, RmtLine),
        PinFunction::I2sCk(n) => (GenericInterfaceI2s, *n, I2sCk),
        PinFunction::I2sWs(n) => (GenericInterfaceI2s, *n, I2sWs),
        PinFunction::I2sSd(n) => (GenericInterfaceI2s, *n, I2sSd),
        PinFunction::I2sMck(n) => (GenericInterfaceI2s, *n, I2sMck),
        PinFunction::DacOut { dac, channel } => (
            GenericInterfaceDac,
            *dac,
            if *channel == 1 { DacOut1 } else { DacOut2 },
        ),
        // Both sub-blocks join the SAME module: one unit, one module, however
        // many of its pads are wired.
        PinFunction::SaiSck { sai, block } => (
            GenericInterfaceSai,
            *sai,
            if *block == 1 { SaiSckA } else { SaiSckB },
        ),
        PinFunction::SaiSd { sai, block } => (
            GenericInterfaceSai,
            *sai,
            if *block == 1 { SaiSdA } else { SaiSdB },
        ),
        PinFunction::SaiFs { sai, block } => (
            GenericInterfaceSai,
            *sai,
            if *block == 1 { SaiFsA } else { SaiFsB },
        ),
        PinFunction::SaiMclk { sai, block } => (
            GenericInterfaceSai,
            *sai,
            if *block == 1 { SaiMclkA } else { SaiMclkB },
        ),
        PinFunction::HspiClk { unit } => (GenericInterfaceHspi, *unit, HsClk),
        PinFunction::HspiNcs { unit } => (GenericInterfaceHspi, *unit, HsNcs),
        PinFunction::HspiDqs { unit, index } => (
            GenericInterfaceHspi,
            *unit,
            if *index == 0 { HsDqs0 } else { HsDqs1 },
        ),
        PinFunction::HspiIo { unit, lane } => (
            GenericInterfaceHspi,
            *unit,
            match lane {
                0 => HsIo0,
                1 => HsIo1,
                2 => HsIo2,
                3 => HsIo3,
                4 => HsIo4,
                5 => HsIo5,
                6 => HsIo6,
                7 => HsIo7,
                8 => HsIo8,
                9 => HsIo9,
                10 => HsIo10,
                11 => HsIo11,
                12 => HsIo12,
                13 => HsIo13,
                14 => HsIo14,
                _ => HsIo15,
            },
        ),
        PinFunction::XspiClk { port } => (GenericInterfaceXspi, *port, XsClk),
        PinFunction::XspiNcs { port, cs } => (
            GenericInterfaceXspi,
            *port,
            if *cs == 1 { XsNcs1 } else { XsNcs2 },
        ),
        PinFunction::XspiDqs { port, index } => (
            GenericInterfaceXspi,
            *port,
            if *index == 0 { XsDqs0 } else { XsDqs1 },
        ),
        PinFunction::XspiIo { port, lane } => (
            GenericInterfaceXspi,
            *port,
            match lane {
                0 => XsIo0,
                1 => XsIo1,
                2 => XsIo2,
                3 => XsIo3,
                4 => XsIo4,
                5 => XsIo5,
                6 => XsIo6,
                7 => XsIo7,
                8 => XsIo8,
                9 => XsIo9,
                10 => XsIo10,
                11 => XsIo11,
                12 => XsIo12,
                13 => XsIo13,
                14 => XsIo14,
                _ => XsIo15,
            },
        ),
        PinFunction::OspiClk { port } => (GenericInterfaceOspi, *port, OsClk),
        PinFunction::OspiNcs { port } => (GenericInterfaceOspi, *port, OsNcs),
        PinFunction::OspiDqs { port } => (GenericInterfaceOspi, *port, OsDqs),
        PinFunction::OspiIo { port, lane } => (
            GenericInterfaceOspi,
            *port,
            match lane {
                0 => OsIo0,
                1 => OsIo1,
                2 => OsIo2,
                3 => OsIo3,
                4 => OsIo4,
                5 => OsIo5,
                6 => OsIo6,
                _ => OsIo7,
            },
        ),
        // The QUADSPI is single-instance, so every pad joins module 1.
        PinFunction::QspiClk => (GenericInterfaceQspi, 1, QsClk),
        PinFunction::QspiNcs { bank } => (
            GenericInterfaceQspi,
            1,
            if *bank == 1 { QsB1Ncs } else { QsB2Ncs },
        ),
        PinFunction::QspiIo { bank, lane } => (
            GenericInterfaceQspi,
            1,
            match (bank, lane) {
                (1, 0) => QsB1Io0,
                (1, 1) => QsB1Io1,
                (1, 2) => QsB1Io2,
                (1, _) => QsB1Io3,
                (_, 0) => QsB2Io0,
                (_, 1) => QsB2Io1,
                (_, 2) => QsB2Io2,
                (_, _) => QsB2Io3,
            },
        ),
        PinFunction::SdmmcCk { unit } => (GenericInterfaceSdmmc, *unit, SdCk),
        PinFunction::SdmmcCmd { unit } => (GenericInterfaceSdmmc, *unit, SdCmd),
        PinFunction::SdmmcD { unit, lane } => (
            GenericInterfaceSdmmc,
            *unit,
            match lane {
                0 => SdD0,
                1 => SdD1,
                2 => SdD2,
                3 => SdD3,
                4 => SdD4,
                5 => SdD5,
                6 => SdD6,
                _ => SdD7,
            },
        ),
        // CAN has a single instance on STM32F1 (CAN1) and pin functions without
        // an index, so the instance is fixed at 1.
        PinFunction::CanRx => (GenericInterfaceCan, 1, CanRx),
        PinFunction::CanTx => (GenericInterfaceCan, 1, CanTx),
        // USB is single-instance on STM32F1 and its pin functions carry no index.
        PinFunction::UsbDm => (GenericInterfaceUsb, 1, UsbDm),
        PinFunction::UsbDp => (GenericInterfaceUsb, 1, UsbDp),
        _ => return None,
    })
}

/// One terminal of a module that wires to an MCU pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ModuleSignal {
    // USART
    Tx,
    Rx,
    /// Flow control. NOT in `ModuleKind::signals()`: they are never auto-wired,
    /// because a serial device does not imply flow control. Assigning the pad on
    /// the canvas is what adds them, and `reconcile_modules` folds them into the
    /// module that owns the instance — the same route a PWM channel takes.
    Cts,
    Rts,
    // LPUART — its own peripheral, so its own signals: a chip can carry both
    // USART1 and LPUART1 and they must never share a wire.
    LpTx,
    LpRx,
    LpCts,
    LpRts,
    // SPI
    Sck,
    Mosi,
    Miso,
    Nss,
    // I2C
    Scl,
    Sda,
    // I2S — a clock, a word-select line, one data line, and an optional
    // master clock for the codec.
    I2sCk,
    I2sWs,
    I2sSd,
    I2sMck,
    // RMT
    /// The channel's single wire — an output on a transmit channel, an input on
    /// a receive one. One name for both, because it is one pad either way and
    /// the direction belongs to the channel.
    RmtLine,
    // DAC — one pad per channel, nothing shared but the block.
    DacOut1,
    DacOut2,
    // SAI — four pads per sub-block, and two sub-blocks per unit.
    SaiSckA,
    SaiSdA,
    SaiFsA,
    SaiMclkA,
    SaiSckB,
    SaiSdB,
    SaiFsB,
    SaiMclkB,
    // SDMMC — a clock, a command line, and up to eight data lanes.
    // QUADSPI — a shared clock, then a chip select and four data lines per
    // bank.
    // OCTOSPI — one port: a clock, a chip select, an optional strobe and up
    // to eight data lines.
    // XSPI — two chip selects, two strobes, up to sixteen data lines.
    // HSPI — one instance, one chip select, two strobes, sixteen data pads.
    HsClk,
    HsNcs,
    HsDqs0,
    HsDqs1,
    HsIo0,
    HsIo1,
    HsIo2,
    HsIo3,
    HsIo4,
    HsIo5,
    HsIo6,
    HsIo7,
    HsIo8,
    HsIo9,
    HsIo10,
    HsIo11,
    HsIo12,
    HsIo13,
    HsIo14,
    HsIo15,
    XsClk,
    XsNcs1,
    XsNcs2,
    XsDqs0,
    XsDqs1,
    XsIo0,
    XsIo1,
    XsIo2,
    XsIo3,
    XsIo4,
    XsIo5,
    XsIo6,
    XsIo7,
    XsIo8,
    XsIo9,
    XsIo10,
    XsIo11,
    XsIo12,
    XsIo13,
    XsIo14,
    XsIo15,
    OsClk,
    OsNcs,
    OsDqs,
    OsIo0,
    OsIo1,
    OsIo2,
    OsIo3,
    OsIo4,
    OsIo5,
    OsIo6,
    OsIo7,
    QsClk,
    QsB1Ncs,
    QsB2Ncs,
    QsB1Io0,
    QsB1Io1,
    QsB1Io2,
    QsB1Io3,
    QsB2Io0,
    QsB2Io1,
    QsB2Io2,
    QsB2Io3,
    SdCk,
    SdCmd,
    SdD0,
    SdD1,
    SdD2,
    SdD3,
    SdD4,
    SdD5,
    SdD6,
    SdD7,
    // PWM — one per timer channel.
    PwmCh1,
    PwmCh2,
    PwmCh3,
    PwmCh4,
    // …and their complementary halves. A separate signal rather than a flag on
    // the channel, because the pad is a different pin with its own wire.
    PwmCh1N,
    PwmCh2N,
    PwmCh3N,
    PwmCh4N,
    // …and the fault lines that switch every one of them off.
    PwmBkin1,
    PwmBkin2,
    // CAN
    CanRx,
    CanTx,
    // USB
    UsbDm,
    UsbDp,
    /// A pin of a user-authored [`ModuleKind::Custom`] module — it carries no
    /// peripheral meaning, so the pin keeps whatever function the user gave it.
    CustomPin,
}

impl ModuleSignal {
    pub fn label(self) -> &'static str {
        match self {
            ModuleSignal::RmtLine => "RMT",
            ModuleSignal::Tx => "TX",
            ModuleSignal::Rx => "RX",
            ModuleSignal::Cts => "CTS",
            ModuleSignal::Rts => "RTS",
            ModuleSignal::LpTx => "TX",
            ModuleSignal::LpRx => "RX",
            ModuleSignal::LpCts => "CTS",
            ModuleSignal::LpRts => "RTS",
            ModuleSignal::Sck => "SCK",
            ModuleSignal::Mosi => "MOSI",
            ModuleSignal::Miso => "MISO",
            ModuleSignal::Nss => "NSS",
            ModuleSignal::Scl => "SCL",
            ModuleSignal::Sda => "SDA",
            ModuleSignal::I2sCk => "CK",
            ModuleSignal::I2sWs => "WS",
            ModuleSignal::I2sSd => "SD",
            ModuleSignal::I2sMck => "MCK",
            ModuleSignal::DacOut1 => "OUT1",
            ModuleSignal::DacOut2 => "OUT2",
            ModuleSignal::SaiSckA => "A SCK",
            ModuleSignal::SaiSdA => "A SD",
            ModuleSignal::SaiFsA => "A FS",
            ModuleSignal::SaiMclkA => "A MCLK",
            ModuleSignal::SaiSckB => "B SCK",
            ModuleSignal::SaiSdB => "B SD",
            ModuleSignal::SaiFsB => "B FS",
            ModuleSignal::SaiMclkB => "B MCLK",
            ModuleSignal::HsClk => "CLK",
            ModuleSignal::HsNcs => "NCS",
            ModuleSignal::HsDqs0 => "DQS0",
            ModuleSignal::HsDqs1 => "DQS1",
            ModuleSignal::HsIo0 => "IO0",
            ModuleSignal::HsIo1 => "IO1",
            ModuleSignal::HsIo2 => "IO2",
            ModuleSignal::HsIo3 => "IO3",
            ModuleSignal::HsIo4 => "IO4",
            ModuleSignal::HsIo5 => "IO5",
            ModuleSignal::HsIo6 => "IO6",
            ModuleSignal::HsIo7 => "IO7",
            ModuleSignal::HsIo8 => "IO8",
            ModuleSignal::HsIo9 => "IO9",
            ModuleSignal::HsIo10 => "IO10",
            ModuleSignal::HsIo11 => "IO11",
            ModuleSignal::HsIo12 => "IO12",
            ModuleSignal::HsIo13 => "IO13",
            ModuleSignal::HsIo14 => "IO14",
            ModuleSignal::HsIo15 => "IO15",
            ModuleSignal::XsClk => "CLK",
            ModuleSignal::XsNcs1 => "NCS1",
            ModuleSignal::XsNcs2 => "NCS2",
            ModuleSignal::XsDqs0 => "DQS0",
            ModuleSignal::XsDqs1 => "DQS1",
            ModuleSignal::XsIo0 => "IO0",
            ModuleSignal::XsIo1 => "IO1",
            ModuleSignal::XsIo2 => "IO2",
            ModuleSignal::XsIo3 => "IO3",
            ModuleSignal::XsIo4 => "IO4",
            ModuleSignal::XsIo5 => "IO5",
            ModuleSignal::XsIo6 => "IO6",
            ModuleSignal::XsIo7 => "IO7",
            ModuleSignal::XsIo8 => "IO8",
            ModuleSignal::XsIo9 => "IO9",
            ModuleSignal::XsIo10 => "IO10",
            ModuleSignal::XsIo11 => "IO11",
            ModuleSignal::XsIo12 => "IO12",
            ModuleSignal::XsIo13 => "IO13",
            ModuleSignal::XsIo14 => "IO14",
            ModuleSignal::XsIo15 => "IO15",
            ModuleSignal::OsClk => "CLK",
            ModuleSignal::OsNcs => "NCS",
            ModuleSignal::OsDqs => "DQS",
            ModuleSignal::OsIo0 => "IO0",
            ModuleSignal::OsIo1 => "IO1",
            ModuleSignal::OsIo2 => "IO2",
            ModuleSignal::OsIo3 => "IO3",
            ModuleSignal::OsIo4 => "IO4",
            ModuleSignal::OsIo5 => "IO5",
            ModuleSignal::OsIo6 => "IO6",
            ModuleSignal::OsIo7 => "IO7",
            ModuleSignal::QsClk => "CLK",
            ModuleSignal::QsB1Ncs => "BK1 NCS",
            ModuleSignal::QsB2Ncs => "BK2 NCS",
            ModuleSignal::QsB1Io0 => "BK1 IO0",
            ModuleSignal::QsB1Io1 => "BK1 IO1",
            ModuleSignal::QsB1Io2 => "BK1 IO2",
            ModuleSignal::QsB1Io3 => "BK1 IO3",
            ModuleSignal::QsB2Io0 => "BK2 IO0",
            ModuleSignal::QsB2Io1 => "BK2 IO1",
            ModuleSignal::QsB2Io2 => "BK2 IO2",
            ModuleSignal::QsB2Io3 => "BK2 IO3",
            ModuleSignal::SdCk => "CK",
            ModuleSignal::SdCmd => "CMD",
            ModuleSignal::SdD0 => "D0",
            ModuleSignal::SdD1 => "D1",
            ModuleSignal::SdD2 => "D2",
            ModuleSignal::SdD3 => "D3",
            ModuleSignal::SdD4 => "D4",
            ModuleSignal::SdD5 => "D5",
            ModuleSignal::SdD6 => "D6",
            ModuleSignal::SdD7 => "D7",
            ModuleSignal::PwmCh1 => "CH1",
            ModuleSignal::PwmCh2 => "CH2",
            ModuleSignal::PwmCh3 => "CH3",
            ModuleSignal::PwmCh4 => "CH4",
            ModuleSignal::PwmCh1N => "CH1N",
            ModuleSignal::PwmCh2N => "CH2N",
            ModuleSignal::PwmCh3N => "CH3N",
            ModuleSignal::PwmCh4N => "CH4N",
            ModuleSignal::PwmBkin1 => "BKIN",
            ModuleSignal::PwmBkin2 => "BKIN2",
            ModuleSignal::CanRx => "RX",
            ModuleSignal::CanTx => "TX",
            ModuleSignal::UsbDm => "D-",
            ModuleSignal::UsbDp => "D+",
            ModuleSignal::CustomPin => "PIN",
        }
    }

    /// The MCU pin function this signal needs on peripheral `instance`.
    pub fn pin_function(self, instance: u8) -> PinFunction {
        match self {
            ModuleSignal::RmtLine => PinFunction::RmtChannel(instance),
            ModuleSignal::Tx => PinFunction::UsartTx(instance),
            ModuleSignal::Rx => PinFunction::UsartRx(instance),
            ModuleSignal::Cts => PinFunction::UsartCts(instance),
            ModuleSignal::Rts => PinFunction::UsartRts(instance),
            ModuleSignal::LpTx => PinFunction::LpuartTx(instance),
            ModuleSignal::LpRx => PinFunction::LpuartRx(instance),
            ModuleSignal::LpCts => PinFunction::LpuartCts(instance),
            ModuleSignal::LpRts => PinFunction::LpuartRts(instance),
            ModuleSignal::Sck => PinFunction::SpiSck(instance),
            ModuleSignal::Mosi => PinFunction::SpiMosi(instance),
            ModuleSignal::Miso => PinFunction::SpiMiso(instance),
            ModuleSignal::Nss => PinFunction::SpiNss(instance),
            ModuleSignal::Scl => PinFunction::I2cScl(instance),
            ModuleSignal::Sda => PinFunction::I2cSda(instance),
            ModuleSignal::I2sCk => PinFunction::I2sCk(instance),
            ModuleSignal::I2sWs => PinFunction::I2sWs(instance),
            ModuleSignal::I2sSd => PinFunction::I2sSd(instance),
            ModuleSignal::I2sMck => PinFunction::I2sMck(instance),
            ModuleSignal::DacOut1 => PinFunction::DacOut {
                dac: instance,
                channel: 1,
            },
            ModuleSignal::DacOut2 => PinFunction::DacOut {
                dac: instance,
                channel: 2,
            },
            ModuleSignal::SaiSckA => PinFunction::SaiSck {
                sai: instance,
                block: 1,
            },
            ModuleSignal::SaiSdA => PinFunction::SaiSd {
                sai: instance,
                block: 1,
            },
            ModuleSignal::SaiFsA => PinFunction::SaiFs {
                sai: instance,
                block: 1,
            },
            ModuleSignal::SaiMclkA => PinFunction::SaiMclk {
                sai: instance,
                block: 1,
            },
            ModuleSignal::SaiSckB => PinFunction::SaiSck {
                sai: instance,
                block: 2,
            },
            ModuleSignal::SaiSdB => PinFunction::SaiSd {
                sai: instance,
                block: 2,
            },
            ModuleSignal::SaiFsB => PinFunction::SaiFs {
                sai: instance,
                block: 2,
            },
            ModuleSignal::SaiMclkB => PinFunction::SaiMclk {
                sai: instance,
                block: 2,
            },
            ModuleSignal::HsClk => PinFunction::HspiClk { unit: instance },
            ModuleSignal::HsNcs => PinFunction::HspiNcs { unit: instance },
            ModuleSignal::HsDqs0 => PinFunction::HspiDqs {
                unit: instance,
                index: 0,
            },
            ModuleSignal::HsDqs1 => PinFunction::HspiDqs {
                unit: instance,
                index: 1,
            },
            ModuleSignal::HsIo0 => PinFunction::HspiIo {
                unit: instance,
                lane: 0,
            },
            ModuleSignal::HsIo1 => PinFunction::HspiIo {
                unit: instance,
                lane: 1,
            },
            ModuleSignal::HsIo2 => PinFunction::HspiIo {
                unit: instance,
                lane: 2,
            },
            ModuleSignal::HsIo3 => PinFunction::HspiIo {
                unit: instance,
                lane: 3,
            },
            ModuleSignal::HsIo4 => PinFunction::HspiIo {
                unit: instance,
                lane: 4,
            },
            ModuleSignal::HsIo5 => PinFunction::HspiIo {
                unit: instance,
                lane: 5,
            },
            ModuleSignal::HsIo6 => PinFunction::HspiIo {
                unit: instance,
                lane: 6,
            },
            ModuleSignal::HsIo7 => PinFunction::HspiIo {
                unit: instance,
                lane: 7,
            },
            ModuleSignal::HsIo8 => PinFunction::HspiIo {
                unit: instance,
                lane: 8,
            },
            ModuleSignal::HsIo9 => PinFunction::HspiIo {
                unit: instance,
                lane: 9,
            },
            ModuleSignal::HsIo10 => PinFunction::HspiIo {
                unit: instance,
                lane: 10,
            },
            ModuleSignal::HsIo11 => PinFunction::HspiIo {
                unit: instance,
                lane: 11,
            },
            ModuleSignal::HsIo12 => PinFunction::HspiIo {
                unit: instance,
                lane: 12,
            },
            ModuleSignal::HsIo13 => PinFunction::HspiIo {
                unit: instance,
                lane: 13,
            },
            ModuleSignal::HsIo14 => PinFunction::HspiIo {
                unit: instance,
                lane: 14,
            },
            ModuleSignal::HsIo15 => PinFunction::HspiIo {
                unit: instance,
                lane: 15,
            },
            ModuleSignal::XsClk => PinFunction::XspiClk { port: instance },
            ModuleSignal::XsNcs1 => PinFunction::XspiNcs {
                port: instance,
                cs: 1,
            },
            ModuleSignal::XsNcs2 => PinFunction::XspiNcs {
                port: instance,
                cs: 2,
            },
            ModuleSignal::XsDqs0 => PinFunction::XspiDqs {
                port: instance,
                index: 0,
            },
            ModuleSignal::XsDqs1 => PinFunction::XspiDqs {
                port: instance,
                index: 1,
            },
            ModuleSignal::XsIo0 => PinFunction::XspiIo {
                port: instance,
                lane: 0,
            },
            ModuleSignal::XsIo1 => PinFunction::XspiIo {
                port: instance,
                lane: 1,
            },
            ModuleSignal::XsIo2 => PinFunction::XspiIo {
                port: instance,
                lane: 2,
            },
            ModuleSignal::XsIo3 => PinFunction::XspiIo {
                port: instance,
                lane: 3,
            },
            ModuleSignal::XsIo4 => PinFunction::XspiIo {
                port: instance,
                lane: 4,
            },
            ModuleSignal::XsIo5 => PinFunction::XspiIo {
                port: instance,
                lane: 5,
            },
            ModuleSignal::XsIo6 => PinFunction::XspiIo {
                port: instance,
                lane: 6,
            },
            ModuleSignal::XsIo7 => PinFunction::XspiIo {
                port: instance,
                lane: 7,
            },
            ModuleSignal::XsIo8 => PinFunction::XspiIo {
                port: instance,
                lane: 8,
            },
            ModuleSignal::XsIo9 => PinFunction::XspiIo {
                port: instance,
                lane: 9,
            },
            ModuleSignal::XsIo10 => PinFunction::XspiIo {
                port: instance,
                lane: 10,
            },
            ModuleSignal::XsIo11 => PinFunction::XspiIo {
                port: instance,
                lane: 11,
            },
            ModuleSignal::XsIo12 => PinFunction::XspiIo {
                port: instance,
                lane: 12,
            },
            ModuleSignal::XsIo13 => PinFunction::XspiIo {
                port: instance,
                lane: 13,
            },
            ModuleSignal::XsIo14 => PinFunction::XspiIo {
                port: instance,
                lane: 14,
            },
            ModuleSignal::XsIo15 => PinFunction::XspiIo {
                port: instance,
                lane: 15,
            },
            ModuleSignal::OsClk => PinFunction::OspiClk { port: instance },
            ModuleSignal::OsNcs => PinFunction::OspiNcs { port: instance },
            ModuleSignal::OsDqs => PinFunction::OspiDqs { port: instance },
            ModuleSignal::OsIo0 => PinFunction::OspiIo {
                port: instance,
                lane: 0,
            },
            ModuleSignal::OsIo1 => PinFunction::OspiIo {
                port: instance,
                lane: 1,
            },
            ModuleSignal::OsIo2 => PinFunction::OspiIo {
                port: instance,
                lane: 2,
            },
            ModuleSignal::OsIo3 => PinFunction::OspiIo {
                port: instance,
                lane: 3,
            },
            ModuleSignal::OsIo4 => PinFunction::OspiIo {
                port: instance,
                lane: 4,
            },
            ModuleSignal::OsIo5 => PinFunction::OspiIo {
                port: instance,
                lane: 5,
            },
            ModuleSignal::OsIo6 => PinFunction::OspiIo {
                port: instance,
                lane: 6,
            },
            ModuleSignal::OsIo7 => PinFunction::OspiIo {
                port: instance,
                lane: 7,
            },
            ModuleSignal::QsClk => PinFunction::QspiClk,
            ModuleSignal::QsB1Ncs => PinFunction::QspiNcs { bank: 1 },
            ModuleSignal::QsB2Ncs => PinFunction::QspiNcs { bank: 2 },
            ModuleSignal::QsB1Io0 => PinFunction::QspiIo { bank: 1, lane: 0 },
            ModuleSignal::QsB1Io1 => PinFunction::QspiIo { bank: 1, lane: 1 },
            ModuleSignal::QsB1Io2 => PinFunction::QspiIo { bank: 1, lane: 2 },
            ModuleSignal::QsB1Io3 => PinFunction::QspiIo { bank: 1, lane: 3 },
            ModuleSignal::QsB2Io0 => PinFunction::QspiIo { bank: 2, lane: 0 },
            ModuleSignal::QsB2Io1 => PinFunction::QspiIo { bank: 2, lane: 1 },
            ModuleSignal::QsB2Io2 => PinFunction::QspiIo { bank: 2, lane: 2 },
            ModuleSignal::QsB2Io3 => PinFunction::QspiIo { bank: 2, lane: 3 },
            ModuleSignal::SdCk => PinFunction::SdmmcCk { unit: instance },
            ModuleSignal::SdCmd => PinFunction::SdmmcCmd { unit: instance },
            ModuleSignal::SdD0 => PinFunction::SdmmcD {
                unit: instance,
                lane: 0,
            },
            ModuleSignal::SdD1 => PinFunction::SdmmcD {
                unit: instance,
                lane: 1,
            },
            ModuleSignal::SdD2 => PinFunction::SdmmcD {
                unit: instance,
                lane: 2,
            },
            ModuleSignal::SdD3 => PinFunction::SdmmcD {
                unit: instance,
                lane: 3,
            },
            ModuleSignal::SdD4 => PinFunction::SdmmcD {
                unit: instance,
                lane: 4,
            },
            ModuleSignal::SdD5 => PinFunction::SdmmcD {
                unit: instance,
                lane: 5,
            },
            ModuleSignal::SdD6 => PinFunction::SdmmcD {
                unit: instance,
                lane: 6,
            },
            ModuleSignal::SdD7 => PinFunction::SdmmcD {
                unit: instance,
                lane: 7,
            },
            // `instance` is the TIMER for these, and the variant is the channel.
            ModuleSignal::PwmCh1 => PinFunction::TimerPwm {
                timer: instance,
                channel: 1,
            },
            ModuleSignal::PwmCh2 => PinFunction::TimerPwm {
                timer: instance,
                channel: 2,
            },
            ModuleSignal::PwmCh3 => PinFunction::TimerPwm {
                timer: instance,
                channel: 3,
            },
            ModuleSignal::PwmCh4 => PinFunction::TimerPwm {
                timer: instance,
                channel: 4,
            },
            ModuleSignal::PwmCh1N => PinFunction::TimerPwmN {
                timer: instance,
                channel: 1,
            },
            ModuleSignal::PwmCh2N => PinFunction::TimerPwmN {
                timer: instance,
                channel: 2,
            },
            ModuleSignal::PwmCh3N => PinFunction::TimerPwmN {
                timer: instance,
                channel: 3,
            },
            ModuleSignal::PwmCh4N => PinFunction::TimerPwmN {
                timer: instance,
                channel: 4,
            },
            ModuleSignal::PwmBkin1 => PinFunction::TimerBreak {
                timer: instance,
                input: 1,
            },
            ModuleSignal::PwmBkin2 => PinFunction::TimerBreak {
                timer: instance,
                input: 2,
            },
            // CAN pin functions carry no instance (single CAN on STM32F1).
            ModuleSignal::CanRx => PinFunction::CanRx,
            ModuleSignal::CanTx => PinFunction::CanTx,
            // USB pin functions carry no instance (single USB FS on STM32F1).
            ModuleSignal::UsbDm => PinFunction::UsbDm,
            ModuleSignal::UsbDp => PinFunction::UsbDp,
            // A custom pin imposes nothing; report GPIO so callers that colour
            // by function still get a sensible value.
            ModuleSignal::CustomPin => PinFunction::GpioOutput,
        }
    }
}

/// A wire from a module terminal to a specific MCU pin (by pin number).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub signal: ModuleSignal,
    pub mcu_pin: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Parity {
    None,
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopBits {
    One,
    Two,
}

/// Which init API a Virtual Module's generated `pins/configs/*.rs` exposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ApiStyle {
    /// `init` returns a STANDARD `embedded-io` / `embedded-hal` 1.0 value, so
    /// driver/app code is portable across HALs. The wrapper's `.0` still gives
    /// the raw HAL object back. This is the default.
    #[default]
    Portable,
    /// `init` returns the CONCRETE `stm32f1xx-hal` type (`Serial`/`Spi`/
    /// `BlockingI2c`) — no bridge, no extra trait crates. Max HAL features.
    Native,
}

/// For the **async** (embassy) runtime, how a SPI/I2C module's generated init
/// drives the bus. (Ignored on the blocking runtime, where [`ApiStyle`] applies
/// instead.) embassy's async SPI/I2C REQUIRE DMA channels, which the IDE doesn't
/// model — so async-DMA leaves a `TODO` in `main.rs` for the user to fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AsyncBusMode {
    /// `init` uses embassy's `new_blocking` and returns `impl embedded_hal::…`
    /// (the blocking `SpiBus`/`I2c` 1.0 traits). No DMA — compiles out of the
    /// box. A blocking driver in an async project is a common, valid pattern.
    /// The default.
    #[default]
    Blocking,
    /// `init` uses embassy's DMA `new` and returns `impl embedded_hal_async::…`
    /// (`.await`-able). Needs DMA channels: `main.rs` gets a `TODO` line where
    /// you pass the channels valid for that peripheral on your chip.
    AsyncDma,
}

/// How an async USART moves its bytes.
///
/// Deliberately NOT [`AsyncBusMode`]: that enum's `Blocking` arm makes no sense
/// here, because an async USART is never blocking — the choice is only about
/// which non-blocking mechanism carries the data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsartMode {
    /// `BufferedUart` — interrupt per byte into a software ring buffer. Needs no
    /// DMA channel, so it compiles for any chip out of the box. The default.
    #[default]
    Buffered,
    /// `UartTx` + `RingBufferedUartRx` — the peripheral talks to DMA directly.
    ///
    /// Split on purpose rather than a plain `Uart<Async>`: that type implements
    /// `embedded_io_async::Write` but **not `Read`**, so a bare DMA `Uart` would
    /// quietly drop half of the portable API. `RingBufferedUartRx` restores it
    /// AND is the reason to want DMA on a UART at all — continuous reception
    /// that cannot drop bytes between reads.
    ///
    /// Needs DMA channels; `main.rs` gets a `TODO` where they go.
    Dma,
}

/// Which halves of a USART are built — CubeMX calls it "Data Direction".
///
/// It is not cosmetic: a one-way UART frees the other pin. Which options are
/// CONSTRUCTIBLE depends on the transport, see [`UsartDirection::options`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UsartDirection {
    /// Receive and transmit — the default, and the only thing the buffered
    /// transport can build.
    #[default]
    TxRx,
    /// Transmit only; the RX pin is not needed at all.
    TxOnly,
    /// Receive only; the TX pin is not needed at all.
    RxOnly,
    /// Single wire (half duplex) on the TX pad — CubeMX's "Single Wire
    /// (Half-Duplex)". One open-drain line carries both directions, which is how
    /// a servo bus, a DMX driver or a 1-wire-style sensor is wired.
    HalfDuplexOnTx,
    /// The same, on the RX pad instead (embassy swaps RX/TX internally).
    HalfDuplexOnRx,
}

impl UsartDirection {
    pub fn as_token(self) -> &'static str {
        match self {
            Self::TxRx => "TxRx",
            Self::TxOnly => "TxOnly",
            Self::RxOnly => "RxOnly",
            Self::HalfDuplexOnTx => "HalfDuplexOnTx",
            Self::HalfDuplexOnRx => "HalfDuplexOnRx",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::TxRx => "Receive and transmit",
            Self::TxOnly => "Transmit only",
            Self::RxOnly => "Receive only",
            Self::HalfDuplexOnTx => "Single wire (half-duplex), on TX pad",
            Self::HalfDuplexOnRx => "Single wire (half-duplex), on RX pad",
        }
    }

    /// One pad carries both directions.
    pub fn is_half_duplex(self) -> bool {
        matches!(self, Self::HalfDuplexOnTx | Self::HalfDuplexOnRx)
    }

    /// `true` when this direction needs the TX pin wired.
    pub fn needs_tx(self) -> bool {
        matches!(self, Self::TxRx | Self::TxOnly | Self::HalfDuplexOnTx)
    }

    /// `true` when this direction needs the RX pin wired.
    pub fn needs_rx(self) -> bool {
        matches!(self, Self::TxRx | Self::RxOnly | Self::HalfDuplexOnRx)
    }

    /// Which DMA channels the constructor takes, as `(tx, rx)`.
    ///
    /// NOT the same question as which PADS it takes: half duplex has one pad but
    /// builds a whole `Uart`, so embassy asks for both channels. Conflating the
    /// two hands `new_half_duplex` one argument too few.
    pub fn dma_halves(self) -> (bool, bool) {
        match self {
            Self::TxOnly => (true, false),
            Self::RxOnly => (false, true),
            _ => (true, true),
        }
    }

    /// The directions `transport` can actually build.
    ///
    /// The buffered driver has NO half of its own — `BufferedUartTx` and
    /// `BufferedUartRx` exist only as the result of `BufferedUart::split()`, so
    /// both pins are consumed either way and "TX only" would be a lie. On DMA,
    /// `UartTx::new` / `UartRx::new` are real constructors that take one pin.
    pub fn options(transport: UsartMode) -> &'static [UsartDirection] {
        match transport {
            // Half duplex IS available buffered — it is `BufferedUart` with one
            // pad, not a half of one, which is exactly why TX-only is not.
            UsartMode::Buffered => &[
                UsartDirection::TxRx,
                UsartDirection::HalfDuplexOnTx,
                UsartDirection::HalfDuplexOnRx,
            ],
            UsartMode::Dma => &[
                UsartDirection::TxRx,
                UsartDirection::TxOnly,
                UsartDirection::RxOnly,
                UsartDirection::HalfDuplexOnTx,
                UsartDirection::HalfDuplexOnRx,
            ],
        }
    }
}

/// Hardware flow control — CubeMX's "Hardware Flow Control (RS232)" plus the
/// RS485 driver-enable line, which is the same pad on an STM32.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UsartFlow {
    #[default]
    None,
    /// CTS only — the far end tells us when to pause.
    Cts,
    /// RTS only — we tell the far end when to pause.
    Rts,
    CtsRts,
    /// RS485 driver enable, asserted around each frame. On STM32 this is the
    /// RTS pad (ST names the signal `RTS_DE`), so it wires like RTS.
    De,
}

impl UsartFlow {
    pub fn as_token(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Cts => "Cts",
            Self::Rts => "Rts",
            Self::CtsRts => "CtsRts",
            Self::De => "De",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Disable",
            Self::Cts => "CTS only",
            Self::Rts => "RTS only",
            Self::CtsRts => "CTS/RTS",
            Self::De => "RS485 driver enable (DE)",
        }
    }

    /// Whether this option needs the CTS / RTS pin wired.
    pub fn needs_cts(self) -> bool {
        matches!(self, Self::Cts | Self::CtsRts)
    }

    /// DE shares the RTS pad, so it counts as needing it.
    pub fn needs_rts(self) -> bool {
        matches!(self, Self::Rts | Self::CtsRts | Self::De)
    }

    /// What embassy can build for this `(transport, direction)` pair — the
    /// constructor list, not the hardware's.
    ///
    /// The irregularity is embassy's: `BufferedUart` has `new_with_rts` but no
    /// `new_with_cts`, while the DMA `Uart` has neither on its own and offers
    /// CTS-only / RTS-only through the one-way `UartTx` / `UartRx` instead.
    /// Offering a combination with no constructor would be a UI that lies.
    pub fn options(transport: UsartMode, direction: UsartDirection) -> &'static [UsartFlow] {
        // No half-duplex constructor takes a flow pad — one wire, no side band.
        if direction.is_half_duplex() {
            return &[UsartFlow::None];
        }
        match (transport, direction) {
            (UsartMode::Buffered, _) => &[
                UsartFlow::None,
                UsartFlow::Rts,
                UsartFlow::CtsRts,
                UsartFlow::De,
            ],
            (UsartMode::Dma, UsartDirection::TxRx) => {
                &[UsartFlow::None, UsartFlow::CtsRts, UsartFlow::De]
            }
            (UsartMode::Dma, UsartDirection::TxOnly) => &[UsartFlow::None, UsartFlow::Cts],
            (UsartMode::Dma, UsartDirection::RxOnly) => &[UsartFlow::None, UsartFlow::Rts],
            // Unreachable: the half-duplex directions returned above.
            (UsartMode::Dma, _) => &[UsartFlow::None],
        }
    }
}

/// Which HALVES of a bus run on DMA, on the STM32F1 blocking runtime.
///
/// Not a bool, because the two directions are independent in the HAL and in
/// practice: a receiver that must not drop bytes wants DMA, while a transmitter
/// sending short bursts is often better off written straight from the CPU —
/// `writeln!` on a plain `Tx` instead of a transfer that consumes the handle
/// and hands it back from `wait()`. Leaving TX off also leaves its channel free
/// for another peripheral, which matters on a chip with seven of them.
///
/// `stm32f1xx-hal` supports every combination directly: `Tx::with_dma` and
/// `Rx::with_dma` are separate methods on the split halves, and SPI has
/// `with_tx_dma` / `with_rx_dma` / `with_rx_tx_dma`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BlockingDma {
    /// Both directions polled by the CPU.
    #[default]
    Off,
    /// Transmit on DMA, receive polled.
    Tx,
    /// Receive on DMA, transmit polled.
    Rx,
    /// Both directions on DMA.
    Both,
}

impl BlockingDma {
    pub const ALL: [BlockingDma; 4] = [
        BlockingDma::Off,
        BlockingDma::Tx,
        BlockingDma::Rx,
        BlockingDma::Both,
    ];

    pub fn tx(self) -> bool {
        matches!(self, BlockingDma::Tx | BlockingDma::Both)
    }

    pub fn rx(self) -> bool {
        matches!(self, BlockingDma::Rx | BlockingDma::Both)
    }

    /// Is any half on DMA? Decides whether the config file uses the DMA
    /// template at all, and whether `main.rs` needs `DMA1.split()`.
    pub fn any(self) -> bool {
        self != BlockingDma::Off
    }

    /// The same choice with the receive half dropped: `Both` becomes `Tx`,
    /// `Rx` becomes `Off`.
    ///
    /// For a bus that has no receive LINE — an SPI wired SCK+MOSI without
    /// MISO. The HAL still builds `with_rx_dma` there (the pin is only a
    /// type-state placeholder), so nothing complains: the channel is reserved,
    /// the transfer runs, and it clocks in whatever the unconfigured pad
    /// happens to read. A channel spent on garbage is worse than no channel.
    pub fn without_rx(self) -> Self {
        match self {
            BlockingDma::Both | BlockingDma::Tx => BlockingDma::Tx,
            BlockingDma::Rx | BlockingDma::Off => BlockingDma::Off,
        }
    }

    /// For `skip_serializing_if`: the default never reaches `mcu.config`.
    fn is_off(&self) -> bool {
        *self == BlockingDma::Off
    }

    /// The persisted spelling. Round-trips through [`Self::from_token`].
    pub fn token(self) -> &'static str {
        match self {
            BlockingDma::Off => "Off",
            BlockingDma::Tx => "Tx",
            BlockingDma::Rx => "Rx",
            BlockingDma::Both => "Both",
        }
    }

    /// The inverse. `None` for anything unrecognised, which the caller reads as
    /// the default rather than as a reason to drop the whole module list.
    pub fn from_token(t: &str) -> Option<BlockingDma> {
        Self::ALL.into_iter().find(|v| v.token() == t)
    }

    pub fn label(self) -> &'static str {
        match self {
            BlockingDma::Off => "Off - the CPU moves every byte",
            BlockingDma::Tx => "TX only",
            BlockingDma::Rx => "RX only",
            BlockingDma::Both => "TX and RX",
        }
    }
}

/// Written as a STRING rather than a RON enum, and read back from a string OR
/// from the `true` / `false` this field held when it was a bool.
///
/// A string because serde's `untagged` shim below cannot recognise a bare RON
/// identifier, so serialising as `Both` would round-trip into a parse error;
/// and the compatibility matters because RON reads `@modules` as ONE value —
/// a field that fails takes every Virtual Module with it, and the user finds an
/// empty canvas.
impl Serialize for BlockingDma {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.token())
    }
}

impl<'de> Deserialize<'de> for BlockingDma {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Shim {
            Text(String),
            Legacy(bool),
        }
        Ok(match Shim::deserialize(d)? {
            Shim::Text(t) => BlockingDma::from_token(&t).unwrap_or_default(),
            // The bool meant "both halves", which is all it could mean.
            Shim::Legacy(true) => BlockingDma::Both,
            Shim::Legacy(false) => BlockingDma::Off,
        })
    }
}

/// USART communication settings + the user's RX/TX data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsartModuleConfig {
    /// USART peripheral instance the module is wired to (1/2/3…).
    pub instance: u8,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: StopBits,
    /// Free-text data model the user authors for received / transmitted frames.
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated peripheral-handle variable names
    /// (e.g. `_tx1_imu`). `#[serde(default)]` keeps old `@modules` markers valid.
    #[serde(default)]
    pub custom_label: String,
    /// Portable (embedded-io) vs native (concrete HAL) init. `#[serde(default)]`
    /// → old configs load as `Portable`.
    #[serde(default)]
    pub api_style: ApiStyle,
    /// Which halves to build (CubeMX's "Data Direction"). Default `TxRx`, which
    /// is what every project generated before this existed.
    #[serde(default)]
    pub direction: UsartDirection,
    /// Hardware flow control. Default `None`, as before.
    #[serde(default)]
    pub flow: UsartFlow,
    /// Swap the RX and TX pads in the peripheral, so a crossed cable (or a
    /// board laid out the other way round) needs no rework. Chip-gated: the
    /// register bit only exists on the newer USART.
    #[serde(default)]
    pub swap_rx_tx: bool,
    /// Invert the TX line's idle/mark levels — for an inverting transceiver, or
    /// an IR link, without an external inverter. Same chip gate.
    #[serde(default)]
    pub invert_tx: bool,
    /// Invert the RX line, independently of TX.
    #[serde(default)]
    pub invert_rx: bool,
    /// Half duplex only: keep the receiver enabled while transmitting, so what
    /// this node sends is read back. Default OFF, which is what a bus with other
    /// talkers wants — the echo would otherwise land in the RX buffer.
    #[serde(default)]
    pub half_duplex_readback: bool,
    /// Buffered (interrupt) vs DMA transport, on the Async runtime only. The
    /// other runtimes ignore it. `#[serde(default)]` → old configs load as
    /// `Buffered`, which is what they generated.
    #[serde(default)]
    pub mode: UsartMode,
    /// DMA channels chosen BY HAND, empty = let the IDE allocate (the normal
    /// case). Named as the embassy singleton, `DMA1_CH4` / `GPDMA1_CH0`.
    ///
    /// The automatic allocation is correct but arbitrary among the channels the
    /// chip allows: it takes the first free one. A board can need a specific
    /// channel anyway - to leave a high-priority one for an ADC, to match an
    /// existing driver, or to work around an erratum - and that is not
    /// something the IDE can infer. Reserved before anything is allocated, so a
    /// hand-picked channel is never handed to another peripheral too.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dma_tx: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dma_rx: String,
    /// Blocking runtime on STM32F1: which halves of this bus run on DMA.
    ///
    /// A different HAL from the async `mode` / `async_mode`:
    /// `stm32f1xx-hal`'s `with_dma`, not embassy's. There is no channel to
    /// choose here — the F1 HAL fixes it per peripheral in its TYPES (USART1 is
    /// dma1::C4/C5, SPI1 dma1::C2/C3), which is why this names DIRECTIONS and
    /// not channels, unlike [`Self::dma_tx`].
    ///
    /// Ignored on every other family and on the Async/Native/RTIC runtimes.
    #[serde(default, skip_serializing_if = "BlockingDma::is_off")]
    pub blocking_dma: BlockingDma,
    /// Bytes of receive buffer, on the Async runtime.
    ///
    /// It means two different things depending on [`mode`](Self::mode), which
    /// is why the label in the UI changes with it:
    ///
    /// * **Buffered** - the size of BOTH software ring buffers, TX and RX. The
    ///   CPU copies byte by byte on each interrupt, so this has to cover what
    ///   the peripheral produces between your reads.
    /// * **DMA** - the circular buffer the controller fills on its own. It only
    ///   has to cover the longest GAP between your reads; reception never
    ///   stops, and overrunning it drops the oldest bytes silently.
    ///
    /// A TX-only DMA link has no buffer at all (the controller sends straight
    /// from your slice), so the field is hidden there rather than shown doing
    /// nothing.
    #[serde(default = "default_usart_buf")]
    pub buf_len: u32,
}

/// 256 bytes - about 22 ms of headroom at 115200 baud, and what every project
/// generated before the size was configurable.
fn default_usart_buf() -> u32 {
    256
}

impl UsartModuleConfig {
    /// Sensible defaults (115200 8N1) for the given USART instance.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            baud_rate: 115_200,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: StopBits::One,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
            api_style: ApiStyle::default(),
            direction: UsartDirection::default(),
            flow: UsartFlow::default(),
            swap_rx_tx: false,
            invert_tx: false,
            invert_rx: false,
            half_duplex_readback: false,
            mode: UsartMode::default(),
            dma_tx: String::new(),
            dma_rx: String::new(),
            buf_len: default_usart_buf(),
            blocking_dma: BlockingDma::default(),
        }
    }
}

/// Which end of a byte goes on the wire first (embassy's `spi::BitOrder`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpiBitOrder {
    /// Most significant bit first - embassy's default and what nearly every
    /// device expects.
    #[default]
    MsbFirst,
    LsbFirst,
}

impl SpiBitOrder {
    /// The `embassy_stm32::spi::BitOrder` variant name.
    pub fn embassy(self) -> &'static str {
        match self {
            SpiBitOrder::MsbFirst => "MsbFirst",
            SpiBitOrder::LsbFirst => "LsbFirst",
        }
    }
}

/// SPI device settings + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiModuleConfig {
    pub instance: u8,
    /// SPI mode 0..=3 (CPOL/CPHA).
    pub mode: u8,
    /// Bus clock in Hz.
    pub clock_hz: u32,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_spiN` handle (e.g. `_spi1_imu`).
    #[serde(default)]
    pub custom_label: String,
    /// Portable (embedded-hal 1.0 `SpiBus`) vs native (`Spi<…>`) init.
    #[serde(default)]
    pub api_style: ApiStyle,
    /// Async runtime only: blocking vs async-DMA embassy init. Ignored on the
    /// blocking runtime (`api_style` applies there).
    #[serde(default)]
    pub async_mode: AsyncBusMode,
    /// DMA channels chosen BY HAND, empty = let the IDE allocate (the normal
    /// case). Named as the embassy singleton, `DMA1_CH4` / `GPDMA1_CH0`.
    ///
    /// The automatic allocation is correct but arbitrary among the channels the
    /// chip allows: it takes the first free one. A board can need a specific
    /// channel anyway - to leave a high-priority one for an ADC, to match an
    /// existing driver, or to work around an erratum - and that is not
    /// something the IDE can infer. Reserved before anything is allocated, so a
    /// hand-picked channel is never handed to another peripheral too.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dma_tx: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dma_rx: String,
    /// Blocking runtime on STM32F1: which halves of this bus run on DMA.
    ///
    /// A different HAL from the async `mode` / `async_mode`:
    /// `stm32f1xx-hal`'s `with_dma`, not embassy's. There is no channel to
    /// choose here — the F1 HAL fixes it per peripheral in its TYPES (USART1 is
    /// dma1::C4/C5, SPI1 dma1::C2/C3), which is why this names DIRECTIONS and
    /// not channels, unlike [`Self::dma_tx`].
    ///
    /// Ignored on every other family and on the Async/Native/RTIC runtimes.
    #[serde(default, skip_serializing_if = "BlockingDma::is_off")]
    pub blocking_dma: BlockingDma,
    /// Which end of a byte goes on the wire first.
    ///
    /// MSB first is what almost every device wants and embassy's default, but
    /// it is not universal - some sensors and shift registers are LSB first,
    /// and getting it wrong gives bit-reversed data rather than silence, which
    /// is why it is worth a field instead of a comment.
    ///
    /// Async runtime only: the STM32F1 HAL takes no bit-order argument.
    #[serde(default)]
    pub bit_order: SpiBitOrder,
}

impl SpiModuleConfig {
    /// Defaults: mode 0, 1 MHz.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            mode: 0,
            clock_hz: 1_000_000,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
            api_style: ApiStyle::default(),
            async_mode: AsyncBusMode::default(),
            bit_order: SpiBitOrder::default(),
            dma_tx: String::new(),
            dma_rx: String::new(),
            blocking_dma: BlockingDma::default(),
        }
    }
}

/// I2C device settings + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct I2cModuleConfig {
    pub instance: u8,
    /// Bus clock in Hz (100 kHz standard / 400 kHz fast).
    pub clock_hz: u32,
    /// 7-bit device address.
    pub address: u8,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_i2cN` handle (e.g. `_i2c1_imu`).
    #[serde(default)]
    pub custom_label: String,
    /// Portable (embedded-hal 1.0 `I2c`) vs native (`BlockingI2c<…>`) init.
    #[serde(default)]
    pub api_style: ApiStyle,
    /// Async runtime only: blocking vs async-DMA embassy init. Ignored on the
    /// blocking runtime (`api_style` applies there).
    #[serde(default)]
    pub async_mode: AsyncBusMode,
    /// DMA channels chosen BY HAND, empty = let the IDE allocate (the normal
    /// case). Named as the embassy singleton, `DMA1_CH4` / `GPDMA1_CH0`.
    ///
    /// The automatic allocation is correct but arbitrary among the channels the
    /// chip allows: it takes the first free one. A board can need a specific
    /// channel anyway - to leave a high-priority one for an ADC, to match an
    /// existing driver, or to work around an erratum - and that is not
    /// something the IDE can infer. Reserved before anything is allocated, so a
    /// hand-picked channel is never handed to another peripheral too.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dma_tx: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dma_rx: String,
    /// How long a transfer may take before it gives up, in milliseconds.
    ///
    /// `0` = leave embassy's default (1000 ms) - and that is what it stays
    /// unless you say otherwise, so no existing project's output moves.
    ///
    /// It matters because I2C hangs are a real failure mode: a device that
    /// stretches the clock forever, or a bus with no pull-ups, blocks the
    /// transfer for as long as the timeout allows.
    ///
    /// Async runtime only. `embassy_time::Duration` needs embassy-stm32's
    /// `time` feature, which the async dependency line enables (via
    /// `time-driver-any`) and the blocking one does not.
    #[serde(default)]
    pub timeout_ms: u32,
}

impl I2cModuleConfig {
    /// Defaults: 100 kHz, address 0x00.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            clock_hz: 100_000,
            address: 0x00,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
            api_style: ApiStyle::default(),
            async_mode: AsyncBusMode::default(),
            timeout_ms: 0,
            dma_tx: String::new(),
            dma_rx: String::new(),
        }
    }
}

/// PWM settings for one TIMER: the frequency its channels share, and a duty
/// cycle per channel.
///
/// Frequency is per-MODULE because it is per-timer in silicon (one prescaler,
/// one reload value); duty is per-channel because that is the only thing a
/// channel owns. A channel with no entry in `duty` starts at 0 % — output
/// enabled, pin low — which is the safe state for a motor driver or a LED.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerModuleConfig {
    /// The TIMER this module drives (TIM1, TIM3, …).
    pub instance: u8,
    /// Shared output frequency in Hz.
    pub freq_hz: u32,
    /// Channel number (1..=4) → duty in HUNDREDTHS of a percent (0..=10_000).
    ///
    /// Not whole percent, because whole percent cannot express the first duty
    /// most people reach for: a hobby servo wants 1.5 ms of a 20 ms frame,
    /// which is 7.5 %. Both backends can take an exact ratio — embassy's
    /// `set_duty_cycle_fraction`, and the F1 HAL's `set_duty` over
    /// `get_max_duty()` — so the resolution missing here was the model's, never
    /// the hardware's.
    #[serde(default)]
    pub duty_x100: std::collections::BTreeMap<u8, u16>,
    /// Whole-percent duty, as written by versions before [`Self::duty_x100`].
    /// Folded into it by [`Self::migrate_duty`] on load and never written back.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub duty: std::collections::BTreeMap<u8, u8>,
    /// How the counter runs — one setting for the whole timer.
    #[serde(default)]
    pub counting: PwmCounting,
    /// Dead time between a channel's pad and its complementary one, in timer
    /// ticks on the same scale as the duty compare value.
    ///
    /// One setting for the whole timer, because there is one BDTR register.
    /// Only reaches the code when a `CHxN` pad is wired — without one there is
    /// no pair to separate. Zero is no dead time at all, which is fine for a
    /// pair driving independent loads and fatal for a half-bridge.
    #[serde(default)]
    pub dead_time: u16,
    /// Per-channel output shape. Absent = every default, which is also the
    /// timer's reset state.
    #[serde(default)]
    pub channels: std::collections::BTreeMap<u8, PwmChannelConfig>,
    /// Break inputs by index — 1 is BKIN, 2 is BKIN2. An entry exists only for
    /// an input whose pad is actually wired; break is not something you enable
    /// in the abstract, it is a line coming in from the board.
    #[serde(default)]
    pub breaks: std::collections::BTreeMap<u8, BreakInputConfig>,
    /// Automatic output enable: after a fault clears, the outputs come back on
    /// the next update event by themselves. Off means they stay dark until
    /// software says otherwise, which is the safer of the two and the reset
    /// state. One bit for the whole timer, whichever input tripped.
    #[serde(default)]
    pub auto_output_enable: bool,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_pwmN` handle (e.g. `_pwm3_servo`).
    #[serde(default)]
    pub custom_label: String,
}

impl TimerModuleConfig {
    /// Defaults: 1 kHz, every channel at 0 %.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            freq_hz: 1_000,
            duty_x100: std::collections::BTreeMap::new(),
            duty: std::collections::BTreeMap::new(),
            counting: PwmCounting::default(),
            dead_time: 0,
            channels: std::collections::BTreeMap::new(),
            breaks: std::collections::BTreeMap::new(),
            auto_output_enable: false,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }

    /// This channel's duty cycle, 0 % when the user has not set one.
    /// Duty of `channel` in hundredths of a percent; 0 for a channel nobody
    /// has touched, which is a pin held low — the safe state for a driver stage.
    pub fn duty_x100_of(&self, channel: u8) -> u16 {
        self.duty_x100.get(&channel).copied().unwrap_or(0)
    }

    /// The same duty as a percentage, for display and for the slider.
    pub fn duty_percent_of(&self, channel: u8) -> f32 {
        self.duty_x100_of(channel) as f32 / 100.0
    }

    /// Set `channel`'s duty, clamped to the full scale. The single door into
    /// the map, so `set_duty_cycle_fraction`'s `num <= denom` cannot be broken
    /// by a hand-edited config.
    pub fn set_duty_x100(&mut self, channel: u8, x100: u16) {
        self.duty_x100.insert(channel, x100.min(10_000));
    }

    /// The output shape of `channel`, all defaults when untouched.
    pub fn channel_of(&self, channel: u8) -> PwmChannelConfig {
        self.channels.get(&channel).copied().unwrap_or_default()
    }

    /// Record `channel`'s output shape. Only called when something actually
    /// changed, so merely opening the panel does not dirty the project with a
    /// row of defaults.
    pub fn set_channel(&mut self, channel: u8, shape: PwmChannelConfig) {
        self.channels.insert(channel, shape);
    }

    /// The settings of break input `index` (1 = BKIN, 2 = BKIN2), all defaults
    /// when the pad is wired but nothing was chosen.
    pub fn break_of(&self, index: u8) -> BreakInputConfig {
        self.breaks.get(&index).copied().unwrap_or_default()
    }

    /// Record break input `index`'s settings, with the filter clamped to a code
    /// [`BREAK_FILTERS`] actually has.
    pub fn set_break(&mut self, index: u8, mut cfg: BreakInputConfig) {
        cfg.filter = cfg.filter.min(BREAK_FILTERS.len() as u8 - 1);
        self.breaks.insert(index, cfg);
    }

    /// Fold a pre-hundredths whole-percent map into [`Self::duty_x100`].
    ///
    /// Idempotent, and it never overwrites a value the new field already
    /// carries: a config written by this version has an empty legacy map, so
    /// this is a no-op there.
    pub fn migrate_duty(&mut self) {
        for (ch, pct) in std::mem::take(&mut self.duty) {
            if !self.duty_x100.contains_key(&ch) {
                self.set_duty_x100(ch, pct as u16 * 100);
            }
        }
    }
}

/// How the timer counts, CubeMX's "Counter Mode" and embassy's `CountingMode`.
///
/// One setting for the whole timer, because there is one counter. Center-
/// aligned is what motor drive wants — the pulse is centred in the period, so
/// the harmonics of several channels do not line up — and the three centred
/// variants differ only in when the compare interrupt fires, which is why they
/// carry the interrupt in their name rather than in a separate field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PwmCounting {
    #[default]
    EdgeUp,
    EdgeDown,
    CenterUpInterrupts,
    CenterDownInterrupts,
    CenterBothInterrupts,
}

impl PwmCounting {
    pub const ALL: [Self; 5] = [
        Self::EdgeUp,
        Self::EdgeDown,
        Self::CenterUpInterrupts,
        Self::CenterDownInterrupts,
        Self::CenterBothInterrupts,
    ];

    /// The `embassy_stm32::timer::low_level::CountingMode` variant.
    pub fn embassy(self) -> &'static str {
        match self {
            Self::EdgeUp => "EdgeAlignedUp",
            Self::EdgeDown => "EdgeAlignedDown",
            Self::CenterUpInterrupts => "CenterAlignedUpInterrupts",
            Self::CenterDownInterrupts => "CenterAlignedDownInterrupts",
            Self::CenterBothInterrupts => "CenterAlignedBothInterrupts",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::EdgeUp => "Edge, up",
            Self::EdgeDown => "Edge, down",
            Self::CenterUpInterrupts => "Center, IRQ up",
            Self::CenterDownInterrupts => "Center, IRQ down",
            Self::CenterBothInterrupts => "Center, IRQ both",
        }
    }
}

/// How the channel's pad drives the line — `embassy_stm32::gpio::OutputType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PwmOutput {
    #[default]
    PushPull,
    OpenDrain,
}

impl PwmOutput {
    pub const ALL: [Self; 2] = [Self::PushPull, Self::OpenDrain];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::PushPull => "PushPull",
            Self::OpenDrain => "OpenDrain",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PushPull => "Push-pull",
            Self::OpenDrain => "Open-drain",
        }
    }
}

/// Which level counts as "on" — `OutputPolarity`. The one to flip for a driver
/// stage that sinks current, where 100 % duty has to hold the pin LOW.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PwmPolarity {
    #[default]
    ActiveHigh,
    ActiveLow,
}

impl PwmPolarity {
    pub const ALL: [Self; 2] = [Self::ActiveHigh, Self::ActiveLow];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::ActiveHigh => "ActiveHigh",
            Self::ActiveLow => "ActiveLow",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ActiveHigh => "Active high",
            Self::ActiveLow => "Active low",
        }
    }
}

/// PWM mode 1 or 2 — `OutputCompareMode`. Mode 2 is mode 1 with the comparison
/// reversed, which is a second way to reach the inversion [`PwmPolarity`] also
/// offers; CubeMX exposes both, and so does this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PwmMode {
    #[default]
    Mode1,
    Mode2,
}

impl PwmMode {
    pub const ALL: [Self; 2] = [Self::Mode1, Self::Mode2];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::Mode1 => "PwmMode1",
            Self::Mode2 => "PwmMode2",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mode1 => "PWM mode 1",
            Self::Mode2 => "PWM mode 2",
        }
    }
}

/// Which level on a break pad means "fault" — `BreakInputPolarity`.
///
/// Active low is the default because that is what a fault line usually is: an
/// open-drain output that is released when everything is fine and pulled down
/// by whatever went wrong, so a broken wire also reads as a fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BreakPolarity {
    #[default]
    ActiveLow,
    ActiveHigh,
}

impl BreakPolarity {
    pub const ALL: [Self; 2] = [Self::ActiveLow, Self::ActiveHigh];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::ActiveLow => "ACTIVE_LOW",
            Self::ActiveHigh => "ACTIVE_HIGH",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ActiveLow => "Active low",
            Self::ActiveHigh => "Active high",
        }
    }
}

/// The sixteen digital-filter codes of a break input, as `(embassy constant,
/// label)`.
///
/// The filter is "how many consecutive samples must agree before the fault is
/// believed", and the sampling clock is either the timer clock or the slower
/// dead-time clock. Kept as one table so the picker and the generated constant
/// can never name different things; the index IS the register value.
pub const BREAK_FILTERS: [(&str, &str); 16] = [
    ("NO_FILTER", "No filter"),
    ("FCK_INT_N2", "fCK_INT, N=2"),
    ("FCK_INT_N4", "fCK_INT, N=4"),
    ("FCK_INT_N8", "fCK_INT, N=8"),
    ("FDTS_DIV2_N6", "fDTS/2, N=6"),
    ("FDTS_DIV2_N8", "fDTS/2, N=8"),
    ("FDTS_DIV4_N6", "fDTS/4, N=6"),
    ("FDTS_DIV4_N8", "fDTS/4, N=8"),
    ("FDTS_DIV8_N6", "fDTS/8, N=6"),
    ("FDTS_DIV8_N8", "fDTS/8, N=8"),
    ("FDTS_DIV16_N5", "fDTS/16, N=5"),
    ("FDTS_DIV16_N6", "fDTS/16, N=6"),
    ("FDTS_DIV16_N8", "fDTS/16, N=8"),
    ("FDTS_DIV32_N5", "fDTS/32, N=5"),
    ("FDTS_DIV32_N6", "fDTS/32, N=6"),
    ("FDTS_DIV32_N8", "fDTS/32, N=8"),
];

/// One break input's settings. Per input, because BKIN and BKIN2 have their own
/// polarity and filter bits; the auto-restart is shared and lives on the module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BreakInputConfig {
    #[serde(default)]
    pub polarity: BreakPolarity,
    /// Index into [`BREAK_FILTERS`], which is also the register value.
    #[serde(default)]
    pub filter: u8,
}

impl BreakInputConfig {
    /// The embassy `FilterValue` constant this filter names, clamped so a
    /// hand-edited config cannot produce an identifier that does not exist.
    pub fn filter_embassy(&self) -> &'static str {
        BREAK_FILTERS[(self.filter as usize).min(BREAK_FILTERS.len() - 1)].0
    }

    pub fn filter_label(&self) -> &'static str {
        BREAK_FILTERS[(self.filter as usize).min(BREAK_FILTERS.len() - 1)].1
    }
}

/// The output shape of ONE channel. All three fields default to the timer's
/// reset state, so a channel nobody touched generates no extra line at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PwmChannelConfig {
    #[serde(default)]
    pub output: PwmOutput,
    #[serde(default)]
    pub polarity: PwmPolarity,
    #[serde(default)]
    pub mode: PwmMode,
}

/// Which way the audio flows. embassy has a constructor per direction, and the
/// pads differ: a receiver's SD is an input.
///
/// Full duplex is missing on purpose — embassy gates `new_full_duplex` on
/// `spi_v4`/`spi_v5`, and the IDE has no SPI IP version to gate on the way it
/// has `usart_ip` for the USART line extras.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum I2sDirection {
    #[default]
    Transmit,
    Receive,
}

impl I2sDirection {
    pub const ALL: [Self; 2] = [Self::Transmit, Self::Receive];

    pub fn label(self) -> &'static str {
        match self {
            Self::Transmit => "Transmit",
            Self::Receive => "Receive",
        }
    }

    /// `true` when the data flows out of the MCU.
    pub fn is_tx(self) -> bool {
        matches!(self, Self::Transmit)
    }
}

/// Who drives the clocks: this chip, or the device on the other end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum I2sMode {
    #[default]
    Master,
    Slave,
}

impl I2sMode {
    /// Whether this family's HAL can be the follower as well as the leader.
    ///
    /// `esp_hal::i2s::master::I2s::new` calls `set_master()` on the way in and
    /// offers no other entry point, so an ESP I2S drives the clocks. The slave
    /// side is a different driver esp-hal does not expose.
    pub fn options(family: &str) -> &'static [Self] {
        if crate::panels::mcu_module::codegen::family::is_esp(family) {
            &[Self::Master]
        } else {
            &Self::ALL
        }
    }

    pub const ALL: [Self; 2] = [Self::Master, Self::Slave];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::Master => "Master",
            Self::Slave => "Slave",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Master => "Master (we clock)",
            Self::Slave => "Slave (they clock)",
        }
    }
}

/// The frame convention on the wire — where the data sits relative to WS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum I2sStandard {
    #[default]
    Philips,
    MsbFirst,
    LsbFirst,
    PcmLongSync,
    PcmShortSync,
}

impl I2sStandard {
    /// The standards this family's HAL can actually build.
    ///
    /// esp-hal has four `Config` constructors — Philips, MSB-first, and the two
    /// PCM sync widths — and no LSB-first one at all. embassy builds all five.
    /// Offering the fifth on an ESP would be a setting the generator has to
    /// quietly replace, which is the kind of lie this table exists to prevent.
    pub fn options(family: &str) -> &'static [Self] {
        if crate::panels::mcu_module::codegen::family::is_esp(family) {
            &[
                Self::Philips,
                Self::MsbFirst,
                Self::PcmLongSync,
                Self::PcmShortSync,
            ]
        } else {
            &Self::ALL
        }
    }

    pub const ALL: [Self; 5] = [
        Self::Philips,
        Self::MsbFirst,
        Self::LsbFirst,
        Self::PcmLongSync,
        Self::PcmShortSync,
    ];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::Philips => "Philips",
            Self::MsbFirst => "MsbFirst",
            Self::LsbFirst => "LsbFirst",
            Self::PcmLongSync => "PcmLongSync",
            Self::PcmShortSync => "PcmShortSync",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Philips => "Philips (I2S)",
            Self::MsbFirst => "MSB first (left justified)",
            Self::LsbFirst => "LSB first (right justified)",
            Self::PcmLongSync => "PCM, long sync",
            Self::PcmShortSync => "PCM, short sync",
        }
    }
}

/// How many bits of data ride in how wide a channel slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum I2sFormat {
    #[default]
    Data16Channel16,
    Data16Channel32,
    Data24Channel32,
    Data32Channel32,
}

impl I2sFormat {
    /// The data widths this family's HAL can actually build.
    ///
    /// The two lists overlap in two places. esp-hal names its widths
    /// `Data{8,16,32}Channel{8,16,24,32}` and embassy names the STM32's four;
    /// 16-in-16 and 32-in-32 are in both, and 16-in-32 and 24-in-32 are not
    /// esp-hal shapes at all.
    pub fn options(family: &str) -> &'static [Self] {
        if crate::panels::mcu_module::codegen::family::is_esp(family) {
            &[Self::Data16Channel16, Self::Data32Channel32]
        } else {
            &Self::ALL
        }
    }

    pub const ALL: [Self; 4] = [
        Self::Data16Channel16,
        Self::Data16Channel32,
        Self::Data24Channel32,
        Self::Data32Channel32,
    ];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::Data16Channel16 => "Data16Channel16",
            Self::Data16Channel32 => "Data16Channel32",
            Self::Data24Channel32 => "Data24Channel32",
            Self::Data32Channel32 => "Data32Channel32",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Data16Channel16 => "16 bit in 16",
            Self::Data16Channel32 => "16 bit in 32",
            Self::Data24Channel32 => "24 bit in 32",
            Self::Data32Channel32 => "32 bit in 32",
        }
    }

    /// The Rust word the ring buffer holds — always `u16`.
    ///
    /// Not the frame width: embassy's `spi::Word` is implemented for `u8` and
    /// `u16` (plus the odd bit widths), never `u32`, because the SPI data
    /// register these blocks share is 16 bits wide. A 24- or 32-bit frame
    /// therefore travels as TWO halves, which is also how the hardware moves it.
    pub fn word(self) -> &'static str {
        "u16"
    }
}

/// Which level the bit clock idles at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum I2sClockPolarity {
    #[default]
    IdleLow,
    IdleHigh,
}

impl I2sClockPolarity {
    pub const ALL: [Self; 2] = [Self::IdleLow, Self::IdleHigh];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::IdleLow => "IdleLow",
            Self::IdleHigh => "IdleHigh",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::IdleLow => "Idle low",
            Self::IdleHigh => "Idle high",
        }
    }
}

/// How the HSPI talks to the device.
///
/// Only two, and that is the driver's whole surface: embassy has
/// `new_blocking_singlespi` and `new_blocking_octospi` and nothing between
/// them, even though the silicon carries sixteen data pads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HspiMode {
    /// Plain SPI: IO0 out, IO1 in.
    Single,
    /// Eight lines — and the strobe, which this call REQUIRES.
    #[default]
    Octal,
}

impl HspiMode {
    pub const ALL: [Self; 2] = [Self::Single, Self::Octal];

    pub fn lanes(self) -> u8 {
        match self {
            Self::Single => 2,
            Self::Octal => 8,
        }
    }

    /// The embassy constructor stem, without the `new_blocking_` prefix.
    pub fn embassy(self) -> &'static str {
        match self {
            Self::Single => "singlespi",
            Self::Octal => "octospi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "Single (1 line)",
            Self::Octal => "Octal (8 lines + DQS0)",
        }
    }
}

/// HSPI controller settings + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HspiModuleConfig {
    pub instance: u8,
    #[serde(default)]
    pub mode: HspiMode,
    /// The HSPI names the device families exactly as the OCTOSPI does, so the
    /// two share one enum.
    #[serde(default)]
    pub memory_type: OspiMemoryType,
    /// Index into [`QSPI_MEMORY_SIZES`].
    #[serde(default)]
    pub device_size: u8,
    pub prescaler: u8,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_hspiN` handle.
    #[serde(default)]
    pub custom_label: String,
}

impl HspiModuleConfig {
    /// Defaults: octal, a standard 16 MiB device, kernel clock / 2.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            mode: HspiMode::default(),
            memory_type: OspiMemoryType::default(),
            device_size: 14,
            prescaler: 1,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }

    pub fn size_embassy(&self) -> &'static str {
        QSPI_MEMORY_SIZES[(self.device_size as usize).min(QSPI_MEMORY_SIZES.len() - 1)]
    }

    pub fn size_label(&self) -> &'static str {
        self.size_embassy().trim_start_matches('_')
    }
}

/// How the XSPI talks to the device — one embassy constructor each.
///
/// The same ambiguity as the OCTOSPI's, one step wider: single and dual share
/// two pads, and the eight-line modes share eight. The strobes are derived from
/// the wiring, the mode is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum XspiMode {
    Single,
    Dual,
    #[default]
    Quad,
    DualQuad,
    /// Eight lines.
    Octal,
    /// Sixteen lines.
    Hexa,
}

impl XspiMode {
    pub const ALL: [Self; 6] = [
        Self::Single,
        Self::Dual,
        Self::Quad,
        Self::DualQuad,
        Self::Octal,
        Self::Hexa,
    ];

    pub fn lanes(self) -> u8 {
        match self {
            Self::Single | Self::Dual => 2,
            Self::Quad => 4,
            Self::DualQuad | Self::Octal => 8,
            Self::Hexa => 16,
        }
    }

    /// The embassy constructor stem, without the `new_blocking_` prefix or any
    /// strobe suffix.
    pub fn embassy(self) -> &'static str {
        match self {
            Self::Single => "singlespi",
            Self::Dual => "dualspi",
            Self::Quad => "quadspi",
            Self::DualQuad => "dualquadspi",
            Self::Octal => "xspi",
            Self::Hexa => "xspi_hexa",
        }
    }

    /// Whether this mode has a strobe variant at all — only the wide ones do.
    pub fn takes_dqs(self) -> bool {
        matches!(self, Self::Octal | Self::Hexa)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "Single (1 line)",
            Self::Dual => "Dual (2 lines)",
            Self::Quad => "Quad (4 lines)",
            Self::DualQuad => "Dual-quad (2 chips, 8 lines)",
            Self::Octal => "Octal (8 lines)",
            Self::Hexa => "Hexadeca (16 lines)",
        }
    }
}

/// What kind of device the XSPI is talking to. Two more than the OCTOSPI's
/// list, so it cannot share that enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum XspiMemoryType {
    Micron,
    Macronix,
    #[default]
    Standard,
    MacronixRam,
    HyperBusMemory,
    HyperBusRegister,
    ApMemory16Bits,
    ApMemory,
}

impl XspiMemoryType {
    pub const ALL: [Self; 8] = [
        Self::Micron,
        Self::Macronix,
        Self::Standard,
        Self::MacronixRam,
        Self::HyperBusMemory,
        Self::HyperBusRegister,
        Self::ApMemory16Bits,
        Self::ApMemory,
    ];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::Micron => "Micron",
            Self::Macronix => "Macronix",
            Self::Standard => "Standard",
            Self::MacronixRam => "MacronixRam",
            Self::HyperBusMemory => "HyperBusMemory",
            Self::HyperBusRegister => "HyperBusRegister",
            Self::ApMemory16Bits => "APMemory16Bits",
            Self::ApMemory => "APMemory",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Micron => "Micron",
            Self::Macronix => "Macronix",
            Self::Standard => "Standard",
            Self::MacronixRam => "Macronix RAM",
            Self::HyperBusMemory => "HyperBus memory",
            Self::HyperBusRegister => "HyperBus register",
            Self::ApMemory16Bits => "AP Memory, 16 bit",
            Self::ApMemory => "AP Memory",
        }
    }
}

/// XSPI port settings + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct XspiModuleConfig {
    /// The IO-manager PORT, mapped 1:1 onto the controller.
    pub instance: u8,
    #[serde(default)]
    pub mode: XspiMode,
    #[serde(default)]
    pub memory_type: XspiMemoryType,
    /// Index into [`QSPI_MEMORY_SIZES`] — the XSPI names its sizes the same way.
    #[serde(default)]
    pub device_size: u8,
    pub prescaler: u8,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_xspiN` handle.
    #[serde(default)]
    pub custom_label: String,
}

impl XspiModuleConfig {
    /// Defaults: quad, a standard 16 MiB device, kernel clock / 2.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            mode: XspiMode::default(),
            memory_type: XspiMemoryType::default(),
            device_size: 14,
            prescaler: 1,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }

    pub fn size_embassy(&self) -> &'static str {
        QSPI_MEMORY_SIZES[(self.device_size as usize).min(QSPI_MEMORY_SIZES.len() - 1)]
    }

    pub fn size_label(&self) -> &'static str {
        self.size_embassy().trim_start_matches('_')
    }
}

/// How the OCTOSPI talks to the device — one embassy constructor each.
///
/// Two of these cannot be told from the wiring: single and dual use the SAME
/// two pads, and octal and dual-quad the same eight. That is why this is a
/// setting and the width is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OspiMode {
    /// Plain SPI over the octo controller: IO0 out, IO1 in.
    Single,
    /// Two lines, both driven.
    Dual,
    #[default]
    Quad,
    /// Two quad devices side by side on eight lines.
    DualQuad,
    Octal,
}

impl OspiMode {
    pub const ALL: [Self; 5] = [
        Self::Single,
        Self::Dual,
        Self::Quad,
        Self::DualQuad,
        Self::Octal,
    ];

    /// How many data lines this mode drives.
    pub fn lanes(self) -> u8 {
        match self {
            Self::Single | Self::Dual => 2,
            Self::Quad => 4,
            Self::DualQuad | Self::Octal => 8,
        }
    }

    /// The embassy constructor stem, without the `new_blocking_` prefix or the
    /// `_with_dqs` suffix.
    pub fn embassy(self) -> &'static str {
        match self {
            Self::Single => "singlespi",
            Self::Dual => "dualspi",
            Self::Quad => "quadspi",
            Self::DualQuad => "dualquadspi",
            Self::Octal => "octospi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "Single (1 line)",
            Self::Dual => "Dual (2 lines)",
            Self::Quad => "Quad (4 lines)",
            Self::DualQuad => "Dual-quad (2 chips, 8 lines)",
            Self::Octal => "Octal (8 lines)",
        }
    }
}

/// What kind of device is on the other end — it changes the command framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OspiMemoryType {
    Micron,
    Macronix,
    #[default]
    Standard,
    MacronixRam,
    HyperBusMemory,
    HyperBusRegister,
}

impl OspiMemoryType {
    pub const ALL: [Self; 6] = [
        Self::Micron,
        Self::Macronix,
        Self::Standard,
        Self::MacronixRam,
        Self::HyperBusMemory,
        Self::HyperBusRegister,
    ];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::Micron => "Micron",
            Self::Macronix => "Macronix",
            Self::Standard => "Standard",
            Self::MacronixRam => "MacronixRam",
            Self::HyperBusMemory => "HyperBusMemory",
            Self::HyperBusRegister => "HyperBusRegister",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Micron => "Micron",
            Self::Macronix => "Macronix",
            Self::Standard => "Standard",
            Self::MacronixRam => "Macronix RAM",
            Self::HyperBusMemory => "HyperBus memory",
            Self::HyperBusRegister => "HyperBus register",
        }
    }
}

/// OCTOSPI port settings + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OspiModuleConfig {
    /// The IO-manager PORT, which the IDE maps 1:1 onto the controller.
    pub instance: u8,
    #[serde(default)]
    pub mode: OspiMode,
    #[serde(default)]
    pub memory_type: OspiMemoryType,
    /// Index into [`QSPI_MEMORY_SIZES`] — the OCTOSPI names its sizes exactly
    /// as the QUADSPI does, so the two share one table.
    #[serde(default)]
    pub device_size: u8,
    /// Clock divider: the bus runs at kernel clock / (prescaler + 1).
    pub prescaler: u8,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_ospiN` handle.
    #[serde(default)]
    pub custom_label: String,
}

impl OspiModuleConfig {
    /// Defaults: quad, a standard 16 MiB flash, kernel clock / 2.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            mode: OspiMode::default(),
            memory_type: OspiMemoryType::default(),
            device_size: 14,
            prescaler: 1,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }

    pub fn size_embassy(&self) -> &'static str {
        QSPI_MEMORY_SIZES[(self.device_size as usize).min(QSPI_MEMORY_SIZES.len() - 1)]
    }

    pub fn size_label(&self) -> &'static str {
        self.size_embassy().trim_start_matches('_')
    }
}

/// The flash sizes embassy names, in order — the index IS the setting.
///
/// A table rather than an enum with twenty-three variants: the picker and the
/// generated constant read the same list, so they cannot name different things.
/// The label is the constant without embassy's leading underscore.
pub const QSPI_MEMORY_SIZES: [&str; 23] = [
    "_1KiB", "_2KiB", "_4KiB", "_8KiB", "_16KiB", "_32KiB", "_64KiB", "_128KiB", "_256KiB",
    "_512KiB", "_1MiB", "_2MiB", "_4MiB", "_8MiB", "_16MiB", "_32MiB", "_64MiB", "_128MiB",
    "_256MiB", "_512MiB", "_1GiB", "_2GiB", "_4GiB",
];

/// How many address bytes the flash chip expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum QspiAddressSize {
    Bits8,
    Bits16,
    #[default]
    Bits24,
    Bits32,
}

impl QspiAddressSize {
    pub const ALL: [Self; 4] = [Self::Bits8, Self::Bits16, Self::Bits24, Self::Bits32];

    /// embassy's spelling, which is NOT uniform — `_8Bit` but `_24bit`.
    pub fn embassy(self) -> &'static str {
        match self {
            Self::Bits8 => "_8Bit",
            Self::Bits16 => "_16Bit",
            Self::Bits24 => "_24bit",
            Self::Bits32 => "_32bit",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bits8 => "8 bit",
            Self::Bits16 => "16 bit",
            Self::Bits24 => "24 bit",
            Self::Bits32 => "32 bit",
        }
    }
}

/// External-flash controller settings + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QspiModuleConfig {
    pub instance: u8,
    /// Index into [`QSPI_MEMORY_SIZES`] — how big the flash chip is.
    #[serde(default)]
    pub memory_size: u8,
    #[serde(default)]
    pub address_size: QspiAddressSize,
    /// Clock divider, 0..=255: the bus runs at kernel clock / (prescaler + 1).
    pub prescaler: u8,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_qspi` handle.
    #[serde(default)]
    pub custom_label: String,
}

impl QspiModuleConfig {
    /// Defaults: 16 MiB, 24-bit addressing, kernel clock / 2 — a plain 128 Mbit
    /// NOR flash, which is what most boards carry.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            memory_size: 14,
            address_size: QspiAddressSize::default(),
            prescaler: 1,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }

    /// The embassy `MemorySize` constant this size names, clamped so a
    /// hand-edited config cannot produce an identifier that does not exist.
    pub fn memory_size_embassy(&self) -> &'static str {
        QSPI_MEMORY_SIZES[(self.memory_size as usize).min(QSPI_MEMORY_SIZES.len() - 1)]
    }

    pub fn memory_size_label(&self) -> &'static str {
        self.memory_size_embassy().trim_start_matches('_')
    }
}

/// SD card / eMMC controller settings + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdmmcModuleConfig {
    /// The controller instance. 0 is the un-numbered `SDIO` of F1/F2/F4/L1.
    pub instance: u8,
    /// `Config::data_transfer_timeout`, in card bus clock periods. embassy's
    /// default is 5 000 000, which is a few seconds on a slow card.
    pub data_timeout: u32,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_sdN` handle.
    #[serde(default)]
    pub custom_label: String,
    /// DMA channel chosen by hand; empty = let the IDE allocate. Only the
    /// older controller takes one at all — the newer has its own inside.
    #[serde(default)]
    pub dma_tx: String,
    #[serde(default)]
    pub dma_rx: String,
}

impl SdmmcModuleConfig {
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            data_timeout: 5_000_000,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
            dma_tx: String::new(),
            dma_rx: String::new(),
        }
    }
}

/// Who drives the SAI clocks — this sub-block, or the device on the other end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SaiMode {
    #[default]
    Master,
    Slave,
}

impl SaiMode {
    pub const ALL: [Self; 2] = [Self::Master, Self::Slave];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::Master => "Master",
            Self::Slave => "Slave",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Master => "Master (we clock)",
            Self::Slave => "Slave (they clock)",
        }
    }
}

/// Which way one sub-block's data flows. The two sub-blocks are independent, so
/// a codec is usually one of each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SaiTxRx {
    #[default]
    Transmitter,
    Receiver,
}

impl SaiTxRx {
    pub const ALL: [Self; 2] = [Self::Transmitter, Self::Receiver];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::Transmitter => "Transmitter",
            Self::Receiver => "Receiver",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Transmitter => "Transmit",
            Self::Receiver => "Receive",
        }
    }
}

/// How many bits of audio ride in one slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SaiDataSize {
    Data8,
    Data10,
    #[default]
    Data16,
    Data20,
    Data24,
    Data32,
}

impl SaiDataSize {
    pub const ALL: [Self; 6] = [
        Self::Data8,
        Self::Data10,
        Self::Data16,
        Self::Data20,
        Self::Data24,
        Self::Data32,
    ];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::Data8 => "Data8",
            Self::Data10 => "Data10",
            Self::Data16 => "Data16",
            Self::Data20 => "Data20",
            Self::Data24 => "Data24",
            Self::Data32 => "Data32",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Data8 => "8 bit",
            Self::Data10 => "10 bit",
            Self::Data16 => "16 bit",
            Self::Data20 => "20 bit",
            Self::Data24 => "24 bit",
            Self::Data32 => "32 bit",
        }
    }

    /// The Rust word the DMA ring buffer holds.
    ///
    /// Unlike I2S, this one really does widen: the SAI ring buffer is a DMA
    /// buffer, and `dma::word::Word` covers `u8`, `u16` and `u32`.
    pub fn word(self) -> &'static str {
        match self {
            Self::Data8 => "u8",
            Self::Data10 | Self::Data16 => "u16",
            _ => "u32",
        }
    }
}

/// Stereo (two slots) or mono (one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SaiStereoMono {
    #[default]
    Stereo,
    Mono,
}

impl SaiStereoMono {
    pub const ALL: [Self; 2] = [Self::Stereo, Self::Mono];

    pub fn embassy(self) -> &'static str {
        match self {
            Self::Stereo => "Stereo",
            Self::Mono => "Mono",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stereo => "Stereo",
            Self::Mono => "Mono",
        }
    }
}

/// One SAI sub-block's settings. A and B carry these independently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaiBlockConfig {
    #[serde(default)]
    pub mode: SaiMode,
    #[serde(default)]
    pub tx_rx: SaiTxRx,
    #[serde(default)]
    pub data_size: SaiDataSize,
    #[serde(default)]
    pub stereo_mono: SaiStereoMono,
    /// Slots per frame, 1..=16.
    pub slot_count: u8,
    /// Frame length in BITS. embassy's default is 32 for a 16-bit stereo frame.
    pub frame_length: u16,
    /// Ring-buffer length in samples, in `data_size`-wide words.
    pub buffer_len: u16,
}

impl Default for SaiBlockConfig {
    fn default() -> Self {
        Self {
            mode: SaiMode::default(),
            tx_rx: SaiTxRx::default(),
            data_size: SaiDataSize::default(),
            stereo_mono: SaiStereoMono::default(),
            slot_count: 2,
            frame_length: 32,
            buffer_len: 256,
        }
    }
}

/// SAI unit settings + data model. The sub-blocks live inside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaiModuleConfig {
    pub instance: u8,
    /// Sub-block (1 = A, 2 = B) → its settings. Absent = every default.
    #[serde(default)]
    pub blocks: std::collections::BTreeMap<u8, SaiBlockConfig>,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_saiN` handles.
    #[serde(default)]
    pub custom_label: String,
    /// DMA channel per sub-block, chosen by hand; empty = let the IDE allocate.
    #[serde(default)]
    pub dma_a: String,
    #[serde(default)]
    pub dma_b: String,
}

impl SaiModuleConfig {
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            blocks: std::collections::BTreeMap::new(),
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
            dma_a: String::new(),
            dma_b: String::new(),
        }
    }

    /// One sub-block's settings, all defaults when untouched.
    pub fn block_of(&self, block: u8) -> SaiBlockConfig {
        self.blocks.get(&block).copied().unwrap_or_default()
    }

    /// Record a sub-block's settings, with the slot count clamped to the four
    /// bits the register has.
    pub fn set_block(&mut self, block: u8, mut cfg: SaiBlockConfig) {
        cfg.slot_count = cfg.slot_count.clamp(1, 16);
        self.blocks.insert(block, cfg);
    }
}

/// Analog output settings + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DacModuleConfig {
    pub instance: u8,
    /// Channel (1 or 2) → the value the pin holds once `init` returns, as a
    /// 12-bit right-aligned number.
    ///
    /// A DAC that comes up at a defined level matters: mid-scale is the resting
    /// point of a bipolar output, and zero is silence for an audio one. There is
    /// no "unset" — the pad drives something the moment the channel is enabled,
    /// so the only honest choice is to say what.
    #[serde(default)]
    pub values: std::collections::BTreeMap<u8, u16>,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_dacN` handle.
    #[serde(default)]
    pub custom_label: String,
}

impl DacModuleConfig {
    /// Defaults: every channel at zero.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            values: std::collections::BTreeMap::new(),
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }

    /// The value channel `channel` starts at, 0 when untouched.
    pub fn value_of(&self, channel: u8) -> u16 {
        self.values.get(&channel).copied().unwrap_or(0)
    }

    /// Record a channel's start value, clamped to the 12 bits the hardware has.
    pub fn set_value(&mut self, channel: u8, value: u16) {
        self.values.insert(channel, value.min(4095));
    }
}

/// I2S audio device settings + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct I2sModuleConfig {
    /// The SPI block this I2S runs on — I2S2 is SPI2.
    pub instance: u8,
    /// Sample rate in Hz: 44_100, 48_000, …
    pub sample_rate_hz: u32,
    #[serde(default)]
    pub direction: I2sDirection,
    #[serde(default)]
    pub mode: I2sMode,
    #[serde(default)]
    pub standard: I2sStandard,
    #[serde(default)]
    pub format: I2sFormat,
    #[serde(default)]
    pub clock_polarity: I2sClockPolarity,
    /// Ring-buffer length in SAMPLES. embassy drives I2S from a ring buffer the
    /// DMA owns for the program's lifetime; too short and the audio breaks up
    /// under any scheduling hiccup.
    pub buffer_len: u16,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_i2sN` handle.
    #[serde(default)]
    pub custom_label: String,
    /// DMA channel chosen by hand, empty = let the IDE allocate. Only one is
    /// used: the direction decides whether it is the TX or the RX channel.
    #[serde(default)]
    pub dma_tx: String,
    #[serde(default)]
    pub dma_rx: String,
}

/// Which way an RMT channel moves its pulses.
///
/// On every part after the original ESP32 this is fixed in silicon — the low
/// channels transmit, the high ones receive — so the module cannot choose. It
/// is still stored, because the ESP32 and the S2 let every channel do either,
/// and because the direction decides the whole shape of the generated file:
/// `configure_tx` against `configure_rx`, an output pad against an input one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RmtDirection {
    #[default]
    Transmit,
    Receive,
}

impl RmtDirection {
    pub const ALL: [Self; 2] = [Self::Transmit, Self::Receive];

    /// The directions CHANNEL `n` can actually take on this chip.
    ///
    /// Every part after the original ESP32 hardwires it: the low channels
    /// transmit and the high ones receive. The ESP32 and the S2 let each
    /// channel do either, so both are offered there. A one-entry list is what
    /// the UI shows locked, with the reason.
    ///
    /// Split at the HALFWAY point rather than by a per-chip table, because that
    /// is the rule esp-hal's own documentation states for all four of the parts
    /// that have it: "`Channel<0>` and `Channel<1>` hardcoded for transmitting
    /// and `Channel<2>` and `Channel<3>` for receiving" on the C3/C5/C6/H2, and
    /// 0-3 against 4-7 on the S3.
    pub fn options(family: &str, channel: u8) -> &'static [Self] {
        if !crate::panels::mcu_module::codegen::family::is_esp(family) {
            return &Self::ALL;
        }
        match family {
            // Eight (four on the S2) channels, each either way round.
            "esp32" | "esp32s2" => &Self::ALL,
            "esp32s3" => {
                if channel < 4 {
                    &[Self::Transmit]
                } else {
                    &[Self::Receive]
                }
            }
            _ => {
                if channel < 2 {
                    &[Self::Transmit]
                } else {
                    &[Self::Receive]
                }
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Transmit => "Transmit",
            Self::Receive => "Receive",
        }
    }

    /// True when the pad is driven by the chip.
    pub fn is_tx(self) -> bool {
        matches!(self, Self::Transmit)
    }
}

/// One RMT channel: a pin, a tick rate, and optionally a carrier.
///
/// The instance is the CHANNEL. Every channel of the one RMT block is
/// independent — its own divider, its own carrier, its own pad — which is why
/// there is a module per channel rather than one module with a channel list
/// (the PWM module is the other way round precisely because a timer's channels
/// share its frequency).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RmtModuleConfig {
    /// The channel number — `rmt.channel0` and so on.
    pub instance: u8,
    #[serde(default)]
    pub direction: RmtDirection,
    /// Divides the RMT source clock to give the tick every duration is counted
    /// in. 1 is the fastest the channel can go; 255 the slowest.
    pub clk_divider: u8,
    /// Transmit only: the level the pad rests at between trains.
    #[serde(default)]
    pub idle_high: bool,
    /// Modulate the output onto a carrier — what an IR LED driver wants.
    #[serde(default)]
    pub carrier: bool,
    /// Carrier frequency in Hz. 38 kHz is the usual one for infrared.
    pub carrier_hz: u32,
    /// Receive only: how long the line must rest before the frame is over, in
    /// ticks. Too short and one train is read as several.
    pub idle_threshold: u16,
    /// What is on the other end — a `WS2812B` strip, a `TSOP38238` receiver.
    /// Carried like every other module's, so the notes read the same way.
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_rmtN` handle.
    #[serde(default)]
    pub custom_label: String,
}

impl RmtModuleConfig {
    /// Defaults: transmit, divider 1 (the finest resolution the channel has),
    /// no carrier, and an idle threshold long enough for a typical IR frame.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            direction: RmtDirection::default(),
            clk_divider: 1,
            idle_high: false,
            carrier: false,
            carrier_hz: 38_000,
            idle_threshold: 10_000,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }
}

impl I2sModuleConfig {
    /// Defaults: 48 kHz Philips, 16-in-16, master, transmitting, 256 samples.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            sample_rate_hz: 48_000,
            direction: I2sDirection::default(),
            mode: I2sMode::default(),
            standard: I2sStandard::default(),
            format: I2sFormat::default(),
            clock_polarity: I2sClockPolarity::default(),
            buffer_len: 256,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
            dma_tx: String::new(),
            dma_rx: String::new(),
        }
    }
}

/// CAN device settings + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanModuleConfig {
    pub instance: u8,
    /// Bus bit rate in bits/s (e.g. 125000 / 250000 / 500000 / 1000000).
    pub bitrate: u32,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated `_canN` handle (e.g. `_can1_obd`).
    #[serde(default)]
    pub custom_label: String,
}

impl CanModuleConfig {
    /// Defaults: 500 kbit/s.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            bitrate: 500_000,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }
}

/// USB full-speed device settings (CDC ACM serial) + data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbModuleConfig {
    /// Single USB FS instance on STM32F1 — always 1.
    pub instance: u8,
    /// USB Vendor ID reported to the host.
    pub vid: u16,
    /// USB Product ID reported to the host.
    pub pid: u16,
    /// Product string shown to the host.
    pub product: String,
    pub rx_model: String,
    pub tx_model: String,
    /// User label appended to the generated handles (e.g. `usb_dev_logger`).
    #[serde(default)]
    pub custom_label: String,
}

impl UsbModuleConfig {
    /// Defaults: the pid.codes test VID:PID (0x16c0:0x27dd), CDC serial.
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            vid: 0x16c0,
            pid: 0x27dd,
            product: "Serial port".to_owned(),
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }
}

/// A user-authored module: a name and an arbitrary list of MCU pins.
///
/// Nothing here is derived from the pin functions — the user adds pins by hand
/// (min 1, max = the chip's pin count) and each becomes a field of the generated
/// `configs/custom_<name>.rs` struct.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomModuleConfig {
    /// Kept only so `ModuleConfig::instance()` has an answer; custom modules are
    /// distinguished by their name, not by a peripheral instance.
    pub instance: u8,
    /// The MCU pin numbers wired to this module, in the order the user added
    /// them — that order is the order of the generated struct fields and of
    /// `new()`'s parameters. EDITED freely; nothing is generated from it until
    /// the user presses **Update** (see [`Self::applied_pins`]).
    #[serde(default)]
    pub pins: Vec<usize>,
    /// The pin list the generated code currently reflects. `pins` is the draft,
    /// this is what `custom_<name>.rs` and the `::new(…)` call were built from —
    /// so adding a pin doesn't silently rewrite code the user may be mid-edit
    /// in; **Update** commits `pins` here.
    #[serde(default)]
    pub applied_pins: Vec<usize>,
    /// Name of the generated struct. Empty = follow the module name; once the
    /// user types here it stays put even if the module is renamed.
    #[serde(default)]
    pub struct_name: String,
    /// Fingerprint of the pins the generated code was built from — each pin's
    /// number, FUNCTION and LABEL. The pin list alone isn't enough: renaming a
    /// pin or flipping it In→Out changes the generated field names, parameter
    /// order stays but the code differs, so Update must light up for those too.
    #[serde(default)]
    pub applied_sig: String,
    /// How many times **Update** has regenerated this module. The generated file
    /// is `custom_<name>.rs` at revision 0 and `custom_<name>_<n>.rs` after that,
    /// so every Update lands in a FRESH file (the previous ones stay on disk as
    /// history — only the current one is declared in `configs/mod.rs` and called
    /// from main.rs, so there are never duplicate structs in the build).
    #[serde(default)]
    pub revision: u32,
    pub rx_model: String,
    pub tx_model: String,
    /// The module's name — also the generated file (`custom_<label>.rs`) and
    /// struct name, so it must sanitize to a valid Rust identifier.
    #[serde(default)]
    pub custom_label: String,
}

impl CustomModuleConfig {
    pub fn new(instance: u8) -> Self {
        Self {
            instance,
            pins: Vec::new(),
            applied_pins: Vec::new(),
            struct_name: String::new(),
            applied_sig: String::new(),
            revision: 0,
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }

    /// Anything the generated code depends on changed since the last **Update**:
    /// the pin list, or one of those pins' function / label (`current_sig`, built
    /// by the caller which can see the pins). That is what enables the button.
    pub fn has_pending_pins(&self, current_sig: &str) -> bool {
        self.pins != self.applied_pins || self.applied_sig != current_sig
    }
}

/// Per-kind configuration payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleConfig {
    Usart(UsartModuleConfig),
    /// LPUART shares the USART's settings struct — every field (baud, parity,
    /// stop bits, buffered/DMA, DMA channels) means the same thing on it. The
    /// VARIANT is what keeps LPUART1 and USART1 from colliding on instance 1.
    Lpuart(UsartModuleConfig),
    Spi(SpiModuleConfig),
    I2c(I2cModuleConfig),
    I2s(I2sModuleConfig),
    Rmt(RmtModuleConfig),
    Dac(DacModuleConfig),
    Sai(SaiModuleConfig),
    Sdmmc(SdmmcModuleConfig),
    Qspi(QspiModuleConfig),
    Ospi(OspiModuleConfig),
    Xspi(XspiModuleConfig),
    Hspi(HspiModuleConfig),
    Timer(TimerModuleConfig),
    Can(CanModuleConfig),
    Usb(UsbModuleConfig),
    Custom(CustomModuleConfig),
}

impl ModuleConfig {
    /// The peripheral instance this module targets.
    pub fn instance(&self) -> u8 {
        match self {
            ModuleConfig::Usart(c) | ModuleConfig::Lpuart(c) => c.instance,
            ModuleConfig::Spi(c) => c.instance,
            ModuleConfig::I2c(c) => c.instance,
            ModuleConfig::I2s(c) => c.instance,
            ModuleConfig::Dac(c) => c.instance,
            ModuleConfig::Sai(c) => c.instance,
            ModuleConfig::Sdmmc(c) => c.instance,
            ModuleConfig::Qspi(c) => c.instance,
            ModuleConfig::Ospi(c) => c.instance,
            ModuleConfig::Xspi(c) => c.instance,
            ModuleConfig::Hspi(c) => c.instance,
            ModuleConfig::Rmt(c) => c.instance,
            ModuleConfig::Timer(c) => c.instance,
            ModuleConfig::Can(c) => c.instance,
            ModuleConfig::Usb(c) => c.instance,
            ModuleConfig::Custom(c) => c.instance,
        }
    }

    pub fn rx_model(&self) -> &str {
        match self {
            ModuleConfig::Usart(c) | ModuleConfig::Lpuart(c) => &c.rx_model,
            ModuleConfig::Spi(c) => &c.rx_model,
            ModuleConfig::I2c(c) => &c.rx_model,
            ModuleConfig::I2s(c) => &c.rx_model,
            ModuleConfig::Dac(c) => &c.rx_model,
            ModuleConfig::Sai(c) => &c.rx_model,
            ModuleConfig::Sdmmc(c) => &c.rx_model,
            ModuleConfig::Qspi(c) => &c.rx_model,
            ModuleConfig::Ospi(c) => &c.rx_model,
            ModuleConfig::Xspi(c) => &c.rx_model,
            ModuleConfig::Hspi(c) => &c.rx_model,
            ModuleConfig::Rmt(c) => &c.rx_model,
            ModuleConfig::Timer(c) => &c.rx_model,
            ModuleConfig::Can(c) => &c.rx_model,
            ModuleConfig::Usb(c) => &c.rx_model,
            ModuleConfig::Custom(c) => &c.rx_model,
        }
    }

    pub fn tx_model(&self) -> &str {
        match self {
            ModuleConfig::Usart(c) | ModuleConfig::Lpuart(c) => &c.tx_model,
            ModuleConfig::Spi(c) => &c.tx_model,
            ModuleConfig::I2c(c) => &c.tx_model,
            ModuleConfig::I2s(c) => &c.tx_model,
            ModuleConfig::Dac(c) => &c.tx_model,
            ModuleConfig::Sai(c) => &c.tx_model,
            ModuleConfig::Sdmmc(c) => &c.tx_model,
            ModuleConfig::Qspi(c) => &c.tx_model,
            ModuleConfig::Ospi(c) => &c.tx_model,
            ModuleConfig::Xspi(c) => &c.tx_model,
            ModuleConfig::Hspi(c) => &c.tx_model,
            ModuleConfig::Rmt(c) => &c.tx_model,
            ModuleConfig::Timer(c) => &c.tx_model,
            ModuleConfig::Can(c) => &c.tx_model,
            ModuleConfig::Usb(c) => &c.tx_model,
            ModuleConfig::Custom(c) => &c.tx_model,
        }
    }

    pub fn rx_model_mut(&mut self) -> &mut String {
        match self {
            ModuleConfig::Usart(c) | ModuleConfig::Lpuart(c) => &mut c.rx_model,
            ModuleConfig::Spi(c) => &mut c.rx_model,
            ModuleConfig::I2c(c) => &mut c.rx_model,
            ModuleConfig::I2s(c) => &mut c.rx_model,
            ModuleConfig::Dac(c) => &mut c.rx_model,
            ModuleConfig::Sai(c) => &mut c.rx_model,
            ModuleConfig::Sdmmc(c) => &mut c.rx_model,
            ModuleConfig::Qspi(c) => &mut c.rx_model,
            ModuleConfig::Ospi(c) => &mut c.rx_model,
            ModuleConfig::Xspi(c) => &mut c.rx_model,
            ModuleConfig::Hspi(c) => &mut c.rx_model,
            ModuleConfig::Rmt(c) => &mut c.rx_model,
            ModuleConfig::Timer(c) => &mut c.rx_model,
            ModuleConfig::Can(c) => &mut c.rx_model,
            ModuleConfig::Usb(c) => &mut c.rx_model,
            ModuleConfig::Custom(c) => &mut c.rx_model,
        }
    }

    pub fn tx_model_mut(&mut self) -> &mut String {
        match self {
            ModuleConfig::Usart(c) | ModuleConfig::Lpuart(c) => &mut c.tx_model,
            ModuleConfig::Spi(c) => &mut c.tx_model,
            ModuleConfig::I2c(c) => &mut c.tx_model,
            ModuleConfig::I2s(c) => &mut c.tx_model,
            ModuleConfig::Dac(c) => &mut c.tx_model,
            ModuleConfig::Sai(c) => &mut c.tx_model,
            ModuleConfig::Sdmmc(c) => &mut c.tx_model,
            ModuleConfig::Qspi(c) => &mut c.tx_model,
            ModuleConfig::Ospi(c) => &mut c.tx_model,
            ModuleConfig::Xspi(c) => &mut c.tx_model,
            ModuleConfig::Hspi(c) => &mut c.tx_model,
            ModuleConfig::Rmt(c) => &mut c.tx_model,
            ModuleConfig::Timer(c) => &mut c.tx_model,
            ModuleConfig::Can(c) => &mut c.tx_model,
            ModuleConfig::Usb(c) => &mut c.tx_model,
            ModuleConfig::Custom(c) => &mut c.tx_model,
        }
    }

    /// User label appended to the module's generated handle variable(s).
    pub fn custom_label(&self) -> &str {
        match self {
            ModuleConfig::Usart(c) | ModuleConfig::Lpuart(c) => &c.custom_label,
            ModuleConfig::Spi(c) => &c.custom_label,
            ModuleConfig::I2c(c) => &c.custom_label,
            ModuleConfig::I2s(c) => &c.custom_label,
            ModuleConfig::Dac(c) => &c.custom_label,
            ModuleConfig::Sai(c) => &c.custom_label,
            ModuleConfig::Sdmmc(c) => &c.custom_label,
            ModuleConfig::Qspi(c) => &c.custom_label,
            ModuleConfig::Ospi(c) => &c.custom_label,
            ModuleConfig::Xspi(c) => &c.custom_label,
            ModuleConfig::Hspi(c) => &c.custom_label,
            ModuleConfig::Rmt(c) => &c.custom_label,
            ModuleConfig::Timer(c) => &c.custom_label,
            ModuleConfig::Can(c) => &c.custom_label,
            ModuleConfig::Usb(c) => &c.custom_label,
            ModuleConfig::Custom(c) => &c.custom_label,
        }
    }

    pub fn custom_label_mut(&mut self) -> &mut String {
        match self {
            ModuleConfig::Usart(c) | ModuleConfig::Lpuart(c) => &mut c.custom_label,
            ModuleConfig::Spi(c) => &mut c.custom_label,
            ModuleConfig::I2c(c) => &mut c.custom_label,
            ModuleConfig::I2s(c) => &mut c.custom_label,
            ModuleConfig::Dac(c) => &mut c.custom_label,
            ModuleConfig::Sai(c) => &mut c.custom_label,
            ModuleConfig::Sdmmc(c) => &mut c.custom_label,
            ModuleConfig::Qspi(c) => &mut c.custom_label,
            ModuleConfig::Ospi(c) => &mut c.custom_label,
            ModuleConfig::Xspi(c) => &mut c.custom_label,
            ModuleConfig::Hspi(c) => &mut c.custom_label,
            ModuleConfig::Rmt(c) => &mut c.custom_label,
            ModuleConfig::Timer(c) => &mut c.custom_label,
            ModuleConfig::Can(c) => &mut c.custom_label,
            ModuleConfig::Usb(c) => &mut c.custom_label,
            ModuleConfig::Custom(c) => &mut c.custom_label,
        }
    }

    /// One-line summary for the schematic box (e.g. "USART1 · 9600 baud").
    pub fn summary(&self) -> String {
        match self {
            ModuleConfig::Usart(c) => format!("USART{}  ·  {} baud", c.instance, c.baud_rate),
            ModuleConfig::Lpuart(c) => format!("LPUART{}  ·  {} baud", c.instance, c.baud_rate),
            ModuleConfig::Timer(c) => format!("TIM{}  ·  {}", c.instance, hz_label(c.freq_hz)),
            ModuleConfig::Rmt(c) => format!(
                "RMT CH{}  ·  {}  ·  /{}",
                c.instance,
                c.direction.label(),
                c.clk_divider
            ),
            ModuleConfig::Spi(c) => {
                format!(
                    "SPI{}  ·  mode {}  ·  {}",
                    c.instance,
                    c.mode,
                    hz_label(c.clock_hz)
                )
            }
            ModuleConfig::I2c(c) => format!("I2C{}  ·  {}", c.instance, hz_label(c.clock_hz)),
            ModuleConfig::Dac(c) => format!("DAC{}  ·  {} ch", c.instance, c.values.len().max(1)),
            ModuleConfig::Sai(c) => {
                format!("SAI{}  ·  {} sub-block", c.instance, c.blocks.len().max(1))
            }
            ModuleConfig::Sdmmc(c) => sdmmc_name(c.instance),
            ModuleConfig::Qspi(c) => format!("QUADSPI  ·  {}", c.memory_size_label()),
            ModuleConfig::Ospi(c) => format!(
                "OCTOSPI{}  ·  {}  ·  {}",
                c.instance,
                c.size_label(),
                c.mode.label()
            ),
            ModuleConfig::Xspi(c) => format!(
                "XSPI{}  ·  {}  ·  {}",
                c.instance,
                c.size_label(),
                c.mode.label()
            ),
            ModuleConfig::Hspi(c) => format!(
                "HSPI{}  ·  {}  ·  {}",
                c.instance,
                c.size_label(),
                c.mode.label()
            ),
            ModuleConfig::I2s(c) => format!(
                "I2S{}  ·  {}  ·  {}",
                c.instance,
                hz_label(c.sample_rate_hz),
                c.direction.label()
            ),
            ModuleConfig::Custom(c) => format!(
                "custom  ·  {} pin{}",
                c.pins.len(),
                if c.pins.len() == 1 { "" } else { "s" }
            ),
            ModuleConfig::Can(c) => {
                format!("CAN{}  ·  {} kbit", c.instance, c.bitrate / 1_000)
            }
            ModuleConfig::Usb(c) => format!("USB CDC  ·  {:04x}:{:04x}", c.vid, c.pid),
        }
    }
}

/// Compact frequency label, e.g. "1 MHz", "400 kHz".
pub fn hz_label(hz: u32) -> String {
    if hz % 1_000_000 == 0 {
        format!("{} MHz", hz / 1_000_000)
    } else if hz % 1_000 == 0 {
        format!("{} kHz", hz / 1_000)
    } else {
        format!("{hz} Hz")
    }
}

/// A virtual module placed beside the MCU on the Pins canvas.
/// (Not `Eq`: `pos` holds `f32`.)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualModule {
    /// Stable id within the project (e.g. "usart_1").
    pub id: String,
    pub kind: ModuleKind,
    /// Display name (e.g. "USART1").
    pub name: String,
    /// Top-left position on the Pins canvas (used by the GUI phase).
    pub pos: (f32, f32),
    pub config: ModuleConfig,
    pub connections: Vec<Connection>,
}

impl VirtualModule {
    /// The MCU pin number currently wired to `signal`, if any.
    pub fn pin_for(&self, signal: ModuleSignal) -> Option<usize> {
        self.connections
            .iter()
            .find(|c| c.signal == signal)
            .map(|c| c.mcu_pin)
    }

    /// The peripheral instance this module targets.
    pub fn instance(&self) -> u8 {
        self.config.instance()
    }
}

#[cfg(test)]
mod rmt_direction_tests {
    use super::RmtDirection;

    /// The direction a channel can take is the chip's to decide, not the user's.
    ///
    /// esp-hal states the split for every part that has one: `Channel<0>` and
    /// `Channel<1>` transmit and `Channel<2>`/`Channel<3>` receive on the
    /// C3/C5/C6/H2, 0-3 against 4-7 on the S3, and the ESP32 and S2 leave every
    /// channel free to do either. A one-entry list is what the UI shows locked.
    #[test]
    fn a_channel_offers_only_the_direction_its_chip_gives_it() {
        for family in ["esp32c3", "esp32c5", "esp32c6", "esp32h2"] {
            assert_eq!(
                RmtDirection::options(family, 0),
                [RmtDirection::Transmit],
                "{family} channel 0"
            );
            assert_eq!(
                RmtDirection::options(family, 2),
                [RmtDirection::Receive],
                "{family} channel 2"
            );
        }
        // Eight channels, split down the middle.
        assert_eq!(
            RmtDirection::options("esp32s3", 3),
            [RmtDirection::Transmit]
        );
        assert_eq!(RmtDirection::options("esp32s3", 4), [RmtDirection::Receive]);
        // …and the two parts where every channel does either.
        for family in ["esp32", "esp32s2"] {
            assert_eq!(RmtDirection::options(family, 0).len(), 2, "{family}");
            assert_eq!(RmtDirection::options(family, 3).len(), 2, "{family}");
        }
    }
}

#[cfg(test)]
mod i2s_option_tests {
    use super::{I2sFormat, I2sMode, I2sStandard};

    /// Everything the I2S module offers on an ESP must have an esp-hal shape.
    ///
    /// The three lists are hand-written against esp-hal's own constructors, so
    /// this pins what is offered and what is deliberately left out. The STM32
    /// side keeps every variant, which is the other half of the claim: the
    /// narrowing is per family, not a general trim.
    #[test]
    fn an_esp_i2s_offers_only_what_esp_hal_can_build() {
        // No `Config::new_tdm_lsb()` exists.
        assert!(!I2sStandard::options("esp32c6").contains(&I2sStandard::LsbFirst));
        assert_eq!(I2sStandard::options("esp32c6").len(), 4);
        assert_eq!(
            I2sStandard::options("stm32f4").len(),
            I2sStandard::ALL.len()
        );

        // The two widths the two HALs both name.
        assert_eq!(
            I2sFormat::options("esp32c6"),
            [I2sFormat::Data16Channel16, I2sFormat::Data32Channel32]
        );
        assert_eq!(I2sFormat::options("stm32f4").len(), I2sFormat::ALL.len());

        // `I2s::new` calls `set_master()`; there is no follower entry point.
        assert_eq!(I2sMode::options("esp32c6"), [I2sMode::Master]);
        assert_eq!(I2sMode::options("stm32f4").len(), 2);
    }
}

#[cfg(test)]
mod blocking_dma_compat_tests {
    use super::{BlockingDma, UsartModuleConfig};

    /// A project saved when this field was a bool must still open. RON parses
    /// `@modules` as ONE value, so a field that fails to read takes every
    /// Virtual Module with it — the user would find an empty canvas.
    #[test]
    fn yesterdays_bool_still_reads() {
        let legacy = concat!(
            "(instance:1,baud_rate:115200,data_bits:8,parity:None,stop_bits:One,",
            "rx_model:\"\",tx_model:\"\",blocking_dma:true)"
        );
        let c: UsartModuleConfig = ron::from_str(legacy).expect("legacy config parses");
        assert_eq!(
            c.blocking_dma,
            BlockingDma::Both,
            "the bool meant both halves"
        );

        let off = legacy.replace("blocking_dma:true", "blocking_dma:false");
        let c: UsartModuleConfig = ron::from_str(&off).expect("legacy false parses");
        assert_eq!(c.blocking_dma, BlockingDma::Off);
    }

    /// ...and the current spelling round-trips, halves included.
    #[test]
    fn every_state_survives_a_round_trip() {
        for d in BlockingDma::ALL {
            let c = UsartModuleConfig {
                blocking_dma: d,
                ..UsartModuleConfig::new(1)
            };
            let text = ron::to_string(&c).expect("serialises");
            let back: UsartModuleConfig = ron::from_str(&text).expect("parses back");
            assert_eq!(back.blocking_dma, d, "{text}");
        }
    }
}
