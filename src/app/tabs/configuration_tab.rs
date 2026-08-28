//! The **Configuration** tab: peripherals that have no pin.
//!
//! Everything on the Pins and Peripherals tabs is reached through a pin. A
//! watchdog is not — it is a box you switch on and give a duration to — so it
//! needs a home of its own rather than a row that pretends to be a pin function.
//!
//! # Durations, not register fields
//!
//! CubeMX shows prescaler / window value / downcounter. embassy takes
//! `new(peri, timeout_us[, window_us])` and derives the registers itself from
//! the live PCLK1, so those fields cannot be generated even if they were shown.
//! The tab therefore edits time, and spends its effort on the thing time cannot
//! tell you by itself: whether the chip can actually express it. See
//! [`crate::panels::mcu_module::watchdog`].

use crate::app::AppIde;
use crate::panels::mcu_module::comparator::{self, CompConfig, CompSettings};
use crate::panels::mcu_module::watchdog::{
    self as wdg, EspWatchdogLimits, EspWdtConfig, IwdgConfig, WatchdogLimits, WwdgConfig,
};
use eframe::egui;
use egui_phosphor::regular as ph;

/// Muted body text, matching the other tabs' explanatory lines.
fn dim(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(11.0)
        .color(egui::Color32::from_rgb(130, 130, 145))
}

/// A microsecond count as something a person can read at a glance.
///
/// Watchdog periods span five orders of magnitude — 125 µs to 132 s — so a
/// single unit is unreadable at one end or the other.
fn human_us(us: u32) -> String {
    if us >= 1_000_000 {
        format!("{:.3} s", us as f64 / 1_000_000.0)
    } else if us >= 1_000 {
        format!("{:.3} ms", us as f64 / 1_000.0)
    } else {
        format!("{us} us")
    }
}

