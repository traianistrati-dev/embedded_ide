//! MCU "Peripherals" tab — the inverse of the Pins tab.
//!
//! Lists every peripheral category the chip exposes and, under each, the pins
//! that can serve it — **one chip per pin**. A pin that can take several signals
//! of the same peripheral (e.g. an ESP32-C3 GPIO that the GPIO matrix can route
//! to SPI2 SCK/MOSI/MISO/NSS) shows a single chip with a ▾ menu to pick the
//! signal, instead of repeating the pin once per signal. This is the mirror of
//! choosing a function on a pin in the Pins tab.

use crate::panels::mcu_module::Mcu;
use crate::panels::mcu_module::Pin;
use crate::panels::mcu_module::PinFunction;
use eframe::egui;
use egui_phosphor::regular as ph;

/// One signal a pin can take inside a category (e.g. "SPI2  SCK").
struct PinOption {
    func: PinFunction,
    label: String,
    /// `true` when the pin currently has exactly this function selected.
    assigned: bool,
}

/// A pin and every signal it can serve within one category (deduped — the pin
/// appears once even when it supports several signals of the peripheral).
struct PinEntry {
    pin_num: usize,
    pin_name: String,
    options: Vec<PinOption>,
}

impl PinEntry {
    /// The signal currently selected on this pin within the category, if any.
    fn assigned(&self) -> Option<&PinOption> {
        self.options.iter().find(|o| o.assigned)
    }
}

/// A peripheral category and the pins that can serve it on this chip.
struct CategoryView {
    name: &'static str,
    color: egui::Color32,
    pins: Vec<PinEntry>,
}

/// Render the Peripherals tab. Returns the `(num, name, func)` change when the
/// user assigns or clears a function, so the caller can re-sync `pins/` files.
pub fn show_peripherals_tab(
    ui: &mut egui::Ui,
    mcu_opt: &mut Option<Mcu>,
) -> Option<(usize, String, PinFunction)> {
    let Some(mcu) = mcu_opt.as_mut() else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No chip selected.").color(egui::Color32::GRAY));
        });
        return None;
    };

    // Owned snapshot — no borrow of `mcu` is held across the UI pass below.
    let categories = build_categories(mcu);
    if categories.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("This chip exposes no selectable functions.")
                    .size(14.0)
                    .color(egui::Color32::GRAY),
            );
        });
        return None;
    }

    // (pin_num, func) to apply after the UI pass (Unset = clear the pin).
    let mut pending: Option<(usize, PinFunction)> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Every peripheral this chip can use, with the pins that can serve it. \
                 Click a pin to assign it; pins with several roles open a dropdown menu. \
                 This is the inverse of the Pins tab.",
            )
            .size(11.0)
            .color(egui::Color32::from_rgb(130, 130, 145)),
        );
        ui.add_space(2.0);

        for cat in &categories {
            category_row(ui, cat, &mut pending);
        }
    });

    if let Some((pin_num, func)) = pending {
        return mcu.apply_pin_function(pin_num, func);
    }
    None
}

