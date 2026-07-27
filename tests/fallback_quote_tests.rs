#![cfg(test)]

#[path = "sep10_test_util.rs"]
mod sep10_test_util;

mod fallback_quote_tests {
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{Address, Env, String, Symbol, Vec};

    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    use anchorkit::contract::{
        AnchorKitContract, AnchorKitContractClient, RoutingOptions, RoutingRequest,
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

    fn base_options(env: &Env, admin: &Address) -> RoutingOptions {
        let mut strategy = Vec::new(env);
        strategy.push_back(Symbol::new(env, "LowestFee"));
        RoutingOptions {
            request: RoutingRequest {
                base_asset: String::from_str(env, "USD"),
                quote_asset: String::from_str(env, "USDC"),
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
        }
    }

    // -------------------------------------------------------------------------
    // Preferred anchor is available — should be selected directly
    // -------------------------------------------------------------------------

    #[test]
    fn test_preferred_anchor_used_when_available() {
        let env = make_env();
        let (admin, client) = setup(&env);

        let preferred = Address::generate(&env);
        let other = Address::generate(&env);
        register_anchor(&env, &client, &preferred);
        register_anchor(&env, &client, &other);

        client.set_anchor_metadata(&preferred, &80u32, &60u64, &80u32, &99u32, &1_000u64);
        client.set_anchor_metadata(&other, &90u32, &60u64, &90u32, &99u32, &1_000u64);

        // preferred has a higher fee but is still valid
        client.submit_quote(
            &preferred,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10_000u64,
            &100u32,
            &100u64,
            &100_000u64,
            &1_003_600u64,
        );
        client.submit_quote(
            &other,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10_000u64,
            &20u32,
            &100u64,
            &100_000u64,
            &1_003_600u64,
        );

        let result = client.route_with_fallback(&base_options(&env, &admin), &preferred);
        assert_eq!(result.anchor, preferred, "Preferred anchor must be selected when available");
    }

    // -------------------------------------------------------------------------
    // Preferred anchor has no quote — fallback to best alternative
    // -------------------------------------------------------------------------

    #[test]
    fn test_fallback_used_when_preferred_has_no_quote() {
        let env = make_env();
        let (admin, client) = setup(&env);

        let preferred = Address::generate(&env);
        let fallback = Address::generate(&env);
        register_anchor(&env, &client, &preferred);
        register_anchor(&env, &client, &fallback);

        client.set_anchor_metadata(&preferred, &80u32, &60u64, &80u32, &99u32, &1_000u64);
        client.set_anchor_metadata(&fallback, &90u32, &60u64, &90u32, &99u32, &1_000u64);

        // Only the fallback anchor has a quote
        client.submit_quote(
            &fallback,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10_000u64,
            &20u32,
            &100u64,
            &100_000u64,
            &1_003_600u64,
        );

        let result = client.route_with_fallback(&base_options(&env, &admin), &preferred);
        assert_eq!(result.anchor, fallback, "Fallback anchor must be used when preferred has no quote");
    }

    // -------------------------------------------------------------------------
    // Preferred anchor quote is expired — fallback triggered
    // -------------------------------------------------------------------------

    #[test]
    fn test_fallback_used_when_preferred_quote_expired() {
        let env = make_env();
        let (admin, client) = setup(&env);

        let preferred = Address::generate(&env);
        let fallback = Address::generate(&env);
        register_anchor(&env, &client, &preferred);
        register_anchor(&env, &client, &fallback);

        client.set_anchor_metadata(&preferred, &80u32, &60u64, &80u32, &99u32, &1_000u64);
        client.set_anchor_metadata(&fallback, &90u32, &60u64, &90u32, &99u32, &1_000u64);

        // Preferred quote expires at t=1_000_500 (500 s from now)
        client.submit_quote(
            &preferred,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10_000u64,
            &50u32,
            &100u64,
            &100_000u64,
            &1_000_500u64,
        );
        // Fallback quote valid for 1 hour
        client.submit_quote(
            &fallback,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10_000u64,
            &30u32,
            &100u64,
            &100_000u64,
            &1_003_600u64,
        );

        // Advance time past preferred's expiry
        set_ledger(&env, 1_001_000);

        let result = client.route_with_fallback(&base_options(&env, &admin), &preferred);
        assert_eq!(result.anchor, fallback, "Fallback anchor must be used when preferred quote is expired");
    }

    // -------------------------------------------------------------------------
    // Preferred anchor is blacklisted — fallback triggered
    // -------------------------------------------------------------------------

    #[test]
    fn test_fallback_used_when_preferred_is_blacklisted() {
        let env = make_env();
        let (admin, client) = setup(&env);

        let preferred = Address::generate(&env);
        let fallback = Address::generate(&env);
        register_anchor(&env, &client, &preferred);
        register_anchor(&env, &client, &fallback);

        client.set_anchor_metadata(&preferred, &80u32, &60u64, &80u32, &99u32, &1_000u64);
        client.set_anchor_metadata(&fallback, &90u32, &60u64, &90u32, &99u32, &1_000u64);

        client.submit_quote(
            &preferred,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10_000u64,
            &50u32,
            &100u64,
            &100_000u64,
            &1_003_600u64,
        );
        client.submit_quote(
            &fallback,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10_000u64,
            &30u32,
            &100u64,
            &100_000u64,
            &1_003_600u64,
        );

        // Blacklist the preferred anchor
        client.blacklist_anchor(
            &preferred,
            &String::from_str(&env, "test_blacklist"),
        );

        let result = client.route_with_fallback(&base_options(&env, &admin), &preferred);
        assert_eq!(result.anchor, fallback, "Fallback anchor must be used when preferred is blacklisted");
        let _ = admin;
    }

    // -------------------------------------------------------------------------
    // No fallback available — must panic with NoQuotesAvailable
    // -------------------------------------------------------------------------

    #[test]
    fn test_no_fallback_panics_when_no_quotes() {
        let env = make_env();
        let (admin, client) = setup(&env);

        let preferred = Address::generate(&env);
        register_anchor(&env, &client, &preferred);
        client.set_anchor_metadata(&preferred, &80u32, &60u64, &80u32, &99u32, &1_000u64);
        // No quotes submitted at all

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.route_with_fallback(&base_options(&env, &admin), &preferred);
        }));
        assert!(result.is_err(), "Must panic when no quotes are available");
    }
}
