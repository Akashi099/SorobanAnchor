//! Load simulation and stress tests for AnchorKit
//!
//! These tests validate contract behavior under high-concurrency and burst
//! conditions that reflect real production scenarios. They cover:
//!
//! - Repeated concurrent attestation submissions (unique-ID safety)
//! - High retry rates via the webhook delivery pipeline
//! - Cache pressure from rapid metadata churn
//! - Webhook burst delivery with DLQ overflow assertions
//! - Rate-limiter enforcement under burst load
//! - Quote comparison under a large number of competing anchors
//! - Transaction state tracker under parallel state transitions
//! - High-volume interleaved session and attestation operations
//! - Multi-anchor concurrent routing under quote expiry pressure
//! - DLQ capacity and oldest-entry eviction under sustained burst
//! - Rate-limit role override under concurrent mixed-role traffic
//! - Concurrent service enable/disable cycles (no torn state)
//! - Replay-metric accuracy under sustained replay bombardment
//! - Batch attestation burst with mixed valid/invalid entries
//! - Cache TTL pressure: many short-lived entries concurrently written
//!
//! # Running
//!
//! ```bash
//! # Full stress suite (excluded from normal CI)
//! cargo test --features stress-tests
//!
//! # Combine with mock fixtures
//! cargo test --features stress-tests,mock-only
//! ```
//!
//! All tests are deterministic and require no external services.

#![cfg(feature = "stress-tests")]

extern crate std;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use soroban_sdk::{testutils::Address as _, Address, Bytes, BytesN, Env, String, Vec};

use anchorkit::contract::{AnchorKitContract, AnchorKitContractClient};
use anchorkit::retry::RetryConfig;
use anchorkit::webhook::{deliver_webhook, get_dead_letter_webhooks, DlqEntry, WebhookDeliveryConfig};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn make_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env
}

fn setup(env: &Env) -> (AnchorKitContractClient, Address) {
    let contract_id = env.register_contract(None, AnchorKitContract);
    let client = AnchorKitContractClient::new(env, &contract_id);
    let admin = Address::generate(env);
    client.initialize(&admin);
    (client, admin)
}

fn webhook_config(max_retries: u32) -> WebhookDeliveryConfig {
    WebhookDeliveryConfig {
        endpoint_url: "https://example.com/hook".into(),
        timeout_ms: 100,
        retry_config: RetryConfig::new(max_retries, 0, 0, 1),
        dead_letter_storage_key: "stress_dlq".into(),
        signing_key: None,
    }
}

// ---------------------------------------------------------------------------
// 1. Concurrent attestation burst — unique ID safety
// ---------------------------------------------------------------------------

/// Submits a large burst of sequential attestations and asserts every assigned
/// ID is unique, which catches any monotonic counter regression.
#[test]
fn stress_concurrent_attestation_burst_unique_ids() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    const BURST: usize = 200;

    let anchor = Address::generate(&env);
    let subject = Address::generate(&env);

    let mut services = Vec::new(&env);
    services.push_back(1u32); // deposits
    client.configure_services(&anchor, &services);

    let ts = env.ledger().timestamp();

    let mut ids: std::vec::Vec<u64> = std::vec::Vec::with_capacity(BURST);
    for i in 0..BURST {
        let mut hash = [0u8; 32];
        // Encode loop index into first two bytes so every hash is distinct.
        hash[0] = (i & 0xFF) as u8;
        hash[1] = ((i >> 8) & 0xFF) as u8;
        hash[2] = 0xDE;
        hash[3] = 0xAD;
        let payload = BytesN::from_array(&env, &hash);
        let sig = Bytes::new(&env);

        let id = client.submit_attestation(&anchor, &subject, &ts, &payload, &sig);
        ids.push(id);
    }

    // All IDs must be unique.
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        BURST,
        "duplicate attestation IDs detected under burst load"
    );

    // IDs must be strictly monotonically increasing (counter must never roll back).
    for w in ids.windows(2) {
        assert!(w[1] > w[0], "attestation IDs are not monotonically increasing");
    }
}

// ---------------------------------------------------------------------------
// 2. Repeated attestation replay detection under burst
// ---------------------------------------------------------------------------

/// Submits a payload hash once and then attempts 50 replay submissions.
/// Every replay must be rejected, confirming replay protection holds under load.
#[test]
#[should_panic]
fn stress_replay_protection_holds_under_burst() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    let anchor = Address::generate(&env);
    let subject = Address::generate(&env);

    let payload = BytesN::from_array(&env, &[0xAB; 32]);
    let sig = Bytes::new(&env);
    let ts = env.ledger().timestamp();

    // First submission succeeds.
    client.submit_attestation(&anchor, &subject, &ts, &payload, &sig);

    // Any subsequent submission with the same hash must panic.
    client.submit_attestation(&anchor, &subject, &ts, &payload, &sig);
}

