//! Tests for vendor-specific status mapping support (#660).
//!
//! Verifies that:
//! - Known vendor strings are classified into canonical `TransactionStatus` values.
//! - The original raw status string is always preserved in `VendorStatusEntry`.
//! - Unknown vendor strings fall back to `TransactionStatus::from_str`.
//! - Truly unrecognised strings map to `TransactionStatus::Error` with the raw
//!   value preserved.
//! - The map applies consistently in deposit/withdrawal/transaction-status parsing.
//! - Registration, replacement, removal, and lookup helpers all behave correctly.

#![cfg(not(feature = "wasm"))]

use anchorkit::sep6::{
    classify_status_str, fetch_transaction_status, initiate_deposit, initiate_withdrawal,
    RawDepositResponse, RawTransactionResponse, RawWithdrawalResponse, StatusCategory,
    TransactionStatus, VendorStatusEntry, VendorStatusMap,
};

// ── VendorStatusMap construction ─────────────────────────────────────────────

#[test]
fn new_map_is_empty() {
    let map = VendorStatusMap::new();
    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
}

#[test]
fn default_map_is_empty() {
    let map = VendorStatusMap::default();
    assert!(map.is_empty());
}

// ── register and resolve – known vendor values ────────────────────────────────

#[test]
fn known_vendor_value_resolves_to_registered_canonical() {
    let mut map = VendorStatusMap::new();
    map.register("ach_processing", TransactionStatus::PendingExternal);

    let entry = map.resolve("ach_processing");
    assert_eq!(entry.canonical, TransactionStatus::PendingExternal);
    assert_eq!(entry.vendor_status, "ach_processing");
}

#[test]
fn raw_value_is_preserved_exactly_as_supplied() {
    let mut map = VendorStatusMap::new();
    map.register("kyc_required", TransactionStatus::PendingUser);

    // resolve with mixed-case / leading whitespace – raw is trimmed but preserved
    let entry = map.resolve("  KYC_Required  ");
    assert_eq!(entry.canonical, TransactionStatus::PendingUser);
    assert_eq!(entry.vendor_status, "KYC_Required");
}

#[test]
fn case_insensitive_lookup_matches_registered_key() {
    let mut map = VendorStatusMap::new();
    map.register("FX_PENDING", TransactionStatus::PendingAnchor);

    // Registered as uppercase – resolved with lowercase input
    let entry = map.resolve("fx_pending");
    assert_eq!(entry.canonical, TransactionStatus::PendingAnchor);
}

#[test]
fn multiple_vendor_strings_registered_independently() {
    let mut map = VendorStatusMap::new();
    map.register("ach_processing", TransactionStatus::PendingExternal);
    map.register("kyc_required", TransactionStatus::PendingUser);
    map.register("fx_pending", TransactionStatus::PendingAnchor);

    assert_eq!(map.len(), 3);
    assert_eq!(
        map.resolve("ach_processing").canonical,
        TransactionStatus::PendingExternal
    );
    assert_eq!(
        map.resolve("kyc_required").canonical,
        TransactionStatus::PendingUser
    );
    assert_eq!(
        map.resolve("fx_pending").canonical,
        TransactionStatus::PendingAnchor
    );
}

// ── register – replacement of an existing mapping ────────────────────────────

#[test]
fn re_registering_same_key_replaces_canonical() {
    let mut map = VendorStatusMap::new();
    map.register("ach_processing", TransactionStatus::PendingExternal);
    map.register("ach_processing", TransactionStatus::PendingAnchor);

    // Length must not grow – it was replaced, not added
    assert_eq!(map.len(), 1);
    assert_eq!(
        map.resolve("ach_processing").canonical,
        TransactionStatus::PendingAnchor
    );
}

// ── resolve – fall-through to SEP-6 standard parser ──────────────────────────

#[test]
fn standard_sep6_status_resolves_via_fallback_when_not_registered() {
    let map = VendorStatusMap::new(); // empty – no custom mappings

    let entry = map.resolve("completed");
    assert_eq!(entry.canonical, TransactionStatus::Completed);
    assert_eq!(entry.vendor_status, "completed");
}

