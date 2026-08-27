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
    let attempt_key = (soroban_sdk::symbol_short!("RPLYAT"), request_id.clone());
    let mut attempt_count: u32 = env
        .storage()
        .temporary()
        .get::<_, u32>(&attempt_key)
        .unwrap_or(0);
    attempt_count += 1;

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
    let attempt_key = (soroban_sdk::symbol_short!("RPLYAT"), request_id.clone());
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

// ---------------------------------------------------------------------------
// Timestamp validation & clock-skew bounds
// ---------------------------------------------------------------------------

/// Default maximum age (in seconds) for a valid replay-protected timestamp (5 minutes).
pub const DEFAULT_MAX_AGE_SECS: u64 = 300;

/// Default maximum allowed forward clock skew (in seconds) for future timestamps (1 minute).
pub const DEFAULT_CLOCK_SKEW_SECS: u64 = 60;

/// Check whether `timestamp` is within the permitted age and clock-skew bounds relative to `now`.
///
/// The comparison is direction-safe to prevent unsigned integer underflow:
/// - A timestamp of `0` is always rejected.
/// - When `timestamp <= now` (past / stale): age is `now.saturating_sub(timestamp)`.
///   Rejected if `age > max_age_secs`.
/// - When `timestamp > now` (future / forward clock skew): drift is `timestamp.saturating_sub(now)`.
///   Rejected if `drift > max_future_skew_secs`.
/// - Values at the exact boundaries (`now - max_age_secs` and `now + max_future_skew_secs`) are accepted.
///
/// # Arguments
///
/// * `timestamp` - The submission or attestation timestamp to validate
/// * `now` - The reference (current ledger) timestamp
/// * `max_age_secs` - Maximum allowed age in the past (seconds)
/// * `max_future_skew_secs` - Maximum allowed forward clock skew (seconds)
///
/// # Returns
///
/// `true` if `timestamp` is non-zero and within permitted bounds, `false` otherwise.
pub fn is_timestamp_valid(
    timestamp: u64,
    now: u64,
    max_age_secs: u64,
    max_future_skew_secs: u64,
) -> bool {
    if timestamp == 0 {
        return false;
    }

    if timestamp <= now {
        let age = now.saturating_sub(timestamp);
        age <= max_age_secs
    } else {
        let future_drift = timestamp.saturating_sub(now);
        future_drift <= max_future_skew_secs
    }
}

/// Check whether `timestamp` is within the default replay age (`DEFAULT_MAX_AGE_SECS` = 300 s)
/// and clock-skew (`DEFAULT_CLOCK_SKEW_SECS` = 60 s) bounds relative to `now`.
pub fn is_default_timestamp_valid(timestamp: u64, now: u64) -> bool {
    is_timestamp_valid(timestamp, now, DEFAULT_MAX_AGE_SECS, DEFAULT_CLOCK_SKEW_SECS)
}

