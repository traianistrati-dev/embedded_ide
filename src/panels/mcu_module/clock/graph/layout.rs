//! Data description of a clock diagram's **static layer** (Phase 3).
//!
//! The hand-tuned Figure-2 positions that used to be hardcoded in
//! `gui/diagram.rs` now live here as data: labelled blocks, right-margin output
//! boxes, on-chain frequency tags, node labels, mux titles and routed wires.
//! `gui/diagram.rs` renders by iterating this structure (reusing the same
//! CubeMX-style primitives), so a chip can ship a different layout to get a
//! different diagram — the answer to "import the clock diagram per MCU".
//!
//! Coordinates are in a virtual space whose extent is whatever the content
//! measures ([`ClockLayout::bounds`]) — the hand-authored figures were drawn
//! against 1000×790, but a layout may be any size. Values shown in boxes/tags
//! are *not* stored — each carries a [`ValueSrc`] resolved live from the
//! graph-evaluated frequencies, so the diagram stays correct as the user edits
//! nodes.

use serde::{Deserialize, Serialize};

use super::model::{LimitKey, NodeState};
use crate::panels::mcu_module::clock::model::ClockLimits;

/// Which frequency a box/tag displays. Resolved at draw time from the evaluated
/// graph frequencies. The named variants are F103 conveniences; [`ValueSrc::Node`]
/// references an arbitrary graph node by id, so any chip's layout works.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueSrc {
    Hclk,
    Sysclk,
    Pclk1,
    Pclk2,
    /// APB1 timer clock (×2 rule).
    Pclk1Tim,
    /// APB2 timer clock (×2 rule).
    Pclk2Tim,
    Adc,
    Usb,
    Pllclk,
    Flitf,
    /// SysTick (honours the SysTick source mux).
    Systick,
    /// RTC clock (honours RTCSEL).
    Rtc,
    /// MCO pin (honours the MCO mux).
    Mco,
    /// An arbitrary graph node, by id — for chips beyond STM32F1.
    Node(String),
    /// A constant (e.g. IWDG = LSI 40 kHz).
    Fixed(u32),
}

/// A static labelled rectangle (oscillators, fixed dividers).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockDef {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub label: String,
    /// Which graph node this box DRAWS, when it draws one.
    ///
    /// Editing a hand-authored figure needs to know who owns each primitive:
    /// without it a source box is just a label at coordinates, so the editor
    /// cannot find it, move it, or delete it with its node — which is why
    /// converting a figure used to scatter every un-owned box below the diagram.
    /// `None` is legitimate for pure decoration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

/// A delivered-clock box on the right margin (value + label, red over limit).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputDef {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub label: String,
    pub src: ValueSrc,
    pub limit: Option<LimitKey>,
}

/// An on-chain frequency tag ("NAME value").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TagDef {
    pub x: f32,
    pub y: f32,
    pub name: String,
    pub src: ValueSrc,
    pub limit: Option<LimitKey>,
}

/// A free-standing text label (node names above dropdowns; mux titles).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabelDef {
    pub x: f32,
    pub y: f32,
    pub text: String,
    /// The node this text belongs to (a mux's title, a node's name), so it
    /// travels with it. `None` for free-standing legend text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

/// An interactive control overlaid on the diagram, editing one graph node's
/// state. Kept deliberately simple + uniform: a dropdown whose options each
/// carry the [`NodeState`] to apply (so muxes, dividers and multipliers are all
/// just "pick an option"). Position is in virtual-canvas coords.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Widget {
    /// Dropdown over `options` (label → state) bound to node `node`.
    Combo {
        node: String,
        x: f32,
        y: f32,
        w: f32,
        options: Vec<(String, NodeState)>,
    },
    /// CubeMX-style trapezoid mux with one radio button per input — same look
    /// as the hand-tuned F103 muxes. `inputs` = (label, dy from `y` where the
    /// input enters, state to apply on pick). `flip` mirrors it horizontally
    /// (inputs on the right, output to the left — e.g. MCO).
    MuxRadios {
        node: String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        flip: bool,
        inputs: Vec<(String, f32, NodeState)>,
    },
    /// Drag-editable frequency (MHz) for a `Source` node (e.g. the HSE
    /// crystal). Keeps the node's `enabled` flag untouched.
    DragMhz {
        node: String,
        x: f32,
        y: f32,
        w: f32,
        min_mhz: f32,
        max_mhz: f32,
    },
}

impl Widget {
    /// The graph node this control edits.
    pub fn node_id(&self) -> &str {
        match self {
            Widget::Combo { node, .. }
            | Widget::MuxRadios { node, .. }
            | Widget::DragMhz { node, .. } => node,
        }
    }
}

