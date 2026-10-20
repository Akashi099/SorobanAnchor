/// Tests for optional anchor referral / routing reason metadata (#298).
///
/// Covers:
/// - Quote records store and return `routing_reason` via `submit_quote_with_reason`.
/// - `submit_quote` (no-reason path) stores `routing_reason: None`.
/// - `get_quote_routing_reason` returns the stored reason or `None`.
/// - Transaction records store and preserve `routing_reason` through state transitions.
/// - `create_txn_record_with_reason` sets the reason; plain
///   `create_transaction_record` leaves it `None`.
/// - SEP-38 `FirmQuote` carries an optional routing reason.
/// - Tracker-level `create_transaction_with_reason` persists the reason and
///   subsequent state transitions preserve it.

#[cfg(test)]
mod routing_reason_tests {
    // ── SEP-38 unit tests (no Soroban env needed) ────────────────────────────

    use anchorkit::sep38::{FirmQuote, RawFirmQuote, request_firm_quote};

    fn make_raw_quote() -> RawFirmQuote {
        RawFirmQuote {
            id: "q-001".to_string(),
            expires_at: "9999999999".to_string(),
            price: "1.05".to_string(),
            sell_amount: "100.00".to_string(),
            buy_amount: "105.00".to_string(),
            sell_asset: "xlm".to_string(),
            buy_asset: "usdc".to_string(),
        }
    }

    #[test]
    fn test_firm_quote_routing_reason_defaults_to_none() {
        let quote = request_firm_quote(make_raw_quote(), 0).unwrap();
        assert_eq!(quote.routing_reason, None);
    }

    #[test]
    fn test_firm_quote_routing_reason_can_be_set() {
        let mut quote = request_firm_quote(make_raw_quote(), 0).unwrap();
        quote.routing_reason = Some("lowest_fee".to_string());
        assert_eq!(quote.routing_reason.as_deref(), Some("lowest_fee"));
    }

    #[test]
    fn test_firm_quote_routing_reason_survives_clone() {
        let mut quote = request_firm_quote(make_raw_quote(), 0).unwrap();
        quote.routing_reason = Some("referral".to_string());
        let cloned = quote.clone();
        assert_eq!(cloned.routing_reason.as_deref(), Some("referral"));
    }
}

// ── Contract-level integration tests ─────────────────────────────────────────

#[path = "sep10_test_util.rs"]
mod sep10_test_util;

