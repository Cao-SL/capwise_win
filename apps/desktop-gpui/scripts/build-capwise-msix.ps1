<#
.SYNOPSIS
    Package an assembled Capwise payload as a signed .msix.

.DESCRIPTION
    MSIX is a second distribution channel next to the NSIS installer. Tauri
    does not emit MSIX, so this script stages the payload, fills in the
    AppxManifest.xml placeholders, generates the required logo assets from
    icons/icon.png, runs makeappx and optionally signs the result.

    The payload is the installed tree: Capwise.exe, cap-cli.exe, cap-muxer.exe,
    the FFmpeg DLLs and any resources the build produced. A candidate NSIS
    install directory works directly.

.EXAMPLE
    ./build-capwise-msix.ps1 -Payload ./candidate/installed -Version 0.3.9 -OutDir ./dist/msix
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Payload,
    [string] $Version = "",
    [string] $OutDir = "",
    [string] $Publisher = "CN=Capwise",
    [string] $PfxBase64 = "",
    [string] $PfxPassword = "",
    [string] $CertificateThumbprint = "",
    [string] $CertificateSubject = "",
    [string] $MakeAppxPath = "",
    [string] $SigntoolPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$Desktop = Join-Path $RepoRoot "apps\desktop-gpui"
$Packaging = Join-Path $Desktop "packaging\windows\msix"
$Payload = (Resolve-Path -LiteralPath $Payload).Path

if ([string]::IsNullOrWhiteSpace($Version)) {
    $match = Select-String -LiteralPath (Join-Path $Desktop "Cargo.toml") -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if ($null -eq $match) { throw "Could not read the version from apps/desktop-gpui/Cargo.toml." }
    $Version = $match.Matches[0].Groups[1].Value
}
$parts = @($Version.Split('.'))
$invalid = @($parts | Where-Object { $_ -notmatch '^\d+$' })
if ($parts.Count -ne 3 -or $invalid.Count -gt 0) {
    throw "Version must be major.minor.patch: $Version"
}
$appxVersion = "$($parts[0]).$($parts[1]).$($parts[2]).0"
if ([string]::IsNullOrWhiteSpace($OutDir)) { $OutDir = Join-Path $RepoRoot "dist\msix\$Version" }
$OutDir = [System.IO.Path]::GetFullPath(
    $(if ([System.IO.Path]::IsPathRooted($OutDir)) { $OutDir }
      else { Join-Path (Get-Location).Path $OutDir })
)

function Resolve-Tool([string] $explicit, [string] $name) {
    if (-not [string]::IsNullOrWhiteSpace($explicit)) {
        if (-not (Test-Path -LiteralPath $explicit -PathType Leaf)) { throw "$name not found: $explicit" }
        return (Resolve-Path -LiteralPath $explicit).Path
    }
    $onPath = Get-Command $name -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($onPath) { return $onPath.Source }
    $sdkRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (Test-Path $sdkRoot) {
        $found = Get-ChildItem $sdkRoot -Directory |
            Sort-Object Name -Descending |
            ForEach-Object { Join-Path $_.FullName "x64\$name" } |
            Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
            Select-Object -First 1
        if ($found) { return $found }
    }
    throw "$name was not found. Pass -$($name -replace '\.exe$','')Path or install the Windows SDK."
}

$makeappx = Resolve-Tool $MakeAppxPath 'makeappx.exe'
Write-Host "makeappx: $makeappx"

$exe = Join-Path $Payload "Capwise.exe"
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Payload has no Capwise.exe: $Payload" }
$manifestTemplate = Join-Path $Packaging "AppxManifest.xml"
if (-not (Test-Path -LiteralPath $manifestTemplate -PathType Leaf)) { throw "Missing $manifestTemplate" }

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$staging = Join-Path $OutDir "staging"
if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
New-Item -ItemType Directory -Path $staging -Force | Out-Null

Write-Host "== Staging payload =="
Get-ChildItem -LiteralPath $Payload -Force | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $staging -Recurse -Force
}

$manifest = (Get-Content -LiteralPath $manifestTemplate -Raw -Encoding UTF8) `
    -replace '(?s)(<Identity\b[^>]*?\bVersion=")[^"]+(")', "`${1}$appxVersion`${2}" `
    -replace '(?s)(<Identity\b[^>]*?\bPublisher=")[^"]+(")', "`${1}$Publisher`${2}"
