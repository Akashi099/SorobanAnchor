//! Transaction history compaction and summary generation (issue #676).
//!
//! Large transaction histories are difficult to work with directly. This module
//! reduces a raw history into a compact, useful form so operators can inspect
//! high-level trends without reading every record.
//!
//! ## Design
//!
//! - [`TransactionSummaryRecord`] is the compacted representation of a single
//!   time window (e.g. one day or one hour).
//! - [`compact_history`] reduces an ordered slice of raw transaction records
//!   into a [`CompactionResult`] containing per-window summaries and overall
//!   aggregate statistics.
//! - [`CompactionConfig`] controls the window size and which status categories
//!   are counted separately.

extern crate alloc;

use alloc::{string::String, vec::Vec};

use crate::errors::{AnchorKitError, ErrorCode};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Controls how transaction history is compacted.
#[derive(Clone, Debug, PartialEq)]
pub struct CompactionConfig {
    /// Width of each summary window in seconds (e.g. 3 600 for hourly).
    /// Must be > 0.
    pub window_seconds: u64,
    /// When `true`, the compaction retains the first and last transaction ID
    /// within each window for audit trail purposes.
    pub retain_boundary_ids: bool,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            window_seconds: 3_600, // hourly windows
            retain_boundary_ids: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Input record
// ---------------------------------------------------------------------------

/// A minimal representation of a single transaction record fed into the
/// compaction pipeline.
#[derive(Clone, Debug, PartialEq)]
pub struct RawTransactionRecord {
    /// Unique transaction identifier.
    pub id: String,
    /// Unix timestamp of the transaction.
    pub timestamp: u64,
    /// Status string (e.g. `"completed"`, `"pending_external"`, `"error"`).
    pub status: String,
    /// Transaction amount as an integer in the asset's minor unit.
    pub amount: u64,
    /// Asset code (e.g. `"USDC"`).
    pub asset_code: String,
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// Compacted summary for a single time window.
#[derive(Clone, Debug, PartialEq)]
pub struct TransactionSummaryRecord {
    /// Unix timestamp marking the start of this window.
    pub window_start: u64,
    /// Unix timestamp marking the end of this window (exclusive).
    pub window_end: u64,
    /// Total number of transactions in this window.
    pub total_count: u32,
    /// Number of transactions with a `"completed"` status.
    pub completed_count: u32,
    /// Number of transactions with a status containing `"pending"`.
    pub pending_count: u32,
    /// Number of transactions with a status containing `"error"` or `"failed"`.
    pub error_count: u32,
    /// Sum of all transaction amounts in this window.
    pub total_volume: u64,
    /// Largest single transaction amount in this window.
    pub max_amount: u64,
    /// Smallest single transaction amount in this window (0 if no transactions).
    pub min_amount: u64,
    /// Average amount (integer division). 0 when `total_count == 0`.
    pub avg_amount: u64,
    /// ID of the first transaction in this window (when `retain_boundary_ids`).
    pub first_id: Option<String>,
    /// ID of the last transaction in this window (when `retain_boundary_ids`).
    pub last_id: Option<String>,
}

/// Overall aggregate statistics across all windows.
#[derive(Clone, Debug, PartialEq)]
pub struct CompactionAggregate {
    /// Total transactions across all windows.
    pub total_count: u64,
    /// Total volume across all windows.
    pub total_volume: u64,
    /// Overall completion rate in [0.0, 1.0].
    pub completion_rate: f64,
    /// Overall error rate in [0.0, 1.0].
    pub error_rate: f64,
    /// Timestamp of the earliest transaction seen.
    pub earliest_timestamp: u64,
    /// Timestamp of the latest transaction seen.
    pub latest_timestamp: u64,
    /// Number of summary windows produced.
    pub window_count: usize,
}

/// The output of a compaction run.
#[derive(Clone, Debug)]
pub struct CompactionResult {
    /// Per-window summaries, ordered by `window_start` ascending.
    pub windows: Vec<TransactionSummaryRecord>,
    /// Aggregate statistics across all windows.
    pub aggregate: CompactionAggregate,
}

// ---------------------------------------------------------------------------
// Status classification helpers
// ---------------------------------------------------------------------------

fn classify_status(status: &str) -> (bool, bool, bool) {
    let s = status.to_ascii_lowercase();
    let completed = s == "completed" || s == "complete";
    let pending = s.contains("pending");
    let error = s.contains("error") || s.contains("failed") || s.contains("failure");
    (completed, pending, error)
}

// ---------------------------------------------------------------------------
// Core compaction function
// ---------------------------------------------------------------------------

/// Compact an ordered slice of raw transaction records into per-window summaries.
///
/// Records **must** be provided in ascending `timestamp` order. If the slice is
/// empty, a [`CompactionResult`] with no windows and zeroed aggregate is returned.
///
/// # Errors
///
/// Returns [`AnchorKitError`] with [`ErrorCode::ValidationError`] when
/// `config.window_seconds` is zero.
///
/// # Examples
///
/// ```rust
/// use anchorkit::transaction_compaction::{
///     compact_history, CompactionConfig, RawTransactionRecord,
/// };
///
/// let records = vec![
///     RawTransactionRecord {
///         id: "t1".into(), timestamp: 100, status: "completed".into(),
///         amount: 500, asset_code: "USDC".into(),
///     },
///     RawTransactionRecord {
///         id: "t2".into(), timestamp: 200, status: "pending_external".into(),
///         amount: 300, asset_code: "USDC".into(),
///     },
/// ];
/// let config = CompactionConfig { window_seconds: 3600, retain_boundary_ids: true };
/// let result = compact_history(&records, &config).unwrap();
/// assert_eq!(result.windows.len(), 1);
/// assert_eq!(result.aggregate.total_count, 2);
/// ```
pub fn compact_history(
    records: &[RawTransactionRecord],
    config: &CompactionConfig,
) -> Result<CompactionResult, AnchorKitError> {
    if config.window_seconds == 0 {
        return Err(AnchorKitError::validation_error(
            "window_seconds must be greater than zero",
        ));
    }

    if records.is_empty() {
        return Ok(CompactionResult {
            windows: Vec::new(),
            aggregate: CompactionAggregate {
                total_count: 0,
                total_volume: 0,
                completion_rate: 0.0,
                error_rate: 0.0,
                earliest_timestamp: 0,
                latest_timestamp: 0,
                window_count: 0,
            },
        });
    }

    let ws = config.window_seconds;
    let mut windows: Vec<TransactionSummaryRecord> = Vec::new();

    // Determine the first window start by aligning to the window boundary.
    let first_ts = records[0].timestamp;
    let mut current_window_start = (first_ts / ws) * ws;
    let mut current_window_end = current_window_start + ws;

    // Scratch state for the window being built.
    let mut total_count: u32 = 0;
    let mut completed_count: u32 = 0;
    let mut pending_count: u32 = 0;
    let mut error_count: u32 = 0;
    let mut total_volume: u64 = 0;
    let mut max_amount: u64 = 0;
    let mut min_amount: u64 = u64::MAX;
    let mut first_id: Option<String> = None;
    let mut last_id: Option<String> = None;

    let flush = |wstart: u64,
                  wend: u64,
                  tc: u32,
                  cc: u32,
                  pc: u32,
                  ec: u32,
                  vol: u64,
                  mx: u64,
                  mn: u64,
                  fid: Option<String>,
                  lid: Option<String>,
                  retain: bool|
     -> TransactionSummaryRecord {
        let avg = if tc > 0 { vol / tc as u64 } else { 0 };
        let min_out = if tc > 0 { mn } else { 0 };
        TransactionSummaryRecord {
            window_start: wstart,
            window_end: wend,
            total_count: tc,
            completed_count: cc,
            pending_count: pc,
            error_count: ec,
            total_volume: vol,
            max_amount: mx,
            min_amount: min_out,
            avg_amount: avg,
            first_id: if retain { fid } else { None },
            last_id: if retain { lid } else { None },
        }
    };

    for rec in records {
        // Advance window if this record falls outside the current one.
        while rec.timestamp >= current_window_end {
            if total_count > 0 || !windows.is_empty() {
                // Only emit non-empty windows, but always emit the current one
                // if it has data.
                if total_count > 0 {
                    windows.push(flush(
                        current_window_start,
                        current_window_end,
                        total_count,
                        completed_count,
                        pending_count,
                        error_count,
                        total_volume,
                        max_amount,
                        min_amount,
                        first_id.take(),
                        last_id.take(),
                        config.retain_boundary_ids,
                    ));
                }
            }
            current_window_start = current_window_end;
            current_window_end = current_window_start + ws;
            total_count = 0;
            completed_count = 0;
            pending_count = 0;
            error_count = 0;
            total_volume = 0;
            max_amount = 0;
            min_amount = u64::MAX;
            first_id = None;
            last_id = None;
        }

        let (completed, pending, error) = classify_status(&rec.status);
        if completed { completed_count += 1; }
        if pending { pending_count += 1; }
        if error { error_count += 1; }

        total_count += 1;
        total_volume = total_volume.saturating_add(rec.amount);
        if rec.amount > max_amount { max_amount = rec.amount; }
        if rec.amount < min_amount { min_amount = rec.amount; }

        if config.retain_boundary_ids {
            if first_id.is_none() { first_id = Some(rec.id.clone()); }
            last_id = Some(rec.id.clone());
        }
    }

    // Flush the final window.
    if total_count > 0 {
        windows.push(flush(
            current_window_start,
            current_window_end,
            total_count,
            completed_count,
            pending_count,
            error_count,
            total_volume,
            max_amount,
            min_amount,
            first_id,
            last_id,
            config.retain_boundary_ids,
        ));
    }

    // Compute aggregate.
    let agg_total: u64 = windows.iter().map(|w| w.total_count as u64).sum();
    let agg_completed: u64 = windows.iter().map(|w| w.completed_count as u64).sum();
    let agg_errors: u64 = windows.iter().map(|w| w.error_count as u64).sum();
    let agg_volume: u64 = windows.iter().map(|w| w.total_volume).sum();

    let completion_rate = if agg_total > 0 {
        agg_completed as f64 / agg_total as f64
    } else {
        0.0
    };
    let error_rate = if agg_total > 0 {
        agg_errors as f64 / agg_total as f64
    } else {
        0.0
    };

    let aggregate = CompactionAggregate {
        total_count: agg_total,
        total_volume: agg_volume,
        completion_rate,
        error_rate,
        earliest_timestamp: records.first().map(|r| r.timestamp).unwrap_or(0),
        latest_timestamp: records.last().map(|r| r.timestamp).unwrap_or(0),
        window_count: windows.len(),
    };

    Ok(CompactionResult { windows, aggregate })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, ts: u64, status: &str, amount: u64) -> RawTransactionRecord {
        RawTransactionRecord {
            id: id.into(),
            timestamp: ts,
            status: status.into(),
            amount,
            asset_code: "USDC".into(),
        }
    }

