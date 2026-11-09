#!/usr/bin/env bash
# Build or sign a Capwise release for one Mac architecture:
#   version → release build → Developer ID signing (secure timestamp, hardened
#   runtime) → notarize + staple the app → designed DMG → notarize + staple the
#   DMG → signed update archive → latest.json.
#
#   scripts/release.sh 0.2.0                 full release into dist/0.2.0
#   scripts/release.sh 0.2.0 --dry-run       no notarization, throwaway update key if none is set
#   scripts/release.sh 0.2.0 --skip-build    reuse the last release binaries
#
# Nothing is uploaded. Credentials come from the keychain or the environment:
#   CAPWISE_SIGN_IDENTITY         Developer ID Application identity (default: first in the keychain)
#   CAPWISE_NOTARY_PROFILE        notarytool keychain profile (default: capwise-notary)
#   CAPWISE_UPDATER_KEY           minisign secret key file (default: ~/.config/capwise/updater.key)
#   CAPWISE_UPDATER_KEY_PASSWORD  its password; otherwise read from the keychain item
#                                 "capwise-updater-key", otherwise minisign asks
#   CAPWISE_RELEASE_NOTES         Markdown file whose text goes into latest.json
#   CAPWISE_PREBUILT_APP           Package a previously assembled Capwise.app
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
source "$HERE/scripts/macos-bundle-assets.sh"
REPO="joeseesun/capwise"
APP_NAME="Capwise"
BUNDLE_ID="com.capwise.app"
ENTITLEMENTS="$HERE/packaging/macos/Entitlements.plist"
PUBLIC_KEY="$HERE/packaging/updater.pub"
NOTARY_PROFILE="${CAPWISE_NOTARY_PROFILE:-capwise-notary}"
UPDATER_KEY="${CAPWISE_UPDATER_KEY:-$HOME/.config/capwise/updater.key}"

VERSION=""
DRY_RUN=0
SKIP_BUILD=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    -h | --help)
      sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    -*)
      echo "Unknown option: $arg" >&2
      exit 2
      ;;
    *) VERSION="${arg#v}" ;;
  esac
done

die() {
  echo "error: $*" >&2
  exit 1
}
step() { printf '\n==> %s\n' "$*"; }

[[ "$(uname -s)" == "Darwin" ]] || die "releases are built on macOS"
[[ -n "$VERSION" ]] || die "usage: scripts/release.sh <version> [--dry-run] [--skip-build]"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "version must be semver, got $VERSION"

PREBUILT_APP="${CAPWISE_PREBUILT_APP:-}"
MACHINE="$(uname -m)"
if [[ -n "$PREBUILT_APP" ]]; then
  [[ -d "$PREBUILT_APP" ]] || die "prebuilt app not found: $PREBUILT_APP"
  PREBUILT_APP="$(cd "$PREBUILT_APP" && pwd -P)"
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PREBUILT_APP/Contents/Info.plist")" == "$VERSION" ]] ||
    die "prebuilt app version differs from $VERSION"
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$PREBUILT_APP/Contents/Info.plist")" == "$BUNDLE_ID" ]] ||
    die "prebuilt app bundle identifier differs from $BUNDLE_ID"
  MACHINE="$(lipo -archs "$PREBUILT_APP/Contents/MacOS/$APP_NAME")"
  SKIP_BUILD=1
fi
case "$MACHINE" in
  arm64) ARCH="aarch64" ;;
  x86_64) ARCH="x86_64" ;;
  *) die "unsupported architecture: $MACHINE" ;;
esac
PLATFORM="darwin-$ARCH"
TAG="v$VERSION"
DIST="$HERE/dist/$VERSION"
WORK="$HERE/target/release-work"
STAGE="$WORK/$APP_NAME.app"
DMG="$DIST/${APP_NAME}_${VERSION}_${ARCH}.dmg"
ARCHIVE_NAME="${APP_NAME}_${VERSION}_${ARCH}.app.tar.gz"
ARCHIVE="$DIST/$ARCHIVE_NAME"

