#![cfg(test)]

mod webhook_middleware_tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use anchorkit::{
        errors::ErrorCode,
        retry::RetryConfig,
        webhook::{deliver_webhook, get_dead_letter_webhooks, verify_webhook_signature, DlqEntry, WebhookDeliveryConfig},
    };

    fn config(max_retries: u32) -> WebhookDeliveryConfig {
        WebhookDeliveryConfig {
            endpoint_url: "https://example.com/hook".into(),
            timeout_ms: 1000,
            retry_config: RetryConfig::new(max_retries, 0, 0, 1),
            dead_letter_storage_key: "test_dlq".into(),
            signing_key: None,
            max_payload_age_seconds: None,
            require_nonce_for_replay_protection: false,
        }
    }

    fn signed_config(max_retries: u32, key: Vec<u8>) -> WebhookDeliveryConfig {
        WebhookDeliveryConfig {
            endpoint_url: "https://example.com/hook".into(),
            timeout_ms: 1000,
            retry_config: RetryConfig::new(max_retries, 0, 0, 1),
            dead_letter_storage_key: "test_dlq".into(),
            signing_key: Some(key),
            max_payload_age_seconds: None,
            require_nonce_for_replay_protection: false,
        }
    }

    // -----------------------------------------------------------------------
    // 1. Immediate success — no retries triggered
    // -----------------------------------------------------------------------
    #[test]
    fn test_immediate_success_no_retries() {
        let call_count = Arc::new(Mutex::new(0u32));
        let cc = call_count.clone();

        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let result = deliver_webhook(
            &config(3),
            r#"{"event":"deposit"}"#,
            &mut dlq,
            move |_url, _body, _sig| {
                *cc.lock().unwrap() += 1;
                Ok(200)
            },
            |_| {},
            || 1_000_000u64,
        );

        assert!(result.is_ok());
        assert_eq!(*call_count.lock().unwrap(), 1, "should call HTTP exactly once");
        assert!(dlq.is_empty(), "DLQ must be empty on success");
    }

    // -----------------------------------------------------------------------
    // 2. Two 503s then 200 — succeeds on 3rd attempt
    // -----------------------------------------------------------------------
    #[test]
    fn test_success_after_two_failures() {
        let call_count = Arc::new(Mutex::new(0u32));
        let cc = call_count.clone();

        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let result = deliver_webhook(
            &config(3),
            r#"{"event":"withdrawal"}"#,
            &mut dlq,
            move |_url, _body, _sig| {
                let mut n = cc.lock().unwrap();
                *n += 1;
                if *n < 3 { Ok(503) } else { Ok(200) }
            },
            |_| {},
            || 1_000_000u64,
        );

        assert!(result.is_ok(), "expected success on 3rd attempt, got {:?}", result);
        assert_eq!(*call_count.lock().unwrap(), 3);
        assert!(dlq.is_empty(), "DLQ must be empty when delivery eventually succeeds");
    }

    // -----------------------------------------------------------------------
    // 3. All retries exhausted — payload lands in DLQ
    // -----------------------------------------------------------------------
    #[test]
    fn test_exhausted_retries_writes_to_dlq() {
        let payload = r#"{"event":"kyc_failed"}"#;
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();

        let result = deliver_webhook(
            &config(3),
            payload,
            &mut dlq,
            |_url, _body, _sig| Ok(503u16),
            |_| {},
            || 1_000_000u64,
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::WebhookDeliveryFailed);
        let ctx = err.context.expect("context must be set");
        assert!(ctx.contains("attempts_made=3"), "context: {ctx}");

        let entries = get_dead_letter_webhooks(&dlq, "test_dlq");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].payload, payload);
        assert_eq!(entries[0].attempts_made, 3);
        assert_eq!(entries[0].last_status_code, 503);
    }

    // -----------------------------------------------------------------------
    // 4. Admin inspection — get_dead_letter_webhooks returns all failed payloads
    // -----------------------------------------------------------------------
    #[test]
    fn test_admin_can_inspect_dlq() {
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let payloads = [r#"{"event":"a"}"#, r#"{"event":"b"}"#];

        for p in &payloads {
            let _ = deliver_webhook(
                &config(1),
                p,
                &mut dlq,
                |_url, _body, _sig| Ok(500u16),
                |_| {},
                || 1_000_000u64,
            );
        }

        let entries = get_dead_letter_webhooks(&dlq, "test_dlq");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].payload, payloads[0]);
        assert_eq!(entries[1].payload, payloads[1]);
        assert!(get_dead_letter_webhooks(&dlq, "no_such_key").is_empty());
    }

    // -----------------------------------------------------------------------
    // 5. Signed payload produces X-Anchor-Signature header
    // -----------------------------------------------------------------------
    #[test]
    fn test_signed_payload_produces_header() {
        let key = b"my-secret-key".to_vec();
        let payload = r#"{"event":"deposit"}"#;
        let received_sig: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let rs = received_sig.clone();

        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let result = deliver_webhook(
            &signed_config(1, key.clone()),
            payload,
            &mut dlq,
            move |_url, _body, sig| {
                *rs.lock().unwrap() = sig.map(|s| s.to_string());
                Ok(200)
            },
            |_| {},
            || 0u64,
        );
        assert!(result.is_ok());
        let sig = received_sig.lock().unwrap().clone().expect("signature must be set");
        assert!(sig.starts_with("sha256="), "got: {sig}");
        // Verify the signature is valid
        assert!(verify_webhook_signature(payload, &sig, &key), "signature should verify");
    }

    // -----------------------------------------------------------------------
    // 6. Unsigned config produces no header
    // -----------------------------------------------------------------------
    #[test]
    fn test_unsigned_config_produces_no_header() {
        let received_sig: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let rs = received_sig.clone();
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let _ = deliver_webhook(
            &config(1),
            "payload",
            &mut dlq,
            move |_url, _body, sig| {
                *rs.lock().unwrap() = sig.map(|s| s.to_string());
                Ok(200)
            },
            |_| {},
            || 0u64,
        );
        assert!(received_sig.lock().unwrap().is_none(), "unsigned config must not produce a header");
    }

    // -----------------------------------------------------------------------
    // 7. verify_webhook_signature — correct key returns true
    // -----------------------------------------------------------------------
    #[test]
    fn test_verify_correct_signature() {
        let key = b"secret";
        let payload = r#"{"event":"test"}"#;
        // Build header using sign_payload indirectly via deliver_webhook
        let sig_captured: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let sc = sig_captured.clone();
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let _ = deliver_webhook(
            &signed_config(1, key.to_vec()),
            payload,
            &mut dlq,
            move |_url, _body, sig| {
                if let Some(s) = sig { *sc.lock().unwrap() = s.to_string(); }
                Ok(200)
            },
            |_| {},
            || 0u64,
        );
        let sig = sig_captured.lock().unwrap().clone();
        assert!(verify_webhook_signature(payload, &sig, key));
    }

    // -----------------------------------------------------------------------
    // 8. verify_webhook_signature — tampered payload returns false
    // -----------------------------------------------------------------------
    #[test]
    fn test_verify_tampered_payload_fails() {
        let key = b"secret";
        let payload = r#"{"event":"test"}"#;
        let sig_captured: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let sc = sig_captured.clone();
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let _ = deliver_webhook(
            &signed_config(1, key.to_vec()),
            payload,
            &mut dlq,
            move |_url, _body, sig| {
                if let Some(s) = sig { *sc.lock().unwrap() = s.to_string(); }
                Ok(200)
            },
            |_| {},
            || 0u64,
        );
        let sig = sig_captured.lock().unwrap().clone();
        assert!(!verify_webhook_signature(r#"{"event":"tampered"}"#, &sig, key));
    }

    // -----------------------------------------------------------------------
    // 9. verify_webhook_signature — wrong key returns false
    // -----------------------------------------------------------------------
    #[test]
    fn test_verify_wrong_key_fails() {
        let key = b"secret";
        let payload = r#"{"event":"test"}"#;
        let sig_captured: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let sc = sig_captured.clone();
        let mut dlq: BTreeMap<String, Vec<DlqEntry>> = BTreeMap::new();
        let _ = deliver_webhook(
            &signed_config(1, key.to_vec()),
            payload,
            &mut dlq,
            move |_url, _body, sig| {
                if let Some(s) = sig { *sc.lock().unwrap() = s.to_string(); }
                Ok(200)
            },
            |_| {},
            || 0u64,
        );
        let sig = sig_captured.lock().unwrap().clone();
        assert!(!verify_webhook_signature(payload, &sig, b"wrong-key"));
    }
}