impl AppIde {
    /// Render the Configuration tab. Called only with a chip selected.
    pub(in crate::app) fn show_configuration_tab(&mut self, ui: &mut egui::Ui) {
        let Some(mcu) = &mut self.mcu else { return };
        let family = mcu.family.clone();
        let limits = wdg::limits_for(&family);
        // The WWDG's whole range is relative to PCLK1, so the Clock tab feeds
        // this one. 0 = no clock model → the range is unknowable, and the tab
        // says so rather than inventing one.
        let pclk1 = match &mcu.clock {
            crate::panels::mcu_module::clock::ClockConfig::Graph(gc) => {
                crate::panels::mcu_module::clock::graph::evaluate(&gc.graph)
                    .get("pclk1")
                    .copied()
                    .unwrap_or(0)
            }
            // No clock graph (F1's own model, or a chip with none): the WWDG
            // range is unknowable and the card says so instead of guessing.
            _ => 0,
        };

        // Straight from the code generator - see `family::dma_uses`. Re-run per
        // frame rather than cached: it is a few string formats, and a cache is
        // exactly how a list starts describing an allocation the project no
        // longer has.
        let uses = crate::panels::mcu_module::codegen::family::dma_uses(mcu);
        // Same rule, same reason: straight from codegen, per frame, never cached.
        let pio = crate::panels::mcu_module::codegen::family::pio_uses(mcu);
        // Only the RP parts have a PIO; elsewhere the card is absent, not empty.
        let has_pio = crate::panels::mcu_module::codegen::rp::is_rp(&family);
        // Whether this board can ever put anything ON it, which is what tells an
        // empty list apart from a chip that has nothing to put there.
        let has_radio_pad = mcu.iter_all_pins().any(|p| p.name == "WL_LED");
        // Whether DMA is even reachable from here, which is what an empty list
        // means most of the time.
        let is_async = matches!(
            mcu.runtime,
            crate::panels::mcu_module::mcu::model::Runtime::Async
        );
        let on_dma_runtime = match mcu.runtime {
            crate::panels::mcu_module::mcu::model::Runtime::Async => true,
            crate::panels::mcu_module::mcu::model::Runtime::Blocking => family == "stm32f1",
            _ => false,
        };

        // The comparators the CHIP has, each with whatever pins are wired for
        // it. Collected before the closure borrows `mcu.comp` mutably.
        let comps: Vec<(u8, Option<String>, Option<String>)> = comparator::instances(mcu)
            .into_iter()
            .map(|n| {
                (
                    n,
                    comparator::wired_pin(mcu, n, "INP"),
                    comparator::wired_pin(mcu, n, "INM"),
                )
            })
            .collect();
        let comp_gen = comparator::Generation::of(&family);

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            ui.label(dim(
                "Peripherals with no pin of their own: which DMA channels the project \
                 uses, and the watchdogs.",
            ));
            ui.add_space(10.0);

            dma_card(
                ui,
                &uses,
                mcu.dma.as_ref(),
                &family,
                on_dma_runtime,
                has_pio.then(|| crate::panels::mcu_module::codegen::rp::dma_channels(&family)),
            );
            ui.add_space(12.0);

            if has_pio {
                pio_card(ui, &pio, &family, is_async, has_radio_pad);
                ui.add_space(12.0);
            }

            comp_cards(ui, &mut mcu.comp, &comps, comp_gen, &family, is_async);
            ui.add_space(12.0);

            let esp = wdg::is_esp(&family);
            ui.label(dim(if esp {
                "Watchdog values are durations - esp-hal works out the prescaler and \
                 counter from them at run time, against the clock as it then is."
            } else {
                "Watchdog values are durations - the HAL derives the prescaler and \
                 counter from them, so what matters here is whether the chip can reach \
                 the time you ask for."
            }));
            ui.add_space(10.0);

            // Not wired for every backend yet: better to say so than to let
            // the cards accept settings that reach no generated file.
            if !wdg::codegen_supported(&family) {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  Watchdog code generation is not implemented for this family yet ({family}). The settings below would not reach the project.",
                        ph::WARNING
                    ))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(235, 150, 90)),
                );
                return;
            }

            if esp {
                // A different chip's watchdogs entirely — different names,
                // different clocks, different lifecycles. Sharing the IWDG card
                // and relabelling it would have been the shorter change and a
                // lie: the ESP has no window watchdog and no boot panic to
                // protect against.
                let el = wdg::esp_limits_for(&family);
                esp_wdt_card(
                    ui,
                    &mut mcu.watchdog.rwdt,
                    "RWDT",
                    "RTC watchdog",
                    wdg::rwdt_range_us(&el),
                    &format!(
                        "In the RTC power domain, counting on the RTC SLOW clock \
                         (~{} kHz), so its period does NOT move with the Clock tab. That \
                         clock is an RC oscillator esp-hal calibrates at boot, so the \
                         period is not crystal-accurate either.",
                        el.rtc_slow_hz / 1000
                    ),
                );
                ui.add_space(12.0);
                esp_mwdt_card(ui, &mut mcu.watchdog.mwdt0, 0, &el, &family);
                ui.add_space(12.0);
                esp_mwdt_card(ui, &mut mcu.watchdog.mwdt1, 1, &el, &family);
            } else {
                iwdg_card(ui, &mut mcu.watchdog.iwdg, &limits);
                ui.add_space(12.0);
                wwdg_card(ui, &mut mcu.watchdog.wwdg, &limits, pclk1, &family);
            }
            ui.add_space(10.0);
        });
    }
}

/// One duration row: a drag value in microseconds plus its human reading.
fn duration_row(ui: &mut egui::Ui, label: &str, value: &mut u32, range: (u32, u32)) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::DragValue::new(value)
                // Clamped to what the chip can express, so the common way of
                // reaching an invalid value is simply not available. Typing one
                // still is, which is what the warning below is for.
                .range(range.0..=range.1)
                .speed((range.1 as f64 - range.0 as f64) / 500.0)
                .suffix(" us"),
        );
        ui.label(dim(human_us(*value)));
    });
}

