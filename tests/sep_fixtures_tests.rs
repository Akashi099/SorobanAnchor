#![cfg(all(test, feature = "mock-only"))]

//! Comprehensive test fixtures for SEP-6, SEP-24, and SEP-38 (#299)
//!
//! This module provides deterministic fixtures and tests that validate
//! normalization across multiple anchor scenarios and edge cases.

mod sep_fixtures_tests {
    use anchorkit::mock::*;
    use anchorkit::{initiate_deposit, initiate_withdrawal, sep24, sep38, TransactionStatus};

    // ── SEP-6 Fixtures ────────────────────────────────────────────────────

    #[test]
    fn test_sep6_deposit_minimal_fields() {
        let raw = mock_deposit_response_minimal();
        let deposit = initiate_deposit(raw).expect("minimal deposit must parse");
        assert_eq!(deposit.transaction_id, "minimal-txn-001");
        assert_eq!(deposit.how, "Send funds");
    }

    #[test]
    fn test_sep6_deposit_full_fields() {
        let raw = mock_deposit_response_full();
        let deposit = initiate_deposit(raw).expect("full deposit must parse");
        assert_eq!(deposit.transaction_id, "full-txn-001");
        assert_eq!(deposit.how, "Send USDC to anchor address");
        assert!(deposit.extra_info.is_some());
        assert!(deposit.min_amount.is_some());
        assert!(deposit.max_amount.is_some());
    }

    #[test]
    fn test_sep6_deposit_anchor_a() {
        let raw = mock_deposit_response_anchor_a();
        let deposit = initiate_deposit(raw).expect("anchor A deposit must parse");
        assert_eq!(deposit.transaction_id, "anchor-a-txn-001");
        assert_eq!(deposit.fee_fixed, Some(2));
    }

    #[test]
    fn test_sep6_deposit_anchor_b() {
        let raw = mock_deposit_response_anchor_b();
        let deposit = initiate_deposit(raw).expect("anchor B deposit must parse");
        assert_eq!(deposit.transaction_id, "anchor-b-txn-001");
        assert_eq!(deposit.fee_fixed, Some(3));
    }

    #[test]
    fn test_sep6_withdrawal_minimal_fields() {
        let raw = mock_withdrawal_response_minimal();
        let withdrawal = initiate_withdrawal(raw).expect("minimal withdrawal must parse");
        assert_eq!(withdrawal.transaction_id, "withdraw-min-001");
    }

    #[test]
    fn test_sep6_withdrawal_full_fields() {
        let raw = mock_withdrawal_response_full();
        let withdrawal = initiate_withdrawal(raw).expect("full withdrawal must parse");
        assert_eq!(withdrawal.transaction_id, "withdraw-full-001");
        assert!(withdrawal.memo.is_some());
        assert!(withdrawal.min_amount.is_some());
        assert!(withdrawal.max_amount.is_some());
    }

    #[test]
    fn test_sep6_transaction_pending() {
        let raw = mock_transaction_response_pending();
        let tx = anchorkit::get_transaction_status(raw).expect("pending tx must parse");
        assert_eq!(tx.transaction_id, MOCK_TXN_ID);
        assert_eq!(tx.status, "pending_external");
    }

    #[test]
    fn test_sep6_transaction_completed() {
        let raw = mock_transaction_response_completed();
        let tx = anchorkit::get_transaction_status(raw).expect("completed tx must parse");
        assert_eq!(tx.transaction_id, MOCK_TXN_ID);
        assert_eq!(tx.status, "completed");
    }

    #[test]
    fn test_sep6_transaction_failed() {
        let raw = mock_transaction_response_failed();
        let tx = anchorkit::get_transaction_status(raw).expect("failed tx must parse");
        assert_eq!(tx.transaction_id, MOCK_TXN_ID);
        assert_eq!(tx.status, "error");
    }

    // ── SEP-24 Fixtures ───────────────────────────────────────────────────

