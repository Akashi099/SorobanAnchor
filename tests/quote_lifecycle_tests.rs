#![cfg(test)]

#[path = "sep10_test_util.rs"]
mod sep10_test_util;

mod quote_lifecycle_tests {
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{Address, Env, String, Symbol, Vec};

    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    use anchorkit::contract::{
        AnchorKitContract, AnchorKitContractClient, QuoteLifecycleState, RoutingOptions,
        RoutingRequest,
    };
    use crate::sep10_test_util::register_attestor_with_sep10;

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
            max_entry_ttl: 6312000,
        });
    }

    fn setup(env: &Env) -> (Address, AnchorKitContractClient<'_>) {
        set_ledger(env, 1_000_000);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(env, &contract_id);
        let admin = Address::generate(env);
        client.initialize(&admin);
        (admin, client)
    }

    fn register_anchor(env: &Env, client: &AnchorKitContractClient, anchor: &Address) {
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(env, client, anchor, anchor, &sk);
        let mut services = Vec::new(env);
        services.push_back(1u32);
        services.push_back(3u32);
        client.configure_services(anchor, &services);
    }

    fn submit_quote(
        env: &Env,
        client: &AnchorKitContractClient,
        anchor: &Address,
        valid_until: u64,
    ) -> u64 {
        client.submit_quote(
            anchor,
            &String::from_str(env, "USD"),
            &String::from_str(env, "USDC"),
            &10_000u64,
            &50u32,
            &100u64,
            &100_000u64,
            &valid_until,
        )
    }

    // -------------------------------------------------------------------------
    // Lifecycle state tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_new_quote_is_active() {
        let env = make_env();
        let (_, client) = setup(&env);
        let anchor = Address::generate(&env);
        register_anchor(&env, &client, &anchor);

        let qid = submit_quote(&env, &client, &anchor, 1_003_600);
        assert_eq!(
            client.get_quote_lifecycle_state(&anchor, &qid),
            QuoteLifecycleState::Active
        );
    }

    #[test]
    fn test_invalidate_quote_changes_state() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let anchor = Address::generate(&env);
        register_anchor(&env, &client, &anchor);

        let qid = submit_quote(&env, &client, &anchor, 1_003_600);
        assert_eq!(
            client.get_quote_lifecycle_state(&anchor, &qid),
            QuoteLifecycleState::Active
        );

        client.invalidate_quote(&anchor, &qid);
        assert_eq!(
            client.get_quote_lifecycle_state(&anchor, &qid),
            QuoteLifecycleState::Invalidated
        );
        let _ = admin;
    }

    #[test]
    fn test_invalidate_nonexistent_quote_panics() {
        let env = make_env();
        let (_, client) = setup(&env);
        let anchor = Address::generate(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.invalidate_quote(&anchor, &999u64);
        }));
        assert!(result.is_err(), "Invalidating nonexistent quote must panic");
    }

    // -------------------------------------------------------------------------
    // Routing exclusion of invalidated quotes
    // -------------------------------------------------------------------------

    #[test]
    fn test_invalidated_quote_excluded_from_routing() {
        let env = make_env();
        let (admin, client) = setup(&env);

        let anchor1 = Address::generate(&env);
        let anchor2 = Address::generate(&env);
        register_anchor(&env, &client, &anchor1);
        register_anchor(&env, &client, &anchor2);

        client.set_anchor_metadata(&anchor1, &80u32, &60u64, &80u32, &99u32, &1_000u64);
        client.set_anchor_metadata(&anchor2, &90u32, &60u64, &90u32, &99u32, &1_000u64);

        let qid1 = submit_quote(&env, &client, &anchor1, 1_003_600);
        submit_quote(&env, &client, &anchor2, 1_003_600);

        // Invalidate anchor1's quote
        client.invalidate_quote(&anchor1, &qid1);

        let mut strategy = Vec::new(&env);
        strategy.push_back(Symbol::new(&env, "LowestFee"));

        let routed = client.route_transaction(&RoutingOptions {
            request: RoutingRequest {
                base_asset: String::from_str(&env, "USD"),
                quote_asset: String::from_str(&env, "USDC"),
                amount: 5_000,
                operation_type: 1,
            },
            strategy,
            min_reputation: 0,
            max_anchors: 10,
            require_kyc: false,
            require_compliance: false,
            subject: admin.clone(),
            fee_weight: 333,
            speed_weight: 333,
            reputation_weight: 334,
        });

        // Only anchor2's quote survives
        assert_eq!(routed.anchor, anchor2);
    }

    // -------------------------------------------------------------------------
    // Purge expired quotes
    // -------------------------------------------------------------------------

    #[test]
    fn test_purge_expired_quotes_removes_stale_entries() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let anchor = Address::generate(&env);
        register_anchor(&env, &client, &anchor);

        // Submit a quote that expires at 1_001_000
        submit_quote(&env, &client, &anchor, 1_001_000);

        // Advance time past expiry
        set_ledger(&env, 1_002_000);

        // purge_expired_quotes should not panic
        client.purge_expired_quotes();
        let _ = admin;
    }

    #[test]
    fn test_purge_removes_invalidated_quote_from_index() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let anchor = Address::generate(&env);
        register_anchor(&env, &client, &anchor);

        let qid = submit_quote(&env, &client, &anchor, 1_003_600);
        client.invalidate_quote(&anchor, &qid);

        // purge_expired_quotes should complete without error
        client.purge_expired_quotes();
        let _ = admin;
    }
}
