//! SEP-38 Anchor RFQ Service Layer
//!
//! Provides normalized service functions for fetching prices and requesting firm quotes
//! across different anchors.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec as AllocVec;

use crate::errors::Error;
use crate::errors::normalize_asset_code;

// ── Normalized response types ────────────────────────────────────────────────

/// Normalized price information from SEP-38 `/prices` endpoint.
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep38::{fetch_prices, RawPrice};
///
/// let raw = RawPrice {
///     buy_asset: "USDC".into(),
///     sell_asset: "XLM".into(),
///     price: "0.15".into(),
/// };
/// let price = fetch_prices(raw).unwrap();
/// assert_eq!(price.buy_asset, "USDC");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Price {
    pub buy_asset: String,
    pub sell_asset: String,
    pub price: String,
}

/// Normalized firm quote from SEP-38 `/quote` endpoint.
///
/// A firm quote is a binding commitment from the anchor to exchange assets at
/// the stated `price` until `expires_at`.
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep38::{request_firm_quote, RawFirmQuote};
///
/// let raw = RawFirmQuote {
///     id: "quote-123".into(),
///     expires_at: "1700000000".into(),
///     price: "0.15".into(),
///     sell_amount: "1000".into(),
///     buy_amount: "150".into(),
///     sell_asset: "xlm".into(),
///     buy_asset: "usdc".into(),
/// };
/// let quote = request_firm_quote(raw, 0).unwrap();
/// assert_eq!(quote.id, "quote-123");
/// assert_eq!(quote.sell_asset, "XLM");
/// assert_eq!(quote.buy_asset, "USDC");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirmQuote {
    pub id: String,
    /// Unix timestamp (seconds) when this quote expires.
    pub expires_at: u64,
    pub price: String,
    pub sell_amount: String,
    pub buy_amount: String,
    /// Normalized (uppercase) asset code being sold.
    pub sell_asset: String,
    /// Normalized (uppercase) asset code being bought.
    pub buy_asset: String,
    /// Optional routing reason or referral code explaining why this quote was
    /// selected (e.g. `"lowest_fee"`, `"referral"`, `"preferred_anchor"`).
    /// `None` when no reason was recorded (#298).
    pub routing_reason: Option<alloc::string::String>,
}

// ── Raw response types (from anchor APIs) ────────────────────────────────────

/// Raw price response from anchor /prices endpoint.
#[derive(Clone, Debug)]
pub struct RawPrice {
    pub buy_asset: String,
    pub sell_asset: String,
    pub price: String,
}

/// Raw quote response from anchor /quote endpoint.
#[derive(Clone, Debug)]
pub struct RawFirmQuote {
    pub id: String,
    /// Unix timestamp as a string (e.g. "1700000000").
    pub expires_at: String,
    pub price: String,
    pub sell_amount: String,
    pub buy_amount: String,
    /// Asset code being sold (e.g. `"XLM"`). Normalized to uppercase.
    pub sell_asset: String,
    /// Asset code being bought (e.g. `"USDC"`). Normalized to uppercase.
    pub buy_asset: String,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Returns `true` if `price_str` is a non-empty, positive decimal string.
fn is_valid_positive_decimal(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Allow optional leading digits, optional single '.', trailing digits
    let mut has_digit = false;
    let mut dot_count = 0u32;
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            has_digit = true;
        } else if ch == '.' {
            dot_count += 1;
            if dot_count > 1 {
                return false;
            }
        } else {
            return false;
        }
    }
    if !has_digit {
        return false;
    }
    // Must be > 0: reject "0", "0.0", "0.00", etc.
    let v: f64 = s.parse().unwrap_or(0.0);
    v > 0.0
}

/// Validates all fields of a raw firm quote.
///
/// Returns `Err(Error::invalid_quote())` if any field is invalid.
/// Returns `Err(Error::stale_quote())` if `expires_at` is not in the future.
fn validate_quote_fields(raw: &RawFirmQuote, current_timestamp: u64) -> Result<u64, Error> {
    if raw.id.is_empty() {
        return Err(Error::invalid_quote());
    }
    let expires_at: u64 = raw.expires_at.parse().map_err(|_| Error::invalid_quote())?;
    if expires_at <= current_timestamp {
        return Err(Error::stale_quote());
    }
    if !is_valid_positive_decimal(&raw.price) {
        return Err(Error::invalid_quote());
    }
    if !is_valid_positive_decimal(&raw.sell_amount) {
        return Err(Error::invalid_quote());
    }
    if !is_valid_positive_decimal(&raw.buy_amount) {
        return Err(Error::invalid_quote());
    }
    Ok(expires_at)
}

// ── Service functions ────────────────────────────────────────────────────────

/// Normalizes a raw `/prices` response from an anchor.
///
/// # Errors
///
/// Returns `Err(Error::invalid_quote())` if `price` is not a positive decimal string
/// or is zero. Returns `Err(Error::invalid_asset_code(...))` if `buy_asset` or
/// `sell_asset` contains invalid characters or exceeds 12 characters.
pub fn fetch_prices(raw: RawPrice) -> Result<Price, Error> {
    if !is_valid_positive_decimal(&raw.price) {
        return Err(Error::invalid_quote());
    }
    Ok(Price {
        buy_asset: normalize_asset_code(&raw.buy_asset)?,
        sell_asset: normalize_asset_code(&raw.sell_asset)?,
        price: raw.price,
    })
}

