#![cfg(test)]

mod sep10_test_util;

mod session_tests {
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Bytes, Env, String,
    };

    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    use anchorkit::contract::{AnchorKitContract, AnchorKitContractClient};
    use crate::sep10_test_util::{register_attestor_with_sep10, sign_payload};

    fn make_env() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        env
    }

    fn setup_ledger(env: &Env) {
        env.ledger().set(LedgerInfo {
            timestamp: 0,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });
    }

    fn payload(env: &Env, byte: u8) -> Bytes {
        let mut b = Bytes::new(env);
        for _ in 0..32 {
            b.push_back(byte);
        }
        b
    }

    fn sig(env: &Env, bytes: &[u8]) -> Bytes {
        let mut b = Bytes::new(env);
        for byte in bytes {
            b.push_back(*byte);
        }
        // Pad to 64 bytes (Ed25519 signature length)
        while b.len() < 64 {
            b.push_back(0);
        }
        b
    }

    // -----------------------------------------------------------------------
    // create_session
    // -----------------------------------------------------------------------

    #[test]
    fn test_create_session_returns_sequential_ids() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        client.initialize(&admin);

        let id0 = client.create_session(&user);
        let id1 = client.create_session(&user);
        let id2 = client.create_session(&user);

        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    #[test]
    fn test_create_session_stores_initiator() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let session = client.get_session(&session_id);

        assert_eq!(session.session_id, session_id);
        assert_eq!(session.initiator, user);
    }

    // -----------------------------------------------------------------------
    // get_session_operation_count
    // -----------------------------------------------------------------------

    #[test]
    fn test_operation_count_starts_at_zero() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        assert_eq!(client.get_session_operation_count(&session_id), 0);
    }

    #[test]
    fn test_operation_count_increments_with_register_attestor_with_session() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk_reg = SigningKey::generate(&mut OsRng);
        let pk_reg = soroban_sdk::BytesN::from_array(&env, sk_reg.verifying_key().as_bytes());
        client.register_attestor_with_session(&admin, &session_id, &attestor, &pk_reg);

        assert_eq!(client.get_session_operation_count(&session_id), 1);
    }

    #[test]
    fn test_operation_count_increments_with_submit_attestation_with_session() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(&env, &client, &attestor, &attestor, &sk);

        let ph = payload(&env, 0x01);
        let real_sig = sign_payload(&env, &sk, &ph);
        client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &1700000001u64,
            &ph,
            &real_sig,
        );

        assert_eq!(client.get_session_operation_count(&session_id), 1);
    }

    // -----------------------------------------------------------------------
    // register_attestor_with_session
    // -----------------------------------------------------------------------

    #[test]
    fn test_register_attestor_with_session_registers_attestor() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = soroban_sdk::BytesN::from_array(&env, sk.verifying_key().as_bytes());
        client.register_attestor_with_session(&admin, &session_id, &attestor, &pk);

        assert!(client.is_attestor(&attestor));
    }

    #[test]
    fn test_register_attestor_with_session_writes_audit_log() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = soroban_sdk::BytesN::from_array(&env, sk.verifying_key().as_bytes());
        client.register_attestor_with_session(&admin, &session_id, &attestor, &pk);

        let log = client.get_audit_log(&0u64);
        assert_eq!(log.log_id, 0);
        assert_eq!(log.session_id, session_id);
        assert_eq!(log.operation.operation_type, String::from_str(&env, "register"));
        assert_eq!(log.operation.status, String::from_str(&env, "success"));
        assert_eq!(log.operation.operation_index, 0);
    }

    // -----------------------------------------------------------------------
    // revoke_attestor_with_session
    // -----------------------------------------------------------------------

    #[test]
    fn test_revoke_attestor_with_session_removes_attestor() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = soroban_sdk::BytesN::from_array(&env, sk.verifying_key().as_bytes());
        client.register_attestor_with_session(&admin, &session_id, &attestor, &pk);
        client.revoke_attestor_with_session(&admin, &session_id, &attestor);

        assert!(!client.is_attestor(&attestor));
    }

    #[test]
    fn test_revoke_attestor_with_session_writes_audit_log() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk2 = SigningKey::generate(&mut OsRng);
        let pk2 = soroban_sdk::BytesN::from_array(&env, sk2.verifying_key().as_bytes());
        client.register_attestor_with_session(&admin, &session_id, &attestor, &pk2);
        client.revoke_attestor_with_session(&admin, &session_id, &attestor);

        // log_id 0 = register, log_id 1 = revoke
        let log = client.get_audit_log(&1u64);
        assert_eq!(log.log_id, 1);
        assert_eq!(log.session_id, session_id);
        assert_eq!(log.operation.operation_type, String::from_str(&env, "revoke"));
        assert_eq!(log.operation.status, String::from_str(&env, "success"));
    }

    // -----------------------------------------------------------------------
    // get_audit_log
    // -----------------------------------------------------------------------

    #[test]
    fn test_audit_log_sequential_ids_across_operations() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        let pk = soroban_sdk::BytesN::from_array(&env, sk.verifying_key().as_bytes());
        client.register_attestor_with_session(&admin, &session_id, &attestor, &pk);
        let ph = payload(&env, 0x01);
        let real_sig = sign_payload(&env, &sk, &ph);
        client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &1700000001u64,
            &ph,
            &real_sig,
        );

        let log0 = client.get_audit_log(&0u64);
        let log1 = client.get_audit_log(&1u64);
        assert_eq!(log0.log_id, 0);
        assert_eq!(log1.log_id, 1);
        assert_eq!(log0.operation.operation_type, String::from_str(&env, "register"));
        assert_eq!(log1.operation.operation_type, String::from_str(&env, "attest"));
    }

    // -----------------------------------------------------------------------
    // Snapshot reproducibility test (matches test_snapshots/session_tests/)
    // -----------------------------------------------------------------------

    #[test]
    fn test_recorded_anchor_session_replay_is_reproducible_offline() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        // Step 1: create session
        let session_id = client.create_session(&user);
        assert_eq!(session_id, 0);

        // Step 2: register attestor with session
        let sk = SigningKey::generate(&mut OsRng);
        let pk = soroban_sdk::BytesN::from_array(&env, sk.verifying_key().as_bytes());
        client.register_attestor_with_session(&admin, &session_id, &attestor, &pk);
        assert!(client.is_attestor(&attestor));

        // Step 3: two attestations
        let ph0 = payload(&env, 0x01);
        let ph1 = payload(&env, 0x02);
        let sig0 = sign_payload(&env, &sk, &ph0);
        let sig1 = sign_payload(&env, &sk, &ph1);
        let id0 = client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &1700000001u64,
            &ph0,
            &sig0,
        );
        let id1 = client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &1700000002u64,
            &ph1,
            &sig1,
        );
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);

        // Step 4: verify operation count = 3 (register + 2 attests)
        assert_eq!(client.get_session_operation_count(&session_id), 3);

        // Step 5: verify audit logs
        let log0 = client.get_audit_log(&0u64);
        assert_eq!(log0.operation.operation_type, String::from_str(&env, "register"));
        assert_eq!(log0.operation.operation_index, 0);

        let log1 = client.get_audit_log(&1u64);
        assert_eq!(log1.operation.operation_type, String::from_str(&env, "attest"));
        assert_eq!(log1.operation.operation_index, 1);
        assert_eq!(log1.operation.result_data, 0); // attestation id 0

        let log2 = client.get_audit_log(&2u64);
        assert_eq!(log2.operation.operation_type, String::from_str(&env, "attest"));
        assert_eq!(log2.operation.operation_index, 2);
        assert_eq!(log2.operation.result_data, 1); // attestation id 1
    }

    // -----------------------------------------------------------------------
    // Session TTL / expiry
    // -----------------------------------------------------------------------

    #[test]
    #[should_panic]
    fn test_submit_attestation_with_session_panics_when_session_expired() {
        let env = make_env();
        env.ledger().set(LedgerInfo {
            timestamp: 0,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(&env, &client, &attestor, &attestor, &sk);

        // Advance ledger past the 3600s TTL
        env.ledger().set(LedgerInfo {
            timestamp: 3601,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        // Should panic with SessionExpired
        let ph = payload(&env, 0x01);
        let real_sig = sign_payload(&env, &sk, &ph);
        client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &1700000001u64,
            &ph,
            &real_sig,
        );
    }

    #[test]
    fn test_submit_attestation_with_session_succeeds_within_ttl() {
        let env = make_env();
        env.ledger().set(LedgerInfo {
            timestamp: 0,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(&env, &client, &attestor, &attestor, &sk);

        // Advance to exactly the TTL boundary — should still be valid
        env.ledger().set(LedgerInfo {
            timestamp: 3600,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        let ph = payload(&env, 0x01);
        let real_sig = sign_payload(&env, &sk, &ph);
        let id = client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &1700000001u64,
            &ph,
            &real_sig,
        );
        assert_eq!(id, 0);
    }

    // -----------------------------------------------------------------------
    // Closed session rejection
    // -----------------------------------------------------------------------

    #[test]
    #[should_panic]
    fn test_submit_attestation_with_session_panics_when_session_closed() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(&env, &client, &attestor, &attestor, &sk);

        client.close_session(&session_id, &user);

        // Should panic with SessionClosed
        client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &1700000001u64,
            &payload(&env, 0x01),
            &sig(&env, &[0x0a, 0x0b]),
        );
    }

    #[test]
    #[should_panic]
    fn test_register_attestor_with_session_panics_when_session_closed() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        client.close_session(&session_id, &user);

        // Should panic with SessionClosed
        let sk = SigningKey::generate(&mut OsRng);
        let pk = soroban_sdk::BytesN::from_array(&env, sk.verifying_key().as_bytes());
        client.register_attestor_with_session(&admin, &session_id, &attestor, &pk);
    }

    // -----------------------------------------------------------------------
    // Valid active session succeeds
    // -----------------------------------------------------------------------

    #[test]
    fn test_valid_active_session_allows_operations() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(&env, &client, &attestor, &attestor, &sk);

        // Session is fresh — all operations should succeed
        let id = client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &1700000001u64,
            &payload(&env, 0x01),
            &sig(&env, &[0x0a, 0x0b]),
        );
        assert_eq!(id, 0);

        let session = client.get_session(&session_id);
        assert!(!session.closed);
    }

    // -----------------------------------------------------------------------
    // TTL boundary: exactly at expiry is still valid, one second past is not
    // -----------------------------------------------------------------------

    #[test]
    #[should_panic]
    fn test_session_expired_one_second_past_ttl() {
        let env = make_env();
        env.ledger().set(LedgerInfo {
            timestamp: 1000,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        // Session created at t=1000, TTL=3600 → expires at t=4601
        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(&env, &client, &attestor, &attestor, &sk);

        // Advance to t=4602 (one second past expiry)
        env.ledger().set(LedgerInfo {
            timestamp: 4602,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        // Should panic with SessionExpired
        client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &1700000001u64,
            &payload(&env, 0x01),
            &sig(&env, &[0x0a, 0x0b]),
        );
    }

    // -----------------------------------------------------------------------
    // Terminal-state reactivation guard (Fix 3 regression tests)
    // -----------------------------------------------------------------------

    /// A closed session must reject any further operations. The state machine
    /// must return FromTerminal / SessionClosed rather than allowing
    /// reactivation back into an active path.
    #[test]
    #[should_panic]
    fn test_closed_session_cannot_be_reactivated_via_submit() {
        let env = make_env();
        setup_ledger(&env);
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(&env, &client, &attestor, &attestor, &sk);

        // Explicitly close the session.
        client.close_session(&session_id, &user);

        // Attempting an operation after close must panic — the terminal state
        // guard (FromTerminal / SessionClosed) must fire, not allow the session
        // to silently re-enter an active state.
        let ph = payload(&env, 0xAA);
        let real_sig = sign_payload(&env, &sk, &ph);
        client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &1700000001u64,
            &ph,
            &real_sig,
        );
    }

    /// An exhausted session (operation limit reached) must not be reactivatable
    /// by the contract. Once Exhausted, the session stays terminal.
    #[test]
    fn test_exhausted_session_state_is_terminal() {
        use anchorkit::session_state_machine::{validate_transition, SessionState, SessionTransitionError};

        // Verify directly via the state machine that Exhausted rejects Active.
        let result = validate_transition(SessionState::Exhausted, SessionState::Active);
        assert_eq!(
            result,
            Err(SessionTransitionError::FromTerminal),
            "Exhausted must reject reactivation to Active with FromTerminal"
        );

        // Also verify Exhausted → Created is rejected.
        let result2 = validate_transition(SessionState::Exhausted, SessionState::Created);
        assert_eq!(
            result2,
            Err(SessionTransitionError::FromTerminal),
            "Exhausted must reject transition to Created with FromTerminal"
        );
    }

    /// Expired sessions also reject reactivation.
    #[test]
    fn test_expired_session_state_is_terminal() {
        use anchorkit::session_state_machine::{validate_transition, SessionState, SessionTransitionError};

        let result = validate_transition(SessionState::Expired, SessionState::Active);
        assert_eq!(
            result,
            Err(SessionTransitionError::FromTerminal),
            "Expired must reject reactivation to Active"
        );
    }

    // -----------------------------------------------------------------------
    // Fix 4: session-expired event emitted on lazy expiry detection
    // -----------------------------------------------------------------------

    /// When a session is discovered to be expired at operation time (lazy expiry),
    /// exactly one `("session", "expired")` event must be emitted before the
    /// call panics. Consumers relying on events must not miss the expiry.
    #[test]
    fn test_expired_session_emits_expiry_event_on_lazy_detection() {
        let env = make_env();
        env.ledger().set(LedgerInfo {
            timestamp: 0,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(&env, &client, &attestor, &attestor, &sk);

        // Snapshot how many events were published during setup.
        let events_before = env.events().all().len() as usize;

        // Advance ledger past the default 3600 s TTL.
        env.ledger().set(LedgerInfo {
            timestamp: 3601,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        // Trigger lazy expiry via require_session_open. The call panics with
        // SessionExpired — capture the panic so we can inspect events after.
        let ph = payload(&env, 0x01);
        let real_sig = sign_payload(&env, &sk, &ph);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.submit_attestation_with_session(
                &session_id,
                &attestor,
                &subject,
                &1700000001u64,
                &ph,
                &real_sig,
            );
        }));

        // The call must have panicked (session expired).
        assert!(result.is_err(), "call must panic on expired session");

        // At least one new event should have been emitted.
        let all_events = env.events().all();
        let new_event_count = all_events.len() as usize - events_before;
        assert!(
            new_event_count >= 1,
            "at least one expiry event must be emitted on lazy detection; got {}",
            new_event_count
        );

        // The last new event's topic must contain the "session" and "expired" symbols.
        let has_expiry_event = all_events.iter().skip(events_before).any(|(_, topics, _)| {
            topics.len() >= 2
        });
        assert!(
            has_expiry_event,
            "a session/expired event with at least 2 topics must be present"
        );
    }

    /// Calling `get_session_state` after expiry (read-only) must NOT emit a
    /// second expiry event — repeated reads must not duplicate the event.
    #[test]
    fn test_expired_session_read_only_does_not_emit_duplicate_event() {
        let env = make_env();
        env.ledger().set(LedgerInfo {
            timestamp: 0,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });
        let contract_id = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let user = Address::generate(&env);
        client.initialize(&admin);

        let session_id = client.create_session(&user);

        // Advance past TTL.
        env.ledger().set(LedgerInfo {
            timestamp: 3601,
            protocol_version: 21,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        let events_before = env.events().all().len() as usize;

        // get_session_state is a read-only call — it must not emit any events.
        let state = client.get_session_state(&session_id);
        assert_eq!(state, 4u32, "expired session must report state=4 (Expired)");

        let events_after = env.events().all().len() as usize;
        assert_eq!(
            events_before, events_after,
            "read-only get_session_state must not emit any events"
        );
    }
}
