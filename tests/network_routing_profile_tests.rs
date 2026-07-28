/// Tests for issue #659: Per-network routing profiles.
///
/// Covers:
/// - Registering and retrieving named network routing profiles.
/// - Setting the active network context.
/// - get_routing_profile returns the active network profile.
/// - get_routing_profile falls back to the default profile when no active match.
/// - get_routing_profile returns None when neither active nor default is set.
/// - Multiple profiles coexist; only the active network's profile is returned.
/// - Default profile fallback when active network profile is missing.
/// - Validation: empty network name is rejected.
/// - Validation: empty strategy name is rejected.
/// - Validation: weights not summing to 1 000 are rejected.
#[cfg(test)]

mod network_routing_profile_tests {
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

    fn register_testnet_profile(env: &Env, client: &AnchorKitContractClient, is_default: bool) {
        client.register_network_routing_profile(
            &String::from_str(env, "testnet"),
            &String::from_str(env, "LowestFee"),
            &400u32, &300u32, &300u32, // weights sum to 1 000
            &0u32,
            &is_default,
        );
    }

    fn register_mainnet_profile(env: &Env, client: &AnchorKitContractClient, is_default: bool) {
        client.register_network_routing_profile(
            &String::from_str(env, "mainnet"),
            &String::from_str(env, "WeightedScore"),
            &333u32, &333u32, &334u32,
            &5000u32, // higher minimum reputation on mainnet
            &is_default,
        );
    }

    // -----------------------------------------------------------------------
    // Registration and retrieval
    // -----------------------------------------------------------------------

    #[test]
    fn test_register_and_retrieve_profile() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        register_testnet_profile(&env, &client, false);

        let profile = client.get_network_routing_profile(&String::from_str(&env, "testnet"));
        assert!(profile.is_some());
        let p = profile.unwrap();
        assert_eq!(p.network_name, String::from_str(&env, "testnet"));
        assert_eq!(p.fee_weight, 400);
        assert_eq!(p.speed_weight, 300);
        assert_eq!(p.reputation_weight, 300);
        assert_eq!(p.min_reputation, 0);
        assert!(!p.is_default);
    }

    #[test]
    fn test_get_nonexistent_profile_returns_none() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        let result = client.get_network_routing_profile(&String::from_str(&env, "unknown"));
        assert!(result.is_none());
    }

    #[test]
    fn test_multiple_profiles_coexist() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        register_testnet_profile(&env, &client, false);
        register_mainnet_profile(&env, &client, false);

        let testnet = client.get_network_routing_profile(&String::from_str(&env, "testnet"));
        let mainnet = client.get_network_routing_profile(&String::from_str(&env, "mainnet"));

        assert!(testnet.is_some());
        assert!(mainnet.is_some());
        assert_eq!(testnet.unwrap().min_reputation, 0);
        assert_eq!(mainnet.unwrap().min_reputation, 5000);
    }

    // -----------------------------------------------------------------------
    // Active network context
    // -----------------------------------------------------------------------

    #[test]
    fn test_set_and_get_active_network() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        client.set_active_network(&String::from_str(&env, "testnet"));
        let active = client.get_active_network();
        assert!(active.is_some());
        assert_eq!(active.unwrap(), String::from_str(&env, "testnet"));
    }

    #[test]
    fn test_get_active_network_returns_none_when_unset() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        let active = client.get_active_network();
        assert!(active.is_none());
    }

    // -----------------------------------------------------------------------
    // Profile resolution via get_routing_profile
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_routing_profile_returns_active_network_profile() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        register_testnet_profile(&env, &client, false);
        register_mainnet_profile(&env, &client, true); // mainnet is default

        client.set_active_network(&String::from_str(&env, "testnet"));

        let profile = client.get_routing_profile();
        assert!(profile.is_some());
        // Should return testnet (active network), not mainnet (default).
        assert_eq!(
            profile.unwrap().network_name,
            String::from_str(&env, "testnet")
        );
    }

    #[test]
    fn test_get_routing_profile_falls_back_to_default_when_active_missing() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // Register only a default profile, no active network set.
        register_mainnet_profile(&env, &client, true);

        let profile = client.get_routing_profile();
        assert!(profile.is_some());
        assert_eq!(
            profile.unwrap().network_name,
            String::from_str(&env, "mainnet")
        );
    }

    #[test]
    fn test_get_routing_profile_falls_back_to_default_when_active_profile_not_registered() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // Set active network to "local" but no "local" profile exists.
        register_mainnet_profile(&env, &client, true);
        client.set_active_network(&String::from_str(&env, "local"));

        let profile = client.get_routing_profile();
        // Should fall back to mainnet default.
        assert!(profile.is_some());
        assert_eq!(
            profile.unwrap().network_name,
            String::from_str(&env, "mainnet")
        );
    }

    #[test]
    fn test_get_routing_profile_returns_none_when_nothing_registered() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        let profile = client.get_routing_profile();
        assert!(profile.is_none());
    }

    #[test]
    fn test_registering_new_default_replaces_old_default() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // testnet is the first default.
        register_testnet_profile(&env, &client, true);
        // mainnet replaces it as default.
        register_mainnet_profile(&env, &client, true);

        // No active network set → should get mainnet (most recent default).
        let profile = client.get_routing_profile();
        assert!(profile.is_some());
        assert_eq!(
            profile.unwrap().network_name,
            String::from_str(&env, "mainnet")
        );
    }

    #[test]
    fn test_default_profile_includes_correct_weights() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        register_mainnet_profile(&env, &client, true);

        let profile = client.get_routing_profile().unwrap();
        assert_eq!(profile.fee_weight + profile.speed_weight + profile.reputation_weight, 1_000);
    }

    // -----------------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------------

    #[test]
    #[should_panic]
    fn test_register_profile_rejects_empty_network_name() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        client.register_network_routing_profile(
            &String::from_str(&env, ""), // empty
            &String::from_str(&env, "LowestFee"),
            &400u32, &300u32, &300u32,
            &0u32,
            &false,
        );
    }

    #[test]
    #[should_panic]
    fn test_register_profile_rejects_empty_strategy_name() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        client.register_network_routing_profile(
            &String::from_str(&env, "testnet"),
            &String::from_str(&env, ""), // empty
            &400u32, &300u32, &300u32,
            &0u32,
            &false,
        );
    }

    #[test]
    #[should_panic]
    fn test_register_profile_rejects_weights_not_summing_to_1000() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        // 400 + 300 + 200 = 900 ≠ 1 000
        client.register_network_routing_profile(
            &String::from_str(&env, "testnet"),
            &String::from_str(&env, "LowestFee"),
            &400u32, &300u32, &200u32, // invalid
            &0u32,
            &false,
        );
    }

    #[test]
    #[should_panic]
    fn test_set_active_network_rejects_empty_name() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        client.set_active_network(&String::from_str(&env, ""));
    }
}