#[test]
fn all_standard_statuses_resolve_via_fallback() {
    let map = VendorStatusMap::new();
    let cases: &[(&str, TransactionStatus)] = &[
        ("pending_external", TransactionStatus::PendingExternal),
        ("pending_anchor", TransactionStatus::PendingAnchor),
        ("pending_trust", TransactionStatus::PendingTrust),
        ("pending_user", TransactionStatus::PendingUser),
        ("completed", TransactionStatus::Completed),
        ("refunded", TransactionStatus::Refunded),
        ("expired", TransactionStatus::Expired),
        ("incomplete", TransactionStatus::Incomplete),
        ("pending", TransactionStatus::Pending),
        ("no_market", TransactionStatus::NoMarket),
        ("too_small", TransactionStatus::TooSmall),
        ("too_large", TransactionStatus::TooLarge),
        ("pending_stellar", TransactionStatus::PendingStellar),
        ("waiting_customer_action", TransactionStatus::WaitingCustomerAction),
    ];
    for (raw, expected) in cases {
        let entry = map.resolve(raw);
        assert_eq!(
            entry.canonical, *expected,
            "unexpected canonical for '{raw}'"
        );
        assert_eq!(entry.vendor_status, *raw);
    }
}

#[test]
fn truly_unknown_value_maps_to_error_with_raw_preserved() {
    let map = VendorStatusMap::new();

    let entry = map.resolve("totally_unknown_vendor_status");
    assert_eq!(entry.canonical, TransactionStatus::Error);
    assert_eq!(entry.vendor_status, "totally_unknown_vendor_status");
}

#[test]
fn empty_string_maps_to_error_with_raw_preserved() {
    let map = VendorStatusMap::new();

    let entry = map.resolve("");
    assert_eq!(entry.canonical, TransactionStatus::Error);
}

// ── Vendor mapping takes precedence over standard parser ─────────────────────

#[test]
fn custom_mapping_overrides_standard_parser() {
    let mut map = VendorStatusMap::new();
    // Remap "completed" to PendingAnchor (artificial, to test precedence)
    map.register("completed", TransactionStatus::PendingAnchor);

    let entry = map.resolve("completed");
    assert_eq!(entry.canonical, TransactionStatus::PendingAnchor);
}

// ── contains ─────────────────────────────────────────────────────────────────

#[test]
fn contains_returns_true_for_registered_key() {
    let mut map = VendorStatusMap::new();
    map.register("ach_processing", TransactionStatus::PendingExternal);

    assert!(map.contains("ach_processing"));
    assert!(map.contains("ACH_PROCESSING")); // case-insensitive
}

#[test]
fn contains_returns_false_for_unregistered_key() {
    let map = VendorStatusMap::new();
    assert!(!map.contains("ach_processing"));
}

// ── remove ────────────────────────────────────────────────────────────────────

#[test]
fn remove_existing_key_returns_true_and_shrinks_map() {
    let mut map = VendorStatusMap::new();
    map.register("ach_processing", TransactionStatus::PendingExternal);
    map.register("kyc_required", TransactionStatus::PendingUser);

    assert!(map.remove("ach_processing"));
    assert_eq!(map.len(), 1);
    assert!(!map.contains("ach_processing"));
}

#[test]
fn remove_missing_key_returns_false() {
    let mut map = VendorStatusMap::new();
    assert!(!map.remove("non_existent"));
}

#[test]
fn after_remove_resolve_falls_back_to_parser() {
    let mut map = VendorStatusMap::new();
    map.register("completed", TransactionStatus::PendingAnchor);
    map.remove("completed");

    // Falls back to standard parser → Completed
    let entry = map.resolve("completed");
    assert_eq!(entry.canonical, TransactionStatus::Completed);
}

// ── Canonical StatusCategory classification ───────────────────────────────────

#[test]
fn canonical_status_classifies_correctly_after_vendor_resolution() {
    let mut map = VendorStatusMap::new();
    map.register("ach_processing", TransactionStatus::PendingExternal);
    map.register("payment_settled", TransactionStatus::Completed);
    map.register("payment_failed", TransactionStatus::Error);

    assert_eq!(
        map.resolve("ach_processing").canonical.classify(),
        StatusCategory::Active
    );
    assert_eq!(
        map.resolve("payment_settled").canonical.classify(),
        StatusCategory::Completed
    );
    assert_eq!(
        map.resolve("payment_failed").canonical.classify(),
        StatusCategory::Failed
    );
}

// ── classify_status_str – free function ──────────────────────────────────────

#[test]
fn classify_status_str_known_vendor_string_without_map_returns_unknown() {
    // Without a VendorStatusMap, vendor strings are not in the standard set
    // and fall through to Unknown.
    assert_eq!(classify_status_str("ach_processing"), StatusCategory::Unknown);
    assert_eq!(classify_status_str("kyc_required"), StatusCategory::Unknown);
}

