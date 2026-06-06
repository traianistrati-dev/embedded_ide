//! Faithful vector recreation of STM32F103 datasheet **Figure 2 — Clock tree**.
//!
//! The figure is laid out in a fixed virtual coordinate space (900×560) and
//! scaled to the available width, so it stays crisp at any size.  Static blocks,
//! mux symbols, wires and the legend are painted; the *configurable* nodes
//! (HSE, PLLSRC, PLLMUL, SW, AHB/APB1/APB2/ADC/USB prescalers, MCO) are live
//! `ComboBox`es placed on top of their blocks.  Frequency tags on the wires turn
//! red when a datasheet limit is exceeded.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, UiBuilder, Vec2};

use super::super::compute::{frequencies, ClockFrequencies};
use super::super::model::{
    Mco, PllSrc, Stm32f1Clock, SysclkSrc, UsbPre, ADC_PRESCALERS, AHB_PRESCALERS, APB_PRESCALERS,
    PLL_MUL_MAX, PLL_MUL_MIN,
};

// ── Virtual canvas size (datasheet figure proportions) ────────────────────────
const VW: f32 = 900.0;
const VH: f32 = 560.0;

// ── Palette (dark IDE theme) ──────────────────────────────────────────────────
const BG: Color32 = Color32::from_rgb(28, 30, 36);
const BOX_FILL: Color32 = Color32::from_rgb(44, 48, 58);
const STROKE_C: Color32 = Color32::from_rgb(150, 160, 175);
const WIRE_C: Color32 = Color32::from_rgb(120, 130, 145);
const LABEL_C: Color32 = Color32::from_rgb(205, 212, 224);
const MUX_FILL: Color32 = Color32::from_rgb(58, 64, 78);
const FREQ_OK: Color32 = Color32::from_rgb(120, 205, 140);
const FREQ_BAD: Color32 = Color32::from_rgb(235, 95, 85);
const ACCENT: Color32 = Color32::from_rgb(90, 150, 230);

/// Virtual → screen transform.
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

/// Draw the diagram and overlay interactive nodes. Returns `true` on any change.
pub fn draw(ui: &mut egui::Ui, c: &mut Stm32f1Clock) -> bool {
    let f = frequencies(c);

    // Responsive canvas: keep the figure aspect ratio, fit the available width.
    let avail_w = ui.available_width().clamp(520.0, 1100.0);
    let scale = avail_w / VW;
    let size = Vec2::new(VW * scale, VH * scale);
    let (resp, painter) = ui.allocate_painter(size, egui::Sense::hover());
    let tf = Tf { origin: resp.rect.min, scale };

    painter.rect_filled(resp.rect, 4.0, BG);

    // ── Static schematic ─────────────────────────────────────────────────────
    draw_static(&painter, &tf, c, &f);

    // ── Interactive nodes (ComboBoxes on top of their blocks) ────────────────
    let mut changed = false;
    changed |= node_hse(ui, &tf, c);
    changed |= node_combo(ui, &tf, 232.0, 150.0, 96.0, "pllsrc", pll_src_text(c.pll_src), || {
        pll_src_options()
    })
    .map(|v| {
        if c.pll_src != v {
            c.pll_src = v;
            true
        } else {
            false
        }
    })
    .unwrap_or(false);
    changed |= node_mul(ui, &tf, c);
    changed |= node_sw(ui, &tf, c);
    changed |= node_div_u16(ui, &tf, 470.0, 168.0, 92.0, "ahb", &mut c.ahb_pre, AHB_PRESCALERS);
    changed |= node_div_u8(ui, &tf, 560.0, 226.0, 86.0, "apb1", &mut c.apb1_pre, APB_PRESCALERS);
    changed |= node_div_u8(ui, &tf, 560.0, 330.0, 86.0, "apb2", &mut c.apb2_pre, APB_PRESCALERS);
    changed |= node_div_u8(ui, &tf, 560.0, 410.0, 86.0, "adc", &mut c.adc_pre, ADC_PRESCALERS);
    changed |= node_usb(ui, &tf, c);
    changed |= node_mco(ui, &tf, c);

    changed
}

