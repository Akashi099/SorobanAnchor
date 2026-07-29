//! Host-boundary replay prevention (#678).
//!
//! The on-chain [`replay_detection`](crate::replay_detection) module catches
//! duplicate request IDs inside Soroban contract execution.  This module
//! provides a complementary **host-side** guard that rejects duplicate
//! transaction submissions *before* any business logic runs, at the boundary
//! where the host application receives a request.
//!
//! ## Design
//!
//! [`HostReplayGuard`] maintains a set of recently-seen request IDs.  Each ID
//! is stored alongside the Unix timestamp at which it was first seen.  Before
//! processing a new request, callers call [`HostReplayGuard::check`]:
//!
//! - **First time**: the ID is recorded and [`CheckResult::Accepted`] is returned.
//! - **Duplicate**: [`CheckResult::Replay`] is returned with the original
//!   timestamp, and the request should be rejected before any further processing.
//!
//! Expired entries (older than the configured `window_secs`) are pruned lazily
//! on every [`check`](HostReplayGuard::check) call so memory stays bounded even
//! under high traffic.
//!
//! ## Usage
//!
//! ```rust
//! use anchorkit::host_replay_prevention::{HostReplayGuard, CheckResult};
//!
//! let mut guard = HostReplayGuard::new(300); // 5-minute dedup window
//!
//! match guard.check("req-001", "GXXX", 1_000_000) {
//!     CheckResult::Accepted => { /* proceed with processing */ }
//!     CheckResult::Replay { first_seen_at } => {
//!         eprintln!("duplicate submission detected (first seen at {})", first_seen_at);
//!     }
//! }
//! ```

extern crate alloc;
use alloc::{string::String, vec::Vec};

// ---------------------------------------------------------------------------
// CheckResult
// ---------------------------------------------------------------------------

/// Outcome of a [`HostReplayGuard::check`] call.
#[derive(Clone, Debug, PartialEq)]
pub enum CheckResult {
    /// The request ID has not been seen before within the dedup window; safe to proceed.
    Accepted,
    /// The request ID was already seen within the window — this is a duplicate.
    Replay {
        /// Unix timestamp (seconds) when the request ID was first received.
        first_seen_at: u64,
    },
}

impl CheckResult {
    /// Return `true` if the result is [`Accepted`](CheckResult::Accepted).
    pub fn is_accepted(&self) -> bool {
        matches!(self, CheckResult::Accepted)
    }

    /// Return `true` if the result is a replay.
    pub fn is_replay(&self) -> bool {
        matches!(self, CheckResult::Replay { .. })
    }
}

// ---------------------------------------------------------------------------
// Internal entry
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct SeenEntry {
    request_id: String,
    actor: String,
    first_seen_at: u64,
}

// ---------------------------------------------------------------------------
// HostReplayGuard (#678)
// ---------------------------------------------------------------------------

/// Host-boundary deduplication guard for transaction request IDs.
///
/// Maintains a sliding window of recently-seen request IDs.  Any submission
/// whose ID was already recorded within `window_secs` is flagged as a replay
/// and should be rejected before business logic runs.
///
/// # Thread safety
///
/// [`HostReplayGuard`] is not `Sync`; for multi-threaded use wrap it in a
/// `Mutex`.
///
/// # Examples
///
/// ```rust
/// use anchorkit::host_replay_prevention::{HostReplayGuard, CheckResult};
///
/// let mut guard = HostReplayGuard::new(60); // 60-second window
///
/// // First submission accepted.
/// assert!(guard.check("req-abc", "GXXX", 1_000).is_accepted());
///
/// // Same ID within the window is a replay.
/// assert!(guard.check("req-abc", "GXXX", 1_030).is_replay());
///
/// // A different ID is accepted.
/// assert!(guard.check("req-xyz", "GXXX", 1_030).is_accepted());
/// ```
#[derive(Debug)]
pub struct HostReplayGuard {
    /// Deduplication window in seconds.
    pub window_secs: u64,
    /// Seen entries, ordered by `first_seen_at` ascending.
    entries: Vec<SeenEntry>,
    /// Total number of replay detections since creation.
    pub replay_count: u64,
    /// Total number of accepted (non-replay) requests since creation.
    pub accepted_count: u64,
}

