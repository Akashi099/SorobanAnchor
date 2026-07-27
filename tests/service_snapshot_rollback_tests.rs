//! Tests for service configuration snapshots and rollback (task c).
//!
//! Verifies that:
//! - snapshot_services captures the current service set and returns a snapshot id.
//! - rollback_services restores a prior configuration on success.
//! - rollback_services returns false for a non-existent snapshot id.
//! - get_service_snapshot retrieves the stored snapshot.
//! - get_service_snapshot_count returns the total snapshot count.
//! - Rollback fires the cache-invalidation hook (capabilities cache is cleared).
//! - Multiple snapshots for the same anchor are independent.
//! - Rolling back to an earlier snapshot then a later snapshot works correctly.

#![cfg(test)]

mod service_snapshot_rollback_tests {
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env, Vec,
    };
    use anchorkit::contract::{AnchorKitContract, AnchorKitContractClient};

    const SERVICE_DEPOSITS:    u32 = 1;
    const SERVICE_WITHDRAWALS: u32 = 2;
    const SERVICE_QUOTES:      u32 = 3;
    const SERVICE_KYC:         u32 = 4;

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
            sequence_number: ts as u32,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        });
    }

    fn setup(env: &Env) -> (Address, AnchorKitContractClient<'_>) {
        set_ledger(env, 1000);
        let cid = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(env, &cid);
        let admin = Address::generate(env);
        client.initialize(&admin);
        (admin, client)
    }

    fn svc_vec(env: &Env, codes: &[u32]) -> Vec<u32> {
        let mut v = Vec::new(env);
        for c in codes { v.push_back(*c); }
        v
    }

    // -----------------------------------------------------------------------
    // Snapshot creation
    // -----------------------------------------------------------------------

    #[test]
    fn snapshot_services_returns_sequential_ids() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        let id0 = client.snapshot_services(
            &anchor,
            &svc_vec(&env, &[SERVICE_DEPOSITS]),
            &soroban_sdk::String::from_str(&env, "snap0"),
        );
        let id1 = client.snapshot_services(
            &anchor,
            &svc_vec(&env, &[SERVICE_DEPOSITS, SERVICE_WITHDRAWALS]),
            &soroban_sdk::String::from_str(&env, "snap1"),
        );

        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
    }

    #[test]
    fn get_service_snapshot_returns_stored_data() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        set_ledger(&env, 2000);
        let sid = client.snapshot_services(
            &anchor,
            &svc_vec(&env, &[SERVICE_DEPOSITS, SERVICE_QUOTES]),
            &soroban_sdk::String::from_str(&env, "before_upgrade"),
        );

        let snap = client.get_service_snapshot(&sid).unwrap();
        assert_eq!(snap.snapshot_id, sid);
        assert_eq!(snap.anchor,      anchor);
        assert_eq!(snap.services.len(), 2);
        assert_eq!(snap.created_at,  2000);
        assert_eq!(snap.description, soroban_sdk::String::from_str(&env, "before_upgrade"));
    }

    #[test]
    fn get_service_snapshot_returns_none_for_missing_id() {
        let env = make_env();
        let (_admin, client) = setup(&env);

        let snap = client.get_service_snapshot(&9999u64);
        assert!(snap.is_none());
    }

    #[test]
    fn get_service_snapshot_count_increments_correctly() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        assert_eq!(client.get_service_snapshot_count(), 0);

        client.snapshot_services(
            &anchor,
            &svc_vec(&env, &[SERVICE_DEPOSITS]),
            &soroban_sdk::String::from_str(&env, "s1"),
        );
        assert_eq!(client.get_service_snapshot_count(), 1);

        client.snapshot_services(
            &anchor,
            &svc_vec(&env, &[SERVICE_WITHDRAWALS]),
            &soroban_sdk::String::from_str(&env, "s2"),
        );
        assert_eq!(client.get_service_snapshot_count(), 2);
    }

    // -----------------------------------------------------------------------
    // Rollback — success path
    // -----------------------------------------------------------------------

    #[test]
    fn rollback_services_restores_prior_configuration() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        // Enable deposits + withdrawals, then snapshot
        client.enable_service(&anchor, &SERVICE_DEPOSITS);
        client.enable_service(&anchor, &SERVICE_WITHDRAWALS);

        let sid = client.snapshot_services(
            &anchor,
            &svc_vec(&env, &[SERVICE_DEPOSITS, SERVICE_WITHDRAWALS]),
            &soroban_sdk::String::from_str(&env, "pre_change"),
        );

        // Now enable quotes as well (change the live state)
        client.enable_service(&anchor, &SERVICE_QUOTES);
        assert!(client.is_service_enabled(&anchor, &SERVICE_QUOTES));

        // Roll back — quotes should disappear
        let ok = client.rollback_services(&sid);
        assert!(ok, "rollback should succeed");

        let state = client.get_service_toggle_state(&anchor);
        assert_eq!(state.enabled_services.len(), 2, "should be back to 2 services");
        assert!(!client.is_service_enabled(&anchor, &SERVICE_QUOTES),
            "rolled-back state must not include the service added after snapshot");
    }

    #[test]
    fn rollback_services_returns_false_for_nonexistent_snapshot() {
        let env = make_env();
        let (_admin, client) = setup(&env);

        let ok = client.rollback_services(&9999u64);
        assert!(!ok, "rollback to non-existent snapshot must return false");
    }

    // -----------------------------------------------------------------------
    // Rollback — cache invalidation hook
    // -----------------------------------------------------------------------

    #[test]
    fn rollback_services_fires_cache_invalidation_hook() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        set_ledger(&env, 1000);
        // Seed the capabilities cache
        let toml_url = soroban_sdk::String::from_str(&env, "https://anchor.example.com/.well-known/stellar.toml");
        let caps     = soroban_sdk::String::from_str(&env, "SEP6");
        client.cache_capabilities(&anchor, &toml_url, &caps, &3600u64);
        assert!(client.get_cache_diagnostics(&anchor).capabilities_cached);

        // Create a snapshot and roll back — the hook must clear the capabilities cache
        let sid = client.snapshot_services(
            &anchor,
            &svc_vec(&env, &[SERVICE_DEPOSITS]),
            &soroban_sdk::String::from_str(&env, "snap"),
        );
        client.rollback_services(&sid);

        assert!(
            !client.get_cache_diagnostics(&anchor).capabilities_cached,
            "rollback must fire the cache-invalidation hook"
        );
    }

    // -----------------------------------------------------------------------
    // Multiple rollbacks between snapshots
    // -----------------------------------------------------------------------

    #[test]
    fn can_rollback_to_earlier_then_later_snapshot() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        // Snapshot A: only deposits
        let sid_a = client.snapshot_services(
            &anchor,
            &svc_vec(&env, &[SERVICE_DEPOSITS]),
            &soroban_sdk::String::from_str(&env, "a"),
        );

        // Snapshot B: deposits + withdrawals
        let sid_b = client.snapshot_services(
            &anchor,
            &svc_vec(&env, &[SERVICE_DEPOSITS, SERVICE_WITHDRAWALS]),
            &soroban_sdk::String::from_str(&env, "b"),
        );

        // Roll to B → 2 services
        client.rollback_services(&sid_b);
        assert_eq!(client.get_service_toggle_state(&anchor).enabled_services.len(), 2);

        // Roll back to A → 1 service
        client.rollback_services(&sid_a);
        assert_eq!(client.get_service_toggle_state(&anchor).enabled_services.len(), 1);

        // Roll forward to B again → 2 services
        client.rollback_services(&sid_b);
        assert_eq!(client.get_service_toggle_state(&anchor).enabled_services.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Snapshots for different anchors are independent
    // -----------------------------------------------------------------------

    #[test]
    fn snapshots_for_different_anchors_are_independent() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor_a = Address::generate(&env);
        let anchor_b = Address::generate(&env);

        let sid_a = client.snapshot_services(
            &anchor_a,
            &svc_vec(&env, &[SERVICE_DEPOSITS]),
            &soroban_sdk::String::from_str(&env, "a"),
        );
        let sid_b = client.snapshot_services(
            &anchor_b,
            &svc_vec(&env, &[SERVICE_QUOTES, SERVICE_KYC]),
            &soroban_sdk::String::from_str(&env, "b"),
        );

        // Roll both back — each affects only its own anchor
        client.rollback_services(&sid_a);
        client.rollback_services(&sid_b);

        let state_a = client.get_service_toggle_state(&anchor_a);
        let state_b = client.get_service_toggle_state(&anchor_b);

        assert_eq!(state_a.enabled_services.len(), 1);
        assert_eq!(state_b.enabled_services.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Audit log records snapshot and rollback actions
    // -----------------------------------------------------------------------

    #[test]
    fn snapshot_and_rollback_are_recorded_in_admin_audit_log() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        use anchorkit::admin_audit_log::AdminAuditLog;

        let cid = client.address.clone();

        let sid = client.snapshot_services(
            &anchor,
            &svc_vec(&env, &[SERVICE_DEPOSITS]),
            &soroban_sdk::String::from_str(&env, "test"),
        );
        client.rollback_services(&sid);

        env.as_contract(&cid, || {
            let count = AdminAuditLog::get_entry_count(&env);
            // At minimum two entries: one for snapshot, one for rollback
            assert!(count >= 2, "expected at least 2 audit entries, got {count}");
        });
    }
}
