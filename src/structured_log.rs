//! Structured logging for operational workflows.
//!
//! This module provides a dependency-free, `no_std`-compatible structured
//! logger so that host-side workflows (attestor registration, transaction
//! status polling, webhook delivery, cache governance) emit consistent,
//! machine-readable log entries instead of ad-hoc `eprintln!` lines.
//!
//! # Log schema
//!
//! Every entry is a [`LogRecord`] and serialises to a single JSON line:
//!
//! ```json
//! {"ts":1712345678,"seq":3,"level":"warn","event":"webhook.delivery_attempt_failed","fields":{"endpoint_url":"https://example.com/hook","attempt":2,"status":503,"error":"HTTP 503"}}
//! ```
//!
//! - `ts` — caller-supplied Unix timestamp in seconds (the crate is `no_std`,
//!   so wall-clock time is always injected, mirroring the `now_fn` convention
//!   used across the crate).
//! - `seq` — monotonic per-logger sequence number, so entries can be ordered
//!   even when several share a timestamp.
//! - `level` — one of `debug` / `info` / `warn` / `error`.
//! - `event` — canonical dotted event name; see [`events`] for the full catalog.
//! - `fields` — event-specific context as typed key/value pairs, serialised in
//!   insertion order.
//!
//! # Design notes
//!
//! Like [`retry`](crate::retry) and [`webhook`](crate::webhook), the logger has
//! no global state and performs no I/O of its own: records accumulate in an
//! in-memory ring buffer and the host decides where to ship them
//! ([`StructuredLogger::drain_json_lines`]). With the `std` feature enabled,
//! [`StructuredLogger::flush_to_stderr`] writes drained lines to stderr.

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::fmt::Write as _;

// ---------------------------------------------------------------------------
// Canonical event names
// ---------------------------------------------------------------------------

/// Canonical event names emitted by the instrumented workflows.
///
/// Names are dotted `workflow.event` pairs so downstream pipelines can filter
/// on the workflow prefix.
pub mod events {
    // Attestor registration.
    pub const ATTESTOR_REGISTRATION_STARTED: &str = "attestor.registration_started";
    pub const ATTESTOR_REGISTRATION_SUCCEEDED: &str = "attestor.registration_succeeded";
    pub const ATTESTOR_REGISTRATION_FAILED: &str = "attestor.registration_failed";

    // Transaction status polling.
    pub const TXSTATUS_MONITOR_STARTED: &str = "txstatus.monitor_started";
    pub const TXSTATUS_POLL_ERROR: &str = "txstatus.poll_error";
    pub const TXSTATUS_STATE_CHANGED: &str = "txstatus.state_changed";
    pub const TXSTATUS_MORE_INFO_AVAILABLE: &str = "txstatus.more_info_available";
    pub const TXSTATUS_COMPLETED: &str = "txstatus.completed";
    pub const TXSTATUS_FAILED: &str = "txstatus.failed";

    // Webhook delivery.
    pub const WEBHOOK_DELIVERY_STARTED: &str = "webhook.delivery_started";
    pub const WEBHOOK_DELIVERY_ATTEMPT_FAILED: &str = "webhook.delivery_attempt_failed";
    pub const WEBHOOK_DELIVERY_SUCCEEDED: &str = "webhook.delivery_succeeded";
    pub const WEBHOOK_DELIVERY_FAILED: &str = "webhook.delivery_failed";
    pub const WEBHOOK_DLQ_ENTRY_ADDED: &str = "webhook.dlq_entry_added";

    // Cache governance.
    pub const CACHE_POLICY_UPDATED: &str = "cache.policy_updated";
    pub const CACHE_POLICY_REJECTED: &str = "cache.policy_rejected";
    pub const CACHE_TTL_CLAMPED: &str = "cache.ttl_clamped";
    pub const CACHE_INVALIDATION_DENIED: &str = "cache.invalidation_denied";
    pub const CACHE_PROPOSAL_CREATED: &str = "cache.proposal_created";
    pub const CACHE_PROPOSAL_ENDORSED: &str = "cache.proposal_endorsed";
    pub const CACHE_PROPOSAL_ENDORSE_FAILED: &str = "cache.proposal_endorse_failed";
    pub const CACHE_PROPOSAL_EXECUTED: &str = "cache.proposal_executed";
    pub const CACHE_PROPOSAL_EXECUTE_FAILED: &str = "cache.proposal_execute_failed";
}

// ---------------------------------------------------------------------------
// Levels
// ---------------------------------------------------------------------------

