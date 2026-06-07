//! Interactive STM32F103 clock tree — a CubeMX-style vector recreation of
//! datasheet **Figure 2**.
//!
//! Design goals (from user feedback):
//! - Orthogonal wiring routed in lanes so paths never cross blocks.
//! - Mux selectors drawn as **trapezoids with radio buttons** (System Clock,
//!   PLL Source, RTC, MCO) so the active input is obvious — like CubeMX.
//! - Dropdowns fully cover their box; the node name is a label **above** it.
//! - All datasheet outputs shown: HCLK, FCLK, SysTick, PCLK1/2, timers, ADC,
//!   USB, RTC, IWDG, FLITFCLK.  Frequency tags turn red past a limit.
//!
//! The figure lives in a fixed virtual space and scales to the panel width.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, UiBuilder, Vec2};

use super::super::compute::{frequencies, ClockFrequencies};
use super::super::model::{
    Mco, PllSrc, RtcSrc, Stm32f1Clock, SysclkSrc, SystickSrc, UsbPre, ADC_PRESCALERS,
    AHB_PRESCALERS, APB_PRESCALERS, PLL_MUL_MAX, PLL_MUL_MIN,
};

// ── Virtual canvas ────────────────────────────────────────────────────────────
const VW: f32 = 1000.0;
const VH: f32 = 790.0;
const M: u32 = 1_000_000;

// ── Palette ───────────────────────────────────────────────────────────────────
const BG: Color32 = Color32::from_rgb(26, 28, 34);
const BOX_FILL: Color32 = Color32::from_rgb(42, 46, 56);
const OUT_FILL: Color32 = Color32::from_rgb(34, 40, 50);
const STROKE_C: Color32 = Color32::from_rgb(140, 150, 165);
const WIRE_C: Color32 = Color32::from_rgb(110, 122, 140);
const LABEL_C: Color32 = Color32::from_rgb(208, 215, 228);
const DIM_C: Color32 = Color32::from_rgb(150, 158, 172);
const MUX_FILL: Color32 = Color32::from_rgb(50, 56, 70);
const MUX_STROKE: Color32 = Color32::from_rgb(95, 140, 215);
const FREQ_OK: Color32 = Color32::from_rgb(120, 205, 140);
const FREQ_BAD: Color32 = Color32::from_rgb(235, 95, 85);

#[derive(Clone, Copy)]
struct Tf {
    origin: Pos2,
    scale: f32,
}
impl Tf {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.origin + Vec2::new(x, y) * self.scale
    }
    fn r(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.p(x, y), Vec2::new(w, h) * self.scale)
    }
    fn fs(&self, pt: f32) -> f32 {
        (pt * self.scale).max(6.0)
    }
}

/// Render the diagram + interactive nodes. Returns `true` on any change.
///
/// `avail_w`/`avail_h` are the viewport size — at `zoom == 1.0` the WHOLE figure
/// fits (no scrolling needed); zooming in enlarges it and reveals scroll bars.
pub fn draw(ui: &mut egui::Ui, c: &mut Stm32f1Clock, avail_w: f32, avail_h: f32, zoom: f32) -> bool {
    let f = frequencies(c);

    // Fit the whole diagram into the viewport (both dimensions), then apply zoom.
    let fit = (avail_w / VW).min(avail_h / VH);
    let scale = (fit * zoom).max(0.12);
    let size = Vec2::new(VW * scale, VH * scale);

    // Reserve the whole canvas up-front so the controls below never overlap the
    // diagram (the interactive widgets are then confined to this exact rect).
    let (rect, _resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    let tf = Tf { origin: rect.min, scale };

    // Clip BOTH the painter and the interactive widgets to the visible viewport
    // (rect ∩ current clip), so anything scrolled out of view disappears
    // instead of floating on top of the panels.
    let clip = rect.intersect(ui.clip_rect());
    let painter = ui.painter().with_clip_rect(clip);
    painter.rect_filled(rect, 4.0, BG);

    draw_wires(&painter, &tf, c, &f);
    draw_static_blocks(&painter, &tf, c, &f);

    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.set_clip_rect(clip);
        changed = interactive_nodes(ui, &tf, c);
    });
    changed
}

