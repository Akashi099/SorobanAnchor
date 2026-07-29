//! Tests for deterministic ordering of quote and attestation results (#663).
//!
//! Verifies that:
//! - `sort_quotes` produces a stable, fully deterministic order for every
//!   `QuoteSortOrder` variant.
//! - `sort_attestations` produces a stable, fully deterministic order for
//!   every `AttestationSortOrder` variant.
//! - Tie cases are broken by `id` ascending so repeated calls always return
//!   the same ordering regardless of input order.
//! - Sorting an empty slice returns an empty result (no panic).
//! - Sorting a single-element slice returns that element unchanged.

#![cfg(test)]

// ── Quote ordering tests (#663) ───────────────────────────────────────────────

#[cfg(not(feature = "wasm"))]
mod quote_ordering_tests {
    use anchorkit::sep38::{sort_quotes, FirmQuote, QuoteSortOrder};

    fn make_quote(id: &str, expires_at: u64, price: &str) -> FirmQuote {
        FirmQuote {
            id: id.into(),
            expires_at,
            price: price.into(),
            sell_amount: "1000".into(),
            buy_amount: "150".into(),
            sell_asset: "XLM".into(),
            buy_asset: "USDC".into(),
            routing_reason: None,
        }
    }

    // ── Empty and single-element edge cases ───────────────────────────────────

    #[test]
    fn sort_quotes_empty_slice_returns_empty() {
        let result = sort_quotes(&[], QuoteSortOrder::PriceAsc);
        assert!(result.is_empty());
    }

