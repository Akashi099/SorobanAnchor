// tests/multi_asset_routing_tests.rs
//
// Integration tests for multi-asset quote routing (#656).
//
// Tests cover:
//   - Single and multi-pair routing (happy path)
//   - Each routing strategy (LowestFee, FastestSettlement, HighestReputation, WeightedScore)
//   - Asset code normalisation (mixed case, whitespace)
//   - Expired-quote filtering
//   - Amount boundary enforcement (min/max)
//   - Reputation filter
//   - Unfilled pairs
//   - Invalid asset combinations (same base/quote, empty code, too long, zero amount)
//   - Mixed valid/invalid pairs — invalid entry propagates error immediately

use anchorkit::multi_asset_routing::{
    route_multi_asset, validate_asset_pair_request,
    normalize_asset_code, pair_key, select_best,
    AssetPairRequest, CandidateQuote, MultiAssetRoutingResult,
};
use anchorkit::errors::Error;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_quote(
    id: u64,
    anchor: &str,
    base: &str,
    quote: &str,
    fee: u32,
    rate: u64,
    reputation: u32,
    settlement_time: u64,
    min_amount: u64,
    max_amount: u64,
    valid_until: u64,
) -> CandidateQuote {
    CandidateQuote {
        quote_id: id,
        anchor: anchor.to_string(),
        base_asset: base.to_string(),
        quote_asset: quote.to_string(),
        rate,
        fee_percentage: fee,
        minimum_amount: min_amount,
        maximum_amount: max_amount,
        valid_until,
        reputation_score: reputation,
        average_settlement_time: settlement_time,
        routing_reason: None,
    }
}

fn req(base: &str, quote_asset: &str, amount: u64, strategy: &str) -> AssetPairRequest {
    AssetPairRequest {
        base_asset: base.to_string(),
        quote_asset: quote_asset.to_string(),
        amount,
        strategy: strategy.to_string(),
        min_reputation: 0,
    }
}

fn req_with_rep(base: &str, quote_asset: &str, amount: u64, strategy: &str, min_rep: u32) -> AssetPairRequest {
    AssetPairRequest {
        base_asset: base.to_string(),
        quote_asset: quote_asset.to_string(),
        amount,
        strategy: strategy.to_string(),
        min_reputation: min_rep,
    }
}

const NOW: u64 = 1_700_000_000;

// ── Asset normalisation ───────────────────────────────────────────────────────

#[test]
fn test_normalize_asset_code_uppercase() {
    assert_eq!(normalize_asset_code("usdc"), "USDC");
    assert_eq!(normalize_asset_code("Usdc"), "USDC");
    assert_eq!(normalize_asset_code("USDC"), "USDC");
}

#[test]
fn test_normalize_asset_code_trims_whitespace() {
    assert_eq!(normalize_asset_code("  xlm  "), "XLM");
}

#[test]
fn test_pair_key_normalizes() {
    assert_eq!(pair_key("xlm", "usdc"), "XLM/USDC");
    assert_eq!(pair_key("XLM", "USDC"), "XLM/USDC");
    assert_eq!(pair_key("  xlm ", " USDC "), "XLM/USDC");
}

// ── Validation ────────────────────────────────────────────────────────────────

#[test]
fn test_validate_valid_request() {
    let r = req("XLM", "USDC", 100, "LowestFee");
    assert!(validate_asset_pair_request(&r).is_ok());
}

#[test]
fn test_validate_empty_base_asset_rejected() {
    let r = req("", "USDC", 100, "LowestFee");
    assert_eq!(validate_asset_pair_request(&r), Err(Error::InvalidAssetCode));
}

#[test]
fn test_validate_empty_quote_asset_rejected() {
    let r = req("XLM", "", 100, "LowestFee");
    assert_eq!(validate_asset_pair_request(&r), Err(Error::InvalidAssetCode));
}

#[test]
fn test_validate_base_too_long_rejected() {
    let r = req("ABCDEFGHIJKLMN", "USDC", 100, "LowestFee"); // 14 chars > 12
    assert_eq!(validate_asset_pair_request(&r), Err(Error::InvalidAssetCode));
}

#[test]
fn test_validate_quote_too_long_rejected() {
    let r = req("XLM", "ABCDEFGHIJKLMN", 100, "LowestFee"); // 14 chars > 12
    assert_eq!(validate_asset_pair_request(&r), Err(Error::InvalidAssetCode));
}

