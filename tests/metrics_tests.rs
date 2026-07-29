#![cfg(test)]

//! Integration tests for the host-side metrics layer (issue #609).
//!
//! Verifies that the registry primitives behave (counters, gauges, latency
//! summaries, namespacing) and that the `*_metered` wrappers emit metrics
//! reflecting the actual behavior of retries, webhook delivery and outbound
//! HTTP requests.

mod metrics_tests {
    use std::collections::BTreeMap;

    use anchorkit::{
        http_client::{post_with_options_metered, OutboundRequestOptions},
        metrics::{names, MetricsRegistry},
        retry::{retry_with_backoff_metered, MockJitterSource, RetryConfig},
        webhook::{deliver_webhook_metered, DlqEntry, WebhookDeliveryConfig},
    };

    fn webhook_config(max_attempts: u32) -> WebhookDeliveryConfig {
        WebhookDeliveryConfig {
            endpoint_url: "https://example.com/hook".into(),
            timeout_ms: 1000,
            retry_config: RetryConfig::new(max_attempts, 0, 0, 1),
            dead_letter_storage_key: "test_dlq".into(),
            signing_key: None,
            max_payload_age_seconds: None,
            require_nonce_for_replay_protection: false,
        }
    }

    // -----------------------------------------------------------------------
    // Registry primitives
    // -----------------------------------------------------------------------

    #[test]
    fn counter_starts_at_zero_and_increments() {
        let metrics = MetricsRegistry::new();
        assert_eq!(metrics.counter("some.counter"), 0);

        metrics.incr("some.counter");
        metrics.incr_by("some.counter", 4);
        assert_eq!(metrics.counter("some.counter"), 5);

        // Unrelated names stay independent.
        assert_eq!(metrics.counter("other.counter"), 0);
    }

    #[test]
    fn counter_saturates_instead_of_overflowing() {
        let metrics = MetricsRegistry::new();
        metrics.incr_by("near.max", u64::MAX - 1);
        metrics.incr_by("near.max", 10);
        assert_eq!(metrics.counter("near.max"), u64::MAX);
    }

    #[test]
    fn gauge_set_and_overwrite() {
        let metrics = MetricsRegistry::new();
        assert_eq!(metrics.gauge("queue.depth"), None);

        metrics.set_gauge("queue.depth", 7);
        assert_eq!(metrics.gauge("queue.depth"), Some(7));

        // Gauges are point-in-time values: last write wins, including zero.
        metrics.set_gauge("queue.depth", 0);
        assert_eq!(metrics.gauge("queue.depth"), Some(0));
    }

    #[test]
    fn latency_summary_tracks_count_avg_and_max() {
        let metrics = MetricsRegistry::new();
        assert!(metrics.latency("op.latency").is_none());

        metrics.observe_latency_ms("op.latency", 10);
        metrics.observe_latency_ms("op.latency", 20);
        metrics.observe_latency_ms("op.latency", 40);

        let summary = metrics.latency("op.latency").expect("latency recorded");
        assert_eq!(summary.count, 3);
        assert_eq!(summary.total_ms, 70);
        assert_eq!(summary.avg_ms(), 23);
        assert_eq!(summary.max_ms, 40);
    }