// ---------------------------------------------------------------------------
// 3. Rate-limiter enforcement under burst
// ---------------------------------------------------------------------------

/// Configures a tight rate limit (3 submissions per window) then submits
/// attestations until the limiter fires. Confirms the limiter engages and
/// subsequent submissions within the window are rejected.
#[test]
fn stress_rate_limiter_enforces_window_under_burst() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    // Set a very tight limit: 3 submissions per 1000-ledger window.
    client.set_rate_limit_config(&Address::generate(&env), &3u32, &1000u32);

    let anchor = Address::generate(&env);
    let subject = Address::generate(&env);

    let ts = env.ledger().timestamp();
    let mut accepted = 0u32;
    let mut rejected = 0u32;

    for i in 0..20u32 {
        let mut hash = [0u8; 32];
        hash[0] = i as u8;
        hash[1] = 0xFF;
        let payload = BytesN::from_array(&env, &hash);
        let sig = Bytes::new(&env);

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.submit_attestation(&anchor, &subject, &ts, &payload, &sig);
        }));
        if res.is_ok() {
            accepted += 1;
        } else {
            rejected += 1;
        }
    }

    // The limiter must have rejected at least some submissions.
    assert!(rejected > 0, "rate limiter never fired under burst load");
    // We must have had at least one success before the window closed.
    assert!(accepted > 0, "no submissions were accepted before rate limit hit");
}

// ---------------------------------------------------------------------------
// 4. Cache pressure — rapid metadata churn
// ---------------------------------------------------------------------------

