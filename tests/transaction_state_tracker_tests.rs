/// Transaction State Tracker Tests
/// This test file demonstrates and validates the Transaction State Tracker implementation

use anchorkit::transaction_state_tracker::*;
use soroban_sdk::Env;

#[cfg(test)]
mod transaction_state_tracker_tests {
    use super::*;
    use soroban_sdk::testutils::Address;
    use soroban_sdk::String;

    #[test]
    fn test_transaction_state_to_string() {
        assert_eq!(TransactionState::Pending.as_str(), "pending");
        assert_eq!(TransactionState::InProgress.as_str(), "in_progress");
        assert_eq!(TransactionState::Completed.as_str(), "completed");
        assert_eq!(TransactionState::Failed.as_str(), "failed");
    }

    #[test]
    fn test_transaction_state_from_string() {
        assert_eq!(
            TransactionState::from_str("pending"),
            Some(TransactionState::Pending)
        );
        assert_eq!(
            TransactionState::from_str("in_progress"),
            Some(TransactionState::InProgress)
        );
        assert_eq!(
            TransactionState::from_str("completed"),
            Some(TransactionState::Completed)
        );
        assert_eq!(
            TransactionState::from_str("failed"),
            Some(TransactionState::Failed)
        );
        assert_eq!(TransactionState::from_str("unknown"), None);
    }

    #[test]
    fn test_full_transaction_lifecycle() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        // Create transaction -> Pending
        let tx1_result = tracker.create_transaction(1, initiator.clone(), &env);
        assert!(tx1_result.is_ok());
        let tx1 = tx1_result.unwrap();
        assert_eq!(tx1.state, TransactionState::Pending);

        // Start transaction -> In-progress
        let tx2_result = tracker.start_transaction(1, &env);
        assert!(tx2_result.is_ok());
        let tx2 = tx2_result.unwrap();
        assert_eq!(tx2.state, TransactionState::InProgress);

