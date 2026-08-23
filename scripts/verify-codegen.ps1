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

.PARAMETER Warnings
  Fail a case that compiles with warnings, not just one that errors. Generated
  code is meant to be warning-free; this is how that stays true.

.EXAMPLE
  pwsh scripts/verify-codegen.ps1
  pwsh scripts/verify-codegen.ps1 -Full -Warnings
#>
[CmdletBinding()]
param(
    [switch]$Full,
    [switch]$Warnings
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

# label, emit test, environment for the run, quick?
#
# The env hash is the case: every key is a knob the emit test reads, and an
# empty hash means "as wired by default".
$CASES = @(
    @{ n = "F1 blocking, full wiring";     t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off" };  q = $true }
    @{ n = "F1 blocking, DMA tx";          t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "tx" };   q = $false }
    @{ n = "F1 blocking, DMA rx";          t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "rx" };   q = $false }
    @{ n = "F1 blocking, DMA both";        t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "both" }; q = $true }
    @{ n = "F1 SPI without MISO";          t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "both"; EIDE_SPI_TXONLY = "1" }; q = $true }
    @{ n = "F1 USART TX only";             t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USART_HALF = "tx" }; q = $true }
    @{ n = "F1 USART RX only";             t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USART_HALF = "rx" }; q = $false }
    @{ n = "F1 I2C SCL only";              t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_I2C_HALF = "scl" };  q = $true }
    @{ n = "F1 I2C SDA only";              t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_I2C_HALF = "sda" };  q = $false }
    @{ n = "F1 CAN TX only";               t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_CAN_HALF = "tx" };   q = $true }
    @{ n = "F1 CAN RX only";               t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_CAN_HALF = "rx" };   q = $false }
    @{ n = "F1 USB, both pads";            t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USB = "both" };      q = $true }
    @{ n = "F1 USB, D- only";              t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USB = "dm" };        q = $true }
    @{ n = "F1 USB, D+ only";              t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USB = "dp" };        q = $false }
    @{ n = "F1 USB D- + GPIO on its pad";  t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_USB = "dm-gpio" };   q = $true }
    @{ n = "F1 every bus half-wired";      t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "both"; EIDE_USART_HALF = "rx"; EIDE_SPI_TXONLY = "1"; EIDE_I2C_HALF = "scl" }; q = $true }
    @{ n = "F1 Async (inert, = blocking)"; t = "emit_f1_dma_project";    e = @{ EIDE_F1_DMA = "off"; EIDE_F1_RUNTIME = "async" }; q = $true }
    @{ n = "F1 RTIC";                      t = "emit_f1_rtic_project";   e = @{};                       q = $true }
    @{ n = "F1 Native";                    t = "emit_f1_native_project"; e = @{};                       q = $true }
)

# Every knob any case sets, so one case cannot leak into the next.
$KNOBS = @("EIDE_F1_DMA", "EIDE_SPI_TXONLY", "EIDE_USART_HALF", "EIDE_I2C_HALF",
           "EIDE_CAN_HALF", "EIDE_USB", "EIDE_F1_RUNTIME")

$cases = if ($Full) { $CASES } else { $CASES | Where-Object { $_.q } }
Write-Host ("running {0} of {1} cases{2}" -f $cases.Count, $CASES.Count,
    $(if ($Full) { "" } else { "  (use -Full for all)" }))
Write-Host ""

$results = @()
foreach ($c in $cases) {
    foreach ($k in $KNOBS) { Remove-Item ("Env:\" + $k) -ErrorAction SilentlyContinue }
    foreach ($k in $c.e.Keys) { Set-Item ("Env:\" + $k) $c.e[$k] }

    Set-Location $repo
    $out = cargo test --bin embedded_ide_0 $c.t -- --ignored --nocapture 2>&1
    if ($out | Select-String -Pattern "panicked at|test result: FAILED") {
        $results += [pscustomobject]@{ Case = $c.n; Status = "EMIT FAILED"; Detail = "the harness's own assertions" }
        continue
    }

    # The harness prints where it wrote and what to build it for; trusting those
    # lines is what keeps this script from duplicating the directory table.
    $dirs = @($out | Select-String -Pattern "^wrote (.+)$" | ForEach-Object { $_.Matches[0].Groups[1].Value.Trim() })
    $target = ($out | Select-String -Pattern "^target: (.+)$" | Select-Object -First 1)
    $target = if ($target) { $target.Matches[0].Groups[1].Value.Trim() } else { $null }
    if (-not $dirs -or -not $target) {
        $results += [pscustomobject]@{ Case = $c.n; Status = "NO OUTPUT"; Detail = "harness printed no 'wrote'/'target:' line" }
        continue
    }

    $status = "ok"
    $detail = ""
    foreach ($d in $dirs) {
        Set-Location $d
        $r = cargo check --target $target 2>&1
        $errs = @($r | Select-String -Pattern "^error(\[|:)").Count
        $warns = @($r | Select-String -Pattern "^warning: ").Count
        if ($errs -gt 0) {
            $status = "$errs ERRORS"
            $detail = ($r | Select-String -Pattern "^error(\[|:)" | Select-Object -First 1).Line.Trim()
            break
        }
        if ($warns -gt 0) {
            if ($Warnings) { $status = "$warns warnings" } elseif ($status -eq "ok") { $status = "ok ($warns warn)" }
            $detail = ($r | Select-String -Pattern "^warning: " | Select-Object -First 1).Line.Trim()
        }
    }
    $results += [pscustomobject]@{ Case = $c.n; Status = $status; Detail = $detail }
    $colour = if ($status -like "*ERROR*") { "Red" } elseif ($status -like "*warn*") { "Yellow" } else { "Green" }
    Write-Host ("  {0,-34} {1}" -f $c.n, $status) -ForegroundColor $colour
}

Set-Location $repo
foreach ($k in $KNOBS) { Remove-Item ("Env:\" + $k) -ErrorAction SilentlyContinue }

Write-Host ""
$bad = @($results | Where-Object {
    $_.Status -like "*ERROR*" -or $_.Status -like "*FAILED*" -or $_.Status -eq "NO OUTPUT" -or
    ($Warnings -and $_.Status -like "*warnings")
})
if ($bad) {
    Write-Host "FAILED:" -ForegroundColor Red
    $bad | ForEach-Object { Write-Host ("  {0}: {1}`n      {2}" -f $_.Case, $_.Status, $_.Detail) -ForegroundColor Red }
    exit 1
}
Write-Host ("all {0} cases compile" -f $results.Count) -ForegroundColor Green
exit 0
