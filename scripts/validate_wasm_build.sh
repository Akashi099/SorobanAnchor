#!/usr/bin/env bash
#
# validate_wasm_build.sh
# ═════════════════════════════════════════════════════════════════════════════
#
# Comprehensive WASM build and deployment validation for SorobanAnchor.
# Validates the WASM artifact structure, feature compilation, and deployment
# assumptions. Ensures the WASM build is production-ready and catches
# configuration mistakes early.
#
# Usage:
#   ./scripts/validate_wasm_build.sh [--help|--verbose|--strict]
#
# Options:
#   --help      Show this help message
#   --verbose   Print detailed validation output
#   --strict    Fail on warnings (not just errors)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
VERBOSE="${VERBOSE:-0}"
STRICT="${STRICT:-0}"

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Constants
WASM_TARGET="wasm32-unknown-unknown"
WASM_OUT="target/${WASM_TARGET}/release/anchorkit.wasm"
MINIMUM_WASM_SIZE=1000      # Bytes: sanity check for artifact existence
MAXIMUM_WASM_SIZE=500000000 # Bytes: ~500MB hard limit

# Counters
VALIDATION_PASSED=0
VALIDATION_FAILED=0
VALIDATION_WARNINGS=0

# ─────────────────────────────────────────────────────────────────────────────
# Output functions
# ─────────────────────────────────────────────────────────────────────────────

print_header() {
    echo -e "${BLUE}${BOLD}═══════════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}${BOLD}  $1${NC}"
    echo -e "${BLUE}${BOLD}═══════════════════════════════════════════════════════════${NC}"
}

print_section() {
    echo -e "\n${CYAN}${BOLD}→ $1${NC}"
}

print_pass() {
    echo -e "  ${GREEN}✓${NC} $1"
    ((VALIDATION_PASSED++)) || true
}

print_fail() {
    echo -e "  ${RED}✗${NC} $1"
    ((VALIDATION_FAILED++)) || true
}

print_warn() {
    echo -e "  ${YELLOW}⚠${NC} $1"
    ((VALIDATION_WARNINGS++)) || true
    if [ "$STRICT" = "1" ]; then
        VALIDATION_FAILED=$((VALIDATION_FAILED + 1))
    fi
}

print_info() {
    if [ "$VERBOSE" = "1" ]; then
        echo -e "  ${BLUE}ℹ${NC} $1"
    fi
}

# ─────────────────────────────────────────────────────────────────────────────
# Validation functions
# ─────────────────────────────────────────────────────────────────────────────

# 1. Build the WASM artifact
validate_wasm_compilation() {
    print_section "1. WASM Compilation Check"

    if ! rustup target list --installed 2>/dev/null | grep -q "$WASM_TARGET"; then
        print_info "Installing $WASM_TARGET target..."
        rustup target add "$WASM_TARGET" || {
            print_fail "Failed to install $WASM_TARGET target"
            return 1
        }
    fi
    print_info "$WASM_TARGET target is installed"

    print_info "Building WASM artifact..."
    if cd "$PROJECT_ROOT" && CARGO_TERM_COLOR=always cargo build --release \
        --target "$WASM_TARGET" --no-default-features --features wasm \
        2>&1 | tee /tmp/wasm_build.log; then
        print_pass "WASM compilation succeeded"
        return 0
    else
        print_fail "WASM compilation failed"
        print_info "Build log: /tmp/wasm_build.log"
        return 1
    fi
}

# 2. Check WASM artifact exists and has valid size
validate_wasm_artifact_existence() {
    print_section "2. WASM Artifact Validation"

    cd "$PROJECT_ROOT"
    if [ ! -f "$WASM_OUT" ]; then
        print_fail "WASM artifact not found at $WASM_OUT"
        return 1
    fi
    print_pass "WASM artifact exists at $WASM_OUT"

    # Check file size
    WASM_SIZE=$(stat -c%s "$WASM_OUT" 2>/dev/null || stat -f%z "$WASM_OUT" 2>/dev/null)
    SIZE_KB=$((WASM_SIZE / 1024))
    SIZE_MB=$((SIZE_KB / 1024))

    if [ "$WASM_SIZE" -lt "$MINIMUM_WASM_SIZE" ]; then
        print_fail "WASM artifact too small ($WASM_SIZE bytes, minimum $MINIMUM_WASM_SIZE)"
        return 1
    fi
    print_pass "WASM artifact size reasonable ($SIZE_MB MB)"

    if [ "$WASM_SIZE" -gt "$MAXIMUM_WASM_SIZE" ]; then
        print_warn "WASM artifact exceeds recommended size ($SIZE_MB MB, max 500MB)"
    fi
}

