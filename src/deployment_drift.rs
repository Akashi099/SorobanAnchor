//! Deployment drift detection (issue #673).
//!
//! Environments drift over time. This module helps operators catch unexpected
//! differences between intended and actual deployment state by comparing a
//! [`DeploymentSpec`] (what you expect) against a [`DeploymentSnapshot`]
//! (what is actually running) and producing a detailed [`DriftReport`].
//!
//! ## Design
//!
//! - [`DeploymentSpec`] describes the intended state: contract version,
//!   network, admin address, and a set of expected config entries.
//! - [`DeploymentSnapshot`] captures the observed state at a point in time.
//! - [`detect_drift`] compares the two and returns a [`DriftReport`] listing
//!   every [`DriftItem`] — fields that are missing, changed, or unexpected.
//! - [`DriftSeverity`] classifies each drift item so operators can triage
//!   critical divergences from cosmetic ones.

extern crate alloc;

use alloc::{string::String, vec::Vec};

// ---------------------------------------------------------------------------
// Spec and snapshot types
// ---------------------------------------------------------------------------

/// A key/value configuration entry in a deployment spec or snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigEntry {
    pub key: String,
    pub value: String,
}

impl ConfigEntry {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self { key: key.into(), value: value.into() }
    }
}

/// The intended (expected) deployment state for a single environment.
#[derive(Clone, Debug, PartialEq)]
pub struct DeploymentSpec {
    /// Human-readable environment name (e.g. `"mainnet"`, `"testnet"`).
    pub environment: String,
    /// Expected deployed contract/binary version string.
    pub expected_version: String,
    /// Expected Stellar network passphrase or short name.
    pub expected_network: String,
    /// Expected admin address.
    pub expected_admin: Option<String>,
    /// Expected configuration key/value pairs.
    pub expected_config: Vec<ConfigEntry>,
}

/// The observed deployment state captured at a point in time.
#[derive(Clone, Debug, PartialEq)]
pub struct DeploymentSnapshot {
    /// Environment this snapshot was taken from.
    pub environment: String,
    /// Unix timestamp when the snapshot was captured.
    pub captured_at: u64,
    /// Observed deployed version string.
    pub observed_version: String,
    /// Observed Stellar network passphrase or short name.
    pub observed_network: String,
    /// Observed admin address.
    pub observed_admin: Option<String>,
    /// Observed configuration key/value pairs.
    pub observed_config: Vec<ConfigEntry>,
}

// ---------------------------------------------------------------------------
// Drift classification
// ---------------------------------------------------------------------------

/// How severe a detected drift item is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DriftSeverity {
    /// The difference is cosmetic or informational (e.g. an extra config key
    /// not listed in the spec but not dangerous).
    Low,
    /// The difference is notable and should be investigated (e.g. a config
    /// value mismatch that could affect behaviour).
    Medium,
    /// The difference is critical and requires immediate action (e.g. wrong
    /// network, wrong version, wrong admin address).
    Critical,
}

impl DriftSeverity {
    pub fn label(&self) -> &'static str {
        match self {
            DriftSeverity::Low => "low",
            DriftSeverity::Medium => "medium",
            DriftSeverity::Critical => "critical",
        }
    }
}

/// A single detected difference between spec and snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct DriftItem {
    /// Short identifier for the field or config key that drifted.
    pub field: String,
    /// Severity of this drift item.
    pub severity: DriftSeverity,
    /// The expected value (from the spec). `None` means "not expected at all".
    pub expected: Option<String>,
    /// The observed value (from the snapshot). `None` means "not present".
    pub observed: Option<String>,
    /// Human-readable description of the drift.
    pub description: String,
}

// ---------------------------------------------------------------------------
// Drift report
// ---------------------------------------------------------------------------

/// The full result of comparing a [`DeploymentSpec`] against a
/// [`DeploymentSnapshot`].
#[derive(Clone, Debug)]
pub struct DriftReport {
    /// Environment name from the spec.
    pub environment: String,
    /// Unix timestamp of the snapshot.
    pub snapshot_captured_at: u64,
    /// All detected drift items, ordered by severity descending.
    pub items: Vec<DriftItem>,
    /// `true` when no drift items were found.
    pub is_clean: bool,
    /// Number of critical drift items.
    pub critical_count: usize,
    /// Number of medium drift items.
    pub medium_count: usize,
    /// Number of low drift items.
    pub low_count: usize,
}

