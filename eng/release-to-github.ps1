#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Publishes already-built, already-verified release archives as a GitHub Release.

.DESCRIPTION
  Safety order, and why:
    1. Re-verify the archives against SHA256SUMS locally (refuse to upload anything
       that doesn't match the checksums the pipeline produced).
    2. Push the (signed, annotated) tag to GitHub if it isn't there yet, and confirm the
       tag object GitHub holds is byte-identical to ours. The mirror pipeline pushes the
       same tag; whichever runs second is a no-op.
    3. Create the release as a DRAFT with the assets. Drafts are invisible to the public.
    4. Download every asset back from GitHub and compare it, byte for byte, with what we
       uploaded, and check the checksums again.
    5. Only then publish. After publishing, fetch the release anonymously (no token) and
       check it is public, not a draft, and that a public download still matches.
  If any step fails the release stays a draft (or is never created) and the script exits
  non-zero. It never overwrites an existing release.

  -DryRun runs the same steps against a throwaway tag and then deletes both the draft and
  the tag, so the whole path can be exercised against real GitHub without publishing.

  Credentials: a GitHub App installation token is minted from a private key held in Azure
  Key Vault (needs `az login` / an Azure CLI task). The key lives only in this process.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$AssetsDir,
    [string]$Version,
    [string]$Tag,
    [string]$Repo = "SpryDynamics/nastro-rs",
    [string]$KeyVaultName,
    [string]$SecretName = "github-app-private-key",
    [Parameter(Mandatory)] [string]$ClientId,
    [Parameter(Mandatory)] [string]$InstallationId,
    [switch]$DryRun,
    # Test-only: with -DryRun, also run the publish + anonymous-verification step on the
    # throwaway tag (it is public for a few seconds, then deleted with its tag).
    [switch]$PublishDuringDryRun
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

function Invoke-Native {
    # Runs a native command, throws on a non-zero exit, returns its output lines. Uses
    # $args (no param block) so flags like -o / -D are passed to the program untouched
    # instead of being parsed as PowerShell parameters.
    $cmd = @($args | ForEach-Object { [string]$_ })
    $out = & $cmd[0] $cmd[1..($cmd.Count - 1)] 2>&1
    if ($LASTEXITCODE -ne 0) {
        # Never echo the command line: it can carry an Authorization header.
        throw "'$($cmd[0]) $($cmd[1])' failed with exit code $LASTEXITCODE`n$($out -join "`n")"
    }
    $out
}
function Get-Sha256([string]$path) { (Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant() }

function Assert-ChecksumsMatch([string]$dir, [string]$label) {
    $sums = Join-Path $dir "SHA256SUMS"
    if (-not (Test-Path $sums)) { throw "$label`: SHA256SUMS missing in $dir" }
    $checked = 0
    foreach ($line in Get-Content $sums | Where-Object { $_.Trim() }) {
        if ($line -notmatch '^([0-9a-f]{64})\s+\*?(.+)$') { throw "$label`: unparseable SHA256SUMS line: $line" }
        $file = Join-Path $dir $Matches[2]
        if (-not (Test-Path $file)) { throw "$label`: listed file missing: $($Matches[2])" }
        if ((Get-Sha256 $file) -ne $Matches[1]) { throw "$label`: CHECKSUM MISMATCH for $($Matches[2])" }
        $checked++
    }
    if ($checked -eq 0) { throw "$label`: SHA256SUMS lists no files" }
    Write-Host "  $label`: $checked file(s) match SHA256SUMS"
}

# ---------------------------------------------------------------- inputs
if (-not $Version) {
    if ((Get-Content (Join-Path $root "Cargo.toml") -Raw) -notmatch '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { throw "Cannot read version from Cargo.toml" }
    $Version = $Matches[1]
}
if (-not $Tag) { $Tag = if ($DryRun) { "v0.0.0-pipeline-test-" + [guid]::NewGuid().ToString("N").Substring(0, 8) } else { "v$Version" } }
if (-not $DryRun -and $Tag -ne "v$Version") { throw "Tag '$Tag' does not match version '$Version'." }
$AssetsDir = (Resolve-Path $AssetsDir).Path

$archives = Get-ChildItem $AssetsDir -File | Where-Object { $_.Name -match '(\.zip|\.tar\.gz|-setup\.exe)$' }
if (-not ($archives | Where-Object Name -like '*windows*')) { throw "No Windows archive in $AssetsDir" }
if (-not ($archives | Where-Object Name -like '*linux*')) { throw "No Linux archive in $AssetsDir" }
if ($archives | Where-Object { $_.Name -notlike "nastro-rs-$Version-*" }) { throw "Archive name does not carry version ${Version}: $($archives.Name -join ', ')" }
$assetFiles = @($archives.FullName) + (Join-Path $AssetsDir "SHA256SUMS")

Write-Host "Release $Tag ($Version) -> $Repo$(if ($DryRun) { '  [DRY RUN]' })"
Write-Host "1. Verifying local archives"
Assert-ChecksumsMatch $AssetsDir "local"

$notesFile = Join-Path ([IO.Path]::GetTempPath()) "release-notes-$([guid]::NewGuid().ToString('N')).md"
$notesArgs = @{ Version = $Version }
if ($DryRun) { $notesArgs.AllowUnreleased = $true }
& (Join-Path $PSScriptRoot "release-notes.ps1") @notesArgs | Set-Content $notesFile -Encoding utf8
Write-Host "   release notes: $((Get-Content $notesFile).Count) line(s)"

# ---------------------------------------------------------------- credentials
if (-not $KeyVaultName) { throw "-KeyVaultName is required" }
$env:GH_APP_PRIVATE_KEY = (Invoke-Native az keyvault secret show --vault-name $KeyVaultName --name $SecretName --query value -o tsv) -join "`n"
try {
    $token = (& (Join-Path $PSScriptRoot "github-app-token.ps1") -ClientId $ClientId -InstallationId $InstallationId).Trim()
}
finally { Remove-Item Env:GH_APP_PRIVATE_KEY -ErrorAction SilentlyContinue }
if ($env:TF_BUILD) { Write-Host "##vso[task.setvariable variable=GH_APP_TOKEN;issecret=true]$token" }
$env:GH_TOKEN = $token
$tmp = Join-Path ([IO.Path]::GetTempPath()) "gh-release-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory $tmp | Out-Null
$releaseCreated = $false
$tagPushedByUs = $false

try {
    # ------------------------------------------------------------ tag
    Write-Host "2. Ensuring tag $Tag exists on GitHub"
    $basic = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes("x-access-token:$token"))
    $gitAuth = @("-c", "http.extraHeader=Authorization: Basic $basic")

    $existing = & gh api "repos/$Repo/git/ref/tags/$Tag" --jq .object.sha 2>$null
    $haveRemote = ($LASTEXITCODE -eq 0)
    if ($DryRun) {
        if ($haveRemote) { throw "Throwaway tag $Tag already exists on GitHub; refusing." }
        $head = (Invoke-Native git -C $root rev-parse HEAD).Trim()
        # tag.gpgsign=true (as in this repo) would turn this into a signed annotated tag and
        # open an editor for the message; a throwaway tag should be a plain lightweight one.
        $env:GIT_EDITOR = "true"
        Invoke-Native git -C $root -c tag.gpgsign=false tag $Tag $head | Out-Null
    }
    $localSha = (Invoke-Native git -C $root rev-parse "refs/tags/$Tag").Trim()
    if (-not $haveRemote) {
        Invoke-Native git -C $root @gitAuth push "https://github.com/$Repo.git" "refs/tags/${Tag}:refs/tags/${Tag}" | Out-Null
        $tagPushedByUs = $true
    }
    $remoteSha = (Invoke-Native gh api "repos/$Repo/git/ref/tags/$Tag" --jq .object.sha).Trim()
    if ($remoteSha -ne $localSha) { throw "Tag $Tag on GitHub ($remoteSha) differs from the local tag ($localSha). Refusing to release." }
    Write-Host "   tag object $localSha matches on both sides"

    # ------------------------------------------------------------ draft
    Write-Host "3. Creating DRAFT release"
    & gh release view $Tag -R $Repo 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "A release for $Tag already exists; refusing to overwrite it." }
    $prerelease = ($Version -match '^0\.' -or $Version -match '-')
    $create = @("release", "create", $Tag, "-R", $Repo, "--draft", "--verify-tag", "--title", "nastro-rs $Version", "--notes-file", $notesFile)
    if ($prerelease) { $create += "--prerelease" }
    Invoke-Native gh @create @assetFiles | Out-Null
    $releaseCreated = $true
    Write-Host "   draft created$(if ($prerelease) { ' (pre-release)' })"

    # ------------------------------------------------------------ verify
    Write-Host "4. Downloading the assets back from GitHub and comparing"
    $dl = Join-Path $tmp "downloaded"
    Invoke-Native gh release download $Tag -R $Repo -D $dl | Out-Null
    $want = ($assetFiles | ForEach-Object { Split-Path $_ -Leaf } | Sort-Object) -join ","
    $got = (Get-ChildItem $dl -File | ForEach-Object Name | Sort-Object) -join ","
    if ($want -ne $got) { throw "Asset set differs. uploaded=[$want] on GitHub=[$got]" }
    foreach ($f in $assetFiles) {
        $name = Split-Path $f -Leaf
        if ((Get-Sha256 $f) -ne (Get-Sha256 (Join-Path $dl $name))) { throw "Asset $name on GitHub is not byte-identical to the one uploaded" }
    }
    Write-Host "   $($assetFiles.Count) asset(s) byte-identical"
    Assert-ChecksumsMatch $dl "downloaded"

    if ($DryRun -and -not $PublishDuringDryRun) {
        Write-Host "5. DRY RUN: not publishing"
    }
    else {
        Write-Host "5. Publishing"
        Invoke-Native gh release edit $Tag -R $Repo --draft=false | Out-Null
        # Anonymous check: what the public sees, with no credentials at all.
        $pub = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/tags/$Tag" -Headers @{ Accept = "application/vnd.github+json" }
        if ($pub.draft) { throw "Release is still a draft after publishing" }
        if ($pub.prerelease -ne $prerelease) { throw "Pre-release flag is wrong on the published release" }
        $zip = $pub.assets | Where-Object name -like "*.zip" | Select-Object -First 1
        $anon = Join-Path $tmp "anon-$($zip.name)"
        Invoke-WebRequest $zip.browser_download_url -OutFile $anon
        if ((Get-Sha256 $anon) -ne (Get-Sha256 (Join-Path $AssetsDir $zip.name))) { throw "Anonymous download of $($zip.name) does not match" }
        Write-Host "   public, not a draft, anonymous download verified: $($pub.html_url)"
    }
}
finally {
    if ($DryRun) {
        Write-Host "Cleaning up throwaway release and tag"
        if ($releaseCreated) { & gh release delete $Tag -R $Repo --yes --cleanup-tag 2>&1 | Out-Null }
        & gh api -X DELETE "repos/$Repo/git/refs/tags/$Tag" 2>&1 | Out-Null
        & git -C $root tag -d $Tag 2>&1 | Out-Null
        & gh api "repos/$Repo/git/ref/tags/$Tag" 2>&1 | Out-Null
        $stillThere = ($LASTEXITCODE -eq 0)
        # This lookup is EXPECTED to fail (404 = the tag is gone). Reset the exit code so that success
        # is not reported as a failure by hosts, such as AzureCLI@2, that exit with the last native code.
        $global:LASTEXITCODE = 0
        if ($stillThere) { Write-Warning "Throwaway tag $Tag is still on GitHub - delete it by hand." } else { Write-Host "   confirmed gone from GitHub" }
    }
    elseif ($releaseCreated -and -not $?) {
        Write-Warning "The draft release for $Tag was left in place for inspection."
    }
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item $notesFile -Force -ErrorAction SilentlyContinue
    Remove-Item Env:GH_TOKEN -ErrorAction SilentlyContinue
}