/// Where one graph node sits on the canvas.
///
/// This is the **source of truth for a derived layout**: the label, the control
/// and the frequency tag are all generated from it
/// ([`super::auto_layout::derive`]), so moving a box moves the whole node — the
/// primitives below are a cache, not something to edit by hand. The
/// hand-authored figures (F1/F4/WBA/ESP) place their primitives directly and
/// carry no boxes at all.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeBox {
    /// The [`super::model::Node`] id this box belongs to.
    pub node: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The complete static diagram description.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClockLayout {
    /// Node positions, for layouts that are DERIVED from the graph (auto-laid-out
    /// or drawn in the editor). Empty for the hand-authored figures — and then
    /// omitted from the `.ron` entirely, so those files don't change at all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeBox>,
    pub blocks: Vec<BlockDef>,
    pub outputs: Vec<OutputDef>,
    pub tags: Vec<TagDef>,
    /// Node-name labels, drawn LEFT_BOTTOM-anchored above their dropdowns.
    pub labels_above: Vec<LabelDef>,
    /// Mux titles, drawn CENTER_BOTTOM-anchored above each mux.
    pub mux_titles: Vec<LabelDef>,
    /// Routed wire polylines (arrowhead on the last segment).
    pub wires: Vec<Vec<(f32, f32)>>,
    /// Interactive controls (dropdowns) editing graph node states.
    #[serde(default)]
    pub widgets: Vec<Widget>,
}

