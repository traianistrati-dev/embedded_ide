//! MCU "Peripherals" tab — the inverse of the Pins tab.
//!
//! Lists **every** selectable function the current chip exposes, grouped by
//! peripheral category. Each category shows how many pins support it and all of
//! those pins (the assigned ones highlighted). An "Assign ▾" popup — and the
//! clickable pin chips — let you pick which pin gets the function, the mirror of
//! choosing a function on a pin in the Pins tab.

use crate::panels::mcu_module::Mcu;
use crate::panels::mcu_module::Pin;
use crate::panels::mcu_module::PinFunction;
use eframe::egui;

/// One assignable `(pin, function)` option inside a category.
struct Entry {
    pin_num: usize,
    pin_name: String,
    func: PinFunction,
    func_label: String,
    /// `true` when this pin currently has exactly this function selected.
    assigned: bool,
}

/// A peripheral category and all its assignable options on this chip.
struct CategoryView {
    name: &'static str,
    color: egui::Color32,
    entries: Vec<Entry>,
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
                "Every function this chip can use. Click a pin chip — or use \
                 “Assign ▾” — to set it. This is the inverse of the Pins tab.",
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

/// Build the per-category view from the chip's pins (available functions).
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

    defs.iter()
        .filter_map(|(name, (r, g, b), pred)| {
            let mut entries = Vec::new();
            for pin in &pins {
                for f in &pin.available_functions {
                    if pred(f) {
                        entries.push(Entry {
                            pin_num: pin.number,
                            pin_name: pin.name.clone(),
                            func: f.clone(),
                            func_label: f.label(),
                            assigned: &pin.selected_function == f,
                        });
                    }
                }
            }
            if entries.is_empty() {
                None
            } else {
                Some(CategoryView {
                    name,
                    color: egui::Color32::from_rgb(*r, *g, *b),
                    entries,
                })
            }
        })
        .collect()
}

/// Draw one category: header (swatch + name + count + Assign popup) and the
/// wrapped, clickable pin chips.
fn category_row(ui: &mut egui::Ui, cat: &CategoryView, pending: &mut Option<(usize, PinFunction)>) {
    let assigned = cat.entries.iter().filter(|e| e.assigned).count();
    let total = cat.entries.len();

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 16.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, cat.color);
        ui.label(egui::RichText::new(cat.name).size(13.0).strong().color(cat.color));

        let badge = if assigned > 0 {
            format!("{assigned}/{total}")
        } else {
            format!("{total}")
        };
        ui.label(
            egui::RichText::new(badge)
                .size(11.0)
                .color(egui::Color32::from_rgb(150, 150, 160)),
        );

        // ── Assign popup — pick which pin gets the function ──
        ui.menu_button(egui::RichText::new("Assign ▾").size(11.0), |ui| {
            ui.set_min_width(200.0);
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for e in &cat.entries {
                    let mark = if e.assigned { "✓ " } else { "    " };
                    let text =
                        format!("{mark}pin{} {}  ·  {}", e.pin_num, e.pin_name, e.func_label);
                    let col = if e.assigned {
                        cat.color
                    } else {
                        egui::Color32::from_rgb(200, 200, 210)
                    };
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new(text).size(11.0).color(col))
                                .frame(false),
                        )
                        .clicked()
                    {
                        *pending = Some(toggle(e));
                        ui.close_menu();
                    }
                }
            });
        });
    });

    // ── Inline chips: all supporting pins; assigned ones highlighted ──
    ui.horizontal_wrapped(|ui| {
        for e in &cat.entries {
            let chip = format!("pin{} {}", e.pin_num, e.pin_name);
            let btn = if e.assigned {
                egui::Button::new(
                    egui::RichText::new(chip).size(11.0).color(egui::Color32::WHITE),
                )
                .fill(cat.color)
            } else {
                egui::Button::new(
                    egui::RichText::new(chip)
                        .size(11.0)
                        .color(egui::Color32::from_rgb(170, 170, 180)),
                )
                .fill(egui::Color32::from_rgb(45, 45, 52))
            };
            let resp = ui.add(btn).on_hover_text(format!(
                "{}\nclick to {}",
                e.func_label,
                if e.assigned { "unassign" } else { "assign" }
            ));
            if resp.clicked() {
                *pending = Some(toggle(e));
            }
        }
    });

    ui.add_space(2.0);
    ui.separator();
}

/// Clicking an entry toggles it: assigned → clear (Unset), else → assign.
fn toggle(e: &Entry) -> (usize, PinFunction) {
    if e.assigned {
        (e.pin_num, PinFunction::Unset)
    } else {
        (e.pin_num, e.func.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::mock_mcu::create_stm32f103c8tx;

    #[test]
    fn categories_cover_expected_peripherals() {
        let mcu = create_stm32f103c8tx();
        let cats = build_categories(&mcu);
        let names: Vec<_> = cats.iter().map(|c| c.name).collect();

        for expected in ["GPIO Output", "GPIO Input", "ADC", "USART", "SPI", "I2C"] {
            assert!(names.contains(&expected), "missing category {expected}");
        }
        // ADC exposes many analog-capable pins.
        let adc = cats.iter().find(|c| c.name == "ADC").unwrap();
        assert!(adc.entries.len() >= 8, "expected ≥8 ADC pins");
        // Every entry carries a non-empty full label, and starts unassigned.
        for c in &cats {
            for e in &c.entries {
                assert!(!e.func_label.is_empty());
                assert!(!e.assigned, "nothing is configured on a fresh chip");
            }
        }
    }

    #[test]
    fn assigned_flag_reflects_selection() {
        let mut mcu = create_stm32f103c8tx();
        // PA0 is pin 10 on the C8T6 and supports GPIO Output.
        mcu.apply_pin_function(10, PinFunction::GpioOutput);

        let cats = build_categories(&mcu);
        let out = cats.iter().find(|c| c.name == "GPIO Output").unwrap();
        let pa0 = out.entries.iter().find(|e| e.pin_num == 10).unwrap();
        assert!(pa0.assigned, "PA0 must be marked assigned in GPIO Output");

        // The same pin appears unassigned under other categories it supports.
        let input = cats.iter().find(|c| c.name == "GPIO Input").unwrap();
        let pa0_in = input.entries.iter().find(|e| e.pin_num == 10).unwrap();
        assert!(!pa0_in.assigned);
    }

    #[test]
    fn toggle_assigns_then_clears() {
        let e = Entry {
            pin_num: 2,
            pin_name: "PC13".into(),
            func: PinFunction::GpioOutput,
            func_label: "GPIO Output".into(),
            assigned: false,
        };
        assert_eq!(toggle(&e), (2, PinFunction::GpioOutput));

        let e2 = Entry { assigned: true, ..e };
        assert_eq!(toggle(&e2), (2, PinFunction::Unset));
    }
}
