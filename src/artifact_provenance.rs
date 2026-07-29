//! Artifact provenance tracking for release artifacts (issue #674).
//!
//! Release artifacts should be traceable to their build inputs and environment
//! for trust and compliance. This module records and verifies the origin,
//! build command, source revision, and environment for each artifact so the
//! full provenance chain is available for audit.
//!
//! ## Design
//!
//! - [`ArtifactProvenance`] is the canonical provenance record for a single
//!   release artifact.
//! - [`ProvenanceStore`] manages an in-memory collection of provenance records
//!   and supports lookup by artifact name or content hash.
//! - [`ProvenanceVerifier`] checks a given record against expected values and
//!   returns a detailed [`VerificationReport`].

extern crate alloc;

use alloc::{string::String, vec::Vec};

use crate::errors::{AnchorKitError, ErrorCode};

// ---------------------------------------------------------------------------
// Core provenance record
// ---------------------------------------------------------------------------

/// Provenance record for a single release artifact.
///
/// Every field that could be unknown at record time is `Option<String>` so
/// callers can attach partial provenance and fill in remaining fields later
/// via [`ProvenanceStore::update`].
#[derive(Clone, Debug, PartialEq)]
pub struct ArtifactProvenance {
    /// Unique identifier for this provenance record (monotonically increasing).
    pub provenance_id: u64,
    /// Name of the artifact (e.g. `"anchorkit-0.1.0-wasm32.wasm"`).
    pub artifact_name: String,
    /// Hex-encoded SHA-256 content hash of the artifact file.
    pub content_hash: String,
    /// Source control revision (e.g. a Git commit SHA).
    pub source_revision: Option<String>,
    /// Source repository URI.
    pub source_repository: Option<String>,
    /// Exact build command used to produce the artifact
    /// (e.g. `"cargo build --release --target wasm32-unknown-unknown"`).
    pub build_command: Option<String>,
    /// Key/value pairs describing the build environment
    /// (e.g. `rustc` version, OS, CI job ID).
    pub build_env: Vec<(String, String)>,
    /// Unix timestamp when this record was created.
    pub recorded_at: u64,
    /// Optional signature over the content hash for non-repudiation.
    pub signature: Option<String>,
}

impl ArtifactProvenance {
    /// Returns `true` when all required audit fields are populated.
    ///
    /// Required for a provenance record to be considered "complete" for
    /// compliance purposes: `source_revision`, `source_repository`, and
    /// `build_command` must all be `Some`.
    pub fn is_complete(&self) -> bool {
        self.source_revision.is_some()
            && self.source_repository.is_some()
            && self.build_command.is_some()
    }
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

/// Result of a single field comparison in a provenance verification.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldVerdict {
    /// Field matched the expected value.
    Match,
    /// Field did not match: `(expected, actual)`.
    Mismatch(String, String),
    /// Expected value was provided but the field is absent in the record.
    Missing,
    /// Field was not checked (no expected value was supplied).
    NotChecked,
}

/// Detailed report produced by [`ProvenanceVerifier::verify`].
#[derive(Clone, Debug)]
pub struct VerificationReport {
    /// Provenance record ID that was verified.
    pub provenance_id: u64,
    /// Artifact name from the record.
    pub artifact_name: String,
    /// Verdict for the content hash field.
    pub content_hash: FieldVerdict,
    /// Verdict for the source revision field.
    pub source_revision: FieldVerdict,
    /// Verdict for the build command field.
    pub build_command: FieldVerdict,
    /// `true` when all checked fields passed.
    pub passed: bool,
}

/// Builder-style verifier for a [`ArtifactProvenance`] record.
///
/// Supply expected values for whichever fields you care about; unset fields
/// are reported as [`FieldVerdict::NotChecked`].
///
/// # Examples
///
/// ```rust
/// use anchorkit::artifact_provenance::{ArtifactProvenance, ProvenanceVerifier};
///
/// let record = ArtifactProvenance {
///     provenance_id: 0,
///     artifact_name: "anchorkit.wasm".into(),
///     content_hash: "abc123".into(),
///     source_revision: Some("deadbeef".into()),
///     source_repository: Some("https://github.com/org/repo".into()),
///     build_command: Some("cargo build --release".into()),
///     build_env: vec![],
///     recorded_at: 1000,
///     signature: None,
/// };
///
/// let report = ProvenanceVerifier::new(&record)
///     .expect_content_hash("abc123")
///     .expect_source_revision("deadbeef")
///     .verify();
///
/// assert!(report.passed);
/// ```
pub struct ProvenanceVerifier<'a> {
    record: &'a ArtifactProvenance,
    expected_hash: Option<&'a str>,
    expected_revision: Option<&'a str>,
    expected_build_command: Option<&'a str>,
}

impl<'a> ProvenanceVerifier<'a> {
    /// Create a new verifier for `record`.
    pub fn new(record: &'a ArtifactProvenance) -> Self {
        Self {
            record,
            expected_hash: None,
            expected_revision: None,
            expected_build_command: None,
        }
    }

    /// Set the expected content hash.
    pub fn expect_content_hash(mut self, hash: &'a str) -> Self {
        self.expected_hash = Some(hash);
        self
    }