/// Build the per-category view from the chip's pins (available functions),
/// grouping each pin's matching signals into a single [`PinEntry`].
fn build_categories(mcu: &Mcu) -> Vec<CategoryView> {
    type Pred = fn(&PinFunction) -> bool;
    // Ordered category table: (display name, RGB, "is this function mine?").
    let defs: &[(&'static str, (u8, u8, u8), Pred)] = &[
        ("GPIO Output", (200, 120, 50), |f| matches!(f, PinFunction::GpioOutput)),
        ("GPIO Input", (70, 160, 70), |f| matches!(f, PinFunction::GpioInput)),
        ("ADC", (150, 70, 200), |f| matches!(f, PinFunction::AdcChannel { .. })),
        ("Timers / PWM", (190, 170, 30), |f| matches!(f, PinFunction::TimerPwm { .. })),
        ("USART", (50, 110, 200), |f| {
            matches!(
                f,
                PinFunction::UsartTx(_)
                    | PinFunction::UsartRx(_)
                    | PinFunction::UsartCts(_)
                    | PinFunction::UsartRts(_)
                    | PinFunction::UsartCk(_)
            )
        }),
        ("SPI", (30, 170, 170), |f| {
            matches!(
                f,
                PinFunction::SpiNss(_)
                    | PinFunction::SpiSck(_)
                    | PinFunction::SpiMiso(_)
                    | PinFunction::SpiMosi(_)
            )
        }),
        ("I2C", (60, 180, 100), |f| {
            matches!(f, PinFunction::I2cScl(_) | PinFunction::I2cSda(_))
        }),
        ("USB", (190, 50, 160), |f| {
            matches!(f, PinFunction::UsbDm | PinFunction::UsbDp)
        }),
        ("CAN", (200, 130, 20), |f| {
            matches!(f, PinFunction::CanRx | PinFunction::CanTx)
        }),
        ("SWD / Debug", (190, 50, 50), |f| {
            matches!(f, PinFunction::SwdIo | PinFunction::SwdClk)
        }),
        ("MCO / Clock", (150, 150, 160), |f| matches!(f, PinFunction::Mco)),
    ];

    let pins: Vec<&Pin> = mcu.iter_all_pins().filter(|p| !p.reserved).collect();

    // Mirror the Pins panel's visibility rule (mcu/gui/mod.rs): a function that
    // is already selected on a *different* pin is hidden everywhere else, so an
    // exclusive signal (e.g. SPI2 SCK) can't be offered — or assigned — twice.
    // GPIO In/Out are shareable and never hidden. This keeps the Peripherals tab
    // consistent with the Pins tab (a signal taken on one pin disappears from
    // the others in both views).
    let taken_elsewhere = |pin_num: usize, f: &PinFunction| {
        !matches!(f, PinFunction::GpioInput | PinFunction::GpioOutput)
            && pins
                .iter()
                .any(|p| p.number != pin_num && &p.selected_function == f)
    };

    defs.iter()
        .filter_map(|(name, (r, g, b), pred)| {
            let mut pin_entries = Vec::new();
            for pin in &pins {
                // A pin already configured for a function belongs to a single
                // category: once it holds a function, it disappears from every
                // *other* category, so the same pin can never be selected twice
                // with two different functionalities. Within its own category
                // the signal menu still lets the user re-pick or unassign it.
                let configured = pin.selected_function != PinFunction::Unset;
                if configured && !pred(&pin.selected_function) {
                    continue;
                }

                let mut options = Vec::new();
                for f in &pin.available_functions {
                    // Always keep the owning pin's option (so a signal can be
                    // unassigned even if it ended up on two pins); hide it only
                    // on pins that don't currently hold it.
                    let owns = &pin.selected_function == f;
                    if pred(f) && (owns || !taken_elsewhere(pin.number, f)) {
                        options.push(PinOption {
                            func: f.clone(),
                            label: f.label(),
                            assigned: owns,
                        });
                    }
                }
                if !options.is_empty() {
                    pin_entries.push(PinEntry {
                        pin_num: pin.number,
                        pin_name: pin.name.clone(),
                        options,
                    });
                }
            }
            if pin_entries.is_empty() {
                None
            } else {
                Some(CategoryView {
                    name,
                    color: egui::Color32::from_rgb(*r, *g, *b),
                    pins: pin_entries,
                })
            }
        })
        .collect()
}

/// The `(pin, func)` to apply when an option is clicked: assigned → clear, else
/// → assign.
fn click_action(pin_num: usize, opt: &PinOption) -> (usize, PinFunction) {
    if opt.assigned {
        (pin_num, PinFunction::Unset)
    } else {
        (pin_num, opt.func.clone())
    }
}

/// Draw one category: header (swatch + name + count) and one chip per pin.
fn category_row(ui: &mut egui::Ui, cat: &CategoryView, pending: &mut Option<(usize, PinFunction)>) {
    let assigned = cat.pins.iter().filter(|p| p.assigned().is_some()).count();
    let total = cat.pins.len();

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 16.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, cat.color);
        ui.label(egui::RichText::new(cat.name).size(13.0).strong().color(cat.color));

        let badge = if assigned > 0 {
            format!("{assigned}/{total} pins")
        } else {
            format!("{total} pins")
        };
        ui.label(
            egui::RichText::new(badge)
                .size(11.0)
                .color(egui::Color32::from_rgb(150, 150, 160)),
        );
    });

    // ── One chip per pin; assigned ones highlighted ──
    ui.horizontal_wrapped(|ui| {
        for pe in &cat.pins {
            pin_chip(ui, cat.color, pe, pending);
        }
    });

    ui.add_space(2.0);
    ui.separator();
}

