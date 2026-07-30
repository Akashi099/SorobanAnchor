#!/usr/bin/env bash
# Validates the current git branch name against the project naming convention.
# See docs/branch-pr-hygiene.md for the full convention.

set -euo pipefail

BRANCH=$(git rev-parse --abbrev-ref HEAD)
PATTERN='^(feat|fix|docs|chore|refactor|test|release)/[a-z0-9][a-z0-9-]{0,39}$'
EXEMPT='^(main|master|HEAD)$'

if [[ "$BRANCH" =~ $EXEMPT ]]; then
    exit 0
fi

if [[ "$BRANCH" =~ $PATTERN ]]; then
    echo "Branch name OK: ${BRANCH}"
    exit 0
else
    echo "ERROR: Branch name '${BRANCH}' does not follow the convention."
    echo ""
    echo "Required format: <type>/<short-description>"
    echo "Allowed types:   feat, fix, docs, chore, refactor, test, release"
    echo "Rules:           lowercase, hyphens only, description ≤ 40 chars"
    echo ""
    echo "Valid examples:"
    echo "  feat/add-roadmap-696"
    echo "  fix/jwt-expiry-parsing"
    echo "  docs/triage-workflow-697"
    echo ""
    echo "See docs/branch-pr-hygiene.md for details."
    exit 1
fi
