//! Request record management: retention policies (#679) and export/archival (#680).
//!
//! This module provides:
//! - [`RequestRetentionPolicy`]: configurable rules for how long request records
//!   are kept and the maximum number to retain.
//! - [`RequestRecord`]: a structured record of a request/transaction event
//!   suitable for off-system archival.
//! - [`RequestRecordStore`]: an in-memory store (host-side) that enforces the
//!   retention policy and supports paginated batch export.
//!
//! ## Retention policy (#679)
//!
//! Operators configure a [`RequestRetentionPolicy`] with:
//! - `max_records`: hard cap on the number of records retained (0 = unlimited).
//! - `max_age_seconds`: maximum age in seconds; older records are pruned (0 = no age limit).
//! - `prune_on_write`: when `true`, the policy is enforced on every [`RequestRecordStore::push`].
//!
//! ## Export and archival (#680)
//!
//! [`RequestRecordStore::export_batch`] returns a window of records by cursor
//! position. Callers iterate through all records by advancing the cursor until
//! `ExportBatch::has_more` is `false`, then write the batches to off-system
//! storage (S3, a database, etc.).
//!
//! [`RequestRecordStore::archive_before`] moves records older than a given
//! timestamp out of the active store into a separate archive `Vec` so they are
//! no longer included in normal queries but remain available for inspection or
//! final export before deletion.

extern crate alloc;

use alloc::{string::String, vec::Vec};

// ---------------------------------------------------------------------------
// RequestRetentionPolicy (#679)
// ---------------------------------------------------------------------------

/// Configurable retention policy for request records.
///
/// # Fields
///
/// - `max_records`: maximum number of records to keep.  `0` means no limit.
/// - `max_age_seconds`: discard records older than this many seconds.  `0` means no limit.
/// - `prune_on_write`: when `true`, [`RequestRecordStore::push`] enforces the
///   policy immediately after inserting the new record.
///
/// # Examples
///
/// ```rust
/// use anchorkit::request_record::RequestRetentionPolicy;
///
/// // Keep at most 1 000 records; no age-based pruning; enforce on every write.
/// let policy = RequestRetentionPolicy::new(1_000, 0, true);
/// assert_eq!(policy.max_records, 1_000);
/// assert!(!policy.has_age_limit());
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct RequestRetentionPolicy {
    /// Maximum number of records to keep (0 = unlimited).
    pub max_records: usize,
    /// Maximum age in seconds for a record (0 = no age limit).
    pub max_age_seconds: u64,
    /// Enforce policy automatically on each `push` call.
    pub prune_on_write: bool,
}

impl RequestRetentionPolicy {
    /// Create a new retention policy.
    pub fn new(max_records: usize, max_age_seconds: u64, prune_on_write: bool) -> Self {
        Self { max_records, max_age_seconds, prune_on_write }
    }

    /// Returns `true` when an age-based limit is configured.
    pub fn has_age_limit(&self) -> bool {
        self.max_age_seconds > 0
    }

    /// Returns `true` when a record-count limit is configured.
    pub fn has_record_limit(&self) -> bool {
        self.max_records > 0
    }
}

impl Default for RequestRetentionPolicy {
    fn default() -> Self {
        // Sensible default: keep 10 000 records, no age pruning, enforce on write.
        Self::new(10_000, 0, true)
    }
}

// ---------------------------------------------------------------------------
// RequestRecord
// ---------------------------------------------------------------------------

/// A single request record eligible for retention management and archival.
///
/// Records are produced by higher-level callers (e.g. the host boundary replay
/// prevention layer or the transaction state tracker) and pushed into a
/// [`RequestRecordStore`].
#[derive(Clone, Debug, PartialEq)]
pub struct RequestRecord {
    /// Unique identifier for this record (e.g. the transaction ID).
    pub id: u64,
    /// Unix timestamp (seconds) when the request was received.
    pub timestamp: u64,
    /// Human-readable label for the operation (e.g. `"attest"`, `"quote"`).
    pub operation: String,
    /// The Stellar address that submitted the request.
    pub actor: String,
    /// Outcome tag: `"accepted"`, `"rejected"`, `"replayed"`, etc.
    pub outcome: String,
    /// Optional free-form detail (error message, routing reason, …).
    pub detail: Option<String>,
}

