//! Mock fixtures for testing without a live anchor.
//!
//! Enabled by the `mock-only` feature flag. Provides pre-built, valid instances
//! of every public response type so integration tests and CI pipelines can
//! exercise the full parsing and validation pipeline without network access.
//!
//! # Example
//!
//! ```rust
//! # #[cfg(feature = "mock-only")]
//! # {
//! use anchorkit::mock::{mock_deposit_response, mock_interactive_deposit_response};
//! use anchorkit::{initiate_deposit, sep24::initiate_interactive_deposit};
//!
//! // Parse a mock raw deposit response
//! let raw = mock_deposit_response();
//! let deposit = initiate_deposit(raw).expect("mock must be valid");
//! assert_eq!(deposit.transaction_id, "mock-txn-001");
//!
//! // Parse a mock interactive deposit response
//! let raw24 = mock_interactive_deposit_response();
//! let interactive = initiate_interactive_deposit(raw24).expect("mock must be valid");
//! assert!(interactive.url.starts_with("https://"));
//! # }
//! ```

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::format;

use crate::sep6::{RawDepositResponse, RawWithdrawalResponse, RawTransactionResponse};
use crate::sep24::{RawInteractiveDepositResponse, RawInteractiveWithdrawalResponse, RawSep24TransactionResponse};
use crate::sep38::{RawPrice, RawFirmQuote};

// ── Sentinel values ───────────────────────────────────────────────────────────

/// Anchor base URL used in all mock responses.
pub const MOCK_ANCHOR_URL: &str = "https://mock-anchor.example.com";

/// Stellar asset code used in mock responses.
pub const MOCK_ASSET_CODE: &str = "USDC";

/// Transaction ID used in SEP-6 mock responses.
pub const MOCK_TXN_ID: &str = "mock-txn-001";

/// Transaction ID used in SEP-24 mock responses.
pub const MOCK_TXN_ID_24: &str = "mock-txn-24-001";

/// Epoch timestamp used in quote mock responses (2024-01-15 00:00:00 UTC).
pub const MOCK_EXPIRES_AT: u64 = 1_705_276_800;

/// A known-valid Stellar testnet account address (correct base32 checksum).
pub const MOCK_ACCOUNT_ID: &str = "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN";

// ── SEP-6 mocks ───────────────────────────────────────────────────────────────

/// Returns a valid [`RawDepositResponse`] suitable for passing to [`initiate_deposit`].
///
/// [`initiate_deposit`]: crate::initiate_deposit
pub fn mock_deposit_response() -> RawDepositResponse {
    RawDepositResponse {
        transaction_id: MOCK_TXN_ID.to_string(),
        how: "Send USDC to the mock anchor address".to_string(),
        extra_info: Some("Use memo: MOCK001".to_string()),
        min_amount: Some(10),
        max_amount: Some(10_000),
        fee_fixed: Some(1),
        status: Some("pending_external".to_string()),
        clawback_enabled: Some(false),
        stellar_memo: Some("MOCK001".to_string()),
        stellar_memo_type: Some("text".to_string()),
        asset_code: Some(MOCK_ASSET_CODE.to_string()),
    }
}

/// Returns a valid [`RawWithdrawalResponse`] suitable for passing to [`initiate_withdrawal`].
///
/// [`initiate_withdrawal`]: crate::initiate_withdrawal
pub fn mock_withdrawal_response() -> RawWithdrawalResponse {
    RawWithdrawalResponse {
        transaction_id: MOCK_TXN_ID.to_string(),
        account_id: MOCK_ACCOUNT_ID.to_string(),
        memo: Some("MOCK-WITHDRAW".to_string()),
        memo_type: Some("text".to_string()),
        min_amount: Some(5),
        max_amount: Some(5_000),
        fee_fixed: Some(2),
        status: Some("pending_anchor".to_string()),
        asset_code: Some(MOCK_ASSET_CODE.to_string()),
    }
}