fn iwdg_card(ui: &mut egui::Ui, cfg: &mut Option<IwdgConfig>, l: &WatchdogLimits) {
    let (lo, hi) = wdg::iwdg_range_us(l);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        let mut on = cfg.is_some();
        ui.horizontal(|ui| {
            if ui.checkbox(&mut on, "").changed() {
                *cfg = on.then(|| IwdgConfig::default_for(l));
            }
            ui.label(egui::RichText::new(format!("{}  IWDG", ph::SHIELD)).strong());
            ui.label(dim("independent watchdog"));
        });
        ui.label(dim(format!(
            "Runs off the LSI ({} kHz), so its period does NOT move with the Clock tab. \
             Range {} .. {}.",
            l.lsi_hz / 1000,
            human_us(lo),
            human_us(hi)
        )));
        let Some(c) = cfg.as_mut() else { return };
        ui.add_space(6.0);
        duration_row(ui, "Period", &mut c.timeout_us, (lo, hi));
        ui.add_space(4.0);
        ui.label(dim(
            "Configured but NOT started: the generated code calls unleash() nowhere, \
             so you choose when it starts biting.",
        ));
        problem_and_reset(ui, wdg::iwdg_problem(c, l), || {
            *c = IwdgConfig::default_for(l)
        });
    });
}

fn wwdg_card(
    ui: &mut egui::Ui,
    cfg: &mut Option<WwdgConfig>,
    l: &WatchdogLimits,
    pclk1: u32,
    family: &str,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        // Shown disabled rather than hidden: a control that vanishes on one chip
        // is harder to understand than one that says why it cannot be used.
        if !wdg::wwdg_supported(family) {
            ui.horizontal(|ui| {
                ui.add_enabled(false, egui::Checkbox::new(&mut false, ""));
                ui.label(
                    egui::RichText::new(format!("{}  WWDG", ph::SHIELD))
                        .strong()
                        .color(egui::Color32::GRAY),
                );
                ui.label(dim("window watchdog"));
            });
            ui.label(dim(
                "Not available on this chip: the STM32F1 HAL (stm32f1xx-hal) implements \
                 only the independent watchdog. Every embassy family has both.",
            ));
            return;
        }
        let range = wdg::wwdg_range_us(l, pclk1);
        let mut on = cfg.is_some();
        ui.horizontal(|ui| {
            let can_enable = range.is_some();
            if ui
                .add_enabled(can_enable, egui::Checkbox::new(&mut on, ""))
                .on_disabled_hover_text(
                    "Set the system clock first - every WWDG period is relative to PCLK1",
                )
                .changed()
            {
                *cfg = on.then(|| WwdgConfig::default_for(l, pclk1)).flatten();
            }
            ui.label(egui::RichText::new(format!("{}  WWDG", ph::SHIELD)).strong());
            ui.label(dim("window watchdog"));
        });
        match range {
            Some((lo, hi)) => ui.label(dim(format!(
                "Counts down from PCLK1 ({} MHz), so its period moves with the Clock tab. \
                 Range {} .. {}.",
                pclk1 / 1_000_000,
                human_us(lo),
                human_us(hi)
            ))),
            None => ui.label(dim(
                "PCLK1 is unknown - configure the Clock tab and the achievable range appears here.",
            )),
        };
        let (Some(c), Some((lo, hi))) = (cfg.as_mut(), range) else {
            return;
        };
        ui.add_space(6.0);
        duration_row(ui, "Period", &mut c.timeout_us, (lo, hi));
        // The window may be zero (no restriction) but never as long as the
        // period; the driver asserts strictly less.
        duration_row(ui, "Closed window", &mut c.window_us, (0, hi));
        ui.add_space(4.0);
        ui.label(dim(
            "Starts immediately and cannot be stopped. Petting DURING the closed window \
             resets the chip, exactly like petting too late - leave it at 0 unless you \
             mean it.",
        ));
        problem_and_reset(ui, wdg::wwdg_problem(c, l, pclk1), || {
            if let Some(d) = WwdgConfig::default_for(l, pclk1) {
                *c = d;
            }
        });
    });
}