/// Normalizes a raw `/quote` response from an anchor.
///
/// Validates all fields and checks expiry against `current_timestamp`.
/// Returns `Err(Error::stale_quote())` if the quote has already expired.
/// Returns `Err(Error::invalid_quote())` if any field is malformed or zero.
pub fn request_firm_quote(raw: RawFirmQuote, current_timestamp: u64) -> Result<FirmQuote, Error> {
    let expires_at = validate_quote_fields(&raw, current_timestamp)?;
    Ok(FirmQuote {
        id: raw.id,
        expires_at,
        price: raw.price,
        sell_amount: raw.sell_amount,
        buy_amount: raw.buy_amount,
        sell_asset: normalize_asset_code(&raw.sell_asset)?,
        buy_asset: normalize_asset_code(&raw.buy_asset)?,
        routing_reason: None,
    })
}

/// Checks if a quote has expired based on the provided timestamp.
///
/// Returns `true` if `expires_at <= current_timestamp`.
pub fn is_quote_expired(quote: &FirmQuote, current_timestamp: u64) -> bool {
    quote.expires_at <= current_timestamp
}

// ── Issue #292: QuoteConstraints — firm quote price and volume validation ─────

/// Optional price and volume constraints for validating a SEP-38 firm quote.
///
/// All fields are optional; `None` means "no constraint on this dimension".
#[derive(Clone, Debug)]
pub struct QuoteConstraints {
    pub min_price: Option<f64>,
    pub max_price: Option<f64>,
    pub min_sell_amount: Option<f64>,
    pub max_sell_amount: Option<f64>,
    pub min_buy_amount: Option<f64>,
    pub max_buy_amount: Option<f64>,
}

impl QuoteConstraints {
    /// A constraint set with no restrictions on any field.
    pub fn unconstrained() -> Self {
        QuoteConstraints {
            min_price: None,
            max_price: None,
            min_sell_amount: None,
            max_sell_amount: None,
            min_buy_amount: None,
            max_buy_amount: None,
        }
    }
}

fn parse_amount_f64(s: &str) -> Result<f64, Error> {
    s.parse::<f64>().map_err(|_| Error::invalid_quote())
}

fn check_range(value: f64, min: Option<f64>, max: Option<f64>) -> Result<(), Error> {
    if let Some(lo) = min {
        if value < lo {
            return Err(Error::invalid_quote());
        }
    }
    if let Some(hi) = max {
        if value > hi {
            return Err(Error::invalid_quote());
        }
    }
    Ok(())
}

/// Validate a raw firm quote against expiry, field correctness, and optional
/// price/volume constraints.
///
/// Calls [`request_firm_quote`] first, then applies the constraints. Any
/// out-of-range field causes `Err(Error::invalid_quote())`.
pub fn validate_firm_quote_with_constraints(
    raw: RawFirmQuote,
    current_timestamp: u64,
    constraints: &QuoteConstraints,
) -> Result<FirmQuote, Error> {
    let quote = request_firm_quote(raw, current_timestamp)?;

    let price = parse_amount_f64(&quote.price)?;
    check_range(price, constraints.min_price, constraints.max_price)?;

    let sell = parse_amount_f64(&quote.sell_amount)?;
    check_range(sell, constraints.min_sell_amount, constraints.max_sell_amount)?;

    let buy = parse_amount_f64(&quote.buy_amount)?;
    check_range(buy, constraints.min_buy_amount, constraints.max_buy_amount)?;

    Ok(quote)
}

// ── Issue #293: QuoteCache — off-chain TTL cache with invalidation ────────────

/// A single entry in the off-chain SEP-38 quote cache.
#[derive(Clone, Debug)]
pub struct CachedQuoteEntry {
    pub key: String,
    pub quote: FirmQuote,
    /// Unix timestamp (seconds) when the entry was inserted.
    pub cached_at: u64,
    /// How many seconds after `cached_at` the entry is considered fresh.
    pub ttl_seconds: u64,
}

/// In-memory off-chain cache for SEP-38 firm quotes with TTL-based expiry and
/// explicit invalidation.
///
/// Entries are keyed by an arbitrary `String` (e.g. anchor ID or asset pair).
/// A `get` call returns `None` for both unknown keys and stale (past-TTL) entries.
#[derive(Debug, Default)]
pub struct QuoteCache {
    entries: AllocVec<CachedQuoteEntry>,
}

impl QuoteCache {
    pub fn new() -> Self {
        QuoteCache {
            entries: AllocVec::new(),
        }
    }

    /// Insert or replace the quote stored under `key`.
    pub fn insert(&mut self, key: String, quote: FirmQuote, now: u64, ttl_seconds: u64) {
        let entry = CachedQuoteEntry {
            key: key.clone(),
            quote,
            cached_at: now,
            ttl_seconds,
        };
        if let Some(pos) = self.entries.iter().position(|e| e.key == key) {
            self.entries[pos] = entry;
        } else {
            self.entries.push(entry);
        }
    }

