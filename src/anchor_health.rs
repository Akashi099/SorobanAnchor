//! Off-chain helpers for anchor health monitoring and proof-of-possession.
//!
//! # Proof-of-possession protocol
//!
//! The proof-of-possession (PoP) flow lets an anchor prove it controls the
//! endpoint it advertises without requiring a live HTTP round-trip from the
//! contract itself (Soroban contracts cannot make outbound HTTP calls).
//!
//! ## Flow
//!
//! ```text
//! 1. Anchor publishes a challenge nonce in its stellar.toml:
//!       ANCHOR_PROOF_CHALLENGE = "<hex-encoded 32-byte nonce>"
//!
//! 2. Off-chain monitor fetches the challenge and computes:
//!       proof_hash = SHA-256(challenge_bytes || endpoint_bytes)
//!
//! 3. Anchor calls AnchorKitContract::register_endpoint_proof(
//!       anchor, endpoint, proof_hash)
//!    on-chain, binding its Stellar identity to the endpoint.
//!
//! 4. Any verifier calls AnchorKitContract::verify_endpoint_proof(
//!       anchor, proof_hash)
//!    to confirm the hash matches and mark the record as verified.
//! ```
//!
//! The helpers in this module implement step 2 and provide utilities for
//! health-event recording that wrap the contract calls.
//!
//! # Health scoring model
//!
//! Beyond the simple uptime counter stored on-chain, the off-chain scoring
//! model combines four signals into a single 0–100 composite score:
//!
//! | Signal              | Weight | Description                                  |
//! |---------------------|--------|----------------------------------------------|
//! | Success rate        |  0.40  | Fraction of calls that succeeded             |
//! | Latency             |  0.25  | How close p50 latency is to a target floor   |
//! | Routing failures    |  0.20  | Fraction of routing attempts that failed     |
//! | Recovery behaviour  |  0.15  | How quickly the anchor recovered after drops |
//!
//! Trend analysis compares the latest window against the previous one and
//! classifies the direction as `Improving`, `Stable`, or `Degrading`.

extern crate alloc;

use alloc::string::String;

// ---------------------------------------------------------------------------
// Proof-of-possession helpers
// ---------------------------------------------------------------------------

/// Compute the proof-of-possession hash that an anchor must submit to
/// [`AnchorKitContract::register_endpoint_proof`].
///
/// `proof_hash = SHA-256(challenge_bytes || endpoint_bytes)`
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::compute_pop_hash;
///
/// let challenge = b"deadbeefdeadbeefdeadbeefdeadbeef";
/// let endpoint  = "https://anchor.example.com";
/// let hash = compute_pop_hash(challenge, endpoint);
/// assert_eq!(hash.len(), 32);
/// ```
pub fn compute_pop_hash(challenge: &[u8], endpoint: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(challenge);
    hasher.update(endpoint.as_bytes());
    hasher.finalize().into()
}

/// Verify that a stored proof hash matches the expected value recomputed from
/// `challenge` and `endpoint`.
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::{compute_pop_hash, verify_pop_challenge};
///
/// let challenge = b"deadbeefdeadbeefdeadbeefdeadbeef";
/// let endpoint  = "https://anchor.example.com";
/// let hash = compute_pop_hash(challenge, endpoint);
///
/// assert!(verify_pop_challenge(&hash, challenge, endpoint));
/// assert!(!verify_pop_challenge(&hash, b"wrongchallenge00wrongchallenge00", endpoint));
/// assert!(!verify_pop_challenge(&hash, challenge, "https://other.example.com"));
/// ```
pub fn verify_pop_challenge(
    stored_hash: &[u8; 32],
    challenge: &[u8],
    endpoint: &str,
) -> bool {
    let expected = compute_pop_hash(challenge, endpoint);
    constant_time_eq(stored_hash, &expected)
}

/// Constant-time byte-slice equality check.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Health event helpers
// ---------------------------------------------------------------------------

/// Outcome of a single anchor endpoint interaction, used as input to
/// health event recording.
#[derive(Debug, Clone, PartialEq)]
pub enum EndpointOutcome {
    /// The call succeeded and returned a valid response.
    Success,
    /// The call failed (network error, timeout, invalid response, etc.).
    Failure(String),
}

impl EndpointOutcome {
    /// Returns `true` for [`EndpointOutcome::Success`].
    pub fn is_success(&self) -> bool {
        matches!(self, EndpointOutcome::Success)
    }

    /// Returns the failure reason, or an empty string for success.
    pub fn failure_reason(&self) -> &str {
        match self {
            EndpointOutcome::Success => "",
            EndpointOutcome::Failure(r) => r.as_str(),
        }
    }
}

/// Classify a raw HTTP status code into an [`EndpointOutcome`].
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::{classify_http_status, EndpointOutcome};
///
/// assert_eq!(classify_http_status(200), EndpointOutcome::Success);
/// assert!(matches!(classify_http_status(500), EndpointOutcome::Failure(_)));
/// ```
pub fn classify_http_status(status: u16) -> EndpointOutcome {
    if (200..300).contains(&status) {
        EndpointOutcome::Success
    } else {
        EndpointOutcome::Failure(alloc::format!("HTTP {status}"))
    }
}

/// Compute an uptime percentage (0.0–100.0) from raw success/failure counts.
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::uptime_percent;
///
/// assert_eq!(uptime_percent(9, 1), 90.0_f64);
/// assert_eq!(uptime_percent(0, 0), 0.0_f64);
/// ```
pub fn uptime_percent(success: u64, failure: u64) -> f64 {
    let total = success + failure;
    if total == 0 {
        return 0.0;
    }
    (success as f64 / total as f64) * 100.0
}

/// Convert basis-point uptime (0–10 000) to a human-readable percentage string.
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::bps_to_percent_str;
///
/// assert_eq!(bps_to_percent_str(10_000), "100.00%");
/// assert_eq!(bps_to_percent_str(9_950),  "99.50%");
/// ```
pub fn bps_to_percent_str(bps: u32) -> String {
    let whole = bps / 100;
    let frac = bps % 100;
    alloc::format!("{whole}.{frac:02}%")
}

// ---------------------------------------------------------------------------
// Health scoring model
// ---------------------------------------------------------------------------

/// Scoring weights for the composite health score.
/// All weights must sum to 1.0.
pub const WEIGHT_SUCCESS_RATE:     f64 = 0.40;
pub const WEIGHT_LATENCY:          f64 = 0.25;
pub const WEIGHT_ROUTING_FAILURES: f64 = 0.20;
pub const WEIGHT_RECOVERY:         f64 = 0.15;

/// Target latency floor in milliseconds. Anchors at or below this are given
/// the maximum latency sub-score.
pub const LATENCY_TARGET_MS: f64 = 500.0;

/// Latency ceiling in milliseconds. At or above this the latency sub-score is 0.
pub const LATENCY_CEILING_MS: f64 = 10_000.0;

/// Minimum score change (absolute, 0–100 scale) that is classified as a trend.
/// Changes smaller than this threshold are considered `Stable`.
pub const TREND_THRESHOLD: f64 = 3.0;

/// The direction of change in an anchor's health score between two consecutive
/// observation windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthTrend {
    /// Score improved by more than [`TREND_THRESHOLD`] points.
    Improving,
    /// Score changed by less than [`TREND_THRESHOLD`] points (either direction).
    Stable,
    /// Score degraded by more than [`TREND_THRESHOLD`] points.
    Degrading,
}

impl HealthTrend {
    /// Returns a human-readable label for the trend direction.
    pub fn label(&self) -> &'static str {
        match self {
            HealthTrend::Improving => "improving",
            HealthTrend::Stable    => "stable",
            HealthTrend::Degrading => "degrading",
        }
    }
}

