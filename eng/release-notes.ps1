#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Extracts one version's section from CHANGELOG.md for use as GitHub release notes.

.DESCRIPTION
  Looks for a heading `## [<Version>]` (optionally followed by ` - date`) and returns
  everything up to the next `## ` heading. A real release must have such a section; if
  it doesn't, this fails rather than publishing empty notes. With -AllowUnreleased
  (used only by the pipeline's dry run) it falls back to the `## [Unreleased]` section.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$Version,
    [string]$ChangelogPath = (Join-Path $PSScriptRoot "..\CHANGELOG.md"),
    [switch]$AllowUnreleased
)
$ErrorActionPreference = "Stop"

function Get-Section([string[]]$lines, [string]$name) {
    $start = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match "^##\s+\[$([regex]::Escape($name))\](\s|$)") { $start = $i + 1; break }
    }
    if ($start -lt 0) { return $null }
    $end = $lines.Count
    for ($j = $start; $j -lt $lines.Count; $j++) {
        if ($lines[$j] -match '^##\s') { $end = $j; break }
    }
    $body = ($lines[$start..($end - 1)] -join "`n").Trim()
    if ($body) { $body } else { $null }
}

$lines = Get-Content $ChangelogPath
$notes = Get-Section $lines $Version
if (-not $notes -and $AllowUnreleased) { $notes = Get-Section $lines "Unreleased" }
if (-not $notes) {
    throw "CHANGELOG.md has no non-empty '## [$Version]' section. Move the Unreleased entries under it before tagging."
}
$notes