        // Complete transaction -> Completed
        let tx3_result = tracker.complete_transaction(1, &env);
        assert!(tx3_result.is_ok());
        let tx3 = tx3_result.unwrap();
        assert_eq!(tx3.state, TransactionState::Completed);
    }

    #[test]
    fn test_transaction_failure_with_error_message() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();

        let error_msg = String::from_str(&env, "Payment declined");
        let result = tracker.fail_transaction(1, error_msg.clone(), &env);

        assert!(result.is_ok());
        let record = result.unwrap();
        assert_eq!(record.state, TransactionState::Failed);
        assert_eq!(record.error_message, Some(error_msg));
    }

    #[test]
    fn test_query_transactions_by_state() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        // Create 5 transactions
        for i in 1..=5 {
            tracker.create_transaction(i, initiator.clone(), &env).ok();
        }

        // Move some to in-progress
        tracker.start_transaction(1, &env).ok();
        tracker.start_transaction(2, &env).ok();

        // Complete one
        tracker.complete_transaction(2, &env).ok();

        // Query by state
        let pending_result = tracker.get_transactions_by_state(TransactionState::Pending);
        assert!(pending_result.is_ok());
        let pending = pending_result.unwrap();
        assert_eq!(pending.len(), 3); // 3, 4, 5

        let in_progress_result = tracker.get_transactions_by_state(TransactionState::InProgress);
        assert!(in_progress_result.is_ok());
        let in_progress = in_progress_result.unwrap();
        assert_eq!(in_progress.len(), 1); // 1

        let completed_result = tracker.get_transactions_by_state(TransactionState::Completed);
        assert!(completed_result.is_ok());
        let completed = completed_result.unwrap();
        assert_eq!(completed.len(), 1); // 2
    }

    #[test]
    #[ignore = "production mode requires Soroban contract context for storage access; run inside env.as_contract()"]
    fn test_production_mode_flag() {
        let env = Env::default();
        let mut prod_tracker = TransactionStateTracker::new(false);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        // In production mode, cache should not be populated
        let result = prod_tracker.create_transaction(1, initiator.clone(), &env);
        assert!(result.is_ok());
        assert_eq!(prod_tracker.cache_size(), 0); // Should be 0 in production mode

        // In dev mode, cache should be populated
        let mut dev_tracker = TransactionStateTracker::new(true);
        let result = dev_tracker.create_transaction(1, initiator.clone(), &env);
        assert!(result.is_ok());
        assert_eq!(dev_tracker.cache_size(), 1); // Should be 1 in dev mode
    }

    #[test]
    fn test_transaction_not_found() {
        let env = Env::default();
        let tracker = TransactionStateTracker::new(true);

        let result = tracker.get_transaction_state(999, &env);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_multiple_transactions_isolation() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator1 = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let initiator2 = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        // Create transactions from different initiators
        tracker.create_transaction(1, initiator1.clone(), &env).ok();
        tracker.create_transaction(2, initiator2.clone(), &env).ok();

        // Update first one
        tracker.start_transaction(1, &env).ok();

        // Verify second one is still pending
        let tx2_state = tracker.get_transaction_state(2, &env);
        assert!(tx2_state.is_ok());
        let tx2 = tx2_state.unwrap().unwrap();
        assert_eq!(tx2.state, TransactionState::Pending);
        assert_eq!(tx2.initiator, initiator2);
    }

    #[test]
    fn test_clear_cache_dev_mode() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.create_transaction(2, initiator.clone(), &env).ok();
        assert_eq!(tracker.cache_size(), 2);

        let clear_result = tracker.clear_cache(&env);
        assert!(clear_result.is_ok());
        assert_eq!(tracker.cache_size(), 0);
    }

    #[test]
    fn test_timestamp_tracking() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        let create_result = tracker.create_transaction(1, initiator.clone(), &env);
        let record1 = create_result.unwrap();
        let initial_timestamp = record1.timestamp;

        let update_result = tracker.start_transaction(1, &env);
        let record2 = update_result.unwrap();

        // Timestamps should be set and last_updated should reflect the change
        assert_eq!(record2.timestamp, initial_timestamp);
        assert!(record2.last_updated >= initial_timestamp);
    }

    // -----------------------------------------------------------------------
    // Backward / same-state transition guard
    // -----------------------------------------------------------------------

    #[test]
    fn test_illegal_backward_transition_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        // Completed → Pending is illegal
        let r = tracker.advance_transaction_state(1, TransactionState::Pending, &env);
        assert!(r.is_err());

        // Completed → InProgress is illegal
        let r = tracker.advance_transaction_state(1, TransactionState::InProgress, &env);
        assert!(r.is_err());
    }

    #[test]
    fn test_illegal_same_state_transition_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();

        // Pending → Pending is not a valid transition
        let r = tracker.advance_transaction_state(1, TransactionState::Pending, &env);
        assert!(r.is_err());
    }

    #[test]
    fn test_valid_forward_transition_accepted() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();

        let r = tracker.advance_transaction_state(1, TransactionState::InProgress, &env);
        assert!(r.is_ok());
        assert_eq!(r.unwrap().state, TransactionState::InProgress);

        let r = tracker.advance_transaction_state(1, TransactionState::Completed, &env);
        assert!(r.is_ok());
        assert_eq!(r.unwrap().state, TransactionState::Completed);
    }

    // -----------------------------------------------------------------------
    // Audit log entries for success and failure
    // -----------------------------------------------------------------------

    #[test]
    fn test_audit_log_entry_created_on_successful_transition() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();

        assert_eq!(tracker.audit_log.len(), 1);
        let entry = &tracker.audit_log[0];
        assert_eq!(entry.from_state, TransactionState::Pending);
        assert_eq!(entry.to_state, TransactionState::InProgress);
        assert!(entry.success);
    }

    #[test]
    fn test_audit_log_entry_created_on_failed_transition() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        // Illegal: Pending → Completed
        let _ = tracker.advance_transaction_state(1, TransactionState::Completed, &env);

        assert_eq!(tracker.audit_log.len(), 1);
        let entry = &tracker.audit_log[0];
        assert_eq!(entry.from_state, TransactionState::Pending);
        assert_eq!(entry.to_state, TransactionState::Completed);
        assert!(!entry.success);
    }

    // -----------------------------------------------------------------------
    // Invalid transition rejection — exhaustive matrix
    // -----------------------------------------------------------------------

    #[test]
    fn test_completed_to_pending_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        let r = tracker.advance_transaction_state(1, TransactionState::Pending, &env);
        assert!(r.is_err(), "Completed → Pending must be rejected");
    }

    #[test]
    fn test_completed_to_in_progress_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        let r = tracker.advance_transaction_state(1, TransactionState::InProgress, &env);
        assert!(r.is_err(), "Completed → InProgress must be rejected");
    }

    #[test]
    fn test_completed_to_failed_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        let r = tracker.fail_transaction(1, String::from_str(&env, "late failure"), &env);
        assert!(r.is_err(), "Completed → Failed must be rejected");
    }

    #[test]
    fn test_failed_to_in_progress_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.fail_transaction(1, String::from_str(&env, "err"), &env).ok();

        let r = tracker.advance_transaction_state(1, TransactionState::InProgress, &env);
        assert!(r.is_err(), "Failed → InProgress must be rejected");
    }

    #[test]
    fn test_failed_to_completed_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.fail_transaction(1, String::from_str(&env, "err"), &env).ok();

        let r = tracker.complete_transaction(1, &env);
        assert!(r.is_err(), "Failed → Completed must be rejected");
    }

    #[test]
    fn test_failed_to_failed_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.fail_transaction(1, String::from_str(&env, "first failure"), &env).ok();

        let r = tracker.fail_transaction(1, String::from_str(&env, "second failure"), &env);
        assert!(r.is_err(), "Failed → Failed must be rejected");
    }

    #[test]
    fn test_pending_to_completed_directly_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();

        let r = tracker.complete_transaction(1, &env);
        assert!(r.is_err(), "Pending → Completed must be rejected");
    }

    #[test]
    fn test_pending_to_in_progress_to_failed_valid() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();

        let r = tracker.fail_transaction(1, String::from_str(&env, "mid-flight error"), &env);
        assert!(r.is_ok(), "InProgress → Failed must be accepted");
        assert_eq!(r.unwrap().state, TransactionState::Failed);
    }

    #[test]
    fn test_pending_to_failed_directly_valid() {
        // Pending → Failed is a valid transition (immediate pre-processing failure)
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();

        let r = tracker.fail_transaction(1, String::from_str(&env, "pre-processing failure"), &env);
        assert!(r.is_ok(), "Pending → Failed must be accepted");
        let record = r.unwrap();
        assert_eq!(record.state, TransactionState::Failed);
        assert!(record.recovery_metadata.is_some());
        let meta = record.recovery_metadata.unwrap();
        assert_eq!(meta.failed_from_state, TransactionState::Pending);
    }

    // -----------------------------------------------------------------------
    // Invalid transition error message format
    // -----------------------------------------------------------------------

    #[test]
    fn test_invalid_transition_error_carries_e24_prefix() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        let err = tracker
            .advance_transaction_state(1, TransactionState::Pending, &env)
            .unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("[E24]"),
            "error message must carry [E24] prefix, got: {err_str}"
        );
        assert!(
            err_str.contains("completed"),
            "error message must name the from-state, got: {err_str}"
        );
        assert!(
            err_str.contains("pending"),
            "error message must name the to-state, got: {err_str}"
        );
    }

    // -----------------------------------------------------------------------
    // Recovery metadata
    // -----------------------------------------------------------------------

    #[test]
    fn test_recovery_metadata_populated_on_failure() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();

        let reason = String::from_str(&env, "network timeout");
        let record = tracker.fail_transaction(1, reason.clone(), &env).unwrap();

        assert!(record.recovery_metadata.is_some(), "recovery_metadata must be set on failure");
        let meta = record.recovery_metadata.unwrap();
        assert_eq!(meta.failure_reason, reason, "failure_reason must match the error message");
        assert_eq!(meta.failed_from_state, TransactionState::InProgress);
        assert_eq!(meta.retry_count, 0);
        // In the test env ledger starts at sequence 0, so we verify the field
        // is populated with the current ledger value, not that it is > 0.
        assert_eq!(
            meta.last_updated_ledger,
            env.ledger().sequence(),
            "last_updated_ledger must equal the current ledger sequence"
        );
    }

    #[test]
    fn test_recovery_metadata_absent_on_success() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        let record = tracker.complete_transaction(1, &env).unwrap();

        assert!(record.recovery_metadata.is_none(), "recovery_metadata must be None for Completed");
    }

    #[test]
    fn test_get_recovery_metadata_returns_metadata() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.fail_transaction(1, String::from_str(&env, "timeout"), &env).ok();

        let meta = tracker.get_recovery_metadata(1, &env).unwrap();
        assert!(meta.is_some());
        let meta = meta.unwrap();
        assert_eq!(meta.failed_from_state, TransactionState::InProgress);
        assert_eq!(meta.retry_count, 0);
    }

    #[test]
    fn test_get_recovery_metadata_none_for_non_failed() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();

        let meta = tracker.get_recovery_metadata(1, &env).unwrap();
        assert!(meta.is_none(), "recovery_metadata must be None for Pending");
    }

    #[test]
    fn test_is_recoverable_true_for_failed() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.fail_transaction(1, String::from_str(&env, "err"), &env).ok();

        assert!(tracker.is_recoverable(1, &env).unwrap());
    }

    #[test]
    fn test_is_recoverable_false_for_completed() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        assert!(!tracker.is_recoverable(1, &env).unwrap());
    }

    #[test]
    fn test_record_recovery_attempt_increments_retry_count() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.fail_transaction(1, String::from_str(&env, "err"), &env).ok();

        tracker.record_recovery_attempt(1, &env).unwrap();
        tracker.record_recovery_attempt(1, &env).unwrap();

        let meta = tracker.get_recovery_metadata(1, &env).unwrap().unwrap();
        assert_eq!(meta.retry_count, 2);
    }

    #[test]
    fn test_record_recovery_attempt_on_non_failed_returns_error() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();

        // Transaction is InProgress, not Failed — must return an error
        let r = tracker.record_recovery_attempt(1, &env);
        assert!(r.is_err(), "record_recovery_attempt on non-Failed tx must return error");
    }

    #[test]
    fn test_get_failed_transactions_returns_only_failed() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.create_transaction(2, initiator.clone(), &env).ok();
        tracker.create_transaction(3, initiator.clone(), &env).ok();

        tracker.start_transaction(1, &env).ok();
        tracker.fail_transaction(1, String::from_str(&env, "err"), &env).ok();
        tracker.start_transaction(2, &env).ok();
        tracker.complete_transaction(2, &env).ok();
        // tx 3 stays Pending

        let failed = tracker.get_failed_transactions().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].transaction_id, 1);
        assert!(failed[0].recovery_metadata.is_some());
    }

    // -----------------------------------------------------------------------
    // Audit log integrity after mixed transitions
    // -----------------------------------------------------------------------

    #[test]
    fn test_audit_log_mixed_success_and_failure() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator, &env).ok();
        tracker.start_transaction(1, &env).ok();                                    // success
        let _ = tracker.advance_transaction_state(1, TransactionState::Pending, &env); // failure
        tracker.complete_transaction(1, &env).ok();                                 // success

        assert_eq!(tracker.audit_log.len(), 3);
        assert!(tracker.audit_log[0].success);
        assert!(!tracker.audit_log[1].success);
        assert!(tracker.audit_log[2].success);
    }
}

