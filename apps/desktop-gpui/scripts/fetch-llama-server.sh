#!/usr/bin/env bash
# Download a pinned llama.cpp build and keep llama-server with the libraries
# it loads, for running Qwen3-ASR locally. Lands in target/llama-server.
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$HERE/target/llama-server"
BUILD="b12144"
SHA="2045e1d2dd7f06beed9a9a7594db1bb1c42e5b92bffcb3bd503679c8d7b6c8a9"
URL="https://github.com/ggml-org/llama.cpp/releases/download/$BUILD/llama-$BUILD-bin-macos-arm64.tar.gz"

if [[ -x "$OUT/llama-server" && "$(cat "$OUT/BUILD" 2>/dev/null)" == "$BUILD" ]]; then
  echo "$OUT"
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
curl -sSfL --retry 3 -o "$WORK/llama.tar.gz" "$URL"
got="$(shasum -a 256 "$WORK/llama.tar.gz" | cut -d' ' -f1)"
if [[ "$got" != "$SHA" ]]; then
  echo "checksum mismatch for llama.cpp $BUILD: $got" >&2
  exit 1
fi
tar xzf "$WORK/llama.tar.gz" -C "$WORK"
SRC="$WORK/llama-$BUILD"

rm -rf "$OUT"
mkdir -p "$OUT"
cp "$SRC/llama-server" "$SRC/LICENSE" "$OUT/"
# The server's own library plus everything it links, with the version
# symlinks it is linked against (`@rpath/libllama.0.dylib` and so on).
for lib in libllama-server-impl libllama-common libmtmd libllama libggml libggml-base \
  libggml-cpu libggml-blas libggml-metal libggml-rpc; do
  cp -a "$SRC"/"$lib".*dylib "$OUT/" 2>/dev/null || true
  cp -a "$SRC"/"$lib".dylib "$OUT/" 2>/dev/null || true
done
echo "$BUILD" > "$OUT/BUILD"
echo "$OUT"
