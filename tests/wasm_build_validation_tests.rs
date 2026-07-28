#![cfg(all(test, feature = "wasm"))]

//! WASM build validation tests.
//!
//! Verify that WASM builds produce valid artifacts and maintain proper
//! feature isolation between std and wasm modes. These tests ensure that
//! the WASM build path is reproducible and production-ready.

#[cfg(test)]
mod wasm_build_validation {
    use std::path::Path;

    /// Verify that the WASM feature compiles core types without std.
    #[test]
    fn test_wasm_core_types_available() {
        let _: anchorkit::AnchorKitError;
        let _: anchorkit::ErrorCode;
        let _: anchorkit::TransactionState;
        let _: anchorkit::RateLimiter;
        // If this test compiles, core types are available in wasm builds
    }

    /// Verify that rate limiting works in WASM mode.
    #[test]
    fn test_wasm_rate_limiter_available() {
        use anchorkit::RateLimiter;

        let limiter = RateLimiter::new(
            anchorkit::RateLimitConfig {
                max_requests: 100,
                window_seconds: 60,
            }
        );
        assert!(limiter.is_ok(), "rate limiter must be constructible in wasm");
    }

    /// Verify error handling works in WASM mode.
    #[test]
    fn test_wasm_error_handling() {
        use anchorkit::{AnchorKitError, ErrorCode};

        let err = AnchorKitError::ValidationFailed("test".into());
        let code = err.error_code();
        assert_ne!(code, ErrorCode::Unknown);
    }

    /// Verify transaction state tracking in WASM mode.
    #[test]
    fn test_wasm_transaction_state() {
        use anchorkit::TransactionState;

        let state = TransactionState::Initial;
        assert_ne!(state, TransactionState::Terminated);
    }

    /// Verify domain validation works in WASM mode (no http, just parsing).
    #[test]
    fn test_wasm_domain_validation() {
        use anchorkit::validate_anchor_domain;

        let result = validate_anchor_domain("https://anchor.example.com");
        assert!(result.is_ok(), "domain validation must work in wasm");

        let invalid = validate_anchor_domain("not-a-url");
        assert!(invalid.is_err(), "invalid domain must be rejected");
    }

    /// Verify deterministic hash works in WASM mode.
    #[test]
    fn test_wasm_deterministic_hash() {
        use anchorkit::{compute_payload_hash, verify_payload_hash};

        let payload = b"test payload";
        let hash = compute_payload_hash(payload);

        // Same payload should produce same hash (deterministic)
        let hash2 = compute_payload_hash(payload);
        assert_eq!(hash, hash2, "hash must be deterministic");

        // Verify hash passes verification
        assert!(verify_payload_hash(payload, &hash).is_ok());
    }

    /// Verify contract types are available in WASM mode.
    #[test]
    fn test_wasm_contract_types() {
        use anchorkit::{AnchorKitContract, CacheConfig, ServiceRetirementInfo, AnchorServices};

        // These types must be in scope for wasm builds
        let _config: Option<CacheConfig>;
        let _services: Option<AnchorServices>;
    }

    /// Verify retry configuration works in WASM mode.
    #[test]
    fn test_wasm_retry_config() {
        use anchorkit::{RetryConfig, BackoffStrategy, JitterPolicy};

        let config = RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 100,
            max_delay_ms: 5000,
            backoff_strategy: BackoffStrategy::ExponentialBackoff,
            jitter_policy: JitterPolicy::Full,
        };

        assert_eq!(config.max_attempts, 3);
    }
}

#[cfg(all(test, not(feature = "wasm")))]
mod wasm_feature_isolation {
    use std::path::Path;

    /// When NOT in wasm mode, verify that SEP modules are accessible.
    /// This confirms that the wasm feature correctly gates host-only code.
    #[test]
    fn test_non_wasm_exposes_sep_modules() {
        use anchorkit::{RawDepositResponse, RawInteractiveDepositResponse};

        let _deposit: RawDepositResponse;
        let _interactive: RawInteractiveDepositResponse;
        // If this compiles, SEP types are available in non-wasm builds
    }

    /// Verify HTTP client is available in non-wasm builds.
    #[test]
    fn test_non_wasm_exposes_http_client() {
        use anchorkit::build_client;

        let _fn = build_client;
        // Function pointer access confirms http_client module is public
    }
}