/// Severity level of a [`LogRecord`], ordered from least to most severe.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl LogLevel {
    /// Lowercase wire name used in the serialised JSON (`"debug"`, `"info"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

// ---------------------------------------------------------------------------
// Field values
// ---------------------------------------------------------------------------

/// Typed value of a context field, so numeric fields stay numeric in the
/// serialised JSON instead of being stringified.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Str(String),
    U64(u64),
    I64(i64),
    Bool(bool),
}

impl From<&str> for FieldValue {
    fn from(v: &str) -> Self {
        FieldValue::Str(v.to_string())
    }
}

impl From<String> for FieldValue {
    fn from(v: String) -> Self {
        FieldValue::Str(v)
    }
}

impl From<u64> for FieldValue {
    fn from(v: u64) -> Self {
        FieldValue::U64(v)
    }
}

impl From<u32> for FieldValue {
    fn from(v: u32) -> Self {
        FieldValue::U64(v as u64)
    }
}

impl From<u16> for FieldValue {
    fn from(v: u16) -> Self {
        FieldValue::U64(v as u64)
    }
}

impl From<usize> for FieldValue {
    fn from(v: usize) -> Self {
        FieldValue::U64(v as u64)
    }
}

impl From<i64> for FieldValue {
    fn from(v: i64) -> Self {
        FieldValue::I64(v)
    }
}

impl From<bool> for FieldValue {
    fn from(v: bool) -> Self {
        FieldValue::Bool(v)
    }
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// One structured log entry.
#[derive(Clone, Debug, PartialEq)]
pub struct LogRecord {
    /// Unix timestamp (seconds) supplied by the caller when the entry was logged.
    pub timestamp: u64,
    /// Monotonic per-logger sequence number (starts at 0).
    pub seq: u64,
    /// Severity level.
    pub level: LogLevel,
    /// Canonical event name (see [`events`]).
    pub event: String,
    /// Event-specific context fields, in insertion order.
    pub fields: Vec<(String, FieldValue)>,
}

/// Append `s` to `out` as a JSON string literal (with quotes and escapes).
fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl LogRecord {
    /// Serialise the record as a single JSON line (no trailing newline).
    ///
    /// Key order is fixed (`ts`, `seq`, `level`, `event`, `fields`) and fields
    /// keep their insertion order, so output is deterministic and diffable.
    pub fn to_json_line(&self) -> String {
        let mut out = String::new();
        let _ = write!(
            out,
            "{{\"ts\":{},\"seq\":{},\"level\":\"{}\",\"event\":",
            self.timestamp,
            self.seq,
            self.level.as_str()
        );
        write_json_string(&mut out, &self.event);
        out.push_str(",\"fields\":{");
        for (i, (key, value)) in self.fields.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_json_string(&mut out, key);
            out.push(':');
            match value {
                FieldValue::Str(s) => write_json_string(&mut out, s),
                FieldValue::U64(n) => {
                    let _ = write!(out, "{}", n);
                }
                FieldValue::I64(n) => {
                    let _ = write!(out, "{}", n);
                }
                FieldValue::Bool(b) => {
                    let _ = write!(out, "{}", b);
                }
            }
        }
        out.push_str("}}");
        out
    }

    /// Return the value of a named field, if present.
    pub fn field(&self, key: &str) -> Option<&FieldValue> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
}

// ---------------------------------------------------------------------------
// Logger
// ---------------------------------------------------------------------------

/// Default maximum number of buffered records before the oldest are dropped.
pub const DEFAULT_LOG_CAPACITY: usize = 1024;

/// In-memory structured logger with level filtering and a bounded buffer.
///
/// Interior mutability (`RefCell`/`Cell`) allows logging through a shared
/// reference, so a single logger can be threaded through the closure-based
/// workflow APIs without fighting the borrow checker — the same pattern
/// [`deliver_webhook`](crate::webhook::deliver_webhook) already uses
/// internally for its retry bookkeeping. Not thread-safe by design: the
/// crate is `no_std` and callers own their concurrency story.
pub struct StructuredLogger {
    min_level: LogLevel,
    capacity: usize,
    seq: Cell<u64>,
    dropped: Cell<u64>,
    buffer: RefCell<Vec<LogRecord>>,
}

impl Default for StructuredLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl StructuredLogger {
    /// Create a logger that keeps `Info` and above, buffering up to
    /// [`DEFAULT_LOG_CAPACITY`] records.
    pub fn new() -> Self {
        StructuredLogger {
            min_level: LogLevel::Info,
            capacity: DEFAULT_LOG_CAPACITY,
            seq: Cell::new(0),
            dropped: Cell::new(0),
            buffer: RefCell::new(Vec::new()),
        }
    }