#[test]
fn test_validate_same_asset_pair_rejected() {
    let r = req("USDC", "USDC", 100, "LowestFee");
    assert_eq!(validate_asset_pair_request(&r), Err(Error::InvalidAssetPair));
}

#[test]
fn test_validate_same_asset_pair_case_insensitive_rejected() {
    let r = req("usdc", "USDC", 100, "LowestFee");
    assert_eq!(validate_asset_pair_request(&r), Err(Error::InvalidAssetPair));
}

#[test]
fn test_validate_zero_amount_rejected() {
    let r = req("XLM", "USDC", 0, "LowestFee");
    assert_eq!(validate_asset_pair_request(&r), Err(Error::InvalidAmount));
}

// ── select_best — strategy unit tests ────────────────────────────────────────

fn make_candidates() -> Vec<CandidateQuote> {
    vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 50, 1_000_000, 80, 60, 1, 0, NOW + 3600),
        make_quote(2, "anchor-b", "XLM", "USDC", 20, 1_000_000, 90, 120, 1, 0, NOW + 3600),
        make_quote(3, "anchor-c", "XLM", "USDC", 35, 1_000_000, 70, 30, 1, 0, NOW + 3600),
    ]
}

#[test]
fn test_select_best_lowest_fee() {
    let candidates = make_candidates();
    let best = select_best(&candidates, "LowestFee", 1.0, 0.0, 0.0).unwrap();
    assert_eq!(best.anchor, "anchor-b"); // fee 20
}

#[test]
fn test_select_best_fastest_settlement() {
    let candidates = make_candidates();
    let best = select_best(&candidates, "FastestSettlement", 0.0, 1.0, 0.0).unwrap();
    assert_eq!(best.anchor, "anchor-c"); // settlement_time 30
}

#[test]
fn test_select_best_highest_reputation() {
    let candidates = make_candidates();
    let best = select_best(&candidates, "HighestReputation", 0.0, 0.0, 1.0).unwrap();
    assert_eq!(best.anchor, "anchor-b"); // reputation 90
}

#[test]
fn test_select_best_weighted_score() {
    let candidates = make_candidates();
    // Equal weights — anchor-b has best fee + reputation, anchor-c best speed
    let best = select_best(&candidates, "WeightedScore", 0.333, 0.333, 0.334);
    assert!(best.is_some());
}

#[test]
fn test_select_best_unknown_strategy_falls_back_to_lowest_fee() {
    let candidates = make_candidates();
    let best = select_best(&candidates, "UndefinedStrategy", 0.5, 0.25, 0.25).unwrap();
    assert_eq!(best.anchor, "anchor-b"); // lowest fee is the fallback
}

#[test]
fn test_select_best_empty_candidates_returns_none() {
    let result = select_best(&[], "LowestFee", 1.0, 0.0, 0.0);
    assert!(result.is_none());
}

// ── route_multi_asset — routing engine integration tests ──────────────────────

#[test]
fn test_route_single_pair_lowest_fee() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 50, 1_000_000, 80, 60, 1, 0, NOW + 3600),
        make_quote(2, "anchor-b", "XLM", "USDC", 20, 1_000_000, 90, 120, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 1);
    assert_eq!(result.unfilled.len(), 0);
    assert_eq!(result.filled[0].anchor, "anchor-b");
    assert_eq!(result.filled[0].pair_key, "XLM/USDC");
    assert_eq!(result.filled[0].strategy_applied, "LowestFee");
}

#[test]
fn test_route_multi_pair_independent_selection() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 30, 1_000_000, 80, 60, 1, 0, NOW + 3600),
        make_quote(2, "anchor-b", "XLM", "USDC", 20, 1_000_000, 70, 90, 1, 0, NOW + 3600),
        make_quote(3, "anchor-c", "BTC", "USDC", 15, 50_000_000, 85, 45, 1, 0, NOW + 3600),
        make_quote(4, "anchor-d", "BTC", "USDC", 25, 50_000_000, 90, 30, 1, 0, NOW + 3600),
    ];
    let requests = vec![
        req("XLM", "USDC", 100, "LowestFee"),
        req("BTC", "USDC", 1, "HighestReputation"),
    ];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 2);
    assert_eq!(result.unfilled.len(), 0);

    let xlm_result = result.filled.iter().find(|r| r.pair_key == "XLM/USDC").unwrap();
    assert_eq!(xlm_result.anchor, "anchor-b"); // lowest fee

    let btc_result = result.filled.iter().find(|r| r.pair_key == "BTC/USDC").unwrap();
    assert_eq!(btc_result.anchor, "anchor-d"); // highest reputation
}

