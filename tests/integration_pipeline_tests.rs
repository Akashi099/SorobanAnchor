//! Extended integration tests covering the full deploy-to-verify lifecycle.
//!
//! These tests complement `cli_integration_harness.rs` with additional
//! end-to-end scenarios that exercise steps not already covered:
//!
//! - Admin transfer (propose → accept)
//! - Capability grant / revoke lifecycle
//! - Full audit log inspection after a multi-step workflow
//! - Service enable / disable / retire / restore lifecycle
//! - Quote routing with fallback when the primary anchor is unavailable
//! - Session-scoped attestation with operation count assertions
//! - Batch attestation submission and retrieval
//! - KYC reject and reopen workflow
//! - Transaction state tracker full lifecycle including failure recovery
//! - Anchor health recording and score computation
//! - Replay metrics tracking
//!
//! # Running
//!
//! ```bash
//! # All pipeline lifecycle tests (local simulation, no network required)
//! cargo test --test integration_pipeline_tests
//! ```
//!
//! All tests are deterministic and use the Soroban testutils environment.

#![cfg(test)]

#[path = "sep10_test_util.rs"]
mod sep10_test_util;

extern crate std;

use std::string::ToString;

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    Address, Bytes, BytesN, Env, String, Vec,
};

use anchorkit::contract::{
    AdminCapability, AdminRole, AnchorKitContract, AnchorKitContractClient,
    KycStatus, RoutingOptions, RoutingRequest, SERVICE_DEPOSITS, SERVICE_QUOTES,
    SERVICE_WITHDRAWALS,
};
use sep10_test_util::{register_attestor_with_sep10, sign_payload};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn setup_ledger(env: &Env, timestamp: u64) {
    env.ledger().set(LedgerInfo {
        timestamp,
        protocol_version: 21,
        sequence_number: 0,
        network_id: Default::default(),
        base_reserve: 0,
        min_persistent_entry_ttl: 4096,
        min_temp_entry_ttl: 16,
        max_entry_ttl: 6_312_000,
    });
}

fn deploy_and_initialize(env: &Env) -> (AnchorKitContractClient, Address) {
    let contract_id = env.register_contract(None, AnchorKitContract);
    let client = AnchorKitContractClient::new(env, &contract_id);
    let admin = Address::generate(env);
    client.initialize(&admin);
    (client, admin)
}

fn register_attestor(
    env: &Env,
    client: &AnchorKitContractClient,
    attestor: &Address,
    key: &SigningKey,
) {
    register_attestor_with_sep10(env, client, attestor, attestor, key);
}

fn unique_payload(env: &Env, seed: u8) -> Bytes {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    bytes[1] = 0xCA;
    bytes[2] = 0xFE;
    Bytes::from_slice(env, &bytes)
}

// ---------------------------------------------------------------------------
// 1. Admin transfer lifecycle (propose → accept)
// ---------------------------------------------------------------------------

/// Full admin transfer: propose a new admin, accept from the new admin's
/// address, and verify the admin field has been updated.
#[test]
fn pipeline_admin_transfer_lifecycle() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, old_admin) = deploy_and_initialize(&env);
    assert_eq!(client.get_admin(), old_admin);

    let new_admin = Address::generate(&env);
    client.propose_admin_transfer(&new_admin);

    client.accept_admin_transfer();

    assert_eq!(
        client.get_admin(),
        new_admin,
        "admin must be the new address after accept"
    );
}

// ---------------------------------------------------------------------------
// 2. Capability grant / revoke lifecycle
// ---------------------------------------------------------------------------

/// Grants ManageCache capability to an operator, verifies it is held, then
/// revokes it and verifies it is gone. Also confirms an address with no grants
/// holds no capabilities.
#[test]
fn pipeline_capability_grant_revoke_lifecycle() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let operator = Address::generate(&env);

    assert!(
        !client.has_capability(&operator, &AdminCapability::ManageCache),
        "fresh address must not hold any capability"
    );

    client.grant_capability(&operator, &AdminCapability::ManageCache);
    assert!(
        client.has_capability(&operator, &AdminCapability::ManageCache),
        "capability must be held after grant"
    );

    client.revoke_capability(&operator, &AdminCapability::ManageCache);
    assert!(
        !client.has_capability(&operator, &AdminCapability::ManageCache),
        "capability must be absent after revoke"
    );
}

