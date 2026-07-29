//! SEP-6 Deposit & Withdrawal Service Layer
//!
//! Provides normalized service functions for initiating deposits, withdrawals,
//! and fetching transaction status across different anchors.

extern crate alloc;
use alloc::string::String;

use crate::errors::Error;
use crate::errors::normalize_asset_code;

// ── Status classification ─────────────────────────────────────────────────────

/// High-level category that a [`TransactionStatus`] belongs to.
///
/// Clients can use this to make decisions without matching on every individual
/// status variant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatusCategory {
    /// The transaction is still being processed (pending, incomplete, waiting).
    Active,
    /// The transaction completed successfully.
    Completed,
    /// The transaction was refunded.
    Refunded,
    /// The transaction expired.
    Expired,
    /// The transaction failed or cannot proceed (error, no_market, too_small,
    /// too_large).
    Failed,
    /// The status is not recognised.
    Unknown,
}

/// Classify a raw status string into a [`StatusCategory`].
///
/// The input is trimmed and lowercased before matching, so minor formatting
/// differences from anchors do not cause misclassification.
///
/// Anchors that return unexpected status strings receive
/// [`StatusCategory::Unknown`] rather than being silently treated as
/// successful.
pub fn classify_status_str(s: &str) -> StatusCategory {
    let normalized = s.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "pending_external"
        | "pending_anchor"
        | "pending_trust"
        | "pending_user"
        | "pending_user_transfer_start"
        | "pending_user_transfer_complete"
        | "pending_stellar"
        | "waiting_customer_action"
        | "incomplete"
        | "pending" => StatusCategory::Active,
        "completed" => StatusCategory::Completed,
        "refunded" => StatusCategory::Refunded,
        "expired" => StatusCategory::Expired,
        "no_market" | "too_small" | "too_large" | "error" => StatusCategory::Failed,
        _ => StatusCategory::Unknown,
    }
}

// ── Vendor-specific status mapping (#660) ────────────────────────────────────

/// A single entry in a vendor-specific status map.
///
/// Maps a raw vendor string to a canonical [`TransactionStatus`]. The original
/// raw value is always preserved alongside the canonical classification so
/// callers never lose vendor detail.
#[derive(Clone, Debug)]
pub struct VendorStatusEntry {
    /// The raw vendor string exactly as supplied in the anchor's response.
    pub vendor_status: String,
    /// The canonical SEP-6 status this vendor string maps to.
    pub canonical: TransactionStatus,
}

/// A collection of vendor-specific status mappings for a single anchor.
///
/// Anchors may return proprietary status strings (e.g. `"ach_processing"`,
/// `"kyc_required"`, `"fx_pending"`) that have no direct SEP-6 equivalent.
/// A `VendorStatusMap` lets you register these values once and then resolve
/// any raw string against them, falling back to the standard
/// [`TransactionStatus::from_str`] parser for unregistered values.
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep6::{VendorStatusMap, TransactionStatus};
///
/// let mut map = VendorStatusMap::new();
/// map.register("ach_processing", TransactionStatus::PendingExternal);
/// map.register("kyc_required",   TransactionStatus::PendingUser);
///
/// // Known vendor value → canonical + raw preserved.
/// let r = map.resolve("ach_processing");
/// assert_eq!(r.canonical, TransactionStatus::PendingExternal);
/// assert_eq!(r.vendor_status, "ach_processing");
///
/// // Unknown vendor value → falls back to SEP-6 parser.
/// let r2 = map.resolve("completed");
/// assert_eq!(r2.canonical, TransactionStatus::Completed);
///
/// // Truly unknown value → Error.
/// let r3 = map.resolve("totally_unknown");
/// assert_eq!(r3.canonical, TransactionStatus::Error);
/// assert_eq!(r3.vendor_status, "totally_unknown");
/// ```
#[derive(Clone, Debug, Default)]
pub struct VendorStatusMap {
    entries: alloc::vec::Vec<VendorStatusEntry>,
}

impl VendorStatusMap {
    /// Create an empty vendor status map.
    pub fn new() -> Self {
        VendorStatusMap {
            entries: alloc::vec::Vec::new(),
        }
    }

    /// Register a vendor-specific status string and its canonical equivalent.
    ///
    /// If `vendor_status` is already registered the existing mapping is
    /// replaced.
    pub fn register(&mut self, vendor_status: &str, canonical: TransactionStatus) {
        let key = vendor_status.trim().to_ascii_lowercase();
        if let Some(pos) = self.entries.iter().position(|e| e.vendor_status == key) {
            self.entries[pos].canonical = canonical;
        } else {
            self.entries.push(VendorStatusEntry {
                vendor_status: key,
                canonical,
            });
        }
    }

    /// Resolve a raw anchor status string.
    ///
    /// Resolution order:
    /// 1. Look up the trimmed, lowercased value in the vendor map.
    /// 2. If not found, fall back to [`TransactionStatus::from_str`].
    ///
    /// The returned [`VendorStatusEntry`] always carries the original
    /// (trimmed) raw string so callers can log or forward vendor detail.
    pub fn resolve(&self, raw: &str) -> VendorStatusEntry {
        let key = raw.trim().to_ascii_lowercase();
        if let Some(entry) = self.entries.iter().find(|e| e.vendor_status == key) {
            return VendorStatusEntry {
                vendor_status: raw.trim().into(),
                canonical: entry.canonical.clone(),
            };
        }
        VendorStatusEntry {
            vendor_status: raw.trim().into(),
            canonical: TransactionStatus::from_str(raw),
        }
    }

    /// Returns `true` if the map contains a custom mapping for `vendor_status`.
    pub fn contains(&self, vendor_status: &str) -> bool {
        let key = vendor_status.trim().to_ascii_lowercase();
        self.entries.iter().any(|e| e.vendor_status == key)
    }