/// A time-windowed bucket of raw health observations.
///
/// Callers accumulate one of these per monitoring window (e.g. 5 min, 1 hour)
/// and pass a slice of recent windows to the scoring functions.
#[derive(Debug, Clone)]
pub struct HealthWindow {
    /// Seconds since UNIX epoch when this window started.
    pub started_at: u64,
    /// Seconds since UNIX epoch when this window ended (or the current time
    /// if the window is still open).
    pub ended_at: u64,
    /// Number of endpoint calls that succeeded in this window.
    pub success_count: u64,
    /// Number of endpoint calls that failed in this window.
    pub failure_count: u64,
    /// Observed p50 latency for successful calls in milliseconds.
    /// Pass `0` when no successful calls were recorded.
    pub p50_latency_ms: f64,
    /// Number of routing-level failures (quote fetch, discovery) in this window.
    pub routing_failure_count: u64,
    /// Total routing attempts in this window.
    pub routing_attempt_count: u64,
    /// Seconds the anchor was unavailable before recovering in this window.
    /// `0` means the anchor never went down, or did not recover.
    pub recovery_time_seconds: u64,
}

impl HealthWindow {
    /// Total calls (success + failure) in this window.
    pub fn total_calls(&self) -> u64 {
        self.success_count + self.failure_count
    }

    /// Success rate in [0.0, 1.0]. Returns 0.0 for empty windows.
    pub fn success_rate(&self) -> f64 {
        let total = self.total_calls();
        if total == 0 {
            return 0.0;
        }
        self.success_count as f64 / total as f64
    }

    /// Routing success rate in [0.0, 1.0]. Returns 1.0 when no routing was attempted.
    pub fn routing_success_rate(&self) -> f64 {
        if self.routing_attempt_count == 0 {
            return 1.0;
        }
        let failures = self.routing_failure_count.min(self.routing_attempt_count);
        1.0 - (failures as f64 / self.routing_attempt_count as f64)
    }
}

/// The composite health score for a single observation window.
///
/// All sub-scores and the composite are in the range [0.0, 100.0].
#[derive(Debug, Clone)]
pub struct HealthScore {
    /// Composite weighted score (0–100).
    pub composite: f64,
    /// Sub-score derived from the endpoint success rate (0–100).
    pub success_rate_score: f64,
    /// Sub-score derived from observed p50 latency (0–100).
    pub latency_score: f64,
    /// Sub-score derived from routing failure rate (0–100).
    pub routing_score: f64,
    /// Sub-score derived from recovery behaviour (0–100).
    pub recovery_score: f64,
}

impl HealthScore {
    /// Qualitative label based on the composite score.
    ///
    /// | Range     | Label       |
    /// |-----------|-------------|
    /// | 80–100    | Healthy     |
    /// | 50–79     | Degraded    |
    /// | 0–49      | Critical    |
    pub fn label(&self) -> &'static str {
        if self.composite >= 80.0 {
            "healthy"
        } else if self.composite >= 50.0 {
            "degraded"
        } else {
            "critical"
        }
    }
}

/// A full snapshot combining the current health score with trend information
/// derived from one or more historical windows.
#[derive(Debug, Clone)]
pub struct AnchorHealthSnapshot {
    /// The composite score computed from the most recent window.
    pub current_score: HealthScore,
    /// The composite score from the previous window (if available).
    pub previous_score: Option<f64>,
    /// The computed trend direction.
    pub trend: HealthTrend,
    /// How many windows were used to compute the trend.
    pub window_count: usize,
}

impl AnchorHealthSnapshot {
    /// Returns `true` when the anchor is considered healthy (score ≥ 80).
    pub fn is_healthy(&self) -> bool {
        self.current_score.composite >= 80.0
    }

    /// Returns `true` when the anchor is degraded but not critical (50 ≤ score < 80).
    pub fn is_degraded(&self) -> bool {
        let c = self.current_score.composite;
        c >= 50.0 && c < 80.0
    }

    /// Returns `true` when the anchor is in a critical state (score < 50).
    pub fn is_critical(&self) -> bool {
        self.current_score.composite < 50.0
    }
}

// ---------------------------------------------------------------------------
// Scoring functions
// ---------------------------------------------------------------------------

/// Compute the latency sub-score from a raw p50 latency value.
///
/// Uses a linear interpolation between [`LATENCY_TARGET_MS`] (score = 100)
/// and [`LATENCY_CEILING_MS`] (score = 0). A value of `0.0` (no data) scores
/// conservatively at 50.0 to avoid rewarding missing data.
fn latency_sub_score(p50_ms: f64) -> f64 {
    if p50_ms <= 0.0 {
        return 50.0; // no data → neutral
    }
    if p50_ms <= LATENCY_TARGET_MS {
        return 100.0;
    }
    if p50_ms >= LATENCY_CEILING_MS {
        return 0.0;
    }
    let range = LATENCY_CEILING_MS - LATENCY_TARGET_MS;
    let above = p50_ms - LATENCY_TARGET_MS;
    (1.0 - above / range) * 100.0
}

/// Compute the recovery sub-score from seconds of downtime before recovery.
///
/// Recovery within 60 s scores 100; beyond 3 600 s (1 h) scores 0.
fn recovery_sub_score(recovery_time_seconds: u64) -> f64 {
    const FAST_RECOVERY_S: f64 = 60.0;
    const SLOW_RECOVERY_S: f64 = 3_600.0;

    if recovery_time_seconds == 0 {
        // No downtime recorded in this window — perfect recovery score.
        return 100.0;
    }
    let t = recovery_time_seconds as f64;
    if t <= FAST_RECOVERY_S {
        return 100.0;
    }
    if t >= SLOW_RECOVERY_S {
        return 0.0;
    }
    let range = SLOW_RECOVERY_S - FAST_RECOVERY_S;
    let above = t - FAST_RECOVERY_S;
    (1.0 - above / range) * 100.0
}

/// Compute the composite [`HealthScore`] for a single [`HealthWindow`].
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::{HealthWindow, score_window};
///
/// let window = HealthWindow {
///     started_at: 0,
///     ended_at: 300,
///     success_count: 95,
///     failure_count: 5,
///     p50_latency_ms: 200.0,
///     routing_failure_count: 1,
///     routing_attempt_count: 20,
///     recovery_time_seconds: 0,
/// };
/// let score = score_window(&window);
/// assert!(score.composite > 80.0, "composite={}", score.composite);
/// assert_eq!(score.label(), "healthy");
/// ```
pub fn score_window(w: &HealthWindow) -> HealthScore {
    let success_rate_score = w.success_rate() * 100.0;
    let latency_score      = latency_sub_score(w.p50_latency_ms);
    let routing_score      = w.routing_success_rate() * 100.0;
    let recovery_score     = recovery_sub_score(w.recovery_time_seconds);

    let composite = WEIGHT_SUCCESS_RATE     * success_rate_score
        + WEIGHT_LATENCY          * latency_score
        + WEIGHT_ROUTING_FAILURES * routing_score
        + WEIGHT_RECOVERY         * recovery_score;

    HealthScore {
        composite: composite.clamp(0.0, 100.0),
        success_rate_score,
        latency_score,
        routing_score,
        recovery_score,
    }
}

/// Compute a [`HealthTrend`] by comparing the composite scores of the two
/// most recent windows.
///
/// Returns [`HealthTrend::Stable`] when fewer than two windows are provided.
pub fn compute_trend(windows: &[HealthWindow]) -> HealthTrend {
    if windows.len() < 2 {
        return HealthTrend::Stable;
    }
    let latest   = score_window(&windows[windows.len() - 1]).composite;
    let previous = score_window(&windows[windows.len() - 2]).composite;
    let delta = latest - previous;
    if delta > TREND_THRESHOLD {
        HealthTrend::Improving
    } else if delta < -TREND_THRESHOLD {
        HealthTrend::Degrading
    } else {
        HealthTrend::Stable
    }
}