mod routing_reason_contract_tests {
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env, String,
    };
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    use anchorkit::contract::{AnchorKitContract, AnchorKitContractClient};
    use crate::sep10_test_util::register_attestor_with_sep10;

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

    fn register_anchor(env: &Env, client: &AnchorKitContractClient, anchor: &Address) {
        let signing_key = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(env, client, anchor, anchor, &signing_key);
        let mut services = soroban_sdk::Vec::new(env);
        services.push_back(1u32);
        services.push_back(3u32);
        client.configure_services(anchor, &services);
    }

    // ── Quote routing reason tests ────────────────────────────────────────────

    /// `submit_quote` (no-reason variant) stores `routing_reason: None`.
    #[test]
    fn test_submit_quote_no_reason_is_none() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);
        let anchor = Address::generate(&env);
        register_anchor(&env, &client, &anchor);

        client.submit_quote(
            &anchor,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10000u64, &25u32, &100u64, &100000u64, &1_003_600u64,
        );

        let quote = client.get_quote(&anchor, &1u64);
        assert_eq!(quote.routing_reason, None);
        assert_eq!(client.get_quote_routing_reason(&anchor, &1u64), None);
    }

    /// `submit_quote_with_reason` persists the routing reason in the Quote record.
    #[test]
    fn test_submit_quote_with_reason_stores_reason() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);
        let anchor = Address::generate(&env);
        register_anchor(&env, &client, &anchor);

        client.submit_quote_with_reason(
            &anchor,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10000u64, &25u32, &100u64, &100000u64, &1_003_600u64,
            &Some(String::from_str(&env, "lowest_fee")),
        );

        let quote = client.get_quote(&anchor, &1u64);
        assert_eq!(
            quote.routing_reason,
            Some(String::from_str(&env, "lowest_fee"))
        );
    }

    /// `get_quote_routing_reason` returns the stored reason.
    #[test]
    fn test_get_quote_routing_reason_returns_stored_value() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);
        let anchor = Address::generate(&env);
        register_anchor(&env, &client, &anchor);

        client.submit_quote_with_reason(
            &anchor,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10000u64, &25u32, &100u64, &100000u64, &1_003_600u64,
            &Some(String::from_str(&env, "referral")),
        );

        let reason = client.get_quote_routing_reason(&anchor, &1u64);
        assert_eq!(reason, Some(String::from_str(&env, "referral")));
    }

    /// `submit_quote_with_reason` with `None` reason stores no reason.
    #[test]
    fn test_submit_quote_with_reason_none_stores_none() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);
        let anchor = Address::generate(&env);
        register_anchor(&env, &client, &anchor);

        client.submit_quote_with_reason(
            &anchor,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10000u64, &25u32, &100u64, &100000u64, &1_003_600u64,
            &None,
        );

        assert_eq!(client.get_quote_routing_reason(&anchor, &1u64), None);
    }

    /// Multiple anchors can store different reasons independently.
    #[test]
    fn test_multiple_anchors_store_independent_reasons() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        let anchor1 = Address::generate(&env);
        let anchor2 = Address::generate(&env);
        register_anchor(&env, &client, &anchor1);
        register_anchor(&env, &client, &anchor2);

        client.submit_quote_with_reason(
            &anchor1,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10000u64, &25u32, &100u64, &100000u64, &1_003_600u64,
            &Some(String::from_str(&env, "lowest_fee")),
        );
        client.submit_quote_with_reason(
            &anchor2,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10000u64, &30u32, &100u64, &100000u64, &1_003_600u64,
            &Some(String::from_str(&env, "preferred_anchor")),
        );

        assert_eq!(
            client.get_quote_routing_reason(&anchor1, &1u64),
            Some(String::from_str(&env, "lowest_fee"))
        );
        assert_eq!(
            client.get_quote_routing_reason(&anchor2, &2u64),
            Some(String::from_str(&env, "preferred_anchor"))
        );
    }

    // ── Transaction routing reason tests ──────────────────────────────────────

    /// `create_transaction_record` (no-reason variant) stores `routing_reason: None`.
    #[test]
    fn test_create_transaction_record_no_reason_is_none() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);
        let initiator = Address::generate(&env);

        let record = client.create_transaction_record(&1u64, &initiator);
        assert_eq!(record.routing_reason, None);
    }

    /// `create_txn_record_with_reason` stores the reason in the record.
    #[test]
    fn test_create_txn_record_with_reason_stores_reason() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);
        let initiator = Address::generate(&env);

        let record = client.create_txn_record_with_reason(
            &1u64,
            &initiator,
            &Some(String::from_str(&env, "referral")),
        );

        assert_eq!(
            record.routing_reason,
            Some(String::from_str(&env, "referral"))
        );
    }

    /// Routing reason persists through Pending → InProgress → Completed transitions.
    #[test]
    fn test_transaction_routing_reason_preserved_through_state_transitions() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);
        let initiator = Address::generate(&env);

        client.create_txn_record_with_reason(
            &1u64,
            &initiator,
            &Some(String::from_str(&env, "lowest_fee")),
        );

        let in_progress = client.start_transaction_record(&1u64);
        assert_eq!(
            in_progress.routing_reason,
            Some(String::from_str(&env, "lowest_fee")),
            "reason must survive Pending→InProgress"
        );

        let completed = client.complete_transaction_record(&1u64);
        assert_eq!(
            completed.routing_reason,
            Some(String::from_str(&env, "lowest_fee")),
            "reason must survive InProgress→Completed"
        );
    }

    /// Routing reason persists when a transaction fails.
    #[test]
    fn test_transaction_routing_reason_preserved_on_failure() {
        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);
        let initiator = Address::generate(&env);

        client.create_txn_record_with_reason(
            &1u64,
            &initiator,
            &Some(String::from_str(&env, "referral")),
        );
        client.start_transaction_record(&1u64);

        let failed = client.fail_transaction_record(
            &1u64,
            &String::from_str(&env, "network timeout"),
        );
        assert_eq!(
            failed.routing_reason,
            Some(String::from_str(&env, "referral")),
            "reason must survive InProgress→Failed"
        );
    }

    // ── #1269: Fee-based selection reason assertion ───────────────────────────

    /// When two anchors submit quotes with distinct fees and the "lowest_fee"
    /// routing reason, the route_transaction result must select the lower-fee
    /// anchor AND carry the "lowest_fee" routing reason.  This test verifies
    /// that the chosen route and its explanatory reason are consistent.
    ///
    /// Fee is the sole decisive criterion (both anchors have identical metadata
    /// in every other dimension), so any other outcome would indicate a bug in
    /// the fee-ranking logic.
    #[test]
    fn test_fee_based_selection_reason_matches_selected_anchor() {
        use soroban_sdk::{Symbol, Vec};
        use anchorkit::contract::{RoutingOptions, RoutingRequest};

        let env = make_env();
        set_ledger(&env, 1_000_000);
        let (client, _) = setup(&env);

        let low_fee_anchor = Address::generate(&env);
        let high_fee_anchor = Address::generate(&env);
        register_anchor(&env, &client, &low_fee_anchor);
        register_anchor(&env, &client, &high_fee_anchor);

        // Identical metadata — fee is the only differentiator.
        client.set_anchor_metadata(&low_fee_anchor, &8000u32, &300u64, &8000u32, &9900u32, &1_000_000u64);
        client.set_anchor_metadata(&high_fee_anchor, &8000u32, &300u64, &8000u32, &9900u32, &1_000_000u64);

        // Low-fee anchor quotes 10 bps; reason tagged as "lowest_fee".
        client.submit_quote_with_reason(
            &low_fee_anchor,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10_000u64, &10u32, &100u64, &100_000u64, &1_003_600u64,
            &Some(String::from_str(&env, "lowest_fee")),
        );

        // High-fee anchor quotes 80 bps; also tagged with "lowest_fee" so that
        // the only way to assert correctness is to check the selected anchor.
        client.submit_quote_with_reason(
            &high_fee_anchor,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10_000u64, &80u32, &100u64, &100_000u64, &1_003_600u64,
            &Some(String::from_str(&env, "lowest_fee")),
        );

        let mut strategy = Vec::new(&env);
        strategy.push_back(Symbol::new(&env, "LowestFee"));
        let options = RoutingOptions {
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
            subject: Address::generate(&env),
            fee_weight: 333,
            speed_weight: 333,
            reputation_weight: 334,
        };

        let best = client.route_transaction(&options);

        // The selected anchor must be the one with the lowest fee.
        assert_eq!(
            best.anchor, low_fee_anchor,
            "LowestFee strategy must select the anchor with the smaller fee_percentage"
        );

        // The winning quote's routing reason must match the fee-selection label.
        assert_eq!(
            best.routing_reason,
            Some(String::from_str(&env, "lowest_fee")),
            "routing reason on the selected quote must agree with the fee-ranking outcome"
        );
    }
}