// ──────────────────────────────────────────────────────────────────────────────
// Wires (drawn first, under the blocks)
// ──────────────────────────────────────────────────────────────────────────────

fn draw_wires(p: &egui::Painter, tf: &Tf, _c: &Stm32f1Clock, _f: &ClockFrequencies) {
    // CubeMX style: muxes show their sources as labelled stubs (drawn inside
    // `mux_radios`), so we avoid long cross-canvas wires.  Only short, local
    // links are drawn here — none of them cross a block.

    // Oscillator → local divider → mux input.
    wire(p, tf, &[(120.0, 300.0), (157.0, 300.0), (157.0, 383.0), (175.0, 383.0)]); // HSI → /2
    wire(p, tf, &[(215.0, 383.0), (226.0, 383.0)]); // /2 → PLL-src HSI/2 input
    wire(p, tf, &[(120.0, 500.0), (148.0, 500.0)]); // HSE → PLLXTPRE
    wire(p, tf, &[(208.0, 502.0), (226.0, 502.0)]); // PLLXTPRE → PLL-src HSE input

    // PLL chain: PLLsrc mux → PLLMUL → PLLCLK node.
    wire(p, tf, &[(290.0, 442.0), (336.0, 442.0)]); // mux out → PLLMUL
    wire(p, tf, &[(420.0, 442.0), (456.0, 442.0)]); // PLLMUL → PLLCLK

    // System mux (SYSCLK) → AHB → HCLK → distribution lane (x = 640).
    wire(p, tf, &[(510.0, 360.0), (540.0, 360.0)]); // SYSCLK → AHB
    wire(p, tf, &[(626.0, 360.0), (640.0, 360.0)]); // AHB → HCLK node
    wire(p, tf, &[(640.0, 360.0), (818.0, 360.0)]); // HCLK → AHB-bus out
    wire(p, tf, &[(640.0, 360.0), (640.0, 700.0)]); // HCLK vertical lane
    wire(p, tf, &[(640.0, 430.0), (700.0, 430.0)]); // → SysTick prescaler
    wire(p, tf, &[(766.0, 430.0), (818.0, 430.0)]); // SysTick → out
    wire(p, tf, &[(640.0, 470.0), (818.0, 470.0)]); // → FCLK out
    wire(p, tf, &[(640.0, 543.0), (720.0, 543.0)]); // → APB1 prescaler
    wire(p, tf, &[(786.0, 543.0), (818.0, 543.0)]); // PCLK1 → APB1 periph
    wire(p, tf, &[(806.0, 556.0), (806.0, 590.0), (818.0, 590.0)]); // → APB1 timer
    wire(p, tf, &[(640.0, 663.0), (720.0, 663.0)]); // → APB2 prescaler
    wire(p, tf, &[(786.0, 663.0), (818.0, 663.0)]); // PCLK2 → APB2 periph
    wire(p, tf, &[(806.0, 676.0), (806.0, 710.0), (818.0, 710.0)]); // → APB2 timer
    wire(p, tf, &[(753.0, 676.0), (753.0, 763.0), (720.0, 763.0)]); // PCLK2 → ADC prescaler
    wire(p, tf, &[(786.0, 763.0), (818.0, 763.0)]); // ADC → out

    // Right-margin outputs (clear top band — no block crossings).
    wire(p, tf, &[(290.0, 122.0), (818.0, 122.0)]); // RTC mux → RTCCLK box
    wire(p, tf, &[(560.0, 245.0), (818.0, 245.0)]); // USB → USBCLK box

    // MCO mux output (mirrored → left) → MCO pin box.
    wire(p, tf, &[(250.0, 638.0), (134.0, 638.0)]);
}

// ──────────────────────────────────────────────────────────────────────────────
// Static blocks + labels + frequency tags
// ──────────────────────────────────────────────────────────────────────────────