need() { command -v "$1" >/dev/null 2>&1 || die "$1 is missing ($2)"; }
need minisign "brew install minisign"
need python3 "install Xcode command line tools"
need cargo "install rustup"

step "Preflight"
IDENTITY="${CAPWISE_SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application:[^"]*\)".*/\1/p' | head -1)}"
if [[ -z "$IDENTITY" ]]; then
  ((DRY_RUN)) || die "no Developer ID Application identity; set CAPWISE_SIGN_IDENTITY"
  IDENTITY="-"
  echo "No Developer ID identity; the dry run signs ad hoc."
fi
echo "Signing identity: $IDENTITY"

if ((DRY_RUN)) && [[ ! -f "$UPDATER_KEY" ]]; then
  UPDATER_KEY="$WORK/dry-run-updater.key"
  PUBLIC_KEY="$WORK/dry-run-updater.pub"
  mkdir -p "$WORK"
  if [[ ! -f "$UPDATER_KEY" ]]; then
    minisign -G -W -f -p "$PUBLIC_KEY" -s "$UPDATER_KEY" >/dev/null
  fi
  CAPWISE_UPDATER_KEY_PASSWORD=""
  echo "Dry run: signing updates with a throwaway key ($UPDATER_KEY)."
  echo "Installed builds will reject them, which is what a dry run should do."
else
  [[ -f "$UPDATER_KEY" ]] || die "no update signing key at $UPDATER_KEY (see docs/release.md)"
  grep -q "public key" "$PUBLIC_KEY" 2>/dev/null ||
    die "packaging/updater.pub is empty; commit the public key before releasing (see docs/release.md)"
fi

if ! ((DRY_RUN)); then
  xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1 ||
    die "notarytool profile \"$NOTARY_PROFILE\" is missing or invalid (see docs/release.md)"
fi

updater_password() {
  if [[ -n "${CAPWISE_UPDATER_KEY_PASSWORD+set}" ]]; then
    printf '%s' "$CAPWISE_UPDATER_KEY_PASSWORD"
  else
    security find-generic-password -s capwise-updater-key -w 2>/dev/null || true
  fi
}

sign_update() {
  local file="$1" password
  password="$(updater_password)"
  rm -f "$file.minisig"
  if [[ -n "$password" ]]; then
    printf '%s\n' "$password" | minisign -S -s "$UPDATER_KEY" -m "$file" -x "$file.minisig" \
      -t "$APP_NAME $VERSION $PLATFORM" >/dev/null
  else
    minisign -S -s "$UPDATER_KEY" -m "$file" -x "$file.minisig" -t "$APP_NAME $VERSION $PLATFORM"
  fi
  minisign -V -q -p "$PUBLIC_KEY" -m "$file" -x "$file.minisig" ||
    die "the update signature does not match $PUBLIC_KEY; the secret key and the committed public key differ"
}

step "Version $VERSION"
CARGO_TOML="$HERE/Cargo.toml"
INFO_PLIST="$HERE/packaging/macos/Info.plist"
current="$(sed -n '/^\[package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$CARGO_TOML" | head -1)"
if [[ "$current" != "$VERSION" ]]; then
  ((SKIP_BUILD)) && die "--skip-build needs Cargo.toml at $VERSION (it is $current)"
  sed -i '' "/^\[package\]/,/^\[/s/^version = \".*\"/version = \"$VERSION\"/" "$CARGO_TOML"
  echo "Cargo.toml: $current → $VERSION"
fi
VERSION="$VERSION" perl -0pi -e \
  's#(<key>CFBundle(?:ShortVersionString|Version)</key>\s*<string>)[^<]*#${1}$ENV{VERSION}#g' "$INFO_PLIST"

if ! ((SKIP_BUILD)); then
  step "Release build"
  (cd "$HERE" && cargo build --release --locked --bin cap-gpui)
  (cd "$ROOT" && cargo build --release --locked -p cap)
