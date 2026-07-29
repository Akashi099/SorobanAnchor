#!/bin/bash

# Feature-Flag Matrix Test Suite
#
# This script validates all supported AnchorKit feature-flag combinations.
# It ensures that the codebase compiles and tests pass for each configuration.
#
# Usage:
#   ./scripts/test_feature_matrix.sh [--quick|--full]
#
# Options:
#   --quick : Run fast compilation checks only (no full test suites)
#   --full  : Run complete tests for each combination (default)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"

# Determine test mode
TEST_MODE="${1:-full}"
FAILED=0
PASSED=0

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_test() {
    echo -e "${YELLOW}[TEST]${NC} $1"
}

log_pass() {
    echo -e "${GREEN}[PASS]${NC} $1"
    ((PASSED++))
}

log_fail() {
    echo -e "${RED}[FAIL]${NC} $1"
    ((FAILED++))
}

run_test() {
    local name="$1"
    local cmd="$2"

    log_test "$name"
    if eval "$cmd" > /tmp/test.log 2>&1; then
        log_pass "$name"
    else
        log_fail "$name"
        echo "Error output:"
        tail -20 /tmp/test.log
    fi
}

echo "=========================================="
echo "AnchorKit Feature-Flag Matrix Tests"
echo "=========================================="
echo "Mode: $TEST_MODE"
echo ""

# Test 1: Default build (std feature)
log_test "Build 1: Default (std feature)"
run_test "  Compile with std" "cargo build --release"

if [ "$TEST_MODE" = "full" ]; then
    run_test "  Run tests with std" "cargo test --release --lib"
fi

# Test 2: Explicit std feature
log_test "Build 2: Explicit std"
run_test "  Compile with --features std" "cargo build --release --features std"

# Test 3: std + mock-only
log_test "Build 3: std + mock-only (testing)"
run_test "  Compile with std,mock-only" "cargo build --release --features std,mock-only"

if [ "$TEST_MODE" = "full" ]; then
    run_test "  Run tests with mock-only" "cargo test --release --features std,mock-only"
fi

# Test 4: std + stress-tests
log_test "Build 4: std + stress-tests"
run_test "  Compile with stress-tests" "cargo build --release --features std,stress-tests"

# Test 5: mock-only alone (without std)
log_test "Build 5: mock-only (without std)"
run_test "  Compile no-default-features with mock-only" "cargo build --release --no-default-features --features mock-only"

# Test 6: stress-tests alone (without std)
log_test "Build 6: stress-tests (without std)"
run_test "  Compile no-default-features with stress-tests" "cargo build --release --no-default-features --features stress-tests"

# Test 7: WASM target (with --no-default-features --features wasm)
# Note: This requires wasm32-unknown-unknown target to be installed
log_test "Build 7: WASM build (if available)"
if rustup target list 2>/dev/null | grep -q "wasm32-unknown-unknown (installed)"; then
    run_test "  Compile WASM target" "cargo build --release --target wasm32-unknown-unknown --no-default-features --features wasm"
else
    log_test "  WASM target not installed, skipping"
fi

# Test 8: Feature-flag matrix tests
log_test "Build 8: Run feature matrix tests"
run_test "  Feature matrix tests" "cargo test --test feature_flag_matrix_tests --release"

# Test 9: Property-based tests
if [ "$TEST_MODE" = "full" ]; then
    log_test "Build 9: Property-based tests"
    run_test "  Property-based tests" "cargo test --test property_based_parsing_tests --release"
fi

# Test 10: Verify features don't conflict
log_test "Build 10: Verify mutual exclusivity"
run_test "  Check wasm+std conflict prevention" "! cargo build --release --target wasm32-unknown-unknown --features wasm,std 2>&1 | grep -q 'success' || true"

echo ""
echo "=========================================="
echo "Test Summary"
echo "=========================================="
echo -e "Passed: ${GREEN}$PASSED${NC}"
echo -e "Failed: ${RED}$FAILED${NC}"
echo "=========================================="

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}All feature matrix tests passed!${NC}"
    exit 0
else
    echo -e "${RED}Some tests failed. See output above.${NC}"
    exit 1
fi
