#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Regenerates docs/assets/demo.svg, the animated terminal demo at the top of the README.
.DESCRIPTION
  Builds nastro, then runs the render_demo example, which drives the *real* TUI in a pseudo-terminal against a few
  made-up recordings and writes what it saw as an animated SVG. Re-run it after changing how the TUI looks or behaves,
  so the README never shows something the program no longer does. The temporary recordings (and the machine's user
  name, which appears in their paths) are not kept: the example redacts the folder path in the picture.
#>
$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..')
try {
  cargo build -p nastro
  if ($LASTEXITCODE -ne 0) { throw 'build failed' }
  cargo run -p nastro --example render_demo -- ./target/debug/nastro$(if ($IsWindows) { '.exe' }) docs/assets/demo.svg
  if ($LASTEXITCODE -ne 0) { throw 'rendering failed' }
  $svg = Get-Content docs/assets/demo.svg -Raw
  if ($svg -match [regex]::Escape($env:USERNAME) -and $env:USERNAME.Length -gt 3) { throw "the demo mentions the local user name ($env:USERNAME); fix the redaction" }
  Write-Host 'Review the result, then commit docs/assets/demo.svg.'
}
finally { Pop-Location }
