//! What a Virtual Module's config rows MEAN, and the collector that carries it
//! to the info pane.
//!
//! # Why the text lives here and not at the row
//!
//! A field can be described in three places: the `///` on the struct in
//! `modules/model.rs`, the `.on_hover_text(...)` on its widget, and now the
//! details pane. Three copies of one sentence is three chances to drift, and
//! the one that drifts is always the copy nobody is looking at.
//!
//! So the sentence is a `pub const` here, and the row site names it twice — once
//! for the hover, once for the collector:
//!
//! ```ignore
//! ui.label("Baud rate");
//! …combo…
//! out.field("Baud rate", docs::USART_BAUD);
//! ```
//!
//! There is one string. Keeping the two in step is not a discipline, it is a
//! `&'static str` binding the compiler checks.
//!
//! # What this is NOT
//!
//! * Not generated from the `///` comments in `model.rs`. Those are written for
//!   whoever maintains this code — "`#[serde(default)]` keeps old `@modules`
//!   markers valid" — and address a different reader. They stay where they are,
//!   and they are not canonical for the UI.
//! * Not a per-field table keyed by identifier. `buf_len` is drawn as
//!   "RX/TX buffer" or "RX DMA buffer" depending on the transport, and it is the
//!   LABEL the reader saw that the pane has to explain.
//! * Not a description of the generated code. That is `codegen`'s to state, and
//!   restating it here is what would rot first.

use std::borrow::Cow;

/// One documented row, as the panel actually drew it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDoc {
    /// The label the row drew — "RX DMA buffer", not `buf_len`. Owned, because
    /// several are built with `format!` from the wiring.
    pub label: String,
    /// The sentence, usually a `const` from this module.
    pub doc: Cow<'static, str>,
}

/// Everything a config arm has to say that is not a control.
///
/// Replaces the bare `notes: &mut Vec<String>` the arms used to take. One
/// parameter still, on a function that already carries sixteen.
#[derive(Default, Debug)]
pub struct ConfigOut {
    notes: Vec<String>,
    fields: Vec<FieldDoc>,
    elsewhere: Vec<FieldDoc>,
    complete: bool,
}

impl ConfigOut {
    /// A standing remark about the module — not tied to one row.
    pub fn note(&mut self, text: impl Into<String>) {
        self.notes.push(text.into());
    }

    /// Document the row just drawn.
    ///
    /// A repeated label is DROPPED, not appended: several arms draw a row per
    /// wired channel or per operator, and four copies of the same sentence with
    /// different pin names in the label is noise, not documentation. The first
    /// one wins, so the label the reader meets first is the one explained.
    pub fn field(&mut self, label: impl Into<String>, doc: impl Into<Cow<'static, str>>) {
        let label = label.into();
        if self.fields.iter().any(|f| f.label == label) {
            return;
        }
        self.fields.push(FieldDoc {
            label,
            doc: doc.into(),
        });
    }

    /// A setting this module HAS, but that this grid does not draw.
    ///
    /// Kept apart from [`Self::field`] and NOT gated by
    /// [`Self::all_fields_documented`], because the two answer different
    /// questions. `field` says "here is what the row you are looking at means";
    /// this says "here is a setting you will not find above, and where it lives
    /// instead". The second is true for every kind from the day it is written,
    /// so withholding it until that kind's rows are documented would hide a
    /// finished answer behind an unfinished one.
    pub fn elsewhere(&mut self, label: impl Into<String>, doc: impl Into<Cow<'static, str>>) {
        let label = label.into();
        if self.elsewhere.iter().any(|f| f.label == label) {
            return;
        }
        self.elsewhere.push(FieldDoc {
            label,
            doc: doc.into(),
        });
    }

    pub fn elsewhere_fields(&self) -> &[FieldDoc] {
        &self.elsewhere
    }