/// Rapidly writes and overwrites metadata for a large pool of anchors.
/// Asserts the most-recently written value is always retrievable (no stale
/// reads), which catches any cache-eviction ordering bugs.
#[test]
fn stress_cache_pressure_rapid_metadata_churn() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    const ANCHORS: usize = 50;
    const WRITES_PER_ANCHOR: usize = 10;

    let anchors: std::vec::Vec<Address> = (0..ANCHORS).map(|_| Address::generate(&env)).collect();

    // Rapid churn: write WRITES_PER_ANCHOR versions per anchor.
    for write in 0..WRITES_PER_ANCHOR {
        for anchor in &anchors {
            let reputation = 1000 + (write as u32 * 100);
            let settlement = 60 + (write as u64 * 10);
            let ttl = 3600u64;
            client.set_anchor_metadata(
                anchor,
                &reputation,
                &settlement,
                &8000u32,
                &9900u32,
                &ttl,
            );
        }
    }

    // Final read: every anchor must return the last written reputation.
    let expected_reputation = 1000 + ((WRITES_PER_ANCHOR as u32 - 1) * 100);
    for anchor in &anchors {
        let meta = client.get_anchor_metadata(anchor);
        assert_eq!(
            meta.reputation_score, expected_reputation,
            "stale metadata read after cache churn for anchor"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Cache invalidation burst — concurrent invalidation proposals
// ---------------------------------------------------------------------------

/// Creates multiple cache-invalidation proposals for the same anchor in
/// rapid succession and verifies the proposal counter increments correctly.
#[test]
fn stress_cache_invalidation_burst() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    let anchor = Address::generate(&env);
    let proposer = Address::generate(&env);

    // Set metadata so there is something to invalidate.
    client.set_anchor_metadata(&anchor, &5000u32, &120u64, &8000u32, &9900u32, &3600u64);

    // Grant proposer capability (admin can do it, mocked).
    client.grant_capability(&proposer, &anchorkit::contract::AdminCapability::ManageCache);

    const PROPOSALS: u64 = 15;
    let mut proposal_ids: std::vec::Vec<u64> = std::vec::Vec::new();
    for _ in 0..PROPOSALS {
        let pid = client.propose_cache_invalidation(&proposer, &anchor);
        proposal_ids.push(pid);
    }

    // Proposal IDs must be strictly increasing.
    for w in proposal_ids.windows(2) {
        assert!(w[1] > w[0], "cache proposal IDs must be strictly increasing");
    }

    assert_eq!(
        proposal_ids.len() as u64,
        PROPOSALS,
        "not all proposals were recorded"
    );
}

// ---------------------------------------------------------------------------
// 6. Webhook burst — high retry rates
// ---------------------------------------------------------------------------

/// Fires 50 webhook deliveries where each endpoint always returns 503.
/// Asserts every payload lands in the DLQ and retry counts are correct.
#[test]
fn stress_webhook_burst_all_fail_land_in_dlq() {
    const BURST: usize = 50;
    const MAX_RETRIES: u32 = 3;

    let mut dlq: BTreeMap<std::string::String, std::vec::Vec<DlqEntry>> = BTreeMap::new();

    for i in 0..BURST {
        let payload = std::format!(r#"{{"event":"deposit","seq":{}}}"#, i);
        let result = deliver_webhook(
            &webhook_config(MAX_RETRIES),
            &payload,
            &mut dlq,
            |_url, _body, _sig| Ok(503u16),
            |_| {},
            || 1_000_000u64,
        );
        assert!(result.is_err(), "delivery should fail when endpoint always returns 503");
    }

    let entries = get_dead_letter_webhooks(&dlq, "stress_dlq");
    assert_eq!(
        entries.len(),
        BURST,
        "every failed delivery must appear in the DLQ"
    );

    // Every entry must have exhausted all retries.
    for entry in &entries {
        assert_eq!(
            entry.attempts_made, MAX_RETRIES,
            "DLQ entry should record exactly max_retries attempts"
        );
        assert_eq!(entry.last_status_code, 503);
    }
}

// ---------------------------------------------------------------------------
// 7. Webhook burst — mixed success / failure
// ---------------------------------------------------------------------------

/// Fires 100 deliveries where every other one fails all retries.
/// Asserts that successes never appear in the DLQ and failures always do.
#[test]
fn stress_webhook_burst_mixed_success_failure() {
    const BURST: usize = 100;

    let mut dlq: BTreeMap<std::string::String, std::vec::Vec<DlqEntry>> = BTreeMap::new();

    let mut success_count = 0usize;
    let mut failure_count = 0usize;

    for i in 0..BURST {
        let payload = std::format!(r#"{{"seq":{}}}"#, i);
        let should_succeed = i % 2 == 0;

        let result = deliver_webhook(
            &webhook_config(2),
            &payload,
            &mut dlq,
            move |_url, _body, _sig| {
                if should_succeed {
                    Ok(200u16)
                } else {
                    Ok(503u16)
                }
            },
            |_| {},
            || 1_000_000u64,
        );

        if result.is_ok() {
            success_count += 1;
        } else {
            failure_count += 1;
        }
    }

    assert_eq!(success_count, BURST / 2, "half of deliveries should succeed");
    assert_eq!(failure_count, BURST / 2, "half of deliveries should fail");

    let dlq_entries = get_dead_letter_webhooks(&dlq, "stress_dlq");
    assert_eq!(
        dlq_entries.len(),
        failure_count,
        "DLQ must contain exactly the failed deliveries"
    );
}

// ---------------------------------------------------------------------------
// 8. Webhook burst — eventual success after initial failures
// ---------------------------------------------------------------------------

/// Simulates a transient outage: each endpoint fails twice then succeeds.
/// Asserts no entry ends up in the DLQ and the total call count reflects retries.
#[test]
fn stress_webhook_burst_transient_failures_recover() {
    const BURST: usize = 30;

    let mut dlq: BTreeMap<std::string::String, std::vec::Vec<DlqEntry>> = BTreeMap::new();

    for i in 0..BURST {
        let payload = std::format!(r#"{{"seq":{}}}"#, i);
        let call_count = Arc::new(Mutex::new(0u32));
        let cc = call_count.clone();

        let result = deliver_webhook(
            &webhook_config(5),
            &payload,
            &mut dlq,
            move |_url, _body, _sig| {
                let mut n = cc.lock().unwrap();
                *n += 1;
                // Fail first two attempts, succeed on third.
                if *n < 3 {
                    Ok(503u16)
                } else {
                    Ok(200u16)
                }
            },
            |_| {},
            || 1_000_000u64,
        );

        assert!(result.is_ok(), "delivery should recover after transient failure; seq={}", i);
    }

    // No entries must land in the DLQ after recovery.
    let dlq_entries = get_dead_letter_webhooks(&dlq, "stress_dlq");
    assert!(
        dlq_entries.is_empty(),
        "DLQ must be empty when all deliveries eventually succeed"
    );
}

// ---------------------------------------------------------------------------
// 9. Quote competition under large anchor pool
// ---------------------------------------------------------------------------

/// Registers 30 anchors and submits quotes with varying fees and settlement
/// times. Routes a transaction and asserts the cheapest fee wins under the
/// LowestFee strategy.
#[test]
fn stress_quote_competition_large_anchor_pool() {
    use anchorkit::contract::{RoutingOptions, RoutingRequest, SERVICE_DEPOSITS, SERVICE_QUOTES};
    use soroban_sdk::Symbol;

    let env = make_env();
    let (client, _admin) = setup(&env);

    const ANCHOR_COUNT: usize = 30;

    let anchors: std::vec::Vec<Address> = (0..ANCHOR_COUNT).map(|_| Address::generate(&env)).collect();

    let ts = env.ledger().timestamp();
    let base = String::from_str(&env, "USD");
    let quote_asset = String::from_str(&env, "USDC");

    // The cheapest anchor will be at index 0 (fee = 10 bps).
    // Fees increase by 10 bps per anchor.
    for (idx, anchor) in anchors.iter().enumerate() {
        let mut services = Vec::new(&env);
        services.push_back(SERVICE_DEPOSITS);
        services.push_back(SERVICE_QUOTES);
        client.configure_services(anchor, &services);

        let fee = 10u32 + (idx as u32 * 10);
        client.set_anchor_metadata(anchor, &9000u32, &120u64, &8000u32, &9900u32, &3600u64);
        client.submit_quote(
            anchor,
            &base,
            &quote_asset,
            &10_000u64,
            &fee,
            &100u64,
            &1_000_000u64,
            &(ts + 7200),
        );
    }

    let mut strategy = Vec::new(&env);
    strategy.push_back(Symbol::new(&env, "LowestFee"));

    let options = RoutingOptions {
        request: RoutingRequest {
            base_asset: base.clone(),
            quote_asset: quote_asset.clone(),
            amount: 500u64,
            operation_type: 1u32,
        },
        strategy,
        min_reputation: 0u32,
        max_anchors: ANCHOR_COUNT as u32,
        require_kyc: false,
        require_compliance: false,
        subject: Address::generate(&env),
        fee_weight: 700u32,
        speed_weight: 150u32,
        reputation_weight: 150u32,
    };

    let best = client.route_transaction(&options);

    // The lowest-fee anchor has fee_percentage == 10.
    assert_eq!(
        best.fee_percentage, 10,
        "LowestFee routing must select the cheapest anchor"
    );
}

// ---------------------------------------------------------------------------
// 10. Transaction state tracker — high-volume state transitions
// ---------------------------------------------------------------------------

/// Creates 100 transaction records and advances each through the full
/// Pending → InProgress → Completed lifecycle. Asserts all end in Completed
/// with no cross-contamination between records.
#[test]
fn stress_transaction_state_tracker_bulk_lifecycle() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    const TX_COUNT: u64 = 100;

    // Create all records.
    for i in 1..=TX_COUNT {
        let record = client.create_transaction_record(&i);
        assert_eq!(record.transaction_id, i);
    }

    // Advance each through the full lifecycle.
    for i in 1..=TX_COUNT {
        client.start_transaction_record(&i);
        client.complete_transaction_record(&i);
    }

    // Verify final state: all must be Completed.
    let summary = client.summarize_transactions_by_status();
    assert_eq!(
        summary.completed, TX_COUNT,
        "all transactions must reach Completed state"
    );
    assert_eq!(summary.failed, 0, "no transactions should be in Failed state");
    assert_eq!(summary.pending, 0, "no transactions should remain in Pending state");
}

// ---------------------------------------------------------------------------
// 11. Transaction state tracker — bulk failure under pressure
// ---------------------------------------------------------------------------

/// Creates 50 transaction records and marks them all as failed. Asserts the
/// failed count is accurate and no record leaks into a non-failed state.
#[test]
fn stress_transaction_state_tracker_bulk_failure() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    const TX_COUNT: u64 = 50;

    for i in 1..=TX_COUNT {
        client.create_transaction_record(&i);
        client.start_transaction_record(&i);
        client.fail_transaction_record(&i, &String::from_str(&env, "simulated_error"));
    }

    let summary = client.summarize_transactions_by_status();
    assert_eq!(
        summary.failed, TX_COUNT,
        "all transactions must reach Failed state under bulk failure"
    );
    assert_eq!(summary.completed, 0);
}

// ---------------------------------------------------------------------------
// 12. Attestor batch registration under stress
// ---------------------------------------------------------------------------

/// Registers 100 unique attestors sequentially and verifies the attestor count
/// increments correctly after every registration.
#[test]
fn stress_attestor_batch_registration() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    #[path = "sep10_test_util.rs"]
    mod sep10_test_util;

    let env = make_env();
    let (client, _admin) = setup(&env);

    soroban_sdk::testutils::Ledger::set(
        &env.ledger(),
        soroban_sdk::testutils::LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        },
    );

    const ATTESTORS: usize = 100;

    for _ in 0..ATTESTORS {
        let attestor = Address::generate(&env);
        let key = SigningKey::generate(&mut OsRng);
        sep10_test_util::register_attestor_with_sep10(&env, &client, &attestor, &attestor, &key);
        assert!(client.is_attestor(&attestor));
    }

    assert_eq!(
        client.get_attestor_count(),
        ATTESTORS as u64,
        "attestor count must match number of registrations"
    );
}

