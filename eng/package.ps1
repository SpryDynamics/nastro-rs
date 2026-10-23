#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Packages already-built release binaries into a distributable archive.

.DESCRIPTION
  Build first:   cargo build --release -p nastro -p nastro-tap
  (Windows)      sign the .exe files here, between build and package
  Then:          ./eng/package.ps1

  Produces, in -OutDir:
    nastro-<version>-<os>-<arch>.zip     (Windows)  or  .tar.gz  (Linux)
    <same name>.sha256                   (checksum, `sha256sum -c` compatible)

  Every archive carries the license, NOTICE and the generated third-party license
  texts alongside the binaries, so redistribution obligations travel with the files.
  Fails loudly if any of those, or either binary, is missing.
#>
[CmdletBinding()]
param(
    [string]$BinDir = (Join-Path $PSScriptRoot "..\target\release"),
    [string]$OutDir = (Join-Path $PSScriptRoot "..\dist"),
    [string]$Version
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if (-not $Version) {
    $manifest = Get-Content (Join-Path $root "Cargo.toml") -Raw
    if ($manifest -notmatch '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
        throw "Could not read [workspace.package] version from Cargo.toml"
    }
    $Version = $Matches[1]
}

$onWindows = $IsWindows -or ($env:OS -eq "Windows_NT")
$osName = if ($onWindows) { "windows" } else { "linux" }
$arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
    "X64"   { "x86_64" }
    "Arm64" { "aarch64" }
    default { throw "Unsupported architecture: $_" }
}
$exe = if ($onWindows) { ".exe" } else { "" }

$binaries = "nastro$exe", "nastro-tap$exe"
$docs = "LICENSE", "NOTICE.md", "THIRD_PARTY_NOTICES.md", "README.md", "CHANGELOG.md"

$missing = @()
foreach ($b in $binaries) { if (-not (Test-Path (Join-Path $BinDir $b))) { $missing += (Join-Path $BinDir $b) } }
foreach ($d in $docs)     { if (-not (Test-Path (Join-Path $root $d)))   { $missing += (Join-Path $root $d) } }
if ($missing) { throw "Cannot package, missing:`n  " + ($missing -join "`n  ") }

$name = "nastro-$Version-$osName-$arch"
$stage = Join-Path $OutDir $name
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

foreach ($b in $binaries) { Copy-Item (Join-Path $BinDir $b) $stage }
foreach ($d in $docs)     { Copy-Item (Join-Path $root $d) $stage }

# A short, accurate run-time requirements note that ships with the binaries.
$requirements = @"
nastro $Version ($osName $arch)

Run-time requirements
  * ffmpeg on PATH (or set NASTRO_FFMPEG to its full path). nastro runs it as a
    separate program to encode AAC/.m4a. It is NOT included in this archive.
"@
if (-not $onWindows) {
    $requirements += @"

  * PulseAudio client library (libpulse0) - present on virtually every desktop
    distribution, including PipeWire systems via pipewire-pulse. Dynamically linked.
  * 'pactl' (pulseaudio-utils) to locate the default output for system-audio capture.
  * 'xdg-open' for "reveal in file manager".
"@
}
$requirements += @"

Keep nastro and nastro-tap in the same folder: nastro finds its capture helper
next to itself first, then on PATH.

Licenses: see LICENSE, NOTICE.md and THIRD_PARTY_NOTICES.md in this folder.
"@
Set-Content (Join-Path $stage "REQUIREMENTS.txt") $requirements

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$archive = if ($onWindows) { Join-Path $OutDir "$name.zip" } else { Join-Path $OutDir "$name.tar.gz" }
if (Test-Path $archive) { Remove-Item $archive -Force }

if ($onWindows) {
    Compress-Archive -Path $stage -DestinationPath $archive -CompressionLevel Optimal
} else {
    # Single top-level directory, so extracting never scatters files into the cwd.
    & tar -C $OutDir -czf $archive $name
    if ($LASTEXITCODE -ne 0) { throw "tar failed with exit code $LASTEXITCODE" }
}

$hash = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
# Two spaces + bare filename: the format `sha256sum -c` expects.
Set-Content "$archive.sha256" "$hash  $(Split-Path $archive -Leaf)" -NoNewline

Remove-Item $stage -Recurse -Force

Write-Host "Packaged $(Split-Path $archive -Leaf)" -ForegroundColor Green
Write-Host "  sha256 $hash"
Write-Host ("  size   {0:N0} bytes" -f (Get-Item $archive).Length)
