pub mod codegen;
pub mod codegen_esp;
pub mod mcu;
pub mod mcu_catalog;
pub mod mock_esp32c3;
pub mod mock_mcu;
pub mod pins;
pub mod project_gen;

// ── Core types re-exports (convenience imports) ──────────────────
pub use pins::logic::{
    pin::{Pin, PIN_FONT_SIZE, PIN_ROUNDING},
    pin_function::PinFunction,
};
pub use mcu::{Mcu, PIN_HEIGHT, PIN_WIDTH, PIN_SPACING};
pub use mcu_catalog::{McuType, ToolchainKind};

// ── Factory & generated code ────────────────────────────────────
pub use mock_esp32c3::create_esp32c3;
pub use mock_mcu::create_stm32f103c8tx;
pub use project_gen::ProjectFiles;
