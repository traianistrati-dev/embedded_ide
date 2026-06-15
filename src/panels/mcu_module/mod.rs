pub mod builtins;
pub mod clock;
pub mod codegen;
pub mod codegen_esp;
pub mod mcu;
pub mod mcu_catalog;
pub mod mcu_def;
pub mod mock_esp32c3;
pub mod mock_mcu;
pub mod modules;
pub mod pins;
pub mod project_gen;
pub mod registry;

// ── Core types re-exports (convenience imports) ──────────────────
pub use pins::logic::{
    pin::{Pin, PIN_FONT_SIZE, PIN_ROUNDING},
    pin_function::PinFunction,
};
pub use mcu::{Mcu, PIN_HEIGHT, PIN_WIDTH, PIN_SPACING};
pub use mcu_catalog::ToolchainKind;

// ── MCU definitions / registry ──────────────────────────────────
pub use builtins::{builtin_definitions, builtin_for};
pub use mcu_def::{McuDefinition, ProjectDef};
pub use registry::{import_file, load_registry, user_mcus_dir};

// ── Factory & generated code ────────────────────────────────────
pub use mock_esp32c3::create_esp32c3;
pub use mock_mcu::create_stm32f103c8tx;
pub use project_gen::ProjectFiles;
