<#
.SYNOPSIS
    Build and stage one runnable Capwise Windows version end to end.

.DESCRIPTION
    This is the local equivalent of the CI "Package Windows candidate" step
    for machines without bun / the Tauri CLI / VS2022. It:
      1. waits for (or reuses) `cargo build --release --bin cap-gpui`
      2. assembles the payload via assemble-windows-payload.ps1
      3. optionally Authenticode-signs the three payload exes
      4. signs the update with the fork's minisign key and emits latest.json,
         SHA256SUMS.txt, capwise-release.json and the winget manifests
      5. optionally builds an MSIX from the staged payload

    A full NSIS installer is NOT produced here: that needs makensis / the
    Tauri CLI. The staged directory is a runnable app and a valid MSIX input.

.EXAMPLE
    ./scripts/build-windows-version.ps1 -MinisignSecretKey J:/Projects/ScreenVideo/capwise-win-update.key
    ./scripts/build-windows-version.ps1 -SkipBuild -PfxBase64 $env:WINDOWS_SIGNING_PFX -BuildMsix
#>
[CmdletBinding()]
param(
    [string] $Version = "",
    [string] $OutDir = "",
    [switch] $SkipBuild,
    [switch] $BuildMsix,
    [switch] $SkipResources,
    [string] $PfxBase64 = "",
    [string] $PfxPassword = "",
    [string] $MinisignSecretKey = "",
    [string] $PythonPath = "",
    [string] $InstallerUrl = "",
    [string] $Publisher = "CN=Capwise"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Desktop = Join-Path $RepoRoot "apps\desktop-gpui"

if ([string]::IsNullOrWhiteSpace($Version)) {
    $match = Select-String -LiteralPath (Join-Path $Desktop "Cargo.toml") -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if ($null -eq $match) { throw "Could not read the version from apps/desktop-gpui/Cargo.toml." }
    $Version = $match.Matches[0].Groups[1].Value
}
if ([string]::IsNullOrWhiteSpace($OutDir)) { $OutDir = Join-Path $RepoRoot "dist\Capwise-$Version" }
$dist = Join-Path $RepoRoot "dist\$Version"

Write-Host "Capwise Windows $Version"
Write-Host "payload : $OutDir"
Write-Host "dist    : $dist"

$env:RUSTUP_TOOLCHAIN = '1.95.0'
$exe = Join-Path $Desktop "target\release\cap-gpui.exe"
if (-not $SkipBuild) {
    Write-Host "`n== cargo build --release --bin cap-gpui =="
    $buildLog = Join-Path $env:TEMP "capwise-version-build-$Version.log"
    & "$env:USERPROFILE\.cargo\bin\cargo.exe" build --offline --release --bin cap-gpui `
        --manifest-path (Join-Path $Desktop "Cargo.toml") 2>&1 |
        Tee-Object -FilePath $buildLog | Select-Object -Last 6
    if ($LASTEXITCODE -ne 0) { throw "release build failed (see $buildLog)" }
}
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    $alt = Join-Path $Desktop "target\release\Cap.exe"
    if (Test-Path -LiteralPath $alt -PathType Leaf) { $exe = $alt }
    else { throw "No release binary at $exe (run without -SkipBuild)" }
}

Write-Host "`n== Stage payload =="
& (Join-Path $PSScriptRoot "assemble-windows-payload.ps1") `
    -OutDir $OutDir -Version $Version -SkipResources:$SkipResources

if (-not [string]::IsNullOrWhiteSpace($PfxBase64)) {
    Write-Host "`n== Authenticode sign payload exes =="
    $paths = @(
        (Join-Path $OutDir "Capwise.exe"),
        (Join-Path $OutDir "cap-cli.exe"),
        (Join-Path $OutDir "cap-muxer.exe")
    ) -join ','
    & (Join-Path $PSScriptRoot "sign-windows-release.ps1") `
        -Paths $paths -PfxBase64 $PfxBase64 -PfxPassword $PfxPassword
}

if (-not [string]::IsNullOrWhiteSpace($MinisignSecretKey)) {
    Write-Host "`n== Publish artefacts (latest.json / winget / release config) =="
    New-Item -ItemType Directory -Path $dist -Force | Out-Null
    $nsisInstaller = Join-Path $dist "Capwise-Windows-x64-Setup.exe"
    if (-not (Test-Path -LiteralPath $nsisInstaller -PathType Leaf)) {
        Write-Host "  SKIPPED: NSIS installer not present at $nsisInstaller"
        Write-Host "  The updater signs and hashes that installer, so publish-windows-release.ps1"
        Write-Host "  needs it. Produce it with CI (tauri bundle) or makensis, put it in $dist,"
        Write-Host "  then run: ./scripts/publish-windows-release.ps1 -Installer `"$nsisInstaller`" \"
        Write-Host "       -InstallerUrl `"https://github.com/Cao-SL/capwise_win/releases/download/v$Version/Capwise-Windows-x64-Setup.exe`" \"
        Write-Host "       -Version $Version -MinisignSecretKey <key>"
    } else {
        if ([string]::IsNullOrWhiteSpace($InstallerUrl)) {
            $InstallerUrl = "https://github.com/Cao-SL/capwise_win/releases/download/v$Version/Capwise-Windows-x64-Setup.exe"
        }
        $pubArgs = @{
            Installer         = $nsisInstaller
            InstallerUrl      = $InstallerUrl
            Version           = $Version
            OutDir            = $dist
            MinisignSecretKey = $MinisignSecretKey
            PfxBase64         = $PfxBase64
            PfxPassword       = $PfxPassword
        }
        if (-not [string]::IsNullOrWhiteSpace($PythonPath)) { $pubArgs.PythonPath = $PythonPath }
        & (Join-Path $PSScriptRoot "publish-windows-release.ps1") @pubArgs
    }
}

if ($BuildMsix) {
    Write-Host "`n== MSIX =="
    $msixArgs = @{
        Payload   = $OutDir
        Version   = $Version
        OutDir    = (Join-Path $dist "msix")
        Publisher = $Publisher
    }
    if (-not [string]::IsNullOrWhiteSpace($PfxBase64)) {
        $msixArgs.PfxBase64 = $PfxBase64
        $msixArgs.PfxPassword = $PfxPassword
    }
    & (Join-Path $Desktop "scripts\build-capwise-msix.ps1") @msixArgs
}

Write-Host "`nDone."
Write-Host "  runnable payload : $OutDir"
Write-Host "  release artefacts: $dist"
Write-Host "  BUILD-INFO.json  : $(Join-Path $OutDir 'BUILD-INFO.json')"