[System.IO.File]::WriteAllText(
    (Join-Path $staging "AppxManifest.xml"),
    $manifest,
    [System.Text.UTF8Encoding]::new($false)
)

Write-Host "== Generating MSIX logo assets =="
$assets = Join-Path $staging "Assets"
New-Item -ItemType Directory -Path $assets -Force | Out-Null
$pyCommand = Get-Command python -ErrorAction SilentlyContinue | Select-Object -First 1
$py = if ($pyCommand) { $pyCommand.Source } else { "" }
if (-not $py) {
    $pyCommand = Get-Command py -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($pyCommand) { $py = $pyCommand.Source }
}
if (-not $py) {
    $bundled = Join-Path $env:USERPROFILE ".dsh\dsh-runtimes\dsh-primary-runtime\dependencies\python\python.exe"
    if (Test-Path $bundled) { $py = $bundled }
}
if (-not $py) { throw "Python with Pillow is needed to generate the MSIX logos." }
$source = Join-Path $Desktop "icons\icon.png"
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing source icon: $source" }
$posix = $assets.Replace('\', '/')
$code = @"
from PIL import Image
im = Image.open(r'$($source.Replace('\','/'))').convert('RGBA')
for size, name in ((44, 'Square44x44Logo.png'), (150, 'Square150x150Logo.png'), (50, 'StoreLogo.png')):
    im.resize((size, size), Image.LANCZOS).save(r'$posix/' + name)
print('logos written')
"@
$code | & $py - 2>&1 | ForEach-Object { "  $_" }
foreach ($name in @('Square44x44Logo.png', 'Square150x150Logo.png', 'StoreLogo.png')) {
    if (-not (Test-Path -LiteralPath (Join-Path $assets $name) -PathType Leaf)) { throw "Logo generation failed: $name" }
}

$msix = Join-Path $OutDir "Capwise-$Version.msix"
Write-Host "== makeappx pack =="
& $makeappx pack /d $staging /p $msix /o /nv 2>&1 | ForEach-Object { "  $_" }
if ($LASTEXITCODE -ne 0) { throw "makeappx pack failed with exit code $LASTEXITCODE" }
if (-not (Test-Path -LiteralPath $msix -PathType Leaf)) { throw "makeappx did not produce $msix" }

Write-Host "== makeappx unpack (round-trip validation) =="
$roundTrip = Join-Path $OutDir "roundtrip"
if (Test-Path $roundTrip) { Remove-Item -Recurse -Force $roundTrip }
& $makeappx unpack /p $msix /d $roundTrip /o 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "makeappx could not read back the package (exit $LASTEXITCODE)" }
$unpackedManifest = Join-Path $roundTrip "AppxManifest.xml"
if (-not (Test-Path -LiteralPath $unpackedManifest -PathType Leaf)) {
    throw "Unpacked package has no AppxManifest.xml"
}
$unpackedXml = [xml](Get-Content -LiteralPath $unpackedManifest -Raw -Encoding UTF8)
if ($unpackedXml.Package.Identity.Version -ne $appxVersion) {
    throw "Unpacked identity version mismatch: $($unpackedXml.Package.Identity.Version)"
}
Write-Host "  round-trip OK, identity $($unpackedXml.Package.Identity.Version)"
Remove-Item -Recurse -Force $roundTrip

$hasPfx = -not [string]::IsNullOrWhiteSpace($PfxBase64)
$hasThumbprint = -not [string]::IsNullOrWhiteSpace($CertificateThumbprint)
$hasSubject = -not [string]::IsNullOrWhiteSpace($CertificateSubject)
if ($hasPfx -or $hasThumbprint -or $hasSubject) {
    Write-Host "== Authenticode signing =="
    $signArgs = @{ Paths = @($msix) }
    if ($hasPfx) {
        $signArgs.PfxBase64 = $PfxBase64
        $signArgs.PfxPassword = $PfxPassword
    } elseif ($hasThumbprint) {
        $signArgs.CertificateThumbprint = $CertificateThumbprint
    } else {
        $signArgs.CertificateSubject = $CertificateSubject
    }
    if (-not [string]::IsNullOrWhiteSpace($SigntoolPath)) { $signArgs.SigntoolPath = $SigntoolPath }
    & (Join-Path $RepoRoot "scripts\sign-windows-release.ps1") @signArgs
}

Write-Host "`nDone: $msix"
Write-Host "Publisher in the manifest must match the signing certificate subject."