// ──────────────────────────────────────────────────────────────────────────────
// Static drawing
// ──────────────────────────────────────────────────────────────────────────────

fn draw_static(p: &egui::Painter, tf: &Tf, c: &Stm32f1Clock, f: &ClockFrequencies) {
    // ── Oscillator blocks (left) ─────────────────────────────────────────────
    block(p, tf, 40.0, 56.0, 90.0, 34.0, "HSI RC\n8 MHz");
    block(p, tf, 150.0, 112.0, 34.0, 24.0, "/2");
    block(p, tf, 30.0, 300.0, 100.0, 34.0, "HSE OSC\n4–16 MHz");
    block(p, tf, 150.0, 300.0, 34.0, 24.0, "/2"); // PLLXTPRE
    block(p, tf, 30.0, 396.0, 100.0, 36.0, "LSE OSC\n32.768 kHz");
    block(p, tf, 150.0, 392.0, 38.0, 22.0, "/128");
    block(p, tf, 30.0, 474.0, 100.0, 34.0, "LSI RC\n40 kHz");

    // CSS marker
    block(p, tf, 196.0, 286.0, 40.0, 22.0, "CSS");

    // ── PLL chain (middle) ───────────────────────────────────────────────────
    // PLLSRC mux + PLLMUL + SW mux are interactive (drawn as block outlines).
    block(p, tf, 350.0, 138.0, 96.0, 50.0, "PLL\n×2…×16");
    // labels for the muxes (the combo boxes sit on these rects)
    outline(p, tf, 232.0, 150.0, 96.0, 26.0); // PLLSRC
    outline(p, tf, 470.0, 150.0, 92.0, 26.0); // SW (SYSCLK)

    // ── Prescalers / buses (right) ───────────────────────────────────────────
    outline(p, tf, 470.0, 168.0, 92.0, 26.0); // AHB
    block(p, tf, 470.0, 70.0, 110.0, 30.0, "USB Prescaler"); // /1,1.5 (combo on top)
    outline(p, tf, 560.0, 226.0, 86.0, 26.0); // APB1
    outline(p, tf, 560.0, 330.0, 86.0, 26.0); // APB2
    outline(p, tf, 560.0, 410.0, 86.0, 26.0); // ADC
    block(p, tf, 660.0, 250.0, 90.0, 40.0, "TIM2/3/4\n×1 / ×2");
    block(p, tf, 660.0, 360.0, 90.0, 40.0, "TIM1\n×1 / ×2");

    // ── RTC / MCO (bottom) ───────────────────────────────────────────────────
    block(p, tf, 260.0, 392.0, 50.0, 26.0, "RTCSEL");
    outline(p, tf, 110.0, 506.0, 96.0, 26.0); // MCO mux

    // ── Wires (signal flow) ──────────────────────────────────────────────────
    let m = 1_000_000u32;
    // HSI → PLLSRC mux & SW mux
    wire(p, tf, &[(130.0, 73.0), (210.0, 73.0), (210.0, 124.0), (150.0, 124.0)]); // HSI → /2
    wire(p, tf, &[(184.0, 124.0), (232.0, 163.0)]); // HSI/2 → PLLSRC
    wire(p, tf, &[(130.0, 73.0), (440.0, 73.0)]); // HSI rail to top (towards SW/MCO/USB area)
    wire(p, tf, &[(456.0, 73.0), (456.0, 150.0)]); // down to SW input (HSI)
    // HSE → PLLXTPRE → PLLSRC, and HSE → SW
    wire(p, tf, &[(130.0, 312.0), (210.0, 312.0), (210.0, 176.0), (232.0, 169.0)]); // HSE → PLLSRC
    wire(p, tf, &[(130.0, 317.0), (440.0, 317.0), (440.0, 176.0), (470.0, 169.0)]); // HSE → SW
    // PLLSRC → PLL → SW
    wire(p, tf, &[(328.0, 163.0), (350.0, 163.0)]);
    wire(p, tf, &[(446.0, 163.0), (470.0, 163.0)]);
    // SW(SYSCLK) → AHB → HCLK
    wire(p, tf, &[(562.0, 163.0), (470.0, 181.0)]); // SYSCLK → AHB (short)
    wire(p, tf, &[(562.0, 181.0), (760.0, 181.0)]); // HCLK rail
    // AHB(HCLK) → APB1 / APB2
    wire(p, tf, &[(600.0, 194.0), (600.0, 226.0)]); // → APB1
    wire(p, tf, &[(610.0, 194.0), (610.0, 330.0)]); // → APB2
    // APB2 → ADC, APB1/APB2 → timers
    wire(p, tf, &[(603.0, 356.0), (603.0, 410.0)]); // PCLK2 → ADC
    wire(p, tf, &[(646.0, 239.0), (660.0, 270.0)]); // APB1 → TIM2/3/4
    wire(p, tf, &[(646.0, 343.0), (660.0, 380.0)]); // APB2 → TIM1
    // PLL → USB prescaler
    wire(p, tf, &[(398.0, 138.0), (398.0, 85.0), (470.0, 85.0)]);
    // LSE/LSI/RTC/MCO
    wire(p, tf, &[(130.0, 414.0), (150.0, 403.0)]); // LSE → /128
    wire(p, tf, &[(188.0, 403.0), (260.0, 403.0)]); // /128 → RTCSEL

    // ── Output frequency tags ────────────────────────────────────────────────
    tag(p, tf, 600.0, 150.0, &mhz(f.sysclk), over(f.sysclk, 72 * m), "SYSCLK");
    tag(p, tf, 765.0, 176.0, &mhz(f.hclk), over(f.hclk, 72 * m), "HCLK");
    tag(p, tf, 655.0, 226.0, &mhz(f.pclk1), over(f.pclk1, 36 * m), "PCLK1");
    tag(p, tf, 655.0, 330.0, &mhz(f.pclk2), over(f.pclk2, 72 * m), "PCLK2");
    tag(p, tf, 655.0, 410.0, &mhz(f.adcclk), over(f.adcclk, 14 * m), "ADCCLK");
    tag(p, tf, 590.0, 70.0, &mhz(f.usbclk), f.usbclk != 48 * m, "USBCLK");
    tag(p, tf, 360.0, 120.0, &mhz(f.pllclk), over(f.pllclk, 72 * m), "PLLCLK");
    let _ = c;

    // ── Legend ───────────────────────────────────────────────────────────────
    legend(p, tf);
}

fn legend(p: &egui::Painter, tf: &Tf) {
    let r = tf.r(640.0, 452.0, 240.0, 92.0);
    p.rect(
        r,
        3.0,
        Color32::from_rgb(34, 37, 44),
        Stroke::new(1.0, STROKE_C),
        egui::StrokeKind::Inside,
    );
    let lines = [
        "Legend",
        "HSE = high-speed external",
        "HSI = high-speed internal",
        "LSE = low-speed external",
        "LSI = low-speed internal",
    ];
    for (i, l) in lines.iter().enumerate() {
        let strong = i == 0;
        p.text(
            tf.p(650.0, 460.0 + i as f32 * 16.0),
            Align2::LEFT_TOP,
            *l,
            FontId::proportional(tf.fs(if strong { 11.0 } else { 9.5 })),
            if strong { LABEL_C } else { STROKE_C },
        );
    }
}

// ── Primitive painters ────────────────────────────────────────────────────────

fn block(p: &egui::Painter, tf: &Tf, x: f32, y: f32, w: f32, h: f32, title: &str) {
    let r = tf.r(x, y, w, h);
    p.rect(r, 3.0, BOX_FILL, Stroke::new(1.2, STROKE_C), egui::StrokeKind::Inside);
    p.text(
        r.center(),
        Align2::CENTER_CENTER,
        title,
        FontId::proportional(tf.fs(9.5)),
        LABEL_C,
    );
}