fn draw_static_blocks(p: &egui::Painter, tf: &Tf, c: &Stm32f1Clock, f: &ClockFrequencies) {
    // ── Oscillators (left) ───────────────────────────────────────────────────
    block(p, tf, 28.0, 78.0, 92.0, 34.0, "LSE OSC\n32.768 kHz");
    block(p, tf, 170.0, 84.0, 46.0, 22.0, "/128");
    block(p, tf, 28.0, 153.0, 92.0, 34.0, "LSI RC\n40 kHz");
    block(p, tf, 28.0, 283.0, 92.0, 34.0, "HSI RC\n8 MHz");
    block(p, tf, 175.0, 372.0, 40.0, 22.0, "/2");
    block(p, tf, 28.0, 483.0, 92.0, 34.0, "HSE OSC\n4–16 MHz");

    // ── All output boxes on the right margin (x = 820) ───────────────────────
    out_box(p, tf, 820.0, 109.0, 160.0, 26.0, "RTCCLK → RTC", rtc_hz(c, f), None);
    out_box(p, tf, 820.0, 149.0, 160.0, 26.0, "IWDGCLK ← LSI", 40_000, None);
    out_box(p, tf, 820.0, 232.0, 160.0, 26.0, "USBCLK → USB", f.usbclk, None);
    out_box(p, tf, 820.0, 272.0, 160.0, 26.0, "FLITFCLK ← HSI", f.flitfclk, None);
    out_box(p, tf, 820.0, 346.0, 160.0, 28.0, "HCLK → AHB / core / DMA", f.hclk, Some(72 * M));
    out_box(p, tf, 820.0, 416.0, 160.0, 28.0, "Cortex SysTick", systick_hz(c, f), None);
    out_box(p, tf, 820.0, 456.0, 160.0, 28.0, "FCLK (free-running)", f.hclk, None);
    out_box(p, tf, 820.0, 529.0, 160.0, 28.0, "APB1 peripherals", f.pclk1, Some(36 * M));
    out_box(p, tf, 820.0, 576.0, 160.0, 28.0, "APB1 timers", f.tim_apb1, None);
    out_box(p, tf, 820.0, 649.0, 160.0, 28.0, "APB2 peripherals", f.pclk2, Some(72 * M));
    out_box(p, tf, 820.0, 696.0, 160.0, 28.0, "APB2 timers", f.tim_apb2, None);
    out_box(p, tf, 820.0, 749.0, 160.0, 28.0, "ADC1/2", f.adcclk, Some(14 * M));

    // ── MCO pin box (far left, fed by the mirrored MCO mux) ──────────────────
    out_box(p, tf, 28.0, 625.0, 106.0, 26.0, "MCO pin", mco_hz(c, f), None);

    // ── Frequency tags on the main chain ─────────────────────────────────────
    tag(p, tf, 424.0, 422.0, &mhz(f.pllclk), over(f.pllclk, 72 * M), "PLLCLK");
    tag(p, tf, 516.0, 344.0, &mhz(f.sysclk), over(f.sysclk, 72 * M), "SYSCLK");
    tag(p, tf, 592.0, 344.0, &mhz(f.hclk), over(f.hclk, 72 * M), "HCLK");
    tag(p, tf, 792.0, 524.0, &mhz(f.pclk1), over(f.pclk1, 36 * M), "PCLK1");
    tag(p, tf, 792.0, 644.0, &mhz(f.pclk2), over(f.pclk2, 72 * M), "PCLK2");

    // ── Node name labels (ABOVE their dropdowns) ─────────────────────────────
    label_above(p, tf, 148.0, 478.0, "PLLXTPRE");
    label_above(p, tf, 336.0, 418.0, "PLLMUL");
    label_above(p, tf, 470.0, 228.0, "USB Prescaler");
    label_above(p, tf, 540.0, 335.0, "AHB Prescaler");
    label_above(p, tf, 700.0, 408.0, "SysTick");
    label_above(p, tf, 720.0, 518.0, "APB1 Prescaler");
    label_above(p, tf, 720.0, 638.0, "APB2 Prescaler");
    label_above(p, tf, 720.0, 738.0, "ADC Prescaler");
    label_above(p, tf, 28.0, 521.0, "HSE crystal");
    // Mux titles (mux center x = 270)
    p.text(tf.p(270.0, 64.0), Align2::CENTER_BOTTOM, "RTC Mux", FontId::proportional(tf.fs(9.0)), DIM_C);
    p.text(tf.p(270.0, 364.0), Align2::CENTER_BOTTOM, "PLL Source", FontId::proportional(tf.fs(9.0)), DIM_C);
    p.text(tf.p(490.0, 294.0), Align2::CENTER_BOTTOM, "System Clock Mux", FontId::proportional(tf.fs(9.0)), DIM_C);
    p.text(tf.p(270.0, 574.0), Align2::CENTER_BOTTOM, "MCO Mux", FontId::proportional(tf.fs(9.0)), DIM_C);
}

