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
use anchorkit::errors::ErrorCode as Error;

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
    let best = select_best(&candidates, "LowestFee", 1.0, 0.0, 0.0).unwrap().unwrap();
    assert_eq!(best.anchor, "anchor-b"); // fee 20
}

#[test]
fn test_select_best_fastest_settlement() {
    let candidates = make_candidates();
    let best = select_best(&candidates, "FastestSettlement", 0.0, 1.0, 0.0).unwrap().unwrap();
    assert_eq!(best.anchor, "anchor-c"); // settlement_time 30
}

#[test]
fn test_select_best_highest_reputation() {
    let candidates = make_candidates();
    let best = select_best(&candidates, "HighestReputation", 0.0, 0.0, 1.0).unwrap().unwrap();
    assert_eq!(best.anchor, "anchor-b"); // reputation 90
}

#[test]
fn test_select_best_weighted_score() {
    let candidates = make_candidates();
    // Equal weights — anchor-b has best fee + reputation, anchor-c best speed
    let best = select_best(&candidates, "WeightedScore", 0.333, 0.333, 0.334).unwrap();
    assert!(best.is_some());
}

#[test]
fn test_select_best_unknown_strategy_is_rejected() {
    let candidates = make_candidates();
    let result = select_best(&candidates, "UndefinedStrategy", 0.5, 0.25, 0.25);
    assert_eq!(result.err(), Some(Error::ValidationError)); // no silent LowestFee fallback
}