    #[test]
    fn test_sep24_interactive_deposit() {
        let raw = mock_interactive_deposit_response();
        let interactive = sep24::initiate_interactive_deposit(raw).expect("interactive deposit must parse");
        assert_eq!(interactive.id, MOCK_TXN_ID_24);
        assert!(interactive.url.contains("sep24"));
    }

    #[test]
    fn test_sep24_interactive_withdrawal() {
        let raw = mock_interactive_withdrawal_response();
        let interactive = sep24::initiate_interactive_withdrawal(raw).expect("interactive withdrawal must parse");
        assert!(interactive.url.contains("withdraw"));
    }

    #[test]
    fn test_sep24_transaction_pending() {
        let raw = mock_sep24_transaction_pending();
        let tx = sep24::fetch_sep24_transaction_status(raw).expect("pending sep24 tx must parse");
        assert_eq!(tx.id, MOCK_TXN_ID_24);
        assert_eq!(tx.status, TransactionStatus::PendingUser);
    }

    #[test]
    fn test_sep24_transaction_completed() {
        let raw = mock_sep24_transaction_completed();
        let tx = sep24::fetch_sep24_transaction_status(raw).expect("completed sep24 tx must parse");
        assert_eq!(tx.id, MOCK_TXN_ID_24);
        assert_eq!(tx.status, TransactionStatus::Completed);
        assert!(tx.stellar_transaction_id.is_some());
    }

    #[test]
    fn test_sep24_transaction_minimal() {
        let raw = mock_sep24_transaction_minimal();
        let tx = sep24::fetch_sep24_transaction_status(raw).expect("minimal sep24 tx must parse");
        assert_eq!(tx.id, "sep24-min-001");
        assert!(tx.more_info_url.is_none());
    }

    #[test]
    fn test_sep24_transaction_full() {
        let raw = mock_sep24_transaction_full();
        let tx = sep24::fetch_sep24_transaction_status(raw).expect("full sep24 tx must parse");
        assert_eq!(tx.id, "sep24-full-001");
        assert!(tx.more_info_url.is_some());
        assert!(tx.stellar_transaction_id.is_some());
    }

    // ── SEP-38 Fixtures ───────────────────────────────────────────────────

    #[test]
    fn test_sep38_price() {
        let raw = mock_price();
        let price = sep38::fetch_prices(raw).expect("price must parse");
        assert_eq!(price.buy_asset, MOCK_ASSET_CODE);
        assert_eq!(price.sell_asset, "XLM");
    }

    #[test]
    fn test_sep38_price_alternative() {
        let raw = mock_price_alternative();
        let price = sep38::fetch_prices(raw).expect("alternative price must parse");
        assert_eq!(price.buy_asset, "EUR");
        assert_eq!(price.sell_asset, "USD");
    }

    #[test]
    fn test_sep38_firm_quote() {
        let raw = mock_firm_quote();
        let quote = sep38::request_firm_quote(raw, MOCK_EXPIRES_AT - 1000).expect("firm quote must parse");
        assert_eq!(quote.id, "mock-quote-001");
        assert!(!quote.id.is_empty());
    }

    #[test]
    fn test_sep38_firm_quote_minimal() {
        let raw = mock_firm_quote_minimal();
        let quote = sep38::request_firm_quote(raw, MOCK_EXPIRES_AT - 1000).expect("minimal quote must parse");
        assert_eq!(quote.id, "quote-min-001");
    }

    #[test]
    fn test_sep38_firm_quote_high_precision() {
        let raw = mock_firm_quote_high_precision();
        let quote = sep38::request_firm_quote(raw, MOCK_EXPIRES_AT - 1000).expect("high precision quote must parse");
        assert_eq!(quote.id, "quote-precision-001");
    }

    #[test]
    fn test_sep38_firm_quote_anchor_a() {
        let raw = mock_firm_quote_anchor_a();
        let quote = sep38::request_firm_quote(raw, MOCK_EXPIRES_AT - 1000).expect("anchor A quote must parse");
        assert_eq!(quote.id, "quote-a-001");
    }