    /// Remove the mapping for `vendor_status`. Returns `true` if it existed.
    pub fn remove(&mut self, vendor_status: &str) -> bool {
        let key = vendor_status.trim().to_ascii_lowercase();
        if let Some(pos) = self.entries.iter().position(|e| e.vendor_status == key) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    /// Return the number of registered vendor mappings.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when no vendor mappings are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── Normalized response types ────────────────────────────────────────────────

/// Normalized status values across all SEP-6 anchors.
///
/// Maps the raw string values returned by anchor APIs to typed variants so
/// callers can use `match` without string comparisons.
///
/// # Examples
///
/// ```rust
/// use anchorkit::TransactionStatus;
///
/// assert_eq!(TransactionStatus::from_str("completed"), TransactionStatus::Completed);
/// assert_eq!(TransactionStatus::from_str("unknown_value"), TransactionStatus::Error);
/// assert_eq!(TransactionStatus::Completed.as_str(), "completed");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransactionStatus {
    Pending,
    Incomplete,
    PendingExternal,
    PendingAnchor,
    PendingTrust,
    PendingUser,
    Completed,
    Refunded,
    Expired,
    /// No market exists for the requested asset pair (SEP-6 `no_market`).
    NoMarket,
    /// Requested amount is below the anchor's minimum (SEP-6 `too_small`).
    TooSmall,
    /// Requested amount exceeds the anchor's maximum (SEP-6 `too_large`).
    TooLarge,
    /// Transaction is pending on-chain Stellar network confirmation.
    PendingStellar,
    /// Waiting for the customer to take an action (SEP-6 `waiting_customer_action`).
    WaitingCustomerAction,
    Error,
}

impl TransactionStatus {
    /// Parse a raw anchor status string into a [`TransactionStatus`] variant.
    ///
    /// The input is trimmed and lowercased first to tolerate minor formatting
    /// differences across anchor implementations.
    ///
    /// Unrecognised strings map to [`TransactionStatus::Error`].
    ///
    /// # Arguments
    ///
    /// * `s` - The raw status string from the anchor API.
    ///
    /// # Returns
    ///
    /// The corresponding [`TransactionStatus`] variant.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::TransactionStatus;
    ///
    /// assert_eq!(TransactionStatus::from_str("pending_external"), TransactionStatus::PendingExternal);
    /// assert_eq!(TransactionStatus::from_str("  PENDING_EXTERNAL  "), TransactionStatus::PendingExternal);
    /// assert_eq!(TransactionStatus::from_str("garbage"), TransactionStatus::Error);
    /// ```
    pub fn from_str(s: &str) -> Self {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "pending_external" => Self::PendingExternal,
            "pending_anchor" => Self::PendingAnchor,
            "pending_trust" => Self::PendingTrust,
            "pending_user"
            | "pending_user_transfer_start"
            | "pending_user_transfer_complete" => Self::PendingUser,
            "completed" => Self::Completed,
            "refunded" => Self::Refunded,
            "expired" => Self::Expired,
            "incomplete" => Self::Incomplete,
            "pending" => Self::Pending,
            "no_market" => Self::NoMarket,
            "too_small" => Self::TooSmall,
            "too_large" => Self::TooLarge,
            "pending_stellar" => Self::PendingStellar,
            "waiting_customer_action" => Self::WaitingCustomerAction,
            _ => Self::Error,
        }
    }

    /// Return the canonical SEP-6 string representation of this status.
    ///
    /// # Returns
    ///
    /// A static `&str` matching the SEP-6 specification.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::TransactionStatus;
    ///
    /// assert_eq!(TransactionStatus::PendingUser.as_str(), "pending_user");
    /// ```
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Incomplete => "incomplete",
            Self::PendingExternal => "pending_external",
            Self::PendingAnchor => "pending_anchor",
            Self::PendingTrust => "pending_trust",
            Self::PendingUser => "pending_user",
            Self::Completed => "completed",
            Self::Refunded => "refunded",
            Self::Expired => "expired",
            Self::NoMarket => "no_market",
            Self::TooSmall => "too_small",
            Self::TooLarge => "too_large",
            Self::PendingStellar => "pending_stellar",
            Self::WaitingCustomerAction => "waiting_customer_action",
            Self::Error => "error",
        }
    }

    /// Classify this status into a [`StatusCategory`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::{TransactionStatus, StatusCategory};
    ///
    /// assert_eq!(TransactionStatus::Completed.classify(), StatusCategory::Completed);
    /// assert_eq!(TransactionStatus::Error.classify(), StatusCategory::Failed);
    /// assert_eq!(TransactionStatus::Pending.classify(), StatusCategory::Active);
    /// ```
    pub fn classify(&self) -> StatusCategory {
        match self {
            Self::Pending
            | Self::Incomplete
            | Self::PendingExternal
            | Self::PendingAnchor
            | Self::PendingTrust
            | Self::PendingUser
            | Self::PendingStellar
            | Self::WaitingCustomerAction => StatusCategory::Active,
            Self::Completed => StatusCategory::Completed,
            Self::Refunded => StatusCategory::Refunded,
            Self::Expired => StatusCategory::Expired,
            Self::NoMarket | Self::TooSmall | Self::TooLarge | Self::Error => StatusCategory::Failed,
        }
    }
}

/// Normalized response for a deposit initiation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepositResponse {
    /// Unique transaction ID assigned by the anchor.
    pub transaction_id: String,
    /// How the user should send funds (e.g. bank account, address).
    pub how: String,
    /// Optional extra instructions from the anchor.
    pub extra_info: Option<String>,
    /// Minimum deposit amount (in asset units), if provided.
    pub min_amount: Option<u64>,
    /// Maximum deposit amount (in asset units), if provided.
    pub max_amount: Option<u64>,
    /// Fee charged for the deposit, if provided.
    pub fee_fixed: Option<u64>,
    /// Current status of the transaction.
    pub status: TransactionStatus,
    /// Whether clawback is enabled for this deposit (SEP-6 `clawback_enabled`).
    pub clawback_enabled: Option<bool>,
    /// Stellar memo for identifying the sender, if provided.
    pub stellar_memo: Option<String>,
    /// Type of `stellar_memo` (e.g. `"text"`, `"id"`, `"hash"`), if provided.
    pub stellar_memo_type: Option<String>,
    /// Normalized (uppercase) asset code, if provided.
    pub asset_code: Option<String>,
}