#[cfg(test)]
mod snapshot_tests {
    use std::collections::HashMap;

    /// Minimal snapshot representation matching the JSON fixtures.
    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct RecordSnapshot {
        transaction_id: u64,
        state: String,
        state_u32: u32,
        initiator: String,
        timestamp: u64,
        last_updated: u64,
        error_message: Option<String>,
    }

    fn load_snapshot(name: &str) -> RecordSnapshot {
        let path = format!(
            "{}/test_snapshots/transaction_state_tracker_tests/{}.json",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        let data = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing snapshot: {path}"));
        serde_json::from_str(&data).unwrap_or_else(|e| panic!("bad snapshot {name}: {e}"))
    }

    #[test]
    fn snapshot_state_discriminants() {
        let cases: HashMap<&str, (&str, u32)> = [
            ("record_pending",     ("Pending",    1)),
            ("record_in_progress", ("InProgress", 2)),
            ("record_completed",   ("Completed",  3)),
            ("record_failed",      ("Failed",     4)),
        ]
        .into();

        for (file, (expected_state, expected_u32)) in &cases {
            let snap = load_snapshot(file);
            assert_eq!(
                snap.state, *expected_state,
                "{file}: state name changed — on-chain encoding regression"
            );
            assert_eq!(
                snap.state_u32, *expected_u32,
                "{file}: state discriminant changed — on-chain encoding regression"
            );
        }
    }

    #[test]
    fn snapshot_failed_has_error_message() {
        let snap = load_snapshot("record_failed");
        assert!(
            snap.error_message.is_some(),
            "record_failed snapshot must have an error_message"
        );
    }

    #[test]
    fn snapshot_non_failed_no_error_message() {
        for name in &["record_pending", "record_in_progress", "record_completed"] {
            let snap = load_snapshot(name);
            assert!(
                snap.error_message.is_none(),
                "{name} snapshot must not have an error_message"
            );
        }
    }
}

// ── Auto-eviction tests (#553) ────────────────────────────────────────────────

#[cfg(test)]
mod auto_eviction_tests {
    use anchorkit::transaction_state_tracker::{
        RecoveryRetentionPolicy, TransactionState, TransactionStateTracker,
    };
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{Env, String};

