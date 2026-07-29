#!/usr/bin/env bash
# diff_api_snapshot.sh — Compare two API contract snapshots.
# Usage: ./scripts/diff_api_snapshot.sh <old.json> <new.json>
#
# Exit code 0 = no breaking changes detected
# Exit code 1 = breaking changes detected (removed or changed items)

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 <old-snapshot.json> <new-snapshot.json>" >&2
  exit 2
fi

OLD=$1
NEW=$2

if ! command -v jq &>/dev/null; then
  echo "ERROR: jq is required but not found in PATH." >&2
  exit 2
fi

OLD_NAMES=$(jq -r '.[].name' "$OLD" | sort)
NEW_NAMES=$(jq -r '.[].name' "$NEW" | sort)

REMOVED=$(diff <(echo "$OLD_NAMES") <(echo "$NEW_NAMES") | grep '^<' | sed 's/^< //')
ADDED=$(diff   <(echo "$OLD_NAMES") <(echo "$NEW_NAMES") | grep '^>' | sed 's/^> //')

BREAKING=false

echo "=== API diff: $(basename "$OLD") → $(basename "$NEW") ==="
echo ""

if [[ -n "$REMOVED" ]]; then
  echo "### Removed items (BREAKING)"
  while IFS= read -r name; do
    echo "  - $name"
  done <<< "$REMOVED"
  BREAKING=true
  echo ""
fi

if [[ -n "$ADDED" ]]; then
  echo "### Added items (non-breaking)"
  while IFS= read -r name; do
    echo "  + $name"
  done <<< "$ADDED"
  echo ""
fi

if [[ "$REMOVED" == "" && "$ADDED" == "" ]]; then
  echo "No public API surface changes detected."
fi

if $BREAKING; then
  echo ""
  echo "RESULT: Breaking changes detected. Two maintainer approvals required." >&2
  exit 1
else
  echo "RESULT: No breaking changes."
  exit 0
fi