#[test]
fn test_select_best_empty_candidates_returns_none() {
    let result = select_best(&[], "LowestFee", 1.0, 0.0, 0.0);
    assert!(result.unwrap().is_none());
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

// ── Self-route exclusion ──────────────────────────────────────────────────────

/// A CandidateQuote where base_asset == quote_asset is a self-route and must
/// never be selected, even when it would otherwise score best.
#[test]
fn test_route_self_route_candidate_is_excluded() {
    let quotes = vec![
        // Self-route: XLM → XLM.  Has perfect fee and reputation scores; must
        // be filtered out before selection despite this favourable profile.
        make_quote(1, "anchor-self", "XLM", "XLM", 0, 1_000_000, 100, 1, 1, 0, NOW + 3600),
        // Valid distinct-pair quote that should win.
        make_quote(2, "anchor-valid", "XLM", "USDC", 20, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 1, "self-route must not fill the corridor");
    assert_eq!(result.filled[0].anchor, "anchor-valid");
}

/// When the only available candidate is a self-route the corridor is unfilled,
/// not erroneously filled with the self-routing anchor.
#[test]
fn test_route_only_self_route_produces_unfilled() {
    let quotes = vec![
        make_quote(1, "anchor-self", "USDC", "USDC", 0, 1_000_000, 100, 1, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("USDC", "EURC", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 0);
    assert_eq!(result.unfilled.len(), 1);
}

// ── Empty-route input rejection ───────────────────────────────────────────────

/// A CandidateQuote with an empty base_asset is an invalid empty-route input
/// and must be rejected before scoring, regardless of the request.
#[test]
fn test_route_empty_base_asset_in_candidate_returns_error() {
    let quotes = vec![
        // Candidate with blank base_asset — this is the malformed input.
        make_quote(1, "anchor-bad", "", "USDC", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600),
        // A perfectly valid quote in the same pool.
        make_quote(2, "anchor-ok", "XLM", "USDC", 20, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let err = route_multi_asset(&requests, &quotes, NOW).unwrap_err();
    assert_eq!(err, Error::InvalidAssetCode);
}

/// A CandidateQuote with an empty quote_asset must likewise be rejected before
/// any scoring occurs.
#[test]
fn test_route_empty_quote_asset_in_candidate_returns_error() {
    let quotes = vec![
        make_quote(1, "anchor-bad", "XLM", "", 10, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let err = route_multi_asset(&requests, &quotes, NOW).unwrap_err();
    assert_eq!(err, Error::InvalidAssetCode);
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

// ── Zero-rate candidate rejection ─────────────────────────────────────────────

/// A CandidateQuote with rate == 0 cannot produce a valid conversion.
/// It must be filtered out before scoring and must never become the selected
/// route, even when it is the only candidate for a corridor.
#[test]
fn test_zero_rate_candidate_never_selected() {
    let quotes = vec![
        // Zero-rate candidate — must be rejected before scoring.
        make_quote(1, "anchor-zero", "XLM", "USDC", 5, 0, 90, 10, 1, 0, NOW + 3600),
        // Valid positive-rate candidate — must win.
        make_quote(2, "anchor-valid", "XLM", "USDC", 20, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 1, "positive-rate candidate must fill the corridor");
    assert_eq!(result.filled[0].anchor, "anchor-valid",
        "zero-rate anchor must not be selected");
    assert_eq!(result.filled[0].rate, 1_000_000);
}

/// When the only available candidate has rate == 0 the corridor is unfilled,
/// not erroneously filled with an unusable quote.
#[test]
fn test_zero_rate_only_candidate_produces_unfilled() {
    let quotes = vec![
        make_quote(1, "anchor-zero", "XLM", "USDC", 5, 0, 90, 10, 1, 0, NOW + 3600),
    ];
    let requests = vec![req("XLM", "USDC", 100, "LowestFee")];
    let result = route_multi_asset(&requests, &quotes, NOW).unwrap();

    assert_eq!(result.filled.len(), 0, "zero-rate-only corridor must be unfilled");
    assert_eq!(result.unfilled, vec!["XLM/USDC".to_string()]);
}

// ── WeightedScore: score ordering is independent of pool composition ──────────

/// Adding a poor (high-fee, slow, low-reputation) candidate to an existing
/// pool must not change the relative ordering of the pre-existing candidates.
/// This verifies that normalization uses fixed domain bounds, not candidate
/// maxima.
#[test]
fn test_weighted_score_ordering_stable_with_poor_candidate_added() {
    // Base pool: three candidates with distinct profiles.
    let base_pool = vec![
        make_quote(1, "anchor-a", "XLM", "USDC", 10, 1_000_000, 90, 20, 1, 0, NOW + 3600),
        make_quote(2, "anchor-b", "XLM", "USDC", 30, 1_000_000, 70, 40, 1, 0, NOW + 3600),
        make_quote(3, "anchor-c", "XLM", "USDC", 50, 1_000_000, 80, 60, 1, 0, NOW + 3600),
    ];

    // Extended pool: base pool + a clearly inferior candidate.
    let mut extended_pool = base_pool.clone();
    extended_pool.push(
        make_quote(99, "anchor-poor", "XLM", "USDC", 99, 1_000_000, 1, 9_999, 1, 0, NOW + 3600),
    );

    let requests = vec![req("XLM", "USDC", 100, "WeightedScore")];

    let result_base = route_multi_asset(&requests, &base_pool, NOW).unwrap();
    let result_extended = route_multi_asset(&requests, &extended_pool, NOW).unwrap();

    assert_eq!(result_base.filled.len(), 1);
    assert_eq!(result_extended.filled.len(), 1);

    // The winner from the base pool must be identical after adding a poor candidate.
    assert_eq!(
        result_base.filled[0].anchor,
        result_extended.filled[0].anchor,
        "adding a poor candidate must not change the WeightedScore winner"
    );

    // The poor candidate itself must never win.
    assert_ne!(
        result_extended.filled[0].anchor, "anchor-poor",
        "the poor candidate must not be selected"
    );
}
multi_asset_routing.rs — Multi-asset quote routing helpers (#656).
//
// This module extends the base routing layer so callers can evaluate and
// select quotes across multiple asset pairs in a single pass.  It is compiled
// as part of the host (non-WASM) build and is re-exported from `lib.rs`.
//
// # Design
//
// The core abstraction is `MultiAssetRoutingRequest`, which bundles one or
// more `AssetPairRequest` entries.  Each entry carries an independent routing
// strategy so callers can mix LowestFee for one corridor with
// HighestReputation for another in the same call.
//
// `MultiAssetRoutingResult` groups the winning quotes per pair and a list of
// any pairs that produced no candidates (`unfilled`).
//
// # Asset normalisation
//
// Asset codes are normalised to uppercase before comparison so that `usdc`,
// `USDC`, and `Usdc` all resolve to the same corridor.  Only ASCII letters are
// case-folded, so no non-ASCII spelling can alias a valid code.
//
// # Invalid combinations
//
// `validate_asset_pair_request` rejects a request when:
//   - either asset code is empty, exceeds 12 characters, or contains anything
//     other than ASCII letters and digits (`InvalidAssetCode`)
//   - `base_asset == quote_asset` (circular corridor, `InvalidAssetPair`)
//   - `amount == 0` (`InvalidAmount`)
//   - `strategy` is not one of `ROUTING_STRATEGIES` (`ValidationError`)

extern crate alloc;
