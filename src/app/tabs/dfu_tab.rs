//! DFU and Flash programming tab.
use super::cargo_tab::render_size_bar;
use crate::dfu::{self, DfuState};
use crate::espflash::{self, EspFlashState};
use crate::openocd::{self, OpenOcdState};
use crate::panels::mcu_module::ToolchainKind;
use crate::size::SizeState;
use eframe::egui;
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
    //espflash_port: &mut String,
    _: &mut String,
    toolchain: &ToolchainKind,
    // Set true when the user clicks Scan / Flash on the Programmer row; the
    // caller runs `AppIde::scan_usb` / `flash_swd` / `flash_esp`. `can_flash`
    // gates the Flash button (a buildable chip config exists).
    scan_out: &mut bool,
    flash_out: &mut bool,
    can_flash: bool,
    // Flash/RAM usage: the row is rendered under the Programmer row and is
    // refreshed automatically after every flash. `size_out` = the manual Size
    // button (the caller runs `start_size_measure_quiet`, which keeps this tab
    // in front instead of jumping to Cargo).
    size_state: &Arc<Mutex<SizeState>>,
    size_out: &mut bool,
    // Shared probe-rs probe (same as Debug / RTT / Runtime) + its `cargo flash`
    // path: `probe_flash_out` fires the flash, `probe_scan` re-runs `probe-rs
    // list`, `probe_flash_state` is the status.
    probe_list: &[crate::probe::ProbeInfo],
    selected_probe: &mut Option<String>,
    probe_scan: &mut bool,
    probe_scan_err: Option<&str>,
    probe_flash_state: &Arc<Mutex<crate::probe_flash::ProbeFlashState>>,
    probe_flash_out: &mut bool,
    // Which probe-rs session currently OWNS the probe ("Debug", "RTT", …), if
    // any. Only one process can hold it, so every flasher is blocked while one
    // runs — and the button says so instead of failing deep inside the tool.
    probe_holder: Option<&'static str>,
    // Tools confirmed missing — buttons needing one are greyed out with a
    // "install it in Tools" hint (see `super::tool_missing`).
    missing_tools: &[&'static str],
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

    let dfu_busy = state.is_busy();
    let any_busy = dfu_busy || ocd_state.is_busy() || esp_state.is_busy();
    let size_snapshot = size_state.lock().unwrap().clone();
    let size_busy = size_snapshot.is_busy();

    // ── What blocks each Flash button right now ──────────────────────────────
    // Computed before the rows: the same sentence colours the button red, fills
    // its disabled hover, and is listed as a notice below — a greyed button that
    // won't say why is the thing this replaces.
    let pf_state = probe_flash_state.lock().unwrap().clone();
    // One process at a time owns a probe. A live Debug / RTT / sampling session
    // makes every flasher fail deep inside the tool with "probe in use".
    let held = |needs: &str| {
        probe_holder.map(|h| {
            format!("the {h} session is holding the probe — stop it first, {needs} needs exclusive access")
        })
    };
    let no_cfg = || {
        (!can_flash).then(|| "no buildable chip configuration yet — set the MCU up first".to_owned())
    };
    let busy_note = || any_busy.then(|| "another flash is already running".to_owned());
    let swd_reason: Option<String> = held("OpenOCD")
        .or_else(no_cfg)
        .or_else(busy_note)
        .or_else(|| {
            (!is_swd).then(|| {
                "pick an ST-Link / J-Link / CMSIS-DAP in the Programmer list (Scan USB finds them)"
                    .to_owned()
            })
        });
    let esp_reason: Option<String> = super::tool_missing(missing_tools, "espflash")
        .then(|| super::needs_tool_hint("espflash"))
        .or_else(no_cfg)
        .or_else(busy_note);
    let probe_reason: Option<String> = super::tool_missing(missing_tools, "probe-rs")
        .then(|| super::needs_tool_hint("probe-rs"))
        .or_else(|| held("cargo flash"))
        .or_else(no_cfg)
        .or_else(busy_note)
        .or_else(|| pf_state.is_busy().then(|| "a probe-rs flash is running".to_owned()));

    // ── Two rows, aligned: programmer / probe on the left, memory on the right ─
    let right_w = (ui.available_width() * 0.42).clamp(190.0, 380.0);

    // Row 1 — the USB programmer (OpenOCD SWD / DFU / espflash) + Flash usage.
    split_row(
        ui,
        right_w,
        |ui| {
            ui.label(
                egui::RichText::new("SWD Programmer:")
                    .size(10.5)
                    .color(egui::Color32::GRAY),
            );

            // Scan USB (detect DFU / ST-Link / J-Link / CMSIS-DAP / USB-serial).
            if ui
                .add_enabled(
                    !dfu_busy,
                    egui::Button::new(
                        egui::RichText::new(format!("{} Scan USB", ph::MAGNIFYING_GLASS))
                            .size(10.5),
                    ),
                )
                .on_hover_text(
                    "Scan for connected USB programmers:\n\
                     - DFU bootloader (STM32 with BOOT0 = 1)\n\
                     - ST-Link / J-Link / CMSIS-DAP\n\
                     - USB-Serial (ESP32-C3, ...)",
                )
                .clicked()
            {
                *scan_out = true;
            }

            // Toolchain-specific Flash button.
            match toolchain {
                ToolchainKind::RustEmbedded => {
                    if flash_button(
                        ui,
                        format!("{} Flash SWD", ph::LIGHTNING),
                        swd_reason.as_deref(),
                        "Build --release, then program via SWD (OpenOCD).\n\
                         Needs: OpenOCD in PATH, a selected ST-Link/J-Link/CMSIS-DAP,\n\
                         the target .cfg below, and SWDIO + SWCLK + GND wiring.",
                    ) {
                        *flash_out = true;
                    }
                }
                ToolchainKind::EspRust => {
                    if flash_button(
                        ui,
                        format!("{} Flash ESP32", ph::LIGHTNING),
                        esp_reason.as_deref(),
                        "Build --release, then flash via espflash.\n\
                         Needs: espflash in PATH, the ESP32 in download mode\n\
                         (hold BOOT -> press RESET -> release BOOT).",
                    ) {
                        *flash_out = true;
                    }
                }
                ToolchainKind::SdccC => {}
            }

            programmer_combo(ui, &progs, dfu_sel_programmer, toolchain);
        },
        |ui| {
            render_size_bar(ui, &size_snapshot, true);
            // ── Size — same measurement as the Cargo tab's button ─────────
            // It also runs on its own after every flash; this is for
            // re-checking without programming the board.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let size_enabled = !size_busy && !any_busy && can_flash;
                if ui
                    .add_enabled(
                        size_enabled,
                        egui::Button::new(
                            egui::RichText::new(if size_busy {
                                "Measuring…".to_owned()
                            } else {
                                format!("{} Size", ph::RULER)
                            })
                            .size(10.5)
                            .color(if size_enabled {
                                egui::Color32::from_rgb(120, 170, 210)
                            } else {
                                egui::Color32::GRAY
                            }),
                        ),
                    )
                    .on_hover_text(
                        "Measure Flash/RAM usage: `cargo build --release`, then read \
                         the ELF section sizes against the memory.x limits.\n\
                         Runs automatically after every flash.",
                    )
                    .clicked()
                {
                    *size_out = true;
                }
            });
        },
    );

    // Row 2 — the probe-rs probe (shared with Debug / RTT) + RAM usage.
    split_row(
        ui,
        right_w,
        |ui| {
            if matches!(toolchain, ToolchainKind::RustEmbedded) {
                super::probe_selector_ui_with(
                    ui,
                    "RS Probe:",
                    probe_list,
                    selected_probe,
                    probe_scan,
                    probe_scan_err,
                    toolchain,
                    |ui| {
                        if flash_button(
                            ui,
                            format!("{} Flash (probe-rs)", ph::LIGHTNING),
                            probe_reason.as_deref(),
                            "Build --release, then flash over the selected debug probe with \
                             probe-rs (`cargo flash`). Uses the SAME probe as the Debug / \
                             RTT / Runtime tabs. Needs probe-rs-tools in PATH.",
                        ) {
                            *probe_flash_out = true;
                        }
                    },
                );
                // Status of the last/ongoing probe-rs flash.
                if !matches!(pf_state, crate::probe_flash::ProbeFlashState::Idle) {
                    ui.label(
                        egui::RichText::new(pf_state.label())
                            .size(10.5)
                            .color(pf_state.color()),
                    );
                    if pf_state.is_busy() {
                        ui.spinner();
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(120));
                    }
                }
            } else {
                ui.label(
                    egui::RichText::new("RS Probe: — probe-rs path is for the ARM toolchain")
                        .size(10.5)
                        .color(egui::Color32::from_gray(110)),
                );
            }
        },
        |ui| {
            render_size_bar(ui, &size_snapshot, false);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                crate::app::helpers::help_panel::toggle_button_with(
                    ui,
                    INFO_ID,
                    "Info",
                    "How the flashing paths differ, and what each programmer kind can do",
                );
            });
        },
    );
    if size_busy {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    }

    // ── Why a Flash button is red ────────────────────────────────────────────
    for (what, reason) in [
        (
            match toolchain {
                ToolchainKind::EspRust => "Flash ESP32",
                _ => "Flash SWD",
            },
            match toolchain {
                ToolchainKind::EspRust => &esp_reason,
                _ => &swd_reason,
            },
        ),
        ("Flash (probe-rs)", &probe_reason),
    ] {
        // The probe-rs row doesn't exist off the ARM toolchain.
        if what == "Flash (probe-rs)" && !matches!(toolchain, ToolchainKind::RustEmbedded) {
            continue;
        }
        if matches!(toolchain, ToolchainKind::SdccC) {
            continue;
        }
        if let Some(r) = reason {
            ui.label(
                egui::RichText::new(format!("{} {what}: {r}.", ph::WARNING))
                    .size(10.0)
                    .color(egui::Color32::from_rgb(230, 110, 100)),
            );
        }
    }

    // ── Info panel (toggled on the right of the Probe row) ───────────────────
    flash_info_panel(ui, progs.get(dfu_sel_programmer));

    // The old per-programmer guidance line lived here; it is part of the Info
    // panel now (it explained the DFU/SWD split, which the table covers).
    let _ = &sel_kind;

    ui.separator();
    // ── Config row — adaptive: ESP32 / SWD (OpenOCD) / DFU ───────────────────
    ui.horizontal(|ui| {
        let build_done = log.iter().any(|l| l.contains("[OK] Build OK"));

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
                egui::RichText::new(ph::ARROW_RIGHT)
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
                egui::RichText::new(ph::ARROW_RIGHT)
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
                    .desired_width(90.0)
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
            let objcopy_done = log.iter().any(|l| l.contains("[OK] firmware.bin ready"));

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
                egui::RichText::new(ph::ARROW_RIGHT)
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
                egui::RichText::new(ph::ARROW_RIGHT)
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
                        .small_button(ph::X)
                        .on_hover_text("Clear — use auto-detect")
                        .clicked()
                    {
                        espflash_port.clear();
                    }
                }
                ui.label(
                    egui::RichText::new(format!("{} leave empty for auto-detect", ph::ARROW_LEFT))
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
                    dfu_sel_programmer.clone(),
                );
            }

            ui.label(
                egui::RichText::new(format!(
                    "{} verify connectivity without flashing",
                    ph::ARROW_LEFT
                ))
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
                let color = if line.starts_with("[OK]") {
                    egui::Color32::from_rgb(80, 200, 100) // green  — success
                } else if line.starts_with(">") {
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

/// Memory key of this tab's Info panel.
const INFO_ID: &str = "flash";

/// A Flash button that says WHY it can't run: `reason` is `None` when it can,
/// otherwise the text goes red (bright, because egui draws a disabled widget
/// faded) and the sentence becomes its disabled hover. Returns true on click.
fn flash_button(ui: &mut egui::Ui, label: String, reason: Option<&str>, hover: &str) -> bool {
    let enabled = reason.is_none();
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).size(10.5).color(if enabled {
            egui::Color32::from_rgb(255, 165, 50)
        } else {
            egui::Color32::from_rgb(255, 90, 80)
        })),
    )
    .on_hover_text(hover)
    .on_disabled_hover_text(reason.unwrap_or_default())
    .clicked()
}