impl RequestRecord {
    /// Construct a new record.
    pub fn new(
        id: u64,
        timestamp: u64,
        operation: impl Into<String>,
        actor: impl Into<String>,
        outcome: impl Into<String>,
        detail: Option<impl Into<String>>,
    ) -> Self {
        Self {
            id,
            timestamp,
            operation: operation.into(),
            actor: actor.into(),
            outcome: outcome.into(),
            detail: detail.map(Into::into),
        }
    }
}

// ---------------------------------------------------------------------------
// ExportBatch (#680)
// ---------------------------------------------------------------------------

/// A paginated batch of [`RequestRecord`]s returned by
/// [`RequestRecordStore::export_batch`].
#[derive(Clone, Debug)]
pub struct ExportBatch {
    /// The records in this batch.
    pub records: Vec<RequestRecord>,
    /// Cursor value to pass as `start_cursor` in the next call.
    /// Meaningless when `has_more` is `false`.
    pub next_cursor: usize,
    /// `true` when there are more records beyond this batch.
    pub has_more: bool,
}

// ---------------------------------------------------------------------------
// RequestRecordStore
// ---------------------------------------------------------------------------

/// Host-side store for request records with retention and export support.
///
/// Combines:
/// - An active record buffer (`records`) subject to the [`RequestRetentionPolicy`].
/// - An archive buffer (`archive`) holding records moved out by
///   [`archive_before`](Self::archive_before).
///
/// # Examples
///
/// ```rust
/// use anchorkit::request_record::{RequestRecord, RequestRecordStore, RequestRetentionPolicy};
///
/// let policy = RequestRetentionPolicy::new(5, 0, true);
/// let mut store = RequestRecordStore::new(policy);
///
/// for i in 0..10_u64 {
///     store.push(RequestRecord::new(i, 1_000 + i, "attest", "GXXX", "accepted", None::<&str>));
/// }
/// // Only 5 most-recent records are kept.
/// assert_eq!(store.len(), 5);
/// ```
#[derive(Debug, Default)]
pub struct RequestRecordStore {
    /// Active records, ordered by insertion (oldest first).
    records: Vec<RequestRecord>,
    /// Archived records (moved out by `archive_before`).
    archive: Vec<RequestRecord>,
    /// The active retention policy.
    pub policy: RequestRetentionPolicy,
}

impl RequestRecordStore {
    /// Create a new store with the given retention policy.
    pub fn new(policy: RequestRetentionPolicy) -> Self {
        Self { records: Vec::new(), archive: Vec::new(), policy }
    }

    /// Push a new record into the store.
    ///
    /// If `policy.prune_on_write` is `true`, the retention policy is enforced
    /// immediately after insertion.
    pub fn push(&mut self, record: RequestRecord) {
        self.records.push(record);
        if self.policy.prune_on_write {
            self.enforce_policy(0);
        }
    }

    /// Enforce the retention policy against the active record buffer.
    ///
    /// `now_secs` is the current Unix timestamp used for age-based pruning.
    /// Pass `0` to skip age-based pruning regardless of the policy setting.
    ///
    /// Returns the number of records removed.
    pub fn enforce_policy(&mut self, now_secs: u64) -> usize {
        let before = self.records.len();

        // Age-based pruning (oldest first).
        if self.policy.has_age_limit() && now_secs > 0 {
            let cutoff = now_secs.saturating_sub(self.policy.max_age_seconds);
            self.records.retain(|r| r.timestamp >= cutoff);
        }

        // Record-count cap: drop oldest records when over limit.
        if self.policy.has_record_limit() {
            let limit = self.policy.max_records;
            while self.records.len() > limit {
                self.records.remove(0);
            }
        }

        before.saturating_sub(self.records.len())
    }

