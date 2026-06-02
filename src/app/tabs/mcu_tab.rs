//! MCU Peripherals tab — displays configured pin functions by peripheral type.

use eframe::egui;
use crate::panels::mcu_module::Mcu;
use crate::panels::mcu_module::Pin;
use crate::panels::mcu_module::PinFunction;

pub fn show_peripherals_tab(ui: &mut egui::Ui, mcu: &Option<Mcu>) {
    let Some(mcu) = mcu else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No chip selected.").color(egui::Color32::GRAY));
        });
        return;
    };

    let configured: Vec<_> = mcu
        .top_pins
        .iter()
        .chain(mcu.bottom_pins.iter())
        .chain(mcu.left_pins.iter())
        .chain(mcu.right_pins.iter())
        .filter(|p| !p.reserved && p.selected_function != PinFunction::Unset)
        .collect();

    if configured.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No pins configured yet.")
                    .size(14.0)
                    .color(egui::Color32::GRAY),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Switch to the Pins tab and assign functions to pins.")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(120, 120, 130)),
            );
        });
        return;
    }

    let mut gpio_in = vec![];
    let mut gpio_out = vec![];
    let mut adc = vec![];
    let mut timers = vec![];
    let mut usart = vec![];
    let mut spi = vec![];
    let mut i2c = vec![];
    let mut usb = vec![];
    let mut can = vec![];
    let mut swd = vec![];
    let mut other = vec![];

    for pin in &configured {
        match &pin.selected_function {
            PinFunction::GpioInput => gpio_in.push(*pin),
            PinFunction::GpioOutput => gpio_out.push(*pin),
            PinFunction::AdcChannel { .. } => adc.push(*pin),
            PinFunction::TimerPwm { .. } => timers.push(*pin),
            PinFunction::UsartTx(_)
            | PinFunction::UsartRx(_)
            | PinFunction::UsartCts(_)
            | PinFunction::UsartRts(_)
            | PinFunction::UsartCk(_) => usart.push(*pin),
            PinFunction::SpiNss(_)
            | PinFunction::SpiSck(_)
            | PinFunction::SpiMiso(_)
            | PinFunction::SpiMosi(_) => spi.push(*pin),
            PinFunction::I2cScl(_) | PinFunction::I2cSda(_) => i2c.push(*pin),
            PinFunction::UsbDm | PinFunction::UsbDp => usb.push(*pin),
            PinFunction::CanRx | PinFunction::CanTx => can.push(*pin),
            PinFunction::SwdIo | PinFunction::SwdClk => swd.push(*pin),
            _ => other.push(*pin),
        }
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);
        periph_section(
            ui,
            "GPIO Input",
            &gpio_in,
            egui::Color32::from_rgb(70, 160, 70),
        );
        periph_section(
            ui,
            "GPIO Output",
            &gpio_out,
            egui::Color32::from_rgb(200, 120, 50),
        );
        periph_section(ui, "ADC", &adc, egui::Color32::from_rgb(150, 70, 200));
        periph_section(
            ui,
            "Timers / PWM",
            &timers,
            egui::Color32::from_rgb(190, 170, 30),
        );
        periph_section(ui, "USART", &usart, egui::Color32::from_rgb(50, 110, 200));
        periph_section(ui, "SPI", &spi, egui::Color32::from_rgb(30, 170, 170));
        periph_section(ui, "I2C", &i2c, egui::Color32::from_rgb(60, 180, 100));
        periph_section(ui, "USB", &usb, egui::Color32::from_rgb(190, 50, 160));
        periph_section(ui, "CAN", &can, egui::Color32::from_rgb(200, 130, 20));
        periph_section(
            ui,
            "SWD / Debug",
            &swd,
            egui::Color32::from_rgb(190, 50, 50),
        );
        periph_section(ui, "Other", &other, egui::Color32::GRAY);
    });
}

pub fn periph_section(ui: &mut egui::Ui, title: &str, pins: &[&Pin], color: egui::Color32) {
    if pins.is_empty() {
        return;
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 16.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, color);
        ui.label(egui::RichText::new(title).size(13.0).strong().color(color));
    });

    let dim = egui::Color32::from_rgb(140, 140, 155);
    egui::Grid::new(format!("periph_grid_{title}"))
        .num_columns(3)
        .spacing([16.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Pin").size(11.0).color(dim));
            ui.label(egui::RichText::new("Name").size(11.0).color(dim));
            ui.label(egui::RichText::new("Function").size(11.0).color(dim));
            ui.end_row();

            for pin in pins {
                ui.label(
                    egui::RichText::new(format!("#{}", pin.number))
                        .size(11.0)
                        .monospace(),
                );
                ui.label(
                    egui::RichText::new(pin.name.as_str())
                        .size(11.0)
                        .monospace()
                        .color(egui::Color32::WHITE),
                );
                ui.label(
                    egui::RichText::new(pin.selected_function.label())
                        .size(11.0)
                        .color(color),
                );
                ui.end_row();
            }
        });

    ui.add_space(2.0);
    ui.separator();
}