    /// Set the expected source revision.
    pub fn expect_source_revision(mut self, rev: &'a str) -> Self {
        self.expected_revision = Some(rev);
        self
    }

    /// Set the expected build command.
    pub fn expect_build_command(mut self, cmd: &'a str) -> Self {
        self.expected_build_command = Some(cmd);
        self
    }

    /// Run the verification and return a [`VerificationReport`].
    pub fn verify(self) -> VerificationReport {
        let hash_verdict = check_field(
            self.expected_hash,
            Some(self.record.content_hash.as_str()),
        );
        let rev_verdict = check_field(
            self.expected_revision,
            self.record.source_revision.as_deref(),
        );
        let cmd_verdict = check_field(
            self.expected_build_command,
            self.record.build_command.as_deref(),
        );

        let passed = is_passing(&hash_verdict)
            && is_passing(&rev_verdict)
            && is_passing(&cmd_verdict);

        VerificationReport {
            provenance_id: self.record.provenance_id,
            artifact_name: self.record.artifact_name.clone(),
            content_hash: hash_verdict,
            source_revision: rev_verdict,
            build_command: cmd_verdict,
            passed,
        }
    }
}

fn check_field(expected: Option<&str>, actual: Option<&str>) -> FieldVerdict {
    match (expected, actual) {
        (None, _) => FieldVerdict::NotChecked,
        (Some(exp), None) => FieldVerdict::Missing,
        (Some(exp), Some(act)) => {
            if exp == act {
                FieldVerdict::Match
            } else {
                FieldVerdict::Mismatch(exp.into(), act.into())
            }
        }
    }
}

fn is_passing(v: &FieldVerdict) -> bool {
    matches!(v, FieldVerdict::Match | FieldVerdict::NotChecked)
}

// ---------------------------------------------------------------------------
// Provenance store
// ---------------------------------------------------------------------------

/// Manages a collection of [`ArtifactProvenance`] records.
pub struct ProvenanceStore {
    records: Vec<ArtifactProvenance>,
    next_id: u64,
}

impl ProvenanceStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            next_id: 0,
        }
    }

    /// Record provenance for a new artifact.
    ///
    /// # Errors
    ///
    /// Returns [`AnchorKitError`] with [`ErrorCode::ValidationError`] when
    /// `artifact_name` or `content_hash` is empty.
    pub fn record(
        &mut self,
        artifact_name: String,
        content_hash: String,
        recorded_at: u64,
    ) -> Result<&ArtifactProvenance, AnchorKitError> {
        if artifact_name.is_empty() {
            return Err(AnchorKitError::validation_error("artifact_name must not be empty"));
        }
        if content_hash.is_empty() {
            return Err(AnchorKitError::validation_error("content_hash must not be empty"));
        }

        let provenance = ArtifactProvenance {
            provenance_id: self.next_id,
            artifact_name,
            content_hash,
            source_revision: None,
            source_repository: None,
            build_command: None,
            build_env: Vec::new(),
            recorded_at,
            signature: None,
        };

        self.next_id += 1;
        self.records.push(provenance);
        Ok(self.records.last().unwrap())
    }

    /// Update fields on an existing provenance record.
    ///
    /// Only `Some` fields in the update closure are applied; `None` leaves
    /// the current value unchanged.
    pub fn update(
        &mut self,
        provenance_id: u64,
        source_revision: Option<String>,
        source_repository: Option<String>,
        build_command: Option<String>,
        build_env: Option<Vec<(String, String)>>,
        signature: Option<String>,
    ) -> Result<(), AnchorKitError> {
        let record = self
            .records
            .iter_mut()
            .find(|r| r.provenance_id == provenance_id)
            .ok_or_else(|| AnchorKitError::new(ErrorCode::TransactionNotFound, "provenance record not found"))?;

        if let Some(v) = source_revision { record.source_revision = Some(v); }
        if let Some(v) = source_repository { record.source_repository = Some(v); }
        if let Some(v) = build_command { record.build_command = Some(v); }
        if let Some(v) = build_env { record.build_env = v; }
        if let Some(v) = signature { record.signature = Some(v); }

        Ok(())
    }

    /// Look up a provenance record by its ID.
    pub fn get_by_id(&self, provenance_id: u64) -> Option<&ArtifactProvenance> {
        self.records.iter().find(|r| r.provenance_id == provenance_id)
    }

    /// Look up all provenance records for an artifact name.
    pub fn get_by_name(&self, artifact_name: &str) -> Vec<&ArtifactProvenance> {
        self.records
            .iter()
            .filter(|r| r.artifact_name == artifact_name)
            .collect()
    }

    /// Look up a provenance record by its content hash.
    pub fn get_by_hash(&self, content_hash: &str) -> Option<&ArtifactProvenance> {
        self.records.iter().find(|r| r.content_hash == content_hash)
    }

    /// Total number of recorded provenance entries.
    pub fn count(&self) -> usize {
        self.records.len()
    }

    /// Return all records, oldest first.
    pub fn list(&self) -> &[ArtifactProvenance] {
        &self.records
    }
}

