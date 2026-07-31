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
    /// Generic device on an SPI bus (SCK/MOSI/MISO/NSS) — "SPI".
    GenericInterfaceSpi,
    /// Generic device on an I2C bus (SCL/SDA) — "I2C".
    GenericInterfaceI2c,
    /// Generic device on a CAN bus (RX/TX) — "CAN".
    GenericInterfaceCan,
    /// USB full-speed device (D-/D+) — "USB".
    GenericInterfaceUsb,
}

impl ModuleKind {
    /// Every kind, in palette order.
    pub const ALL: [ModuleKind; 5] = [
        ModuleKind::GenericInterfaceUsart,
        ModuleKind::GenericInterfaceSpi,
        ModuleKind::GenericInterfaceI2c,
        ModuleKind::GenericInterfaceCan,
        ModuleKind::GenericInterfaceUsb,
    ];

    /// The `(required, optional)` signals this kind needs to auto-wire.
    /// Single source of truth: `Mcu::add_module` wires from it, and the palette
    /// asks the SAME table whether a chip can host the kind — so what the UI
    /// offers can never drift from what actually succeeds.
    pub fn signals(self) -> (&'static [ModuleSignal], &'static [ModuleSignal]) {
        use ModuleSignal::*;
        match self {
            ModuleKind::GenericInterfaceUsart => (&[Tx, Rx], &[]),
            ModuleKind::GenericInterfaceSpi => (&[Sck, Mosi, Miso], &[Nss]),
            ModuleKind::GenericInterfaceI2c => (&[Scl, Sda], &[]),
            ModuleKind::GenericInterfaceCan => (&[CanRx, CanTx], &[]),
            ModuleKind::GenericInterfaceUsb => (&[UsbDm, UsbDp], &[]),
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
            ModuleKind::GenericInterfaceSpi => "SPI",
            ModuleKind::GenericInterfaceI2c => "I2C",
            ModuleKind::GenericInterfaceCan => "CAN",
            ModuleKind::GenericInterfaceUsb => "USB",
        }
    }

