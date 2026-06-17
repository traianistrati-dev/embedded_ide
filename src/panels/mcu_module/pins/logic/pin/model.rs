//! Pin data model — struct definition and rendering constants.

use super::super::pin_function::PinFunction;

pub const PIN_FONT_SIZE: f32 = 10.0;
pub const PIN_ROUNDING: f32 = 0.0;

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
}