    /// Update the retention policy and immediately apply it.
    ///
    /// `now_secs` is used for age-based pruning (pass `0` to skip).
    pub fn set_policy(&mut self, policy: RequestRetentionPolicy, now_secs: u64) {
        self.policy = policy;
        self.enforce_policy(now_secs);
    }

    /// Return the number of active (non-archived) records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Return `true` when the active record buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Return the number of archived records.
    pub fn archive_len(&self) -> usize {
        self.archive.len()
    }

    // ── Export / archival (#680) ─────────────────────────────────────────────

    /// Export a paginated batch of active records starting at `start_cursor`.
    ///
    /// `batch_size` is capped at 100 to prevent unbounded allocations.
    ///
    /// # Arguments
    ///
    /// * `start_cursor` – Zero-based index into the active record buffer.
    /// * `batch_size`   – Maximum number of records per batch (capped at 100).
    ///
    /// # Returns
    ///
    /// An [`ExportBatch`] with the records and a cursor for the next call.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::request_record::{RequestRecord, RequestRecordStore, RequestRetentionPolicy};
    ///
    /// let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
    /// for i in 0..25_u64 {
    ///     store.push(RequestRecord::new(i, 1_000 + i, "attest", "GXXX", "accepted", None::<&str>));
    /// }
    ///
    /// let batch = store.export_batch(0, 10);
    /// assert_eq!(batch.records.len(), 10);
    /// assert!(batch.has_more);
    ///
    /// let last = store.export_batch(batch.next_cursor, 10);
    /// assert_eq!(last.records.len(), 10);
    /// assert!(last.has_more);
    ///
    /// let tail = store.export_batch(last.next_cursor, 10);
    /// assert_eq!(tail.records.len(), 5);
    /// assert!(!tail.has_more);
    /// ```
    pub fn export_batch(&self, start_cursor: usize, batch_size: usize) -> ExportBatch {
        const MAX_BATCH: usize = 100;
        let effective_size = batch_size.min(MAX_BATCH);

        if start_cursor >= self.records.len() {
            return ExportBatch {
                records: Vec::new(),
                next_cursor: start_cursor,
                has_more: false,
            };
        }

        let end = (start_cursor + effective_size).min(self.records.len());
        let records = self.records[start_cursor..end].to_vec();
        let next_cursor = end;
        let has_more = next_cursor < self.records.len();

        ExportBatch { records, next_cursor, has_more }
    }

    /// Export all active records in a single allocation (no pagination).
    ///
    /// Suitable for small stores where a single snapshot is acceptable.
    pub fn export_all(&self) -> Vec<RequestRecord> {
        self.records.clone()
    }

    /// Move all active records whose `timestamp < cutoff_secs` into the
    /// archive buffer.
    ///
    /// Archived records are no longer included in [`len`](Self::len),
    /// [`export_batch`](Self::export_batch), or [`export_all`](Self::export_all).
    /// They remain accessible via [`archive_export`](Self::archive_export).
    ///
    /// Returns the number of records archived.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::request_record::{RequestRecord, RequestRecordStore, RequestRetentionPolicy};
    ///
    /// let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
    /// store.push(RequestRecord::new(1, 1_000, "attest", "GXXX", "accepted", None::<&str>));
    /// store.push(RequestRecord::new(2, 2_000, "attest", "GXXX", "accepted", None::<&str>));
    /// store.push(RequestRecord::new(3, 3_000, "attest", "GXXX", "accepted", None::<&str>));
    ///
    /// let archived = store.archive_before(2_500);
    /// assert_eq!(archived, 2);
    /// assert_eq!(store.len(), 1);
    /// assert_eq!(store.archive_len(), 2);
    /// ```
    pub fn archive_before(&mut self, cutoff_secs: u64) -> usize {
        let mut to_archive: Vec<RequestRecord> = Vec::new();
        let mut remaining: Vec<RequestRecord> = Vec::new();

        for record in self.records.drain(..) {
            if record.timestamp < cutoff_secs {
                to_archive.push(record);
            } else {
                remaining.push(record);
            }
        }

        let count = to_archive.len();
        self.archive.extend(to_archive);
        self.records = remaining;
        count
    }