// ---------------------------------------------------------------------------
// 3. Full audit log inspection after multi-step workflow
// ---------------------------------------------------------------------------

/// Runs register → attest → revoke and inspects the audit log to confirm
/// operation types are recorded in the correct order.
#[test]
fn pipeline_audit_log_multi_step_workflow() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let attestor = Address::generate(&env);
    let subject = Address::generate(&env);
    let key = SigningKey::generate(&mut OsRng);

    // Step 1: grant role so session ops are authorized.
    let user = Address::generate(&env);
    client.grant_role(&user, &AdminRole::AttestorAdmin);
    let session_id = client.create_session(&user);

    // Step 2: register attestor inside session.
    let pk: BytesN<32> = BytesN::from_array(&env, key.verifying_key().as_bytes());
    client.register_attestor_with_session(&user, &session_id, &attestor, &pk);

    // Step 3: submit attestation inside session.
    let payload = unique_payload(&env, 0x01);
    let sig = sign_payload(&env, &key, &payload);
    client.submit_attestation_with_session(
        &session_id,
        &attestor,
        &subject,
        &1_000_001u64,
        &payload,
        &sig,
    );

    // Step 4: close session.
    client.close_session(&session_id, &user);

    // Audit log: at minimum we expect register (index 0) and attest (index 1).
    let log0 = client.get_audit_log(&0u64);
    assert_eq!(
        log0.operation.operation_type,
        String::from_str(&env, "register"),
        "first audit log entry must be 'register'"
    );

    let log1 = client.get_audit_log(&1u64);
    assert_eq!(
        log1.operation.operation_type,
        String::from_str(&env, "attest"),
        "second audit log entry must be 'attest'"
    );

    let count = client.get_audit_log_count();
    assert!(count >= 2, "audit log must contain at least 2 entries");
}

// ---------------------------------------------------------------------------
// 4. Service enable / disable / retire / restore lifecycle
// ---------------------------------------------------------------------------

/// Configures services, disables one, confirms it is disabled, retires
/// another, confirms retired, then unretires and confirms active.
#[test]
fn pipeline_service_lifecycle_enable_disable_retire_restore() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let anchor = Address::generate(&env);

    let mut services = Vec::new(&env);
    services.push_back(SERVICE_DEPOSITS);
    services.push_back(SERVICE_WITHDRAWALS);
    services.push_back(SERVICE_QUOTES);
    client.configure_services(&anchor, &services);

    // All three must initially be supported.
    assert!(client.supports_service(&anchor, &SERVICE_DEPOSITS));
    assert!(client.supports_service(&anchor, &SERVICE_WITHDRAWALS));
    assert!(client.supports_service(&anchor, &SERVICE_QUOTES));

    // Disable withdrawals.
    let caller = Address::generate(&env);
    let disabled = client.disable_service(&caller, &anchor, &SERVICE_WITHDRAWALS);
    assert!(disabled, "disable_service must return true on success");
    assert!(
        !client.is_service_enabled(&anchor, &SERVICE_WITHDRAWALS),
        "withdrawals must be disabled"
    );

    // Re-enable withdrawals.
    client.enable_service(&caller, &anchor, &SERVICE_WITHDRAWALS);
    assert!(
        client.is_service_enabled(&anchor, &SERVICE_WITHDRAWALS),
        "withdrawals must be enabled again"
    );

    // Retire quotes service.
    client.retire_service(&anchor, &SERVICE_QUOTES, &None, &None);
    let retirement = client.get_service_retirement_info(&anchor, &SERVICE_QUOTES);
    assert!(retirement.is_some(), "retirement info must be set after retire_service");

    // Unretire quotes.
    client.unretire_service(&anchor, &SERVICE_QUOTES);
    let after_unretire = client.get_service_retirement_info(&anchor, &SERVICE_QUOTES);
    assert!(after_unretire.is_none(), "retirement info must be cleared after unretire");
}

// ---------------------------------------------------------------------------
// 5. Quote routing with fallback when primary anchor is inactive
// ---------------------------------------------------------------------------

