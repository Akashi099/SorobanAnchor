//! On-chain governance for stellar.toml capability cache invalidation.
//! On-chain governance for cache invalidation proposals and entry-level cache policies.
//!
//! Any registered attestor can propose that a specific anchor's capability
//! cache entry be invalidated. Once a configurable quorum of distinct attestors
//! endorse the proposal the cache entry is automatically invalidated and a
//! `force_refresh` is triggered. Proposals expire after a configurable number
//! of ledgers.
//!
//! ## Cache Policy Model
//!
//! Beyond the simple on-chain invalidation proposals this module also hosts a
//! **policy model** that governs how cache entries are created, refreshed, and
//! expired across entry types (metadata, capabilities, and custom keys).
//!
//! Each [`CacheEntryType`] carries its own [`CachePolicy`] which defines:
//! - `min_ttl_seconds` / `max_ttl_seconds` — hard bounds on caller-supplied TTLs.
//! - `refresh_threshold_pct` — how far into the TTL (0–100 %) a background refresh
//!   should be triggered.
//! - `allow_forced_invalidation` — whether emergency forced-eviction is permitted.
//!
//! Policies are stored under a separate key from the governance config so they
//! can be updated independently by admins.

use soroban_sdk::{contracttype, Address, BytesN, Env, Vec};
use crate::deterministic_hash::make_storage_key;
use crate::errors::{AnchorKitError, ErrorCode};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default number of endorsements required to reach quorum.
pub const DEFAULT_QUORUM: u32 = 3;
/// Default proposal TTL: ~1 day at 5 s/ledger.
pub const DEFAULT_EXPIRY_LEDGERS: u32 = 17_280;

// Default policy constants — metadata entries.
/// Minimum TTL allowed for metadata cache entries (60 s).
pub const METADATA_MIN_TTL_SECONDS: u64 = 60;
/// Maximum TTL allowed for metadata cache entries (24 h).
pub const METADATA_MAX_TTL_SECONDS: u64 = 86_400;
/// Refresh when 80 % of the TTL has elapsed (i.e. at 20 % remaining).
pub const METADATA_REFRESH_THRESHOLD_PCT: u32 = 80;

// Default policy constants — capabilities entries.
/// Minimum TTL allowed for capabilities cache entries (5 min).
pub const CAPABILITIES_MIN_TTL_SECONDS: u64 = 300;
/// Maximum TTL allowed for capabilities cache entries (7 days).
pub const CAPABILITIES_MAX_TTL_SECONDS: u64 = 604_800;
/// Refresh when 75 % of the TTL has elapsed.
pub const CAPABILITIES_REFRESH_THRESHOLD_PCT: u32 = 75;

// Default policy constants — generic / other entries.
/// Minimum TTL allowed for any other cached values (30 s).
pub const OTHER_MIN_TTL_SECONDS: u64 = 30;
/// Maximum TTL allowed for any other cached values (12 h).
pub const OTHER_MAX_TTL_SECONDS: u64 = 43_200;
/// Refresh when 70 % of the TTL has elapsed.
pub const OTHER_REFRESH_THRESHOLD_PCT: u32 = 70;

// ---------------------------------------------------------------------------
// Cache entry type discriminant
// ---------------------------------------------------------------------------

/// The category of a cache entry, used to look up the correct [`CachePolicy`].
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum CacheEntryType {
    /// Anchor metadata (reputation score, liquidity, uptime, etc.).
    Metadata     = 0,
    /// Stellar capability / TOML entries.
    Capabilities = 1,
    /// Any other cached value not covered by the above.
    Other        = 2,
}

// ---------------------------------------------------------------------------
// Policy types
// ---------------------------------------------------------------------------

