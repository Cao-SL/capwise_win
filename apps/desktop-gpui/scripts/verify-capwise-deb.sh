#!/usr/bin/env bash
set -euo pipefail

package="${1:?Usage: verify-capwise-deb.sh <Capwise.deb>}"
test "$(dpkg-deb -f "$package" Package)" = capwise
test "$(dpkg-deb -f "$package" Architecture)" = amd64
sudo apt-get install -y "$(realpath "$package")" mesa-vulkan-drivers xvfb xauth
desktop=/usr/share/applications/Capwise.desktop
grep -qx 'Name=Capwise' "$desktop"
libraries="$(ldd /opt/capwise/bin/Cap)"
if grep -q 'not found' <<< "$libraries"; then
  printf '%s\n' "$libraries" >&2
  exit 1
fi
profile="$(mktemp -d)"
trap 'rm -rf -- "$profile"' EXIT
export CAP_GPUI_APP_DATA_DIR="$profile"
export CAP_GPUI_LOG_DIR="$profile/logs"
export CAP_GPUI_RECORDINGS_DIR="$profile/recordings"
mkdir -p "$CAP_GPUI_RECORDINGS_DIR"
printf '%s\n' '{"general_settings":{"hasCompletedStartup":true,"hasCompletedOnboarding":true,"enableTelemetry":false}}' > "$profile/store"
result=0
timeout 25s xvfb-run -a dbus-run-session -- /usr/bin/capwise > "$profile/launch.log" 2>&1 || result="$?"
cat "$profile/launch.log"
test "$result" -eq 124
grep -R -q 'main window opened' "$CAP_GPUI_LOG_DIR"
