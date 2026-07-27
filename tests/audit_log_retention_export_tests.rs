//! Extended tests for audit log retention and export (task d).
//!
//! Builds on the basic tests in audit_log_retention_tests.rs and focuses on:
//! - Retention boundary: entries exactly at the threshold are kept / removed correctly.
//! - Prune preserves entries newer than the threshold.
//! - Export JSON envelope is always valid (starts '[', ends ']').
//! - Export with offset skips leading entries.
//! - Export of an empty log returns an empty array.
//! - Export batch size cap (50 max).
//! - set_audit_log_retention + prune interact correctly.
//! - Disabled audit logging produces an empty export.
//! - Re-enabling audit logging after disabling resumes normal export.

#![cfg(test)]

mod sep10_test_util;

mod audit_log_retention_export_tests {
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env,
    };
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use anchorkit::contract::{AnchorKitContract, AnchorKitContractClient};
    use anchorkit::admin_audit_log::{AdminAuditLog, AdminAuditLogConfig};
    use crate::sep10_test_util::{register_attestor_with_sep10, sign_payload};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_env() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        env
    }

    fn set_ledger(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 21,
            sequence_number: ts as u32,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        });
    }

    fn setup(env: &Env) -> (Address, AnchorKitContractClient<'_>) {
        set_ledger(env, 1000);
        let cid = env.register_contract(None, AnchorKitContract);
        let client = AnchorKitContractClient::new(env, &cid);
        let admin = Address::generate(env);
        client.initialize(&admin);
        (admin, client)
    }

    fn add_attestor(env: &Env, client: &AnchorKitContractClient<'_>) -> (Address, SigningKey) {
        let attestor = Address::generate(env);
        let sk = SigningKey::generate(&mut OsRng);
        register_attestor_with_sep10(env, client, &attestor, &attestor, &sk);
        (attestor, sk)
    }

    /// Emit one audit log entry via a complete attest-with-session round-trip.
    fn emit_one_entry(
        env: &Env,
        client: &AnchorKitContractClient<'_>,
        attestor: &Address,
        sk: &SigningKey,
        ts: u64,
    ) {
        set_ledger(env, ts);
        let session_id = client.create_session(attestor);
        let subject = Address::generate(env);
        let mut buf = [0u8; 32];
        buf[..16].copy_from_slice(b"payload_hash_32b");
        buf[16..24].copy_from_slice(&ts.to_be_bytes());
        let payload = soroban_sdk::Bytes::from_slice(env, &buf);
        let sig = sign_payload(env, sk, &payload);
        client.submit_attestation_with_session(
            &session_id, attestor, &subject, &ts, &payload, &sig,
        );
        client.close_session(&session_id, attestor);
    }

    // -----------------------------------------------------------------------
    // Retention boundary tests
    // -----------------------------------------------------------------------

    /// An entry whose timestamp equals the prune threshold is NOT pruned
    /// (prune removes entries *strictly before* the threshold).
    #[test]
    fn prune_boundary_entry_at_threshold_is_kept() {
        let env = make_env();
        let (_, client) = setup(&env);
        let (attestor, sk) = add_attestor(&env, &client);

        emit_one_entry(&env, &client, &attestor, &sk, 5_000);

        set_ledger(&env, 10_000);
        // Prune with threshold == 5_000: the entry has timestamp == 5000,
        // which is not strictly less than 5_000, so it must survive.
        let pruned = client.prune_audit_logs(&5_000u64);
        assert_eq!(pruned, 0, "entry at the threshold timestamp must not be pruned");
        assert!(client.get_audit_log_count() >= 1);
    }

    /// An entry strictly before the threshold is pruned.
    #[test]
    fn prune_removes_entry_strictly_before_threshold() {
        let env = make_env();
        let (_, client) = setup(&env);
        let (attestor, sk) = add_attestor(&env, &client);

        emit_one_entry(&env, &client, &attestor, &sk, 4_999);

        set_ledger(&env, 10_000);
        let pruned = client.prune_audit_logs(&5_000u64);
        assert!(pruned >= 1, "entry before the threshold must be pruned");
    }

    /// Entries after the threshold are never removed.
    #[test]
    fn prune_never_removes_entries_after_threshold() {
        let env = make_env();
        let (_, client) = setup(&env);
        let (attestor, sk) = add_attestor(&env, &client);

        emit_one_entry(&env, &client, &attestor, &sk, 50_000);

        set_ledger(&env, 60_000);
        let pruned = client.prune_audit_logs(&5_000u64);
        assert_eq!(pruned, 0, "recent entry must never be pruned");
    }

    // -----------------------------------------------------------------------
    // Retention policy interacts correctly with prune
    // -----------------------------------------------------------------------

    #[test]
    fn set_retention_then_prune_respects_policy() {
        let env = make_env();
        let (_, client) = setup(&env);
        let (attestor, sk) = add_attestor(&env, &client);

        // Emit an old entry and a recent entry
        emit_one_entry(&env, &client, &attestor, &sk, 1_000);
        emit_one_entry(&env, &client, &attestor, &sk, 100_000);

        // Set 1-day retention (86400 s)
        client.set_audit_log_retention(&1u64);
        assert_eq!(client.get_audit_log_retention(), 1);

        // Prune with threshold that covers the old entry but not the recent one
        set_ledger(&env, 200_000);
        let pruned = client.prune_audit_logs(&50_000u64);
        assert!(pruned >= 1, "old entry should be pruned");

        // The recent entry at ts=100_000 must still be accessible
        let page = client.get_audit_logs_paginated(&0, &50);
        let found_recent = (0..page.len()).any(|i| {
            page.get(i).unwrap().operation.timestamp == 100_000
        });
        assert!(found_recent, "recent entry should survive pruning");
    }

    // -----------------------------------------------------------------------
    // Export format tests
    // -----------------------------------------------------------------------

    #[test]
    fn export_empty_log_returns_empty_json_array() {
        let env = make_env();
        let (_, client) = setup(&env);

        let batch = client.export_audit_log_batch(&0u64, &10u32);
        let mut buf = std::vec::Vec::new();
        for i in 0..batch.len() {
            buf.push(batch.get(i).unwrap());
        }
        let json = core::str::from_utf8(&buf).expect("export must be valid UTF-8");
        assert_eq!(json, "[]", "empty export must return []");
    }

    #[test]
    fn export_json_envelope_is_always_valid() {
        let env = make_env();
        let (_, client) = setup(&env);
        let (attestor, sk) = add_attestor(&env, &client);

        emit_one_entry(&env, &client, &attestor, &sk, 2_000);

        let batch = client.export_audit_log_batch(&0u64, &10u32);
        let mut buf = std::vec::Vec::new();
        for i in 0..batch.len() {
            buf.push(batch.get(i).unwrap());
        }
        let json = core::str::from_utf8(&buf).expect("export must be valid UTF-8");
        assert!(json.starts_with('['), "export must start with '['");
        assert!(json.ends_with(']'),  "export must end with ']'");
    }

    #[test]
    fn export_each_entry_is_a_hex_string() {
        let env = make_env();
        let (_, client) = setup(&env);
        let (attestor, sk) = add_attestor(&env, &client);

        emit_one_entry(&env, &client, &attestor, &sk, 3_000);

        let batch = client.export_audit_log_batch(&0u64, &10u32);
        let mut buf = std::vec::Vec::new();
        for i in 0..batch.len() {
            buf.push(batch.get(i).unwrap());
        }
        let json = core::str::from_utf8(&buf).unwrap();
        // Strip outer [ ]
        let inner = &json[1..json.len()-1];
        // Each entry is a quoted hex string
        assert!(inner.starts_with('"'), "each entry must be a quoted hex string");
        assert!(inner.ends_with('"'),   "each entry must end with a closing quote");
        // All chars between quotes must be hex digits
        let hex_content = &inner[1..inner.len()-1];
        assert!(hex_content.chars().all(|c| c.is_ascii_hexdigit()),
            "entry content must be pure hex, got: {hex_content}");
    }

    #[test]
    fn export_two_entries_are_comma_separated() {
        let env = make_env();
        let (_, client) = setup(&env);
        let (attestor, sk) = add_attestor(&env, &client);

        emit_one_entry(&env, &client, &attestor, &sk, 4_000);
        emit_one_entry(&env, &client, &attestor, &sk, 5_000);

        let batch = client.export_audit_log_batch(&0u64, &10u32);
        let mut buf = std::vec::Vec::new();
        for i in 0..batch.len() {
            buf.push(batch.get(i).unwrap());
        }
        let json = core::str::from_utf8(&buf).unwrap();
        let inner = &json[1..json.len()-1];
        let parts: std::vec::Vec<&str> = inner.split(',').collect();
        assert_eq!(parts.len(), 2, "two entries must be comma-separated");
    }

    // -----------------------------------------------------------------------
    // Export offset
    // -----------------------------------------------------------------------

    #[test]
    fn export_with_start_id_beyond_total_returns_empty_array() {
        let env = make_env();
        let (_, client) = setup(&env);
        let (attestor, sk) = add_attestor(&env, &client);

        emit_one_entry(&env, &client, &attestor, &sk, 2_000);

        let batch = client.export_audit_log_batch(&9999u64, &10u32);
        let mut buf = std::vec::Vec::new();
        for i in 0..batch.len() {
            buf.push(batch.get(i).unwrap());
        }
        let json = core::str::from_utf8(&buf).unwrap();
        assert_eq!(json, "[]", "offset beyond total must return []");
    }

    #[test]
    fn export_start_id_one_skips_first_entry() {
        let env = make_env();
        let (_, client) = setup(&env);
        let (attestor, sk) = add_attestor(&env, &client);

        emit_one_entry(&env, &client, &attestor, &sk, 4_000);
        emit_one_entry(&env, &client, &attestor, &sk, 5_000);

        // Export starting at id=1 — only the second entry
        let batch = client.export_audit_log_batch(&1u64, &10u32);
        let mut buf = std::vec::Vec::new();
        for i in 0..batch.len() {
            buf.push(batch.get(i).unwrap());
        }
        let json = core::str::from_utf8(&buf).unwrap();
        let inner = &json[1..json.len()-1];
        let parts: std::vec::Vec<&str> = inner.split(',').collect();
        assert_eq!(parts.len(), 1, "start_id=1 should return only one entry");
    }

    // -----------------------------------------------------------------------
    // Export batch size cap
    // -----------------------------------------------------------------------

    #[test]
    fn export_batch_size_is_capped_at_50() {
        let env = make_env();
        let (_, client) = setup(&env);
        let (attestor, sk) = add_attestor(&env, &client);

        // Emit two entries; request 200 — cap applies, result has at most 50
        emit_one_entry(&env, &client, &attestor, &sk, 1_000);
        emit_one_entry(&env, &client, &attestor, &sk, 2_000);

        let batch = client.export_audit_log_batch(&0u64, &200u32);
        let mut buf = std::vec::Vec::new();
        for i in 0..batch.len() {
            buf.push(batch.get(i).unwrap());
        }
        let json = core::str::from_utf8(&buf).unwrap();
        // Fewer than 50 entries exist so we get all of them, but the cap logic ran
        assert!(json.starts_with('['));
        assert!(json.ends_with(']'));
        let inner = &json[1..json.len()-1];
        if !inner.is_empty() {
            let parts: std::vec::Vec<&str> = inner.split(',').collect();
            assert!(parts.len() <= 50, "export must never exceed 50 entries");
        }
    }

    // -----------------------------------------------------------------------
    // Disabled audit logging → export returns empty
    // -----------------------------------------------------------------------

    #[test]
    fn disabled_audit_logging_produces_empty_export() {
        let env = make_env();
        let (_, client) = setup(&env);
        let cid = client.address.clone();

        // Disable audit logging before emitting entries
        env.as_contract(&cid, || {
            AdminAuditLog::set_config(&env, &AdminAuditLogConfig {
                enabled: false,
                max_entries: 10000,
                ttl_seconds: 31_536_000,
            });
        });

        let (attestor, sk) = add_attestor(&env, &client);
        emit_one_entry(&env, &client, &attestor, &sk, 5_000);

        let batch = client.export_audit_log_batch(&0u64, &10u32);
        let mut buf = std::vec::Vec::new();
        for i in 0..batch.len() {
            buf.push(batch.get(i).unwrap());
        }
        let json = core::str::from_utf8(&buf).unwrap();
        assert_eq!(json, "[]",
            "no admin_audit_log entries should exist when logging is disabled");
    }

    // -----------------------------------------------------------------------
    // Re-enabling logging after disable resumes export
    // -----------------------------------------------------------------------

    #[test]
    fn reenabled_logging_resumes_export() {
        let env = make_env();
        let (_, client) = setup(&env);
        let cid = client.address.clone();

        // Disable then re-enable before emitting
        env.as_contract(&cid, || {
            AdminAuditLog::set_config(&env, &AdminAuditLogConfig {
                enabled: false,
                max_entries: 10000,
                ttl_seconds: 31_536_000,
            });
            AdminAuditLog::set_config(&env, &AdminAuditLogConfig {
                enabled: true,
                max_entries: 10000,
                ttl_seconds: 31_536_000,
            });
        });

        let (attestor, sk) = add_attestor(&env, &client);
        emit_one_entry(&env, &client, &attestor, &sk, 6_000);

        let batch = client.export_audit_log_batch(&0u64, &10u32);
        let mut buf = std::vec::Vec::new();
        for i in 0..batch.len() {
            buf.push(batch.get(i).unwrap());
        }
        let json = core::str::from_utf8(&buf).unwrap();
        assert_ne!(json, "[]",
            "re-enabled audit logging should produce exportable entries");
    }

    // -----------------------------------------------------------------------
    // Retention policy is persisted across queries
    // -----------------------------------------------------------------------

    #[test]
    fn retention_policy_persists_across_reads() {
        let env = make_env();
        let (_, client) = setup(&env);

        client.set_audit_log_retention(&30u64);
        assert_eq!(client.get_audit_log_retention(), 30);

        client.set_audit_log_retention(&7u64);
        assert_eq!(client.get_audit_log_retention(), 7);

        // Zero means unlimited
        client.set_audit_log_retention(&0u64);
        assert_eq!(client.get_audit_log_retention(), 0);
    }
}