    #[test]
    fn empty_input_returns_zeroed_aggregate() {
        let result = compact_history(&[], &CompactionConfig::default()).unwrap();
        assert!(result.windows.is_empty());
        assert_eq!(result.aggregate.total_count, 0);
        assert_eq!(result.aggregate.window_count, 0);
    }

    #[test]
    fn zero_window_seconds_rejected() {
        let cfg = CompactionConfig { window_seconds: 0, retain_boundary_ids: false };
        assert_eq!(
            compact_history(&[rec("t1", 100, "completed", 50)], &cfg)
                .unwrap_err()
                .code,
            ErrorCode::ValidationError
        );
    }

    #[test]
    fn single_record_produces_one_window() {
        let records = vec![rec("t1", 500, "completed", 100)];
        let cfg = CompactionConfig { window_seconds: 3600, retain_boundary_ids: true };
        let result = compact_history(&records, &cfg).unwrap();
        assert_eq!(result.windows.len(), 1);
        let w = &result.windows[0];
        assert_eq!(w.total_count, 1);
        assert_eq!(w.completed_count, 1);
        assert_eq!(w.total_volume, 100);
        assert_eq!(w.max_amount, 100);
        assert_eq!(w.min_amount, 100);
        assert_eq!(w.avg_amount, 100);
        assert_eq!(w.first_id.as_deref(), Some("t1"));
        assert_eq!(w.last_id.as_deref(), Some("t1"));
    }

