#!/usr/bin/env bash
# Build Capwise, assemble a signed .app and replace /Applications/Capwise.app.
# Signing with Developer ID on every build keeps macOS permission grants
# (Screen Recording, Accessibility, Camera, Microphone) across rebuilds.
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
PROFILE="${PROFILE:-debug}"
IDENTITY="${CAPWISE_SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application:[^"]*\)".*/\1/p' | head -1)}"
STAGE="$HERE/target/Capwise.app"
DEST="/Applications/Capwise.app"
ENTITLEMENTS="$HERE/packaging/macos/Entitlements.plist"

if [[ -z "$IDENTITY" ]]; then
  echo "No Developer ID Application identity found; set CAPWISE_SIGN_IDENTITY." >&2
  exit 1
fi

cargo_flags=()
[[ "$PROFILE" == "release" ]] && cargo_flags+=(--release)
(cd "$HERE" && cargo build ${cargo_flags[@]+"${cargo_flags[@]}"})
(cd "$ROOT" && cargo build -p cap ${cargo_flags[@]+"${cargo_flags[@]}"})

"$HERE/scripts/fetch-caption-fonts.sh" >/dev/null
"$HERE/scripts/fetch-llama-server.sh" >/dev/null

rm -rf "$STAGE"
mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources/onnxruntime/lib" "$STAGE/Contents/Frameworks"
cp "$HERE/packaging/macos/Info.plist" "$STAGE/Contents/Info.plist"
xcrun actool "$HERE/packaging/macos/AppIcon.icon" --compile "$STAGE/Contents/Resources" \
  --platform macosx --minimum-deployment-target 13.0 --app-icon AppIcon --target-device mac \
  --output-partial-info-plist "$HERE/target/icon-partial.plist" >/dev/null
mkdir -p "$STAGE/Contents/Helpers/llama-server" "$STAGE/Contents/Resources/licenses"
cp -a "$HERE/target/llama-server/llama-server" "$HERE"/target/llama-server/*.dylib "$STAGE/Contents/Helpers/llama-server/"
cp "$HERE/target/llama-server/LICENSE" "$STAGE/Contents/Resources/licenses/llama.cpp-LICENSE.txt"
mkdir -p "$STAGE/Contents/Resources/fonts"
cp "$HERE"/target/caption-fonts/*.{otf,ttf,txt} "$STAGE/Contents/Resources/fonts/"
cp "$HERE/target/$PROFILE/cap-gpui" "$STAGE/Contents/MacOS/Capwise"
cp "$ROOT/target/$PROFILE/cap" "$STAGE/Contents/MacOS/cap-cli"
cp "$(ls -d "$ROOT"/target/onnxruntime-osx-*/lib/libonnxruntime.dylib | head -1)" \
  "$STAGE/Contents/Resources/onnxruntime/lib/libonnxruntime.dylib"
# ditto keeps the framework's internal Versions/Current symlinks intact.
ditto "$ROOT/target/native-deps/Spacedrive.framework" "$STAGE/Contents/Frameworks/Spacedrive.framework"
rm -rf "$STAGE/Contents/Frameworks/Spacedrive.framework/Versions/A/Headers" \
  "$STAGE/Contents/Frameworks/Spacedrive.framework/Headers"

sign() { codesign --force --timestamp=none --options runtime --sign "$IDENTITY" "$@"; }

# Inside-out: libraries, helper binary, then the app itself.
while IFS= read -r -d '' dylib; do
  sign "$dylib"
done < <(find "$STAGE/Contents/Frameworks" "$STAGE/Contents/Resources" "$STAGE/Contents/Helpers" -type f -name "*.dylib" -print0)
sign "$STAGE/Contents/Helpers/llama-server/llama-server"
sign "$STAGE/Contents/Frameworks/Spacedrive.framework"
sign --entitlements "$ENTITLEMENTS" "$STAGE/Contents/MacOS/cap-cli"
sign --entitlements "$ENTITLEMENTS" --identifier com.capwise.app "$STAGE"
codesign --verify --strict "$STAGE"

pkill -x Capwise 2>/dev/null && sleep 1 || true
rm -rf "$DEST"
ditto "$STAGE" "$DEST"
echo "Installed $DEST ($PROFILE, signed by $IDENTITY)"