#[test]
fn test_route_no_matching_pair_produces_unfilled() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 20, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![
        req("XLM", "USDC", 100, "LowestFee"),
        req("BTC", "USDC", 1, "LowestFee"), // no BTC/USDC quotes
    ];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 1);
    assert_eq!(result.unfilled.len(), 1);
    assert!(result.unfilled.contains(&"BTC/USDC".to_string()));
}

#[test]
fn test_route_all_pairs_unfilled_when_no_quotes() {
    let requests = vec![
        req("XLM", "USDC", 100, "LowestFee"),
        req("BTC", "USDC", 1, "LowestFee"),
    ];
    let result = route_multi_asset(&requests, &[], NOW).unwrap();

    assert_eq!(result.filled.len(), 0);
    assert_eq!(result.unfilled.len(), 2);
}

// ── Expired quote filtering ───────────────────────────────────────────────────

#[test]
fn test_route_expired_quotes_are_excluded() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW - 1), // expired
        make_quote(2, "anchor-b", "XLM", "USDC", 20, 1_000_000, 90, 30, 1, 0, NOW + 3600), // valid
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 1);
    // anchor-a has lower fee but is expired; anchor-b should win
    assert_eq!(result.filled[0].anchor, "anchor-b");
}

#[test]
fn test_route_all_expired_produces_unfilled() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW - 100),
        make_quote(2, "anchor-b", "XLM", "USDC", 20, 1_000_000, 90, 30, 1, 0, NOW - 1),
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 0);
    assert_eq!(result.unfilled.len(), 1);
}

// ── Amount boundary enforcement ───────────────────────────────────────────────

#[test]
fn test_route_amount_below_minimum_excluded() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 500, 0, NOW + 3600), // min 500
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")]; // amount < min
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 0);
    assert_eq!(result.unfilled.len(), 1);
}

#[test]
fn test_route_amount_above_maximum_excluded() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 50, NOW + 3600), // max 50
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")]; // amount > max
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 0);
    assert_eq!(result.unfilled.len(), 1);
}

#[test]
fn test_route_zero_maximum_means_no_upper_limit() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600), // max 0 = unlimited
    ];
    let requests = vec![req("XLM", "USDC", 999_999_999, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 1);
}

// ── Reputation filter ─────────────────────────────────────────────────────────

#[test]
fn test_route_reputation_filter_excludes_low_reputation() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 60, 60, 1, 0, NOW + 3600), // rep 60
        make_quote(2, "anchor-b", "XLM", "USDC", 20, 1_000_000, 90, 60, 1, 0, NOW + 3600), // rep 90
    ];
    let requests = vec![req_with_rep("XLM", "USDC", 100, "LowestFee", 75)]; // min_rep 75
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 1);
    assert_eq!(result.filled[0].anchor, "anchor-b"); // anchor-a excluded by reputation
}

#[test]
fn test_route_all_below_reputation_threshold_produces_unfilled() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 50, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![req_with_rep("XLM", "USDC", 100, "LowestFee", 80)];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 0);
    assert_eq!(result.unfilled.len(), 1);
}

// ── Asset code case normalisation in routing ──────────────────────────────────

#[test]
fn test_route_case_insensitive_asset_matching() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    // Request uses lowercase — should still match
    let requests = vec![req("xlm", "usdc", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 1);
    assert_eq!(result.filled[0].pair_key, "XLM/USDC");
    assert_eq!(result.filled[0].anchor, "anchor-a");
}

#[test]
fn test_route_mixed_case_in_quotes_matched_correctly() {
    // Quote stored with lowercase asset codes
    let mut q = make_quote(1, "anchor-a", "xlm", "usdc", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600);
    q.base_asset = "xlm".to_string();
    q.quote_asset = "usdc".to_string();

    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let result = route_multi_asset(&[req("XLM", "USDC", 100, "LowestFee")], &[q], NOW).unwrap();

    assert_eq!(result.filled.len(), 1);
}