/// One row split in two: the left block takes what is left, the right block a
/// fixed width. Both rows use the same `right_w`, so the separators — and the
/// Flash/RAM indicators behind them — line up in one column, which a plain
/// `ui.horizontal` can't do.
fn split_row(
    ui: &mut egui::Ui,
    right_w: f32,
    left: impl FnOnce(&mut egui::Ui),
    right: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        let h = ui.spacing().interact_size.y;
        // Never demand more than the panel has: content wider than the region
        // re-widens the surrounding side panel every frame.
        let right_w = right_w.min((ui.available_width() - 60.0).max(0.0));
        let left_w = (ui.available_width() - right_w - 12.0).max(60.0);
        // Wrapping, not a plain row: a narrow panel makes the buttons + combo
        // spill onto a second line instead of demanding width the region hasn't
        // got (which would re-widen the editor side panel every frame).
        ui.allocate_ui_with_layout(
            egui::vec2(left_w, h),
            egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
            |ui| {
                ui.set_max_width(left_w);
                left(ui);
            },
        );
        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2(right_w, h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_max_width(right_w);
                right(ui);
            },
        );
    });
}

/// The programmer ComboBox (USB scan results), split out of the row so the row
/// itself stays readable.
fn programmer_combo(
    ui: &mut egui::Ui,
    progs: &HashMap<String, dfu::ProgrammerInfo>,
    dfu_sel_programmer: &mut String,
    toolchain: &ToolchainKind,
) {
    {
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
            // No floor: a width bigger than the region re-widens the editor
            // side panel every frame (the panel-growth rule).
            .width((ui.available_width() - 2.0).max(0.0))
            .height(progs.len() as f32 * 30.0)
            .show_ui(ui, |ui| {
                if progs.is_empty() {
                    ui.label(
                        egui::RichText::new("No programmer detected. Click 'Scan USB'.")
                            .size(10.5)
                            .color(egui::Color32::GRAY),
                    );
                }
                for (key, p) in progs.iter() {
                    // Determine if this programmer is compatible with the selected toolchain
                    let is_stm_programmer = matches!(
                        p.kind.as_str(),
                        "DFU Bootloader" | "ST-Link" | "STLink" | "STM" | "J-Link" | "CMSIS-DAP"
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
                            egui::Button::selectable(
                                dfu_sel_programmer == key,
                                egui::RichText::new(format!(
                                    "{}  [{}] {}; {}",
                                    p.name, p.vid_pid, p.port, p.extra_details
                                ))
                                .size(10.5)
                                .monospace(),
                            ),
                        )
                        .clicked()
                        .then(|| {
                            if is_compatible {
                                *dfu_sel_programmer = key.clone();
                            }
                        });
                    });
                }
            });
    }
}

