//! Toolchain kinds shared across the MCU module.
//!
//! Chip identity, pin layout and project parameters are no longer a hardcoded
//! `McuType` enum — they live in importable `McuDefinition`s (see `mcu_def.rs`,
//! `builtins.rs`).  Only the toolchain *family* tag remains here, since codegen
//! and project-file generation dispatch on it.

/// Toolchain kind — determines how code is compiled and flashed for this chip.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ToolchainKind {
    /// ARM Cortex-M: cargo + probe-rs / OpenOCD / DFU
    RustEmbedded,
    /// ESP32 (RISC-V or Xtensa): cargo + espflash
    EspRust,
    /// STM8: SDCC + stm8flash — on hold, not yet implemented
    SdccC,
}
