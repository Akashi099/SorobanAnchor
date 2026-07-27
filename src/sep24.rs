//! SEP-24 Interactive Deposit & Withdrawal Service Layer
//!
//! Provides normalized service functions for initiating interactive deposits,
//! interactive withdrawals, and fetching transaction status for SEP-24 flows.

extern crate alloc;
use alloc::string::String;

use crate::domain_validator::validate_anchor_domain;
use crate::errors::{AnchorKitError, ErrorCode};
use crate::errors::normalize_asset_code;
use crate::sep6::TransactionStatus;
use crate::url_normalizer::normalize_url;

/// Raw response from anchor's `/transactions/deposit/interactive` endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawInteractiveDepositResponse {
    pub url: String,
    pub id: String,
}

/// Raw response from anchor's `/transactions/withdraw/interactive` endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawInteractiveWithdrawalResponse {
    pub url: String,
    pub id: String,
}

/// Raw response from anchor's `/transaction` endpoint for SEP-24.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawSep24TransactionResponse {
    pub id: String,
    pub status: String,
    pub more_info_url: Option<String>,
    pub stellar_transaction_id: Option<String>,
    /// Asset code for this transaction (e.g. `"USDC"`). Normalized to uppercase.
    pub asset_code: Option<String>,
}

/// Normalized response for interactive deposit initiation.
///
/// The `url` field is validated and normalized to a canonical HTTPS origin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractiveDepositResponse {
    /// URL to redirect user to for interactive flow (normalized).
    pub url: String,
    /// Unique transaction ID assigned by the anchor.
    pub id: String,
}

/// Normalized response for interactive withdrawal initiation.
///
/// The `url` field is validated and normalized to a canonical HTTPS origin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractiveWithdrawalResponse {
    /// URL to redirect user to for interactive flow (normalized).
    pub url: String,
    /// Unique transaction ID assigned by the anchor.
    pub id: String,
}

/// Normalized response for SEP-24 transaction status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sep24TransactionStatusResponse {
    /// Unique transaction ID.
    pub id: String,
    /// Current status of the transaction.
    pub status: TransactionStatus,
    /// URL with more information about the transaction (SEP-24 specific).
    pub more_info_url: Option<String>,
    /// Stellar transaction ID if available (SEP-24 specific).
    pub stellar_transaction_id: Option<String>,
    /// Normalized (uppercase) asset code, if provided.
    pub asset_code: Option<String>,
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Validates that a SEP-24 interactive flow URL is a well-formed HTTPS URL
/// with a valid, non-empty path, and (optionally) that its origin matches an
/// expected value.
///
/// The input URL is first **normalized** via [`normalize_url`] (lowercases
/// scheme/host, strips default port and bare trailing slash) so that
/// semantically identical variants are treated the same way.
///
/// Normalization is followed by structural validation via
/// [`validate_anchor_domain`], which enforces https-only, no userinfo, no IP
/// literals, and a valid registered domain.
///
/// When `allowed_origin` is `Some`, the scheme + host (+ optional port) of
/// `url` must match it case-insensitively. Any mismatch returns
/// [`ErrorCode::InvalidEndpointFormat`].
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep24::validate_interactive_url;
///
/// // No origin pinning
/// assert!(validate_interactive_url("https://anchor.example.com/dep", None).is_ok());
///
/// // Pinned origin matches
/// assert!(validate_interactive_url(
///     "https://anchor.example.com/dep",
///     Some("https://anchor.example.com"),
/// ).is_ok());
///
/// // Pinned origin mismatch
/// assert!(validate_interactive_url(
///     "https://evil.example.com/dep",
///     Some("https://anchor.example.com"),
/// ).is_err());
/// ```
pub fn validate_interactive_url(url: &str, allowed_origin: Option<&str>) -> Result<(), AnchorKitError> {
    let normalized = normalize_url(url)?;
    validate_anchor_domain(&normalized).map_err(|_| AnchorKitError::invalid_endpoint_format())?;

    if normalized == "https://" {
        return Err(AnchorKitError::invalid_endpoint_format());
    }

    if let Some(origin) = allowed_origin {
        let url_origin = extract_normalized_origin(&normalized);
        let expected_origin = extract_normalized_origin(
            &normalize_url(origin).unwrap_or_else(|_| alloc::format!("{}", origin)),
        );
        if url_origin != expected_origin {
            return Err(AnchorKitError::with_context(
                ErrorCode::InvalidEndpointFormat,
                "interactive URL origin does not match allowed origin",
                &normalized,
            ));
        }
    }

    if has_open_redirect_pattern(&normalized) {
        return Err(AnchorKitError::with_context(
            ErrorCode::InvalidEndpointFormat,
            "interactive URL contains unsafe redirect parameters",
            &normalized,
        ));
    }

    Ok(())
}

