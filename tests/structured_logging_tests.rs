//! Integration tests for structured logging across operational workflows (#608).
//!
//! Verifies that the instrumented workflows (attestor registration, transaction
//! status polling, webhook delivery, cache governance) emit the documented
//! event names with the documented context fields, and that the serialised
//! JSON-line format is consistent across modules.

#![cfg(not(feature = "wasm"))]

use std::collections::BTreeMap;

use anchorkit::retry::{BackoffStrategy, JitterPolicy, RetryConfig};
use anchorkit::streaming_monitor::{PollResult, StreamingTransactionMonitor};
use anchorkit::structured_log::{events, FieldValue, LogLevel, StructuredLogger};
use anchorkit::transaction_state_tracker::TransactionState;
use anchorkit::webhook::{deliver_webhook_logged, DlqEntry, WebhookDeliveryConfig};
use anchorkit::{log_attestor_registration, LogRecord};

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn webhook_config(max_attempts: u32) -> WebhookDeliveryConfig {
    WebhookDeliveryConfig {
        endpoint_url: "https://example.com/hook".to_string(),
        timeout_ms: 1000,
        retry_config: RetryConfig {
            max_attempts,
            base_delay_ms: 1,
            backoff_multiplier: 1,
            max_delay_ms: 10,
            strategy: BackoffStrategy::Exponential,
            jitter_policy: JitterPolicy::None,
        },
        dead_letter_storage_key: "dlq-key".to_string(),
        signing_key: None,
        max_payload_age_seconds: None,
        require_nonce_for_replay_protection: false,
    }
}

fn events_of(records: &[LogRecord]) -> Vec<&str> {
    records.iter().map(|r| r.event.as_str()).collect()
}

fn str_field<'a>(record: &'a LogRecord, key: &str) -> &'a str {
    match record.field(key) {
        Some(FieldValue::Str(s)) => s.as_str(),
        other => panic!("expected string field {key:?}, got {other:?}"),
    }
}

