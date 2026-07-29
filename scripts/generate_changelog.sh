#!/usr/bin/env bash
# generate_changelog.sh — Generate or preview CHANGELOG.md from conventional commits.
# Usage: ./scripts/generate_changelog.sh [<tag>] [--dry-run] [--prepend]
#
# Requires: git-cliff  (install: cargo install git-cliff)
# Output:   CHANGELOG.md (repo root) unless --dry-run is passed

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DRY_RUN=false
PREPEND=false
TAG=""

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=true ;;
    --prepend) PREPEND=true ;;
    v*)        TAG="$arg" ;;
    [0-9]*)    TAG="v$arg" ;;
  esac
done

# Default tag from Cargo.toml version
if [[ -z "$TAG" ]]; then
  VERSION=$(grep '^version' "$ROOT/Cargo.toml" | head -1 | sed 's/.*= *"\(.*\)"/\1/')
  TAG="v${VERSION}"
fi

if ! command -v git-cliff &>/dev/null; then
  echo "ERROR: git-cliff is not installed." >&2
  echo "  Install with: cargo install git-cliff" >&2
  exit 1
fi

echo "Generating changelog for $TAG..."

if $DRY_RUN; then
  git -C "$ROOT" cliff --unreleased --tag "$TAG"
  exit 0
fi

if $PREPEND; then
  git -C "$ROOT" cliff --unreleased --tag "$TAG" --prepend "$ROOT/CHANGELOG.md"
  echo "Changelog prepended to CHANGELOG.md"
else
  git -C "$ROOT" cliff --unreleased --tag "$TAG" --output "$ROOT/CHANGELOG.md"
  echo "CHANGELOG.md written."
fi
