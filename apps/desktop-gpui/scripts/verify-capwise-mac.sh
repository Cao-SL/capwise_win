#!/usr/bin/env bash
set -euo pipefail

app="${1:?Usage: verify-capwise-mac.sh <Capwise.app> [--resources-only]}"
source_wallpapers="$(cd "$(dirname "$0")/../assets/backgrounds" && pwd)"
bundled_wallpapers="$app/Contents/Resources/assets/backgrounds"
if [[ ! -s "$source_wallpapers/macOS/sequoia-dark.jpg" || ! -d "$bundled_wallpapers" ]] ||
   [[ -L "$bundled_wallpapers" || -n "$(find "$bundled_wallpapers" -type l -print -quit)" ]] ||
   ! diff -qr "$source_wallpapers" "$bundled_wallpapers"; then
  echo "Capwise.app is missing or has mismatched built-in wallpapers." >&2
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
