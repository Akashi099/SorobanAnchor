//! Tests for cache compaction and cleanup routines (task a).
//!
//! Verifies that:
//! - `compact_cache` removes expired metadata entries and returns the freed count.
//! - `compact_cache` removes expired capabilities entries.
//! - Fresh entries are never removed during compaction.
//! - The instance-level cache count is decremented correctly after compaction.
//! - Compaction on an empty list is a safe no-op.
//! - Mixed (fresh + expired) entries are handled correctly.

#![cfg(test)]

mod cache_compaction_tests {
    use soroban_sdk::{
        testutils::{Address as _, Ledger, LedgerInfo},
        Address, Env, Vec,
    };
    use anchorkit::contract::{AnchorKitContract, AnchorKitContractClient, AnchorMetadata};

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

    fn sample_metadata(env: &Env, anchor: &Address) -> AnchorMetadata {
        AnchorMetadata {
            anchor: anchor.clone(),
            reputation_score: 9000,
            liquidity_score: 8000,
            uptime_percentage: 9900,
            total_volume: 1_000_000,
            average_settlement_time: 120,
            is_active: true,
        }
    }

    fn anchor_vec(env: &Env, anchors: &[Address]) -> Vec<Address> {
        let mut v = Vec::new(env);
        for a in anchors {
            v.push_back(a.clone());
        }
        v
    }

    // -----------------------------------------------------------------------
    // Compaction on empty list
    // -----------------------------------------------------------------------

