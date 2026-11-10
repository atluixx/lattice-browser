#!/usr/bin/env bash
# M0 benchmark gate: Firefox baseline + Servo spike load measurements.
# Usage: bench/bench.sh [url] [timeout_secs]
# Env: CARGO_TARGET_DIR override supported; NIX_HARDENING_ENABLE="" required for servo 0.1.4.
set -euo pipefail
URL="${1:-https://example.com/}"
TIMEOUT="${2:-25}"
SPIKE="${CARGO_TARGET_DIR:-/tmp/lattice-servo-check2}/debug/examples/spike_load"
OUT_PNG="/tmp/opencode/lattice-spike-$(date +%s).png"

echo "=== Lattice M0 bench ==="
echo "--- system ---"
free -h
echo "--- firefox RSS total ---"
ps aux --sort=-%mem | grep -i firefox | grep -v grep | awk '{rss+=$6} END {printf "firefox_total_kb=%d firefox_total_mb=%.1f\n", rss, rss/1024}' || true
echo "--- spike ---"
if [ -x "$SPIKE" ]; then
  echo "bin=$SPIKE url=$URL timeout=$TIMEOUT"
  # Peak RSS of the spike process (includes Servo + software rendering).
  # NOTE: spike may segfault during process teardown *after* printing
  # results (no orderly Servo shutdown API in 0.1.4). Judge by output, not exit code.
  /usr/bin/time -v "$SPIKE" "$URL" "$TIMEOUT" "$OUT_PNG" 2>&1 | grep -E "^(url|title|status|frames|elapsed_ms|pixels|.*Maximum resident)" || true
  echo "png=$OUT_PNG"
else
  echo "spike not built: $SPIKE"
  echo "build with: nix-shell -p rustPlatform.bindgenHook llvmPackages.libstdcxxClang ... --run 'cargo build --example spike_load --features servo-embed -p lattice-embed'"
fi
echo "=== acceptance (M0): spike reaches LoadStatus::Complete on example.com + YouTube watch page; RSS recorded per run ==="
