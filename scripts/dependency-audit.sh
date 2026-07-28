#!/usr/bin/env bash
# dependency-audit.sh — Automated dependency auditing and policy enforcement.
#
# Usage:
#   ./scripts/dependency-audit.sh              # full audit (default)
#   ./scripts/dependency-audit.sh --quick      # skip security scan
#   ./scripts/dependency-audit.sh --ci         # CI mode: fail on any policy violation
#   ./scripts/dependency-audit.sh --report     # write machine-readable JSON report
#
# Tools used:
#   cargo audit    — vulnerability scanning (cargo install cargo-audit)
#   cargo license  — license enumeration   (cargo install cargo-license)
#   cargo-deny     — policy enforcement    (cargo install cargo-deny)
#
# Policy thresholds (override via environment variables):
#   AUDIT_MAX_DEPS=300           — warn if transitive dep count exceeds this
#   AUDIT_DENIED_LICENSES        — space-separated list of denied SPDX license IDs
#
# Exit codes:
#   0 — no policy violations
#   1 — one or more violations found

set -euo pipefail

# ── Colour helpers ─────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

pass()    { echo -e "  ${GREEN}✓${NC} $*"; }
fail()    { echo -e "  ${RED}✗${NC} $*"; FAILURES=$((FAILURES + 1)); }
warn()    { echo -e "  ${YELLOW}⚠${NC} $*"; }
info()    { echo -e "  ${CYAN}ℹ${NC} $*"; }
section() { echo -e "\n${BOLD}${CYAN}━━ $* ━━${NC}"; }

# ── Argument parsing ───────────────────────────────────────────────────────────
QUICK=false
CI_MODE=false
REPORT=false

for arg in "$@"; do
    case "$arg" in
        --quick)  QUICK=true ;;
        --ci)     CI_MODE=true ;;
        --report) REPORT=true ;;
        *)        echo "Unknown argument: $arg" >&2; exit 1 ;;
    esac
done

# Policy thresholds
MAX_DEPS="${AUDIT_MAX_DEPS:-300}"
DENIED_LICENSES="${AUDIT_DENIED_LICENSES:-GPL AGPL SSPL BUSL}"

FAILURES=0
REPORT_LINES=()

report_add() { REPORT_LINES+=("$1"); }

# ── Header ─────────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}╔══════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║   SorobanAnchor Dependency Audit         ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════╝${NC}"
echo -e "  Date    : $(date -u '+%Y-%m-%d %H:%M UTC')"
echo -e "  Branch  : $(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo 'unknown')"
echo -e "  Commit  : $(git rev-parse --short HEAD 2>/dev/null || echo 'unknown')"
echo -e "  Mode    : $([ "$QUICK" = "true" ] && echo 'quick' || echo 'full')$([ "$CI_MODE" = "true" ] && echo ' (CI)' || echo '')"
echo ""

# ── 1. Lock file freshness ─────────────────────────────────────────────────────
section "1. Lock file freshness"

if cargo update --dry-run 2>&1 | grep -q "Updating"; then
    OUTDATED=$(cargo update --dry-run 2>&1 | grep "Updating" || true)
    warn "Cargo.lock is out of date. Outdated packages:"
    echo "$OUTDATED" | sed 's/^/     /'
    warn "Run 'cargo update' to pick up patch releases, then review and commit Cargo.lock."
    report_add "lock_file_outdated=true"
    if $CI_MODE; then
        fail "Lock file outdated — failing in CI mode"
    fi
else
    pass "Cargo.lock is up to date"
    report_add "lock_file_outdated=false"
fi

# Verify Cargo.lock is committed
if git ls-files --error-unmatch Cargo.lock &>/dev/null 2>&1; then
    pass "Cargo.lock is committed to the repository"
    report_add "lock_file_committed=true"
else
    fail "Cargo.lock is NOT committed — required for reproducible builds"
    report_add "lock_file_committed=false"
fi

# ── 2. Cargo.toml pinning policy ─────────────────────────────────────────────
section "2. Dependency version pinning"

# Check for unpinned (open-range) dependency versions in Cargo.toml.
UNPINNED=$(grep -E '^\s*(version\s*=\s*"[^"]*[*^~>]|"[*^~>])' Cargo.toml 2>/dev/null \
    | grep -v '^\s*#' || true)

if [[ -n "$UNPINNED" ]]; then
    warn "Potentially unpinned dependency versions found:"
    echo "$UNPINNED" | sed 's/^/     /'
    warn "Prefer exact versions (e.g. '= \"1.2.3\"') for supply-chain stability."
    report_add "unpinned_versions=true"
else
    pass "No obvious open-range version constraints detected"
    report_add "unpinned_versions=false"
fi

