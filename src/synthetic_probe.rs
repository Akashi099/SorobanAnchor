//! Synthetic health probes (issue #687).
//!
//! Synthetic probes provide early warning of service degradation even when
//! real user traffic is low or absent.  Rather than waiting for a genuine
//! request to fail, the monitoring workflow periodically issues artificial
//! "probes" against each anchor's well-known endpoints and records the result
//! as a health observation.
//!
//! # Probe types
//!
//! | Type | What is checked |
//! |------|----------------|
//! | `Ping` | Round-trip latency and basic connectivity to the anchor root URL |
//! | `StellarToml` | Fetch and parse `/.well-known/stellar.toml` |
//! | `Sep6Info` | Fetch the SEP-6 `/info` endpoint and check the response shape |
//! | `Custom` | Arbitrary operator-supplied label (e.g. a deep-health URL) |
//!
//! # Running probes
//!
//! [`SyntheticProbeRunner`] accepts a list of [`ProbeConfig`]s and a
//! `probe_fn` closure, executes them in order, and returns a
//! [`ProbeReport`] for each one.  The `probe_fn` is injected so the runner is
//! fully testable without a live network.
//!
//! # Integration with health scoring
//!
//! Probe results can be converted into [`HealthWindow`](crate::anchor_health::HealthWindow)
//! observations via [`probe_result_to_health_window`], feeding directly into
//! the existing composite health-scoring pipeline.
//!
//! # Example
//!
//! ```rust
//! use anchorkit::synthetic_probe::{
//!     ProbeConfig, ProbeKind, ProbeResult, SyntheticProbeRunner,
//! };
//!
//! let probes = alloc::vec![
//!     ProbeConfig::new(1, ProbeKind::Ping, "https://anchor.example.com").unwrap(),
//!     ProbeConfig::new(2, ProbeKind::StellarToml, "https://anchor.example.com/.well-known/stellar.toml").unwrap(),
//! ];
//!
//! let mut runner = SyntheticProbeRunner::new(probes);
//!
//! let reports = runner.run_all(
//!     |config| {
//!         // Simulate a successful probe.
//!         Ok(ProbeResult::success(config.id, 42))
//!     },
//!     || 1_000_000,
//! );
//!
//! assert_eq!(reports.len(), 2);
//! assert!(reports.iter().all(|r| r.result.is_ok()));
//! ```

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use crate::anchor_health::{HealthWindow, EndpointOutcome, classify_http_status};
use crate::errors::AnchorKitError;
use crate::validation::validate_endpoint_url;

// ---------------------------------------------------------------------------
// ProbeKind
// ---------------------------------------------------------------------------

/// The type of synthetic probe to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeKind {
    /// Basic connectivity and latency check against the anchor root URL.
    Ping,
    /// Fetch and validate `/.well-known/stellar.toml`.
    StellarToml,
    /// Fetch the SEP-6 `/info` endpoint and verify the response shape.
    Sep6Info,
    /// Fetch the SEP-24 `/info` endpoint and verify the response shape.
    Sep24Info,
    /// Operator-defined probe with a custom label.
    Custom(String),
}

impl ProbeKind {
    /// Human-readable label.
    pub fn label(&self) -> &str {
        match self {
            ProbeKind::Ping        => "ping",
            ProbeKind::StellarToml => "stellar_toml",
            ProbeKind::Sep6Info    => "sep6_info",
            ProbeKind::Sep24Info   => "sep24_info",
            ProbeKind::Custom(l)   => l.as_str(),
        }
    }
}

// ---------------------------------------------------------------------------
// ProbeConfig
// ---------------------------------------------------------------------------

/// Configuration for one synthetic probe.
#[derive(Clone, Debug)]
pub struct ProbeConfig {
    /// Unique identifier for this probe (used to correlate results).
    pub id: u64,
    /// Kind of probe to execute.
    pub kind: ProbeKind,
    /// Target URL or identifier for this probe.
    pub target: String,
    /// Maximum allowed latency in milliseconds.
    /// A successful probe that exceeds this threshold is classified as a
    /// [`ProbeOutcome::SlowSuccess`] rather than a full success.
    pub latency_threshold_ms: u64,
}