// ──────────────────────────────────────────────────────────────────────────────
// Interactive nodes
// ──────────────────────────────────────────────────────────────────────────────

fn interactive_nodes(ui: &mut egui::Ui, tf: &Tf, c: &mut Stm32f1Clock) -> bool {
    let mut changed = false;
    const MW: f32 = 40.0; // mux width (narrow)

    // RTC Mux: HSE/128, LSE, LSI  (output → right)
    changed |= mux_radios(ui, tf, 250.0, 72.0, MW, 100.0,
        &[("HSE/128", 28.0), ("LSE", 50.0), ("LSI", 72.0)],
        rtc_sel(c.rtc_src), false,
        |idx| c.rtc_src = [RtcSrc::HseDiv128, RtcSrc::Lse, RtcSrc::Lsi][idx]);

    // PLL Source Mux: HSI/2 (aligned with /2 block), HSE (aligned with PLLXTPRE)
    let pll_sel = if c.pll_src == PllSrc::HsiDiv2 { 0 } else { 1 };
    changed |= mux_radios(ui, tf, 250.0, 370.0, MW, 145.0,
        &[("HSI/2", 13.0), ("HSE", 132.0)],
        Some(pll_sel), false,
        |idx| c.pll_src = if idx == 0 { PllSrc::HsiDiv2 } else { PllSrc::Hse });

    // System Clock Mux: HSI, HSE, PLLCLK  (output aligned with AHB at y≈360)
    let sw_sel = match c.sysclk_src {
        SysclkSrc::Hsi => 0,
        SysclkSrc::Hse => 1,
        SysclkSrc::Pll => 2,
    };
    changed |= mux_radios(ui, tf, 470.0, 300.0, MW, 120.0,
        &[("HSI", 24.0), ("HSE", 60.0), ("PLLCLK", 96.0)],
        Some(sw_sel), false,
        |idx| c.sysclk_src = [SysclkSrc::Hsi, SysclkSrc::Hse, SysclkSrc::Pll][idx]);

    // MCO Mux: mirrored (inputs on the right, output → left MCO pin)
    changed |= mux_radios(ui, tf, 250.0, 580.0, MW, 116.0,
        &[("SYSCLK", 20.0), ("HSI", 48.0), ("HSE", 76.0), ("PLL/2", 104.0)],
        mco_sel(c.mco), true,
        |idx| c.mco = [Mco::Sysclk, Mco::Hsi, Mco::Hse, Mco::PllDiv2][idx]);

    // ── Dropdowns (node name labelled above by the static layer) ─────────────
    changed |= node_hse(ui, tf, c);
    changed |= pllxtpre_dropdown(ui, tf, c);
    changed |= combo_u8(ui, tf, 336.0, 430.0, 84.0, "pllmul", &mut c.pll_mul,
        &(PLL_MUL_MIN..=PLL_MUL_MAX).collect::<Vec<_>>(), |v| format!("×{v}"));
    changed |= usb_dropdown(ui, tf, c);
    changed |= combo_u16(ui, tf, 540.0, 347.0, 86.0, "ahb", &mut c.ahb_pre, AHB_PRESCALERS, |v| format!("/ {v}"));
    changed |= systick_dropdown(ui, tf, c);
    changed |= combo_u8(ui, tf, 720.0, 530.0, 66.0, "apb1", &mut c.apb1_pre, APB_PRESCALERS, |v| format!("/ {v}"));
    changed |= combo_u8(ui, tf, 720.0, 650.0, 66.0, "apb2", &mut c.apb2_pre, APB_PRESCALERS, |v| format!("/ {v}"));
    changed |= combo_u8(ui, tf, 720.0, 750.0, 66.0, "adc", &mut c.adc_pre, ADC_PRESCALERS, |v| format!("/ {v}"));

    changed
}