    /// Return the cached quote for `key` if it has not yet expired; `None` otherwise.
    pub fn get(&self, key: &str, now: u64) -> Option<&FirmQuote> {
        self.entries
            .iter()
            .find(|e| e.key == key && now < e.cached_at.saturating_add(e.ttl_seconds))
            .map(|e| &e.quote)
    }

    /// Explicitly remove the entry for `key`. Returns `true` if it existed.
    pub fn invalidate(&mut self, key: &str) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e.key == key) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    /// Remove all entries whose TTL has expired. Returns the number evicted.
    pub fn evict_stale(&mut self, now: u64) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|e| now < e.cached_at.saturating_add(e.ttl_seconds));
        before - self.entries.len()
    }

    /// Total number of entries currently held (including stale ones).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── Issue #294: QuoteComparator — best-of-route aggregator ───────────────────

/// Weights for scoring a firm quote across cost and expiry dimensions.
///
/// A fully normalized comparator has `cost_weight + expiry_weight = 1.0`.
pub struct QuoteComparator {
    /// Weight applied to the cost dimension (lower price → higher score).
    pub cost_weight: f64,
    /// Weight applied to the expiry margin (longer until expiry → higher score).
    pub expiry_weight: f64,
}

impl QuoteComparator {
    pub fn new(cost_weight: f64, expiry_weight: f64) -> Self {
        QuoteComparator {
            cost_weight,
            expiry_weight,
        }
    }

    /// Compute a normalized score in [0.0, 1.0] for a single quote.
    ///
    /// - `cost_score = 1 - price / max_price` (cheaper is better).
    /// - `expiry_score = (expires_at - now) / max_expiry_margin` (more time is better).
    pub fn score(
        &self,
        quote: &FirmQuote,
        now: u64,
        max_price: f64,
        max_expiry_margin: u64,
    ) -> f64 {
        let price: f64 = quote.price.parse().unwrap_or(0.0);
        let cost_score = if max_price > 0.0 {
            (1.0_f64 - price / max_price).max(0.0)
        } else {
            1.0
        };
        let expiry_margin = quote.expires_at.saturating_sub(now);
        let expiry_score = if max_expiry_margin > 0 {
            (expiry_margin as f64 / max_expiry_margin as f64).min(1.0)
        } else {
            0.0
        };
        self.cost_weight * cost_score + self.expiry_weight * expiry_score
    }
}

/// A firm quote paired with its normalized composite score.
#[derive(Clone, Debug)]
pub struct ScoredQuote {
    pub score: f64,
    pub quote: FirmQuote,
}

/// Select the best non-expired quote from `quotes` using `comparator`.
///
/// Expired quotes are excluded before scoring. Returns `None` when `quotes` is
/// empty or every entry has already expired.
pub fn select_best_quote<'a>(
    quotes: &'a [FirmQuote],
    comparator: &QuoteComparator,
    now: u64,
) -> Option<&'a FirmQuote> {
    let active: AllocVec<&FirmQuote> = quotes
        .iter()
        .filter(|q| !is_quote_expired(q, now))
        .collect();

    if active.is_empty() {
        return None;
    }

    let max_price = active
        .iter()
        .filter_map(|q| q.price.parse::<f64>().ok())
        .fold(0.0_f64, f64::max);

    let max_expiry = active
        .iter()
        .map(|q| q.expires_at.saturating_sub(now))
        .max()
        .unwrap_or(0);

    let mut best: Option<(f64, &FirmQuote)> = None;
    for q in active.iter() {
        let score = comparator.score(q, now, max_price, max_expiry);
        match best {
            None => {
                best = Some((score, q));
            }
            Some((best_score, _)) if score > best_score => {
                best = Some((score, q));
            }
            _ => {}
        }
    }

    best.map(|(_, q)| q)
}

// ── Issue #295: AnchorFeeHistory — historical fee and spread estimation ───────

/// A single historical fee and spread observation for an anchor.
#[derive(Clone, Debug)]
pub struct FeeObservation {
    pub fee_bps: u32,
    pub spread_bps: u32,
    /// Unix timestamp (seconds) when this observation was recorded.
    pub observed_at: u64,
}

/// Tracks historical fee and spread observations for a single anchor within a
/// configurable retention window.
///
/// Observations older than `retention_seconds` are evicted automatically when
/// a new one is recorded or when a query method is called.
pub struct AnchorFeeHistory {
    observations: AllocVec<FeeObservation>,
    retention_seconds: u64,
}

impl AnchorFeeHistory {
    pub fn new(retention_seconds: u64) -> Self {
        AnchorFeeHistory {
            observations: AllocVec::new(),
            retention_seconds,
        }
    }

    /// Record a new fee/spread observation, evicting entries outside the window.
    pub fn record(&mut self, fee_bps: u32, spread_bps: u32, now: u64) {
        let cutoff = now.saturating_sub(self.retention_seconds);
        self.observations.retain(|o| o.observed_at >= cutoff);
        self.observations.push(FeeObservation {
            fee_bps,
            spread_bps,
            observed_at: now,
        });
    }