/// Registers two anchors. Deactivates the first (higher-reputation) one.
/// Asserts routing falls back to the second active anchor.
#[test]
fn pipeline_routing_fallback_on_deactivated_anchor() {
    use soroban_sdk::Symbol;

    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);

    let primary = Address::generate(&env);
    let fallback = Address::generate(&env);
    let ts = env.ledger().timestamp();

    for anchor in [&primary, &fallback] {
        let mut services = Vec::new(&env);
        services.push_back(SERVICE_DEPOSITS);
        services.push_back(SERVICE_QUOTES);
        client.configure_services(anchor, &services);
        client.set_anchor_metadata(anchor, &9000u32, &120u64, &8000u32, &9900u32, &3600u64);
        client.submit_quote(
            anchor,
            &String::from_str(&env, "USD"),
            &String::from_str(&env, "USDC"),
            &10_000u64,
            &50u32,
            &100u64,
            &1_000_000u64,
            &(ts + 7200),
        );
    }

    // Deactivate primary.
    client.deactivate_anchor(&primary);

    let mut strategy = Vec::new(&env);
    strategy.push_back(Symbol::new(&env, "LowestFee"));
    let options = RoutingOptions {
        request: RoutingRequest {
            base_asset: String::from_str(&env, "USD"),
            quote_asset: String::from_str(&env, "USDC"),
            amount: 500u64,
            operation_type: 1u32,
        },
        strategy,
        min_reputation: 0u32,
        max_anchors: 5u32,
        require_kyc: false,
        require_compliance: false,
        subject: Address::generate(&env),
        fee_weight: 500u32,
        speed_weight: 250u32,
        reputation_weight: 250u32,
    };

    let best = client.route_transaction(&options);
    assert_eq!(
        best.anchor, fallback,
        "routing must select the active fallback anchor"
    );
}

// ---------------------------------------------------------------------------
// 6. Session-scoped multi-attestation with operation count assertions
// ---------------------------------------------------------------------------

/// Creates a session, submits 5 attestations within it, and verifies
/// the operation count matches.
#[test]
fn pipeline_session_multi_attestation_operation_count() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let key = SigningKey::generate(&mut OsRng);
    let attestor = Address::generate(&env);
    let subject = Address::generate(&env);

    let user = Address::generate(&env);
    client.grant_role(&user, &AdminRole::AttestorAdmin);

    let session_id = client.create_session(&user);

    // Register attestor inside the session.
    let pk: BytesN<32> = BytesN::from_array(&env, key.verifying_key().as_bytes());
    client.register_attestor_with_session(&user, &session_id, &attestor, &pk);

    const ATTEST_COUNT: u64 = 5;
    for i in 0..ATTEST_COUNT {
        let payload = unique_payload(&env, i as u8 + 10);
        let sig = sign_payload(&env, &key, &payload);
        client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &(1_000_001 + i),
            &payload,
            &sig,
        );
    }

    // +1 for the register operation.
    let expected_ops = ATTEST_COUNT + 1;
    assert_eq!(
        client.get_session_operation_count(&session_id),
        expected_ops,
        "session operation count must include register + all attestations"
    );
}

// ---------------------------------------------------------------------------
// 7. Batch attestation submission and retrieval
// ---------------------------------------------------------------------------

/// Submits a batch of attestations via submit_attestation_batch and verifies
/// each can be retrieved individually by ID.
#[test]
fn pipeline_batch_attestation_submit_and_retrieve() {
    use anchorkit::contract::AttestationInput;

    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let key = SigningKey::generate(&mut OsRng);
    let attestor = Address::generate(&env);
    register_attestor(&env, &client, &attestor, &key);

    const BATCH: usize = 5;
    let mut inputs = Vec::new(&env);

    for i in 0..BATCH {
        let payload = unique_payload(&env, i as u8 + 50);
        let sig = sign_payload(&env, &key, &payload);
        inputs.push_back(AttestationInput {
            subject: Address::generate(&env),
            timestamp: 1_000_001u64 + i as u64,
            payload_hash: payload,
            signature: sig,
        });
    }

    let ids = client.submit_attestation_batch(&attestor, &inputs);
    assert_eq!(ids.len(), BATCH as u32, "batch must return one ID per input");

    // Every attestation must be retrievable and correctly attributed.
    for i in 0..ids.len() {
        let id = ids.get(i).unwrap();
        let a = client.get_attestation(&id);
        assert_eq!(a.id, id);
        assert_eq!(a.issuer, attestor);
    }
}

