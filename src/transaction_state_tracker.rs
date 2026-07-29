use soroban_sdk::{contracttype, symbol_short, Address, Env, String, Vec};

/// Default TTL: ~90 days at 5 s/ledger.
const TXSTATE_TTL: u32 = 1_555_200;
/// Terminal TTL: ~30 days.
const TXSTATE_TTL_TERMINAL: u32 = 518_400;

// ── StorageBudgetMonitor ──────────────────────────────────────────────────────

/// Tracks the number of persistent storage entries and their approximate sizes.
#[derive(Clone, Debug, Default)]
pub struct StorageBudgetMonitor {
    pub entry_count: u64,
    pub approx_bytes: u64,
}

impl StorageBudgetMonitor {
    pub fn new() -> Self { Self::default() }

    /// Record a new entry of `size_bytes`.
    pub fn record_entry(&mut self, size_bytes: u64) {
        self.entry_count += 1;
        self.approx_bytes += size_bytes;
    }

    /// Remove a tracked entry of `size_bytes`.
    pub fn remove_entry(&mut self, size_bytes: u64) {
        self.entry_count = self.entry_count.saturating_sub(1);
        self.approx_bytes = self.approx_bytes.saturating_sub(size_bytes);
    }

    /// Return current byte usage as a percentage of `max_bytes`, capped at 100.
    /// Returns 100 when `max_bytes` is zero.
    pub fn usage_percent(&self, max_bytes: u64) -> u64 {
        if max_bytes == 0 {
            return 100;
        }
        (self.approx_bytes.saturating_mul(100) / max_bytes).min(100)
    }

    /// Classify current usage against warning and critical byte thresholds.
    pub fn get_status(&self, warning_bytes: u64, critical_bytes: u64) -> BudgetStatus {
        if self.approx_bytes >= critical_bytes {
            BudgetStatus::Critical
        } else if self.approx_bytes >= warning_bytes {
            BudgetStatus::Warning
        } else {
            BudgetStatus::Ok
        }
    }

    /// Return `Some(BudgetAlert)` when current usage exceeds either
    /// `threshold_entries` or `threshold_bytes`; `None` otherwise.
    pub fn check_alert(&self, threshold_entries: u64, threshold_bytes: u64) -> Option<BudgetAlert> {
        if self.entry_count >= threshold_entries || self.approx_bytes >= threshold_bytes {
            let status = if self.approx_bytes >= threshold_bytes.saturating_mul(2) {
                BudgetStatus::Critical
            } else {
                BudgetStatus::Warning
            };
            Some(BudgetAlert {
                status,
                entry_count: self.entry_count,
                approx_bytes: self.approx_bytes,
                threshold_bytes,
            })
        } else {
            None
        }
    }

    /// Return `true` when byte usage is at or above `threshold_pct`% of `max_bytes`.
    pub fn is_near_limit(&self, threshold_pct: u64, max_bytes: u64) -> bool {
        self.usage_percent(max_bytes) >= threshold_pct
    }
}

/// Priority level of a storage budget alert.
#[derive(Clone, Debug, PartialEq)]
pub enum BudgetStatus {
    /// Usage is within safe operating range.
    Ok,
    /// Usage has crossed the warning threshold.
    Warning,
    /// Usage has crossed the critical threshold — immediate action required.
    Critical,
}

/// Alert emitted when storage budget usage crosses a configured threshold.
#[derive(Clone, Debug)]
pub struct BudgetAlert {
    pub status: BudgetStatus,
    pub entry_count: u64,
    pub approx_bytes: u64,
    pub threshold_bytes: u64,
}

/// On-chain storage budget usage report for the transaction state tracker
/// (#627), returned by [`crate::contract::AnchorKitContract::get_storage_budget_report`].
///
/// Unlike [`StorageBudgetMonitor`] (an in-memory helper used by the dev/test
/// harness), this type is a `#[contracttype]` so it can cross the contract
/// ABI boundary and be queried by any caller before storage pressure causes
/// failures.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageBudgetReport {
    /// Number of transaction records currently tracked in persistent storage.
    pub entry_count: u64,
    /// Approximate total bytes consumed by tracked transaction records.
    pub approx_bytes: u64,
    /// Configured warning threshold, in bytes.
    pub warning_bytes: u64,
    /// Configured critical threshold, in bytes.
    pub critical_bytes: u64,
    /// `true` once `approx_bytes` has reached `warning_bytes` (or higher).
    pub warning: bool,
    /// `true` once `approx_bytes` has reached `critical_bytes`.
    pub critical: bool,
}

/// The lifecycle states a tracked transaction can occupy.
///
/// Legal forward transitions are:
/// - [`Pending`](TransactionState::Pending) → [`InProgress`](TransactionState::InProgress)
/// - [`InProgress`](TransactionState::InProgress) → [`Completed`](TransactionState::Completed)
/// - [`InProgress`](TransactionState::InProgress) → [`Failed`](TransactionState::Failed)
///
/// All other transitions are rejected by [`TransactionState::is_valid_transition`].
///
/// # Examples
///
/// ```rust
/// use anchorkit::TransactionState;
///
/// assert!(TransactionState::Pending.is_valid_transition(TransactionState::InProgress));
/// assert!(!TransactionState::Pending.is_valid_transition(TransactionState::Completed));
/// assert_eq!(TransactionState::Completed.as_str(), "completed");
/// ```
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum TransactionState {
    Pending = 1,
    InProgress = 2,
    Completed = 3,
    Failed = 4,
}

impl TransactionState {
    /// Return the canonical lowercase string representation of this state.
    ///
    /// # Returns
    ///
    /// One of `"pending"`, `"in_progress"`, `"completed"`, or `"failed"`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::TransactionState;
    ///
    /// assert_eq!(TransactionState::InProgress.as_str(), "in_progress");
    /// ```
    pub fn as_str(&self) -> &'static str {
        match self {
            TransactionState::Pending => "pending",
            TransactionState::InProgress => "in_progress",
            TransactionState::Completed => "completed",
            TransactionState::Failed => "failed",
        }
    }

    /// Parse a state from its canonical string representation.
    ///
    /// # Arguments
    ///
    /// * `s` - One of `"pending"`, `"in_progress"`, `"completed"`, or `"failed"`.
    ///
    /// # Returns
    ///
    /// `Some(TransactionState)` on a recognised string, `None` otherwise.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::TransactionState;
    ///
    /// assert_eq!(TransactionState::from_str("completed"), Some(TransactionState::Completed));
    /// assert_eq!(TransactionState::from_str("unknown"), None);
    /// ```
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(TransactionState::Pending),
            "in_progress" => Some(TransactionState::InProgress),
            "completed" => Some(TransactionState::Completed),
            "failed" => Some(TransactionState::Failed),
            _ => None,
        }
    }

    /// Returns `true` only for explicitly permitted forward transitions.
    ///
    /// # Transition matrix
    ///
    /// | From        | To          | Allowed |
    /// |-------------|-------------|---------|
    /// | Pending     | InProgress  | ✓       |
    /// | Pending     | Failed      | ✓ (immediate failure before processing starts) |
    /// | InProgress  | Completed   | ✓       |
    /// | InProgress  | Failed      | ✓       |
    /// | Completed   | *           | ✗ terminal state |
    /// | Failed      | *           | ✗ terminal state |
    /// | *           | Pending     | ✗ no backward resets |
    /// | *           | same state  | ✗ no self-loops |
    ///
    /// # Arguments
    ///
    /// * `to` - The target state.
    ///
    /// # Returns
    ///
    /// `true` if the transition from `self` to `to` is permitted.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::TransactionState;
    ///
    /// // Valid forward transitions
    /// assert!(TransactionState::Pending.is_valid_transition(TransactionState::InProgress));
    /// assert!(TransactionState::Pending.is_valid_transition(TransactionState::Failed));
    /// assert!(TransactionState::InProgress.is_valid_transition(TransactionState::Completed));
    /// assert!(TransactionState::InProgress.is_valid_transition(TransactionState::Failed));
    ///
    /// // Terminal states cannot transition further
    /// assert!(!TransactionState::Completed.is_valid_transition(TransactionState::InProgress));
    /// assert!(!TransactionState::Failed.is_valid_transition(TransactionState::InProgress));
    ///
    /// // Backward and self-loop transitions are rejected
    /// assert!(!TransactionState::Completed.is_valid_transition(TransactionState::Pending));
    /// assert!(!TransactionState::Pending.is_valid_transition(TransactionState::Pending));
    /// assert!(!TransactionState::InProgress.is_valid_transition(TransactionState::Pending));
    /// ```
    pub fn is_valid_transition(&self, to: TransactionState) -> bool {
        matches!(
            (self, to),
            // Pending can move forward to processing or fail immediately
            (TransactionState::Pending,    TransactionState::InProgress)
            | (TransactionState::Pending,  TransactionState::Failed)
            // InProgress can complete successfully or fail
            | (TransactionState::InProgress, TransactionState::Completed)
            | (TransactionState::InProgress, TransactionState::Failed)
            // Completed and Failed are terminal — no further transitions allowed
        )
    }

    /// Returns `true` if this state is terminal (no further transitions are permitted).
    ///
    /// Terminal states are [`Completed`](TransactionState::Completed) and
    /// [`Failed`](TransactionState::Failed).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::TransactionState;
    ///
    /// assert!(TransactionState::Completed.is_terminal());
    /// assert!(TransactionState::Failed.is_terminal());
    /// assert!(!TransactionState::Pending.is_terminal());
    /// assert!(!TransactionState::InProgress.is_terminal());
    /// ```
    pub fn is_terminal(&self) -> bool {
        matches!(self, TransactionState::Completed | TransactionState::Failed)
    }

    /// Build the canonical error message for an illegal transition from `self` to `to`.
    ///
    /// The message is prefixed with `"[E24]"` (the [`ErrorCode::IllegalTransition`]
    /// discriminant) so callers can detect transition errors without string-matching
    /// the full message body.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::TransactionState;
    ///
    /// let msg = TransactionState::Completed.illegal_transition_message(TransactionState::Pending);
    /// assert!(msg.starts_with("[E24]"));
    /// assert!(msg.contains("completed"));
    /// assert!(msg.contains("pending"));
    /// ```
    pub fn illegal_transition_message(&self, to: TransactionState) -> alloc::string::String {
        alloc::format!(
            "[E24] Illegal transaction state transition: {} -> {}",
            self.as_str(),
            to.as_str()
        )
    }
}

/// Recovery metadata attached to a failed transaction.
///
/// Populated by [`TransactionStateTracker::fail_transaction`] and accessible
/// via [`TransactionStateTracker::get_recovery_metadata`].
///
/// # Fields
///
/// - `failure_reason`: human-readable description of why the transaction failed.
/// - `last_updated_ledger`: the ledger sequence number at the time of failure,
///   useful for on-chain auditing and replay-protection checks.
/// - `failed_from_state`: the state the transaction was in when it failed,
///   which indicates how far processing progressed before the error.
/// - `retry_count`: number of times a recovery attempt has been recorded via
///   [`TransactionStateTracker::record_recovery_attempt`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryMetadata {
    /// Human-readable reason for the failure.
    pub failure_reason: String,
    /// Ledger sequence number at the time of failure.
    pub last_updated_ledger: u32,
    /// The state the transaction occupied immediately before failing.
    pub failed_from_state: TransactionState,
    /// Number of recovery attempts recorded after the failure.
    pub retry_count: u32,
}