/// Returns a valid pending [`RawTransactionResponse`] for SEP-6 status polling.
pub fn mock_transaction_response_pending() -> RawTransactionResponse {
    RawTransactionResponse {
        transaction_id: MOCK_TXN_ID.to_string(),
        kind: Some("deposit".to_string()),
        status: "pending_external".to_string(),
        amount_in: Some(100),
        amount_out: Some(99),
        amount_fee: Some(1),
        message: Some("Waiting for external transfer".to_string()),
    }
}

/// Returns a completed [`RawTransactionResponse`] for SEP-6 status polling.
pub fn mock_transaction_response_completed() -> RawTransactionResponse {
    RawTransactionResponse {
        transaction_id: MOCK_TXN_ID.to_string(),
        kind: Some("deposit".to_string()),
        status: "completed".to_string(),
        amount_in: Some(100),
        amount_out: Some(99),
        amount_fee: Some(1),
        message: Some("Deposit complete".to_string()),
    }
}

// ── SEP-24 mocks ──────────────────────────────────────────────────────────────

/// Returns a valid [`RawInteractiveDepositResponse`] suitable for passing to
/// [`initiate_interactive_deposit`].
///
/// [`initiate_interactive_deposit`]: crate::sep24::initiate_interactive_deposit
pub fn mock_interactive_deposit_response() -> RawInteractiveDepositResponse {
    RawInteractiveDepositResponse {
        url: format!("{MOCK_ANCHOR_URL}/sep24/transactions/deposit/interactive?token=mock_jwt"),
        id: MOCK_TXN_ID_24.to_string(),
    }
}

/// Returns a valid [`RawInteractiveWithdrawalResponse`] suitable for passing to
/// [`initiate_interactive_withdrawal`].
///
/// [`initiate_interactive_withdrawal`]: crate::sep24::initiate_interactive_withdrawal
pub fn mock_interactive_withdrawal_response() -> RawInteractiveWithdrawalResponse {
    RawInteractiveWithdrawalResponse {
        url: format!("{MOCK_ANCHOR_URL}/sep24/transactions/withdraw/interactive?token=mock_jwt"),
        id: "mock-txn-24-withdraw-001".to_string(),
    }
}

/// Returns a pending [`RawSep24TransactionResponse`] for SEP-24 status checking.
pub fn mock_sep24_transaction_pending() -> RawSep24TransactionResponse {
    RawSep24TransactionResponse {
        id: MOCK_TXN_ID_24.to_string(),
        status: "pending_user_transfer_start".to_string(),
        more_info_url: Some(format!("{MOCK_ANCHOR_URL}/sep24/transaction?id={MOCK_TXN_ID_24}")),
        stellar_transaction_id: None,
        asset_code: Some(MOCK_ASSET_CODE.to_string()),
    }
}

/// Returns a completed [`RawSep24TransactionResponse`] for SEP-24 status checking.
pub fn mock_sep24_transaction_completed() -> RawSep24TransactionResponse {
    RawSep24TransactionResponse {
        id: MOCK_TXN_ID_24.to_string(),
        status: "completed".to_string(),
        more_info_url: Some(format!("{MOCK_ANCHOR_URL}/sep24/transaction?id={MOCK_TXN_ID_24}")),
        stellar_transaction_id: Some("mock-stellar-sep24-txn".to_string()),
        asset_code: Some(MOCK_ASSET_CODE.to_string()),
    }
}

// ── SEP-38 mocks ──────────────────────────────────────────────────────────────

/// Returns a valid [`RawPrice`] for SEP-38 price queries.
///
/// Asset codes use the plain uppercase form expected by [`fetch_prices`].
///
/// [`fetch_prices`]: crate::sep38::fetch_prices
pub fn mock_price() -> RawPrice {
    RawPrice {
        buy_asset: MOCK_ASSET_CODE.to_string(),
        sell_asset: "XLM".to_string(),
        price: "1.02".to_string(),
    }
}