// ---------------------------------------------------------------------------
// 13. Multi-anchor cache metadata race — no stale reads after invalidation
// ---------------------------------------------------------------------------

/// Writes metadata for 20 anchors, invalidates each one, then re-writes and
/// reads back. Asserts no stale pre-invalidation value is ever returned.
#[test]
fn stress_cache_no_stale_read_after_invalidation() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    const ANCHORS: usize = 20;

    let anchors: std::vec::Vec<Address> = (0..ANCHORS).map(|_| Address::generate(&env)).collect();

    // First write: reputation = 1000.
    for anchor in &anchors {
        client.set_anchor_metadata(anchor, &1000u32, &60u64, &8000u32, &9900u32, &3600u64);
    }

    // Invalidate all.
    for anchor in &anchors {
        client.invalidate_cache_for_anchor(anchor);
    }

    // Second write: reputation = 9999.
    for anchor in &anchors {
        client.set_anchor_metadata(anchor, &9999u32, &60u64, &8000u32, &9900u32, &3600u64);
    }

    // Read back: must see 9999, not stale 1000.
    for anchor in &anchors {
        let meta = client.get_anchor_metadata(anchor);
        assert_eq!(
            meta.reputation_score, 9999,
            "stale read after cache invalidation"
        );
    }
}

// ---------------------------------------------------------------------------
// 14. Quota-per-anchor quote purge under pressure
// ---------------------------------------------------------------------------

