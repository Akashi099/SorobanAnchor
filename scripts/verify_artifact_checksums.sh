#!/usr/bin/env bash
# verify_artifact_checksums.sh — Verify release artifact checksums.
#
# Usage:
#   ./scripts/verify_artifact_checksums.sh                   # auto-detect version from Cargo.toml
#   ./scripts/verify_artifact_checksums.sh 0.1.0             # explicit version
#   ./scripts/verify_artifact_checksums.sh --generate        # generate new checksums for current build
#   ./scripts/verify_artifact_checksums.sh --generate 0.1.0  # generate for specific version
#
# In verify mode (default) the script:
#   1. Locates the release tarball and checksum file under dist/.
#   2. Verifies the tarball checksum against the recorded .sha256 file.
#   3. Extracts the bundle and independently checksums each artifact inside it.
#   4. Prints a pass/fail summary for every artifact.
#
# In --generate mode the script recomputes and writes checksum files for all
# artifacts in an existing bundle directory. Use after `make release` to
# capture a known-good baseline.
#
# Exit codes:
#   0 — all checks passed
#   1 — one or more checks failed

set -euo pipefail

# ── Colour helpers ─────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

pass()    { echo -e "  ${GREEN}✓${NC}  $*"; }
fail()    { echo -e "  ${RED}✗${NC}  $*"; FAILURES=$((FAILURES + 1)); }
warn()    { echo -e "  ${YELLOW}⚠${NC}  $*"; }
section() { echo -e "\n${BOLD}${CYAN}$*${NC}"; }

# ── Argument parsing ───────────────────────────────────────────────────────────
GENERATE=false
VERSION=""

for arg in "$@"; do
    case "$arg" in
        --generate) GENERATE=true ;;
        --*)        echo "Unknown flag: $arg" >&2; exit 1 ;;
        *)          VERSION="$arg" ;;
    esac
done

if [[ -z "$VERSION" ]]; then
    VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')
fi

DIST_DIR="dist"
BUNDLE_DIR="${DIST_DIR}/anchorkit-${VERSION}"
TARBALL="${DIST_DIR}/anchorkit-${VERSION}.tar.gz"
CHECKSUM_FILE="${DIST_DIR}/anchorkit-${VERSION}.sha256"
ARTIFACT_CHECKSUMS="${BUNDLE_DIR}/CHECKSUMS.sha256"

FAILURES=0

# ── sha256 helper (cross-platform) ────────────────────────────────────────────
sha256_file() {
    if command -v sha256sum &>/dev/null; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum &>/dev/null; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        echo "ERROR: neither sha256sum nor shasum found" >&2
        exit 1
    fi
}

sha256_write() {
    # Write "<hash>  <filename>" to stdout in sha256sum-compatible format.
    local file="$1"
    local hash
    hash=$(sha256_file "$file")
    echo "${hash}  ${file}"
}

# ── Generate mode ──────────────────────────────────────────────────────────────
if $GENERATE; then
    echo ""
    echo -e "${BOLD}=== Artifact Checksum Generation ===${NC}"
    echo "    Version      : ${VERSION}"
    echo "    Bundle dir   : ${BUNDLE_DIR}"
    echo ""

    if [[ ! -d "${BUNDLE_DIR}" ]]; then
        echo "Bundle directory not found: ${BUNDLE_DIR}" >&2
        echo "Run 'make release' first." >&2
        exit 1
    fi

    # Generate per-artifact checksum manifest inside the bundle.
    section "Generating per-artifact checksums → ${ARTIFACT_CHECKSUMS}"
    {
        find "${BUNDLE_DIR}" -type f ! -name 'CHECKSUMS.sha256' | sort | while read -r f; do
            hash=$(sha256_file "$f")
            rel="${f#${BUNDLE_DIR}/}"
            echo "${hash}  ${rel}"
        done
    } > "${ARTIFACT_CHECKSUMS}"
    echo "  Written: ${ARTIFACT_CHECKSUMS}"
    cat "${ARTIFACT_CHECKSUMS}" | sed 's/^/    /'

    # Generate / overwrite the tarball checksum if the tarball exists.
    if [[ -f "${TARBALL}" ]]; then
        section "Generating tarball checksum → ${CHECKSUM_FILE}"
        sha256_write "${TARBALL}" > "${CHECKSUM_FILE}"
        echo "  Written: ${CHECKSUM_FILE}"
        cat "${CHECKSUM_FILE}" | sed 's/^/    /'
    else
        warn "Tarball not found (${TARBALL}); skipping tarball checksum."
        warn "Run 'make release' to produce the tarball, then re-run with --generate."
    fi

    echo ""
    echo -e "${GREEN}${BOLD}✅ Checksum files generated.${NC}"
    echo "Commit ${ARTIFACT_CHECKSUMS} and ${CHECKSUM_FILE} to the release tag."
    exit 0
fi