/// Returns a valid non-expired [`RawFirmQuote`] for SEP-38 quote requests.
///
/// The `expires_at` is set to a fixed future timestamp. Use
/// [`MOCK_EXPIRES_AT`] when checking expiry logic in tests.
/// Asset codes use the plain uppercase form expected by [`request_firm_quote`].
///
/// [`request_firm_quote`]: crate::sep38::request_firm_quote
pub fn mock_firm_quote() -> RawFirmQuote {
    RawFirmQuote {
        id: "mock-quote-001".to_string(),
        expires_at: MOCK_EXPIRES_AT.to_string(),
        price: "1.02".to_string(),
        sell_amount: "100".to_string(),
        buy_amount: "102".to_string(),
        sell_asset: "XLM".to_string(),
        buy_asset: MOCK_ASSET_CODE.to_string(),
    }
}

// ── Composite helpers ─────────────────────────────────────────────────────────

/// A summary of a mock anchor's supported capabilities, returned by
/// [`mock_anchor_capabilities`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockAnchorCapabilities {
    pub anchor_url: String,
    pub asset_code: String,
    pub supports_sep6: bool,
    pub supports_sep24: bool,
    pub supports_sep38: bool,
}

/// Returns a [`MockAnchorCapabilities`] describing the standard mock anchor.
pub fn mock_anchor_capabilities() -> MockAnchorCapabilities {
    MockAnchorCapabilities {
        anchor_url: MOCK_ANCHOR_URL.to_string(),
        asset_code: MOCK_ASSET_CODE.to_string(),
        supports_sep6: true,
        supports_sep24: true,
        supports_sep38: true,
    }
}

// ── Edge case fixtures (#299) ──────────────────────────────────────────────────

/// Returns a SEP-6 deposit response with minimal fields (edge case).
pub fn mock_deposit_response_minimal() -> RawDepositResponse {
    RawDepositResponse {
        transaction_id: "minimal-txn-001".to_string(),
        how: "Send funds".to_string(),
        extra_info: None,
        min_amount: None,
        max_amount: None,
        fee_fixed: None,
        status: None,
        clawback_enabled: None,
        stellar_memo: None,
        stellar_memo_type: None,
        asset_code: None,
    }
}

/// Returns a SEP-6 deposit response with all optional fields populated.
pub fn mock_deposit_response_full() -> RawDepositResponse {
    RawDepositResponse {
        transaction_id: "full-txn-001".to_string(),
        how: "Send USDC to anchor address".to_string(),
        extra_info: Some("Additional instructions".to_string()),
        min_amount: Some(1),
        max_amount: Some(100_000),
        fee_fixed: Some(5),
        status: Some("pending_external".to_string()),
        clawback_enabled: Some(true),
        stellar_memo: Some("a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3".to_string()),
        stellar_memo_type: Some("hash".to_string()),
        asset_code: Some("USDC".to_string()),
    }
}

/// Returns a SEP-6 withdrawal response with minimal fields.
pub fn mock_withdrawal_response_minimal() -> RawWithdrawalResponse {
    RawWithdrawalResponse {
        transaction_id: "withdraw-min-001".to_string(),
        account_id: MOCK_ACCOUNT_ID.to_string(),
        memo: None,
        memo_type: None,
        min_amount: None,
        max_amount: None,
        fee_fixed: None,
        status: None,
        asset_code: None,
    }
}

/// Returns a SEP-6 withdrawal response with all optional fields.
pub fn mock_withdrawal_response_full() -> RawWithdrawalResponse {
    RawWithdrawalResponse {
        transaction_id: "withdraw-full-001".to_string(),
        account_id: MOCK_ACCOUNT_ID.to_string(),
        memo: Some("WITHDRAWMEMO".to_string()),
        memo_type: Some("text".to_string()),
        min_amount: Some(10),
        max_amount: Some(50_000),
        fee_fixed: Some(10),
        status: Some("pending_anchor".to_string()),
        asset_code: Some("USDC".to_string()),
    }
}

/// Returns a SEP-6 transaction response with failed status.
pub fn mock_transaction_response_failed() -> RawTransactionResponse {
    RawTransactionResponse {
        transaction_id: MOCK_TXN_ID.to_string(),
        kind: Some("deposit".to_string()),
        status: "error".to_string(),
        amount_in: Some(100),
        amount_out: None,
        amount_fee: Some(1),
        message: Some("Transaction failed: invalid account".to_string()),
    }
}