    /// Set the minimum level; records below it are discarded at the call site.
    pub fn with_min_level(mut self, level: LogLevel) -> Self {
        self.min_level = level;
        self
    }

    /// Set the buffer capacity. `0` means unlimited (use with caution).
    /// When the buffer is full the oldest record is evicted (FIFO) and the
    /// [`dropped`](Self::dropped) counter is incremented.
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Record an entry. Returns `true` when the record was accepted, `false`
    /// when it was filtered out by the minimum level.
    ///
    /// `timestamp` is a Unix timestamp in seconds, injected by the caller
    /// (this crate is `no_std` and has no clock of its own).
    pub fn log(
        &self,
        level: LogLevel,
        event: &str,
        timestamp: u64,
        fields: &[(&str, FieldValue)],
    ) -> bool {
        if level < self.min_level {
            return false;
        }
        let seq = self.seq.get();
        self.seq.set(seq + 1);
        let record = LogRecord {
            timestamp,
            seq,
            level,
            event: event.to_string(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        };
        let mut buffer = self.buffer.borrow_mut();
        if self.capacity > 0 && buffer.len() >= self.capacity {
            buffer.remove(0);
            self.dropped.set(self.dropped.get() + 1);
        }
        buffer.push(record);
        true
    }

    /// Shorthand for [`log`](Self::log) at `Debug` level.
    pub fn debug(&self, event: &str, timestamp: u64, fields: &[(&str, FieldValue)]) -> bool {
        self.log(LogLevel::Debug, event, timestamp, fields)
    }

    /// Shorthand for [`log`](Self::log) at `Info` level.
    pub fn info(&self, event: &str, timestamp: u64, fields: &[(&str, FieldValue)]) -> bool {
        self.log(LogLevel::Info, event, timestamp, fields)
    }

    /// Shorthand for [`log`](Self::log) at `Warn` level.
    pub fn warn(&self, event: &str, timestamp: u64, fields: &[(&str, FieldValue)]) -> bool {
        self.log(LogLevel::Warn, event, timestamp, fields)
    }

    /// Shorthand for [`log`](Self::log) at `Error` level.
    pub fn error(&self, event: &str, timestamp: u64, fields: &[(&str, FieldValue)]) -> bool {
        self.log(LogLevel::Error, event, timestamp, fields)
    }

    /// Snapshot of all buffered records, oldest first.
    pub fn records(&self) -> Vec<LogRecord> {
        self.buffer.borrow().clone()
    }

    /// Serialise all buffered records to JSON lines without draining them.
    pub fn json_lines(&self) -> Vec<String> {
        self.buffer
            .borrow()
            .iter()
            .map(LogRecord::to_json_line)
            .collect()
    }

    /// Drain the buffer, returning the serialised JSON lines. Subsequent calls
    /// return only records logged after this one; sequence numbers keep
    /// incrementing across drains.
    pub fn drain_json_lines(&self) -> Vec<String> {
        let mut buffer = self.buffer.borrow_mut();
        let lines = buffer.iter().map(LogRecord::to_json_line).collect();
        buffer.clear();
        lines
    }

    /// Number of currently buffered records.
    pub fn len(&self) -> usize {
        self.buffer.borrow().len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.borrow().is_empty()
    }

    /// Number of records evicted because the buffer was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.get()
    }

    /// Discard all buffered records (the sequence counter is not reset).
    pub fn clear(&self) {
        self.buffer.borrow_mut().clear();
    }

    /// Drain the buffer and write each record as a JSON line to stderr.
    ///
    /// This is the drop-in replacement for the ad-hoc `eprintln!` diagnostics
    /// the crate used previously.
    #[cfg(feature = "std")]
    pub fn flush_to_stderr(&self) {
        for line in self.drain_json_lines() {
            std::eprintln!("{}", line);
        }
    }
}

// ---------------------------------------------------------------------------
// Attestor registration instrumentation
// ---------------------------------------------------------------------------

