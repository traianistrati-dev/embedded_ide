<#
.SYNOPSIS
  Emit generated projects for a matrix of configurations and cross-compile them.

.DESCRIPTION
  The `#[ignore]`d emit tests each write ONE project to a temp directory. That
  is the only way this repo can find out whether the code it generates actually
  builds, and every codegen bug found so far was found by running one of them by
  hand — which means the ones nobody thought to run stayed broken for months:
  half-wired buses that named undeclared bindings, a CAN path that had never
  compiled at all, a PWM example printed against an API that no longer existed.

  This script is those runs, written down. It drives each emit test with the
  environment the case needs, reads the `wrote <path>` / `target: <triple>`
  lines the test prints, cross-compiles what it finds, and reports one line per
  case. Adding a case is one row in $CASES.

.PARAMETER Full
  Run every case. Without it, a representative subset that still covers each
  runtime and each "half-wired" shape.

.NOTES
  Warnings are checked too, and by COUNT: each case declares how many it is
  allowed (`w`, default none) and anything else fails. There is no "warnings are
  fine" mode — that is precisely how five of them lived in the ESP backend, on
  the fully-wired path, until this matrix first covered it.

.EXAMPLE
  pwsh scripts/verify-codegen.ps1
  pwsh scripts/verify-codegen.ps1 -Full
#>
[CmdletBinding()]
param(
    [switch]$Full
)

$ErrorActionPreference = "Continue"
$repo = Split-Path -Parent $PSScriptRoot

# `rtic-macros` counts every CARGO_FEATURE_* variable it can see as an enabled
# feature, cargo's own and the shell's alike, and refuses to build when there is
# more than one. A stray one in the user environment therefore breaks every RTIC
# case with an error that points at a correct Cargo.toml. The IDE reports this
# at startup (see `required_tools.rs`); here we simply do not pass it on.
$leaked = @(Get-ChildItem Env: | Where-Object { $_.Name -like "CARGO_FEATURE_*" })
foreach ($v in $leaked) { Remove-Item ("Env:\" + $v.Name) -ErrorAction SilentlyContinue }
if ($leaked) {
    Write-Host ("note: ignoring {0} stray CARGO_FEATURE_* variable(s) from this environment: {1}" -f
        $leaked.Count, ($leaked.Name -join ", ")) -ForegroundColor DarkYellow
}

# Where the STM32Cube database is, if it is anywhere. The two importer cases
# need it; everything else is built from definitions bundled in the repo.
$CUBE_DB = if ($env:EIDE_CUBE_DB) { $env:EIDE_CUBE_DB }
           else { "H:\stm32cube-database-master\stm32cube-database-master\db\mcu" }

# label, emit test, environment for the run, quick?, prerequisite path, and `w`
# — how many warnings the case is allowed, default none.
#
# EXACTLY that many, not "at most": generated code is meant to be warning-free,
# and the few that remain are deliberate (a half-wired bus leaves its pad bound
# and unused, which is the compiler naming the same pad the generated comment
# names). Writing the number down is what makes a NEW warning fail the run —
# a threshold of "warnings are fine" is how five of them lived in the ESP
# backend, on the fully-wired path, until this matrix first covered it.
# It also fails when a case stops warning, so a fixed one cannot quietly keep
# its allowance.
#
# The env hash is the case: every key is a knob the emit test reads, and an
# empty hash means "as wired by default".
$CASES = @(
    @{ n = "F1 blocking, full wiring";     t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off" };  q = $true }
    @{ n = "F1 blocking, DMA tx";          t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "tx" };   q = $false }
    @{ n = "F1 blocking, DMA rx";          t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "rx" };   q = $false }
    @{ n = "F1 blocking, DMA both";        t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "both" }; q = $true }
    @{ n = "F1 SPI without MISO";          t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "both"; EIDE_SPI_TXONLY = "1" }; q = $true }
    # `w` is how many warnings this case is ALLOWED — see the note above $CASES.
    # A half-wired bus leaves its pad bound and unused on purpose, and that
    # warning is the compiler naming the same pad the generated comment does.
    @{ n = "F1 USART TX only";             t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USART_HALF = "tx" }; q = $true;  w = 2 }
    @{ n = "F1 USART RX only";             t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USART_HALF = "rx" }; q = $false; w = 2 }
    @{ n = "F1 I2C SCL only";              t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_I2C_HALF = "scl" };  q = $true;  w = 2 }
    @{ n = "F1 I2C SDA only";              t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_I2C_HALF = "sda" };  q = $false; w = 2 }
    @{ n = "F1 CAN TX only";               t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_CAN_HALF = "tx" };   q = $true;  w = 2 }
    @{ n = "F1 CAN RX only";               t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_CAN_HALF = "rx" };   q = $false; w = 2 }
    @{ n = "F1 USB, both pads";            t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USB = "both" };      q = $true }
    @{ n = "F1 USB, D- only";              t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USB = "dm" };        q = $true }
    @{ n = "F1 USB, D+ only";              t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USB = "dp" };        q = $false }
    @{ n = "F1 USB D- + GPIO on its pad";  t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USB = "dm-gpio" };   q = $true }
    @{ n = "F1 every bus half-wired";      t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "both"; EIDE_USART_HALF = "rx"; EIDE_SPI_TXONLY = "1"; EIDE_I2C_HALF = "scl" }; q = $true; w = 3 }
    @{ n = "F1 Async (inert, = blocking)"; t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_F1_RUNTIME = "async" }; q = $true }
    @{ n = "F1 RTIC";                      t = "emit_f1_rtic_project";   e = @{};                       q = $true }
    @{ n = "F1 Native";                    t = "emit_f1_native_project"; e = @{};                       q = $true }

    # A different HAL and a different entry point, so a different set of ways to
    # be wrong: esp-hal bindings, and the esp-rtos scheduler on the async one.
    @{ n = "ESP32-C3 blocking";            t = "emit_esp32c3_project";       e = @{ ESP_ASYNC_RUNTIME = "blocking" }; q = $true }
    @{ n = "ESP32-C3 async (esp-rtos)";    t = "emit_esp32c3_project";       e = @{};                       q = $true }
    # The harness wires ONE LEDC channel by default, so the two cases above only
    # ever reach the single-channel shape. Two channels is a different file: the
    # return type becomes a tuple, and the duty trait addresses it by POSITION,
    # which is not the channel number.
    @{ n = "ESP32-C3, two PWM channels";   t = "emit_esp32c3_project";       e = @{ EIDE_ESP_PWM = "0,2" }; q = $true }

    # ONE test, NINE projects, four targets — GPIO, async, USART, DMA on F4/F2/F7,
    # the watchdogs and WBA. Each prints its own `target:`, so they are paired
    # individually rather than forced onto one triple.
    @{ n = "embassy (9 projects)";         t = "emit_embassy_project";       e = @{};                       q = $true }

    # These two build from a REAL part in the vendor database rather than from a
    # bundled definition, which is the only way to exercise the importer's own
    # output — channel names, interrupt names, the `bind_interrupts!` grouping.
    # `p` is what they need; without it they are skipped, not failed, because a
    # machine without the database is a normal machine.
    @{ n = "imported chip, async DMA";     t = "emit_imported_dma_project";  e = @{}; q = $true; p = $CUBE_DB }
    @{ n = "imported chip, comparators";   t = "emit_comp_project";          e = @{}; q = $true; p = $CUBE_DB }

    # Not a project: a VERDICT (`v`). STM32WL30 is the chip the import preflight
    # was written for — `embassy-stm32` publishes no `stm32wl3*` feature, its
    # clock tree is an architecture no recipe can read, and its DMA channels
    # only appeared once `parse_value` started reading the vendor's own range.
    # It cannot be cross-compiled BECAUSE of the first of those, so what is
    # pinned here is the verdict itself, with G071 alongside as the control.
    # This case fails the day a `stm32wl3` recipe lands and the answer has to
    # change — which is the only way anyone would remember to change it.
    @{ n = "WL30 preflight verdict";       t = "wl30_is_the_chip_this_preflight_exists_for"; e = @{}; q = $true; p = $CUBE_DB; v = $true }
)

