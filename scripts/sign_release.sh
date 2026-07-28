#!/usr/bin/env bash
# sign_release.sh — Sign SorobanAnchor release artifacts with GPG or minisign.
#
# Usage:
#   ./scripts/sign_release.sh [VERSION]
#
# Arguments:
#   VERSION   Optional semver string. Defaults to the value in Cargo.toml.
#
# Required tools (at least one signing backend must be available):
#   gpg       — used when ANCHORKIT_SIGNING_BACKEND=gpg (default)
#   minisign  — used when ANCHORKIT_SIGNING_BACKEND=minisign
#
# Environment variables:
#   ANCHORKIT_SIGNING_BACKEND   gpg | minisign   (default: gpg)
#   ANCHORKIT_GPG_KEY_ID        GPG key fingerprint / email used for signing
#                               (required for gpg backend, otherwise uses the
#                               default GPG key)
#   ANCHORKIT_MINISIGN_KEY      Path to the minisign secret key
#                               (default: ~/.minisign/minisign.key)
#
# Outputs:
#   dist/anchorkit-<VERSION>.tar.gz.sig         — detached signature (GPG armor)
#   dist/anchorkit-<VERSION>.tar.gz.minisig     — minisign signature
#   dist/anchorkit-<VERSION>.sha256             — SHA-256 checksum
#   dist/anchorkit-<VERSION>.sha256.sig         — signed checksum file
#
# Verification:
#   Use verify_release.sh (in this directory) to verify any release bundle.
#
# Dry-run support:
#   Pass --dry-run as an extra argument to print the commands that would be
#   executed without actually signing anything.

set -euo pipefail

# ── Argument parsing ──────────────────────────────────────────────────────────
DRY_RUN=false
VERSION_ARG=""
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=true ;;
    *) VERSION_ARG="$arg" ;;
  esac
done

# ── Resolve version ───────────────────────────────────────────────────────────
CARGO_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')
VERSION="${VERSION_ARG:-$CARGO_VERSION}"
DIST_DIR="dist"
BUNDLE_DIR="${DIST_DIR}/anchorkit-${VERSION}"
TARBALL="${DIST_DIR}/anchorkit-${VERSION}.tar.gz"
CHECKSUM_FILE="${DIST_DIR}/anchorkit-${VERSION}.sha256"

# ── Backend selection ─────────────────────────────────────────────────────────
BACKEND="${ANCHORKIT_SIGNING_BACKEND:-gpg}"

echo "=== SorobanAnchor Release Signing ==="
echo "    Version : ${VERSION}"
echo "    Backend : ${BACKEND}"
echo "    Dry-run : ${DRY_RUN}"
echo ""

# ── Check that artifacts exist ────────────────────────────────────────────────
if [[ ! -f "${TARBALL}" ]]; then
  echo "ERROR: tarball not found: ${TARBALL}"
  echo "Run 'make release' first to build the artifacts."
  exit 1
fi

# ── Produce / update SHA-256 checksum ─────────────────────────────────────────
echo "[1/3] Generating SHA-256 checksum …"
if [[ "$DRY_RUN" == "true" ]]; then
  echo "      [dry-run] would write ${CHECKSUM_FILE}"
else
  if command -v sha256sum &>/dev/null; then
    sha256sum "${TARBALL}" > "${CHECKSUM_FILE}"
  elif command -v shasum &>/dev/null; then
    shasum -a 256 "${TARBALL}" > "${CHECKSUM_FILE}"
  else
    echo "ERROR: neither sha256sum nor shasum found."
    exit 1
  fi
  echo "      ${CHECKSUM_FILE}"
fi

# ── Sign artifacts ────────────────────────────────────────────────────────────
echo ""
echo "[2/3] Signing artifacts (${BACKEND}) …"

GPG_KEY_ARGS=()
if [[ -n "${ANCHORKIT_GPG_KEY_ID:-}" ]]; then
  GPG_KEY_ARGS=(--local-user "${ANCHORKIT_GPG_KEY_ID}")
fi

sign_file() {
  local file="$1"
  local sig_file=""
  case "${BACKEND}" in
    gpg)
      sig_file="${file}.sig"
      if [[ "$DRY_RUN" == "true" ]]; then
        echo "      [dry-run] gpg --armor --detach-sign ${GPG_KEY_ARGS[*]:-} -o ${sig_file} ${file}"
      else
        gpg --armor --detach-sign "${GPG_KEY_ARGS[@]}" -o "${sig_file}" "${file}"
        echo "      ${sig_file}"
      fi
      ;;
    minisign)
      local key_path="${ANCHORKIT_MINISIGN_KEY:-${HOME}/.minisign/minisign.key}"
      sig_file="${file}.minisig"
      if [[ "$DRY_RUN" == "true" ]]; then
        echo "      [dry-run] minisign -Sm ${file} -s ${key_path}"
      else
        if ! command -v minisign &>/dev/null; then
          echo "ERROR: minisign not found. Install it from https://jedisct1.github.io/minisign/"
          exit 1
        fi
        minisign -Sm "${file}" -s "${key_path}"
        echo "      ${sig_file}"
      fi
      ;;
    *)
      echo "ERROR: Unknown signing backend '${BACKEND}'. Set ANCHORKIT_SIGNING_BACKEND to 'gpg' or 'minisign'."
      exit 1
      ;;
  esac
}

sign_file "${TARBALL}"
sign_file "${CHECKSUM_FILE}"

# ── List outputs ──────────────────────────────────────────────────────────────
echo ""
echo "[3/3] Signed artifacts:"
echo ""
case "${BACKEND}" in
  gpg)
    [[ "$DRY_RUN" == "false" ]] && ls -lh \
      "${TARBALL}" "${TARBALL}.sig" \
      "${CHECKSUM_FILE}" "${CHECKSUM_FILE}.sig" 2>/dev/null || true
    ;;
  minisign)
    [[ "$DRY_RUN" == "false" ]] && ls -lh \
      "${TARBALL}" "${TARBALL}.minisig" \
      "${CHECKSUM_FILE}" "${CHECKSUM_FILE}.minisig" 2>/dev/null || true
    ;;
esac

echo ""
echo "=== Signing complete ==="
echo ""
echo "Distribute the following files alongside your release:"
echo "  ${TARBALL}"
echo "  ${CHECKSUM_FILE}"
case "${BACKEND}" in
  gpg)
    echo "  ${TARBALL}.sig"
    echo "  ${CHECKSUM_FILE}.sig"
    echo ""
    echo "Verification command:"
    echo "  ./scripts/verify_release.sh dist/anchorkit-${VERSION}.tar.gz"
    ;;
  minisign)
    echo "  ${TARBALL}.minisig"
    echo "  ${CHECKSUM_FILE}.minisig"
    echo ""
    echo "Verification command:"
    echo "  ANCHORKIT_SIGNING_BACKEND=minisign ./scripts/verify_release.sh dist/anchorkit-${VERSION}.tar.gz"
    ;;
esac
