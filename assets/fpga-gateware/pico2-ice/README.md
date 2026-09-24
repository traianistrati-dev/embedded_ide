# FPGA gateware for the pico2-ice

The firmware in `src/main.rs` loads `top.bin` into the iCE40UP5K at every
boot. The RP2350 sends it straight into the FPGA's configuration RAM; the
FPGA's own flash is never written.

The IDE put these files here once and never overwrites them. Replace
`top.bin` with your own design and the next Build, Check or Flash picks it up.

| File | What it is |
|---|---|
| `top.bin` | The bitstream the firmware loads (an `icepack` binary, 104,090 bytes for a UP5K) |
| `top.v` | Its source |
| `pico2_ice.pcf` | tinyVision's pin catalogue for the board: which FPGA pin each name is |
| `LICENSE-tinyvision-MIT.txt` | The licence `pico2_ice.pcf` comes under |

## What the default design shows

It is deliberately not the factory image. The FPGA's flash already holds
tinyVision's blink (dark, red, blue, green, 0.35 s each), and the FPGA boots it
by itself when the firmware releases the FPGA without loading anything. A blink
that looked like that one would prove nothing.

| LED (the FPGA's RGB LED) | Meaning |
|---|---|
| Red blinking, blue dark | `top.bin` was loaded. It runs on the FPGA's own oscillator. The factory image blinks red too, but it also lights blue. |
| Green blinking | The RP2350's clock (GP21) reaches FPGA pin 35. |
| Blue | Never lights. If it does, the FPGA is running the factory image instead. |

The green LED D3 beside the FPGA lights when the FPGA holds any
configuration at all (CDONE).

## Rebuilding it, or building your own

With the open-source iCE40 tools from the OSS CAD Suite:

```
yosys -q -p "synth_ice40 -top top -json top.json" top.v
nextpnr-ice40 --up5k --package sg48 --pcf pico2_ice.pcf --json top.json --asc top.asc
icepack top.asc top.bin
```

With the YoWASP Python packages instead
(`pip install yowasp-yosys yowasp-nextpnr-ice40`; `icepack` comes with the
second one), each command carries a `yowasp-` prefix:

```
yowasp-yosys -q -p "synth_ice40 -top top -json top.json" top.v
yowasp-nextpnr-ice40 --up5k --package sg48 --pcf pico2_ice.pcf --json top.json --asc top.asc
yowasp-icepack top.asc top.bin
```

The shipped `top.bin` was built with the YoWASP commands, Yosys 0.69 and
nextpnr-ice40 0.11.1. Its SHA-256 is
`1e5adfe18561a163fa187107169e4fe46861d925737b2165002df7c2a4e1a436`, and
rebuilding reproduces it byte for byte.

Your own design can use any name in `pico2_ice.pcf`. The file lists every
FPGA pin on the board, including the PMOD headers and the pins shared with
the RP2350. One name is wrong for this board: `SRAM_SS` (pin 37) comes from
the original pico-ice. On the pico2-ice, pin 37 reaches only header J2 pin 17,
and the PSRAM hangs off the RP2350, where the FPGA cannot reach it.

### From VHDL

GHDL can turn VHDL into Verilog that the same flow accepts. This route is
marked experimental by GHDL itself.

```
ghdl -a --std=08 top.vhd
ghdl --synth --std=08 --out=verilog top > top_vhdl.v
```

Then run the three commands above on `top_vhdl.v`.

In Windows PowerShell 5, which the IDE's Terminal tab runs, `>` writes UTF-16
and Yosys cannot read that. Write the file this way instead:

```
ghdl --synth --std=08 --out=verilog top | Out-File -Encoding ascii top_vhdl.v
```

## Getting the stock firmware back

Flashing a generated project replaces the RP2350's MicroPython. To restore it:
hold the BOOTSEL button (or short BT, J2 pin 38, to GND) while plugging in USB,
and copy `pico2_ice.uf2` from the pico-ice-micropython releases onto the drive
that appears.

MicroPython keeps its files above the first 1 MiB of flash, which a generated
project does not reach, so its `main.py` is normally still there. If it is gone
(after a full erase, for example), the FPGA stays in reset, because the board
pulls CRESET low. Create `main.py` on the MicroPython drive with:

```python
from machine import Pin
import ice
fpga = ice.fpga(cdone=Pin(40), clock=Pin(21), creset=Pin(31), cram_cs=Pin(5), cram_mosi=Pin(4), cram_sck=Pin(6), frequency=48)
fpga.start()
```