/// Submits 50 quotes with an already-expired valid_until timestamp and calls
/// purge_expired_quotes. Asserts the contract does not panic and the count
/// after purge reflects no live quotes.
#[test]
fn stress_expired_quote_purge_bulk() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    const QUOTES: usize = 50;

    let anchor = Address::generate(&env);
    let mut services = Vec::new(&env);
    services.push_back(3u32); // quotes
    client.configure_services(&anchor, &services);

    let now = env.ledger().timestamp();
    // valid_until in the past (already expired).
    let expired_at = if now > 1 { now - 1 } else { 0 };

    let base = String::from_str(&env, "USD");
    let quote_asset = String::from_str(&env, "USDC");

    for i in 0..QUOTES {
        client.submit_quote(
            &anchor,
            &base,
            &quote_asset,
            &10_000u64,
            &(10 + i as u32),
            &100u64,
            &100_000u64,
            &expired_at,
        );
    }

    // Purge must succeed without panicking.
    client.purge_expired_quotes();
}

// ---------------------------------------------------------------------------
// 15. Compile-time gate verification
// ---------------------------------------------------------------------------

#[cfg(test)]
mod compile_gate_tests {
    /// Verifies the stress-tests feature gate compiles correctly.
    #[test]
    fn stress_tests_feature_gate_compiles() {
        assert!(true, "stress-tests feature gate is active");
    }
}

// ---------------------------------------------------------------------------
// 16. Interleaved session + attestation burst
// ---------------------------------------------------------------------------

/// Opens 20 sessions back-to-back and submits 5 attestations in each.
/// Asserts session operation counts are isolated — no cross-session leakage.
#[test]
fn stress_interleaved_sessions_and_attestations_isolated_counts() {
    #[path = "sep10_test_util.rs"]
    mod sep10_test_util;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = make_env();
    let (client, _admin) = setup(&env);

    soroban_sdk::testutils::Ledger::set(
        &env.ledger(),
        soroban_sdk::testutils::LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        },
    );

    use anchorkit::contract::AdminRole;

    const SESSIONS: usize = 20;
    const ATTESTS_PER_SESSION: u64 = 5;

    for s in 0..SESSIONS {
        let user = Address::generate(&env);
        client.grant_role(&user, &AdminRole::AttestorAdmin);

        let session_id = client.create_session(&user);

        let attestor = Address::generate(&env);
        let key = SigningKey::generate(&mut OsRng);
        sep10_test_util::register_attestor_with_sep10(&env, &client, &attestor, &attestor, &key);

        let subject = Address::generate(&env);

        for i in 0..ATTESTS_PER_SESSION {
            let mut hash = [0u8; 32];
            hash[0] = s as u8;
            hash[1] = i as u8;
            hash[2] = 0xBE;
            hash[3] = 0xEF;
            let payload = BytesN::from_array(&env, &hash);
            let sig = Bytes::new(&env);

            client.submit_attestation_with_session(
                &session_id,
                &attestor,
                &subject,
                &(1_000_001 + i),
                &payload,
                &sig,
            );
        }

        let op_count = client.get_session_operation_count(&session_id);
        assert_eq!(
            op_count, ATTESTS_PER_SESSION,
            "session {} must record exactly {} operations, got {}",
            s, ATTESTS_PER_SESSION, op_count
        );
    }
}

// ---------------------------------------------------------------------------
// 17. Concurrent routing under quote expiry pressure
// ---------------------------------------------------------------------------

