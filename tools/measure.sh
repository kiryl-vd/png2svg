#!/bin/sh
# Converts every fixture and prints size and statistics.
# Usage: tools/measure.sh [extra png2svg options]   (needs a release build)
BIN="$(dirname "$0")/../target/release/png2svg"
OUT="${TMPDIR:-/tmp}/png2svg-measure"
mkdir -p "$OUT"
for f in logo icon text screenshot pixelart photo gradient; do
  IN="$(dirname "$0")/../tests/fixtures/$f.png"
  "$BIN" -f -v "$@" "$IN" "$OUT/$f.svg" 2>&1 >/dev/null | grep -E "blend|colors|regions|svg size" | tr '\n' ' '
  echo "[$f png=$(wc -c < "$IN" | tr -d ' ') paths=$(grep -o '<path' "$OUT/$f.svg" | wc -l | tr -d ' ')]"
done
