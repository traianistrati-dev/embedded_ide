//! The Board tab: several chip projects shown as one system.
//!
//! A system is a FOLDER of ordinary chip projects plus a `system.config` at its
//! root. Every chip stays a complete project that opens on its own, and build,
//! flash and rust-analyzer keep working on one chip at a time; this module only
//! reads the chips and draws them together.
//!
//! - [`model`]: the `system.config` file.
//! - [`snapshot`]: what a chip frame shows, read from a chip's files on disk
//!   (or from the live chip, for the one that is open).
//! - [`parts`]: external parts (an FPGA, a sensor) described by hand.
//! - [`links`]: what a link between two modules means pin by pin, and what
//!   does not match.
//! - [`layout`]: the automatic arrangement inside a frame, and a link's path.
//! - [`gui`]: the canvas.

pub mod gui;
pub mod layout;
pub mod links;
pub mod model;
pub mod parts;
pub mod snapshot;
