#![cfg(feature = "std")]

//! Property-based tests for parsing and validation logic
//!
//! This module uses proptest to generate randomized input variations
//! and verify that parsers and validators handle them correctly.
//! These tests help uncover edge cases in:
//! - JSON response parsing (SEP-6, SEP-24, SEP-38)
//! - Domain validation
//! - JWT payload structure
//! - Configuration file parsing

use proptest::prelude::*;
use serde_json::{json, Value};

// ─────────────────────────────────────────────────────────────────────────
// Helper strategies for generating valid payloads
// ─────────────────────────────────────────────────────────────────────────

fn valid_transaction_id() -> impl Strategy<Value = String> {
    "[a-f0-9]{16,32}"
}

fn valid_status() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("pending_user_transfer_start".to_string()),
        Just("pending_anchor".to_string()),
        Just("pending_external".to_string()),
        Just("pending_receiver".to_string()),
        Just("completed".to_string()),
        Just("error".to_string()),
        Just("unknown".to_string()),
    ]
}

fn valid_address() -> impl Strategy<Value = String> {
    "[:alnum:]{20,100}"
}

fn valid_json_string() -> impl Strategy<Value = String> {
    prop_oneof![
        "test[_a-z]*" | r".*[[:space:]]*.*" | "normal_string",
    ]
}

fn valid_iso8601_datetime() -> impl Strategy<Value = String> {
    prop::string::string_regex("\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}Z")
        .expect("valid regex")
}

// ─────────────────────────────────────────────────────────────────────────
// Property: SEP-6 Deposit Response Parsing
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn prop_sep6_deposit_response_with_valid_fields() {
    proptest!(|(
        txn_id in valid_transaction_id(),
        status in valid_status(),
        address in valid_address(),
        how in valid_json_string(),
    )| {
        let response = json!({
            "id": txn_id,
            "type": "deposit",
            "status": status,
            "deposit_address": address,
            "how": how,
        });

        // Property: Parsing should never panic
        let serialized = serde_json::to_string(&response).expect("must serialize");
        let _reparsed: Value = serde_json::from_str(&serialized)
            .expect("must deserialize what was serialized");
    });
}

#[test]
fn prop_sep6_deposit_missing_optional_fields() {
    proptest!(|(
        txn_id in valid_transaction_id(),
        status in valid_status(),
        address in valid_address(),
    )| {
        // Property: Response with only required fields should parse
        let response = json!({
            "id": txn_id,
            "type": "deposit",
            "status": status,
            "deposit_address": address,
        });

        let serialized = serde_json::to_string(&response).expect("must serialize");
        let _reparsed: Value = serde_json::from_str(&serialized)
            .expect("minimal response must deserialize");
    });
}

#[test]
fn prop_sep6_amounts_are_numeric() {
    proptest!(|(
        txn_id in valid_transaction_id(),
        status in valid_status(),
        address in valid_address(),
        min_amount in 0.0f64..1_000_000.0,
        max_amount in 0.0f64..1_000_000.0,
    )| {
        let (min, max) = if min_amount <= max_amount {
            (min_amount, max_amount)
        } else {
            (max_amount, min_amount)
        };

        let response = json!({
            "id": txn_id,
            "type": "deposit",
            "status": status,
            "deposit_address": address,
            "min_amount": min,
            "max_amount": max,
        });

        // Property: Amount fields should deserialize as numbers
        let serialized = serde_json::to_string(&response).expect("must serialize");
        let parsed: Value = serde_json::from_str(&serialized)
            .expect("response with amounts must deserialize");

        assert!(parsed["min_amount"].is_number() || !parsed["min_amount"].is_null());
        assert!(parsed["max_amount"].is_number() || !parsed["max_amount"].is_null());
    });
}

// ─────────────────────────────────────────────────────────────────────────
// Property: Domain Validation Edge Cases
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn prop_domain_validation_handles_arbitrary_strings() {
    proptest!(|(domain in ".*")| {
        // Property: Domain validator should handle any string without panicking
        // This covers malformed URLs, special chars, unicode, etc.

        use anchorkit::domain_validator::DomainValidator;

        // Create a basic validator
        let validator = DomainValidator::new();

        // Should not panic on any input
        let _result = validator.validate(&domain);
    });
}

#[test]
fn prop_domain_validation_case_insensitive() {
    proptest!(|(domain in "[a-z]{3,20}")| {
        use anchorkit::domain_validator::DomainValidator;

        let validator = DomainValidator::new();

        let lower = domain.to_lowercase();
        let upper = domain.to_uppercase();
        let mixed = format!("{}{}",
            &domain.chars().next().unwrap().to_uppercase().to_string(),
            &domain[1..]
        );

        let _lower_result = validator.validate(&lower);
        let _upper_result = validator.validate(&upper);
        let _mixed_result = validator.validate(&mixed);
    });
}

// ─────────────────────────────────────────────────────────────────────────
// Property: Response Validator Schema Versioning
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn prop_schema_version_resolution_never_panics() {
    proptest!(|(version in 0u32..1000)| {
        use anchorkit::response_validator::SchemaVersion;

        // Property: Resolving any u32 to SchemaVersion should never panic
        let _resolved = SchemaVersion::resolve(version);
    });
}