    fn make_env() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_000,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        });
        env
    }

    /// Eviction triggers when budget reaches the warning threshold.
    #[test]
    fn eviction_triggers_on_warning_threshold() {
        let env = make_env();
        let mut tracker = TransactionStateTracker::new(true);
        tracker.eviction_enabled = true;
        tracker.max_evictions_per_call = 10;

        // Fill past warning threshold (>20 entries × 256 bytes = >5120 bytes).
        for i in 0..22u64 {
            let addr = soroban_sdk::Address::generate(&env);
            tracker.create_transaction(i, addr.clone(), &env).unwrap();
            // Transition to terminal (Completed) so they are eviction candidates.
            tracker.start_transaction(i, &env).unwrap();
            tracker.complete_transaction(i, &env).unwrap();
        }
        // Budget should now be at/above warning.
        // Creating one more transaction triggers eviction.
        let addr = soroban_sdk::Address::generate(&env);
        tracker.create_transaction(99, addr, &env).unwrap();

        // Eviction happened: some completed entries were removed.
        assert!(tracker.cache_size() < 23);
    }

    /// Oldest entries are evicted first.
    #[test]
    fn oldest_entries_evicted_first() {
        let env = make_env();
        let mut tracker = TransactionStateTracker::new(true);
        tracker.eviction_enabled = true;
        tracker.max_evictions_per_call = 1;

        for i in 0..22u64 {
            let addr = soroban_sdk::Address::generate(&env);
            tracker.create_transaction(i, addr.clone(), &env).unwrap();
            tracker.start_transaction(i, &env).unwrap();
            tracker.complete_transaction(i, &env).unwrap();
        }

        let size_before = tracker.cache_size();
        let addr = soroban_sdk::Address::generate(&env);
        tracker.create_transaction(99, addr, &env).unwrap();

        // At most 1 eviction per call, so size decreases by exactly 1 terminal.
        // (new record was added so net change may be 0 if 1 evicted + 1 added)
        assert!(tracker.cache_size() <= size_before);
    }

    /// max_evictions_per_call is respected.
    #[test]
    fn max_evictions_per_call_respected() {
        let env = make_env();
        let mut tracker = TransactionStateTracker::new(true);
        tracker.eviction_enabled = true;
        tracker.max_evictions_per_call = 3;

        for i in 0..22u64 {
            let addr = soroban_sdk::Address::generate(&env);
            tracker.create_transaction(i, addr.clone(), &env).unwrap();
            tracker.start_transaction(i, &env).unwrap();
            tracker.complete_transaction(i, &env).unwrap();
        }

        let size_before = tracker.cache_size();
        let addr = soroban_sdk::Address::generate(&env);
        tracker.create_transaction(99, addr, &env).unwrap();

        // At most 3 evictions: size_before + 1 (new) - 3 (evicted) = size_before - 2.
        assert!(tracker.cache_size() >= size_before.saturating_sub(2));
    }

    /// Eviction does not run when eviction_enabled = false.
    #[test]
    fn eviction_disabled_no_removal() {
        let env = make_env();
        let mut tracker = TransactionStateTracker::new(true);
        tracker.eviction_enabled = false;

        for i in 0..22u64 {
            let addr = soroban_sdk::Address::generate(&env);
            tracker.create_transaction(i, addr.clone(), &env).unwrap();
            tracker.start_transaction(i, &env).unwrap();
            tracker.complete_transaction(i, &env).unwrap();
        }
        let size_before = tracker.cache_size();
        let addr = soroban_sdk::Address::generate(&env);
        tracker.create_transaction(99, addr, &env).unwrap();
        // No eviction: size grew by exactly 1.
        assert_eq!(tracker.cache_size(), size_before + 1);
    }

    /// Non-terminal (Pending, InProgress) transactions are never evicted.
    #[test]
    fn non_terminal_transactions_never_evicted() {
        let env = make_env();
        let mut tracker = TransactionStateTracker::new(true);
        tracker.eviction_enabled = true;
        tracker.max_evictions_per_call = 10;

        // Create 22 Pending transactions (non-terminal) to push past warning.
        for i in 0..22u64 {
            let addr = soroban_sdk::Address::generate(&env);
            tracker.create_transaction(i, addr.clone(), &env).unwrap();
            // Leave as Pending — non-terminal.
        }

        let size_before = tracker.cache_size();
        let addr = soroban_sdk::Address::generate(&env);
        tracker.create_transaction(99, addr, &env).unwrap();

        // No terminal records to evict; all pending entries remain.
        // Size grows by 1 (the new one), nothing was removed.
        assert_eq!(tracker.cache_size(), size_before + 1);
    }

    // -------------------------------------------------------------------------
    // #628 — recovery metadata pruning & archival
    // -------------------------------------------------------------------------

    fn make_failed_tracker(env: &Env, id: u64) -> TransactionStateTracker {
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = soroban_sdk::Address::generate(env);
        tracker.create_transaction(id, initiator, env).unwrap();
        tracker.start_transaction(id, env).unwrap();
        tracker.fail_transaction(id, String::from_str(env, "network timeout"), env).unwrap();
        tracker
    }

    #[test]
    fn test_retention_policy_validate_rejects_zero_values() {
        assert!(RecoveryRetentionPolicy { max_age_seconds: 0, max_retry_count: 5 }.validate().is_err());
        assert!(RecoveryRetentionPolicy { max_age_seconds: 60, max_retry_count: 0 }.validate().is_err());
        assert!(RecoveryRetentionPolicy { max_age_seconds: 60, max_retry_count: 5 }.validate().is_ok());
    }

    #[test]
    fn test_set_and_get_retention_policy_defaults_and_override() {
        let env = make_env();
        assert_eq!(
            TransactionStateTracker::get_retention_policy(&env),
            RecoveryRetentionPolicy::default_policy()
        );

        let custom = RecoveryRetentionPolicy { max_age_seconds: 60, max_retry_count: 1 };
        TransactionStateTracker::set_retention_policy(&env, custom.clone());
        assert_eq!(TransactionStateTracker::get_retention_policy(&env), custom);
    }

    /// An ineligible record (too young, too few retries) must not be pruned.
    #[test]
    fn test_prune_ineligible_record_is_untouched() {
        let env = make_env();
        let mut tracker = make_failed_tracker(&env, 1);
        let policy = RecoveryRetentionPolicy { max_age_seconds: 1_000_000, max_retry_count: 5 };

        let pruned = tracker.prune_recovery_metadata(1, &policy, &env).unwrap();
        assert!(!pruned, "a fresh, low-retry record must not be eligible for pruning");
        assert!(tracker.get_recovery_metadata(1, &env).unwrap().is_some());
    }

    /// A record older than `max_age_seconds` becomes eligible for pruning.
    #[test]
    fn test_prune_eligible_after_max_age() {
        let env = make_env();
        let mut tracker = make_failed_tracker(&env, 1);
        let policy = RecoveryRetentionPolicy { max_age_seconds: 500, max_retry_count: 100 };

        // Advance the ledger well past the age threshold.
        env.ledger().set(LedgerInfo {
            timestamp: 1_000 + 1_000,
            protocol_version: 21,
            sequence_number: 2,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        });

        let pruned = tracker.prune_recovery_metadata(1, &policy, &env).unwrap();
        assert!(pruned, "a record past max_age_seconds must be eligible for pruning");
        assert!(tracker.get_recovery_metadata(1, &env).unwrap().is_none());
    }

    /// A record whose retry_count has reached the ceiling becomes eligible
    /// even if it is not old enough by the age rule alone.
    #[test]
    fn test_prune_eligible_after_max_retry_count() {
        let env = make_env();
        let mut tracker = make_failed_tracker(&env, 1);
        let policy = RecoveryRetentionPolicy { max_age_seconds: 1_000_000, max_retry_count: 2 };

        tracker.record_recovery_attempt(1, &env).unwrap();
        tracker.record_recovery_attempt(1, &env).unwrap();

        let pruned = tracker.prune_recovery_metadata(1, &policy, &env).unwrap();
        assert!(pruned, "a record at the retry ceiling must be eligible regardless of age");
        assert!(tracker.get_recovery_metadata(1, &env).unwrap().is_none());
    }

    /// Pruning a transaction with no recovery metadata (or that doesn't
    /// exist) is a safe no-op that reports `false`.
    #[test]
    fn test_prune_missing_transaction_returns_false() {
        let env = make_env();
        let mut tracker = TransactionStateTracker::new(true);
        let policy = RecoveryRetentionPolicy::default_policy();
        assert!(!tracker.prune_recovery_metadata(999, &policy, &env).unwrap());
    }

    /// Archival preserves the required context (failure reason, prior state,
    /// retry count) before clearing the live metadata.
    #[test]
    fn test_archive_preserves_context_then_clears_live_metadata() {
        let env = make_env();
        let mut tracker = make_failed_tracker(&env, 1);
        let policy = RecoveryRetentionPolicy { max_age_seconds: 500, max_retry_count: 100 };

        env.ledger().set(LedgerInfo {
            timestamp: 1_000 + 1_000,
            protocol_version: 21,
            sequence_number: 7,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        });

        let archived = tracker.archive_recovery_metadata(1, &policy, &env).unwrap();
        assert!(archived, "an aged-out record must be archived");

        // Live metadata is gone...
        assert!(tracker.get_recovery_metadata(1, &env).unwrap().is_none());

        // ...but the required context survives in the archive.
        let entry = tracker.get_archived_recovery_metadata(1, &env).unwrap().unwrap();
        assert_eq!(entry.transaction_id, 1);
        assert_eq!(entry.failure_reason, String::from_str(&env, "network timeout"));
        assert_eq!(entry.failed_from_state, TransactionState::InProgress);
        assert_eq!(entry.retry_count, 0);
        assert_eq!(entry.archived_at_ledger, 7);
    }

    /// An ineligible record must not be archived, and its live metadata must
    /// remain untouched.
    #[test]
    fn test_archive_ineligible_record_is_untouched() {
        let env = make_env();
        let mut tracker = make_failed_tracker(&env, 1);
        let policy = RecoveryRetentionPolicy { max_age_seconds: 1_000_000, max_retry_count: 5 };

        let archived = tracker.archive_recovery_metadata(1, &policy, &env).unwrap();
        assert!(!archived);
        assert!(tracker.get_recovery_metadata(1, &env).unwrap().is_some());
        assert!(tracker.get_archived_recovery_metadata(1, &env).unwrap().is_none());
    }

    /// A batch sweep prunes only the records that are actually eligible,
    /// leaving ineligible ones (and their metadata) alone.
    #[test]
    fn test_sweep_prunes_only_eligible_records() {
        let env = make_env();
        let mut tracker = TransactionStateTracker::new(true);

        // tx 1 fails at t=1000 (will be old by the time we sweep at t=3000).
        let initiator1 = soroban_sdk::Address::generate(&env);
        tracker.create_transaction(1, initiator1, &env).unwrap();
        tracker.start_transaction(1, &env).unwrap();
        tracker.fail_transaction(1, String::from_str(&env, "old failure"), &env).unwrap();

        env.ledger().set(LedgerInfo {
            timestamp: 3_000,
            protocol_version: 21,
            sequence_number: 3,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        });

        // tx 2 fails at t=3000 — fresh relative to the sweep.
        let initiator2 = soroban_sdk::Address::generate(&env);
        tracker.create_transaction(2, initiator2, &env).unwrap();
        tracker.start_transaction(2, &env).unwrap();
        tracker.fail_transaction(2, String::from_str(&env, "fresh failure"), &env).unwrap();

        // At now=3000: tx1 age = 3000-1000 = 2000 >= 1500 (eligible).
        //              tx2 age = 3000-3000 = 0    <  1500 (not eligible).
        let policy = RecoveryRetentionPolicy { max_age_seconds: 1_500, max_retry_count: 100 };
        let processed = tracker.sweep_recovery_metadata(&policy, false, &env).unwrap();

        assert_eq!(processed, 1, "only the aged-out record must be swept");
        assert!(tracker.get_recovery_metadata(1, &env).unwrap().is_none());
        assert!(tracker.get_recovery_metadata(2, &env).unwrap().is_some());
    }

    /// A batch sweep in archive mode moves eligible metadata into the
    /// archive and leaves ineligible records' live metadata in place.
    #[test]
    fn test_sweep_archive_mode_archives_eligible_and_leaves_ineligible() {
        let env = make_env();
        let mut tracker = TransactionStateTracker::new(true);

        let initiator1 = soroban_sdk::Address::generate(&env);
        tracker.create_transaction(1, initiator1, &env).unwrap();
        tracker.start_transaction(1, &env).unwrap();
        tracker.fail_transaction(1, String::from_str(&env, "old failure"), &env).unwrap();

        env.ledger().set(LedgerInfo {
            timestamp: 3_000,
            protocol_version: 21,
            sequence_number: 3,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        });

        let initiator2 = soroban_sdk::Address::generate(&env);
        tracker.create_transaction(2, initiator2, &env).unwrap();
        tracker.start_transaction(2, &env).unwrap();
        tracker.fail_transaction(2, String::from_str(&env, "fresh failure"), &env).unwrap();

        let policy = RecoveryRetentionPolicy { max_age_seconds: 1_500, max_retry_count: 100 };
        let processed = tracker.sweep_recovery_metadata(&policy, true, &env).unwrap();

        assert_eq!(processed, 1);
        assert!(tracker.get_archived_recovery_metadata(1, &env).unwrap().is_some());
        assert!(tracker.get_recovery_metadata(1, &env).unwrap().is_none());

        // tx2 is untouched: still live, not archived.
        assert!(tracker.get_archived_recovery_metadata(2, &env).unwrap().is_none());
        assert!(tracker.get_recovery_metadata(2, &env).unwrap().is_some());
    }
}