    #[test]
    fn test_sep38_firm_quote_anchor_b() {
        let raw = mock_firm_quote_anchor_b();
        let quote = sep38::request_firm_quote(raw, MOCK_EXPIRES_AT - 1000).expect("anchor B quote must parse");
        assert_eq!(quote.id, "quote-b-001");
    }

    // ── Cross-anchor normalization tests ──────────────────────────────────

    #[test]
    fn test_sep6_normalization_consistency_across_anchors() {
        let raw_a = mock_deposit_response_anchor_a();
        let raw_b = mock_deposit_response_anchor_b();

        let deposit_a = initiate_deposit(raw_a).expect("anchor A must parse");
        let deposit_b = initiate_deposit(raw_b).expect("anchor B must parse");

        // Both should normalize to the same structure
        assert_eq!(deposit_a.how.is_empty(), false);
        assert_eq!(deposit_b.how.is_empty(), false);
        // But have different transaction IDs
        assert_ne!(deposit_a.transaction_id, deposit_b.transaction_id);
    }

    #[test]
    fn test_sep38_quote_comparison_across_anchors() {
        let raw_a = mock_firm_quote_anchor_a();
        let raw_b = mock_firm_quote_anchor_b();

        let quote_a = sep38::request_firm_quote(raw_a, MOCK_EXPIRES_AT - 1000).expect("anchor A quote must parse");
        let quote_b = sep38::request_firm_quote(raw_b, MOCK_EXPIRES_AT - 1000).expect("anchor B quote must parse");

        // Anchor B has better rate (lower price)
        assert!(quote_b.rate < quote_a.rate);
    }

    // ── Edge case tests ───────────────────────────────────────────────────

    #[test]
    fn test_sep6_deposit_with_zero_amounts() {
        let mut raw = mock_deposit_response_minimal();
        raw.min_amount = Some(0);
        raw.max_amount = Some(0);
        let deposit = initiate_deposit(raw).expect("zero amounts must parse");
        assert_eq!(deposit.transaction_id, "minimal-txn-001");
    }

    #[test]
    fn test_sep6_withdrawal_with_large_amounts() {
        let mut raw = mock_withdrawal_response_full();
        raw.min_amount = Some(1_000_000);
        raw.max_amount = Some(1_000_000_000);
        let withdrawal = initiate_withdrawal(raw).expect("large amounts must parse");
        assert_eq!(withdrawal.min_amount, Some(1_000_000));
    }

    #[test]
    fn test_sep38_quote_with_very_small_price() {
        let mut raw = mock_firm_quote_minimal();
        raw.price = "0.00001".to_string();
        let quote = sep38::request_firm_quote(raw, MOCK_EXPIRES_AT - 1000).expect("small price must parse");
        assert!(!quote.id.is_empty());
    }

    #[test]
    fn test_sep38_quote_with_very_large_price() {
        let mut raw = mock_firm_quote_minimal();
        raw.price = "999999.99".to_string();
        let quote = sep38::request_firm_quote(raw, MOCK_EXPIRES_AT - 1000).expect("large price must parse");
        assert!(!quote.id.is_empty());
    }

    #[test]
    fn test_sep24_transaction_with_missing_optional_fields() {
        let raw = mock_sep24_transaction_minimal();
        let tx = sep24::get_sep24_transaction_status(raw).expect("minimal sep24 must parse");
        assert!(tx.more_info_url.is_none());
        assert!(tx.stellar_transaction_id.is_none());
        assert!(tx.asset_code.is_none());
    }

    #[test]
    fn test_sep6_transaction_status_values() {
        let statuses = vec![
            "pending_external",
            "pending_anchor",
            "pending_user_transfer_start",
            "pending_user_transfer_complete",
            "completed",
            "error",
        ];

        for status in statuses {
            let mut raw = mock_transaction_response_pending();
            raw.status = status.to_string();
            let tx = anchorkit::get_transaction_status(raw).expect(&format!("status {} must parse", status));
            assert_eq!(tx.status, status);
        }
    }

    // ── Additional transaction state fixtures (#639) ──────────────────────

