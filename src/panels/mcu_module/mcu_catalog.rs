/// All data needed to generate a buildable Cargo project for a specific chip.
pub struct McuProjectConfig {
    /// Cargo package name, e.g. "stm32f103c8t6"
    pub pkg_name:        &'static str,
    /// Rust target triple, e.g. "thumbv7m-none-eabi"
    pub target:          &'static str,
    /// Linker flash origin, e.g. "0x08000000"
    pub flash_origin:    &'static str,
    /// Flash size string for memory.x, e.g. "64K"
    pub flash_size:      &'static str,
    /// Linker RAM origin, e.g. "0x20000000"
    pub ram_origin:      &'static str,
    /// RAM size string for memory.x, e.g. "20K"
    pub ram_size:        &'static str,
    /// Full TOML dependency line for the HAL crate
    pub hal_dep:         &'static str,
    /// probe-rs chip identifier for the runner
    pub probe_chip:      &'static str,
    /// Human-readable comment placed at the top of memory.x
    pub memory_comment:  &'static str,
}

// ─────────────────────────────────────────────────────────────────────────────

/// Registry of all MCU chips available in the IDE.
/// `is_supported()` indicates whether the chip's pinout is implemented in the current build.
#[derive(PartialEq, Clone, Debug)]
pub enum McuType {
    Stm32f103c8t6,
    Stm8s103f3p6,
    Esp32c3,
}

impl McuType {
    /// Full list of chips shown in the selector dropdown
    pub fn all() -> Vec<McuType> {
        vec![
            McuType::Stm32f103c8t6,
            McuType::Stm8s103f3p6,
            McuType::Esp32c3,
        ]
    }

    /// Display label shown in the UI
    pub fn label(&self) -> &str {
        match self {
            McuType::Stm32f103c8t6 => "STM32F103C8T6",
            McuType::Stm8s103f3p6 => "STM8S103F3P6",
            McuType::Esp32c3 => "ESP32-C3",
        }
    }

    /// Returns true if this chip has a fully implemented pinout
    pub fn is_supported(&self) -> bool {
        matches!(self, McuType::Stm32f103c8t6)
    }

    /// CPU architecture family shown next to the dropdown
    pub fn family(&self) -> &str {
        match self {
            McuType::Stm32f103c8t6 => "ARM Cortex-M3",
            McuType::Stm8s103f3p6  => "STM8 8-bit",
            McuType::Esp32c3       => "RISC-V 32-bit",
        }
    }

    /// Returns project-generation metadata for this chip, or `None` if not yet supported.
    pub fn project_config(&self) -> Option<McuProjectConfig> {
        match self {
            McuType::Stm32f103c8t6 => Some(McuProjectConfig {
                pkg_name:       "stm32f103c8t6",
                target:         "thumbv7m-none-eabi",
                flash_origin:   "0x08000000",
                flash_size:     "64K",
                ram_origin:     "0x20000000",
                ram_size:       "20K",
                hal_dep:        r#"stm32f1xx-hal = { version = "0.10", features = ["stm32f103", "medium"] }"#,
                probe_chip:     "STM32F103C8",
                memory_comment: "STM32F103C8T6  —  64 KiB Flash / 20 KiB RAM",
            }),
            _ => None,
        }
    }
}
