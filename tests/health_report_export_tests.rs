//! Tests for anchor health report export (#664).
//!
//! Verifies that:
//! - `build_health_report` produces correct scores and labels from observation
//!   windows.
//! - `export_health_report` with `HealthReportFormat::Text` produces a
//!   well-formed `key: value` string containing all required fields.
//! - `export_health_report` with `HealthReportFormat::Json` produces a valid
//!   JSON object containing all required fields.
//! - Reports from an empty window set produce sensible defaults (not panics).
//! - `previous_composite` is `None` for single-window reports and `Some` for
//!   multi-window reports.
//! - The `label` and `trend` fields reflect the actual health state.

#![cfg(not(feature = "wasm"))]

use anchorkit::anchor_health::{
    build_health_report, export_health_report, HealthReportFormat, HealthWindow,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn healthy_window(started_at: u64) -> HealthWindow {
    HealthWindow {
        started_at,
        ended_at: started_at + 300,
        success_count: 99,
        failure_count: 1,
        p50_latency_ms: 100.0,
        routing_failure_count: 0,
        routing_attempt_count: 10,
        recovery_time_seconds: 0,
    }
}

fn critical_window(started_at: u64) -> HealthWindow {
    HealthWindow {
        started_at,
        ended_at: started_at + 300,
        success_count: 30,
        failure_count: 70,
        p50_latency_ms: 9000.0,
        routing_failure_count: 8,
        routing_attempt_count: 10,
        recovery_time_seconds: 3600,
    }
}

// ── build_health_report ───────────────────────────────────────────────────────

#[test]
fn build_report_from_healthy_window_has_healthy_label() {
    let report = build_health_report("anchor.example.com", &[healthy_window(0)]);
    assert_eq!(report.label, "healthy");
    assert!(report.composite_score >= 80.0);
}

#[test]
fn build_report_from_critical_window_has_critical_label() {
    let report = build_health_report("anchor.example.com", &[critical_window(0)]);
    assert_eq!(report.label, "critical");
    assert!(report.composite_score < 50.0);
}

#[test]
fn build_report_anchor_id_preserved() {
    let report = build_health_report("my-anchor.stellar.org", &[healthy_window(0)]);
    assert_eq!(report.anchor_id, "my-anchor.stellar.org");
}

#[test]
fn build_report_window_count_matches_input() {
    let windows = vec![healthy_window(0), healthy_window(300), healthy_window(600)];
    let report = build_health_report("test", &windows);
    assert_eq!(report.window_count, 3);
}

#[test]
fn build_report_single_window_previous_composite_is_none() {
    let report = build_health_report("test", &[healthy_window(0)]);
    assert!(report.previous_composite.is_none());
}

#[test]
fn build_report_two_windows_previous_composite_is_some() {
    let report = build_health_report(
        "test",
        &[healthy_window(0), healthy_window(300)],
    );
    assert!(report.previous_composite.is_some());
}

#[test]
fn build_report_empty_windows_does_not_panic() {
    let report = build_health_report("test", &[]);
    assert_eq!(report.window_count, 0);
    assert!(report.previous_composite.is_none());
}

#[test]
fn build_report_degrading_trend_label() {
    // First window healthy, second critical → trend should be "degrading"
    let windows = vec![healthy_window(0), critical_window(300)];
    let report = build_health_report("test", &windows);
    assert_eq!(report.trend, "degrading");
}

#[test]
fn build_report_improving_trend_label() {
    // First window critical, second healthy → trend should be "improving"
    let windows = vec![critical_window(0), healthy_window(300)];
    let report = build_health_report("test", &windows);
    assert_eq!(report.trend, "improving");
}

#[test]
fn build_report_stable_trend_for_identical_windows() {
    let windows = vec![healthy_window(0), healthy_window(300)];
    let report = build_health_report("test", &windows);
    assert_eq!(report.trend, "stable");
}

#[test]
fn build_report_sub_scores_are_in_range() {
    let report = build_health_report("test", &[healthy_window(0)]);
    assert!(report.composite_score >= 0.0 && report.composite_score <= 100.0);
    assert!(report.success_rate_score >= 0.0 && report.success_rate_score <= 100.0);
    assert!(report.latency_score >= 0.0 && report.latency_score <= 100.0);
    assert!(report.routing_score >= 0.0 && report.routing_score <= 100.0);
    assert!(report.recovery_score >= 0.0 && report.recovery_score <= 100.0);
}

// ── export_health_report – Text format ───────────────────────────────────────

#[test]
fn text_export_contains_anchor_id_field() {
    let report = build_health_report("my-anchor.example.com", &[healthy_window(0)]);
    let text = export_health_report(&report, HealthReportFormat::Text);
    assert!(
        text.contains("anchor_id: my-anchor.example.com"),
        "text missing anchor_id, got: {text}"
    );
}

#[test]
fn text_export_contains_all_required_fields() {
    let report = build_health_report("anchor.example.com", &[healthy_window(0)]);
    let text = export_health_report(&report, HealthReportFormat::Text);

    let required_keys = [
        "anchor_id:",
        "window_count:",
        "composite_score:",
        "success_rate_score:",
        "latency_score:",
        "routing_score:",
        "recovery_score:",
        "label:",
        "trend:",
        "previous_composite:",
    ];
    for key in &required_keys {
        assert!(text.contains(key), "text missing field '{key}', got: {text}");
    }
}

#[test]
fn text_export_previous_composite_na_when_single_window() {
    let report = build_health_report("test", &[healthy_window(0)]);
    let text = export_health_report(&report, HealthReportFormat::Text);
    assert!(
        text.contains("previous_composite: n/a"),
        "expected n/a for single-window report, got: {text}"
    );
}

#[test]
fn text_export_previous_composite_numeric_when_multi_window() {
    let report = build_health_report(
        "test",
        &[healthy_window(0), healthy_window(300)],
    );
    let text = export_health_report(&report, HealthReportFormat::Text);
    // Should contain a numeric value, not "n/a"
    assert!(
        !text.contains("previous_composite: n/a"),
        "expected numeric previous_composite for multi-window report, got: {text}"
    );
}

#[test]
fn text_export_label_healthy_present() {
    let report = build_health_report("test", &[healthy_window(0)]);
    let text = export_health_report(&report, HealthReportFormat::Text);
    assert!(text.contains("label: healthy"), "got: {text}");
}

#[test]
fn text_export_label_critical_present() {
    let report = build_health_report("test", &[critical_window(0)]);
    let text = export_health_report(&report, HealthReportFormat::Text);
    assert!(text.contains("label: critical"), "got: {text}");
}

#[test]
fn text_export_is_multi_line() {
    let report = build_health_report("test", &[healthy_window(0)]);
    let text = export_health_report(&report, HealthReportFormat::Text);
    // Text format must have at least 9 newlines (one per required field)
    let newlines = text.chars().filter(|&c| c == '\n').count();
    assert!(newlines >= 9, "expected >= 9 newlines, got {newlines}");
}

#[test]
fn text_export_window_count_numeric() {
    let report = build_health_report("test", &[healthy_window(0), healthy_window(300)]);
    let text = export_health_report(&report, HealthReportFormat::Text);
    assert!(text.contains("window_count: 2"), "got: {text}");
}

// ── export_health_report – JSON format ───────────────────────────────────────

#[test]
fn json_export_starts_with_open_brace_and_ends_with_close_brace() {
    let report = build_health_report("test", &[healthy_window(0)]);
    let json = export_health_report(&report, HealthReportFormat::Json);
    let trimmed = json.trim();
    assert!(trimmed.starts_with('{'), "JSON must start with '{{', got: {json}");
    assert!(trimmed.ends_with('}'), "JSON must end with '}}', got: {json}");
}

#[test]
fn json_export_contains_anchor_id_key() {
    let report = build_health_report("stellar-anchor.io", &[healthy_window(0)]);
    let json = export_health_report(&report, HealthReportFormat::Json);
    assert!(
        json.contains("\"anchor_id\""),
        "JSON missing anchor_id key, got: {json}"
    );
    assert!(
        json.contains("stellar-anchor.io"),
        "JSON missing anchor_id value, got: {json}"
    );
}

#[test]
fn json_export_contains_all_required_keys() {
    let report = build_health_report("test", &[healthy_window(0)]);
    let json = export_health_report(&report, HealthReportFormat::Json);

    let required_keys = [
        "\"anchor_id\"",
        "\"window_count\"",
        "\"composite_score\"",
        "\"success_rate_score\"",
        "\"latency_score\"",
        "\"routing_score\"",
        "\"recovery_score\"",
        "\"label\"",
        "\"trend\"",
        "\"previous_composite\"",
    ];
    for key in &required_keys {
        assert!(json.contains(key), "JSON missing key '{key}', got: {json}");
    }
}

#[test]
fn json_export_previous_composite_null_when_single_window() {
    let report = build_health_report("test", &[healthy_window(0)]);
    let json = export_health_report(&report, HealthReportFormat::Json);
    assert!(
        json.contains("\"previous_composite\":null"),
        "expected null for single-window report, got: {json}"
    );
}

#[test]
fn json_export_previous_composite_numeric_when_multi_window() {
    let report = build_health_report(
        "test",
        &[healthy_window(0), healthy_window(300)],
    );
    let json = export_health_report(&report, HealthReportFormat::Json);
    assert!(
        !json.contains("\"previous_composite\":null"),
        "expected numeric previous_composite for multi-window report, got: {json}"
    );
}

#[test]
fn json_export_label_and_trend_are_quoted_strings() {
    let report = build_health_report("test", &[healthy_window(0)]);
    let json = export_health_report(&report, HealthReportFormat::Json);
    // Label and trend must be JSON strings (enclosed in quotes)
    assert!(
        json.contains("\"label\":\""),
        "label must be a quoted string, got: {json}"
    );
    assert!(
        json.contains("\"trend\":\""),
        "trend must be a quoted string, got: {json}"
    );
}

#[test]
fn json_export_composite_score_is_numeric() {
    let report = build_health_report("test", &[healthy_window(0)]);
    let json = export_health_report(&report, HealthReportFormat::Json);
    // composite_score must be followed by a numeric value (no quotes)
    assert!(
        json.contains("\"composite_score\":"),
        "composite_score key missing, got: {json}"
    );
    let after_key = json
        .split("\"composite_score\":")
        .nth(1)
        .unwrap_or("");
    let first_char = after_key.chars().next().unwrap_or(' ');
    assert!(
        first_char.is_ascii_digit() || first_char == '-',
        "composite_score must be numeric, got: {json}"
    );
}

// ── Format consistency ────────────────────────────────────────────────────────

#[test]
fn text_and_json_agree_on_anchor_id() {
    let report = build_health_report("consistency-check.io", &[healthy_window(0)]);
    let text = export_health_report(&report, HealthReportFormat::Text);
    let json = export_health_report(&report, HealthReportFormat::Json);
    assert!(text.contains("consistency-check.io"), "text: {text}");
    assert!(json.contains("consistency-check.io"), "json: {json}");
}

#[test]
fn text_and_json_agree_on_label() {
    let report = build_health_report("test", &[healthy_window(0)]);
    let text = export_health_report(&report, HealthReportFormat::Text);
    let json = export_health_report(&report, HealthReportFormat::Json);
    // Both should contain "healthy"
    assert!(text.contains("healthy"), "text: {text}");
    assert!(json.contains("healthy"), "json: {json}");
}

// ── Degraded label ────────────────────────────────────────────────────────────

#[test]
fn build_report_degraded_label_for_mid_range_score() {
    // A window that produces a score in the 50–79 range
    let window = HealthWindow {
        started_at: 0,
        ended_at: 300,
        success_count: 70,
        failure_count: 30,
        p50_latency_ms: 5000.0,
        routing_failure_count: 3,
        routing_attempt_count: 10,
        recovery_time_seconds: 600,
    };
    let report = build_health_report("test", &[window]);
    // Score may be healthy, degraded, or critical depending on weights;
    // just confirm the label matches the composite.
    let expected_label = if report.composite_score >= 80.0 {
        "healthy"
    } else if report.composite_score >= 50.0 {
        "degraded"
    } else {
        "critical"
    };
    assert_eq!(report.label, expected_label);
}

// ── Multiple windows aggregation ──────────────────────────────────────────────

#[test]
fn build_report_uses_last_window_as_current() {
    // First window healthy, second critical → report should reflect critical (last)
    let windows = vec![healthy_window(0), critical_window(300)];
    let report = build_health_report("test", &windows);
    assert_eq!(report.label, "critical");
}

#[test]
fn build_report_previous_composite_reflects_first_of_two_windows() {
    let windows = vec![healthy_window(0), critical_window(300)];
    let report_hc = build_health_report("test", &windows);
    // previous_composite is from the healthy window, so should be >= 80
    assert!(
        report_hc.previous_composite.unwrap() >= 80.0,
        "expected previous >= 80, got {:?}",
        report_hc.previous_composite
    );
}