/// Normalize a URL then extract `scheme://host` (with optional `:port`).
///
/// Uses [`normalize_url`] to canonicalize the input first, then strips any
/// trailing path, query, or fragment.
fn extract_normalized_origin(normalized_url: &str) -> String {
    let after_scheme = if let Some(rest) = normalized_url.find("://").map(|i| &normalized_url[i + 3..]) {
        rest
    } else {
        return String::new();
    };
    let host_and_port = if let Some(slash) = after_scheme.find('/') {
        &after_scheme[..slash]
    } else {
        after_scheme
    };
    let scheme_end = normalized_url.find("://").unwrap();
    let scheme = &normalized_url[..scheme_end];
    alloc::format!("{}://{}", scheme.to_ascii_lowercase(), host_and_port.to_ascii_lowercase())
}

/// Detect common open-redirect patterns in query parameters.
///
/// Returns `true` when the URL contains a parameter name that looks like a
/// redirect target (`redirect`, `redirect_url`, `callback`, `next`, `return_url`,
/// `return_uri`, `continue`, `dest`, `destination`, `goto`, `target`, `to`)
/// **and** the parameter value is a full URL (contains `://`).
///
/// A parameter value that is a mere relative path or empty is allowed (it is
/// not an open redirect).
fn has_open_redirect_pattern(url: &str) -> bool {
    let param_names = [
        "redirect", "redirect_url", "redirect_uri", "callback", "next",
        "return_url", "return_uri", "continue", "dest", "destination",
        "goto", "target", "to",
    ];

    let query_start = match url.find('?') {
        Some(pos) => pos + 1,
        None => return false,
    };
    let query = &url[query_start..];

    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let val = parts.next().unwrap_or("");

        if param_names.contains(&key) && val.contains("://") {
            return true;
        }
    }
    false
}