impl DriftReport {
    /// Returns `true` when any item has [`DriftSeverity::Critical`] severity.
    pub fn has_critical(&self) -> bool {
        self.critical_count > 0
    }

    /// Returns only the items at or above `min_severity`.
    pub fn items_at_or_above(&self, min_severity: DriftSeverity) -> Vec<&DriftItem> {
        self.items
            .iter()
            .filter(|i| i.severity >= min_severity)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Core detection function
// ---------------------------------------------------------------------------

/// Compare a [`DeploymentSpec`] against a [`DeploymentSnapshot`] and return
/// a [`DriftReport`] listing all detected differences.
///
/// # Examples
///
/// ```rust
/// use anchorkit::deployment_drift::{
///     ConfigEntry, DeploymentSpec, DeploymentSnapshot, detect_drift, DriftSeverity,
/// };
///
/// let spec = DeploymentSpec {
///     environment: "testnet".into(),
///     expected_version: "0.1.0".into(),
///     expected_network: "Test SDF Network ; September 2015".into(),
///     expected_admin: Some("GABC123".into()),
///     expected_config: vec![ConfigEntry::new("rate_limit", "100")],
/// };
///
/// let snapshot = DeploymentSnapshot {
///     environment: "testnet".into(),
///     captured_at: 9999,
///     observed_version: "0.2.0".into(), // drifted
///     observed_network: "Test SDF Network ; September 2015".into(),
///     observed_admin: Some("GABC123".into()),
///     observed_config: vec![ConfigEntry::new("rate_limit", "100")],
/// };
///
/// let report = detect_drift(&spec, &snapshot);
/// assert!(!report.is_clean);
/// assert!(report.has_critical());
/// ```
pub fn detect_drift(spec: &DeploymentSpec, snapshot: &DeploymentSnapshot) -> DriftReport {
    let mut items: Vec<DriftItem> = Vec::new();

    // ── Version ─────────────────────────────────────────────────────────────
    if spec.expected_version != snapshot.observed_version {
        items.push(DriftItem {
            field: "version".into(),
            severity: DriftSeverity::Critical,
            expected: Some(spec.expected_version.clone()),
            observed: Some(snapshot.observed_version.clone()),
            description: alloc::format!(
                "deployed version '{}' differs from expected '{}'",
                snapshot.observed_version, spec.expected_version
            ),
        });
    }

    // ── Network ─────────────────────────────────────────────────────────────
    if spec.expected_network != snapshot.observed_network {
        items.push(DriftItem {
            field: "network".into(),
            severity: DriftSeverity::Critical,
            expected: Some(spec.expected_network.clone()),
            observed: Some(snapshot.observed_network.clone()),
            description: alloc::format!(
                "network '{}' differs from expected '{}'",
                snapshot.observed_network, spec.expected_network
            ),
        });
    }

    // ── Admin address ────────────────────────────────────────────────────────
    match (&spec.expected_admin, &snapshot.observed_admin) {
        (Some(exp), Some(obs)) if exp != obs => {
            items.push(DriftItem {
                field: "admin".into(),
                severity: DriftSeverity::Critical,
                expected: Some(exp.clone()),
                observed: Some(obs.clone()),
                description: alloc::format!(
                    "admin address '{}' differs from expected '{}'",
                    obs, exp
                ),
            });
        }
        (Some(exp), None) => {
            items.push(DriftItem {
                field: "admin".into(),
                severity: DriftSeverity::Critical,
                expected: Some(exp.clone()),
                observed: None,
                description: "expected admin address is absent in snapshot".into(),
            });
        }
        (None, Some(obs)) => {
            items.push(DriftItem {
                field: "admin".into(),
                severity: DriftSeverity::Medium,
                expected: None,
                observed: Some(obs.clone()),
                description: alloc::format!(
                    "unexpected admin address '{}' found in snapshot",
                    obs
                ),
            });
        }
        _ => {}
    }

    // ── Config entries ───────────────────────────────────────────────────────

    // Check that every expected config key is present and matches.
    for expected_entry in &spec.expected_config {
        match snapshot
            .observed_config
            .iter()
            .find(|e| e.key == expected_entry.key)
        {
            None => {
                items.push(DriftItem {
                    field: alloc::format!("config.{}", expected_entry.key),
                    severity: DriftSeverity::Medium,
                    expected: Some(expected_entry.value.clone()),
                    observed: None,
                    description: alloc::format!(
                        "expected config key '{}' is absent in snapshot",
                        expected_entry.key
                    ),
                });
            }
            Some(obs_entry) if obs_entry.value != expected_entry.value => {
                items.push(DriftItem {
                    field: alloc::format!("config.{}", expected_entry.key),
                    severity: DriftSeverity::Medium,
                    expected: Some(expected_entry.value.clone()),
                    observed: Some(obs_entry.value.clone()),
                    description: alloc::format!(
                        "config key '{}': observed '{}', expected '{}'",
                        expected_entry.key, obs_entry.value, expected_entry.value
                    ),
                });
            }
            _ => {}
        }
    }

    // Flag extra config keys in the snapshot that the spec doesn't mention.
    for obs_entry in &snapshot.observed_config {
        if !spec
            .expected_config
            .iter()
            .any(|e| e.key == obs_entry.key)
        {
            items.push(DriftItem {
                field: alloc::format!("config.{}", obs_entry.key),
                severity: DriftSeverity::Low,
                expected: None,
                observed: Some(obs_entry.value.clone()),
                description: alloc::format!(
                    "config key '{}' is present in snapshot but not in spec",
                    obs_entry.key
                ),
            });
        }
    }

    // Sort: Critical first, then Medium, then Low.
    items.sort_by(|a, b| b.severity.cmp(&a.severity));

    let critical_count = items.iter().filter(|i| i.severity == DriftSeverity::Critical).count();
    let medium_count   = items.iter().filter(|i| i.severity == DriftSeverity::Medium).count();
    let low_count      = items.iter().filter(|i| i.severity == DriftSeverity::Low).count();
    let is_clean       = items.is_empty();

    DriftReport {
        environment: spec.environment.clone(),
        snapshot_captured_at: snapshot.captured_at,
        items,
        is_clean,
        critical_count,
        medium_count,
        low_count,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn base_spec() -> DeploymentSpec {
        DeploymentSpec {
            environment: "testnet".into(),
            expected_version: "0.1.0".into(),
            expected_network: "Test SDF Network ; September 2015".into(),
            expected_admin: Some("GABC123".into()),
            expected_config: vec![
                ConfigEntry::new("rate_limit", "100"),
                ConfigEntry::new("max_sessions", "50"),
            ],
        }
    }

    fn base_snapshot() -> DeploymentSnapshot {
        DeploymentSnapshot {
            environment: "testnet".into(),
            captured_at: 9999,
            observed_version: "0.1.0".into(),
            observed_network: "Test SDF Network ; September 2015".into(),
            observed_admin: Some("GABC123".into()),
            observed_config: vec![
                ConfigEntry::new("rate_limit", "100"),
                ConfigEntry::new("max_sessions", "50"),
            ],
        }
    }

    #[test]
    fn no_drift_when_spec_and_snapshot_match() {
        let report = detect_drift(&base_spec(), &base_snapshot());
        assert!(report.is_clean);
        assert_eq!(report.critical_count, 0);
        assert_eq!(report.medium_count, 0);
        assert_eq!(report.low_count, 0);
    }

    #[test]
    fn version_mismatch_is_critical() {
        let mut snap = base_snapshot();
        snap.observed_version = "0.2.0".into();
        let report = detect_drift(&base_spec(), &snap);
        assert!(!report.is_clean);
        assert!(report.has_critical());
        let item = report.items.iter().find(|i| i.field == "version").unwrap();
        assert_eq!(item.severity, DriftSeverity::Critical);
        assert_eq!(item.expected.as_deref(), Some("0.1.0"));
        assert_eq!(item.observed.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn network_mismatch_is_critical() {
        let mut snap = base_snapshot();
        snap.observed_network = "Public Global Stellar Network ; September 2015".into();
        let report = detect_drift(&base_spec(), &snap);
        assert!(report.has_critical());
        assert!(report.items.iter().any(|i| i.field == "network"));
    }

    #[test]
    fn admin_mismatch_is_critical() {
        let mut snap = base_snapshot();
        snap.observed_admin = Some("GDIFFERENT".into());
        let report = detect_drift(&base_spec(), &snap);
        assert!(report.has_critical());
        assert!(report.items.iter().any(|i| i.field == "admin"));
    }

    #[test]
    fn missing_expected_admin_is_critical() {
        let mut snap = base_snapshot();
        snap.observed_admin = None;
        let report = detect_drift(&base_spec(), &snap);
        assert!(report.has_critical());
    }

    #[test]
    fn unexpected_admin_in_snapshot_is_medium() {
        let mut spec = base_spec();
        spec.expected_admin = None;
        let report = detect_drift(&spec, &base_snapshot());
        let item = report.items.iter().find(|i| i.field == "admin").unwrap();
        assert_eq!(item.severity, DriftSeverity::Medium);
    }

    #[test]
    fn config_value_mismatch_is_medium() {
        let mut snap = base_snapshot();
        snap.observed_config[0].value = "200".into(); // rate_limit changed
        let report = detect_drift(&base_spec(), &snap);
        assert_eq!(report.medium_count, 1);
        let item = report.items.iter().find(|i| i.field == "config.rate_limit").unwrap();
        assert_eq!(item.severity, DriftSeverity::Medium);
    }

    #[test]
    fn missing_config_key_is_medium() {
        let mut snap = base_snapshot();
        snap.observed_config.retain(|e| e.key != "max_sessions");
        let report = detect_drift(&base_spec(), &snap);
        assert!(report.items.iter().any(|i| i.field == "config.max_sessions"));
        assert_eq!(
            report.items.iter().find(|i| i.field == "config.max_sessions").unwrap().severity,
            DriftSeverity::Medium
        );
    }

    #[test]
    fn extra_config_key_in_snapshot_is_low() {
        let mut snap = base_snapshot();
        snap.observed_config.push(ConfigEntry::new("extra_key", "val"));
        let report = detect_drift(&base_spec(), &snap);
        let item = report.items.iter().find(|i| i.field == "config.extra_key").unwrap();
        assert_eq!(item.severity, DriftSeverity::Low);
    }

    #[test]
    fn items_sorted_critical_first() {
        let mut snap = base_snapshot();
        snap.observed_version = "0.2.0".into();        // critical
        snap.observed_config[0].value = "999".into();  // medium
        snap.observed_config.push(ConfigEntry::new("extra", "x")); // low
        let report = detect_drift(&base_spec(), &snap);
        let severities: Vec<&DriftSeverity> =
            report.items.iter().map(|i| &i.severity).collect();
        // Verify descending order (Critical >= Medium >= Low).
        for window in severities.windows(2) {
            assert!(window[0] >= window[1]);
        }
    }

    #[test]
    fn items_at_or_above_filters_correctly() {
        let mut snap = base_snapshot();
        snap.observed_version = "0.2.0".into();
        snap.observed_config[0].value = "999".into();
        snap.observed_config.push(ConfigEntry::new("extra", "x"));
        let report = detect_drift(&base_spec(), &snap);
        let critical_only = report.items_at_or_above(DriftSeverity::Critical);
        assert!(critical_only.iter().all(|i| i.severity == DriftSeverity::Critical));
        let med_and_above = report.items_at_or_above(DriftSeverity::Medium);
        assert!(med_and_above.iter().all(|i| i.severity >= DriftSeverity::Medium));
    }

    #[test]
    fn report_environment_matches_spec() {
        let report = detect_drift(&base_spec(), &base_snapshot());
        assert_eq!(report.environment, "testnet");
        assert_eq!(report.snapshot_captured_at, 9999);
    }
}
