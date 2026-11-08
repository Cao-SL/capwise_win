#!/usr/bin/env bash
# One command for the Capwise performance baseline. See scripts/bench_perf.py.
#   scripts/bench.sh                         run every stage, save bench-results/<date>.json
#   scripts/bench.sh --stages startup        any subset
#   scripts/bench.sh compare a.json b.json   regression diff
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
if [[ "${1:-}" == "compare" ]]; then
  exec python3 "$HERE/bench_perf.py" "$@"
fi
OUT_DIR="${BENCH_OUT_DIR:-$HERE/../bench-results}"
mkdir -p "$OUT_DIR"
exec python3 "$HERE/bench_perf.py" run --out "$OUT_DIR/$(date +%Y%m%d-%H%M%S).json" "$@"