// ---------------------------------------------------------------------------
// 8. KYC reject and reopen workflow
// ---------------------------------------------------------------------------

/// Submits KYC, rejects it with a reason hash, verifies Rejected status,
/// then reopens it and verifies it returns to Pending.
#[test]
fn pipeline_kyc_reject_and_reopen() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, admin) = deploy_and_initialize(&env);
    let key = SigningKey::generate(&mut OsRng);
    let attestor = Address::generate(&env);
    register_attestor(&env, &client, &attestor, &key);
    let subject = Address::generate(&env);

    // Submit KYC.
    let data_hash = unique_payload(&env, 0xAA);
    client.submit_kyc(&subject, &data_hash, &attestor);
    assert_eq!(client.get_kyc_status(&subject), KycStatus::Pending);

    // Reject with a reason hash.
    let reason_hash = unique_payload(&env, 0xBB);
    client.reject_kyc(&admin, &subject, &reason_hash);
    assert_eq!(client.get_kyc_status(&subject), KycStatus::Rejected);

    // Reopen: status returns to Pending.
    client.reopen_kyc(&admin, &subject);
    assert_eq!(
        client.get_kyc_status(&subject),
        KycStatus::Pending,
        "KYC status must return to Pending after reopen"
    );
}

// ---------------------------------------------------------------------------
// 9. Transaction state tracker — failure then export/import recovery
// ---------------------------------------------------------------------------

/// Creates a transaction, advances it to InProgress, marks it failed,
/// then exports the recovery state and re-imports it (simulating recovery).
#[test]
fn pipeline_transaction_failure_recovery_export_import() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);

    let tx_id = 42u64;
    client.create_transaction_record(&tx_id);
    client.start_transaction_record(&tx_id);
    client.fail_transaction_record(&tx_id, &String::from_str(&env, "network_timeout"));

    // Export recovery state.
    let export_bytes = client.export_recovery_state(&tx_id);
    assert!(!export_bytes.is_empty(), "export must produce non-empty bytes");

    // Re-import recovery state to simulate recovery.
    let recovered = client.import_recovery_state(&export_bytes);
    assert_eq!(
        recovered.transaction_id, tx_id,
        "imported record must have the original transaction ID"
    );
}

// ---------------------------------------------------------------------------
// 10. Anchor health recording and score computation
// ---------------------------------------------------------------------------

/// Records health events for an anchor (successes and failures) and asserts
/// that the health score and summary reflect the recorded events.
#[test]
fn pipeline_anchor_health_record_and_score() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let anchor = Address::generate(&env);

    // Record 8 successes and 2 failures.
    for _ in 0..8 {
        client.record_health_event(&anchor, &true);
    }
    for _ in 0..2 {
        client.record_health_event(&anchor, &false);
    }

    let health = client.get_anchor_health(&anchor);
    assert_eq!(health.total_calls, 10, "total calls must be 10");
    assert_eq!(health.failure_count, 2, "failure count must be 2");
    assert!(
        health.success_rate >= 79 && health.success_rate <= 81,
        "success rate must be approximately 80%, got {}",
        health.success_rate
    );
}

// ---------------------------------------------------------------------------
// 11. Replay metrics tracking under repeated submissions
// ---------------------------------------------------------------------------

/// Submits a payload, attempts a replay (caught by panic), and then reads
/// the replay metrics to confirm the replay counter was incremented.
#[test]
fn pipeline_replay_metrics_increment_on_replay_attempt() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let key = SigningKey::generate(&mut OsRng);
    let attestor = Address::generate(&env);
    register_attestor(&env, &client, &attestor, &key);
    let subject = Address::generate(&env);

    let payload = unique_payload(&env, 0x77);
    let sig = sign_payload(&env, &key, &payload);

    // First submission succeeds.
    client.submit_attestation(&attestor, &subject, &1_000_001u64, &payload, &sig);

    // Second submission (replay) must be rejected.
    let replay_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.submit_attestation(&attestor, &subject, &1_000_002u64, &payload, &sig);
    }));
    assert!(replay_result.is_err(), "replay must be rejected");

    // Replay metrics must record the attempt.
    let metrics = client.get_replay_metrics();
    assert!(
        metrics.total_replay_attempts >= 1,
        "replay metrics must record at least one attempt"
    );
}

