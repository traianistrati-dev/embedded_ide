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

/// How a GPIO pin is driven / terminated — the part of the binding the user can
/// change (`into_floating_input` ⇄ `into_pull_up_input`, …).
///
/// It is a NEUTRAL key, not a HAL method name, because the same choice has to
/// survive a Runtime switch: `PullUp` is `into_pull_up_input(&mut gpiob.crl)` on
/// the blocking `stm32f1xx-hal` and `Input::new(p.PB5, Pull::Up)` on embassy.
/// Each backend renders it in its own syntax and says which of these it offers
/// ([`FamilyBackend::gpio_modes`]).
///
/// `None` on a pin means "the backend's default" — floating for an input,
/// push-pull for an output — which is what every project generated before this
/// existed, so a missing `@iomode` section round-trips unchanged.
///
/// [`FamilyBackend::gpio_modes`]: crate::panels::mcu_module::codegen::family::FamilyBackend::gpio_modes
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum GpioMode {
    // Inputs
    Floating,
    PullUp,
    PullDown,
    // Outputs
    PushPull,
    OpenDrain,
}

impl GpioMode {
    /// Token persisted in `mcu.config` `@iomode`.
    pub fn as_token(self) -> &'static str {
        match self {
            Self::Floating => "Floating",
            Self::PullUp => "PullUp",
            Self::PullDown => "PullDown",
            Self::PushPull => "PushPull",
            Self::OpenDrain => "OpenDrain",
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        match s.trim() {
            "Floating" => Some(Self::Floating),
            "PullUp" => Some(Self::PullUp),
            "PullDown" => Some(Self::PullDown),
            "PushPull" => Some(Self::PushPull),
            "OpenDrain" => Some(Self::OpenDrain),
            _ => None,
        }
    }

    /// Row text in the mode list under the selected function.
    pub fn label(self) -> &'static str {
        match self {
            Self::Floating => "Floating",
            Self::PullUp => "Pull-up",
            Self::PullDown => "Pull-down",
            Self::PushPull => "Push-pull",
            Self::OpenDrain => "Open-drain",
        }
    }

    /// The `stm32f1xx-hal` (and generally `into_*`) method this mode maps to.
    pub fn into_method(self) -> &'static str {
        match self {
            Self::Floating => "into_floating_input",
            Self::PullUp => "into_pull_up_input",
            Self::PullDown => "into_pull_down_input",
            Self::PushPull => "into_push_pull_output",
            Self::OpenDrain => "into_open_drain_output",
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
    /// Drive / pull mode of a GPIO In/Out pin — `None` = the backend's default
    /// (floating input, push-pull output). Chosen from the mode list under the
    /// function in the chip, persisted in `mcu.config` (`@iomode`), and rendered
    /// by each backend in its own syntax. See [`GpioMode`].
    pub io_mode: Option<GpioMode>,
    /// `(vendor signal, alternate-function index)` for this pin, from the chip
    /// definition ([`crate::panels::mcu_module::mcu_def::PinDef::af`]).
    ///
    /// Per (pin, SIGNAL) rather than per function: the same signal sits on a
    /// different AF number on a different pin, so the index cannot live inside
    /// `PinFunction` — a value that is shared across the whole chip. Empty on
    /// STM32F1 (no per-pin AF mux; it remaps whole peripherals through AFIO) and
    /// for definitions imported before the indices were captured.
    pub af: Vec<(String, u8)>,
}

impl Pin {
    /// `true` when any available function is a serial communication bus
    /// (USART / SPI / I2C / USB / CAN). Such pins get an orange number on the
    /// chip so they stand out from plain GPIO / analog / power pins.
    /// The alternate-function index this pin uses for `signal`, if the vendor
    /// published one. `signal` is the datasheet name (`TIM1_CH1N`), which is what
    /// [`PinFunction::Other`] carries.
    pub fn af_of(&self, signal: &str) -> Option<u8> {
        self.af
            .iter()
            .find(|(s, _)| s.eq_ignore_ascii_case(signal))
            .map(|(_, n)| *n)
    }

    /// The AF index of the function currently selected on this pin, when that
    /// function is a generic alternate one. `None` for GPIO / modelled
    /// peripherals (whose driver sets the AF itself) and on STM32F1.
    pub fn selected_af(&self) -> Option<u8> {
        match &self.selected_function {
            PinFunction::Other(signal) => self.af_of(signal),
            _ => None,
        }
    }

    pub fn has_bus_function(&self) -> bool {
        self.available_functions.iter().any(PinFunction::is_bus)
    }
}