# Every knob any case sets, so one case cannot leak into the next.
$KNOBS = @("EIDE_F1_DMA", "EIDE_SPI_TXONLY", "EIDE_USART_HALF", "EIDE_I2C_HALF",
           "EIDE_CAN_HALF", "EIDE_USB", "EIDE_F1_RUNTIME", "ESP_ASYNC_RUNTIME",
           "EIDE_ESP_PWM")

$cases = if ($Full) { $CASES } else { $CASES | Where-Object { $_.q } }
Write-Host ("running {0} of {1} cases{2}" -f $cases.Count, $CASES.Count,
    $(if ($Full) { "" } else { "  (use -Full for all)" }))
Write-Host ""

$results = @()
foreach ($c in $cases) {
    if ($c.p -and -not (Test-Path $c.p)) {
        $results += [pscustomobject]@{ Case = $c.n; Status = "skipped"; Detail = "no vendor database at $($c.p)" }
        Write-Host ("  {0,-34} skipped (no database)" -f $c.n) -ForegroundColor DarkGray
        continue
    }
    foreach ($k in $KNOBS) { Remove-Item ("Env:\" + $k) -ErrorAction SilentlyContinue }
    foreach ($k in $c.e.Keys) { Set-Item ("Env:\" + $k) $c.e[$k] }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()

    Set-Location $repo
    $out = cargo test --bin embedded_ide_0 $c.t -- --ignored --nocapture 2>&1
    if ($out | Select-String -Pattern "panicked at|test result: FAILED") {
        $results += [pscustomobject]@{ Case = $c.n; Status = "EMIT FAILED"; Detail = "the harness's own assertions" }
        continue
    }
    # A filter that matches nothing is a SUCCESSFUL cargo run of zero tests, so
    # a typo in the test name would otherwise read as "the harness printed
    # nothing" — a wrong diagnosis pointing at the wrong file.
    if (-not ($out | Select-String -Pattern "test result: ok\. [1-9]")) {
        $results += [pscustomobject]@{ Case = $c.n; Status = "NO SUCH TEST"; Detail = "cargo ran 0 tests for filter '$($c.t)'" }
        continue
    }

    # A verdict case has nothing to build: the two checks above — the test ran,
    # and it did not fail — ARE the case. Everything below is about projects.
    if ($c.v) {
        $results += [pscustomobject]@{ Case = $c.n; Status = "ok (verdict)"; Detail = ""; Seconds = $sw.Elapsed.TotalSeconds }
        Write-Host ("  {0,-34} {1,-22} {2,5:N0}s" -f $c.n, "ok (verdict)", $sw.Elapsed.TotalSeconds) -ForegroundColor Green
        continue
    }

    # The harness says where it wrote and what to build it for; trusting those
    # lines is what keeps this script from duplicating the directory table.
    #
    # Three shapes exist today and all three are accepted, because normalising
    # them means editing a dozen tests to fix a script:
    #   wrote <path>                    F1, ESP — followed by its own `target:`
    #   wrote <path> (Display Name)     the chip-database harnesses
    #   wrote <path>  …no target line   the embassy harness, several projects
    # A `target:` line applies to the `wrote` above it, so a harness emitting
    # several projects for several targets pairs up correctly. `t2` on the case
    # is the fallback for the ones that print none.
    $projects = @()
    foreach ($line in $out) {
        $l = "$line"
        if ($l -match "^wrote (\S+)") {
            $projects += [pscustomobject]@{ Dir = $matches[1]; Target = $c.t2 }
        } elseif ($l -match "^target: (\S+)" -and $projects.Count -gt 0) {
            $projects[-1].Target = $matches[1]
        }
    }
    $projects = @($projects | Where-Object { $_.Dir -and (Test-Path $_.Dir) })
    if (-not $projects) {
        $results += [pscustomobject]@{ Case = $c.n; Status = "NO OUTPUT"; Detail = "harness printed no usable 'wrote' line" }
        continue
    }
    if ($projects | Where-Object { -not $_.Target }) {
        $results += [pscustomobject]@{ Case = $c.n; Status = "NO TARGET"; Detail = "harness printed no 'target:' and the case declares no t2" }
        continue
    }

    $status = "ok"
    $detail = ""
    $seen = 0
    foreach ($p in $projects) {
        Set-Location $p.Dir
        $r = cargo check --target $p.Target 2>&1
        $errs = @($r | Select-String -Pattern "^error(\[|:)").Count
        $w = @($r | Select-String -Pattern "^warning: ")
        $seen += $w.Count
        if ($errs -gt 0) {
            $status = "$errs ERRORS"
            $detail = ($r | Select-String -Pattern "^error(\[|:)" | Select-Object -First 1).Line.Trim()
            break
        }
        if ($w.Count -gt 0 -and -not $detail) { $detail = $w[0].Line.Trim() }
    }
    $allowed = if ($null -ne $c.w) { [int]$c.w } else { 0 }
    if ($status -eq "ok" -and $seen -ne $allowed) {
        $status = "$seen warn, expected $allowed"
        if ($seen -lt $allowed) { $detail = "fewer warnings than declared - lower `w` on this case" }
    } elseif ($status -eq "ok" -and $seen -gt 0) {
        $status = "ok ($seen expected)"
    }
    $results += [pscustomobject]@{ Case = $c.n; Status = $status; Detail = $detail; Seconds = $sw.Elapsed.TotalSeconds }
    $colour = if ($status -like "*ERROR*") { "Red" } elseif ($status -like "*warn*") { "Yellow" } else { "Green" }
    Write-Host ("  {0,-34} {1,-22} {2,5:N0}s" -f $c.n, $status, $sw.Elapsed.TotalSeconds) -ForegroundColor $colour
}

Set-Location $repo
foreach ($k in $KNOBS) { Remove-Item ("Env:\" + $k) -ErrorAction SilentlyContinue }

Write-Host ""
$bad = @($results | Where-Object {
    $_.Status -like "*ERROR*" -or $_.Status -like "*FAILED*" -or
    $_.Status -like "NO *" -or $_.Status -like "*expected*" -and $_.Status -notlike "ok *"
})
if ($bad) {
    Write-Host "FAILED:" -ForegroundColor Red
    $bad | ForEach-Object { Write-Host ("  {0}: {1}`n      {2}" -f $_.Case, $_.Status, $_.Detail) -ForegroundColor Red }
    exit 1
}
$skipped = @($results | Where-Object { $_.Status -eq "skipped" })
$ran = $results.Count - $skipped.Count
$timed = @($results | Where-Object { $_.Seconds })
if ($timed) {
    $total = ($timed | Measure-Object -Property Seconds -Sum).Sum
    $worst = $timed | Sort-Object -Property Seconds -Descending | Select-Object -First 3
    Write-Host ("{0:N0}s total; slowest: {1}" -f $total,
        (($worst | ForEach-Object { "{0} {1:N0}s" -f $_.Case, $_.Seconds }) -join ", ")) -ForegroundColor DarkGray
}
Write-Host ("all {0} cases pass{1}" -f $ran,
    $(if ($skipped) { " ($($skipped.Count) skipped)" } else { "" })) -ForegroundColor Green
exit 0