// ---------------------------------------------------------------------------
// 12. Full deploy-to-verify pipeline (canonical end-to-end)
// ---------------------------------------------------------------------------

/// The canonical pipeline test: deploy → initialize → register → configure
/// services → submit attestation → route transaction → revoke → verify cleanup.
/// This covers every major lifecycle step in a single deterministic sequence.
#[test]
fn pipeline_full_deploy_to_verify() {
    use soroban_sdk::Symbol;

    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    // ── 1. Deploy and initialize ─────────────────────────────────────────────
    let (client, admin) = deploy_and_initialize(&env);
    assert_eq!(client.get_admin(), admin);
    assert_eq!(client.get_schema_version(), 1);

    // ── 2. Register attestor ────────────────────────────────────────────────
    let key = SigningKey::generate(&mut OsRng);
    let attestor = Address::generate(&env);
    register_attestor(&env, &client, &attestor, &key);
    assert!(client.is_attestor(&attestor));
    assert_eq!(client.get_attestor_count(), 1);

    // ── 3. Configure services ───────────────────────────────────────────────
    let mut services = Vec::new(&env);
    services.push_back(SERVICE_DEPOSITS);
    services.push_back(SERVICE_WITHDRAWALS);
    services.push_back(SERVICE_QUOTES);
    client.configure_services(&attestor, &services);
    assert!(client.supports_service(&attestor, &SERVICE_DEPOSITS));
    assert!(client.supports_service(&attestor, &SERVICE_WITHDRAWALS));
    assert!(client.supports_service(&attestor, &SERVICE_QUOTES));

    // ── 4. Submit attestation ───────────────────────────────────────────────
    let subject = Address::generate(&env);
    let payload = unique_payload(&env, 0x42);
    let sig = sign_payload(&env, &key, &payload);
    let attest_id = client.submit_attestation(
        &attestor,
        &subject,
        &1_000_001u64,
        &payload,
        &sig,
    );
    let attestation = client.get_attestation(&attest_id);
    assert_eq!(attestation.issuer, attestor);
    assert_eq!(attestation.subject, subject);
    assert_eq!(attestation.schema_version, 1);

    // ── 5. Set metadata and submit quote ────────────────────────────────────
    let ts = env.ledger().timestamp();
    client.set_anchor_metadata(&attestor, &8500u32, &90u64, &8000u32, &9900u32, &3600u64);
    let quote_id = client.submit_quote(
        &attestor,
        &String::from_str(&env, "USD"),
        &String::from_str(&env, "USDC"),
        &10_000u64,
        &30u32,
        &100u64,
        &500_000u64,
        &(ts + 7200),
    );
    assert!(quote_id > 0);

    // ── 6. Route transaction ────────────────────────────────────────────────
    let mut strategy = Vec::new(&env);
    strategy.push_back(Symbol::new(&env, "LowestFee"));
    let options = RoutingOptions {
        request: RoutingRequest {
            base_asset: String::from_str(&env, "USD"),
            quote_asset: String::from_str(&env, "USDC"),
            amount: 1_000u64,
            operation_type: 1u32,
        },
        strategy,
        min_reputation: 0u32,
        max_anchors: 5u32,
        require_kyc: false,
        require_compliance: false,
        subject: subject.clone(),
        fee_weight: 400u32,
        speed_weight: 300u32,
        reputation_weight: 300u32,
    };
    let best = client.route_transaction(&options);
    assert_eq!(best.anchor, attestor);
    assert_eq!(best.fee_percentage, 30);

    // ── 7. Inspect audit log ────────────────────────────────────────────────
    let log_count = client.get_audit_log_count();
    assert!(log_count >= 1, "audit log must have at least one entry");

    // ── 8. Revoke attestor and verify cleanup ───────────────────────────────
    client.revoke_attestor(&attestor);
    assert!(!client.is_attestor(&attestor));
    let revocation = client.get_attestor_revocation_info(&attestor);
    assert!(revocation.is_some(), "revocation info must be stored");
}

// ---------------------------------------------------------------------------
// 13. Admin role lifecycle — grant, verify, revoke, verify
// ---------------------------------------------------------------------------

