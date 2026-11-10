<#
.SYNOPSIS
    Authenticode-sign Windows release binaries with a PFX held in CI secrets.

.DESCRIPTION
    Upstream signs via an external SignPath service (scripts/windows-release-signing.ps1),
    which a downstream fork cannot use. This signs in place with signtool and an RFC3161
    timestamp, then verifies every file with Get-AuthenticodeSignature.

    The Windows SDK's signtool is not on PATH on GitHub runners, so the newest
    x64 copy under "C:\Program Files (x86)\Windows Kits\10\bin" is used when
    Get-Command cannot find it. Pass -SigntoolPath to override (also useful for tests).

.EXAMPLE
    ./sign-windows-release.ps1 -Paths ./Capwise.exe,./cap-cli.exe `
        -PfxBase64 $env:WINDOWS_SIGNING_PFX -PfxPassword $env:WINDOWS_SIGNING_PFX_PASSWORD
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string[]] $Paths,
    [Parameter(Mandatory = $true)][string] $PfxBase64,
    [string] $PfxPassword = "",
    [string] $TimestampUrl = "http://timestamp.digicert.com",
    [string] $SigntoolPath = "",
    [switch] $AllowSelfSigned
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-Signtool {
    if (-not [string]::IsNullOrWhiteSpace($SigntoolPath)) {
        return (Resolve-Path -LiteralPath $SigntoolPath).Path
    }
    $onPath = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($null -ne $onPath) {
        return $onPath.Source
    }
    $kits = "C:\Program Files (x86)\Windows Kits\10\bin"
    if (Test-Path -LiteralPath $kits) {
        $candidate = Get-ChildItem -LiteralPath $kits -Directory |
            Sort-Object Name -Descending |
            ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" } |
            Where-Object { Test-Path -LiteralPath $_ } |
            Select-Object -First 1
        if ($null -ne $candidate) {
            return $candidate
        }
    }
    throw "signtool.exe was not found. Install the Windows SDK signing tools or pass -SigntoolPath."
}

$resolvedPaths = foreach ($entry in $Paths) {
    foreach ($path in ($entry -split ',')) {
        $trimmed = $path.Trim()
        if ([string]::IsNullOrWhiteSpace($trimmed)) {
            continue
        }
        $resolved = (Resolve-Path -LiteralPath $trimmed).Path
        if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
            throw "Not a file: $trimmed"
        }
        $resolved
    }
}
if (@($resolvedPaths).Count -eq 0) {
    throw "At least one path is required."
}

$signtool = Resolve-Signtool
Write-Host "Using signtool: $signtool"

$tempRoot = if ([string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) { $env:TEMP } else { $env:RUNNER_TEMP }
$pfxPath = Join-Path $tempRoot "capwise-sign-$([guid]::NewGuid().ToString('N')).pfx"

try {
    [System.IO.File]::WriteAllBytes($pfxPath, [Convert]::FromBase64String($PfxBase64.Trim()))

    foreach ($path in $resolvedPaths) {
        $arguments = @(
            "sign",
            "/fd", "SHA256",
            "/td", "SHA256",
            "/tr", $TimestampUrl,
            "/f", $pfxPath
        )
        if (-not [string]::IsNullOrEmpty($PfxPassword)) {
            $arguments += @("/p", $PfxPassword)
        }
        $arguments += $path

        & $signtool @arguments | Out-Host
        if ($LASTEXITCODE -ne 0) {
            throw "signtool failed for '$path' with exit code $LASTEXITCODE."
        }

        $signature = Get-AuthenticodeSignature -FilePath $path
        $hasSigner = $null -ne $signature.SignerCertificate
        $trusted = $signature.Status -eq "Valid"
        if (-not $hasSigner -or (-not $trusted -and -not $AllowSelfSigned)) {
            throw "Authenticode verification failed for '$path': $($signature.Status)."
        }
        if (-not $trusted) {
            Write-Host "Signed (self-signed, not trusted): $path ($($signature.SignerCertificate.Subject))"
        } else {
            Write-Host "Signed and verified: $path ($($signature.SignerCertificate.Subject))"
        }
    }
}
finally {
    if (Test-Path -LiteralPath $pfxPath) {
        Remove-Item -LiteralPath $pfxPath -Force
    }
}
