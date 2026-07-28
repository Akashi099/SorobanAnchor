#!/usr/bin/env bash
# coverage_with_thresholds.sh — Run coverage analysis and enforce per-module thresholds.
#
# Usage:
#   ./scripts/coverage_with_thresholds.sh [--baseline <file>] [--report-delta] [--output-dir <dir>]
#
# Options:
#   --baseline <file>     JSON file containing prior coverage percentages used for
#                         delta computation.  When omitted, delta reporting is skipped.
#   --report-delta        Print per-module coverage deltas vs. the baseline.
#   --output-dir <dir>    Directory to write HTML and JSON reports (default: coverage/).
#
# Exit codes:
#   0  All thresholds met.
#   1  One or more thresholds violated, or unexpected error.
#
# Coverage thresholds (aligned with docs/coverage-metrics.md):
#   contract.rs                      >= 85 %
#   rate_limiter.rs                  >= 90 %
#   retry.rs                         >= 90 %
#   transaction_state_tracker.rs     >= 85 %
#
# CI integration:
#   Add the following step to .github/workflows/ci.yml (or code-quality.yml):
#
#     - name: Coverage with thresholds
#       run: ./scripts/coverage_with_thresholds.sh --output-dir coverage
#
#     - name: Upload coverage HTML
#       if: always()
#       uses: actions/upload-artifact@v4
#       with:
#         name: coverage-report
#         path: coverage/
#         retention-days: 14
#
# Delta reporting:
#   To detect regressions between runs, save the JSON summary from a base run
#   and pass it via --baseline on the next run:
#
#     # On the base commit (e.g. main):
#     ./scripts/coverage_with_thresholds.sh --output-dir coverage
#     cp coverage/summary.json /tmp/baseline.json
#
#     # On the feature branch:
#     ./scripts/coverage_with_thresholds.sh \
#         --baseline /tmp/baseline.json \
#         --report-delta

set -euo pipefail

# ── Defaults ─────────────────────────────────────────────────────────────────
OUTPUT_DIR="coverage"
BASELINE_FILE=""
REPORT_DELTA=false

# ── Parse arguments ───────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --baseline)     BASELINE_FILE="$2"; shift 2 ;;
    --report-delta) REPORT_DELTA=true; shift ;;
    --output-dir)   OUTPUT_DIR="$2"; shift 2 ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

mkdir -p "$OUTPUT_DIR"

# ── Coverage thresholds ───────────────────────────────────────────────────────
# Keys must match the fragment used in tarpaulin's HTML file-path output.
declare -A THRESHOLDS=(
  ["contract"]="85"
  ["rate_limiter"]="90"
  ["retry"]="90"
  ["transaction_state_tracker"]="85"
)

# ── Tooling check ─────────────────────────────────────────────────────────────
if ! command -v cargo-tarpaulin &>/dev/null; then
  echo "[coverage] Installing cargo-tarpaulin …"
  cargo install cargo-tarpaulin --locked
fi

echo "============================================================"
echo " SorobanAnchor — Coverage with Thresholds"
echo " Output dir : $OUTPUT_DIR"
echo "============================================================"
echo ""

# ── Run tarpaulin ─────────────────────────────────────────────────────────────
echo "[1/4] Running cargo-tarpaulin …"
cargo tarpaulin \
  --out Html \
  --out Json \
  --output-dir "$OUTPUT_DIR" \
  --exclude-files "tests/*" \
  --timeout 300 \
  --verbose 2>&1 | tee "$OUTPUT_DIR/tarpaulin.log"

JSON_REPORT="$OUTPUT_DIR/tarpaulin-report.json"
if [[ ! -f "$JSON_REPORT" ]]; then
  echo "[coverage] ERROR: expected JSON report at $JSON_REPORT — check tarpaulin output."
  exit 1
fi

echo ""
echo "[2/4] Parsing per-module coverage …"

# Extract per-file coverage percentages from the tarpaulin JSON report.
# Tarpaulin's JSON schema:  { "files": [ { "path": "src/foo.rs", "covered": N, "coverable": M }, ... ] }
declare -A ACTUAL_PCT

