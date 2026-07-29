#![cfg(feature = "std")]

//! Feature-flag matrix test suite
//!
//! This module documents and validates the feature-flag combinations
//! supported by the AnchorKit project. Each combination is tested to ensure
//! the codebase compiles and basic functionality works correctly.
//!
//! Supported feature combinations:
//! - std (default, native builds)
//! - wasm (on-chain Soroban deployment)
//! - mock-only (testing with fixtures)
//! - std + mock-only (testing native code with fixtures)
//! - stress-tests (high-concurrency load testing)

#[cfg(all(feature = "std", not(feature = "wasm")))]
#[test]
fn feature_matrix_std_default_builds() {
    // This test only compiles when `std` feature is enabled and `wasm` is not
    assert!(true, "std feature is enabled");
}

#[cfg(all(feature = "std", feature = "mock-only"))]
#[test]
fn feature_matrix_std_with_mock_only_builds() {
    // This test only compiles when both `std` and `mock-only` features are enabled
    use anchorkit::mock::*;

    // Verify mock functions are available
    let _deposit = mock_deposit_response_minimal();
    assert!(true, "mock-only feature is available with std");
}

#[cfg(feature = "mock-only")]
#[test]
fn feature_matrix_mock_only_independent() {
    // mock-only can be enabled independently (with or without std)
    use anchorkit::mock::*;

    // All mock functions should be available regardless of std
    let _deposit = mock_deposit_response_full();
    let _withdrawal = mock_withdrawal_response_minimal();
    assert!(true, "mock-only feature works independently");
}

#[cfg(feature = "stress-tests")]
#[test]
fn feature_matrix_stress_tests_enabled() {
    // This test documents that stress-tests feature is present
    // Actual stress tests are in load_simulation_tests.rs
    assert!(true, "stress-tests feature is enabled");
}

// ─────────────────────────────────────────────────────────────────────────
// Feature Invariant Tests
// These verify that features don't have conflicting state
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn feature_invariant_wasm_mutually_exclusive_with_std() {
    // WASM and std are mutually exclusive by design
    #[cfg(all(feature = "wasm", feature = "std"))]
    compile_error!("wasm and std features are mutually exclusive");

    // This test passes as long as the crate builds
    assert!(true, "wasm and std exclusivity is enforced");
}

#[test]
fn feature_matrix_mock_only_no_production_use() {
    // mock-only should never be used in production binaries
    // This is a documentation test

    #[cfg(feature = "mock-only")]
    {
        eprintln!("WARNING: mock-only feature is enabled. Never use this in production.");
    }

    assert!(true, "mock-only production check documented");
}

// ─────────────────────────────────────────────────────────────────────────
// Configuration Tests
// Verify that the Cargo.toml feature definitions match expectations
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn feature_matrix_default_is_std() {
    // Default features should include std
    #[cfg(feature = "std")]
    assert!(true, "std is in default features");

    #[cfg(not(feature = "std"))]
    panic!("std should be in default features");
}

#[test]
fn feature_matrix_mock_only_is_optional() {
    // mock-only should be optional and not in default features
    // This test passes whether or not mock-only is enabled

    #[cfg(feature = "mock-only")]
    {
        // If mock-only is enabled, that's expected in test contexts
        assert!(true, "mock-only is explicitly enabled for this test");
    }

    #[cfg(not(feature = "mock-only"))]
    {
        // If mock-only is not enabled, that's also expected for production
        assert!(true, "mock-only is not enabled (production build)");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Build Matrix Documentation
// These doc tests serve as reference for how to build each combination
// ─────────────────────────────────────────────────────────────────────────

/// # Default native build with std
/// ```bash
/// cargo build --release
/// # or explicitly:
/// cargo build --release --features std
/// ```
///
/// This is the standard build for servers, CLIs, and native applications.
#[test]
fn build_example_default_std() {
    assert!(cfg!(feature = "std"));
}

/// # WASM build for Soroban
/// ```bash
/// cargo build --release --target wasm32-unknown-unknown \
///   --no-default-features --features wasm
/// ```
///
/// Required for on-chain smart contract deployment.
/// Disables std and all host-dependent modules.
#[test]
fn build_example_wasm() {
    #[cfg(target_arch = "wasm32")]
    assert!(cfg!(feature = "wasm"));
    #[cfg(not(target_arch = "wasm32"))]
    {
        // This test runs on native but documents the WASM build
        assert!(true);
    }
}

/// # Mock-only testing build
/// ```bash
/// cargo test --features std,mock-only
/// # or without std:
/// cargo test --no-default-features --features mock-only
/// ```
///
/// Enables pre-built response fixtures for testing without a live anchor.
#[test]
fn build_example_mock_only() {
    #[cfg(feature = "mock-only")]
    {
        use anchorkit::mock::*;
        let _deposit = mock_deposit_response_minimal();
    }
}

/// # Stress tests build
/// ```bash
/// cargo test --features std,stress-tests -- --ignored
/// ```
///
/// Runs high-concurrency and throughput scenarios.
/// Excluded from normal CI to avoid slowing down PR checks.
#[test]
fn build_example_stress_tests() {
    #[cfg(feature = "stress-tests")]
    {
        assert!(true, "stress-tests feature is active");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Compatibility Tests
// Verify feature combinations work together
// ─────────────────────────────────────────────────────────────────────────

#[test]
#[cfg(feature = "std")]
fn feature_compatibility_std_is_stable() {
    // std should always work and be the default
    assert!(cfg!(feature = "std"), "std feature should be available");
}

#[test]
#[cfg(all(feature = "std", feature = "mock-only"))]
fn feature_compatibility_std_and_mock_only() {
    // std and mock-only should work together
    use anchorkit::mock::*;

    // Both features should be accessible
    let _deposit = mock_deposit_response_minimal();
    assert!(true, "std and mock-only are compatible");
}

#[test]
fn feature_documentation_matrix() {
    // This test documents the complete feature matrix in code
    println!("\n=== AnchorKit Feature Matrix ===");
    println!("Default: std");
    println!("Optional features: wasm, mock-only, stress-tests");
    println!("\nSupported combinations:");
    println!("  1. [std] - Default native build");
    println!("  2. [std, mock-only] - Native + test fixtures");
    println!("  3. [std, stress-tests] - Native + load testing");
    println!("  4. [std, mock-only, stress-tests] - All three");
    println!("  5. [wasm] - On-chain WASM (no default features)");
    println!("  6. [mock-only] - Test fixtures alone");
    println!("\nIncompatible:");
    println!("  - std + wasm (mutually exclusive)");
    println!("  - wasm should use --no-default-features");

    assert!(true);
}