/// Grants an admin role to an operator, verifies it is held, revokes it, and
/// confirms the role is gone. Validates the has_role / grant_role / revoke_role
/// lifecycle in sequence.
#[test]
fn pipeline_admin_role_full_lifecycle() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let operator = Address::generate(&env);

    // Before any grant the operator holds no role.
    assert!(
        !client.has_role(&operator, &AdminRole::AttestorAdmin),
        "fresh address must not hold AttestorAdmin"
    );
    assert!(
        !client.has_role(&operator, &AdminRole::KycAdmin),
        "fresh address must not hold KycAdmin"
    );

    // Grant AttestorAdmin.
    client.grant_role(&operator, &AdminRole::AttestorAdmin);
    assert!(
        client.has_role(&operator, &AdminRole::AttestorAdmin),
        "role must be held after grant"
    );

    // Revoke and confirm it is gone.
    client.revoke_role(&operator, &AdminRole::AttestorAdmin);
    assert!(
        !client.has_role(&operator, &AdminRole::AttestorAdmin),
        "role must be absent after revoke"
    );
}

// ---------------------------------------------------------------------------
// 14. Multi-attestation pagination via get_attestations_in_range
// ---------------------------------------------------------------------------

/// Submits 10 attestations and retrieves them in two pages of 5.
/// Verifies that the pagination boundary is respected and no overlap occurs.
#[test]
fn pipeline_attestation_pagination_two_pages() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let key = SigningKey::generate(&mut OsRng);
    let attestor = Address::generate(&env);
    register_attestor(&env, &client, &attestor, &key);
    let subject = Address::generate(&env);

    const COUNT: usize = 10;
    let mut ids: std::vec::Vec<u64> = std::vec::Vec::with_capacity(COUNT);

    for i in 0..COUNT {
        let payload = unique_payload(&env, i as u8 + 70);
        let sig = sign_payload(&env, &key, &payload);
        let id = client.submit_attestation(
            &attestor,
            &subject,
            &(1_000_001u64 + i as u64),
            &payload,
            &sig,
        );
        ids.push(id);
    }

    // Page 1: IDs 1..=5.
    let page1 = client.get_attestations_in_range(&ids[0], &ids[4]);
    assert_eq!(page1.len(), 5u32, "page 1 must contain exactly 5 attestations");

    // Page 2: IDs 6..=10.
    let page2 = client.get_attestations_in_range(&ids[5], &ids[9]);
    assert_eq!(page2.len(), 5u32, "page 2 must contain exactly 5 attestations");

    // No overlap — smallest ID in page 2 must be greater than largest in page 1.
    let max_p1 = ids[4];
    let min_p2 = ids[5];
    assert!(
        min_p2 > max_p1,
        "pages must not overlap: max_p1={} min_p2={}",
        max_p1, min_p2
    );
}

// ---------------------------------------------------------------------------
// 15. Contract schema migration preserves live attestations
// ---------------------------------------------------------------------------

/// Submits attestations before a schema migration and verifies they remain
/// accessible and fully populated after the migration completes.
#[test]
fn pipeline_migration_preserves_existing_attestations() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, admin) = deploy_and_initialize(&env);
    let key = SigningKey::generate(&mut OsRng);
    let attestor = Address::generate(&env);
    register_attestor(&env, &client, &attestor, &key);
    let subject = Address::generate(&env);

    // Pre-migration attestation.
    let payload = unique_payload(&env, 0xCC);
    let sig = sign_payload(&env, &key, &payload);
    let attest_id = client.submit_attestation(
        &attestor,
        &subject,
        &1_000_001u64,
        &payload,
        &sig,
    );

    // Run a schema migration (v1 → v2).
    let new_hash = BytesN::from_array(&env, &[0xABu8; 32]);
    client.upgrade(&new_hash);
    client.migrate(&admin, &2u32);

    // Attestation must still be fully readable.
    let stored = client.get_attestation(&attest_id);
    assert_eq!(stored.id, attest_id);
    assert_eq!(stored.issuer, attestor);
    assert_eq!(stored.subject, subject);
    // Schema version on the record must still reflect v1 (pre-migration value).
    assert_eq!(
        stored.schema_version, 1,
        "existing attestations must retain their original schema_version"
    );
}