# ── Verify mode ────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}=== Artifact Checksum Verification ===${NC}"
echo "    Version      : ${VERSION}"
echo "    Tarball      : ${TARBALL}"
echo "    Checksum file: ${CHECKSUM_FILE}"
echo ""

# Step 1: Verify tarball checksum.
section "Step 1: Tarball integrity"

if [[ ! -f "${TARBALL}" ]]; then
    fail "Tarball not found: ${TARBALL}"
    echo ""
    echo -e "${RED}${BOLD}❌ Verification failed — no tarball to check.${NC}"
    echo "Run 'make release' to build the release artifacts."
    exit 1
fi

if [[ ! -f "${CHECKSUM_FILE}" ]]; then
    warn "Checksum file not found: ${CHECKSUM_FILE}"
    warn "Run with --generate after building to create a baseline."
else
    RECORDED_HASH=$(awk '{print $1}' "${CHECKSUM_FILE}")
    ACTUAL_HASH=$(sha256_file "${TARBALL}")
    if [[ "${RECORDED_HASH}" == "${ACTUAL_HASH}" ]]; then
        pass "Tarball checksum matches recorded value (${ACTUAL_HASH:0:16}...)"
    else
        fail "Tarball checksum MISMATCH"
        echo "      Recorded : ${RECORDED_HASH}"
        echo "      Actual   : ${ACTUAL_HASH}"
    fi
fi

# Step 2: Extract and verify individual artifact checksums.
section "Step 2: Per-artifact checksums"

EXTRACT_DIR=$(mktemp -d)
cleanup() { rm -rf "${EXTRACT_DIR}"; }
trap cleanup EXIT

echo "  Extracting ${TARBALL}..."
tar -xzf "${TARBALL}" -C "${EXTRACT_DIR}" --strip-components=1

if [[ ! -f "${EXTRACT_DIR}/CHECKSUMS.sha256" ]]; then
    warn "No CHECKSUMS.sha256 found in bundle."
    warn "Run with --generate to create one, then re-release."
else
    echo "  Verifying per-artifact checksums from CHECKSUMS.sha256..."
    echo ""
    while IFS= read -r line; do
        [[ -z "${line}" ]] && continue
        EXPECTED_HASH=$(echo "${line}" | awk '{print $1}')
        REL_PATH=$(echo "${line}" | awk '{print $2}')
        FULL_PATH="${EXTRACT_DIR}/${REL_PATH}"
        if [[ ! -f "${FULL_PATH}" ]]; then
            fail "${REL_PATH} — file missing from bundle"
        else
            ACTUAL=$(sha256_file "${FULL_PATH}")
            if [[ "${EXPECTED_HASH}" == "${ACTUAL}" ]]; then
                pass "${REL_PATH}"
            else
                fail "${REL_PATH} — checksum mismatch"
                echo "        Expected : ${EXPECTED_HASH}"
                echo "        Actual   : ${ACTUAL}"
            fi
        fi
    done < "${EXTRACT_DIR}/CHECKSUMS.sha256"
fi

# Step 3: Verify WASM magic bytes as a sanity check.
section "Step 3: WASM artifact sanity"

WASM_PATH="${EXTRACT_DIR}/anchorkit.wasm"
if [[ ! -f "${WASM_PATH}" ]]; then
    fail "anchorkit.wasm not found in bundle"
else
    if command -v xxd &>/dev/null; then
        MAGIC=$(xxd -l 4 "${WASM_PATH}" | awk '{print $2$3}' | head -1)
        if [[ "${MAGIC}" == "0061736d" ]]; then
            WASM_SIZE=$(du -sh "${WASM_PATH}" | cut -f1)
            pass "anchorkit.wasm has valid WASM magic bytes (size: ${WASM_SIZE})"
        else
            fail "anchorkit.wasm does not have valid WASM magic bytes (got: ${MAGIC})"
        fi
    else
        warn "xxd not available — skipping WASM magic byte check"
        pass "anchorkit.wasm present"
    fi
fi

# Step 4: Verify CLI binary presence and executability.
section "Step 4: CLI binary sanity"

CLI_PATH="${EXTRACT_DIR}/anchorkit"
if [[ ! -f "${CLI_PATH}" ]]; then
    fail "anchorkit CLI binary not found in bundle"
elif [[ -x "${CLI_PATH}" ]]; then
    CLI_SIZE=$(du -sh "${CLI_PATH}" | cut -f1)
    pass "anchorkit CLI binary is executable (size: ${CLI_SIZE})"
else
    warn "anchorkit CLI binary present but not executable"
    pass "anchorkit CLI binary present"
fi

# ── Summary ────────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}────────────────────────────────────────${NC}"
if [[ "${FAILURES}" -eq 0 ]]; then
    echo -e "${GREEN}${BOLD}✅ All artifact checksum checks passed.${NC}"
    exit 0
else
    echo -e "${RED}${BOLD}❌ ${FAILURES} artifact checksum check(s) failed — review output above.${NC}"
    exit 1
fi