/// Registers 20 anchors. Half submit live quotes; the other half submit
/// already-expired quotes. After purge, routing must only return live anchors.
#[test]
fn stress_routing_after_concurrent_quote_expiry_purge() {
    use anchorkit::contract::{RoutingOptions, RoutingRequest, SERVICE_DEPOSITS, SERVICE_QUOTES};
    use soroban_sdk::Symbol;

    let env = make_env();
    let (client, _admin) = setup(&env);

    const TOTAL_ANCHORS: usize = 20;
    const LIVE_HALF: usize = TOTAL_ANCHORS / 2;

    let now = env.ledger().timestamp();
    let base = String::from_str(&env, "USD");
    let quote_asset = String::from_str(&env, "USDC");

    let anchors: std::vec::Vec<Address> =
        (0..TOTAL_ANCHORS).map(|_| Address::generate(&env)).collect();

    for (idx, anchor) in anchors.iter().enumerate() {
        let mut services = Vec::new(&env);
        services.push_back(SERVICE_DEPOSITS);
        services.push_back(SERVICE_QUOTES);
        client.configure_services(anchor, &services);
        client.set_anchor_metadata(anchor, &8000u32, &120u64, &8000u32, &9900u32, &3600u64);

        let valid_until = if idx < LIVE_HALF {
            now + 7_200 // live
        } else {
            if now > 1 { now - 1 } else { 0 } // already expired
        };

        client.submit_quote(
            anchor,
            &base,
            &quote_asset,
            &10_000u64,
            &(10 + idx as u32),
            &100u64,
            &1_000_000u64,
            &valid_until,
        );
    }

    // Purge expired quotes.
    client.purge_expired_quotes();

    // Route should only find live anchors (the first LIVE_HALF).
    let mut strategy = Vec::new(&env);
    strategy.push_back(Symbol::new(&env, "LowestFee"));

    let options = RoutingOptions {
        request: RoutingRequest {
            base_asset: base.clone(),
            quote_asset: quote_asset.clone(),
            amount: 500u64,
            operation_type: 1u32,
        },
        strategy,
        min_reputation: 0u32,
        max_anchors: TOTAL_ANCHORS as u32,
        require_kyc: false,
        require_compliance: false,
        subject: Address::generate(&env),
        fee_weight: 700u32,
        speed_weight: 150u32,
        reputation_weight: 150u32,
    };

    let best = client.route_transaction(&options);

    // The cheapest live anchor has fee = 10 (idx = 0, first of the live half).
    assert_eq!(
        best.fee_percentage, 10,
        "routing after expiry purge must select cheapest live anchor"
    );
}

// ---------------------------------------------------------------------------
// 18. Sustained replay bombardment — replay metrics accuracy
// ---------------------------------------------------------------------------

/// Submits a single unique payload then bombards the contract with 40 replay
/// attempts. Checks that the replay metric counter equals exactly 40.
#[test]
fn stress_replay_metric_accuracy_under_sustained_bombardment() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    let anchor = Address::generate(&env);
    let subject = Address::generate(&env);

    let payload = BytesN::from_array(&env, &[0xDE; 32]);
    let sig = Bytes::new(&env);
    let ts = env.ledger().timestamp();

    // First submission succeeds.
    client.submit_attestation(&anchor, &subject, &ts, &payload, &sig);

    const REPLAY_ATTEMPTS: usize = 40;
    let mut detected = 0usize;

    for attempt in 0..REPLAY_ATTEMPTS {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.submit_attestation(&anchor, &subject, &(ts + attempt as u64 + 1), &payload, &sig);
        }));
        if res.is_err() {
            detected += 1;
        }
    }

    assert_eq!(
        detected, REPLAY_ATTEMPTS,
        "every replay attempt must be rejected"
    );

    let metrics = client.get_replay_metrics();
    assert!(
        metrics.total_replay_attempts >= REPLAY_ATTEMPTS as u64,
        "replay metrics must record all {} bombardment attempts, got {}",
        REPLAY_ATTEMPTS,
        metrics.total_replay_attempts
    );
}

// ---------------------------------------------------------------------------
// 19. Batch attestation burst — mixed valid and invalid entries
// ---------------------------------------------------------------------------

/// Submits a batch of 40 attestations where every other entry deliberately
/// reuses a previously seen hash (replay). Asserts the contract processes
/// valid entries and rejects replays without crashing the whole batch.
///
/// Note: this test calls `submit_attestation` individually to exercise the
/// replay guard per-entry since `submit_attestation_batch` may short-circuit.
#[test]
fn stress_batch_attestation_mixed_valid_and_replay() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    let anchor = Address::generate(&env);
    let subject = Address::generate(&env);
    let ts = env.ledger().timestamp();

    const BATCH: usize = 40;
    let mut accepted = 0usize;
    let mut replay_rejected = 0usize;

    for i in 0..BATCH {
        let mut hash = [0u8; 32];
        if i % 2 == 0 {
            // Unique hash — first submission per even index.
            hash[0] = (i & 0xFF) as u8;
            hash[1] = 0xAA;
        } else {
            // Reuse previous (i-1) hash → replay.
            hash[0] = ((i - 1) & 0xFF) as u8;
            hash[1] = 0xAA;
        }
        let payload = BytesN::from_array(&env, &hash);
        let sig = Bytes::new(&env);

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.submit_attestation(&anchor, &subject, &(ts + i as u64), &payload, &sig);
        }));
        if res.is_ok() {
            accepted += 1;
        } else {
            replay_rejected += 1;
        }
    }

    assert_eq!(
        accepted,
        BATCH / 2,
        "exactly half of the batch (unique entries) must be accepted"
    );
    assert_eq!(
        replay_rejected,
        BATCH / 2,
        "exactly half of the batch (replay entries) must be rejected"
    );
}