/// Governance policy for one category of cache entries.
///
/// Applies to writes (TTL clamping), reads (staleness detection), and
/// forced-invalidation operations.
///
/// # Validation rules (enforced by [`CachePolicy::validate`])
///
/// - `min_ttl_seconds < max_ttl_seconds`
/// - `refresh_threshold_pct` is in `[1, 99]`
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CachePolicy {
    /// Minimum acceptable TTL in seconds. A write that supplies a smaller TTL
    /// is clamped up to this value.
    pub min_ttl_seconds: u64,
    /// Maximum acceptable TTL in seconds. A write that supplies a larger TTL
    /// is clamped down to this value.
    pub max_ttl_seconds: u64,
    /// Percentage of the TTL after which a background refresh should be
    /// triggered. For example `80` means "start refreshing when 80 % of the
    /// TTL has elapsed (20 % remaining)".
    ///
    /// Must be in the range `[1, 99]`.
    pub refresh_threshold_pct: u32,
    /// Whether callers are permitted to perform a forced (out-of-band)
    /// invalidation of entries in this category. When `false` the only path
    /// to eviction is natural TTL expiry or the on-chain governance proposal
    /// workflow.
    pub allow_forced_invalidation: bool,
}

impl CachePolicy {
    /// Default policy for [`CacheEntryType::Metadata`].
    pub fn default_metadata() -> Self {
        CachePolicy {
            min_ttl_seconds: METADATA_MIN_TTL_SECONDS,
            max_ttl_seconds: METADATA_MAX_TTL_SECONDS,
            refresh_threshold_pct: METADATA_REFRESH_THRESHOLD_PCT,
            allow_forced_invalidation: true,
        }
    }

    /// Default policy for [`CacheEntryType::Capabilities`].
    pub fn default_capabilities() -> Self {
        CachePolicy {
            min_ttl_seconds: CAPABILITIES_MIN_TTL_SECONDS,
            max_ttl_seconds: CAPABILITIES_MAX_TTL_SECONDS,
            refresh_threshold_pct: CAPABILITIES_REFRESH_THRESHOLD_PCT,
            allow_forced_invalidation: true,
        }
    }

    /// Default policy for [`CacheEntryType::Other`].
    pub fn default_other() -> Self {
        CachePolicy {
            min_ttl_seconds: OTHER_MIN_TTL_SECONDS,
            max_ttl_seconds: OTHER_MAX_TTL_SECONDS,
            refresh_threshold_pct: OTHER_REFRESH_THRESHOLD_PCT,
            allow_forced_invalidation: false,
        }
    }

    /// Return the default policy for a given entry type.
    pub fn default_for(entry_type: CacheEntryType) -> Self {
        match entry_type {
            CacheEntryType::Metadata     => Self::default_metadata(),
            CacheEntryType::Capabilities => Self::default_capabilities(),
            CacheEntryType::Other        => Self::default_other(),
        }
    }

    /// Check internal consistency.
    ///
    /// Returns `Err` with [`ErrorCode::ValidationError`] when:
    /// - `min_ttl_seconds == 0`
    /// - `min_ttl_seconds >= max_ttl_seconds`
    /// - `refresh_threshold_pct` is outside `[1, 99]`
    pub fn validate(&self) -> Result<(), AnchorKitError> {
        if self.min_ttl_seconds == 0 {
            return Err(AnchorKitError::validation_error(
                "min_ttl_seconds must be greater than zero",
            ));
        }
        if self.min_ttl_seconds >= self.max_ttl_seconds {
            return Err(AnchorKitError::validation_error(
                "min_ttl_seconds must be less than max_ttl_seconds",
            ));
        }
        if self.refresh_threshold_pct == 0 || self.refresh_threshold_pct >= 100 {
            return Err(AnchorKitError::validation_error(
                "refresh_threshold_pct must be in range [1, 99]",
            ));
        }
        Ok(())
    }

    /// Clamp `requested_ttl` into the `[min_ttl_seconds, max_ttl_seconds]` band
    /// defined by this policy.
    ///
    /// If `requested_ttl` is zero the midpoint of the allowed band is returned,
    /// which is a reasonable default when the caller did not supply a preference.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::cache_governance::CachePolicy;
    ///
    /// let policy = CachePolicy::default_metadata(); // min=60, max=86400
    /// // Zero → midpoint (60 + (86400-60)/2 = 43230)
    /// assert_eq!(policy.clamp_ttl(0), 43_230);
    /// // Below min → clamped up to min
    /// assert_eq!(policy.clamp_ttl(30), 60);
    /// // Within band → unchanged
    /// assert_eq!(policy.clamp_ttl(3_600), 3_600);
    /// // Above max → clamped down to max
    /// assert_eq!(policy.clamp_ttl(999_999), 86_400);
    /// ```
    pub fn clamp_ttl(&self, requested_ttl: u64) -> u64 {
        if requested_ttl == 0 {
            // Default: midpoint of the allowed band.
            return self.min_ttl_seconds.saturating_add(
                self.max_ttl_seconds.saturating_sub(self.min_ttl_seconds) / 2,
            );
        }
        requested_ttl
            .max(self.min_ttl_seconds)
            .min(self.max_ttl_seconds)
    }