impl Default for ProvenanceStore {
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

    #[test]
    fn record_empty_name_rejected() {
        let mut store = ProvenanceStore::new();
        let err = store.record("".into(), "abc".into(), 0).unwrap_err();
        assert_eq!(err.code, ErrorCode::ValidationError);
    }

    #[test]
    fn record_empty_hash_rejected() {
        let mut store = ProvenanceStore::new();
        let err = store.record("artifact.wasm".into(), "".into(), 0).unwrap_err();
        assert_eq!(err.code, ErrorCode::ValidationError);
    }

    #[test]
    fn record_creates_entry_with_correct_fields() {
        let mut store = ProvenanceStore::new();
        let p = store.record("a.wasm".into(), "hash1".into(), 1000).unwrap();
        assert_eq!(p.provenance_id, 0);
        assert_eq!(p.artifact_name, "a.wasm");
        assert_eq!(p.content_hash, "hash1");
        assert_eq!(p.recorded_at, 1000);
        assert!(!p.is_complete());
    }

    #[test]
    fn update_populates_optional_fields() {
        let mut store = ProvenanceStore::new();
        store.record("a.wasm".into(), "h1".into(), 0).unwrap();
        store
            .update(
                0,
                Some("abc123".into()),
                Some("https://github.com/org/repo".into()),
                Some("cargo build --release".into()),
                Some(vec![("rustc".into(), "1.80.0".into())]),
                None,
            )
            .unwrap();
        let p = store.get_by_id(0).unwrap();
        assert!(p.is_complete());
        assert_eq!(p.source_revision.as_deref(), Some("abc123"));
        assert_eq!(p.build_env[0].0, "rustc");
    }

    #[test]
    fn update_unknown_id_returns_error() {
        let mut store = ProvenanceStore::new();
        let err = store.update(99, None, None, None, None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::TransactionNotFound);
    }

    #[test]
    fn get_by_hash_finds_correct_record() {
        let mut store = ProvenanceStore::new();
        store.record("a.wasm".into(), "h1".into(), 0).unwrap();
        store.record("b.wasm".into(), "h2".into(), 1).unwrap();
        let p = store.get_by_hash("h2").unwrap();
        assert_eq!(p.artifact_name, "b.wasm");
    }

    #[test]
    fn get_by_name_returns_all_versions() {
        let mut store = ProvenanceStore::new();
        store.record("a.wasm".into(), "h1".into(), 0).unwrap();
        store.record("a.wasm".into(), "h2".into(), 1).unwrap();
        store.record("b.wasm".into(), "h3".into(), 2).unwrap();
        let results = store.get_by_name("a.wasm");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn verifier_all_match_passes() {
        let record = ArtifactProvenance {
            provenance_id: 0,
            artifact_name: "a.wasm".into(),
            content_hash: "deadbeef".into(),
            source_revision: Some("abc123".into()),
            source_repository: Some("https://github.com/org/repo".into()),
            build_command: Some("cargo build".into()),
            build_env: vec![],
            recorded_at: 0,
            signature: None,
        };
        let report = ProvenanceVerifier::new(&record)
            .expect_content_hash("deadbeef")
            .expect_source_revision("abc123")
            .expect_build_command("cargo build")
            .verify();
        assert!(report.passed);
        assert_eq!(report.content_hash, FieldVerdict::Match);
    }

    #[test]
    fn verifier_hash_mismatch_fails() {
        let record = ArtifactProvenance {
            provenance_id: 1,
            artifact_name: "b.wasm".into(),
            content_hash: "real_hash".into(),
            source_revision: None,
            source_repository: None,
            build_command: None,
            build_env: vec![],
            recorded_at: 0,
            signature: None,
        };
        let report = ProvenanceVerifier::new(&record)
            .expect_content_hash("wrong_hash")
            .verify();
        assert!(!report.passed);
        assert!(matches!(report.content_hash, FieldVerdict::Mismatch(_, _)));
    }

    #[test]
    fn verifier_missing_field_fails() {
        let record = ArtifactProvenance {
            provenance_id: 2,
            artifact_name: "c.wasm".into(),
            content_hash: "h".into(),
            source_revision: None, // absent
            source_repository: None,
            build_command: None,
            build_env: vec![],
            recorded_at: 0,
            signature: None,
        };
        let report = ProvenanceVerifier::new(&record)
            .expect_source_revision("abc")
            .verify();
        assert!(!report.passed);
        assert_eq!(report.source_revision, FieldVerdict::Missing);
    }

    #[test]
    fn is_complete_requires_all_three_fields() {
        let mut p = ArtifactProvenance {
            provenance_id: 0,
            artifact_name: "a".into(),
            content_hash: "h".into(),
            source_revision: None,
            source_repository: None,
            build_command: None,
            build_env: vec![],
            recorded_at: 0,
            signature: None,
        };
        assert!(!p.is_complete());
        p.source_revision = Some("rev".into());
        assert!(!p.is_complete());
        p.source_repository = Some("repo".into());
        assert!(!p.is_complete());
        p.build_command = Some("cargo build".into());
        assert!(p.is_complete());
    }
}
