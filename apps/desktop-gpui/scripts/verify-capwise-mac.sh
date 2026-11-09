#!/usr/bin/env bash
set -euo pipefail

app="${1:?Usage: verify-capwise-mac.sh <Capwise.app> [--resources-only]}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
source "$HERE/scripts/macos-bundle-assets.sh"
resources="$app/Contents/Resources"
for tree in "${CAPWISE_ASSET_TREES[@]}"; do
  source_tree="$HERE/assets/$tree"
  bundled_tree="$resources/assets/$tree"
  if [[ ! -d "$source_tree" || ! -d "$bundled_tree" || -L "$bundled_tree" ]] ||
     [[ -z "$(find "$source_tree" -type f -print -quit)" ]] ||
     [[ -n "$(find "$bundled_tree" -type l -print -quit)" ]] ||
     ! diff -qr "$source_tree" "$bundled_tree"; then
    echo "Capwise.app has missing or mismatched assets: $tree" >&2
    exit 1
  fi
done
for required in   Info.plist MacOS/Capwise MacOS/cap-cli Resources/Assets.car Resources/AppIcon.icns   Helpers/llama-server/llama-server Resources/licenses/llama.cpp-LICENSE.txt   Resources/onnxruntime/lib/libonnxruntime.dylib; do
  if [[ ! -s "$app/Contents/$required" ]]; then
    echo "Capwise.app is missing $required" >&2
    exit 1
  fi
done
for library in avcodec avdevice avfilter avformat avutil postproc swresample swscale; do
  path="$app/Contents/Frameworks/Spacedrive.framework/Libraries/lib$library.dylib"
  if [[ ! -s "$path" ]]; then
    echo "Capwise.app is missing $path" >&2
    exit 1
  fi
done
if ! (cd "$resources/fonts" && shasum -a 256 -c "$HERE/assets/caption-fonts/SHA256SUMS" >/dev/null); then
  echo "Capwise.app has missing or mismatched caption fonts or font licenses." >&2
  exit 1
fi
if [[ "${2:-}" == "--resources-only" ]]; then
  exit 0
fi

binary="$app/Contents/MacOS/Capwise"
test "$(lipo -archs "$binary")" = "$(uname -m)"
codesign --verify --deep --strict "$app"
"$app/Contents/MacOS/cap-cli" --version

profile="$(mktemp -d)"
pid=""
cleanup() {
  if [[ -n "$pid" ]]; then
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf -- "$profile"
}
trap cleanup EXIT
export CAP_GPUI_APP_DATA_DIR="$profile"
export CAP_GPUI_LOG_DIR="$profile/logs"
export CAP_GPUI_RECORDINGS_DIR="$profile/recordings"
export RUST_LOG=info
mkdir -p "$CAP_GPUI_RECORDINGS_DIR"
printf '%s\n' '{"general_settings":{"hasCompletedStartup":true,"hasCompletedOnboarding":true,"enableTelemetry":false}}' > "$profile/store"
"$binary" > "$profile/launch.log" 2>&1 &
pid=$!
for attempt in {1..30}; do
  if ! kill -0 "$pid" 2>/dev/null; then break; fi
  if grep -q 'main window opened' "$profile/launch.log"; then
    echo 'Capwise opened its main window'
    exit 0
  fi
  sleep 1
done
cat "$profile/launch.log"
exit 1