/// One pin chip. A single-signal pin is a plain toggle button; a multi-signal
/// pin opens a ▾ menu listing its signals so the pin is shown only once.
fn pin_chip(
    ui: &mut egui::Ui,
    color: egui::Color32,
    pe: &PinEntry,
    pending: &mut Option<(usize, PinFunction)>,
) {
    let assigned = pe.assigned();
    let base = format!("pin{} {}", pe.pin_num, pe.pin_name);

    let text_col = if assigned.is_some() {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(170, 170, 180)
    };

    if pe.options.len() == 1 {
        // Single role — direct toggle, with the signal shown inline.
        let opt = &pe.options[0];
        let label = format!("{base} · {}", opt.label);
        let btn = egui::Button::new(egui::RichText::new(label).size(11.0).color(text_col))
            .fill(chip_fill(color, opt.assigned));
        let resp = ui.add(btn).on_hover_text(format!(
            "{}\nclick to {}",
            opt.label,
            if opt.assigned { "unassign" } else { "assign" }
        ));
        if resp.clicked() {
            *pending = Some(click_action(pe.pin_num, opt));
        }
        return;
    }

    // Multiple roles — one chip with a ▾ menu, so the pin is listed once.
    let label = match assigned {
        Some(o) => format!("{base} · {} {}", o.label, ph::CARET_DOWN),
        None => format!("{base} {}", ph::CARET_DOWN),
    };
    let title = egui::RichText::new(label).size(11.0).color(text_col);
    let button = egui::Button::new(title).fill(chip_fill(color, assigned.is_some()));
    egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        ui.set_min_width(150.0);
        for opt in &pe.options {
            let mark = if opt.assigned {
                format!("{} ", ph::CHECK)
            } else {
                "    ".to_string()
            };
            let col = if opt.assigned {
                color
            } else {
                egui::Color32::from_rgb(205, 205, 215)
            };
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new(format!("{mark}{}", opt.label))
                            .size(11.0)
                            .color(col),
                    )
                    .frame(false),
                )
                .clicked()
            {
                *pending = Some(click_action(pe.pin_num, opt));
                ui.close();
            }
        }
    });
}

