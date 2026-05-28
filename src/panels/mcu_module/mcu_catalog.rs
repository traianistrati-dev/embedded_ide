/// Toolchain kind — determines how code is compiled and flashed for this chip.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolchainKind {
    /// ARM Cortex-M: cargo + probe-rs / OpenOCD / DFU
    RustEmbedded,
    /// ESP32 (RISC-V or Xtensa): cargo + espflash
    EspRust,
    /// STM8: SDCC + stm8flash — on hold, not yet implemented
    SdccC,
}

// ─────────────────────────────────────────────────────────────────────────────

/// All data needed to generate a buildable Cargo project for a specific chip.
pub struct McuProjectConfig {
    /// Cargo package name, e.g. "stm32f103c8t6"
    pub pkg_name: &'static str,
    /// Rust target triple, e.g. "thumbv7m-none-eabi"
    pub target: &'static str,
    /// Linker flash origin — empty for EspRust (linker script provided by HAL)
    pub flash_origin: &'static str,
    /// Flash size for memory.x — empty for EspRust
    pub flash_size: &'static str,
    /// Linker RAM origin — empty for EspRust
    pub ram_origin: &'static str,
    /// RAM size for memory.x — empty for EspRust
    pub ram_size: &'static str,
    /// Primary HAL dependency line (placed in Cargo.toml [dependencies])
    pub hal_dep: &'static str,
    /// Chip identifier: probe-rs chip for RustEmbedded; espflash chip for EspRust
    pub probe_chip: &'static str,
    /// Human-readable comment for memory.x — empty for EspRust
    pub memory_comment: &'static str,
    /// Determines the generated file set, flash tool, and code template
    pub toolchain: ToolchainKind,
}

// ─────────────────────────────────────────────────────────────────────────────

/// Registry of all MCU chips available in the IDE.
/// `is_supported()` indicates whether the chip's pin diagram is implemented.
#[derive(PartialEq, Clone, Debug)]
pub enum McuType {
    Stm32f103c8t6,
    Stm8s103f3p6,
    Esp32c3,
}

impl McuType {
    /// Full list of chips shown in the selector dropdown.
    pub fn all() -> Vec<McuType> {
        vec![
            McuType::Stm32f103c8t6,
            McuType::Stm8s103f3p6,
            McuType::Esp32c3,
        ]
    }

    /// Display label shown in the UI.
    pub fn label(&self) -> &str {
        match self {
            McuType::Stm32f103c8t6 => "STM32F103C8T6",
            McuType::Stm8s103f3p6 => "STM8S103F3P6",
            McuType::Esp32c3 => "ESP32-C3",
        }
    }

    /// Returns `true` if this chip has a fully implemented graphical pin diagram.
    pub fn is_supported(&self) -> bool {
        matches!(self, McuType::Stm32f103c8t6 | McuType::Esp32c3)
    }

    /// CPU architecture family shown next to the selector.
    pub fn family(&self) -> &str {
        match self {
            McuType::Stm32f103c8t6 => "ARM Cortex-M3",
            McuType::Stm8s103f3p6 => "STM8 8-bit",
            McuType::Esp32c3 => "RISC-V 32-bit",
        }
    }

    /// Toolchain kind for this chip — available without a full project config.
    pub fn toolchain(&self) -> ToolchainKind {
        match self {
            McuType::Stm32f103c8t6 => ToolchainKind::RustEmbedded,
            McuType::Esp32c3 => ToolchainKind::EspRust,
            McuType::Stm8s103f3p6 => ToolchainKind::SdccC,
        }
    }

    /// Returns project-generation metadata for this chip, or `None` if not yet supported.
    pub fn project_config(&self) -> Option<McuProjectConfig> {
        match self {
            McuType::Stm32f103c8t6 => Some(McuProjectConfig {
                pkg_name: "stm32f103c8t6",
                target: "thumbv7m-none-eabi",
                flash_origin: "0x08000000",
                flash_size: "64K",
                ram_origin: "0x20000000",
                ram_size: "20K",
                hal_dep: r#"stm32f1xx-hal = { version = "0.10", features = ["stm32f103", "medium", "rt"] }"#,
                probe_chip: "STM32F103C8",
                memory_comment: "STM32F103C8T6  —  64 KiB Flash / 20 KiB RAM",
                toolchain: ToolchainKind::RustEmbedded,
            }),

            McuType::Esp32c3 => Some(McuProjectConfig {
                pkg_name: "esp32c3",
                target: "riscv32imc-unknown-none-elf",
                // memory.x is not used — esp-hal provides its own linker script
                flash_origin: "",
                flash_size: "",
                ram_origin: "",
                ram_size: "",
                hal_dep: r#"esp-hal = { version = "0.23", features = ["esp32c3"] }"#,
                probe_chip: "esp32c3", // chip name for espflash
                memory_comment: "",
                toolchain: ToolchainKind::EspRust,
            }),

            // STM8 — on hold
            _ => None,
        }
    }
}
