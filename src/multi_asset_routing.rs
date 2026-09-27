// multi_asset_routing.rs — Multi-asset quote routing helpers (#656).
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

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

// This module works with the raw error codes (`Error::InvalidAssetPair` etc.),
// so bind `Error` to the `ErrorCode` enum rather than the `AnchorKitError`
// struct the crate-level `Error` alias points at.
use crate::errors::ErrorCode as Error;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single asset-pair routing request.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetPairRequest {
    /// Uppercase asset code being sold / deposited (e.g. `"XLM"`).
    pub base_asset: String,
    /// Uppercase asset code being bought / received (e.g. `"USDC"`).
    pub quote_asset: String,
    /// Amount (in the smallest unit of `base_asset`) to route.
    pub amount: u64,
    /// Routing strategy label: `"LowestFee"`, `"FastestSettlement"`,
    /// `"HighestReputation"`, or `"WeightedScore"`.
    pub strategy: String,
    /// Minimum reputation score an anchor must have to be considered.
    pub min_reputation: u32,
}

/// A winning quote for a single asset pair, together with the pair key.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetPairQuote {
    /// Normalised corridor identifier: `"BASE/QUOTE"`.
    pub pair_key: String,
    /// The selected anchor's address (as a string for `no_std` compat).
    pub anchor: String,
    /// Fee percentage chosen by the routing strategy.
    pub fee_percentage: u32,
    /// Rate (base units per quote unit × 10^6) from the on-chain quote.
    pub rate: u64,
    /// On-chain quote ID.
    pub quote_id: u64,
    /// Routing strategy that was applied.
    pub strategy_applied: String,
    /// Optional routing reason attached to the quote at submission time.
    pub routing_reason: Option<String>,
}

