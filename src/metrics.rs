//! Lightweight in-process metrics for host-side anchor operations.
//!
//! Production anchors need operational visibility: success rates, retry
//! volume, failure counts and resource usage. This module provides a small
//! dependency-free metrics layer that the host-side code paths (HTTP
//! requests, retries, webhook delivery) attach to via `*_metered` wrapper
//! functions.
//!
//! # Design
//!
//! * **No external crates.** Counters and gauges are plain `u64` values in
//!   `BTreeMap`s guarded by `RefCell`, mirroring the interior-mutability
//!   pattern already used by webhook delivery. This keeps the crate free of
//!   a metrics-backend dependency; a snapshot can be exported to any backend
//!   by the embedding application.
//! * **Saturating arithmetic.** The workspace builds with
//!   `overflow-checks = true` in release, so all updates saturate instead of
//!   panicking at the counter ceiling.
//! * **Namespacing.** [`MetricsRegistry::with_namespace`] prefixes every
//!   metric name (`<namespace>.<name>`), matching the `metrics_namespace`
//!   field that runtime configs already declare under `monitoring`.
//!
//! # Example
//!
//! ```
//! use anchorkit::metrics::{names, MetricsRegistry};
//!
//! let metrics = MetricsRegistry::new();
//! metrics.record_call(names::CONTRACT_CALL, true);
//! metrics.record_call(names::CONTRACT_CALL, false);
//!
//! assert_eq!(metrics.counter(&names::calls(names::CONTRACT_CALL)), 2);
//! assert_eq!(metrics.counter(&names::failures(names::CONTRACT_CALL)), 1);
//! ```

#[cfg(feature = "std")]
extern crate std;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use core::cell::RefCell;

// ---------------------------------------------------------------------------
// Metric names
// ---------------------------------------------------------------------------

/// Canonical metric names emitted by the built-in `*_metered` wrappers.
///
/// Free functions build per-operation names (`<operation>.calls`,
/// `<operation>.retry.attempts`, ...) so unrelated operations never share a
/// counter.
pub mod names {
    use alloc::format;
    use alloc::string::String;

    /// Total outbound HTTP POSTs started via `post_with_options_metered`.
    pub const HTTP_REQUESTS: &str = "http.requests";
    /// HTTP POSTs that completed with a status below 400.
    pub const HTTP_SUCCESSES: &str = "http.successes";
    /// HTTP POSTs that completed with a status of 400 or above.
    pub const HTTP_ERROR_RESPONSES: &str = "http.error_responses";
    /// HTTP POSTs that failed at the transport layer (no status received).
    pub const HTTP_TRANSPORT_ERRORS: &str = "http.transport_errors";

    /// Webhook delivery operations started via `deliver_webhook_metered`.
    pub const WEBHOOK_DELIVERIES: &str = "webhook.deliveries";
    /// Individual webhook POST attempts (including retries).
    pub const WEBHOOK_ATTEMPTS: &str = "webhook.attempts";
    /// Webhook deliveries that eventually succeeded.
    pub const WEBHOOK_SUCCESSES: &str = "webhook.successes";
    /// Webhook deliveries that exhausted all attempts and failed.
    pub const WEBHOOK_FAILURES: &str = "webhook.failures";
    /// Entries appended to the dead-letter queue.
    pub const WEBHOOK_DLQ_ENTRIES: &str = "webhook.dlq_entries";
    /// Gauge: total entries currently held across all DLQ keys.
    pub const WEBHOOK_DLQ_DEPTH: &str = "webhook.dlq_depth";

    /// Operation name for Soroban contract invocations recorded through
    /// [`super::MetricsRegistry::record_call`].
    pub const CONTRACT_CALL: &str = "contract_call";

    /// `<operation>.calls` — total invocations of an operation.
    pub fn calls(operation: &str) -> String {
        format!("{operation}.calls")
    }

    /// `<operation>.successes` — invocations that succeeded.
    pub fn successes(operation: &str) -> String {
        format!("{operation}.successes")
    }

    /// `<operation>.failures` — invocations that failed.
    pub fn failures(operation: &str) -> String {
        format!("{operation}.failures")
    }

    /// `<operation>.retry.attempts` — closure executions inside a retry loop.
    pub fn retry_attempts(operation: &str) -> String {
        format!("{operation}.retry.attempts")
    }

    /// `<operation>.retry.backoffs` — sleeps between attempts (i.e. actual retries).
    pub fn retry_backoffs(operation: &str) -> String {
        format!("{operation}.retry.backoffs")
    }

    /// `<operation>.retry.successes` — retry loops that returned `Ok`.
    pub fn retry_successes(operation: &str) -> String {
        format!("{operation}.retry.successes")
    }

    /// `<operation>.retry.failures` — retry loops that gave up with `Err`.
    pub fn retry_failures(operation: &str) -> String {
        format!("{operation}.retry.failures")
    }
}

// ---------------------------------------------------------------------------
// Latency summary
// ---------------------------------------------------------------------------

/// Aggregated latency observations for one operation.
///
/// Stores count/total/max rather than raw samples so memory stays bounded
/// regardless of call volume.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LatencySummary {
    /// Number of latency observations recorded.
    pub count: u64,
    /// Sum of all observed latencies in milliseconds.
    pub total_ms: u64,
    /// Largest single observed latency in milliseconds.
    pub max_ms: u64,
}

