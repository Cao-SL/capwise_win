#!/usr/bin/env bash
# Download the open-source (SIL OFL 1.1) Chinese caption fonts Capwise ships,
# verify them, and leave them with their licenses in target/caption-fonts.
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$HERE/target/caption-fonts"
mkdir -p "$OUT"

fetch() {
  local url="$1" file="$2" sha="$3"
  if [[ -f "$OUT/$file" ]] && [[ "$(shasum -a 256 "$OUT/$file" | cut -d' ' -f1)" == "$sha" ]]; then
    return
  fi
  curl -sSfL --retry 3 -o "$OUT/$file.part" "$url"
  local got
  got="$(shasum -a 256 "$OUT/$file.part" | cut -d' ' -f1)"
  if [[ "$got" != "$sha" ]]; then
    rm -f "$OUT/$file.part"
    echo "checksum mismatch for $file: $got" >&2
    exit 1
  fi
  mv "$OUT/$file.part" "$OUT/$file"
}

# 思源黑体 (Noto Sans SC), the Simplified Chinese subset, Bold.
fetch "https://github.com/notofonts/noto-cjk/raw/main/Sans/SubsetOTF/SC/NotoSansSC-Bold.otf" \
  NotoSansSC-Bold.otf c6cb5a93abaa9edc8ee7463b7ebb7f42d618d40e6ed2f7a5371c97b0b64767c0
# 霞鹜文楷 轻便版 Medium.
fetch "https://github.com/lxgw/LxgwWenKai-Lite/releases/download/v1.530/LXGWWenKaiLite-Medium.ttf" \
  LXGWWenKaiLite-Medium.ttf 9282063131640c9558a102560cf6d6f8ecc4bb441e74f2bcff032ce4786238d0
# 得意黑 (Smiley Sans).
fetch "https://github.com/atelier-anchor/smiley-sans/releases/download/v2.0.1/smiley-sans-v2.0.1.zip" \
  smiley-sans.zip 299c0be6c960ae37361762eca76f7d0cd516615435bb96c0d4b98a1e70178a07
unzip -qo -j "$OUT/smiley-sans.zip" SmileySans-Oblique.ttf -d "$OUT"

curl -sSfL --retry 3 -o "$OUT/OFL-NotoSansSC.txt" https://github.com/notofonts/noto-cjk/raw/main/Sans/LICENSE
curl -sSfL --retry 3 -o "$OUT/OFL-LXGWWenKaiLite.txt" https://github.com/lxgw/LxgwWenKai-Lite/raw/main/OFL.txt
curl -sSfL --retry 3 -o "$OUT/OFL-SmileySans.txt" https://github.com/atelier-anchor/smiley-sans/raw/main/LICENSE
echo "$OUT"