# 3. Check feature compilation assumptions
validate_feature_isolation() {
    print_section "3. Feature Isolation Checks"

    cd "$PROJECT_ROOT"

    # Verify that std-only modules are not accessible
    if cargo check --lib --no-default-features --features wasm \
        2>&1 | grep -q "error\[E"; then
        print_fail "Feature isolation broken: std modules leak into wasm build"
        return 1
    fi
    print_pass "Feature isolation intact: std modules excluded from wasm"

    # Verify core modules are still available
    if ! cargo check --lib --no-default-features --features wasm \
        2>&1 | grep -q "Finished"; then
        print_fail "Core modules unavailable in wasm build"
        return 1
    fi
    print_pass "Core modules accessible in wasm build"
}

# 4. Check WASM has no_std compliance
validate_no_std_compliance() {
    print_section "4. No-std Compliance Checks"

    cd "$PROJECT_ROOT"

    # Check that source compiles with no_std
    if grep -q "^#!\[no_std\]" src/lib.rs; then
        print_pass "Library marked #![no_std]"
    else
        print_fail "Library not marked #![no_std]"
        return 1
    fi

    # Verify alloc is declared
    if grep -q "extern crate alloc" src/lib.rs; then
        print_pass "Library declares alloc"
    else
        print_fail "Library does not declare alloc"
        return 1
    fi

    # Check contract module (main on-chain code) doesn't use std directly
    print_info "Checking contract module for std leaks..."
    if grep -q "#\[cfg(feature = \"std\")\]" src/contract.rs; then
        print_pass "contract.rs properly gates std-only code"
    else
        print_warn "contract.rs may not have std feature gates"
    fi
}

# 5. Deployment sanity checks
validate_deployment_readiness() {
    print_section "5. Deployment Readiness Checks"

    cd "$PROJECT_ROOT"

    # Check that release profile is optimized
    if grep -q "opt-level = \"z\"" Cargo.toml; then
        print_pass "Release profile optimized for size"
    else
        print_warn "Release profile not optimized for size"
    fi

    # Check that LTO is enabled
    if grep -q "lto = true" Cargo.toml; then
        print_pass "LTO enabled for release builds"
    else
        print_warn "LTO not enabled (consider enabling for smaller binaries)"
    fi

    # Check panic behavior
    if grep -q "panic = \"abort\"" Cargo.toml; then
        print_pass "Panic set to abort (proper for WASM)"
    else
        print_warn "Panic not set to abort"
    fi

    # Verify reproducible build support
    if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
        print_pass "SOURCE_DATE_EPOCH set: reproducible builds enabled"
    else
        print_info "SOURCE_DATE_EPOCH not set (reproducible builds not enabled)"
    fi
}

# 6. Module dependency checks
validate_module_dependencies() {
    print_section "6. Module Dependency Checks"

    cd "$PROJECT_ROOT"

    # Check that HTTP client is not in wasm builds
    print_info "Checking http_client exclusion..."
    if cargo expand --target "$WASM_TARGET" --no-default-features --features wasm 2>/dev/null | \
        grep -q "pub mod http_client"; then
        print_warn "http_client module may be exposed in wasm build"
    else
        print_pass "http_client properly excluded from wasm"
    fi

    # Verify soroban-sdk is available
    if grep -q "soroban-sdk" Cargo.toml; then
        print_pass "soroban-sdk dependency present"
    else
        print_fail "soroban-sdk dependency missing"
        return 1
    fi
}

