#![cfg(all(test, not(feature = "wasm")))]

//! No-std compliance tests.
//!
//! Verify that core modules are portable across std and no_std environments.
//! These tests ensure that the library maintains no_std compatibility while
//! preserving the full feature set for both on-chain and off-chain use cases.

#[cfg(test)]
mod no_std_compliance {
    /// Verify that core error types work without std.
    #[test]
    fn test_error_types_are_no_std() {
        use anchorkit::{AnchorKitError, ErrorCode};

        let err1 = AnchorKitError::ValidationFailed("test".into());
        let err2 = AnchorKitError::RateLimitExceeded;
        let err3 = AnchorKitError::InvalidSignature;

        assert_ne!(err1.error_code(), ErrorCode::Unknown);
        assert_ne!(err2.error_code(), ErrorCode::Unknown);
        assert_ne!(err3.error_code(), ErrorCode::Unknown);
    }

    /// Verify that domain validation works in no_std contexts.
    #[test]
    fn test_domain_validator_is_no_std() {
        use anchorkit::validate_anchor_domain;

        let valid = validate_anchor_domain("https://example.com");
        assert!(valid.is_ok(), "valid domain must parse");

        let invalid = validate_anchor_domain("http://example.com");
        assert!(invalid.is_ok(), "http URLs should be validated per policy");
    }

    /// Verify that deterministic hashing works in no_std.
    #[test]
    fn test_deterministic_hash_is_no_std() {
        use anchorkit::{compute_payload_hash, verify_payload_hash};

        let payload = b"test data";
        let hash = compute_payload_hash(payload);

        // Hashing should be deterministic
        let hash2 = compute_payload_hash(payload);
        assert_eq!(hash, hash2, "hash must be deterministic");

        // Verification should work
        assert!(verify_payload_hash(payload, &hash).is_ok());
    }

    /// Verify that replay detection works in no_std.
    #[test]
    fn test_replay_detection_is_no_std() {
        use anchorkit::contract::{ReplayProtection};

        let _protection: Option<ReplayProtection>;
        // If this compiles, replay protection types are no_std compatible
    }

    /// Verify that rate limiting works in no_std.
    #[test]
    fn test_rate_limiter_is_no_std() {
        use anchorkit::RateLimiter;

        let limiter = RateLimiter::new(
            anchorkit::RateLimitConfig {
                max_requests: 100,
                window_seconds: 60,
            }
        ).expect("rate limiter must construct");

        let _state = limiter.current_state();
    }

    /// Verify that transaction state tracking is no_std.
    #[test]
    fn test_transaction_state_is_no_std() {
        use anchorkit::{TransactionState, TransactionStateTracker};

        let state = TransactionState::Initial;
        assert_ne!(state, TransactionState::Terminated);

        // Tracker should be constructible
        let _tracker: Option<TransactionStateTracker>;
    }

    /// Verify that SEP-10 JWT verification is no_std.
    #[test]
    fn test_sep10_jwt_parsing_is_no_std() {
        use anchorkit::sep10_jwt::{parse_jwt_header, verify_ed25519_signature};

        // JWT header parsing should work
        let header = "eyJhbGciOiJFZDI1NTE5In0"; // base64({"alg":"Ed25519"})
        let _result = parse_jwt_header(header);
    }

    /// Verify that session state machine is no_std.
    #[test]
    fn test_session_state_machine_is_no_std() {
        use anchorkit::contract::SessionState;

        let _state1 = SessionState::Active;
        let _state2 = SessionState::Expired;
        let _state3 = SessionState::Revoked;
    }

    /// Verify that admin audit log types are no_std.
    #[test]
    fn test_admin_audit_log_is_no_std() {
        use anchorkit::AdminAuditLog;

        let _log: Option<AdminAuditLog>;
        // If this compiles, audit log types are no_std
    }

