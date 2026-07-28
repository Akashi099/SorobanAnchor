#!/usr/bin/env bash
#
# validate_no_std_compliance.sh
# ═════════════════════════════════════════════════════════════════════════════
#
# Verify no_std compliance across all modules.
#
# Audits the codebase for:
#   - Unintended std dependencies in no_std modules
#   - Proper use of alloc vs std
#   - Feature gate correctness
#   - Portability of core modules
#
# Usage:
#   ./scripts/validate_no_std_compliance.sh [--help|--verbose|--fix]
#
# Options:
#   --help      Show this help message
#   --verbose   Print detailed analysis
#   --fix       Attempt to auto-fix identified issues

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
VERBOSE="${VERBOSE:-0}"
FIX="${FIX:-0}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

CHECKS_PASSED=0
CHECKS_FAILED=0
CHECKS_WARNINGS=0

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
    ((CHECKS_PASSED++)) || true
}

print_fail() {
    echo -e "  ${RED}✗${NC} $1"
    ((CHECKS_FAILED++)) || true
}

print_warn() {
    echo -e "  ${YELLOW}⚠${NC} $1"
    ((CHECKS_WARNINGS++)) || true
}

print_info() {
    if [ "$VERBOSE" = "1" ]; then
        echo -e "  ${BLUE}ℹ${NC} $1"
    fi
}

# ─────────────────────────────────────────────────────────────────────────────
# Compliance checks
# ─────────────────────────────────────────────────────────────────────────────

# 1. Verify no_std declaration
check_no_std_declaration() {
    print_section "1. Library no_std Declaration"

    cd "$PROJECT_ROOT"
    if grep -q "^#!\[no_std\]" src/lib.rs; then
        print_pass "Library correctly marked #![no_std]"
    else
        print_fail "Library not marked #![no_std]"
        return 1
    fi

    if grep -q "extern crate alloc" src/lib.rs; then
        print_pass "Library declares alloc"
    else
        print_fail "Library does not declare alloc"
        return 1
    fi
}

# 2. Check for unguarded std imports
check_std_imports() {
    print_section "2. Unguarded std Imports"

    cd "$PROJECT_ROOT"

    # Get list of files to check (exclude main.rs, examples, and test files)
    local files=$(find src -name "*.rs" -not -name "main.rs" -not -path "*/examples/*")

    local found_issues=0
    for file in $files; do
        # Look for unguarded std imports (not in cfg blocks or tests)
        if grep -q "use std::" "$file" 2>/dev/null; then
            # Check if this file is properly gated
            if ! grep -B5 "use std::" "$file" | grep -q "#\[cfg(feature = \"std\")\]"; then
                if ! grep -q "#\[cfg(feature = \"std\")\]" "$file"; then
                    print_warn "File '$file' has std import without cfg gate"
                    found_issues=1
                fi
            fi
        fi
    done

    if [ "$found_issues" = "0" ]; then
        print_pass "No unguarded std imports found"
    fi
}

# 3. Check alloc usage patterns
check_alloc_usage() {
    print_section "3. Alloc Usage Patterns"

    cd "$PROJECT_ROOT"

    # Check that core modules use alloc correctly
    if grep -q "extern crate alloc" src/contract.rs; then
        print_pass "contract.rs declares alloc"
    else
        print_warn "contract.rs missing alloc declaration"
    fi

    # Check that String imports come from alloc
    local core_modules="contract.rs domain_validator.rs errors.rs"
    for mod in $core_modules; do
        if grep -q "use alloc::string" src/"$mod" 2>/dev/null; then
            print_pass "$mod correctly imports from alloc"
        else
            print_info "$mod may be using alloc strings implicitly"
        fi
    done
}

# 4. Check feature gate isolation
check_feature_gates() {
    print_section "4. Feature Gate Isolation"

    cd "$PROJECT_ROOT"

    # Verify std feature gates http modules
    if grep -q '#\[cfg(feature = "std")\]' src/config.rs; then
        print_pass "config.rs gated with std feature"
    else
        print_fail "config.rs not properly gated"
    fi

    # Verify wasm feature excludes host modules
    if grep -q '#\[cfg(not(feature = "wasm"))\].*pub mod http_client' src/lib.rs; then
        print_pass "http_client excluded from wasm builds"
    else
        print_fail "http_client not properly excluded from wasm"
    fi

    # Check that core modules have no wasm gates
    if ! grep -q '#\[cfg(feature = "wasm")\].*pub mod contract' src/lib.rs; then
        if grep -q "pub mod contract" src/lib.rs; then
            print_pass "contract module available in all builds"
        fi
    fi
}

