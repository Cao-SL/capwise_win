<#
.SYNOPSIS
    Build the latest.json the native updater reads, for a Windows NSIS release.

.DESCRIPTION
    Mirrors the Windows arm of installer.rs::parse_release: the manifest is
    {"version":..., "notes":..., "platforms":{"windows-x86_64":{"url":...,"signature":...}}}.
    The signature is the base64 content of the .minisig file produced by
    minisign -S; the app verifies it against packaging/updater.pub while the
    installer is streaming to disk. Nothing here signs anything -- run minisign
    first and pass the resulting values in.

.EXAMPLE
    ./build-windows-update-manifest.ps1 `
        -Version 0.3.9 `
        -Installer ./dist/0.3.9/Capwise-Windows-x64-Setup.exe `
        -Url https://github.com/Cao-SL/capwise_win/releases/download/v0.3.9/Capwise-Windows-x64-Setup.exe `
        -SignatureFile ./dist/0.3.9/Capwise-Windows-x64-Setup.exe.minisig `
        -OutDir ./dist/0.3.9
#>
param(
    [Parameter(Mandatory = $true)][string] $Version,
    [Parameter(Mandatory = $true)][string] $Installer,
    [Parameter(Mandatory = $true)][string] $Url,
    [Parameter(Mandatory = $true)][string] $SignatureFile,
    [Parameter(Mandatory = $true)][string] $OutDir,
    [string] $Notes = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Installer = (Resolve-Path -LiteralPath $Installer).Path
$SignatureFile = (Resolve-Path -LiteralPath $SignatureFile).Path

$parsedVersion = $Version.Trim().TrimStart('v')
if ($parsedVersion -notmatch '^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$') {
    throw "Version '$Version' is not a semver the updater can parse."
}

$parsedUrl = [System.Uri]::new($Url)
if ($parsedUrl.Scheme -ne "https") {
    throw "installer.rs rejects non-HTTPS update URLs: $Url"
}

$signature = (Get-Content -LiteralPath $SignatureFile -Raw).Trim()
if ([string]::IsNullOrWhiteSpace($signature)) {
    throw "The minisig file is empty; installer.rs rejects an empty signature."
}
try {
    $null = [Convert]::FromBase64String($signature)
} catch {
    throw "The minisig content is not base64: $($_.Exception.Message)"
}

$hash = (Get-FileHash -LiteralPath $Installer -Algorithm SHA256).Hash.ToLowerInvariant()
$installerName = Split-Path -Leaf $Installer

$manifest = [ordered]@{
    version   = $parsedVersion
    notes     = $Notes
    platforms = [ordered]@{
        'windows-x86_64' = [ordered]@{
            url       = $Url
            signature = $signature
        }
    }
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$jsonPath = Join-Path $OutDir 'latest.json'
# Windows PowerShell 5.1 has no utf8NoBOM encoding value; write UTF-8 without
# a BOM explicitly so the updater's serde_json::from_slice sees plain JSON.
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($jsonPath, ($manifest | ConvertTo-Json -Depth 6), $utf8NoBom)
$sumsPath = Join-Path $OutDir 'SHA256SUMS.txt'
[System.IO.File]::WriteAllText($sumsPath, "$hash  $installerName", $utf8NoBom)

Write-Host "Wrote $jsonPath"
Write-Host "SHA256(${installerName}) = $hash"
Write-Host "Remember to upload the installer, latest.json and SHA256SUMS.txt to the URL above."
