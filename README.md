# Embedded IDE

A desktop IDE for **bare-metal Rust** firmware. Pick a microcontroller, configure
its pins, peripherals and clock tree visually, and the IDE writes a complete,
buildable Cargo project for you — then edit, check, and flash it without ever
leaving the app.

It is built for the workflow of small MCU projects: you spend your time deciding
*what each pin does* and *how fast the chip runs*, and the IDE turns those
decisions into correct HAL setup code. Your own application logic is always kept
safe across regenerations.

> Status: early development (`v0.1`). Two chip families ship today —
> **STM32F1** (Cortex-M3) and **ESP32-C3** (RISC-V) — and more chips can be added
> as plain data files.

---

## What you can do

- **Configure a chip visually** — assign pin functions on a chip diagram, or work
  peripheral-by-peripheral.
- **Drop in ready-made peripheral devices** — add a USART / SPI / I²C device and
  the IDE auto-wires the pins and generates its init code.
- **Design the clock tree interactively** — adjust sources, multiplexers,
  multipliers and dividers and watch every frequency (and every over-limit
  warning) update live.
- **Get generated firmware** — a full `src/main.rs` plus the whole Cargo project,
  with your code preserved.
- **Edit with a real code editor** — rust-analyzer completion and diagnostics,
  go-to-definition, rename, formatting, and Cargo.toml dependency completion.
- **Build & flash** — `cargo check`/`build` and one-click flashing over SWD,
  probe-rs, DFU, or espflash.
- **Save & reopen projects** — round-trips the chip, every pin, the clock config,
  and your edits.

---

## Visual MCU configuration

### Pins tab
A vector diagram of the chip with all of its pins. Click a pin to choose what it
does from the functions that pin actually supports. Once an exclusive signal is
taken (say `USART1_TX`), it disappears from the other pins so it can never be
assigned twice.

Supported pin functions:

| Group | Functions |
|-------|-----------|
| **GPIO** | digital input, digital output |
| **Analog** | ADC channels |
| **Timers** | PWM output (per timer / channel) |
| **USART** | TX, RX, CTS, RTS, CK (synchronous clock) |
| **SPI** | NSS (chip-select), SCK, MISO, MOSI |
| **I²C** | SCL, SDA |
| **USB** | D−, D+ |
| **CAN** | RX, TX |
| **Debug** | SWDIO, SWCLK |
| **Clock** | MCO (master clock output) |

Each pin can also be given a **custom label** (e.g. `led`, `uart_dbg`) with an
input/output direction. The label flows into the generated variable name
(`pc13_out_led`), so your code reads the way you think about the board.

### Peripherals tab
The same configuration seen the other way around: every peripheral the chip
exposes, listing the pins that can serve each signal. Where a single pin can be
routed to several signals of one peripheral (for example an ESP32-C3 GPIO that
the GPIO matrix can route to SPI SCK/MOSI/MISO/NSS), the pin shows up **once**
with a dropdown instead of repeating — so the view stays compact.

### Virtual device modules
Instead of wiring a peripheral pin by pin, you can drop a **device** onto the
canvas and let the IDE do the wiring:

- **USART device** — configurable baud rate, parity and stop bits.
- **SPI device** — full SCK/MOSI/MISO/NSS bus.
- **I²C device** — with a 7-bit device address.

Adding a device automatically claims a free peripheral instance and its pins,
draws the connections, and generates the matching init code (baud-rate constants,
bus setup, etc.). Each module can be renamed, and the name carries through to the
generated variables. The module list lives below the chip and expands on click to
show each device's details.

---

## Clock configuration

The **Clock tab** is a live, interactive clock-tree diagram — not a form. The
oscillators, multiplexers (shown as trapezoid selectors with radio buttons),
multipliers and dividers are all editable, and the tree is re-evaluated as you
change it:

- **Live frequencies** — every node shows its current frequency, recomputed
  instantly from the sources down to SYSCLK and the peripheral buses.
- **Over-limit warnings** — if a setting pushes a node past the chip's allowed
  maximum, it's flagged right on the diagram.
- **Real codegen** — the clock you draw drives the actual setup chain in the
  generated firmware (`rcc.cfgr…freeze()` on STM32, `CpuClock` on ESP32-C3), so
  what you see is what the chip runs.
- **Per-chip clock trees** — for STM32F1 that means HSI / HSE / PLL (with its
  input mux and multiplier), the SYSCLK selector and the bus prescalers; ESP32-C3
  ships its own graph. The whole tree, its limits and its on-screen layout come
  from the chip's data file, so a new chip brings its own clock tree with it.

---

## Code generation

- The IDE generates **`src/main.rs`** from your pin + clock configuration using
  the chip's HAL (`stm32f1xx-hal` for STM32, `esp-hal` for ESP32-C3).