/// Draw a vertical trapezoid mux with one radio button per input. Returns
/// `true` and runs `on_pick` if the user selects a new input.
///
/// `flip = false`: inputs on the left, output on the right (the usual case).
/// `flip = true`:  mirrored — inputs on the right, output on the left (MCO).
fn mux_radios(
    ui: &mut egui::Ui,
    tf: &Tf,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    inputs: &[(&str, f32)], // (label, dy from y where the input enters)
    selected: Option<usize>,
    flip: bool,
    mut on_pick: impl FnMut(usize),
) -> bool {
    let painter = ui.painter().clone();
    // Small, bounded taper → nearly rectangular mux so the radio buttons always
    // sit comfortably INSIDE the trapezoid (CubeMX style).
    let taper = (h * 0.16).clamp(8.0, 12.0);
    let poly = if flip {
        // Tall on the right (inputs), short on the left (output).
        vec![
            tf.p(x + w, y),
            tf.p(x + w, y + h),
            tf.p(x, y + h - taper),
            tf.p(x, y + taper),
        ]
    } else {
        // Tall on the left (inputs), short on the right (output).
        vec![
            tf.p(x, y),
            tf.p(x, y + h),
            tf.p(x + w, y + h - taper),
            tf.p(x + w, y + taper),
        ]
    };
    painter.add(Shape::convex_polygon(poly, MUX_FILL, Stroke::new(1.2, MUX_STROKE)));

    let mut changed = false;
    for (i, (label, dy)) in inputs.iter().enumerate() {
        let cy = y + dy;
        let stroke = Stroke::new(1.2, WIRE_C);
        // Radio sits just inside the wide edge (left for normal, right for flip);
        // the input stub stops at the mux edge so the wire meets the radio.
        let (stub_a, stub_b, lbl_x, lbl_align, radio_cx) = if flip {
            (x + w + 24.0, x + w, x + w + 26.0, Align2::LEFT_CENTER, x + w - 11.0)
        } else {
            (x - 24.0, x, x - 26.0, Align2::RIGHT_CENTER, x + 11.0)
        };
        painter.line_segment([tf.p(stub_a, cy), tf.p(stub_b, cy)], stroke);
        painter.text(tf.p(lbl_x, cy), lbl_align, *label, FontId::proportional(tf.fs(8.0)), DIM_C);

        let center = tf.p(radio_cx, cy);
        let rsz = 12.0 * tf.scale;
        let rrect = Rect::from_center_size(center, Vec2::splat(rsz));
        let is_sel = selected == Some(i);
        if ui.put(rrect, egui::RadioButton::new(is_sel, "")).clicked() {
            on_pick(i);
            changed = true;
        }
    }
    changed
}

fn rtc_sel(s: RtcSrc) -> Option<usize> {
    match s {
        RtcSrc::HseDiv128 => Some(0),
        RtcSrc::Lse => Some(1),
        RtcSrc::Lsi => Some(2),
        RtcSrc::None => None,
    }
}
fn mco_sel(s: Mco) -> Option<usize> {
    match s {
        Mco::Sysclk => Some(0),
        Mco::Hsi => Some(1),
        Mco::Hse => Some(2),
        Mco::PllDiv2 => Some(3),
        Mco::None => None,
    }
}

fn node_hse(ui: &mut egui::Ui, tf: &Tf, c: &mut Stm32f1Clock) -> bool {
    let rect = tf.r(28.0, 533.0, 92.0, 22.0);
    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        let mut mhz = c.hse_hz as f64 / 1e6;
        if ui
            .add(egui::DragValue::new(&mut mhz).range(1.0..=25.0).speed(0.1).suffix(" MHz"))
            .changed()
        {
            c.hse_hz = (mhz * 1e6).round() as u32;
            changed = true;
        }
    });
    changed
}