/// Normalized response for a withdrawal initiation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawalResponse {
    /// Unique transaction ID assigned by the anchor.
    pub transaction_id: String,
    /// Stellar account the user should send funds to.
    pub account_id: String,
    /// Optional memo to attach to the Stellar payment.
    pub memo: Option<String>,
    /// Optional memo type (`text`, `id`, `hash`).
    pub memo_type: Option<String>,
    /// Minimum withdrawal amount (in asset units), if provided.
    pub min_amount: Option<u64>,
    /// Maximum withdrawal amount (in asset units), if provided.
    pub max_amount: Option<u64>,
    /// Fee charged for the withdrawal, if provided.
    pub fee_fixed: Option<u64>,
    /// Current status of the transaction.
    pub status: TransactionStatus,
    /// Normalized (uppercase) asset code, if provided.
    pub asset_code: Option<String>,
}

/// Normalized transaction status response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransactionStatusResponse {
    pub transaction_id: String,
    pub kind: TransactionKind,
    pub status: TransactionStatus,
    /// Amount sent by the user (in asset units), if known.
    pub amount_in: Option<u64>,
    /// Amount received by the user after fees (in asset units), if known.
    pub amount_out: Option<u64>,
    /// Fee charged (in asset units), if known.
    pub amount_fee: Option<u64>,
    /// Human-readable message from the anchor, if any.
    pub message: Option<String>,
}

/// Whether the transaction is a deposit or withdrawal.
///
/// # Examples
///
/// ```rust
/// use anchorkit::TransactionKind;
///
/// assert_eq!(TransactionKind::from_str("withdrawal"), TransactionKind::Withdrawal);
/// assert_eq!(TransactionKind::from_str("deposit"), TransactionKind::Deposit);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransactionKind {
    Deposit,
    Withdrawal,
}

impl TransactionKind {
    /// Parse a raw kind string into a [`TransactionKind`] variant.
    ///
    /// Both `"withdrawal"` and `"withdraw"` map to [`TransactionKind::Withdrawal`].
    /// Everything else maps to [`TransactionKind::Deposit`].
    ///
    /// # Arguments
    ///
    /// * `s` - The raw kind string from the anchor API.
    ///
    /// # Returns
    ///
    /// The corresponding [`TransactionKind`] variant.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::TransactionKind;
    ///
    /// assert_eq!(TransactionKind::from_str("withdraw"), TransactionKind::Withdrawal);
    /// assert_eq!(TransactionKind::from_str("deposit"), TransactionKind::Deposit);
    /// ```
    pub fn from_str(s: &str) -> Self {
        match s {
            "withdrawal" | "withdraw" => Self::Withdrawal,
            _ => Self::Deposit,
        }
    }
}

// ── Raw anchor response shapes (anchor-agnostic input) ───────────────────────

/// Raw fields from an anchor's `/deposit` response.
/// Callers populate only the fields the anchor actually returns.
pub struct RawDepositResponse {
    pub transaction_id: String,
    pub how: String,
    pub extra_info: Option<String>,
    pub min_amount: Option<u64>,
    pub max_amount: Option<u64>,
    pub fee_fixed: Option<u64>,
    /// Raw status string from the anchor (e.g. `"pending_external"`).
    pub status: Option<String>,
    /// Whether clawback is enabled for this deposit.
    pub clawback_enabled: Option<bool>,
    /// Stellar memo for identifying the sender.
    pub stellar_memo: Option<String>,
    /// Type of `stellar_memo`.
    pub stellar_memo_type: Option<String>,
    /// Asset code for this deposit (e.g. `"USDC"`). Normalized to uppercase.
    pub asset_code: Option<String>,
}

/// Raw fields from an anchor's `/withdraw` response.
pub struct RawWithdrawalResponse {
    pub transaction_id: String,
    pub account_id: String,
    pub memo: Option<String>,
    pub memo_type: Option<String>,
    pub min_amount: Option<u64>,
    pub max_amount: Option<u64>,
    pub fee_fixed: Option<u64>,
    pub status: Option<String>,
    /// Asset code for this withdrawal (e.g. `"USDC"`). Normalized to uppercase.
    pub asset_code: Option<String>,
}

/// Raw fields from an anchor's `/transaction` response.
pub struct RawTransactionResponse {
    pub transaction_id: String,
    pub kind: Option<String>,
    pub status: String,
    pub amount_in: Option<u64>,
    pub amount_out: Option<u64>,
    pub amount_fee: Option<u64>,
    pub message: Option<String>,
}

// ── Optional-field validation ─────────────────────────────────────────────────

/// Valid SEP-6 memo type strings.
const VALID_MEMO_TYPES: &[&str] = &["text", "id", "hash"];

/// Validate that whenever a memo value is present, a valid memo type is also present.
/// Returns an error when:
/// - `memo` is `Some` but `memo_type` is `None`
/// - `memo_type` is `Some` but not one of `"text"`, `"id"`, `"hash"`
fn validate_memo_pair(memo: Option<&str>, memo_type: Option<&str>) -> Result<(), crate::errors::Error> {
    if memo.is_some() {
        match memo_type {
            None => return Err(crate::errors::Error::invalid_transaction_intent()),
            Some(mt) if !VALID_MEMO_TYPES.contains(&mt) => {
                return Err(crate::errors::Error::invalid_transaction_intent());
            }
            _ => {}
        }
    }
    Ok(())
}

// ── Service functions ─────────────────────────────────────────────────────────