    fn active<'a>(&'a self, now: u64) -> AllocVec<&'a FeeObservation> {
        let cutoff = now.saturating_sub(self.retention_seconds);
        self.observations
            .iter()
            .filter(|o| o.observed_at >= cutoff)
            .collect()
    }

    /// Average fee in basis points over the retention window, or `None` if empty.
    pub fn average_fee_bps(&self, now: u64) -> Option<f64> {
        let obs = self.active(now);
        if obs.is_empty() {
            return None;
        }
        let sum: u64 = obs.iter().map(|o| o.fee_bps as u64).sum();
        Some(sum as f64 / obs.len() as f64)
    }

    /// Average spread in basis points over the retention window, or `None` if empty.
    pub fn average_spread_bps(&self, now: u64) -> Option<f64> {
        let obs = self.active(now);
        if obs.is_empty() {
            return None;
        }
        let sum: u64 = obs.iter().map(|o| o.spread_bps as u64).sum();
        Some(sum as f64 / obs.len() as f64)
    }

    /// Estimated total round-trip cost (fee + spread) in bps, or `None` if empty.
    pub fn estimated_cost_bps(&self, now: u64) -> Option<f64> {
        Some(self.average_fee_bps(now)? + self.average_spread_bps(now)?)
    }

    /// Number of observations within the current retention window.
    pub fn observation_count(&self, now: u64) -> usize {
        self.active(now).len()
    }

    /// Population standard deviation of fee observations in the retention window.
    ///
    /// Returns `None` when fewer than two observations are present (stddev is
    /// undefined for zero samples and trivially zero for one).
    pub fn fee_volatility(&self, now: u64) -> Option<f64> {
        let obs = self.active(now);
        if obs.len() < 2 {
            return None;
        }
        let n = obs.len();
        let mean = obs.iter().map(|o| o.fee_bps as f64).sum::<f64>() / n as f64;
        let variance = obs
            .iter()
            .map(|o| {
                let diff = o.fee_bps as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / n as f64;
        Some(variance.sqrt())
    }

    /// Recency-weighted average fee in basis points.
    ///
    /// Applies exponential decay so that the most recent observation carries
    /// the highest weight. The decay factor is `0.9^age_rank` where `age_rank`
    /// is 0 for the newest observation and increases for older ones.
    /// Returns `None` when there are no observations in the window.
    pub fn recency_weighted_average_fee_bps(&self, now: u64) -> Option<f64> {
        let mut obs = self.active(now);
        if obs.is_empty() {
            return None;
        }
        obs.sort_by_key(|o| o.observed_at);
        const DECAY: f64 = 0.9;
        let mut weighted_sum = 0.0_f64;
        let mut weight_total = 0.0_f64;
        for (rank, o) in obs.iter().rev().enumerate() {
            let weight = DECAY.powi(rank as i32);
            weighted_sum += o.fee_bps as f64 * weight;
            weight_total += weight;
        }
        if weight_total == 0.0 {
            return None;
        }
        Some(weighted_sum / weight_total)
    }
}

// ── CrossAnchorFeeAggregator ──────────────────────────────────────────────────

/// Cross-anchor fee anomaly report produced by [`CrossAnchorFeeAggregator::compute_report`].
#[derive(Clone, Debug)]
pub struct FeeAnomalyReport {
    /// Cluster-wide median fee in basis points.
    pub median_fee_bps: u64,
    /// Anchors whose 7-day average fee exceeds the median by more than
    /// `anomaly_threshold_bps`, listed as `(anchor_id, average_fee_bps)`.
    pub anomalous_anchors: AllocVec<(String, u64)>,
    /// Length of the observation window used for the 7-day average (seconds).
    pub observation_window_seconds: u64,
    /// Per-anchor fee volatility (population stddev in bps) for anchors that
    /// have at least two observations. Listed as `(anchor_id, volatility_bps_x100)`
    /// where the value is `stddev * 100` rounded to the nearest integer.
    pub anchor_volatilities: AllocVec<(String, u64)>,
}

/// Aggregates fee observations across multiple anchors and identifies anomalies.
///
/// An anchor is flagged as anomalous when its 7-day average fee exceeds the
/// cluster-wide median by more than `anomaly_threshold_bps` basis points.
pub struct CrossAnchorFeeAggregator {
    anchors: alloc::collections::BTreeMap<String, AnchorFeeHistory>,
    /// Threshold above the cluster median (in bps) that classifies an anchor as anomalous.
    pub anomaly_threshold_bps: u64,
}

impl CrossAnchorFeeAggregator {
    /// Default anomaly threshold: 150 bps above the cluster median.
    pub const DEFAULT_ANOMALY_THRESHOLD_BPS: u64 = 150;

    /// Observation window for the 7-day average: 7 × 24 × 3600 seconds.
    pub const WINDOW_SECONDS: u64 = 7 * 24 * 3600;

    pub fn new(anomaly_threshold_bps: u64) -> Self {
        CrossAnchorFeeAggregator {
            anchors: alloc::collections::BTreeMap::new(),
            anomaly_threshold_bps,
        }
    }

    /// Record a fee observation for `anchor_id`.
    pub fn insert_observation(&mut self, anchor_id: &str, fee_bps: u32, timestamp: u64) {
        let history = self
            .anchors
            .entry(anchor_id.into())
            .or_insert_with(|| AnchorFeeHistory::new(Self::WINDOW_SECONDS));
        history.record(fee_bps, 0, timestamp);
    }

    /// Compute the [`FeeAnomalyReport`] for the current cluster state.
    ///
    /// Anchors with no observations within the window are excluded from the
    /// median computation and are never flagged as anomalous. The report also
    /// includes per-anchor fee volatility (standard deviation) so callers can
    /// distinguish consistently high-fee anchors from erratically priced ones.
    pub fn compute_report(&self, current_time: u64) -> FeeAnomalyReport {
        self.compute_report_impl(current_time, false)
    }

    /// Like [`compute_report`] but uses recency-weighted averages instead of
    /// simple 7-day averages when ranking anchor fees.
    ///
    /// Recent observations receive exponentially higher weight (`0.9^age_rank`),
    /// making the anomaly detection more sensitive to sudden fee spikes.
    pub fn compute_extended_report(&self, current_time: u64) -> FeeAnomalyReport {
        self.compute_report_impl(current_time, true)
    }

    fn compute_report_impl(&self, current_time: u64, use_recency_weight: bool) -> FeeAnomalyReport {
        let mut averages: AllocVec<(String, u64)> = AllocVec::new();
        let mut anchor_volatilities: AllocVec<(String, u64)> = AllocVec::new();

        for (id, history) in &self.anchors {
            let avg_opt = if use_recency_weight {
                history.recency_weighted_average_fee_bps(current_time)
            } else {
                history.average_fee_bps(current_time)
            };
            if let Some(avg) = avg_opt {
                averages.push((id.clone(), avg as u64));
            }
            if let Some(vol) = history.fee_volatility(current_time) {
                anchor_volatilities.push((id.clone(), (vol * 100.0).round() as u64));
            }
        }

        if averages.is_empty() {
            return FeeAnomalyReport {
                median_fee_bps: 0,
                anomalous_anchors: AllocVec::new(),
                observation_window_seconds: Self::WINDOW_SECONDS,
                anchor_volatilities,
            };
        }

        // Sort by fee to compute median.
        let mut fees: AllocVec<u64> = averages.iter().map(|(_, f)| *f).collect();
        fees.sort_unstable();
        let median = {
            let n = fees.len();
            if n % 2 == 1 {
                fees[n / 2]
            } else {
                fees[n / 2 - 1]
            }
        };

        let threshold = self.anomaly_threshold_bps;
        let anomalous_anchors: AllocVec<(String, u64)> = averages
            .into_iter()
            .filter(|(_, avg)| avg.saturating_sub(median) > threshold)
            .collect();

        FeeAnomalyReport {
            median_fee_bps: median,
            anomalous_anchors,
            observation_window_seconds: Self::WINDOW_SECONDS,
            anchor_volatilities,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn valid_raw(expires_at: &str) -> RawFirmQuote {
        RawFirmQuote {
            id: "quote-123".to_string(),
            expires_at: expires_at.to_string(),
            price: "0.15".to_string(),
            sell_amount: "1000".to_string(),
            buy_amount: "150".to_string(),
            sell_asset: "XLM".to_string(),
            buy_asset: "USDC".to_string(),
        }
    }

    // ── fetch_prices ─────────────────────────────────────────────────────────

    #[test]
    fn test_fetch_prices_valid() {
        let raw = RawPrice {
            buy_asset: "USDC".to_string(),
            sell_asset: "XLM".to_string(),
            price: "0.15".to_string(),
        };
        let result = fetch_prices(raw).unwrap();
        assert_eq!(result.buy_asset, "USDC");
        assert_eq!(result.sell_asset, "XLM");
        assert_eq!(result.price, "0.15");
    }

    #[test]
    fn test_fetch_prices_empty_price_rejected() {
        let raw = RawPrice {
            buy_asset: "USDC".to_string(),
            sell_asset: "XLM".to_string(),
            price: "".to_string(),
        };
        assert!(fetch_prices(raw).is_err());
    }

    #[test]
    fn test_fetch_prices_zero_price_rejected() {
        let raw = RawPrice {
            buy_asset: "USDC".to_string(),
            sell_asset: "XLM".to_string(),
            price: "0.0".to_string(),
        };
        assert!(fetch_prices(raw).is_err());
    }

    #[test]
    fn test_fetch_prices_malformed_price_rejected() {
        let raw = RawPrice {
            buy_asset: "USDC".to_string(),
            sell_asset: "XLM".to_string(),
            price: "abc".to_string(),
        };
        assert!(fetch_prices(raw).is_err());
    }

    // ── request_firm_quote ───────────────────────────────────────────────────

    #[test]
    fn test_request_firm_quote_valid() {
        let raw = valid_raw("2000");
        let result = request_firm_quote(raw, 1000).unwrap();
        assert_eq!(result.id, "quote-123");
        assert_eq!(result.expires_at, 2000u64);
        assert_eq!(result.price, "0.15");
    }

    #[test]
    fn test_expired_quote_rejected() {
        // expires_at=1000, now=2000 → stale
        let raw = valid_raw("1000");
        let err = request_firm_quote(raw, 2000).unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::StaleQuote);
    }

    #[test]
    fn test_quote_at_exact_expiry_rejected() {
        // expires_at == now → stale
        let raw = valid_raw("1500");
        let err = request_firm_quote(raw, 1500).unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::StaleQuote);
    }

    #[test]
    fn test_empty_id_rejected() {
        let mut raw = valid_raw("2000");
        raw.id = "".to_string();
        assert!(request_firm_quote(raw, 1000).is_err());
    }

    #[test]
    fn test_malformed_price_rejected() {
        let mut raw = valid_raw("2000");
        raw.price = "not-a-number".to_string();
        let err = request_firm_quote(raw, 1000).unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidQuote);
    }

    #[test]
    fn test_zero_sell_amount_rejected() {
        let mut raw = valid_raw("2000");
        raw.sell_amount = "0".to_string();
        let err = request_firm_quote(raw, 1000).unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidQuote);
    }

    #[test]
    fn test_zero_buy_amount_rejected() {
        let mut raw = valid_raw("2000");
        raw.buy_amount = "0".to_string();
        let err = request_firm_quote(raw, 1000).unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidQuote);
    }

    #[test]
    fn test_malformed_expires_at_rejected() {
        let mut raw = valid_raw("not-a-timestamp");
        raw.expires_at = "not-a-timestamp".to_string();
        let err = request_firm_quote(raw, 1000).unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidQuote);
    }

    // ── is_quote_expired ─────────────────────────────────────────────────────

    #[test]
    fn test_is_quote_expired_true() {
        let quote = FirmQuote {
            id: "q".to_string(),
            expires_at: 1000,
            price: "0.15".to_string(),
            sell_amount: "1000".to_string(),
            buy_amount: "150".to_string(),
            sell_asset: "XLM".to_string(),
            buy_asset: "USDC".to_string(),
            routing_reason: None,
        };
        assert!(is_quote_expired(&quote, 2000));
    }

    #[test]
    fn test_is_quote_expired_false() {
        let quote = FirmQuote {
            id: "q".to_string(),
            expires_at: 2000,
            price: "0.15".to_string(),
            sell_amount: "1000".to_string(),
            buy_amount: "150".to_string(),
            sell_asset: "XLM".to_string(),
            buy_asset: "USDC".to_string(),
            routing_reason: None,
        };
        assert!(!is_quote_expired(&quote, 1000));
    }

    #[test]
    fn test_is_quote_expired_at_boundary() {
        let quote = FirmQuote {
            id: "q".to_string(),
            expires_at: 1500,
            price: "0.15".to_string(),
            sell_amount: "1000".to_string(),
            buy_amount: "150".to_string(),
            sell_asset: "XLM".to_string(),
            buy_asset: "USDC".to_string(),
            routing_reason: None,
        };
        assert!(is_quote_expired(&quote, 1500));
    }

    // ── asset code normalization ──────────────────────────────────────────────

    #[test]
    fn test_fetch_prices_normalizes_lowercase_codes() {
        let raw = RawPrice {
            buy_asset: "usdc".to_string(),
            sell_asset: "xlm".to_string(),
            price: "0.15".to_string(),
        };
        let result = fetch_prices(raw).unwrap();
        assert_eq!(result.buy_asset, "USDC");
        assert_eq!(result.sell_asset, "XLM");
    }

    #[test]
    fn test_fetch_prices_invalid_buy_asset_rejected() {
        let raw = RawPrice {
            buy_asset: "BAD CODE".to_string(),
            sell_asset: "XLM".to_string(),
            price: "0.15".to_string(),
        };
        let err = fetch_prices(raw).unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidAssetCode);
    }

    #[test]
    fn test_request_firm_quote_normalizes_asset_codes() {
        let mut raw = valid_raw("2000");
        raw.sell_asset = "xlm".to_string();
        raw.buy_asset = "usdc".to_string();
        let result = request_firm_quote(raw, 1000).unwrap();
        assert_eq!(result.sell_asset, "XLM");
        assert_eq!(result.buy_asset, "USDC");
    }

    #[test]
    fn test_request_firm_quote_invalid_sell_asset_rejected() {
        let mut raw = valid_raw("2000");
        raw.sell_asset = "TOOLONGCODE13".to_string();
        let err = request_firm_quote(raw, 1000).unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidAssetCode);
    }

    // ── Test helpers ─────────────────────────────────────────────────────────

    fn make_quote(id: &str, expires_at: u64) -> FirmQuote {
        FirmQuote {
            id: id.to_string(),
            expires_at,
            price: "0.15".to_string(),
            sell_amount: "1000".to_string(),
            buy_amount: "150".to_string(),
            sell_asset: "XLM".to_string(),
            buy_asset: "USDC".to_string(),
            routing_reason: None,
        }
    }

    fn make_quote_with_price(id: &str, expires_at: u64, price: &str) -> FirmQuote {
        FirmQuote {
            id: id.to_string(),
            expires_at,
            price: price.to_string(),
            sell_amount: "1000".to_string(),
            buy_amount: "150".to_string(),
            sell_asset: "XLM".to_string(),
            buy_asset: "USDC".to_string(),
            routing_reason: None,
        }
    }

    fn unconstrained() -> QuoteConstraints {
        QuoteConstraints::unconstrained()
    }

    // ── #292: validate_firm_quote_with_constraints ────────────────────────────

    #[test]
    fn test_constraints_unconstrained_passes() {
        let raw = valid_raw("2000");
        assert!(validate_firm_quote_with_constraints(raw, 1000, &unconstrained()).is_ok());
    }

    #[test]
    fn test_constraints_price_below_min_rejected() {
        let raw = valid_raw("2000");
        let c = QuoteConstraints {
            min_price: Some(1.0),
            ..unconstrained()
        };
        assert!(validate_firm_quote_with_constraints(raw, 1000, &c).is_err());
    }

    #[test]
    fn test_constraints_price_above_max_rejected() {
        let raw = valid_raw("2000");
        let c = QuoteConstraints {
            max_price: Some(0.10),
            ..unconstrained()
        };
        assert!(validate_firm_quote_with_constraints(raw, 1000, &c).is_err());
    }

    #[test]
    fn test_constraints_price_within_range_passes() {
        let raw = valid_raw("2000");
        let c = QuoteConstraints {
            min_price: Some(0.10),
            max_price: Some(0.20),
            ..unconstrained()
        };
        assert!(validate_firm_quote_with_constraints(raw, 1000, &c).is_ok());
    }

    #[test]
    fn test_constraints_sell_amount_too_small_rejected() {
        let raw = valid_raw("2000");
        let c = QuoteConstraints {
            min_sell_amount: Some(2000.0),
            ..unconstrained()
        };
        assert!(validate_firm_quote_with_constraints(raw, 1000, &c).is_err());
    }

    #[test]
    fn test_constraints_sell_amount_too_large_rejected() {
        let raw = valid_raw("2000");
        let c = QuoteConstraints {
            max_sell_amount: Some(500.0),
            ..unconstrained()
        };
        assert!(validate_firm_quote_with_constraints(raw, 1000, &c).is_err());
    }

    #[test]
    fn test_constraints_buy_amount_too_small_rejected() {
        let raw = valid_raw("2000");
        let c = QuoteConstraints {
            min_buy_amount: Some(500.0),
            ..unconstrained()
        };
        assert!(validate_firm_quote_with_constraints(raw, 1000, &c).is_err());
    }

    #[test]
    fn test_constraints_stale_quote_rejected_before_range_check() {
        // expires_at=500, now=1000 → stale, should fail regardless of constraints
        let raw = valid_raw("500");
        let err = validate_firm_quote_with_constraints(raw, 1000, &unconstrained()).unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::StaleQuote);
    }

    // ── #293: QuoteCache ──────────────────────────────────────────────────────

    #[test]
    fn test_cache_hit() {
        let mut cache = QuoteCache::new();
        let q = make_quote("q1", 3000);
        cache.insert("anchor1".to_string(), q.clone(), 1000, 600);
        assert_eq!(cache.get("anchor1", 1500), Some(&q));
    }

    #[test]
    fn test_cache_miss_unknown_key() {
        let cache = QuoteCache::new();
        assert_eq!(cache.get("unknown", 1000), None);
    }

    #[test]
    fn test_cache_stale_entry_not_returned() {
        let mut cache = QuoteCache::new();
        cache.insert("anchor1".to_string(), make_quote("q1", 3000), 1000, 100);
        // now=1101 > cached_at(1000) + ttl(100) = 1100
        assert_eq!(cache.get("anchor1", 1101), None);
    }

    #[test]
    fn test_cache_invalidate_removes_entry() {
        let mut cache = QuoteCache::new();
        cache.insert("anchor1".to_string(), make_quote("q1", 3000), 1000, 600);
        assert!(cache.invalidate("anchor1"));
        assert_eq!(cache.get("anchor1", 1000), None);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_cache_invalidate_missing_key_returns_false() {
        let mut cache = QuoteCache::new();
        assert!(!cache.invalidate("no_such_key"));
    }

    #[test]
    fn test_cache_evict_stale_removes_expired_entries() {
        let mut cache = QuoteCache::new();
        cache.insert("a1".to_string(), make_quote("q1", 3000), 1000, 100);
        cache.insert("a2".to_string(), make_quote("q2", 3000), 1000, 600);
        let evicted = cache.evict_stale(1200);
        assert_eq!(evicted, 1);
        assert_eq!(cache.len(), 1);
        assert!(cache.get("a2", 1200).is_some());
    }

    #[test]
    fn test_cache_replace_existing_key() {
        let mut cache = QuoteCache::new();
        cache.insert("a1".to_string(), make_quote("q1", 3000), 1000, 600);
        let q2 = make_quote("q2", 4000);
        cache.insert("a1".to_string(), q2.clone(), 1100, 600);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get("a1", 1200), Some(&q2));
    }

    // ── #294: select_best_quote ───────────────────────────────────────────────

    #[test]
    fn test_select_best_quote_cheapest_wins_with_cost_weight() {
        let cheap = make_quote_with_price("cheap", 5000, "0.10");
        let expensive = make_quote_with_price("exp", 5000, "0.90");
        let cmp = QuoteComparator::new(1.0, 0.0);
        let quotes = [cheap.clone(), expensive];
        let best = select_best_quote(&quotes, &cmp, 1000).unwrap();
        assert_eq!(best.id, "cheap");
    }

    #[test]
    fn test_select_best_quote_longer_expiry_wins_with_expiry_weight() {
        let soon = make_quote("soon", 1100);
        let later = make_quote("later", 5000);
        let cmp = QuoteComparator::new(0.0, 1.0);
        let quotes = [soon, later.clone()];
        let best = select_best_quote(&quotes, &cmp, 1000).unwrap();
        assert_eq!(best.id, "later");
    }

    #[test]
    fn test_select_best_quote_all_expired_returns_none() {
        let q1 = make_quote("q1", 500);
        let q2 = make_quote("q2", 800);
        let cmp = QuoteComparator::new(0.5, 0.5);
        assert!(select_best_quote(&[q1, q2], &cmp, 1000).is_none());
    }

    #[test]
    fn test_select_best_quote_empty_slice_returns_none() {
        let cmp = QuoteComparator::new(0.5, 0.5);
        assert!(select_best_quote(&[], &cmp, 1000).is_none());
    }

    #[test]
    fn test_select_best_quote_skips_expired_picks_live() {
        let expired = make_quote("expired", 500);
        let live = make_quote("live", 5000);
        let cmp = QuoteComparator::new(0.5, 0.5);
        let quotes = [expired, live.clone()];
        let best = select_best_quote(&quotes, &cmp, 1000).unwrap();
        assert_eq!(best.id, "live");
    }

    #[test]
    fn test_select_best_quote_balanced_weights() {
        // Same expiry, different prices — cheaper should win under 50/50 weights
        let cheap = make_quote_with_price("cheap", 3000, "0.10");
        let expensive = make_quote_with_price("exp", 3000, "0.90");
        let cmp = QuoteComparator::new(0.5, 0.5);
        let quotes = [expensive, cheap.clone()];
        let best = select_best_quote(&quotes, &cmp, 1000).unwrap();
        assert_eq!(best.id, "cheap");
    }

    // ── #295: AnchorFeeHistory ────────────────────────────────────────────────

    #[test]
    fn test_fee_history_average_fee_bps() {
        let mut h = AnchorFeeHistory::new(3600);
        h.record(100, 10, 1000);
        h.record(200, 20, 1500);
        let avg = h.average_fee_bps(2000).unwrap();
        assert!((avg - 150.0).abs() < 1e-6, "expected 150.0, got {}", avg);
    }

    #[test]
    fn test_fee_history_average_spread_bps() {
        let mut h = AnchorFeeHistory::new(3600);
        h.record(100, 10, 1000);
        h.record(100, 20, 1500);
        let avg = h.average_spread_bps(2000).unwrap();
        assert!((avg - 15.0).abs() < 1e-6, "expected 15.0, got {}", avg);
    }

    #[test]
    fn test_fee_history_estimated_cost_bps() {
        let mut h = AnchorFeeHistory::new(3600);
        h.record(100, 50, 1000);
        let cost = h.estimated_cost_bps(2000).unwrap();
        assert!((cost - 150.0).abs() < 1e-6, "expected 150.0, got {}", cost);
    }

    #[test]
    fn test_fee_history_empty_returns_none() {
        let h = AnchorFeeHistory::new(3600);
        assert!(h.average_fee_bps(1000).is_none());
        assert!(h.average_spread_bps(1000).is_none());
        assert!(h.estimated_cost_bps(1000).is_none());
    }

    #[test]
    fn test_fee_history_evicts_observations_outside_retention_window() {
        let mut h = AnchorFeeHistory::new(100);
        h.record(100, 10, 1000);
        // Second observation at 1200; cutoff = 1200 - 100 = 1100, so first is evicted
        h.record(200, 20, 1200);
        let avg = h.average_fee_bps(1200).unwrap();
        assert!((avg - 200.0).abs() < 1e-6, "expected 200.0, got {}", avg);
    }

    #[test]
    fn test_fee_history_observation_count() {
        let mut h = AnchorFeeHistory::new(3600);
        h.record(100, 10, 1000);
        h.record(200, 20, 1500);
        assert_eq!(h.observation_count(2000), 2);
    }

    #[test]
    fn test_fee_history_stale_observation_excluded_from_query() {
        let mut h = AnchorFeeHistory::new(100);
        // Directly push a stale observation (won't be evicted since no record() call clears it)
        h.observations.push(FeeObservation {
            fee_bps: 999,
            spread_bps: 999,
            observed_at: 500,
        });
        h.observations.push(FeeObservation {
            fee_bps: 50,
            spread_bps: 10,
            observed_at: 1950,
        });
        // now=2000, cutoff=1900; observation at 500 is stale, 1950 is active
        let avg = h.average_fee_bps(2000).unwrap();
        assert!((avg - 50.0).abs() < 1e-6, "expected 50.0, got {}", avg);
    }
}