    /// Say that this arm documents EVERY row it drew.
    ///
    /// Until an arm calls this, [`Self::fields`] hands back `None` and the pane
    /// shows no Fields section at all. That is deliberate: a half-filled roster
    /// reads as "this module has three settings", which is worse than saying
    /// nothing. Rolling the next kind out is one call at the end of its arm.
    pub fn all_fields_documented(&mut self) {
        self.complete = true;
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// The documented rows, or `None` while this kind's arm is not finished —
    /// see [`Self::all_fields_documented`].
    pub fn fields(&self) -> Option<&[FieldDoc]> {
        self.complete.then_some(self.fields.as_slice())
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.fields.is_empty() && self.elsewhere.is_empty()
    }
}

// ── Rows shared by USART / SPI / I2C ─────────────────────────────────────────

pub const INIT_API: &str = "Which TYPE `init` hands back. Portable gives an embedded-io / \
                            embedded-hal 1.0 value, so driver code moves between HALs; Native \
                            gives the HAL's own type, with everything it can do.";

pub const INIT_API_ESP: &str = "Fixed on an ESP: every esp-hal driver is the concrete type, \
                                because that is what esp-hal implements its traits on. The \
                                Portable bridge is an STM32F1 choice.";

pub const INIT_API_NATIVE: &str = "Fixed by the Native runtime, which uses the concrete HAL type \
                                   for every peripheral. Change it on the System tab to choose \
                                   per module again.";

pub const INIT_API_DMA: &str = "Fixed by the DMA transport: stm32f1xx-hal's DMA handles are its \
                                own types, and there is no portable bus to bridge them to. Turn \
                                DMA off and the choice comes back.";

pub const BLOCKING_TRANSPORT: &str = "Which HALF of the bus the DMA moves, on the blocking F1 \
                                      HAL. Directions and not channels, because stm32f1xx-hal \
                                      fixes the channel per peripheral in its types. The half you \
                                      leave off keeps the ordinary polled handle and frees its \
                                      channel.";

pub const DMA_CHANNEL: &str = "The channel this direction takes. Automatic gives it the first one \
                               the peripheral can use that nothing else has claimed - pin it by \
                               hand to leave a particular channel free, or to match a driver you \
                               already have.";

// ── Settings every module has, that its grid does not draw ───────────────────

pub const SHARED_INSTANCE: &str = "Which peripheral this module drives. Not editable: it is read \
                                   back off the pads - a module is keyed on the function each \
                                   wired pin carries - so moving the wiring to another instance is \
                                   what changes it.";

pub const SHARED_INSTANCE_TIMER_STM32: &str = "The TIM this module drives, read back off the pads \
                                               rather than chosen: a channel pad names its timer, \
                                               so re-wiring is what moves the module.";

pub const SHARED_INSTANCE_TIMER_ESP: &str = "The LEDC timer this module drives. Unlike an STM32's \
                                             TIM, its channels are not welded to pads - the GPIO \
                                             matrix routes them - so the pad picks the channel and \
                                             the module keeps the timer.";

pub const SHARED_INSTANCE_TIMER_RP: &str = "The PWM SLICE this module drives. A slice is welded to \
                                            its pads on the RP: pad n belongs to slice (n/2) % 8, \
                                            and A or B by whether it is even or odd.";

pub const SHARED_INSTANCE_CUSTOM: &str = "A custom module drives no peripheral, so this number \
                                          means nothing - it exists only so every module can \
                                          answer the same question. Custom modules are told apart \
                                          by their id.";

pub const SHARED_NAME: &str = "The label from the Name row above, and from the field inside the \
                               module's box on the canvas. It is lowercased, every run of \
                               non-alphanumerics becomes one underscore, and the result is \
                               appended to the generated handle - `servo` gives `_pwm3_servo`.";

pub const SHARED_NAME_RP: &str = "The label from the Name row above, and from the field inside the \
                                  module's box on the canvas. The Pico backend does not read it \
                                  yet, so handles keep their bare names there; every other backend \
                                  appends it - `servo` gives `_pwm3_servo`.";

pub const SHARED_DATA_MODELS: &str = "Free-text Rust this module carries for the frames it sends \
                                      and receives. Nothing edits them today - a palette template \
                                      can fill them, or you can by hand - and whatever is there is \
                                      written into main.rs as a `mod` block named after the \
                                      module's id. Additively: a block already there is left \
                                      alone, so your edits inside it survive regeneration.";

// ── USART / LPUART ───────────────────────────────────────────────────────────

pub const USART_BAUD: &str = "Bits per second on the wire, both ways. It has to match the other \
                              end exactly - there is no negotiation on a UART.";

pub const USART_DATA_BITS: &str = "Bits in one character, parity NOT counted. 8 is what almost \
                                   everything uses; 9 exists for the addressed multi-drop modes.";

pub const USART_PARITY: &str = "An extra bit that makes the count of ones even or odd, so a \
                                single flipped bit is noticed. It is taken OUT of the character: \
                                8 data bits with parity is 9 bits on the wire.";

pub const USART_STOP_BITS: &str = "How long the line is held idle after each character, giving \
                                   the receiver time to resynchronise. One is normal; two is for \
                                   a slow or noisy peer.";

pub const USART_BUF_BUFFERED: &str = "Size of BOTH software ring buffers, TX and RX. The CPU \
                                      copies byte by byte on each interrupt, so this has to cover \
                                      what arrives between your reads.";

pub const USART_BUF_DMA: &str = "The circular buffer the DMA controller fills on its own. \
                                 Reception never stops, so this only has to cover the longest GAP \
                                 between your reads - overrun it and the OLDEST bytes are dropped, \
                                 silently.";

pub const USART_ASYNC_TRANSPORT: &str = "Who moves the bytes. Buffered costs one interrupt per \
                                         byte and no DMA channel, so it always builds; DMA hands \
                                         the peripheral straight to the controller and keeps \
                                         receiving between your reads, at the price of channels.";

pub const USART_TRANSFERS_ESP: &str = "Fixed on an ESP: esp-hal moves UART bytes over DMA through \
                                       UHCI, a driver of its own this generator does not write \
                                       yet. The SPI module's DMA is a different peripheral and \
                                       does work.";

pub const USART_DIRECTION: &str = "Which halves to build. A one-way link frees the other pad for \
                                   something else; the half-duplex modes put both directions on \
                                   ONE wire.";

pub const USART_DIRECTION_LOCKED: &str = "Both halves, and no choice: embassy has no buffered \
                                          TX-only or RX-only - `BufferedUartTx` and \
                                          `BufferedUartRx` come only from splitting a \
                                          `BufferedUart`, so both pads are used either way. \
                                          Switch the transport to DMA for a one-way UART.";

/// The same row on an RP, where BOTH halves of the sentence above are false.
///
/// embassy-rp has real standalone constructors on both transports -
/// `BufferedUartTx::new` / `BufferedUartRx::new` (uart/buffered.rs:344, :195)
/// and `UartTx::<Async>::new` / `UartRx::<Async>::new` (uart/mod.rs:251, :403),
/// the DMA ones taking a single channel. So the pair is not the chip's limit and
/// switching transport unlocks nothing: `async_bus_lines` skips a UART with only
/// one pad wired, on either transport.
pub const USART_DIRECTION_LOCKED_RP: &str = "Both halves, and no choice yet. embassy-rp does have one-way constructors on both transports -      BufferedUartTx / BufferedUartRx and UartTx / UartRx, the DMA pair taking one channel each - but this backend emits only the bidirectional form, so a UART with a single pad wired generates nothing. A gap in the generator, not in the chip.";

pub const USART_LINE: &str = "Pad-level fixes done inside the peripheral, so a crossed cable or an \
                              inverting transceiver needs no rework: swap RX with TX, or invert \
                              either line's idle level.";

pub const USART_LINE_ABSENT: &str = "This USART has no swap or invert bits - they arrived with a \
                                     later revision of the peripheral, and embassy's `Config` has \
                                     no field for them here.";

pub const USART_FLOW: &str = "Hardware handshaking. CTS lets the peer stop this node mid-stream, \
                              RTS lets this node stop the peer, and DE drives a transceiver's \
                              direction pin for RS-485. Each needs its own pad wired.";

pub const USART_HALF_DUPLEX_READBACK: &str = "One wire carries both directions, so everything this \
                                              node sends lands on its own receiver. Off - the \
                                              default - mutes the receiver while transmitting, \
                                              which is what a bus with other talkers wants; on \
                                              keeps the echo, which is how you check that a driver \
                                              can be shouted down.";

#[cfg(test)]
mod tests {
    use super::*;

