param(
	[Parameter(Mandatory = $true)][string] $Installer,
	[Parameter(Mandatory = $true)][string] $Version,
	[switch] $RequireAuthenticode
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$Installer = (Resolve-Path -LiteralPath $Installer).Path
$Desktop = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Root = (Resolve-Path (Join-Path $Desktop "../..")).Path
$Test = Join-Path $env:RUNNER_TEMP ("capwise-" + [Guid]::NewGuid().ToString("N"))
$Destination = Join-Path $Test "app"
$Profile = Join-Path $Test "profile"
$env:CAP_GPUI_APP_DATA_DIR = $Profile
$env:CAP_GPUI_TAURI_STORE = Join-Path $Profile "store"
$env:CAP_GPUI_LOG_DIR = Join-Path $Profile "logs"
$env:CAP_GPUI_RECORDINGS_DIR = Join-Path $Profile "recordings"
$env:RUST_LOG = "info"
$null = New-Item -ItemType Directory -Path $env:CAP_GPUI_RECORDINGS_DIR -Force
'{"general_settings":{"hasCompletedStartup":true,"hasCompletedOnboarding":true,"enableTelemetry":false,"updateDownloadPolicy":"never"},"recording_start_safety":{"confirmBeforeRecordingWithoutMicrophone":false}}' |
	Set-Content -LiteralPath $env:CAP_GPUI_TAURI_STORE -Encoding utf8NoBOM

$Install = Start-Process -FilePath $Installer -ArgumentList "/S", "/D=$Destination" -Wait -PassThru
if ($Install.ExitCode -ne 0) { throw "Installer failed: $($Install.ExitCode)" }

$Required = @(
	"Capwise.exe", "cap-cli.exe", "cap-muxer.exe",
	"avcodec-61.dll", "avdevice-61.dll", "avfilter-10.dll", "avformat-61.dll",
	"avutil-59.dll", "postproc-58.dll", "swresample-5.dll", "swscale-8.dll",
	"dxcompiler.dll", "dxil.dll", "onnxruntime.dll", "onnxruntime_providers_shared.dll"
)
foreach ($Name in $Required) {
	$File = Get-Item -LiteralPath (Join-Path $Destination $Name)
	if ($File.Length -eq 0) { throw "Bundled file is empty: $Name" }
}
if ($RequireAuthenticode) {
	$Signed = @($Installer, (Join-Path $Destination "Capwise.exe"), (Join-Path $Destination "cap-cli.exe"), (Join-Path $Destination "cap-muxer.exe"))
	foreach ($Path in $Signed) {
		$Signature = Get-AuthenticodeSignature -FilePath $Path
		if ($Signature.Status -ne "Valid" -or $null -eq $Signature.SignerCertificate) {
			throw "Missing or invalid Authenticode signature: $Path ($($Signature.Status))"
		}
		Write-Output "Verified Authenticode signature on $([IO.Path]::GetFileName($Path))"
	}
}
foreach ($Name in @("Cap.exe", "cap-gpui.exe", "cap-exporter.exe")) {
	if (Test-Path -LiteralPath (Join-Path $Destination $Name)) {
		throw "Installer unexpectedly contains $Name"
	}
}
$AssetTrees = @(
	@{ Source = "assets/backgrounds"; Destination = "assets/backgrounds"; Extensions = @(".jpg", ".jpeg", ".png", ".webp") },
	@{ Source = "assets/music"; Destination = "assets/music"; Extensions = @(".mp3") },
	@{ Source = "assets/caption-fonts"; Destination = "fonts"; Extensions = @(".ttf", ".otf", ".txt") }
)
foreach ($Tree in $AssetTrees) {
	$Source = Join-Path $Desktop $Tree.Source
	$Files = @(Get-ChildItem -LiteralPath $Source -Recurse -File | Where-Object { $_.Extension -in $Tree.Extensions })
	if ($Files.Count -eq 0) { throw "No source assets in $Source" }
	foreach ($File in $Files) {
		$Relative = [IO.Path]::GetRelativePath($Source, $File.FullName)
		$Bundled = Join-Path (Join-Path $Destination $Tree.Destination) $Relative
		if ((Get-FileHash -LiteralPath $File.FullName).Hash -ne (Get-FileHash -LiteralPath $Bundled).Hash) {
			throw "Bundled asset mismatch: $Bundled"
		}
	}
	Write-Output "Verified $($Files.Count) bundled files in $($Tree.Destination)"
}
$CliVersion = & (Join-Path $Destination "cap-cli.exe") --version
if ($LASTEXITCODE -ne 0 -or $CliVersion -ne "cap $Version") {
	throw "Unexpected bundled CLI version: $CliVersion"
}

$env:CAP_GPUI_AUTO_RECORD = "studio:5"
$env:CAP_GPUI_AUTO_NO_MIC = "1"
$env:CAP_GPUI_AUTO_EXPORT = Join-Path $Test "capture.mp4"
$App = Start-Process -FilePath (Join-Path $Destination "Capwise.exe") -PassThru
try {
	$Deadline = [DateTime]::UtcNow.AddMinutes(3)
	do {
		if ($App.HasExited) { throw "Capwise exited: $($App.ExitCode)" }
		$LogFiles = @(Get-ChildItem -LiteralPath $env:CAP_GPUI_LOG_DIR -File -ErrorAction SilentlyContinue)
		$Log = ($LogFiles | Get-Content -ErrorAction SilentlyContinue) -join "`n"
		if ($Log -match "editor export completed" -and (Test-Path -LiteralPath $env:CAP_GPUI_AUTO_EXPORT)) { break }
		Start-Sleep -Seconds 2
	} while ([DateTime]::UtcNow -lt $Deadline)
	if ($Log -notmatch "main window opened" -or $Log -notmatch "editor export completed") {
		throw "Capwise did not open, record, and export the test clip."
	}
	$FFprobe = Join-Path $Root "target/ffmpeg/bin/ffprobe.exe"
	$Tracks = & $FFprobe -v error -select_streams v:0 -show_entries stream=codec_type -of csv=p=0 $env:CAP_GPUI_AUTO_EXPORT
	if ($LASTEXITCODE -ne 0 -or $Tracks -notcontains "video") { throw "Export has no decodable video stream." }
	$FFmpeg = Join-Path $Root "target/ffmpeg/bin/ffmpeg.exe"
	& $FFmpeg -v error -xerror -i $env:CAP_GPUI_AUTO_EXPORT -map 0:v:0 -f null -
	if ($LASTEXITCODE -ne 0) { throw "Export failed full video decode." }
	Write-Output "Installed Capwise $Version, opened the window, recorded and exported a video."
} finally {
	if (!$App.HasExited) { Stop-Process -Id $App.Id }
	Get-ChildItem -LiteralPath $env:CAP_GPUI_LOG_DIR -File -Filter "cap-gpui.log.*" -ErrorAction SilentlyContinue |
		Get-Content -Tail 80 -ErrorAction SilentlyContinue
}
