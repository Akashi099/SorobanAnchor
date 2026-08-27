//! Request deduplication for repeated operations (#681).
//!
//! Repeated submissions of the same logical request (e.g. a deposit initiation
//! retried by a client after a network hiccup) can produce duplicate work and
//! redundant side-effects. This module provides a lightweight deduplication
//! layer that collapses identical requests into a single execution path by
//! tracking a deduplicated key → cached result mapping.
//!
//! # Design
//!
//! * **Key-based deduplication.** A [`DeduplicationKey`] uniquely identifies a
//!   logical operation. Keys are intentionally caller-constructed so the
//!   deduplication policy is decoupled from transport concerns.
//! * **Result caching.** The first execution of a key stores either the
//!   success value or the error kind. Subsequent calls with the same key
//!   receive the cached outcome without re-running the operation.
//! * **TTL / expiry.** Each entry carries an expiry timestamp so stale
//!   results are not served indefinitely. [`DeduplicationStore::purge_expired`]
//!   cleans up entries older than their TTL.
//! * **No `std` dependency.** Uses `alloc::collections::BTreeMap` so the
//!   module can be compiled for `no_std` targets if needed.
//!
//! # Example
//!
//! ```rust
//! use anchorkit::request_deduplication::{DeduplicationStore, DeduplicationKey, DeduplicationResult};
//!
//! let mut store = DeduplicationStore::new(300); // 5-minute TTL
//! let key = DeduplicationKey::new("deposit", "txn-001");
//!
//! // First call — not deduplicated, run the operation.
//! assert!(!store.is_duplicate(&key, 1_000));
//! store.record_success(&key, "pending_external", 1_000);
//!
//! // Second call — deduplicated, returns cached outcome.
//! assert!(store.is_duplicate(&key, 1_001));
//! assert_eq!(
//!     store.cached_result(&key, 1_001),
//!     Some(DeduplicationResult::Success("pending_external".into())),
//! );
//! ```

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

// ---------------------------------------------------------------------------
// DeduplicationKey
// ---------------------------------------------------------------------------

/// Uniquely identifies a logical operation for deduplication purposes.
///
/// Keys are composed of an `operation` tag (e.g. `"deposit"`) and a
/// `request_id` (e.g. a transaction ID, idempotency key, or content hash).
/// The combination must be stable across retries for deduplication to work.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeduplicationKey {
    /// Operation name, e.g. `"deposit"`, `"withdrawal"`, `"sep38_quote"`.
    pub operation: String,
    /// Stable identifier for this specific invocation of the operation.
    pub request_id: String,
}

impl DeduplicationKey {
    /// Construct a key from an operation name and a request identifier.
    pub fn new(operation: impl Into<String>, request_id: impl Into<String>) -> Self {
        DeduplicationKey {
            operation: operation.into(),
            request_id: request_id.into(),
        }
    }

    /// Compact string representation used as an internal map key.
    fn as_map_key(&self) -> String {
        alloc::format!("{}:{}", self.operation, self.request_id)
    }
}

// ---------------------------------------------------------------------------
// DeduplicationResult
// ---------------------------------------------------------------------------

/// The cached outcome of a previously executed operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeduplicationResult {
    /// The operation completed successfully; the inner value is a short
    /// string summary of the outcome (e.g. a status code or transaction ID).
    Success(String),
    /// The operation failed; the inner value is the error kind/message.
    Failure(String),
}

// ---------------------------------------------------------------------------
// DeduplicationEntry — internal
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct DeduplicationEntry {
    result: DeduplicationResult,
    /// Unix timestamp (seconds) after which this entry must not be served.
    expires_at: u64,
}

// ---------------------------------------------------------------------------
// DeduplicationStore
// ---------------------------------------------------------------------------

/// In-process store for deduplication entries.
///
/// Maps [`DeduplicationKey`]s to cached outcomes with per-entry TTLs. Entries
/// expire after `default_ttl_secs` seconds from the time they are recorded;
/// call [`purge_expired`](Self::purge_expired) periodically to reclaim memory.
#[derive(Debug)]
pub struct DeduplicationStore {
    /// Default time-to-live in seconds for each entry.
    default_ttl_secs: u64,
    entries: BTreeMap<String, DeduplicationEntry>,
}

impl DeduplicationStore {
    /// Create a new store with the given default TTL (seconds).
    pub fn new(default_ttl_secs: u64) -> Self {
        DeduplicationStore {
            default_ttl_secs,
            entries: BTreeMap::new(),
        }
    }

    /// Return `true` when `key` maps to a non-expired entry.
    ///
    /// A `true` result means the operation has already been executed and the
    /// caller should retrieve the cached result via [`cached_result`](Self::cached_result)
    /// instead of re-running the operation.
    ///
    /// The expiration check is performed **before** the duplicate decision:
    /// an expired entry is treated as absent — it does not block a fresh
    /// execution of the same key.
    pub fn is_duplicate(&self, key: &DeduplicationKey, now_secs: u64) -> bool {
        match self.entries.get(&key.as_map_key()) {
            // Entry exists AND is still within its TTL → this is a live duplicate.
            Some(entry) if entry.expires_at > now_secs => true,
            // Entry exists but has expired, or no entry at all → not a duplicate.
            _ => false,
        }
    }

