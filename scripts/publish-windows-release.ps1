<#
.SYNOPSIS
    Assemble one Windows release from a built NSIS installer.

.DESCRIPTION
    Ties together the three per-piece tools in this repo:
      1. optionally Authenticode-sign the installer (sign-windows-release.ps1)
      2. emit latest.json + SHA256SUMS.txt (build-windows-update-manifest.ps1)
      3. materialise the winget manifest templates for this version

    It does NOT upload anything and never handles the minisign private key:
    pass the already-produced .minisig file. Every external value is a
    parameter, so the script is safe to run against a local fixture.

.EXAMPLE
    ./publish-windows-release.ps1 `
        -Installer ./candidate/Capwise-Windows-x64-Setup.exe `
        -Version 0.3.9 `
        -InstallerUrl https://github.com/Cao-SL/capwise_win/releases/download/v0.3.9/Capwise-Windows-x64-Setup.exe `
        -SignatureFile ./candidate/Capwise-Windows-x64-Setup.exe.minisig `
        -OutDir ./dist/0.3.9
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Installer,
    [Parameter(Mandatory = $true)][string] $InstallerUrl,
    [string] $SignatureFile = "",
    [string] $Version = "",
    [string] $OutDir = "",
    [string] $Notes = "",
    [string] $PfxBase64 = "",
    [string] $PfxPassword = "",
    [string] $WingetTemplateDir = "",
    [string] $MinisignPublicKey = "",
    [string] $UpdateEndpoint = "",
    [string] $MinisignSecretKey = "",
    [string] $PythonPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Desktop = Join-Path $RepoRoot "apps\desktop-gpui"
$Installer = (Resolve-Path -LiteralPath $Installer).Path
if (-not [string]::IsNullOrWhiteSpace($SignatureFile)) {
    $SignatureFile = (Resolve-Path -LiteralPath $SignatureFile).Path
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    $match = Select-String -LiteralPath (Join-Path $Desktop "Cargo.toml") -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if ($null -eq $match) { throw "Could not read the version from apps/desktop-gpui/Cargo.toml." }
    $Version = $match.Matches[0].Groups[1].Value
}
if ([string]::IsNullOrWhiteSpace($OutDir)) { $OutDir = Join-Path $RepoRoot "dist\$Version" }
if ([string]::IsNullOrWhiteSpace($WingetTemplateDir)) { $WingetTemplateDir = Join-Path $RepoRoot "packaging\winget" }

$parsedUrl = [System.Uri]::new($InstallerUrl)
if ($parsedUrl.Scheme -ne "https") { throw "InstallerUrl must be HTTPS: $InstallerUrl" }
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
if (-not (Test-Path -LiteralPath $WingetTemplateDir -PathType Container)) {
    throw "Winget template directory not found: $WingetTemplateDir"
}

Write-Host "== Release config =="
$releaseTemplate = Join-Path $Desktop "packaging\windows\capwise-release.example.json"
if (-not (Test-Path -LiteralPath $releaseTemplate -PathType Leaf)) {
    throw "Release config template not found: $releaseTemplate"
}
$release = Get-Content -LiteralPath $releaseTemplate -Raw -Encoding UTF8 | ConvertFrom-Json
if (-not [string]::IsNullOrWhiteSpace($MinisignPublicKey)) {
    $release.plugins.updater.pubkey = $MinisignPublicKey
}
if (-not [string]::IsNullOrWhiteSpace($UpdateEndpoint)) {
    $endpointUri = [System.Uri]::new($UpdateEndpoint)
    if ($endpointUri.Scheme -ne "https") { throw "UpdateEndpoint must be HTTPS: $UpdateEndpoint" }
    $release.plugins.updater.endpoints = @($UpdateEndpoint)
}
if ($release.plugins.updater.pubkey -notmatch '^RW[A-Za-z0-9+/]+={0,2}$') {
    throw "The release config has an invalid minisign public key."
}
foreach ($endpoint in $release.plugins.updater.endpoints) {
    if ([System.Uri]::new($endpoint).Scheme -ne "https") { throw "Updater endpoint must be HTTPS: $endpoint" }
    if ($endpoint -match 'example\.com') { throw "Updater endpoint is still an example URL: $endpoint" }
}
$releaseOut = Join-Path $OutDir "capwise-release.json"
[System.IO.File]::WriteAllText(
    $releaseOut,
    ($release | ConvertTo-Json -Depth 12),
    [System.Text.UTF8Encoding]::new($false)
)
Write-Host "  wrote capwise-release.json"

if (-not [string]::IsNullOrWhiteSpace($PfxBase64)) {
    Write-Host "== Authenticode signing =="
    $signArgs = @{
        Paths       = $Installer
        PfxBase64   = $PfxBase64
        PfxPassword = $PfxPassword
    }
    & (Join-Path $PSScriptRoot "sign-windows-release.ps1") @signArgs
}

$sha256 = (Get-FileHash -LiteralPath $Installer -Algorithm SHA256).Hash.ToUpperInvariant()
Write-Host "Installer SHA256: $sha256"

if ([string]::IsNullOrWhiteSpace($SignatureFile)) {
    if ([string]::IsNullOrWhiteSpace($MinisignSecretKey)) {
        throw "Pass -SignatureFile or -MinisignSecretKey so the updater manifest has a signature."
    }
    $secret = (Resolve-Path -LiteralPath $MinisignSecretKey).Path
    if ([string]::IsNullOrWhiteSpace($PythonPath)) {
        $pyCommand = Get-Command python -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($pyCommand) { $PythonPath = $pyCommand.Source }
        if ([string]::IsNullOrWhiteSpace($PythonPath)) {
            $bundled = Join-Path $env:USERPROFILE ".dsh\dsh-runtimes\dsh-primary-runtime\dependencies\python\python.exe"
            if (Test-Path $bundled) { $PythonPath = $bundled }
        }
    }
    if ([string]::IsNullOrWhiteSpace($PythonPath)) { throw "Python with py-minisign is needed to sign the update." }
    Write-Host "== Signing the update with minisign =="
    & $PythonPath (Join-Path $PSScriptRoot "minisign-keytool.py") sign --secret $secret --file $Installer
    $SignatureFile = "$Installer.minisig"
    $signatureCheck = & $PythonPath (Join-Path $PSScriptRoot "minisign-keytool.py") verify `
        --public (Join-Path $Desktop "packaging\updater.pub") --file $Installer 2>&1
    Write-Host "  $signatureCheck"
}
$SignatureFile = (Resolve-Path -LiteralPath $SignatureFile).Path