impl HostReplayGuard {
    /// Create a new guard with the given deduplication window.
    ///
    /// # Arguments
    ///
    /// * `window_secs` – How long (in seconds) a request ID is remembered.
    ///   Pass `0` to remember IDs indefinitely (no expiry).
    pub fn new(window_secs: u64) -> Self {
        Self {
            window_secs,
            entries: Vec::new(),
            replay_count: 0,
            accepted_count: 0,
        }
    }

    /// Check whether `request_id` from `actor` at `now_secs` is a replay.
    ///
    /// Expired entries are pruned before the lookup.
    ///
    /// # Arguments
    ///
    /// * `request_id` – Unique identifier for the request (e.g. a nonce or
    ///   transaction hash).
    /// * `actor`      – The submitting party's identifier (used for metrics;
    ///   the dedup key is `request_id` alone).
    /// * `now_secs`   – Current Unix timestamp in seconds.
    ///
    /// # Returns
    ///
    /// [`CheckResult::Accepted`] if the ID is new; [`CheckResult::Replay`] if
    /// it was already seen within the window.
    pub fn check(&mut self, request_id: &str, actor: &str, now_secs: u64) -> CheckResult {
        // Prune expired entries first.
        self.prune_expired(now_secs);

        // Look for an existing entry with the same request_id.
        if let Some(entry) = self.entries.iter().find(|e| e.request_id == request_id) {
            self.replay_count += 1;
            return CheckResult::Replay { first_seen_at: entry.first_seen_at };
        }

        // New ID — record it.
        self.entries.push(SeenEntry {
            request_id: String::from(request_id),
            actor: String::from(actor),
            first_seen_at: now_secs,
        });
        self.accepted_count += 1;
        CheckResult::Accepted
    }

    /// Manually prune all entries older than `now_secs - window_secs`.
    ///
    /// Called automatically by [`check`](Self::check), but exposed so callers
    /// can trigger a cleanup sweep independently (e.g. on a background timer).
    pub fn prune_expired(&mut self, now_secs: u64) {
        if self.window_secs == 0 {
            return; // Infinite window — nothing expires.
        }
        let cutoff = now_secs.saturating_sub(self.window_secs);
        self.entries.retain(|e| e.first_seen_at >= cutoff);
    }

    /// Return the number of entries currently held in the window.
    pub fn window_size(&self) -> usize {
        self.entries.len()
    }

    /// Check whether `request_id` is currently in the window without recording it.
    ///
    /// Useful for pre-flight inspection without side effects.
    pub fn is_known(&self, request_id: &str) -> bool {
        self.entries.iter().any(|e| e.request_id == request_id)
    }