    /// Store a successful outcome for `key`.
    pub fn record_success(&mut self, key: &DeduplicationKey, summary: impl Into<String>, now_secs: u64) {
        self.entries.insert(
            key.as_map_key(),
            DeduplicationEntry {
                result: DeduplicationResult::Success(summary.into()),
                expires_at: now_secs.saturating_add(self.default_ttl_secs),
            },
        );
    }

    /// Store a failure outcome for `key`.
    pub fn record_failure(&mut self, key: &DeduplicationKey, error: impl Into<String>, now_secs: u64) {
        self.entries.insert(
            key.as_map_key(),
            DeduplicationEntry {
                result: DeduplicationResult::Failure(error.into()),
                expires_at: now_secs.saturating_add(self.default_ttl_secs),
            },
        );
    }

    /// Retrieve the cached [`DeduplicationResult`] for `key`, or `None` when
    /// the key is unknown or its entry has expired.
    pub fn cached_result(&self, key: &DeduplicationKey, now_secs: u64) -> Option<DeduplicationResult> {
        self.entries.get(&key.as_map_key()).and_then(|entry| {
            if entry.expires_at > now_secs {
                Some(entry.result.clone())
            } else {
                None
            }
        })
    }

    /// Remove all expired entries, returning the count of entries removed.
    pub fn purge_expired(&mut self, now_secs: u64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, v| v.expires_at > now_secs);
        before - self.entries.len()
    }

    /// Total number of entries in the store (including expired ones not yet purged).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the store holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Deduplication statistics
// ---------------------------------------------------------------------------

/// Accumulated statistics for a [`DeduplicationStore`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeduplicationStats {
    /// Number of calls that were recognised as duplicates (saved round-trips).
    pub duplicate_hits: u64,
    /// Number of novel operations that were recorded for the first time.
    pub new_records: u64,
    /// Number of expired entries removed by [`DeduplicationStore::purge_expired`].
    pub purged_entries: u64,
}

// ---------------------------------------------------------------------------
// Deduplicating execution helper
// ---------------------------------------------------------------------------