/// One ESP watchdog: a checkbox, a period, and what it is clocked from.
///
/// Simpler than [`iwdg_card`] by one whole concern. On an STM32 an unreachable
/// duration is a **panic at boot**, so that card exists largely to keep the
/// user inside a range; `esp-hal`'s `set_timeout` cannot fail, and the counters
/// outrun the microsecond `u32` this tab counts in. So the range shown here is
/// a floor of one tick and a ceiling that belongs to the tab, not the chip.
fn esp_wdt_card(
    ui: &mut egui::Ui,
    cfg: &mut Option<EspWdtConfig>,
    title: &str,
    subtitle: &str,
    range: (u32, u32),
    note: &str,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        let mut on = cfg.is_some();
        ui.horizontal(|ui| {
            if ui.checkbox(&mut on, "").changed() {
                *cfg = on.then(EspWdtConfig::default_for);
            }
            ui.label(egui::RichText::new(format!("{}  {title}", ph::SHIELD)).strong());
            ui.label(dim(subtitle));
        });
        ui.label(dim(note));
        let Some(c) = cfg.as_mut() else { return };
        ui.add_space(6.0);
        duration_row(ui, "Period", &mut c.timeout_us, range);
        ui.add_space(4.0);
        ui.label(dim(
            "esp_hal::init() switches every watchdog OFF on the way in, and the \
             generated code only configures this one - it starts biting when you call \
             enable().",
        ));
        problem_and_reset(ui, wdg::esp_wdt_problem(c, range, title), || {
            *c = EspWdtConfig::default_for()
        });
    });
}

/// A timer group's watchdog, or the reason this chip has no second one.
fn esp_mwdt_card(
    ui: &mut egui::Ui,
    cfg: &mut Option<EspWdtConfig>,
    n: u8,
    l: &EspWatchdogLimits,
    chip: &str,
) {
    // Shown disabled rather than hidden — the same rule the WWDG card follows
    // on an F1.
    if n == 1 && !l.has_mwdt1 {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled(false, egui::Checkbox::new(&mut false, ""));
                ui.label(
                    egui::RichText::new(format!("{}  MWDT1", ph::SHIELD))
                        .strong()
                        .color(egui::Color32::GRAY),
                );
                ui.label(dim("timer group 1 watchdog"));
            });
            ui.label(dim(format!(
                "Not available on this chip: the {chip} has one timer group, so there is \
                 no TIMG1 to take a watchdog from."
            )));
        });
        return;
    }
    esp_wdt_card(
        ui,
        cfg,
        &format!("MWDT{n}"),
        &format!("timer group {n} watchdog"),
        wdg::mwdt_range_us(),
        "Clocked from APB, but the period still means what it says: esp-hal reads the \
         live clock and works out the prescaler itself, so the Clock tab does not \
         stretch it.",
    );
}

/// The shared footer: what is wrong (if anything) and the way back to a value
/// that is known good.
///
/// The warning is not decoration. Every problem it reports is a **panic at
/// boot**, not a compile error, so this line is the only place the mistake can
/// be caught before a board resets in a loop.
fn problem_and_reset(ui: &mut egui::Ui, problem: Option<String>, mut reset: impl FnMut()) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .button(format!("{} Reset", ph::ARROW_COUNTER_CLOCKWISE))
            .on_hover_text("Restore the longest period this chip can express, window disabled")
            .clicked()
        {
            reset();
        }
        if let Some(msg) = problem {
            ui.label(
                egui::RichText::new(format!("{}  {msg}", ph::WARNING))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(235, 150, 90)),
            );
        }
    });
}

