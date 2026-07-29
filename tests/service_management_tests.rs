//! Tests for anchor service enable/disable toggles, service rollback handling,
//! and the structured service retirement lifecycle.
//!
//! These tests verify that:
//! 1. Services can be enabled/disabled individually
//! 2. Service state is tracked correctly
//! 3. Service configuration snapshots can be created
//! 4. Rollback to previous configurations works
//! 5. Multiple services can be managed together
//! 6. Services can be retired through the structured lifecycle
//! 7. Invalid retirement transitions are rejected
//! 8. Retirement records persist and carry full history

#![cfg(test)]

mod service_management_tests {
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{Address, Env, Vec};

    use anchorkit::service_management::{ServiceManager, ServiceToggleState, ServiceConfigSnapshot};
    use anchorkit::service_management::ServiceRetirementState;

    // Service codes
    const SERVICE_DEPOSITS: u32 = 1;
    const SERVICE_WITHDRAWALS: u32 = 2;
    const SERVICE_QUOTES: u32 = 3;
    const SERVICE_KYC: u32 = 4;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_env() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        env
    }

    fn set_ledger(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        });
    }

    fn create_service_vec(env: &Env, services: &[u32]) -> Vec<u32> {
        let mut vec = Vec::new(env);
        for service in services {
            vec.push_back(*service);
        }
        vec
    }

    // -----------------------------------------------------------------------
    // Service Enable/Disable Tests
    // -----------------------------------------------------------------------

    /// Test that a service can be enabled
    #[test]
    fn service_can_be_enabled() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);

        let result = ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);
        assert!(result);

        let state = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state.enabled_services.len(), 1);
        assert_eq!(state.enabled_services.get(0).unwrap(), SERVICE_DEPOSITS);
    }

    /// Test that a service can be disabled
    #[test]
    fn service_can_be_disabled() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);

        // Enable a service first
        ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);

        // Disable it
        let result = ServiceManager::disable_service(&env, &anchor, SERVICE_DEPOSITS);
        assert!(result);

        let state = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state.enabled_services.len(), 0);
        assert_eq!(state.disabled_services.len(), 1);
    }

    /// Test that enabling an already enabled service returns false
    #[test]
    fn enabling_already_enabled_service_returns_false() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);

        ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);
        let result = ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);

        assert!(!result);
    }

    /// Test that disabling an already disabled service returns false
    #[test]
    fn disabling_already_disabled_service_returns_false() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);

        ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);
        ServiceManager::disable_service(&env, &anchor, SERVICE_DEPOSITS);
        let result = ServiceManager::disable_service(&env, &anchor, SERVICE_DEPOSITS);

        assert!(!result);
    }

    /// Test that multiple services can be enabled
    #[test]
    fn multiple_services_can_be_enabled() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);

        ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);
        ServiceManager::enable_service(&env, &anchor, SERVICE_WITHDRAWALS);
        ServiceManager::enable_service(&env, &anchor, SERVICE_QUOTES);

        let state = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state.enabled_services.len(), 3);
    }

    /// Test that services can be selectively disabled
    #[test]
    fn services_can_be_selectively_disabled() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);

        ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);
        ServiceManager::enable_service(&env, &anchor, SERVICE_WITHDRAWALS);
        ServiceManager::enable_service(&env, &anchor, SERVICE_QUOTES);

        ServiceManager::disable_service(&env, &anchor, SERVICE_WITHDRAWALS);

        let state = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state.enabled_services.len(), 2);
        assert_eq!(state.disabled_services.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Service Status Query Tests
    // -----------------------------------------------------------------------

    /// Test that service enabled status can be queried
    #[test]
    fn service_enabled_status_can_be_queried() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);

        ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);

        assert!(ServiceManager::is_service_enabled(&env, &anchor, SERVICE_DEPOSITS));
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, SERVICE_WITHDRAWALS));
    }

    /// Test that disabled service returns false
    #[test]
    fn disabled_service_returns_false() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);

        ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);
        ServiceManager::disable_service(&env, &anchor, SERVICE_DEPOSITS);

        assert!(!ServiceManager::is_service_enabled(&env, &anchor, SERVICE_DEPOSITS));
    }

    // -----------------------------------------------------------------------
    // Snapshot Tests
    // -----------------------------------------------------------------------

    /// Test that a service configuration snapshot can be created
    #[test]
    fn service_configuration_snapshot_can_be_created() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);
        let services = create_service_vec(&env, &[SERVICE_DEPOSITS, SERVICE_WITHDRAWALS]);

        let snapshot_id = ServiceManager::create_snapshot(
            &env,
            &anchor,
            &services,
            "initial_config",
        );

        assert_eq!(snapshot_id, 0);

        let snapshot = ServiceManager::get_snapshot(&env, snapshot_id).unwrap();
        assert_eq!(snapshot.snapshot_id, 0);
        assert_eq!(snapshot.anchor, anchor);
        assert_eq!(snapshot.services.len(), 2);
    }

    /// Test that multiple snapshots can be created
    #[test]
    fn multiple_snapshots_can_be_created() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);
        let services1 = create_service_vec(&env, &[SERVICE_DEPOSITS]);
        let services2 = create_service_vec(&env, &[SERVICE_DEPOSITS, SERVICE_WITHDRAWALS]);

        let snap1 = ServiceManager::create_snapshot(&env, &anchor, &services1, "config_v1");
        let snap2 = ServiceManager::create_snapshot(&env, &anchor, &services2, "config_v2");

        assert_eq!(snap1, 0);
        assert_eq!(snap2, 1);

        let snapshot1 = ServiceManager::get_snapshot(&env, snap1).unwrap();
        let snapshot2 = ServiceManager::get_snapshot(&env, snap2).unwrap();

        assert_eq!(snapshot1.services.len(), 1);
        assert_eq!(snapshot2.services.len(), 2);
    }

    /// Test that snapshot includes timestamp
    #[test]
    fn snapshot_includes_timestamp() {
        let env = make_env();
        set_ledger(&env, 5000);

        let anchor = Address::generate(&env);
        let services = create_service_vec(&env, &[SERVICE_DEPOSITS]);

        let snapshot_id = ServiceManager::create_snapshot(&env, &anchor, &services, "test");
        let snapshot = ServiceManager::get_snapshot(&env, snapshot_id).unwrap();

        assert_eq!(snapshot.created_at, 5000);
    }

    /// Test that snapshot includes description
    #[test]
    fn snapshot_includes_description() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);
        let services = create_service_vec(&env, &[SERVICE_DEPOSITS]);

        let snapshot_id = ServiceManager::create_snapshot(
            &env,
            &anchor,
            &services,
            "before_maintenance",
        );
        let snapshot = ServiceManager::get_snapshot(&env, snapshot_id).unwrap();

        assert_eq!(snapshot.description, soroban_sdk::String::from_str(&env, "before_maintenance"));
    }

    /// Test that snapshot count is tracked
    #[test]
    fn snapshot_count_is_tracked() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);
        let services = create_service_vec(&env, &[SERVICE_DEPOSITS]);

        assert_eq!(ServiceManager::get_snapshot_count(&env), 0);

        ServiceManager::create_snapshot(&env, &anchor, &services, "snap1");
        assert_eq!(ServiceManager::get_snapshot_count(&env), 1);

        ServiceManager::create_snapshot(&env, &anchor, &services, "snap2");
        assert_eq!(ServiceManager::get_snapshot_count(&env), 2);
    }

    // -----------------------------------------------------------------------
    // Rollback Tests
    // -----------------------------------------------------------------------

    /// Test that rollback to a snapshot works
    #[test]
    fn rollback_to_snapshot_works() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);
        let services = create_service_vec(&env, &[SERVICE_DEPOSITS, SERVICE_WITHDRAWALS]);

        // Create snapshot
        let snapshot_id = ServiceManager::create_snapshot(&env, &anchor, &services, "initial");

        // Change services
        ServiceManager::enable_service(&env, &anchor, SERVICE_QUOTES);
        ServiceManager::disable_service(&env, &anchor, SERVICE_WITHDRAWALS);

        let state_before = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state_before.enabled_services.len(), 2); // DEPOSITS, QUOTES

        // Rollback
        let result = ServiceManager::rollback_to_snapshot(&env, snapshot_id);
        assert!(result);

        let state_after = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state_after.enabled_services.len(), 2); // DEPOSITS, WITHDRAWALS
    }

    /// Test that rollback to non-existent snapshot returns false
    #[test]
    fn rollback_to_non_existent_snapshot_returns_false() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);

        let result = ServiceManager::rollback_to_snapshot(&env, 999);
        assert!(!result);
    }

    /// Test that multiple rollbacks can be performed
    #[test]
    fn multiple_rollbacks_can_be_performed() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);
        let services1 = create_service_vec(&env, &[SERVICE_DEPOSITS]);
        let services2 = create_service_vec(&env, &[SERVICE_DEPOSITS, SERVICE_WITHDRAWALS]);

        let snap1 = ServiceManager::create_snapshot(&env, &anchor, &services1, "config1");
        let snap2 = ServiceManager::create_snapshot(&env, &anchor, &services2, "config2");

        // Rollback to snap2
        ServiceManager::rollback_to_snapshot(&env, snap2);
        let state = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state.enabled_services.len(), 2);

        // Rollback to snap1
        ServiceManager::rollback_to_snapshot(&env, snap1);
        let state = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state.enabled_services.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Bulk Operations Tests
    // -----------------------------------------------------------------------

    /// Test that all services can be enabled at once
    #[test]
    fn all_services_can_be_enabled_at_once() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);
        let all_services = create_service_vec(&env, &[
            SERVICE_DEPOSITS,
            SERVICE_WITHDRAWALS,
            SERVICE_QUOTES,
            SERVICE_KYC,
        ]);

        ServiceManager::enable_all_services(&env, &anchor, &all_services);

        let state = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state.enabled_services.len(), 4);
        assert_eq!(state.disabled_services.len(), 0);
    }

    /// Test that all services can be disabled at once
    #[test]
    fn all_services_can_be_disabled_at_once() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);
        let all_services = create_service_vec(&env, &[
            SERVICE_DEPOSITS,
            SERVICE_WITHDRAWALS,
            SERVICE_QUOTES,
            SERVICE_KYC,
        ]);

        ServiceManager::enable_all_services(&env, &anchor, &all_services);
        ServiceManager::disable_all_services(&env, &anchor, &all_services);

        let state = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state.enabled_services.len(), 0);
        assert_eq!(state.disabled_services.len(), 4);
    }

    // -----------------------------------------------------------------------
    // State Persistence Tests
    // -----------------------------------------------------------------------

    /// Test that service state persists across queries
    #[test]
    fn service_state_persists_across_queries() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor = Address::generate(&env);

        ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);

        let state1 = ServiceManager::get_service_state(&env, &anchor);
        let state2 = ServiceManager::get_service_state(&env, &anchor);

        assert_eq!(state1.enabled_services.len(), 1);
        assert_eq!(state2.enabled_services.len(), 1);
    }

    /// Test that different anchors have independent service states
    #[test]
    fn different_anchors_have_independent_states() {
        let env = make_env();
        set_ledger(&env, 1000);

        let anchor1 = Address::generate(&env);
        let anchor2 = Address::generate(&env);

        ServiceManager::enable_service(&env, &anchor1, SERVICE_DEPOSITS);
        ServiceManager::enable_service(&env, &anchor2, SERVICE_WITHDRAWALS);

        let state1 = ServiceManager::get_service_state(&env, &anchor1);
        let state2 = ServiceManager::get_service_state(&env, &anchor2);

        assert_eq!(state1.enabled_services.len(), 1);
        assert_eq!(state2.enabled_services.len(), 1);
        assert_eq!(state1.enabled_services.get(0).unwrap(), SERVICE_DEPOSITS);
        assert_eq!(state2.enabled_services.get(0).unwrap(), SERVICE_WITHDRAWALS);
    }

    /// Test that service state includes update timestamp
    #[test]
    fn service_state_includes_update_timestamp() {
        let env = make_env();
        set_ledger(&env, 5000);

        let anchor = Address::generate(&env);

        ServiceManager::enable_service(&env, &anchor, SERVICE_DEPOSITS);

        let state = ServiceManager::get_service_state(&env, &anchor);
        assert_eq!(state.updated_at, 5000);
    }
}

