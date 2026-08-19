//! Pin color logic — background and text colors based on reserved status and
//! function, plus the plain-English role of a reserved pin.

use super::model::Pin;
use eframe::egui;

const POWER: egui::Color32 = egui::Color32::from_rgb(200, 50, 50);
const GROUND: egui::Color32 = egui::Color32::from_rgb(30, 30, 30);
const VBAT: egui::Color32 = egui::Color32::from_rgb(220, 100, 100);

/// Is this reserved pin a supply rail?
///
/// PREFIX, not an exact name. Real parts label these `VDDA/VREF+`, `VSSA/VREF-`,
/// `VDD_1`, `VSS_2` — an exact match caught only the bare `VDD`/`VSS`, so every
/// decorated variant fell through to the grey that means "some other reserved
/// pin" and the supply rails stopped being findable at a glance.
fn is_power(name: &str) -> bool {
    let n = name.trim().to_ascii_uppercase();
    n.starts_with("VDD") || n.starts_with("VBAT")
}

/// The same for ground, including the ESP32 spelling.
fn is_ground(name: &str) -> bool {
    let n = name.trim().to_ascii_uppercase();
    n.starts_with("VSS") || n.starts_with("GND")
}

/// What a reserved pin is FOR, in one line, for the panel that opens when you
/// click it.
///
/// Reserved pins carry no selectable functions, so without this the panel would
/// be a header over an empty box. Matched by prefix for the same reason the
/// colours are.
pub fn reserved_role(name: &str) -> &'static str {
    let n = name.trim().to_ascii_uppercase();
    // Order matters: the longer, more specific prefixes are tested first, or
    // `VDDA` would be answered by the plain-`VDD` arm.
    if n.starts_with("VBAT") {
        "Backup supply - keeps the RTC and the backup registers alive while VDD is off."
    } else if n.starts_with("VDDA") {
        "Analog supply. Often shared with the converter reference (VREF+); decouple it separately from VDD."
    } else if n.starts_with("VREF") {
        "ADC/DAC voltage reference. Its accuracy sets the accuracy of every conversion."
    } else if n.starts_with("VDD") {
        "Digital supply. Each one wants its own decoupling capacitor."
    } else if n.starts_with("VSSA") {
        "Analog ground. Tie it to VSS at a single point to keep converter noise out of it."
    } else if n.starts_with("VSS") || n.starts_with("GND") {
        "Ground."
    } else if n.starts_with("NRST") || n == "RST" {
        "Reset, active LOW. Driven by the internal pull-up circuit at power-on."
    } else if n.starts_with("BOOT") {
        "Boot mode select, sampled at reset: it chooses whether the chip starts from flash or from the bootloader."
    } else if n.starts_with("NPOR") {
        "Power-on reset."
    } else if n.starts_with("CHIP_PU") {
        "Chip enable, active HIGH. Held low the part stays in reset."
    } else {
        "Reserved - fixed by the package, not configurable here."
    }
}

impl Pin {
    /// Determine background color for this pin.
    ///
    /// Reserved pins (power, ground, reset) have specific colors.
    /// User-configurable pins inherit color from their selected function.
    pub fn get_background_color(&self) -> egui::Color32 {
        if self.reserved {
            // Ground first: `VSS`/`GND` can never be a supply, and testing it
            // first keeps each rule a single prefix check.
            if is_ground(&self.name) {
                return GROUND;
            }
            if is_power(&self.name) {
                // VBAT keeps its lighter red. It is a supply, but not THE
                // supply, and telling them apart on the diagram is worth a shade.
                return if self.name.trim().to_ascii_uppercase().starts_with("VBAT") {
                    VBAT
                } else {
                    POWER
                };
            }
            // ESP32-C3 rails, which do not follow the ST naming.
            if matches!(
                self.name.as_str(),
                "VDD3P3" | "VDD3P3_CPU" | "VDD3P3_RTC" | "VDD_SPI"
            ) {
                return POWER;
            }
            // Misc reserved (NRST, BOOT0, CHIP_PU, LNA_IN, …)
            return egui::Color32::LIGHT_GRAY;
        }
        self.selected_function.color()
    }

    /// Determine text color for this pin.
    ///
    /// Reserved pins use white on the near-black ground colour and black
    /// elsewhere; user-configurable pins use black.
    pub fn get_text_color(&self) -> egui::Color32 {
        if self.reserved && is_ground(&self.name) {
            return egui::Color32::WHITE;
        }
        egui::Color32::BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bg(name: &str) -> egui::Color32 {
        Pin::new_reserved(1, name).get_background_color()
    }

    /// The decorated names real chips actually use. Every one of these was grey
    /// before, because the match was on the exact string.
    #[test]
    fn decorated_rail_names_still_read_as_rails() {
        for n in ["VDD", "VDDA", "VDDA/VREF+", "VDD_1", "VDDIO"] {
            assert_eq!(bg(n), POWER, "{n}");
        }
        for n in ["VSS", "VSSA", "VSSA/VREF-", "VSS_2", "GND"] {
            assert_eq!(bg(n), GROUND, "{n}");
        }
    }

    #[test]
    fn vbat_stays_distinguishable_from_the_main_supply() {
        assert_eq!(bg("VBAT"), VBAT);
        assert_ne!(bg("VBAT"), bg("VDD"));
    }

    #[test]
    fn ground_text_stays_readable_on_black() {
        assert_eq!(
            Pin::new_reserved(1, "VSSA/VREF-").get_text_color(),
            egui::Color32::WHITE
        );
        assert_eq!(
            Pin::new_reserved(1, "VDD").get_text_color(),
            egui::Color32::BLACK
        );
    }

    /// Every reserved pin says something specific where it can; the catch-all is
    /// the last resort, not the common case.
    #[test]
    fn the_roles_are_specific_where_they_can_be() {
        assert!(reserved_role("VDDA/VREF+").contains("Analog supply"));
        assert!(reserved_role("VSSA").contains("Analog ground"));
        assert!(reserved_role("VBAT").contains("Backup"));
        assert!(reserved_role("NRST").contains("active LOW"));
        assert!(reserved_role("BOOT0").contains("Boot mode"));
        // The prefix order matters: VDDA must not fall into the plain-VDD arm.
        assert_ne!(reserved_role("VDDA"), reserved_role("VDD"));
        assert!(reserved_role("LNA_IN").starts_with("Reserved"));
    }
}
