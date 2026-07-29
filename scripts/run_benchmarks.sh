#!/bin/bash
# run_benchmarks.sh — Run the SorobanAnchor performance benchmark suite.
#
# Usage:
#   ./scripts/run_benchmarks.sh                 # Run all benchmarks
#   ./scripts/run_benchmarks.sh --save main     # Save results as baseline 'main'
#   ./scripts/run_benchmarks.sh --compare main  # Compare against baseline 'main'
#
# Results are written to target/criterion/ with HTML reports at
# target/criterion/report/index.html.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=== SorobanAnchor Performance Benchmarks ==="
echo ""

BASELINE_NAME="${2:-}"
MODE="${1:-}"

case "$MODE" in
  --save)
    if [ -z "$BASELINE_NAME" ]; then
      echo "Usage: $0 --save <baseline-name>"
      exit 1
    fi
    echo "Running benchmarks and saving baseline: $BASELINE_NAME"
    cargo bench --bench load_benchmarks -- --save-baseline "$BASELINE_NAME"
    echo ""
    echo "✅ Baseline '$BASELINE_NAME' saved."
    ;;
  --compare)
    if [ -z "$BASELINE_NAME" ]; then
      echo "Usage: $0 --compare <baseline-name>"
      exit 1
    fi
    echo "Running benchmarks and comparing against baseline: $BASELINE_NAME"
    if cargo bench --bench load_benchmarks -- --baseline "$BASELINE_NAME" 2>&1 | tee /tmp/bench_output.txt; then
      if grep -q "Performance has regressed" /tmp/bench_output.txt; then
        echo ""
        echo "⚠️  WARNING: Performance regression detected!"
        echo "Review the output above for affected benchmarks."
        exit 1
      else
        echo ""
        echo "✅ No significant regressions detected against baseline '$BASELINE_NAME'."
      fi
    fi
    ;;
  "")
    echo "Running all benchmarks..."
    cargo bench --bench load_benchmarks
    ;;
  *)
    echo "Unknown option: $MODE"
    echo "Usage: $0 [--save <name> | --compare <name>]"
    exit 1
    ;;
esac

echo ""
echo "=== Benchmark Groups ==="
echo ""
echo "  attestation_verification"
echo "    • single_attestation_verification    — Expected: > 5 M ops/s"
echo "    • batch_attestation_verification_100 — Expected: > 3 M ops/s"
echo ""
echo "  batch_attestor_registration"
echo "    • /10, /50, /100                     — Expected: < 50 µs at 100 attestors"
echo ""
echo "  rate_limit_check"
echo "    • /100, /500, /1000                  — Expected: > 10 M ops/s at 1 000"
echo ""
echo "  anchor_routing"
echo "    • /10, /25, /50                      — Expected: < 5 µs at 50 anchors"
echo ""
echo "  quote_routing"
echo "    • /10, /25, /50                      — Expected: < 10 µs at 50 quotes"
echo ""
echo "  metadata_cache_lookup"
echo "    • hit/100, hit/500, hit/1000         — Expected: > 20 M ops/s (hot)"
echo "    • miss/100, miss/500, miss/1000      — Expected: > 10 M ops/s (cold)"
echo ""
echo "  transaction_status_normalization"
echo "    • single_normalize                   — Expected: > 20 M ops/s"
echo "    • batch_normalize_100                — Expected: > 15 M ops/s"
echo ""
echo "  replay_detection"
echo "    • lookup_replay/100–10000            — Expected: > 20 M ops/s"
echo "    • insert_new/100–10000               — Expected: < 500 ns per entry"
echo ""
echo "  deterministic_hash"
echo "    • hash_32_bytes                      — Expected: < 500 ns"
echo "    • hash_1000_sequential               — Expected: < 1 ms"
echo ""
echo "  batch_attestation"
echo "    • /10, /50, /100                     — Expected: < 200 µs at 100 entries"
echo ""
echo "HTML reports: target/criterion/report/index.html"
echo ""
