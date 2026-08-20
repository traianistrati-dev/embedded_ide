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
            ModuleKind::GenericInterfaceSpi => (&[Sck, Mosi, Miso], &[Nss]),
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
    /// Blocking runtime on STM32F1: run this bus on DMA instead of polling.
    ///
    /// A separate flag from the async `mode` / `async_mode`, because it is a
    /// different HAL: `stm32f1xx-hal`'s `Rx::with_dma` / `Spi::with_rx_tx_dma`,
    /// not embassy's. There is no channel to choose here — the F1 HAL fixes it
    /// per peripheral in its TYPES (USART1 is dma1::C4/C5, SPI1 dma1::C2/C3),
    /// which is why this is a bool and not [`Self::dma_tx`].
    ///
    /// Ignored on every other family and on the Async/Native/RTIC runtimes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blocking_dma: bool,
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
            blocking_dma: false,
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
    /// Blocking runtime on STM32F1: run this bus on DMA instead of polling.
    ///
    /// A separate flag from the async `mode` / `async_mode`, because it is a
    /// different HAL: `stm32f1xx-hal`'s `Rx::with_dma` / `Spi::with_rx_tx_dma`,
    /// not embassy's. There is no channel to choose here — the F1 HAL fixes it
    /// per peripheral in its TYPES (USART1 is dma1::C4/C5, SPI1 dma1::C2/C3),
    /// which is why this is a bool and not [`Self::dma_tx`].
    ///
    /// Ignored on every other family and on the Async/Native/RTIC runtimes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blocking_dma: bool,
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
            blocking_dma: false,
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
    /// Channel number (1..=4) → duty cycle in percent.
    #[serde(default)]
    pub duty: std::collections::BTreeMap<u8, u8>,
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
            duty: std::collections::BTreeMap::new(),
            rx_model: String::new(),
            tx_model: String::new(),
            custom_label: String::new(),
        }
    }

    /// This channel's duty cycle, 0 % when the user has not set one.
    pub fn duty_of(&self, channel: u8) -> u8 {
        self.duty.get(&channel).copied().unwrap_or(0)
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