    #[test]
    fn compact_cache_on_empty_list_is_noop() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let freed = client.compact_cache(&Vec::new(&env));
        assert_eq!(freed, 0);
    }

    // -----------------------------------------------------------------------
    // Metadata cache compaction
    // -----------------------------------------------------------------------

    #[test]
    fn compact_cache_removes_expired_metadata_entry() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        // Cache metadata with a 100-second TTL at t=1000
        set_ledger(&env, 1000);
        let meta = sample_metadata(&env, &anchor);
        client.cache_metadata(&anchor, &meta, &100u64);

        assert_eq!(client.get_cache_count(), 1);

        // Advance past the TTL (100s primary + default SWR grace = 300s → total 400s)
        set_ledger(&env, 1000 + 500);

        let freed = client.compact_cache(&anchor_vec(&env, &[anchor.clone()]));
        assert_eq!(freed, 1, "one expired metadata entry should be freed");
        assert_eq!(client.get_cache_count(), 0, "count should be decremented");
    }

    #[test]
    fn compact_cache_does_not_remove_fresh_metadata_entry() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        set_ledger(&env, 1000);
        let meta = sample_metadata(&env, &anchor);
        client.cache_metadata(&anchor, &meta, &3600u64); // 1h TTL

        // Only 100 seconds have passed — entry is still fresh
        set_ledger(&env, 1100);

        let freed = client.compact_cache(&anchor_vec(&env, &[anchor.clone()]));
        assert_eq!(freed, 0, "fresh entry must not be removed");
        assert_eq!(client.get_cache_count(), 1, "count must not be decremented");
    }

    // -----------------------------------------------------------------------
    // Capabilities cache compaction
    // -----------------------------------------------------------------------

    #[test]
    fn compact_cache_removes_expired_capabilities_entry() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        set_ledger(&env, 1000);
        let toml_url = soroban_sdk::String::from_str(&env, "https://anchor.example.com/.well-known/stellar.toml");
        let caps    = soroban_sdk::String::from_str(&env, "SEP6,SEP24");
        client.cache_capabilities(&anchor, &toml_url, &caps, &50u64); // 50s TTL

        assert_eq!(client.get_cache_count(), 1);

        // Advance past the 50-second capabilities TTL
        set_ledger(&env, 1100);

        let freed = client.compact_cache(&anchor_vec(&env, &[anchor.clone()]));
        assert_eq!(freed, 1);
        assert_eq!(client.get_cache_count(), 0);
    }

    #[test]
    fn compact_cache_does_not_remove_fresh_capabilities_entry() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        set_ledger(&env, 1000);
        let toml_url = soroban_sdk::String::from_str(&env, "https://anchor.example.com/.well-known/stellar.toml");
        let caps    = soroban_sdk::String::from_str(&env, "SEP6");
        client.cache_capabilities(&anchor, &toml_url, &caps, &3600u64); // 1h TTL

        set_ledger(&env, 1100); // only 100s elapsed

        let freed = client.compact_cache(&anchor_vec(&env, &[anchor.clone()]));
        assert_eq!(freed, 0);
        assert_eq!(client.get_cache_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Both caches for a single anchor
    // -----------------------------------------------------------------------

    #[test]
    fn compact_cache_removes_both_expired_slots_for_anchor() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        set_ledger(&env, 1000);
        let meta = sample_metadata(&env, &anchor);
        client.cache_metadata(&anchor, &meta, &50u64);

        let toml_url = soroban_sdk::String::from_str(&env, "https://anchor.example.com/.well-known/stellar.toml");
        let caps    = soroban_sdk::String::from_str(&env, "SEP24");
        client.cache_capabilities(&anchor, &toml_url, &caps, &50u64);

        assert_eq!(client.get_cache_count(), 2);

        // Advance past both TTLs
        set_ledger(&env, 1500);

        let freed = client.compact_cache(&anchor_vec(&env, &[anchor.clone()]));
        assert_eq!(freed, 2, "both expired slots should be freed");
        assert_eq!(client.get_cache_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Mixed fresh and expired entries across multiple anchors
    // -----------------------------------------------------------------------

    #[test]
    fn compact_cache_handles_mixed_fresh_and_expired_anchors() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor_expired = Address::generate(&env);
        let anchor_fresh   = Address::generate(&env);

        set_ledger(&env, 1000);
        // anchor_expired: short TTL
        client.cache_metadata(&anchor_expired, &sample_metadata(&env, &anchor_expired), &60u64);
        // anchor_fresh: long TTL
        client.cache_metadata(&anchor_fresh,   &sample_metadata(&env, &anchor_fresh),   &7200u64);

        assert_eq!(client.get_cache_count(), 2);

        // Move time so anchor_expired is past its combined TTL but anchor_fresh is still fresh
        set_ledger(&env, 1500);

        let freed = client.compact_cache(&anchor_vec(&env, &[
            anchor_expired.clone(),
            anchor_fresh.clone(),
        ]));
        assert_eq!(freed, 1, "only the expired anchor's slot should be freed");
        assert_eq!(client.get_cache_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Cache count stays accurate after multiple compaction passes
    // -----------------------------------------------------------------------

    #[test]
    fn cache_count_remains_accurate_after_repeated_compaction() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let a1 = Address::generate(&env);
        let a2 = Address::generate(&env);

        set_ledger(&env, 1000);
        client.cache_metadata(&a1, &sample_metadata(&env, &a1), &50u64);
        client.cache_metadata(&a2, &sample_metadata(&env, &a2), &3600u64);
        assert_eq!(client.get_cache_count(), 2);

        // First compaction: only a1 expired
        set_ledger(&env, 1500);
        client.compact_cache(&anchor_vec(&env, &[a1.clone(), a2.clone()]));
        assert_eq!(client.get_cache_count(), 1);

        // Second compaction: nothing new has expired
        client.compact_cache(&anchor_vec(&env, &[a1.clone(), a2.clone()]));
        assert_eq!(client.get_cache_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Anchor not in cache — compact is a safe no-op
    // -----------------------------------------------------------------------

    #[test]
    fn compact_cache_with_uncached_anchor_is_noop() {
        let env = make_env();
        let (_admin, client) = setup(&env);
        let anchor = Address::generate(&env);

        set_ledger(&env, 5000);
        let freed = client.compact_cache(&anchor_vec(&env, &[anchor.clone()]));
        assert_eq!(freed, 0);
        assert_eq!(client.get_cache_count(), 0);
    }
}