fn pllxtpre_dropdown(ui: &mut egui::Ui, tf: &Tf, c: &mut Stm32f1Clock) -> bool {
    // Only meaningful on the HSE→PLL branch.
    let cur = if c.pll_src == PllSrc::HseDiv2 { "/ 2" } else { "/ 1" };
    let rect = tf.r(150.0, 490.0, 60.0, 24.0);
    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt("pllxtpre")
            .selected_text(egui::RichText::new(cur).size(tf.fs(9.0)))
            .width(rect.width())
            .show_ui(ui, |ui| {
                if ui.selectable_label(cur == "/ 1", "/ 1").clicked() && c.pll_src != PllSrc::HsiDiv2 {
                    c.pll_src = PllSrc::Hse;
                    changed = true;
                }
                if ui.selectable_label(cur == "/ 2", "/ 2").clicked() && c.pll_src != PllSrc::HsiDiv2 {
                    c.pll_src = PllSrc::HseDiv2;
                    changed = true;
                }
            });
    });
    changed
}

fn usb_dropdown(ui: &mut egui::Ui, tf: &Tf, c: &mut Stm32f1Clock) -> bool {
    let rect = tf.r(470.0, 232.0, 90.0, 26.0);
    let cur = match c.usb_pre {
        UsbPre::Div1_5 => "/ 1.5",
        UsbPre::Div1 => "/ 1",
    };
    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt("usbpre")
            .selected_text(egui::RichText::new(cur).size(tf.fs(9.0)))
            .width(rect.width())
            .show_ui(ui, |ui| {
                if ui.selectable_label(cur == "/ 1.5", "/ 1.5").clicked() {
                    c.usb_pre = UsbPre::Div1_5;
                    changed = true;
                }
                if ui.selectable_label(cur == "/ 1", "/ 1").clicked() {
                    c.usb_pre = UsbPre::Div1;
                    changed = true;
                }
            });
    });
    changed
}

fn systick_dropdown(ui: &mut egui::Ui, tf: &Tf, c: &mut Stm32f1Clock) -> bool {
    let rect = tf.r(700.0, 418.0, 66.0, 24.0);
    let cur = match c.systick_src {
        SystickSrc::HclkDiv8 => "/ 8",
        SystickSrc::Hclk => "/ 1",
    };
    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt("systick")
            .selected_text(egui::RichText::new(cur).size(tf.fs(9.0)))
            .width(rect.width())
            .show_ui(ui, |ui| {
                if ui.selectable_label(cur == "/ 8", "/ 8").clicked() {
                    c.systick_src = SystickSrc::HclkDiv8;
                    changed = true;
                }
                if ui.selectable_label(cur == "/ 1", "/ 1").clicked() {
                    c.systick_src = SystickSrc::Hclk;
                    changed = true;
                }
            });
    });
    changed
}

fn combo_u8(
    ui: &mut egui::Ui,
    tf: &Tf,
    x: f32,
    y: f32,
    w: f32,
    id: &str,
    value: &mut u8,
    options: &[u8],
    label: impl Fn(u8) -> String,
) -> bool {
    let rect = tf.r(x, y, w, 26.0);
    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(egui::RichText::new(label(*value)).size(tf.fs(9.0)))
            .width(rect.width())
            .show_ui(ui, |ui| {
                for &opt in options {
                    if ui.selectable_label(*value == opt, label(opt)).clicked() {
                        *value = opt;
                        changed = true;
                    }
                }
            });
    });
    changed
}

fn combo_u16(
    ui: &mut egui::Ui,
    tf: &Tf,
    x: f32,
    y: f32,
    w: f32,
    id: &str,
    value: &mut u16,
    options: &[u16],
    label: impl Fn(u16) -> String,
) -> bool {
    let rect = tf.r(x, y, w, 26.0);
    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(egui::RichText::new(label(*value)).size(tf.fs(9.0)))
            .width(rect.width())
            .show_ui(ui, |ui| {
                for &opt in options {
                    if ui.selectable_label(*value == opt, label(opt)).clicked() {
                        *value = opt;
                        changed = true;
                    }
                }
            });
    });
    changed
}

// ──────────────────────────────────────────────────────────────────────────────
// Primitive painters
// ──────────────────────────────────────────────────────────────────────────────

fn block(p: &egui::Painter, tf: &Tf, x: f32, y: f32, w: f32, h: f32, title: &str) {
    let r = tf.r(x, y, w, h);
    p.rect(r, 3.0, BOX_FILL, Stroke::new(1.2, STROKE_C), egui::StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, title, FontId::proportional(tf.fs(9.0)), LABEL_C);
}