fi
step "Assemble $APP_NAME.app"
mkdir -p "$WORK" "$DIST"
[[ -z "$PREBUILT_APP" || "$PREBUILT_APP" != "$(cd "$WORK" && pwd -P)/$APP_NAME.app" ]] ||
  die "prebuilt app cannot be the release staging app"
rm -rf "$STAGE"
rm -f "$DMG" "$ARCHIVE" "$ARCHIVE.minisig"
if [[ -n "$PREBUILT_APP" ]]; then
  ditto "$PREBUILT_APP" "$STAGE"
  while IFS= read -r -d '' candidate; do
    if file -b "$candidate" | grep -q "Mach-O"; then
      lipo "$candidate" -verify_arch "$MACHINE" || die "wrong architecture: $candidate"
    fi
  done < <(find "$STAGE" -type f -print0)
else
  "$HERE/scripts/fetch-caption-fonts.sh" >/dev/null
  "$HERE/scripts/fetch-llama-server.sh" >/dev/null

  mkdir -p "$DIST" "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources/onnxruntime/lib" "$STAGE/Contents/Frameworks"
  cp "$INFO_PLIST" "$STAGE/Contents/Info.plist"
  xcrun actool "$HERE/packaging/macos/AppIcon.icon" --compile "$STAGE/Contents/Resources" \
    --platform macosx --minimum-deployment-target 13.0 --app-icon AppIcon --target-device mac \
    --output-partial-info-plist "$WORK/icon-partial.plist" >/dev/null
  mkdir -p "$STAGE/Contents/Helpers/llama-server" "$STAGE/Contents/Resources/licenses"
  cp -a "$HERE/target/llama-server/llama-server" "$HERE"/target/llama-server/*.dylib "$STAGE/Contents/Helpers/llama-server/"
  cp "$HERE/target/llama-server/LICENSE" "$STAGE/Contents/Resources/licenses/llama.cpp-LICENSE.txt"
  cp "$ROOT/LICENSE" "$STAGE/Contents/Resources/licenses/Capwise-LICENSE.txt"
  cp "$ROOT/licenses/LICENSE-MIT" "$STAGE/Contents/Resources/licenses/Cap-MIT.txt"
  mkdir -p "$STAGE/Contents/Resources/assets" "$STAGE/Contents/Resources/fonts"
  cp "$HERE"/target/caption-fonts/*.{otf,ttf,txt} "$STAGE/Contents/Resources/fonts/"
  cp "$HERE/target/release/cap-gpui" "$STAGE/Contents/MacOS/$APP_NAME"
  cp "$ROOT/target/release/cap" "$STAGE/Contents/MacOS/cap-cli"
  cp "$(ls -d "$ROOT"/target/onnxruntime-osx-*/lib/libonnxruntime.dylib | head -1)" \
    "$STAGE/Contents/Resources/onnxruntime/lib/libonnxruntime.dylib"
  ditto "$ROOT/target/native-deps/Spacedrive.framework" "$STAGE/Contents/Frameworks/Spacedrive.framework"
  rm -rf "$STAGE/Contents/Frameworks/Spacedrive.framework/Versions/A/Headers" \
    "$STAGE/Contents/Frameworks/Spacedrive.framework/Headers"
fi
mkdir -p "$STAGE/Contents/Resources/assets"
copy_capwise_assets "$HERE/assets" "$STAGE/Contents/Resources/assets"
"$HERE/scripts/verify-capwise-mac.sh" "$STAGE" --resources-only

xattr -cr "$STAGE"

step "Sign"
timestamp_flag="--timestamp"
[[ "$IDENTITY" == "-" ]] && timestamp_flag="--timestamp=none"
sign() { codesign --force "$timestamp_flag" --options runtime --sign "$IDENTITY" "$@"; }
while IFS= read -r -d '' dylib; do
  sign "$dylib"