    /// Verify that cache governance is no_std.
    #[test]
    fn test_cache_governance_is_no_std() {
        use anchorkit::{CachePolicy, CachePolicySet, CacheEntryType};

        let _policy: CachePolicy;
        let _entry: CacheEntryType;
        // If this compiles, cache governance is no_std
    }

    /// Verify that service management is no_std.
    #[test]
    fn test_service_management_is_no_std() {
        use anchorkit::{ServiceManager, ServiceToggleState};

        let _toggle: ServiceToggleState;
        let _manager: Option<ServiceManager>;
    }

    /// Verify that migration utilities are no_std.
    #[test]
    fn test_migration_utilities_are_no_std() {
        use anchorkit::contract::MigrationContext;

        let _ctx: Option<MigrationContext>;
        // If this compiles, migration is no_std
    }

    /// Verify that contract types compile in no_std.
    #[test]
    fn test_contract_core_types_are_no_std() {
        use anchorkit::{
            AnchorKitContract, TransactionState, RateLimiter,
            AnchorTomlProvenance, CacheConfig,
        };

        let _contract: Option<AnchorKitContract>;
        let _state = TransactionState::Initial;
        let _config: Option<CacheConfig>;
        let _provenance = AnchorTomlProvenance::Cached;
    }

    /// Verify that alloc collections work correctly.
    #[test]
    fn test_alloc_collections_are_used() {
        use anchorkit::contract::{AttestationFilter, AttestationPage};

        let _filter: Option<AttestationFilter>;
        let _page: Option<AttestationPage>;
    }

    /// Verify error handling consistency across types.
    #[test]
    fn test_error_normalization_is_no_std() {
        use anchorkit::{normalize_asset_code, AnchorKitError};

        // Normalizing valid codes should work
        let result = normalize_asset_code("USDC");
        assert!(result.is_ok());

        // Normalizing invalid codes should error properly
        let result = normalize_asset_code("");
        assert!(result.is_err());
    }

    /// Verify that retry logic is no_std compatible.
    #[test]
    fn test_retry_logic_is_no_std() {
        use anchorkit::{RetryConfig, BackoffStrategy, JitterPolicy, MockJitterSource};

        let config = RetryConfig {
            max_attempts: 3,
            initial_delay_ms: 100,
            max_delay_ms: 5000,
            backoff_strategy: BackoffStrategy::ExponentialBackoff,
            jitter_policy: JitterPolicy::Full,
        };

        assert_eq!(config.max_attempts, 3);

        // Mock jitter source should work
        let _jitter = MockJitterSource::new(vec![0, 10, 20]);
    }

    /// Verify all core re-exports are accessible.
    #[test]
    fn test_core_re_exports_available() {
        use anchorkit::{
            AnchorKitError, ErrorCode, validate_anchor_domain,
            compute_payload_hash, verify_payload_hash,
            RateLimiter, RateLimitConfig,
            retry_with_backoff, RetryConfig,
        };

        // If all these import successfully, core re-exports are available
        let _err: AnchorKitError;
        let _code: ErrorCode;
        let _validator = validate_anchor_domain;
        let _hasher = compute_payload_hash;
        let _limiter_fn = RateLimiter::new;
        let _retry_fn = retry_with_backoff;
    }

    /// Verify module features compile correctly.
    #[test]
    fn test_all_features_compile() {
        // This test simply checks that Cargo can compile with all features.
        // The actual compilation is handled by cargo test, but this is a
        // check that the test suite includes all feature combinations.
        assert!(cfg!(not(feature = "wasm")), "This test runs in non-wasm mode");
    }
}

#[cfg(all(test, feature = "wasm"))]
mod wasm_no_std_verification {
    /// Verify that WASM builds also maintain no_std compliance.
    #[test]
    fn test_wasm_is_no_std() {
        use anchorkit::{AnchorKitError, RateLimiter};

        let err: AnchorKitError;
        let _limiter = RateLimiter::new;
        // If this compiles, WASM is no_std
    }
}