- Generated code sits inside a `// <<< GENERATED >>> … // <<< GENERATED END >>>`
  block. **Everything you write outside the markers** — your `loop {}` body,
  helper functions, extra `use`s — **is preserved** every time the configuration
  changes and the block is refreshed.
- The **entire Cargo project** is produced and kept in sync: `Cargo.toml`,
  `.cargo/config.toml`, `memory.x`, `build.rs`, `.gitignore`, `src/main.rs`, and
  any source files you add.
- **All config files are editable.** Their chip-derived parts live in a
  `GENERATED` block (using each file's own comment style — `//`, `#`, `/* */`),
  and anything you add outside is kept when the block is regenerated.

### Checking that the generated code builds

Unit tests can tell you the generator produced the *text* you expected. Only a
compiler can tell you that text is a program. `scripts/verify-codegen.ps1`
emits a matrix of configurations and cross-compiles each one:

```powershell
pwsh scripts/verify-codegen.ps1          # representative subset
pwsh scripts/verify-codegen.ps1 -Full    # every case
```

The quick set is 20 cases and takes about 12 minutes on a warm tree. Each case
prints its own time, and the run ends with a total and the three most expensive
— which is how you find out that one case, `embassy`, was a third of the bill
before it was trimmed.

Warnings count as failures. Not by a switch — each case declares how many it is
allowed (`w`, default none), and any other number fails, in either direction.
Generated code is meant to be warning-free; the few that are deliberate (a
half-wired bus leaves its pad bound and unused, which is the compiler naming the
same pad the generated comment names) are written down as numbers instead of
being waved through.

To run it before every push:

```bash
git config core.hooksPath scripts/hooks
```

The hook only fires when something under `src/panels/mcu_module/` changed, so a
README edit costs nothing; `git push --no-verify` skips it outright.

It covers every runtime (Blocking, RTIC, Native, and Async where it is inert),
both HALs (`stm32f1xx-hal` and embassy-stm32, plus `esp-hal`), and each
half-wired shape — a bus with one pad missing, a SPI without MISO, a USB with
one data pin. Those last ones are the paths that break without anyone noticing:
the peripheral you configured simply does not appear in `main.rs`, or appears
naming a binding that was never declared.

**One run at a time, machine-wide.** Every case writes to a fixed directory
under the temp dir, so two runs share one `target/` and tear each other's
artifacts apart — and the damage does not look like concurrency. It surfaces as
`could not write output`, `failed to write fingerprint`, `link.exe: 1104`:
errors that read as a codegen regression and point at the wrong file. The script
therefore takes a lock and *waits* rather than refusing, because a pre-push hook
that exits non-zero aborts the push. While waiting it names the run it is
waiting for and how to stop waiting. The emit harnesses warn (they do not block)
when you start one by hand into a live run — the other half of the same trap.

The give-away that a failure is concurrency and not codegen is the **timing**: a
case that reports seven errors in six seconds never compiled anything.

Two kinds of case exist. Most emit a project and cross-compile it. A **verdict**
case (`v`) runs a host test and stops there, for a chip that *cannot* be
compiled: `WL30 preflight verdict` pins that STM32WL30 has no clock code and no
`embassy-stm32` feature, with an STM32G071 alongside as the control. It fails
the day a `stm32wl3` recipe lands and the answer has to change.

A case whose harness writes several projects may cross-compile only some of them
in quick mode (`only`), with all of them still built by `-Full`. A name in that
list matching nothing fails the case outright rather than quietly shrinking it.

Adding a case is one row in the script's `$CASES` table. The emit harnesses
themselves are `#[ignore]`d tests (`cargo test <name> -- --ignored`) that print
`wrote <path>` and `target: <triple>`; the script reads those lines rather than
keeping its own copy of where anything lands, and a harness that emits several
projects for several targets is paired up line by line.

Two cases build from a real part in the STM32Cube database. Point
`EIDE_CUBE_DB` at your copy, or let them be skipped — a machine without the
database reports them as skipped rather than failed.

Cross-compiling needs each chip's target installed, e.g.
`rustup target add thumbv7m-none-eabi thumbv7em-none-eabihf riscv32imc-unknown-none-elf`.

---

## Code editor

A syntax-highlighted editor with a project tree (generated files plus your own
modules under `src/`) and a **rust-analyzer** backend.

### Language intelligence (rust-analyzer)
- **Completion** as you type after `.` and `::`, or on demand with `Ctrl+Space`.
- **Diagnostics** — errors and `cargo check` results in the bottom panel, with
  click-to-jump. Inline error messages can also be shown right in the code.
- **Go to definition** (`F12`) opens the target in a dedicated **Definition** tab,
  with the relevant line highlighted so it stands out.
- **Rename** (`Ctrl+R`) renames a symbol across the whole project.
- Re-checks run on a short idle debounce (and on save), so typing stays smooth.

### Editing shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+Space` | Completion — code suggestions, or in **Cargo.toml**, crate names + live versions |
| `Ctrl+/` | Toggle line comment (`//` for Rust, `#` for TOML) |
| `Ctrl+↑` / `Ctrl+↓` | Move the selected line(s) up / down |
| `Ctrl+X` | Delete the line at the cursor |
| `Ctrl+Shift+F` | Format / re-indent the file |
| `Ctrl+R` | Rename the symbol project-wide |
| `F12` | Go to definition (Definition tab) |
| `Ctrl+C` | Copy — over an inline error, copies the message **with its code** (e.g. `… [E0599]`) |

Hovering an inline error also shows a clickable `[E####]` link to the official
Rust error documentation.

### Right-click menu
Right-clicking in the editor opens a context menu listing **every command above
with its shortcut**, so you don't have to memorise them — including Delete line,
Toggle comment, Move line up/down, Format, Rename, Go to definition, Completion,
Copy and Select all.

### Cargo.toml dependency completion
Inside `Cargo.toml`, `Ctrl+Space` helps you add dependencies:

1. It suggests a **curated list of embedded-relevant crates** (HALs, PACs,
   `embedded-hal`, `embassy-*`, `defmt`, `heapless`, drivers, and more).
2. After you pick a crate, it fetches that crate's **available versions live from
   crates.io** and lets you choose one.

---

## Build & flash

- **Check / Build** — runs `cargo` in a managed workspace; errors are parsed and
  listed in the bottom panel with click-to-jump.
- **Flash** — program the board straight from the toolbar:
  - **STM32** — SWD via **OpenOCD**, or `cargo run` via **probe-rs** (already
    wired up in the generated `.cargo/config.toml`). USB-DFU programmers are
    detected too.
  - **ESP32-C3** — via **espflash** (`espflash flash --monitor`).
- **Required Tools tab** — checks whether the external tools each workflow needs
  are installed, so you find out before you flash, not during.

---

## Project management

- **Save / Open project** — export the generated project to a folder and reopen
  it later; the IDE restores the exact chip, pin assignments, clock config, and
  all of your edits. Saving an existing project writes back to its folder; a new
  project asks where to put it.
- **Import MCU** — drop a chip definition (`.ron`) into your `mcus/` folder (or
  use *Import MCU…*) and it shows up in the chip picker. A new chip **inside an
  already-supported family needs no rebuild** — it's pure data.

---

## Supported microcontrollers

| Chip | Core | HAL | Highlights |
|------|------|-----|------------|
| **STM32F103C8T6** | ARM Cortex-M3 | `stm32f1xx-hal` | full pin map, peripherals, clock tree, codegen, SWD/DFU/probe-rs flashing |
| **ESP32-C3** | RISC-V 32-bit | `esp-hal` | pin matrix, peripherals, clock graph, codegen, espflash |

More importable examples (including graph-clock demos and an STM32F103RB) ship in
`assets/mcus/examples/`.

---

## Getting started

### Prerequisites
- **Rust** (stable, edition 2024) — install with [rustup](https://rustup.rs/).
- **rust-analyzer** on your `PATH` — for completion and diagnostics.
- Build targets for the chips you use:
  - STM32: `rustup target add thumbv7m-none-eabi`
  - ESP32-C3: `rustup target add riscv32imc-unknown-none-elf`
- Flashing tools as needed: [`probe-rs`](https://probe.rs/),
  [OpenOCD](https://openocd.org/),
  [`espflash`](https://github.com/esp-rs/espflash), `dfu-util`. The **Required
  Tools** tab checks these for you.

### Run the IDE
```bash
cargo run            # debug
cargo run --release  # release
```
The app opens maximized with a default STM32F103C8T6. Configure pins and the
clock, and the generated `src/main.rs` appears in the editor.

---

## Typical workflow
1. **Choose a chip** (or *Import MCU…* a `.ron`).
2. **Pins / Peripherals** — assign functions, or drop in USART/SPI/I²C devices.
3. **Clock** — shape the clock tree and watch frequencies + warnings update live.
4. The editor shows the generated `main.rs`; **write your logic in the `loop {}`
   body** — it survives every regeneration.
5. **Check / Build** — fix anything from the bottom panel.
6. **Flash** — program the board (SWD / probe-rs / DFU / espflash).
7. **Save Project** — export to a folder and reopen anytime.

---

## Under the hood (briefly)

The IDE is built with [egui/eframe](https://github.com/emilk/egui). Chip
definitions — pin layouts, peripheral maps, clock trees and limits — are stored
as **data** (`.ron` files), while each chip *family* has a small code backend for
HAL-specific generation. The practical upshot: adding a new chip to an existing
family is just data, and the clock tree you see is driven by that data, not
hard-coded.

```bash
cargo test    # clock evaluator, codegen, pin/peripheral logic, editor helpers, …
```

## License
Not yet specified.