    #[test]
    fn multiple_records_same_window() {
        let records = vec![
            rec("t1", 100, "completed", 200),
            rec("t2", 200, "pending_external", 100),
            rec("t3", 300, "error", 50),
        ];
        let cfg = CompactionConfig { window_seconds: 3600, retain_boundary_ids: true };
        let result = compact_history(&records, &cfg).unwrap();
        assert_eq!(result.windows.len(), 1);
        let w = &result.windows[0];
        assert_eq!(w.total_count, 3);
        assert_eq!(w.completed_count, 1);
        assert_eq!(w.pending_count, 1);
        assert_eq!(w.error_count, 1);
        assert_eq!(w.total_volume, 350);
        assert_eq!(w.max_amount, 200);
        assert_eq!(w.min_amount, 50);
        assert_eq!(w.first_id.as_deref(), Some("t1"));
        assert_eq!(w.last_id.as_deref(), Some("t3"));
    }

    #[test]
    fn records_spanning_two_windows() {
        let cfg = CompactionConfig { window_seconds: 100, retain_boundary_ids: false };
        let records = vec![
            rec("t1", 0,   "completed", 10),
            rec("t2", 50,  "completed", 20),
            rec("t3", 100, "completed", 30), // starts new window at t=100
            rec("t4", 150, "completed", 40),
        ];
        let result = compact_history(&records, &cfg).unwrap();
        assert_eq!(result.windows.len(), 2);
        assert_eq!(result.windows[0].total_count, 2);
        assert_eq!(result.windows[1].total_count, 2);
        assert_eq!(result.aggregate.total_count, 4);
    }

