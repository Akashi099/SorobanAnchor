//! Tests for partial quote response handling (#662).
//!
//! Verifies that:
//! - Partial quote payloads are parsed without error even when fields are absent.
//! - Every missing or unparseable field name is recorded in `missing_fields`.
//! - A complete `PartialFirmQuote` can be promoted to a `FirmQuote` via `into_full`.
//! - An incomplete `PartialFirmQuote` returns an error from `into_full`.
//! - Asset codes in partial quotes are normalized to uppercase.
//! - Stale `expires_at` is accepted by `parse_partial_quote` (staleness is the
//!   caller's concern, not the parser's).
//! - Invalid (but present) field values are treated as missing and recorded.

#![cfg(not(feature = "wasm"))]

use anchorkit::sep38::{parse_partial_quote, FirmQuote, PartialFirmQuote, RawPartialFirmQuote};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn full_raw() -> RawPartialFirmQuote {
    RawPartialFirmQuote {
        id: Some("q-full".into()),
        expires_at: Some("9999999999".into()),
        price: Some("0.15".into()),
        sell_amount: Some("1000".into()),
        buy_amount: Some("150".into()),
        sell_asset: Some("XLM".into()),
        buy_asset: Some("USDC".into()),
    }
}

// ── All fields present ────────────────────────────────────────────────────────

#[test]
fn full_raw_produces_complete_partial_quote() {
    let partial = parse_partial_quote(full_raw());

    assert_eq!(partial.id, Some("q-full".into()));
    assert_eq!(partial.expires_at, Some(9_999_999_999u64));
    assert_eq!(partial.price, Some("0.15".into()));
    assert_eq!(partial.sell_amount, Some("1000".into()));
    assert_eq!(partial.buy_amount, Some("150".into()));
    assert_eq!(partial.sell_asset, Some("XLM".into()));
    assert_eq!(partial.buy_asset, Some("USDC".into()));
    assert!(partial.missing_fields.is_empty());
    assert!(partial.is_complete());
}

#[test]
fn complete_partial_quote_promotes_to_full_quote() {
    let full: FirmQuote = parse_partial_quote(full_raw()).into_full().unwrap();

    assert_eq!(full.id, "q-full");
    assert_eq!(full.expires_at, 9_999_999_999u64);
    assert_eq!(full.price, "0.15");
    assert_eq!(full.sell_amount, "1000");
    assert_eq!(full.buy_amount, "150");
    assert_eq!(full.sell_asset, "XLM");
    assert_eq!(full.buy_asset, "USDC");
}

// ── Missing fields are surfaced in missing_fields ────────────────────────────

#[test]
fn missing_id_recorded_in_missing_fields() {
    let mut raw = full_raw();
    raw.id = None;

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"id"));
    assert!(partial.id.is_none());
}

#[test]
fn missing_expires_at_recorded() {
    let mut raw = full_raw();
    raw.expires_at = None;

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"expires_at"));
    assert!(partial.expires_at.is_none());
}

#[test]
fn missing_price_recorded() {
    let mut raw = full_raw();
    raw.price = None;

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"price"));
    assert!(partial.price.is_none());
}

#[test]
fn missing_sell_amount_recorded() {
    let mut raw = full_raw();
    raw.sell_amount = None;

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"sell_amount"));
    assert!(partial.sell_amount.is_none());
}

#[test]
fn missing_buy_amount_recorded() {
    let mut raw = full_raw();
    raw.buy_amount = None;

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"buy_amount"));
    assert!(partial.buy_amount.is_none());
}

#[test]
fn missing_sell_asset_recorded() {
    let mut raw = full_raw();
    raw.sell_asset = None;

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"sell_asset"));
    assert!(partial.sell_asset.is_none());
}

#[test]
fn missing_buy_asset_recorded() {
    let mut raw = full_raw();
    raw.buy_asset = None;

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"buy_asset"));
    assert!(partial.buy_asset.is_none());
}

#[test]
fn all_fields_missing_records_all_seven_names() {
    let raw = RawPartialFirmQuote::default();

    let partial = parse_partial_quote(raw);
    assert_eq!(partial.missing_fields.len(), 7);
    for name in &["id", "expires_at", "price", "sell_amount", "buy_amount", "sell_asset", "buy_asset"] {
        assert!(
            partial.missing_fields.contains(name),
            "expected '{}' in missing_fields",
            name
        );
    }
}

#[test]
fn multiple_missing_fields_all_recorded() {
    let mut raw = full_raw();
    raw.expires_at = None;
    raw.sell_amount = None;

    let partial = parse_partial_quote(raw);
    assert_eq!(partial.missing_fields.len(), 2);
    assert!(partial.missing_fields.contains(&"expires_at"));
    assert!(partial.missing_fields.contains(&"sell_amount"));
    assert!(!partial.is_complete());
}

// ── Invalid present values treated as missing ─────────────────────────────────

#[test]
fn invalid_expires_at_treated_as_missing() {
    let mut raw = full_raw();
    raw.expires_at = Some("not-a-timestamp".into());

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"expires_at"));
    assert!(partial.expires_at.is_none());
}

#[test]
fn zero_expires_at_treated_as_missing() {
    let mut raw = full_raw();
    raw.expires_at = Some("0".into());

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"expires_at"));
}

#[test]
fn invalid_price_treated_as_missing() {
    let mut raw = full_raw();
    raw.price = Some("abc".into());

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"price"));
    assert!(partial.price.is_none());
}

#[test]
fn zero_price_treated_as_missing() {
    let mut raw = full_raw();
    raw.price = Some("0".into());

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"price"));
}

