//! Virtual electronic modules attached to the MCU on the Pins canvas.
//!
//! A module mimics a physical peripheral device (USART / SPI / I2C) and
//! auto-connects to compatible MCU pins, drawn as a simplified schematic next to
//! the chip. This is the data model; auto-wiring lives in [`super::autowire`].

use crate::panels::mcu_module::pins::logic::pin_function::PinFunction;
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
    pub const ALL: [ModuleKind; 8] = [
        ModuleKind::GenericInterfaceUsart,
        ModuleKind::GenericInterfaceLpuart,
        ModuleKind::GenericInterfaceSpi,
        ModuleKind::GenericInterfaceI2c,
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
            blocking_dma: BlockingDma::default(),
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
            ModuleConfig::Spi(c) => {
                format!(
                    "SPI{}  ·  mode {}  ·  {}",
                    c.instance,
                    c.mode,
                    hz_label(c.clock_hz)
                )
            }
            ModuleConfig::I2c(c) => format!("I2C{}  ·  {}", c.instance, hz_label(c.clock_hz)),
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
