/// Tests for issue #658: Time-based routing policies.
///
/// Covers:
/// - Registering and retrieving timed routing policies.
/// - Enabling and disabling policies.
/// - Active policy selection at various times of day.
/// - Midnight-wrapping windows.
/// - Always-active policies (start == end).
/// - Priority and tie-breaking (lowest policy_id wins at equal priority).
/// - Validation: empty strategy name or out-of-range window seconds rejected.
/// - get_active_routing_policy returns None when no policy matches.
#[cfg(test)]

mod timed_routing_policy_tests {
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env, String,
    };

    use anchorkit::contract::{AnchorKitContract, AnchorKitContractClient};

    fn make_env() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        env
    }

    fn set_ledger(env: &Env, timestamp: u64) {
        env.ledger().set(LedgerInfo {
            timestamp,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });
    }

    fn setup(env: &Env) -> (AnchorKitContractClient, Address) {
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(env, &contract_id);
        let admin = Address::generate(env);
        client.initialize(&admin);
        (client, admin)
    }

    // -----------------------------------------------------------------------
    // Registration and retrieval
    // -----------------------------------------------------------------------

    #[test]
    fn test_register_and_retrieve_policy() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // 09:00 – 17:00 UTC
        let id = client.register_timed_routing_policy(
            &String::from_str(&env, "Business Hours"),
            &String::from_str(&env, "LowestFee"),
            &32400u32, // 09:00
            &61200u32, // 17:00
            &0u32,
        );

        assert_eq!(id, 1);

        let policy = client.get_timed_routing_policy(&id);
        assert!(policy.is_some());
        let p = policy.unwrap();
        assert_eq!(p.policy_id, 1);
        assert!(p.enabled);
        assert_eq!(p.window.window_start_secs, 32400);
        assert_eq!(p.window.window_end_secs, 61200);
    }

    #[test]
    fn test_policy_ids_are_monotonically_increasing() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        let id1 = client.register_timed_routing_policy(
            &String::from_str(&env, "P1"),
            &String::from_str(&env, "LowestFee"),
            &0u32, &0u32, &0u32,
        );
        let id2 = client.register_timed_routing_policy(
            &String::from_str(&env, "P2"),
            &String::from_str(&env, "FastestSettlement"),
            &0u32, &0u32, &0u32,
        );

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    #[test]
    fn test_get_nonexistent_policy_returns_none() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        let result = client.get_timed_routing_policy(&999u64);
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // Enable / disable
    // -----------------------------------------------------------------------

    #[test]
    fn test_disable_policy_excluded_from_active_selection() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        let id = client.register_timed_routing_policy(
            &String::from_str(&env, "Always"),
            &String::from_str(&env, "LowestFee"),
            &0u32, &0u32, // always-active (start == end)
            &0u32,
        );

        // Policy is enabled by default — should be selected at any time.
        let active = client.get_active_routing_policy(&1_000_000u64);
        assert!(active.is_some());

        // Disable it.
        client.set_timed_policy_enabled(&id, &false);

        let active_after = client.get_active_routing_policy(&1_000_000u64);
        assert!(active_after.is_none());
    }

    #[test]
    fn test_re_enable_policy_resumes_selection() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        let id = client.register_timed_routing_policy(
            &String::from_str(&env, "Always"),
            &String::from_str(&env, "WeightedScore"),
            &0u32, &0u32,
            &0u32,
        );
        client.set_timed_policy_enabled(&id, &false);
        client.set_timed_policy_enabled(&id, &true);

        let active = client.get_active_routing_policy(&1_000_000u64);
        assert!(active.is_some());
    }

    // -----------------------------------------------------------------------
    // Active policy evaluation
    // -----------------------------------------------------------------------

    #[test]
    fn test_policy_active_within_window() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // 10:00 – 12:00 UTC  (36 000 – 43 200 seconds-since-midnight)
        client.register_timed_routing_policy(
            &String::from_str(&env, "Morning"),
            &String::from_str(&env, "FastestSettlement"),
            &36_000u32,
            &43_200u32,
            &0u32,
        );

        // 11:00 UTC = 39 600 seconds into the day
        // Use a UNIX timestamp where (ts % 86400) == 39600:
        // 86400 * N + 39600; pick N=11 → 950 400 + 39 600 = 990 000
        let ts_inside = 990_000u64;
        assert_eq!(ts_inside % 86_400, 39_600);

        let active = client.get_active_routing_policy(&ts_inside);
        assert!(active.is_some());
        assert_eq!(
            active.unwrap(),
            String::from_str(&env, "FastestSettlement")
        );
    }

    #[test]
    fn test_policy_inactive_outside_window() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // 10:00 – 12:00 UTC
        client.register_timed_routing_policy(
            &String::from_str(&env, "Morning"),
            &String::from_str(&env, "FastestSettlement"),
            &36_000u32,
            &43_200u32,
            &0u32,
        );

        // 15:00 UTC = 54 000 seconds into the day
        // 86400 * 11 + 54000 = 1 004 400
        let ts_outside = 1_004_400u64;
        assert_eq!(ts_outside % 86_400, 54_000);

        let active = client.get_active_routing_policy(&ts_outside);
        assert!(active.is_none());
    }

    #[test]
    fn test_midnight_wrapping_window_active_after_midnight() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // 22:00 – 02:00 UTC  (79 200 – 7 200): wraps midnight
        client.register_timed_routing_policy(
            &String::from_str(&env, "Night"),
            &String::from_str(&env, "HighestReputation"),
            &79_200u32,
            &7_200u32,
            &0u32,
        );

        // 01:00 UTC = 3 600 seconds into the day (inside window)
        let ts_inside = 86_400u64 * 12 + 3_600; // = 1 036 800
        assert_eq!(ts_inside % 86_400, 3_600);

        let active = client.get_active_routing_policy(&ts_inside);
        assert!(active.is_some());
    }

    #[test]
    fn test_midnight_wrapping_window_inactive_in_midday() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // 22:00 – 02:00 UTC
        client.register_timed_routing_policy(
            &String::from_str(&env, "Night"),
            &String::from_str(&env, "HighestReputation"),
            &79_200u32,
            &7_200u32,
            &0u32,
        );

        // 12:00 UTC = 43 200 seconds (outside window)
        let ts_outside = 86_400u64 * 12 + 43_200;
        assert_eq!(ts_outside % 86_400, 43_200);

        let active = client.get_active_routing_policy(&ts_outside);
        assert!(active.is_none());
    }

    #[test]
    fn test_always_active_policy_selected_at_any_time() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // start == end → always-active
        client.register_timed_routing_policy(
            &String::from_str(&env, "Always"),
            &String::from_str(&env, "LowestFee"),
            &12_000u32,
            &12_000u32,
            &0u32,
        );

        for ts in [0u64, 43_200, 86_399, 1_000_000] {
            let active = client.get_active_routing_policy(&ts);
            assert!(active.is_some(), "Expected policy to be active at ts={ts}");
        }
    }

    // -----------------------------------------------------------------------
    // Priority and tie-breaking
    // -----------------------------------------------------------------------

    #[test]
    fn test_lower_priority_value_wins() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // Both always-active.
        client.register_timed_routing_policy(
            &String::from_str(&env, "Low priority"),
            &String::from_str(&env, "LowestFee"),
            &0u32, &0u32,
            &10u32, // priority 10
        );
        client.register_timed_routing_policy(
            &String::from_str(&env, "High priority"),
            &String::from_str(&env, "FastestSettlement"),
            &0u32, &0u32,
            &1u32, // priority 1 — should win
        );

        let active = client.get_active_routing_policy(&1_000_000u64);
        assert!(active.is_some());
        assert_eq!(active.unwrap(), String::from_str(&env, "FastestSettlement"));
    }

    #[test]
    fn test_tied_priority_lower_policy_id_wins() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // Both always-active, same priority.
        client.register_timed_routing_policy(
            &String::from_str(&env, "P1"),
            &String::from_str(&env, "WeightedScore"), // id=1
            &0u32, &0u32,
            &5u32,
        );
        client.register_timed_routing_policy(
            &String::from_str(&env, "P2"),
            &String::from_str(&env, "LowestFee"),     // id=2
            &0u32, &0u32,
            &5u32,
        );

        // id=1 has lower id → should win tie-break.
        let active = client.get_active_routing_policy(&1_000_000u64);
        assert!(active.is_some());
        assert_eq!(active.unwrap(), String::from_str(&env, "WeightedScore"));
    }

    #[test]
    fn test_no_active_policy_returns_none() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // No policies registered at all.
        let active = client.get_active_routing_policy(&1_000_000u64);
        assert!(active.is_none());
    }

    // -----------------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------------

    #[test]
    #[should_panic]
    fn test_register_policy_rejects_empty_strategy_name() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        client.register_timed_routing_policy(
            &String::from_str(&env, "Name"),
            &String::from_str(&env, ""), // empty — should panic
            &0u32, &0u32,
            &0u32,
        );
    }

    #[test]
    #[should_panic]
    fn test_register_policy_rejects_invalid_window_start() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // 86 400 is >= 86 400 → invalid
        client.register_timed_routing_policy(
            &String::from_str(&env, "Name"),
            &String::from_str(&env, "LowestFee"),
            &86_400u32, // invalid
            &0u32,
            &0u32,
        );
    }

    #[test]
    #[should_panic]
    fn test_register_policy_rejects_invalid_window_end() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        client.register_timed_routing_policy(
            &String::from_str(&env, "Name"),
            &String::from_str(&env, "LowestFee"),
            &0u32,
            &86_400u32, // invalid
            &0u32,
        );
    }

    #[test]
    #[should_panic]
    fn test_set_enabled_on_nonexistent_policy_panics() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        client.set_timed_policy_enabled(&999u64, &false);
    }
}