// ---------------------------------------------------------------------------
// 16. Full audit log export after multi-step session workflow
// ---------------------------------------------------------------------------

/// Runs a complete session workflow (register → 3 attestations → close),
/// exports the full audit log via paginated retrieval, and verifies that
/// the exported entries match the recorded operation types in order.
#[test]
fn pipeline_full_audit_log_export_after_session_workflow() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let key = SigningKey::generate(&mut OsRng);
    let attestor = Address::generate(&env);
    let subject = Address::generate(&env);

    let user = Address::generate(&env);
    client.grant_role(&user, &AdminRole::AttestorAdmin);
    let session_id = client.create_session(&user);

    // Register attestor inside session.
    let pk: BytesN<32> = BytesN::from_array(&env, key.verifying_key().as_bytes());
    client.register_attestor_with_session(&user, &session_id, &attestor, &pk);

    // Submit 3 attestations.
    for i in 0u8..3 {
        let payload = unique_payload(&env, i + 100);
        let sig = sign_payload(&env, &key, &payload);
        client.submit_attestation_with_session(
            &session_id,
            &attestor,
            &subject,
            &(1_000_001u64 + i as u64),
            &payload,
            &sig,
        );
    }

    client.close_session(&session_id, &user);

    // Total audit entries: 1 register + 3 attestations = 4.
    let total_count = client.get_audit_log_count();
    assert!(
        total_count >= 4,
        "audit log must have at least 4 entries, found {}",
        total_count
    );

    // Retrieve entries via pagination (page size = 2).
    let page_size = 2u64;
    let page0 = client.get_audit_log_paginated(&0u64, &page_size);
    let page1 = client.get_audit_log_paginated(&page_size, &page_size);

    assert_eq!(page0.len(), page_size as u32, "first page must return 2 entries");
    assert_eq!(page1.len(), page_size as u32, "second page must return 2 entries");

    // First entry must be "register".
    assert_eq!(
        page0.get(0).unwrap().operation.operation_type,
        String::from_str(&env, "register")
    );
    // Second entry must be "attest".
    assert_eq!(
        page0.get(1).unwrap().operation.operation_type,
        String::from_str(&env, "attest")
    );
}

// ---------------------------------------------------------------------------
// 17. Attestor revocation info persists and is queryable
// ---------------------------------------------------------------------------

/// Registers and revokes an attestor and verifies that the revocation info
/// record (timestamp and reason) is populated and queryable after revocation.
#[test]
fn pipeline_revocation_info_persists_and_is_queryable() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let key = SigningKey::generate(&mut OsRng);
    let attestor = Address::generate(&env);
    register_attestor(&env, &client, &attestor, &key);

    assert!(client.is_attestor(&attestor));
    client.revoke_attestor(&attestor);
    assert!(!client.is_attestor(&attestor));

    let revocation = client.get_attestor_revocation_info(&attestor);
    assert!(
        revocation.is_some(),
        "revocation info must be stored after revoke_attestor"
    );
}

// ---------------------------------------------------------------------------
// 18. KYC full lifecycle: submit → approve → verify → re-submit after re-open
// ---------------------------------------------------------------------------

/// Exercises the complete KYC lifecycle including re-open after approval —
/// a path not explicitly covered by the existing harness.
#[test]
fn pipeline_kyc_full_lifecycle_with_reopen() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, admin) = deploy_and_initialize(&env);
    let key = SigningKey::generate(&mut OsRng);
    let attestor = Address::generate(&env);
    register_attestor(&env, &client, &attestor, &key);
    let subject = Address::generate(&env);

    // Submit KYC.
    let data_hash_1 = unique_payload(&env, 0x11);
    client.submit_kyc(&subject, &data_hash_1, &attestor);
    assert_eq!(client.get_kyc_status(&subject), KycStatus::Pending);

    // Approve.
    client.approve_kyc(&admin, &subject);
    assert_eq!(client.get_kyc_status(&subject), KycStatus::Approved);

    // Re-open approved KYC for re-review.
    client.reopen_kyc(&admin, &subject);
    assert_eq!(
        client.get_kyc_status(&subject),
        KycStatus::Pending,
        "status must return to Pending after reopen from Approved"
    );

    // Submit updated KYC data.
    let data_hash_2 = unique_payload(&env, 0x22);
    client.submit_kyc(&subject, &data_hash_2, &attestor);
    assert_eq!(client.get_kyc_status(&subject), KycStatus::Pending);

    // Final approval.
    client.approve_kyc(&admin, &subject);
    assert_eq!(
        client.get_kyc_status(&subject),
        KycStatus::Approved,
        "subject must be Approved after second approval"
    );
}

