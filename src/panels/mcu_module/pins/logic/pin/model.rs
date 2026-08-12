//! Pin data model — struct definition and rendering constants.

use super::super::pin_function::PinFunction;

pub const PIN_FONT_SIZE: f32 = 10.0;
pub const PIN_ROUNDING: f32 = 0.0;

/// Which edge of an input pin raises an interrupt.
///
/// `None` on a pin (the default) means "read it by polling" — the common case,
/// and why this is opt-in rather than implied by `GpioInput`. Only meaningful on
/// the RTIC runtime, where each interrupt-enabled pin becomes a hardware task.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Edge {
    Rising,
    Falling,
    Both,
}

impl Edge {
    /// Token persisted in `mcu.config` `@irq`.
    pub fn as_token(self) -> &'static str {
        match self {
            Self::Rising => "Rising",
            Self::Falling => "Falling",
            Self::Both => "Both",
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        match s.trim() {
            "Rising" => Some(Self::Rising),
            "Falling" => Some(Self::Falling),
            "Both" => Some(Self::Both),
            _ => None,
        }
    }

    /// The `stm32f1xx-hal` `ExtiPin::trigger_on_edge` argument.
    pub fn hal_variant(self) -> &'static str {
        match self {
            Self::Rising => "Edge::Rising",
            Self::Falling => "Edge::Falling",
            Self::Both => "Edge::RisingFalling",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rising => "Rising",
            Self::Falling => "Falling",
            Self::Both => "Both edges",
        }
    }
}

/// Represents a physical pin on a microcontroller.
///
/// Each pin has:
/// - A name (e.g., "PA5", "GPIO_21") used for display
/// - A number for identification
/// - A reserved flag indicating if it cannot be reconfigured (VDD, VSS, NRST)
/// - Available functions (GPIO, ADC, SPI, USART, etc.)
/// - Currently selected function
/// - An optional user label appended to the generated variable name
#[derive(Clone)]
pub struct Pin {
    pub name: String,
    pub number: usize,
    pub reserved: bool,
    pub available_functions: Vec<PinFunction>,
    pub selected_function: PinFunction,
    /// User-typed name appended to the generated binding, e.g. a `pc13` output
    /// with label "led" generates `let pc13_out_led = …`. Empty by default;
    /// editable via the in/out arrow on the Pins canvas (GPIO In/Out & PWM).
    pub custom_label: String,
    /// Interrupt trigger for a GPIO input, when the user asked for one.
    ///
    /// Opt-in: most inputs are polled, so deriving "interrupt" from
    /// `GpioInput` alone would generate a task for every button. Consumed by the
    /// RTIC backend; ignored by the other runtimes, which do not wire the NVIC.
    /// Persisted in `mcu.config` (`@irq`).
    pub irq: Option<Edge>,
}

impl Pin {
    /// `true` when any available function is a serial communication bus
    /// (USART / SPI / I2C / USB / CAN). Such pins get an orange number on the
    /// chip so they stand out from plain GPIO / analog / power pins.
    pub fn has_bus_function(&self) -> bool {
        self.available_functions.iter().any(PinFunction::is_bus)
    }
}
