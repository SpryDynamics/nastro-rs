#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Builds the Windows installer (installer/nastro-rs.iss) from already-built release binaries.

.DESCRIPTION
  Build first:   cargo build --release -p nastro -p nastro-tap
  (sign the .exe files here, as eng/package.ps1's callers do)
  Then:          ./eng/build-installer.ps1 [-Sign]

  Produces, in -OutDir:
    nastro-rs-<version>-windows-x86_64-setup.exe
    <same name>.sha256

  With -Sign the installer and its uninstaller are signed through eng/sign-file.ps1 (needs the same `az login`
  context as signing the binaries). Requires Inno Setup 6 (https://jrsoftware.org/isinfo.php): found on PATH, or in
  its default per-user / per-machine locations.
#>
[CmdletBinding()]
param(
    [string]$BinDir = (Join-Path $PSScriptRoot '..\target\release'),
    [string]$OutDir = (Join-Path $PSScriptRoot '..\dist'),
    [string]$Version,
    [switch]$Sign
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

if (-not $Version) {
    $manifest = Get-Content (Join-Path $root 'Cargo.toml') -Raw
    if ($manifest -notmatch '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { throw 'Could not read the version from Cargo.toml' }
    $Version = $Matches[1]
}
foreach ($b in 'nastro.exe', 'nastro-tap.exe') {
    if (-not (Test-Path (Join-Path $BinDir $b))) { throw "missing $b in $BinDir (build first)" }
}

$iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    $iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) { throw 'Inno Setup 6 (ISCC.exe) not found. Install it: winget install JRSoftware.InnoSetup' }

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$setup = Join-Path $OutDir "nastro-rs-$Version-windows-x86_64-setup.exe"
if (Test-Path $setup) { Remove-Item $setup -Force }

$args_ = @("/DAppVersion=$Version", "/DBinDir=$((Resolve-Path $BinDir).Path)", "/DOutputDir=$OutDir")
if ($Sign) {
    $signScript = Join-Path $PSScriptRoot 'sign-file.ps1'
    # $q is a quote and $f the file, in Inno Setup's command-line syntax.
    $args_ += '/DSign=1'
    $args_ += "/Sazsign=pwsh.exe -NoProfile -File `$q$signScript`$q `$f"
}
$args_ += (Join-Path $root 'installer\nastro-rs.iss')

& $iscc @args_
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
if (-not (Test-Path $setup)) { throw "expected $setup was not produced" }

$hash = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLowerInvariant()
Set-Content "$setup.sha256" "$hash  $(Split-Path $setup -Leaf)" -NoNewline
Write-Host "Built $(Split-Path $setup -Leaf)" -ForegroundColor Green
Write-Host "  sha256 $hash"
Write-Host ("  size   {0:N0} bytes" -f (Get-Item $setup).Length)
