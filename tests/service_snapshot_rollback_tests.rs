#![cfg(test)]

mod service_snapshot_rollback_tests {
    use anchorkit::service_management::{ServiceConfigSnapshot, ServiceManager};
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{Address, Env, Vec};

    // -- helpers ------------------------------------------------------

    fn make_env() -> Env {
        Env::default()
    }

    fn make_anchor(env: &Env) -> Address {
        Address::generate(env)
    }

    fn set_time(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 22,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1,
            min_persistent_entry_ttl: 1,
            max_entry_ttl: 100_000_000,
        });
    }

    // -- Blank snapshot name tests ----------------------------------------------------

    #[test]
    fn test_blank_snapshot_name_rejected() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let mut svcs = Vec::new(&env);
        svcs.push_back(1u32);

        let err = ServiceManager::create_snapshot(&env, &anchor, &svcs, "")
            .expect_err("empty snapshot name must be rejected");
        assert_eq!(
            err.code,
            anchorkit::ErrorCode::InvalidTemplate,
            "expected InvalidTemplate error for blank snapshot name"
        );
    }

    #[test]
    fn test_whitespace_only_snapshot_name_rejected() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let mut svcs = Vec::new(&env);
        svcs.push_back(1u32);

        let err = ServiceManager::create_snapshot(&env, &anchor, &svcs, "   ")
            .expect_err("whitespace-only snapshot name must be rejected");
        assert_eq!(
            err.code,
            anchorkit::ErrorCode::InvalidTemplate,
            "expected InvalidTemplate error for whitespace-only snapshot name"
        );
    }

    // -- Duplicate service entries are rejected --------------------------------------

    #[test]
    fn test_duplicate_services_rejected() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let mut svcs = Vec::new(&env);
        svcs.push_back(1u32);
        svcs.push_back(1u32);

        let err = ServiceManager::create_snapshot(&env, &anchor, &svcs, "dupes")
            .expect_err("duplicate services must be rejected");
        assert_eq!(
            err.code,
            anchorkit::ErrorCode::InvalidTemplate,
            "expected InvalidTemplate error for duplicate services"
        );
    }

    // -- Over-limit service lists are rejected ---------------------------------------

    #[test]
    fn test_over_limit_services_rejected() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let mut svcs = Vec::new(&env);
        // Exceed the configured service-count limit.
        for i in 0..(ServiceManager::MAX_SERVICES + 1) {
            svcs.push_back(i);
        }

        let err = ServiceManager::create_snapshot(&env, &anchor, &svcs, "too_many")
            .expect_err("over-limit service list must be rejected");
        assert_eq!(
            err.code,
            anchorkit::ErrorCode::InvalidTemplate,
            "expected InvalidTemplate error for over-limit service list"
        );
    }

    // -- Valid snapshot creation -----------------------------------------------------

    #[test]
    fn test_valid_snapshot_creation() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let mut svcs = Vec::new(&env);
        svcs.push_back(1u32);
        svcs.push_back(2u32);

        set_time(&env, 1_000_000);
        let snapshot_id =
            ServiceManager::create_snapshot(&env, &anchor, &svcs, "before_upgrade")
                .expect("valid snapshot name must succeed");
        assert_eq!(snapshot_id, 0);

        let snapshot: ServiceConfigSnapshot =
            ServiceManager::get_snapshot(&env, snapshot_id).expect("snapshot must exist");
        assert_eq!(snapshot.anchor, anchor);
        assert_eq!(snapshot.services.len(), 2);
        assert_eq!(snapshot.created_at, 1_000_000);
        assert_eq!(snapshot.description.to_buffer(), "before_upgrade".as_bytes());
    }

    // -- Rollback preserves services -----------------------------------------------------

    #[test]
    fn test_rollback_restores_snapshot_services() {
        let env = make_env();
        let anchor = make_anchor(&env);

        // Enable initial services
        ServiceManager::enable_service(&env, &anchor, 1).unwrap();
        ServiceManager::enable_service(&env, &anchor, 2).unwrap();
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 1));
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 2));

        // Create snapshot of current state
        let mut snapshot_svcs = Vec::new(&env);
        snapshot_svcs.push_back(1u32);
        snapshot_svcs.push_back(2u32);
        set_time(&env, 1_000_000);
        let snap_id =
            ServiceManager::create_snapshot(&env, &anchor, &snapshot_svcs, "checkpoint")
                .unwrap();

        // Disable a service (drift from snapshot)
        ServiceManager::disable_service(&env, &anchor, 1).unwrap();
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 1));

        // Rollback to snapshot
        let rolled_back = ServiceManager::rollback_to_snapshot(&env, snap_id);
        assert!(rolled_back);
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 1));
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 2));
    }

    // -- Out-of-range rollback index fails without mutation ----------------------

    #[test]
    fn test_rollback_out_of_range_index_fails_without_mutation() {
        let env = make_env();
        let anchor = make_anchor(&env);
        ServiceManager::enable_service(&env, &anchor, 1).unwrap();
        let mut snapshot_svcs = Vec::new(&env);
        snapshot_svcs.push_back(1u32);
        set_time(&env, 1_000_000);
        let snap_id = ServiceManager::create_snapshot(&env, &anchor, &snapshot_svcs, "checkpoint").unwrap();
        assert_eq!(snap_id, 0);
        ServiceManager::disable_service(&env, &anchor, 1).unwrap();
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 1));
        let rolled_back = ServiceManager::rollback_to_snapshot(&env, snap_id + 1);
        assert!(!rolled_back);
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 1));
    }

    // -- Duplicate service entries are rejected before mutation ------------------

    #[test]
    fn test_rollback_rejects_duplicate_services_without_mutation() {
        let env = make_env();
        let anchor = make_anchor(&env);

        ServiceManager::enable_service(&env, &anchor, 1).unwrap();
        ServiceManager::enable_service(&env, &anchor, 2).unwrap();

        // Snapshot with a duplicated service entry is inconsistent.
        let mut snapshot_svcs = Vec::new(&env);
        snapshot_svcs.push_back(1u32);
        snapshot_svcs.push_back(1u32);
        set_time(&env, 1_000_000);
        let snap_id =
            ServiceManager::create_snapshot(&env, &anchor, &snapshot_svcs, "dupes").unwrap();

        // Drift the live state away from the snapshot.
        ServiceManager::disable_service(&env, &anchor, 1).unwrap();
        ServiceManager::disable_service(&env, &anchor, 2).unwrap();
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 1));
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 2));

        // Invalid snapshot must fail before any mutation occurs.
        let rolled_back = ServiceManager::rollback_to_snapshot(&env, snap_id);
        assert!(!rolled_back);
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 1));
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 2));
    }

    // -- Missing prerequisite services are rejected before mutation --------------

    #[test]
    fn test_rollback_rejects_missing_prerequisite_without_mutation() {
        let env = make_env();
        let anchor = make_anchor(&env);

        ServiceManager::enable_service(&env, &anchor, 1).unwrap();
        ServiceManager::enable_service(&env, &anchor, 2).unwrap();

        // Snapshot omits service 1, which service 2 depends on.
        let mut snapshot_svcs = Vec::new(&env);
        snapshot_svcs.push_back(2u32);
        set_time(&env, 1_000_000);
        let snap_id =
            ServiceManager::create_snapshot(&env, &anchor, &snapshot_svcs, "incomplete").unwrap();

        // Drift the live state away from the snapshot.
        ServiceManager::disable_service(&env, &anchor, 1).unwrap();
        ServiceManager::disable_service(&env, &anchor, 2).unwrap();
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 1));
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 2));

        // Dependency-incomplete snapshot must fail before any mutation occurs.
        let rolled_back = ServiceManager::rollback_to_snapshot(&env, snap_id);
        assert!(!rolled_back);
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 1));
        assert!(!ServiceManager::is_service_enabled(&env, &anchor, 2));
    }

    // -- Snapshot count increments correctly ----------------------------

    #[test]
    fn test_snapshot_count_increments() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let mut svcs = Vec::new(&env);
        svcs.push_back(1u32);
        set_time(&env, 1_000_000);
        let first = ServiceManager::create_snapshot(&env, &anchor, &svcs, "first").unwrap();
        let second = ServiceManager::create_snapshot(&env, &anchor, &svcs, "second").unwrap();
        assert_eq!(first, 0);
        assert_eq!(second, 1);
    }
}