    /// Export a paginated batch from the archive buffer.
    ///
    /// Same pagination contract as [`export_batch`](Self::export_batch).
    pub fn archive_export(&self, start_cursor: usize, batch_size: usize) -> ExportBatch {
        const MAX_BATCH: usize = 100;
        let effective_size = batch_size.min(MAX_BATCH);

        if start_cursor >= self.archive.len() {
            return ExportBatch {
                records: Vec::new(),
                next_cursor: start_cursor,
                has_more: false,
            };
        }

        let end = (start_cursor + effective_size).min(self.archive.len());
        let records = self.archive[start_cursor..end].to_vec();
        let next_cursor = end;
        let has_more = next_cursor < self.archive.len();

        ExportBatch { records, next_cursor, has_more }
    }

    /// Drain and discard all archived records.
    ///
    /// Call this after a successful off-system write to reclaim memory.
    /// Returns the number of records discarded.
    pub fn clear_archive(&mut self) -> usize {
        let count = self.archive.len();
        self.archive.clear();
        count
    }

    /// Look up a single active record by its `id` field.
    pub fn get_by_id(&self, id: u64) -> Option<&RequestRecord> {
        self.records.iter().find(|r| r.id == id)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(id: u64, ts: u64) -> RequestRecord {
        RequestRecord::new(id, ts, "attest", "GXXX", "accepted", None::<&str>)
    }

    // ── RetentionPolicy ─────────────────────────────────────────────────────

    #[test]
    fn test_policy_default_has_record_limit() {
        let p = RequestRetentionPolicy::default();
        assert!(p.has_record_limit());
        assert!(!p.has_age_limit());
        assert!(p.prune_on_write);
    }

    #[test]
    fn test_policy_unlimited_no_limits() {
        let p = RequestRetentionPolicy::new(0, 0, false);
        assert!(!p.has_record_limit());
        assert!(!p.has_age_limit());
    }

    // ── push + enforce_policy (record cap) ─────────────────────────────────

    #[test]
    fn test_push_enforces_record_cap() {
        let policy = RequestRetentionPolicy::new(3, 0, true);
        let mut store = RequestRecordStore::new(policy);
        for i in 0..5_u64 {
            store.push(make_record(i, 1_000 + i));
        }
        // Only the 3 most-recent records should remain.
        assert_eq!(store.len(), 3);
        assert_eq!(store.records[0].id, 2);
        assert_eq!(store.records[2].id, 4);
    }

    #[test]
    fn test_push_no_cap_unlimited() {
        let policy = RequestRetentionPolicy::new(0, 0, true);
        let mut store = RequestRecordStore::new(policy);
        for i in 0..50_u64 {
            store.push(make_record(i, i));
        }
        assert_eq!(store.len(), 50);
    }

    // ── enforce_policy (age-based) ───────────────────────────────────────────

    #[test]
    fn test_enforce_policy_age_prunes_old_records() {
        let policy = RequestRetentionPolicy::new(0, 100, false);
        let mut store = RequestRecordStore::new(policy);
        store.records.push(make_record(1, 900));  // old
        store.records.push(make_record(2, 950));  // old
        store.records.push(make_record(3, 1_050)); // recent

        let removed = store.enforce_policy(1_050);
        assert_eq!(removed, 2);
        assert_eq!(store.len(), 1);
        assert_eq!(store.records[0].id, 3);
    }

    #[test]
    fn test_enforce_policy_age_zero_skips_pruning() {
        let policy = RequestRetentionPolicy::new(0, 100, false);
        let mut store = RequestRecordStore::new(policy);
        store.records.push(make_record(1, 100));
        // now_secs = 0 → skip age pruning
        let removed = store.enforce_policy(0);
        assert_eq!(removed, 0);
        assert_eq!(store.len(), 1);
    }

    // ── set_policy ───────────────────────────────────────────────────────────

    #[test]
    fn test_set_policy_applies_immediately() {
        let policy = RequestRetentionPolicy::new(0, 0, false);
        let mut store = RequestRecordStore::new(policy);
        for i in 0..20_u64 {
            store.records.push(make_record(i, i));
        }
        assert_eq!(store.len(), 20);

        store.set_policy(RequestRetentionPolicy::new(5, 0, true), 0);
        assert_eq!(store.len(), 5);
    }

    // ── export_batch ─────────────────────────────────────────────────────────

    #[test]
    fn test_export_batch_first_page() {
        let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
        for i in 0..25_u64 {
            store.records.push(make_record(i, i));
        }

        let batch = store.export_batch(0, 10);
        assert_eq!(batch.records.len(), 10);
        assert_eq!(batch.next_cursor, 10);
        assert!(batch.has_more);
    }

    #[test]
    fn test_export_batch_last_page() {
        let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
        for i in 0..25_u64 {
            store.records.push(make_record(i, i));
        }

        let batch = store.export_batch(20, 10);
        assert_eq!(batch.records.len(), 5);
        assert!(!batch.has_more);
    }

    #[test]
    fn test_export_batch_beyond_end_returns_empty() {
        let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
        store.records.push(make_record(1, 1));

        let batch = store.export_batch(100, 10);
        assert!(batch.records.is_empty());
        assert!(!batch.has_more);
    }

    #[test]
    fn test_export_batch_capped_at_100() {
        let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
        for i in 0..200_u64 {
            store.records.push(make_record(i, i));
        }

        let batch = store.export_batch(0, 200);
        assert_eq!(batch.records.len(), 100);
    }

    #[test]
    fn test_export_all_returns_full_copy() {
        let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
        for i in 0..10_u64 {
            store.records.push(make_record(i, i));
        }
        let all = store.export_all();
        assert_eq!(all.len(), 10);
    }

    // ── archive_before ───────────────────────────────────────────────────────

    #[test]
    fn test_archive_before_moves_old_records() {
        let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
        store.records.push(make_record(1, 1_000));
        store.records.push(make_record(2, 2_000));
        store.records.push(make_record(3, 3_000));

        let archived = store.archive_before(2_500);
        assert_eq!(archived, 2);
        assert_eq!(store.len(), 1);
        assert_eq!(store.archive_len(), 2);
    }

    #[test]
    fn test_archive_before_nothing_to_archive() {
        let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
        store.records.push(make_record(1, 5_000));

        let archived = store.archive_before(1_000);
        assert_eq!(archived, 0);
        assert_eq!(store.len(), 1);
        assert_eq!(store.archive_len(), 0);
    }

    #[test]
    fn test_archive_export_paginates() {
        let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
        for i in 0..15_u64 {
            store.archive.push(make_record(i, i));
        }

        let batch = store.archive_export(0, 10);
        assert_eq!(batch.records.len(), 10);
        assert!(batch.has_more);

        let tail = store.archive_export(batch.next_cursor, 10);
        assert_eq!(tail.records.len(), 5);
        assert!(!tail.has_more);
    }

    #[test]
    fn test_clear_archive() {
        let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
        store.archive.push(make_record(1, 1));
        store.archive.push(make_record(2, 2));

        let cleared = store.clear_archive();
        assert_eq!(cleared, 2);
        assert_eq!(store.archive_len(), 0);
    }

    // ── get_by_id ────────────────────────────────────────────────────────────

    #[test]
    fn test_get_by_id_found() {
        let mut store = RequestRecordStore::new(RequestRetentionPolicy::default());
        store.records.push(make_record(42, 1_000));
        let rec = store.get_by_id(42).unwrap();
        assert_eq!(rec.id, 42);
    }

    #[test]
    fn test_get_by_id_not_found() {
        let store = RequestRecordStore::new(RequestRetentionPolicy::default());
        assert!(store.get_by_id(99).is_none());
    }
}