/// Returns a SEP-24 transaction response with minimal fields.
pub fn mock_sep24_transaction_minimal() -> RawSep24TransactionResponse {
    RawSep24TransactionResponse {
        id: "sep24-min-001".to_string(),
        status: "pending_user_transfer_start".to_string(),
        more_info_url: None,
        stellar_transaction_id: None,
        asset_code: None,
    }
}

/// Returns a SEP-24 transaction response with all fields populated.
pub fn mock_sep24_transaction_full() -> RawSep24TransactionResponse {
    RawSep24TransactionResponse {
        id: "sep24-full-001".to_string(),
        status: "completed".to_string(),
        more_info_url: Some(format!("{MOCK_ANCHOR_URL}/sep24/transaction?id=sep24-full-001")),
        stellar_transaction_id: Some("stellar-txn-full-001".to_string()),
        asset_code: Some("USDC".to_string()),
    }
}

/// Returns a SEP-38 firm quote with minimal fields.
pub fn mock_firm_quote_minimal() -> RawFirmQuote {
    RawFirmQuote {
        id: "quote-min-001".to_string(),
        expires_at: MOCK_EXPIRES_AT.to_string(),
        price: "1.0".to_string(),
        sell_amount: "100".to_string(),
        buy_amount: "100".to_string(),
        sell_asset: "XLM".to_string(),
        buy_asset: "USDC".to_string(),
    }
}

/// Returns a SEP-38 firm quote with high precision amounts.
pub fn mock_firm_quote_high_precision() -> RawFirmQuote {
    RawFirmQuote {
        id: "quote-precision-001".to_string(),
        expires_at: MOCK_EXPIRES_AT.to_string(),
        price: "1.123456789".to_string(),
        sell_amount: "1000.123456789".to_string(),
        buy_amount: "1123.456789012".to_string(),
        sell_asset: "XLM".to_string(),
        buy_asset: "USDC".to_string(),
    }
}

/// Returns a SEP-38 price with different asset pair.
pub fn mock_price_alternative() -> RawPrice {
    RawPrice {
        buy_asset: "EUR".to_string(),
        sell_asset: "USD".to_string(),
        price: "0.92".to_string(),
    }
}

// ── SEP-6 additional transaction state fixtures ─────────────────────────────

/// Returns a SEP-6 transaction response with "pending_anchor" status.
pub fn mock_transaction_response_pending_anchor() -> RawTransactionResponse {
    RawTransactionResponse {
        transaction_id: MOCK_TXN_ID.to_string(),
        kind: Some("withdrawal".to_string()),
        status: "pending_anchor".to_string(),
        amount_in: Some(500),
        amount_out: Some(490),
        amount_fee: Some(10),
        message: Some("Anchor is processing your withdrawal".to_string()),
    }
}

/// Returns a SEP-6 transaction response with "pending_user_transfer_complete" status.
pub fn mock_transaction_response_pending_user_transfer_complete() -> RawTransactionResponse {
    RawTransactionResponse {
        transaction_id: MOCK_TXN_ID.to_string(),
        kind: Some("deposit".to_string()),
        status: "pending_user_transfer_complete".to_string(),
        amount_in: Some(200),
        amount_out: None,
        amount_fee: Some(2),
        message: Some("Waiting for user to complete transfer".to_string()),
    }
}

/// Returns a SEP-6 deposit response with "completed" status.
pub fn mock_transaction_response_completed() -> RawTransactionResponse {
    RawTransactionResponse {
        transaction_id: MOCK_TXN_ID.to_string(),
        kind: Some("deposit".to_string()),
        status: "completed".to_string(),
        amount_in: Some(100),
        amount_out: Some(99),
        amount_fee: Some(1),
        message: Some("Transaction complete".to_string()),
    }
}

