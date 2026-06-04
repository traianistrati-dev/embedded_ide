//! DFU and Flash programming tab.
use crate::dfu::{self, DfuState};
use crate::espflash::{self, EspFlashState};
use crate::openocd::{self, OpenOcdState};
use crate::panels::mcu_module::ToolchainKind;
use eframe::egui;
use egui::TextBuffer;
use egui_phosphor::regular as ph;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
pub fn show_dfu_tab(
    ui: &mut egui::Ui,
    dfu_state: &Arc<Mutex<DfuState>>,
    dfu_log: &Arc<Mutex<Vec<String>>>,
    dfu_programmers: &Arc<Mutex<HashMap<String, dfu::ProgrammerInfo>>>,
    dfu_sel_programmer: &mut String,
    dfu_flash_addr: &mut String,
    openocd_state: &Arc<Mutex<OpenOcdState>>,
    openocd_target_cfg: &mut String,
    espflash_state: &Arc<Mutex<EspFlashState>>,
    espflash_port: &mut String,
    toolchain: &ToolchainKind,
) {
    let state = dfu_state.lock().unwrap().clone();
    let ocd_state = openocd_state.lock().unwrap().clone();
    let esp_state = espflash_state.lock().unwrap().clone();
    let log = dfu_log.lock().unwrap().clone();
    let progs = dfu_programmers.lock().unwrap().clone();

    // Sort programmers: compatible with selected toolchain first, then by name
    // progs.sort_by(|a, b| {
    //     let a_is_stm = matches!(
    //         a.kind,
    //         "DFU Bootloader" | "ST-Link" | "J-Link" | "CMSIS-DAP"
    //     );
    //     let a_is_esp = matches!(a.kind, "USB-Serial" | "ESP32");
    //     let b_is_stm = matches!(
    //         b.kind,
    //         "DFU Bootloader" | "ST-Link" | "J-Link" | "CMSIS-DAP"
    //     );
    //     let b_is_esp = matches!(b.kind, "USB-Serial" | "ESP32");

    //     let a_compatible = match toolchain {
    //         ToolchainKind::RustEmbedded => a_is_stm,
    //         ToolchainKind::EspRust => a_is_esp,
    //         ToolchainKind::SdccC => false,
    //     };
    //     let b_compatible = match toolchain {
    //         ToolchainKind::RustEmbedded => b_is_stm,
    //         ToolchainKind::EspRust => b_is_esp,
    //         ToolchainKind::SdccC => false,
    //     };

    //     // Compatible first, then by name
    //     b_compatible
    //         .cmp(&a_compatible)
    //         .then_with(|| a.name.cmp(&b.name))
    // });

    // Determine selected programmer kind for adaptive config UI
    let sel_kind = progs
        .get(dfu_sel_programmer)
        .map(|p| p.kind.clone())
        .unwrap_or("".to_string());
    let is_swd = matches!(sel_kind.as_str(), "ST-Link" | "J-Link" | "CMSIS-DAP");
    let interface_cfg = openocd::interface_cfg_for_kind(&sel_kind);

    // ── Programmer selector ComboBox ──────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Programmer:")
                .size(10.5)
                .color(egui::Color32::GRAY),
        );

        let combo_label = if progs.is_empty() {
            "— none detected —".to_string()
        } else {
            progs
                .get(dfu_sel_programmer)
                .map(|p| p.combo_label())
                .unwrap_or_else(|| "— select —".to_string())
        };

        egui::ComboBox::from_id_salt("dfu_programmer_selector")
            .selected_text(egui::RichText::new(&combo_label).size(10.5).monospace())
            .width(ui.available_width() - 2.0)
            .show_ui(ui, |ui| {
                if progs.is_empty() {
                    ui.label(
                        egui::RichText::new("No programmer detected. Click 'Scan USB'.")
                            .size(10.5)
                            .color(egui::Color32::GRAY),
                    );
                }
                for (i, (key, p)) in progs.iter().enumerate() {
                    // Determine if this programmer is compatible with the selected toolchain
                    let is_stm_programmer = matches!(
                        p.kind.as_str(),
                        "DFU Bootloader" | "ST-Link" | "J-Link" | "CMSIS-DAP"
                    );
                    let is_esp_programmer = matches!(p.kind.as_str(), "USB-Serial" | "ESP32");

                    let is_compatible = match toolchain {
                        ToolchainKind::RustEmbedded => is_stm_programmer,
                        ToolchainKind::EspRust => is_esp_programmer,
                        ToolchainKind::SdccC => false,
                    };

                    let kind_color = match p.kind.as_str() {
                        "DFU Bootloader" => egui::Color32::from_rgb(100, 200, 255),
                        "ST-Link" => egui::Color32::from_rgb(100, 220, 120),
                        "J-Link" => egui::Color32::from_rgb(220, 180, 60),
                        "CMSIS-DAP" => egui::Color32::from_rgb(180, 140, 220),
                        "USB-Serial" => egui::Color32::from_rgb(200, 160, 100),
                        "ESP32" => egui::Color32::from_rgb(220, 120, 60),
                        _ => egui::Color32::GRAY,
                    };

                    // Disable color for incompatible programmers
                    let display_color = if is_compatible {
                        kind_color
                    } else {
                        egui::Color32::from_gray(80)
                    };

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("[{}] {}", p.kind, p.port))
                                .size(10.0)
                                .monospace()
                                .color(display_color),
                        );
                        ui.add_enabled(
                            is_compatible,
                            egui::SelectableLabel::new(
                                *dfu_sel_programmer == p.port.clone(),
                                egui::RichText::new(format!(
                                    "{}  [{}] {}",
                                    p.name, p.vid_pid, p.port
                                ))
                                .size(10.5)
                                .monospace(),
                            ),
                        )
                        .clicked()
                        .then(|| {
                            if is_compatible {
                                *dfu_sel_programmer = p.port.clone();
                            }
                        });
                    });
                }
            });
    });

    // Guidance text for selected programmer
    if let Some(p) = progs.get(dfu_sel_programmer) {
        let guidance = p.guidance();
        if !guidance.is_empty() {
            let color = match p.kind.as_str() {
                "DFU Bootloader" => egui::Color32::from_rgb(80, 200, 100),
                "ST-Link" | "J-Link" | "CMSIS-DAP" => egui::Color32::from_rgb(180, 180, 100),
                _ => egui::Color32::from_rgb(160, 160, 170),
            };
            for line in guidance.lines() {
                ui.label(egui::RichText::new(line).size(10.0).color(color).italics());
            }
        }
    }

    ui.separator();

    // ── Config row — adaptive: ESP32 / SWD (OpenOCD) / DFU ───────────────────
    ui.horizontal(|ui| {
        let build_done = log.iter().any(|l| l.contains("✔ Build OK"));

        // Helper: render a phase indicator icon + label
        let phase_widget = |ui: &mut egui::Ui, icon: &str, label: &str, color: egui::Color32| {
            ui.label(egui::RichText::new(icon).size(11.5).color(color));
            ui.label(egui::RichText::new(label).size(11.0).color(color));
        };

        if *toolchain == ToolchainKind::EspRust {
            // ── EspRust config ────────────────────────────────────────────────
            ui.label(
                egui::RichText::new("Tool: espflash")
                    .size(10.5)
                    .color(egui::Color32::from_rgb(220, 140, 60)),
            );
            ui.separator();

            // Phases: Build → Flash ESP
            let (b_icon, b_col) = match &esp_state {
                EspFlashState::Building => {
                    (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(220, 180, 60))
                }
                _ if build_done => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                EspFlashState::Error(_) if !build_done && !log.is_empty() => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, b_icon, "Build", b_col);
            ui.label(
                egui::RichText::new("→")
                    .size(11.0)
                    .color(egui::Color32::from_gray(70)),
            );

            let (f_icon, f_col) = match &esp_state {
                EspFlashState::Flashing => {
                    (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(220, 140, 60))
                }
                EspFlashState::Success => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                EspFlashState::Error(_) if build_done => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, f_icon, "Flash ESP32", f_col);
        } else if is_swd {
            // ── SWD / OpenOCD config ──────────────────────────────────────────
            ui.label(
                egui::RichText::new("Interface:")
                    .size(10.5)
                    .color(egui::Color32::GRAY),
            );
            ui.label(
                egui::RichText::new(interface_cfg)
                    .size(10.5)
                    .monospace()
                    .color(egui::Color32::from_rgb(120, 160, 200)),
            )
            .on_hover_text("OpenOCD interface config — auto-selected from programmer type");

            ui.separator();

            ui.label(
                egui::RichText::new("Target:")
                    .size(10.5)
                    .color(egui::Color32::GRAY),
            );
            ui.add(
                egui::TextEdit::singleline(openocd_target_cfg)
                    .desired_width(140.0)
                    .font(egui::TextStyle::Monospace),
            )
            .on_hover_text(
                "OpenOCD target config file.\n\
                 Examples: target/stm32f1x.cfg, target/stm32f4x.cfg\n\
                 Full list in your OpenOCD install under scripts/target/",
            );

            // Quick-select presets for common STM32 families
            for (label, cfg, tip) in [
                ("F1", "target/stm32f1x.cfg", "STM32F1 (F103, F100, F105, …)"),
                ("F4", "target/stm32f4x.cfg", "STM32F4 (F407, F411, F401, …)"),
                ("H7", "target/stm32h7x.cfg", "STM32H7 (H743, H750, …)"),
                ("L4", "target/stm32l4x.cfg", "STM32L4 (L432, L476, L496, …)"),
            ] {
                if ui
                    .add(egui::Button::new(egui::RichText::new(label).size(10.0)))
                    .on_hover_text(tip)
                    .clicked()
                {
                    *openocd_target_cfg = cfg.to_string();
                }
            }

            ui.separator();

            // SWD phases: Build → Flash (no objcopy — OpenOCD programs ELF directly)
            let (b_icon, b_col) = match &ocd_state {
                OpenOcdState::Building => (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(220, 180, 60)),
                _ if build_done => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                OpenOcdState::Error(_) if !build_done && !log.is_empty() => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, b_icon, "Build", b_col);
            ui.label(
                egui::RichText::new("→")
                    .size(11.0)
                    .color(egui::Color32::from_gray(70)),
            );

            let (f_icon, f_col) = match &ocd_state {
                OpenOcdState::Flashing => {
                    (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(100, 180, 255))
                }
                OpenOcdState::Success => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                OpenOcdState::Error(_) if build_done => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, f_icon, "Flash SWD", f_col);
        } else {
            // ── DFU config ────────────────────────────────────────────────────
            ui.label(
                egui::RichText::new("Flash addr:")
                    .size(10.5)
                    .color(egui::Color32::GRAY),
            );
            ui.add(
                egui::TextEdit::singleline(dfu_flash_addr)
                    .desired_width(82.0)
                    .font(egui::TextStyle::Monospace),
            )
            .on_hover_text(
                "Start address passed to dfu-util.\n\
                 0x08000000 — standard (no bootloader)\n\
                 0x08002000 — with 8 KB bootloader offset",
            );

            if ui
                .add(egui::Button::new(egui::RichText::new("Std").size(10.0)))
                .on_hover_text("0x08000000 — standard flash start (no bootloader)")
                .clicked()
            {
                *dfu_flash_addr = "0x08000000".to_string();
            }
            if ui
                .add(egui::Button::new(egui::RichText::new("+BL").size(10.0)))
                .on_hover_text("0x08002000 — 8 KB bootloader offset")
                .clicked()
            {
                *dfu_flash_addr = "0x08002000".to_string();
            }

            ui.separator();

            // DFU phases: Build → Objcopy → Flash
            let objcopy_done = log.iter().any(|l| l.contains("✔ firmware.bin ready"));

            let (b_icon, b_col) = match &state {
                DfuState::Building => (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(220, 180, 60)),
                _ if build_done => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                DfuState::Error(_) if !build_done && !log.is_empty() => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, b_icon, "Build", b_col);
            ui.label(
                egui::RichText::new("→")
                    .size(11.0)
                    .color(egui::Color32::from_gray(70)),
            );

            let (o_icon, o_col) = match &state {
                _ if objcopy_done => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                DfuState::Flashing | DfuState::Success => {
                    (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100))
                }
                DfuState::Error(_) if build_done && !objcopy_done => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                DfuState::Building if build_done => {
                    (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(220, 180, 60))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, o_icon, "Objcopy", o_col);
            ui.label(
                egui::RichText::new("→")
                    .size(11.0)
                    .color(egui::Color32::from_gray(70)),
            );

            let (f_icon, f_col) = match &state {
                DfuState::Flashing => (ph::CIRCLE_NOTCH, egui::Color32::from_rgb(100, 180, 255)),
                DfuState::Success => (ph::CHECK_CIRCLE, egui::Color32::from_rgb(80, 200, 100)),
                DfuState::Error(_) if objcopy_done => {
                    (ph::X_CIRCLE, egui::Color32::from_rgb(220, 80, 70))
                }
                _ => (ph::CIRCLE_NOTCH, egui::Color32::from_gray(70)),
            };
            phase_widget(ui, f_icon, "Flash", f_col);
        }

        // Clear button — right-aligned, always visible, resets both states + log
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(
                    egui::RichText::new(format!("{} Clear", ph::X))
                        .size(10.0)
                        .color(egui::Color32::GRAY),
                ))
                .clicked()
            {
                dfu_log.lock().unwrap().clear();
                *dfu_state.lock().unwrap() = DfuState::Idle;
                *openocd_state.lock().unwrap() = OpenOcdState::Idle;
                *espflash_state.lock().unwrap() = EspFlashState::Idle;
            }
        });
    });
    /*
        // ── ESP32 port selector ───────────────────────────────────────────────────
        // Shown only for EspRust.  Lets the user type a COM port (e.g. "COM3") so
        // espflash targets the correct device when multiple serial ports are present.
        // Leave empty to let espflash auto-detect.
        if *toolchain == ToolchainKind::EspRust {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Port:")
                        .size(11.0)
                        .color(egui::Color32::from_gray(160)),
                );
                let resp = ui.add(
                    egui::TextEdit::singleline(espflash_port)
                        .hint_text("auto (e.g. COM3)")
                        .desired_width(120.0)
                        .font(egui::TextStyle::Monospace),
                );
                resp.on_hover_text(
                    "Serial port for espflash.\n\
                     Leave empty to auto-detect.\n\
                     Windows example: COM3\n\
                     Linux example:   /dev/ttyUSB0\n\
                     macOS example:   /dev/cu.usbserial-0001",
                );
                if !espflash_port.is_empty() {
                    if ui
                        .small_button("✕")
                        .on_hover_text("Clear — use auto-detect")
                        .clicked()
                    {
                        espflash_port.clear();
                    }
                }
                ui.label(
                    egui::RichText::new("← leave empty for auto-detect")
                        .size(10.0)
                        .color(egui::Color32::from_gray(100))
                        .italics(),
                );
            });
            ui.add_space(2.0);
        }
    */
    // ── ESP32 diagnostic row (read-only chip identification) ─────────────────
    // Shown only for EspRust; sits between the config row and the log area.
    if *toolchain == ToolchainKind::EspRust {
        ui.horizontal(|ui| {
            let busy = esp_state.is_busy();
            let reading = matches!(esp_state, EspFlashState::ReadingInfo);

            let btn_color = if reading {
                egui::Color32::from_rgb(100, 180, 255)
            } else if busy {
                egui::Color32::GRAY
            } else {
                egui::Color32::from_rgb(100, 180, 255)
            };

            let btn_label = if reading {
                format!("{} Reading…", ph::CIRCLE_NOTCH)
            } else {
                format!("{} Read Chip Info", ph::INFO)
            };

            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(egui::RichText::new(&btn_label).size(10.5).color(btn_color)),
                )
                .on_hover_text(
                    "Run `espflash board-info` — connects to the chip and reads:\n\
                     chip type, silicon revision, flash size, MAC address.\n\
                     \n\
                     Nothing is written to flash — safe to run at any time.\n\
                     Use this to verify the USB cable / COM port works before flashing.",
                )
                .clicked()
            {
                espflash::read_board_info(
                    Arc::clone(espflash_state),
                    Arc::clone(dfu_log),
                    ui.ctx().clone(),
                );
            }

            ui.label(
                egui::RichText::new("← verify connectivity without flashing")
                    .size(10.0)
                    .color(egui::Color32::from_gray(100))
                    .italics(),
            );
        });
    }

    ui.separator();

    // ── DFU Error / Success banners ───────────────────────────────────────────
    /*
        if let DfuState::Error(msg) = &state {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(ph::X_CIRCLE)
                        .size(13.0)
                        .color(egui::Color32::from_rgb(220, 80, 70)),
                );
                ui.label(
                    egui::RichText::new(format!("DFU: {}", msg.lines().next().unwrap_or("Error")))
                        .size(11.0)
                        .color(egui::Color32::from_rgb(220, 80, 70))
                        .strong(),
                );
            });
            ui.separator();
        }
        if matches!(state, DfuState::Success) {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(ph::CHECK_CIRCLE)
                        .size(13.0)
                        .color(egui::Color32::from_rgb(80, 200, 100)),
                );
                ui.label(
                    egui::RichText::new("Device programmed via DFU successfully!")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(80, 200, 100))
                        .strong(),
                );
            });
            ui.separator();
        }
    */
    // ── OpenOCD Error / Success banners ───────────────────────────────────────
    if let OpenOcdState::Error(msg) = &ocd_state {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ph::X_CIRCLE)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(220, 80, 70)),
            );
            ui.label(
                egui::RichText::new(format!("SWD: {}", msg.lines().next().unwrap_or("Error")))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(220, 80, 70))
                    .strong(),
            );
        });
        ui.separator();
    }
    if matches!(ocd_state, OpenOcdState::Success) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ph::CHECK_CIRCLE)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(80, 200, 100)),
            );
            ui.label(
                egui::RichText::new("Device programmed via SWD successfully!")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(80, 200, 100))
                    .strong(),
            );
        });
        ui.separator();
    }

    // ── ESP32 flash Error / Success banners ───────────────────────────────────
    if let EspFlashState::Error(msg) = &esp_state {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ph::X_CIRCLE)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(220, 80, 70)),
            );
            ui.label(
                egui::RichText::new(format!("ESP: {}", msg.lines().next().unwrap_or("Error")))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(220, 80, 70))
                    .strong(),
            );
        });
        ui.separator();
    }
    if matches!(esp_state, EspFlashState::Success) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ph::CHECK_CIRCLE)
                    .size(13.0)
                    .color(egui::Color32::from_rgb(80, 200, 100)),
            );
            ui.label(
                egui::RichText::new("ESP32 programmed successfully!")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(80, 200, 100))
                    .strong(),
            );
        });
        ui.separator();
    }

    // ── Scrollable log (shared between DFU and OpenOCD operations) ────────────
    egui::ScrollArea::vertical()
        .id_salt("dfu_log_scroll")
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if log.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "No output yet.\n\
                         • Flash USB    — DFU mode (STM32 with BOOT0 = 1)\n\
                         • Flash SWD    — ST-Link / J-Link via OpenOCD\n\
                         • Flash ESP32  — espflash (ESP32-C3 via USB-Serial)",
                    )
                    .size(11.0)
                    .color(egui::Color32::GRAY),
                );
                return;
            }

            for line in &log {
                // Colour-code lines by content prefix.
                //
                // IMPORTANT: cargo prints "   Compiling proc-macro-error-attr2"
                // and "   Compiling proc-macro-error2" which contain the word
                // "error" inside the crate name — NOT an actual build error.
                // Real cargo errors always start with "error" at column 0
                // (e.g. "error[E0308]: …" or "error: …").
                // Warnings start with "warning" at column 0.
                // Using starts_with() avoids false positives from crate names.
                let trimmed = line.trim_start();
                let color = if line.starts_with("✔") {
                    egui::Color32::from_rgb(80, 200, 100) // green  — success
                } else if line.starts_with("▶") {
                    egui::Color32::from_rgb(100, 180, 255) // blue   — command header
                } else if trimmed.starts_with("error[") || trimmed.starts_with("error:") {
                    egui::Color32::from_rgb(220, 100, 80) // red    — real compile error
                } else if trimmed.starts_with("warning[") || trimmed.starts_with("warning:") {
                    egui::Color32::from_rgb(210, 170, 40) // yellow — compile warning
                } else if trimmed.starts_with("Compiling ")
                    || trimmed.starts_with("Finished ")
                    || trimmed.starts_with("Running ")
                {
                    egui::Color32::from_rgb(130, 170, 130) // muted green — progress
                } else {
                    egui::Color32::from_rgb(175, 180, 192) // grey   — normal output
                };
                ui.label(
                    egui::RichText::new(line.as_str())
                        .size(10.5)
                        .monospace()
                        .color(color),
                );
            }
        });
}