#[test]
fn zero_sell_amount_treated_as_missing() {
    let mut raw = full_raw();
    raw.sell_amount = Some("0.0".into());

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"sell_amount"));
}

#[test]
fn invalid_sell_amount_treated_as_missing() {
    let mut raw = full_raw();
    raw.sell_amount = Some("--bad--".into());

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"sell_amount"));
}

#[test]
fn empty_id_treated_as_missing() {
    let mut raw = full_raw();
    raw.id = Some("".into());

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"id"));
    assert!(partial.id.is_none());
}

#[test]
fn invalid_sell_asset_treated_as_missing() {
    let mut raw = full_raw();
    raw.sell_asset = Some("TOO LONG CODE EXCEEDS LIMIT".into());

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"sell_asset"));
    assert!(partial.sell_asset.is_none());
}

#[test]
fn invalid_buy_asset_treated_as_missing() {
    let mut raw = full_raw();
    raw.buy_asset = Some("BAD CODE!".into());

    let partial = parse_partial_quote(raw);
    assert!(partial.missing_fields.contains(&"buy_asset"));
    assert!(partial.buy_asset.is_none());
}

// ── Asset code normalization ──────────────────────────────────────────────────

#[test]
fn lowercase_asset_codes_normalized_to_uppercase() {
    let mut raw = full_raw();
    raw.sell_asset = Some("xlm".into());
    raw.buy_asset = Some("usdc".into());

    let partial = parse_partial_quote(raw);
    assert_eq!(partial.sell_asset, Some("XLM".into()));
    assert_eq!(partial.buy_asset, Some("USDC".into()));
    assert!(partial.missing_fields.is_empty());
}

// ── Stale expires_at is accepted ─────────────────────────────────────────────

#[test]
fn stale_expires_at_is_accepted_not_recorded_as_missing() {
    // A past timestamp is valid data – it is the caller's job to check freshness.
    let mut raw = full_raw();
    raw.expires_at = Some("1".into()); // epoch + 1 second

    let partial = parse_partial_quote(raw);
    assert!(!partial.missing_fields.contains(&"expires_at"));
    assert_eq!(partial.expires_at, Some(1u64));
}

// ── is_complete and into_full ─────────────────────────────────────────────────

#[test]
fn is_complete_true_when_no_missing_fields() {
    let partial = parse_partial_quote(full_raw());
    assert!(partial.is_complete());
}

#[test]
fn is_complete_false_when_any_field_missing() {
    let mut raw = full_raw();
    raw.price = None;

    let partial = parse_partial_quote(raw);
    assert!(!partial.is_complete());
}

#[test]
fn into_full_returns_error_when_incomplete() {
    let mut raw = full_raw();
    raw.expires_at = None;
    raw.buy_amount = None;

    let partial = parse_partial_quote(raw);
    assert!(!partial.is_complete());
    assert!(partial.into_full().is_err());
}

#[test]
fn into_full_preserves_all_values_from_complete_partial() {
    let partial = parse_partial_quote(full_raw());
    let full = partial.into_full().expect("should promote successfully");

    assert_eq!(full.id, "q-full");
    assert_eq!(full.expires_at, 9_999_999_999u64);
    assert_eq!(full.price, "0.15");
    assert_eq!(full.sell_amount, "1000");
    assert_eq!(full.buy_amount, "150");
    assert_eq!(full.sell_asset, "XLM");
    assert_eq!(full.buy_asset, "USDC");
    // routing_reason is not carried through partial quotes
    assert!(full.routing_reason.is_none());
}

// ── Typical partial-response scenarios ───────────────────────────────────────

#[test]
fn rate_limited_response_with_only_id_and_price() {
    // Simulates an anchor that returns id + price but nothing else (rate-limited)
    let raw = RawPartialFirmQuote {
        id: Some("q-rate-limited".into()),
        expires_at: None,
        price: Some("0.22".into()),
        sell_amount: None,
        buy_amount: None,
        sell_asset: None,
        buy_asset: None,
    };

    let partial = parse_partial_quote(raw);
    assert_eq!(partial.id, Some("q-rate-limited".into()));
    assert_eq!(partial.price, Some("0.22".into()));
    assert_eq!(partial.missing_fields.len(), 5); // expires_at, sell_amount, buy_amount, sell_asset, buy_asset
    assert!(!partial.is_complete());
}

#[test]
fn incomplete_upstream_response_missing_amounts() {
    // Anchor upstream missing amounts but has identity and timing
    let raw = RawPartialFirmQuote {
        id: Some("q-upstream-partial".into()),
        expires_at: Some("9999999999".into()),
        price: Some("0.10".into()),
        sell_amount: None,
        buy_amount: None,
        sell_asset: Some("XLM".into()),
        buy_asset: Some("USDC".into()),
    };

    let partial = parse_partial_quote(raw);
    assert_eq!(partial.missing_fields.len(), 2);
    assert!(partial.missing_fields.contains(&"sell_amount"));
    assert!(partial.missing_fields.contains(&"buy_amount"));
    assert!(!partial.is_complete());
}

#[test]
fn all_present_but_one_field_not_complete() {
    // All 7 fields present in the raw, but sell_asset has invalid code
    let raw = RawPartialFirmQuote {
        id: Some("q-almost".into()),
        expires_at: Some("9999999999".into()),
        price: Some("0.15".into()),
        sell_amount: Some("500".into()),
        buy_amount: Some("75".into()),
        sell_asset: Some("BAD CODE!@#".into()), // invalid
        buy_asset: Some("USDC".into()),
    };

    let partial = parse_partial_quote(raw);
    assert_eq!(partial.missing_fields.len(), 1);
    assert!(partial.missing_fields.contains(&"sell_asset"));
    assert!(!partial.is_complete());
}