    #[test]
    fn boundary_ids_not_retained_when_disabled() {
        let records = vec![rec("t1", 10, "completed", 5)];
        let cfg = CompactionConfig { window_seconds: 3600, retain_boundary_ids: false };
        let result = compact_history(&records, &cfg).unwrap();
        assert!(result.windows[0].first_id.is_none());
        assert!(result.windows[0].last_id.is_none());
    }

    #[test]
    fn aggregate_completion_and_error_rates() {
        let records = vec![
            rec("t1", 10, "completed", 1),
            rec("t2", 20, "completed", 1),
            rec("t3", 30, "error", 1),
            rec("t4", 40, "failed", 1),
        ];
        let cfg = CompactionConfig { window_seconds: 3600, retain_boundary_ids: false };
        let result = compact_history(&records, &cfg).unwrap();
        let agg = &result.aggregate;
        assert_eq!(agg.total_count, 4);
        assert!((agg.completion_rate - 0.5).abs() < 1e-9);
        assert!((agg.error_rate - 0.5).abs() < 1e-9);
    }

    #[test]
    fn aggregate_timestamps_span_full_range() {
        let records = vec![
            rec("t1", 1000, "completed", 1),
            rec("t2", 2000, "completed", 1),
            rec("t3", 5000, "completed", 1),
        ];
        let cfg = CompactionConfig::default();
        let result = compact_history(&records, &cfg).unwrap();
        assert_eq!(result.aggregate.earliest_timestamp, 1000);
        assert_eq!(result.aggregate.latest_timestamp, 5000);
    }

    #[test]
    fn classify_status_variants() {
        assert_eq!(classify_status("completed"), (true, false, false));
        assert_eq!(classify_status("COMPLETED"), (true, false, false));
        assert_eq!(classify_status("pending_external"), (false, true, false));
        assert_eq!(classify_status("error"), (false, false, true));
        assert_eq!(classify_status("failed"), (false, false, true));
        assert_eq!(classify_status("unknown"), (false, false, false));
    }
}