Write-Host "== Update manifest =="
$manifestArgs = @{
    Version       = $Version
    Installer     = $Installer
    Url           = $InstallerUrl
    SignatureFile = $SignatureFile
    OutDir        = $OutDir
    Notes         = $Notes
}
& (Join-Path $Desktop "scripts\build-windows-update-manifest.ps1") @manifestArgs

Write-Host "== Winget manifests =="
$wingetOut = Join-Path $OutDir "winget"
New-Item -ItemType Directory -Path $wingetOut -Force | Out-Null
foreach ($template in Get-ChildItem -LiteralPath $WingetTemplateDir -Filter '*.yaml') {
    $text = Get-Content -LiteralPath $template.FullName -Raw -Encoding UTF8
    $text = $text -replace '(?m)^PackageVersion:\s*\S+', "PackageVersion: $Version"
    $text = $text.Replace('REPLACE_WITH_HTTPS_INSTALLER_URL', $InstallerUrl)
    $text = $text.Replace('REPLACE_WITH_INSTALLER_SHA256_UPPERCASE_HEX', $sha256)
    [System.IO.File]::WriteAllText(
        (Join-Path $wingetOut $template.Name),
        $text,
        [System.Text.UTF8Encoding]::new($false)
    )
    Write-Host "  wrote $($template.Name)"
}

Write-Host "== Placeholder guard =="
$generated = Get-ChildItem -LiteralPath $OutDir -Recurse -File | Where-Object {
    $_.Extension -in '.json', '.yaml', '.yml', '.txt'
}
$leftovers = @()
foreach ($file in $generated) {
    $text = Get-Content -LiteralPath $file.FullName -Raw -Encoding UTF8
    if ($text -match 'REPLACE_WITH_|updates\.example\.com|example\.com/setup') {
        $leftovers += $file.FullName
    }
}
if ($leftovers.Count -gt 0) {
    throw ("Unresolved template placeholders in:`n  " + ($leftovers -join "`n  "))
}
Write-Host "  clean: $($generated.Count) generated files"

$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
$checklist = @(
    "Capwise Windows release $Version",
    "",
    "Upload to $($parsedUrl.GetLeftPart([System.UriPartial]::Path)) :",
    "  - $([System.IO.Path]::GetFileName($Installer))  (SHA256 $sha256)",
    "  - latest.json",
    "  - SHA256SUMS.txt",
    "",
    "Winget: run 'winget validate --manifest $wingetOut' then place the three",
    "YAMLs under manifests/c/CaoSL/Capwise/$Version/ in winget-pkgs.",
    "",
    "Update endpoint: publish latest.json at the updater URL and keep that",
    "version in sync with the installed build.",
    "Release config: $releaseOut (pass it to tauri bundle with --config).",
    "The updater verifies latest.json against packaging/updater.pub; keep the",
    "minisign secret key outside the repository."
) -join "`n"
[System.IO.File]::WriteAllText((Join-Path $OutDir "PUBLISH-CHECKLIST.txt"), $checklist, $utf8NoBom)

Write-Host "`nDone. Artifacts in $OutDir"
Write-Host "Next: winget validate --manifest $wingetOut"
