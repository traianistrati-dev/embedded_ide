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
    // Raspberry Pi Pico board pads. These are BOARD pins, not chip pins, and
    // every one of them answers a question a user actually asks while wiring.
    // The CYW43 radio's four lines on a Pico W / Pico 2 W. They are GP23, GP24,
    // GP25 and GP29 on the die, but on a W board they are spoken for — and GP25
    // is the one that surprises people, because on a non-W Pico it is the LED.
    } else if n == "WL_ON" {
        "Powers the CYW43 radio (GP23). Held low until the wireless driver brings it up."
    } else if n == "WL_D" {
        "The radio's SPI data line (GP24), driven by a PIO program rather than the SPI block."
    } else if n == "WL_CS" {
        "The radio's chip select (GP25). The on-board LED is NOT here on a W board - it hangs off the radio's own GPIO0, so blinking it needs the wireless driver."
    } else if n == "WL_CLK" {
        "The radio's SPI clock (GP29), shared with the VSYS sense divider."
    } else if n == "AGND" {
        "Analog ground, for the ADC. Star-tie it to GND so converter noise does not ride on the digital return."
    } else if n == "ADC_VREF" {
        "ADC reference. Filtered 3V3 on the board; drive it separately for a cleaner conversion."
    } else if n == "RUN" {
        "Chip enable, active HIGH. Pull it LOW to reset the board; it is how an external circuit holds the Pico down."
    } else if n == "3V3_OUT" {
        "3.3 V from the on-board regulator. Good for about 300 mA, and it is what powers the chip."
    } else if n == "3V3_EN" {
        "Enables the 3.3 V regulator, pulled HIGH on the board. Pull it LOW to switch the Pico off."
    } else if n == "VSYS" {
        "Main input, 1.8 to 5.5 V. Feeds the regulator through a diode from VBUS, so it can also be back-powered."
    } else if n == "VBUS" {
        "5 V straight from the micro-USB connector, present only while USB is plugged in."
    } else if n.starts_with("NPOR") {
        "Power-on reset."
    } else if n.starts_with("CHIP_PU") || n.starts_with("CHIP_EN") {
        // Two spellings of one pin: an ESP32/S2/S3/C5/C6/C61 calls it CHIP_PU,
        // a C2/C3/H2 calls it CHIP_EN. Only the first was answered, so the most
        // important reserved pad on half the Espressif parts read "Reserved -
        // fixed by the package".
        "Chip enable, active HIGH. Held low the part stays in reset."
    } else if n.starts_with("XTAL_") {
        "Main crystal. Its frequency is what the PLL multiplies up - the Clock tab shows which."
    } else if n.starts_with("LNA_IN") || n.starts_with("ANT") {
        "Radio antenna feed. It reaches the antenna through a matching network, and nothing else may load it."
    } else if n.starts_with("CAP") {
        "Filter capacitor for an on-chip supply. It takes the part its datasheet specifies and no signal."
    } else if n.starts_with("GPIO") {
        // A GPIO that is nonetheless RESERVED. On an ESP32-C5 the datasheet
        // gives pins 25-32 as GPIO15-GPIO22, and the esp-metadata release
        // esp-hal pins has no `peripherals.GPIO15` for the part — so the pad is
        // on the package and there is nothing the generated code could name.
        // Without this arm those seven pads read "Reserved - fixed by the
        // package", which says nothing about why a numbered GPIO is greyed out.
        "On the package, but this chip's HAL has no singleton for it - most often the in-package flash bus. Nothing generated can name it."
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
            // No exact-match list of Espressif rails here. There used to be one
            // — `VDD3P3`, `VDD3P3_CPU`, `VDD3P3_RTC`, `VDD_SPI` — and it was
            // both DEAD and STALE: every one of those starts with `VDD`, so
            // `is_power` above had already answered, and the eight parts added
            // after the C3 brought `VDDA1..8`, `VDDPST1..3`, `VDD_SDIO` and
            // `VDDA_PMU`, none of which were in it.
            //
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

        // The Espressif pads. `LNA_IN` was pinned to the generic answer here,
        // which described the state rather than an invariant — on an ESP32 most
        // reserved pads are one of these, and the panel exists to say what a pin
        // is for.
        assert!(reserved_role("LNA_IN").contains("antenna"));
        assert!(reserved_role("ANT_2G").contains("antenna"));
        assert!(reserved_role("XTAL_P").contains("crystal"));
        assert!(reserved_role("CAP1").contains("capacitor"));
        // One pin, two spellings: an ESP32 says CHIP_PU, a C3 says CHIP_EN.
        assert_eq!(reserved_role("CHIP_EN"), reserved_role("CHIP_PU"));
        assert!(reserved_role("CHIP_EN").contains("Chip enable"));

        // …and the generic is still there, for a pad nothing is known about.
        assert!(reserved_role("PAD_7").starts_with("Reserved"));
    }
}