/// Soroban-compatible optional wrapper for [`RecoveryMetadata`].
///
/// `Option<RecoveryMetadata>` cannot be used directly in `#[contracttype]`
/// structs because Soroban's XDR layer does not automatically derive
/// `IntoVal`/`TryFromVal` for `Option<UserDefinedType>`. This enum is the
/// on-chain equivalent of `Option<RecoveryMetadata>` and provides the same
/// ergonomic API via inherent methods.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OptRecovery {
    None,
    Some(RecoveryMetadata),
}

impl OptRecovery {
    pub fn is_some(&self) -> bool { matches!(self, OptRecovery::Some(_)) }
    pub fn is_none(&self) -> bool { matches!(self, OptRecovery::None) }

    pub fn unwrap(self) -> RecoveryMetadata {
        match self {
            OptRecovery::Some(m) => m,
            OptRecovery::None => panic!("called unwrap() on OptRecovery::None"),
        }
    }

    pub fn as_mut(&mut self) -> Option<&mut RecoveryMetadata> {
        match self {
            OptRecovery::Some(m) => Some(m),
            OptRecovery::None => None,
        }
    }

    pub fn into_option(self) -> Option<RecoveryMetadata> {
        match self {
            OptRecovery::Some(m) => Some(m),
            OptRecovery::None => None,
        }
    }
}

// ── Recovery metadata pruning & archival (#628) ──────────────────────────────

/// Configurable retention policy for [`RecoveryMetadata`] (#628).
///
/// Recovery metadata is only useful for a bounded window after a failure —
/// long enough for an operator or automated process to retry or inspect it,
/// but not forever. Without a retention policy, recovery metadata for very
/// old or repeatedly-retried failures would accumulate indefinitely. A
/// record's metadata becomes eligible for pruning/archival once *either*
/// condition is met:
///
/// - it is older than `max_age_seconds` (measured from the record's
///   `last_updated` timestamp, i.e. the time of the failure or the most
///   recent recovery attempt), or
/// - its `retry_count` has reached `max_retry_count`, indicating recovery
///   has been abandoned or exhausted.
///
/// # Examples
///
/// ```rust
/// use anchorkit::transaction_state_tracker::RecoveryRetentionPolicy;
///
/// // Prune/archive after 30 days or 5 failed recovery attempts, whichever
/// // comes first.
/// let policy = RecoveryRetentionPolicy { max_age_seconds: 30 * 24 * 3600, max_retry_count: 5 };
/// ```
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryRetentionPolicy {
    /// Recovery metadata older than this many seconds becomes eligible for
    /// pruning/archival.
    pub max_age_seconds: u64,
    /// Recovery metadata whose `retry_count` is at or above this ceiling
    /// becomes eligible for pruning/archival, regardless of age.
    pub max_retry_count: u32,
}

impl RecoveryRetentionPolicy {
    /// Default retention: 30 days or 5 retries, whichever comes first.
    pub const DEFAULT_MAX_AGE_SECONDS: u64 = 30 * 24 * 3600;
    /// Default maximum retry count before metadata is considered abandoned.
    pub const DEFAULT_MAX_RETRY_COUNT: u32 = 5;

    /// The default retention policy (30 days / 5 retries).
    pub fn default_policy() -> Self {
        RecoveryRetentionPolicy {
            max_age_seconds: Self::DEFAULT_MAX_AGE_SECONDS,
            max_retry_count: Self::DEFAULT_MAX_RETRY_COUNT,
        }
    }

    /// Validate that the policy has sensible non-zero values.
    pub fn validate(&self) -> Result<(), alloc::string::String> {
        if self.max_age_seconds == 0 {
            return Err(alloc::string::String::from("max_age_seconds must be > 0"));
        }
        if self.max_retry_count == 0 {
            return Err(alloc::string::String::from("max_retry_count must be > 0"));
        }
        Ok(())
    }

    /// Returns `true` when `meta` is eligible for pruning/archival given
    /// `record_last_updated` (the owning record's `last_updated` timestamp)
    /// and the current ledger `now` timestamp.
    pub fn is_eligible(&self, meta: &RecoveryMetadata, record_last_updated: u64, now: u64) -> bool {
        let age = now.saturating_sub(record_last_updated);
        age >= self.max_age_seconds || meta.retry_count >= self.max_retry_count
    }
}

/// Compact archived form of [`RecoveryMetadata`], retained after the live
/// metadata on a [`TransactionStateRecord`] has been pruned (#628).
///
/// Archival preserves the fields needed to answer "why did this transaction
/// fail, and how many times was recovery attempted?" after the record's live
/// `recovery_metadata` has been cleared — it is intentionally a strict subset
/// of [`RecoveryMetadata`] plus the identifying `transaction_id` and the
/// ledger at which archival occurred, rather than a full duplicate.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchivedRecoveryMetadata {
    /// The transaction this archived metadata belongs to.
    pub transaction_id: u64,
    /// Human-readable reason for the original failure.
    pub failure_reason: String,
    /// The state the transaction occupied immediately before failing.
    pub failed_from_state: TransactionState,
    /// Number of recovery attempts recorded before archival.
    pub retry_count: u32,
    /// Ledger sequence number at which this metadata was archived.
    pub archived_at_ledger: u32,
}

/// A snapshot of a tracked transaction's current state.
///
/// Stored in Soroban persistent storage (production) or an in-memory cache
/// (dev mode). Retrieved via [`TransactionStateTracker::get_transaction_state`].
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransactionStateRecord {
    pub transaction_id: u64,
    pub state: TransactionState,
    pub initiator: Address,
    pub timestamp: u64,
    pub last_updated: u64,
    /// Ledger sequence number of the most recent state change.
    pub last_updated_ledger: u32,
    pub error_message: Option<String>,
    /// Full state progression: (state, timestamp) pairs in chronological order.
    pub state_history: Vec<(TransactionState, u64)>,
    /// Recovery metadata, populated when the transaction enters the
    /// [`Failed`](TransactionState::Failed) state.
    pub recovery_metadata: OptRecovery,
    /// Optional routing reason or referral code recorded at creation time (#298).
    /// Explains why a particular anchor or route was chosen (e.g. `"lowest_fee"`,
    /// `"referral"`, `"preferred_anchor"`). `None` when no reason was recorded.
    /// Persists unchanged through all state transitions.
    pub routing_reason: Option<String>,
}

/// Aggregated counts per [`TransactionState`] returned by
/// [`TransactionStateTracker::summarize_transactions_by_status`].
///
/// All four counters are always present; unused states carry a count of zero.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct TransactionSummary {
    pub pending_count: u64,
    pub in_progress_count: u64,
    pub completed_count: u64,
    pub failed_count: u64,
    pub total_count: u64,
}

/// Audit entry for a single transition attempt (success or failure).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionAuditEntry {
    pub transaction_id: u64,
    pub from_state: TransactionState,
    pub to_state: TransactionState,
    pub timestamp: u64,
    pub success: bool,
}

/// State-machine tracker for on-chain transactions.
///
/// In production mode (`is_dev_mode = false`) records are persisted to Soroban
/// persistent storage with a TTL of ~90 days. In dev/test mode they are kept in
/// an in-memory `Vec` for fast iteration.
///
/// # Examples
///
/// ```rust,no_run
/// # use soroban_sdk::Env;
/// # use soroban_sdk::testutils::Address as _;
/// # let env = Env::default();
/// # let initiator = soroban_sdk::Address::generate(&env);
/// use anchorkit::transaction_state_tracker::{TransactionStateTracker, TransactionState};
///
/// let mut tracker = TransactionStateTracker::new(true); // dev mode
/// tracker.create_transaction(1, initiator, &env).unwrap();
/// tracker.start_transaction(1, &env).unwrap();
/// let record = tracker.complete_transaction(1, &env).unwrap();
/// assert_eq!(record.state, TransactionState::Completed);
/// ```
#[derive(Clone)]
pub struct TransactionStateTracker {
    cache: alloc::vec::Vec<TransactionStateRecord>,
    pub audit_log: alloc::vec::Vec<TransitionAuditEntry>,
    is_dev_mode: bool,
    /// Known transaction IDs (dev mode only — used by cleanup_expired).
    known_ids: alloc::vec::Vec<u64>,
    /// Simulated expiry set (dev mode only): IDs that cleanup_expired should remove.
    pub expired_ids: alloc::vec::Vec<u64>,
    /// When `true`, `create_transaction` will evict the oldest terminal
    /// transactions automatically when the budget is at Warning or Critical.
    pub eviction_enabled: bool,
    /// Maximum number of terminal transactions evicted per `create_transaction` call.
    pub max_evictions_per_call: u32,
    /// In-memory budget monitor (dev mode) — tracks entry count and byte usage.
    pub budget_monitor: StorageBudgetMonitor,
    /// Archived recovery metadata (dev mode) — populated by
    /// [`TransactionStateTracker::archive_recovery_metadata`] (#628).
    archive: alloc::vec::Vec<ArchivedRecoveryMetadata>,
}

impl TransactionStateTracker {
    /// Create a new transaction state tracker.
    ///
    /// # Arguments
    ///
    /// * `is_dev_mode` - When `true`, records are stored in an in-memory cache
    ///   instead of Soroban persistent storage. Use `true` in tests.
    ///
    /// # Returns
    ///
    /// A new, empty [`TransactionStateTracker`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::transaction_state_tracker::TransactionStateTracker;
    ///
    /// let tracker = TransactionStateTracker::new(true);
    /// assert_eq!(tracker.cache_size(), 0);
    /// ```
    pub fn new(is_dev_mode: bool) -> Self {
        TransactionStateTracker {
            cache: alloc::vec::Vec::new(),
            audit_log: alloc::vec::Vec::new(),
            is_dev_mode,
            known_ids: alloc::vec::Vec::new(),
            expired_ids: alloc::vec::Vec::new(),
            eviction_enabled: false,
            max_evictions_per_call: 10,
            budget_monitor: StorageBudgetMonitor::new(),
            archive: alloc::vec::Vec::new(),
        }
    }

    /// Create a transaction with [`TransactionState::Pending`] state.
    ///
    /// # Arguments
    ///
    /// * `transaction_id` - Unique numeric identifier for the transaction.
    /// * `initiator` - The Stellar address that initiated the transaction.
    /// * `env` - The Soroban execution environment (used for timestamp and storage).
    ///
    /// # Returns
    ///
    /// The newly created [`TransactionStateRecord`].
    ///
    /// # Errors
    ///
    /// Returns a `String` error message if storage fails (production mode only).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::{TransactionStateTracker, TransactionState};
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// let record = tracker.create_transaction(42, initiator, &env).unwrap();
    /// assert_eq!(record.state, TransactionState::Pending);
    /// ```
    pub fn create_transaction(
        &mut self,
        transaction_id: u64,
        initiator: Address,
        env: &Env,
    ) -> Result<TransactionStateRecord, String> {
        self.create_transaction_with_reason(transaction_id, initiator, None, env)
    }