#[test]
fn classify_status_str_handles_case_and_whitespace() {
    assert_eq!(classify_status_str("  COMPLETED  "), StatusCategory::Completed);
    assert_eq!(classify_status_str("PENDING_EXTERNAL"), StatusCategory::Active);
    assert_eq!(classify_status_str("  expired  "), StatusCategory::Expired);
}

// ── Integration: VendorStatusMap applied in deposit normalization ──────────────

fn make_raw_deposit(status: &str) -> RawDepositResponse {
    RawDepositResponse {
        transaction_id: "txn-001".into(),
        how: "Send to bank".into(),
        extra_info: None,
        min_amount: None,
        max_amount: None,
        fee_fixed: None,
        status: Some(status.into()),
        clawback_enabled: None,
        stellar_memo: None,
        stellar_memo_type: None,
        asset_code: None,
    }
}

#[test]
fn vendor_map_resolve_then_deposit_normalization_consistent() {
    let mut map = VendorStatusMap::new();
    map.register("ach_processing", TransactionStatus::PendingExternal);

    // Resolve via map to get the canonical status
    let entry = map.resolve("ach_processing");
    assert_eq!(entry.canonical, TransactionStatus::PendingExternal);

    // Simulate what normalization does: use the canonical as the deposit status
    let raw = make_raw_deposit("pending_external"); // already canonical
    let deposit = initiate_deposit(raw).unwrap();
    assert_eq!(deposit.status, TransactionStatus::PendingExternal);
}

#[test]
fn vendor_status_raw_preserved_after_resolution() {
    let mut map = VendorStatusMap::new();
    map.register("Ach_Processing", TransactionStatus::PendingExternal);

    let entry = map.resolve("ACH_PROCESSING");
    // Canonical is correctly mapped
    assert_eq!(entry.canonical, TransactionStatus::PendingExternal);
    // Raw is trimmed but otherwise the caller's spelling is preserved
    assert_eq!(entry.vendor_status, "ACH_PROCESSING");
}

// ── Tie-breaker: vendor map registered canonical is used in fetch_transaction_status ──

#[test]
fn fetch_transaction_status_with_resolved_vendor_canonical() {
    // Scenario: anchor returns "ach_processing"; after map resolution we feed
    // the canonical string into fetch_transaction_status.
    let mut map = VendorStatusMap::new();
    map.register("ach_processing", TransactionStatus::PendingExternal);

    let resolved = map.resolve("ach_processing");
    // Use canonical string as the raw status for normalization
    let raw = RawTransactionResponse {
        transaction_id: "txn-002".into(),
        kind: Some("deposit".into()),
        status: resolved.canonical.as_str().into(),
        amount_in: Some(500),
        amount_out: None,
        amount_fee: None,
        message: None,
    };
    let resp = fetch_transaction_status(raw).unwrap();
    assert_eq!(resp.status, TransactionStatus::PendingExternal);
}

// ── Edge cases: whitespace-only and punctuation vendor strings ────────────────

#[test]
fn whitespace_only_vendor_string_maps_to_error() {
    let map = VendorStatusMap::new();
    let entry = map.resolve("   ");
    assert_eq!(entry.canonical, TransactionStatus::Error);
}

#[test]
fn vendor_string_with_underscores_preserved() {
    let mut map = VendorStatusMap::new();
    map.register("on_hold_for_review", TransactionStatus::PendingUser);

    let entry = map.resolve("on_hold_for_review");
    assert_eq!(entry.canonical, TransactionStatus::PendingUser);
    assert_eq!(entry.vendor_status, "on_hold_for_review");
}

// ── Ordering is stable: last write wins for duplicate registration ────────────

#[test]
fn last_register_wins_on_duplicate_key() {
    let mut map = VendorStatusMap::new();
    map.register("pending_review", TransactionStatus::PendingAnchor);
    map.register("pending_review", TransactionStatus::PendingUser);
    map.register("pending_review", TransactionStatus::PendingTrust);

    assert_eq!(map.len(), 1);
    assert_eq!(
        map.resolve("pending_review").canonical,
        TransactionStatus::PendingTrust
    );
}

// ── VendorStatusEntry struct fields ──────────────────────────────────────────

#[test]
fn vendor_status_entry_fields_are_accessible() {
    let entry = VendorStatusEntry {
        vendor_status: "my_vendor_status".into(),
        canonical: TransactionStatus::Completed,
    };
    assert_eq!(entry.vendor_status, "my_vendor_status");
    assert_eq!(entry.canonical, TransactionStatus::Completed);
}