/// Validates that a transaction ID is well-formed according to SEP-24 requirements.
///
/// A valid SEP-24 transaction ID must:
/// - Not be empty
/// - Be between 1 and 64 characters in length
/// - Contain only alphanumeric characters, hyphens, and underscores
/// - Not start or end with a hyphen or underscore
/// - Not contain consecutive hyphens or underscores
pub fn validate_transaction_id(id: &str) -> Result<(), AnchorKitError> {
    // Check for empty ID
    if id.is_empty() {
        return Err(AnchorKitError::new(
            ErrorCode::ValidationError,
            "Transaction ID must not be empty",
        ));
    }

    // Check maximum length (64 characters is reasonable for most systems)
    if id.len() > 64 {
        return Err(AnchorKitError::new(
            ErrorCode::ValidationError,
            "Transaction ID is too long (max 64 characters)",
        ));
    }

    // Check for invalid characters
    for c in id.chars() {
        if !c.is_alphanumeric() && c != '-' && c != '_' {
            return Err(AnchorKitError::new(
                ErrorCode::ValidationError,
                "Transaction ID contains invalid characters (only alphanumeric, hyphen, underscore allowed)",
            ));
        }
    }

    // Check that ID doesn't start or end with hyphen or underscore
    if id.starts_with('-') || id.starts_with('_') {
        return Err(AnchorKitError::new(
            ErrorCode::ValidationError,
            "Transaction ID cannot start with hyphen or underscore",
        ));
    }

    if id.ends_with('-') || id.ends_with('_') {
        return Err(AnchorKitError::new(
            ErrorCode::ValidationError,
            "Transaction ID cannot end with hyphen or underscore",
        ));
    }

    // Check for consecutive special characters
    let mut prev_char: Option<char> = None;
    for c in id.chars() {
        if let Some(prev) = prev_char {
            if (prev == '-' && c == '-') || (prev == '_' && c == '_') {
                return Err(AnchorKitError::new(
                    ErrorCode::ValidationError,
                    "Transaction ID cannot contain consecutive hyphens or underscores",
                ));
            }
        }
        prev_char = Some(c);
    }

    // Additional check: ensure there's at least one alphanumeric character
    if !id.chars().any(|c| c.is_alphanumeric()) {
        return Err(AnchorKitError::new(
            ErrorCode::ValidationError,
            "Transaction ID must contain at least one alphanumeric character",
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Service functions
// ---------------------------------------------------------------------------

/// Normalizes the anchor's `/transactions/deposit/interactive` response.
///
/// Validates that both `url` and `id` are non-empty before returning the
/// normalised struct.
///
/// # Arguments
///
/// * `raw` - A [`RawInteractiveDepositResponse`] from the anchor's endpoint.
///
/// # Returns
///
/// A normalised [`InteractiveDepositResponse`] on success.
///
/// # Errors
///
/// Returns [`AnchorKitError`] with code [`ErrorCode::ValidationError`] if
/// `url` or `id` is empty.
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep24::{initiate_interactive_deposit, RawInteractiveDepositResponse};
///
/// let raw = RawInteractiveDepositResponse {
///     url: "https://anchor.example.com/deposit".into(),
///     id: "tx-123".into(),
/// };
/// let resp = initiate_interactive_deposit(raw).unwrap();
/// assert_eq!(resp.url, "https://anchor.example.com/deposit");
/// assert_eq!(resp.id, "tx-123");
/// ```
pub fn initiate_interactive_deposit(
    raw: RawInteractiveDepositResponse,
) -> Result<InteractiveDepositResponse, AnchorKitError> {
    initiate_interactive_deposit_with_origin(raw, None)
}

/// Like [`initiate_interactive_deposit`] but pins the interactive URL to
/// `expected_origin` when provided (e.g. the anchor's
/// `transfer_server_sep0024` value from stellar.toml).
pub fn initiate_interactive_deposit_with_origin(
    raw: RawInteractiveDepositResponse,
    expected_origin: Option<&str>,
) -> Result<InteractiveDepositResponse, AnchorKitError> {
    let normalized_url = normalize_url(&raw.url)?;
    validate_interactive_url(&normalized_url, expected_origin)?;
    validate_transaction_id(&raw.id)?;
    Ok(InteractiveDepositResponse {
        url: normalized_url,
        id: raw.id,
    })
}

/// Normalizes the anchor's `/transactions/withdraw/interactive` response.
///
/// Validates that both `url` and `id` are non-empty before returning the
/// normalised struct.
///
/// # Arguments
///
/// * `raw` - A [`RawInteractiveWithdrawalResponse`] from the anchor's endpoint.
///
/// # Returns
///
/// A normalised [`InteractiveWithdrawalResponse`] on success.
///
/// # Errors
///
/// Returns [`AnchorKitError`] with code [`ErrorCode::ValidationError`] if
/// `url` or `id` is empty.
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep24::{initiate_interactive_withdrawal, RawInteractiveWithdrawalResponse};
///
/// let raw = RawInteractiveWithdrawalResponse {
///     url: "https://anchor.example.com/withdraw".into(),
///     id: "tx-456".into(),
/// };
/// let resp = initiate_interactive_withdrawal(raw).unwrap();
/// assert_eq!(resp.id, "tx-456");
/// ```
pub fn initiate_interactive_withdrawal(
    raw: RawInteractiveWithdrawalResponse,
) -> Result<InteractiveWithdrawalResponse, AnchorKitError> {
    initiate_interactive_withdrawal_with_origin(raw, None)
}

/// Like [`initiate_interactive_withdrawal`] but pins the interactive URL to
/// `expected_origin` when provided.
pub fn initiate_interactive_withdrawal_with_origin(
    raw: RawInteractiveWithdrawalResponse,
    expected_origin: Option<&str>,
) -> Result<InteractiveWithdrawalResponse, AnchorKitError> {
    let normalized_url = normalize_url(&raw.url)?;
    validate_interactive_url(&normalized_url, expected_origin)?;
    validate_transaction_id(&raw.id)?;
    Ok(InteractiveWithdrawalResponse {
        url: normalized_url,
        id: raw.id,
    })
}

/// Normalizes the anchor's `/transaction` response for SEP-24 flows.
///
/// Maps SEP-24 specific fields (`more_info_url`, `stellar_transaction_id`) and
/// normalises the status string via [`TransactionStatus::from_str`].
///
/// # Arguments
///
/// * `raw` - A [`RawSep24TransactionResponse`] from the anchor's `/transaction` endpoint.
///
/// # Returns
///
/// A normalised [`Sep24TransactionStatusResponse`] on success.
///
/// # Errors
///
/// Returns [`AnchorKitError`] with code [`ErrorCode::ValidationError`] if
/// `id` or `status` is empty.
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep24::{fetch_sep24_transaction_status, RawSep24TransactionResponse};
/// use anchorkit::TransactionStatus;
///
/// let raw = RawSep24TransactionResponse {
///     id: "tx-789".into(),
///     status: "completed".into(),
///     more_info_url: Some("https://anchor.example.com/tx/tx-789".into()),
///     stellar_transaction_id: Some("stellar-tx-123".into()),
///     asset_code: None,
/// };
/// let resp = fetch_sep24_transaction_status(raw).unwrap();
/// assert_eq!(resp.status, TransactionStatus::Completed);
/// assert!(resp.more_info_url.is_some());
/// ```
pub fn fetch_sep24_transaction_status(
    raw: RawSep24TransactionResponse,
) -> Result<Sep24TransactionStatusResponse, AnchorKitError> {
    if raw.id.is_empty() {
        return Err(AnchorKitError::new(
            ErrorCode::ValidationError,
            "Missing id field in SEP-24 transaction response",
        ));
    }
    if raw.status.is_empty() {
        return Err(AnchorKitError::new(
            ErrorCode::ValidationError,
            "Missing status field in SEP-24 transaction response",
        ));
    }
    let more_info_url = match raw.more_info_url {
        Some(ref url) => {
            let normalized = normalize_url(url)?;
            validate_interactive_url(&normalized, None)?;
            Some(normalized)
        }
        None => None,
    };
    let asset_code = raw.asset_code.as_deref()
        .map(normalize_asset_code)
        .transpose()?;

    Ok(Sep24TransactionStatusResponse {
        id: raw.id,
        status: TransactionStatus::from_str(&raw.status),
        more_info_url,
        stellar_transaction_id: raw.stellar_transaction_id,
        asset_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_interactive_url_accepts_https() {
        assert!(validate_interactive_url("https://anchor.example.com/deposit", None).is_ok());
    }

    #[test]
    fn test_validate_interactive_url_rejects_http() {
        assert!(validate_interactive_url("http://anchor.example.com/deposit", None).is_err());
    }

    #[test]
    fn test_validate_interactive_url_rejects_relative() {
        assert!(validate_interactive_url("/deposit/interactive", None).is_err());
        assert!(validate_interactive_url("deposit/interactive", None).is_err());
    }

    #[test]
    fn test_validate_interactive_url_rejects_data_uri() {
        assert!(validate_interactive_url("data:text/html,<h1>phish</h1>", None).is_err());
    }

    #[test]
    fn test_validate_interactive_url_rejects_empty() {
        assert!(validate_interactive_url("", None).is_err());
    }

    #[test]
    fn test_validate_interactive_url_rejects_userinfo() {
        assert!(validate_interactive_url("https://user:pass@anchor.example.com/deposit", None).is_err());
        assert!(validate_interactive_url("https://user@anchor.example.com/deposit", None).is_err());
    }

    #[test]
    fn test_validate_interactive_url_rejects_ip_literal() {
        assert!(validate_interactive_url("https://[::1]/deposit", None).is_err());
        assert!(validate_interactive_url("https://[2001:db8::1]/deposit", None).is_err());
    }

    #[test]
    fn test_validate_interactive_url_accepts_valid_with_path_and_query() {
        assert!(validate_interactive_url("https://anchor.example.com/sep24/deposit?asset=USDC", None).is_ok());
    }

    #[test]
    fn test_validate_interactive_url_normalizes_before_checking() {
        assert!(validate_interactive_url("HTTPS://ANCHOR.EXAMPLE.COM/DEPOSIT", None).is_ok());
    }

    #[test]
    fn test_validate_interactive_url_strips_default_port() {
        assert!(validate_interactive_url("https://anchor.example.com:443/deposit", None).is_ok());
    }

    #[test]
    fn test_validate_interactive_url_rejects_open_redirect_redirect() {
        assert!(validate_interactive_url(
            "https://anchor.example.com/auth?redirect=https://evil.com",
            None,
        ).is_err());
    }

    #[test]
    fn test_validate_interactive_url_rejects_open_redirect_callback() {
        assert!(validate_interactive_url(
            "https://anchor.example.com/return?callback=https://phish.com",
            None,
        ).is_err());
    }

    #[test]
    fn test_validate_interactive_url_rejects_open_redirect_next() {
        assert!(validate_interactive_url(
            "https://anchor.example.com/login?next=https://malicious.com",
            None,
        ).is_err());
    }

    #[test]
    fn test_validate_interactive_url_accepts_redirect_path_only() {
        assert!(validate_interactive_url(
            "https://anchor.example.com/auth?redirect=/local/path",
            None,
        ).is_ok());
    }

    #[test]
    fn test_validate_interactive_url_accepts_redirect_empty_value() {
        assert!(validate_interactive_url(
            "https://anchor.example.com/auth?redirect=",
            None,
        ).is_ok());
    }

    #[test]
    fn test_validate_interactive_url_rejects_open_redirect_return_url() {
        assert!(validate_interactive_url(
            "https://anchor.example.com/checkout?return_url=https://evil.org/capture",
            None,
        ).is_err());
    }

    #[test]
    fn test_validate_interactive_url_accepts_safe_query_params() {
        assert!(validate_interactive_url(
            "https://anchor.example.com/deposit?asset=USDC&amount=100",
            None,
        ).is_ok());
    }

    // -----------------------------------------------------------------------
    // validate_transaction_id
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_transaction_id_accepts_valid() {
        assert!(validate_transaction_id("tx-123").is_ok());
        assert!(validate_transaction_id("tx_abc_456").is_ok());
        assert!(validate_transaction_id("ABC123").is_ok());
    }

    #[test]
    fn test_validate_transaction_id_rejects_empty() {
        assert!(validate_transaction_id("").is_err());
    }

    #[test]
    fn test_validate_transaction_id_rejects_invalid_chars() {
        assert!(validate_transaction_id("tx 123").is_err());
        assert!(validate_transaction_id("tx/123").is_err());
        assert!(validate_transaction_id("tx@123").is_err());
    }

    #[test]
    fn test_validate_transaction_id_rejects_consecutive_hyphens() {
        assert!(validate_transaction_id("tx--123").is_err());
    }

    #[test]
    fn test_validate_transaction_id_rejects_consecutive_underscores() {
        assert!(validate_transaction_id("tx__123").is_err());
    }

    #[test]
    fn test_validate_transaction_id_rejects_leading_hyphen() {
        assert!(validate_transaction_id("-tx-123").is_err());
    }

    #[test]
    fn test_validate_transaction_id_rejects_trailing_hyphen() {
        assert!(validate_transaction_id("tx-123-").is_err());
    }

    #[test]
    fn test_validate_transaction_id_rejects_leading_underscore() {
        assert!(validate_transaction_id("_tx-123").is_err());
    }

    #[test]
    fn test_validate_transaction_id_rejects_non_alphanumeric_only() {
        assert!(validate_transaction_id("--").is_err());
        assert!(validate_transaction_id("_-").is_err());
    }

    #[test]
    fn test_validate_transaction_id_max_length() {
        let long_id = "a".repeat(64);
        assert!(validate_transaction_id(&long_id).is_ok());
        let too_long = "a".repeat(65);
        assert!(validate_transaction_id(&too_long).is_err());
    }

    #[test]
    fn test_validate_transaction_id_mixed_special_and_alpha_ok() {
        assert!(validate_transaction_id("tx_abc-123_XYZ").is_ok());
    }

    // -----------------------------------------------------------------------
    // initiate_interactive_deposit
    // -----------------------------------------------------------------------

    #[test]
    fn test_initiate_interactive_deposit_success() {
        let raw = RawInteractiveDepositResponse {
            url: "https://anchor.example.com/deposit".to_string(),
            id: "tx-123".to_string(),
        };
        let result = initiate_interactive_deposit(raw).unwrap();
        assert_eq!(result.url, "https://anchor.example.com/deposit");
        assert_eq!(result.id, "tx-123");
    }

    #[test]
    fn test_initiate_interactive_deposit_rejects_http_url() {
        let raw = RawInteractiveDepositResponse {
            url: "http://anchor.example.com/deposit".to_string(),
            id: "tx-123".to_string(),
        };
        assert!(initiate_interactive_deposit(raw).is_err());
    }

    #[test]
    fn test_initiate_interactive_deposit_rejects_relative_url() {
        let raw = RawInteractiveDepositResponse {
            url: "/deposit/interactive".to_string(),
            id: "tx-123".to_string(),
        };
        assert!(initiate_interactive_deposit(raw).is_err());
    }

    #[test]
    fn test_initiate_interactive_deposit_rejects_data_uri() {
        let raw = RawInteractiveDepositResponse {
            url: "data:text/html,<h1>phish</h1>".to_string(),
            id: "tx-123".to_string(),
        };
        assert!(initiate_interactive_deposit(raw).is_err());
    }

    #[test]
    fn test_initiate_interactive_deposit_missing_url() {
        let raw = RawInteractiveDepositResponse {
            url: "".to_string(),
            id: "tx-123".to_string(),
        };
        assert!(initiate_interactive_deposit(raw).is_err());
    }

    #[test]
    fn test_initiate_interactive_deposit_missing_id() {
        let raw = RawInteractiveDepositResponse {
            url: "https://anchor.example.com/deposit".to_string(),
            id: "".to_string(),
        };
        assert!(initiate_interactive_deposit(raw).is_err());
    }

    #[test]
    fn test_initiate_interactive_deposit_normalizes_url() {
        let raw = RawInteractiveDepositResponse {
            url: "HTTPS://ANCHOR.EXAMPLE.COM:443/Deposit".to_string(),
            id: "tx-123".to_string(),
        };
        let result = initiate_interactive_deposit(raw).unwrap();
        assert_eq!(result.url, "https://anchor.example.com/Deposit");
        assert_eq!(result.id, "tx-123");
    }

    #[test]
    fn test_initiate_interactive_deposit_rejects_open_redirect() {
        let raw = RawInteractiveDepositResponse {
            url: "https://anchor.example.com/deposit?redirect=https://evil.com".to_string(),
            id: "tx-123".to_string(),
        };
        assert!(initiate_interactive_deposit(raw).is_err());
    }

    // -----------------------------------------------------------------------
    // initiate_interactive_withdrawal
    // -----------------------------------------------------------------------

    #[test]
    fn test_initiate_interactive_withdrawal_success() {
        let raw = RawInteractiveWithdrawalResponse {
            url: "https://anchor.example.com/withdraw".to_string(),
            id: "tx-456".to_string(),
        };
        let result = initiate_interactive_withdrawal(raw).unwrap();
        assert_eq!(result.url, "https://anchor.example.com/withdraw");
        assert_eq!(result.id, "tx-456");
    }

    #[test]
    fn test_initiate_interactive_withdrawal_rejects_http_url() {
        let raw = RawInteractiveWithdrawalResponse {
            url: "http://anchor.example.com/withdraw".to_string(),
            id: "tx-456".to_string(),
        };
        assert!(initiate_interactive_withdrawal(raw).is_err());
    }

    #[test]
    fn test_initiate_interactive_withdrawal_rejects_relative_url() {
        let raw = RawInteractiveWithdrawalResponse {
            url: "/withdraw/interactive".to_string(),
            id: "tx-456".to_string(),
        };
        assert!(initiate_interactive_withdrawal(raw).is_err());
    }

    #[test]
    fn test_initiate_interactive_withdrawal_rejects_data_uri() {
        let raw = RawInteractiveWithdrawalResponse {
            url: "data:text/html,<h1>phish</h1>".to_string(),
            id: "tx-456".to_string(),
        };
        assert!(initiate_interactive_withdrawal(raw).is_err());
    }

    #[test]
    fn test_initiate_interactive_withdrawal_missing_url() {
        let raw = RawInteractiveWithdrawalResponse {
            url: "".to_string(),
            id: "tx-456".to_string(),
        };
        assert!(initiate_interactive_withdrawal(raw).is_err());
    }

    #[test]
    fn test_initiate_interactive_withdrawal_missing_id() {
        let raw = RawInteractiveWithdrawalResponse {
            url: "https://anchor.example.com/withdraw".to_string(),
            id: "".to_string(),
        };
        assert!(initiate_interactive_withdrawal(raw).is_err());
    }

    #[test]
    fn test_initiate_interactive_withdrawal_normalizes_url() {
        let raw = RawInteractiveWithdrawalResponse {
            url: "HTTPS://ANCHOR.EXAMPLE.COM:443/Withdraw".to_string(),
            id: "tx-456".to_string(),
        };
        let result = initiate_interactive_withdrawal(raw).unwrap();
        assert_eq!(result.url, "https://anchor.example.com/Withdraw");
    }

    // -----------------------------------------------------------------------
    // fetch_sep24_transaction_status
    // -----------------------------------------------------------------------

    #[test]
    fn test_fetch_sep24_transaction_status_success() {
        let raw = RawSep24TransactionResponse {
            id: "tx-789".to_string(),
            status: "completed".to_string(),
            more_info_url: Some("https://anchor.example.com/tx/tx-789".to_string()),
            stellar_transaction_id: Some("stellar-tx-123".to_string()),
            asset_code: None,
        };
        let result = fetch_sep24_transaction_status(raw).unwrap();
        assert_eq!(result.id, "tx-789");
        assert_eq!(result.status, TransactionStatus::Completed);
        assert_eq!(
            result.more_info_url,
            Some("https://anchor.example.com/tx/tx-789".to_string())
        );
        assert_eq!(
            result.stellar_transaction_id,
            Some("stellar-tx-123".to_string())
        );
    }

    #[test]
    fn test_fetch_sep24_transaction_status_rejects_http_more_info_url() {
        let raw = RawSep24TransactionResponse {
            id: "tx-789".to_string(),
            status: "completed".to_string(),
            more_info_url: Some("http://anchor.example.com/tx/tx-789".to_string()),
            stellar_transaction_id: None,
            asset_code: None,
        };
        assert!(fetch_sep24_transaction_status(raw).is_err());
    }

    #[test]
    fn test_fetch_sep24_transaction_status_rejects_relative_more_info_url() {
        let raw = RawSep24TransactionResponse {
            id: "tx-789".to_string(),
            status: "completed".to_string(),
            more_info_url: Some("/tx/tx-789".to_string()),
            stellar_transaction_id: None,
            asset_code: None,
        };
        assert!(fetch_sep24_transaction_status(raw).is_err());
    }

    #[test]
    fn test_fetch_sep24_transaction_status_none_more_info_url_ok() {
        let raw = RawSep24TransactionResponse {
            id: "tx-789".to_string(),
            status: "completed".to_string(),
            more_info_url: None,
            stellar_transaction_id: None,
            asset_code: None,
        };
        assert!(fetch_sep24_transaction_status(raw).is_ok());
    }

    #[test]
    fn test_fetch_sep24_transaction_status_missing_id() {
        let raw = RawSep24TransactionResponse {
            id: "".to_string(),
            status: "completed".to_string(),
            more_info_url: None,
            stellar_transaction_id: None,
            asset_code: None,
        };
        assert!(fetch_sep24_transaction_status(raw).is_err());
    }

    #[test]
    fn test_fetch_sep24_transaction_status_missing_status() {
        let raw = RawSep24TransactionResponse {
            id: "tx-789".to_string(),
            status: "".to_string(),
            more_info_url: None,
            stellar_transaction_id: None,
            asset_code: None,
        };
        assert!(fetch_sep24_transaction_status(raw).is_err());
    }

    #[test]
    fn test_fetch_sep24_transaction_status_pending() {
        let raw = RawSep24TransactionResponse {
            id: "tx-pending".to_string(),
            status: "pending_user".to_string(),
            more_info_url: None,
            stellar_transaction_id: None,
            asset_code: None,
        };
        let result = fetch_sep24_transaction_status(raw).unwrap();
        assert_eq!(result.status, TransactionStatus::PendingUser);
    }

    // ── Optional field combination tests (#255) ───────────────────────────────

    #[test]
    fn test_sep24_status_pending_stellar_accepted() {
        let raw = RawSep24TransactionResponse {
            id: "tx-stellar".to_string(),
            status: "pending_stellar".to_string(),
            more_info_url: None,
            stellar_transaction_id: None,
            asset_code: None,
        };
        let result = fetch_sep24_transaction_status(raw).unwrap();
        assert_eq!(result.status, TransactionStatus::PendingStellar);
    }

    #[test]
    fn test_sep24_status_waiting_customer_action_accepted() {
        let raw = RawSep24TransactionResponse {
            id: "tx-wca".to_string(),
            status: "waiting_customer_action".to_string(),
            more_info_url: None,
            stellar_transaction_id: None,
            asset_code: None,
        };
        let result = fetch_sep24_transaction_status(raw).unwrap();
        assert_eq!(result.status, TransactionStatus::WaitingCustomerAction);
    }

    #[test]
    fn test_sep24_stellar_tx_id_optional_absent_is_ok() {
        let raw = RawSep24TransactionResponse {
            id: "tx-no-stellar-id".to_string(),
            status: "completed".to_string(),
            more_info_url: None,
            stellar_transaction_id: None,
            asset_code: None,
        };
        let result = fetch_sep24_transaction_status(raw).unwrap();
        assert!(result.stellar_transaction_id.is_none());
    }

    #[test]
    fn test_sep24_stellar_tx_id_optional_present_is_propagated() {
        let raw = RawSep24TransactionResponse {
            id: "tx-has-stellar-id".to_string(),
            status: "completed".to_string(),
            more_info_url: None,
            stellar_transaction_id: Some("stellar-abc-123".to_string()),
            asset_code: None,
        };
        let result = fetch_sep24_transaction_status(raw).unwrap();
        assert_eq!(result.stellar_transaction_id, Some("stellar-abc-123".to_string()));
    }

    #[test]
    fn test_sep24_all_optional_fields_absent_accepted() {
        let raw = RawSep24TransactionResponse {
            id: "tx-minimal".to_string(),
            status: "pending_anchor".to_string(),
            more_info_url: None,
            stellar_transaction_id: None,
            asset_code: None,
        };
        assert!(fetch_sep24_transaction_status(raw).is_ok());
    }

    #[test]
    fn test_sep24_unknown_status_maps_to_error_variant() {
        let raw = RawSep24TransactionResponse {
            id: "tx-unk".to_string(),
            status: "some_future_status".to_string(),
            more_info_url: None,
            stellar_transaction_id: None,
            asset_code: None,
        };
        let result = fetch_sep24_transaction_status(raw).unwrap();
        assert_eq!(result.status, TransactionStatus::Error);
    }
}