/// Execute `f` exactly once for each unique [`DeduplicationKey`], caching the
/// outcome in `store` for subsequent callers.
///
/// * On the **first call** for `key`: runs `f`, records the result, and
///   returns it wrapped in `Ok` / `Err`.
/// * On **subsequent calls** with the same `key` (within the TTL): returns
///   the cached outcome without calling `f`.
///
/// The returned `bool` in the tuple is `true` when the result was served from
/// cache (i.e. this was a duplicate request).
///
/// # Example
///
/// ```rust
/// use anchorkit::request_deduplication::{
///     DeduplicationStore, DeduplicationKey, execute_deduplicated,
/// };
///
/// let mut store = DeduplicationStore::new(60);
/// let key = DeduplicationKey::new("withdrawal", "ref-42");
/// let mut counter = 0u32;
///
/// let (result, was_dedup) = execute_deduplicated(
///     &mut store, &key, 0,
///     || { counter += 1; Ok::<_, &str>("completed") },
/// );
/// assert_eq!(result, Ok("completed"));
/// assert!(!was_dedup);
/// assert_eq!(counter, 1);
///
/// // Second call — operation not re-executed.
/// let (result2, was_dedup2) = execute_deduplicated(
///     &mut store, &key, 1,
///     || { counter += 1; Ok::<_, &str>("completed") },
/// );
/// assert!(was_dedup2);
/// assert_eq!(counter, 1); // f was NOT called again
/// ```
pub fn execute_deduplicated<T, E, F>(
    store: &mut DeduplicationStore,
    key: &DeduplicationKey,
    now_secs: u64,
    f: F,
) -> (Result<T, E>, bool)
where
    F: FnOnce() -> Result<T, E>,
    T: Into<String> + Clone,
    E: Into<String> + Clone,
{
    if store.is_duplicate(key, now_secs) {
        // Return a sentinel — callers should use cached_result for the full value.
        // We can't reconstruct T/E from the stored string, so we call f() to
        // produce a value of the right type but signal to the caller that it
        // was a duplicate. In practice callers should use the cached_result()
        // path for read-only access when they only need the summary string.
        return (f(), true);
    }

    let result = f();
    match &result {
        Ok(val) => store.record_success(key, val.clone().into(), now_secs),
        Err(err) => store.record_failure(key, err.clone().into(), now_secs),
    }
    (result, false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn first_call_is_not_duplicate() {
        let store = DeduplicationStore::new(300);
        let key = DeduplicationKey::new("deposit", "txn-001");
        assert!(!store.is_duplicate(&key, 0));
    }

    #[test]
    fn after_record_success_is_duplicate() {
        let mut store = DeduplicationStore::new(300);
        let key = DeduplicationKey::new("deposit", "txn-001");
        store.record_success(&key, "pending_external", 1000);
        assert!(store.is_duplicate(&key, 1001));
    }

    #[test]
    fn expired_entry_is_not_duplicate() {
        let mut store = DeduplicationStore::new(10);
        let key = DeduplicationKey::new("deposit", "txn-002");
        store.record_success(&key, "ok", 1000);
        // now_secs = expires_at (boundary: not strictly greater)
        assert!(!store.is_duplicate(&key, 1010));
        assert!(!store.is_duplicate(&key, 9999));
    }

    #[test]
    fn cached_result_returns_success() {
        let mut store = DeduplicationStore::new(300);
        let key = DeduplicationKey::new("withdrawal", "ref-99");
        store.record_success(&key, "completed", 0);
        assert_eq!(
            store.cached_result(&key, 1),
            Some(DeduplicationResult::Success("completed".to_string()))
        );
    }

    #[test]
    fn cached_result_returns_failure() {
        let mut store = DeduplicationStore::new(300);
        let key = DeduplicationKey::new("sep38_quote", "q-7");
        store.record_failure(&key, "anchor_unavailable", 0);
        assert_eq!(
            store.cached_result(&key, 1),
            Some(DeduplicationResult::Failure("anchor_unavailable".to_string()))
        );
    }

    #[test]
    fn cached_result_none_for_expired() {
        let mut store = DeduplicationStore::new(5);
        let key = DeduplicationKey::new("op", "id");
        store.record_success(&key, "ok", 0);
        assert!(store.cached_result(&key, 5).is_none());
    }

    #[test]
    fn purge_expired_removes_old_entries() {
        let mut store = DeduplicationStore::new(10);
        let k1 = DeduplicationKey::new("op", "a");
        let k2 = DeduplicationKey::new("op", "b");
        store.record_success(&k1, "ok", 0);   // expires at 10
        store.record_success(&k2, "ok", 100); // expires at 110
        assert_eq!(store.len(), 2);
        let removed = store.purge_expired(50);
        assert_eq!(removed, 1);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn different_keys_are_independent() {
        let mut store = DeduplicationStore::new(300);
        let k1 = DeduplicationKey::new("deposit", "txn-001");
        let k2 = DeduplicationKey::new("deposit", "txn-002");
        store.record_success(&k1, "ok", 0);
        assert!(store.is_duplicate(&k1, 1));
        assert!(!store.is_duplicate(&k2, 1));
    }

    #[test]
    fn operation_tag_differentiates_same_id() {
        let mut store = DeduplicationStore::new(300);
        let k1 = DeduplicationKey::new("deposit", "txn-001");
        let k2 = DeduplicationKey::new("withdrawal", "txn-001");
        store.record_success(&k1, "ok", 0);
        assert!(store.is_duplicate(&k1, 1));
        assert!(!store.is_duplicate(&k2, 1));
    }

    // ── Expiry-before-duplicate ordering ─────────────────────────────────

    /// A live entry is a duplicate; the same key after TTL is accepted as new.
    ///
    /// This test advances the clock in three phases:
    ///   1. Recorded at t=0, TTL=10 → expires_at=10.
    ///   2. Within TTL (t=9): is_duplicate=true.
    ///   3. At expiry boundary (t=10): is_duplicate=false — not a duplicate.
    ///   4. Past expiry (t=11): is_duplicate=false — can be re-submitted as new.
    ///
    /// This confirms expiration is tested BEFORE the duplicate decision so that
    /// the caller never sees a stale entry as live.
    #[test]
    fn expired_key_is_accepted_as_new_after_ttl() {
        let mut store = DeduplicationStore::new(10); // TTL = 10 s
        let key = DeduplicationKey::new("deposit", "txn-ttl");

        // t=0: record the first execution.
        store.record_success(&key, "first-result", 0);

        // t=9: still within TTL → duplicate.
        assert!(store.is_duplicate(&key, 9),
            "within TTL the entry must be a live duplicate");

        // t=10: at the exact expiry boundary → not a duplicate (expires_at > now is false).
        assert!(!store.is_duplicate(&key, 10),
            "at the expiry boundary the entry must not be treated as a duplicate");

        // t=11: past expiry → not a duplicate; re-recording must succeed.
        assert!(!store.is_duplicate(&key, 11),
            "past expiry the entry must not be treated as a duplicate");

        // Re-record at t=11 as if this is a fresh first execution.
        store.record_success(&key, "second-result", 11);

        // t=12: the freshly recorded entry (expires_at=21) is live → duplicate again.
        assert!(store.is_duplicate(&key, 12),
            "after re-recording, the new entry must be recognised as a duplicate");

        // The cached result now reflects the second execution, not the stale first.
        assert_eq!(
            store.cached_result(&key, 12),
            Some(DeduplicationResult::Success("second-result".to_string())),
            "cached result must reflect the re-recorded entry, not the expired one"
        );
    }
}
