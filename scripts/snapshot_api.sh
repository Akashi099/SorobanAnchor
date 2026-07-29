#!/usr/bin/env bash
# snapshot_api.sh — Generate a public API surface snapshot for AnchorKit.
# Usage: ./scripts/snapshot_api.sh [--dry-run]
#
# Requires: cargo (nightly toolchain), jq
# Output:   api_snapshots/anchorkit-<VERSION>.json

set -euo pipefail

DRY_RUN=false
for arg in "$@"; do
  [[ "$arg" == "--dry-run" ]] && DRY_RUN=true
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

VERSION=$(grep '^version' "$ROOT/Cargo.toml" | head -1 | sed 's/.*= *"\(.*\)"/\1/')
OUT_DIR="$ROOT/api_snapshots"
OUT_FILE="$OUT_DIR/anchorkit-${VERSION}.json"

if $DRY_RUN; then
  echo "[dry-run] Would generate snapshot: $OUT_FILE"
  exit 0
fi

# Require jq
if ! command -v jq &>/dev/null; then
  echo "ERROR: jq is required but not found in PATH." >&2
  exit 1
fi

mkdir -p "$OUT_DIR"

echo "Building rustdoc JSON for v${VERSION}..."
cargo +nightly rustdoc --lib \
    --manifest-path "$ROOT/Cargo.toml" \
    -- \
    -Z unstable-options \
    --output-format json \
    --document-private-items=false 2>/dev/null

DOC_JSON="$ROOT/target/doc/anchorkit.json"

if [[ ! -f "$DOC_JSON" ]]; then
  echo "ERROR: rustdoc JSON output not found at $DOC_JSON" >&2
  exit 1
fi

echo "Extracting public API surface..."
jq '
  .index
  | to_entries
  | map(select(.value.visibility == "public"))
  | map({
      id:   .key,
      name: .value.name,
      kind: .value.kind,
      docs: (.value.docs // "")
    })
  | sort_by(.name)
' "$DOC_JSON" > "$OUT_FILE"

echo "Snapshot written to $OUT_FILE"
echo "  Items captured: $(jq length "$OUT_FILE")"