/// Normalize a raw anchor deposit response into a canonical [`DepositResponse`].
///
/// Validates that the required fields `transaction_id` and `how` are non-empty,
/// then maps optional fields and normalises the status string.
///
/// # Edge-case handling
///
/// - Empty or whitespace-only status string defaults to [`TransactionStatus::Pending`].
/// - Status is trimmed and lowercased before matching.
/// - If both `min_amount` and `max_amount` are present, `min_amount <= max_amount`
///   is enforced; violation returns [`Error::InvalidTransactionIntent`].
/// - Asset codes are normalized to uppercase.
///
/// # Arguments
///
/// * `raw` - A [`RawDepositResponse`] populated from the anchor's `/deposit` endpoint.
///
/// # Returns
///
/// A normalised [`DepositResponse`] on success.
///
/// # Errors
///
/// Returns [`Error::InvalidTransactionIntent`] if `transaction_id` or `how` is empty,
/// or if `min_amount > max_amount` when both are present.
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep6::{initiate_deposit, RawDepositResponse, TransactionStatus};
///
/// let raw = RawDepositResponse {
///     transaction_id: "txn-001".into(),
///     how: "Send to bank account 1234".into(),
///     extra_info: None,
///     min_amount: Some(10),
///     max_amount: Some(10_000),
///     fee_fixed: Some(1),
///     status: Some("pending_external".into()),
///     clawback_enabled: None,
///     stellar_memo: None,
///     stellar_memo_type: None,
///     asset_code: Some("usdc".into()),
/// };
/// let resp = initiate_deposit(raw).unwrap();
/// assert_eq!(resp.transaction_id, "txn-001");
/// assert_eq!(resp.status, TransactionStatus::PendingExternal);
/// assert_eq!(resp.asset_code, Some("USDC".into()));
/// ```
pub fn initiate_deposit(raw: RawDepositResponse) -> Result<DepositResponse, Error> {
    if raw.transaction_id.is_empty() || raw.how.is_empty() {
        return Err(Error::invalid_transaction_intent());
    }
    if let (Some(min), Some(max)) = (raw.min_amount, raw.max_amount) {
        if min > max {
            return Err(Error::invalid_transaction_intent());
        }
    }
    validate_memo_pair(raw.stellar_memo.as_deref(), raw.stellar_memo_type.as_deref())?;
    let asset_code = raw.asset_code.as_deref()
        .map(normalize_asset_code)
        .transpose()?;

    Ok(DepositResponse {
        transaction_id: raw.transaction_id,
        how: raw.how,
        extra_info: raw.extra_info,
        min_amount: raw.min_amount,
        max_amount: raw.max_amount,
        fee_fixed: raw.fee_fixed,
        status: raw
            .status
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(TransactionStatus::from_str)
            .unwrap_or(TransactionStatus::Pending),
        clawback_enabled: raw.clawback_enabled,
        stellar_memo: raw.stellar_memo,
        stellar_memo_type: raw.stellar_memo_type,
        asset_code,
    })
}

/// Normalize a raw anchor withdrawal response into a canonical [`WithdrawalResponse`].
///
/// Validates that `transaction_id` and `account_id` are non-empty, then maps
/// optional fields and normalises the status string.
///
/// # Edge-case handling
///
/// - Empty or whitespace-only status string defaults to [`TransactionStatus::Pending`].
/// - Status is trimmed and lowercased before matching.
/// - If both `min_amount` and `max_amount` are present, `min_amount <= max_amount`
///   is enforced; violation returns [`Error::InvalidTransactionIntent`].
/// - Asset codes are normalized to uppercase.
///
/// # Arguments
///
/// * `raw` - A [`RawWithdrawalResponse`] populated from the anchor's `/withdraw` endpoint.
///
/// # Returns
///
/// A normalised [`WithdrawalResponse`] on success.
///
/// # Errors
///
/// Returns [`Error::InvalidTransactionIntent`] if `transaction_id` or `account_id` is empty,
/// or if `min_amount > max_amount` when both are present.
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep6::{initiate_withdrawal, RawWithdrawalResponse, TransactionStatus};
///
/// let raw = RawWithdrawalResponse {
///     transaction_id: "txn-002".into(),
///     account_id: "GABC123".into(),
///     memo: Some("12345".into()),
///     memo_type: Some("id".into()),
///     min_amount: None,
///     max_amount: None,
///     fee_fixed: None,
///     status: Some("pending_user".into()),
///     asset_code: None,
/// };
/// let resp = initiate_withdrawal(raw).unwrap();
/// assert_eq!(resp.status, TransactionStatus::PendingUser);
/// ```
pub fn initiate_withdrawal(raw: RawWithdrawalResponse) -> Result<WithdrawalResponse, Error> {
    if raw.transaction_id.is_empty() || raw.account_id.is_empty() {
        return Err(Error::invalid_transaction_intent());
    }
    if let (Some(min), Some(max)) = (raw.min_amount, raw.max_amount) {
        if min > max {
            return Err(Error::invalid_transaction_intent());
        }
    }
    validate_memo_pair(raw.memo.as_deref(), raw.memo_type.as_deref())?;
    let asset_code = raw.asset_code.as_deref()
        .map(normalize_asset_code)
        .transpose()?;

    Ok(WithdrawalResponse {
        transaction_id: raw.transaction_id,
        account_id: raw.account_id,
        memo: raw.memo,
        memo_type: raw.memo_type,
        min_amount: raw.min_amount,
        max_amount: raw.max_amount,
        fee_fixed: raw.fee_fixed,
        status: raw
            .status
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(TransactionStatus::from_str)
            .unwrap_or(TransactionStatus::Pending),
        asset_code,
    })
}

/// Normalize a raw anchor transaction-status response into a canonical
/// [`TransactionStatusResponse`].
///
/// # Edge-case handling
///
/// - Empty or whitespace-only status is treated as [`TransactionStatus::Error`].
/// - Status string is trimmed and lowercased before matching.
/// - Missing `kind` defaults to [`TransactionKind::Deposit`].
/// - Non-retryable missing `transaction_id` returns an error.
///
/// # Arguments
///
/// * `raw` - A [`RawTransactionResponse`] from the anchor's `/transaction` endpoint.
///
/// # Returns
///
/// A normalised [`TransactionStatusResponse`] on success.
///
/// # Errors
///
/// Returns [`Error::InvalidTransactionIntent`] if `transaction_id` is empty.
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep6::{fetch_transaction_status, RawTransactionResponse, TransactionStatus};
///
/// let raw = RawTransactionResponse {
///     transaction_id: "txn-001".into(),
///     kind: Some("deposit".into()),
///     status: "completed".into(),
///     amount_in: Some(100),
///     amount_out: Some(99),
///     amount_fee: Some(1),
///     message: None,
/// };
/// let resp = fetch_transaction_status(raw).unwrap();
/// assert_eq!(resp.status, TransactionStatus::Completed);
/// ```
pub fn fetch_transaction_status(
    raw: RawTransactionResponse,
) -> Result<TransactionStatusResponse, Error> {
    if raw.transaction_id.is_empty() {
        return Err(Error::invalid_transaction_intent());
    }

    Ok(TransactionStatusResponse {
        transaction_id: raw.transaction_id,
        kind: raw
            .kind
            .as_deref()
            .map(TransactionKind::from_str)
            .unwrap_or(TransactionKind::Deposit),
        status: if raw.status.trim().is_empty() {
            TransactionStatus::Error
        } else {
            TransactionStatus::from_str(&raw.status)
        },
        amount_in: raw.amount_in,
        amount_out: raw.amount_out,
        amount_fee: raw.amount_fee,
        message: raw.message,
    })
}

