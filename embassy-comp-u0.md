# stm32: generate the comparator pin impls for `comp_u0`, and add an example

Base: `6d6afaa` (embassy-rs/embassy `main`).
Patch: `embassy-comp-u0.patch` — `git apply embassy-comp-u0.patch`.

## The bug

`comp.rs` has `#[cfg(comp_u0)]` arms throughout and `lib.rs` exposes the module
on `comp_u0`, so the driver looks supported on the STM32U0. It is not reachable:
`build.rs` generates the `NonInvertingPin` / `InvertingPin` impls only for

```rust
regs.version == "u5" || regs.version == "v1" || regs.version == "v2"
```

so on a U0 no pin implements them and `Comp::new` cannot be called at all:

```
error[E0277]: the trait bound `PB2: embassy_stm32::comp::NonInvertingPin<COMP1>` is not satisfied
```

The crate itself builds, which is presumably why this went unnoticed — there was
no comparator example anywhere in the tree to try it with.

## The fix

Add `u0` to that list. One line.

## The example

`examples/stm32u0/src/bin/comp.rs`, on the STM32U083MC the U0 examples target.
COMP1 against an internal VREFINT tap (one pin), COMP2 against a pin (two).

Two things it points out, both specific to this family:

* the comparators **share their interrupt with the ADC** (`ADC_COMP1_2`), so a
  program using both puts all three handlers on one `bind_interrupts!` key;
* there are two power modes here, not three — no `PowerMode::UltraLowPower`.

## Verified

`cargo check --bin comp` in `examples/stm32u0`, on a checkout with **only this
patch applied**, so it does not lean on anything else in flight.

**Not tested on hardware** — I have no U0 board; this is compile-verified only.

## Related

I have a second PR adding `comp_v3` (STM32L4 / L4+ / L5), which needs the same
`build.rs` list. The two touch that one line, so whichever lands second wants a
trivial rebase.