    #[test]
    fn test_sep6_transaction_pending_anchor() {
        let raw = mock_transaction_response_pending_anchor();
        let tx = anchorkit::get_transaction_status(raw).expect("pending_anchor tx must parse");
        assert_eq!(tx.status, "pending_anchor");
        assert_eq!(tx.kind, "withdrawal");
    }

    #[test]
    fn test_sep6_transaction_pending_user_transfer_complete() {
        let raw = mock_transaction_response_pending_user_transfer_complete();
        let tx = anchorkit::get_transaction_status(raw).expect("pending_user_transfer_complete must parse");
        assert_eq!(tx.status, "pending_user_transfer_complete");
        assert_eq!(tx.kind, "deposit");
        assert!(tx.amount_out.is_none());
    }

    #[test]
    fn test_sep6_transaction_expired() {
        let raw = mock_transaction_response_expired();
        let tx = anchorkit::get_transaction_status(raw).expect("expired tx must parse");
        assert_eq!(tx.status, "expired_transaction");
        assert!(tx.amount_in.is_none());
        assert!(tx.amount_out.is_none());
    }

    #[test]
    fn test_sep6_withdrawal_large_limits() {
        let raw = mock_withdrawal_response_large_limits();
        let withdrawal = initiate_withdrawal(raw).expect("large limits withdrawal must parse");
        assert_eq!(withdrawal.min_amount, Some(1_000));
        assert_eq!(withdrawal.max_amount, Some(1_000_000));
        assert_eq!(withdrawal.fee_fixed, Some(50));
    }

    #[test]
    fn test_sep6_withdrawal_completed() {
        let raw = mock_withdrawal_response_completed();
        let withdrawal = initiate_withdrawal(raw).expect("completed withdrawal must parse");
        assert_eq!(withdrawal.status, Some("completed".to_string()));
        assert_eq!(withdrawal.transaction_id, "withdraw-complete-001");
    }

    #[test]
    fn test_sep24_transaction_pending_anchor() {
        let raw = mock_sep24_transaction_pending_anchor();
        let tx = sep24::fetch_sep24_transaction_status(raw).expect("pending_anchor sep24 tx must parse");
        assert_eq!(tx.status, TransactionStatus::PendingAnchor);
    }

    #[test]
    fn test_sep24_transaction_pending_user_transfer_complete() {
        let raw = mock_sep24_transaction_pending_user_transfer_complete();
        let tx = sep24::fetch_sep24_transaction_status(raw).expect("pending_user_transfer_complete must parse");
        assert_eq!(tx.id, "sep24-pending-user-transfer-001");
    }

    #[test]
    fn test_sep24_transaction_expired() {
        let raw = mock_sep24_transaction_expired();
        let tx = sep24::fetch_sep24_transaction_status(raw).expect("expired sep24 tx must parse");
        assert_eq!(tx.id, "sep24-expired-001");
        assert!(tx.more_info_url.is_none());
    }

    #[test]
    fn test_sep24_transaction_refunded() {
        let raw = mock_sep24_transaction_refunded();
        let tx = sep24::fetch_sep24_transaction_status(raw).expect("refunded sep24 tx must parse");
        assert_eq!(tx.id, "sep24-refunded-001");
        assert_eq!(tx.status, TransactionStatus::Refunded);
        assert!(tx.stellar_transaction_id.is_some());
    }