/// Returns a SEP-6 transaction response with "expired_transaction" status.
pub fn mock_transaction_response_expired() -> RawTransactionResponse {
    RawTransactionResponse {
        transaction_id: "expired-txn-001".to_string(),
        kind: Some("deposit".to_string()),
        status: "expired_transaction".to_string(),
        amount_in: None,
        amount_out: None,
        amount_fee: None,
        message: Some("Transaction expired".to_string()),
    }
}

// ── SEP-6 additional withdrawal response fixtures ──────────────────────────

/// Returns a SEP-6 withdrawal response with large amount limits.
pub fn mock_withdrawal_response_large_limits() -> RawWithdrawalResponse {
    RawWithdrawalResponse {
        transaction_id: "withdraw-large-001".to_string(),
        account_id: MOCK_ACCOUNT_ID.to_string(),
        memo: Some("LARGEWITHDRAW".to_string()),
        memo_type: Some("text".to_string()),
        min_amount: Some(1_000),
        max_amount: Some(1_000_000),
        fee_fixed: Some(50),
        status: Some("pending_user_transfer_start".to_string()),
        asset_code: Some("USDC".to_string()),
    }
}

/// Returns a SEP-6 withdrawal response with "completed" status.
pub fn mock_withdrawal_response_completed() -> RawWithdrawalResponse {
    RawWithdrawalResponse {
        transaction_id: "withdraw-complete-001".to_string(),
        account_id: MOCK_ACCOUNT_ID.to_string(),
        memo: Some("DONE".to_string()),
        memo_type: Some("text".to_string()),
        min_amount: Some(5),
        max_amount: Some(5_000),
        fee_fixed: Some(2),
        status: Some("completed".to_string()),
        asset_code: Some("USDC".to_string()),
    }
}

// ── SEP-24 additional transaction state fixtures ──────────────────────────

/// Returns a SEP-24 transaction response with "pending_anchor" status.
pub fn mock_sep24_transaction_pending_anchor() -> RawSep24TransactionResponse {
    RawSep24TransactionResponse {
        id: "sep24-pending-anchor-001".to_string(),
        status: "pending_anchor".to_string(),
        more_info_url: Some(format!("{MOCK_ANCHOR_URL}/sep24/transaction?id=sep24-pending-anchor-001")),
        stellar_transaction_id: None,
        asset_code: Some(MOCK_ASSET_CODE.to_string()),
    }
}

/// Returns a SEP-24 transaction response with "pending_user_transfer_complete" status.
pub fn mock_sep24_transaction_pending_user_transfer_complete() -> RawSep24TransactionResponse {
    RawSep24TransactionResponse {
        id: "sep24-pending-user-transfer-001".to_string(),
        status: "pending_user_transfer_complete".to_string(),
        more_info_url: Some(format!("{MOCK_ANCHOR_URL}/sep24/transaction?id=sep24-pending-user-transfer-001")),
        stellar_transaction_id: None,
        asset_code: Some(MOCK_ASSET_CODE.to_string()),
    }
}

/// Returns a SEP-24 transaction response with "expired_transaction" status.
pub fn mock_sep24_transaction_expired() -> RawSep24TransactionResponse {
    RawSep24TransactionResponse {
        id: "sep24-expired-001".to_string(),
        status: "expired_transaction".to_string(),
        more_info_url: None,
        stellar_transaction_id: None,
        asset_code: None,
    }
}

/// Returns a SEP-24 transaction response with "refunded" status.
pub fn mock_sep24_transaction_refunded() -> RawSep24TransactionResponse {
    RawSep24TransactionResponse {
        id: "sep24-refunded-001".to_string(),
        status: "refunded".to_string(),
        more_info_url: Some(format!("{MOCK_ANCHOR_URL}/sep24/transaction?id=sep24-refunded-001")),
        stellar_transaction_id: Some("stellar-refund-txn".to_string()),
        asset_code: Some(MOCK_ASSET_CODE.to_string()),
    }
}

// ── SEP-38 additional quote fixtures ──────────────────────────────────────

/// Returns a SEP-38 firm quote with expired timestamp.
pub fn mock_firm_quote_expired() -> RawFirmQuote {
    RawFirmQuote {
        id: "quote-expired-001".to_string(),
        expires_at: "1609459200".to_string(), // 2021-01-01
        price: "1.0".to_string(),
        sell_amount: "100".to_string(),
        buy_amount: "100".to_string(),
        sell_asset: "XLM".to_string(),
        buy_asset: "USDC".to_string(),
    }
}