/// Build a full [`AnchorHealthSnapshot`] from a slice of recent windows.
///
/// The snapshot uses the *last* window as the current observation and the
/// second-to-last as the previous one for trend comparison.
///
/// # Arguments
///
/// * `windows` – Time-ordered slice of windows, oldest first.
///   Pass an empty slice to get a zeroed snapshot with a `Stable` trend.
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::{
///     HealthWindow, build_health_snapshot, HealthTrend,
/// };
///
/// let good = HealthWindow {
///     started_at: 0, ended_at: 300,
///     success_count: 99, failure_count: 1,
///     p50_latency_ms: 100.0,
///     routing_failure_count: 0, routing_attempt_count: 10,
///     recovery_time_seconds: 0,
/// };
/// let bad = HealthWindow {
///     started_at: 300, ended_at: 600,
///     success_count: 40, failure_count: 60,
///     p50_latency_ms: 8000.0,
///     routing_failure_count: 8, routing_attempt_count: 10,
///     recovery_time_seconds: 1800,
/// };
/// let snapshot = build_health_snapshot(&[good, bad]);
/// assert!(snapshot.is_critical());
/// assert_eq!(snapshot.trend, HealthTrend::Degrading);
/// ```
pub fn build_health_snapshot(windows: &[HealthWindow]) -> AnchorHealthSnapshot {
    if windows.is_empty() {
        let zero_window = HealthWindow {
            started_at: 0, ended_at: 0,
            success_count: 0, failure_count: 0,
            p50_latency_ms: 0.0,
            routing_failure_count: 0, routing_attempt_count: 0,
            recovery_time_seconds: 0,
        };
        return AnchorHealthSnapshot {
            current_score: score_window(&zero_window),
            previous_score: None,
            trend: HealthTrend::Stable,
            window_count: 0,
        };
    }

    let current_score = score_window(&windows[windows.len() - 1]);
    let previous_score = if windows.len() >= 2 {
        Some(score_window(&windows[windows.len() - 2]).composite)
    } else {
        None
    };
    let trend = compute_trend(windows);

    AnchorHealthSnapshot {
        current_score,
        previous_score,
        trend,
        window_count: windows.len(),
    }
}

/// Aggregate multiple windows into a single summary window.
///
/// Useful when an operator wants to compute a single representative score
/// across a longer time range (e.g. the last 24 windows combined).
pub fn aggregate_windows(windows: &[HealthWindow]) -> HealthWindow {
    let mut agg = HealthWindow {
        started_at: windows.first().map(|w| w.started_at).unwrap_or(0),
        ended_at:   windows.last().map(|w| w.ended_at).unwrap_or(0),
        success_count: 0,
        failure_count: 0,
        p50_latency_ms: 0.0,
        routing_failure_count: 0,
        routing_attempt_count: 0,
        recovery_time_seconds: 0,
    };

    let mut latency_sum = 0.0f64;
    let mut latency_count = 0u64;

    for w in windows {
        agg.success_count         += w.success_count;
        agg.failure_count         += w.failure_count;
        agg.routing_failure_count += w.routing_failure_count;
        agg.routing_attempt_count += w.routing_attempt_count;
        agg.recovery_time_seconds += w.recovery_time_seconds;
        if w.p50_latency_ms > 0.0 {
            latency_sum   += w.p50_latency_ms;
            latency_count += 1;
        }
    }

    if latency_count > 0 {
        agg.p50_latency_ms = latency_sum / latency_count as f64;
    }

    agg
}

// ---------------------------------------------------------------------------
// HealthWindowBuilder — convenience builder for tests and monitoring loops
// ---------------------------------------------------------------------------

/// A fluent builder for constructing a [`HealthWindow`] in tests or when
/// the caller only has a subset of the available signals.
pub struct HealthWindowBuilder {
    inner: HealthWindow,
}

impl HealthWindowBuilder {
    /// Start a new builder with the given time range and zeroed counters.
    pub fn new(started_at: u64, ended_at: u64) -> Self {
        HealthWindowBuilder {
            inner: HealthWindow {
                started_at,
                ended_at,
                success_count: 0,
                failure_count: 0,
                p50_latency_ms: 0.0,
                routing_failure_count: 0,
                routing_attempt_count: 0,
                recovery_time_seconds: 0,
            },
        }
    }

    pub fn successes(mut self, n: u64) -> Self { self.inner.success_count = n; self }
    pub fn failures(mut self, n: u64) -> Self  { self.inner.failure_count = n; self }
    pub fn p50_latency(mut self, ms: f64) -> Self { self.inner.p50_latency_ms = ms; self }
    pub fn routing(mut self, attempts: u64, failures: u64) -> Self {
        self.inner.routing_attempt_count = attempts;
        self.inner.routing_failure_count = failures;
        self
    }
    pub fn recovery(mut self, seconds: u64) -> Self {
        self.inner.recovery_time_seconds = seconds;
        self
    }

    /// Consume the builder and return the constructed [`HealthWindow`].
    pub fn build(self) -> HealthWindow { self.inner }
}

// ---------------------------------------------------------------------------
// Issue #664: Health report export
// ---------------------------------------------------------------------------

/// Supported serialization formats for [`export_health_report`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HealthReportFormat {
    /// Structured text with `key: value` pairs — easy to grep and human-readable.
    Text,
    /// JSON object — suitable for ingestion by dashboards and incident tooling.
    Json,
}

/// A complete exportable health report for a single anchor.
///
/// Contains the current composite score, sub-scores, trend, window statistics,
/// and an overall label suitable for use in dashboards and alerting pipelines.
///
/// The `maintenance_active` flag indicates whether a scheduled maintenance
/// window is in effect for this anchor.  When `true`, health degradation
/// alerts should be suppressed because the outage is planned.
///
/// Obtain one via [`build_health_report`] or [`build_health_report_with_maintenance`]
/// and serialise it with [`export_health_report`].
#[derive(Clone, Debug)]
pub struct AnchorHealthReport {
    /// Identifier for the anchor (e.g. domain or contract address).
    pub anchor_id: String,
    /// How many observation windows were included.
    pub window_count: usize,
    /// Composite score (0–100) for the most recent window.
    pub composite_score: f64,
    /// Sub-score derived from the success rate (0–100).
    pub success_rate_score: f64,
    /// Sub-score derived from p50 latency (0–100).
    pub latency_score: f64,
    /// Sub-score derived from routing failure rate (0–100).
    pub routing_score: f64,
    /// Sub-score derived from recovery behaviour (0–100).
    pub recovery_score: f64,
    /// Qualitative label: `"healthy"`, `"degraded"`, or `"critical"`.
    pub label: String,
    /// Trend direction: `"improving"`, `"stable"`, or `"degrading"`.
    pub trend: String,
    /// Composite score from the previous window, if available.
    pub previous_composite: Option<f64>,
    /// Whether the anchor is currently inside a scheduled maintenance window.
    ///
    /// When `true`, degraded or critical scores should **not** trigger alerts
    /// because the service disruption is intentional and planned.
    pub maintenance_active: bool,
}

impl AnchorHealthReport {
    /// Returns `true` when the composite score is ≥ 80 (healthy range).
    pub fn is_healthy(&self) -> bool { self.composite_score >= 80.0 }
    /// Returns `true` when the composite score is in [50, 80) (degraded range).
    pub fn is_degraded(&self) -> bool { self.composite_score >= 50.0 && self.composite_score < 80.0 }
    /// Returns `true` when the composite score is < 50 (critical range).
    pub fn is_critical(&self) -> bool { self.composite_score < 50.0 }
}

/// Build an [`AnchorHealthReport`] for a named anchor from its observation windows.
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::{HealthWindow, build_health_report, export_health_report, HealthReportFormat};
///
/// let w = HealthWindow {
///     started_at: 0, ended_at: 300,
///     success_count: 99, failure_count: 1,
///     p50_latency_ms: 100.0,
///     routing_failure_count: 0, routing_attempt_count: 10,
///     recovery_time_seconds: 0,
/// };
/// let report = build_health_report("anchor.example.com", &[w]);
/// assert_eq!(report.label, "healthy");
/// let text = export_health_report(&report, HealthReportFormat::Text);
/// assert!(text.contains("anchor_id: anchor.example.com"));
/// let json = export_health_report(&report, HealthReportFormat::Json);
/// assert!(json.contains("\"anchor_id\""));
/// ```
pub fn build_health_report(anchor_id: &str, windows: &[HealthWindow]) -> AnchorHealthReport {
    let snapshot = build_health_snapshot(windows);
    let score = &snapshot.current_score;
    AnchorHealthReport {
        anchor_id: anchor_id.into(),
        window_count: snapshot.window_count,
        composite_score: score.composite,
        success_rate_score: score.success_rate_score,
        latency_score: score.latency_score,
        routing_score: score.routing_score,
        recovery_score: score.recovery_score,
        label: score.label().into(),
        trend: snapshot.trend.label().into(),
        previous_composite: snapshot.previous_score,
        maintenance_active: false,
    }
}