/// Normalize a list of raw SEP-6 transaction responses (from `GET /transactions`)
/// into canonical [`TransactionStatusResponse`] values.
///
/// Entries with an empty `transaction_id` are silently skipped.
///
/// # Arguments
///
/// * `raw_list` - A `Vec` of [`RawTransactionResponse`] values from the anchor.
///
/// # Returns
///
/// A `Vec` of normalised [`TransactionStatusResponse`] values (empty entries excluded).
///
/// # Examples
///
/// ```rust
/// use anchorkit::sep6::{list_transactions, RawTransactionResponse};
///
/// let raw_list = vec![
///     RawTransactionResponse {
///         transaction_id: "txn-001".into(),
///         kind: Some("deposit".into()),
///         status: "completed".into(),
///         amount_in: Some(100),
///         amount_out: Some(99),
///         amount_fee: Some(1),
///         message: None,
///     },
///     RawTransactionResponse {
///         transaction_id: "".into(), // skipped
///         kind: None,
///         status: "completed".into(),
///         amount_in: None,
///         amount_out: None,
///         amount_fee: None,
///         message: None,
///     },
/// ];
/// let result = list_transactions(raw_list);
/// assert_eq!(result.len(), 1);
/// ```
pub fn list_transactions(
    raw_list: alloc::vec::Vec<RawTransactionResponse>,
) -> alloc::vec::Vec<TransactionStatusResponse> {
    raw_list
        .into_iter()
        .filter(|r| !r.transaction_id.is_empty())
        .map(|r| TransactionStatusResponse {
            transaction_id: r.transaction_id,
            kind: r
                .kind
                .as_deref()
                .map(TransactionKind::from_str)
                .unwrap_or(TransactionKind::Deposit),
            status: TransactionStatus::from_str(&r.status),
            amount_in: r.amount_in,
            amount_out: r.amount_out,
            amount_fee: r.amount_fee,
            message: r.message,
        })
        .collect()
}

// ── Polling ───────────────────────────────────────────────────────────────────

/// Configuration for [`poll_transaction_status`].
#[derive(Clone, Debug)]
pub struct PollConfig {
    /// Interval between polls in milliseconds.
    pub interval_ms: u64,
    /// Maximum total polling duration in milliseconds before timing out.
    pub max_duration_ms: u64,
    /// Status values that stop polling (transaction reached a terminal state).
    pub terminal_states: alloc::vec::Vec<TransactionStatus>,
}

impl Default for PollConfig {
    fn default() -> Self {
        PollConfig {
            interval_ms: 2_000,
            max_duration_ms: 60_000,
            terminal_states: alloc::vec![
                TransactionStatus::Completed,
                TransactionStatus::Refunded,
                TransactionStatus::Expired,
                TransactionStatus::Error,
                TransactionStatus::NoMarket,
                TransactionStatus::TooSmall,
                TransactionStatus::TooLarge,
            ],
        }
    }
}

/// Result of a [`poll_transaction_status`] call.
#[derive(Clone, Debug, PartialEq)]
pub enum PollResult {
    /// Transaction reached a terminal state.
    Completed(TransactionStatusResponse),
    /// Maximum duration elapsed before a terminal state was reached.
    TimedOut,
    /// A non-transient error occurred.
    Failed(crate::errors::Error),
}