/// Returns a SEP-38 firm quote with very large amounts.
pub fn mock_firm_quote_large_amounts() -> RawFirmQuote {
    RawFirmQuote {
        id: "quote-large-001".to_string(),
        expires_at: MOCK_EXPIRES_AT.to_string(),
        price: "1.002".to_string(),
        sell_amount: "10000000".to_string(),
        buy_amount: "10020000".to_string(),
        sell_asset: "XLM".to_string(),
        buy_asset: "USDC".to_string(),
    }
}

/// Returns a SEP-38 firm quote with very small amounts.
pub fn mock_firm_quote_small_amounts() -> RawFirmQuote {
    RawFirmQuote {
        id: "quote-small-001".to_string(),
        expires_at: MOCK_EXPIRES_AT.to_string(),
        price: "1.001".to_string(),
        sell_amount: "0.01".to_string(),
        buy_amount: "0.01001".to_string(),
        sell_asset: "XLM".to_string(),
        buy_asset: "USDC".to_string(),
    }
}

/// Returns a SEP-38 price for cryptocurrency pair (not fiat).
pub fn mock_price_crypto_pair() -> RawPrice {
    RawPrice {
        buy_asset: "BTC".to_string(),
        sell_asset: "ETH".to_string(),
        price: "15.5".to_string(),
    }
}

// ── Multi-anchor fixtures (#299) ───────────────────────────────────────────────

/// Returns a SEP-6 deposit response from "Anchor A".
pub fn mock_deposit_response_anchor_a() -> RawDepositResponse {
    RawDepositResponse {
        transaction_id: "anchor-a-txn-001".to_string(),
        how: "Send to Anchor A address".to_string(),
        extra_info: Some("Anchor A specific instructions".to_string()),
        min_amount: Some(50),
        max_amount: Some(50_000),
        fee_fixed: Some(2),
        status: Some("pending_external".to_string()),
        clawback_enabled: Some(false),
        stellar_memo: Some("ANCHORA".to_string()),
        stellar_memo_type: Some("text".to_string()),
        asset_code: Some("USDC".to_string()),
    }
}

/// Returns a SEP-6 deposit response from "Anchor B" with different terms.
pub fn mock_deposit_response_anchor_b() -> RawDepositResponse {
    RawDepositResponse {
        transaction_id: "anchor-b-txn-001".to_string(),
        how: "Send to Anchor B address".to_string(),
        extra_info: Some("Anchor B specific instructions".to_string()),
        min_amount: Some(100),
        max_amount: Some(100_000),
        fee_fixed: Some(3),
        status: Some("pending_external".to_string()),
        clawback_enabled: Some(true),
        stellar_memo: Some("ANCHORB".to_string()),
        stellar_memo_type: Some("text".to_string()),
        asset_code: Some("USDC".to_string()),
    }
}

/// Returns a SEP-38 firm quote from "Anchor A".
pub fn mock_firm_quote_anchor_a() -> RawFirmQuote {
    RawFirmQuote {
        id: "quote-a-001".to_string(),
        expires_at: MOCK_EXPIRES_AT.to_string(),
        price: "1.01".to_string(),
        sell_amount: "1000".to_string(),
        buy_amount: "1010".to_string(),
        sell_asset: "XLM".to_string(),
        buy_asset: "USDC".to_string(),
    }
}

/// Returns a SEP-38 firm quote from "Anchor B" with better rate.
pub fn mock_firm_quote_anchor_b() -> RawFirmQuote {
    RawFirmQuote {
        id: "quote-b-001".to_string(),
        expires_at: MOCK_EXPIRES_AT.to_string(),
        price: "1.005".to_string(),
        sell_amount: "1000".to_string(),
        buy_amount: "1005".to_string(),
        sell_asset: "XLM".to_string(),
        buy_asset: "USDC".to_string(),
    }
}
