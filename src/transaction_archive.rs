//! Archived transaction history support (issue #675).
//!
//! Long-lived transaction histories can become expensive to keep in active
//! on-chain storage. This module provides a clearly-defined archive path so
//! operators can move old records out of hot storage while retaining a
//! verifiable retrieval path.
//!
//! ## Design
//!
//! - [`TransactionArchive`] is the on-chain envelope written when records are
//!   archived.  It stores the archive timestamp, the record count, and a
//!   SHA-256 commitment over all archived transaction IDs so the archive can
//!   be verified without re-fetching every record.
//! - [`ArchiveIndex`] is a lightweight persistent index mapping an archive ID
//!   to its [`TransactionArchive`] envelope.
//! - [`TransactionArchiveManager`] exposes the archive/retrieval operations.

extern crate alloc;

use alloc::{string::String, vec::Vec};

use crate::errors::{AnchorKitError, ErrorCode};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Metadata envelope stored for a single archived batch of transactions.
#[derive(Clone, Debug, PartialEq)]
pub struct TransactionArchive {
    /// Unique, monotonically-increasing identifier for this archive batch.
    pub archive_id: u64,
    /// Ledger timestamp when the archive was created.
    pub archived_at: u64,
    /// Number of transaction records included in this archive batch.
    pub record_count: u32,
    /// SHA-256 commitment over the concatenated archived transaction IDs.
    /// Allows integrity verification without re-reading every record.
    pub commitment: [u8; 32],
    /// Human-readable label supplied by the operator (e.g. `"2025-Q1"`).
    pub label: String,
    /// Optional URI where the full archive payload can be retrieved
    /// (e.g. an IPFS CID or an S3 object key).
    pub retrieval_uri: Option<String>,
}

/// Status of an archived record retrieval attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum ArchiveRetrievalStatus {
    /// Archive envelope found; retrieval URI is available.
    Found(TransactionArchive),
    /// Archive ID exists but no retrieval URI has been set.
    PendingUri(TransactionArchive),
    /// No archive with the given ID exists.
    NotFound,
}

// ---------------------------------------------------------------------------
// Archive commitment helpers
// ---------------------------------------------------------------------------

