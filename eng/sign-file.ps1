#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Signs one file with Azure Artifact Signing (RFC 3161 timestamped). Called by Inno Setup for the installer and its
  uninstaller (see eng/build-installer.ps1), and usable by hand. Needs an `az login` context that holds the Signer role.
.NOTES
  The `sign` CLI picks what it can sign from the file *extension*, and Inno Setup hands us temporary files such as
  `uninst.e32.tmp` (a PE file with a .tmp name). Those are signed through a copy named .exe, then copied back: the
  signature is embedded in the file, so the content is what matters.
#>
[CmdletBinding()]
param([Parameter(Mandatory)][string]$File)
$ErrorActionPreference = 'Stop'
$tool = Join-Path $env:USERPROFILE '.dotnet\tools\az-artifact-sign.exe'
if (-not (Test-Path $tool)) { $tool = (Get-Command az-artifact-sign -ErrorAction Stop).Source }

$signable = '.exe', '.dll', '.msi', '.sys', '.ps1', '.cab', '.cat', '.vsix'
$target = $File
$temp = $null
if ($signable -notcontains [IO.Path]::GetExtension($File).ToLowerInvariant()) {
    $temp = Join-Path ([IO.Path]::GetTempPath()) ("nastro-sign-" + [guid]::NewGuid().ToString('N') + '.exe')
    Copy-Item -LiteralPath $File $temp
    $target = $temp
}
try {
    Write-Host "signing $File$(if ($temp) { " (via $temp)" })"
    & $tool sign $target --description nastro-rs --description-url https://github.com/SpryDynamics/nastro-rs `
      --credential-type azure-cli --verify
    if ($LASTEXITCODE -ne 0) { throw "az-artifact-sign exited with code $LASTEXITCODE" }
    if ($temp) { Copy-Item -LiteralPath $temp $File -Force }
}
catch { Write-Host "ERROR: $_"; exit 1 }
finally { if ($temp) { Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue } }
