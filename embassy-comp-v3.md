# stm32: add comparator support for `comp_v3` (STM32L4, L4+, L5)

Base: `6d6afaa` (embassy-rs/embassy `main`).
Patch: `embassy-comp-v3.patch` — `git apply embassy-comp-v3.patch`.

## What

`embassy_stm32::comp` covers `comp_u5`, `comp_v1`, `comp_v2` and `comp_u0`.
This adds `comp_v3`, which is the STM32L4, L4+ and L5 comparator — 176 chips in
`stm32-metapac` that have the peripheral in the PAC and no driver.

It also adds the first comparator examples in the tree, one per generation the
driver now covers with a distinct shape:

* `examples/stm32l4/src/bin/comp.rs` — COMP1 against an internal VREFINT tap
  (one pin) and COMP2 against a pin (two), both on the single `COMP` vector
  this family gives them.
* `examples/stm32g4/src/bin/comp.rs` — the same two shapes, but on **two**
  vectors: COMP1 on `COMP1_2_3` and COMP4 on `COMP4`, which is what a G491
  (no COMP5) routes them to. It also uses the millivolt hysteresis steps, and
  says in a comment why `Config::power_mode` is left alone here — the driver
  only writes it on `comp_u5`.
* `examples/stm32u5/src/bin/comp.rs` — the `comp_u5` generation, where
  `power_mode` IS applied, so the example sets it. It also points at
  `window_mode` / `window_output`, which exist on this generation and on no
  other one the driver covers.
* `examples/stm32wba/src/bin/comp.rs` — the same generation on a radio part,
  with that chip's pins. Kept as its own file rather than a pointer at the U5
  one, the way `blinky` and `button_exti` already are, so it can be run
  straight off a WBA55 board.
A `comp_u0` example and a `build.rs` fix that goes with it are a separate PR
(`embassy-comp-u0.patch`) — an independent bug, not part of this change. The two
touch the same `build.rs` line, so whichever merges second needs a one-line
rebase.

The register block is close to `comp_v2`:

| | `comp_v2` | `comp_v3` |
|---|---|---|
| hysteresis | 8 steps, 10..70 mV | 4 levels (`NONE/LOW/MEDIUM/HIGH`) |
| power mode | absent | `PWRMODE`, 4 levels |
| blanking | 7 sources | 2 (`TIM1OC5`, `TIM2OC3`) |
| INMSEL | `vals::Inm` enum | raw `u8` (no enumeration in stm32-data) |
| window mode | absent | absent |
| `SCALEN` / `BRGEN` | present | present |

## Mapping decisions, for review

* **`Hysteresis`** joins the four-level group with `u5`/`v1`/`u0`.
* **`PowerMode::UltraLowPower` → `Pwrmode::VeryLowSpeed`.** v3 has four modes;
  `PowerMode` has three, so `LowSpeed` is not reachable. Say the word and I will
  add a variant instead.
* **`BlankingSource::Blank3` is `#[cfg(not(comp_v3))]`** — v3 has two sources.
* **`INMSEL` is written as a number.** `set_inmsel` takes `u8` here because
  stm32-data carries no enumeration for this version. The order used is the one
  `comp_v2` encodes in `vals::Inm` (`0` = ¼ VREFINT … `7` = INM2), which matches
  RM0351 §COMP_CSR. **This is the part most worth a second pair of eyes.**
* **`SCALEN` / `BRGEN`** follow the same rule as v2, expressed on the numeric
  field: the scaler for INMSEL 0..=3 (the VREFINT taps), the bridge for 0..=2
  (the divided ones).
* **EXTI lines 21 and 22**, as on the G4.

## The one thing that is not clean

`stm32-data` gives L4/L5 comparators **no RCC entry** (`rcc: None`), so
`peripherals::COMPx::RCC_INFO` does not exist and `Info` cannot be filled the
usual way. Their registers sit behind the SYSCFG clock gate — which is exactly
what the G4's own COMP metadata records (`APB2ENR.SYSCFGEN`) — so this patch
borrows `peripherals::SYSCFG::RCC_INFO` under `#[cfg(comp_v3)]`.

The tidier fix is in stm32-data: give COMP on L4/L5 the same RCC entry the G4
has, after which that `cfg` can go. Happy to do that instead if you prefer it
first.

## Verified

`cargo check` on the patched tree, one chip per comparator generation, so the
new `cfg` arms cannot have broken an existing one:

| chip | version | target | |
|---|---|---|---|
| STM32L476RG | v3 | thumbv7em-none-eabihf | ok |
| STM32L4R5ZI | v3 | thumbv7em-none-eabihf | ok |
| STM32L552ZE | v3 | thumbv8m.main-none-eabihf | ok |
| STM32G474RE | v2 | thumbv7em-none-eabihf | ok |
| STM32U575ZI | u5 | thumbv8m.main-none-eabihf | ok |
| STM32G071RB | v1 | thumbv6m-none-eabi | ok |
| STM32U083RC | u0 | thumbv6m-none-eabi | ok |
| STM32WBA55CG | u5 | thumbv8m.main-none-eabihf | ok |
| STM32F103C8 | none | thumbv7m-none-eabi | ok (stub module) |

All four build: `cargo check --bin comp` in `examples/stm32l4` (STM32L4R5ZI),
`examples/stm32g4` (STM32G491RE), `examples/stm32u5` (STM32U5G9ZJ) and
`examples/stm32wba` (STM32WBA55CG). Each exercises both constructors and the
async wait; the L4 one looks like this:

```rust
bind_interrupts!(pub struct Irqs {
    COMP => comp::InterruptHandler<peripherals::COMP1>,
            comp::InterruptHandler<peripherals::COMP2>;
});

let mut comp1 = Comp::new(p.COMP1, p.PB2, Irqs, config);    // 3/4 VREFINT
comp1.enable();

let mut comp2 = Comp::new_with_input_minus_pin(p.COMP2, p.PB4, p.PB7, Irqs, Config::default());
comp2.enable();

loop {
    comp1.wait_for_any_edge().await;
    info!("COMP1 crossed, now {}", comp1.output_level());
}
```

The same program was also compiled for an STM32L476RG (plain L4) outside the
examples tree, to check the driver on both flash sizes of the family.

**Not tested on hardware** — I have no L4/L5 board, so the example has never
been run, only built. Everything above is
compile-verified only, and the INMSEL encoding and the SYSCFG gate are the two
claims that deserve a check on silicon before merge.