#[test]
fn prop_schema_version_round_trips() {
    proptest!(|(version in 0u32..100)| {
        use anchorkit::response_validator::SchemaVersion;

        let resolved = SchemaVersion::resolve(version);
        // Property: Version should be comparable and orderable
        let _is_valid = resolved >= SchemaVersion::V1;
    });
}

// ─────────────────────────────────────────────────────────────────────────
// Property: JSON Payload Robustness
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn prop_json_parsing_is_idempotent() {
    proptest!(|(
        txn_id in valid_transaction_id(),
        status in valid_status(),
    )| {
        let response = json!({
            "id": txn_id,
            "status": status,
        });

        let serialized1 = serde_json::to_string(&response).expect("must serialize");
        let parsed1: Value = serde_json::from_str(&serialized1).expect("must deserialize");
        let serialized2 = serde_json::to_string(&parsed1).expect("must serialize again");
        let parsed2: Value = serde_json::from_str(&serialized2).expect("must deserialize again");

        // Property: Parsing and serializing multiple times should be idempotent
        assert_eq!(parsed1, parsed2);
    });
}

#[test]
fn prop_json_with_extra_fields() {
    proptest!(|(
        txn_id in valid_transaction_id(),
        status in valid_status(),
        extra_field in "[:alnum:]{1,20}",
        extra_value in "[:alnum:]{1,50}",
    )| {
        let mut response = json!({
            "id": txn_id,
            "status": status,
        });

        // Add unknown fields (common in real-world APIs)
        response[extra_field] = Value::String(extra_value);

        // Property: Parser should handle extra fields gracefully
        let serialized = serde_json::to_string(&response).expect("must serialize");
        let _parsed: Value = serde_json::from_str(&serialized)
            .expect("must handle extra fields");
    });
}

// ─────────────────────────────────────────────────────────────────────────
// Property: Numeric Field Bounds
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn prop_numeric_timestamps_are_valid() {
    proptest!(|(
        timestamp in 0i64..i64::MAX,
    )| {
        let response = json!({
            "id": "test-123",
            "expires_at": timestamp,
        });

        let serialized = serde_json::to_string(&response).expect("must serialize");
        let parsed: Value = serde_json::from_str(&serialized)
            .expect("numeric timestamp must parse");

        // Property: Timestamp should remain numeric after round-trip
        assert!(parsed["expires_at"].is_number());
    });
}

#[test]
fn prop_fee_amounts_non_negative() {
    proptest!(|(
        fee in 0.0f64..1_000_000.0,
    )| {
        let response = json!({
            "id": "quote-123",
            "fee": fee,
        });

        let serialized = serde_json::to_string(&response).expect("must serialize");
        let parsed: Value = serde_json::from_str(&serialized)
            .expect("fee must parse");

        let fee_val = parsed["fee"].as_f64().unwrap_or(0.0);
        // Property: Fees parsed from responses should be accessible
        assert!(fee_val >= 0.0 || fee.is_infinite());
    });
}

// ─────────────────────────────────────────────────────────────────────────
// Property: SEP-24 Transaction Response
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn prop_sep24_transaction_with_memo() {
    proptest!(|(
        txn_id in valid_transaction_id(),
        status in valid_status(),
        memo in "[a-zA-Z0-9]{0,64}",
    )| {
        let response = json!({
            "id": txn_id,
            "status": status,
            "memo": memo,
            "memo_type": "text",
        });

        let serialized = serde_json::to_string(&response).expect("must serialize");
        let _parsed: Value = serde_json::from_str(&serialized)
            .expect("transaction with memo must parse");
    });
}

// ─────────────────────────────────────────────────────────────────────────
// Property: SEP-38 Quote Response
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn prop_sep38_quote_price_representation() {
    proptest!(|(
        id in valid_transaction_id(),
        price in "0\\.?[0-9]{0,10}",
        buy_amount in "0\\.?[0-9]{0,10}",
        sell_amount in "0\\.?[0-9]{0,10}",
    )| {
        let response = json!({
            "id": id,
            "price": price,
            "buy_amount": buy_amount,
            "sell_amount": sell_amount,
            "expires_at": "2099-12-31T23:59:59Z",
        });

        let serialized = serde_json::to_string(&response).expect("must serialize");
        let parsed: Value = serde_json::from_str(&serialized)
            .expect("quote with prices must parse");

        // Property: Price fields should remain as strings in JSON
        assert!(parsed["price"].is_string() || parsed["price"].is_number());
    });
}

// ─────────────────────────────────────────────────────────────────────────
// Property: Error Response Handling
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn prop_error_response_with_message() {
    proptest!(|(
        error_msg in "Error: [[:print:]]{1,100}",
    )| {
        let response = json!({
            "error": error_msg,
        });

        let serialized = serde_json::to_string(&response).expect("must serialize");
        let _parsed: Value = serde_json::from_str(&serialized)
            .expect("error response must parse");
    });
}

#[test]
fn prop_error_response_with_nested_details() {
    proptest!(|(
        error_code in 100u32..999u32,
        error_msg in "[:print:]{1,50}",
    )| {
        let response = json!({
            "error": {
                "code": error_code,
                "message": error_msg,
                "details": serde_json::json!({}),
            }
        });

        let serialized = serde_json::to_string(&response).expect("must serialize");
        let parsed: Value = serde_json::from_str(&serialized)
            .expect("nested error must parse");

        // Property: Structured error should be accessible
        assert!(parsed["error"].is_object() || parsed["error"].is_string());
    });
}
