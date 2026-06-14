# Embedded IDE

A desktop IDE for **bare-metal Rust** firmware development, built with [egui/eframe](https://github.com/emilk/egui). Pick a microcontroller, configure its pins and clock tree visually, and the IDE generates a complete, buildable Cargo project — then build, check, and flash it without leaving the app.

> Status: early development (`v0.1`). Two chip families are supported today (STM32F1, ESP32-C3); more can be added as data files or small backends.

---

## Features

### Visual MCU configuration
- **Pins tab** — a vector chip diagram with all four pin rows. Click a pin to pick its function (GPIO in/out, ADC, timer/PWM, USART, SPI, I2C, USB, CAN, SWD, MCO). A function already assigned to one pin is hidden on the others so an exclusive signal can't be assigned twice.
- **Peripherals tab** — the inverse view: every peripheral the chip exposes, with the pins that can serve each. Pins that support several signals of one peripheral (e.g. an ESP32-C3 GPIO routable to SPI SCK/MOSI/MISO/NSS via the GPIO matrix) appear **once** with a ▾ menu, instead of repeating per signal.
- **Clock tab** — an interactive, **data-driven clock-tree diagram**. Sources, multiplexers (trapezoid MUX + radio buttons), dividers and multipliers are evaluated live; frequencies and over-limit warnings update as you edit. The whole graph + layout is imported per chip from its `.ron`, so a new chip's clock tree is data, not code.

### Code generation
- Generates `src/main.rs` from the pin + clock configuration using the chip's HAL (`stm32f1xx-hal` for STM32, `esp-hal` for ESP32-C3).
- Generated code lives inside a `// <<< GENERATED >>> … // <<< GENERATED END >>>` block; **your code outside the markers (the `loop {}` body, helpers) is preserved** across every regeneration.
- The clock configuration drives the real setup chain (`rcc.cfgr…freeze()` on STM32, `CpuClock::_…MHz` on ESP32-C3).

### Editable project files
- The full Cargo project is generated: `Cargo.toml`, `.cargo/config.toml`, `memory.x`, `build.rs`, `.gitignore`, `src/main.rs`, plus any user source files.
- Each config file is **editable** — its chip-derived content sits in a `<<< GENERATED >>>` block (using that file's comment syntax: `//`, `#`, or `/* */`), and anything you add outside the block is kept when the block is refreshed on a chip change.

### Built-in code editor
- Syntax-highlighted editor ([egui_code_editor](https://crates.io/crates/egui_code_editor)) with a project tree (generated files + your own modules under `src/`).
- **rust-analyzer integration**: code completion (`.`, `::`, Ctrl+Space) and diagnostics. Edits are pushed to rust-analyzer on a short idle debounce so typing stays smooth; `cargo check` errors surface in the bottom **rust-analyzer / Cargo Check** panel.

### Build & flash
- **Build / Check** — run `cargo` in a managed workspace; errors are parsed and listed with click-to-jump.
- **Flash** — flash the board from the toolbar:
  - **STM32** — SWD via **OpenOCD**, or `cargo run` via **probe-rs** (configured in the generated `.cargo/config.toml`). USB-DFU programmer detection is included.
  - **ESP32-C3** — via **espflash** (`espflash flash --monitor`).
- **Required Tools tab** — checks for the external tools each workflow needs.

### Project management & MCU import
- **Save / Open project** — export the generated project to a folder and reopen it later; the IDE restores the exact chip, pin state, clock config, and your edits.
- **Import MCU** — drop a chip definition (`.ron`) into the user `mcus/` folder (or use *Import MCU…*) and it appears in the chip selector. New chips **inside an already-supported family need no recompile** — they are pure data.

---

## Supported microcontrollers

| Chip | Core | Toolchain | Notes |
|------|------|-----------|-------|
| STM32F103C8T6 | ARM Cortex-M3 | `stm32f1xx-hal` | full clock tree, peripherals, codegen |
| ESP32-C3 | RISC-V 32-bit | `esp-hal` | clock graph, peripherals, codegen |

Definitions live in [`assets/mcus/`](assets/mcus/) (`*.ron`). Example importable chips (incl. graph-clock demos) are in [`assets/mcus/examples/`](assets/mcus/examples/).

---

## Getting started

### Prerequisites
- **Rust** (stable, edition 2024) — install via [rustup](https://rustup.rs/).
- **rust-analyzer** on `PATH` — for completions and diagnostics.
- Target toolchains for the chips you build:
  - STM32: `rustup target add thumbv7m-none-eabi`
  - ESP32-C3: `rustup target add riscv32imc-unknown-none-elf`
- Flashing tools, as needed: [`probe-rs`](https://probe.rs/), [OpenOCD](https://openocd.org/), [`espflash`](https://github.com/esp-rs/espflash), `dfu-util`. (The **Required Tools** tab checks these for you.)

### Run the IDE
```bash
cargo run            # debug
cargo run --release  # release
```

The app opens maximized. Pick a chip (or start with the default STM32F103C8T6), configure pins/clock, and the generated `src/main.rs` appears in the editor.

---

## Typical workflow
1. **Choose a chip** (or *Import MCU…* a `.ron`).
2. **Pins / Peripherals** — assign functions.
3. **Clock** — set the clock tree; watch frequencies + warnings update live.
4. The editor shows the generated `main.rs`; **write your logic in the `loop {}` body** (kept across regeneration).
5. **Build / Check** — fix errors from the bottom panel.
6. **Flash** — program the board (SWD / espflash / probe-rs).
7. **Save Project** — export to a folder; reopen anytime.

---

## Architecture

A layered, **data-where-possible / code-where-necessary** design:

- **Layer 1 — MCU definition (data, per chip):** [`mcu_def.rs`](src/panels/mcu_module/mcu_def.rs) `McuDefinition` (serde/RON) aggregates pin layout, project parameters (target, HAL dep, flash/RAM map, probe chip), and the clock (`ClockDef`). Bundled built-ins via `include_str!`, plus a runtime user `mcus/` scan ([`registry.rs`](src/panels/mcu_module/registry.rs)).
- **Layer 2 — family backend (code, per family):** [`codegen/family.rs`](src/panels/mcu_module/codegen/family.rs) `FamilyBackend` captures the HAL-specific `main.rs` generation. Adding a new *family* = one new backend; new *chips* in a supported family = data only.
- **Clock graph (data):** [`clock/graph/`](src/panels/mcu_module/clock/graph/) — a generic node-graph (`Source`/`Mux`/`Divider`/`Multiplier`/…) with a topological frequency evaluator, a per-chip diagram layout, and interactive widgets. STM32 and ESP32-C3 each ship their graph; codegen reads node states back out.
- **App shell:** [`app.rs`](src/app.rs) + [`app/`](src/app/) — egui panels (project tree, MCU configurator, editor), the rust-analyzer client ([`lsp.rs`](src/lsp.rs)), and the build/flash drivers ([`build.rs`](src/build.rs), [`openocd.rs`](src/openocd.rs), [`espflash.rs`](src/espflash.rs), [`dfu.rs`](src/dfu.rs)).

### Source layout
```
src/
├── main.rs                     # eframe entry point
├── app.rs, app/                # UI shell, panels, project I/O, dialogs
├── editor/                     # code editor + diagnostics overlay
├── project_tree/               # file tree state + rendering
├── lsp.rs                      # rust-analyzer client
├── build.rs, openocd.rs,       # build + flash drivers
│   espflash.rs, dfu.rs
├── required_tools.rs           # external-tool checks
└── panels/mcu_module/
    ├── mcu_def.rs, registry.rs # chip definitions + registry
    ├── pins/                   # pin model + chip diagram
    ├── clock/                  # clock model, graph, diagram
    ├── codegen/, codegen_esp.rs# per-family code generation
    └── project_gen.rs          # full Cargo-project file generation
assets/mcus/                    # built-in + example chip .ron files
```

### Generated project layout (STM32)
```
<project>/
├── .cargo/config.toml   # target, runner (probe-rs), linker flags
├── src/main.rs          # generated HAL code + your loop body
├── build.rs             # copies memory.x to OUT_DIR
├── memory.x             # flash/RAM map for the chip
├── Cargo.toml
└── .gitignore
```
(ESP32-C3 omits `memory.x`/`build.rs` — `esp-hal` supplies them.)

---

## Tech stack
- **UI:** `eframe` / `egui` 0.34, `egui_code_editor`, `egui-phosphor` (icons), `rfd` (file dialogs).
- **Data:** `serde` + `ron` (chip definitions), `serde_json` (LSP).
- **System:** `notify` (workspace file watching), `serialport` + `rusb` (programmer/serial access).

## Testing
```bash
cargo test
```
The suite (150+ tests) covers the clock graph evaluator and equivalence sweeps, code generation and marker-splice idempotency, pin/peripheral logic, the registry/round-trip, and the editable-config splice.

---

## Roadmap / limitations
- More chip families (STM32F4/F0, RP2040, nRF52…) — each needs a small codegen backend; chips within them are then data-only.
- Inline editor diagnostics (squiggles) are currently disabled in favor of the bottom panel (they could lag behind fast edits); re-enable via `SHOW_INLINE_DIAGNOSTICS` in `app/editor_panel/completion.rs`.
- ESP32-C3 clock codegen covers the CPU clock; APB/RTC are managed by `esp-hal`.

## License
Not yet specified.