    #[test]
    fn sort_quotes_single_element_returns_same() {
        let q = make_quote("only", 5000, "0.10");
        let result = sort_quotes(&[q.clone()], QuoteSortOrder::PriceAsc);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "only");
    }

    // ── PriceAsc ──────────────────────────────────────────────────────────────

    #[test]
    fn price_asc_cheapest_first() {
        let a = make_quote("a", 5000, "0.30");
        let b = make_quote("b", 5000, "0.10");
        let c = make_quote("c", 5000, "0.20");

        let sorted = sort_quotes(&[a, b, c], QuoteSortOrder::PriceAsc);
        assert_eq!(sorted[0].price, "0.10");
        assert_eq!(sorted[1].price, "0.20");
        assert_eq!(sorted[2].price, "0.30");
    }

    #[test]
    fn price_asc_tie_broken_by_id_ascending() {
        // Same price, different ids
        let x = make_quote("z-id", 5000, "0.15");
        let y = make_quote("a-id", 5000, "0.15");

        let sorted = sort_quotes(&[x, y], QuoteSortOrder::PriceAsc);
        assert_eq!(sorted[0].id, "a-id");
        assert_eq!(sorted[1].id, "z-id");
    }

    #[test]
    fn price_asc_deterministic_across_permutations() {
        let a = make_quote("q1", 5000, "0.10");
        let b = make_quote("q2", 5000, "0.20");
        let c = make_quote("q3", 5000, "0.10"); // ties with a

        let order1 = sort_quotes(&[a.clone(), b.clone(), c.clone()], QuoteSortOrder::PriceAsc);
        let order2 = sort_quotes(&[c.clone(), a.clone(), b.clone()], QuoteSortOrder::PriceAsc);
        let order3 = sort_quotes(&[b.clone(), c.clone(), a.clone()], QuoteSortOrder::PriceAsc);

        // All permutations must produce the same id sequence
        let ids1: Vec<&str> = order1.iter().map(|q| q.id.as_str()).collect();
        let ids2: Vec<&str> = order2.iter().map(|q| q.id.as_str()).collect();
        let ids3: Vec<&str> = order3.iter().map(|q| q.id.as_str()).collect();

        assert_eq!(ids1, ids2);
        assert_eq!(ids1, ids3);
    }

    // ── PriceDesc ─────────────────────────────────────────────────────────────

    #[test]
    fn price_desc_most_expensive_first() {
        let a = make_quote("a", 5000, "0.10");
        let b = make_quote("b", 5000, "0.30");
        let c = make_quote("c", 5000, "0.20");

        let sorted = sort_quotes(&[a, b, c], QuoteSortOrder::PriceDesc);
        assert_eq!(sorted[0].price, "0.30");
        assert_eq!(sorted[1].price, "0.20");
        assert_eq!(sorted[2].price, "0.10");
    }

    #[test]
    fn price_desc_tie_broken_by_id_ascending() {
        let x = make_quote("z-id", 5000, "0.15");
        let y = make_quote("a-id", 5000, "0.15");

        let sorted = sort_quotes(&[x, y], QuoteSortOrder::PriceDesc);
        assert_eq!(sorted[0].id, "a-id");
        assert_eq!(sorted[1].id, "z-id");
    }

    // ── ExpiresAtAsc ──────────────────────────────────────────────────────────

    #[test]
    fn expires_at_asc_soonest_first() {
        let a = make_quote("a", 3000, "0.15");
        let b = make_quote("b", 1000, "0.15");
        let c = make_quote("c", 2000, "0.15");

        let sorted = sort_quotes(&[a, b, c], QuoteSortOrder::ExpiresAtAsc);
        assert_eq!(sorted[0].expires_at, 1000);
        assert_eq!(sorted[1].expires_at, 2000);
        assert_eq!(sorted[2].expires_at, 3000);
    }

    #[test]
    fn expires_at_asc_tie_broken_by_id_ascending() {
        let x = make_quote("z-id", 2000, "0.15");
        let y = make_quote("a-id", 2000, "0.15");

        let sorted = sort_quotes(&[x, y], QuoteSortOrder::ExpiresAtAsc);
        assert_eq!(sorted[0].id, "a-id");
        assert_eq!(sorted[1].id, "z-id");
    }

    // ── ExpiresAtDesc ─────────────────────────────────────────────────────────

    #[test]
    fn expires_at_desc_latest_first() {
        let a = make_quote("a", 1000, "0.15");
        let b = make_quote("b", 3000, "0.15");
        let c = make_quote("c", 2000, "0.15");

        let sorted = sort_quotes(&[a, b, c], QuoteSortOrder::ExpiresAtDesc);
        assert_eq!(sorted[0].expires_at, 3000);
        assert_eq!(sorted[1].expires_at, 2000);
        assert_eq!(sorted[2].expires_at, 1000);
    }

    #[test]
    fn expires_at_desc_tie_broken_by_id_ascending() {
        let x = make_quote("z-id", 2000, "0.15");
        let y = make_quote("a-id", 2000, "0.15");

        let sorted = sort_quotes(&[x, y], QuoteSortOrder::ExpiresAtDesc);
        assert_eq!(sorted[0].id, "a-id");
        assert_eq!(sorted[1].id, "z-id");
    }

    // ── IdAsc ─────────────────────────────────────────────────────────────────

    #[test]
    fn id_asc_lexicographic_order() {
        let a = make_quote("bravo", 5000, "0.15");
        let b = make_quote("alpha", 5000, "0.15");
        let c = make_quote("charlie", 5000, "0.15");

        let sorted = sort_quotes(&[a, b, c], QuoteSortOrder::IdAsc);
        assert_eq!(sorted[0].id, "alpha");
        assert_eq!(sorted[1].id, "bravo");
        assert_eq!(sorted[2].id, "charlie");
    }

    #[test]
    fn id_asc_already_sorted_is_stable() {
        let a = make_quote("a", 1000, "0.10");
        let b = make_quote("b", 2000, "0.20");
        let c = make_quote("c", 3000, "0.30");

        let sorted = sort_quotes(&[a, b, c], QuoteSortOrder::IdAsc);
        assert_eq!(sorted[0].id, "a");
        assert_eq!(sorted[1].id, "b");
        assert_eq!(sorted[2].id, "c");
    }

    // ── Determinism across repeated calls ─────────────────────────────────────

    #[test]
    fn repeated_sort_same_input_same_output() {
        let quotes = vec![
            make_quote("q3", 3000, "0.30"),
            make_quote("q1", 1000, "0.10"),
            make_quote("q2", 2000, "0.20"),
        ];

        let r1 = sort_quotes(&quotes, QuoteSortOrder::ExpiresAtAsc);
        let r2 = sort_quotes(&quotes, QuoteSortOrder::ExpiresAtAsc);

        let ids1: Vec<&str> = r1.iter().map(|q| q.id.as_str()).collect();
        let ids2: Vec<&str> = r2.iter().map(|q| q.id.as_str()).collect();
        assert_eq!(ids1, ids2);
    }

    #[test]
    fn all_ties_resolved_by_id_ascending_for_all_orders() {
        // Five quotes identical except id
        let ids = ["echo", "alpha", "delta", "bravo", "charlie"];
        let quotes: Vec<FirmQuote> = ids
            .iter()
            .map(|id| make_quote(id, 5000, "0.15"))
            .collect();

        for order in &[
            QuoteSortOrder::PriceAsc,
            QuoteSortOrder::PriceDesc,
            QuoteSortOrder::ExpiresAtAsc,
            QuoteSortOrder::ExpiresAtDesc,
        ] {
            let sorted = sort_quotes(&quotes, order.clone());
            let sorted_ids: Vec<&str> = sorted.iter().map(|q| q.id.as_str()).collect();
            let mut expected = sorted_ids.clone();
            expected.sort_unstable();
            assert_eq!(
                sorted_ids, expected,
                "ties not resolved by id-asc for order {:?}",
                order
            );
        }
    }

    // ── Input slice is not mutated ────────────────────────────────────────────

    #[test]
    fn sort_quotes_does_not_mutate_input() {
        let a = make_quote("z", 5000, "0.30");
        let b = make_quote("a", 5000, "0.10");
        let input = vec![a.clone(), b.clone()];

        let _sorted = sort_quotes(&input, QuoteSortOrder::PriceAsc);
        // Original slice order unchanged
        assert_eq!(input[0].id, "z");
        assert_eq!(input[1].id, "a");
    }
}