if command -v python3 &>/dev/null; then
  PARSE_SCRIPT=$(cat <<'PYEOF'
import json, sys, os

with open(sys.argv[1]) as f:
    data = json.load(f)

# tarpaulin v0.27+ uses "files" as a top-level list
files = data.get("files", [])
for entry in files:
    path   = entry.get("path", "")
    cov    = entry.get("covered", 0)
    total  = entry.get("coverable", 1)
    pct    = (cov * 100 // total) if total > 0 else 0
    # emit module_name=pct pairs so the shell can read them
    module = os.path.splitext(os.path.basename(path))[0]
    print(f"{module}={pct}")
PYEOF
  )
  while IFS='=' read -r mod pct; do
    ACTUAL_PCT["$mod"]="$pct"
  done < <(python3 -c "$PARSE_SCRIPT" "$JSON_REPORT" 2>/dev/null || true)
else
  echo "[coverage] WARNING: python3 not found; per-module threshold checks will be skipped."
fi

# ── Write human-readable summary JSON for delta baseline ──────────────────────
SUMMARY_FILE="$OUTPUT_DIR/summary.json"
{
  echo "{"
  first=true
  for mod in "${!ACTUAL_PCT[@]}"; do
    [[ "$first" == "true" ]] || echo ","
    first=false
    printf '  "%s": %s' "$mod" "${ACTUAL_PCT[$mod]}"
  done
  echo ""
  echo "}"
} > "$SUMMARY_FILE"

# ── Threshold enforcement ─────────────────────────────────────────────────────
echo ""
echo "[3/4] Enforcing coverage thresholds …"
echo ""

PASSED=true
declare -A RESULTS

for mod in "${!THRESHOLDS[@]}"; do
  threshold="${THRESHOLDS[$mod]}"
  actual="${ACTUAL_PCT[$mod]:-UNKNOWN}"

  if [[ "$actual" == "UNKNOWN" ]]; then
    echo "  ⚠  $mod.rs : coverage data not found in report (module may be excluded or renamed)."
    RESULTS["$mod"]="UNKNOWN"
    continue
  fi

  if (( actual >= threshold )); then
    echo "  ✓  $mod.rs : ${actual}%  (threshold: ${threshold}%)"
    RESULTS["$mod"]="PASS"
  else
    echo "  ✗  $mod.rs : ${actual}%  (threshold: ${threshold}%) — BELOW THRESHOLD"
    RESULTS["$mod"]="FAIL"
    PASSED=false
  fi
done

# ── Delta reporting ───────────────────────────────────────────────────────────
if [[ "$REPORT_DELTA" == "true" && -n "$BASELINE_FILE" && -f "$BASELINE_FILE" ]]; then
  echo ""
  echo "[4/4] Coverage delta vs. baseline ($BASELINE_FILE) …"
  echo ""

  if command -v python3 &>/dev/null; then
    DELTA_SCRIPT=$(cat <<'PYEOF'
import json, sys

with open(sys.argv[1]) as f:
    baseline = json.load(f)
with open(sys.argv[2]) as f:
    current = json.load(f)

regressions = []
for mod, cur_pct in current.items():
    base_pct = baseline.get(mod)
    if base_pct is None:
        print(f"  +new  {mod}: {cur_pct}%  (no baseline)")
        continue
    delta = cur_pct - base_pct
    arrow = "▲" if delta > 0 else ("▼" if delta < 0 else "=")
    print(f"  {arrow}  {mod}: {cur_pct}%  (was {base_pct}%, Δ{delta:+d}%)")
    if delta < -2:  # flag regressions larger than 2 percentage points
        regressions.append((mod, base_pct, cur_pct, delta))

if regressions:
    print("")
    print("REGRESSIONS DETECTED (> 2% drop):")
    for mod, base, cur, d in regressions:
        print(f"  !! {mod}: {base}% → {cur}% ({d:+d}%)")
    sys.exit(1)
PYEOF
    )
    python3 -c "$DELTA_SCRIPT" "$BASELINE_FILE" "$SUMMARY_FILE" || {
      echo ""
      echo "ERROR: Coverage regressions detected. See output above."
      PASSED=false
    }
  else
    echo "[coverage] WARNING: python3 not found; delta reporting skipped."
  fi
else
  echo "[4/4] Delta reporting: skipped (no --baseline provided or --report-delta not set)."
fi

# ── Final verdict ─────────────────────────────────────────────────────────────
echo ""
echo "============================================================"
if [[ "$PASSED" == "true" ]]; then
  echo " RESULT: ALL THRESHOLDS MET"
  echo ""
  echo " Reports:"
  echo "   HTML   : $OUTPUT_DIR/index.html"
  echo "   JSON   : $JSON_REPORT"
  echo "   Summary: $SUMMARY_FILE"
  echo "============================================================"
  exit 0
else
  echo " RESULT: THRESHOLD VIOLATIONS — see output above"
  echo ""
  echo " Reports:"
  echo "   HTML   : $OUTPUT_DIR/index.html"
  echo "   JSON   : $JSON_REPORT"
  echo "   Summary: $SUMMARY_FILE"
  echo "============================================================"
  exit 1
fi