/// Build an [`AnchorHealthReport`] with an explicit maintenance-window flag.
///
/// Pass `maintenance_active = true` when the anchor is known to be inside a
/// scheduled maintenance window.  Consumers should suppress degradation alerts
/// whenever this flag is set, because the service disruption is intentional.
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::{
///     HealthWindow, build_health_report_with_maintenance,
///     export_health_report, HealthReportFormat,
/// };
///
/// let bad = HealthWindow {
///     started_at: 0, ended_at: 300,
///     success_count: 10, failure_count: 90,
///     p50_latency_ms: 9000.0,
///     routing_failure_count: 9, routing_attempt_count: 10,
///     recovery_time_seconds: 3600,
/// };
/// // Score is critical but the outage is planned
/// let report = build_health_report_with_maintenance("anchor.example.com", &[bad], true);
/// assert!(report.maintenance_active);
/// assert_eq!(report.label, "critical");
/// let text = export_health_report(&report, HealthReportFormat::Text);
/// assert!(text.contains("maintenance_active: true"));
/// ```
pub fn build_health_report_with_maintenance(
    anchor_id: &str,
    windows: &[HealthWindow],
    maintenance_active: bool,
) -> AnchorHealthReport {
    let mut report = build_health_report(anchor_id, windows);
    report.maintenance_active = maintenance_active;
    report
}

/// Returns `true` when the report should suppress degradation alerts.
///
/// Alerts are suppressed when the anchor is currently inside a maintenance
/// window, regardless of the raw health score.
pub fn should_suppress_alert(report: &AnchorHealthReport) -> bool {
    report.maintenance_active
}

/// Serialize an [`AnchorHealthReport`] into the requested [`HealthReportFormat`].
///
/// - `HealthReportFormat::Text` — produces a multi-line `key: value` string.
/// - `HealthReportFormat::Json` — produces a compact JSON object.
pub fn export_health_report(report: &AnchorHealthReport, format: HealthReportFormat) -> String {
    match format {
        HealthReportFormat::Text => export_health_report_text(report),
        HealthReportFormat::Json => export_health_report_json(report),
    }
}

fn export_health_report_text(r: &AnchorHealthReport) -> String {
    let prev = match r.previous_composite {
        Some(p) => alloc::format!("{p:.2}"),
        None => "n/a".into(),
    };
    alloc::format!(
        "anchor_id: {}\n\
         window_count: {}\n\
         composite_score: {:.2}\n\
         success_rate_score: {:.2}\n\
         latency_score: {:.2}\n\
         routing_score: {:.2}\n\
         recovery_score: {:.2}\n\
         label: {}\n\
         trend: {}\n\
         previous_composite: {}\n\
         maintenance_active: {}\n",
        r.anchor_id,
        r.window_count,
        r.composite_score,
        r.success_rate_score,
        r.latency_score,
        r.routing_score,
        r.recovery_score,
        r.label,
        r.trend,
        prev,
        r.maintenance_active,
    )
}

fn export_health_report_json(r: &AnchorHealthReport) -> String {
    let prev = match r.previous_composite {
        Some(p) => alloc::format!("{p:.2}"),
        None => "null".into(),
    };
    alloc::format!(
        "{{\
\"anchor_id\":\"{}\",\
\"window_count\":{},\
\"composite_score\":{:.2},\
\"success_rate_score\":{:.2},\
\"latency_score\":{:.2},\
\"routing_score\":{:.2},\
\"recovery_score\":{:.2},\
\"label\":\"{}\",\
\"trend\":\"{}\",\
\"previous_composite\":{},\
\"maintenance_active\":{}\
}}",
        r.anchor_id,
        r.window_count,
        r.composite_score,
        r.success_rate_score,
        r.latency_score,
        r.routing_score,
        r.recovery_score,
        r.label,
        r.trend,
        prev,
        r.maintenance_active,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── PoP helpers ──────────────────────────────────────────────────────────

    #[test]
    fn compute_pop_hash_is_deterministic() {
        let challenge = b"test_challenge_bytes_32_chars___";
        let endpoint = "https://anchor.example.com";
        let h1 = compute_pop_hash(challenge, endpoint);
        let h2 = compute_pop_hash(challenge, endpoint);
        assert_eq!(h1, h2);
    }

    #[test]
    fn compute_pop_hash_differs_on_different_inputs() {
        let challenge = b"test_challenge_bytes_32_chars___";
        let h1 = compute_pop_hash(challenge, "https://anchor.example.com");
        let h2 = compute_pop_hash(challenge, "https://other.example.com");
        assert_ne!(h1, h2);

        let h3 = compute_pop_hash(b"different_challenge_bytes_______", "https://anchor.example.com");
        assert_ne!(h1, h3);
    }

    #[test]
    fn verify_pop_challenge_success() {
        let challenge = b"test_challenge_bytes_32_chars___";
        let endpoint = "https://anchor.example.com";
        let hash = compute_pop_hash(challenge, endpoint);
        assert!(verify_pop_challenge(&hash, challenge, endpoint));
    }

    #[test]
    fn verify_pop_challenge_wrong_challenge_fails() {
        let challenge = b"test_challenge_bytes_32_chars___";
        let endpoint = "https://anchor.example.com";
        let hash = compute_pop_hash(challenge, endpoint);
        assert!(!verify_pop_challenge(&hash, b"wrong_challenge_bytes_32_chars__", endpoint));
    }

    #[test]
    fn verify_pop_challenge_wrong_endpoint_fails() {
        let challenge = b"test_challenge_bytes_32_chars___";
        let endpoint = "https://anchor.example.com";
        let hash = compute_pop_hash(challenge, endpoint);
        assert!(!verify_pop_challenge(&hash, challenge, "https://evil.example.com"));
    }

    // ── Endpoint outcome helpers ─────────────────────────────────────────────

    #[test]
    fn classify_http_status_success_range() {
        for code in [200u16, 201, 204, 299] {
            assert_eq!(classify_http_status(code), EndpointOutcome::Success, "code={code}");
        }
    }

    #[test]
    fn classify_http_status_failure_range() {
        for code in [400u16, 404, 500, 503] {
            assert!(matches!(classify_http_status(code), EndpointOutcome::Failure(_)), "code={code}");
        }
    }

    #[test]
    fn uptime_percent_calculations() {
        assert_eq!(uptime_percent(0, 0), 0.0);
        assert_eq!(uptime_percent(1, 0), 100.0);
        assert_eq!(uptime_percent(0, 1), 0.0);
        assert!((uptime_percent(9, 1) - 90.0).abs() < 1e-9);
        assert!((uptime_percent(1, 1) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn bps_to_percent_str_formatting() {
        assert_eq!(bps_to_percent_str(10_000), "100.00%");
        assert_eq!(bps_to_percent_str(9_950), "99.50%");
        assert_eq!(bps_to_percent_str(5_000), "50.00%");
        assert_eq!(bps_to_percent_str(0), "0.00%");
        assert_eq!(bps_to_percent_str(1), "0.01%");
    }

    #[test]
    fn endpoint_outcome_helpers() {
        assert!(EndpointOutcome::Success.is_success());
        assert!(!EndpointOutcome::Failure("err".into()).is_success());
        assert_eq!(EndpointOutcome::Success.failure_reason(), "");
        assert_eq!(EndpointOutcome::Failure("timeout".into()).failure_reason(), "timeout");
    }

    // ── Latency sub-score ────────────────────────────────────────────────────

    #[test]
    fn latency_sub_score_at_target_is_100() {
        assert!((latency_sub_score(LATENCY_TARGET_MS) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn latency_sub_score_below_target_is_100() {
        assert!((latency_sub_score(100.0) - 100.0).abs() < 1e-9);
        assert!((latency_sub_score(0.001) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn latency_sub_score_at_ceiling_is_0() {
        assert!((latency_sub_score(LATENCY_CEILING_MS) - 0.0).abs() < 1e-9);
        assert!((latency_sub_score(99_999.0) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn latency_sub_score_midpoint() {
        let mid = (LATENCY_TARGET_MS + LATENCY_CEILING_MS) / 2.0;
        let score = latency_sub_score(mid);
        assert!((score - 50.0).abs() < 1e-9, "score={score}");
    }

    #[test]
    fn latency_sub_score_no_data_is_neutral() {
        assert!((latency_sub_score(0.0) - 50.0).abs() < 1e-9);
    }

    // ── Recovery sub-score ───────────────────────────────────────────────────

    #[test]
    fn recovery_sub_score_no_downtime_is_100() {
        assert!((recovery_sub_score(0) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn recovery_sub_score_fast_recovery_is_100() {
        assert!((recovery_sub_score(30) - 100.0).abs() < 1e-9);
        assert!((recovery_sub_score(60) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn recovery_sub_score_slow_recovery_is_0() {
        assert!((recovery_sub_score(3_600) - 0.0).abs() < 1e-9);
        assert!((recovery_sub_score(7_200) - 0.0).abs() < 1e-9);
    }

    // ── score_window – healthy anchor ────────────────────────────────────────

    #[test]
    fn score_healthy_anchor_exceeds_80() {
        let window = HealthWindowBuilder::new(0, 300)
            .successes(99).failures(1)
            .p50_latency(150.0)
            .routing(20, 0)
            .recovery(0)
            .build();
        let score = score_window(&window);
        assert!(
            score.composite > 80.0,
            "expected healthy score >80, got {}",
            score.composite
        );
        assert_eq!(score.label(), "healthy");
    }

    #[test]
    fn score_healthy_anchor_sub_scores_are_high() {
        let window = HealthWindowBuilder::new(0, 300)
            .successes(100).failures(0)
            .p50_latency(100.0)
            .routing(10, 0)
            .recovery(0)
            .build();
        let score = score_window(&window);
        assert!((score.success_rate_score - 100.0).abs() < 1e-9);
        assert!((score.latency_score - 100.0).abs() < 1e-9);
        assert!((score.routing_score - 100.0).abs() < 1e-9);
        assert!((score.recovery_score - 100.0).abs() < 1e-9);
        assert!((score.composite - 100.0).abs() < 1e-9);
    }

    // ── score_window – degraded anchor ───────────────────────────────────────

    #[test]
    fn score_degraded_anchor_falls_between_50_and_80() {
        let window = HealthWindowBuilder::new(0, 300)
            .successes(75).failures(25)
            .p50_latency(3000.0)
            .routing(10, 2)
            .recovery(300)
            .build();
        let score = score_window(&window);
        assert!(
            score.composite >= 50.0 && score.composite < 80.0,
            "expected degraded score in [50,80), got {}",
            score.composite
        );
        assert_eq!(score.label(), "degraded");
    }

    // ── score_window – failing anchor ────────────────────────────────────────

    #[test]
    fn score_failing_anchor_below_50() {
        let window = HealthWindowBuilder::new(0, 300)
            .successes(20).failures(80)
            .p50_latency(9000.0)
            .routing(10, 9)
            .recovery(3600)
            .build();
        let score = score_window(&window);
        assert!(
            score.composite < 50.0,
            "expected critical score <50, got {}",
            score.composite
        );
        assert_eq!(score.label(), "critical");
    }

    #[test]
    fn score_completely_failing_anchor_is_near_zero() {
        let window = HealthWindowBuilder::new(0, 300)
            .successes(0).failures(100)
            .p50_latency(0.0)   // no successful calls → neutral latency
            .routing(10, 10)
            .recovery(7200)
            .build();
        let score = score_window(&window);
        // success_rate=0 (40pts), latency=50 neutral (25pts×0=12.5), routing=0 (20pts), recovery=0 (15pts)
        // composite ≈ 0×0.4 + 50×0.25 + 0×0.2 + 0×0.15 = 12.5
        assert!(score.composite < 20.0, "score={}", score.composite);
        assert_eq!(score.label(), "critical");
    }

    // ── Trend analysis ───────────────────────────────────────────────────────

    #[test]
    fn trend_improving_when_score_rises_significantly() {
        let bad = HealthWindowBuilder::new(0, 300)
            .successes(50).failures(50).p50_latency(5000.0).routing(10, 5).recovery(600).build();
        let good = HealthWindowBuilder::new(300, 600)
            .successes(98).failures(2).p50_latency(200.0).routing(10, 0).recovery(0).build();
        let trend = compute_trend(&[bad, good]);
        assert_eq!(trend, HealthTrend::Improving);
        assert_eq!(trend.label(), "improving");
    }

    #[test]
    fn trend_degrading_when_score_falls_significantly() {
        let good = HealthWindowBuilder::new(0, 300)
            .successes(98).failures(2).p50_latency(200.0).routing(10, 0).recovery(0).build();
        let bad = HealthWindowBuilder::new(300, 600)
            .successes(30).failures(70).p50_latency(8000.0).routing(10, 8).recovery(3600).build();
        let trend = compute_trend(&[good, bad]);
        assert_eq!(trend, HealthTrend::Degrading);
        assert_eq!(trend.label(), "degrading");
    }

    #[test]
    fn trend_stable_when_change_is_small() {
        let w1 = HealthWindowBuilder::new(0, 300)
            .successes(90).failures(10).p50_latency(400.0).routing(10, 1).recovery(0).build();
        let w2 = HealthWindowBuilder::new(300, 600)
            .successes(91).failures(9).p50_latency(420.0).routing(10, 1).recovery(0).build();
        let trend = compute_trend(&[w1, w2]);
        assert_eq!(trend, HealthTrend::Stable);
        assert_eq!(trend.label(), "stable");
    }

    #[test]
    fn trend_stable_with_only_one_window() {
        let w = HealthWindowBuilder::new(0, 300)
            .successes(80).failures(20).p50_latency(600.0).build();
        assert_eq!(compute_trend(&[w]), HealthTrend::Stable);
    }

    #[test]
    fn trend_stable_with_empty_slice() {
        assert_eq!(compute_trend(&[]), HealthTrend::Stable);
    }

    // ── build_health_snapshot ────────────────────────────────────────────────

    #[test]
    fn snapshot_healthy_anchor() {
        let w = HealthWindowBuilder::new(0, 300)
            .successes(99).failures(1).p50_latency(150.0).routing(10, 0).recovery(0).build();
        let snap = build_health_snapshot(&[w]);
        assert!(snap.is_healthy(), "score={}", snap.current_score.composite);
        assert!(!snap.is_degraded());
        assert!(!snap.is_critical());
        assert_eq!(snap.trend, HealthTrend::Stable);
        assert_eq!(snap.window_count, 1);
        assert!(snap.previous_score.is_none());
    }

    #[test]
    fn snapshot_degraded_anchor() {
        let w = HealthWindowBuilder::new(0, 300)
            .successes(70).failures(30).p50_latency(3000.0).routing(10, 2).recovery(180).build();
        let snap = build_health_snapshot(&[w]);
        assert!(snap.is_degraded() || snap.is_critical(),
            "score={}", snap.current_score.composite);
    }

    #[test]
    fn snapshot_critical_anchor() {
        let w = HealthWindowBuilder::new(0, 300)
            .successes(10).failures(90).p50_latency(9000.0).routing(10, 9).recovery(7200).build();
        let snap = build_health_snapshot(&[w]);
        assert!(snap.is_critical(), "score={}", snap.current_score.composite);
    }

    #[test]
    fn snapshot_with_trend_includes_previous_score() {
        let good = HealthWindowBuilder::new(0, 300)
            .successes(99).failures(1).p50_latency(150.0).routing(10, 0).recovery(0).build();
        let bad  = HealthWindowBuilder::new(300, 600)
            .successes(20).failures(80).p50_latency(9000.0).routing(10, 9).recovery(3600).build();
        let snap = build_health_snapshot(&[good, bad]);
        assert!(snap.previous_score.is_some());
        assert_eq!(snap.trend, HealthTrend::Degrading);
        assert_eq!(snap.window_count, 2);
    }

    #[test]
    fn snapshot_empty_windows_returns_zeroed() {
        let snap = build_health_snapshot(&[]);
        assert_eq!(snap.window_count, 0);
        assert_eq!(snap.trend, HealthTrend::Stable);
        assert!(snap.previous_score.is_none());
    }

    // ── aggregate_windows ────────────────────────────────────────────────────

    #[test]
    fn aggregate_sums_counts_and_averages_latency() {
        let w1 = HealthWindowBuilder::new(0, 300)
            .successes(50).failures(5).p50_latency(200.0).routing(10, 1).recovery(30).build();
        let w2 = HealthWindowBuilder::new(300, 600)
            .successes(50).failures(5).p50_latency(400.0).routing(10, 1).recovery(30).build();
        let agg = aggregate_windows(&[w1, w2]);
        assert_eq!(agg.success_count, 100);
        assert_eq!(agg.failure_count, 10);
        assert_eq!(agg.routing_failure_count, 2);
        assert_eq!(agg.routing_attempt_count, 20);
        assert_eq!(agg.recovery_time_seconds, 60);
        assert!((agg.p50_latency_ms - 300.0).abs() < 1e-9);
        assert_eq!(agg.started_at, 0);
        assert_eq!(agg.ended_at, 600);
    }

    // ── HealthWindowBuilder ──────────────────────────────────────────────────

    #[test]
    fn builder_defaults_are_zero() {
        let w = HealthWindowBuilder::new(100, 200).build();
        assert_eq!(w.success_count, 0);
        assert_eq!(w.failure_count, 0);
        assert_eq!(w.p50_latency_ms, 0.0);
        assert_eq!(w.routing_attempt_count, 0);
        assert_eq!(w.routing_failure_count, 0);
        assert_eq!(w.recovery_time_seconds, 0);
        assert_eq!(w.started_at, 100);
        assert_eq!(w.ended_at, 200);
    }

    // ── HealthWindow helpers ─────────────────────────────────────────────────

    #[test]
    fn health_window_success_rate() {
        let w = HealthWindowBuilder::new(0, 1).successes(9).failures(1).build();
        assert!((w.success_rate() - 0.9).abs() < 1e-9);
    }

    #[test]
    fn health_window_success_rate_empty_is_zero() {
        let w = HealthWindowBuilder::new(0, 1).build();
        assert_eq!(w.success_rate(), 0.0);
    }

    #[test]
    fn health_window_routing_success_rate_no_attempts_is_one() {
        let w = HealthWindowBuilder::new(0, 1).build();
        assert_eq!(w.routing_success_rate(), 1.0);
    }

    #[test]
    fn health_window_routing_success_rate_with_failures() {
        let w = HealthWindowBuilder::new(0, 1).routing(10, 3).build();
        assert!((w.routing_success_rate() - 0.7).abs() < 1e-9);
    }

    // ── Composite weight consistency check ───────────────────────────────────

    #[test]
    fn scoring_weights_sum_to_one() {
        let sum = WEIGHT_SUCCESS_RATE + WEIGHT_LATENCY + WEIGHT_ROUTING_FAILURES + WEIGHT_RECOVERY;
        assert!((sum - 1.0).abs() < 1e-10, "weights sum to {sum}");
    }

    // ── Maintenance-aware health reporting ───────────────────────────────────

    #[test]
    fn build_health_report_default_maintenance_active_is_false() {
        let w = HealthWindowBuilder::new(0, 300)
            .successes(99).failures(1).p50_latency(100.0).routing(10, 0).recovery(0).build();
        let report = build_health_report("anchor.example.com", &[w]);
        assert!(!report.maintenance_active);
    }

    #[test]
    fn build_health_report_with_maintenance_sets_flag_true() {
        let w = HealthWindowBuilder::new(0, 300)
            .successes(99).failures(1).p50_latency(100.0).routing(10, 0).recovery(0).build();
        let report = build_health_report_with_maintenance("anchor.example.com", &[w], true);
        assert!(report.maintenance_active);
    }

    #[test]
    fn build_health_report_with_maintenance_false_matches_default() {
        let w = HealthWindowBuilder::new(0, 300)
            .successes(99).failures(1).p50_latency(100.0).routing(10, 0).recovery(0).build();
        let default_report = build_health_report("anchor.example.com", &[w.clone()]);
        let explicit_report = build_health_report_with_maintenance("anchor.example.com", &[w], false);
        assert_eq!(default_report.maintenance_active, explicit_report.maintenance_active);
        assert_eq!(default_report.composite_score, explicit_report.composite_score);
    }

    #[test]
    fn should_suppress_alert_returns_true_during_maintenance() {
        let w = HealthWindowBuilder::new(0, 300)
            .successes(10).failures(90).p50_latency(9000.0).routing(10, 9).recovery(3600).build();
        let report = build_health_report_with_maintenance("anchor.example.com", &[w], true);
        assert!(report.is_critical()); // score is bad…
        assert!(should_suppress_alert(&report)); // …but alert is suppressed
    }

    #[test]
    fn should_suppress_alert_returns_false_outside_maintenance() {
        let w = HealthWindowBuilder::new(0, 300)
            .successes(10).failures(90).p50_latency(9000.0).routing(10, 9).recovery(3600).build();
        let report = build_health_report_with_maintenance("anchor.example.com", &[w], false);
        assert!(report.is_critical());
        assert!(!should_suppress_alert(&report)); // not suppressed — real incident
    }

    #[test]
    fn export_text_includes_maintenance_active_true() {
        let w = HealthWindowBuilder::new(0, 300).successes(80).failures(20).build();
        let report = build_health_report_with_maintenance("anc", &[w], true);
        let text = export_health_report(&report, HealthReportFormat::Text);
        assert!(text.contains("maintenance_active: true"), "text={text}");
    }

    #[test]
    fn export_text_includes_maintenance_active_false() {
        let w = HealthWindowBuilder::new(0, 300).successes(80).failures(20).build();
        let report = build_health_report_with_maintenance("anc", &[w], false);
        let text = export_health_report(&report, HealthReportFormat::Text);
        assert!(text.contains("maintenance_active: false"), "text={text}");
    }

    #[test]
    fn export_json_includes_maintenance_active_true() {
        let w = HealthWindowBuilder::new(0, 300).successes(80).failures(20).build();
        let report = build_health_report_with_maintenance("anc", &[w], true);
        let json = export_health_report(&report, HealthReportFormat::Json);
        assert!(json.contains("\"maintenance_active\":true"), "json={json}");
    }

    #[test]
    fn export_json_includes_maintenance_active_false() {
        let w = HealthWindowBuilder::new(0, 300).successes(80).failures(20).build();
        let report = build_health_report_with_maintenance("anc", &[w], false);
        let json = export_health_report(&report, HealthReportFormat::Json);
        assert!(json.contains("\"maintenance_active\":false"), "json={json}");
    }

    #[test]
    fn maintenance_active_does_not_alter_scores() {
        let w = HealthWindowBuilder::new(0, 300)
            .successes(99).failures(1).p50_latency(150.0).routing(10, 0).recovery(0).build();
        let normal = build_health_report("anc", &[w.clone()]);
        let maint  = build_health_report_with_maintenance("anc", &[w], true);
        assert!((normal.composite_score - maint.composite_score).abs() < 1e-9);
        assert_eq!(normal.label, maint.label);
        assert_eq!(normal.trend, maint.trend);
    }
}


// ---------------------------------------------------------------------------
// Service-Level Objectives (SLOs)
// ---------------------------------------------------------------------------

/// A configurable SLO target for a single anchor.
///
/// Each threshold is a value in [0.0, 100.0] that represents the *minimum
/// acceptable* score on the corresponding dimension.  A value of `None` means
/// that dimension is not included in SLO evaluation.
///
/// At least one threshold must be set; constructing an all-`None` target
/// returns an error from [`SloTarget::validate`].
#[derive(Clone, Debug)]
pub struct SloTarget {
    /// Minimum acceptable composite health score (0–100).
    pub min_composite: Option<f64>,
    /// Minimum acceptable success-rate sub-score (0–100).
    pub min_success_rate_score: Option<f64>,
    /// Minimum acceptable latency sub-score (0–100).
    pub min_latency_score: Option<f64>,
    /// Minimum acceptable routing sub-score (0–100).
    pub min_routing_score: Option<f64>,
    /// Minimum acceptable recovery sub-score (0–100).
    pub min_recovery_score: Option<f64>,
}

impl SloTarget {
    /// Construct a simple SLO that only checks the composite score.
    pub fn composite_only(min_composite: f64) -> Self {
        SloTarget {
            min_composite: Some(min_composite),
            min_success_rate_score: None,
            min_latency_score: None,
            min_routing_score: None,
            min_recovery_score: None,
        }
    }

    /// Validate that this SLO target is well-formed:
    /// - Every configured threshold must be in [0.0, 100.0].
    /// - At least one threshold must be set.
    ///
    /// Returns `Err(InvalidSloConfig)` with a descriptive context string on failure.
    pub fn validate(&self) -> Result<(), crate::errors::AnchorKitError> {
        let mut any = false;
        let checks: [(&str, Option<f64>); 5] = [
            ("min_composite",         self.min_composite),
            ("min_success_rate_score", self.min_success_rate_score),
            ("min_latency_score",     self.min_latency_score),
            ("min_routing_score",     self.min_routing_score),
            ("min_recovery_score",    self.min_recovery_score),
        ];
        for (name, val) in checks {
            if let Some(v) = val {
                any = true;
                if !(0.0..=100.0).contains(&v) {
                    return Err(crate::errors::AnchorKitError::invalid_slo_config(
                        &alloc::format!("{name}={v:.2} is out of range [0, 100]"),
                    ));
                }
            }
        }
        if !any {
            return Err(crate::errors::AnchorKitError::invalid_slo_config(
                "at least one SLO threshold must be configured",
            ));
        }
        Ok(())
    }
}

/// The result of evaluating a single [`SloTarget`] against a [`HealthScore`].
#[derive(Clone, Debug)]
pub struct SloEvaluation {
    /// `true` when all configured thresholds are met.
    pub satisfied: bool,
    /// Human-readable list of individual threshold outcomes.
    pub violations: alloc::vec::Vec<SloViolationDetail>,
}

impl SloEvaluation {
    /// Returns `true` when no thresholds were violated.
    pub fn is_satisfied(&self) -> bool {
        self.satisfied
    }
}

/// Details about a single threshold violation.
#[derive(Clone, Debug)]
pub struct SloViolationDetail {
    /// Name of the dimension that was violated (e.g. `"min_composite"`).
    pub dimension: String,
    /// The configured minimum threshold.
    pub threshold: f64,
    /// The actual observed value.
    pub actual: f64,
}

impl SloViolationDetail {
    /// Format as a human-readable string.
    pub fn describe(&self) -> String {
        alloc::format!(
            "{}: required>={:.2} actual={:.2}",
            self.dimension, self.threshold, self.actual
        )
    }
}

/// Evaluate a [`SloTarget`] against a [`HealthScore`].
///
/// Every configured threshold is checked; all violations are collected rather
/// than stopping at the first failure.
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::{
///     HealthWindowBuilder, score_window, SloTarget, evaluate_slo,
/// };
///
/// let window = HealthWindowBuilder::new(0, 300)
///     .successes(99).failures(1).p50_latency(100.0).routing(10, 0).recovery(0)
///     .build();
/// let score = score_window(&window);
/// let target = SloTarget::composite_only(80.0);
/// let eval = evaluate_slo(&target, &score);
/// assert!(eval.is_satisfied());
/// ```
pub fn evaluate_slo(target: &SloTarget, score: &HealthScore) -> SloEvaluation {
    let mut violations: alloc::vec::Vec<SloViolationDetail> = alloc::vec::Vec::new();

    let checks: [(&str, Option<f64>, f64); 5] = [
        ("min_composite",          target.min_composite,          score.composite),
        ("min_success_rate_score", target.min_success_rate_score, score.success_rate_score),
        ("min_latency_score",      target.min_latency_score,      score.latency_score),
        ("min_routing_score",      target.min_routing_score,      score.routing_score),
        ("min_recovery_score",     target.min_recovery_score,     score.recovery_score),
    ];

    for (dim, threshold_opt, actual) in checks {
        if let Some(threshold) = threshold_opt {
            if actual < threshold {
                violations.push(SloViolationDetail {
                    dimension: dim.into(),
                    threshold,
                    actual,
                });
            }
        }
    }

    SloEvaluation {
        satisfied: violations.is_empty(),
        violations,
    }
}

/// Evaluate SLO targets against all windows in a report and return the result.
///
/// When `maintenance_active` is `true` on the report the evaluation is skipped
/// and a satisfied result is returned (planned outages should not trigger SLO
/// violations).
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::{
///     HealthWindowBuilder, build_health_report, SloTarget, evaluate_slo_for_report,
/// };
///
/// let w = HealthWindowBuilder::new(0, 300)
///     .successes(50).failures(50).p50_latency(8000.0).routing(10, 8).recovery(3600)
///     .build();
/// let report = build_health_report("anchor.example.com", &[w]);
/// let target = SloTarget::composite_only(80.0);
/// let eval = evaluate_slo_for_report(&report, &target);
/// assert!(!eval.is_satisfied());
/// assert!(!eval.violations.is_empty());
/// ```
pub fn evaluate_slo_for_report(report: &AnchorHealthReport, target: &SloTarget) -> SloEvaluation {
    // Suppress SLO evaluation during planned maintenance
    if report.maintenance_active {
        return SloEvaluation { satisfied: true, violations: alloc::vec::Vec::new() };
    }

    let score = HealthScore {
        composite:          report.composite_score,
        success_rate_score: report.success_rate_score,
        latency_score:      report.latency_score,
        routing_score:      report.routing_score,
        recovery_score:     report.recovery_score,
    };
    evaluate_slo(target, &score)
}

// ---------------------------------------------------------------------------
// SLO-enriched health report
// ---------------------------------------------------------------------------

/// A health report that bundles an [`SloEvaluation`] alongside the standard
/// metrics.  Produced by [`build_slo_report`].
#[derive(Clone, Debug)]
pub struct SloHealthReport {
    /// The underlying health report (scores, trend, maintenance flag).
    pub health: AnchorHealthReport,
    /// The SLO target that was evaluated.
    pub target: SloTarget,
    /// The evaluation result.
    pub evaluation: SloEvaluation,
}

impl SloHealthReport {
    /// Returns `true` when all SLO thresholds are met (or maintenance is active).
    pub fn is_slo_satisfied(&self) -> bool {
        self.evaluation.is_satisfied()
    }

    /// Returns a human-readable summary of any violations.
    pub fn violation_summary(&self) -> String {
        if self.evaluation.violations.is_empty() {
            return "all SLO targets met".into();
        }
        let parts: alloc::vec::Vec<String> = self.evaluation.violations
            .iter()
            .map(|v| v.describe())
            .collect();
        parts.join("; ")
    }
}

/// Build an [`SloHealthReport`] from observation windows and an SLO target.
///
/// Returns `Err(InvalidSloConfig)` when the target fails validation.
///
/// # Examples
///
/// ```rust
/// use anchorkit::anchor_health::{HealthWindowBuilder, SloTarget, build_slo_report};
///
/// let w = HealthWindowBuilder::new(0, 300)
///     .successes(99).failures(1).p50_latency(100.0).routing(10, 0).recovery(0)
///     .build();
/// let target = SloTarget::composite_only(80.0);
/// let report = build_slo_report("anchor.example.com", &[w], target, false).unwrap();
/// assert!(report.is_slo_satisfied());
/// ```
pub fn build_slo_report(
    anchor_id: &str,
    windows: &[HealthWindow],
    target: SloTarget,
    maintenance_active: bool,
) -> Result<SloHealthReport, crate::errors::AnchorKitError> {
    target.validate()?;
    let health = build_health_report_with_maintenance(anchor_id, windows, maintenance_active);
    let evaluation = evaluate_slo_for_report(&health, &target);
    Ok(SloHealthReport { health, target, evaluation })
}

// ---------------------------------------------------------------------------
// SLO tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod slo_tests {
    use super::*;

    fn healthy_window() -> HealthWindow {
        HealthWindowBuilder::new(0, 300)
            .successes(99).failures(1).p50_latency(100.0).routing(10, 0).recovery(0).build()
    }

    fn critical_window() -> HealthWindow {
        HealthWindowBuilder::new(0, 300)
            .successes(10).failures(90).p50_latency(9000.0).routing(10, 9).recovery(3600).build()
    }

    // ── SloTarget::validate ───────────────────────────────────────────────

    #[test]
    fn validate_all_none_rejected() {
        let t = SloTarget {
            min_composite: None,
            min_success_rate_score: None,
            min_latency_score: None,
            min_routing_score: None,
            min_recovery_score: None,
        };
        let err = t.validate().expect_err("all-None SLO must be rejected");
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidSloConfig);
    }

    #[test]
    fn validate_threshold_above_100_rejected() {
        let t = SloTarget::composite_only(101.0);
        let err = t.validate().expect_err("threshold > 100 must be rejected");
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidSloConfig);
        assert!(err.context.as_deref().unwrap_or("").contains("101.00"));
    }

    #[test]
    fn validate_threshold_below_0_rejected() {
        let t = SloTarget::composite_only(-1.0);
        let err = t.validate().expect_err("threshold < 0 must be rejected");
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidSloConfig);
    }

    #[test]
    fn validate_valid_composite_only_passes() {
        let t = SloTarget::composite_only(80.0);
        assert!(t.validate().is_ok());
    }

    #[test]
    fn validate_full_target_passes() {
        let t = SloTarget {
            min_composite: Some(80.0),
            min_success_rate_score: Some(70.0),
            min_latency_score: Some(60.0),
            min_routing_score: Some(75.0),
            min_recovery_score: Some(50.0),
        };
        assert!(t.validate().is_ok());
    }

    // ── evaluate_slo — satisfied ──────────────────────────────────────────

    #[test]
    fn evaluate_slo_satisfied_for_healthy_anchor() {
        let score = score_window(&healthy_window());
        let target = SloTarget::composite_only(80.0);
        let eval = evaluate_slo(&target, &score);
        assert!(eval.is_satisfied());
        assert!(eval.violations.is_empty());
    }

    #[test]
    fn evaluate_slo_satisfied_when_exactly_at_threshold() {
        let score = score_window(&healthy_window());
        // Use a threshold exactly equal to the computed composite
        let target = SloTarget::composite_only(score.composite);
        let eval = evaluate_slo(&target, &score);
        assert!(eval.is_satisfied(), "exact threshold must be satisfied");
    }

    // ── evaluate_slo — violated ───────────────────────────────────────────

    #[test]
    fn evaluate_slo_violated_for_critical_anchor() {
        let score = score_window(&critical_window());
        let target = SloTarget::composite_only(80.0);
        let eval = evaluate_slo(&target, &score);
        assert!(!eval.is_satisfied());
        assert_eq!(eval.violations.len(), 1);
        assert_eq!(eval.violations[0].dimension, "min_composite");
    }

    #[test]
    fn evaluate_slo_collects_multiple_violations() {
        let score = score_window(&critical_window());
        let target = SloTarget {
            min_composite: Some(80.0),
            min_success_rate_score: Some(90.0),
            min_latency_score: Some(80.0),
            min_routing_score: Some(80.0),
            min_recovery_score: Some(80.0),
        };
        let eval = evaluate_slo(&target, &score);
        assert!(!eval.is_satisfied());
        assert!(eval.violations.len() > 1, "multiple violations expected");
    }

    #[test]
    fn violation_detail_describe_format() {
        let d = SloViolationDetail {
            dimension: "min_composite".into(),
            threshold: 80.0,
            actual: 42.5,
        };
        let s = d.describe();
        assert!(s.contains("min_composite"), "desc={s}");
        assert!(s.contains("80.00"), "desc={s}");
        assert!(s.contains("42.50"), "desc={s}");
    }

    // ── evaluate_slo_for_report ───────────────────────────────────────────

    #[test]
    fn evaluate_slo_for_report_satisfied_for_healthy() {
        let report = build_health_report("anc", &[healthy_window()]);
        let target = SloTarget::composite_only(80.0);
        let eval = evaluate_slo_for_report(&report, &target);
        assert!(eval.is_satisfied());
    }

    #[test]
    fn evaluate_slo_for_report_violated_for_critical() {
        let report = build_health_report("anc", &[critical_window()]);
        let target = SloTarget::composite_only(80.0);
        let eval = evaluate_slo_for_report(&report, &target);
        assert!(!eval.is_satisfied());
    }

    #[test]
    fn evaluate_slo_for_report_suppressed_during_maintenance() {
        // Even a critical report must be treated as satisfied when maintenance is active
        let report = build_health_report_with_maintenance("anc", &[critical_window()], true);
        let target = SloTarget::composite_only(80.0);
        let eval = evaluate_slo_for_report(&report, &target);
        assert!(
            eval.is_satisfied(),
            "SLO evaluation must be suppressed during maintenance"
        );
        assert!(eval.violations.is_empty());
    }

    // ── build_slo_report ──────────────────────────────────────────────────

    #[test]
    fn build_slo_report_satisfied_healthy_anchor() {
        let target = SloTarget::composite_only(80.0);
        let report = build_slo_report("anc", &[healthy_window()], target, false).unwrap();
        assert!(report.is_slo_satisfied());
        assert_eq!(report.violation_summary(), "all SLO targets met");
    }

    #[test]
    fn build_slo_report_violated_critical_anchor() {
        let target = SloTarget::composite_only(80.0);
        let report = build_slo_report("anc", &[critical_window()], target, false).unwrap();
        assert!(!report.is_slo_satisfied());
        let summary = report.violation_summary();
        assert!(summary.contains("min_composite"), "summary={summary}");
    }

    #[test]
    fn build_slo_report_suppressed_during_maintenance() {
        let target = SloTarget::composite_only(80.0);
        let report = build_slo_report("anc", &[critical_window()], target, true).unwrap();
        assert!(
            report.is_slo_satisfied(),
            "SLO must be suppressed during maintenance"
        );
    }

    #[test]
    fn build_slo_report_rejects_invalid_target() {
        let bad_target = SloTarget::composite_only(110.0);
        let err = build_slo_report("anc", &[healthy_window()], bad_target, false)
            .expect_err("invalid target must be rejected");
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidSloConfig);
    }

    #[test]
    fn build_slo_report_carries_health_data() {
        let target = SloTarget::composite_only(50.0);
        let report = build_slo_report("myanchor", &[healthy_window()], target, false).unwrap();
        assert_eq!(report.health.anchor_id, "myanchor");
        assert!(report.health.composite_score > 80.0);
    }

    #[test]
    fn violation_summary_lists_all_violations() {
        let target = SloTarget {
            min_composite: Some(80.0),
            min_success_rate_score: Some(90.0),
            min_latency_score: None,
            min_routing_score: None,
            min_recovery_score: None,
        };
        let report = build_slo_report("anc", &[critical_window()], target, false).unwrap();
        let summary = report.violation_summary();
        assert!(summary.contains("min_composite"), "summary={summary}");
        assert!(summary.contains("min_success_rate_score"), "summary={summary}");
    }

    #[test]
    fn slo_target_composite_only_constructor() {
        let t = SloTarget::composite_only(75.0);
        assert_eq!(t.min_composite, Some(75.0));
        assert!(t.min_success_rate_score.is_none());
        assert!(t.min_latency_score.is_none());
        assert!(t.min_routing_score.is_none());
        assert!(t.min_recovery_score.is_none());
    }
}
