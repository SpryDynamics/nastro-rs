#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Tests the installer's optional-tools steps (FFmpeg, speaker labels, model, Hugging Face token) without a network
  and without touching the real machine: a stand-in `winget` and `whisperx` are used, the config goes to a temporary
  file (NASTRO_CONFIG), and PATH is cut down to the system folders plus the stand-ins.

.EXAMPLE
  ./eng/test-installer-deps.ps1 -Setup dist\nastro-rs-0.1.2-windows-x86_64-setup.exe `
      -FakeWhisperx target\release\nastro-fake-whisperx.exe
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Setup,
    [Parameter(Mandatory)][string]$FakeWhisperx
)
$ErrorActionPreference = 'Stop'
$Setup = (Resolve-Path $Setup).Path
$FakeWhisperx = (Resolve-Path $FakeWhisperx).Path
$work = Join-Path ([IO.Path]::GetTempPath()) ("nastro-deps-" + [guid]::NewGuid().ToString('N'))
$fakebin = Join-Path $work 'bin'
New-Item -ItemType Directory $fakebin -Force | Out-Null
$wingetLog = Join-Path $work 'winget.log'
$token = 'hf_TestTokenOnlyForTheInstallerTests1234'
$failures = 0

# The stand-in winget: records its arguments, succeeds (or fails with FAKE_WINGET_EXIT).
Set-Content (Join-Path $fakebin 'winget.cmd') @'
@echo off
if "%1"=="--version" exit /b 0
echo %*>>"%FAKE_WINGET_LOG%"
exit /b %FAKE_WINGET_EXIT%
'@

function Invoke-Setup([string]$Name, [string[]]$SetupArgs, [hashtable]$Env = @{}, [switch]$NoWinget) {
    $dir = Join-Path $work "$Name\app"
    $log = Join-Path $work "$Name.log"
    $cfg = Join-Path $work "$Name\config.toml"
    Remove-Item $wingetLog -ErrorAction SilentlyContinue
    $sys = "$env:SystemRoot\System32;$env:SystemRoot"
    $saved = @{}
    $vars = @{ PATH = $(if ($NoWinget) { $sys } else { "$fakebin;$sys" }); NASTRO_CONFIG = $cfg; NASTRO_WHISPERX = $FakeWhisperx
               FAKE_WINGET_LOG = $wingetLog; FAKE_WINGET_EXIT = '0'; USERPROFILE = (Join-Path $work "$Name\home") }
    foreach ($k in $Env.Keys) { $vars[$k] = $Env[$k] }
    foreach ($k in $vars.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $vars[$k]) }
    try {
        $a = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$dir`"", "/LOG=`"$log`"") + $SetupArgs
        $p = Start-Process $Setup -ArgumentList $a -Wait -PassThru
    }
    finally { foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) } }
    [pscustomobject]@{ Exit = $p.ExitCode; Dir = $dir; Log = $log; Config = $cfg
                       Winget = $(if (Test-Path $wingetLog) { Get-Content $wingetLog -Raw } else { '' }) }
}

function Check([bool]$Condition, [string]$What) {
    if ($Condition) { Write-Host "  ok   $What" } else { Write-Host "  FAIL $What" -ForegroundColor Red; $script:failures++ }
}
function Remove-Install($r) {
    $u = Join-Path $r.Dir 'unins000.exe'
    if (Test-Path $u) { Start-Process $u -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait; Start-Sleep 1 }
}
function Config($r) { if (Test-Path $r.Config) { Get-Content $r.Config -Raw } else { '' } }

try {
    Write-Host 'A plain silent install touches neither the network nor the settings'
    $r = Invoke-Setup 'plain' @()
    Check ($r.Exit -eq 0) 'installer exit code 0'
    Check (Test-Path "$($r.Dir)\nastro.exe") 'nastro installed'
    Check ($r.Winget -eq '') 'winget was not called'
    Check (-not (Test-Path $r.Config)) 'no settings written'
    Remove-Install $r

    Write-Host 'Speaker labels: model, uv through winget, token from a file'
    $tokenFile = Join-Path $work 'token.txt'
    Set-Content $tokenFile $token
    $r = Invoke-Setup 'wx' @('/TASKS="whisperx"', '/MODEL=small', "/HFTOKENFILE=`"$tokenFile`"")
    Check ($r.Exit -eq 0) 'installer exit code 0'
    Check ($r.Winget -match 'install --id astral-sh\.uv -e --silent') 'uv installed through winget'
    Check ((Config $r) -match 'whisper_model = "small"') 'chosen model saved'
    Check ((Config $r) -match [regex]::Escape("hf_token = `"$token`"")) 'token saved in the settings'
    Check ((Test-Path $tokenFile)) 'the user''s own token file is left alone'
    Check (-not ((Get-Content $r.Log -Raw) -match [regex]::Escape($token))) 'token is not in the setup log'
    Check (-not ($r.Winget -match [regex]::Escape($token))) 'token never went to winget'
    Remove-Install $r

    Write-Host 'A model name that does not exist is ignored'
    $r = Invoke-Setup 'badmodel' @('/TASKS="whisperx"', '/MODEL=gigantic')
    Check ($r.Exit -eq 0) 'installer exit code 0'
    Check (-not ((Config $r) -match 'whisper_model')) 'settings unchanged'
    Remove-Install $r

    Write-Host 'winget failing does not fail the installation'
    $r = Invoke-Setup 'wingetfail' @('/TASKS="whisperx"', '/MODEL=base') @{ FAKE_WINGET_EXIT = '1' }
    Check ($r.Exit -eq 0) 'installer exit code 0'
    Check (Test-Path "$($r.Dir)\nastro.exe") 'nastro installed anyway'
    Check ((Config $r) -match 'whisper_model = "base"') 'model still saved'
    Check (-not ((Config $r) -match 'hf_token')) 'no token stored when speaker labels were not installed'
    Remove-Install $r

    Write-Host 'No winget at all does not fail the installation either'
    $r = Invoke-Setup 'nowinget' @('/TASKS="whisperx"') -NoWinget
    Check ($r.Exit -eq 0) 'installer exit code 0'
    Check (Test-Path "$($r.Dir)\nastro.exe") 'nastro installed anyway'
    Remove-Install $r

    # FFmpeg is only offered when it is missing, so this needs a machine without it (a CI agent).
    $ffmpegHere = [bool](Get-Command ffmpeg.exe -ErrorAction SilentlyContinue) -or (Test-Path "$env:LOCALAPPDATA\Microsoft\WinGet\Links\ffmpeg.exe") -or $env:NASTRO_FFMPEG
    if ($ffmpegHere) { Write-Host 'FFmpeg: skipped (FFmpeg is already installed on this machine, so the installer does not offer it)' }
    else {
        Write-Host 'FFmpeg through winget'
        $r = Invoke-Setup 'ffmpeg' @('/TASKS="ffmpeg"')
        Check ($r.Exit -eq 0) 'installer exit code 0'
        Check ($r.Winget -match 'install --id Gyan\.FFmpeg -e --silent') 'FFmpeg installed through winget'
        Remove-Install $r
    }
}
finally { Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue }

if ($failures) { throw "$failures installer check(s) failed" }
Write-Host 'installer optional tools: all checks passed' -ForegroundColor Green