// ── Attestation ordering tests (#663) ─────────────────────────────────────────

mod attestation_ordering_tests {
    use soroban_sdk::{Bytes, Env};
    use anchorkit::contract::{sort_attestations, Attestation, AttestationSortOrder};

    fn make_attestation(env: &Env, id: u64, timestamp: u64, issuer_seed: u8) -> Attestation {
        use soroban_sdk::testutils::Address as _;
        use soroban_sdk::Address;

        // Generate a deterministic address based on issuer_seed to keep tests predictable.
        // We actually just need any distinct address; testutils::generate is fine.
        let _ = issuer_seed;
        let addr = Address::generate(env);
        let mut hash = Bytes::new(env);
        for _ in 0..32 {
            hash.push_back(issuer_seed);
        }
        Attestation {
            id,
            issuer: addr.clone(),
            subject: addr,
            timestamp,
            payload_hash: hash.clone(),
            signature: hash,
            schema_version: 1,
        }
    }

    fn make_env() -> Env {
        Env::default()
    }

    // ── Edge cases ─────────────────────────────────────────────────────────────

    #[test]
    fn sort_attestations_empty_returns_empty() {
        let _env = make_env();
        let result = sort_attestations(&[], AttestationSortOrder::IdAsc);
        assert!(result.is_empty());
    }