/// Poll a transaction until it reaches a terminal state or the timeout expires.
///
/// `fetch_fn` is called at most once per `config.interval_ms`. Transient errors
/// are retried via `retry_with_backoff`. `sleep_fn` is injected so callers can
/// use real or mock sleep.
///
/// # Errors (via `PollResult::Failed`)
/// Non-retryable errors returned by `fetch_fn` stop polling immediately.
pub fn poll_transaction_status<F, S>(
    tx_id: &str,
    config: &PollConfig,
    mut fetch_fn: F,
    mut sleep_fn: S,
) -> PollResult
where
    F: FnMut(&str) -> Result<TransactionStatusResponse, crate::errors::Error>,
    S: FnMut(u64),
{
    use crate::retry::{retry_with_backoff, RetryConfig, MockJitterSource};

    let retry_cfg = RetryConfig::new(3, 100, 1_000, 2);
    let mut elapsed_ms: u64 = 0;

    loop {
        let mut js = MockJitterSource::new(alloc::vec![0]);
        let result = retry_with_backoff(
            &retry_cfg,
            |_| fetch_fn(tx_id),
            |e| crate::retry::is_retryable(e.code),
            |_| {},
            &mut js,
        );

        match result {
            Err(e) => return PollResult::Failed(e),
            Ok(resp) => {
                if config.terminal_states.contains(&resp.status) {
                    return PollResult::Completed(resp);
                }
            }
        }

        if elapsed_ms + config.interval_ms >= config.max_duration_ms {
            return PollResult::TimedOut;
        }

        sleep_fn(config.interval_ms);
        elapsed_ms += config.interval_ms;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::{vec};

    fn raw_deposit() -> RawDepositResponse {
        RawDepositResponse {
            transaction_id: "txn-001".to_string(),
            how: "Send to bank account 1234".to_string(),
            extra_info: None,
            min_amount: Some(10),
            max_amount: Some(10_000),
            fee_fixed: Some(1),
            status: Some("pending_external".to_string()),
            clawback_enabled: None,
            stellar_memo: None,
            stellar_memo_type: None,
            asset_code: None,
        }
    }

    fn raw_withdrawal() -> RawWithdrawalResponse {
        RawWithdrawalResponse {
            transaction_id: "txn-002".to_string(),
            account_id: "GABC123".to_string(),
            memo: Some("12345".to_string()),
            memo_type: Some("id".to_string()),
            min_amount: Some(5),
            max_amount: Some(5_000),
            fee_fixed: Some(2),
            status: Some("pending_user".to_string()),
            asset_code: None,
        }
    }

    fn raw_tx_status() -> RawTransactionResponse {
        RawTransactionResponse {
            transaction_id: "txn-001".to_string(),
            kind: Some("deposit".to_string()),
            status: "completed".to_string(),
            amount_in: Some(100),
            amount_out: Some(99),
            amount_fee: Some(1),
            message: None,
        }
    }

    #[test]
    fn test_initiate_deposit_normalizes_response() {
        let resp = initiate_deposit(raw_deposit()).unwrap();
        assert_eq!(resp.transaction_id, "txn-001");
        assert_eq!(resp.status, TransactionStatus::PendingExternal);
        assert_eq!(resp.fee_fixed, Some(1));
    }

    #[test]
    fn test_initiate_deposit_missing_fields_returns_error() {
        let mut raw = raw_deposit();
        raw.transaction_id = "".to_string();
        assert_eq!(initiate_deposit(raw), Err(Error::invalid_transaction_intent()));
    }

    #[test]
    fn test_initiate_deposit_defaults_status_to_pending() {
        let mut raw = raw_deposit();
        raw.status = None;
        let resp = initiate_deposit(raw).unwrap();
        assert_eq!(resp.status, TransactionStatus::Pending);
    }

    #[test]
    fn test_initiate_withdrawal_normalizes_response() {
        let resp = initiate_withdrawal(raw_withdrawal()).unwrap();
        assert_eq!(resp.transaction_id, "txn-002");
        assert_eq!(resp.status, TransactionStatus::PendingUser);
        assert_eq!(resp.memo_type, Some("id".to_string()));
    }

    #[test]
    fn test_initiate_withdrawal_missing_account_returns_error() {
        let mut raw = raw_withdrawal();
        raw.account_id = "".to_string();
        assert_eq!(
            initiate_withdrawal(raw),
            Err(Error::invalid_transaction_intent())
        );
    }

    #[test]
    fn test_fetch_transaction_status_normalizes_response() {
        let resp = fetch_transaction_status(raw_tx_status()).unwrap();
        assert_eq!(resp.status, TransactionStatus::Completed);
        assert_eq!(resp.kind, TransactionKind::Deposit);
        assert_eq!(resp.amount_out, Some(99));
    }

    #[test]
    fn test_fetch_transaction_status_missing_id_returns_error() {
        let mut raw = raw_tx_status();
        raw.transaction_id = "".to_string();
        assert_eq!(
            fetch_transaction_status(raw),
            Err(Error::invalid_transaction_intent())
        );
    }

    #[test]
    fn test_fetch_transaction_status_unknown_status_maps_to_error() {
        let mut raw = raw_tx_status();
        raw.status = "some_unknown_status".to_string();
        let resp = fetch_transaction_status(raw).unwrap();
        assert_eq!(resp.status, TransactionStatus::Error);
    }

    #[test]
    fn test_withdrawal_kind_normalization() {
        let mut raw = raw_tx_status();
        raw.kind = Some("withdraw".to_string());
        let resp = fetch_transaction_status(raw).unwrap();
        assert_eq!(resp.kind, TransactionKind::Withdrawal);
    }

    #[test]
    fn test_list_transactions_normalizes_all() {
        let raw_list = vec![
            RawTransactionResponse {
                transaction_id: "txn-001".to_string(),
                kind: Some("deposit".to_string()),
                status: "completed".to_string(),
                amount_in: Some(100),
                amount_out: Some(99),
                amount_fee: Some(1),
                message: None,
            },
            RawTransactionResponse {
                transaction_id: "txn-002".to_string(),
                kind: Some("withdrawal".to_string()),
                status: "pending_external".to_string(),
                amount_in: None,
                amount_out: None,
                amount_fee: None,
                message: Some("awaiting bank".to_string()),
            },
        ];
        let result = list_transactions(raw_list);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].transaction_id, "txn-001");
        assert_eq!(result[0].status, TransactionStatus::Completed);
        assert_eq!(result[0].kind, TransactionKind::Deposit);
        assert_eq!(result[1].transaction_id, "txn-002");
        assert_eq!(result[1].status, TransactionStatus::PendingExternal);
        assert_eq!(result[1].kind, TransactionKind::Withdrawal);
    }

    #[test]
    fn test_list_transactions_skips_empty_ids() {
        let raw_list = vec![
            RawTransactionResponse {
                transaction_id: "".to_string(),
                kind: None,
                status: "completed".to_string(),
                amount_in: None,
                amount_out: None,
                amount_fee: None,
                message: None,
            },
            RawTransactionResponse {
                transaction_id: "txn-valid".to_string(),
                kind: None,
                status: "completed".to_string(),
                amount_in: Some(50),
                amount_out: Some(49),
                amount_fee: Some(1),
                message: None,
            },
        ];
        let result = list_transactions(raw_list);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].transaction_id, "txn-valid");
    }

    #[test]
    fn test_list_transactions_empty_input() {
        let result = list_transactions(vec![]);
        assert!(result.is_empty());
    }

    // ── Polling tests ─────────────────────────────────────────────────────────

    fn make_response(status: TransactionStatus) -> TransactionStatusResponse {
        TransactionStatusResponse {
            transaction_id: "txn-poll".to_string(),
            kind: TransactionKind::Deposit,
            status,
            amount_in: None,
            amount_out: None,
            amount_fee: None,
            message: None,
        }
    }

    #[test]
    fn test_poll_completes_before_timeout() {
        let config = PollConfig {
            interval_ms: 100,
            max_duration_ms: 10_000,
            terminal_states: vec![TransactionStatus::Completed],
        };
        let mut call_count = 0u32;
        let result = poll_transaction_status(
            "txn-poll",
            &config,
            |_| {
                call_count += 1;
                Ok(make_response(TransactionStatus::Completed))
            },
            |_| {},
        );
        assert_eq!(result, PollResult::Completed(make_response(TransactionStatus::Completed)));
        assert_eq!(call_count, 1);
    }

    #[test]
    fn test_poll_times_out() {
        let config = PollConfig {
            interval_ms: 1_000,
            max_duration_ms: 2_000,
            terminal_states: vec![TransactionStatus::Completed],
        };
        let result = poll_transaction_status(
            "txn-poll",
            &config,
            |_| Ok(make_response(TransactionStatus::Pending)),
            |_| {},
        );
        assert_eq!(result, PollResult::TimedOut);
    }

    #[test]
    fn test_poll_retries_transient_error_then_succeeds() {
        use crate::errors::{Error, ErrorCode};
        let config = PollConfig {
            interval_ms: 100,
            max_duration_ms: 10_000,
            terminal_states: vec![TransactionStatus::Completed],
        };
        let mut call_count = 0u32;
        let result = poll_transaction_status(
            "txn-poll",
            &config,
            |_| {
                call_count += 1;
                if call_count < 3 {
                    Err(Error::from_code(ErrorCode::ServicesNotConfigured))
                } else {
                    Ok(make_response(TransactionStatus::Completed))
                }
            },
            |_| {},
        );
        assert_eq!(result, PollResult::Completed(make_response(TransactionStatus::Completed)));
        assert_eq!(call_count, 3);
    }

    // ── Optional field combination tests (#255) ───────────────────────────────

    #[test]
    fn test_deposit_memo_without_memo_type_is_rejected() {
        let mut raw = raw_deposit();
        raw.stellar_memo = Some("12345".to_string());
        raw.stellar_memo_type = None;
        assert_eq!(initiate_deposit(raw), Err(Error::invalid_transaction_intent()));
    }

    #[test]
    fn test_deposit_memo_with_invalid_memo_type_is_rejected() {
        let mut raw = raw_deposit();
        raw.stellar_memo = Some("12345".to_string());
        raw.stellar_memo_type = Some("fax".to_string()); // invalid type
        assert_eq!(initiate_deposit(raw), Err(Error::invalid_transaction_intent()));
    }

    #[test]
    fn test_deposit_memo_with_valid_text_type_is_accepted() {
        let mut raw = raw_deposit();
        raw.stellar_memo = Some("hello".to_string());
        raw.stellar_memo_type = Some("text".to_string());
        assert!(initiate_deposit(raw).is_ok());
    }

    #[test]
    fn test_deposit_memo_with_valid_id_type_is_accepted() {
        let mut raw = raw_deposit();
        raw.stellar_memo = Some("99999".to_string());
        raw.stellar_memo_type = Some("id".to_string());
        assert!(initiate_deposit(raw).is_ok());
    }

    #[test]
    fn test_deposit_memo_with_valid_hash_type_is_accepted() {
        let mut raw = raw_deposit();
        raw.stellar_memo = Some("abc123".to_string());
        raw.stellar_memo_type = Some("hash".to_string());
        assert!(initiate_deposit(raw).is_ok());
    }

    #[test]
    fn test_deposit_no_memo_no_memo_type_is_accepted() {
        let mut raw = raw_deposit();
        raw.stellar_memo = None;
        raw.stellar_memo_type = None;
        assert!(initiate_deposit(raw).is_ok());
    }

    #[test]
    fn test_withdrawal_memo_without_memo_type_is_rejected() {
        let mut raw = raw_withdrawal();
        raw.memo = Some("12345".to_string());
        raw.memo_type = None;
        assert_eq!(initiate_withdrawal(raw), Err(Error::invalid_transaction_intent()));
    }

    #[test]
    fn test_withdrawal_memo_with_invalid_memo_type_is_rejected() {
        let mut raw = raw_withdrawal();
        raw.memo = Some("12345".to_string());
        raw.memo_type = Some("telegraph".to_string());
        assert_eq!(initiate_withdrawal(raw), Err(Error::invalid_transaction_intent()));
    }

    #[test]
    fn test_withdrawal_memo_with_valid_id_type_is_accepted() {
        let raw = raw_withdrawal(); // already has memo="12345" and memo_type="id"
        assert!(initiate_withdrawal(raw).is_ok());
    }

    #[test]
    fn test_withdrawal_no_memo_no_memo_type_is_accepted() {
        let mut raw = raw_withdrawal();
        raw.memo = None;
        raw.memo_type = None;
        assert!(initiate_withdrawal(raw).is_ok());
    }

    #[test]
    fn test_status_pending_stellar_round_trip() {
        assert_eq!(TransactionStatus::from_str("pending_stellar"), TransactionStatus::PendingStellar);
        assert_eq!(TransactionStatus::PendingStellar.as_str(), "pending_stellar");
    }

    #[test]
    fn test_status_waiting_customer_action_round_trip() {
        assert_eq!(
            TransactionStatus::from_str("waiting_customer_action"),
            TransactionStatus::WaitingCustomerAction
        );
        assert_eq!(TransactionStatus::WaitingCustomerAction.as_str(), "waiting_customer_action");
    }

    #[test]
    fn test_poll_terminal_state_detection_all_variants() {
        let terminals = vec![
            TransactionStatus::Completed,
            TransactionStatus::Refunded,
            TransactionStatus::Expired,
            TransactionStatus::Error,
            TransactionStatus::NoMarket,
            TransactionStatus::TooSmall,
            TransactionStatus::TooLarge,
        ];
        for status in terminals {
            let config = PollConfig {
                interval_ms: 100,
                max_duration_ms: 10_000,
                terminal_states: vec![status.clone()],
            };
            let result = poll_transaction_status(
                "txn-poll",
                &config,
                |_| Ok(make_response(status.clone())),
                |_| {},
            );
            assert!(matches!(result, PollResult::Completed(_)), "expected Completed for {:?}", status);
        }
    }

    // ── #614 Status classification ─────────────────────────────────────────

    #[test]
    fn test_transaction_status_classify_active() {
        for status in &[
            TransactionStatus::Pending,
            TransactionStatus::Incomplete,
            TransactionStatus::PendingExternal,
            TransactionStatus::PendingAnchor,
            TransactionStatus::PendingTrust,
            TransactionStatus::PendingUser,
            TransactionStatus::PendingStellar,
            TransactionStatus::WaitingCustomerAction,
        ] {
            assert_eq!(status.classify(), StatusCategory::Active, "expected Active for {:?}", status);
        }
    }

    #[test]
    fn test_transaction_status_classify_completed() {
        assert_eq!(TransactionStatus::Completed.classify(), StatusCategory::Completed);
    }

    #[test]
    fn test_transaction_status_classify_refunded() {
        assert_eq!(TransactionStatus::Refunded.classify(), StatusCategory::Refunded);
    }

    #[test]
    fn test_transaction_status_classify_expired() {
        assert_eq!(TransactionStatus::Expired.classify(), StatusCategory::Expired);
    }

    #[test]
    fn test_transaction_status_classify_failed() {
        for status in &[
            TransactionStatus::NoMarket,
            TransactionStatus::TooSmall,
            TransactionStatus::TooLarge,
            TransactionStatus::Error,
        ] {
            assert_eq!(status.classify(), StatusCategory::Failed, "expected Failed for {:?}", status);
        }
    }

    #[test]
    fn test_classify_status_str_all_categories() {
        assert_eq!(classify_status_str("pending"), StatusCategory::Active);
        assert_eq!(classify_status_str("completed"), StatusCategory::Completed);
        assert_eq!(classify_status_str("refunded"), StatusCategory::Refunded);
        assert_eq!(classify_status_str("expired"), StatusCategory::Expired);
        assert_eq!(classify_status_str("error"), StatusCategory::Failed);
        assert_eq!(classify_status_str("no_market"), StatusCategory::Failed);
        assert_eq!(classify_status_str("too_small"), StatusCategory::Failed);
        assert_eq!(classify_status_str("too_large"), StatusCategory::Failed);
        assert_eq!(classify_status_str("garbage_status"), StatusCategory::Unknown);
        assert_eq!(classify_status_str(""), StatusCategory::Unknown);
    }

    #[test]
    fn test_classify_status_str_normalizes_input() {
        assert_eq!(classify_status_str("  COMPLETED  "), StatusCategory::Completed);
        assert_eq!(classify_status_str("PENDING_EXTERNAL"), StatusCategory::Active);
        assert_eq!(classify_status_str("  pending_user  "), StatusCategory::Active);
    }

    // ── #613 Edge-case normalization ─────────────────────────────────────────

    #[test]
    fn test_status_from_str_case_insensitive() {
        assert_eq!(TransactionStatus::from_str("COMPLETED"), TransactionStatus::Completed);
        assert_eq!(TransactionStatus::from_str("Pending_External"), TransactionStatus::PendingExternal);
        assert_eq!(TransactionStatus::from_str("  PENDING  "), TransactionStatus::Pending);
    }

    #[test]
    fn test_initiate_deposit_empty_status_defaults_to_pending() {
        let mut raw = raw_deposit();
        raw.status = Some("".to_string());
        let resp = initiate_deposit(raw).unwrap();
        assert_eq!(resp.status, TransactionStatus::Pending);
    }

    #[test]
    fn test_initiate_deposit_whitespace_status_defaults_to_pending() {
        let mut raw = raw_deposit();
        raw.status = Some("   ".to_string());
        let resp = initiate_deposit(raw).unwrap();
        assert_eq!(resp.status, TransactionStatus::Pending);
    }

    #[test]
    fn test_initiate_withdrawal_empty_status_defaults_to_pending() {
        let mut raw = raw_withdrawal();
        raw.status = Some("".to_string());
        let resp = initiate_withdrawal(raw).unwrap();
        assert_eq!(resp.status, TransactionStatus::Pending);
    }

    #[test]
    fn test_fetch_transaction_status_empty_status_maps_to_error() {
        let mut raw = raw_tx_status();
        raw.status = "".to_string();
        let resp = fetch_transaction_status(raw).unwrap();
        assert_eq!(resp.status, TransactionStatus::Error);
    }

    #[test]
    fn test_fetch_transaction_status_whitespace_status_maps_to_error() {
        let mut raw = raw_tx_status();
        raw.status = "   ".to_string();
        let resp = fetch_transaction_status(raw).unwrap();
        assert_eq!(resp.status, TransactionStatus::Error);
    }

    #[test]
    fn test_initiate_deposit_min_amount_gt_max_amount_rejected() {
        let mut raw = raw_deposit();
        raw.min_amount = Some(100);
        raw.max_amount = Some(10);
        assert_eq!(initiate_deposit(raw), Err(Error::invalid_transaction_intent()));
    }

    #[test]
    fn test_initiate_deposit_min_amount_eq_max_amount_accepted() {
        let mut raw = raw_deposit();
        raw.min_amount = Some(100);
        raw.max_amount = Some(100);
        assert!(initiate_deposit(raw).is_ok());
    }

    #[test]
    fn test_initiate_withdrawal_min_amount_gt_max_amount_rejected() {
        let mut raw = raw_withdrawal();
        raw.min_amount = Some(100);
        raw.max_amount = Some(10);
        assert_eq!(initiate_withdrawal(raw), Err(Error::invalid_transaction_intent()));
    }

    #[test]
    fn test_initiate_withdrawal_min_amount_eq_max_amount_accepted() {
        let mut raw = raw_withdrawal();
        raw.min_amount = Some(50);
        raw.max_amount = Some(50);
        assert!(initiate_withdrawal(raw).is_ok());
    }

    #[test]
    fn test_status_case_variants_mapped_correctly() {
        for (input, expected) in &[
            ("pending_external", TransactionStatus::PendingExternal),
            ("PENDING_EXTERNAL", TransactionStatus::PendingExternal),
            ("Pending_Anchor", TransactionStatus::PendingAnchor),
            ("  PENDING_TRUST  ", TransactionStatus::PendingTrust),
            ("PENDING_USER_TRANSFER_START", TransactionStatus::PendingUser),
            ("Pending_User_Transfer_Complete", TransactionStatus::PendingUser),
            ("NO_MARKET", TransactionStatus::NoMarket),
            ("TOO_SMALL", TransactionStatus::TooSmall),
            ("TOO_LARGE", TransactionStatus::TooLarge),
        ] {
            assert_eq!(TransactionStatus::from_str(input), *expected, "mismatch for '{}'", input);
        }
    }
}