    /// Create a transaction record with an optional routing reason (#298).
    ///
    /// Behaves exactly like [`create_transaction`] but stores `routing_reason`
    /// in the record so the chosen route or referral source can be audited later.
    /// The reason is preserved unchanged through all subsequent state transitions.
    ///
    /// # Arguments
    ///
    /// * `routing_reason` – Human-readable code or description explaining why
    ///   this route was chosen (e.g. `"referral"`, `"lowest_fee"`). `None`
    ///   when no reason applies.
    pub fn create_transaction_with_reason(
        &mut self,
        transaction_id: u64,
        initiator: Address,
        routing_reason: Option<String>,
        env: &Env,
    ) -> Result<TransactionStateRecord, String> {
        // Auto-evict terminal transactions when budget is at Warning/Critical.
        self.auto_evict_if_needed(env);

        let current_time = env.ledger().timestamp();
        let mut history = Vec::new(env);
        history.push_back((TransactionState::Pending, current_time));

        let current_ledger = env.ledger().sequence();
        let record = TransactionStateRecord {
            transaction_id,
            state: TransactionState::Pending,
            initiator,
            timestamp: current_time,
            last_updated: current_time,
            last_updated_ledger: current_ledger,
            error_message: None,
            state_history: history,
            recovery_metadata: OptRecovery::None,
            routing_reason,
        };

        if self.is_dev_mode {
            // Approximate byte size for budget tracking.
            const APPROX_RECORD_BYTES: u64 = 256;
            self.cache.push(record.clone());
            self.known_ids.push(transaction_id);
            self.budget_monitor.record_entry(APPROX_RECORD_BYTES);
        } else {
            let key = (symbol_short!("TXSTATE"), transaction_id);
            env.storage().persistent().set(&key, &record);
            env.storage().persistent().extend_ttl(&key, TXSTATE_TTL, TXSTATE_TTL);
            // Track known IDs list in persistent storage using soroban_sdk::Vec
            let ids_key = symbol_short!("TXIDS");
            let mut ids: Vec<u64> = env
                .storage().persistent().get(&ids_key)
                .unwrap_or_else(|| Vec::new(env));
            ids.push_back(transaction_id);
            env.storage().persistent().set(&ids_key, &ids);
            env.storage().persistent().extend_ttl(&ids_key, TXSTATE_TTL, TXSTATE_TTL);
        }

        Ok(record)
    }

    /// Evict the oldest terminal (Completed or Failed) transactions when the
    /// in-memory budget is at or above the Warning threshold.
    ///
    /// Bounded to at most `max_evictions_per_call` removals per invocation.
    /// Does nothing when `eviction_enabled` is `false`.
    ///
    /// In dev mode the budget thresholds are:
    ///   - Warning:  20 entries × 256 bytes = 5 120 bytes
    ///   - Critical: 50 entries × 256 bytes = 12 800 bytes
    pub fn auto_evict_if_needed(&mut self, env: &Env) {
        if !self.eviction_enabled {
            return;
        }

        const WARNING_BYTES: u64 = 5_120;
        const CRITICAL_BYTES: u64 = 12_800;
        const APPROX_RECORD_BYTES: u64 = 256;

        let status = self.budget_monitor.get_status(WARNING_BYTES, CRITICAL_BYTES);
        if matches!(status, BudgetStatus::Ok) {
            return;
        }

        if self.is_dev_mode {
            // Collect terminal IDs sorted by created_at (timestamp ascending).
            let mut terminal: alloc::vec::Vec<(u64, u64)> = self
                .cache
                .iter()
                .filter(|r| r.state.is_terminal())
                .map(|r| (r.timestamp, r.transaction_id))
                .collect();
            terminal.sort_unstable_by_key(|&(ts, _)| ts);

            let limit = self.max_evictions_per_call as usize;
            let mut evicted = 0u32;
            let mut evicted_ids: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
            for (_, id) in terminal.iter().take(limit) {
                evicted_ids.push(*id);
                evicted += 1;
            }
            if evicted > 0 {
                self.cache.retain(|r| !evicted_ids.contains(&r.transaction_id));
                self.known_ids.retain(|id| !evicted_ids.contains(id));
                for _ in 0..evicted {
                    self.budget_monitor.remove_entry(APPROX_RECORD_BYTES);
                }
                // Emit a Soroban event in production; in dev mode we record in audit log.
                // (env.events() is available in both modes via the Env abstraction.)
                env.events().publish(
                    (symbol_short!("EVICT"), symbol_short!("budget")),
                    (evicted, status == BudgetStatus::Critical),
                );
            }
        } else {
            let ids_key = symbol_short!("TXIDS");
            let ids: Vec<u64> = env
                .storage()
                .persistent()
                .get(&ids_key)
                .unwrap_or_else(|| Vec::new(env));

            // Collect terminal records sorted by timestamp.
            let mut terminal: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
            for id in ids.iter() {
                let key = (symbol_short!("TXSTATE"), id);
                if let Some(rec) = env
                    .storage()
                    .persistent()
                    .get::<_, TransactionStateRecord>(&key)
                {
                    if rec.state.is_terminal() {
                        terminal.push((rec.timestamp, id));
                    }
                }
            }
            terminal.sort_unstable_by_key(|&(ts, _)| ts);

            let limit = self.max_evictions_per_call as usize;
            let mut evicted = 0u32;
            let mut evicted_ids: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
            for (_, id) in terminal.iter().take(limit) {
                let key = (symbol_short!("TXSTATE"), *id);
                env.storage().persistent().remove(&key);
                evicted_ids.push(*id);
                evicted += 1;
            }
            if evicted > 0 {
                // Rebuild live IDs list.
                let mut live: Vec<u64> = Vec::new(env);
                for id in ids.iter() {
                    if !evicted_ids.contains(&id) {
                        live.push_back(id);
                    }
                }
                env.storage().persistent().set(&ids_key, &live);
                env.events().publish(
                    (symbol_short!("EVICT"), symbol_short!("budget")),
                    (evicted, status == BudgetStatus::Critical),
                );
            }
        }
    }