    /// Default config for this kind on `instance`.
    pub fn default_config(self, instance: u8) -> ModuleConfig {
        match self {
            ModuleKind::GenericInterfaceUsart => {
                ModuleConfig::Usart(UsartModuleConfig::new(instance))
            }
            ModuleKind::GenericInterfaceSpi => ModuleConfig::Spi(SpiModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceI2c => ModuleConfig::I2c(I2cModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceCan => ModuleConfig::Can(CanModuleConfig::new(instance)),
            ModuleKind::GenericInterfaceUsb => ModuleConfig::Usb(UsbModuleConfig::new(instance)),
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
        PinFunction::SpiSck(n) => (GenericInterfaceSpi, *n, Sck),
        PinFunction::SpiMosi(n) => (GenericInterfaceSpi, *n, Mosi),
        PinFunction::SpiMiso(n) => (GenericInterfaceSpi, *n, Miso),
        PinFunction::SpiNss(n) => (GenericInterfaceSpi, *n, Nss),
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
    // SPI
    Sck,
    Mosi,
    Miso,
    Nss,
    // I2C
    Scl,
    Sda,
    // CAN
    CanRx,
    CanTx,
    // USB
    UsbDm,
    UsbDp,
}

impl ModuleSignal {
    pub fn label(self) -> &'static str {
        match self {
            ModuleSignal::Tx => "TX",
            ModuleSignal::Rx => "RX",
            ModuleSignal::Sck => "SCK",
            ModuleSignal::Mosi => "MOSI",
            ModuleSignal::Miso => "MISO",
            ModuleSignal::Nss => "NSS",
            ModuleSignal::Scl => "SCL",
            ModuleSignal::Sda => "SDA",
            ModuleSignal::CanRx => "RX",
            ModuleSignal::CanTx => "TX",
            ModuleSignal::UsbDm => "D-",
            ModuleSignal::UsbDp => "D+",
        }
    }

    /// The MCU pin function this signal needs on peripheral `instance`.
    pub fn pin_function(self, instance: u8) -> PinFunction {
        match self {
            ModuleSignal::Tx => PinFunction::UsartTx(instance),
            ModuleSignal::Rx => PinFunction::UsartRx(instance),
            ModuleSignal::Sck => PinFunction::SpiSck(instance),
            ModuleSignal::Mosi => PinFunction::SpiMosi(instance),
            ModuleSignal::Miso => PinFunction::SpiMiso(instance),
            ModuleSignal::Nss => PinFunction::SpiNss(instance),
            ModuleSignal::Scl => PinFunction::I2cScl(instance),
            ModuleSignal::Sda => PinFunction::I2cSda(instance),
            // CAN pin functions carry no instance (single CAN on STM32F1).
            ModuleSignal::CanRx => PinFunction::CanRx,
            ModuleSignal::CanTx => PinFunction::CanTx,
            // USB pin functions carry no instance (single USB FS on STM32F1).
            ModuleSignal::UsbDm => PinFunction::UsbDm,
            ModuleSignal::UsbDp => PinFunction::UsbDp,
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

/// Per-kind configuration payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleConfig {
    Usart(UsartModuleConfig),
    Spi(SpiModuleConfig),
    I2c(I2cModuleConfig),
    Can(CanModuleConfig),
    Usb(UsbModuleConfig),
}

impl ModuleConfig {
    /// The peripheral instance this module targets.
    pub fn instance(&self) -> u8 {
        match self {
            ModuleConfig::Usart(c) => c.instance,
            ModuleConfig::Spi(c) => c.instance,
            ModuleConfig::I2c(c) => c.instance,
            ModuleConfig::Can(c) => c.instance,
            ModuleConfig::Usb(c) => c.instance,
        }
    }

    pub fn rx_model(&self) -> &str {
        match self {
            ModuleConfig::Usart(c) => &c.rx_model,
            ModuleConfig::Spi(c) => &c.rx_model,
            ModuleConfig::I2c(c) => &c.rx_model,
            ModuleConfig::Can(c) => &c.rx_model,
            ModuleConfig::Usb(c) => &c.rx_model,
        }
    }

    pub fn tx_model(&self) -> &str {
        match self {
            ModuleConfig::Usart(c) => &c.tx_model,
            ModuleConfig::Spi(c) => &c.tx_model,
            ModuleConfig::I2c(c) => &c.tx_model,
            ModuleConfig::Can(c) => &c.tx_model,
            ModuleConfig::Usb(c) => &c.tx_model,
        }
    }

    pub fn rx_model_mut(&mut self) -> &mut String {
        match self {
            ModuleConfig::Usart(c) => &mut c.rx_model,
            ModuleConfig::Spi(c) => &mut c.rx_model,
            ModuleConfig::I2c(c) => &mut c.rx_model,
            ModuleConfig::Can(c) => &mut c.rx_model,
            ModuleConfig::Usb(c) => &mut c.rx_model,
        }
    }

    pub fn tx_model_mut(&mut self) -> &mut String {
        match self {
            ModuleConfig::Usart(c) => &mut c.tx_model,
            ModuleConfig::Spi(c) => &mut c.tx_model,
            ModuleConfig::I2c(c) => &mut c.tx_model,
            ModuleConfig::Can(c) => &mut c.tx_model,
            ModuleConfig::Usb(c) => &mut c.tx_model,
        }
    }

    /// User label appended to the module's generated handle variable(s).
    pub fn custom_label(&self) -> &str {
        match self {
            ModuleConfig::Usart(c) => &c.custom_label,
            ModuleConfig::Spi(c) => &c.custom_label,
            ModuleConfig::I2c(c) => &c.custom_label,
            ModuleConfig::Can(c) => &c.custom_label,
            ModuleConfig::Usb(c) => &c.custom_label,
        }
    }

    pub fn custom_label_mut(&mut self) -> &mut String {
        match self {
            ModuleConfig::Usart(c) => &mut c.custom_label,
            ModuleConfig::Spi(c) => &mut c.custom_label,
            ModuleConfig::I2c(c) => &mut c.custom_label,
            ModuleConfig::Can(c) => &mut c.custom_label,
            ModuleConfig::Usb(c) => &mut c.custom_label,
        }
    }

    /// One-line summary for the schematic box (e.g. "USART1 · 9600 baud").
    pub fn summary(&self) -> String {
        match self {
            ModuleConfig::Usart(c) => format!("USART{}  ·  {} baud", c.instance, c.baud_rate),
            ModuleConfig::Spi(c) => {
                format!(
                    "SPI{}  ·  mode {}  ·  {}",
                    c.instance,
                    c.mode,
                    hz_label(c.clock_hz)
                )
            }
            ModuleConfig::I2c(c) => format!("I2C{}  ·  {}", c.instance, hz_label(c.clock_hz)),
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
