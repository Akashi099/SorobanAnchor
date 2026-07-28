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
// `USDC`, and `Usdc` all resolve to the same corridor.  Callers that pass
// mismatched cases receive an `InvalidAssetCode` error so the mismatch is
// surfaced clearly rather than silently producing no results.
//
// # Invalid combinations
//
// `validate_asset_pair_request` returns `InvalidAssetPair` when:
//   - `base_asset == quote_asset` (circular corridor)
//   - either asset code is empty or exceeds 12 characters
//   - `amount == 0`

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

/// Validate a single `AssetPairRequest`.  Returns `Err(Error::InvalidAssetPair)`
/// when the request is malformed.
pub fn validate_asset_pair_request(req: &AssetPairRequest) -> Result<(), Error> {
    let base = normalize_asset_code(&req.base_asset);
    let quote = normalize_asset_code(&req.quote_asset);

    if base.is_empty() || base.len() > 12 {
        return Err(Error::InvalidAssetCode);
    }
    if quote.is_empty() || quote.len() > 12 {
        return Err(Error::InvalidAssetCode);
    }
    if base == quote {
        return Err(Error::InvalidAssetPair);
    }
    if req.amount == 0 {
        return Err(Error::InvalidAmount);
    }
    Ok(())
}

/// Normalise an asset code to uppercase, trimming whitespace.
pub fn normalize_asset_code(code: &str) -> String {
    code.trim().to_uppercase()
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
/// Returns `None` when the candidate list is empty.
pub fn select_best<'a>(
    candidates: &'a [CandidateQuote],
    strategy: &str,
    fee_weight: f32,
    speed_weight: f32,
    reputation_weight: f32,
) -> Option<&'a CandidateQuote> {
    if candidates.is_empty() {
        return None;
    }

    match strategy {
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
            let max_fee: f32 = candidates
                .iter()
                .map(|q| q.fee_percentage as f32)
                .fold(0.0_f32, f32::max);
            let max_time: f32 = candidates
                .iter()
                .map(|q| q.average_settlement_time as f32)
                .fold(0.0_f32, f32::max);
            let max_rep: f32 = candidates
                .iter()
                .map(|q| q.reputation_score as f32)
                .fold(0.0_f32, f32::max);

            candidates.iter().max_by(|a, b| {
                let score_a = weighted_score(a, fee_weight, speed_weight, reputation_weight, max_fee, max_time, max_rep);
                let score_b = weighted_score(b, fee_weight, speed_weight, reputation_weight, max_fee, max_time, max_rep);
                score_a.partial_cmp(&score_b).unwrap_or(core::cmp::Ordering::Equal)
            })
        }

        // Unknown strategy — fall back to lowest fee
        _ => candidates.iter().min_by_key(|q| q.fee_percentage),
    }
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
    let mut result = MultiAssetRoutingResult::default();

    for req in requests {
        validate_asset_pair_request(req)?;

        let base = normalize_asset_code(&req.base_asset);
        let quote = normalize_asset_code(&req.quote_asset);
        let key = pair_key(&base, &quote);

        // Filter candidates for this pair
        let candidates: Vec<&CandidateQuote> = all_quotes
            .iter()
            .filter(|q| {
                normalize_asset_code(&q.base_asset) == base
                    && normalize_asset_code(&q.quote_asset) == quote
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

        if let Some(best) = select_best(&owned, &req.strategy, 0.333, 0.333, 0.334) {
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
