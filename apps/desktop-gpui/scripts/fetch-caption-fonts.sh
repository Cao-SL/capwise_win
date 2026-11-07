#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$HERE/target/caption-fonts"
FONT_ASSETS="$HERE/assets/caption-fonts"

(cd "$FONT_ASSETS" && shasum -a 256 -c SHA256SUMS)
mkdir -p "$OUT"
cp "$FONT_ASSETS"/*.ttf "$FONT_ASSETS"/*.otf "$FONT_ASSETS"/*.txt "$OUT/"
echo "$OUT"