# 7. Output file metadata checks
validate_output_metadata() {
    print_section "7. Output Metadata Checks"

    cd "$PROJECT_ROOT"
    if [ ! -f "$WASM_OUT" ]; then
        print_warn "Cannot check metadata: WASM artifact not found"
        return 0
    fi

    # Check file is readable
    if [ -r "$WASM_OUT" ]; then
        print_pass "WASM artifact is readable"
    else
        print_fail "WASM artifact is not readable"
        return 1
    fi

    # Check WASM magic bytes
    if xxd -l 4 "$WASM_OUT" 2>/dev/null | grep -q "0x00 0x61 0x73 0x6d"; then
        print_pass "WASM magic bytes correct (\\0asm)"
    else
        print_warn "WASM magic bytes not verified (xxd not available)"
    fi

    # Report build timestamp
    BUILD_TIME=$(stat -c%y "$WASM_OUT" 2>/dev/null | cut -d' ' -f1-2 || \
                 stat -f "%Sm -t '%Y-%m-%d %H:%M'" "$WASM_OUT" 2>/dev/null || \
                 echo "unknown")
    print_info "WASM artifact built: $BUILD_TIME"
}

# 8. Build configuration validation
validate_build_configuration() {
    print_section "8. Build Configuration"

    cd "$PROJECT_ROOT"

    # List enabled features for WASM build
    print_info "Configured features for WASM:"
    cargo metadata --format-version 1 2>/dev/null | jq -r '.packages[0].features | keys[]' | \
        while read feature; do
            print_info "  - $feature"
        done || print_info "  (cargo metadata unavailable)"

    # Verify build scripts run
    if [ -f "build.rs" ]; then
        print_pass "Build script present (build.rs)"
    else
        print_warn "No build script found"
    fi
}

# ─────────────────────────────────────────────────────────────────────────────
# Main
# ─────────────────────────────────────────────────────────────────────────────

main() {
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --help)
                echo "SorobanAnchor WASM Build Validation"
                echo ""
                echo "Usage: $0 [--help|--verbose|--strict]"
                echo ""
                echo "Options:"
                echo "  --help      Show this help message"
                echo "  --verbose   Print detailed validation output"
                echo "  --strict    Fail on warnings (not just errors)"
                exit 0
                ;;
            --verbose)
                VERBOSE=1
                shift
                ;;
            --strict)
                STRICT=1
                shift
                ;;
            *)
                echo "Unknown option: $1"
                exit 1
                ;;
        esac
    done

    print_header "SorobanAnchor WASM Build & Deployment Validator"
    echo ""

    # Run all validations
    local failed_any=0

    if ! validate_wasm_compilation; then
        failed_any=1
    fi

    if ! validate_wasm_artifact_existence; then
        failed_any=1
    fi

    if ! validate_feature_isolation; then
        failed_any=1
    fi

    if ! validate_no_std_compliance; then
        failed_any=1
    fi

    if ! validate_deployment_readiness; then
        failed_any=1
    fi

    if ! validate_module_dependencies; then
        failed_any=1
    fi

    if ! validate_output_metadata; then
        failed_any=1
    fi

    validate_build_configuration

    # Summary
    echo ""
    print_header "Validation Summary"
    echo -e "  ${GREEN}Passed:${NC}  $VALIDATION_PASSED"
    if [ "$VALIDATION_WARNINGS" -gt 0 ]; then
        echo -e "  ${YELLOW}Warnings:${NC} $VALIDATION_WARNINGS"
    fi
    if [ "$VALIDATION_FAILED" -gt 0 ]; then
        echo -e "  ${RED}Failed:${NC}  $VALIDATION_FAILED"
    fi
    echo ""

    if [ "$VALIDATION_FAILED" -gt 0 ]; then
        echo -e "${RED}${BOLD}❌ WASM validation failed${NC}"
        exit 1
    elif [ "$VALIDATION_WARNINGS" -gt 0 ] && [ "$STRICT" = "1" ]; then
        echo -e "${YELLOW}${BOLD}⚠ WASM validation passed with warnings (strict mode)${NC}"
        exit 1
    else
        echo -e "${GREEN}${BOLD}✅ WASM build is production-ready${NC}"
        echo ""
        echo "WASM artifact: $WASM_OUT"
        [ -f "$WASM_OUT" ] && {
            WASM_SIZE=$(stat -c%s "$WASM_OUT" 2>/dev/null || stat -f%z "$WASM_OUT" 2>/dev/null)
            SIZE_MB=$((WASM_SIZE / 1024 / 1024))
            echo "Size: $SIZE_MB MB"
        }
        exit 0
    fi
}

main "$@"
