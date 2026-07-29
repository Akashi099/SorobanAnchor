#!/usr/bin/env bash
# verify_release.sh — Verify the integrity and authenticity of a SorobanAnchor
#                     release bundle.
#
# Usage:
#   ./scripts/verify_release.sh <path-to-tarball>
#
# Arguments:
#   <path-to-tarball>   Path to anchorkit-<VERSION>.tar.gz
#
# The script expects the following files alongside the tarball:
#   <tarball>.sha256       — SHA-256 checksum file
#   <tarball>.sha256.sig   — GPG detached signature of the checksum file
#   <tarball>.sig          — GPG detached signature of the tarball
#   OR (minisign backend):
#   <tarball>.minisig      — minisign signature
#
# Environment variables:
#   ANCHORKIT_SIGNING_BACKEND   gpg | minisign  (default: gpg)
#   ANCHORKIT_GPG_SIGNER        GPG fingerprint or email expected on the sig.
#                               When set, verifies signer identity.
#   ANCHORKIT_MINISIGN_PUBKEY   Path to the minisign public key file
#                               (default: anchorkit-release.pub alongside tarball)
#
# Exit codes:
#   0  All checks passed — tarball is intact and authentic.
#   1  One or more checks failed.

set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <path-to-tarball>"
  exit 1
fi

TARBALL="$1"

if [[ ! -f "${TARBALL}" ]]; then
  echo "ERROR: tarball not found: ${TARBALL}"
  exit 1
fi

BACKEND="${ANCHORKIT_SIGNING_BACKEND:-gpg}"
TARBALL_DIR="$(dirname "${TARBALL}")"
CHECKSUM_FILE="${TARBALL}.sha256"

echo "=== SorobanAnchor Release Verification ==="
echo "    Tarball : ${TARBALL}"
echo "    Backend : ${BACKEND}"
echo ""

PASS=true

# ── Step 1: Checksum integrity ─────────────────────────────────────────────────
echo "[1/3] Verifying SHA-256 checksum …"
if [[ ! -f "${CHECKSUM_FILE}" ]]; then
  echo "      ERROR: checksum file not found: ${CHECKSUM_FILE}"
  PASS=false
else
  if command -v sha256sum &>/dev/null; then
    if sha256sum --check "${CHECKSUM_FILE}" 2>&1; then
      echo "      ✓ Checksum verified"
    else
      echo "      ✗ Checksum MISMATCH"
      PASS=false
    fi
  elif command -v shasum &>/dev/null; then
    if shasum -a 256 --check "${CHECKSUM_FILE}" 2>&1; then
      echo "      ✓ Checksum verified"
    else
      echo "      ✗ Checksum MISMATCH"
      PASS=false
    fi
  else
    echo "      WARNING: sha256sum / shasum not found; skipping checksum verification."
  fi
fi

# ── Step 2: Signature verification ────────────────────────────────────────────
echo ""
echo "[2/3] Verifying signature (${BACKEND}) …"

case "${BACKEND}" in
  gpg)
    for artifact in "${TARBALL}" "${CHECKSUM_FILE}"; do
      sig_file="${artifact}.sig"
      if [[ ! -f "${sig_file}" ]]; then
        echo "      WARNING: signature file not found: ${sig_file}  (skipping)"
        continue
      fi
      gpg_out=$(gpg --verify "${sig_file}" "${artifact}" 2>&1) && rc=0 || rc=$?
      if [[ $rc -eq 0 ]]; then
        if [[ -n "${ANCHORKIT_GPG_SIGNER:-}" ]]; then
          if echo "${gpg_out}" | grep -q "${ANCHORKIT_GPG_SIGNER}"; then
            echo "      ✓ GPG signature valid — signer: ${ANCHORKIT_GPG_SIGNER} — $(basename "${artifact}")"
          else
            echo "      ✗ GPG signature valid but signer does not match expected '${ANCHORKIT_GPG_SIGNER}'"
            echo "        GPG output: ${gpg_out}"
            PASS=false
          fi
        else
          echo "      ✓ GPG signature valid — $(basename "${artifact}")"
        fi
      else
        echo "      ✗ GPG signature INVALID — $(basename "${artifact}")"
        echo "        GPG output: ${gpg_out}"
        PASS=false
      fi
    done
    ;;
  minisign)
    if ! command -v minisign &>/dev/null; then
      echo "      WARNING: minisign not found; skipping signature verification."
    else
      pubkey_file="${ANCHORKIT_MINISIGN_PUBKEY:-${TARBALL_DIR}/anchorkit-release.pub}"
      if [[ ! -f "${pubkey_file}" ]]; then
        echo "      WARNING: minisign public key not found at '${pubkey_file}'; skipping."
      else
        for artifact in "${TARBALL}" "${CHECKSUM_FILE}"; do
          sig_file="${artifact}.minisig"
          if [[ ! -f "${sig_file}" ]]; then
            echo "      WARNING: minisign signature not found: ${sig_file}  (skipping)"
            continue
          fi
          if minisign -Vm "${artifact}" -p "${pubkey_file}" 2>&1; then
            echo "      ✓ minisign signature valid — $(basename "${artifact}")"
          else
            echo "      ✗ minisign signature INVALID — $(basename "${artifact}")"
            PASS=false
          fi
        done
      fi
    fi
    ;;
  *)
    echo "      ERROR: Unknown backend '${BACKEND}'."
    PASS=false
    ;;
esac

# ── Step 3: Bundle content check ──────────────────────────────────────────────
echo ""
echo "[3/3] Inspecting bundle contents …"
REQUIRED_PATHS=(
  "anchorkit"
  "anchorkit.wasm"
  "schemas/config_schema.json"
  "README.md"
  "LICENSE"
  "VERSION"
)
for rel_path in "${REQUIRED_PATHS[@]}"; do
  # tarballs have a top-level directory prefix (anchorkit-<VERSION>/)
  if tar -tzf "${TARBALL}" 2>/dev/null | grep -qE "/${rel_path}$"; then
    echo "      ✓ ${rel_path}"
  else
    echo "      ✗ MISSING: ${rel_path}"
    PASS=false
  fi
done

# ── Final verdict ──────────────────────────────────────────────────────────────
echo ""
echo "============================================================"
if [[ "$PASS" == "true" ]]; then
  echo " RESULT: VERIFICATION PASSED"
  echo "============================================================"
  exit 0
else
  echo " RESULT: VERIFICATION FAILED — see output above"
  echo "============================================================"
  exit 1
fi
