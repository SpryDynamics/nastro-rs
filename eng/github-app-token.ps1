#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Mints a short-lived (1 hour) GitHub App installation token and prints it to stdout.

.DESCRIPTION
  The private key is read from the environment variable named by -KeyEnvVar (default
  GH_APP_PRIVATE_KEY), never from a command-line argument, so it cannot show up in a
  process listing or a logged command line. The token carries only the permissions the
  app was installed with, on only the repositories it was installed on.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$ClientId,
    [Parameter(Mandatory)] [string]$InstallationId,
    [string]$KeyEnvVar = "GH_APP_PRIVATE_KEY",
    [string]$ApiBase = "https://api.github.com"
)
$ErrorActionPreference = "Stop"

$pem = [Environment]::GetEnvironmentVariable($KeyEnvVar)
if ([string]::IsNullOrWhiteSpace($pem)) { throw "Environment variable $KeyEnvVar is empty." }

function ConvertTo-Base64Url([byte[]]$bytes) {
    [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
}

$now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
$header = ConvertTo-Base64Url ([Text.Encoding]::UTF8.GetBytes('{"alg":"RS256","typ":"JWT"}'))
# iat backdated 60s for clock skew; GitHub caps exp at 10 minutes.
$payload = ConvertTo-Base64Url ([Text.Encoding]::UTF8.GetBytes(
        (@{ iat = $now - 60; exp = $now + 540; iss = $ClientId } | ConvertTo-Json -Compress)))

$rsa = [Security.Cryptography.RSA]::Create()
try {
    $rsa.ImportFromPem($pem)
    $signature = ConvertTo-Base64Url ($rsa.SignData(
            [Text.Encoding]::UTF8.GetBytes("$header.$payload"),
            [Security.Cryptography.HashAlgorithmName]::SHA256,
            [Security.Cryptography.RSASignaturePadding]::Pkcs1))
}
finally { $rsa.Dispose() }

$response = Invoke-RestMethod -Method Post -Uri "$ApiBase/app/installations/$InstallationId/access_tokens" -Headers @{
    Authorization = "Bearer $header.$payload.$signature"
    Accept        = "application/vnd.github+json"
}
$response.token