/// Why the DMA card is empty, in the words that fit THIS chip.
///
/// Apart from the drawing because the four answers are a real decision and
/// a `ui.label` is not: one of them used to tell Espressif users to
/// "re-import it from the STM32Cube database", and nothing could have
/// noticed.
fn dma_note(
    dma: Option<&crate::panels::mcu_module::mcu_def::DmaDef>,
    family: &str,
    on_dma_runtime: bool,
) -> String {
    // FOUR different silences, and the difference is the whole point:
    // "nothing asked for it" is not the same as "it could not be given".
    if crate::panels::mcu_module::codegen::family::is_esp(family) {
        // FIRST, because every branch below assumes a chip whose DMA
        // the IDE models. An Espressif part on the Async runtime was
        // told to "re-import it from the STM32Cube database" — advice
        // with no object: no CubeMX release has ever described an
        // ESP32. It reached them because the branch was excluded for
        // one family by name (`!= "stm32f1"`) rather than by asking
        // whether the family uses a `DmaDef` at all.
        // The runtime is not asked about at all. This branch used to send
        // people to the Async runtime, on the belief that esp-hal keeps DMA
        // on the async drivers — `with_dma` is on `impl Spi<'d, Blocking>`.
        format!(
            "No bus is on DMA yet. Set a SPI module's Transfers to DMA and the channel it \
             takes appears here - on either runtime, since esp-hal's `with_dma` is on the \
             blocking driver too. On {family} a channel is not split into TX and RX: one \
             channel drives both halves."
        )
    } else if !on_dma_runtime {
        format!(
            "No DMA on this runtime for {family}. Switch a bus to the Async runtime \
                 (System tab) - or, on the STM32F1, turn on the Blocking DMA transport \
                 in a USART or SPI module."
        )
    } else if dma.is_none() && family != "stm32f1" {
        "This chip carries no DMA channel data - re-import it from the STM32Cube \
             database so the IDE can allocate channels instead of leaving a TODO."
            .to_owned()
    } else {
        "No bus is on DMA yet. Turn it on in a USART, SPI or I2C module and the \
             channels it takes appear here."
            .to_owned()
    }
}

/// The PIO card: which state machines the project takes, and who has them.
///
/// The RP parts only — no other family in the registry has a PIO, and a card
/// reading "0 of 0" on every STM32 would be noise. Read-only for the same reason
/// the DMA card is: nothing here is chosen, it is the RESULT of what is wired.
///
/// It exists because the radio takes PIO0/sm0 SILENTLY. Someone who wanted PIO
/// for something else on a Pico W had no way to learn it was gone short of the
/// compiler saying so — which is the situation the DMA card was built to end.
fn pio_card(
    ui: &mut egui::Ui,
    uses: &[crate::panels::mcu_module::codegen::rp::PioUse],
    family: &str,
    is_async: bool,
    has_radio_pad: bool,
) {
    use crate::panels::mcu_module::codegen::rp;
    let blocks = rp::pio_blocks(family);
    let total = blocks * rp::PIO_SMS;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{}  PIO", ph::CPU))
                    .size(13.0)
                    .strong(),
            );
            let s = if uses.len() == 1 { "" } else { "s" };
            ui.label(dim(format!(
                "{} of {total} state machine{s} used - {blocks} blocks of {} each",
                uses.len(),
                rp::PIO_SMS
            )));
        });
        ui.add_space(6.0);

        if uses.is_empty() {
            // Say WHY it is empty. The three reasons are genuinely different,
            // and only the last one is something the user can act on.
            ui.label(dim(if !has_radio_pad {
                "Nothing on this board uses PIO. Every state machine is yours."
            } else if !is_async {
                "Nothing uses PIO on this runtime. The radio's driver is async only, so a Blocking project leaves every state machine free."
            } else {
                "Nothing uses PIO yet. Driving WL_LED puts the radio on PIO0/sm0 - the wifi chip's half-duplex SPI is not something an SPI block on this chip can produce."
            }));
            return;
        }

        egui::Grid::new("pio_uses")
            .num_columns(3)
            .spacing([18.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(dim("State machine"));
                ui.label(dim("Used by"));
                ui.label(dim("Interrupt"));
                ui.end_row();
                for u in uses {
                    ui.label(
                        egui::RichText::new(format!("{}/{}", u.block, u.sm))
                            .size(11.5)
                            .strong(),
                    );
                    ui.label(egui::RichText::new(&u.user).size(11.5));
                    ui.label(dim(if u.irq.is_empty() { "-" } else { &u.irq }));
                    ui.end_row();
                }
            });

        ui.add_space(6.0);
        // Which blocks are wholly untouched - the real answer to "can I still
        // use PIO for something?".
        let taken: Vec<&str> = uses.iter().map(|u| u.block.as_str()).collect();
        let free: Vec<String> = (0..blocks)
            .map(|i| format!("PIO{i}"))
            .filter(|b| !taken.contains(&b.as_str()))
            .collect();
        ui.label(dim(if free.is_empty() {
            "Every block has something on it.".to_owned()
        } else {
            format!("Untouched: {}", free.join(", "))
        }));
        // Easy to miss: a block is not free because three of its four state
        // machines are.
        ui.label(dim(
            "A block's instruction memory is shared by its four state machines, so a second program there has to fit alongside the first.",
        ));
    });
}

