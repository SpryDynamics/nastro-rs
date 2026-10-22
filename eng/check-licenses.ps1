#!/usr/bin/env pwsh
<#
.SYNOPSIS
  CI license-compliance gate: fails if any dependency (direct or transitive) carries a
  license outside deny.toml's [licenses] allow-list, or has a known security advisory.
#>
$ErrorActionPreference = "Stop"

if (-not (Get-Command cargo-deny -ErrorAction SilentlyContinue)) {
    Write-Host "Installing cargo-deny..."
    cargo install cargo-deny --locked | Out-Null
}

cargo deny check

if ($LASTEXITCODE -ne 0) {
    Write-Error "cargo-deny check failed - see output above."
    exit $LASTEXITCODE
}

Write-Host "License/advisory/bans/sources checks passed." -ForegroundColor Green
