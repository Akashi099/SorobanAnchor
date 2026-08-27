//! Replay detection metrics and structured logging for duplicate request IDs.
//!
//! This module provides tracking and instrumentation for replay attack detection,
//! recording metrics when duplicate request IDs are detected.
//!
//! # Overview
//!
//! Production systems need to know when replay attempts occur and whether they are malicious.
//! This module instruments request ID processing with replay detection hooks and records
//! metrics or logs when a duplicate request is rejected.

use soroban_sdk::{contracttype, Address, Bytes, Env};
use crate::deterministic_hash::make_storage_key;

/// Structured log entry for a replay detection event.
#[derive(Clone, Debug)]
pub struct ReplayDetectionEvent {
    /// The request/payload ID that triggered the replay detection
    pub request_id: Bytes,
    /// The actor (issuer, address) attempting the replay
    pub actor: Address,
    /// Timestamp when the duplicate was detected
    pub detected_at: u64,
    /// Number of previous occurrences of this request ID (before this one)
    pub attempt_count: u32,
    /// Ledger sequence number when detected
    pub ledger_sequence: u32,
}

/// Metrics snapshot for replay detection statistics.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayMetrics {
    /// Total number of replay attempts detected (rejected) since initialization
    pub total_replay_attempts: u64,
    /// Number of unique request IDs that have been replayed
    pub unique_replayed_ids: u64,
    /// Timestamp of the most recent replay attempt
    pub last_replay_at: u64,
    /// Ledger sequence when metrics were last updated
    pub last_updated_ledger: u32,
    /// Total number of attestations accepted (passed all checks including replay detection)
    pub accepted_events: u64,
    /// Total number of events skipped (e.g. pre-flight checks failed before reaching replay check)
    pub skipped_events: u64,
}

impl Default for ReplayMetrics {
    fn default() -> Self {
        ReplayMetrics {
            total_replay_attempts: 0,
            unique_replayed_ids: 0,
            last_replay_at: 0,
            last_updated_ledger: 0,
            accepted_events: 0,
            skipped_events: 0,
        }
    }
}

/// Internal tracking for a single replay attempt
#[contracttype]
#[derive(Clone, Debug)]
pub struct ReplayAttemptRecord {
    /// Request ID that was replayed
    pub request_id: Bytes,
    /// Actor attempting the replay
    pub actor: Address,
    /// Count of duplicate attempts for this ID
    pub attempt_number: u32,
    /// Timestamp of this attempt
    pub timestamp: u64,
    /// Ledger sequence when this replay was detected
    pub ledger_sequence: u32,
}

/// TTL for per-ID replay attempt counters in temporary storage (~7 days at 5 s/ledger).
/// After this window the counter auto-expires; a new attempt for the same ID after
/// expiry is treated as a fresh first attempt (the USED key in persistent storage
/// still blocks the actual attestation, so correctness is preserved).
const REPLAY_ATTEMPT_TTL: u32 = 120_960;