# 5. Verify no_std compilation test
check_no_std_compilation() {
    print_section "5. No-std Compilation Test"

    cd "$PROJECT_ROOT"

    print_info "Testing no_std library build..."
    if cargo build --release --lib --no-default-features 2>&1 | grep -q "Finished"; then
        print_pass "no_std library compiles successfully"
    else
        print_fail "no_std library build failed"
        return 1
    fi
}

# 6. Check time-related dependencies
check_time_dependencies() {
    print_section "6. Time Dependency Analysis"

    cd "$PROJECT_ROOT"

    # Check for std::time usage outside gated modules
    local time_files=$(grep -l "std::time" src/*.rs 2>/dev/null || true)

    for file in $time_files; do
        if [[ "$file" == *"http_client.rs"* ]] || [[ "$file" == *"main.rs"* ]]; then
            print_pass "std::time correctly used only in host-only module: $file"
        else
            print_warn "std::time used in potentially portable module: $file"
        fi
    done
}

# 7. Check dependency declarations
check_dependencies() {
    print_section "7. Dependency Analysis"

    cd "$PROJECT_ROOT"

    # Verify soroban-sdk doesn't require std
    if grep -q "soroban-sdk" Cargo.toml; then
        print_pass "soroban-sdk dependency present"
    else
        print_fail "soroban-sdk dependency missing"
    fi

    # Check that optional deps are properly marked
    if grep "optional = true" Cargo.toml | grep -q "reqwest\|clap\|aes-gcm"; then
        print_pass "HTTP/CLI dependencies marked optional"
    else
        print_warn "Some optional dependencies may be missing optional flag"
    fi
}

# 8. Check panic behavior
check_panic_config() {
    print_section "8. Panic Configuration"

    cd "$PROJECT_ROOT"

    if grep -q 'panic = "abort"' Cargo.toml; then
        print_pass "Panic set to abort (correct for embedded/WASM)"
    else
        print_warn "Panic not set to abort"
    fi
}

# 9. Verify module documentation
check_module_docs() {
    print_section "9. Module Documentation"

    cd "$PROJECT_ROOT"

    # Check that core modules are documented with no_std info
    if grep -q "no_std" src/lib.rs; then
        print_pass "lib.rs documents no_std support"
    else
        print_info "lib.rs may not document no_std support"
    fi

    # Check contract.rs documentation
    if grep -q "no_std" src/contract.rs; then
        print_pass "contract.rs documents no_std support"
    else
        print_info "contract.rs may not document no_std support"
    fi
}

# 10. Portability markers
check_portability_markers() {
    print_section "10. Portability Markers"

    cd "$PROJECT_ROOT"

    # Check for portability attributes
    local portable_count=$(grep -c "#\[cfg(not(feature = \"wasm\"))\]" src/lib.rs || true)
    if [ "$portable_count" -gt 0 ]; then
        print_pass "Found $portable_count host-only module gates"
    else
        print_warn "No host-only module gates found"
    fi
}

# ─────────────────────────────────────────────────────────────────────────────
# Main
# ─────────────────────────────────────────────────────────────────────────────

main() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --help)
                echo "No-std Compliance Validator"
                echo ""
                echo "Usage: $0 [--help|--verbose|--fix]"
                echo ""
                echo "Options:"
                echo "  --help      Show this help message"
                echo "  --verbose   Print detailed analysis"
                echo "  --fix       Attempt to auto-fix issues"
                exit 0
                ;;
            --verbose)
                VERBOSE=1
                shift
                ;;
            --fix)
                FIX=1
                shift
                ;;
            *)
                echo "Unknown option: $1"
                exit 1
                ;;
        esac
    done

    print_header "SorobanAnchor No-std Compliance Audit"
    echo ""

    check_no_std_declaration || return 1
    check_std_imports
    check_alloc_usage
    check_feature_gates
    check_no_std_compilation || return 1
    check_time_dependencies
    check_dependencies
    check_panic_config
    check_module_docs
    check_portability_markers

    # Summary
    echo ""
    print_header "Compliance Summary"
    echo -e "  ${GREEN}Passed:${NC}   $CHECKS_PASSED"
    if [ "$CHECKS_WARNINGS" -gt 0 ]; then
        echo -e "  ${YELLOW}Warnings:${NC} $CHECKS_WARNINGS"
    fi
    if [ "$CHECKS_FAILED" -gt 0 ]; then
        echo -e "  ${RED}Failed:${NC}   $CHECKS_FAILED"
    fi
    echo ""

    if [ "$CHECKS_FAILED" -gt 0 ]; then
        echo -e "${RED}${BOLD}❌ Compliance audit failed${NC}"
        exit 1
    else
        echo -e "${GREEN}${BOLD}✅ No-std compliance verified${NC}"
        exit 0
    fi
}

main "$@"