    /// Every const in the table, so the hygiene tests below cannot miss one by
    /// being written before it existed.
    ///
    /// Named consts are what make this possible at all: an inline literal at a
    /// row site is invisible to a test, which is half the reason for this
    /// module.
    const ALL: &[(&str, &str)] = &[
        ("INIT_API", INIT_API),
        ("INIT_API_ESP", INIT_API_ESP),
        ("INIT_API_NATIVE", INIT_API_NATIVE),
        ("INIT_API_DMA", INIT_API_DMA),
        ("BLOCKING_TRANSPORT", BLOCKING_TRANSPORT),
        ("DMA_CHANNEL", DMA_CHANNEL),
        ("SHARED_INSTANCE", SHARED_INSTANCE),
        ("SHARED_INSTANCE_TIMER_STM32", SHARED_INSTANCE_TIMER_STM32),
        ("SHARED_INSTANCE_TIMER_ESP", SHARED_INSTANCE_TIMER_ESP),
        ("SHARED_INSTANCE_TIMER_RP", SHARED_INSTANCE_TIMER_RP),
        ("SHARED_INSTANCE_CUSTOM", SHARED_INSTANCE_CUSTOM),
        ("SHARED_NAME", SHARED_NAME),
        ("SHARED_NAME_RP", SHARED_NAME_RP),
        ("SHARED_DATA_MODELS", SHARED_DATA_MODELS),
        ("USART_BAUD", USART_BAUD),
        ("USART_DATA_BITS", USART_DATA_BITS),
        ("USART_PARITY", USART_PARITY),
        ("USART_STOP_BITS", USART_STOP_BITS),
        ("USART_BUF_BUFFERED", USART_BUF_BUFFERED),
        ("USART_BUF_DMA", USART_BUF_DMA),
        ("USART_ASYNC_TRANSPORT", USART_ASYNC_TRANSPORT),
        ("USART_TRANSFERS_ESP", USART_TRANSFERS_ESP),
        ("USART_DIRECTION", USART_DIRECTION),
        ("USART_DIRECTION_LOCKED", USART_DIRECTION_LOCKED),
        ("USART_LINE", USART_LINE),
        ("USART_LINE_ABSENT", USART_LINE_ABSENT),
        ("USART_FLOW", USART_FLOW),
        ("USART_HALF_DUPLEX_READBACK", USART_HALF_DUPLEX_READBACK),
    ];