impl ClockLayout {
    /// `true` when the layout carries no drawable primitives at all — the cue
    /// to auto-generate one from the graph topology (see
    /// [`super::auto_layout::auto_layout`]). An AI-imported clock lands here.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.blocks.is_empty()
            && self.outputs.is_empty()
            && self.tags.is_empty()
            && self.labels_above.is_empty()
            && self.mux_titles.is_empty()
            && self.wires.is_empty()
            && self.widgets.is_empty()
    }

    /// A box per node the figure can LOCATE — the handles edit mode drags.
    ///
    /// This is what lets a hand-authored figure be edited **in place**. Every
    /// primitive that names a node contributes to that node's bounding box:
    /// controls, owned blocks and labels, and the outputs/tags whose
    /// [`ValueSrc::Node`] already named one. A node the figure never draws is
    /// simply not returned — it has no handle, rather than being relocated.
    ///
    /// Unlike [`nodes`](Self::nodes) these are DERIVED each time; they are not
    /// the source of truth, the primitives are.
    pub fn node_anchors(&self) -> Vec<NodeBox> {
        use std::collections::BTreeMap;
        // Owned keys: the primitives are borrowed from `self` for the whole walk,
        // and one `String` per node is nothing next to redrawing the diagram.
        let mut acc: BTreeMap<String, (f32, f32, f32, f32)> = BTreeMap::new();
        let mut add = |id: &str, x: f32, y: f32, w: f32, h: f32| {
            if id.is_empty() {
                return;
            }
            let e = acc
                .entry(id.to_owned())
                .or_insert((f32::MAX, f32::MAX, f32::MIN, f32::MIN));
            e.0 = e.0.min(x);
            e.1 = e.1.min(y);
            e.2 = e.2.max(x + w);
            e.3 = e.3.max(y + h);
        };
        fn node_of(src: &ValueSrc) -> Option<&str> {
            match src {
                ValueSrc::Node(id) => Some(id.as_str()),
                _ => None,
            }
        }

        for b in &self.blocks {
            if let Some(n) = &b.node {
                add(n, b.x, b.y, b.w, b.h);
            }
        }
        for l in self.labels_above.iter().chain(&self.mux_titles) {
            if let Some(n) = &l.node {
                add(n, l.x, l.y, 0.0, 0.0);
            }
        }
        for o in &self.outputs {
            if let Some(n) = node_of(&o.src) {
                add(n, o.x, o.y, o.w, o.h);
            }
        }
        for t in &self.tags {
            if let Some(n) = node_of(&t.src) {
                add(n, t.x, t.y, 0.0, 12.0);
            }
        }
        for w in &self.widgets {
            let (x, y, ww, hh) = match w {
                Widget::Combo { x, y, w, .. } => (*x, *y, *w, 26.0),
                Widget::DragMhz { x, y, w, .. } => (*x, *y, *w, 22.0),
                Widget::MuxRadios { x, y, w, h, .. } => (*x, *y, *w, *h),
            };
            add(w.node_id(), x, y, ww, hh);
        }

        acc.into_iter()
            .map(|(node, (x0, y0, x1, y1))| NodeBox {
                node,
                x: x0,
                y: y0,
                w: (x1 - x0).max(24.0),
                h: (y1 - y0).max(18.0),
            })
            .collect()
    }

    /// Translate every primitive that belongs to `node` — wires included.
    ///
    /// A wire is a bare polyline: it records no owner, and the hand-drawn figures
    /// were authored long before anything needed one. Rather than change the
    /// stored format (which would invalidate every `.ron` already written), the
    /// attachment is worked out from GEOMETRY: a wire end sitting on this node's
    /// box is this node's end. Only that end moves, and its neighbouring bend
    /// follows just enough to keep the segment orthogonal — so the other end
    /// stays put and the route is not redrawn.
    pub fn move_node(&mut self, node: &str, dx: f32, dy: f32) {
        // Measured BEFORE the primitives move, since that is where the wires
        // are still attached.
        let anchor = self.node_anchors().into_iter().find(|a| a.node == node);
        let owns = |n: &Option<String>| n.as_deref() == Some(node);
        let owns_src = |src: &ValueSrc| matches!(src, ValueSrc::Node(id) if id == node);

        for b in &mut self.blocks {
            if owns(&b.node) {
                b.x += dx;
                b.y += dy;
            }
        }
        for l in self.labels_above.iter_mut().chain(&mut self.mux_titles) {
            if owns(&l.node) {
                l.x += dx;
                l.y += dy;
            }
        }
        for o in &mut self.outputs {
            if owns_src(&o.src) {
                o.x += dx;
                o.y += dy;
            }
        }
        for t in &mut self.tags {
            if owns_src(&t.src) {
                t.x += dx;
                t.y += dy;
            }
        }
        for w in &mut self.widgets {
            if w.node_id() != node {
                continue;
            }
            match w {
                Widget::Combo { x, y, .. }
                | Widget::DragMhz { x, y, .. }
                | Widget::MuxRadios { x, y, .. } => {
                    *x += dx;
                    *y += dy;
                }
            }
        }
        for nb in &mut self.nodes {
            if nb.node == node {
                nb.x += dx;
                nb.y += dy;
            }
        }
        if let Some(a) = anchor {
            self.move_wire_ends_at(&a, dx, dy);
        }
    }

    /// Drag along the wire ends that sit on `a`, keeping the routes orthogonal.
    fn move_wire_ends_at(&mut self, a: &NodeBox, dx: f32, dy: f32) {
        /// How far off the box an end may sit and still count as attached. A
        /// wire meets its node ON the boundary, so this only absorbs the small
        /// insets the figures draw with.
        const TOL: f32 = 14.0;
        let attached = |p: (f32, f32)| {
            p.0 >= a.x - TOL && p.0 <= a.x + a.w + TOL && p.1 >= a.y - TOL && p.1 <= a.y + a.h + TOL
        };
        for wire in &mut self.wires {
            if wire.len() < 2 {
                continue;
            }
            let last = wire.len() - 1;
            // Both ends first, so a wire attached at both is simply translated.
            let (head, tail) = (attached(wire[0]), attached(wire[last]));
            if head {
                shift_wire_end(wire, 0, dx, dy);
            }
            if tail {
                shift_wire_end(wire, last, dx, dy);
            }
        }
    }

    /// Drop every primitive that belongs to `node` — used when the editor
    /// deletes it from a hand-authored figure.
    pub fn remove_node_primitives(&mut self, node: &str) {
        let keep = |n: &Option<String>| n.as_deref() != Some(node);
        let keep_src = |src: &ValueSrc| !matches!(src, ValueSrc::Node(id) if id == node);
        self.blocks.retain(|b| keep(&b.node));
        self.labels_above.retain(|l| keep(&l.node));
        self.mux_titles.retain(|l| keep(&l.node));
        self.outputs.retain(|o| keep_src(&o.src));
        self.tags.retain(|t| keep_src(&t.src));
        self.widgets.retain(|w| w.node_id() != node);
        self.nodes.retain(|nb| nb.node != node);
    }

    /// The drawn extent `(w, h)` in virtual coordinates — every primitive plus a
    /// margin.
    ///
    /// This replaced the renderer's fixed 1000×790 canvas. That constant was not
    /// a *size* but the denominator of a fit-to-viewport scale, so a diagram
    /// bigger than the STM32F103 figure did not get more room — it got drawn
    /// smaller. Now a layout is as large as it needs to be and the pan/zoom
    /// [`Scene`](eframe::egui::Scene) owns the view. The hand-authored F1/F4/WBA
    /// layouts measure back at ≈1000×790, so they are unchanged.
    ///
    /// Text widths are estimated (≈0.55 em at the sizes `gui::diagram` uses) —
    /// this sizes a canvas, it does not need to be exact.
    pub fn bounds(&self) -> (f32, f32) {
        /// Right-hand allowance for a label drawn from `x`.
        fn text_w(s: &str, pt: f32) -> f32 {
            s.chars().count() as f32 * pt * 0.55
        }
        let mut w: f32 = 0.0;
        let mut h: f32 = 0.0;
        let mut fit = |x: f32, y: f32| {
            w = w.max(x);
            h = h.max(y);
        };

        for nb in &self.nodes {
            fit(nb.x + nb.w, nb.y + nb.h);
        }
        for b in &self.blocks {
            fit(b.x + b.w, b.y + b.h);
        }
        for o in &self.outputs {
            fit(o.x + o.w, o.y + o.h);
        }
        for t in &self.tags {
            // "NAME 72 MHz" — the name plus a value of its own.
            fit(t.x + text_w(&t.name, 9.0) + 60.0, t.y + 14.0);
        }
        for l in self.labels_above.iter().chain(&self.mux_titles) {
            fit(l.x + text_w(&l.text, 9.0), l.y + 12.0);
        }
        for poly in &self.wires {
            for &(x, y) in poly {
                fit(x, y);
            }
        }
        for wg in &self.widgets {
            match wg {
                Widget::Combo { x, y, w: cw, .. } => fit(x + cw, y + 26.0),
                Widget::DragMhz { x, y, w: dw, .. } => fit(x + dw, y + 22.0),
                Widget::MuxRadios {
                    x,
                    y,
                    w: mw,
                    h: mh,
                    flip,
                    inputs,
                    ..
                } => {
                    // A flipped mux carries its input stubs + labels on the RIGHT.
                    let stubs = if *flip {
                        26.0 + inputs
                            .iter()
                            .map(|(l, _, _)| text_w(l, 8.0))
                            .fold(0.0, f32::max)
                    } else {
                        0.0
                    };
                    fit(x + mw + stubs, y + mh);
                }
            }
        }

        const PAD: f32 = 24.0;
        (w + PAD, h + PAD)
    }
}

