#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Signs one file with Azure Artifact Signing (RFC 3161 timestamped). Called by Inno Setup for the installer and its
  uninstaller (see eng/build-installer.ps1), and usable by hand. Needs an `az login` context that holds the Signer role.
#>
[CmdletBinding()]
param([Parameter(Mandatory)][string]$File)
$ErrorActionPreference = 'Stop'
$tool = Join-Path $env:USERPROFILE '.dotnet\tools\az-artifact-sign.exe'
if (-not (Test-Path $tool)) { $tool = (Get-Command az-artifact-sign -ErrorAction Stop).Source }
& $tool sign $File --description nastro-rs --description-url https://github.com/SpryDynamics/nastro-rs `
  --credential-type azure-cli --verify
if ($LASTEXITCODE -ne 0) { Write-Error "signing $File failed with exit code $LASTEXITCODE"; exit 1 }