/// Just the outline of an interactive node (the ComboBox is drawn on top).
fn outline(p: &egui::Painter, tf: &Tf, x: f32, y: f32, w: f32, h: f32) {
    let r = tf.r(x, y, w, h);
    p.rect(r, 3.0, MUX_FILL, Stroke::new(1.0, ACCENT), egui::StrokeKind::Inside);
}

fn wire(p: &egui::Painter, tf: &Tf, pts: &[(f32, f32)]) {
    let stroke = Stroke::new(1.3, WIRE_C);
    for w in pts.windows(2) {
        p.line_segment([tf.p(w[0].0, w[0].1), tf.p(w[1].0, w[1].1)], stroke);
    }
    // arrowhead on the last segment
    if pts.len() >= 2 {
        let a = pts[pts.len() - 2];
        let b = pts[pts.len() - 1];
        arrowhead(p, tf, a, b, WIRE_C);
    }
}

fn arrowhead(p: &egui::Painter, tf: &Tf, from: (f32, f32), to: (f32, f32), color: Color32) {
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
    p.add(Shape::convex_polygon(vec![b, p1, p2], color, Stroke::NONE));
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

// ──────────────────────────────────────────────────────────────────────────────
// Interactive node helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Place a closure-built ComboBox at a virtual rect; returns the picked value.
fn node_combo<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    tf: &Tf,
    x: f32,
    y: f32,
    w: f32,
    id: &str,
    current_text: &str,
    options: impl Fn() -> Vec<(T, &'static str)>,
) -> Option<T> {
    let rect = tf.r(x, y, w, 26.0);
    let mut picked = None;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(egui::RichText::new(current_text).size(tf.fs(9.5)))
            .width(rect.width())
            .show_ui(ui, |ui| {
                for (v, label) in options() {
                    if ui.selectable_label(false, label).clicked() {
                        picked = Some(v);
                    }
                }
            });
    });
    picked
}

fn node_hse(ui: &mut egui::Ui, tf: &Tf, c: &mut Stm32f1Clock) -> bool {
    let rect = tf.r(30.0, 338.0, 100.0, 24.0);
    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        let mut mhz = c.hse_hz as f64 / 1e6;
        if ui
            .add(
                egui::DragValue::new(&mut mhz)
                    .range(1.0..=25.0)
                    .speed(0.1)
                    .suffix(" MHz"),
            )
            .changed()
        {
            c.hse_hz = (mhz * 1e6).round() as u32;
            changed = true;
        }
    });
    changed
}

fn node_mul(ui: &mut egui::Ui, tf: &Tf, c: &mut Stm32f1Clock) -> bool {
    let rect = tf.r(352.0, 156.0, 92.0, 26.0);
    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt("pllmul")
            .selected_text(egui::RichText::new(format!("×{}", c.pll_mul)).size(tf.fs(9.5)))
            .width(rect.width())
            .show_ui(ui, |ui| {
                for v in PLL_MUL_MIN..=PLL_MUL_MAX {
                    if ui.selectable_label(c.pll_mul == v, format!("×{v}")).clicked() {
                        c.pll_mul = v;
                        changed = true;
                    }
                }
            });
    });
    changed
}

fn node_sw(ui: &mut egui::Ui, tf: &Tf, c: &mut Stm32f1Clock) -> bool {
    if let Some(v) = node_combo(ui, tf, 470.0, 150.0, 92.0, "sw", sysclk_text(c.sysclk_src), || {
        vec![
            (SysclkSrc::Hsi, "HSI"),
            (SysclkSrc::Hse, "HSE"),
            (SysclkSrc::Pll, "PLL"),
        ]
    }) {
        if c.sysclk_src != v {
            c.sysclk_src = v;
            return true;
        }
    }
    false
}