/// The DMA card: every channel the project takes, and who has it.
///
/// Read-only on purpose. The channel a peripheral gets is chosen in its Virtual
/// Module (Automatic, or pinned by hand); this is the place that shows the
/// RESULT, which no single module can — a channel is only "taken" relative to
/// every other peripheral in the project.
fn dma_card(
    ui: &mut egui::Ui,
    uses: &[crate::panels::mcu_module::codegen::dma_map::DmaUse],
    dma: Option<&crate::panels::mcu_module::mcu_def::DmaDef>,
    family: &str,
    on_dma_runtime: bool,
    // Channels the chip has when no vendor `DmaDef` describes it. The Pico
    // has no CubeMX database behind it, and a card that could not count its
    // channels answered neither of the two questions anyone asks here.
    plain_total: Option<usize>,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{}  DMA", ph::ARROWS_LEFT_RIGHT))
                    .size(13.0)
                    .strong(),
            );
            let plain = plain_total.filter(|_| dma.is_none());
            if let Some(total) = plain {
                ui.label(dim(format!(
                    "{} of {total} channels used - any channel serves any peripheral",
                    uses.len()
                )));
            }
            if let Some(d) = dma {
                let total = d.channels.len();
                ui.label(dim(format!(
                    "{} of {total} channel{} used{}",
                    uses.len(),
                    if total == 1 { "" } else { "s" },
                    if d.mux {
                        " - any channel serves any peripheral on this chip"
                    } else {
                        ""
                    }
                )));
            }
        });
        ui.add_space(6.0);

        if uses.is_empty() {
            let why = dma_note(dma, family, on_dma_runtime);
            ui.label(dim(why));
            return;
        }

        egui::Grid::new("dma_uses")
            .num_columns(3)
            .spacing([18.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(dim("Channel"));
                ui.label(dim("Used by"));
                ui.label(dim("Interrupt"));
                ui.end_row();
                for u in uses {
                    ui.label(egui::RichText::new(&u.peri).size(11.5).strong());
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&u.user).size(11.5));
                        if u.manual {
                            // The one row a reader must not mistake for the
                            // allocator's doing: someone pinned this by hand.
                            ui.label(
                                egui::RichText::new("pinned")
                                    .size(10.0)
                                    .color(egui::Color32::from_rgb(200, 170, 100)),
                            )
                            .on_hover_text(
                                "Chosen by hand in the Virtual Module, not allocated. \
                                 Reserved before anything else is handed out.",
                            );
                        }
                    });
                    // Empty on the F1 blocking path: the HAL owns the interrupt
                    // and no generated line names it.
                    ui.label(dim(if u.irq.is_empty() { "-" } else { &u.irq }));
                    ui.end_row();
                }
            });

        // What is still free, which is the question asked right before adding
        // one more peripheral.
        if let Some(total) = plain_total.filter(|_| dma.is_none()) {
            let free: Vec<String> = (0..total)
                .map(|i| format!("DMA_CH{i}"))
                .filter(|c| !uses.iter().any(|u| &u.peri == c))
                .collect();
            ui.add_space(6.0);
            ui.label(dim(if free.is_empty() {
                "No channel left.".to_owned()
            } else {
                format!("Free: {}", free.join(", "))
            }));
        }
        if let Some(d) = dma {
            let free: Vec<&str> = d
                .channels
                .iter()
                .map(|c| c.peri.as_str())
                .filter(|p| !uses.iter().any(|u| u.peri == *p))
                .collect();
            ui.add_space(6.0);
            ui.label(dim(if free.is_empty() {
                "No channel left - another peripheral on DMA would keep its TODO.".to_owned()
            } else {
                format!("Free: {}", free.join(", "))
            }));
        }
    });
}