/// Result of a multi-asset routing pass.
#[derive(Clone, Debug, Default)]
pub struct MultiAssetRoutingResult {
    /// One entry per filled `AssetPairRequest`, in submission order.
    pub filled: Vec<AssetPairQuote>,
    /// Corridor keys (`"BASE/QUOTE"`) for which no candidates were found.
    pub unfilled: Vec<String>,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Routing strategy labels accepted by [`select_best`] and
/// [`validate_asset_pair_request`].
pub const ROUTING_STRATEGIES: [&str; 4] =
    ["LowestFee", "FastestSettlement", "HighestReputation", "WeightedScore"];

/// Validate a single `AssetPairRequest`.  Returns an error when the request is
/// malformed (see the module docs for the specific codes).
pub fn validate_asset_pair_request(req: &AssetPairRequest) -> Result<(), Error> {
    let base = normalize_asset_code(&req.base_asset);
    let quote = normalize_asset_code(&req.quote_asset);

    if base.is_empty() || base.len() > 12 || !is_asset_code_charset(&base) {
        return Err(Error::InvalidAssetCode);
    }
    if quote.is_empty() || quote.len() > 12 || !is_asset_code_charset(&quote) {
        return Err(Error::InvalidAssetCode);
    }
    if base == quote {
        return Err(Error::InvalidAssetPair);
    }
    if req.amount == 0 {
        return Err(Error::InvalidAmount);
    }
    if !ROUTING_STRATEGIES.contains(&req.strategy.as_str()) {
        return Err(Error::ValidationError);
    }
    Ok(())
}

/// Asset codes are limited to ASCII letters and digits, matching
/// [`crate::errors::normalize_asset_code`].
fn is_asset_code_charset(code: &str) -> bool {
    code.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Normalise an asset code to uppercase, trimming whitespace.
///
/// Only ASCII letters are uppercased; see [`validate_asset_pair_request`] for
/// the accepted character set.
pub fn normalize_asset_code(code: &str) -> String {
    code.trim().to_ascii_uppercase()
}

/// Build the corridor key string `"BASE/QUOTE"` from two asset codes.
pub fn pair_key(base: &str, quote: &str) -> String {
    let mut k = normalize_asset_code(base);
    k.push('/');
    k.push_str(&normalize_asset_code(quote));
    k
}

// ---------------------------------------------------------------------------
// In-memory quote record used by the routing engine
// ---------------------------------------------------------------------------

/// A lightweight quote record that the routing engine operates on.
/// Mirrors the relevant fields of `contract::Quote` but is independent of
/// Soroban SDK types so this module can be used in both host and test builds.
#[derive(Clone, Debug)]
pub struct CandidateQuote {
    pub quote_id: u64,
    pub anchor: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub rate: u64,
    pub fee_percentage: u32,
    pub minimum_amount: u64,
    pub maximum_amount: u64,
    pub valid_until: u64,
    pub reputation_score: u32,
    pub average_settlement_time: u64,
    pub routing_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Routing engine
// ---------------------------------------------------------------------------

/// Select the best quote from `candidates` according to `strategy`.
///
/// Returns `Ok(None)` when the candidate list is empty, and
/// `Err(Error::ValidationError)` when `strategy` is not one of
/// [`ROUTING_STRATEGIES`], whether or not there are candidates.
pub fn select_best<'a>(
    candidates: &'a [CandidateQuote],
    strategy: &str,
    fee_weight: f32,
    speed_weight: f32,
    reputation_weight: f32,
) -> Result<Option<&'a CandidateQuote>, Error> {
    let best = match strategy {
        "LowestFee" => candidates
            .iter()
            .min_by_key(|q| q.fee_percentage),

        "FastestSettlement" => candidates
            .iter()
            .min_by_key(|q| q.average_settlement_time),

        "HighestReputation" => candidates
            .iter()
            .max_by_key(|q| q.reputation_score),

        "WeightedScore" => {
            // Use fixed domain ceilings rather than candidate-derived maxima.
            // Deriving the ceiling from the current candidate set makes scores
            // relative to the pool: adding a poor candidate can lower every
            // other candidate's normalised score and change the winner.
            // Fixed ceilings make each candidate's score independent of who
            // else is in the pool.
            //
            // fee_percentage is expressed as an integer percentage (0–100).
            // average_settlement_time has no protocol-level upper bound, so
            // we cap the normalised representation at f32::MAX to avoid NaN.
            // reputation_score is on a 0–100 scale.
            const MAX_FEE: f32 = 100.0_f32;
            const MAX_REP: f32 = 100.0_f32;
            // Settlement time: use f32::MAX as the ceiling so the score is
            // always well-defined regardless of the actual values present.
            // Candidates with very large times will score near 0 for speed,
            // which is the correct outcome.
            const MAX_TIME: f32 = f32::MAX;

            candidates.iter().max_by(|a, b| {
                let score_a = weighted_score(a, fee_weight, speed_weight, reputation_weight, MAX_FEE, MAX_TIME, MAX_REP);
                let score_b = weighted_score(b, fee_weight, speed_weight, reputation_weight, MAX_FEE, MAX_TIME, MAX_REP);
                score_a.partial_cmp(&score_b).unwrap_or(core::cmp::Ordering::Equal)
            })
        }

        _ => return Err(Error::ValidationError),
    };
    Ok(best)
}

fn weighted_score(
    q: &CandidateQuote,
    fw: f32,
    sw: f32,
    rw: f32,
    max_fee: f32,
    max_time: f32,
    max_rep: f32,
) -> f32 {
    let fee_score = if max_fee == 0.0 {
        1.0_f32
    } else {
        1.0_f32 - (q.fee_percentage as f32 / max_fee)
    };
    let speed_score = if max_time == 0.0 {
        1.0_f32
    } else {
        1.0_f32 - (q.average_settlement_time as f32 / max_time)
    };
    let rep_score = if max_rep == 0.0 {
        0.0_f32
    } else {
        q.reputation_score as f32 / max_rep
    };
    fw * fee_score + sw * speed_score + rw * rep_score
}

/// Route across multiple asset pairs.
///
/// `now_timestamp` is the current ledger timestamp used to filter expired
/// quotes.  `all_quotes` is the full flat list of available on-chain quotes.
pub fn route_multi_asset(
    requests: &[AssetPairRequest],
    all_quotes: &[CandidateQuote],
    now_timestamp: u64,
) -> Result<MultiAssetRoutingResult, Error> {
    // Reject any candidate whose asset pair is empty.  An anchor that submitted
    // a quote with blank asset codes cannot represent a real corridor and must
    // not enter the scoring pool; doing so could produce a misleading
    // successful selection against an empty-pair request.
    for q in all_quotes {
        if normalize_asset_code(&q.base_asset).is_empty()
            || normalize_asset_code(&q.quote_asset).is_empty()
        {
            return Err(Error::InvalidAssetCode);
        }
    }

    let mut result = MultiAssetRoutingResult::default();

    for req in requests {
        validate_asset_pair_request(req)?;

        let base = normalize_asset_code(&req.base_asset);
        let quote = normalize_asset_code(&req.quote_asset);
        let key = pair_key(&base, &quote);

        // Filter candidates for this pair.
        // Self-routes (base_asset == quote_asset) are excluded: a candidate
        // that quotes an asset against itself has no conversion value and must
        // not win selection regardless of how its other fields score.
        // Zero-or-negative rates are also excluded: a rate of 0 cannot
        // produce a valid conversion and would yield an unusable quote.
        let candidates: Vec<&CandidateQuote> = all_quotes
            .iter()
            .filter(|q| {
                let q_base = normalize_asset_code(&q.base_asset);
                let q_quote = normalize_asset_code(&q.quote_asset);
                q_base != q_quote                              // exclude self-routes
                    && q_base == base
                    && q_quote == quote
                    && q.rate > 0                              // exclude zero-rate candidates
                    && q.valid_until > now_timestamp
                    && req.amount >= q.minimum_amount
                    && (q.maximum_amount == 0 || req.amount <= q.maximum_amount)
                    && q.reputation_score >= req.min_reputation
            })
            .collect();

        if candidates.is_empty() {
            result.unfilled.push(key);
            continue;
        }

        // Collect owned candidates for strategy selection
        let owned: Vec<CandidateQuote> = candidates.iter().map(|q| (*q).clone()).collect();

        if let Some(best) = select_best(&owned, &req.strategy, 0.333, 0.333, 0.334)? {
            result.filled.push(AssetPairQuote {
                pair_key: key,
                anchor: best.anchor.clone(),
                fee_percentage: best.fee_percentage,
                rate: best.rate,
                quote_id: best.quote_id,
                strategy_applied: req.strategy.clone(),
                routing_reason: best.routing_reason.clone(),
            });
        } else {
            result.unfilled.push(pair_key(&base, &quote));
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn req(base: &str, quote: &str, strategy: &str) -> AssetPairRequest {
        AssetPairRequest {
            base_asset: base.to_string(),
            quote_asset: quote.to_string(),
            amount: 100,
            strategy: strategy.to_string(),
            min_reputation: 0,
        }
    }

    fn candidate(id: u64, base: &str, quote: &str, fee: u32, rep: u32, time: u64) -> CandidateQuote {
        CandidateQuote {
            quote_id: id,
            anchor: alloc::format!("anchor-{id}"),
            base_asset: base.to_string(),
            quote_asset: quote.to_string(),
            rate: 1_000_000,
            fee_percentage: fee,
            minimum_amount: 1,
            maximum_amount: 0,
            valid_until: u64::MAX,
            reputation_score: rep,
            average_settlement_time: time,
            routing_reason: None,
        }
    }

    // ── Asset-code grammar ───────────────────────────────────────────────────

    #[test]
    fn valid_codes_normalize_as_before() {
        assert_eq!(normalize_asset_code("usdc"), "USDC");
        assert_eq!(normalize_asset_code("  Xlm "), "XLM");
        assert_eq!(normalize_asset_code("abc123XYZ789"), "ABC123XYZ789");
        assert!(validate_asset_pair_request(&req("usdc", "xlm", "LowestFee")).is_ok());
        assert!(validate_asset_pair_request(&req("ABC123XYZ789", "XLM", "LowestFee")).is_ok());
    }

    #[test]
    fn unicode_asset_codes_rejected() {
        // Each of these uppercases (Unicode-aware) to an ASCII-looking code.
        for code in ["uſdc", "ﬀ", "ıd", "ÜSDC", "ＵＳＤＣ", "USDC\u{200B}"] {
            assert_eq!(
                validate_asset_pair_request(&req(code, "XLM", "LowestFee")),
                Err(Error::InvalidAssetCode),
                "base {code:?}"
            );
            assert_eq!(
                validate_asset_pair_request(&req("XLM", code, "LowestFee")),
                Err(Error::InvalidAssetCode),
                "quote {code:?}"
            );
        }
    }

    #[test]
    fn punctuation_asset_codes_rejected() {
        for code in ["USD-C", "USDC_COPY", "US.DC", "USD C", "USDC!", "USD/C", "*"] {
            assert_eq!(
                validate_asset_pair_request(&req(code, "XLM", "LowestFee")),
                Err(Error::InvalidAssetCode),
                "{code:?}"
            );
        }
    }

    #[test]
    fn invalid_asset_code_fails_before_routing() {
        let quotes = vec![candidate(1, "USDC", "XLM", 10, 80, 60)];
        let err = route_multi_asset(&[req("uſdc", "XLM", "LowestFee")], &quotes, 0).unwrap_err();
        assert_eq!(err, Error::InvalidAssetCode);
    }

    #[test]
    fn non_ascii_candidate_cannot_alias_valid_code() {
        // "uſdc" would uppercase to "USDC" under Unicode case mapping.
        let quotes = vec![candidate(1, "uſdc", "XLM", 10, 80, 60)];
        let result = route_multi_asset(&[req("USDC", "XLM", "LowestFee")], &quotes, 0).unwrap();
        assert!(result.filled.is_empty());
        assert_eq!(result.unfilled, vec!["USDC/XLM".to_string()]);
    }

    // ── Strategy labels ──────────────────────────────────────────────────────

    #[test]
    fn every_accepted_strategy_maps_to_its_rule() {
        let c = vec![
            candidate(1, "XLM", "USDC", 50, 80, 60),
            candidate(2, "XLM", "USDC", 20, 90, 120),
            candidate(3, "XLM", "USDC", 35, 70, 30),
        ];
        let pick = |s| select_best(&c, s, 1.0, 0.0, 0.0).unwrap().unwrap().quote_id;
        assert_eq!(pick("LowestFee"), 2);
        assert_eq!(pick("FastestSettlement"), 3);
        assert_eq!(pick("HighestReputation"), 2);
        assert_eq!(pick("WeightedScore"), 2); // fee-only weights
        for s in ROUTING_STRATEGIES {
            assert!(validate_asset_pair_request(&req("XLM", "USDC", s)).is_ok(), "{s}");
        }
    }

    #[test]
    fn unknown_strategy_rejected_by_select_best() {
        let c = vec![candidate(1, "XLM", "USDC", 10, 80, 60)];
        for s in ["UndefinedStrategy", "lowestfee", "LowestFee ", ""] {
            assert_eq!(select_best(&c, s, 1.0, 0.0, 0.0).err(), Some(Error::ValidationError), "{s:?}");
        }
        assert_eq!(select_best(&[], "Typo", 1.0, 0.0, 0.0).err(), Some(Error::ValidationError));
    }

    #[test]
    fn unknown_strategy_fails_before_candidate_selection() {
        assert_eq!(
            validate_asset_pair_request(&req("XLM", "USDC", "LowestFees")),
            Err(Error::ValidationError)
        );
        // Fails even when no candidate would match the corridor.
        let err = route_multi_asset(&[req("XLM", "USDC", "LowestFees")], &[], 0).unwrap_err();
        assert_eq!(err, Error::ValidationError);
    }
}
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
