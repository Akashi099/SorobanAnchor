#!/usr/bin/env bash
# Release checklist automation for SorobanAnchor.
# Usage: ./scripts/release-checklist.sh <version>
# See docs/release-checklist.md for full documentation.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

pass() { echo -e "${GREEN}[PASS]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; FAILURES=$((FAILURES + 1)); }
info() { echo -e "${YELLOW}[INFO]${NC} $1"; }

FAILURES=0

if [[ $# -lt 1 ]]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 0.3.0"
    exit 1
fi

VERSION="$1"
info "Running release checklist for v${VERSION}"
echo "-------------------------------------------"

# 1. Git working tree is clean
if git diff --quiet && git diff --cached --quiet; then
    pass "Working tree is clean"
else
    fail "Working tree has uncommitted changes — commit or stash them first"
fi

# 2. On main branch
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [[ "$CURRENT_BRANCH" == "main" ]]; then
    pass "On main branch"
else
    fail "Not on main branch (current: ${CURRENT_BRANCH})"
fi

# 3. Up to date with origin/main
git fetch origin main --quiet 2>/dev/null || true
LOCAL=$(git rev-parse HEAD)
REMOTE=$(git rev-parse origin/main 2>/dev/null || echo "unknown")
if [[ "$LOCAL" == "$REMOTE" ]]; then
    pass "Branch is up to date with origin/main"
else
    fail "Branch is behind origin/main — pull first"
fi

# 4. Cargo.toml version matches
CARGO_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
if [[ "$CARGO_VERSION" == "$VERSION" ]]; then
    pass "Cargo.toml version matches (${VERSION})"
else
    fail "Cargo.toml version is '${CARGO_VERSION}', expected '${VERSION}'"
fi

# 5. CHANGELOG.md has an entry for this version
if grep -q "\[${VERSION}\]\|## ${VERSION}\|# ${VERSION}" CHANGELOG.md 2>/dev/null; then
    pass "CHANGELOG.md contains entry for v${VERSION}"
else
    fail "CHANGELOG.md has no entry for v${VERSION} — update it before releasing"
fi

# 6. Formatting
info "Checking formatting..."
if cargo fmt --all -- --check 2>&1; then
    pass "cargo fmt check passed"
else
    fail "Formatting issues found — run 'cargo fmt --all' to fix"
fi

# 7. Linting
info "Running clippy..."
if cargo clippy --all-targets --all-features -- -D warnings 2>&1; then
    pass "cargo clippy passed"
else
    fail "Clippy warnings found — fix them before releasing"
fi

# 8. Tests
info "Running tests..."
if cargo test 2>&1; then
    pass "cargo test passed"
else
    fail "Tests failed — fix them before releasing"
fi

# 9. WASM build
info "Building WASM target..."
if cargo build --target wasm32-unknown-unknown 2>&1; then
    pass "WASM build succeeded"
else
    fail "WASM build failed — check no_std compliance"
fi

# 10. Dependency audit
if command -v cargo-audit &>/dev/null; then
    info "Running dependency audit..."
    if cargo audit 2>&1; then
        pass "cargo audit: no vulnerabilities"
    else
        fail "Vulnerabilities found — resolve them before releasing"
    fi
else
    info "cargo-audit not installed — skipping (install with: cargo install cargo-audit)"
fi

# 11. API snapshot diff
if [[ -f scripts/diff_api_snapshot.sh ]]; then
    info "Checking API snapshot diff..."
    if bash scripts/diff_api_snapshot.sh 2>&1; then
        pass "API snapshot has no unexpected diff"
    else
        fail "Unexpected API snapshot diff — review and update if intentional"
    fi
else
    info "diff_api_snapshot.sh not found — skipping API snapshot check"
fi

echo "-------------------------------------------"
if [[ $FAILURES -eq 0 ]]; then
    echo -e "${GREEN}All checks passed. Ready to release v${VERSION}.${NC}"
    echo ""
    echo "Next steps:"
    echo "  git tag -s v${VERSION} -m 'Release v${VERSION}'"
    echo "  git push origin v${VERSION}"
    echo "  ./scripts/package_release.sh"
    echo "  ./scripts/sign_release.sh"
    exit 0
else
    echo -e "${RED}${FAILURES} check(s) failed. Fix the issues above before releasing.${NC}"
    exit 1
fi
