#![cfg(test)]

mod sep10_test_util;

mod replay_metrics_tests {
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Bytes, Env,
    };
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    use anchorkit::contract::{AnchorKitContract, AnchorKitContractClient};
    use crate::sep10_test_util::{register_attestor_with_sep10, sign_payload};

    fn make_env() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });
        env
    }

    fn payload(env: &Env, byte: u8) -> Bytes {
        let mut b = Bytes::new(env);
        for _ in 0..32 {
            b.push_back(byte);
        }
        b
    }

    /// Set up a fresh contract with one registered attestor.
    fn setup(env: &Env) -> (AnchorKitContractClient, Address, Address, SigningKey) {
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(env, &contract_id);
        let admin = Address::generate(env);
        let issuer = Address::generate(env);
        client.initialize(&admin);
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(env, &client, &issuer, &issuer, &sk);
        (client, admin, issuer, sk)
    }

    // -----------------------------------------------------------------------
    // Accepted-event counter increments after a successful attestation
    // -----------------------------------------------------------------------

    #[test]
    fn test_accepted_events_counter_increments_on_submit() {
        let env = make_env();
        let (client, _, issuer, sk) = setup(&env);

        let hash = payload(&env, 0x01);
        let sig = sign_payload(&env, &sk, &hash);
        let subject = Address::generate(&env);

        // Before any submission the counter must be zero.
        let before = client.get_replay_metrics();
        assert_eq!(before.accepted_events, 0);

        client.submit_attestation(&issuer, &subject, &1_000_001u64, &hash, &sig);

        let after = client.get_replay_metrics();
        assert_eq!(after.accepted_events, 1);
        assert_eq!(after.total_replay_attempts, 0);
        assert_eq!(after.skipped_events, 0);
    }

    #[test]
    fn test_accepted_events_accumulate_across_multiple_submissions() {
        let env = make_env();
        let (client, _, issuer, sk) = setup(&env);
        let subject = Address::generate(&env);

        for i in 0u8..3 {
            let hash = payload(&env, i + 1);
            let sig = sign_payload(&env, &sk, &hash);
            client.submit_attestation(&issuer, &subject, &(1_000_001u64 + i as u64), &hash, &sig);
        }

        let metrics = client.get_replay_metrics();
        assert_eq!(metrics.accepted_events, 3);
        assert_eq!(metrics.total_replay_attempts, 0);
    }

    // -----------------------------------------------------------------------
    // Replay-attempt counter increments on duplicate, accepted stays unchanged
    // -----------------------------------------------------------------------

    #[test]
    fn test_replay_attempt_does_not_increment_accepted() {
        let env = make_env();
        let (client, _, issuer, sk) = setup(&env);
        let subject = Address::generate(&env);

        let hash = payload(&env, 0xAA);
        let sig = sign_payload(&env, &sk, &hash);

        // First submission — should succeed and bump accepted.
        client.submit_attestation(&issuer, &subject, &1_000_001u64, &hash, &sig);

        let mid = client.get_replay_metrics();
        assert_eq!(mid.accepted_events, 1);
        assert_eq!(mid.total_replay_attempts, 0);

        // Second submission with same hash — should be rejected as a replay.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.submit_attestation(&issuer, &subject, &1_000_002u64, &hash, &sig);
        }));
        assert!(result.is_err(), "duplicate submission must be rejected");

        let after = client.get_replay_metrics();
        // accepted must NOT have changed.
        assert_eq!(after.accepted_events, 1, "accepted_events must not change on replay");
        // replay counter must have incremented.
        assert_eq!(after.total_replay_attempts, 1);
        assert_eq!(after.unique_replayed_ids, 1);
    }

    // -----------------------------------------------------------------------
    // Counters are independent across different issuers
    // -----------------------------------------------------------------------

    #[test]
    fn test_accepted_events_count_multiple_issuers() {
        let env = make_env();
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);
        let admin = Address::generate(&env);
        client.initialize(&admin);

        let issuer_a = Address::generate(&env);
        let issuer_b = Address::generate(&env);
        let subject = Address::generate(&env);

        let sk_a = SigningKey::generate(&mut OsRng);
        let sk_b = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(&env, &client, &issuer_a, &issuer_a, &sk_a);
        register_attestor_with_sep10(&env, &client, &issuer_b, &issuer_b, &sk_b);

        // Same hash, two different issuers — both should be accepted.
        let hash = payload(&env, 0x42);
        let sig_a = sign_payload(&env, &sk_a, &hash);
        let sig_b = sign_payload(&env, &sk_b, &hash);
        client.submit_attestation(&issuer_a, &subject, &1_000_001u64, &hash, &sig_a);
        client.submit_attestation(&issuer_b, &subject, &1_000_002u64, &hash, &sig_b);

        let metrics = client.get_replay_metrics();
        assert_eq!(metrics.accepted_events, 2);
        assert_eq!(metrics.total_replay_attempts, 0);
    }

    // -----------------------------------------------------------------------
    // get_replay_count_for_id reflects per-id replay count
    // -----------------------------------------------------------------------

    #[test]
    fn test_replay_count_per_id_matches_total_attempts() {
        let env = make_env();
        let (client, _, issuer, sk) = setup(&env);
        let subject = Address::generate(&env);

        let hash = payload(&env, 0x77);
        let sig = sign_payload(&env, &sk, &hash);

        client.submit_attestation(&issuer, &subject, &1_000_001u64, &hash, &sig);

        // First replay
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.submit_attestation(&issuer, &subject, &1_000_002u64, &hash, &sig);
        }));
        // Second replay
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.submit_attestation(&issuer, &subject, &1_000_003u64, &hash, &sig);
        }));

        let metrics = client.get_replay_metrics();
        assert_eq!(metrics.total_replay_attempts, 2);
        assert_eq!(metrics.unique_replayed_ids, 1);

        let per_id = client.get_replay_count_for_id(&hash);
        assert_eq!(per_id, 2, "per-id count must match replay attempts for that id");
    }
}