// ── Tracker-level unit tests ─────────────────────────────────────────────────

#[cfg(test)]
mod routing_reason_tracker_tests {
    use anchorkit::transaction_state_tracker::TransactionStateTracker;
    use soroban_sdk::{testutils::Address as _, Env, String};

    /// `create_transaction` defaults routing_reason to None.
    #[test]
    fn test_tracker_create_transaction_reason_defaults_none() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        let record = tracker.create_transaction(1, initiator, &env).unwrap();
        assert_eq!(record.routing_reason, None);
    }

    /// `create_transaction_with_reason` stores the given reason.
    #[test]
    fn test_tracker_create_transaction_with_reason_stores_reason() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let reason = Some(String::from_str(&env, "lowest_fee"));

        let record = tracker
            .create_transaction_with_reason(1, initiator, reason.clone(), &env)
            .unwrap();
        assert_eq!(record.routing_reason, reason);
    }

    /// `create_transaction_with_reason` with `None` stores no reason.
    #[test]
    fn test_tracker_create_transaction_with_reason_none_stores_none() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        let record = tracker
            .create_transaction_with_reason(1, initiator, None, &env)
            .unwrap();
        assert_eq!(record.routing_reason, None);
    }

    /// The routing reason is preserved after a state transition in dev-mode tracker.
    #[test]
    fn test_tracker_routing_reason_preserved_through_transitions() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let reason = Some(String::from_str(&env, "referral"));

        tracker
            .create_transaction_with_reason(1, initiator, reason.clone(), &env)
            .unwrap();

        let started = tracker.start_transaction(1, &env).unwrap();
        assert_eq!(started.routing_reason, reason, "reason must survive start");

        let completed = tracker.complete_transaction(1, &env).unwrap();
        assert_eq!(completed.routing_reason, reason, "reason must survive complete");
    }

    /// The routing reason is preserved when a transaction fails.
    #[test]
    fn test_tracker_routing_reason_preserved_on_failure() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let reason = Some(String::from_str(&env, "preferred_anchor"));

        tracker
            .create_transaction_with_reason(1, initiator, reason.clone(), &env)
            .unwrap();
        tracker.start_transaction(1, &env).unwrap();

        let error_msg = String::from_str(&env, "payment declined");
        let failed = tracker.fail_transaction(1, error_msg, &env).unwrap();
        assert_eq!(failed.routing_reason, reason, "reason must survive failure");
    }
}