    /// Determine whether an entry with the given `age_seconds` has entered the
    /// background-refresh window.
    ///
    /// Returns `true` when `age_seconds >= ttl_seconds * refresh_threshold_pct / 100`.
    pub fn needs_refresh(&self, age_seconds: u64, ttl_seconds: u64) -> bool {
        // threshold = ttl * pct / 100  (integer, rounded down)
        let threshold = ttl_seconds
            .saturating_mul(self.refresh_threshold_pct as u64)
            / 100;
        age_seconds >= threshold
    }

    /// Determine whether an entry with the given `age_seconds` has fully expired.
    ///
    /// Returns `true` when `age_seconds >= ttl_seconds`.
    pub fn is_expired(&self, age_seconds: u64, ttl_seconds: u64) -> bool {
        age_seconds >= ttl_seconds
    }
}

// ---------------------------------------------------------------------------
// Bundled policy set — all three entry types in one storable struct
// ---------------------------------------------------------------------------

/// The complete set of governance policies for all cache entry types.
///
/// Stored under a single persistent key and updated atomically by
/// [`set_policy_set`].
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct CachePolicySet {
    pub metadata: CachePolicy,
    pub capabilities: CachePolicy,
    pub other: CachePolicy,
}

impl CachePolicySet {
    /// All-default policy set.
    pub fn default_set() -> Self {
        CachePolicySet {
            metadata:     CachePolicy::default_metadata(),
            capabilities: CachePolicy::default_capabilities(),
            other:        CachePolicy::default_other(),
        }
    }

    /// Retrieve the policy for a specific entry type.
    pub fn for_type(&self, entry_type: CacheEntryType) -> &CachePolicy {
        match entry_type {
            CacheEntryType::Metadata     => &self.metadata,
            CacheEntryType::Capabilities => &self.capabilities,
            CacheEntryType::Other        => &self.other,
        }
    }

    /// Validate all three contained policies.
    pub fn validate(&self) -> Result<(), AnchorKitError> {
        self.metadata.validate()?;
        self.capabilities.validate()?;
        self.other.validate()
    }
}

// ---------------------------------------------------------------------------
// Storage types
// ---------------------------------------------------------------------------

/// An on-chain cache invalidation proposal.
#[contracttype]
#[derive(Clone, Debug)]
pub struct CacheInvalidationProposal {
    /// Sequential proposal ID.
    pub proposal_id: u64,
    /// Anchor whose capability cache entry should be invalidated.
    pub anchor: Address,
    /// Attestor who created the proposal.
    pub proposer: Address,
    /// Addresses that have endorsed this proposal (no duplicates).
    pub endorsements: Vec<Address>,
    /// Ledger sequence number when the proposal was created.
    pub created_at_ledger: u32,
    /// Number of ledgers after creation before the proposal expires.
    pub expiry_ledgers: u32,
    /// Whether the proposal has already been executed.
    pub executed: bool,
}

/// Governance configuration for cache invalidation proposals.
#[contracttype]
#[derive(Clone, Debug)]
pub struct CacheGovernanceConfig {
    /// Number of endorsements required to reach quorum.
    pub quorum_threshold: u32,
    /// Proposal TTL in ledgers.
    pub proposal_expiry_ledgers: u32,
}

impl CacheGovernanceConfig {
    pub fn default_config() -> Self {
        CacheGovernanceConfig {
            quorum_threshold: DEFAULT_QUORUM,
            proposal_expiry_ledgers: DEFAULT_EXPIRY_LEDGERS,
        }
    }
}

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