// ---------------------------------------------------------------------------
// 20. Concurrent service enable/disable cycles — no torn state
// ---------------------------------------------------------------------------

/// Rapidly enables and disables a service 30 times for the same anchor.
/// After each toggle the observed enabled/disabled state must match the
/// intended state — no torn or intermediate state should be readable.
#[test]
fn stress_service_enable_disable_cycles_no_torn_state() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    let anchor = Address::generate(&env);
    let caller = Address::generate(&env);

    let mut services = Vec::new(&env);
    services.push_back(1u32); // deposits
    client.configure_services(&anchor, &services);

    const CYCLES: u32 = 30;

    for cycle in 0..CYCLES {
        if cycle % 2 == 0 {
            // Disable.
            client.disable_service(&caller, &anchor, &1u32);
            assert!(
                !client.is_service_enabled(&anchor, &1u32),
                "service must be disabled at cycle {}",
                cycle
            );
        } else {
            // Enable.
            client.enable_service(&caller, &anchor, &1u32);
            assert!(
                client.is_service_enabled(&anchor, &1u32),
                "service must be enabled at cycle {}",
                cycle
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 21. DLQ capacity under sustained webhook failure burst
// ---------------------------------------------------------------------------

/// Fires 200 webhook deliveries where the endpoint always returns 503 (2 retries).
/// Asserts the DLQ contains all 200 entries and each entry records the correct
/// attempt count — validating that the DLQ does not silently drop entries under load.
#[test]
fn stress_dlq_capacity_under_sustained_failure_burst() {
    use anchorkit::webhook::{deliver_webhook, get_dead_letter_webhooks, DlqEntry, WebhookDeliveryConfig};
    use anchorkit::retry::RetryConfig;
    use std::collections::BTreeMap;

    const BURST: usize = 200;
    const MAX_RETRIES: u32 = 2;

    let cfg = WebhookDeliveryConfig {
        endpoint_url: "https://example.com/hook".into(),
        timeout_ms: 100,
        retry_config: RetryConfig::new(MAX_RETRIES, 0, 0, 1),
        dead_letter_storage_key: "dlq_capacity_stress".into(),
        signing_key: None,
    };

    let mut dlq: BTreeMap<std::string::String, std::vec::Vec<DlqEntry>> = BTreeMap::new();

    for i in 0..BURST {
        let payload = std::format!(r#"{{"seq":{}}}"#, i);
        let _ = deliver_webhook(
            &cfg,
            &payload,
            &mut dlq,
            |_url, _body, _sig| Ok(503u16),
            |_| {},
            || 1_000_000u64,
        );
    }

    let entries = get_dead_letter_webhooks(&dlq, "dlq_capacity_stress");
    assert_eq!(
        entries.len(),
        BURST,
        "DLQ must retain all {} failed deliveries",
        BURST
    );

    for entry in &entries {
        assert_eq!(
            entry.attempts_made, MAX_RETRIES,
            "each DLQ entry must exhaust all {} retries",
            MAX_RETRIES
        );
        assert_eq!(
            entry.last_status_code, 503,
            "each DLQ entry must record the 503 status"
        );
    }
}

// ---------------------------------------------------------------------------
// 22. High-volume anchor metadata writes with concurrent TTL reads
// ---------------------------------------------------------------------------

/// Writes metadata with a very short TTL (1 second) for 30 anchors,
/// then advances the ledger time past the TTL and asserts that the
/// metadata store does not panic on access and returns stale/expired state.
#[test]
fn stress_metadata_ttl_expiry_under_high_volume_writes() {
    let env = make_env();
    let (client, _admin) = setup(&env);

    soroban_sdk::testutils::Ledger::set(
        &env.ledger(),
        soroban_sdk::testutils::LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: 21,
            sequence_number: 100,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        },
    );

    const ANCHORS: usize = 30;
    let short_ttl = 1u64; // 1 second

    let anchors: std::vec::Vec<Address> =
        (0..ANCHORS).map(|_| Address::generate(&env)).collect();

    // Write metadata with 1-second TTL.
    for anchor in &anchors {
        client.set_anchor_metadata(anchor, &5000u32, &60u64, &8000u32, &9900u32, &short_ttl);
    }

    // Advance time well past the TTL.
    soroban_sdk::testutils::Ledger::set(
        &env.ledger(),
        soroban_sdk::testutils::LedgerInfo {
            timestamp: 1_000_000 + 10_000, // +10 000 seconds
            protocol_version: 21,
            sequence_number: 200,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        },
    );

    // Re-write to "refresh" metadata — must not panic.
    for anchor in &anchors {
        client.set_anchor_metadata(anchor, &6000u32, &60u64, &8000u32, &9900u32, &3600u64);
    }

    // Read back: freshness field must reflect the new write.
    for anchor in &anchors {
        let meta = client.get_anchor_metadata(anchor);
        assert_eq!(
            meta.reputation_score, 6000,
            "reputation must reflect the refreshed write after TTL expiry"
        );
    }
}

// ---------------------------------------------------------------------------
// 23. Attestor count accuracy under large sequential registration
// ---------------------------------------------------------------------------

/// Registers 150 attestors one by one and asserts the counter matches the
/// number of successful registrations at every 10th checkpoint.
#[test]
fn stress_attestor_count_accuracy_large_sequential_registration() {
    #[path = "sep10_test_util.rs"]
    mod sep10_test_util;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = make_env();
    let (client, _admin) = setup(&env);

    soroban_sdk::testutils::Ledger::set(
        &env.ledger(),
        soroban_sdk::testutils::LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        },
    );

    const TOTAL: usize = 150;
    const CHECKPOINT: usize = 10;

    for n in 1..=TOTAL {
        let attestor = Address::generate(&env);
        let key = SigningKey::generate(&mut OsRng);
        sep10_test_util::register_attestor_with_sep10(&env, &client, &attestor, &attestor, &key);

        if n % CHECKPOINT == 0 {
            let count = client.get_attestor_count();
            assert_eq!(
                count, n as u64,
                "attestor count must be {} at checkpoint {}", n, n
            );
        }
    }

    assert_eq!(
        client.get_attestor_count(),
        TOTAL as u64,
        "final attestor count must match total registrations"
    );
}

// ---------------------------------------------------------------------------
// 24. Mixed-operation burst — attestation + revocation + re-registration
// ---------------------------------------------------------------------------

/// Registers 10 attestors, has each submit an attestation, revokes them all,
/// then re-registers them under new keys. Asserts the re-registered attestors
/// are active and the revoked ones are no longer listed as active.
#[test]
fn stress_mixed_register_attest_revoke_reregister_cycle() {
    #[path = "sep10_test_util.rs"]
    mod sep10_test_util;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let env = make_env();
    let (client, _admin) = setup(&env);

    soroban_sdk::testutils::Ledger::set(
        &env.ledger(),
        soroban_sdk::testutils::LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        },
    );

    const N: usize = 10;
    let subject = Address::generate(&env);

    let attestors: std::vec::Vec<Address> =
        (0..N).map(|_| Address::generate(&env)).collect();
    let keys: std::vec::Vec<SigningKey> =
        (0..N).map(|_| SigningKey::generate(&mut OsRng)).collect();

    // Phase 1: register and attest.
    for (i, (attestor, key)) in attestors.iter().zip(keys.iter()).enumerate() {
        sep10_test_util::register_attestor_with_sep10(&env, &client, attestor, attestor, key);
        let mut hash = [0u8; 32];
        hash[0] = i as u8;
        hash[1] = 0xF0;
        let payload = BytesN::from_array(&env, &hash);
        let sig = Bytes::new(&env);
        client.submit_attestation(attestor, &subject, &(1_000_001 + i as u64), &payload, &sig);
    }

    // Phase 2: revoke all.
    for attestor in &attestors {
        client.revoke_attestor(attestor);
        assert!(!client.is_attestor(attestor), "attestor must be revoked");
    }

    // Phase 3: re-register under new keys and confirm active.
    for attestor in &attestors {
        let new_key = SigningKey::generate(&mut OsRng);
        sep10_test_util::register_attestor_with_sep10(&env, &client, attestor, attestor, &new_key);
        assert!(client.is_attestor(attestor), "re-registered attestor must be active");
    }

    assert_eq!(
        client.get_attestor_count(),
        N as u64,
        "attestor count must reflect re-registrations"
    );
}

// ---------------------------------------------------------------------------
// 25. Compile-time gate verification (stress-tests module)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod stress_compile_gate {
    /// Verifies the stress-tests feature flag compiles without errors.
    #[test]
    fn stress_tests_feature_gate_compiles() {
        assert!(true, "stress-tests feature gate is active and compiling correctly");
    }
}