impl ProbeConfig {
    /// Create a probe configuration with a default latency threshold of 2 000 ms.
    ///
    /// The `target` is validated with the shared repository endpoint validator
    /// ([`validate_endpoint_url`]) so malformed schemes, embedded credentials,
    /// and control characters (including CR/LF) are rejected before any network
    /// work begins.  Valid endpoints are accepted unchanged.
    ///
    /// Returns `Err` if `target` is blank (empty or whitespace-only) or fails
    /// endpoint validation, using the same
    /// [`AnchorKitError::invalid_endpoint_format`] error returned by the shared
    /// domain validator so callers see a consistent error type.
    pub fn new(id: u64, kind: ProbeKind, target: impl Into<String>) -> Result<Self, AnchorKitError> {
        let target = target.into();
        if target.trim().is_empty() {
            return Err(AnchorKitError::invalid_endpoint_format());
        }
        validate_endpoint_url(&target).map_err(|_| AnchorKitError::invalid_endpoint_format())?;
        Ok(ProbeConfig {
            id,
            kind,
            target,
            latency_threshold_ms: 2_000,
        })
    }

    /// Set the latency threshold (builder-style).
    ///
    /// Returns `Err(AnchorKitError::validation_error)` when `ms` is zero
    /// (a zero threshold immediately classifies every probe as slow) or when
    /// `ms` exceeds [`MAX_PROBE_TIMEOUT_MS`] (prevents silent overflow when
    /// the value is later converted to a `std::time::Duration`).
    pub fn with_latency_threshold(mut self, ms: u64) -> Result<Self, AnchorKitError> {
        if ms == 0 {
            return Err(AnchorKitError::validation_error(
                "probe latency threshold must be greater than zero",
            ));
        }
        if ms > MAX_PROBE_TIMEOUT_MS {
            return Err(AnchorKitError::validation_error(
                &alloc::format!(
                    "probe latency threshold {ms} ms exceeds maximum {MAX_PROBE_TIMEOUT_MS} ms"
                ),
            ));
        }
        self.latency_threshold_ms = ms;
        Ok(self)
    }
}

/// Maximum allowed probe latency threshold in milliseconds (1 hour).
///
/// Values above this are rejected by [`ProbeConfig::with_latency_threshold`]
/// to prevent silent overflow when the threshold is converted to a
/// `std::time::Duration` or compared against a `u32` timer register.
pub const MAX_PROBE_TIMEOUT_MS: u64 = 3_600_000;

// ---------------------------------------------------------------------------
// ProbeOutcome
// ---------------------------------------------------------------------------

/// Fine-grained outcome of a probe execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Probe succeeded within the latency threshold.
    Success,
    /// Probe succeeded but took longer than `latency_threshold_ms`.
    SlowSuccess,
    /// Probe failed (network error, unexpected response, timeout, etc.).
    Failure(String),
}

impl ProbeOutcome {
    /// Returns `true` for `Success` and `SlowSuccess`.
    pub fn is_ok(&self) -> bool {
        matches!(self, ProbeOutcome::Success | ProbeOutcome::SlowSuccess)
    }

    /// Convert to the coarser [`EndpointOutcome`] used by health scoring.
    pub fn to_endpoint_outcome(&self) -> EndpointOutcome {
        match self {
            ProbeOutcome::Success | ProbeOutcome::SlowSuccess => EndpointOutcome::Success,
            ProbeOutcome::Failure(r) => EndpointOutcome::Failure(r.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// ProbeResult
// ---------------------------------------------------------------------------

/// The measured result of executing one probe.
#[derive(Clone, Debug)]
pub struct ProbeResult {
    /// ID of the probe this result belongs to.
    pub probe_id: u64,
    /// Outcome of the probe.
    pub outcome: ProbeOutcome,
    /// Observed round-trip latency in milliseconds.
    pub latency_ms: u64,
}

impl ProbeResult {
    /// Convenience constructor for a successful probe.
    pub fn success(probe_id: u64, latency_ms: u64) -> Self {
        ProbeResult {
            probe_id,
            outcome: ProbeOutcome::Success,
            latency_ms,
        }
    }

    /// Convenience constructor for a failed probe.
    pub fn failure(probe_id: u64, latency_ms: u64, reason: impl Into<String>) 

/* … truncated 6799 chars — edit only what you need near the top … */