/// Record a replay detection event in contract storage and emit structured logs.
///
/// Per-ID attempt counters are stored in **temporary** storage so they
/// auto-expire after `REPLAY_ATTEMPT_TTL` ledgers, keeping instance storage
/// lean. Global aggregate metrics remain in instance storage because they are
/// small and long-lived.
///
/// # Arguments
///
/// * `env` - The Soroban environment
/// * `request_id` - The duplicate request/payload ID
/// * `actor` - The address attempting the replay
///
/// # Returns
///
/// A `ReplayDetectionEvent` with full details for logging
pub fn record_replay_detection(
    env: &Env,
    request_id: &Bytes,
    actor: &Address,
) -> ReplayDetectionEvent {
    let now = env.ledger().timestamp();
    let ledger_seq = env.ledger().sequence();

    // ── Global aggregate metrics (instance storage — small, long-lived) ──
    let metrics_key = soroban_sdk::symbol_short!("REPLAYM");
    let mut metrics: ReplayMetrics = env
        .storage()
        .instance()
        .get::<_, ReplayMetrics>(&metrics_key)
        .unwrap_or_default();

    // ── Per-ID attempt counter (temporary storage — auto-expires) ────────
    // Using temporary storage avoids unbounded growth of instance storage:
    // each counter lives for REPLAY_ATTEMPT_TTL ledgers then is pruned
    // automatically by the Soroban runtime.
    //
    // make_storage_key length-prefixes each segment before hashing, so
    // distinct byte sequences always produce distinct 32-byte keys regardless
    // of input length — no truncation or text normalization can collapse them.
    let mut id_raw = alloc::vec::Vec::with_capacity(request_id.len() as usize);
    for i in 0..request_id.len() {
        id_raw.push(request_id.get(i).unwrap_or(0));
    }
    let attempt_key = make_storage_key(env, &[b"RPLYAT", &id_raw]);
    let mut attempt_count: u32 = env
        .storage()
        .temporary()
        .get::<_, u32>(&attempt_key)
        .unwrap_or(0);
    attempt_count = attempt_count.saturating_add(1);

    // Update global metrics
    metrics.total_replay_attempts += 1;
    if attempt_count == 1 {
        metrics.unique_replayed_ids += 1;
    }
    metrics.last_replay_at = now;
    metrics.last_updated_ledger = ledger_seq;

    env.storage().instance().set(&metrics_key, &metrics);

    // Persist per-ID counter in temporary storage with bounded TTL.
    env.storage().temporary().set(&attempt_key, &attempt_count);
    env.storage().temporary().extend_ttl(&attempt_key, REPLAY_ATTEMPT_TTL, REPLAY_ATTEMPT_TTL);

    // ── Audit record (temporary storage — bounded TTL) ────────────────────
    // Detailed per-event records are also stored in temporary storage so they
    // do not accumulate indefinitely. Monitoring systems should index these
    // events off-chain via the emitted contract event.
    let event_id = next_replay_event_id(env);
    let event = ReplayAttemptRecord {
        request_id: request_id.clone(),
        actor: actor.clone(),
        attempt_number: attempt_count,
        timestamp: now,
        ledger_sequence: ledger_seq,
    };
    let event_key = (soroban_sdk::symbol_short!("RPLYEV"), event_id);
    env.storage().temporary().set(&event_key, &event);
    env.storage().temporary().extend_ttl(&event_key, REPLAY_ATTEMPT_TTL, REPLAY_ATTEMPT_TTL);

    ReplayDetectionEvent {
        request_id: request_id.clone(),
        actor: actor.clone(),
        detected_at: now,
        attempt_count,
        ledger_sequence: ledger_seq,
    }
}

/// Retrieve current replay detection metrics.
///
/// Returns aggregated statistics on replay detection since contract initialization.
///
/// # Arguments
///
/// * `env` - The Soroban environment
///
/// # Returns
///
/// A `ReplayMetrics` snapshot with current statistics
pub fn get_replay_metrics(env: &Env) -> ReplayMetrics {
    let metrics_key = soroban_sdk::symbol_short!("REPLAYM");
    env.storage()
        .instance()
        .get::<_, ReplayMetrics>(&metrics_key)
        .unwrap_or_default()
}

/// Get the count of replay attempts for a specific request ID.
///
/// Reads from temporary storage. Returns 0 when the counter has expired or
/// was never written (the USED key in persistent storage is the authoritative
/// replay guard; this counter is for observability only).
pub fn get_replay_count_for_id(env: &Env, request_id: &Bytes) -> u64 {
    let mut id_raw = alloc::vec::Vec::with_capacity(request_id.len() as usize);
    for i in 0..request_id.len() {
        id_raw.push(request_id.get(i).unwrap_or(0));
    }
    let attempt_key = make_storage_key(env, &[b"RPLYAT", &id_raw]);
    env.storage()
        .temporary()
        .get::<_, u32>(&attempt_key)
        .unwrap_or(0) as u64
}