    #[test]
    fn sort_attestations_single_element_unchanged() {
        let env = make_env();
        let a = make_attestation(&env, 42, 1000, 0x01);
        let result = sort_attestations(&[a], AttestationSortOrder::IdAsc);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, 42);
    }

    // ── IdAsc ──────────────────────────────────────────────────────────────────

    #[test]
    fn id_asc_orders_by_id_numerically() {
        let env = make_env();
        let a = make_attestation(&env, 30, 1000, 0x01);
        let b = make_attestation(&env, 10, 2000, 0x02);
        let c = make_attestation(&env, 20, 3000, 0x03);

        let sorted = sort_attestations(&[a, b, c], AttestationSortOrder::IdAsc);
        assert_eq!(sorted[0].id, 10);
        assert_eq!(sorted[1].id, 20);
        assert_eq!(sorted[2].id, 30);
    }

    // ── IdDesc ─────────────────────────────────────────────────────────────────

    #[test]
    fn id_desc_orders_newest_id_first() {
        let env = make_env();
        let a = make_attestation(&env, 10, 1000, 0x01);
        let b = make_attestation(&env, 30, 2000, 0x02);
        let c = make_attestation(&env, 20, 3000, 0x03);

        let sorted = sort_attestations(&[a, b, c], AttestationSortOrder::IdDesc);
        assert_eq!(sorted[0].id, 30);
        assert_eq!(sorted[1].id, 20);
        assert_eq!(sorted[2].id, 10);
    }

    // ── TimestampAsc ───────────────────────────────────────────────────────────

    #[test]
    fn timestamp_asc_oldest_first() {
        let env = make_env();
        let a = make_attestation(&env, 1, 3000, 0x01);
        let b = make_attestation(&env, 2, 1000, 0x02);
        let c = make_attestation(&env, 3, 2000, 0x03);

        let sorted = sort_attestations(&[a, b, c], AttestationSortOrder::TimestampAsc);
        assert_eq!(sorted[0].timestamp, 1000);
        assert_eq!(sorted[1].timestamp, 2000);
        assert_eq!(sorted[2].timestamp, 3000);
    }

    #[test]
    fn timestamp_asc_tie_broken_by_id_ascending() {
        let env = make_env();
        let a = make_attestation(&env, 99, 1000, 0x01);
        let b = make_attestation(&env, 1, 1000, 0x02);
        let c = make_attestation(&env, 50, 1000, 0x03);

        let sorted = sort_attestations(&[a, b, c], AttestationSortOrder::TimestampAsc);
        assert_eq!(sorted[0].id, 1);
        assert_eq!(sorted[1].id, 50);
        assert_eq!(sorted[2].id, 99);
    }

    // ── TimestampDesc ──────────────────────────────────────────────────────────

    #[test]
    fn timestamp_desc_most_recent_first() {
        let env = make_env();
        let a = make_attestation(&env, 1, 1000, 0x01);
        let b = make_attestation(&env, 2, 3000, 0x02);
        let c = make_attestation(&env, 3, 2000, 0x03);

        let sorted = sort_attestations(&[a, b, c], AttestationSortOrder::TimestampDesc);
        assert_eq!(sorted[0].timestamp, 3000);
        assert_eq!(sorted[1].timestamp, 2000);
        assert_eq!(sorted[2].timestamp, 1000);
    }

    #[test]
    fn timestamp_desc_tie_broken_by_id_ascending() {
        let env = make_env();
        let a = make_attestation(&env, 99, 5000, 0x01);
        let b = make_attestation(&env, 1, 5000, 0x02);
        let c = make_attestation(&env, 50, 5000, 0x03);

        let sorted = sort_attestations(&[a, b, c], AttestationSortOrder::TimestampDesc);
        assert_eq!(sorted[0].id, 1);
        assert_eq!(sorted[1].id, 50);
        assert_eq!(sorted[2].id, 99);
    }

    // ── Determinism across repeated calls ─────────────────────────────────────

    #[test]
    fn repeated_sort_same_input_produces_same_output() {
        let env = make_env();
        let records: Vec<Attestation> = vec![
            make_attestation(&env, 3, 3000, 0x01),
            make_attestation(&env, 1, 1000, 0x02),
            make_attestation(&env, 2, 2000, 0x03),
        ];

        let r1 = sort_attestations(&records, AttestationSortOrder::TimestampDesc);
        let r2 = sort_attestations(&records, AttestationSortOrder::TimestampDesc);

        let ids1: Vec<u64> = r1.iter().map(|a| a.id).collect();
        let ids2: Vec<u64> = r2.iter().map(|a| a.id).collect();
        assert_eq!(ids1, ids2);
    }

    #[test]
    fn sort_different_permutations_produce_same_result() {
        let env = make_env();
        let a = make_attestation(&env, 10, 1000, 0x01);
        let b = make_attestation(&env, 20, 2000, 0x02);
        let c = make_attestation(&env, 30, 3000, 0x03);

        let sorted_abc = sort_attestations(&[a.clone(), b.clone(), c.clone()], AttestationSortOrder::IdAsc);
        let sorted_cba = sort_attestations(&[c.clone(), b.clone(), a.clone()], AttestationSortOrder::IdAsc);
        let sorted_bac = sort_attestations(&[b.clone(), a.clone(), c.clone()], AttestationSortOrder::IdAsc);

        let ids_abc: Vec<u64> = sorted_abc.iter().map(|a| a.id).collect();
        let ids_cba: Vec<u64> = sorted_cba.iter().map(|a| a.id).collect();
        let ids_bac: Vec<u64> = sorted_bac.iter().map(|a| a.id).collect();

        assert_eq!(ids_abc, ids_cba);
        assert_eq!(ids_abc, ids_bac);
    }

    // ── Input is not mutated ────────────────────────────────────────────────────

    #[test]
    fn sort_attestations_does_not_mutate_input() {
        let env = make_env();
        let a = make_attestation(&env, 30, 3000, 0x01);
        let b = make_attestation(&env, 10, 1000, 0x02);
        let input = vec![a, b];

        let _sorted = sort_attestations(&input, AttestationSortOrder::IdAsc);
        assert_eq!(input[0].id, 30); // original order preserved
        assert_eq!(input[1].id, 10);
    }
}
