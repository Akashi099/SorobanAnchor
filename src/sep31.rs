//! SEP-31 Direct Payment Service Layer
//!
//! Provides normalized service functions for initiating direct payments
//! across anchors implementing SEP-31.

extern crate alloc;
use alloc::string::String;

use crate::errors::Error;
use crate::errors::normalize_asset_code;
use crate::response_validator::validate_stellar_account_id;

/// Raw fields from an anchor's direct payment initiation response.
pub struct RawSep31PaymentResponse {
    pub id: String,
    pub stellar_account_id: String,
    pub stellar_memo: Option<String>,
    pub stellar_memo_type: Option<String>,
    /// Amount of the sending asset (e.g. `"100.50"`). Validated as a positive decimal.
    pub amount: Option<String>,
    /// Asset code being sent (e.g. `"USDC"`). Normalized to uppercase.
    pub asset_code: Option<String>,
    /// Client-supplied idempotency key for safe retries.
    pub idempotency_key: Option<String>,
}

/// Validated direct payment response from a SEP-31 anchor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sep31PaymentResponse {
    pub id: String,
    pub stellar_account_id: String,
    pub stellar_memo: Option<String>,
    pub stellar_memo_type: Option<String>,
    /// Amount of the sending asset (e.g. `"100.50"`). Validated as a positive decimal.
    pub amount: Option<String>,
    /// Normalized (uppercase) asset code being sent.
    pub asset_code: Option<String>,
    /// Client-supplied idempotency key for safe retries.
    pub idempotency_key: Option<String>,
}

/// Valid SEP-31 memo type strings.
const VALID_MEMO_TYPES: &[&str] = &["text", "id", "hash"];

/// Maximum length for a transaction ID.
const MAX_ID_LENGTH: usize = 64;
/// Maximum length for an idempotency key.
const MAX_IDEMPOTENCY_KEY_LENGTH: usize = 64;
/// Maximum length for a memo value.
const MAX_MEMO_LENGTH: usize = 256;