/// The Info panel: how the flashing paths differ, side by side, plus what the
/// SELECTED programmer kind can actually do (the old per-programmer guidance
/// line, which used to sit under the row claiming SWD needed an external
/// OpenOCD — this tab has had a button for it for a while).
fn flash_info_panel(ui: &mut egui::Ui, selected: Option<&dfu::ProgrammerInfo>) {
    crate::app::helpers::help_panel::custom_panel(ui, INFO_ID, |ui| {
        let head = |ui: &mut egui::Ui, t: &str| {
            ui.label(
                egui::RichText::new(t)
                    .size(11.0)
                    .strong()
                    .color(egui::Color32::from_rgb(200, 210, 230)),
            );
        };
        let cell = |ui: &mut egui::Ui, t: &str| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(t)
                        .size(10.5)
                        .color(egui::Color32::from_gray(195)),
                )
                .wrap(),
            );
        };
        egui::Grid::new("flash_info_table")
            .num_columns(3)
            .spacing([12.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                head(ui, "");
                head(ui, "Flash SWD");
                head(ui, "Flash (probe-rs)");
                ui.end_row();
                for (aspect, swd, prs) in FLASH_COMPARISON {
                    ui.label(
                        egui::RichText::new(*aspect)
                            .size(10.5)
                            .color(egui::Color32::from_gray(150)),
                    );
                    cell(ui, swd);
                    cell(ui, prs);
                    ui.end_row();
                }
            });
        ui.add_space(6.0);
        for note in FLASH_NOTES {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{} {note}", ph::DOT))
                        .size(10.5)
                        .color(egui::Color32::from_gray(140)),
                )
                .wrap(),
            );
        }
        // What the programmer picked in the row above is good for.
        if let Some(p) = selected {
            let guidance = p.guidance();
            if !guidance.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("Selected programmer — {} ({})", p.name, p.kind))
                        .size(11.0)
                        .strong()
                        .color(egui::Color32::from_rgb(200, 210, 230)),
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(guidance)
                            .size(10.5)
                            .color(egui::Color32::from_gray(180)),
                    )
                    .wrap(),
                );
            }
        }
    });
}