/// Chip background: category colour when assigned, neutral otherwise.
fn chip_fill(color: egui::Color32, assigned: bool) -> egui::Color32 {
    if assigned {
        color
    } else {
        egui::Color32::from_rgb(45, 45, 52)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::mcu_catalog::ToolchainKind;
    use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;

    fn category<'a>(cats: &'a [CategoryView], name: &str) -> &'a CategoryView {
        cats.iter().find(|c| c.name == name).unwrap()
    }

    #[test]
    fn categories_cover_expected_peripherals() {
        let mcu = create_stm32f103c8tx();
        let cats = build_categories(&mcu);
        let names: Vec<_> = cats.iter().map(|c| c.name).collect();

        for expected in ["GPIO Output", "GPIO Input", "ADC", "USART", "SPI", "I2C"] {
            assert!(names.contains(&expected), "missing category {expected}");
        }
        // ADC exposes many analog-capable pins.
        assert!(category(&cats, "ADC").pins.len() >= 8, "expected ≥8 ADC pins");
        // Every option carries a non-empty label, and starts unassigned.
        for c in &cats {
            for p in &c.pins {
                assert!(!p.options.is_empty());
                for o in &p.options {
                    assert!(!o.label.is_empty());
                    assert!(!o.assigned, "nothing is configured on a fresh chip");
                }
            }
        }
    }

    #[test]
    fn assigned_flag_reflects_selection() {
        let mut mcu = create_stm32f103c8tx();
        // PA0 is pin 10 on the C8T6 and supports GPIO Output.
        mcu.apply_pin_function(10, PinFunction::GpioOutput);

        let cats = build_categories(&mcu);
        let pa0 = category(&cats, "GPIO Output")
            .pins
            .iter()
            .find(|p| p.pin_num == 10)
            .unwrap();
        assert!(pa0.assigned().is_some(), "PA0 must be assigned in GPIO Output");

        // A configured pin disappears from every *other* category, so it can't
        // be selected twice with a different functionality.
        let pa0_in = category(&cats, "GPIO Input")
            .pins
            .iter()
            .find(|p| p.pin_num == 10);
        assert!(
            pa0_in.is_none(),
            "PA0 is configured as GPIO Output → must not appear under GPIO Input"
        );
    }

    /// Once a pin holds a function it is offered only inside that function's
    /// category — never under any other peripheral it could otherwise serve.
    #[test]
    fn configured_pin_hidden_from_other_categories() {
        let mut mcu = create_stm32f103c8tx();
        // PA0 (pin 10) supports both GPIO and ADC1 IN0 on the C8T6.
        // Before assignment it shows under both GPIO Output and ADC.
        let before = build_categories(&mcu);
        assert!(category(&before, "ADC").pins.iter().any(|p| p.pin_num == 10));
        assert!(category(&before, "GPIO Output").pins.iter().any(|p| p.pin_num == 10));

        // Assign ADC → PA0 must vanish from GPIO Output (and every other row).
        mcu.apply_pin_function(10, PinFunction::AdcChannel { adc: 1, channel: 0 });
        let after = build_categories(&mcu);
        assert!(
            category(&after, "ADC").pins.iter().any(|p| p.pin_num == 10),
            "PA0 stays in its own ADC category"
        );
        assert!(
            !category(&after, "GPIO Output").pins.iter().any(|p| p.pin_num == 10),
            "PA0 is taken by ADC → hidden from GPIO Output"
        );
    }

    /// A pin that supports several signals of one peripheral (ESP32-C3 GPIO
    /// matrix) is listed ONCE with every signal as a menu option — not repeated.
    #[test]
    fn multi_signal_pin_is_listed_once() {
        let mcu = Mcu::new(
            "t".into(),
            "esp32c3".into(),
            ToolchainKind::EspRust,
            vec![],
            vec![],
            vec![Pin::new(28, "GPIO21").with_functions(vec![
                PinFunction::SpiSck(2),
                PinFunction::SpiMosi(2),
                PinFunction::SpiMiso(2),
                PinFunction::SpiNss(2),
            ])],
            vec![],
        );
        let cats = build_categories(&mcu);
        let spi = category(&cats, "SPI");
        assert_eq!(spi.pins.len(), 1, "GPIO21 should appear once, not 4×");
        assert_eq!(spi.pins[0].options.len(), 4, "all 4 SPI signals as options");
    }

    /// An exclusive signal assigned to one pin is hidden from the others in the
    /// Peripherals tab (mirrors the Pins panel), so it can't be assigned twice.
    #[test]
    fn taken_signal_is_hidden_on_other_pins() {
        let spi_sck = || vec![PinFunction::SpiSck(1)];
        let mut mcu = Mcu::new(
            "t".into(),
            "stm32f1".into(),
            ToolchainKind::RustEmbedded,
            vec![],
            vec![],
            vec![Pin::new(1, "PA5").with_functions(spi_sck())],
            vec![Pin::new(2, "PB3").with_functions(spi_sck())],
        );

        // Both pins offer SPI1 SCK before anything is assigned.
        assert_eq!(category(&build_categories(&mcu), "SPI").pins.len(), 2);

        // Assign it to pin 1 → it disappears from pin 2 (only the owner remains).
        mcu.apply_pin_function(1, PinFunction::SpiSck(1));
        let cats = build_categories(&mcu);
        let spi = category(&cats, "SPI");
        assert_eq!(spi.pins.len(), 1, "taken signal hidden on the other pin");
        assert_eq!(spi.pins[0].pin_num, 1);
        assert!(spi.pins[0].assigned().is_some());
    }

    #[test]
    fn click_action_assigns_then_clears() {
        let unset = PinOption {
            func: PinFunction::GpioOutput,
            label: "GPIO Output".into(),
            assigned: false,
        };
        assert_eq!(click_action(2, &unset), (2, PinFunction::GpioOutput));

        let set = PinOption { assigned: true, ..unset };
        assert_eq!(click_action(2, &set), (2, PinFunction::Unset));
    }
}