/// Calculate the age or future skew of `timestamp` relative to `now`, returning `None` if
/// the timestamp is zero or exceeds the configured bounds.
///
/// Returns:
/// - `Some(Ok(age))` if `timestamp <= now` and within `max_age_secs`
/// - `Some(Err(future_drift))` if `timestamp > now` and within `max_future_skew_secs`
/// - `None` if `timestamp == 0` or out of bounds (stale or excessively future)
pub fn calculate_timestamp_age(
    timestamp: u64,
    now: u64,
    max_age_secs: u64,
    max_future_skew_secs: u64,
) -> Option<Result<u64, u64>> {
    if timestamp == 0 {
        return None;
    }

    if timestamp <= now {
        let age = now.saturating_sub(timestamp);
        if age <= max_age_secs {
            Some(Ok(age))
        } else {
            None
        }
    } else {
        let future_drift = timestamp.saturating_sub(now);
        if future_drift <= max_future_skew_secs {
            Some(Err(future_drift))
        } else {
            None
        }
    }
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

    // ── Timestamp and Clock-Skew Boundary Tests (#780) ───────────────────────

    #[test]
    fn test_timestamp_within_bounds_accepted() {
        let now = 1_000_000u64;
        let max_age = 300u64;
        let future_skew = 60u64;

        // Current time
        assert!(is_timestamp_valid(now, now, max_age, future_skew));
        assert_eq!(calculate_timestamp_age(now, now, max_age, future_skew), Some(Ok(0)));

        // Within past age (100 s old)
        assert!(is_timestamp_valid(now - 100, now, max_age, future_skew));
        assert_eq!(calculate_timestamp_age(now - 100, now, max_age, future_skew), Some(Ok(100)));

        // Within future skew (30 s future)
        assert!(is_timestamp_valid(now + 30, now, max_age, future_skew));
        assert_eq!(calculate_timestamp_age(now + 30, now, max_age, future_skew), Some(Err(30)));
    }

    #[test]
    fn test_timestamp_at_exact_boundaries_accepted() {
        let now = 1_000_000u64;
        let max_age = 300u64;
        let future_skew = 60u64;

        // Exact past boundary (age == max_age)
        let stale_boundary = now - max_age;
        assert!(is_timestamp_valid(stale_boundary, now, max_age, future_skew));
        assert_eq!(calculate_timestamp_age(stale_boundary, now, max_age, future_skew), Some(Ok(max_age)));

        // Exact future boundary (drift == future_skew)
        let future_boundary = now + future_skew;
        assert!(is_timestamp_valid(future_boundary, now, max_age, future_skew));
        assert_eq!(calculate_timestamp_age(future_boundary, now, max_age, future_skew), Some(Err(future_skew)));
    }

    #[test]
    fn test_stale_and_future_beyond_boundaries_rejected() {
        let now = 1_000_000u64;
        let max_age = 300u64;
        let future_skew = 60u64;

        // Just beyond past boundary (301 s old)
        let too_stale = now - max_age - 1;
        assert!(!is_timestamp_valid(too_stale, now, max_age, future_skew));
        assert_eq!(calculate_timestamp_age(too_stale, now, max_age, future_skew), None);

        // Just beyond future boundary (61 s future)
        let too_future = now + future_skew + 1;
        assert!(!is_timestamp_valid(too_future, now, max_age, future_skew));
        assert_eq!(calculate_timestamp_age(too_future, now, max_age, future_skew), None);

        // Zero timestamp is always invalid
        assert!(!is_timestamp_valid(0, now, max_age, future_skew));
        assert_eq!(calculate_timestamp_age(0, now, max_age, future_skew), None);
    }

    #[test]
    fn test_direction_safe_comparison_no_underflow() {
        let max_age = 300u64;
        let future_skew = 60u64;

        // When now is small (e.g. now = 50 < max_age)
        let now_small = 50u64;
        assert!(is_timestamp_valid(20, now_small, max_age, future_skew));
        assert!(is_timestamp_valid(50, now_small, max_age, future_skew));
        assert!(is_timestamp_valid(100, now_small, max_age, future_skew));
        assert!(!is_timestamp_valid(120, now_small, max_age, future_skew)); // 120 - 50 = 70 > 60

        // When timestamp is near u64::MAX
        let now = 1_000_000u64;
        assert!(!is_timestamp_valid(u64::MAX, now, max_age, future_skew));
        assert_eq!(calculate_timestamp_age(u64::MAX, now, max_age, future_skew), None);

        // Default bounds helper
        assert!(is_default_timestamp_valid(1_000_000, 1_000_000));
        assert!(is_default_timestamp_valid(1_000_000 - 300, 1_000_000));
        assert!(is_default_timestamp_valid(1_000_000 + 60, 1_000_000));
        assert!(!is_default_timestamp_valid(1_000_000 - 301, 1_000_000));
        assert!(!is_default_timestamp_valid(1_000_000 + 61, 1_000_000));
    }
}