/// `(aspect, Flash SWD, Flash (probe-rs))` — the comparison table in the Info
/// panel. Both write over the same SWD wires; what differs is the tool driving
/// them and where each gets its configuration.
const FLASH_COMPARISON: &[(&str, &str, &str)] = &[
    ("Tool", "OpenOCD", "probe-rs (`cargo flash`)"),
    (
        "Command",
        "openocd -f <interface.cfg> -f <target.cfg> -c \"program <elf> verify reset exit\"",
        "cargo flash --release --chip <chip> --target <triplet> [--probe VID:PID:Serial]",
    ),
    (
        "Adapter comes from",
        "the Programmer row above — the USB scan (ST-Link / J-Link / CMSIS-DAP)",
        "the Probe row — the SAME probe as the Debug, RTT and Profile tabs",
    ),
    (
        "Chip identity comes from",
        "the target .cfg you type below (e.g. target/stm32f1x.cfg)",
        "the project's probe-rs chip name (e.g. STM32F103C8)",
    ),
    ("Needs installed", "OpenOCD in PATH", "probe-rs-tools in PATH"),
    (
        "Use it when",
        "probe-rs doesn't know your chip, or you need OpenOCD-specific adapter \
         settings (speed, reset sequence, exotic interfaces)",
        "normally — one probe identity across flashing, debugging and logging",
    ),
];

/// Notes under the comparison table.
const FLASH_NOTES: &[&str] = &[
    "Both build `cargo build --release` first, and both refresh the Flash/RAM \
     measurement on the right afterwards.",
    "Only ONE process can hold a probe: stop a Debug or RTT session before \
     flashing, or the flash fails with \"probe in use\". A Flash button turns RED \
     with the reason when that is the case.",
    "DFU is a third path and needs no probe at all: the MCU's own USB bootloader \
     (BOOT0 = 1 + reset) writes the .bin at the Flash address set below. That is \
     what the DFU-specific fields are for.",
    "Flash ESP32 (espflash) replaces Flash SWD on the ESP toolchain: it goes over \
     the serial port with the board in download mode, not over SWD.",
];

