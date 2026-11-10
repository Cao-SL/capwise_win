<#
.SYNOPSIS
    Assemble a runnable Capwise Windows payload from local release artifacts.

.DESCRIPTION
    This is the manual equivalent of what `bun run cap-setup` + `tauri bundle`
    produce, for machines where the Tauri CLI / bun toolchain is unavailable.
    It stages Capwise.exe, the two sidecars, the runtime DLLs and the bundle
    resources declared in tauri.conf.json into one directory, then writes a
    BUILD-INFO.json with the version, source commit and a SHA256 manifest.

    The result is a runnable directory. A NSIS/MSIX installer still needs
    makensis / makeappx plus a signing certificate; use
    build-capwise-msix.ps1 on the staged directory for the MSIX route.

.EXAMPLE
    ./scripts/assemble-windows-payload.ps1 -OutDir ./dist/Capwise-0.3.10
#>
[CmdletBinding()]
param(
    [string] $OutDir = "",
    [string] $Version = "",
    [switch] $SkipResources
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Desktop = Join-Path $RepoRoot "apps\desktop-gpui"
$NativeDeps = Join-Path $RepoRoot "target\native-deps"
$FfmpegBin = Join-Path $RepoRoot "target\ffmpeg-7.1-full_build-shared\bin"
$ReleaseDir = Join-Path $Desktop "target\release"
$RootReleaseDir = Join-Path $RepoRoot "target\release"

if ([string]::IsNullOrWhiteSpace($Version)) {
    $match = Select-String -LiteralPath (Join-Path $Desktop "Cargo.toml") -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if ($null -eq $match) { throw "Could not read the version from apps/desktop-gpui/Cargo.toml." }
    $Version = $match.Matches[0].Groups[1].Value
}
if ([string]::IsNullOrWhiteSpace($OutDir)) { $OutDir = Join-Path $RepoRoot "dist\Capwise-$Version" }
$OutDir = [System.IO.Path]::GetFullPath(
    $(if ([System.IO.Path]::IsPathRooted($OutDir)) { $OutDir }
      else { Join-Path (Get-Location).Path $OutDir })
)

function Require([string] $path, [string] $what) {
    if (-not (Test-Path -LiteralPath $path)) { throw "Missing $what : $path" }
}

$mainExe = Join-Path $ReleaseDir "cap-gpui.exe"
foreach ($fallback in @(
    (Join-Path $ReleaseDir "Cap.exe"),
    (Join-Path $ReleaseDir "deps\cap_gpui.exe"),
    (Join-Path $ReleaseDir "deps\cap-gpui.exe")
)) {
    if (-not (Test-Path -LiteralPath $mainExe -PathType Leaf) -and
        (Test-Path -LiteralPath $fallback -PathType Leaf)) { $mainExe = $fallback }
}
Require $mainExe "release GPUI binary (run cargo build --release first)"
Require (Join-Path $RootReleaseDir "cap-muxer.exe") "cap-muxer.exe sidecar"
Require (Join-Path $RootReleaseDir "cap.exe") "cap CLI sidecar"
Require $FfmpegBin "ffmpeg DLL directory"

if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
Write-Host "== Binaries =="
Copy-Item -LiteralPath $mainExe -Destination (Join-Path $OutDir "Capwise.exe") -Force
Copy-Item -LiteralPath (Join-Path $RootReleaseDir "cap-muxer.exe") -Destination (Join-Path $OutDir "cap-muxer.exe") -Force
Copy-Item -LiteralPath (Join-Path $RootReleaseDir "cap.exe") -Destination (Join-Path $OutDir "cap-cli.exe") -Force
Write-Host "  Capwise.exe, cap-muxer.exe, cap-cli.exe"

Write-Host "== Runtime DLLs =="
$dllCount = 0
foreach ($dir in @($FfmpegBin, (Join-Path $NativeDeps "dxc"), (Join-Path $NativeDeps "onnxruntime\lib"))) {
    if (-not (Test-Path -LiteralPath $dir -PathType Container)) {
        Write-Host "  SKIP (absent): $dir"
        continue
    }
    Get-ChildItem -LiteralPath $dir -Filter '*.dll' | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $OutDir $_.Name) -Force
        $script:dllCount++
    }
}
Write-Host "  $dllCount DLL(s)"
if ($dllCount -lt 8) { throw "Expected at least the 8 ffmpeg DLLs; got $dllCount." }

if (-not $SkipResources) {
    Write-Host "== Bundle resources (tauri.conf.json mapping) =="
    $fontsDst = Join-Path $OutDir "fonts"
    New-Item -ItemType Directory -Path $fontsDst -Force | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $Desktop "assets\caption-fonts") -File |
        Where-Object { $_.Extension -in '.ttf', '.otf', '.txt' } |
        ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $fontsDst -Force }

    foreach ($set in @('macOS', 'blue', 'cities', 'dark', 'orange', 'purple')) {
        $src = Join-Path $Desktop "assets\backgrounds\$set"
        if (-not (Test-Path -LiteralPath $src -PathType Container)) { continue }
        $dst = Join-Path $OutDir "assets\backgrounds\$set"
        New-Item -ItemType Directory -Path $dst -Force | Out-Null
        Get-ChildItem -LiteralPath $src -File | ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination $dst -Force
        }
    }
    $musicSrc = Join-Path $Desktop "assets\music"
    if (Test-Path -LiteralPath $musicSrc -PathType Container) {
        $musicDst = Join-Path $OutDir "assets\music"
        New-Item -ItemType Directory -Path $musicDst -Force | Out-Null
        Get-ChildItem -LiteralPath $musicSrc -Filter '*.mp3' -File | ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination $musicDst -Force
        }
    }
    $iconsDst = Join-Path $OutDir "icons"
    New-Item -ItemType Directory -Path $iconsDst -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $Desktop "icons\icon.ico") -Destination $iconsDst -Force
    Write-Host "  fonts, backgrounds, music, icons"
}

Write-Host "== Manifest =="
$commit = (git -C $RepoRoot rev-parse HEAD).Trim()
$files = Get-ChildItem -LiteralPath $OutDir -Recurse -File | Sort-Object FullName
$hashes = @($files | ForEach-Object {
    [ordered]@{
        path   = $_.FullName.Substring($OutDir.Length + 1).Replace('\', '/')
        bytes  = $_.Length
        sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
})
$info = [ordered]@{
    product = "Capwise"
    version = $Version
    commit  = $commit
    built_at = (Get-Date).ToUniversalTime().ToString('o')
    platform = "windows-x86_64"
    file_count = $hashes.Count
    total_bytes = ($files | Measure-Object Length -Sum).Sum
    files = $hashes
}
$json = $info | ConvertTo-Json -Depth 6
[System.IO.File]::WriteAllText(
    (Join-Path $OutDir "BUILD-INFO.json"),
    $json,
    [System.Text.UTF8Encoding]::new($false)
)
Write-Host ("  {0} files, {1:N1} MB" -f $hashes.Count, ($info.total_bytes / 1MB))
Write-Host "`nDone: $OutDir"