fn config_key(env: &Env) -> BytesN<32> {
    make_storage_key(env, &[b"CGOV_CFG"])
}

fn policy_set_key(env: &Env) -> BytesN<32> {
    make_storage_key(env, &[b"CGOV_POL"])
}

fn proposal_count_key(env: &Env) -> BytesN<32> {
    make_storage_key(env, &[b"CGOV_CNT"])
}

fn proposal_key(env: &Env, proposal_id: u64) -> BytesN<32> {
    make_storage_key(env, &[b"CGOV_PROP", &proposal_id.to_be_bytes()])
}

// ---------------------------------------------------------------------------
// Public API — governance config
// ---------------------------------------------------------------------------

/// Read the governance configuration, returning the default if not set.
pub fn get_config(env: &Env) -> CacheGovernanceConfig {
    env.storage()
        .persistent()
        .get::<_, CacheGovernanceConfig>(&config_key(env))
        .unwrap_or_else(CacheGovernanceConfig::default_config)
}

/// Persist updated governance configuration (admin-only enforcement is in the
/// contract layer).
pub fn set_config(env: &Env, config: CacheGovernanceConfig) {
    env.storage().persistent().set(&config_key(env), &config);
}

// ---------------------------------------------------------------------------
// Public API — cache policy
// ---------------------------------------------------------------------------

/// Read the full [`CachePolicySet`], returning sensible defaults when none has
/// been configured yet.
pub fn get_policy_set(env: &Env) -> CachePolicySet {
    env.storage()
        .persistent()
        .get::<_, CachePolicySet>(&policy_set_key(env))
        .unwrap_or_else(CachePolicySet::default_set)
}

/// Persist an updated [`CachePolicySet`] after validating all three contained
/// policies. Returns `Err(ValidationError)` when any policy is internally
/// inconsistent.
///
/// Admin-only enforcement is delegated to the contract layer.
pub fn set_policy_set(env: &Env, policies: CachePolicySet) -> Result<(), AnchorKitError> {
    policies.validate()?;
    env.storage().persistent().set(&policy_set_key(env), &policies);
    Ok(())
}

/// Retrieve the active [`CachePolicy`] for a single [`CacheEntryType`].
///
/// Equivalent to `get_policy_set(env).for_type(entry_type)` but avoids
/// loading the full set when only one policy is needed.
pub fn get_policy(env: &Env, entry_type: CacheEntryType) -> CachePolicy {
    let set = get_policy_set(env);
    set.for_type(entry_type).clone()
}

/// **Write-path enforcement** — clamp a caller-supplied TTL to the policy
/// band and determine whether a background refresh flag should be set on
/// an existing entry of the given age.
///
/// Returns `(effective_ttl, refresh_needed)`:
/// - `effective_ttl` — the TTL to actually store, clamped to
///   `[policy.min_ttl_seconds, policy.max_ttl_seconds]`.
/// - `refresh_needed` — `true` when the entry's current age already meets
///   or exceeds the refresh threshold (used when overwriting a still-active
///   entry with fresh data — the flag will be `false` on a brand-new write).
///
/// Pass `current_age_seconds = 0` for brand-new entries.
pub fn enforce_write_policy(
    env: &Env,
    entry_type: CacheEntryType,
    requested_ttl: u64,
    current_age_seconds: u64,
) -> (u64, bool) {
    let policy = get_policy(env, entry_type);
    let effective_ttl = policy.clamp_ttl(requested_ttl);
    let refresh_needed = policy.needs_refresh(current_age_seconds, effective_ttl);
    (effective_ttl, refresh_needed)
}

/// **Read-path enforcement** — classify an entry by its current age and TTL
/// according to the policy for its entry type.
///
/// Returns one of:
/// - `(false, false)` — entry is within the primary TTL and does not yet
///   need a background refresh.
/// - `(true, false)`  — entry has passed the refresh threshold but is still
///   within the primary TTL; a background refresh is recommended.
/// - `(true, true)`   — entry has fully expired; it must not be served.
///
/// The tuple is `(needs_refresh, is_expired)`.
pub fn enforce_read_policy(
    env: &Env,
    entry_type: CacheEntryType,
    age_seconds: u64,
    ttl_seconds: u64,
) -> (bool, bool) {
    let policy = get_policy(env, entry_type);
    let expired = policy.is_expired(age_seconds, ttl_seconds);
    let refresh = policy.needs_refresh(age_seconds, ttl_seconds);
    (refresh, expired)
}