/// Validate that whenever a memo value is present, a valid memo type is also present.
fn validate_memo_pair(memo: Option<&str>, memo_type: Option<&str>) -> Result<(), Error> {
    if memo.is_some() {
        match memo_type {
            None => return Err(Error::invalid_transaction_intent()),
            Some(mt) if !VALID_MEMO_TYPES.contains(&mt) => {
                return Err(Error::invalid_transaction_intent());
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validate that a memo value, when present, does not exceed the maximum length.
fn validate_memo_length(memo: Option<&str>) -> Result<(), Error> {
    if let Some(m) = memo {
        if m.len() > MAX_MEMO_LENGTH {
            return Err(Error::validation_error(
                &alloc::format!("memo exceeds maximum length of {} characters", MAX_MEMO_LENGTH),
            ));
        }
    }
    Ok(())
}

/// Validate that a string is a syntactically valid positive decimal number.
///
/// Accepts strings like `"100"`, `"100.50"`, `"0.01"`. Rejects empty strings,
/// negative values, multiple dots, and non-digit characters.
fn validate_positive_decimal(s: &str, field: &str) -> Result<(), Error> {
    if s.is_empty() {
        return Err(Error::validation_error(
            &alloc::format!("{} must not be empty", field),
        ));
    }
    let mut has_dot = false;
    let mut has_digit = false;
    for (i, c) in s.chars().enumerate() {
        if c == '.' {
            if has_dot {
                return Err(Error::validation_error(
                    &alloc::format!("{} contains multiple decimal points", field),
                ));
            }
            has_dot = true;
            // A lone "." is not valid
            if s.len() == 1 {
                return Err(Error::validation_error(
                    &alloc::format!("{} is not a valid decimal", field),
                ));
            }
        } else if c.is_ascii_digit() {
            has_digit = true;
        } else if c == '-' && i == 0 {
            return Err(Error::validation_error(
                &alloc::format!("{} must not be negative", field),
            ));
        } else {
            return Err(Error::validation_error(
                &alloc::format!("{} contains invalid character '{}'", field, c),
            ));
        }
    }
    if !has_digit {
        return Err(Error::validation_error(
            &alloc::format!("{} must contain at least one digit", field),
        ));
    }
    Ok(())
}

/// Validate an idempotency key is well-formed.
///
/// A valid key must be non-empty, at most 64 characters, and contain only
/// printable ASCII characters.
fn validate_idempotency_key(key: Option<&str>) -> Result<(), Error> {
    if let Some(k) = key {
        if k.is_empty() {
            return Err(Error::validation_error("idempotency key must not be empty"));
        }
        if k.len() > MAX_IDEMPOTENCY_KEY_LENGTH {
            return Err(Error::validation_error(
                &alloc::format!("idempotency key exceeds maximum length of {} characters", MAX_IDEMPOTENCY_KEY_LENGTH),
            ));
        }
        for c in k.chars() {
            if !c.is_ascii_graphic() && c != ' ' {
                return Err(Error::validation_error(
                    "idempotency key contains non-printable characters",
                ));
            }
        }
    }
    Ok(())
}

/// Normalize a raw SEP-31 direct payment response into a canonical
/// [`Sep31PaymentResponse`].
///
/// Validates and normalizes the following fields:
/// - `id`: Must be non-empty and at most 64 characters.
/// - `stellar_account_id`: Must be a valid Stellar account ID.
/// - `stellar_memo` / `stellar_memo_type`: Must be consistent when present;
///   memo value must not exceed 256 bytes.
/// - `amount`: When present, must be a valid positive decimal.
/// - `asset_code`: When present, is normalized to uppercase.
/// - `idempotency_key`: When present, must be non-empty, non-printable
///   characters rejected, and at most 64 characters.
pub fn initiate_sep31_payment(
    raw: RawSep31PaymentResponse,
) -> Result<Sep31PaymentResponse, Error> {
    if raw.id.is_empty() {
        return Err(Error::invalid_transaction_intent());
    }
    if raw.id.len() > MAX_ID_LENGTH {
        return Err(Error::validation_error(
            &alloc::format!("id exceeds maximum length of {} characters", MAX_ID_LENGTH),
        ));
    }
    validate_stellar_account_id(&raw.stellar_account_id)?;
    validate_memo_pair(
        raw.stellar_memo.as_deref(),
        raw.stellar_memo_type.as_deref(),
    )?;
    validate_memo_length(raw.stellar_memo.as_deref())?;

    let amount = match raw.amount {
        Some(ref a) => {
            validate_positive_decimal(a, "amount")?;
            Some(a.clone())
        }
        None => None,
    };

    let asset_code = raw.asset_code
        .as_deref()
        .map(normalize_asset_code)
        .transpose()?;

    validate_idempotency_key(raw.idempotency_key.as_deref())?;

    Ok(Sep31PaymentResponse {
        id: raw.id,
        stellar_account_id: raw.stellar_account_id,
        stellar_memo: raw.stellar_memo,
        stellar_memo_type: raw.stellar_memo_type,
        amount,
        asset_code,
        idempotency_key: raw.idempotency_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    const VALID_ACCOUNT: &str =
        "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";

    fn raw_payment() -> RawSep31PaymentResponse {
        RawSep31PaymentResponse {
            id: "pay-001".to_string(),
            stellar_account_id: VALID_ACCOUNT.to_string(),
            stellar_memo: None,
            stellar_memo_type: None,
            amount: None,
            asset_code: None,
            idempotency_key: None,
        }
    }

    #[test]
    fn test_initiate_sep31_payment_accepts_valid_response() {
        let resp = initiate_sep31_payment(raw_payment()).unwrap();
        assert_eq!(resp.id, "pay-001");
        assert_eq!(resp.stellar_account_id, VALID_ACCOUNT);
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_empty_id() {
        let mut raw = raw_payment();
        raw.id = String::new();
        assert_eq!(
            initiate_sep31_payment(raw),
            Err(Error::invalid_transaction_intent())
        );
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_id_too_long() {
        let mut raw = raw_payment();
        raw.id = "a".repeat(65);
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_invalid_account_id() {
        let mut raw = raw_payment();
        raw.stellar_account_id = "not-a-valid-account".to_string();
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_memo_without_type() {
        let mut raw = raw_payment();
        raw.stellar_memo = Some("12345".to_string());
        raw.stellar_memo_type = None;
        assert_eq!(
            initiate_sep31_payment(raw),
            Err(Error::invalid_transaction_intent())
        );
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_invalid_memo_type() {
        let mut raw = raw_payment();
        raw.stellar_memo = Some("12345".to_string());
        raw.stellar_memo_type = Some("fax".to_string());
        assert_eq!(
            initiate_sep31_payment(raw),
            Err(Error::invalid_transaction_intent())
        );
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_memo_too_long() {
        let mut raw = raw_payment();
        raw.stellar_memo = Some("x".repeat(257));
        raw.stellar_memo_type = Some("text".to_string());
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_accepts_memo_at_max_length() {
        let mut raw = raw_payment();
        raw.stellar_memo = Some("x".repeat(256));
        raw.stellar_memo_type = Some("text".to_string());
        assert!(initiate_sep31_payment(raw).is_ok());
    }

    #[test]
    fn test_initiate_sep31_payment_valid_memo_types() {
        for mt in &["text", "id", "hash"] {
            let mut raw = raw_payment();
            raw.stellar_memo = Some("test-value".to_string());
            raw.stellar_memo_type = Some(mt.to_string());
            assert!(initiate_sep31_payment(raw).is_ok(), "memo_type '{}' should be accepted", mt);
        }
    }

    #[test]
    fn test_initiate_sep31_payment_accepts_amount() {
        for amt in &["100", "100.50", "0.01", "999999999.999999999"] {
            let mut raw = raw_payment();
            raw.amount = Some(amt.to_string());
            let resp = initiate_sep31_payment(raw).unwrap();
            assert_eq!(resp.amount.as_deref(), Some(*amt));
        }
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_negative_amount() {
        let mut raw = raw_payment();
        raw.amount = Some("-50.00".to_string());
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_amount_multiple_dots() {
        let mut raw = raw_payment();
        raw.amount = Some("10.0.0".to_string());
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_amount_empty() {
        let mut raw = raw_payment();
        raw.amount = Some("".to_string());
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_amount_non_numeric() {
        let mut raw = raw_payment();
        raw.amount = Some("abc".to_string());
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_normalizes_asset_code() {
        let mut raw = raw_payment();
        raw.asset_code = Some("usdc".to_string());
        let resp = initiate_sep31_payment(raw).unwrap();
        assert_eq!(resp.asset_code.as_deref(), Some("USDC"));
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_invalid_asset_code() {
        let mut raw = raw_payment();
        raw.asset_code = Some("".to_string());
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_accepts_idempotency_key() {
        let mut raw = raw_payment();
        raw.idempotency_key = Some("unique-key-001".to_string());
        let resp = initiate_sep31_payment(raw).unwrap();
        assert_eq!(resp.idempotency_key.as_deref(), Some("unique-key-001"));
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_empty_idempotency_key() {
        let mut raw = raw_payment();
        raw.idempotency_key = Some("".to_string());
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_idempotency_key_too_long() {
        let mut raw = raw_payment();
        raw.idempotency_key = Some("k".repeat(65));
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_accepts_idempotency_key_at_max_length() {
        let mut raw = raw_payment();
        raw.idempotency_key = Some("k".repeat(64));
        assert!(initiate_sep31_payment(raw).is_ok());
    }

    #[test]
    fn test_initiate_sep31_payment_rejects_idempotency_key_with_non_printable() {
        let mut raw = raw_payment();
        raw.idempotency_key = Some("key\x00with-null".to_string());
        assert!(initiate_sep31_payment(raw).is_err());
    }

    #[test]
    fn test_initiate_sep31_payment_idempotency_key_with_spaces_accepted() {
        let mut raw = raw_payment();
        raw.idempotency_key = Some("key with spaces".to_string());
        assert!(initiate_sep31_payment(raw).is_ok());
    }

    #[test]
    fn test_initiate_sep31_payment_memo_type_id_accepts_digit_memo() {
        let mut raw = raw_payment();
        raw.stellar_memo = Some("12345".to_string());
        raw.stellar_memo_type = Some("id".to_string());
        assert!(initiate_sep31_payment(raw).is_ok());
    }

    #[test]
    fn test_initiate_sep31_payment_all_optional_fields_absent() {
        let raw = raw_payment();
        let resp = initiate_sep31_payment(raw).unwrap();
        assert!(resp.amount.is_none());
        assert!(resp.asset_code.is_none());
        assert!(resp.idempotency_key.is_none());
    }

    #[test]
    fn test_initiate_sep31_payment_accepts_asset_code_native() {
        let mut raw = raw_payment();
        raw.asset_code = Some("native".to_string());
        let resp = initiate_sep31_payment(raw).unwrap();
        assert_eq!(resp.asset_code.as_deref(), Some("native"));
    }
}