/// Move one end of a routed wire, taking its first bend along far enough to keep
/// the segment orthogonal.
///
/// A route is a chain of horizontal and vertical runs. Moving only the endpoint
/// would tilt the run it belongs to; moving the whole polyline would drag the
/// far end off its own node. So the neighbouring point follows in the ONE axis
/// that run is aligned on — vertical run: take `dx`; horizontal run: take `dy`.
fn shift_wire_end(wire: &mut [(f32, f32)], idx: usize, dx: f32, dy: f32) {
    let neighbour = if idx == 0 { 1 } else { idx - 1 };
    let (ex, ey) = wire[idx];
    let (nx, ny) = wire[neighbour];
    let vertical = (nx - ex).abs() < f32::EPSILON;
    let horizontal = (ny - ey).abs() < f32::EPSILON;

    wire[idx] = (ex + dx, ey + dy);
    if vertical {
        wire[neighbour].0 += dx;
    }
    if horizontal {
        wire[neighbour].1 += dy;
    }
}

const M: u32 = 1_000_000;

/// The STM32F103 Figure-2 static layout (ported verbatim from the original
/// `gui/diagram.rs`). Takes `limits` only to print the HSE crystal range.
pub fn stm32f1_layout(limits: &ClockLimits) -> ClockLayout {
    let blk = |x, y, w, h, label: &str, node: &str| BlockDef {
        x,
        y,
        w,
        h,
        label: label.to_owned(),
        node: (!node.is_empty()).then(|| node.to_owned()),
    };
    let out = |x, y, w, h, label: &str, src, limit| OutputDef {
        x,
        y,
        w,
        h,
        label: label.to_owned(),
        src,
        limit,
    };
    let tag = |x, y, name: &str, src, limit| TagDef {
        x,
        y,
        name: name.to_owned(),
        src,
        limit,
    };
    let lbl = |x, y, text: &str, node: &str| LabelDef {
        x,
        y,
        text: text.to_owned(),
        node: (!node.is_empty()).then(|| node.to_owned()),
    };
    let combo = |node: &str, x, y, w, options: Vec<(String, NodeState)>| Widget::Combo {
        node: node.to_owned(),
        x,
        y,
        w,
        options,
    };
    let mux =
        |node: &str, x, y, w, h, flip, inputs: Vec<(String, f32, NodeState)>| Widget::MuxRadios {
            node: node.to_owned(),
            x,
            y,
            w,
            h,
            flip,
            inputs,
        };
    // One trapezoid input: (label, dy where the wire enters, mux index to pick).
    let mi = |label: &str, dy: f32, i: usize| (label.to_owned(), dy, NodeState::Index(i));
    // Index-based options for a divider (`/N`), and value-based for the PLL mul.
    let div_opts = |vals: &[u32]| -> Vec<(String, NodeState)> {
        vals.iter()
            .enumerate()
            .map(|(i, v)| (format!("/ {v}"), NodeState::Index(i)))
            .collect()
    };
    let mul_opts: Vec<(String, NodeState)> = (2..=16u32)
        .map(|v| (format!("×{v}"), NodeState::Value(v)))
        .collect();
    let s = |label: &str, state: NodeState| (label.to_owned(), state);

    ClockLayout {
        // Hand-authored: the primitives below ARE the layout, so there are
        // no node boxes to derive them from.
        nodes: Vec::new(),
        blocks: vec![
            blk(28.0, 78.0, 92.0, 34.0, "LSE OSC\n32.768 kHz", "lse"),
            blk(170.0, 84.0, 46.0, 22.0, "/128", "hse_div128"),
            blk(28.0, 153.0, 92.0, 34.0, "LSI RC\n40 kHz", "lsi"),
            blk(28.0, 283.0, 92.0, 34.0, "HSI RC\n8 MHz", "hsi"),
            blk(175.0, 372.0, 40.0, 22.0, "/2", "hsi_div2"),
            blk(
                28.0,
                483.0,
                92.0,
                34.0,
                &format!(
                    "HSE OSC\n{}–{} MHz",
                    limits.hse_min_hz / M,
                    limits.hse_max_hz / M
                ),
                "hse",
            ),
        ],
        outputs: vec![
            out(
                820.0,
                109.0,
                160.0,
                26.0,
                "RTCCLK -> RTC",
                ValueSrc::Rtc,
                None,
            ),
            out(
                820.0,
                149.0,
                160.0,
                26.0,
                "IWDGCLK <- LSI",
                ValueSrc::Fixed(40_000),
                None,
            ),
            out(
                820.0,
                232.0,
                160.0,
                26.0,
                "USBCLK -> USB",
                ValueSrc::Usb,
                None,
            ),
            out(
                820.0,
                272.0,
                160.0,
                26.0,
                "FLITFCLK <- HSI",
                ValueSrc::Flitf,
                None,
            ),
            out(
                820.0,
                346.0,
                160.0,
                28.0,
                "HCLK -> AHB / core / DMA",
                ValueSrc::Hclk,
                Some(LimitKey::HclkMax),
            ),
            out(
                820.0,
                416.0,
                160.0,
                28.0,
                "Cortex SysTick",
                ValueSrc::Systick,
                None,
            ),
            out(
                820.0,
                456.0,
                160.0,
                28.0,
                "FCLK (free-running)",
                ValueSrc::Hclk,
                None,
            ),
            out(
                820.0,
                529.0,
                160.0,
                28.0,
                "APB1 peripherals",
                ValueSrc::Pclk1,
                Some(LimitKey::Pclk1Max),
            ),
            out(
                820.0,
                576.0,
                160.0,
                28.0,
                "APB1 timers",
                ValueSrc::Pclk1Tim,
                None,
            ),
            out(
                820.0,
                649.0,
                160.0,
                28.0,
                "APB2 peripherals",
                ValueSrc::Pclk2,
                Some(LimitKey::Pclk2Max),
            ),
            out(
                820.0,
                696.0,
                160.0,
                28.0,
                "APB2 timers",
                ValueSrc::Pclk2Tim,
                None,
            ),
            out(
                820.0,
                749.0,
                160.0,
                28.0,
                "ADC1/2",
                ValueSrc::Adc,
                Some(LimitKey::AdcclkMax),
            ),
            out(28.0, 625.0, 106.0, 26.0, "MCO pin", ValueSrc::Mco, None),
        ],
        tags: vec![
            tag(
                460.0,
                442.0,
                "PLLCLK",
                ValueSrc::Pllclk,
                Some(LimitKey::SysclkMax),
            ),
            tag(
                516.0,
                344.0,
                "SYSCLK",
                ValueSrc::Sysclk,
                Some(LimitKey::SysclkMax),
            ),
            tag(
                592.0,
                344.0,
                "HCLK",
                ValueSrc::Hclk,
                Some(LimitKey::HclkMax),
            ),
            tag(
                792.0,
                524.0,
                "PCLK1",
                ValueSrc::Pclk1,
                Some(LimitKey::Pclk1Max),
            ),
            tag(
                792.0,
                644.0,
                "PCLK2",
                ValueSrc::Pclk2,
                Some(LimitKey::Pclk2Max),
            ),
        ],
        labels_above: vec![
            lbl(148.0, 478.0, "PLLXTPRE", "pllxtpre"),
            lbl(336.0, 418.0, "PLLMUL", "pllmul"),
            lbl(470.0, 228.0, "USB Prescaler", "usb"),
            lbl(540.0, 335.0, "AHB Prescaler", "ahb"),
            lbl(700.0, 408.0, "SysTick", "systick"),
            lbl(720.0, 518.0, "APB1 Prescaler", "apb1"),
            lbl(720.0, 638.0, "APB2 Prescaler", "apb2"),
            lbl(720.0, 738.0, "ADC Prescaler", "adc"),
            lbl(28.0, 521.0, "HSE crystal", "hse"),
        ],
        mux_titles: vec![
            lbl(270.0, 64.0, "RTC Mux", "rtc"),
            lbl(270.0, 364.0, "PLL Source", "pllsrc"),
            lbl(490.0, 294.0, "System Clock Mux", "sw"),
            lbl(270.0, 574.0, "MCO Mux", "mco"),
        ],
        wires: vec![
            vec![(120.0, 300.0), (120.0, 383.0), (175.0, 383.0)], // HSI → /2 (single bend)
            vec![(215.0, 383.0), (226.0, 383.0)],
            vec![(120.0, 500.0), (148.0, 500.0)],
            vec![(208.0, 502.0), (226.0, 502.0)],
            vec![(290.0, 442.0), (336.0, 442.0)],
            vec![(420.0, 442.0), (456.0, 442.0)], // PLLMUL → PLLCLK (tag now at 460,442)
            vec![(510.0, 360.0), (540.0, 360.0)],
            vec![(626.0, 360.0), (640.0, 360.0)],
            vec![(640.0, 360.0), (818.0, 360.0)],
            vec![(640.0, 360.0), (640.0, 700.0)],
            vec![(640.0, 430.0), (700.0, 430.0)],
            vec![(766.0, 430.0), (818.0, 430.0)],
            vec![(640.0, 470.0), (818.0, 470.0)],
            vec![(640.0, 543.0), (720.0, 543.0)],
            vec![(786.0, 543.0), (818.0, 543.0)],
            vec![(806.0, 556.0), (806.0, 590.0), (818.0, 590.0)],
            vec![(640.0, 663.0), (720.0, 663.0)],
            vec![(786.0, 663.0), (818.0, 663.0)],
            vec![(806.0, 676.0), (806.0, 710.0), (818.0, 710.0)],
            vec![(753.0, 676.0), (753.0, 763.0), (720.0, 763.0)],
            vec![(786.0, 763.0), (818.0, 763.0)],
            vec![(290.0, 122.0), (818.0, 122.0)],
            vec![(560.0, 245.0), (818.0, 245.0)],
            vec![(250.0, 638.0), (134.0, 638.0)],
        ],
        // Interactive controls for the F103 *graph* path — same look and the
        // SAME positions as the typed path's `interactive_nodes`: trapezoid
        // mux radios for the four muxes, a drag-MHz for the HSE crystal, and
        // dropdowns for the prescalers (the typed path ignores these).
        widgets: vec![
            mux(
                "rtc",
                250.0,
                72.0,
                40.0,
                100.0,
                false,
                vec![
                    mi("HSE/128", 28.0, 0),
                    mi("LSE", 50.0, 1),
                    mi("LSI", 72.0, 2),
                ],
            ),
            mux(
                "pllsrc",
                250.0,
                370.0,
                40.0,
                145.0,
                false,
                vec![mi("HSI/2", 13.0, 0), mi("HSE", 132.0, 1)],
            ),
            mux(
                "sw",
                470.0,
                300.0,
                40.0,
                120.0,
                false,
                vec![
                    mi("HSI", 24.0, 0),
                    mi("HSE", 60.0, 1),
                    mi("PLLCLK", 96.0, 2),
                ],
            ),
            mux(
                "mco",
                250.0,
                580.0,
                40.0,
                116.0,
                true,
                vec![
                    mi("SYSCLK", 20.0, 0),
                    mi("HSI", 48.0, 1),
                    mi("HSE", 76.0, 2),
                    mi("PLL/2", 104.0, 3),
                ],
            ),
            Widget::DragMhz {
                node: "hse".to_owned(),
                x: 28.0,
                y: 533.0,
                w: 92.0,
                min_mhz: 1.0,
                max_mhz: 25.0,
            },
            combo(
                "pllxtpre",
                150.0,
                490.0,
                60.0,
                vec![s("/ 1", NodeState::Index(0)), s("/ 2", NodeState::Index(1))],
            ),
            combo("pllmul", 336.0, 430.0, 84.0, mul_opts),
            combo(
                "ahb",
                540.0,
                347.0,
                86.0,
                div_opts(&[1, 2, 4, 8, 16, 64, 128, 256, 512]),
            ),
            combo("apb1", 720.0, 530.0, 66.0, div_opts(&[1, 2, 4, 8, 16])),
            combo("apb2", 720.0, 650.0, 66.0, div_opts(&[1, 2, 4, 8, 16])),
            combo("adc", 720.0, 750.0, 66.0, div_opts(&[2, 4, 6, 8])),
            combo(
                "usb",
                470.0,
                232.0,
                90.0,
                vec![
                    s("/ 1.5", NodeState::Index(0)),
                    s("/ 1", NodeState::Index(1)),
                ],
            ),
            combo(
                "systick",
                700.0,
                418.0,
                66.0,
                vec![s("/ 8", NodeState::Index(0)), s("/ 1", NodeState::Index(1))],
            ),
        ],
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The hand-drawn F103 figure can be EDITED IN PLACE: every node it draws
    /// gets a handle, and moving one takes its own primitives along and nothing
    /// else. This is what replaced converting the figure to a generated one,
    /// which used to scatter every box the seed could not identify.
    #[test]
    fn a_hand_authored_figure_anchors_and_moves_its_nodes() {
        let mut lay = stm32f1_layout(&ClockLimits::default());
        assert!(lay.nodes.is_empty(), "a hand-authored layout owns no boxes");

        let anchors = lay.node_anchors();
        for id in [
            "hse", "hsi", "lse", "lsi", "sw", "pllmul", "ahb", "apb1", "rtc", "mco",
        ] {
            assert!(
                anchors.iter().any(|a| a.node == id),
                "`{id}` should have a handle: {:?}",
                anchors.iter().map(|a| &a.node).collect::<Vec<_>>()
            );
        }

        // Moving one node takes its block, its label and its widget with it…
        let hse_block = |l: &ClockLayout| {
            let b = l
                .blocks
                .iter()
                .find(|b| b.node.as_deref() == Some("hse"))
                .unwrap();
            (b.x, b.y)
        };
        let hse_widget =
            |l: &ClockLayout| match l.widgets.iter().find(|w| w.node_id() == "hse").unwrap() {
                Widget::Combo { x, y, .. }
                | Widget::DragMhz { x, y, .. }
                | Widget::MuxRadios { x, y, .. } => (*x, *y),
            };
        let (b0, w0) = (hse_block(&lay), hse_widget(&lay));
        // …and leaves a different node exactly where it was.
        let hsi0 = hse_block(&lay);
        let lsi_block = |l: &ClockLayout| {
            let b = l
                .blocks
                .iter()
                .find(|b| b.node.as_deref() == Some("lsi"))
                .unwrap();
            (b.x, b.y)
        };
        let lsi0 = lsi_block(&lay);
        let _ = hsi0;

        lay.move_node("hse", 40.0, -15.0);
        assert_eq!(hse_block(&lay), (b0.0 + 40.0, b0.1 - 15.0));
        assert_eq!(hse_widget(&lay), (w0.0 + 40.0, w0.1 - 15.0));
        assert_eq!(lsi_block(&lay), lsi0, "an unrelated node must not move");
    }

    /// Deleting a node from a hand-drawn figure removes what drew it — and only
    /// that.
    #[test]
    fn removing_a_node_takes_only_its_own_primitives() {
        let mut lay = stm32f1_layout(&ClockLimits::default());
        let before = lay.blocks.len() + lay.widgets.len() + lay.outputs.len();

        lay.remove_node_primitives("hse");
        assert!(!lay.blocks.iter().any(|b| b.node.as_deref() == Some("hse")));
        assert!(!lay.widgets.iter().any(|w| w.node_id() == "hse"));
        assert!(lay.node_anchors().iter().all(|a| a.node != "hse"));

        let after = lay.blocks.len() + lay.widgets.len() + lay.outputs.len();
        assert!(after < before, "something was removed");
        assert!(
            lay.blocks.iter().any(|b| b.node.as_deref() == Some("lsi")),
            "the rest of the figure survives"
        );
    }

    /// A node's wires travel with it: the attached END moves, the far end stays
    /// on its own node, and the route stays orthogonal.
    #[test]
    fn wires_follow_the_node_they_are_attached_to() {
        let mut lay = ClockLayout {
            blocks: vec![BlockDef {
                x: 100.0,
                y: 100.0,
                w: 60.0,
                h: 20.0,
                label: "src".into(),
                node: Some("src".into()),
            }],
            // Leaves the box's right edge, bends, ends far away on another node.
            wires: vec![vec![
                (160.0, 110.0),
                (200.0, 110.0),
                (200.0, 300.0),
                (400.0, 300.0),
            ]],
            ..Default::default()
        };

        lay.move_node("src", 30.0, -10.0);
        let w = &lay.wires[0];
        assert_eq!(w[0], (190.0, 100.0), "the attached end moved with the node");
        assert_eq!(
            w[1],
            (200.0, 100.0),
            "the bend took the vertical run's dy so the first run stays horizontal"
        );
        assert_eq!(w[2], (200.0, 300.0), "the middle of the route is untouched");
        assert_eq!(
            w[3],
            (400.0, 300.0),
            "and the far end stays on its own node"
        );
        // Still orthogonal end to end.
        for pair in w.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(
                a.0 == b.0 || a.1 == b.1,
                "segment {a:?}->{b:?} went diagonal"
            );
        }
    }

    /// A wire that merely passes nearby is not claimed — only ENDS attach.
    #[test]
    fn a_passing_wire_is_not_dragged() {
        let mut lay = ClockLayout {
            blocks: vec![BlockDef {
                x: 100.0,
                y: 100.0,
                w: 60.0,
                h: 20.0,
                label: "src".into(),
                node: Some("src".into()),
            }],
            // Runs straight THROUGH the box, ending well away from it.
            wires: vec![vec![(0.0, 110.0), (400.0, 110.0)]],
            ..Default::default()
        };
        let before = lay.wires[0].clone();
        lay.move_node("src", 30.0, 30.0);
        assert_eq!(lay.wires[0], before);
    }

    /// On the real F103 figure, moving a node takes its wiring along.
    #[test]
    fn the_f103_figure_keeps_its_wires_attached() {
        let mut lay = stm32f1_layout(&ClockLimits::default());
        let hse = lay
            .node_anchors()
            .into_iter()
            .find(|a| a.node == "hse")
            .expect("hse is locatable");
        let touching = |l: &ClockLayout, a: &NodeBox| -> usize {
            l.wires
                .iter()
                .filter(|w| {
                    w.iter().any(|p| {
                        p.0 >= a.x - 14.0
                            && p.0 <= a.x + a.w + 14.0
                            && p.1 >= a.y - 14.0
                            && p.1 <= a.y + a.h + 14.0
                    })
                })
                .count()
        };
        assert!(touching(&lay, &hse) > 0, "the HSE box has wiring");

        let before = lay.wires.clone();
        lay.move_node("hse", 25.0, 25.0);
        assert_ne!(lay.wires, before, "its wires moved with it");

        // Every route stays orthogonal after the move.
        for w in &lay.wires {
            for pair in w.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                assert!(
                    (a.0 - b.0).abs() < 0.01 || (a.1 - b.1).abs() < 0.01,
                    "segment {a:?}->{b:?} went diagonal"
                );
            }
        }
    }

    /// Decoration stays decoration: a primitive that names no node is never
    /// claimed by one, and never moved by one. Built here rather than taken from
    /// a shipped figure — every label in those is owned now, and this must hold
    /// regardless of how thoroughly they are annotated.
    #[test]
    fn unowned_primitives_belong_to_nobody() {
        let owned = |node: &str| LabelDef {
            x: 10.0,
            y: 10.0,
            text: "t".into(),
            node: Some(node.to_owned()),
        };
        let legend = LabelDef {
            x: 500.0,
            y: 700.0,
            text: "HSE = high-speed external".into(),
            node: None,
        };
        let mut lay = ClockLayout {
            labels_above: vec![owned("hse"), legend.clone()],
            blocks: vec![BlockDef {
                x: 0.0,
                y: 0.0,
                w: 50.0,
                h: 20.0,
                label: "decor".into(),
                node: None,
            }],
            ..Default::default()
        };

        // It is not a handle…
        assert_eq!(lay.node_anchors().len(), 1, "only `hse` is locatable");
        // …it does not move…
        lay.move_node("hse", 100.0, 100.0);
        assert_eq!(lay.labels_above[1], legend);
        assert_eq!((lay.blocks[0].x, lay.blocks[0].y), (0.0, 0.0));
        // …and it is not deleted with the node.
        lay.remove_node_primitives("hse");
        assert_eq!(lay.labels_above, vec![legend]);
        assert_eq!(lay.blocks.len(), 1);
    }

    /// The hand-authored STM32F103 figure measures back at roughly the 1000×790
    /// canvas it was drawn against — so replacing that constant with a measured
    /// extent did not move it.
    #[test]
    fn f1_layout_measures_its_original_canvas() {
        let (w, h) = stm32f1_layout(&ClockLimits::default()).bounds();
        assert!(
            (900.0..=1120.0).contains(&w),
            "F1 layout should still be ~1000 wide, got {w}"
        );
        assert!(
            (700.0..=880.0).contains(&h),
            "F1 layout should still be ~790 tall, got {h}"
        );
    }

    /// An empty layout has no extent to speak of — the caller (`is_empty`) fills
    /// it from the graph instead.
    #[test]
    fn an_empty_layout_measures_almost_nothing() {
        let (w, h) = ClockLayout::default().bounds();
        assert!(w < 50.0 && h < 50.0, "expected just padding, got {w}×{h}");
    }

    /// Bounds cover every primitive, not just the boxes.
    #[test]
    fn bounds_cover_the_furthest_primitive() {
        let mut lay = ClockLayout {
            blocks: vec![BlockDef {
                x: 10.0,
                y: 10.0,
                w: 100.0,
                h: 20.0,
                label: "HSI".to_owned(),
                node: None,
            }],
            ..Default::default()
        };
        let (w0, _) = lay.bounds();
        lay.wires.push(vec![(0.0, 0.0), (600.0, 40.0)]);
        let (w1, h1) = lay.bounds();
        assert!(w1 > w0, "a wire reaching further must widen the extent");
        assert!(w1 >= 600.0 && h1 >= 40.0);
    }
}