fn out_box(p: &egui::Painter, tf: &Tf, x: f32, y: f32, w: f32, h: f32, label: &str, hz: u32, limit: Option<u32>) {
    let r = tf.r(x, y, w, h);
    p.rect(r, 3.0, OUT_FILL, Stroke::new(1.0, STROKE_C), egui::StrokeKind::Inside);
    let bad = limit.map(|l| hz > l).unwrap_or(false);
    let col = if bad { FREQ_BAD } else { FREQ_OK };
    // value (left) + label (right)
    p.text(
        tf.p(x + 6.0, y + h / 2.0),
        Align2::LEFT_CENTER,
        mhz(hz),
        FontId::monospace(tf.fs(9.5)),
        col,
    );
    p.text(
        tf.p(x + w - 6.0, y + h / 2.0),
        Align2::RIGHT_CENTER,
        label,
        FontId::proportional(tf.fs(8.5)),
        DIM_C,
    );
}

fn label_above(p: &egui::Painter, tf: &Tf, x: f32, y: f32, text: &str) {
    p.text(tf.p(x, y), Align2::LEFT_BOTTOM, text, FontId::proportional(tf.fs(8.5)), DIM_C);
}

fn wire(p: &egui::Painter, tf: &Tf, pts: &[(f32, f32)]) {
    let stroke = Stroke::new(1.3, WIRE_C);
    for w in pts.windows(2) {
        p.line_segment([tf.p(w[0].0, w[0].1), tf.p(w[1].0, w[1].1)], stroke);
    }
    if pts.len() >= 2 {
        let a = pts[pts.len() - 2];
        let b = pts[pts.len() - 1];
        arrowhead(p, tf, a, b);
    }
}

fn arrowhead(p: &egui::Painter, tf: &Tf, from: (f32, f32), to: (f32, f32)) {
    let a = tf.p(from.0, from.1);
    let b = tf.p(to.0, to.1);
    let dir = (b - a).normalized();
    if !dir.is_finite() {
        return;
    }
    let n = Vec2::new(-dir.y, dir.x);
    let s = 5.0 * tf.scale.max(0.5);
    let p1 = b - dir * s + n * (s * 0.5);
    let p2 = b - dir * s - n * (s * 0.5);
    p.add(Shape::convex_polygon(vec![b, p1, p2], WIRE_C, Stroke::NONE));
}

fn tag(p: &egui::Painter, tf: &Tf, x: f32, y: f32, value: &str, bad: bool, name: &str) {
    let color = if bad { FREQ_BAD } else { FREQ_OK };
    p.text(
        tf.p(x, y),
        Align2::LEFT_CENTER,
        format!("{name} {value}"),
        FontId::monospace(tf.fs(9.0)),
        color,
    );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn systick_hz(c: &Stm32f1Clock, f: &ClockFrequencies) -> u32 {
    match c.systick_src {
        SystickSrc::HclkDiv8 => f.hclk / 8,
        SystickSrc::Hclk => f.hclk,
    }
}

fn rtc_hz(c: &Stm32f1Clock, f: &ClockFrequencies) -> u32 {
    match c.rtc_src {
        RtcSrc::HseDiv128 => f.hse / 128,
        RtcSrc::Lse => 32_768,
        RtcSrc::Lsi => 40_000,
        RtcSrc::None => 0,
    }
}

fn mco_hz(c: &Stm32f1Clock, f: &ClockFrequencies) -> u32 {
    match c.mco {
        Mco::None => 0,
        Mco::Sysclk => f.sysclk,
        Mco::Hsi => f.hsi,
        Mco::Hse => f.hse,
        Mco::PllDiv2 => f.pllclk / 2,
    }
}

fn mhz(hz: u32) -> String {
    if hz == 0 {
        return "—".to_string();
    }
    if hz < 1_000_000 {
        let khz = hz as f64 / 1000.0;
        if (khz.fract()).abs() < 1e-6 {
            return format!("{} kHz", khz as u32);
        }
        return format!("{khz:.3} kHz");
    }
    let m = hz as f64 / 1e6;
    if (m.fract()).abs() < 1e-6 {
        format!("{} MHz", m as u32)
    } else {
        format!("{m:.2} MHz")
    }
}
fn over(hz: u32, limit: u32) -> bool {
    hz > limit
}
