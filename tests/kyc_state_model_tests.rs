#![cfg(test)]

#[path = "sep10_test_util.rs"]
mod sep10_test_util;

mod kyc_state_model_tests {
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::{Address, Bytes, Env};

    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    use anchorkit::contract::{AnchorKitContract, AnchorKitContractClient, KycStatus};
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

    fn register_attestor(env: &Env, client: &AnchorKitContractClient, attestor: &Address) {
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(env, client, attestor, attestor, &sk);
    }

    fn data_hash(env: &Env) -> Bytes {
        Bytes::from_slice(env, b"kyc_data_hash_padded_to_32bytes_")
    }

    fn reason_hash(env: &Env) -> Bytes {
        Bytes::from_slice(env, b"rejection_reason_hash_32bytes__!")
    }

    // -------------------------------------------------------------------------
    // Valid progression tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_not_submitted_to_pending() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        assert_eq!(client.get_kyc_status(&subject), KycStatus::NotSubmitted);
        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Pending);
        let _ = admin;
    }

    #[test]
    fn test_pending_to_approved() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        client.approve_kyc(&admin, &subject);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Approved);
    }

    #[test]
    fn test_pending_to_rejected() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        client.reject_kyc(&admin, &subject, &reason_hash(&env));
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Rejected);
    }

    #[test]
    fn test_rejected_to_reopened() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        client.reject_kyc(&admin, &subject, &reason_hash(&env));
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Rejected);

        client.reopen_kyc(&admin, &subject);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Reopened);
    }

    #[test]
    fn test_reopened_to_pending_full_cycle() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        // Submit → Reject → Reopen → Re-submit → Approve
        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        client.reject_kyc(&admin, &subject, &reason_hash(&env));
        client.reopen_kyc(&admin, &subject);

        // Advance time so the 24 h cooldown passes relative to submitted_at
        set_ledger(&env, 1_000_000 + 90_001);
        let new_hash = Bytes::from_slice(&env, b"new_kyc_hash_padded_to_32bytes!!");
        client.submit_kyc(&subject, &new_hash, &attestor);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Pending);

        client.approve_kyc(&admin, &subject);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Approved);
    }

    #[test]
    fn test_expired_to_pending() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        client.approve_kyc(&admin, &subject);

        // Advance past 30-day approval expiry
        set_ledger(&env, 1_000_000 + 30 * 24 * 60 * 60 + 1);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Expired);

        let new_hash = Bytes::from_slice(&env, b"new_kyc_hash_padded_to_32bytes!!");
        client.submit_kyc(&subject, &new_hash, &attestor);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Pending);
    }

    // -------------------------------------------------------------------------
    // Invalid transition tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_approved_cannot_be_approved_again() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        client.approve_kyc(&admin, &subject);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.approve_kyc(&admin, &subject);
        }));
        assert!(result.is_err(), "Approved → Approved must be rejected");
    }

    #[test]
    fn test_approved_cannot_be_rejected() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        client.approve_kyc(&admin, &subject);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.reject_kyc(&admin, &subject, &reason_hash(&env));
        }));
        assert!(result.is_err(), "Approved → Rejected must be rejected");
    }

    #[test]
    fn test_pending_cannot_be_reopened() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Pending);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.reopen_kyc(&admin, &subject);
        }));
        assert!(result.is_err(), "Pending → Reopened must be rejected");
    }

    #[test]
    fn test_not_submitted_cannot_be_reopened() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let subject = Address::generate(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.reopen_kyc(&admin, &subject);
        }));
        assert!(result.is_err(), "NotSubmitted → Reopened must be rejected");
    }

    #[test]
    fn test_resubmit_blocked_within_cooldown() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        client.reject_kyc(&admin, &subject, &reason_hash(&env));
        client.reopen_kyc(&admin, &subject);

        // Do NOT advance time — 24 h cooldown still active
        let new_hash = Bytes::from_slice(&env, b"new_kyc_hash_padded_to_32bytes!!");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.submit_kyc(&subject, &new_hash, &attestor);
        }));
        assert!(result.is_err(), "Re-submission within cooldown must be rejected");
    }

    // -------------------------------------------------------------------------
    // Recovery after rejection
    // -------------------------------------------------------------------------

    #[test]
    fn test_recovery_after_rejection_via_reopen() {
        let env = make_env();
        let (admin, client) = setup(&env);
        let attestor = Address::generate(&env);
        let subject = Address::generate(&env);
        register_attestor(&env, &client, &attestor);

        client.submit_kyc(&subject, &data_hash(&env), &attestor);
        client.reject_kyc(&admin, &subject, &reason_hash(&env));
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Rejected);

        client.reopen_kyc(&admin, &subject);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Reopened);

        set_ledger(&env, 1_000_000 + 90_001);
        let new_hash = Bytes::from_slice(&env, b"recovery_hash_padded_to_32bytes!");
        client.submit_kyc(&subject, &new_hash, &attestor);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Pending);

        client.approve_kyc(&admin, &subject);
        assert_eq!(client.get_kyc_status(&subject), KycStatus::Approved);
    }
}