done < <(find "$STAGE/Contents/Frameworks" "$STAGE/Contents/Resources" "$STAGE/Contents/Helpers" -type f -name "*.dylib" -print0)
sign "$STAGE/Contents/Helpers/llama-server/llama-server"
sign "$STAGE/Contents/Frameworks/Spacedrive.framework"
sign --entitlements "$ENTITLEMENTS" "$STAGE/Contents/MacOS/cap-cli"
sign --entitlements "$ENTITLEMENTS" --identifier "$BUNDLE_ID" "$STAGE"
codesign --verify --deep --strict "$STAGE"

unsigned_runtime=()
while IFS= read -r -d '' file; do
  if file -b "$file" | grep -q "Mach-O"; then
    flags="$(codesign -dv "$file" 2>&1 | sed -n 's/.*flags=0x[0-9a-f]*(\([^)]*\)).*/\1/p')"
    [[ "$flags" == *runtime* ]] || unsigned_runtime+=("${file#"$STAGE"/}")
  fi
done < <(find "$STAGE" -type f -perm -u+x -print0)
((${#unsigned_runtime[@]} == 0)) || die "hardened runtime missing on: ${unsigned_runtime[*]}"
echo "Every executable has the hardened runtime."

notarize() {
  local file="$1"
  local log="$WORK/$(basename "$file").notary.log" id
  if ! xcrun notarytool submit "$file" --keychain-profile "$NOTARY_PROFILE" --wait --timeout 2h |
    tee "$log" || ! grep -q "status: Accepted" "$log"; then
    id="$(sed -n 's/^ *id: //p' "$log" | head -1)"
    [[ -n "$id" ]] && xcrun notarytool log "$id" --keychain-profile "$NOTARY_PROFILE" || true
    die "notarization of $(basename "$file") was not accepted"
  fi
}

staple() {
  local attempt
  for attempt in 1 2 3 4 5; do
    xcrun stapler staple "$1" && return 0
    echo "stapler attempt $attempt failed; retrying in 30s"
    sleep 30
  done
  die "could not staple $1"
}

if ((DRY_RUN)); then
  step "Notarize the app (skipped: dry run)"
else
  step "Notarize the app"
  ditto -c -k --keepParent "$STAGE" "$WORK/$APP_NAME-notarize.zip"
  notarize "$WORK/$APP_NAME-notarize.zip"
  rm -f "$WORK/$APP_NAME-notarize.zip"
  staple "$STAGE"
  spctl --assess --type execute -vv "$STAGE"
fi

step "Update archive"
COPYFILE_DISABLE=1 tar -czf "$ARCHIVE" -C "$WORK" "$APP_NAME.app"
sign_update "$ARCHIVE"
echo "$ARCHIVE_NAME signed and verified against $(basename "$PUBLIC_KEY")"

step "DMG"
DMG_TOOLS="$HERE/target/release-tools"
if [[ ! -x "$DMG_TOOLS/bin/dmgbuild" ]]; then
  python3 -m venv "$DMG_TOOLS"
  "$DMG_TOOLS/bin/python3" -m pip install --quiet "dmgbuild==1.6.5"
fi
"$HERE/scripts/release-dmg-background.swift" "$WORK/dmg-background.png" "$WORK/dmg-background@2x.png"
tiffutil -cathidpicheck "$WORK/dmg-background.png" "$WORK/dmg-background@2x.png" \
  -out "$WORK/dmg-background.tiff" 2>/dev/null
cat >"$WORK/dmg-settings.py" <<'PY'
import os

app = os.environ["CAPWISE_DMG_APP"]
format = "UDZO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
background = os.environ["CAPWISE_DMG_BACKGROUND"]
window_rect = ((200, 160), (660, 400))
icon_size = 112
text_size = 13
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
default_view = "icon-view"
icon_locations = {os.path.basename(app): (170, 178), "Applications": (490, 178)}
PY
CAPWISE_DMG_APP="$STAGE" CAPWISE_DMG_BACKGROUND="$WORK/dmg-background.tiff" \
  "$DMG_TOOLS/bin/python3" -m dmgbuild -s "$WORK/dmg-settings.py" "$APP_NAME $VERSION" "$DMG" >/dev/null
codesign --force "$timestamp_flag" --sign "$IDENTITY" "$DMG"

if ((DRY_RUN)); then
  step "Notarize the DMG (skipped: dry run)"
else
  step "Notarize the DMG"
  notarize "$DMG"
  staple "$DMG"
  xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature -vv "$DMG"
fi

step "Verify the shipped bundles"
verify_shipped_app() {
  local app="$1" executable
  "$HERE/scripts/verify-capwise-mac.sh" "$app" --resources-only
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" == "$VERSION" ]] ||
    die "wrong version in $app"
  for executable in "$APP_NAME" cap-cli; do
    lipo "$app/Contents/MacOS/$executable" -verify_arch "$MACHINE" ||
      die "wrong architecture: $app/Contents/MacOS/$executable"
  done
  codesign --verify --deep --strict "$app"
  if ! ((DRY_RUN)); then
    xcrun stapler validate "$app"
  fi
}
(
  verification="$(mktemp -d "$WORK/verify.XXXXXX")"
  mounted=0
  cleanup_verification() {
    if ((mounted)); then
      hdiutil detach "$verification/dmg" -quiet || return 1
    fi
    rm -rf -- "$verification"
  }
  trap cleanup_verification EXIT
  mkdir -p "$verification/archive" "$verification/dmg"
  tar -xzf "$ARCHIVE" -C "$verification/archive"
  verify_shipped_app "$verification/archive/$APP_NAME.app"
  hdiutil attach -quiet -readonly -nobrowse -mountpoint "$verification/dmg" "$DMG"
  mounted=1
  [[ "$(readlink "$verification/dmg/Applications")" == "/Applications" ]] ||
    die "DMG is missing its Applications shortcut"
  ditto "$verification/dmg/$APP_NAME.app" "$verification/$APP_NAME.app"
  verify_shipped_app "$verification/$APP_NAME.app"
  hdiutil detach "$verification/dmg" -quiet
  mounted=0
)
echo "DMG and update archive contain complete, signed app bundles."

step "latest.json"
notes=""
if [[ -n "${CAPWISE_RELEASE_NOTES:-}" ]]; then
  [[ -f "$CAPWISE_RELEASE_NOTES" ]] || die "release notes file $CAPWISE_RELEASE_NOTES does not exist"
  notes="$(cat "$CAPWISE_RELEASE_NOTES")"
fi
CAPWISE_NOTES="$notes" python3 - "$DIST/latest.json" "$VERSION" "$PLATFORM" \
  "https://github.com/$REPO/releases/download/$TAG/$ARCHIVE_NAME" "$(base64 -i "$ARCHIVE.minisig" | tr -d '\n')" <<'PY'
import datetime, json, os, sys

path, version, platform, url, signature = sys.argv[1:]
manifest = {
    "version": version,
    "notes": os.environ.get("CAPWISE_NOTES", ""),
    "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": {},
}
if os.path.exists(path):
    with open(path) as existing:
        previous = json.load(existing)
    if previous.get("version") == version:
        manifest["platforms"] = previous.get("platforms", {})
manifest["platforms"][platform] = {"url": url, "signature": signature}
with open(path, "w") as out:
    json.dump(manifest, out, indent=2, ensure_ascii=False)
    out.write("\n")
PY

if [[ "$ARCH" == "aarch64" ]]; then
  cp "$DMG" "$DIST/$APP_NAME.dmg"
fi
(cd "$DIST" && shasum -a 256 ./*.dmg ./*.app.tar.gz >SHA256SUMS.txt)

step "Done"
ls -lh "$DIST"
if ((DRY_RUN)); then
  echo
  echo "Dry run: not notarized, update signed with a throwaway key. Do not publish these files."
else
  cat <<EOF

Nothing has been uploaded. To publish (after committing the version bump and tagging $TAG):
  gh release create $TAG --repo $REPO --title "灵录 Capwise $VERSION" --notes-file <notes.md> \\
    "$DIST"/*.dmg "$ARCHIVE" "$ARCHIVE.minisig" "$DIST/latest.json" "$DIST/SHA256SUMS.txt"
EOF
fi