/// One card per comparator the chip has — CubeMX's COMP panel, minus the
/// fields embassy has no way to apply.
///
/// Left out on purpose, rather than shown and ignored:
/// * **Interrupt Trigger Mode** — embassy arms EXTI inside `wait_for_*`, so the
///   edge is chosen where you await, not here.
/// * **Output Internal Selection** (routing to a timer) and **Deglitcher** —
///   absent from `comp::Config` entirely.
/// * **External Output** — `COMP{n}_OUT` is an ordinary pin function on the
///   Pins tab; `Comp::new` takes no output pin.
fn comp_cards(
    ui: &mut egui::Ui,
    settings: &mut CompSettings,
    comps: &[(u8, Option<String>, Option<String>)],
    generation: Option<comparator::Generation>,
    family: &str,
    is_async: bool,
) {
    ui.label(
        egui::RichText::new(format!("{}  Comparators", ph::WAVE_SQUARE))
            .size(13.0)
            .strong(),
    );
    ui.add_space(4.0);

    if comps.is_empty() {
        ui.label(dim("This chip has no analog comparators."));
        return;
    }
    let Some(generation) = generation else {
        // The honest reason PER FAMILY: "registers but no driver" and "no
        // registers at all" are months and never apart, and the card is where
        // someone finds out which one they are looking at.
        ui.label(
            egui::RichText::new(format!(
                "{}  {}",
                ph::WARNING,
                comparator::unsupported_reason(family).unwrap_or_default()
            ))
            .size(11.0)
            .color(egui::Color32::from_rgb(235, 150, 90)),
        );
        return;
    };
    if !is_async {
        ui.label(
            egui::RichText::new(format!(
                "{}  `Comp::new` takes an interrupt binding, so comparators are generated on \
                 the Async runtime only. Switch it in the System tab.",
                ph::WARNING
            ))
            .size(11.0)
            .color(egui::Color32::from_rgb(235, 150, 90)),
        );
        return;
    }
    ui.label(dim(
        "The [+] input is a pin: configure COMPn_INP on the Pins tab. Everything below \
         is a register with no pin of its own.",
    ));
    ui.add_space(6.0);

    for (n, inp, inm) in comps {
        comp_card(ui, settings, *n, inp.as_deref(), inm.as_deref(), generation);
        ui.add_space(6.0);
    }
}