    /// Forcibly forget a request ID (e.g. after an admin-approved resubmission).
    ///
    /// Returns `true` if the entry was found and removed, `false` if it was not present.
    pub fn forget(&mut self, request_id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.request_id != request_id);
        self.entries.len() < before
    }

    /// Reset all state (entries and counters).
    pub fn reset(&mut self) {
        self.entries.clear();
        self.replay_count = 0;
        self.accepted_count = 0;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Basic accept / replay ───────────────────────────────────────────────

    #[test]
    fn test_new_id_is_accepted() {
        let mut guard = HostReplayGuard::new(300);
        assert!(guard.check("req-001", "GXXX", 1_000).is_accepted());
        assert_eq!(guard.accepted_count, 1);
        assert_eq!(guard.replay_count, 0);
    }

    #[test]
    fn test_duplicate_id_is_replay() {
        let mut guard = HostReplayGuard::new(300);
        guard.check("req-001", "GXXX", 1_000);
        let result = guard.check("req-001", "GXXX", 1_001);
        assert!(result.is_replay());
        assert_eq!(guard.replay_count, 1);
        if let CheckResult::Replay { first_seen_at } = result {
            assert_eq!(first_seen_at, 1_000);
        }
    }

    #[test]
    fn test_different_ids_both_accepted() {
        let mut guard = HostReplayGuard::new(300);
        assert!(guard.check("req-001", "GXXX", 1_000).is_accepted());
        assert!(guard.check("req-002", "GXXX", 1_001).is_accepted());
        assert_eq!(guard.window_size(), 2);
    }

    // ── Window expiry / pruning ──────────────────────────────────────────────

    #[test]
    fn test_expired_id_is_accepted_again() {
        let mut guard = HostReplayGuard::new(60);
        guard.check("req-001", "GXXX", 1_000);
        // 70 seconds later — entry should have expired.
        let result = guard.check("req-001", "GXXX", 1_070);
        assert!(result.is_accepted(), "expired entry should be re-accepted");
    }

    #[test]
    fn test_prune_expired_removes_old_entries() {
        let mut guard = HostReplayGuard::new(60);
        guard.check("req-old", "GXXX", 1_000);
        guard.check("req-new", "GXXX", 1_090);

        // Prune at t=1_090: req-old (age=90) is outside window.
        guard.prune_expired(1_090);

        assert_eq!(guard.window_size(), 1);
        assert!(!guard.is_known("req-old"));
        assert!(guard.is_known("req-new"));
    }

    #[test]
    fn test_infinite_window_never_expires() {
        let mut guard = HostReplayGuard::new(0); // window_secs = 0 → infinite
        guard.check("req-001", "GXXX", 1_000);
        // Very far in the future — should still be a replay.
        assert!(guard.check("req-001", "GXXX", 9_999_999).is_replay());
    }

    // ── is_known / forget ────────────────────────────────────────────────────

    #[test]
    fn test_is_known_after_accept() {
        let mut guard = HostReplayGuard::new(300);
        guard.check("req-001", "GXXX", 1_000);
        assert!(guard.is_known("req-001"));
        assert!(!guard.is_known("req-999"));
    }

    #[test]
    fn test_forget_removes_entry() {
        let mut guard = HostReplayGuard::new(300);
        guard.check("req-001", "GXXX", 1_000);
        assert!(guard.forget("req-001"));
        assert!(!guard.is_known("req-001"));
        // After forgetting, the same ID is accepted again.
        assert!(guard.check("req-001", "GXXX", 1_001).is_accepted());
    }

    #[test]
    fn test_forget_returns_false_for_unknown_id() {
        let mut guard = HostReplayGuard::new(300);
        assert!(!guard.forget("req-nope"));
    }

    // ── reset ────────────────────────────────────────────────────────────────

    #[test]
    fn test_reset_clears_all_state() {
        let mut guard = HostReplayGuard::new(300);
        guard.check("req-001", "GXXX", 1_000);
        guard.check("req-001", "GXXX", 1_001); // replay

        guard.reset();

        assert_eq!(guard.window_size(), 0);
        assert_eq!(guard.accepted_count, 0);
        assert_eq!(guard.replay_count, 0);
        assert!(guard.check("req-001", "GXXX", 1_002).is_accepted());
    }

    // ── window_size tracking ─────────────────────────────────────────────────

    #[test]
    fn test_window_size_stays_bounded_after_expiry() {
        let mut guard = HostReplayGuard::new(10);
        for i in 0..20_u64 {
            guard.check(&alloc::format!("req-{}", i), "GXXX", i);
        }
        // At t=20, all entries older than t=10 are pruned on next check.
        guard.check("req-new", "GXXX", 20);
        // Only entries from t=10..=20 remain (11 entries).
        assert!(guard.window_size() <= 11);
    }
}