    /// A `\`-continued literal keeps the newline's leading indentation UNLESS
    /// the backslash eats it — and rustfmt has more than once joined such a
    /// literal back onto one line, spaces and all. `ui.label` does not collapse
    /// them, so the reader gets a gap in the middle of a sentence.
    ///
    /// This has bitten the module panel twice (project memory
    /// `rustfmt-joins-continued-strings`; forty instances collapsed in
    /// `modules.rs` on 2026-08-28). Here it cannot: the strings have names, so a
    /// test can walk them.
    #[test]
    fn no_doc_string_carries_a_run_of_spaces() {
        for (name, s) in ALL {
            assert!(!s.contains("  "), "{name} has a run of spaces:\n{s}");
        }
    }

    /// A const that is empty, or that stops mid-thought, is worse than no
    /// entry: the pane would draw a label with nothing beside it.
    #[test]
    fn every_doc_string_is_a_finished_sentence() {
        for (name, s) in ALL {
            assert!(s.len() > 30, "{name} is too short to be a sentence: {s:?}");
            assert!(
                s.ends_with('.') || s.ends_with('?'),
                "{name} does not end a sentence: {s:?}"
            );
            assert_eq!(s.trim(), *s, "{name} has stray whitespace at an end");
        }
    }

    /// The pane must not show a partial roster — see
    /// [`ConfigOut::all_fields_documented`].
    #[test]
    fn fields_are_withheld_until_an_arm_says_it_is_done() {
        let mut out = ConfigOut::default();
        out.field("Baud rate", USART_BAUD);
        assert!(out.fields().is_none(), "not marked complete yet");
        out.all_fields_documented();
        assert_eq!(out.fields().map(<[_]>::len), Some(1));
    }

    /// A row drawn once per wired channel is documented once.
    #[test]
    fn a_repeated_label_is_documented_once() {
        let mut out = ConfigOut::default();
        out.field("CH duty", USART_BAUD);
        out.field("CH duty", USART_PARITY);
        out.all_fields_documented();
        let f = out.fields().unwrap();
        assert_eq!(f.len(), 1);
        // The FIRST wins: it is the one the reader met first.
        assert_eq!(f[0].doc, USART_BAUD);
    }

    /// "Set elsewhere" is NOT held back by the completeness gate: it is true for
    /// every kind the day it is written, and hiding a finished answer behind an
    /// unfinished one would be the wrong trade.
    #[test]
    fn the_elsewhere_group_does_not_wait_for_the_rows() {
        let mut out = ConfigOut::default();
        out.elsewhere("Instance", SHARED_INSTANCE);
        assert!(out.fields().is_none(), "the rows are still ungated");
        assert_eq!(out.elsewhere_fields().len(), 1);
        // …and it dedupes the same way.
        out.elsewhere("Instance", SHARED_NAME);
        assert_eq!(out.elsewhere_fields().len(), 1);
        assert_eq!(out.elsewhere_fields()[0].doc, SHARED_INSTANCE);
    }

    /// Notes and fields travel together but stay separate — the pane draws them
    /// under different headings.
    #[test]
    fn notes_and_fields_do_not_mix() {
        let mut out = ConfigOut::default();
        assert!(out.is_empty());
        out.note("something to know");
        out.field("Parity", USART_PARITY);
        assert_eq!(out.notes().len(), 1);
        assert!(!out.is_empty());
    }
}