fn u64_field(record: &LogRecord, key: &str) -> u64 {
    match record.field(key) {
        Some(FieldValue::U64(n)) => *n,
        other => panic!("expected u64 field {key:?}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Cross-module format consistency
// ---------------------------------------------------------------------------

/// Every record from every instrumented workflow serialises with the same
/// JSON-line envelope: ts, seq, level, event, fields — in that order.
#[test]
fn all_workflows_share_the_same_json_envelope() {
    let logger = StructuredLogger::new();

    // Webhook workflow.
    let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
    let _ = deliver_webhook_logged(
        &webhook_config(1),
        "payload",
        &mut dlq,
        |_, _, _| Ok(200),
        |_| {},
        || 1000,
        &logger,
    );

    // Polling workflow.
    let mut monitor = StreamingTransactionMonitor::new(7, 0);
    monitor.run_logged(
        |_| {
            Ok(PollResult::Completed {
                stellar_tx_id: "tx".to_string(),
            })
        },
        |_| {},
        |_| {},
        || 2000,
        &logger,
    );

    // Attestor workflow.
    let _: Result<(), &str> =
        log_attestor_registration(&logger, 3000, "GATTESTOR", "GISSUER", || Ok(()));

    let lines = logger.json_lines();
    assert!(lines.len() >= 5);
    for (i, line) in lines.iter().enumerate() {
        assert!(
            line.starts_with(&format!("{{\"ts\":")),
            "line {i} missing ts prefix: {line}"
        );
        let seq_marker = format!("\"seq\":{i}");
        assert!(line.contains(&seq_marker), "line {i} missing {seq_marker}: {line}");
        assert!(line.contains("\"level\":\""), "line {i} missing level: {line}");
        assert!(line.contains("\"event\":\""), "line {i} missing event: {line}");
        assert!(line.contains("\"fields\":{"), "line {i} missing fields: {line}");
        assert!(line.ends_with("}}"), "line {i} bad terminator: {line}");
    }
}

// ---------------------------------------------------------------------------
// Webhook delivery
// ---------------------------------------------------------------------------

#[test]
fn webhook_success_logs_started_then_succeeded() {
    let logger = StructuredLogger::new();
    let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

    let result = deliver_webhook_logged(
        &webhook_config(3),
        "payload",
        &mut dlq,
        |_, _, _| Ok(200),
        |_| {},
        || 1000,
        &logger,
    );

    assert!(result.is_ok());
    let records = logger.records();
    assert_eq!(
        events_of(&records),
        vec![
            events::WEBHOOK_DELIVERY_STARTED,
            events::WEBHOOK_DELIVERY_SUCCEEDED,
        ]
    );
    assert_eq!(str_field(&records[0], "endpoint_url"), "https://example.com/hook");
    assert_eq!(u64_field(&records[0], "max_attempts"), 3);
    assert_eq!(records[0].field("signed"), Some(&FieldValue::Bool(false)));
    assert_eq!(u64_field(&records[1], "attempts"), 1);
}

#[test]
fn webhook_exhaustion_logs_each_attempt_then_failure_and_dlq_depth() {
    let logger = StructuredLogger::new();
    let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

    let result = deliver_webhook_logged(
        &webhook_config(2),
        "payload",
        &mut dlq,
        |_, _, _| Ok(503),
        |_| {},
        || 9999,
        &logger,
    );

    assert!(result.is_err());
    let records = logger.records();
    assert_eq!(
        events_of(&records),
        vec![
            events::WEBHOOK_DELIVERY_STARTED,
            events::WEBHOOK_DELIVERY_ATTEMPT_FAILED,
            events::WEBHOOK_DELIVERY_ATTEMPT_FAILED,
            events::WEBHOOK_DELIVERY_FAILED,
            events::WEBHOOK_DLQ_ENTRY_ADDED,
        ]
    );

    // Per-attempt context.
    assert_eq!(u64_field(&records[1], "attempt"), 1);
    assert_eq!(u64_field(&records[2], "attempt"), 2);
    assert_eq!(u64_field(&records[1], "status"), 503);
    assert_eq!(str_field(&records[1], "error"), "HTTP 503");
    assert_eq!(records[1].level, LogLevel::Warn);

    // Terminal failure context matches the DLQ entry that was written.
    assert_eq!(records[3].level, LogLevel::Error);
    assert_eq!(u64_field(&records[3], "last_status"), 503);
    assert_eq!(u64_field(&records[4], "dlq_depth"), 1);
    assert_eq!(str_field(&records[4], "dlq_key"), "dlq-key");
    assert_eq!(dlq.get("dlq-key").map(Vec::len), Some(1));
}

#[test]
fn webhook_transport_error_logs_status_zero() {
    let logger = StructuredLogger::new();
    let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

    let _ = deliver_webhook_logged(
        &webhook_config(1),
        "payload",
        &mut dlq,
        |_, _, _| Err("connection refused".to_string()),
        |_| {},
        || 42,
        &logger,
    );

    let records = logger.records();
    let attempt = records
        .iter()
        .find(|r| r.event == events::WEBHOOK_DELIVERY_ATTEMPT_FAILED)
        .expect("attempt record");
    assert_eq!(u64_field(attempt, "status"), 0);
    assert_eq!(str_field(attempt, "error"), "connection refused");
}

/// The logged wrapper must not change delivery behaviour: same result, same
/// DLQ contents as the plain `deliver_webhook`.
#[test]
fn webhook_logged_wrapper_is_behaviour_preserving() {
    let logger = StructuredLogger::new();
    let mut dlq_logged: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
    let mut dlq_plain: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

    let logged = deliver_webhook_logged(
        &webhook_config(2),
        "payload",
        &mut dlq_logged,
        |_, _, _| Ok(500),
        |_| {},
        || 7,
        &logger,
    );
    let plain = anchorkit::webhook::deliver_webhook(
        &webhook_config(2),
        "payload",
        &mut dlq_plain,
        |_, _, _| Ok(500),
        |_| {},
        || 7,
    );

    assert_eq!(logged.is_err(), plain.is_err());
    assert_eq!(dlq_logged, dlq_plain);
}

// ---------------------------------------------------------------------------
// Transaction status polling
// ---------------------------------------------------------------------------

#[test]
fn polling_logs_monitor_start_state_changes_and_completion() {
    let logger = StructuredLogger::new();
    let mut monitor = StreamingTransactionMonitor::new(42, 250);
    let states = vec![
        PollResult::Pending(TransactionState::Pending),
        PollResult::Pending(TransactionState::InProgress),
        PollResult::Completed {
            stellar_tx_id: "HASH123".to_string(),
        },
    ];
    let mut idx = 0usize;

    monitor.run_logged(
        |_| {
            let s = states[idx.min(states.len() - 1)].clone();
            idx += 1;
            Ok(s)
        },
        |_| {},
        |_| {},
        || 1234,
        &logger,
    );

    let records = logger.records();
    assert_eq!(records[0].event, events::TXSTATUS_MONITOR_STARTED);
    assert_eq!(u64_field(&records[0], "transaction_id"), 42);
    assert_eq!(u64_field(&records[0], "poll_interval_ms"), 250);

    let changes: Vec<&LogRecord> = records
        .iter()
        .filter(|r| r.event == events::TXSTATUS_STATE_CHANGED)
        .collect();
    assert_eq!(changes.len(), 2);
    assert_eq!(str_field(changes[0], "from"), "Pending");
    assert_eq!(str_field(changes[0], "to"), "InProgress");
    assert_eq!(str_field(changes[1], "to"), "Completed");

    let done = records.last().unwrap();
    assert_eq!(done.event, events::TXSTATUS_COMPLETED);
    assert_eq!(str_field(done, "stellar_tx_id"), "HASH123");
}

#[test]
fn polling_failure_logs_poll_errors_then_terminal_failure() {
    let logger = StructuredLogger::new();
    let mut monitor = StreamingTransactionMonitor::new(1, 0)
        .with_retry(RetryConfig::new(2, 0, 0, 1));

    monitor.run_logged(
        |_| Err("rpc unreachable".to_string()),
        |_| {},
        |_| {},
        || 5,
        &logger,
    );

    let records = logger.records();
    let poll_errors: Vec<&LogRecord> = records
        .iter()
        .filter(|r| r.event == events::TXSTATUS_POLL_ERROR)
        .collect();
    assert!(!poll_errors.is_empty());
    assert_eq!(str_field(poll_errors[0], "error"), "rpc unreachable");
    assert_eq!(u64_field(poll_errors[0], "consecutive_errors"), 1);
    assert_eq!(poll_errors[0].level, LogLevel::Warn);

    let terminal = records.last().unwrap();
    assert_eq!(terminal.event, events::TXSTATUS_FAILED);
    assert_eq!(terminal.level, LogLevel::Error);
    assert_eq!(str_field(terminal, "reason"), "rpc unreachable");
}

/// The logged wrapper delegates to `run`, so downstream event consumers and
/// transition tracking behave exactly as without logging.
#[test]
fn polling_logged_wrapper_preserves_events_and_transitions() {
    let states = vec![
        PollResult::Pending(TransactionState::Pending),
        PollResult::Pending(TransactionState::InProgress),
        PollResult::Completed {
            stellar_tx_id: "tx".to_string(),
        },
    ];

    let logger = StructuredLogger::new();
    let mut logged_monitor = StreamingTransactionMonitor::new(1, 0);
    let mut idx = 0usize;
    let mut logged_events = Vec::new();
    logged_monitor.run_logged(
        |_| {
            let s = states[idx.min(states.len() - 1)].clone();
            idx += 1;
            Ok(s)
        },
        |e| logged_events.push(e),
        |_| {},
        || 0,
        &logger,
    );

    let mut plain_monitor = StreamingTransactionMonitor::new(1, 0);
    let mut idx = 0usize;
    let mut plain_events = Vec::new();
    plain_monitor.run(
        |_| {
            let s = states[idx.min(states.len() - 1)].clone();
            idx += 1;
            Ok(s)
        },
        |e| plain_events.push(e),
        |_| {},
        || 0,
    );

    assert_eq!(logged_events, plain_events);
    assert_eq!(logged_monitor.get_transitions(), plain_monitor.get_transitions());
}

// ---------------------------------------------------------------------------
// Attestor registration
// ---------------------------------------------------------------------------

#[test]
fn attestor_registration_workflow_logs_lifecycle() {
    let logger = StructuredLogger::new();

    let ok: Result<(), &str> =
        log_attestor_registration(&logger, 100, "GABC", "GISSUER", || Ok(()));
    assert!(ok.is_ok());

    let err: Result<(), &str> =
        log_attestor_registration(&logger, 200, "GDEF", "GISSUER", || Err("capacity exceeded"));
    assert!(err.is_err());

    let records = logger.records();
    assert_eq!(
        events_of(&records),
        vec![
            events::ATTESTOR_REGISTRATION_STARTED,
            events::ATTESTOR_REGISTRATION_SUCCEEDED,
            events::ATTESTOR_REGISTRATION_STARTED,
            events::ATTESTOR_REGISTRATION_FAILED,
        ]
    );
    assert_eq!(str_field(&records[0], "attestor"), "GABC");
    assert_eq!(str_field(&records[0], "sep10_issuer"), "GISSUER");
    assert_eq!(records[3].level, LogLevel::Error);
    assert!(str_field(&records[3], "error").contains("capacity exceeded"));
}

// ---------------------------------------------------------------------------
// Cache governance
// ---------------------------------------------------------------------------

mod cache_governance_logging {
    use super::*;
    use anchorkit::cache_governance::{
        self, CacheEntryType, CachePolicy, CachePolicySet,
    };
    use anchorkit::contract::AnchorKitContract;

    fn make_env() -> (Env, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let cid = env.register_contract(None, AnchorKitContract);
        (env, cid)
    }

    #[test]
    fn proposal_lifecycle_emits_created_endorsed_executed() {
        let (env, cid) = make_env();
        let logger = StructuredLogger::new();
        let proposer = Address::generate(&env);
        let e2 = Address::generate(&env);
        let e3 = Address::generate(&env);
        let anchor = Address::generate(&env);

        env.as_contract(&cid, || {
            let pid = cache_governance::propose_logged(&env, &proposer, &anchor, &logger);
            cache_governance::endorse_logged(&env, &e2, pid, &logger).unwrap();
            cache_governance::endorse_logged(&env, &e3, pid, &logger).unwrap();
            let executed_anchor = cache_governance::execute_logged(&env, pid, &logger).unwrap();
            assert_eq!(executed_anchor, anchor);
        });

        let records = logger.records();
        assert_eq!(
            events_of(&records),
            vec![
                events::CACHE_PROPOSAL_CREATED,
                events::CACHE_PROPOSAL_ENDORSED,
                events::CACHE_PROPOSAL_ENDORSED,
                events::CACHE_PROPOSAL_EXECUTED,
            ]
        );
        assert_eq!(u64_field(&records[0], "proposal_id"), 0);
        assert!(!str_field(&records[0], "anchor").is_empty());
        assert!(!str_field(&records[0], "proposer").is_empty());
        // Endorsement counts include the proposer's auto-endorsement.
        assert_eq!(u64_field(&records[1], "endorsement_count"), 2);
        assert_eq!(u64_field(&records[2], "endorsement_count"), 3);
        assert_eq!(str_field(&records[3], "anchor"), str_field(&records[0], "anchor"));
    }

    #[test]
    fn endorsing_missing_proposal_logs_warn_failure() {
        let (env, cid) = make_env();
        let logger = StructuredLogger::new();
        let endorser = Address::generate(&env);

        env.as_contract(&cid, || {
            let result = cache_governance::endorse_logged(&env, &endorser, 999, &logger);
            assert!(result.is_err());
        });

        let records = logger.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].event, events::CACHE_PROPOSAL_ENDORSE_FAILED);
        assert_eq!(records[0].level, LogLevel::Warn);
        assert_eq!(u64_field(&records[0], "proposal_id"), 999);
        assert!(!str_field(&records[0], "error").is_empty());
    }

    #[test]
    fn ttl_clamp_is_logged_but_in_band_ttl_is_silent() {
        let (env, cid) = make_env();
        let logger = StructuredLogger::new();

        env.as_contract(&cid, || {
            // In-band TTL (metadata band is [60, 86400]): no log entry.
            let (ttl, _) = cache_governance::enforce_write_policy_logged(
                &env,
                CacheEntryType::Metadata,
                3_600,
                0,
                &logger,
            );
            assert_eq!(ttl, 3_600);
            assert!(logger.is_empty());

            // Out-of-band TTL: clamped and logged.
            let (ttl, _) = cache_governance::enforce_write_policy_logged(
                &env,
                CacheEntryType::Metadata,
                999_999_999,
                0,
                &logger,
            );
            assert_eq!(ttl, 86_400);
        });

        let records = logger.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].event, events::CACHE_TTL_CLAMPED);
        assert_eq!(records[0].level, LogLevel::Warn);
        assert_eq!(str_field(&records[0], "entry_type"), "metadata");
        assert_eq!(u64_field(&records[0], "requested_ttl_seconds"), 999_999_999);
        assert_eq!(u64_field(&records[0], "effective_ttl_seconds"), 86_400);
    }

    #[test]
    fn denied_forced_invalidation_is_logged() {
        let (env, cid) = make_env();
        let logger = StructuredLogger::new();

        env.as_contract(&cid, || {
            // The default "other" policy forbids forced invalidation.
            let result = cache_governance::enforce_invalidation_policy_logged(
                &env,
                CacheEntryType::Other,
                &logger,
            );
            assert!(result.is_err());

            // Metadata allows it: no extra log entry.
            let result = cache_governance::enforce_invalidation_policy_logged(
                &env,
                CacheEntryType::Metadata,
                &logger,
            );
            assert!(result.is_ok());
        });

        let records = logger.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].event, events::CACHE_INVALIDATION_DENIED);
        assert_eq!(str_field(&records[0], "entry_type"), "other");
    }

    #[test]
    fn policy_update_and_rejection_are_logged() {
        let (env, cid) = make_env();
        let logger = StructuredLogger::new();

        env.as_contract(&cid, || {
            // Valid set: updated.
            let result = cache_governance::set_policy_set_logged(
                &env,
                CachePolicySet::default_set(),
                &logger,
            );
            assert!(result.is_ok());

            // Invalid set (min >= max): rejected.
            let mut bad = CachePolicySet::default_set();
            bad.metadata = CachePolicy {
                min_ttl_seconds: 100,
                max_ttl_seconds: 50,
                refresh_threshold_pct: 80,
                allow_forced_invalidation: true,
            };
            let result = cache_governance::set_policy_set_logged(&env, bad, &logger);
            assert!(result.is_err());
        });

        let records = logger.records();
        assert_eq!(
            events_of(&records),
            vec![events::CACHE_POLICY_UPDATED, events::CACHE_POLICY_REJECTED]
        );
        assert_eq!(u64_field(&records[0], "metadata_max_ttl_seconds"), 86_400);
        assert_eq!(records[1].level, LogLevel::Warn);
    }
}