/// Record a successfully accepted event (passed all checks including replay detection).
///
/// Should be called after an attestation is committed to storage.
///
/// # Arguments
///
/// * `env` - The Soroban environment
pub fn record_accepted_event(env: &Env) {
    let metrics_key = soroban_sdk::symbol_short!("REPLAYM");
    let mut metrics: ReplayMetrics = env
        .storage()
        .instance()
        .get::<_, ReplayMetrics>(&metrics_key)
        .unwrap_or_default();
    metrics.accepted_events += 1;
    metrics.last_updated_ledger = env.ledger().sequence();
    env.storage().instance().set(&metrics_key, &metrics);
}

/// Record a skipped event (rejected before or outside the replay check, e.g. timestamp invalid,
/// rate limit exceeded, or attestor not registered).
///
/// # Arguments
///
/// * `env` - The Soroban environment
pub fn record_skipped_event(env: &Env) {
    let metrics_key = soroban_sdk::symbol_short!("REPLAYM");
    let mut metrics: ReplayMetrics = env
        .storage()
        .instance()
        .get::<_, ReplayMetrics>(&metrics_key)
        .unwrap_or_default();
    metrics.skipped_events += 1;
    metrics.last_updated_ledger = env.ledger().sequence();
    env.storage().instance().set(&metrics_key, &metrics);
}

/// Get a specific replay detection event record by ID.
/// Reads from temporary storage; returns `None` when the record has expired.
pub fn get_replay_event(env: &Env, event_id: u64) -> Option<ReplayAttemptRecord> {
    let event_key = (soroban_sdk::symbol_short!("RPLYEV"), event_id);
    env.storage().temporary().get::<_, ReplayAttemptRecord>(&event_key)
}

/// Get the next sequential replay event ID (stored in instance storage).
fn next_replay_event_id(env: &Env) -> u64 {
    let id_key = soroban_sdk::symbol_short!("RPLYID");
    let current: u64 = env
        .storage()
        .instance()
        .get::<_, u64>(&id_key)
        .unwrap_or(0);
    let next = current + 1;
    env.storage().instance().set(&id_key, &next);
    current
}