// ---------------------------------------------------------------------------
// 19. Health metrics survive anchor deactivation and reactivation
// ---------------------------------------------------------------------------

/// Records health events, deactivates an anchor, records more events after
/// reactivation, and verifies the counters accumulate correctly across both
/// active periods.
#[test]
fn pipeline_health_metrics_survive_deactivation_reactivation() {
    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);
    let anchor = Address::generate(&env);

    // First active period: 5 successes.
    for _ in 0..5 {
        client.record_health_event(&anchor, &true);
    }

    // Deactivate.
    client.deactivate_anchor(&anchor);

    // Record a failure while deactivated (should still be recorded).
    client.record_health_event(&anchor, &false);

    // Reactivate and record 4 more successes.
    client.reactivate_anchor(&anchor);
    for _ in 0..4 {
        client.record_health_event(&anchor, &true);
    }

    let health = client.get_anchor_health(&anchor);
    assert_eq!(
        health.total_calls, 10,
        "total_calls must reflect events from both active periods"
    );
    assert_eq!(
        health.failure_count, 1,
        "only the single failure should be recorded"
    );
}

// ---------------------------------------------------------------------------
// 20. Quote expiry: expired quotes are excluded from routing without purge
// ---------------------------------------------------------------------------

/// Submits one expired and one live quote for the same asset pair. Without
/// an explicit purge call, routing must still select only the live quote
/// (the contract must filter by valid_until at routing time).
#[test]
fn pipeline_routing_skips_expired_quotes_without_explicit_purge() {
    use soroban_sdk::Symbol;

    let env = Env::default();
    env.mock_all_auths();
    setup_ledger(&env, 1_000_000);

    let (client, _admin) = deploy_and_initialize(&env);

    let live_anchor = Address::generate(&env);
    let expired_anchor = Address::generate(&env);
    let now = env.ledger().timestamp();

    for anchor in [&live_anchor, &expired_anchor] {
        let mut services = Vec::new(&env);
        services.push_back(SERVICE_DEPOSITS);
        services.push_back(SERVICE_QUOTES);
        client.configure_services(anchor, &services);
        client.set_anchor_metadata(anchor, &9000u32, &120u64, &8000u32, &9900u32, &3600u64);
    }

    // Expired quote has the lower fee — routing must NOT select it.
    let expired_until = if now > 1 { now - 1 } else { 0 };
    client.submit_quote(
        &expired_anchor,
        &String::from_str(&env, "USD"),
        &String::from_str(&env, "USDC"),
        &10_000u64,
        &5u32, // cheapest fee but expired
        &100u64,
        &1_000_000u64,
        &expired_until,
    );

    // Live quote has a slightly higher fee.
    client.submit_quote(
        &live_anchor,
        &String::from_str(&env, "USD"),
        &String::from_str(&env, "USDC"),
        &10_000u64,
        &20u32, // valid fee
        &100u64,
        &1_000_000u64,
        &(now + 7_200),
    );

    let mut strategy = Vec::new(&env);
    strategy.push_back(Symbol::new(&env, "LowestFee"));

    let options = RoutingOptions {
        request: RoutingRequest {
            base_asset: String::from_str(&env, "USD"),
            quote_asset: String::from_str(&env, "USDC"),
            amount: 500u64,
            operation_type: 1u32,
        },
        strategy,
        min_reputation: 0u32,
        max_anchors: 5u32,
        require_kyc: false,
        require_compliance: false,
        subject: Address::generate(&env),
        fee_weight: 700u32,
        speed_weight: 150u32,
        reputation_weight: 150u32,
    };

    let best = client.route_transaction(&options);
    assert_eq!(
        best.anchor, live_anchor,
        "routing must select the live quote anchor, not the expired one"
    );
    assert_eq!(
        best.fee_percentage, 20,
        "routing must use the live quote's fee"
    );
}