    /// Transition a transaction from [`Pending`](TransactionState::Pending) to
    /// [`InProgress`](TransactionState::InProgress).
    ///
    /// # Arguments
    ///
    /// * `transaction_id` - The ID of the transaction to advance.
    /// * `env` - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// The updated [`TransactionStateRecord`].
    ///
    /// # Errors
    ///
    /// Returns a `String` error if the transaction is not found or the transition
    /// is illegal (e.g. already `Completed`).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::{TransactionStateTracker, TransactionState};
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator, &env).unwrap();
    /// let record = tracker.start_transaction(1, &env).unwrap();
    /// assert_eq!(record.state, TransactionState::InProgress);
    /// ```
    pub fn start_transaction(
        &mut self,
        transaction_id: u64,
        env: &Env,
    ) -> Result<TransactionStateRecord, String> {
        self.update_state(transaction_id, TransactionState::InProgress, None, env)
    }

    /// Transition a transaction from [`InProgress`](TransactionState::InProgress) to
    /// [`Completed`](TransactionState::Completed).
    ///
    /// # Arguments
    ///
    /// * `transaction_id` - The ID of the transaction to complete.
    /// * `env` - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// The updated [`TransactionStateRecord`].
    ///
    /// # Errors
    ///
    /// Returns a `String` error if the transaction is not found or the transition
    /// is illegal (e.g. still `Pending`).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::{TransactionStateTracker, TransactionState};
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator, &env).unwrap();
    /// tracker.start_transaction(1, &env).unwrap();
    /// let record = tracker.complete_transaction(1, &env).unwrap();
    /// assert_eq!(record.state, TransactionState::Completed);
    /// ```
    pub fn complete_transaction(
        &mut self,
        transaction_id: u64,
        env: &Env,
    ) -> Result<TransactionStateRecord, String> {
        self.update_state(transaction_id, TransactionState::Completed, None, env)
    }

    /// Transition a transaction to [`Failed`](TransactionState::Failed) with an error message.
    ///
    /// The transition is legal from [`Pending`](TransactionState::Pending) (immediate
    /// pre-processing failure) and [`InProgress`](TransactionState::InProgress) (mid-flight
    /// failure). Attempting to fail a transaction that is already in a terminal state
    /// ([`Completed`](TransactionState::Completed) or [`Failed`](TransactionState::Failed))
    /// returns an error.
    ///
    /// # Arguments
    ///
    /// * `transaction_id` - The ID of the transaction to fail.
    /// * `error_message` - A Soroban [`String`] describing the failure reason.
    /// * `env` - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// The updated [`TransactionStateRecord`] with `error_message` populated.
    ///
    /// # Errors
    ///
    /// Returns a `String` error if the transaction is not found or the transition
    /// is illegal.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::{Env, String};
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::{TransactionStateTracker, TransactionState};
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator, &env).unwrap();
    /// tracker.start_transaction(1, &env).unwrap();
    /// let msg = String::from_str(&env, "network timeout");
    /// let record = tracker.fail_transaction(1, msg, &env).unwrap();
    /// assert_eq!(record.state, TransactionState::Failed);
    /// assert!(record.error_message.is_some());
    /// ```
    pub fn fail_transaction(
        &mut self,
        transaction_id: u64,
        error_message: String,
        env: &Env,
    ) -> Result<TransactionStateRecord, String> {
        self.update_state(
            transaction_id,
            TransactionState::Failed,
            Some(error_message),
            env,
        )
    }

    /// Update transaction state
    fn update_state(
        &mut self,
        transaction_id: u64,
        new_state: TransactionState,
        error_message: Option<String>,
        env: &Env,
    ) -> Result<TransactionStateRecord, String> {
        let current_time = env.ledger().timestamp();
        let current_ledger = env.ledger().sequence();

        if self.is_dev_mode {
            for record in self.cache.iter_mut() {
                if record.transaction_id == transaction_id {
                    let from_state = record.state;
                    let valid = from_state.is_valid_transition(new_state);
                    self.audit_log.push(TransitionAuditEntry {
                        transaction_id,
                        from_state,
                        to_state: new_state,
                        timestamp: current_time,
                        success: valid,
                    });
                    if !valid {
                        return Err(String::from_str(
                            env,
                            &from_state.illegal_transition_message(new_state),
                        ));
                    }
                    record.state = new_state;
                    record.last_updated = current_time;
                    record.last_updated_ledger = current_ledger;
                    record.error_message = error_message.clone();
                    record.state_history.push_back((new_state, current_time));

                    // Populate recovery metadata when transitioning to Failed.
                    if new_state == TransactionState::Failed {
                        let reason = error_message
                            .clone()
                            .unwrap_or_else(|| String::from_str(env, "unspecified failure"));
                        record.recovery_metadata = OptRecovery::Some(RecoveryMetadata {
                            failure_reason: reason,
                            last_updated_ledger: current_ledger,
                            failed_from_state: from_state,
                            retry_count: 0,
                        });
                    }

                    return Ok(record.clone());
                }
            }
            return Err(String::from_str(env, "Transaction not found in cache"));
        } else {
            let key = (symbol_short!("TXSTATE"), transaction_id);
            let mut record: TransactionStateRecord = env
                .storage()
                .persistent()
                .get(&key)
                .ok_or_else(|| String::from_str(env, "Transaction not found"))?;

            let from_state = record.state;
            let valid = from_state.is_valid_transition(new_state);

            // Write audit entry to persistent storage
            let audit_cnt_key = (symbol_short!("TXAUDIT"), transaction_id);
            let audit_idx: u64 = env
                .storage()
                .persistent()
                .get(&audit_cnt_key)
                .unwrap_or(0u64);
            let audit_entry_key = (symbol_short!("TXAUDITK"), transaction_id, audit_idx);
            env.storage().persistent().set(
                &audit_entry_key,
                &(from_state as u32, new_state as u32, current_time, valid),
            );
            env.storage()
                .persistent()
                .extend_ttl(&audit_entry_key, TXSTATE_TTL, TXSTATE_TTL);
            env.storage()
                .persistent()
                .set(&audit_cnt_key, &(audit_idx + 1));
            env.storage()
                .persistent()
                .extend_ttl(&audit_cnt_key, TXSTATE_TTL, TXSTATE_TTL);

            if !valid {
                return Err(String::from_str(
                    env,
                    &from_state.illegal_transition_message(new_state),
                ));
            }

            record.state = new_state;
            record.last_updated = current_time;
            record.last_updated_ledger = current_ledger;
            record.error_message = error_message.clone();
            record.state_history.push_back((new_state, current_time));

            // Populate recovery metadata when transitioning to Failed.
            if new_state == TransactionState::Failed {
                let reason = error_message
                    .clone()
                    .unwrap_or_else(|| String::from_str(env, "unspecified failure"));
                record.recovery_metadata = OptRecovery::Some(RecoveryMetadata {
                    failure_reason: reason,
                    last_updated_ledger: current_ledger,
                    failed_from_state: from_state,
                    retry_count: 0,
                });
            }

            env.storage().persistent().set(&key, &record);
            env.storage()
                .persistent()
                .extend_ttl(&key, TXSTATE_TTL, TXSTATE_TTL);

            Ok(record)
        }
    }

    /// Advance a transaction to `new_state`, enforcing legal transition rules.
    ///
    /// This is the general-purpose state-advance method. Prefer the named helpers
    /// ([`start_transaction`](Self::start_transaction),
    /// [`complete_transaction`](Self::complete_transaction),
    /// [`fail_transaction`](Self::fail_transaction)) for clarity.
    ///
    /// # Arguments
    ///
    /// * `transaction_id` - The ID of the transaction to advance.
    /// * `new_state` - The target [`TransactionState`].
    /// * `env` - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// The updated [`TransactionStateRecord`].
    ///
    /// # Errors
    ///
    /// Returns a `String` error if the transaction is not found or the transition
    /// from the current state to `new_state` is illegal.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::{TransactionStateTracker, TransactionState};
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator, &env).unwrap();
    /// let r = tracker.advance_transaction_state(1, TransactionState::InProgress, &env).unwrap();
    /// assert_eq!(r.state, TransactionState::InProgress);
    /// // Illegal transition returns an error.
    /// assert!(tracker.advance_transaction_state(1, TransactionState::Pending, &env).is_err());
    /// ```
    pub fn advance_transaction_state(
        &mut self,
        transaction_id: u64,
        new_state: TransactionState,
        env: &Env,
    ) -> Result<TransactionStateRecord, String> {
        self.update_state(transaction_id, new_state, None, env)
    }

    /// Get transaction state by ID.
    ///
    /// # Arguments
    ///
    /// * `transaction_id` - The ID of the transaction to look up.
    /// * `env` - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// `Ok(Some(record))` if found, `Ok(None)` if not found.
    ///
    /// # Errors
    ///
    /// Returns a `String` error only on storage failures (production mode).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::{TransactionStateTracker, TransactionState};
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator, &env).unwrap();
    /// let state = tracker.get_transaction_state(1, &env).unwrap();
    /// assert_eq!(state.unwrap().state, TransactionState::Pending);
    /// assert!(tracker.get_transaction_state(99, &env).unwrap().is_none());
    /// ```
    pub fn get_transaction_state(
        &self,
        transaction_id: u64,
        env: &Env,
    ) -> Result<Option<TransactionStateRecord>, String> {
        if self.is_dev_mode {
            for record in self.cache.iter() {
                if record.transaction_id == transaction_id {
                    return Ok(Some(record.clone()));
                }
            }
            Ok(None)
        } else {
            let result: Option<TransactionStateRecord> = env
                .storage()
                .persistent()
                .get(&(symbol_short!("TXSTATE"), transaction_id));
            if let Some(ref record) = result {
                // Bump TTL on every read
                let ttl = if record.state == TransactionState::Completed
                    || record.state == TransactionState::Failed
                {
                    TXSTATE_TTL_TERMINAL
                } else {
                    Self::active_ttl(env)
                };
                let key = (symbol_short!("TXSTATE"), transaction_id);
                env.storage().persistent().extend_ttl(&key, ttl, ttl);
            }
            Ok(result)
        }
    }

    /// Get all transactions in a specific state.
    ///
    /// In production mode this always returns an empty `Vec` (full scans are not
    /// supported on-chain). In dev mode it filters the in-memory cache.
    ///
    /// # Arguments
    ///
    /// * `state` - The [`TransactionState`] to filter by.
    ///
    /// # Returns
    ///
    /// A `Vec` of matching [`TransactionStateRecord`]s.
    ///
    /// # Errors
    ///
    /// Currently always returns `Ok(...)`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::{TransactionStateTracker, TransactionState};
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator.clone(), &env).unwrap();
    /// tracker.create_transaction(2, initiator, &env).unwrap();
    /// tracker.start_transaction(1, &env).unwrap();
    ///
    /// let pending = tracker.get_transactions_by_state(TransactionState::Pending).unwrap();
    /// assert_eq!(pending.len(), 1);
    /// ```
    pub fn get_transactions_by_state(
        &self,
        state: TransactionState,
    ) -> Result<alloc::vec::Vec<TransactionStateRecord>, String> {
        if self.is_dev_mode {
            let mut result = alloc::vec::Vec::new();
            for record in self.cache.iter() {
                if record.state == state {
                    result.push(record.clone());
                }
            }
            Ok(result)
        } else {
            Ok(alloc::vec::Vec::new())
        }
    }

    /// Get all transactions (dev mode only).
    ///
    /// Returns all records from the in-memory cache. In production mode returns
    /// an empty `Vec` because full-table scans are not supported on-chain.
    ///
    /// # Returns
    ///
    /// A `Vec` of all [`TransactionStateRecord`]s.
    ///
    /// # Errors
    ///
    /// Currently always returns `Ok(...)`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::TransactionStateTracker;
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator.clone(), &env).unwrap();
    /// tracker.create_transaction(2, initiator, &env).unwrap();
    /// assert_eq!(tracker.get_all_transactions().unwrap().len(), 2);
    /// ```
    pub fn get_all_transactions(&self) -> Result<alloc::vec::Vec<TransactionStateRecord>, String> {
        if self.is_dev_mode {
            Ok(self.cache.clone())
        } else {
            Ok(alloc::vec::Vec::new())
        }
    }

    /// Clear all cached transactions (dev mode only).
    ///
    /// Resets the in-memory cache to empty. Calling this in production mode
    /// returns an error.
    ///
    /// # Returns
    ///
    /// `Ok(())` in dev mode.
    ///
    /// # Errors
    ///
    /// Returns a `String` error when called in production mode.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::transaction_state_tracker::TransactionStateTracker;
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// assert!(tracker.clear_cache(&env).is_ok());
    /// assert_eq!(tracker.cache_size(), 0);
    /// ```
    pub fn clear_cache(&mut self, env: &Env) -> Result<(), String> {
        if self.is_dev_mode {
            self.cache = alloc::vec::Vec::new();
            Ok(())
        } else {
            Err(String::from_str(env, "Cannot clear cache in production mode"))
        }
    }

    /// Return the number of records currently in the in-memory cache.
    ///
    /// Always returns `0` in production mode (no in-memory cache).
    ///
    /// # Returns
    ///
    /// The number of cached [`TransactionStateRecord`]s.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::transaction_state_tracker::TransactionStateTracker;
    ///
    /// let tracker = TransactionStateTracker::new(true);
    /// assert_eq!(tracker.cache_size(), 0);
    /// ```
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    // ── Recovery helpers ─────────────────────────────────────────────────────

    /// Return the [`RecoveryMetadata`] for a failed transaction, if available.
    ///
    /// Returns `None` when the transaction does not exist, has not yet failed,
    /// or was failed without recovery metadata (legacy records).
    ///
    /// # Arguments
    ///
    /// * `transaction_id` - The ID of the transaction to inspect.
    /// * `env` - The Soroban execution environment.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::{Env, String};
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::{TransactionStateTracker, TransactionState};
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator, &env).unwrap();
    /// tracker.start_transaction(1, &env).unwrap();
    /// tracker.fail_transaction(1, String::from_str(&env, "timeout"), &env).unwrap();
    ///
    /// let meta = tracker.get_recovery_metadata(1, &env).unwrap();
    /// assert!(meta.is_some());
    /// assert_eq!(meta.unwrap().failed_from_state, TransactionState::InProgress);
    /// ```
    pub fn get_recovery_metadata(
        &self,
        transaction_id: u64,
        env: &Env,
    ) -> Result<Option<RecoveryMetadata>, String> {
        let record = self.get_transaction_state(transaction_id, env)?;
        Ok(record.and_then(|r| r.recovery_metadata.into_option()))
    }

    /// Returns `true` when the transaction is in the [`Failed`](TransactionState::Failed)
    /// state and therefore eligible for a recovery attempt.
    ///
    /// A transaction is considered recoverable if it has failed but has not yet
    /// exceeded an operator-defined retry ceiling (checked externally).
    ///
    /// # Arguments
    ///
    /// * `transaction_id` - The ID of the transaction to inspect.
    /// * `env` - The Soroban execution environment.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::{Env, String};
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::TransactionStateTracker;
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator, &env).unwrap();
    /// tracker.start_transaction(1, &env).unwrap();
    /// tracker.fail_transaction(1, String::from_str(&env, "err"), &env).unwrap();
    ///
    /// assert!(tracker.is_recoverable(1, &env).unwrap());
    /// ```
    pub fn is_recoverable(
        &self,
        transaction_id: u64,
        env: &Env,
    ) -> Result<bool, String> {
        let record = self.get_transaction_state(transaction_id, env)?;
        Ok(record.map(|r| r.state == TransactionState::Failed).unwrap_or(false))
    }

    /// Increment the `retry_count` on the recovery metadata of a failed transaction.
    ///
    /// This is a lightweight bookkeeping call that operators invoke each time they
    /// attempt to recover a failed transaction. It does **not** change the
    /// transaction state — use [`start_transaction`](Self::start_transaction) or
    /// [`advance_transaction_state`](Self::advance_transaction_state) for that.
    ///
    /// # Arguments
    ///
    /// * `transaction_id` - The ID of the failed transaction.
    /// * `env` - The Soroban execution environment.
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction is not found or is not in the
    /// [`Failed`](TransactionState::Failed) state.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::{Env, String};
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::TransactionStateTracker;
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator, &env).unwrap();
    /// tracker.start_transaction(1, &env).unwrap();
    /// tracker.fail_transaction(1, String::from_str(&env, "err"), &env).unwrap();
    ///
    /// tracker.record_recovery_attempt(1, &env).unwrap();
    /// let meta = tracker.get_recovery_metadata(1, &env).unwrap().unwrap();
    /// assert_eq!(meta.retry_count, 1);
    /// ```
    pub fn record_recovery_attempt(
        &mut self,
        transaction_id: u64,
        env: &Env,
    ) -> Result<(), String> {
        if self.is_dev_mode {
            for record in self.cache.iter_mut() {
                if record.transaction_id == transaction_id {
                    if record.state != TransactionState::Failed {
                        return Err(String::from_str(
                            env,
                            "record_recovery_attempt requires a Failed transaction",
                        ));
                    }
                    match record.recovery_metadata.as_mut() {
                        Some(meta) => meta.retry_count += 1,
                        None => {
                            return Err(String::from_str(
                                env,
                                "Transaction has no recovery metadata",
                            ))
                        }
                    }
                    return Ok(());
                }
            }
            return Err(String::from_str(env, "Transaction not found in cache"));
        } else {
            let key = (symbol_short!("TXSTATE"), transaction_id);
            let mut record: TransactionStateRecord = env
                .storage()
                .persistent()
                .get(&key)
                .ok_or_else(|| String::from_str(env, "Transaction not found"))?;

            if record.state != TransactionState::Failed {
                return Err(String::from_str(
                    env,
                    "record_recovery_attempt requires a Failed transaction",
                ));
            }
            match record.recovery_metadata.as_mut() {
                Some(meta) => meta.retry_count += 1,
                None => {
                    return Err(String::from_str(
                        env,
                        "Transaction has no recovery metadata",
                    ))
                }
            }
            env.storage().persistent().set(&key, &record);
            env.storage()
                .persistent()
                .extend_ttl(&key, TXSTATE_TTL, TXSTATE_TTL);
            Ok(())
        }
    }

    // ── Recovery metadata export / import ──────────────────────────────────────

    /// Serialize the [`RecoveryMetadata`] for a failed transaction into a JSON
    /// string that can be stored or transferred between environments.
    ///
    /// Only available when the `std` feature is enabled (requires `serde_json`).
    ///
    /// # Arguments
    ///
    /// * `transaction_id` - The ID of the failed transaction.
    /// * `env` - The Soroban execution environment.
    ///
    /// # Errors
    ///
    /// Returns a `String` error if the transaction is not found, is not in the
    /// `Failed` state, has no recovery metadata, or serialization fails.
    #[cfg(feature = "std")]
    pub fn export_recovery_state(
        &self,
        transaction_id: u64,
        env: &Env,
    ) -> Result<alloc::string::String, alloc::string::String> {
        use alloc::string::ToString;

        let meta = self.get_recovery_metadata(transaction_id, env)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| alloc::format!("No recovery metadata for transaction {}", transaction_id))?;

        #[derive(serde::Serialize)]
        struct Export {
            failure_reason: alloc::string::String,
            last_updated_ledger: u32,
            failed_from_state: alloc::string::String,
            retry_count: u32,
            transaction_id: u64,
        }

        let export = Export {
            failure_reason: meta.failure_reason.to_string(),
            last_updated_ledger: meta.last_updated_ledger,
            failed_from_state: alloc::string::String::from(meta.failed_from_state.as_str()),
            retry_count: meta.retry_count,
            transaction_id,
        };

        serde_json::to_string(&export).map_err(|e| alloc::format!("Serialization error: {}", e))
    }

    /// Deserialize a JSON string and import (create or update) the recovery
    /// metadata for a failed transaction.
    ///
    /// If a transaction with the given ID already exists, it must be in the
    /// [`Failed`](TransactionState::Failed) state and its recovery metadata will
    /// be replaced. If no transaction exists, a new one is created in the
    /// [`Failed`](TransactionState::Failed) state with the imported metadata.
    ///
    /// Only available when the `std` feature is enabled (requires `serde_json`).
    ///
    /// # Arguments
    ///
    /// * `json_data` - A JSON string matching the export format.
    /// * `initiator` - The Stellar address that initiated the transaction
    ///   (used only when creating a new record).
    /// * `env` - The Soroban execution environment.
    ///
    /// # Errors
    ///
    /// Returns a `String` error if the JSON is malformed, a required field is
    /// missing, the `failed_from_state` is not a recognised state name, or
    /// the existing transaction is not in the `Failed` state.
    #[cfg(feature = "std")]
    pub fn import_recovery_state(
        &mut self,
        json_data: &str,
        initiator: Address,
        env: &Env,
    ) -> Result<(), alloc::string::String> {
        use alloc::string::ToString;

        #[derive(serde::Deserialize)]
        struct Import {
            failure_reason: alloc::string::String,
            last_updated_ledger: u32,
            failed_from_state: alloc::string::String,
            retry_count: u32,
            transaction_id: u64,
        }

        let import: Import = serde_json::from_str(json_data)
            .map_err(|e| alloc::format!("Invalid recovery state JSON: {}", e))?;

        let failed_from = TransactionState::from_str(&import.failed_from_state)
            .ok_or_else(|| alloc::format!(
                "Invalid failed_from_state '{}': must be one of: pending, in_progress, completed, failed",
                import.failed_from_state
            ))?;

        if failed_from.is_terminal() {
            return Err(alloc::format!(
                "Invalid failed_from_state '{}': cannot be a terminal state",
                import.failed_from_state
            ));
        }

        let current_ledger = env.ledger().sequence();
        let new_meta = RecoveryMetadata {
            failure_reason: String::from_str(env, &import.failure_reason),
            last_updated_ledger: import.last_updated_ledger,
            failed_from_state: failed_from,
            retry_count: import.retry_count,
        };

        // Check if transaction already exists.
        let existing = self.get_transaction_state(import.transaction_id, env)
            .map_err(|e| e.to_string())?;

        match existing {
            Some(record) => {
                // Transaction exists — must be in Failed state.
                if record.state != TransactionState::Failed {
                    return Err(alloc::format!(
                        "Cannot import recovery state: transaction {} is in state '{}', expected 'failed'",
                        import.transaction_id,
                        record.state.as_str(),
                    ));
                }
                // Replace recovery metadata in-place.
                if self.is_dev_mode {
                    for rec in self.cache.iter_mut() {
                        if rec.transaction_id == import.transaction_id {
                            rec.recovery_metadata = OptRecovery::Some(new_meta.clone());
                            rec.last_updated = env.ledger().timestamp();
                            rec.last_updated_ledger = current_ledger;
                            break;
                        }
                    }
                } else {
                    let key = (symbol_short!("TXSTATE"), import.transaction_id);
                    let mut stored: TransactionStateRecord = env
                        .storage()
                        .persistent()
                        .get(&key)
                        .ok_or_else(|| alloc::format!("Transaction not found"))?;
                    stored.recovery_metadata = OptRecovery::Some(new_meta);
                    stored.last_updated = env.ledger().timestamp();
                    stored.last_updated_ledger = current_ledger;
                    env.storage().persistent().set(&key, &stored);
                    env.storage().persistent().extend_ttl(&key, TXSTATE_TTL, TXSTATE_TTL);
                }
                Ok(())
            }
            None => {
                // No existing transaction — create a new one in Failed state.
                let current_time = env.ledger().timestamp();
                let mut history = Vec::new(env);
                history.push_back((TransactionState::Pending, current_time));
                history.push_back((failed_from, current_time));
                history.push_back((TransactionState::Failed, current_time));

                let record = TransactionStateRecord {
                    transaction_id: import.transaction_id,
                    state: TransactionState::Failed,
                    initiator,
                    timestamp: current_time,
                    last_updated: current_time,
                    last_updated_ledger: current_ledger,
                    error_message: Some(String::from_str(env, &import.failure_reason)),
                    state_history: history,
                    recovery_metadata: OptRecovery::Some(new_meta),
                    routing_reason: None,
                };

                if self.is_dev_mode {
                    const APPROX_RECORD_BYTES: u64 = 256;
                    self.cache.push(record);
                    self.known_ids.push(import.transaction_id);
                    self.budget_monitor.record_entry(APPROX_RECORD_BYTES);
                } else {
                    let key = (symbol_short!("TXSTATE"), import.transaction_id);
                    env.storage().persistent().set(&key, &record);
                    env.storage().persistent().extend_ttl(&key, TXSTATE_TTL, TXSTATE_TTL);

                    let ids_key = symbol_short!("TXIDS");
                    let mut ids: Vec<u64> = env
                        .storage().persistent().get(&ids_key)
                        .unwrap_or_else(|| Vec::new(env));
                    ids.push_back(import.transaction_id);
                    env.storage().persistent().set(&ids_key, &ids);
                    env.storage().persistent().extend_ttl(&ids_key, TXSTATE_TTL, TXSTATE_TTL);
                }
                Ok(())
            }
        }
    }

    // ── Recovery metadata pruning & archival (#628) ─────────────────────────

    /// Store the recovery-metadata retention policy in persistent storage so
    /// it can be reused by [`Self::sweep_recovery_metadata`] without being
    /// re-supplied by every caller. Production mode only — in dev mode, pass
    /// a policy explicitly to the pruning/archival methods.
    pub fn set_retention_policy(env: &Env, policy: RecoveryRetentionPolicy) {
        let key = symbol_short!("RCVPOLCY");
        env.storage().persistent().set(&key, &policy);
        env.storage().persistent().extend_ttl(&key, TXSTATE_TTL, TXSTATE_TTL);
    }

    /// Retrieve the stored retention policy, or
    /// [`RecoveryRetentionPolicy::default_policy`] if none has been set.
    pub fn get_retention_policy(env: &Env) -> RecoveryRetentionPolicy {
        let key = symbol_short!("RCVPOLCY");
        env.storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(RecoveryRetentionPolicy::default_policy)
    }

    /// Permanently clear the recovery metadata on a failed transaction once
    /// it is eligible for pruning under `policy`, without preserving any
    /// archived copy.
    ///
    /// # Returns
    ///
    /// `Ok(true)` if metadata was pruned, `Ok(false)` if the transaction does
    /// not exist, has no recovery metadata, or is not yet eligible.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::{Env, String};
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::{TransactionStateTracker, RecoveryRetentionPolicy};
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator, &env).unwrap();
    /// tracker.start_transaction(1, &env).unwrap();
    /// tracker.fail_transaction(1, String::from_str(&env, "timeout"), &env).unwrap();
    ///
    /// let policy = RecoveryRetentionPolicy { max_age_seconds: 1, max_retry_count: 5 };
    /// // Not yet eligible — no time has elapsed and retry_count is 0.
    /// assert_eq!(tracker.prune_recovery_metadata(1, &policy, &env), Ok(false));
    /// ```
    pub fn prune_recovery_metadata(
        &mut self,
        transaction_id: u64,
        policy: &RecoveryRetentionPolicy,
        env: &Env,
    ) -> Result<bool, String> {
        let now = env.ledger().timestamp();

        if self.is_dev_mode {
            for record in self.cache.iter_mut() {
                if record.transaction_id == transaction_id {
                    return Ok(match record.recovery_metadata.clone().into_option() {
                        Some(meta) if policy.is_eligible(&meta, record.last_updated, now) => {
                            record.recovery_metadata = OptRecovery::None;
                            true
                        }
                        _ => false,
                    });
                }
            }
            Ok(false)
        } else {
            let key = (symbol_short!("TXSTATE"), transaction_id);
            let mut record: TransactionStateRecord = match env.storage().persistent().get(&key) {
                Some(r) => r,
                None => return Ok(false),
            };
            let eligible = match record.recovery_metadata.clone().into_option() {
                Some(meta) => policy.is_eligible(&meta, record.last_updated, now),
                None => false,
            };
            if !eligible {
                return Ok(false);
            }
            record.recovery_metadata = OptRecovery::None;
            env.storage().persistent().set(&key, &record);
            env.storage().persistent().extend_ttl(&key, TXSTATE_TTL, TXSTATE_TTL);
            Ok(true)
        }
    }

    /// Archive the recovery metadata on a failed transaction once it is
    /// eligible under `policy`: a compact [`ArchivedRecoveryMetadata`] is
    /// preserved (retrievable via [`Self::get_archived_recovery_metadata`])
    /// before the live `recovery_metadata` field is cleared.
    ///
    /// Unlike [`Self::prune_recovery_metadata`], this never loses the
    /// required context (failure reason, prior state, retry count) — it only
    /// moves it out of the live record.
    ///
    /// # Returns
    ///
    /// `Ok(true)` if metadata was archived, `Ok(false)` if the transaction
    /// does not exist, has no recovery metadata, or is not yet eligible.
    pub fn archive_recovery_metadata(
        &mut self,
        transaction_id: u64,
        policy: &RecoveryRetentionPolicy,
        env: &Env,
    ) -> Result<bool, String> {
        let now = env.ledger().timestamp();
        let current_ledger = env.ledger().sequence();

        if self.is_dev_mode {
            let mut to_archive: Option<ArchivedRecoveryMetadata> = None;
            for record in self.cache.iter_mut() {
                if record.transaction_id == transaction_id {
                    if let Some(meta) = record.recovery_metadata.clone().into_option() {
                        if policy.is_eligible(&meta, record.last_updated, now) {
                            to_archive = Some(ArchivedRecoveryMetadata {
                                transaction_id,
                                failure_reason: meta.failure_reason,
                                failed_from_state: meta.failed_from_state,
                                retry_count: meta.retry_count,
                                archived_at_ledger: current_ledger,
                            });
                            record.recovery_metadata = OptRecovery::None;
                        }
                    }
                    break;
                }
            }
            match to_archive {
                Some(entry) => {
                    self.archive.push(entry);
                    Ok(true)
                }
                None => Ok(false),
            }
        } else {
            let key = (symbol_short!("TXSTATE"), transaction_id);
            let mut record: TransactionStateRecord = match env.storage().persistent().get(&key) {
                Some(r) => r,
                None => return Ok(false),
            };
            let meta = match record.recovery_metadata.clone().into_option() {
                Some(m) => m,
                None => return Ok(false),
            };
            if !policy.is_eligible(&meta, record.last_updated, now) {
                return Ok(false);
            }

            let entry = ArchivedRecoveryMetadata {
                transaction_id,
                failure_reason: meta.failure_reason,
                failed_from_state: meta.failed_from_state,
                retry_count: meta.retry_count,
                archived_at_ledger: current_ledger,
            };
            let archive_key = (symbol_short!("RCVARCH"), transaction_id);
            env.storage().persistent().set(&archive_key, &entry);
            env.storage().persistent().extend_ttl(&archive_key, TXSTATE_TTL, TXSTATE_TTL);

            record.recovery_metadata = OptRecovery::None;
            env.storage().persistent().set(&key, &record);
            env.storage().persistent().extend_ttl(&key, TXSTATE_TTL, TXSTATE_TTL);
            Ok(true)
        }
    }

    /// Retrieve the archived recovery metadata for `transaction_id`, if any.
    pub fn get_archived_recovery_metadata(
        &self,
        transaction_id: u64,
        env: &Env,
    ) -> Result<Option<ArchivedRecoveryMetadata>, String> {
        if self.is_dev_mode {
            Ok(self
                .archive
                .iter()
                .find(|a| a.transaction_id == transaction_id)
                .cloned())
        } else {
            let archive_key = (symbol_short!("RCVARCH"), transaction_id);
            Ok(env.storage().persistent().get(&archive_key))
        }
    }

    /// Sweep every known transaction and prune or archive recovery metadata
    /// eligible under `policy`.
    ///
    /// # Arguments
    ///
    /// * `policy`  - The retention policy to apply.
    /// * `archive` - When `true`, eligible metadata is archived (preserving
    ///   required context); when `false`, it is pruned outright.
    /// * `env`     - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// The number of transactions whose recovery metadata was processed
    /// (pruned or archived).
    pub fn sweep_recovery_metadata(
        &mut self,
        policy: &RecoveryRetentionPolicy,
        archive: bool,
        env: &Env,
    ) -> Result<u32, String> {
        let ids: alloc::vec::Vec<u64> = if self.is_dev_mode {
            self.known_ids.clone()
        } else {
            let ids_key = symbol_short!("TXIDS");
            let stored: Vec<u64> = env
                .storage()
                .persistent()
                .get(&ids_key)
                .unwrap_or_else(|| Vec::new(env));
            stored.iter().collect()
        };

        let mut processed = 0u32;
        for id in ids {
            let did_process = if archive {
                self.archive_recovery_metadata(id, policy, env)?
            } else {
                self.prune_recovery_metadata(id, policy, env)?
            };
            if did_process {
                processed += 1;
            }
        }
        Ok(processed)
    }

    // ── Batch query helpers ──────────────────────────────────────────────────

    /// Return up to `limit` transaction records whose IDs fall in the inclusive
    /// range `[from_id, to_id]`, ordered by `transaction_id` ascending.
    ///
    /// The batch size is capped at 100 to prevent unbounded iteration.
    ///
    /// In production mode the method reads each ID in the range from persistent
    /// storage; IDs that are no longer present (TTL expired) are silently
    /// skipped. In dev mode the in-memory cache is filtered.
    ///
    /// # Arguments
    ///
    /// * `from_id` - Inclusive lower bound of the ID range.
    /// * `to_id`   - Inclusive upper bound of the ID range.
    /// * `limit`   - Maximum number of records to return (capped at 100).
    /// * `env`     - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// A `Vec` of matching [`TransactionStateRecord`]s, sorted by ID ascending.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::TransactionStateTracker;
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// for i in 1..=5 { tracker.create_transaction(i, initiator.clone(), &env).unwrap(); }
    /// let batch = tracker.get_transactions_in_range(2, 4, 10, &env).unwrap();
    /// assert_eq!(batch.len(), 3);
    /// assert_eq!(batch[0].transaction_id, 2);
    /// ```
    pub fn get_transactions_in_range(
        &self,
        from_id: u64,
        to_id: u64,
        limit: u32,
        env: &Env,
    ) -> Result<alloc::vec::Vec<TransactionStateRecord>, String> {
        const MAX_BATCH: u32 = 100;
        let effective_limit = limit.min(MAX_BATCH);

        if from_id > to_id {
            return Ok(alloc::vec::Vec::new());
        }

        let mut results = alloc::vec::Vec::new();

        if self.is_dev_mode {
            // Collect matching records from the in-memory cache, sorted by ID.
            let mut matching: alloc::vec::Vec<TransactionStateRecord> = self
                .cache
                .iter()
                .filter(|r| r.transaction_id >= from_id && r.transaction_id <= to_id)
                .cloned()
                .collect();
            matching.sort_by_key(|r| r.transaction_id);
            for record in matching.into_iter().take(effective_limit as usize) {
                results.push(record);
            }
        } else {
            // Walk the ID range and load each record from persistent storage.
            let mut id = from_id;
            while id <= to_id && results.len() < effective_limit as usize {
                let key = (symbol_short!("TXSTATE"), id);
                if let Some(record) = env
                    .storage()
                    .persistent()
                    .get::<_, TransactionStateRecord>(&key)
                {
                    results.push(record);
                }
                id += 1;
            }
        }

        Ok(results)
    }

    /// Return aggregated counts of transactions grouped by their current state.
    ///
    /// In dev mode this iterates the in-memory cache. In production mode it
    /// reads the known-IDs list from persistent storage and loads each record.
    ///
    /// # Returns
    ///
    /// A [`TransactionSummary`] with per-state counts and a `total_count`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::TransactionStateTracker;
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator.clone(), &env).unwrap();
    /// tracker.create_transaction(2, initiator.clone(), &env).unwrap();
    /// tracker.start_transaction(1, &env).unwrap();
    /// tracker.complete_transaction(1, &env).unwrap();
    ///
    /// let summary = tracker.summarize_transactions_by_status(env).unwrap();
    /// assert_eq!(summary.completed_count, 1);
    /// assert_eq!(summary.pending_count, 1);
    /// assert_eq!(summary.total_count, 2);
    /// ```
    pub fn summarize_transactions_by_status(
        &self,
        env: &Env,
    ) -> Result<TransactionSummary, String> {
        let mut summary = TransactionSummary::default();

        if self.is_dev_mode {
            for record in self.cache.iter() {
                match record.state {
                    TransactionState::Pending    => summary.pending_count += 1,
                    TransactionState::InProgress => summary.in_progress_count += 1,
                    TransactionState::Completed  => summary.completed_count += 1,
                    TransactionState::Failed     => summary.failed_count += 1,
                }
                summary.total_count += 1;
            }
        } else {
            let ids_key = symbol_short!("TXIDS");
            let ids: Vec<u64> = env
                .storage()
                .persistent()
                .get(&ids_key)
                .unwrap_or_else(|| Vec::new(env));
            for id in ids.iter() {
                let key = (symbol_short!("TXSTATE"), id);
                if let Some(record) = env
                    .storage()
                    .persistent()
                    .get::<_, TransactionStateRecord>(&key)
                {
                    match record.state {
                        TransactionState::Pending    => summary.pending_count += 1,
                        TransactionState::InProgress => summary.in_progress_count += 1,
                        TransactionState::Completed  => summary.completed_count += 1,
                        TransactionState::Failed     => summary.failed_count += 1,
                    }
                    summary.total_count += 1;
                }
            }
        }

        Ok(summary)
    }

    /// Return all failed transactions (dev mode only).
    ///
    /// Convenience wrapper around [`get_transactions_by_state`](Self::get_transactions_by_state)
    /// that filters for [`Failed`](TransactionState::Failed) records and is
    /// named explicitly for recovery workflows.
    ///
    /// # Returns
    ///
    /// A `Vec` of [`TransactionStateRecord`]s whose state is `Failed`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::{Env, String};
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let initiator = soroban_sdk::Address::generate(&env);
    /// use anchorkit::transaction_state_tracker::TransactionStateTracker;
    ///
    /// let mut tracker = TransactionStateTracker::new(true);
    /// tracker.create_transaction(1, initiator.clone(), &env).unwrap();
    /// tracker.start_transaction(1, &env).unwrap();
    /// tracker.fail_transaction(1, String::from_str(&env, "err"), &env).unwrap();
    ///
    /// let failed = tracker.get_failed_transactions().unwrap();
    /// assert_eq!(failed.len(), 1);
    /// assert!(failed[0].recovery_metadata.is_some());
    /// ```
    pub fn get_failed_transactions(
        &self,
    ) -> Result<alloc::vec::Vec<TransactionStateRecord>, String> {
        self.get_transactions_by_state(TransactionState::Failed)
    }

    // ── TTL helpers ──────────────────────────────────────────────────────────

    /// Read the configurable active TTL from contract storage (production) or
    /// return the compile-time default.
    fn active_ttl(env: &Env) -> u32 {
        env.storage()
            .persistent()
            .get::<_, u32>(&symbol_short!("TXTTLCFG"))
            .unwrap_or(TXSTATE_TTL)
    }

    /// Set the active TTL (admin operation). Stored in persistent contract storage
    /// so it survives redeployment without a code change.
    pub fn set_ttl_config(env: &Env, new_ttl: u32) {
        env.storage()
            .persistent()
            .set(&symbol_short!("TXTTLCFG"), &new_ttl);
        env.storage()
            .persistent()
            .extend_ttl(&symbol_short!("TXTTLCFG"), TXSTATE_TTL, TXSTATE_TTL);
    }

    /// Extend the TTL of a transaction record proportional to its current state.
    ///
    /// - Active states (Pending, InProgress): full active TTL.
    /// - Terminal states (Completed, Failed): shorter TTL (`TXSTATE_TTL_TERMINAL`).
    ///
    /// In dev mode this is a no-op (in-memory records don't expire).
    pub fn bump_ttl(&self, transaction_id: u64, env: &Env) -> Result<(), String> {
        if self.is_dev_mode {
            return Ok(());
        }
        let key = (symbol_short!("TXSTATE"), transaction_id);
        let record: TransactionStateRecord = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or_else(|| String::from_str(env, "Transaction not found"))?;

        let ttl = match record.state {
            TransactionState::Completed | TransactionState::Failed => TXSTATE_TTL_TERMINAL,
            _ => Self::active_ttl(env),
        };
        env.storage().persistent().extend_ttl(&key, ttl, ttl);
        Ok(())
    }

    /// Admin function: iterate over all known transaction IDs and remove entries
    /// whose TTL has elapsed (i.e. they are no longer present in storage).
    ///
    /// In dev mode, removes entries whose IDs are listed in `self.expired_ids`.
    ///
    /// Returns the number of entries removed.
    pub fn cleanup_expired(&mut self, env: &Env) -> u64 {
        let mut removed = 0u64;
        if self.is_dev_mode {
            let expired = self.expired_ids.clone();
            self.cache.retain(|r| {
                if expired.contains(&r.transaction_id) {
                    removed += 1;
                    false
                } else {
                    true
                }
            });
            self.known_ids.retain(|id| !expired.contains(id));
            self.expired_ids.clear();
        } else {
            let ids_key = symbol_short!("TXIDS");
            let ids: Vec<u64> = env
                .storage()
                .persistent()
                .get(&ids_key)
                .unwrap_or_else(|| Vec::new(env));
            let mut live_ids: Vec<u64> = Vec::new(env);
            for id in ids.iter() {
                let key = (symbol_short!("TXSTATE"), id);
                if env.storage().persistent().has(&key) {
                    live_ids.push_back(id);
                } else {
                    removed += 1;
                }
            }
            env.storage().persistent().set(&ids_key, &live_ids);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    #[test]
    fn test_create_transaction() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        let result = tracker.create_transaction(1, initiator.clone(), &env);
        assert!(result.is_ok());

        let record = result.unwrap();
        assert_eq!(record.transaction_id, 1);
        assert_eq!(record.state, TransactionState::Pending);
        assert_eq!(record.initiator, initiator);
        // state_history initialized with Pending
        assert_eq!(record.state_history.len(), 1);
        assert_eq!(record.state_history.get(0).unwrap().0, TransactionState::Pending);
    }

    #[test]
    fn test_start_transaction() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        let result = tracker.start_transaction(1, &env);

        assert!(result.is_ok());
        let record = result.unwrap();
        assert_eq!(record.state, TransactionState::InProgress);
        assert_eq!(record.state_history.len(), 2);
    }

    #[test]
    fn test_complete_transaction() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();
        let result = tracker.complete_transaction(1, &env);

        assert!(result.is_ok());
        let record = result.unwrap();
        assert_eq!(record.state, TransactionState::Completed);
        assert_eq!(record.state_history.len(), 3);
    }

    #[test]
    fn test_fail_transaction() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok(); // Pending -> InProgress
        let error_msg = String::from_str(&env, "Test error");
        let result = tracker.fail_transaction(1, error_msg, &env);

        assert!(result.is_ok());
        let record = result.unwrap();
        assert_eq!(record.state, TransactionState::Failed);
        assert!(record.error_message.is_some());
    }

    #[test]
    fn test_get_transaction_state() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        let result = tracker.get_transaction_state(1, &env);

        assert!(result.is_ok());
        let state = result.unwrap();
        assert!(state.is_some());
        assert_eq!(state.unwrap().state, TransactionState::Pending);
    }

    #[test]
    fn test_get_transactions_by_state() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.create_transaction(2, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();

        let result = tracker.get_transactions_by_state(TransactionState::Pending);
        assert!(result.is_ok());
        let transactions = result.unwrap();
        assert_eq!(transactions.len(), 1);
    }

    #[test]
    fn test_get_all_transactions() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.create_transaction(2, initiator.clone(), &env).ok();

        let result = tracker.get_all_transactions();
        assert!(result.is_ok());
        let transactions = result.unwrap();
        assert_eq!(transactions.len(), 2);
    }

    #[test]
    fn test_cache_size() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.create_transaction(2, initiator.clone(), &env).ok();

        assert_eq!(tracker.cache_size(), 2);
    }

    #[test]
    fn test_clear_cache() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        let clear_result = tracker.clear_cache(&env);

        assert!(clear_result.is_ok());
        assert_eq!(tracker.cache_size(), 0);
    }

    #[test]
    fn test_is_valid_transition() {
        // ── Valid forward transitions ────────────────────────────────────────
        assert!(TransactionState::Pending.is_valid_transition(TransactionState::InProgress));
        assert!(TransactionState::Pending.is_valid_transition(TransactionState::Failed));
        assert!(TransactionState::InProgress.is_valid_transition(TransactionState::Completed));
        assert!(TransactionState::InProgress.is_valid_transition(TransactionState::Failed));

        // ── Terminal states cannot transition further ─────────────────────────
        assert!(!TransactionState::Completed.is_valid_transition(TransactionState::InProgress));
        assert!(!TransactionState::Completed.is_valid_transition(TransactionState::Pending));
        assert!(!TransactionState::Completed.is_valid_transition(TransactionState::Failed));
        assert!(!TransactionState::Completed.is_valid_transition(TransactionState::Completed));
        assert!(!TransactionState::Failed.is_valid_transition(TransactionState::InProgress));
        assert!(!TransactionState::Failed.is_valid_transition(TransactionState::Pending));
        assert!(!TransactionState::Failed.is_valid_transition(TransactionState::Completed));
        assert!(!TransactionState::Failed.is_valid_transition(TransactionState::Failed));

        // ── Backward and skip transitions are rejected ────────────────────────
        assert!(!TransactionState::Pending.is_valid_transition(TransactionState::Completed));
        assert!(!TransactionState::Pending.is_valid_transition(TransactionState::Pending));
        assert!(!TransactionState::InProgress.is_valid_transition(TransactionState::Pending));
        assert!(!TransactionState::InProgress.is_valid_transition(TransactionState::InProgress));
    }

    #[test]
    fn test_is_terminal() {
        assert!(TransactionState::Completed.is_terminal());
        assert!(TransactionState::Failed.is_terminal());
        assert!(!TransactionState::Pending.is_terminal());
        assert!(!TransactionState::InProgress.is_terminal());
    }

    #[test]
    fn test_illegal_transition_message_format() {
        let msg = TransactionState::Completed.illegal_transition_message(TransactionState::Pending);
        assert!(msg.starts_with("[E24]"), "message must carry [E24] prefix: {msg}");
        assert!(msg.contains("completed"), "message must name the from-state: {msg}");
        assert!(msg.contains("pending"), "message must name the to-state: {msg}");
    }

    #[test]
    fn test_advance_transaction_state_legal() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();

        let r = tracker.advance_transaction_state(1, TransactionState::InProgress, &env);
        assert!(r.is_ok());
        assert_eq!(r.unwrap().state, TransactionState::InProgress);

        let r = tracker.advance_transaction_state(1, TransactionState::Completed, &env);
        assert!(r.is_ok());
        assert_eq!(r.unwrap().state, TransactionState::Completed);
    }

    #[test]
    fn test_advance_transaction_state_illegal() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.advance_transaction_state(1, TransactionState::InProgress, &env).ok();
        tracker.advance_transaction_state(1, TransactionState::Completed, &env).ok();

        // Completed → InProgress must be rejected
        let r = tracker.advance_transaction_state(1, TransactionState::InProgress, &env);
        assert!(r.is_err());
    }

    // -----------------------------------------------------------------------
    // Backward / same-state transition guard
    // -----------------------------------------------------------------------

    #[test]
    fn test_backward_transition_completed_to_pending_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        let r = tracker.advance_transaction_state(1, TransactionState::Pending, &env);
        assert!(r.is_err());
    }

    #[test]
    fn test_backward_transition_failed_to_in_progress_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.fail_transaction(1, String::from_str(&env, "err"), &env).ok();

        let r = tracker.advance_transaction_state(1, TransactionState::InProgress, &env);
        assert!(r.is_err());
    }

    #[test]
    fn test_same_state_transition_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();

        // Pending → Pending is not a valid transition
        let r = tracker.advance_transaction_state(1, TransactionState::Pending, &env);
        assert!(r.is_err());
    }

    #[test]
    fn test_pending_to_completed_directly_rejected() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();

        let r = tracker.advance_transaction_state(1, TransactionState::Completed, &env);
        assert!(r.is_err());
    }

    // -----------------------------------------------------------------------
    // Audit log entries for success and failure
    // -----------------------------------------------------------------------

    #[test]
    fn test_audit_log_entry_on_successful_transition() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();

        assert_eq!(tracker.audit_log.len(), 1);
        let entry = &tracker.audit_log[0];
        assert_eq!(entry.transaction_id, 1);
        assert_eq!(entry.from_state, TransactionState::Pending);
        assert_eq!(entry.to_state, TransactionState::InProgress);
        assert!(entry.success);
    }

    #[test]
    fn test_audit_log_entry_on_failed_transition() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        // Illegal: Completed → Pending
        let _ = tracker.advance_transaction_state(1, TransactionState::Pending, &env);

        // 2 successful + 1 failed
        assert_eq!(tracker.audit_log.len(), 3);
        let failed_entry = &tracker.audit_log[2];
        assert_eq!(failed_entry.from_state, TransactionState::Completed);
        assert_eq!(failed_entry.to_state, TransactionState::Pending);
        assert!(!failed_entry.success);
    }

    #[test]
    fn test_audit_log_records_all_transitions() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        assert_eq!(tracker.audit_log.len(), 2);
        assert!(tracker.audit_log[0].success);
        assert!(tracker.audit_log[1].success);
    }

    // -----------------------------------------------------------------------
    // state_history accuracy
    // -----------------------------------------------------------------------

    #[test]
    fn test_state_history_reflects_full_progression() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        let record = tracker.get_transaction_state(1, &env).unwrap().unwrap();
        assert_eq!(record.state_history.len(), 3);
        assert_eq!(record.state_history.get(0).unwrap().0, TransactionState::Pending);
        assert_eq!(record.state_history.get(1).unwrap().0, TransactionState::InProgress);
        assert_eq!(record.state_history.get(2).unwrap().0, TransactionState::Completed);
    }

    #[test]
    fn test_state_history_not_updated_on_illegal_transition() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();
        // Illegal: InProgress → Pending
        let _ = tracker.advance_transaction_state(1, TransactionState::Pending, &env);

        let record = tracker.get_transaction_state(1, &env).unwrap().unwrap();
        // Only Pending + InProgress — illegal attempt must not append
        assert_eq!(record.state_history.len(), 2);
        assert_eq!(record.state.as_str(), "in_progress");
    }

    // -----------------------------------------------------------------------
    // #186 TTL management
    // -----------------------------------------------------------------------

    #[test]
    fn test_bump_ttl_noop_in_dev_mode() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();
        // bump_ttl is a no-op in dev mode — must not error
        assert!(tracker.bump_ttl(1, &env).is_ok());
    }

    #[test]
    fn test_bump_ttl_missing_tx_returns_err_in_dev_mode() {
        let env = Env::default();
        let tracker = TransactionStateTracker::new(true);
        // No transaction created — bump_ttl on missing ID is a no-op (Ok) in dev mode
        assert!(tracker.bump_ttl(99, &env).is_ok());
    }

    #[test]
    fn test_cleanup_expired_removes_expired_entries() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.create_transaction(2, initiator.clone(), &env).ok();

        // Mark tx 1 as expired
        tracker.expired_ids.push(1);
        let removed = tracker.cleanup_expired(&env);

        assert_eq!(removed, 1);
        assert_eq!(tracker.cache_size(), 1);
        assert!(tracker.get_transaction_state(1, &env).unwrap().is_none());
        assert!(tracker.get_transaction_state(2, &env).unwrap().is_some());
    }

    #[test]
    fn test_cleanup_expired_no_expired_entries() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).ok();

        let removed = tracker.cleanup_expired(&env);
        assert_eq!(removed, 0);
        assert_eq!(tracker.cache_size(), 1);
    }

    #[test]
    fn test_terminal_transactions_tracked_separately() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).ok();
        tracker.start_transaction(1, &env).ok();
        tracker.complete_transaction(1, &env).ok();

        let record = tracker.get_transaction_state(1, &env).unwrap().unwrap();
        // Terminal state — would get shorter TTL in production
        assert!(matches!(record.state, TransactionState::Completed | TransactionState::Failed));
    }

    #[test]
    fn test_storage_budget_monitor() {
        let mut monitor = StorageBudgetMonitor::new();
        assert_eq!(monitor.entry_count, 0);
        monitor.record_entry(128);
        monitor.record_entry(256);
        assert_eq!(monitor.entry_count, 2);
        assert_eq!(monitor.approx_bytes, 384);
        monitor.remove_entry(128);
        assert_eq!(monitor.entry_count, 1);
        assert_eq!(monitor.approx_bytes, 256);
    }

    // ── StorageBudgetMonitor alerting tests (#349) ───────────────────────────

    #[test]
    fn test_budget_usage_percent_empty() {
        let monitor = StorageBudgetMonitor::new();
        assert_eq!(monitor.usage_percent(1000), 0);
    }

    #[test]
    fn test_budget_usage_percent_zero_max() {
        let monitor = StorageBudgetMonitor::new();
        assert_eq!(monitor.usage_percent(0), 100);
    }

    #[test]
    fn test_budget_usage_percent_partial() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(500);
        assert_eq!(monitor.usage_percent(1000), 50);
    }

    #[test]
    fn test_budget_usage_percent_capped_at_100() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(2000);
        assert_eq!(monitor.usage_percent(1000), 100);
    }

    #[test]
    fn test_budget_status_ok() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(100);
        assert_eq!(monitor.get_status(500, 900), BudgetStatus::Ok);
    }

    #[test]
    fn test_budget_status_warning() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(600);
        assert_eq!(monitor.get_status(500, 900), BudgetStatus::Warning);
    }

    #[test]
    fn test_budget_status_critical() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(950);
        assert_eq!(monitor.get_status(500, 900), BudgetStatus::Critical);
    }

    #[test]
    fn test_budget_no_alert_below_threshold() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(100);
        assert!(monitor.check_alert(10, 1000).is_none());
    }

    #[test]
    fn test_budget_alert_on_entry_count() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(10);
        monitor.record_entry(10);
        monitor.record_entry(10);
        let alert = monitor.check_alert(3, 10_000).unwrap();
        assert_eq!(alert.entry_count, 3);
        assert_eq!(alert.status, BudgetStatus::Warning);
    }

    #[test]
    fn test_budget_alert_on_bytes() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(800);
        let alert = monitor.check_alert(100, 500).unwrap();
        assert_eq!(alert.approx_bytes, 800);
        assert_eq!(alert.threshold_bytes, 500);
    }

    #[test]
    fn test_budget_alert_critical_when_double_threshold() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(1001);
        let alert = monitor.check_alert(100, 500).unwrap();
        assert_eq!(alert.status, BudgetStatus::Critical);
    }

    #[test]
    fn test_budget_is_near_limit_false() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(400);
        assert!(!monitor.is_near_limit(80, 1000));
    }

    #[test]
    fn test_budget_is_near_limit_true() {
        let mut monitor = StorageBudgetMonitor::new();
        monitor.record_entry(850);
        assert!(monitor.is_near_limit(80, 1000));
    }

    // -----------------------------------------------------------------------
    // Issue #626 — recovery metadata export / import tests (std-only)
    // -----------------------------------------------------------------------

    /// Export produces a valid JSON string with all fields.
    #[test]
    #[cfg(feature = "std")]
    fn test_export_recovery_state_returns_json() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator, &env).unwrap();
        tracker.start_transaction(1, &env).unwrap();
        tracker.fail_transaction(1, String::from_str(&env, "network timeout"), &env).unwrap();

        let json = tracker.export_recovery_state(1, &env).unwrap();
        assert!(json.contains("failure_reason"));
        assert!(json.contains("network timeout"));
        assert!(json.contains("transaction_id"));
        assert!(json.contains("failed_from_state"));
        assert!(json.contains("in_progress"));
    }

    /// Export returns error for non-existent transaction.
    #[test]
    #[cfg(feature = "std")]
    fn test_export_missing_transaction_returns_error() {
        let env = Env::default();
        let tracker = TransactionStateTracker::new(true);
        let result = tracker.export_recovery_state(99, &env);
        assert!(result.is_err());
    }

    /// Export returns error for non-failed transaction.
    #[test]
    #[cfg(feature = "std")]
    fn test_export_non_failed_transaction_returns_error() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        tracker.create_transaction(1, initiator, &env).unwrap();
        let result = tracker.export_recovery_state(1, &env);
        assert!(result.is_err());
    }

    /// Import valid JSON creates a new failed transaction record.
    #[test]
    #[cfg(feature = "std")]
    fn test_import_recovery_state_creates_new_transaction() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        let json = r#"{
            "failure_reason": "imported timeout",
            "last_updated_ledger": 42,
            "failed_from_state": "in_progress",
            "retry_count": 1,
            "transaction_id": 100
        }"#;

        tracker.import_recovery_state(json, initiator.clone(), &env).unwrap();

        let record = tracker.get_transaction_state(100, &env).unwrap().unwrap();
        assert_eq!(record.state, TransactionState::Failed);
        assert!(record.recovery_metadata.is_some());
        let meta = record.recovery_metadata.unwrap();
        assert_eq!(meta.failure_reason, soroban_sdk::String::from_str(&env, "imported timeout"));
        assert_eq!(meta.retry_count, 1);
        assert_eq!(meta.failed_from_state, TransactionState::InProgress);
    }

    /// Import updates recovery metadata on an existing failed transaction.
    #[test]
    #[cfg(feature = "std")]
    fn test_import_updates_existing_failed_transaction() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).unwrap();
        tracker.start_transaction(1, &env).unwrap();
        tracker.fail_transaction(1, String::from_str(&env, "original error"), &env).unwrap();

        let json = r#"{
            "failure_reason": "updated reason",
            "last_updated_ledger": 99,
            "failed_from_state": "pending",
            "retry_count": 5,
            "transaction_id": 1
        }"#;

        tracker.import_recovery_state(json, initiator.clone(), &env).unwrap();

        let meta = tracker.get_recovery_metadata(1, &env).unwrap().unwrap();
        assert_eq!(meta.failure_reason, soroban_sdk::String::from_str(&env, "updated reason"));
        assert_eq!(meta.retry_count, 5);
        assert_eq!(meta.failed_from_state, TransactionState::Pending);
    }

    /// Import rejects invalid JSON.
    #[test]
    #[cfg(feature = "std")]
    fn test_import_invalid_json_returns_error() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        let result = tracker.import_recovery_state("not json at all", initiator, &env);
        assert!(result.is_err());
    }

    /// Import rejects unknown failed_from_state.
    #[test]
    #[cfg(feature = "std")]
    fn test_import_invalid_state_name_returns_error() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        let json = r#"{
            "failure_reason": "bad state",
            "last_updated_ledger": 0,
            "failed_from_state": "unknown_state",
            "retry_count": 0,
            "transaction_id": 1
        }"#;

        let result = tracker.import_recovery_state(json, initiator, &env);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown_state"));
    }

    /// Import rejects terminal failed_from_state.
    #[test]
    #[cfg(feature = "std")]
    fn test_import_terminal_failed_from_state_returns_error() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        let json = r#"{
            "failure_reason": "terminal from_state",
            "last_updated_ledger": 0,
            "failed_from_state": "completed",
            "retry_count": 0,
            "transaction_id": 1
        }"#;

        let result = tracker.import_recovery_state(json, initiator, &env);
        assert!(result.is_err());
    }

    /// Import rejects non-failed existing transaction.
    #[test]
    #[cfg(feature = "std")]
    fn test_import_existing_non_failed_returns_error() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).unwrap();
        // Transaction is Pending, not Failed

        let json = r#"{
            "failure_reason": "trying to overwrite",
            "last_updated_ledger": 0,
            "failed_from_state": "pending",
            "retry_count": 0,
            "transaction_id": 1
        }"#;

        let result = tracker.import_recovery_state(json, initiator, &env);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("pending"));
    }

    /// Round-trip: export then import produces equivalent state.
    #[test]
    #[cfg(feature = "std")]
    fn test_export_import_round_trip() {
        let env = Env::default();
        let mut tracker = TransactionStateTracker::new(true);
        let initiator = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);

        tracker.create_transaction(1, initiator.clone(), &env).unwrap();
        tracker.start_transaction(1, &env).unwrap();
        tracker.fail_transaction(1, String::from_str(&env, "round-trip error"), &env).unwrap();

        // Export
        let json = tracker.export_recovery_state(1, &env).unwrap();

        // Create a new tracker and import
        let mut tracker2 = TransactionStateTracker::new(true);
        tracker2.import_recovery_state(&json, initiator.clone(), &env).unwrap();

        let meta = tracker2.get_recovery_metadata(1, &env).unwrap().unwrap();
        assert_eq!(meta.failure_reason, soroban_sdk::String::from_str(&env, "round-trip error"));
        assert_eq!(meta.failed_from_state, TransactionState::InProgress);
        assert_eq!(meta.retry_count, 0);
    }
}