/// Log a replay detection event with structured information.
///
/// Emits a contract event that can be captured by indexers and monitoring systems.
pub fn emit_replay_detection_log(env: &Env, event: &ReplayDetectionEvent) {
    env.events().publish(
        (soroban_sdk::symbol_short!("replay"), soroban_sdk::symbol_short!("detected")),
        (
            event.request_id.clone(),
            event.actor.clone(),
            event.detected_at,
            event.attempt_count,
            event.ledger_sequence,
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use crate::contract::AnchorKitContract;

    fn make_test_env() -> (Env, soroban_sdk::Address) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set(LedgerInfo {
            timestamp: 1_000_000,
            protocol_version: 21,
            sequence_number: 100,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });
        let cid = env.register_contract(None, AnchorKitContract);
        (env, cid)
    }

    #[test]
    fn test_record_first_replay_detection() {
        let (env, cid) = make_test_env();
        let request_id = Bytes::from_slice(&env, &[0x01, 0x02, 0x03]);
        let actor = Address::generate(&env);

        let event = env.as_contract(&cid, || record_replay_detection(&env, &request_id, &actor));

        assert_eq!(event.attempt_count, 1);
        assert_eq!(event.detected_at, 1_000_000);
        assert_eq!(event.ledger_sequence, 100);
    }

    #[test]
    fn test_replay_metrics_accumulate() {
        let (env, cid) = make_test_env();
        let request_id = Bytes::from_slice(&env, &[0x01, 0x02, 0x03]);
        let actor = Address::generate(&env);

        env.as_contract(&cid, || { record_replay_detection(&env, &request_id, &actor); });
        env.as_contract(&cid, || { record_replay_detection(&env, &request_id, &actor); });

        let metrics = env.as_contract(&cid, || get_replay_metrics(&env));
        assert_eq!(metrics.total_replay_attempts, 2);
        assert_eq!(metrics.unique_replayed_ids, 1);
    }

    #[test]
    fn test_multiple_request_ids_tracked() {
        let (env, cid) = make_test_env();
        let req_id_1 = Bytes::from_slice(&env, &[0x01]);
        let req_id_2 = Bytes::from_slice(&env, &[0x02]);
        let actor = Address::generate(&env);

        env.as_contract(&cid, || { record_replay_detection(&env, &req_id_1, &actor); });
        env.as_contract(&cid, || { record_replay_detection(&env, &req_id_2, &actor); });

        let metrics = env.as_contract(&cid, || get_replay_metrics(&env));
        assert_eq!(metrics.total_replay_attempts, 2);
        assert_eq!(metrics.unique_replayed_ids, 2);
    }

    #[test]
    fn test_get_replay_count_for_id() {
        let (env, cid) = make_test_env();
        let request_id = Bytes::from_slice(&env, &[0x05]);
        let actor = Address::generate(&env);

        // Counter lives in temporary storage; starts at 0.
        assert_eq!(env.as_contract(&cid, || get_replay_count_for_id(&env, &request_id)), 0);

        env.as_contract(&cid, || { record_replay_detection(&env, &request_id, &actor); });
        assert_eq!(env.as_contract(&cid, || get_replay_count_for_id(&env, &request_id)), 1);

        env.as_contract(&cid, || { record_replay_detection(&env, &request_id, &actor); });
        assert_eq!(env.as_contract(&cid, || get_replay_count_for_id(&env, &request_id)), 2);
    }

    #[test]
    fn test_replay_event_retrieval() {
        let (env, cid) = make_test_env();
        let request_id = Bytes::from_slice(&env, &[0x10]);
        let actor = Address::generate(&env);

        env.as_contract(&cid, || { record_replay_detection(&env, &request_id, &actor); });

        // Event is stored in temporary storage.
        let event_opt = env.as_contract(&cid, || get_replay_event(&env, 0));
        assert!(event_opt.is_some());
        let event = event_opt.unwrap();
        assert_eq!(event.attempt_number, 1);
    }

    /// Two byte-distinct request_ids must map to separate attempt counters.
    /// This pins the byte-preservation guarantee of the make_storage_key-based
    /// attempt key: [0xAA] and [0xAA, 0x00] differ by a single trailing byte
    /// and must never share a counter.
    #[test]
    fn test_distinct_request_ids_have_separate_counters() {
        let (env, cid) = make_test_env();
        let actor = Address::generate(&env);
        let id_a = Bytes::from_slice(&env, &[0xAA]);
        let id_b = Bytes::from_slice(&env, &[0xAA, 0x00]);

        env.as_contract(&cid, || { record_replay_detection(&env, &id_a, &actor); });

        let count_a = env.as_contract(&cid, || get_replay_count_for_id(&env, &id_a));
        let count_b = env.as_contract(&cid, || get_replay_count_for_id(&env, &id_b));

        assert_eq!(count_a, 1, "id_a must have one attempt recorded");
        assert_eq!(count_b, 0, "id_b must be unaffected by id_a's recording");
    }

    /// Verify that per-ID counters do NOT accumulate in instance storage.
    /// After recording a replay, instance storage should only hold the
    /// aggregate metrics key, not per-ID attempt keys.
    #[test]
    fn test_per_id_counter_not_in_instance_storage() {
        let (env, cid) = make_test_env();
        let request_id = Bytes::from_slice(&env, &[0xAB, 0xCD]);
        let actor = Address::generate(&env);

        env.as_contract(&cid, || { record_replay_detection(&env, &request_id, &actor); });

        // The per-ID key must be absent from instance storage.
        let old_instance_key = (soroban_sdk::symbol_short!("REPLAYAT"), request_id.clone());
        let in_instance: bool = env.as_contract(&cid, || {
            env.storage().instance().has(&old_instance_key)
        });
        assert!(!in_instance, "per-ID counter must not be stored in instance storage");

        // But the counter must be readable via get_replay_count_for_id.
        let count = env.as_contract(&cid, || get_replay_count_for_id(&env, &request_id));
        assert_eq!(count, 1);
    }
}