/// **Invalidation-path enforcement** — check whether forced invalidation is
/// permitted for the given entry type under the active policy.
///
/// Returns `Ok(())` when allowed.
/// Returns `Err(ValidationError)` with the message
/// `"forced invalidation not permitted by policy"` when disabled.
pub fn enforce_invalidation_policy(
    env: &Env,
    entry_type: CacheEntryType,
) -> Result<(), AnchorKitError> {
    let policy = get_policy(env, entry_type);
    if !policy.allow_forced_invalidation {
        return Err(AnchorKitError::validation_error(
            "forced invalidation not permitted by policy",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API — proposals
// ---------------------------------------------------------------------------

/// Create a new cache invalidation proposal.
///
/// Returns the new `proposal_id`. The `proposer` must be a registered
/// attestor; that check is enforced by the contract layer.
pub fn propose(env: &Env, proposer: &Address, anchor: &Address) -> u64 {
    let cfg = get_config(env);
    let proposal_id: u64 = env
        .storage()
        .persistent()
        .get::<_, u64>(&proposal_count_key(env))
        .unwrap_or(0);

    // Build the initial endorsements list with the proposer as first endorser.
    let mut endorsements = Vec::new(env);
    endorsements.push_back(proposer.clone());

    let proposal = CacheInvalidationProposal {
        proposal_id,
        anchor: anchor.clone(),
        proposer: proposer.clone(),
        endorsements,
        created_at_ledger: env.ledger().sequence(),
        expiry_ledgers: cfg.proposal_expiry_ledgers,
        executed: false,
    };

    env.storage()
        .persistent()
        .set(&proposal_key(env, proposal_id), &proposal);
    env.storage()
        .persistent()
        .set(&proposal_count_key(env), &(proposal_id + 1));

    proposal_id
}

/// Add an endorsement to an existing proposal.
///
/// Duplicate endorsements from the same address are silently ignored.
/// Returns `Err(NotFound)` when the proposal does not exist.
/// Returns `Err(StaleQuote)` when the proposal is expired.
pub fn endorse(env: &Env, endorser: &Address, proposal_id: u64) -> Result<(), AnchorKitError> {
    let key = proposal_key(env, proposal_id);
    let mut proposal = env
        .storage()
        .persistent()
        .get::<_, CacheInvalidationProposal>(&key)
        .ok_or_else(AnchorKitError::cache_not_found)?;

    if is_expired(env, &proposal) {
        return Err(AnchorKitError::new(ErrorCode::StaleQuote, "proposal expired"));
    }
    if proposal.executed {
        return Err(AnchorKitError::validation_error("proposal already executed"));
    }

    // Deduplication: ignore if already endorsed by this address.
    for i in 0..proposal.endorsements.len() {
        if proposal.endorsements.get(i).unwrap() == *endorser {
            return Ok(());
        }
    }

    proposal.endorsements.push_back(endorser.clone());
    env.storage().persistent().set(&key, &proposal);
    Ok(())
}

/// Execute a proposal that has reached quorum.
///
/// Callable by anyone once quorum is met. Returns the anchor address whose
/// cache was invalidated so the contract layer can call `force_refresh_metadata`.
///
/// Returns `Err(StaleQuote)` when expired, `Err(ValidationError)` when quorum
/// has not been reached, `Err(NotFound)` when the proposal does not exist.
pub fn execute(env: &Env, proposal_id: u64) -> Result<Address, AnchorKitError> {
    let key = proposal_key(env, proposal_id);
    let mut proposal = env
        .storage()
        .persistent()
        .get::<_, CacheInvalidationProposal>(&key)
        .ok_or_else(AnchorKitError::cache_not_found)?;

    if is_expired(env, &proposal) {
        return Err(AnchorKitError::new(ErrorCode::StaleQuote, "proposal expired"));
    }
    if proposal.executed {
        return Err(AnchorKitError::validation_error("proposal already executed"));
    }

    let cfg = get_config(env);
    if proposal.endorsements.len() < cfg.quorum_threshold {
        return Err(AnchorKitError::validation_error("quorum not reached"));
    }

    proposal.executed = true;
    env.storage().persistent().set(&key, &proposal);

    Ok(proposal.anchor.clone())
}

/// Read a proposal by ID. Returns `None` if it does not exist.
pub fn get_proposal(env: &Env, proposal_id: u64) -> Option<CacheInvalidationProposal> {
    env.storage()
        .persistent()
        .get::<_, CacheInvalidationProposal>(&proposal_key(env, proposal_id))
}

/// Total number of proposals ever created.
pub fn proposal_count(env: &Env) -> u64 {
    env.storage()
        .persistent()
        .get::<_, u64>(&proposal_count_key(env))
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Structured-logging wrappers (host-side only)
// ---------------------------------------------------------------------------

/// Structured-logging wrappers around the governance API.
///
/// Each `*_logged` function behaves exactly like its unlogged counterpart and
/// additionally records structured entries (see [`crate::structured_log`]) on
/// the supplied logger. Only compiled for host builds — on-chain (`wasm`)
/// builds keep using the plain functions and the contract event stream.
#[cfg(not(feature = "wasm"))]
mod logged {
    use super::*;
    use crate::structured_log::{events, StructuredLogger};

    /// Render a soroban [`Address`] as a host string for log fields.
    fn address_field(_env: &Env, addr: &Address) -> alloc::string::String {
        let s = addr.to_string();
        let len = s.len() as usize;
        let mut buf = alloc::vec![0u8; len];
        s.copy_into_slice(&mut buf);
        alloc::string::String::from_utf8_lossy(&buf).into_owned()
    }

    /// Wire name of a [`CacheEntryType`] for log fields.
    fn entry_type_field(entry_type: CacheEntryType) -> &'static str {
        match entry_type {
            CacheEntryType::Metadata => "metadata",
            CacheEntryType::Capabilities => "capabilities",
            CacheEntryType::Other => "other",
        }
    }

    /// [`set_policy_set`] plus a `cache.policy_updated` (info) entry on
    /// success or `cache.policy_rejected` (warn) when validation fails.
    pub fn set_policy_set_logged(
        env: &Env,
        policies: CachePolicySet,
        logger: &StructuredLogger,
    ) -> Result<(), AnchorKitError> {
        let ts = env.ledger().timestamp();
        let summary = [
            ("metadata_max_ttl_seconds", policies.metadata.max_ttl_seconds.into()),
            ("capabilities_max_ttl_seconds", policies.capabilities.max_ttl_seconds.into()),
            ("other_max_ttl_seconds", policies.other.max_ttl_seconds.into()),
        ];
        match set_policy_set(env, policies) {
            Ok(()) => {
                logger.info(events::CACHE_POLICY_UPDATED, ts, &summary);
                Ok(())
            }
            Err(err) => {
                logger.warn(
                    events::CACHE_POLICY_REJECTED,
                    ts,
                    &[("error", alloc::format!("{:?}", err).into())],
                );
                Err(err)
            }
        }
    }

    /// [`enforce_write_policy`] plus a `cache.ttl_clamped` (warn) entry when
    /// the caller-supplied TTL had to be adjusted to the policy band.
    pub fn enforce_write_policy_logged(
        env: &Env,
        entry_type: CacheEntryType,
        requested_ttl: u64,
        current_age_seconds: u64,
        logger: &StructuredLogger,
    ) -> (u64, bool) {
        let (effective_ttl, refresh_needed) =
            enforce_write_policy(env, entry_type, requested_ttl, current_age_seconds);
        if requested_ttl != 0 && effective_ttl != requested_ttl {
            logger.warn(
                events::CACHE_TTL_CLAMPED,
                env.ledger().timestamp(),
                &[
                    ("entry_type", entry_type_field(entry_type).into()),
                    ("requested_ttl_seconds", requested_ttl.into()),
                    ("effective_ttl_seconds", effective_ttl.into()),
                ],
            );
        }
        (effective_ttl, refresh_needed)
    }

    /// [`enforce_invalidation_policy`] plus a `cache.invalidation_denied`
    /// (warn) entry when the policy forbids forced invalidation.
    pub fn enforce_invalidation_policy_logged(
        env: &Env,
        entry_type: CacheEntryType,
        logger: &StructuredLogger,
    ) -> Result<(), AnchorKitError> {
        let result = enforce_invalidation_policy(env, entry_type);
        if result.is_err() {
            logger.warn(
                events::CACHE_INVALIDATION_DENIED,
                env.ledger().timestamp(),
                &[("entry_type", entry_type_field(entry_type).into())],
            );
        }
        result
    }

    /// [`propose`] plus a `cache.proposal_created` (info) entry.
    pub fn propose_logged(
        env: &Env,
        proposer: &Address,
        anchor: &Address,
        logger: &StructuredLogger,
    ) -> u64 {
        let proposal_id = propose(env, proposer, anchor);
        logger.info(
            events::CACHE_PROPOSAL_CREATED,
            env.ledger().timestamp(),
            &[
                ("proposal_id", proposal_id.into()),
                ("anchor", address_field(env, anchor).into()),
                ("proposer", address_field(env, proposer).into()),
                ("ledger", env.ledger().sequence().into()),
            ],
        );
        proposal_id
    }

    /// [`endorse`] plus a `cache.proposal_endorsed` (info) entry with the
    /// running endorsement count, or `cache.proposal_endorse_failed` (warn)
    /// when the proposal is missing, expired, or already executed.
    pub fn endorse_logged(
        env: &Env,
        endorser: &Address,
        proposal_id: u64,
        logger: &StructuredLogger,
    ) -> Result<(), AnchorKitError> {
        let ts = env.ledger().timestamp();
        match endorse(env, endorser, proposal_id) {
            Ok(()) => {
                let endorsement_count = get_proposal(env, proposal_id)
                    .map(|p| p.endorsements.len())
                    .unwrap_or(0);
                logger.info(
                    events::CACHE_PROPOSAL_ENDORSED,
                    ts,
                    &[
                        ("proposal_id", proposal_id.into()),
                        ("endorser", address_field(env, endorser).into()),
                        ("endorsement_count", endorsement_count.into()),
                    ],
                );
                Ok(())
            }
            Err(err) => {
                logger.warn(
                    events::CACHE_PROPOSAL_ENDORSE_FAILED,
                    ts,
                    &[
                        ("proposal_id", proposal_id.into()),
                        ("error", alloc::format!("{:?}", err).into()),
                    ],
                );
                Err(err)
            }
        }
    }

    /// [`execute`] plus a `cache.proposal_executed` (info) entry, or
    /// `cache.proposal_execute_failed` (warn) when execution is not possible.
    pub fn execute_logged(
        env: &Env,
        proposal_id: u64,
        logger: &StructuredLogger,
    ) -> Result<Address, AnchorKitError> {
        let ts = env.ledger().timestamp();
        match execute(env, proposal_id) {
            Ok(anchor) => {
                logger.info(
                    events::CACHE_PROPOSAL_EXECUTED,
                    ts,
                    &[
                        ("proposal_id", proposal_id.into()),
                        ("anchor", address_field(env, &anchor).into()),
                    ],
                );
                Ok(anchor)
            }
            Err(err) => {
                logger.warn(
                    events::CACHE_PROPOSAL_EXECUTE_FAILED,
                    ts,
                    &[
                        ("proposal_id", proposal_id.into()),
                        ("error", alloc::format!("{:?}", err).into()),
                    ],
                );
                Err(err)
            }
        }
    }
}

#[cfg(not(feature = "wasm"))]
pub use logged::{
    endorse_logged, enforce_invalidation_policy_logged, enforce_write_policy_logged,
    execute_logged, propose_logged, set_policy_set_logged,
};

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn is_expired(env: &Env, proposal: &CacheInvalidationProposal) -> bool {
    let current = env.ledger().sequence();
    let expiry = proposal
        .created_at_ledger
        .saturating_add(proposal.expiry_ledgers);
    current >= expiry
}