    #[test]
    fn namespace_prefixes_every_metric_name() {
        let metrics = MetricsRegistry::with_namespace("anchorkit.test");
        assert_eq!(metrics.namespace(), Some("anchorkit.test"));

        metrics.incr(names::HTTP_REQUESTS);
        metrics.set_gauge(names::WEBHOOK_DLQ_DEPTH, 3);
        metrics.observe_latency_ms("op.latency", 5);

        // Reads through the registry resolve the same qualified name.
        assert_eq!(metrics.counter(names::HTTP_REQUESTS), 1);

        // The snapshot exposes fully-qualified names for export.
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.counters.get("anchorkit.test.http.requests"), Some(&1));
        assert_eq!(snapshot.gauges.get("anchorkit.test.webhook.dlq_depth"), Some(&3));
        assert!(snapshot.latencies.contains_key("anchorkit.test.op.latency"));
    }

    #[test]
    fn record_call_counts_contract_call_outcomes() {
        let metrics = MetricsRegistry::new();

        metrics.record_call(names::CONTRACT_CALL, true);
        metrics.record_call(names::CONTRACT_CALL, true);
        metrics.record_call(names::CONTRACT_CALL, false);

        assert_eq!(metrics.counter(&names::calls(names::CONTRACT_CALL)), 3);
        assert_eq!(metrics.counter(&names::successes(names::CONTRACT_CALL)), 2);
        assert_eq!(metrics.counter(&names::failures(names::CONTRACT_CALL)), 1);
    }

    #[test]
    fn snapshot_copies_all_metric_kinds() {
        let metrics = MetricsRegistry::new();
        metrics.incr("a.counter");
        metrics.set_gauge("a.gauge", 9);
        metrics.observe_latency_ms("a.latency", 12);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.counters.get("a.counter"), Some(&1));
        assert_eq!(snapshot.gauges.get("a.gauge"), Some(&9));
        assert_eq!(snapshot.latencies.get("a.latency").map(|s| s.max_ms), Some(12));

        // The snapshot is a copy, not a live view.
        metrics.incr("a.counter");
        assert_eq!(snapshot.counters.get("a.counter"), Some(&1));
        assert_eq!(metrics.counter("a.counter"), 2);
    }

    // -----------------------------------------------------------------------
    // Retry instrumentation
    // -----------------------------------------------------------------------

    #[test]
    fn retry_metered_success_on_first_attempt() {
        let metrics = MetricsRegistry::new();
        let config = RetryConfig::new(3, 0, 0, 1);
        let mut jitter = MockJitterSource::new(vec![0, 0, 0, 0]);

        let result: Result<u32, &str> = retry_with_backoff_metered(
            &config,
            |_attempt| Ok(42),
            |_e| true,
            |_ms| {},
            &mut jitter,
            &metrics,
            "toml_fetch",
        );

        assert_eq!(result, Ok(42));
        assert_eq!(metrics.counter(&names::retry_attempts("toml_fetch")), 1);
        assert_eq!(metrics.counter(&names::retry_backoffs("toml_fetch")), 0);
        assert_eq!(metrics.counter(&names::retry_successes("toml_fetch")), 1);
        assert_eq!(metrics.counter(&names::retry_failures("toml_fetch")), 0);
    }

    #[test]
    fn retry_metered_counts_each_attempt_and_backoff() {
        let metrics = MetricsRegistry::new();
        let config = RetryConfig::new(5, 0, 0, 1);
        let mut jitter = MockJitterSource::new(vec![0, 0, 0, 0, 0, 0]);

        // Fail twice, then succeed on the third attempt.
        let result: Result<u32, &str> = retry_with_backoff_metered(
            &config,
            |attempt| if attempt < 2 { Err("transient") } else { Ok(7) },
            |_e| true,
            |_ms| {},
            &mut jitter,
            &metrics,
            "status_poll",
        );

        assert_eq!(result, Ok(7));
        assert_eq!(metrics.counter(&names::retry_attempts("status_poll")), 3);
        assert_eq!(metrics.counter(&names::retry_backoffs("status_poll")), 2);
        assert_eq!(metrics.counter(&names::retry_successes("status_poll")), 1);
        assert_eq!(metrics.counter(&names::retry_failures("status_poll")), 0);
    }

    #[test]
    fn retry_metered_exhaustion_records_failure() {
        let metrics = MetricsRegistry::new();
        let config = RetryConfig::new(3, 0, 0, 1);
        let mut jitter = MockJitterSource::new(vec![0, 0, 0, 0]);

        let result: Result<u32, &str> = retry_with_backoff_metered(
            &config,
            |_attempt| Err("still down"),
            |_e| true,
            |_ms| {},
            &mut jitter,
            &metrics,
            "status_poll",
        );

        assert_eq!(result, Err("still down"));
        assert_eq!(metrics.counter(&names::retry_attempts("status_poll")), 3);
        // Backoff sleeps happen between attempts, never after the last one.
        assert_eq!(metrics.counter(&names::retry_backoffs("status_poll")), 2);
        assert_eq!(metrics.counter(&names::retry_successes("status_poll")), 0);
        assert_eq!(metrics.counter(&names::retry_failures("status_poll")), 1);
    }

    #[test]
    fn retry_metered_non_retryable_fails_without_backoff() {
        let metrics = MetricsRegistry::new();
        let config = RetryConfig::new(3, 0, 0, 1);
        let mut jitter = MockJitterSource::new(vec![0, 0, 0, 0]);

        let result: Result<u32, &str> = retry_with_backoff_metered(
            &config,
            |_attempt| Err("fatal"),
            |_e| false,
            |_ms| {},
            &mut jitter,
            &metrics,
            "status_poll",
        );

        assert_eq!(result, Err("fatal"));
        assert_eq!(metrics.counter(&names::retry_attempts("status_poll")), 1);
        assert_eq!(metrics.counter(&names::retry_backoffs("status_poll")), 0);
        assert_eq!(metrics.counter(&names::retry_failures("status_poll")), 1);
    }

    #[test]
    fn retry_metered_keeps_operations_independent() {
        let metrics = MetricsRegistry::new();
        let config = RetryConfig::new(3, 0, 0, 1);

        let mut jitter = MockJitterSource::new(vec![0, 0, 0, 0]);
        let _: Result<u32, &str> = retry_with_backoff_metered(
            &config, |_| Ok(1), |_e| true, |_ms| {}, &mut jitter, &metrics, "op_a",
        );
        let mut jitter = MockJitterSource::new(vec![0, 0, 0, 0]);
        let _: Result<u32, &str> = retry_with_backoff_metered(
            &config, |_| Err("x"), |_e| true, |_ms| {}, &mut jitter, &metrics, "op_b",
        );

        assert_eq!(metrics.counter(&names::retry_successes("op_a")), 1);
        assert_eq!(metrics.counter(&names::retry_failures("op_a")), 0);
        assert_eq!(metrics.counter(&names::retry_successes("op_b")), 0);
        assert_eq!(metrics.counter(&names::retry_failures("op_b")), 1);
    }

    // -----------------------------------------------------------------------
    // Webhook delivery instrumentation
    // -----------------------------------------------------------------------

    #[test]
    fn webhook_metered_success_records_delivery_and_attempt() {
        let metrics = MetricsRegistry::new();
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

        let result = deliver_webhook_metered(
            &webhook_config(3),
            r#"{"event":"tx"}"#,
            &mut dlq,
            |_url, _body, _sig| Ok(200),
            |_ms| {},
            || 1_700_000_000,
            &metrics,
        );

        assert!(result.is_ok());
        assert_eq!(metrics.counter(names::WEBHOOK_DELIVERIES), 1);
        assert_eq!(metrics.counter(names::WEBHOOK_ATTEMPTS), 1);
        assert_eq!(metrics.counter(names::WEBHOOK_SUCCESSES), 1);
        assert_eq!(metrics.counter(names::WEBHOOK_FAILURES), 0);
        assert_eq!(metrics.counter(names::WEBHOOK_DLQ_ENTRIES), 0);
        assert_eq!(metrics.gauge(names::WEBHOOK_DLQ_DEPTH), Some(0));
    }

    #[test]
    fn webhook_metered_retry_then_success_counts_every_attempt() {
        let metrics = MetricsRegistry::new();
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let attempts = std::cell::Cell::new(0u32);

        let result = deliver_webhook_metered(
            &webhook_config(3),
            r#"{"event":"tx"}"#,
            &mut dlq,
            |_url, _body, _sig| {
                attempts.set(attempts.get() + 1);
                if attempts.get() < 3 { Ok(503) } else { Ok(204) }
            },
            |_ms| {},
            || 1_700_000_000,
            &metrics,
        );

        assert!(result.is_ok());
        assert_eq!(metrics.counter(names::WEBHOOK_ATTEMPTS), 3);
        assert_eq!(metrics.counter(names::WEBHOOK_SUCCESSES), 1);
        assert_eq!(metrics.counter(names::WEBHOOK_FAILURES), 0);
        assert_eq!(metrics.gauge(names::WEBHOOK_DLQ_DEPTH), Some(0));
    }

    #[test]
    fn webhook_metered_failure_records_dlq_growth() {
        let metrics = MetricsRegistry::new();
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

        let result = deliver_webhook_metered(
            &webhook_config(3),
            r#"{"event":"tx"}"#,
            &mut dlq,
            |_url, _body, _sig| Ok(500),
            |_ms| {},
            || 1_700_000_000,
            &metrics,
        );

        assert!(result.is_err());
        assert_eq!(metrics.counter(names::WEBHOOK_DELIVERIES), 1);
        assert_eq!(metrics.counter(names::WEBHOOK_ATTEMPTS), 3);
        assert_eq!(metrics.counter(names::WEBHOOK_SUCCESSES), 0);
        assert_eq!(metrics.counter(names::WEBHOOK_FAILURES), 1);
        assert_eq!(metrics.counter(names::WEBHOOK_DLQ_ENTRIES), 1);
        assert_eq!(metrics.gauge(names::WEBHOOK_DLQ_DEPTH), Some(1));

        // A second failed delivery deepens the DLQ and the gauge follows.
        let result = deliver_webhook_metered(
            &webhook_config(3),
            r#"{"event":"tx2"}"#,
            &mut dlq,
            |_url, _body, _sig| Err("connection refused".into()),
            |_ms| {},
            || 1_700_000_100,
            &metrics,
        );

        assert!(result.is_err());
        assert_eq!(metrics.counter(names::WEBHOOK_DELIVERIES), 2);
        assert_eq!(metrics.counter(names::WEBHOOK_FAILURES), 2);
        assert_eq!(metrics.counter(names::WEBHOOK_DLQ_ENTRIES), 2);
        assert_eq!(metrics.gauge(names::WEBHOOK_DLQ_DEPTH), Some(2));
    }

    // -----------------------------------------------------------------------
    // Outbound HTTP instrumentation
    // -----------------------------------------------------------------------

    #[test]
    fn http_post_metered_classifies_success() {
        let metrics = MetricsRegistry::new();
        let opts = OutboundRequestOptions::from_seed("metrics-test");

        let result = post_with_options_metered(
            "https://anchor.example/tx",
            r#"{"id":1}"#,
            Some(&opts),
            |_url, _body, _headers| Ok(201),
            &metrics,
        );

        assert_eq!(result, Ok(201));
        assert_eq!(metrics.counter(names::HTTP_REQUESTS), 1);
        assert_eq!(metrics.counter(names::HTTP_SUCCESSES), 1);
        assert_eq!(metrics.counter(names::HTTP_ERROR_RESPONSES), 0);
        assert_eq!(metrics.counter(names::HTTP_TRANSPORT_ERRORS), 0);
    }

    #[test]
    fn http_post_metered_classifies_error_response() {
        let metrics = MetricsRegistry::new();

        let result = post_with_options_metered(
            "https://anchor.example/tx",
            r#"{"id":1}"#,
            None,
            |_url, _body, _headers| Ok(503),
            &metrics,
        );

        assert_eq!(result, Ok(503));
        assert_eq!(metrics.counter(names::HTTP_REQUESTS), 1);
        assert_eq!(metrics.counter(names::HTTP_SUCCESSES), 0);
        assert_eq!(metrics.counter(names::HTTP_ERROR_RESPONSES), 1);
        assert_eq!(metrics.counter(names::HTTP_TRANSPORT_ERRORS), 0);
    }

    #[test]
    fn http_post_metered_classifies_transport_error() {
        let metrics = MetricsRegistry::new();

        let result = post_with_options_metered(
            "https://anchor.example/tx",
            r#"{"id":1}"#,
            None,
            |_url, _body, _headers| Err("dns failure".into()),
            &metrics,
        );

        assert_eq!(result, Err("dns failure".into()));
        assert_eq!(metrics.counter(names::HTTP_REQUESTS), 1);
        assert_eq!(metrics.counter(names::HTTP_TRANSPORT_ERRORS), 1);
    }

    // -----------------------------------------------------------------------
    // Runtime-config integration (std feature only)
    // -----------------------------------------------------------------------

    #[cfg(feature = "std")]
    mod std_only {
        use anchorkit::config::MonitoringConfig;
        use anchorkit::metrics::{names, time_operation, MetricsRegistry};

        fn monitoring(enable: Option<bool>, namespace: Option<&str>) -> MonitoringConfig {
            MonitoringConfig {
                enable_metrics: enable,
                log_all_operations: None,
                alert_on_failed_attestations: None,
                alert_on_replay_attempts: None,
                metrics_namespace: namespace.map(Into::into),
                alerts: None,
            }
        }

        #[test]
        fn from_monitoring_config_is_opt_in() {
            assert!(MetricsRegistry::from_monitoring_config(None).is_none());
            assert!(
                MetricsRegistry::from_monitoring_config(Some(&monitoring(None, None))).is_none()
            );
            assert!(
                MetricsRegistry::from_monitoring_config(Some(&monitoring(Some(false), None)))
                    .is_none()
            );
            assert!(
                MetricsRegistry::from_monitoring_config(Some(&monitoring(Some(true), None)))
                    .is_some()
            );
        }

        #[test]
        fn from_monitoring_config_adopts_namespace() {
            let cfg = monitoring(Some(true), Some("anchorkit.fiat_ramp"));
            let metrics = MetricsRegistry::from_monitoring_config(Some(&cfg)).expect("enabled");
            assert_eq!(metrics.namespace(), Some("anchorkit.fiat_ramp"));

            metrics.incr(names::HTTP_REQUESTS);
            let snapshot = metrics.snapshot();
            assert_eq!(
                snapshot.counters.get("anchorkit.fiat_ramp.http.requests"),
                Some(&1)
            );
        }

        #[test]
        fn time_operation_records_latency_and_passes_through() {
            let metrics = MetricsRegistry::new();
            let out = time_operation(&metrics, "op.latency", || 5u32);
            assert_eq!(out, 5);

            let summary = metrics.latency("op.latency").expect("latency recorded");
            assert_eq!(summary.count, 1);
        }
    }
}