/// Compute the SHA-256 commitment for an ordered slice of transaction IDs.
///
/// The commitment is `SHA-256(id_0 || "\n" || id_1 || "\n" || … || id_n || "\n")`.
///
/// # Examples
///
/// ```rust
/// use anchorkit::transaction_archive::compute_archive_commitment;
///
/// let ids = vec!["txn-001".to_string(), "txn-002".to_string()];
/// let c1 = compute_archive_commitment(&ids);
/// let c2 = compute_archive_commitment(&ids);
/// assert_eq!(c1, c2); // deterministic
///
/// let other = compute_archive_commitment(&["txn-003".to_string()]);
/// assert_ne!(c1, other);
/// ```
pub fn compute_archive_commitment(transaction_ids: &[String]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for id in transaction_ids {
        hasher.update(id.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().into()
}

/// Verify that a stored commitment matches the recomputed one for `transaction_ids`.
///
/// # Examples
///
/// ```rust
/// use anchorkit::transaction_archive::{compute_archive_commitment, verify_archive_commitment};
///
/// let ids = vec!["txn-a".to_string()];
/// let commitment = compute_archive_commitment(&ids);
/// assert!(verify_archive_commitment(&commitment, &ids));
/// assert!(!verify_archive_commitment(&commitment, &["txn-b".to_string()]));
/// ```
pub fn verify_archive_commitment(stored: &[u8; 32], transaction_ids: &[String]) -> bool {
    let expected = compute_archive_commitment(transaction_ids);
    // Constant-time comparison.
    let mut diff = 0u8;
    for (a, b) in stored.iter().zip(expected.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// In-memory archive manager
// ---------------------------------------------------------------------------

/// Manages a collection of [`TransactionArchive`] envelopes in memory.
///
/// In production the archive index would be backed by persistent storage
/// (e.g. Soroban contract storage or an off-chain database). This struct
/// provides the core logic that can be wrapped by any storage backend.
pub struct TransactionArchiveManager {
    archives: Vec<TransactionArchive>,
    next_id: u64,
}

impl TransactionArchiveManager {
    /// Create an empty archive manager.
    pub fn new() -> Self {
        Self {
            archives: Vec::new(),
            next_id: 0,
        }
    }

    /// Archive a batch of transaction IDs, returning the new [`TransactionArchive`].
    ///
    /// # Arguments
    ///
    /// * `transaction_ids` – ordered slice of IDs being archived (must be non-empty).
    /// * `archived_at` – Unix timestamp for the archive event.
    /// * `label` – operator-supplied label (e.g. `"2025-Q1"`).
    /// * `retrieval_uri` – optional URI for the full payload.
    ///
    /// # Errors
    ///
    /// Returns [`AnchorKitError`] with [`ErrorCode::ValidationError`] when
    /// `transaction_ids` is empty.
    pub fn archive(
        &mut self,
        transaction_ids: &[String],
        archived_at: u64,
        label: String,
        retrieval_uri: Option<String>,
    ) -> Result<TransactionArchive, AnchorKitError> {
        if transaction_ids.is_empty() {
            return Err(AnchorKitError::validation_error(
                "transaction_ids must not be empty",
            ));
        }

        let commitment = compute_archive_commitment(transaction_ids);
        let archive = TransactionArchive {
            archive_id: self.next_id,
            archived_at,
            record_count: transaction_ids.len() as u32,
            commitment,
            label,
            retrieval_uri,
        };

        self.next_id += 1;
        self.archives.push(archive.clone());
        Ok(archive)
    }

    /// Retrieve an archive by ID.
    pub fn get(&self, archive_id: u64) -> ArchiveRetrievalStatus {
        match self.archives.iter().find(|a| a.archive_id == archive_id) {
            None => ArchiveRetrievalStatus::NotFound,
            Some(a) if a.retrieval_uri.is_some() => {
                ArchiveRetrievalStatus::Found(a.clone())
            }
            Some(a) => ArchiveRetrievalStatus::PendingUri(a.clone()),
        }
    }

    /// Update the retrieval URI for an existing archive.
    ///
    /// Returns `true` when the archive was found and updated.
    pub fn set_retrieval_uri(&mut self, archive_id: u64, uri: String) -> bool {
        match self.archives.iter_mut().find(|a| a.archive_id == archive_id) {
            Some(a) => {
                a.retrieval_uri = Some(uri);
                true
            }
            None => false,
        }
    }

    /// Total number of archived batches.
    pub fn archive_count(&self) -> usize {
        self.archives.len()
    }

    /// Return all archive envelopes, oldest first.
    pub fn list(&self) -> &[TransactionArchive] {
        &self.archives
    }
}

impl Default for TransactionArchiveManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn compute_commitment_is_deterministic() {
        let a = compute_archive_commitment(&ids(&["t1", "t2"]));
        let b = compute_archive_commitment(&ids(&["t1", "t2"]));
        assert_eq!(a, b);
    }

    #[test]
    fn compute_commitment_differs_on_different_input() {
        let a = compute_archive_commitment(&ids(&["t1"]));
        let b = compute_archive_commitment(&ids(&["t2"]));
        assert_ne!(a, b);
    }

    #[test]
    fn verify_commitment_round_trip() {
        let list = ids(&["txn-001", "txn-002", "txn-003"]);
        let c = compute_archive_commitment(&list);
        assert!(verify_archive_commitment(&c, &list));
        assert!(!verify_archive_commitment(&c, &ids(&["txn-001", "txn-999"])));
    }

    #[test]
    fn archive_manager_empty_ids_rejected() {
        let mut mgr = TransactionArchiveManager::new();
        let err = mgr.archive(&[], 1000, "batch-1".into(), None).unwrap_err();
        assert_eq!(err.code, ErrorCode::ValidationError);
    }

    #[test]
    fn archive_manager_creates_archive_with_correct_fields() {
        let mut mgr = TransactionArchiveManager::new();
        let list = ids(&["t1", "t2", "t3"]);
        let archive = mgr.archive(&list, 9999, "q1".into(), None).unwrap();
        assert_eq!(archive.archive_id, 0);
        assert_eq!(archive.record_count, 3);
        assert_eq!(archive.archived_at, 9999);
        assert_eq!(archive.label, "q1");
        assert!(archive.retrieval_uri.is_none());
        assert!(verify_archive_commitment(&archive.commitment, &list));
    }

    #[test]
    fn archive_manager_ids_are_monotonic() {
        let mut mgr = TransactionArchiveManager::new();
        let a1 = mgr.archive(&ids(&["t1"]), 1, "a".into(), None).unwrap();
        let a2 = mgr.archive(&ids(&["t2"]), 2, "b".into(), None).unwrap();
        assert_eq!(a1.archive_id, 0);
        assert_eq!(a2.archive_id, 1);
        assert_eq!(mgr.archive_count(), 2);
    }

    #[test]
    fn get_returns_pending_uri_when_uri_absent() {
        let mut mgr = TransactionArchiveManager::new();
        mgr.archive(&ids(&["t1"]), 1, "lbl".into(), None).unwrap();
        assert!(matches!(mgr.get(0), ArchiveRetrievalStatus::PendingUri(_)));
    }

    #[test]
    fn get_returns_found_when_uri_present() {
        let mut mgr = TransactionArchiveManager::new();
        mgr.archive(&ids(&["t1"]), 1, "lbl".into(), Some("ipfs://cid".into())).unwrap();
        assert!(matches!(mgr.get(0), ArchiveRetrievalStatus::Found(_)));
    }

    #[test]
    fn get_returns_not_found_for_unknown_id() {
        let mgr = TransactionArchiveManager::new();
        assert_eq!(mgr.get(99), ArchiveRetrievalStatus::NotFound);
    }

    #[test]
    fn set_retrieval_uri_updates_existing_archive() {
        let mut mgr = TransactionArchiveManager::new();
        mgr.archive(&ids(&["t1"]), 1, "lbl".into(), None).unwrap();
        assert!(mgr.set_retrieval_uri(0, "ipfs://abc".into()));
        if let ArchiveRetrievalStatus::Found(a) = mgr.get(0) {
            assert_eq!(a.retrieval_uri.as_deref(), Some("ipfs://abc"));
        } else {
            panic!("expected Found after setting URI");
        }
    }

    #[test]
    fn set_retrieval_uri_returns_false_for_unknown_id() {
        let mut mgr = TransactionArchiveManager::new();
        assert!(!mgr.set_retrieval_uri(42, "uri".into()));
    }

    #[test]
    fn list_returns_archives_oldest_first() {
        let mut mgr = TransactionArchiveManager::new();
        mgr.archive(&ids(&["t1"]), 1, "first".into(), None).unwrap();
        mgr.archive(&ids(&["t2"]), 2, "second".into(), None).unwrap();
        let list = mgr.list();
        assert_eq!(list[0].label, "first");
        assert_eq!(list[1].label, "second");
    }
}
