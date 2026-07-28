#!/usr/bin/env bash
# package_release.sh — Build and bundle all SorobanAnchor release artifacts.
#
# Usage:
#   ./scripts/package_release.sh [VERSION]
#
# Arguments:
#   VERSION   Optional semver string (e.g. "0.2.0"). Defaults to the value in
#             Cargo.toml if not supplied.
#
# Outputs:
#   dist/anchorkit-<VERSION>.tar.gz   — release tarball
#   dist/anchorkit-<VERSION>/         — unpacked artifact directory
#
# Required tools:
#   cargo, rustup (with wasm32-unknown-unknown target), tar
#
# Optional tools:
#   wasm-opt (binaryen) — optimizes the WASM artifact if available
#   sha256sum / shasum  — generates a checksum file

set -euo pipefail

# ── Resolve version ──────────────────────────────────────────────────────────
CARGO_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')
VERSION="${1:-$CARGO_VERSION}"
DIST_DIR="dist"
BUNDLE_DIR="${DIST_DIR}/anchorkit-${VERSION}"
TARBALL="${DIST_DIR}/anchorkit-${VERSION}.tar.gz"
WASM_TARGET="wasm32-unknown-unknown"
WASM_OUT="target/${WASM_TARGET}/release/anchorkit.wasm"
WASM_OPT_OUT="${BUNDLE_DIR}/anchorkit.opt.wasm"

echo "=== SorobanAnchor Release Packaging ==="
echo "    Version : ${VERSION}"
echo "    Bundle  : ${BUNDLE_DIR}"
echo "    Tarball : ${TARBALL}"
echo ""

# ── Step 1: Ensure WASM target is installed ──────────────────────────────────
echo "[1/7] Checking wasm32-unknown-unknown target..."
if ! rustup target list --installed | grep -q "${WASM_TARGET}"; then
    echo "      Installing ${WASM_TARGET}..."
    rustup target add "${WASM_TARGET}"
fi
echo "      OK"

# ── Step 2: Build native CLI binary ─────────────────────────────────────────
echo "[2/7] Building native CLI binary (release)..."
cargo build --release
echo "      OK: target/release/anchorkit"

# ── Step 3: Build WASM contract ──────────────────────────────────────────────
echo "[3/7] Building WASM contract..."
cargo build --release \
    --target "${WASM_TARGET}" \
    --no-default-features \
    --features wasm
echo "      OK: ${WASM_OUT}"
echo "      Raw size: $(du -sh "${WASM_OUT}" | cut -f1)"

# ── Step 4: Optimize WASM (optional) ────────────────────────────────────────
echo "[4/7] Optimizing WASM (wasm-opt)..."
if command -v wasm-opt &>/dev/null; then
    wasm-opt -Oz --strip-debug "${WASM_OUT}" -o "${WASM_OUT}.opt"
    echo "      Optimized size: $(du -sh "${WASM_OUT}.opt" | cut -f1)"
    WASM_FINAL="${WASM_OUT}.opt"
else
    echo "      wasm-opt not found — skipping optimization (install binaryen for smaller output)"
    WASM_FINAL="${WASM_OUT}"
fi

# ── Step 5: Assemble bundle directory ────────────────────────────────────────
echo "[5/7] Assembling bundle at ${BUNDLE_DIR}..."
rm -rf "${BUNDLE_DIR}"
mkdir -p "${BUNDLE_DIR}/configs"
mkdir -p "${BUNDLE_DIR}/docs"
mkdir -p "${BUNDLE_DIR}/schemas"

# CLI binary
cp "target/release/anchorkit" "${BUNDLE_DIR}/anchorkit"

# WASM contract
cp "${WASM_FINAL}" "${BUNDLE_DIR}/anchorkit.wasm"

# Schema files
cp "config_schema.json" "${BUNDLE_DIR}/schemas/config_schema.json"

# Example configs
cp configs/*.json "${BUNDLE_DIR}/configs/" 2>/dev/null || true
cp configs/*.toml "${BUNDLE_DIR}/configs/" 2>/dev/null || true

# Documentation
cp README.md "${BUNDLE_DIR}/README.md"
cp LICENSE   "${BUNDLE_DIR}/LICENSE"
cp -r docs/* "${BUNDLE_DIR}/docs/" 2>/dev/null || true

# Write a VERSION file
echo "${VERSION}" > "${BUNDLE_DIR}/VERSION"

echo "      Bundle contents:"
find "${BUNDLE_DIR}" -type f | sort | sed 's/^/        /'

# ── Step 6: Create tarball ───────────────────────────────────────────────────
echo "[6/7] Creating tarball ${TARBALL}..."
mkdir -p "${DIST_DIR}"
tar -czf "${TARBALL}" -C "${DIST_DIR}" "anchorkit-${VERSION}"
echo "      Tarball size: $(du -sh "${TARBALL}" | cut -f1)"

# ── Step 7: Generate checksum ────────────────────────────────────────────────
echo "[7/7] Generating checksum..."
CHECKSUM_FILE="${DIST_DIR}/anchorkit-${VERSION}.sha256"
if command -v sha256sum &>/dev/null; then
    sha256sum "${TARBALL}" > "${CHECKSUM_FILE}"
elif command -v shasum &>/dev/null; then
    shasum -a 256 "${TARBALL}" > "${CHECKSUM_FILE}"
else
    echo "      sha256sum/shasum not found — skipping checksum"
    CHECKSUM_FILE=""
fi
if [[ -n "${CHECKSUM_FILE}" ]]; then
    echo "      Checksum: $(cat "${CHECKSUM_FILE}")"
fi

echo ""
echo "=== Release packaging complete ==="
echo "    Tarball  : ${TARBALL}"
[[ -n "${CHECKSUM_FILE:-}" ]] && echo "    Checksum : ${CHECKSUM_FILE}"
echo ""
echo "Artifact manifest:"
echo "  anchorkit              — CLI binary"
echo "  anchorkit.wasm         — Soroban WASM contract"
echo "  schemas/config_schema.json — JSON schema for anchor configs"
echo "  configs/               — Example anchor configurations"
echo "  docs/                  — Documentation"
echo "  README.md              — Project documentation"
echo "  LICENSE                — MIT license"
echo "  VERSION                — Release version string"
echo ""
echo "Next steps:"
echo "  Sign artifacts : bash scripts/sign_release.sh ${VERSION}"
echo "  Verify bundle  : bash scripts/verify_release.sh ${TARBALL}"