// =============================================================================
// Service Retirement Lifecycle Tests
// =============================================================================

mod retirement_lifecycle_tests {
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{Address, Env};
    use anchorkit::service_management::{ServiceManager, ServiceRetirementState};

    const SVC_DEPOSITS: u32 = 1;
    const SVC_WITHDRAWALS: u32 = 2;

    fn make_env() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        env
    }

    fn set_ts(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        });
    }

    // -----------------------------------------------------------------------
    // Default state
    // -----------------------------------------------------------------------

    /// A service that has never been touched is implicitly Active.
    #[test]
    fn new_service_is_active_by_default() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);
        assert_eq!(
            ServiceManager::get_retirement_state(&env, &anchor, SVC_DEPOSITS),
            ServiceRetirementState::Active
        );
    }

    // -----------------------------------------------------------------------
    // Valid transition path
    // -----------------------------------------------------------------------

    /// Full lifecycle: Active → Deprecated → Disabled → Retired.
    #[test]
    fn full_retirement_path_succeeds() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        let rec = ServiceManager::deprecate_service(
            &env, &anchor, SVC_DEPOSITS, "Deposits EOL in Q4", None,
        ).unwrap();
        assert_eq!(rec.state, ServiceRetirementState::Deprecated);
        assert!(rec.deprecation_notice.is_some());

        let rec = ServiceManager::disable_for_retirement(&env, &anchor, SVC_DEPOSITS, None).unwrap();
        assert_eq!(rec.state, ServiceRetirementState::Disabled);

        let rec = ServiceManager::retire_service_lifecycle(&env, &anchor, SVC_DEPOSITS).unwrap();
        assert_eq!(rec.state, ServiceRetirementState::Retired);
        assert!(rec.state.is_terminal());
    }

    /// Deprecated service can be restored to Active (un-deprecate).
    #[test]
    fn undeprecate_restores_active() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "tentative", None).unwrap();
        let rec = ServiceManager::undeprecate_service(&env, &anchor, SVC_DEPOSITS).unwrap();
        assert_eq!(rec.state, ServiceRetirementState::Active);
    }

    /// After un-deprecation the full retirement path can be restarted.
    #[test]
    fn re_deprecation_after_undeprecation_succeeds() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "first try", None).unwrap();
        ServiceManager::undeprecate_service(&env, &anchor, SVC_DEPOSITS).unwrap();
        let rec = ServiceManager::deprecate_service(
            &env, &anchor, SVC_DEPOSITS, "second try", None,
        ).unwrap();
        assert_eq!(rec.state, ServiceRetirementState::Deprecated);
    }

    // -----------------------------------------------------------------------
    // Invalid transitions — each must return Err with [E65] prefix
    // -----------------------------------------------------------------------

    /// Active → Disabled is forbidden (must go through Deprecated first).
    #[test]
    fn active_to_disabled_rejected() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        let err = ServiceManager::disable_for_retirement(&env, &anchor, SVC_DEPOSITS, None)
            .unwrap_err();
        assert!(err.contains("[E65]"), "expected [E65] prefix: {err}");
        assert!(err.contains("active"),   "expected 'active' in: {err}");
        assert!(err.contains("disabled"), "expected 'disabled' in: {err}");
    }

    /// Active → Retired is forbidden.
    #[test]
    fn active_to_retired_rejected() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        let err = ServiceManager::retire_service_lifecycle(&env, &anchor, SVC_DEPOSITS)
            .unwrap_err();
        assert!(err.contains("[E65]"), "expected [E65] prefix: {err}");
    }

    /// Deprecated → Retired without disabling first is forbidden.
    #[test]
    fn deprecated_to_retired_without_disabling_rejected() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "notice", None).unwrap();
        let err = ServiceManager::retire_service_lifecycle(&env, &anchor, SVC_DEPOSITS)
            .unwrap_err();
        assert!(err.contains("[E65]"), "expected [E65] prefix: {err}");
        assert!(err.contains("deprecated"), "expected 'deprecated' in: {err}");
        assert!(err.contains("retired"),    "expected 'retired' in: {err}");
    }

    /// Disabled → Active is forbidden (no reversal once disabled).
    #[test]
    fn disabled_to_active_rejected() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "notice", None).unwrap();
        ServiceManager::disable_for_retirement(&env, &anchor, SVC_DEPOSITS, None).unwrap();

        let err = ServiceManager::undeprecate_service(&env, &anchor, SVC_DEPOSITS).unwrap_err();
        assert!(err.contains("[E65]"), "expected [E65] prefix: {err}");
    }

    /// Disabled → Deprecated is forbidden.
    #[test]
    fn disabled_to_deprecated_rejected() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "notice", None).unwrap();
        ServiceManager::disable_for_retirement(&env, &anchor, SVC_DEPOSITS, None).unwrap();

        let err = ServiceManager::transition_retirement(
            &env, &anchor, SVC_DEPOSITS,
            ServiceRetirementState::Deprecated,
            None, None, None,
        ).unwrap_err();
        assert!(err.contains("[E65]"), "expected [E65] prefix: {err}");
    }

    /// Retired is terminal — no further transitions allowed.
    #[test]
    fn retired_service_blocks_all_further_transitions() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "notice", None).unwrap();
        ServiceManager::disable_for_retirement(&env, &anchor, SVC_DEPOSITS, None).unwrap();
        ServiceManager::retire_service_lifecycle(&env, &anchor, SVC_DEPOSITS).unwrap();

        let targets = [
            ServiceRetirementState::Active,
            ServiceRetirementState::Deprecated,
            ServiceRetirementState::Disabled,
            ServiceRetirementState::Retired,
        ];
        for target in targets {
            let result = ServiceManager::transition_retirement(
                &env, &anchor, SVC_DEPOSITS, target, None, None, None,
            );
            assert!(result.is_err(),
                "Retired → {target:?} should be rejected");
            assert!(result.unwrap_err().contains("[E65]"));
        }
    }

    /// Self-loop: Active → Active is rejected.
    #[test]
    fn same_state_self_loop_rejected() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        let err = ServiceManager::transition_retirement(
            &env, &anchor, SVC_DEPOSITS,
            ServiceRetirementState::Active,
            None, None, None,
        ).unwrap_err();
        assert!(err.contains("[E65]"));
    }

    // -----------------------------------------------------------------------
    // Record content and persistence
    // -----------------------------------------------------------------------

    /// Deprecation notice is stored on the record.
    #[test]
    fn deprecation_notice_persisted() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(
            &env, &anchor, SVC_DEPOSITS, "Will be removed in Q4 2026", None,
        ).unwrap();

        let rec = ServiceManager::get_retirement_record(&env, &anchor, SVC_DEPOSITS);
        let notice = rec.deprecation_notice.unwrap();
        assert_eq!(notice, soroban_sdk::String::from_str(&env, "Will be removed in Q4 2026"));
    }

    /// Planned timestamps are stored when provided.
    #[test]
    fn planned_disable_and_retire_timestamps_stored() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "notice", Some(2000)).unwrap();
        let rec = ServiceManager::disable_for_retirement(&env, &anchor, SVC_DEPOSITS, Some(3000)).unwrap();

        assert_eq!(rec.planned_disable_at, Some(2000));
        assert_eq!(rec.planned_retire_at,  Some(3000));
    }

    /// `last_updated` advances with each transition.
    #[test]
    fn last_updated_advances_with_each_transition() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "n", None).unwrap();
        let r1 = ServiceManager::get_retirement_record(&env, &anchor, SVC_DEPOSITS);
        assert_eq!(r1.last_updated, 1000);

        set_ts(&env, 4000);
        ServiceManager::disable_for_retirement(&env, &anchor, SVC_DEPOSITS, None).unwrap();
        let r2 = ServiceManager::get_retirement_record(&env, &anchor, SVC_DEPOSITS);
        assert_eq!(r2.last_updated, 4000);

        set_ts(&env, 8000);
        ServiceManager::retire_service_lifecycle(&env, &anchor, SVC_DEPOSITS).unwrap();
        let r3 = ServiceManager::get_retirement_record(&env, &anchor, SVC_DEPOSITS);
        assert_eq!(r3.last_updated, 8000);
    }

    /// Full state history is recorded in order.
    #[test]
    fn state_history_records_all_transitions_in_order() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "n", None).unwrap();
        ServiceManager::disable_for_retirement(&env, &anchor, SVC_DEPOSITS, None).unwrap();
        ServiceManager::retire_service_lifecycle(&env, &anchor, SVC_DEPOSITS).unwrap();

        let rec = ServiceManager::get_retirement_record(&env, &anchor, SVC_DEPOSITS);
        assert_eq!(rec.state_history.len(), 3);
        assert_eq!(rec.state_history.get(0).unwrap().0, ServiceRetirementState::Deprecated as u32);
        assert_eq!(rec.state_history.get(1).unwrap().0, ServiceRetirementState::Disabled   as u32);
        assert_eq!(rec.state_history.get(2).unwrap().0, ServiceRetirementState::Retired     as u32);
    }

    /// History is not appended on a rejected transition.
    #[test]
    fn failed_transition_does_not_append_to_history() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        // Attempt invalid Active → Disabled
        let _ = ServiceManager::disable_for_retirement(&env, &anchor, SVC_DEPOSITS, None);

        let rec = ServiceManager::get_retirement_record(&env, &anchor, SVC_DEPOSITS);
        assert_eq!(rec.state_history.len(), 0, "failed transition must not append to history");
        assert_eq!(rec.state, ServiceRetirementState::Active);
    }

    // -----------------------------------------------------------------------
    // Isolation
    // -----------------------------------------------------------------------

    /// Different service codes on the same anchor are independent.
    #[test]
    fn different_service_codes_are_independent() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "deposits EOL", None).unwrap();

        assert_eq!(
            ServiceManager::get_retirement_state(&env, &anchor, SVC_WITHDRAWALS),
            ServiceRetirementState::Active,
            "withdrawals should still be Active"
        );
    }

    /// Different anchors have independent retirement records.
    #[test]
    fn different_anchors_have_independent_retirement_records() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor1 = Address::generate(&env);
        let anchor2 = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor1, SVC_DEPOSITS, "anchor1 EOL", None).unwrap();

        assert_eq!(
            ServiceManager::get_retirement_state(&env, &anchor2, SVC_DEPOSITS),
            ServiceRetirementState::Active,
            "anchor2 service should be unaffected"
        );
    }

    /// Toggle enable/disable is independent of retirement state.
    #[test]
    fn toggle_and_retirement_are_independent() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        // Enable the service via toggle
        ServiceManager::enable_service(&env, &anchor, SVC_DEPOSITS);
        assert!(ServiceManager::is_service_enabled(&env, &anchor, SVC_DEPOSITS));

        // Deprecate via retirement lifecycle — toggle state unchanged
        ServiceManager::deprecate_service(&env, &anchor, SVC_DEPOSITS, "notice", None).unwrap();
        assert_eq!(
            ServiceManager::get_retirement_state(&env, &anchor, SVC_DEPOSITS),
            ServiceRetirementState::Deprecated
        );
        // Toggle is still enabled (separate subsystem)
        assert!(ServiceManager::is_service_enabled(&env, &anchor, SVC_DEPOSITS));
    }

    // -----------------------------------------------------------------------
    // is_valid_transition matrix (enum-level)
    // -----------------------------------------------------------------------

    #[test]
    fn transition_matrix_valid_paths() {
        use ServiceRetirementState::*;
        assert!(Active.is_valid_transition(Deprecated));
        assert!(Deprecated.is_valid_transition(Active));
        assert!(Deprecated.is_valid_transition(Disabled));
        assert!(Disabled.is_valid_transition(Retired));
    }

    #[test]
    fn transition_matrix_invalid_paths() {
        use ServiceRetirementState::*;
        // Skip steps
        assert!(!Active.is_valid_transition(Disabled));
        assert!(!Active.is_valid_transition(Retired));
        assert!(!Deprecated.is_valid_transition(Retired));
        // No reversal from Disabled
        assert!(!Disabled.is_valid_transition(Active));
        assert!(!Disabled.is_valid_transition(Deprecated));
        // Retired is terminal
        assert!(!Retired.is_valid_transition(Active));
        assert!(!Retired.is_valid_transition(Deprecated));
        assert!(!Retired.is_valid_transition(Disabled));
        assert!(!Retired.is_valid_transition(Retired));
        // Self-loops
        assert!(!Active.is_valid_transition(Active));
        assert!(!Deprecated.is_valid_transition(Deprecated));
        assert!(!Disabled.is_valid_transition(Disabled));
    }
}