# Check for [patch] overrides pointing to non-registry sources (potential supply-chain risk)
if grep -q '^\[patch\]' Cargo.toml 2>/dev/null; then
    PATCH_SOURCES=$(awk '/^\[patch\]/,/^\[/' Cargo.toml | grep -E 'git|path' || true)
    if [[ -n "$PATCH_SOURCES" ]]; then
        warn "[patch] overrides using git or path sources detected:"
        echo "$PATCH_SOURCES" | sed 's/^/     /'
        warn "Ensure all patched crates are reviewed and approved."
        report_add "patch_overrides=true"
        if $CI_MODE; then
            fail "[patch] overrides in CI mode — review required"
        fi
    else
        pass "No non-registry [patch] overrides found"
        report_add "patch_overrides=false"
    fi
else
    pass "No [patch] overrides in Cargo.toml"
    report_add "patch_overrides=false"
fi

# ── 3. License compliance ─────────────────────────────────────────────────────
section "3. License compliance"
echo "  Denied license identifiers: ${DENIED_LICENSES}"
echo ""

if command -v cargo-license &>/dev/null || cargo license --version &>/dev/null 2>&1; then
    LICENSE_OUTPUT=$(cargo license --color never --avoid-build-deps 2>/dev/null \
        | grep -v "^anchorkit" || true)
    LICENSE_FAILURES=0

    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        crate=$(echo "$line" | awk '{print $1}')
        license=$(echo "$line" | sed 's/^[^ ]* //')
        denied=false
        for dl in $DENIED_LICENSES; do
            if echo "$license" | grep -qi "$dl"; then
                denied=true; break
            fi
        done
        if $denied; then
            fail "DENIED  $crate — $license"
            LICENSE_FAILURES=$((LICENSE_FAILURES + 1))
        else
            echo -e "  ${GREEN}✓${NC}        $crate — $license"
        fi
    done <<< "$LICENSE_OUTPUT"

    if [[ "$LICENSE_FAILURES" -eq 0 ]]; then
        pass "All dependency licenses are compliant"
        report_add "license_violations=0"
    else
        report_add "license_violations=${LICENSE_FAILURES}"
    fi
else
    warn "cargo-license not installed."
    info "Install with: cargo install cargo-license"
    warn "Skipping license check — recommended for full compliance."
    report_add "license_violations=skipped"

    # Fallback: list direct deps from metadata
    echo ""
    info "Direct dependencies (from cargo metadata, no license info):"
    cargo metadata --no-deps --format-version 1 2>/dev/null \
        | python3 -c "
import sys, json
data = json.load(sys.stdin)
pkg = next((p for p in data['packages'] if p['name'] == 'anchorkit'), None)
if pkg:
    deps = sorted(set(d['name'] for d in pkg['dependencies']))
    for d in deps:
        print(f'    - {d}')
" 2>/dev/null || info "(cargo metadata unavailable)"
fi

# ── 4. Security vulnerability scan ────────────────────────────────────────────
section "4. Security vulnerability scan (cargo audit)"

if $QUICK; then
    warn "Skipped (--quick mode). Run without --quick for a full security scan."
    report_add "security_scan=skipped"
elif command -v cargo-audit &>/dev/null || cargo audit --version &>/dev/null 2>&1; then
    echo ""
    AUDIT_RESULT=0
    # --deny warnings: any advisory (including informational) fails the build
    if cargo audit --deny warnings 2>&1; then
        pass "No known vulnerabilities found"
        report_add "security_scan=pass"
    else
        AUDIT_RESULT=$?
        fail "Security vulnerabilities detected — see output above"
        report_add "security_scan=fail"
        # Counts are already printed by cargo-audit; bump our failure counter.
        FAILURES=$((FAILURES + 1))
        # Reset so the final FAILURES count is accurate (we already incremented above).
        FAILURES=$((FAILURES - 1))
        fail "cargo audit reported vulnerabilities"
    fi
else
    warn "cargo-audit not installed."
    info "Install with: cargo install cargo-audit"
    warn "Vulnerability scan skipped — REQUIRED for production releases."
    report_add "security_scan=skipped"
    if $CI_MODE; then
        fail "cargo-audit is required in CI mode — install it in the CI toolchain"
    fi
fi

# ── 5. cargo-deny policy enforcement ─────────────────────────────────────────
section "5. Policy enforcement (cargo deny)"

if command -v cargo-deny &>/dev/null || cargo deny --version &>/dev/null 2>&1; then
    echo ""
    if cargo deny check 2>&1; then
        pass "cargo deny: all policies satisfied"
        report_add "cargo_deny=pass"
    else
        fail "cargo deny: policy violation(s) detected — see output above"
        report_add "cargo_deny=fail"
    fi
else
    warn "cargo-deny not installed."
    info "Install with: cargo install cargo-deny"
    info "cargo-deny enforces license, ban, and advisory policies via deny.toml."
    report_add "cargo_deny=skipped"
fi

# ── 6. Dependency statistics ───────────────────────────────────────────────────
section "6. Dependency statistics"