    #[test]
    fn test_sep38_firm_quote_expired() {
        let raw = mock_firm_quote_expired();
        // Use a timestamp after the quote expires
        let now = 1609459201;
        let result = sep38::request_firm_quote(raw, now);
        // Depending on implementation, this might error or mark as expired
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_sep38_firm_quote_large_amounts() {
        let raw = mock_firm_quote_large_amounts();
        let quote = sep38::request_firm_quote(raw, MOCK_EXPIRES_AT - 1000).expect("large amounts quote must parse");
        assert_eq!(quote.id, "quote-large-001");
        assert_eq!(quote.sell_amount_str, "10000000");
    }

    #[test]
    fn test_sep38_firm_quote_small_amounts() {
        let raw = mock_firm_quote_small_amounts();
        let quote = sep38::request_firm_quote(raw, MOCK_EXPIRES_AT - 1000).expect("small amounts quote must parse");
        assert_eq!(quote.id, "quote-small-001");
        assert_eq!(quote.sell_amount_str, "0.01");
    }

    #[test]
    fn test_sep38_price_crypto_pair() {
        let raw = mock_price_crypto_pair();
        let price = sep38::fetch_prices(raw).expect("crypto pair price must parse");
        assert_eq!(price.buy_asset, "BTC");
        assert_eq!(price.sell_asset, "ETH");
        assert_eq!(price.rate_str, "15.5");
    }

    // ── Realistic integration flow tests ──────────────────────────────────

    #[test]
    fn test_realistic_sep6_deposit_flow() {
        // User initiates deposit
        let deposit = initiate_deposit(mock_deposit_response()).expect("initial deposit must parse");
        assert_eq!(deposit.transaction_id, MOCK_TXN_ID);

        // Poll for status: still pending
        let status = anchorkit::get_transaction_status(mock_transaction_response_pending())
            .expect("pending status must parse");
        assert_eq!(status.status, "pending_external");

        // Poll again: now completed
        let completed = anchorkit::get_transaction_status(mock_transaction_response_completed())
            .expect("completed status must parse");
        assert_eq!(completed.status, "completed");
    }

    #[test]
    fn test_realistic_sep24_interactive_flow() {
        // User starts interactive deposit
        let interactive = sep24::initiate_interactive_deposit(mock_interactive_deposit_response())
            .expect("interactive deposit must parse");
        assert_eq!(interactive.id, MOCK_TXN_ID_24);

        // Poll: user hasn't started transfer
        let status = sep24::fetch_sep24_transaction_status(mock_sep24_transaction_pending_user_transfer_complete())
            .expect("pending status must parse");
        assert_eq!(status.id, "sep24-pending-user-transfer-001");

        // Poll: transfer complete
        let completed = sep24::fetch_sep24_transaction_status(mock_sep24_transaction_completed())
            .expect("completed status must parse");
        assert_eq!(completed.status, TransactionStatus::Completed);
    }

    #[test]
    fn test_realistic_sep38_quote_lifecycle() {
        // Request quote
        let quote = sep38::request_firm_quote(
            mock_firm_quote(),
            MOCK_EXPIRES_AT - 3600,
        ).expect("initial quote must parse");
        assert_eq!(quote.id, "mock-quote-001");

        // Check expiry (not yet expired)
        assert!(!sep38::is_quote_expired(&quote, MOCK_EXPIRES_AT - 1000));

        // Later: check expiry (expired)
        assert!(sep38::is_quote_expired(&quote, MOCK_EXPIRES_AT + 1000));
    }

    #[test]
    fn test_sep6_withdrawal_with_different_memo_types() {
        let mut raw_text = mock_withdrawal_response();
        raw_text.memo_type = Some("text".to_string());
        raw_text.memo = Some("MYTEXT".to_string());
        let w1 = initiate_withdrawal(raw_text).expect("text memo must parse");
        assert_eq!(w1.memo_type, Some("text".to_string()));

        let mut raw_hash = mock_withdrawal_response();
        raw_hash.memo_type = Some("hash".to_string());
        raw_hash.memo = Some("a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3".to_string());
        let w2 = initiate_withdrawal(raw_hash).expect("hash memo must parse");
        assert_eq!(w2.memo_type, Some("hash".to_string()));
    }

    #[test]
    fn test_multiple_anchor_comparison() {
        let a = initiate_deposit(mock_deposit_response_anchor_a()).expect("anchor A deposit must parse");
        let b = initiate_deposit(mock_deposit_response_anchor_b()).expect("anchor B deposit must parse");

        // Different fees and limits
        assert_ne!(a.fee_fixed, b.fee_fixed);
        assert_ne!(a.min_amount, b.min_amount);
        assert_ne!(a.max_amount, b.max_amount);

        // But both valid
        assert!(!a.how.is_empty());
        assert!(!b.how.is_empty());
    }
}