/// The card for one comparator.
fn comp_card(
    ui: &mut egui::Ui,
    settings: &mut CompSettings,
    n: u8,
    inp: Option<&str>,
    inm: Option<&str>,
    generation: comparator::Generation,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        let mut on = settings.contains_key(&n);
        ui.horizontal(|ui| {
            if ui.checkbox(&mut on, format!("COMP{n}")).changed() {
                if on {
                    settings.insert(n, CompConfig::default());
                } else {
                    settings.remove(&n);
                }
            }
            match inp {
                Some(pin) => {
                    ui.label(dim(format!("[+] {pin}")));
                }
                None => {
                    ui.label(
                        egui::RichText::new(format!("{}  no COMP{n}_INP pin", ph::WARNING))
                            .size(10.5)
                            .color(egui::Color32::from_rgb(235, 150, 90)),
                    )
                    .on_hover_text(
                        "The non-inverting input is a pin, and nothing is generated without \
                         it. Configure COMPn_INP on the Pins tab.",
                    );
                }
            }
        });

        let Some(cfg) = settings.get_mut(&n) else {
            return;
        };
        ui.add_space(4.0);
        egui::Grid::new(format!("comp{n}_grid"))
            .num_columns(2)
            .spacing([14.0, 5.0])
            .show(ui, |ui| {
                ui.label("Input [-]");
                egui::ComboBox::from_id_salt(format!("comp{n}_inm"))
                    .selected_text(cfg.inverting_input.label())
                    .show_ui(ui, |ui| {
                        for v in comparator::InvertingInput::options(generation) {
                            ui.selectable_value(&mut cfg.inverting_input, *v, v.label());
                        }
                    });
                ui.end_row();

                // Only the two pin choices need a second pin; say so exactly
                // where the choice was made.
                if cfg.inverting_input.needs_pin() {
                    ui.label(dim("[-] pin"));
                    match inm {
                        Some(pin) => {
                            ui.label(dim(pin));
                        }
                        None => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  configure COMP{n}_INM on the Pins tab",
                                    ph::WARNING
                                ))
                                .size(10.5)
                                .color(egui::Color32::from_rgb(235, 150, 90)),
                            );
                        }
                    }
                    ui.end_row();
                }

                // embassy writes this register field only on the U5/WBA
                // generation; on a G4 the setting would be inert, so the row is
                // absent rather than shown and ignored.
                if generation.has_power_mode() {
                    ui.label("Speed / power");
                    egui::ComboBox::from_id_salt(format!("comp{n}_power"))
                        .selected_text(cfg.power_mode.label())
                        .show_ui(ui, |ui| {
                            for v in comparator::PowerMode::ALL {
                                ui.selectable_value(&mut cfg.power_mode, v, v.label());
                            }
                        });
                    ui.end_row();
                }

                ui.label("Hysteresis");
                // Reading `@comp` cannot know the chip; a level from the other
                // generation is corrected here, where the user can see it.
                if !cfg.hysteresis.fits(generation) {
                    cfg.hysteresis = comparator::Hysteresis::None;
                }
                egui::ComboBox::from_id_salt(format!("comp{n}_hyst"))
                    .selected_text(cfg.hysteresis.label())
                    .show_ui(ui, |ui| {
                        for v in comparator::Hysteresis::options(generation) {
                            ui.selectable_value(&mut cfg.hysteresis, *v, v.label());
                        }
                    });
                ui.end_row();

                ui.label("Output polarity");
                egui::ComboBox::from_id_salt(format!("comp{n}_pol"))
                    .selected_text(cfg.output_polarity.label())
                    .show_ui(ui, |ui| {
                        for v in comparator::OutputPolarity::ALL {
                            ui.selectable_value(&mut cfg.output_polarity, v, v.label());
                        }
                    });
                ui.end_row();

                ui.label("Blanking source");
                egui::ComboBox::from_id_salt(format!("comp{n}_blank"))
                    .selected_text(cfg.blanking_source.label())
                    .show_ui(ui, |ui| {
                        for v in comparator::BlankingSource::ALL {
                            ui.selectable_value(&mut cfg.blanking_source, v, v.label());
                        }
                    });
                ui.end_row();
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::mcu_module::builtins::builtin_definitions;
    use crate::panels::mcu_module::codegen::family;

    /// No chip may be sent to a database that has never heard of it.
    ///
    /// The empty-DMA message had four cases and the Espressif one fell into
    /// "re-import it from the STM32Cube database" — because the branch was
    /// excluded for one family BY NAME (`!= "stm32f1"`) instead of by asking
    /// whether the family uses a `DmaDef`. Every bundled chip is walked through
    /// both runtimes here, since the runtime picks the branch.
    #[test]
    fn the_dma_note_never_sends_an_esp_chip_to_cubemx() {
        for d in builtin_definitions() {
            for on_dma_runtime in [true, false] {
                let note = dma_note(d.dma.as_ref(), &d.family, on_dma_runtime);
                assert!(!note.trim().is_empty(), "{}: no explanation", d.id);
                if family::is_esp(&d.family) {
                    assert!(
                        !note.contains("STM32Cube"),
                        "{}: sent to CubeMX — {note}",
                        d.id
                    );
                    // And the one it does get names the chip and says what is
                    // actually missing, rather than what to go and fetch.
                    assert!(note.contains(&d.family), "{}: {note}", d.id);
                    assert!(note.contains("esp-hal"), "{}: {note}", d.id);
                }
            }
        }
    }

    /// …while the families that DO carry channel data keep their advice.
    #[test]
    fn a_chip_with_no_channel_data_is_still_told_where_to_get_it() {
        let note = dma_note(None, "stm32wl3", true);
        assert!(note.contains("STM32Cube"), "{note}");
        // The F1 is excluded for a real reason: its channels are fixed in the
        // HAL's types, so there is nothing to import.
        assert!(!dma_note(None, "stm32f1", true).contains("STM32Cube"));
    }
}