TOTAL=$(grep -c '^name = ' Cargo.lock 2>/dev/null || echo 0)
DIRECT=$(cargo metadata --no-deps --format-version 1 2>/dev/null \
    | python3 -c "
import sys, json
data = json.load(sys.stdin)
pkg = next((p for p in data['packages'] if p['name'] == 'anchorkit'), None)
print(len(pkg['dependencies']) if pkg else '?')
" 2>/dev/null || grep -cE '^\[dependencies' Cargo.toml || echo '?')

echo ""
echo "  Transitive dependency count : ${TOTAL}"
echo "  Direct dependency count     : ${DIRECT}"
echo "  Max allowed (policy)        : ${MAX_DEPS}"
echo ""

report_add "total_deps=${TOTAL}"

if [[ "${TOTAL}" =~ ^[0-9]+$ ]] && [[ "${TOTAL}" -gt "${MAX_DEPS}" ]]; then
    warn "Transitive dependency count (${TOTAL}) exceeds policy threshold (${MAX_DEPS})."
    warn "Review Cargo.toml for opportunities to reduce the dependency surface."
    if $CI_MODE; then
        fail "Dependency count exceeds CI policy threshold"
    fi
    report_add "dep_count_policy=warn"
else
    pass "Dependency count (${TOTAL}) is within policy threshold (${MAX_DEPS})"
    report_add "dep_count_policy=pass"
fi

# ── 7. Unused dependency hint ──────────────────────────────────────────────────
section "7. Unused dependency detection"

if command -v cargo-udeps &>/dev/null || cargo udeps --version &>/dev/null 2>&1; then
    echo ""
    if cargo +nightly udeps --all-targets 2>&1; then
        pass "No unused dependencies detected"
        report_add "unused_deps=none"
    else
        warn "Potentially unused dependencies found — review and remove if confirmed"
        report_add "unused_deps=found"
    fi
else
    info "Install cargo-udeps for unused dependency detection:"
    info "  cargo install cargo-udeps"
    info "  cargo +nightly udeps --all-targets"
    report_add "unused_deps=skipped"
fi

# ── 8. Maintainer response guide ──────────────────────────────────────────────
section "8. Maintainer response guide"

echo ""
echo "  ┌─ When cargo audit reports a vulnerability ─────────────────────────────┐"
echo "  │  1. Check if the affected code path is reachable in AnchorKit.         │"
echo "  │  2. Check the advisory for a patched version of the crate.             │"
echo "  │  3. Open a dedicated PR to update the dependency.                      │"
echo "  │  4. If no fix is available and the code path is not reachable,         │"
echo "  │     add an 'ignore' entry in audit.toml with a justification comment.  │"
echo "  │  5. Never merge a PR with an unacknowledged HIGH/CRITICAL advisory.    │"
echo "  └────────────────────────────────────────────────────────────────────────┘"
echo ""
echo "  ┌─ When a new dependency is added ───────────────────────────────────────┐"
echo "  │  1. Verify the crate is actively maintained (recent commits/releases). │"
echo "  │  2. Check for prior advisories on https://rustsec.org.                 │"
echo "  │  3. Prefer an exact version pin ('= \"x.y.z\"') in Cargo.toml.          │"
echo "  │  4. If the crate is a transitive dep of soroban-sdk, bump soroban-sdk  │"
echo "  │     only through the contract upgrade procedure in governance.md.      │"
echo "  │  5. Submit the dependency change as a standalone PR for easy review.   │"
echo "  └────────────────────────────────────────────────────────────────────────┘"

# ── JSON report ────────────────────────────────────────────────────────────────
if $REPORT; then
    REPORT_FILE="dependency-audit-report.json"
    {
        echo "{"
        echo "  \"date\": \"$(date -u '+%Y-%m-%dT%H:%M:%SZ')\","
        echo "  \"commit\": \"$(git rev-parse HEAD 2>/dev/null || echo 'unknown')\","
        echo "  \"failures\": ${FAILURES},"
        for kv in "${REPORT_LINES[@]}"; do
            key="${kv%%=*}"
            val="${kv#*=}"
            # Quote non-numeric values
            if [[ "$val" =~ ^[0-9]+$ ]]; then
                echo "  \"${key}\": ${val},"
            else
                echo "  \"${key}\": \"${val}\","
            fi
        done
        echo "  \"policy_max_deps\": ${MAX_DEPS}"
        echo "}"
    } > "${REPORT_FILE}"
    echo ""
    info "Machine-readable report written to: ${REPORT_FILE}"
fi

# ── Summary ────────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}════════════════════════════════════════════${NC}"
if [[ "${FAILURES}" -eq 0 ]]; then
    echo -e "${GREEN}${BOLD}✅ Dependency audit complete — no policy violations found.${NC}"
    exit 0
else
    echo -e "${RED}${BOLD}❌ Dependency audit found ${FAILURES} violation(s) — review output above.${NC}"
    echo ""
    echo "  See docs/governance-and-security.md for the full response process."
    exit 1
fi