// ── Invalid combinations return errors immediately ────────────────────────────

#[test]
fn test_route_invalid_same_asset_pair_returns_error() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("USDC", "USDC", 100, "LowestFee")]; // circular
    let err = route_multi_asset(&requests, &quotes, NOW).unwrap_err();
    assert_eq!(err, Error::InvalidAssetPair);
}

#[test]
fn test_route_zero_amount_returns_error() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("XLM", "USDC", 0, "LowestFee")];
    let err = route_multi_asset(&requests, &quotes, NOW).unwrap_err();
    assert_eq!(err, Error::InvalidAmount);
}

#[test]
fn test_route_empty_base_asset_returns_error() {
    let requests = vec![req("", "USDC", 100, "LowestFee")];
    let err = route_multi_asset(&requests, &[], NOW).unwrap_err();
    assert_eq!(err, Error::InvalidAssetCode);
}

#[test]
fn test_route_empty_quote_asset_returns_error() {
    let requests = vec![req("XLM", "", 100, "LowestFee")];
    let err = route_multi_asset(&requests, &[], NOW).unwrap_err();
    assert_eq!(err, Error::InvalidAssetCode);
}

#[test]
fn test_route_asset_code_too_long_returns_error() {
    let requests = vec![req("AVERYLONGCODEEXCEEDING12", "USDC", 100, "LowestFee")];
    let err = route_multi_asset(&requests, &[], NOW).unwrap_err();
    assert_eq!(err, Error::InvalidAssetCode);
}

#[test]
fn test_route_invalid_request_in_batch_aborts_entire_call() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    // First request valid, second invalid — entire call should fail
    let requests = vec![
        req("XLM", "USDC", 100, "LowestFee"),
        req("USDC", "USDC", 50, "LowestFee"), // circular — invalid
    ];
    let err = route_multi_asset(&requests, &quotes, NOW).unwrap_err();
    assert_eq!(err, Error::InvalidAssetPair);
}

// ── routing_reason propagated ─────────────────────────────────────────────────

#[test]
fn test_route_routing_reason_propagated_when_set() {
    let mut q = make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600);
    q.routing_reason = Some("promotional-rate".to_string());

    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &[q], NOW).unwrap();

    assert_eq!(result.filled.len(), 1);
    assert_eq!(result.filled[0].routing_reason, Some("promotional-rate".to_string()));
}

#[test]
fn test_route_routing_reason_none_when_not_set() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled[0].routing_reason, None);
}

// ── Empty request list ────────────────────────────────────────────────────────

#[test]
fn test_route_empty_request_list_returns_empty_result() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    let result = route_multi_asset(&[], &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 0);
    assert_eq!(result.unfilled.len(), 0);
}

// ── Multiple strategies in a single batch ────────────────────────────────────

#[test]
fn test_route_mixed_strategies_in_single_call() {
    let quotes = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 50, 1_000_000, 80, 30, 1, 0, NOW + 3600),
        make_quote(2, "anchor-b", "XLM", "USDC", 20, 1_000_000, 60, 120, 1, 0, NOW + 3600),
        make_quote(3, "anchor-c", "BTC", "USDC", 30, 50_000_000, 95, 15, 1, 0, NOW + 3600),
        make_quote(4, "anchor-d", "BTC", "USDC", 40, 50_000_000, 50, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![
        req("XLM", "USDC", 100, "LowestFee"),           // anchor-b (fee 20)
        req("BTC", "USDC", 1, "FastestSettlement"),     // anchor-c (time 15)
    ];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 2);

    let xlm = result.filled.iter().find(|r| r.pair_key == "XLM/USDC").unwrap();
    assert_eq!(xlm.anchor, "anchor-b");

    let btc = result.filled.iter().find(|r| r.pair_key == "BTC/USDC").unwrap();
    assert_eq!(btc.anchor, "anchor-c");
}

// ── Quote fields are propagated correctly ─────────────────────────────────────

#[test]
fn test_route_quote_id_and_rate_propagated() {
    let quotes = vec![
        make_quote(42, "anchor-a", "XLM", "USDC", 10, 2_500_000, 80, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled[0].quote_id, 42);
    assert_eq!(result.filled[0].rate, 2_500_000);
    assert_eq!(result.filled[0].fee_percentage, 10);
}
