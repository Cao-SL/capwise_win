<#
.SYNOPSIS
    Report the Windows GPU capabilities the remaining zero-copy work depends on.

.DESCRIPTION
    Read-only probe. It never creates a D3D device or changes settings; it
    collects adapter descriptions, driver versions, the ffmpeg AV1 encoder
    candidates that are actually available, and the presence of the shared
    fence / D3D11-on-12 prerequisites. The JSON output is meant to be attached
    to an issue before enabling any of the paths in
    apps/desktop-gpui/docs/windows-gpu-roadmap.md.

.EXAMPLE
    ./scripts/probe-windows-gpu.ps1 | ConvertFrom-Json
    ./scripts/probe-windows-gpu.ps1 -OutFile ./gpu-capabilities.json
#>
[CmdletBinding()]
param(
    [string] $OutFile = "",
    [string] $FfmpegPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if ([string]::IsNullOrWhiteSpace($FfmpegPath)) {
    $candidate = Join-Path $RepoRoot "target\ffmpeg-7.1-full_build-shared\bin\ffmpeg.exe"
    if (Test-Path -LiteralPath $candidate -PathType Leaf) { $FfmpegPath = $candidate }
    if ([string]::IsNullOrWhiteSpace($FfmpegPath)) {
        $onPath = Get-Command ffmpeg -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($onPath) { $FfmpegPath = $onPath.Source }
    }
}

$adapters = @()
try {
    $adapters = @(Get-CimInstance Win32_VideoController -ErrorAction Stop | ForEach-Object {
        $ramMb = $null
        if ($_.AdapterRAM) { $ramMb = [math]::Round($_.AdapterRAM / 1MB) }
        [ordered]@{
            name            = $_.Name
            driver_version  = $_.DriverVersion
            video_processor = $_.VideoProcessor
            adapter_ram_mb  = $ramMb
            pnp_device_id   = $_.PNPDeviceID
        }
    })
} catch {
    $adapters = @([ordered]@{ error = $_.Exception.Message })
}

$av1FfmpegPath = $null
if ($FfmpegPath) { $av1FfmpegPath = $FfmpegPath }
$av1 = [ordered]@{
    ffmpeg_path = $av1FfmpegPath
    encoders    = @()
    available   = $false
}
if ($FfmpegPath -and (Test-Path -LiteralPath $FfmpegPath -PathType Leaf)) {
    $lines = & $FfmpegPath -hide_banner -encoders 2>$null
    $av1["encoders"] = @($lines | Select-String -Pattern 'av1|AV1' | ForEach-Object { $_.Line.Trim() })
    $av1["available"] = @($av1["encoders"] | Where-Object {
        $_ -match '\b(av1_nvenc|av1_qsv|av1_amf|libaom-av1|libsvtav1)\b'
    }).Count -gt 0
}

$build = $null
$displayBuild = $null
try {
    $currentVersion = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
    $build = $currentVersion.CurrentBuild
    $displayBuild = $currentVersion.DisplayVersion
} catch {
    $build = $null
    $displayBuild = $null
}
$windows = [ordered]@{
    version       = [Environment]::OSVersion.Version.ToString()
    build         = $build
    display_build = $displayBuild
}

$d3d = [ordered]@{
    d3d12_dll = Test-Path (Join-Path $env:WINDIR 'System32\d3d12.dll')
    dxgi_dll  = Test-Path (Join-Path $env:WINDIR 'System32\dxgi.dll')
    d3d11_dll = Test-Path (Join-Path $env:WINDIR 'System32\d3d11.dll')
}

$roadmap = [ordered]@{
    stage2_shared_fence = "requires D3D12 shared heap + D3D11 OpenSharedResource; see windows-gpu-roadmap.md project 2"
    hw_decode_texture   = "requires MFDecodedFrame.textures to reach the renderer before the sample is recycled; see project 1"
    av1_hardware_encode = "requires an AV1 encoder candidate above AND a codec abstraction in crates/enc-ffmpeg; see project 4"
    directml_co_device  = "requires an ort injection point; DirectML EP detection with CPU fallback is already implemented"
}

$report = [ordered]@{
    generated_at = (Get-Date).ToUniversalTime().ToString('o')
    windows      = $windows
    adapters     = $adapters
    d3d_runtime  = $d3d
    av1_encode   = $av1
    roadmap      = $roadmap
}
$json = $report | ConvertTo-Json -Depth 6

if (-not [string]::IsNullOrWhiteSpace($OutFile)) {
    $resolvedOut = if ([System.IO.Path]::IsPathRooted($OutFile)) {
        $OutFile
    } else {
        Join-Path (Get-Location).Path $OutFile
    }
    [System.IO.File]::WriteAllText(
        $resolvedOut,
        $json,
        [System.Text.UTF8Encoding]::new($false)
    )
    Write-Host "wrote $resolvedOut"
}
$json