impl LatencySummary {
    /// Mean observed latency in milliseconds (0 when nothing was recorded).
    pub fn avg_ms(&self) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.total_ms / self.count
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Point-in-time copy of every metric held by a [`MetricsRegistry`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// Monotonically increasing event counts, keyed by metric name.
    pub counters: BTreeMap<String, u64>,
    /// Last-written point-in-time values, keyed by metric name.
    pub gauges: BTreeMap<String, u64>,
    /// Aggregated latency observations, keyed by metric name.
    pub latencies: BTreeMap<String, LatencySummary>,
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// In-process registry of counters, gauges and latency summaries.
///
/// Uses `RefCell` interior mutability so instrumentation can happen behind
/// `&self` inside `Fn` closures (the same pattern webhook delivery uses to
/// capture per-attempt state). The registry is intentionally not `Sync`;
/// host-side anchor operations in this crate are single-threaded blocking
/// calls.
#[derive(Debug, Default)]
pub struct MetricsRegistry {
    namespace: Option<String>,
    counters: RefCell<BTreeMap<String, u64>>,
    gauges: RefCell<BTreeMap<String, u64>>,
    latencies: RefCell<BTreeMap<String, LatencySummary>>,
}

impl MetricsRegistry {
    /// Create an empty registry with no namespace prefix.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty registry whose metric names are prefixed with
    /// `<namespace>.` — e.g. `anchorkit.fiat_ramp.http.requests`.
    pub fn with_namespace(namespace: impl Into<String>) -> Self {
        Self {
            namespace: Some(namespace.into()),
            ..Self::default()
        }
    }

    /// The namespace prefix, if one was configured.
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    fn qualify(&self, name: &str) -> String {
        match &self.namespace {
            Some(ns) => format!("{ns}.{name}"),
            None => name.to_string(),
        }
    }

    /// Increment the counter `name` by 1.
    pub fn incr(&self, name: &str) {
        self.incr_by(name, 1);
    }

    /// Increment the counter `name` by `delta`, saturating at `u64::MAX`.
    pub fn incr_by(&self, name: &str, delta: u64) {
        let key = self.qualify(name);
        let mut counters = self.counters.borrow_mut();
        let entry = counters.entry(key).or_insert(0);
        *entry = entry.saturating_add(delta);
    }

    /// Current value of the counter `name` (0 if it was never incremented).
    pub fn counter(&self, name: &str) -> u64 {
        self.counters
            .borrow()
            .get(&self.qualify(name))
            .copied()
            .unwrap_or(0)
    }

    /// Set the gauge `name` to `value`, replacing any previous value.
    pub fn set_gauge(&self, name: &str, value: u64) {
        self.gauges.borrow_mut().insert(self.qualify(name), value);
    }

    /// Current value of the gauge `name`, or `None` if it was never set.
    pub fn gauge(&self, name: &str) -> Option<u64> {
        self.gauges.borrow().get(&self.qualify(name)).copied()
    }

    /// Record one latency observation (milliseconds) for `name`.
    pub fn observe_latency_ms(&self, name: &str, elapsed_ms: u64) {
        let key = self.qualify(name);
        let mut latencies = self.latencies.borrow_mut();
        let summary = latencies.entry(key).or_default();
        summary.count = summary.count.saturating_add(1);
        summary.total_ms = summary.total_ms.saturating_add(elapsed_ms);
        if elapsed_ms > summary.max_ms {
            summary.max_ms = elapsed_ms;
        }
    }

    /// Aggregated latency for `name`, or `None` if nothing was observed.
    pub fn latency(&self, name: &str) -> Option<LatencySummary> {
        self.latencies.borrow().get(&self.qualify(name)).cloned()
    }

    /// Record one invocation of `operation`, bumping `<operation>.calls`
    /// plus `<operation>.successes` or `<operation>.failures`.
    ///
    /// Use [`names::CONTRACT_CALL`] as the operation to count Soroban
    /// contract invocations made by an embedding application.
    pub fn record_call(&self, operation: &str, success: bool) {
        self.incr(&names::calls(operation));
        if success {
            self.incr(&names::successes(operation));
        } else {
            self.incr(&names::failures(operation));
        }
    }

    /// Copy every metric into an owned [`MetricsSnapshot`].
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            counters: self.counters.borrow().clone(),
            gauges: self.gauges.borrow().clone(),
            latencies: self.latencies.borrow().clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime-config integration (std only)
// ---------------------------------------------------------------------------

#[cfg(feature = "std")]
impl MetricsRegistry {
    /// Build a registry from the `monitoring` section of a runtime config.
    ///
    /// Returns `Some` only when `enable_metrics` is explicitly `true`
    /// (metrics are opt-in). The registry adopts `metrics_namespace` when
    /// one is configured. Previously these config fields were parsed but
    /// never read by any code path.
    pub fn from_monitoring_config(
        monitoring: Option<&crate::config::MonitoringConfig>,
    ) -> Option<Self> {
        let monitoring = monitoring?;
        if !monitoring.enable_metrics.unwrap_or(false) {
            return None;
        }
        Some(match monitoring.metrics_namespace.as_deref() {
            Some(ns) => Self::with_namespace(ns),
            None => Self::new(),
        })
    }
}

/// Run `f`, recording its wall-clock duration against `name` (std only).
///
/// Returns whatever `f` returns; the latency is recorded whether the inner
/// operation logically succeeded or not.
#[cfg(feature = "std")]
pub fn time_operation<T>(metrics: &MetricsRegistry, name: &str, f: impl FnOnce() -> T) -> T {
    let start = std::time::Instant::now();
    let out = f();
    let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    metrics.observe_latency_ms(name, elapsed_ms);
    out
}