/// Instrument a host-side attestor registration workflow.
///
/// Attestor registration itself runs on-chain
/// ([`register_attestor`](crate::contract::AnchorKitContract::register_attestor)
/// emits an `attestor.added` contract event), so the host wraps its submission
/// in this helper to get correlated off-chain logs: an
/// `attestor.registration_started` entry before `submit` runs, then
/// `attestor.registration_succeeded` or `attestor.registration_failed`
/// (with the `Debug` rendering of the error) depending on the outcome.
/// The result of `submit` is passed through unchanged.
pub fn log_attestor_registration<T, E, F>(
    logger: &StructuredLogger,
    timestamp: u64,
    attestor: &str,
    sep10_issuer: &str,
    submit: F,
) -> Result<T, E>
where
    E: core::fmt::Debug,
    F: FnOnce() -> Result<T, E>,
{
    logger.info(
        events::ATTESTOR_REGISTRATION_STARTED,
        timestamp,
        &[
            ("attestor", attestor.into()),
            ("sep10_issuer", sep10_issuer.into()),
        ],
    );
    match submit() {
        Ok(value) => {
            logger.info(
                events::ATTESTOR_REGISTRATION_SUCCEEDED,
                timestamp,
                &[("attestor", attestor.into())],
            );
            Ok(value)
        }
        Err(err) => {
            logger.error(
                events::ATTESTOR_REGISTRATION_FAILED,
                timestamp,
                &[
                    ("attestor", attestor.into()),
                    ("error", alloc::format!("{:?}", err).into()),
                ],
            );
            Err(err)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_line_has_fixed_key_order_and_typed_fields() {
        let logger = StructuredLogger::new();
        logger.info(
            "webhook.delivery_started",
            1700000000,
            &[
                ("endpoint_url", "https://example.com/hook".into()),
                ("max_attempts", 3u32.into()),
                ("signed", false.into()),
            ],
        );
        let lines = logger.json_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            "{\"ts\":1700000000,\"seq\":0,\"level\":\"info\",\"event\":\"webhook.delivery_started\",\"fields\":{\"endpoint_url\":\"https://example.com/hook\",\"max_attempts\":3,\"signed\":false}}"
        );
    }

    #[test]
    fn string_fields_are_json_escaped() {
        let logger = StructuredLogger::new();
        logger.error(
            "webhook.delivery_failed",
            1,
            &[("error", "quote \" backslash \\ newline \n".into())],
        );
        let line = &logger.json_lines()[0];
        assert!(line.contains("quote \\\" backslash \\\\ newline \\n"));
    }

    #[test]
    fn min_level_filters_records() {
        let logger = StructuredLogger::new().with_min_level(LogLevel::Warn);
        assert!(!logger.debug("e", 0, &[]));
        assert!(!logger.info("e", 0, &[]));
        assert!(logger.warn("e", 0, &[]));
        assert!(logger.error("e", 0, &[]));
        assert_eq!(logger.len(), 2);
    }

    #[test]
    fn capacity_evicts_oldest_and_counts_drops() {
        let logger = StructuredLogger::new().with_capacity(2);
        logger.info("first", 1, &[]);
        logger.info("second", 2, &[]);
        logger.info("third", 3, &[]);
        let records = logger.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].event, "second");
        assert_eq!(records[1].event, "third");
        assert_eq!(logger.dropped(), 1);
    }

    #[test]
    fn sequence_numbers_are_monotonic_across_drains() {
        let logger = StructuredLogger::new();
        logger.info("a", 0, &[]);
        logger.drain_json_lines();
        logger.info("b", 0, &[]);
        let records = logger.records();
        assert_eq!(records[0].seq, 1);
    }

    #[test]
    fn attestor_registration_success_logs_start_and_success() {
        let logger = StructuredLogger::new();
        let result: Result<u32, &str> =
            log_attestor_registration(&logger, 500, "GATTESTOR", "GISSUER", || Ok(7));
        assert_eq!(result, Ok(7));
        let records = logger.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].event, events::ATTESTOR_REGISTRATION_STARTED);
        assert_eq!(records[1].event, events::ATTESTOR_REGISTRATION_SUCCEEDED);
        assert_eq!(
            records[0].field("sep10_issuer"),
            Some(&FieldValue::Str("GISSUER".into()))
        );
    }

    #[test]
    fn attestor_registration_failure_logs_error_with_context() {
        let logger = StructuredLogger::new();
        let result: Result<u32, &str> =
            log_attestor_registration(&logger, 500, "GATTESTOR", "GISSUER", || Err("denied"));
        assert!(result.is_err());
        let records = logger.records();
        assert_eq!(records[1].event, events::ATTESTOR_REGISTRATION_FAILED);
        assert_eq!(records[1].level, LogLevel::Error);
        assert_eq!(
            records[1].field("error"),
            Some(&FieldValue::Str("\"denied\"".into()))
        );
    }
}
