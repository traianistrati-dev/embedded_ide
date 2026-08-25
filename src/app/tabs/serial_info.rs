//! The "Info" view of the Serial tab: a drawn explanation of how Bridge (MITM)
//! mode gets between an application and a device that are already talking.
//!
//! Painted with the egui painter rather than shipped as an image, for the same
//! reason the MCU diagram is: it stays sharp at any zoom, it costs no asset, and
//! the port names in it are the REAL ones from the current session — a picture
//! that says `COM20` when the user's pair is `COM6` teaches the wrong thing.

use eframe::egui;

/// Design-space size the layout is authored in; scaled to fit the panel.
const DES_W: f32 = 660.0;
const DES_H: f32 = 250.0;

/// One labelled box in design space.
struct Box {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    title: String,
    sub: String,
}

/// Draw the bridge explainer into the available width, `height` tall.
///
/// `device` / `app_side` / `ide_side` are the ports of the CURRENT session when
/// known, so the diagram doubles as a readout of what is actually wired up.
pub fn show_bridge_info(
    ui: &mut egui::Ui,
    height: f32,
    device: &str,
    app_side: &str,
    ide_side: &str,
    unix: bool,
) {
    let avail = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(avail, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);

    // Fit the design into the panel, never magnifying past 1:1 — a diagram
    // blown up to fill a tall panel reads as a mistake.
    let scale = (rect.width() / DES_W).min(rect.height() / DES_H).min(1.0);
    let ox = rect.left() + (rect.width() - DES_W * scale) / 2.0;
    let oy = rect.top() + (rect.height() - DES_H * scale).max(0.0) / 2.0;
    let p = |x: f32, y: f32| egui::pos2(ox + x * scale, oy + y * scale);
    let f = |size: f32| egui::FontId::proportional(size * scale);

    let dim = egui::Color32::from_rgb(150, 158, 172);
    let fg = egui::Color32::from_rgb(215, 220, 230);
    let border = egui::Color32::from_rgb(95, 102, 118);
    let fill = egui::Color32::from_rgb(38, 42, 50);

    let named = |label: &str, port: &str| {
        if port.is_empty() {
            format!("{label} —")
        } else {
            format!("{label} {port}")
        }
    };
    let pair_title = if unix {
        "socat PTY pair"
    } else {
        "com0com pair"
    };
    let boxes = [
        Box {
            x: 10.0,
            y: 10.0,
            w: 170.0,
            h: 52.0,
            title: "Other application".into(),
            sub: named("opens", app_side),
        },
        Box {
            x: 450.0,
            y: 10.0,
            w: 200.0,
            h: 52.0,
            title: "Device".into(),
            sub: named("real port", device),
        },
        Box {
            x: 10.0,
            y: 110.0,
            w: 290.0,
            h: 120.0,
            title: String::new(),
            sub: pair_title.into(),
        },
        Box {
            x: 370.0,
            y: 110.0,
            w: 280.0,
            h: 120.0,
            title: "Embedded IDE".into(),
            sub: "relays every byte".into(),
        },
    ];

    for b in &boxes {
        let r = egui::Rect::from_min_max(p(b.x, b.y), p(b.x + b.w, b.y + b.h));
        painter.rect_filled(r, 6.0 * scale, fill);
        painter.rect_stroke(
            r,
            6.0 * scale,
            egui::Stroke::new(1.2_f32, border),
            egui::StrokeKind::Middle,
        );
        if !b.title.is_empty() {
            painter.text(
                p(b.x + b.w / 2.0, b.y + 18.0),
                egui::Align2::CENTER_CENTER,
                &b.title,
                f(13.0),
                fg,
            );
        }
    }
    // Sub-labels: inside the small boxes, at the FOOT of the two big ones (their
    // middle is occupied by the inner boxes / the log legend).
    painter.text(
        p(95.0, 48.0),
        egui::Align2::CENTER_CENTER,
        &boxes[0].sub,
        f(11.0),
        dim,
    );
    painter.text(
        p(550.0, 48.0),
        egui::Align2::CENTER_CENTER,
        &boxes[1].sub,
        f(11.0),
        dim,
    );
    painter.text(
        p(155.0, 216.0),
        egui::Align2::CENTER_CENTER,
        &boxes[2].sub,
        f(11.0),
        dim,
    );
    painter.text(
        p(510.0, 152.0),
        egui::Align2::CENTER_CENTER,
        &boxes[3].sub,
        f(11.0),
        dim,
    );

    // The two ends of the virtual pair, inside the pair box.
    let ends = [
        (
            30.0,
            if app_side.is_empty() {
                "app side"
            } else {
                app_side
            },
        ),
        (
            170.0,
            if ide_side.is_empty() {
                "IDE side"
            } else {
                ide_side
            },
        ),
    ];
    for (x, label) in ends {
        let r = egui::Rect::from_min_max(p(x, 140.0), p(x + 100.0, 190.0));
        painter.rect_filled(r, 4.0 * scale, egui::Color32::from_rgb(46, 52, 62));
        painter.rect_stroke(
            r,
            4.0 * scale,
            egui::Stroke::new(1.0_f32, border),
            egui::StrokeKind::Middle,
        );
        painter.text(
            p(x + 50.0, 165.0),
            egui::Align2::CENTER_CENTER,
            label,
            f(12.0),
            fg,
        );
    }

    // Every link is two-way: a reply travels back the road the command came in.
    let link = |a: egui::Pos2, b: egui::Pos2| {
        painter.line_segment([a, b], egui::Stroke::new(1.3_f32, dim));
        arrow_head(&painter, a, b, dim);
        arrow_head(&painter, b, a, dim);
    };
    link(p(95.0, 66.0), p(80.0, 136.0));
    link(p(134.0, 165.0), p(166.0, 165.0));
    link(p(304.0, 165.0), p(366.0, 165.0));
    link(p(550.0, 106.0), p(550.0, 66.0));

    // The log legend, in the IDE box, in the colours the log itself uses.
    for (i, (col, label)) in [
        (crate::serial::DIR_APP, ">>  app to device"),
        (crate::serial::DIR_SENSOR, "<<  device to app"),
    ]
    .into_iter()
    .enumerate()
    {
        let y = 176.0 + i as f32 * 22.0;
        painter.rect_filled(
            egui::Rect::from_min_max(p(398.0, y - 5.0), p(408.0, y + 5.0)),
            2.0 * scale,
            col,
        );
        painter.text(p(416.0, y), egui::Align2::LEFT_CENTER, label, f(11.0), col);
    }
}

/// Two short strokes forming an open arrowhead at `to`, pointing away from
/// `from`. (Same construction as the Pins-canvas wires.)
fn arrow_head(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, color: egui::Color32) {
    let v = to - from;
    if v.length() < 1.0 {
        return;
    }
    let dir = v.normalized();
    let rot = egui::emath::Rot2::from_angle(std::f32::consts::TAU / 12.0);
    let stroke = egui::Stroke::new(1.3_f32, color);
    painter.line_segment([to, to - 7.0 * (rot * dir)], stroke);
    painter.line_segment([to, to - 7.0 * (rot.inverse() * dir)], stroke);
}