fn node_usb(ui: &mut egui::Ui, tf: &Tf, c: &mut Stm32f1Clock) -> bool {
    if let Some(v) = node_combo(ui, tf, 470.0, 72.0, 110.0, "usbpre", usb_text(c.usb_pre), || {
        vec![(UsbPre::Div1_5, "/ 1.5"), (UsbPre::Div1, "/ 1")]
    }) {
        if c.usb_pre != v {
            c.usb_pre = v;
            return true;
        }
    }
    false
}

fn node_mco(ui: &mut egui::Ui, tf: &Tf, c: &mut Stm32f1Clock) -> bool {
    if let Some(v) = node_combo(ui, tf, 110.0, 506.0, 96.0, "mco", mco_text(c.mco), || {
        vec![
            (Mco::None, "MCO: off"),
            (Mco::Sysclk, "SYSCLK"),
            (Mco::Hsi, "HSI"),
            (Mco::Hse, "HSE"),
            (Mco::PllDiv2, "PLL/2"),
        ]
    }) {
        if c.mco != v {
            c.mco = v;
            return true;
        }
    }
    false
}

fn node_div_u16(
    ui: &mut egui::Ui,
    tf: &Tf,
    x: f32,
    y: f32,
    w: f32,
    id: &str,
    value: &mut u16,
    options: &[u16],
) -> bool {
    let rect = tf.r(x, y, w, 26.0);
    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(egui::RichText::new(format!("/ {value}")).size(tf.fs(9.5)))
            .width(rect.width())
            .show_ui(ui, |ui| {
                for &opt in options {
                    if ui.selectable_label(*value == opt, format!("/ {opt}")).clicked() {
                        *value = opt;
                        changed = true;
                    }
                }
            });
    });
    changed
}

fn node_div_u8(
    ui: &mut egui::Ui,
    tf: &Tf,
    x: f32,
    y: f32,
    w: f32,
    id: &str,
    value: &mut u8,
    options: &[u8],
) -> bool {
    let rect = tf.r(x, y, w, 26.0);
    let mut changed = false;
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(egui::RichText::new(format!("/ {value}")).size(tf.fs(9.5)))
            .width(rect.width())
            .show_ui(ui, |ui| {
                for &opt in options {
                    if ui.selectable_label(*value == opt, format!("/ {opt}")).clicked() {
                        *value = opt;
                        changed = true;
                    }
                }
            });
    });
    changed
}

// ── Label/format helpers ──────────────────────────────────────────────────────

fn pll_src_options() -> Vec<(PllSrc, &'static str)> {
    vec![
        (PllSrc::HsiDiv2, "HSI/2"),
        (PllSrc::Hse, "HSE"),
        (PllSrc::HseDiv2, "HSE/2"),
    ]
}
fn pll_src_text(v: PllSrc) -> &'static str {
    match v {
        PllSrc::HsiDiv2 => "HSI/2",
        PllSrc::Hse => "HSE",
        PllSrc::HseDiv2 => "HSE/2",
    }
}
fn sysclk_text(v: SysclkSrc) -> &'static str {
    match v {
        SysclkSrc::Hsi => "HSI",
        SysclkSrc::Hse => "HSE",
        SysclkSrc::Pll => "PLL",
    }
}
fn usb_text(v: UsbPre) -> &'static str {
    match v {
        UsbPre::Div1_5 => "/ 1.5",
        UsbPre::Div1 => "/ 1",
    }
}
fn mco_text(v: Mco) -> &'static str {
    match v {
        Mco::None => "MCO: off",
        Mco::Sysclk => "SYSCLK",
        Mco::Hsi => "HSI",
        Mco::Hse => "HSE",
        Mco::PllDiv2 => "PLL/2",
    }
}

fn mhz(hz: u32) -> String {
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
