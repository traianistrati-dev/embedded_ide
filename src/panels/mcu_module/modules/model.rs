//! Virtual electronic modules attached to the MCU on the Pins canvas.
//!
//! A module mimics a physical peripheral device (e.g. a USART sensor) and
//! auto-connects to compatible MCU pins, drawn as a simplified schematic next to
//! the chip. This is the data model; auto-wiring lives in [`super::autowire`].

use serde::{Deserialize, Serialize};

/// Kind of virtual module. New kinds (I2C/SPI/…) are added here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleKind {
    /// Generic device speaking over USART (TX/RX) — shown as "GI_USART".
    GenericInterfaceUsart,
}

impl ModuleKind {
    /// Short tag used in the palette and as the default module name prefix.
    pub fn short(self) -> &'static str {
        match self {
            ModuleKind::GenericInterfaceUsart => "GI_USART",
        }
    }
}

/// One terminal of a module that wires to an MCU pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleSignal {
    Tx,
    Rx,
}

impl ModuleSignal {
    pub fn label(self) -> &'static str {
        match self {
            ModuleSignal::Tx => "TX",
            ModuleSignal::Rx => "RX",
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
        }
    }
}

/// Per-kind configuration payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleConfig {
    Usart(UsartModuleConfig),
}

/// A virtual module placed beside the MCU on the Pins canvas.
/// (Not `Eq`: `pos` holds `f32`.)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualModule {
    /// Stable id within the project (e.g. "gi_usart_1").
    pub id: String,
    pub kind: ModuleKind,
    /// Display name (e.g. "GI_USART1").
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

    /// The USART instance this module targets (only kind today).
    pub fn usart_instance(&self) -> u8 {
        match &self.config {
            ModuleConfig::Usart(c) => c.instance,
        }
    }
}
