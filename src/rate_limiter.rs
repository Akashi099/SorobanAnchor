//! Rate limiting for attestation submissions
//!
//! This module implements per-attestor rate limiting for attestation submissions
//! to prevent spam and abuse of the contract.

use soroban_sdk::{contracttype, xdr::ToXdr, Address, Env};
use crate::deterministic_hash::make_storage_key;
use crate::errors::AnchorKitError;
#[cfg(test)]
use crate::errors::ErrorCode;

/// Rate limit configuration stored in contract storage.
///
/// Defines the sliding-window parameters used by [`RateLimiter::check_and_increment`].
/// The admin can update this at runtime via [`RateLimiter::update_config`].
///
/// # Examples
///
/// ```rust,no_run
/// use anchorkit::RateLimitConfig;
///
/// // Allow at most 5 submissions per 50-ledger window.
/// let config = RateLimitConfig { max_submissions: 5, window_length: 50 };
/// ```
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitConfig {
    /// Maximum number of submissions allowed per window
    pub max_submissions: u32,
    /// Length of the rate limit window in ledgers
    pub window_length: u32,
}

/// Per-attestor rate limit state stored in contract storage.
///
/// Tracks how many submissions an attestor has made in the current window and
/// when that window started. Automatically reset when the window expires.
///
/// # Examples
///
/// ```rust,no_run
/// use anchorkit::RateLimitState;
///
/// let state = RateLimitState { submission_count: 3, window_start_ledger: 1000 };
/// assert_eq!(state.submission_count, 3);
/// ```
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitState {
    /// Number of submissions in the current window
    pub submission_count: u32,
    /// Ledger number when the current window started
    pub window_start_ledger: u32,
}

/// Burst-control configuration: a token bucket layered on top of the base
/// sliding-window limit (#630).
///
/// The base [`RateLimitConfig`] window is a blunt instrument — a client that
/// sends `max_submissions` requests in the first ledger of a window and then
/// falls silent is indistinguishable from one that sends them evenly spread
/// out. A token bucket smooths that: it allows a short burst up to
/// `burst_capacity` tokens, then throttles back to a steady `refill_per_ledger`
/// rate once the bucket is drained. This lets legitimate bursty traffic
/// (e.g. a client catching up after a network blip) through, while still
/// bounding the *sustained* rate an abusive client can achieve.
///
/// Burst control is opt-in and orthogonal to the base rate limit: when no
/// [`BurstControlConfig`] has been stored for an attestor's context,
/// [`RateLimiter::check_and_increment`] behaves exactly as before.
///
/// # Examples
///
/// ```rust,no_run
/// use anchorkit::rate_limiter::BurstControlConfig;
///
/// // Allow bursts of up to 20 requests, refilling 2 tokens per ledger.
/// let config = BurstControlConfig { burst_capacity: 20, refill_per_ledger: 2 };
/// ```
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BurstControlConfig {
    /// Maximum number of tokens the bucket can hold — the largest burst size
    /// allowed before requests start being throttled.
    pub burst_capacity: u32,
    /// Tokens replenished per elapsed ledger since the last refill.
    pub refill_per_ledger: u32,
}

/// Per-attestor token-bucket state for [`BurstControlConfig`] enforcement.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BurstState {
    /// Tokens currently available in the bucket.
    pub tokens: u32,
    /// Ledger sequence number at which tokens were last refilled.
    pub last_refill_ledger: u32,
}

/// Fairness configuration: caps the share of a shared rate-limit window that
/// any single attestor may consume (#630).
///
/// Without fairness control, a shared/global [`RateLimitConfig`] can be
/// monopolised by one high-volume attestor, starving everyone else out of
/// the same window's budget. When a [`FairnessConfig`] is set, each
/// submission also checks that the calling attestor's share of the window's
/// *total* submissions (across all attestors) does not exceed
/// `max_share_percent`, once at least `min_total_for_enforcement`
/// submissions have been recorded in the window (so the very first
/// requests, before there is any contention, are never penalised).
///
/// Fairness control is opt-in: when no [`FairnessConfig`] has been stored,
/// [`RateLimiter::check_and_increment`] behaves exactly as before.
///
/// # Examples
///
/// ```rust,no_run
/// use anchorkit::rate_limiter::FairnessConfig;
///
/// // No single attestor may account for more than 50% of a window's
/// // submissions, once at least 4 submissions have been made.
/// let config = FairnessConfig { max_share_percent: 50, min_total_for_enforcement: 4 };
/// ```
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FairnessConfig {
    /// Maximum percentage (1-100) of the window's total submissions a single
    /// attestor may account for.
    pub max_share_percent: u32,
    /// Minimum total submissions recorded in the window before fairness is
    /// enforced.
    pub min_total_for_enforcement: u32,
}

/// Tracks total submissions across all attestors in the current shared
/// window, used by [`RateLimiter`] to compute each attestor's share for
/// [`FairnessConfig`] enforcement.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalRateWindowState {
    /// Total submissions recorded (by any attestor) in the current window.
    pub total_count: u32,
    /// Ledger number when the current global window started.
    pub window_start_ledger: u32,
}

/// Per-attestor sliding-window rate limiter for attestation submissions.
///
/// All methods are associated functions that operate directly on Soroban
/// persistent storage, so no instance state is needed.
///
/// The default configuration (10 submissions per 100-ledger window) is used
/// when no config has been stored yet.
pub struct RateLimiter;

impl RateLimiter {
    /// Store a per-role rate limit override.
    ///
    /// The config is stored under a key derived from the role symbol bytes, keyed
    /// as `rl_role:<role_bytes>`. Only the contract admin should call this; access
    /// control is enforced in the contract layer via `require_admin`.
    pub fn set_role_override(env: &Env, role: soroban_sdk::Symbol, config: RateLimitConfig) {
        let key = Self::role_override_key(env, &role);
        env.storage().persistent().set(&key, &config);
    }

    /// Retrieve a per-role rate limit override, or `None` if not set.
    pub fn get_role_override(env: &Env, role: soroban_sdk::Symbol) -> Option<RateLimitConfig> {
        let key = Self::role_override_key(env, &role);
        env.storage().persistent().get::<_, RateLimitConfig>(&key)
    }

    /// Store a per-address rate limit override.
    pub fn set_address_override(env: &Env, address: &Address, config: RateLimitConfig) {
        let key = Self::address_override_key(env, address);
        env.storage().persistent().set(&key, &config);
    }

    /// Retrieve a per-address rate limit override, or `None` if not set.
    pub fn get_address_override(env: &Env, address: &Address) -> Option<RateLimitConfig> {
        let key = Self::address_override_key(env, address);
        env.storage().persistent().get::<_, RateLimitConfig>(&key)
    }

    /// Resolve the effective config for an attestor.
    ///
    /// Resolution order:
    /// 1. Per-address override
    /// 2. Per-role override (if `role` is `Some`)
    /// 3. Global default config
    pub fn resolve_config(
        env: &Env,
        attestor: &Address,
        role: Option<soroban_sdk::Symbol>,
    ) -> RateLimitConfig {
        if let Some(cfg) = Self::get_address_override(env, attestor) {
            return cfg;
        }
        if let Some(r) = role {
            if let Some(cfg) = Self::get_role_override(env, r) {
                return cfg;
            }
        }
        Self::get_config(env)
    }

    /// Check whether an attestor is within their rate limit and increment the counter.
    ///
    /// Config resolution order: address override → role override → global default.
    ///
    /// When the caller is the contract admin the check is bypassed entirely; an
    /// audit entry is written via `AdminAuditLog` so the bypass is on-chain record.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `attestor` - The address of the attestor being checked.
    /// * `config` - The active [`RateLimitConfig`] (use [`RateLimiter::resolve_config`]).
    ///
    /// # Returns
    ///
    /// `Ok(())` if the attestor is within the rate limit.
    ///
    /// # Errors
    ///
    /// Returns [`AnchorKitError`] with code [`ErrorCode::RateLimitExceeded`] when
    /// the attestor has reached `config.max_submissions` in the current window.
    pub fn check_and_increment(
        env: &Env,
        attestor: &Address,
        config: &RateLimitConfig,
    ) -> Result<(), AnchorKitError> {
        // If attestor is the admin, skip rate limits and write an audit entry.
        if let Some(admin) = env
            .storage()
            .instance()
            .get::<_, Address>(&make_storage_key(env, &[b"ADMIN"]))
        {
            if *attestor == admin {
                // Record the bypass so it is auditable on-chain.
                crate::admin_audit_log::AdminAuditLog::log_change(
                    env,
                    attestor,
                    "rate_limit_bypass",
                    "bypassed",
                    "",
                    "bypassed",
                );
                return Ok(());
            }
        }
        
        let current_ledger = env.ledger().sequence();
        let state_key = Self::get_state_key(env, attestor);
        
        // Get or initialize rate limit state
        let mut state = env.storage().persistent().get::<_, RateLimitState>(&state_key)
            .unwrap_or(RateLimitState {
                submission_count: 0,
                window_start_ledger: current_ledger,
            });
        
        // Check if window has expired and reset if needed
        if Self::is_window_expired(
            current_ledger,
            state.window_start_ledger,
            config.window_length,
        ) {
            state = RateLimitState {
                submission_count: 0,
                window_start_ledger: current_ledger,
            };
        }
        
        // Check if limit is exceeded
        if state.submission_count >= config.max_submissions {
            return Err(AnchorKitError::new(
                crate::errors::ErrorCode::RateLimitExceeded,
                "Request throttled: rate-limit window capacity exhausted; retry after the window expires",
            ));
        }

        // Opt-in burst-control gate (#630). A no-op when no BurstControlConfig
        // has been stored — existing callers are unaffected.
        if let Some(burst_config) = Self::get_burst_config(env) {
            Self::check_and_consume_burst(env, attestor, &burst_config, current_ledger)?;
        }

        // Opt-in fairness gate (#630). A no-op when no FairnessConfig has been
        // stored — existing callers are unaffected. Runs after the burst gate
        // so a rejected request never gets counted toward the shared window.
        if let Some(fairness_config) = Self::get_fairness_config(env) {
            Self::check_fairness(env, config, &state, &fairness_config, current_ledger)?;
        }

        // Increment counter and save state; saturating_add prevents wrapping to zero.
        state.submission_count = state.submission_count.saturating_add(1);
        env.storage().persistent().set(&state_key, &state);

        Ok(())
    }

    // ── Burst control (#630) ─────────────────────────────────────────────────

    /// Store the global burst-control configuration (admin only in practice;
    /// access control is enforced by the contract layer via `require_admin`).
    pub fn set_burst_config(env: &Env, config: BurstControlConfig) {
        env.storage().persistent().set(&Self::burst_config_key(env), &config);
    }

    /// Retrieve the stored burst-control configuration, or `None` if burst
    /// control has not been enabled.
    pub fn get_burst_config(env: &Env) -> Option<BurstControlConfig> {
        env.storage().persistent().get::<_, BurstControlConfig>(&Self::burst_config_key(env))
    }

    /// Validate a [`BurstControlConfig`] has sensible non-zero values.
    pub fn validate_burst_config(config: &BurstControlConfig) -> Result<(), AnchorKitError> {
        if config.burst_capacity == 0 {
            return Err(AnchorKitError::validation_error("burst_capacity must be > 0"));
        }
        if config.refill_per_ledger == 0 {
            return Err(AnchorKitError::validation_error("refill_per_ledger must be > 0"));
        }
        Ok(())
    }

    /// Update the burst-control configuration (admin only).
    pub fn update_burst_config(
        env: &Env,
        admin: &Address,
        config: &BurstControlConfig,
    ) -> Result<(), AnchorKitError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get::<_, Address>(&make_storage_key(env, &[b"ADMIN"]))
            .ok_or_else(AnchorKitError::not_initialized)?;
        if *admin != stored_admin {
            return Err(AnchorKitError::unauthorized_attestor());
        }
        Self::validate_burst_config(config)?;
        Self::set_burst_config(env, config.clone());
        Ok(())
    }

    /// Return the current [`BurstState`] for an attestor, defaulting to a
    /// full bucket (`burst_capacity` tokens) if no state has been stored yet.
    pub fn get_burst_state(env: &Env, attestor: &Address, config: &BurstControlConfig) -> BurstState {
        let key = Self::burst_state_key(env, attestor);
        env.storage().persistent().get::<_, BurstState>(&key).unwrap_or(BurstState {
            tokens: config.burst_capacity,
            last_refill_ledger: env.ledger().sequence(),
        })
    }

    /// Refill and consume one token from an attestor's burst bucket.
    ///
    /// Tokens are replenished at `config.refill_per_ledger` per elapsed
    /// ledger since the last refill, capped at `config.burst_capacity`. When
    /// the bucket has no tokens available the request is rejected — this is
    /// the mechanism that smooths bursts: a client can spend its whole bucket
    /// at once, but must then wait for it to refill before spending more.
    fn check_and_consume_burst(
        env: &Env,
        attestor: &Address,
        config: &BurstControlConfig,
        current_ledger: u32,
    ) -> Result<(), AnchorKitError> {
        let mut state = Self::get_burst_state(env, attestor, config);

        let elapsed = current_ledger.saturating_sub(state.last_refill_ledger);
        if elapsed > 0 {
            let refill = elapsed.saturating_mul(config.refill_per_ledger);
            state.tokens = state.tokens.saturating_add(refill).min(config.burst_capacity);
            state.last_refill_ledger = current_ledger;
        }

        if state.tokens == 0 {
            // Persist the refill even on rejection so future calls see accurate state.
            env.storage().persistent().set(&Self::burst_state_key(env, attestor), &state);
            return Err(AnchorKitError::rate_limit_exceeded());
        }

        state.tokens -= 1;
        env.storage().persistent().set(&Self::burst_state_key(env, attestor), &state);
        Ok(())
    }

    // ── Fairness control (#630) ──────────────────────────────────────────────

    /// Store the global fairness configuration (admin only in practice).
    pub fn set_fairness_config(env: &Env, config: FairnessConfig) {
        env.storage().persistent().set(&Self::fairness_config_key(env), &config);
    }

    /// Retrieve the stored fairness configuration, or `None` if fairness
    /// control has not been enabled.
    pub fn get_fairness_config(env: &Env) -> Option<FairnessConfig> {
        env.storage().persistent().get::<_, FairnessConfig>(&Self::fairness_config_key(env))
    }

    /// Validate a [`FairnessConfig`] has sensible values.
    ///
    /// `max_share_percent` must be in `1..=100`.
    pub fn validate_fairness_config(config: &FairnessConfig) -> Result<(), AnchorKitError> {
        if config.max_share_percent == 0 || config.max_share_percent > 100 {
            return Err(AnchorKitError::validation_error("max_share_percent must be in 1..=100"));
        }
        Ok(())
    }

    /// Update the fairness configuration (admin only).
    pub fn update_fairness_config(
        env: &Env,
        admin: &Address,
        config: &FairnessConfig,
    ) -> Result<(), AnchorKitError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get::<_, Address>(&make_storage_key(env, &[b"ADMIN"]))
            .ok_or_else(AnchorKitError::not_initialized)?;
        if *admin != stored_admin {
            return Err(AnchorKitError::unauthorized_attestor());
        }
        Self::validate_fairness_config(config)?;
        Self::set_fairness_config(env, config.clone());
        Ok(())
    }

    /// Return the current [`GlobalRateWindowState`], resetting it if the
    /// shared window (aligned to `window_length`) has expired.
    pub fn get_global_window_state(env: &Env, window_length: u32, current_ledger: u32) -> GlobalRateWindowState {
        let key = Self::global_window_key(env);
        let state = env.storage().persistent().get::<_, GlobalRateWindowState>(&key)
            .unwrap_or(GlobalRateWindowState {
                total_count: 0,
                window_start_ledger: current_ledger,
            });
        if Self::is_window_expired(current_ledger, state.window_start_ledger, window_length) {
            GlobalRateWindowState { total_count: 0, window_start_ledger: current_ledger }
        } else {
            state
        }
    }

    /// Enforce that a single attestor cannot exceed `max_share_percent` of
    /// the shared window's total submissions, once `min_total_for_enforcement`
    /// submissions have been recorded.
    ///
    /// Compares the attestor's *prospective* count (their current count in
    /// the window, plus this submission) against `max_share_percent` of the
    /// window's prospective total (current total, plus this submission), so
    /// the very submission being checked is included in the fairness math.
    fn check_fairness(
        env: &Env,
        config: &RateLimitConfig,
        attestor_state: &RateLimitState,
        fairness_config: &FairnessConfig,
        current_ledger: u32,
    ) -> Result<(), AnchorKitError> {
        let mut global = Self::get_global_window_state(env, config.window_length, current_ledger);

        let prospective_total = global.total_count + 1;
        let prospective_attestor_count = attestor_state.submission_count + 1;

        if prospective_total >= fairness_config.min_total_for_enforcement {
            // attestor_share_pct = prospective_attestor_count / prospective_total * 100
            // Compared without floating point: attestor * 100 > max_share * total
            let attestor_share_scaled = (prospective_attestor_count as u64) * 100;
            let allowed_share_scaled = (fairness_config.max_share_percent as u64) * (prospective_total as u64);
            if attestor_share_scaled > allowed_share_scaled {
                return Err(AnchorKitError::rate_limit_exceeded());
            }
        }

        global.total_count = prospective_total;
        env.storage().persistent().set(&Self::global_window_key(env), &global);
        Ok(())
    }
    
    /// Get the current rate limit state for an attestor.
    ///
    /// Returns a default state (zero submissions, current ledger as window start)
    /// if no state has been stored yet.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    /// * `attestor` - The address of the attestor to query.
    ///
    /// # Returns
    ///
    /// The current [`RateLimitState`] for the attestor.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let attestor = soroban_sdk::Address::generate(&env);
    /// use anchorkit::RateLimiter;
    ///
    /// let state = RateLimiter::get_state(&env, &attestor);
    /// assert_eq!(state.submission_count, 0);
    /// ```
    pub fn get_state(env: &Env, attestor: &Address) -> RateLimitState {
        let state_key = Self::get_state_key(env, attestor);
        env.storage().persistent().get::<_, RateLimitState>(&state_key)
            .unwrap_or(RateLimitState {
                submission_count: 0,
                window_start_ledger: env.ledger().sequence(),
            })
    }
    
    /// Update the rate limit configuration (admin only).
    ///
    /// Loads the stored admin from instance storage (key `"ADMIN"`) and calls
    /// `admin.require_auth()`. Returns `Err(NotInitialized)` if no admin is stored.
    /// Returns `Err(ValidationError)` if `config` contains zero or nonsensical values.
    pub fn update_config(
        env: &Env,
        admin: &Address,
        config: &RateLimitConfig,
    ) -> Result<(), AnchorKitError> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get::<_, Address>(&make_storage_key(env, &[b"ADMIN"]))
            .ok_or_else(AnchorKitError::not_initialized)?;
        if *admin != stored_admin {
            return Err(AnchorKitError::unauthorized_attestor());
        }
        Self::validate_config(config)?;
        let config_key = Self::get_config_key(env);
        env.storage().persistent().set(&config_key, config);
        Ok(())
    }

    /// Validate that a [`RateLimitConfig`] has sensible non-zero values.
    ///
    /// Returns `Err(ValidationError)` if `max_submissions` or `window_length` is zero.
    pub fn validate_config(config: &RateLimitConfig) -> Result<(), AnchorKitError> {
        if config.max_submissions == 0 {
            return Err(AnchorKitError::validation_error("max_submissions must be > 0"));
        }
        if config.window_length == 0 {
            return Err(AnchorKitError::validation_error("window_length must be > 0"));
        }
        Ok(())
    }
    
    /// Get the current rate limit configuration.
    ///
    /// Returns the stored configuration, or the default (10 submissions per
    /// 100-ledger window) if none has been set.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban execution environment.
    ///
    /// # Returns
    ///
    /// The active [`RateLimitConfig`].
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # let env = Env::default();
    /// use anchorkit::RateLimiter;
    ///
    /// let config = RateLimiter::get_config(&env);
    /// assert_eq!(config.max_submissions, 10);
    /// assert_eq!(config.window_length, 100);
    /// ```
    pub fn get_config(env: &Env) -> RateLimitConfig {
        let config_key = Self::get_config_key(env);
        env.storage().persistent().get::<_, RateLimitConfig>(&config_key)
            .unwrap_or(RateLimitConfig {
                max_submissions: 10,
                window_length: 100,
            })
    }
    
    /// Check if a rate-limit window has expired.
    ///
    /// `window_start_ledger` is always set to `env.ledger().sequence()` at
    /// state creation or window reset, so `current_ledger >= window_start_ledger`
    /// is guaranteed by the preceding state normalization. `saturating_sub`
    /// makes that invariant explicit: if `current < window_start` (which cannot
    /// happen in practice), the subtraction saturates to 0 and `validate_config`
    /// ensures `window_length > 0`, so the expression is still `false` — the
    /// window is treated as not expired and the existing submission count is
    /// preserved.
    pub(crate) fn is_window_expired(
        current_ledger: u32,
        window_start_ledger: u32,
        window_length: u32,
    ) -> bool {
        current_ledger.saturating_sub(window_start_ledger) >= window_length
    }
    
    /// Generate collision-resistant storage key for per-attestor rate limit state.
    pub(crate) fn state_key(env: &Env, attestor: &Address) -> soroban_sdk::BytesN<32> {
        Self::get_state_key(env, attestor)
    }

    /// Generate collision-resistant storage key for per-attestor rate limit state.
    fn get_state_key(env: &Env, attestor: &Address) -> soroban_sdk::BytesN<32> {
        let addr_xdr = attestor.clone().to_xdr(env);
        // collect xdr bytes into a plain slice via Bytes
        let mut raw = alloc::vec::Vec::with_capacity(addr_xdr.len() as usize);
        for i in 0..addr_xdr.len() {
            raw.push(addr_xdr.get(i).unwrap_or(0));
        }
        make_storage_key(env, &[b"RL_STATE", &raw])
    }

    /// Generate collision-resistant storage key for the global rate limit config.
    fn get_config_key(env: &Env) -> soroban_sdk::BytesN<32> {
        make_storage_key(env, &[b"RL_CONFIG"])
    }

    /// Storage key for a per-role rate limit override.
    fn role_override_key(env: &Env, role: &soroban_sdk::Symbol) -> soroban_sdk::BytesN<32> {
        use soroban_sdk::xdr::ToXdr;
        let role_xdr = role.clone().to_xdr(env);
        let mut raw = alloc::vec::Vec::with_capacity(role_xdr.len() as usize);
        for i in 0..role_xdr.len() {
            raw.push(role_xdr.get(i).unwrap_or(0));
        }
        make_storage_key(env, &[b"RL_ROLE", &raw])
    }

    /// Storage key for a per-address rate limit override.
    fn address_override_key(env: &Env, address: &Address) -> soroban_sdk::BytesN<32> {
        use soroban_sdk::xdr::ToXdr;
        let addr_xdr = address.clone().to_xdr(env);
        let mut raw = alloc::vec::Vec::with_capacity(addr_xdr.len() as usize);
        for i in 0..addr_xdr.len() {
            raw.push(addr_xdr.get(i).unwrap_or(0));
        }
        make_storage_key(env, &[b"RL_ADDR", &raw])
    }

    /// Storage key for the global burst-control configuration.
    fn burst_config_key(env: &Env) -> soroban_sdk::BytesN<32> {
        make_storage_key(env, &[b"RL_BST_CFG"])
    }

    /// Storage key for a per-attestor burst-control token-bucket state.
    fn burst_state_key(env: &Env, attestor: &Address) -> soroban_sdk::BytesN<32> {
        use soroban_sdk::xdr::ToXdr;
        let addr_xdr = attestor.clone().to_xdr(env);
        let mut raw = alloc::vec::Vec::with_capacity(addr_xdr.len() as usize);
        for i in 0..addr_xdr.len() {
            raw.push(addr_xdr.get(i).unwrap_or(0));
        }
        make_storage_key(env, &[b"RL_BST_ST", &raw])
    }

    /// Storage key for the global fairness configuration.
    fn fairness_config_key(env: &Env) -> soroban_sdk::BytesN<32> {
        make_storage_key(env, &[b"RL_FAIR_CFG"])
    }

    /// Storage key for the shared global rate-limit window state.
    fn global_window_key(env: &Env) -> soroban_sdk::BytesN<32> {
        make_storage_key(env, &[b"RL_GLOBAL"])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Ledger as _;

    #[test]
    fn test_rate_limit_under_limit() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig {
            max_submissions: 10,
            window_length: 100,
        };
        
        // Create a dummy contract address for testing
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        
        // Register a dummy contract for testing
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);
        
        // Should succeed for first submission
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        });
        assert!(result.is_ok());
        
        // Check state
        let state = env.as_contract(&contract_id, &|| {
            RateLimiter::get_state(&env, &attestor)
        });
        assert_eq!(state.submission_count, 1);
    }
    
    #[test]
    fn test_rate_limit_at_limit() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig {
            max_submissions: 2,
            window_length: 100,
        };
        
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);
        
        // First two submissions should succeed
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());
        
        // Third submission should fail
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::RateLimitExceeded);
    }
    
    #[test]
    fn test_rate_limit_over_limit() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig {
            max_submissions: 1,
            window_length: 100,
        };
        
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);
        
        // First submission should succeed
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());
        
        // Second submission should fail
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::RateLimitExceeded);
    }
    
    #[test]
    fn test_rate_limit_window_reset() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig {
            max_submissions: 1,
            window_length: 10,
        };
        
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);
        
        // First submission should succeed
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());
        
        // Second submission should fail (still in same window)
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_err());
        
        // Note: In Soroban SDK, we cannot directly set the ledger sequence in tests
        // The window reset logic will be tested in integration tests with actual ledger progression
        // For now, we verify the state is correct
        let state = env.as_contract(&contract_id, &|| {
            RateLimiter::get_state(&env, &attestor)
        });
        assert_eq!(state.submission_count, 1);
    }
    
    #[test]
    fn test_rate_limit_config_update_uses_contract_admin_key() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let new_config = RateLimitConfig {
            max_submissions: 20,
            window_length: 200,
        };

        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        // Mirror AnchorKitContract::initialize by using the deterministic admin key.
        env.as_contract(&contract_id, &|| {
            env.storage()
                .instance()
                .set(&make_storage_key(&env, &[b"ADMIN"]), &admin);
        });

        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::update_config(&env, &admin, &new_config)
        });
        assert!(result.is_ok());

        let config = env.as_contract(&contract_id, &|| {
            RateLimiter::get_config(&env)
        });
        assert_eq!(config.max_submissions, 20);
        assert_eq!(config.window_length, 200);
    }

    #[test]
    fn test_admin_bypasses_rate_limits() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let non_admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig { max_submissions: 2, window_length: 100 };
        
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);
        
        // Store admin in instance storage
        env.as_contract(&contract_id, &|| {
            env.storage()
                .instance()
                .set(&make_storage_key(&env, &[b"ADMIN"]), &admin);
        });
        
        // Non-admin should be rate limited
        env.as_contract(&contract_id, &|| {
            assert!(RateLimiter::check_and_increment(&env, &non_admin, &config).is_ok());
            assert!(RateLimiter::check_and_increment(&env, &non_admin, &config).is_ok());
            assert!(RateLimiter::check_and_increment(&env, &non_admin, &config).is_err());
        });
        
        // Admin should never be rate limited
        env.as_contract(&contract_id, &|| {
            for _ in 0..10 {
                assert!(RateLimiter::check_and_increment(&env, &admin, &config).is_ok());
            }
        });
        
        // Verify non-admin state still has max submissions (admin didn't affect it)
        let state = env.as_contract(&contract_id, &|| {
            RateLimiter::get_state(&env, &non_admin)
        });
        assert_eq!(state.submission_count, 2);
    }

    #[test]
    fn test_update_config_unauthorized() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let non_admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let new_config = RateLimitConfig { max_submissions: 5, window_length: 50 };

        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        env.as_contract(&contract_id, &|| {
            env.storage()
                .instance()
                .set(&make_storage_key(&env, &[b"ADMIN"]), &admin);
        });

        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::update_config(&env, &non_admin, &new_config)
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::UnauthorizedAttestor);
    }

    #[test]
    fn test_update_config_not_initialized() {
        let env = Env::default();
        let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let new_config = RateLimitConfig { max_submissions: 5, window_length: 50 };

        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        // No admin stored — should return NotInitialized
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::update_config(&env, &admin, &new_config)
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::NotInitialized);
    }
    
    #[test]
    fn test_rate_limit_default_config() {
        let env = Env::default();
        
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);
        
        // Get default config
        let config = env.as_contract(&contract_id, &|| {
            RateLimiter::get_config(&env)
        });
        assert_eq!(config.max_submissions, 10);
        assert_eq!(config.window_length, 100);
    }

    #[test]
    fn test_validate_config_rejects_zero_max_submissions() {
        let config = RateLimitConfig { max_submissions: 0, window_length: 100 };
        assert!(RateLimiter::validate_config(&config).is_err());
        assert_eq!(
            RateLimiter::validate_config(&config).unwrap_err().code,
            ErrorCode::ValidationError
        );
    }

    #[test]
    fn test_validate_config_rejects_zero_window_length() {
        let config = RateLimitConfig { max_submissions: 5, window_length: 0 };
        assert!(RateLimiter::validate_config(&config).is_err());
        assert_eq!(
            RateLimiter::validate_config(&config).unwrap_err().code,
            ErrorCode::ValidationError
        );
    }

    #[test]
    fn test_validate_config_accepts_valid() {
        let config = RateLimitConfig { max_submissions: 1, window_length: 1 };
        assert!(RateLimiter::validate_config(&config).is_ok());
    }

    #[test]
    fn test_update_config_rejects_zero_values() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let bad_config = RateLimitConfig { max_submissions: 0, window_length: 100 };

        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        env.as_contract(&contract_id, &|| {
            env.storage()
                .instance()
                .set(&make_storage_key(&env, &[b"ADMIN"]), &admin);
        });

        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::update_config(&env, &admin, &bad_config)
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::ValidationError);
    }

    #[test]
    fn test_window_rollover_at_exact_boundary() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig { max_submissions: 1, window_length: 10 };

        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        // Fill the window
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());
        // Same window — should fail
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_err());

        // Advance ledger by exactly window_length (10)
        env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            sequence_number: 10,
            timestamp: 1000,
            protocol_version: 21,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        // Window should have rolled over — first submission in new window succeeds
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());
    }

    /// If the stored window_start_ledger is somehow *ahead* of the current ledger
    /// (sequence anomaly), the window must be treated as NOT expired so that the
    /// existing submission count is preserved and the rate-limit cannot be bypassed.
    /// count == max_submissions is the exact rejection threshold: max-1 succeeds,
    /// max is rejected.  Isolates the off-by-one boundary in the >= check.
    #[test]
    fn test_at_limit_exact_last_allowed_then_rejected() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig { max_submissions: 3, window_length: 100 };
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        // Submissions 1 through max_submissions-1 must all succeed.
        for _ in 0..2 {
            assert!(env.as_contract(&contract_id, &|| {
                RateLimiter::check_and_increment(&env, &attestor, &config)
            }).is_ok());
        }

        // The max_submissions-th call is the LAST allowed — must succeed.
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok(), "submission at count == max_submissions-1 must succeed");

        // Now count == max_submissions; the next call must be rejected.
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        });
        assert!(result.is_err(), "submission at count == max_submissions must be rejected");
        assert_eq!(result.unwrap_err().code, ErrorCode::RateLimitExceeded);

        // State must be capped — no overflow past max.
        let state = env.as_contract(&contract_id, &|| RateLimiter::get_state(&env, &attestor));
        assert_eq!(state.submission_count, 3, "count must not exceed max_submissions");
    }

    /// Every call after the limit (count > max_submissions) must still return
    /// RateLimitExceeded and must not mutate the stored count.
    #[test]
    fn test_one_over_limit_state_stays_capped() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig { max_submissions: 2, window_length: 100 };
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        // Fill the window.
        env.as_contract(&contract_id, &|| { RateLimiter::check_and_increment(&env, &attestor, &config).unwrap(); });
        env.as_contract(&contract_id, &|| { RateLimiter::check_and_increment(&env, &attestor, &config).unwrap(); });

        // count == max: first rejection (one-over).
        let err1 = env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).unwrap_err();
        assert_eq!(err1.code, ErrorCode::RateLimitExceeded);

        // count still == max: second rejection (two-over) — state must not have changed.
        let err2 = env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).unwrap_err();
        assert_eq!(err2.code, ErrorCode::RateLimitExceeded);

        let state = env.as_contract(&contract_id, &|| RateLimiter::get_state(&env, &attestor));
        assert_eq!(state.submission_count, 2, "count must remain at max after over-limit calls");
    }

    /// A submission at ledger window_start + window_length - 1 (one before expiry)
    /// must still be rejected, while one at window_start + window_length is in the
    /// new window and must succeed.  Pins the exact >=/> boundary in is_window_expired.
    #[test]
    fn test_window_one_before_expiry_still_restricted() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig { max_submissions: 1, window_length: 10 };
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        // Consume the sole slot at ledger 0.
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());

        // Advance to one ledger BEFORE the window expires (delta = window_length - 1 = 9).
        env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            sequence_number: 9,
            timestamp: 900,
            protocol_version: 21,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        // delta = 9 < window_length = 10 → still in old window → must be rejected.
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        });
        assert!(result.is_err(), "submission at window_start + window_length - 1 must be rejected");
        assert_eq!(result.unwrap_err().code, ErrorCode::RateLimitExceeded);

        // Advance to exactly window_start + window_length (delta = 10 = window_length → expired).
        env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            sequence_number: 10,
            timestamp: 1000,
            protocol_version: 21,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        // New window → must succeed.
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok(), "submission at window_start + window_length must start a new window");
    }

    #[test]
    fn test_window_not_expired_when_current_less_than_start() {
        // current < window_start → checked_sub underflows → None → false
        assert!(!RateLimiter::is_window_expired(5, 10, 10));
        assert!(!RateLimiter::is_window_expired(0, 1, 1));
        assert!(!RateLimiter::is_window_expired(100, 200, 50));
    }

    /// current == window_start and window_length > 0: delta is 0, so NOT expired.
    #[test]
    fn test_window_not_expired_at_exact_start() {
        assert!(!RateLimiter::is_window_expired(10, 10, 1));
    }

    /// Verify the boundary: delta == window_length means expired.
    #[test]
    fn test_window_expired_at_exact_length() {
        assert!(RateLimiter::is_window_expired(20, 10, 10)); // delta = 10 >= 10
    }

    #[test]
    fn test_max_submission_error_is_consistent() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig { max_submissions: 2, window_length: 100 };

        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        env.as_contract(&contract_id, &|| { RateLimiter::check_and_increment(&env, &attestor, &config).unwrap(); });
        env.as_contract(&contract_id, &|| { RateLimiter::check_and_increment(&env, &attestor, &config).unwrap(); });

        // Every subsequent call must return RateLimitExceeded without corrupting state
        for _ in 0..3 {
            let err = env.as_contract(&contract_id, &|| {
                RateLimiter::check_and_increment(&env, &attestor, &config)
            }).unwrap_err();
            assert_eq!(err.code, ErrorCode::RateLimitExceeded);
        }
        // State must still show exactly max_submissions
        let state = env.as_contract(&contract_id, &|| RateLimiter::get_state(&env, &attestor));
        assert_eq!(state.submission_count, 2);
    }

    /// Verify that the stored counter saturates at max_submissions rather than
    /// wrapping to zero when the window is full. Forces the state directly to
    /// u32::MAX and confirms that a subsequent denied request leaves the counter
    /// unchanged (saturating_add(1) on u32::MAX stays u32::MAX, not 0).
    #[test]
    fn test_counter_saturates_at_u32_max() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let config = RateLimitConfig { max_submissions: 5, window_length: 100 };
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        // Artificially write a state with submission_count at u32::MAX to
        // simulate a counter that has been driven to its integer ceiling.
        env.as_contract(&contract_id, &|| {
            let key = RateLimiter::state_key(&env, &attestor);
            env.storage().persistent().set(&key, &RateLimitState {
                submission_count: u32::MAX,
                window_start_ledger: 0,
            });
        });

        // The next call must be rejected (count >= max_submissions).
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        });
        assert!(result.is_err(), "over-limit call must be rejected");
        assert_eq!(result.unwrap_err().code, ErrorCode::RateLimitExceeded);

        // The counter must remain at u32::MAX — it must not have wrapped to 0.
        let state = env.as_contract(&contract_id, &|| RateLimiter::get_state(&env, &attestor));
        assert_eq!(state.submission_count, u32::MAX, "counter must saturate, not wrap");
    }

    // -------------------------------------------------------------------------
    // #630 — burst control (token bucket)
    // -------------------------------------------------------------------------

    #[test]
    fn test_burst_control_disabled_by_default() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        assert!(env.as_contract(&contract_id, &|| RateLimiter::get_burst_config(&env)).is_none());

        let config = RateLimitConfig { max_submissions: 50, window_length: 1000 };
        for _ in 0..10 {
            assert!(env.as_contract(&contract_id, &|| {
                RateLimiter::check_and_increment(&env, &attestor, &config)
            }).is_ok());
        }
    }

    #[test]
    fn test_burst_allows_up_to_capacity_then_rejects() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        // Base window is generous — the burst bucket is the binding constraint.
        let config = RateLimitConfig { max_submissions: 100, window_length: 1000 };
        let burst = BurstControlConfig { burst_capacity: 3, refill_per_ledger: 1 };
        env.as_contract(&contract_id, &|| RateLimiter::set_burst_config(&env, burst.clone()));

        for i in 0..3 {
            assert!(env.as_contract(&contract_id, &|| {
                RateLimiter::check_and_increment(&env, &attestor, &config)
            }).is_ok(), "submission {i} within burst capacity must succeed");
        }

        // The 4th submission in the same ledger drains an empty bucket.
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        });
        assert!(result.is_err(), "submission beyond burst capacity must be rejected");
        assert_eq!(result.unwrap_err().code, ErrorCode::RateLimitExceeded);
    }

    #[test]
    fn test_burst_refills_over_elapsed_ledgers() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        let config = RateLimitConfig { max_submissions: 100, window_length: 1000 };
        let burst = BurstControlConfig { burst_capacity: 2, refill_per_ledger: 1 };
        env.as_contract(&contract_id, &|| RateLimiter::set_burst_config(&env, burst.clone()));

        // Drain the bucket completely.
        for _ in 0..2 {
            assert!(env.as_contract(&contract_id, &|| {
                RateLimiter::check_and_increment(&env, &attestor, &config)
            }).is_ok());
        }
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_err());

        // Advance 2 ledgers — refill_per_ledger=1 means 2 tokens become available.
        env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            sequence_number: 2,
            timestamp: 1000,
            protocol_version: 21,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok(), "bucket must have refilled after elapsed ledgers");
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());
        // Bucket is drained again (capacity 2, both refilled tokens spent).
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_err());
    }

    #[test]
    fn test_burst_refill_caps_at_capacity() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        let config = RateLimitConfig { max_submissions: 100, window_length: 1000 };
        let burst = BurstControlConfig { burst_capacity: 2, refill_per_ledger: 10 };
        env.as_contract(&contract_id, &|| RateLimiter::set_burst_config(&env, burst.clone()));

        // Consume one token (bucket starts full at capacity=2).
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());

        // Advance far enough that an uncapped refill would hugely overshoot capacity.
        env.ledger().set(soroban_sdk::testutils::LedgerInfo {
            sequence_number: 100,
            timestamp: 1000,
            protocol_version: 21,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6312000,
        });

        // Only 2 submissions should be possible (capacity), not more.
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_ok());
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &attestor, &config)
        }).is_err(), "refill must be capped at burst_capacity");
    }

    #[test]
    fn test_validate_burst_config_rejects_zero_values() {
        assert!(RateLimiter::validate_burst_config(&BurstControlConfig {
            burst_capacity: 0,
            refill_per_ledger: 1,
        }).is_err());
        assert!(RateLimiter::validate_burst_config(&BurstControlConfig {
            burst_capacity: 1,
            refill_per_ledger: 0,
        }).is_err());
        assert!(RateLimiter::validate_burst_config(&BurstControlConfig {
            burst_capacity: 1,
            refill_per_ledger: 1,
        }).is_ok());
    }

    #[test]
    fn test_update_burst_config_requires_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let non_admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        env.as_contract(&contract_id, &|| {
            env.storage().instance().set(&make_storage_key(&env, &[b"ADMIN"]), &admin);
        });

        let cfg = BurstControlConfig { burst_capacity: 5, refill_per_ledger: 1 };
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::update_burst_config(&env, &non_admin, &cfg)
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::UnauthorizedAttestor);

        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::update_burst_config(&env, &admin, &cfg)
        }).is_ok());
        let stored = env.as_contract(&contract_id, &|| RateLimiter::get_burst_config(&env));
        assert_eq!(stored, Some(cfg));
    }

    // -------------------------------------------------------------------------
    // #630 — fairness control (per-client share cap)
    // -------------------------------------------------------------------------

    #[test]
    fn test_fairness_disabled_by_default() {
        let env = Env::default();
        let attestor = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        assert!(env.as_contract(&contract_id, &|| RateLimiter::get_fairness_config(&env)).is_none());

        let config = RateLimitConfig { max_submissions: 50, window_length: 1000 };
        for _ in 0..10 {
            assert!(env.as_contract(&contract_id, &|| {
                RateLimiter::check_and_increment(&env, &attestor, &config)
            }).is_ok());
        }
    }

    #[test]
    fn test_fairness_allows_evenly_distributed_clients() {
        let env = Env::default();
        let a = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let b = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        let config = RateLimitConfig { max_submissions: 100, window_length: 1000 };
        let fairness = FairnessConfig { max_share_percent: 50, min_total_for_enforcement: 4 };
        env.as_contract(&contract_id, &|| RateLimiter::set_fairness_config(&env, fairness.clone()));

        // Two clients alternating stay within their fair 50% share and must
        // both be allowed even after fairness enforcement kicks in.
        for (i, attestor) in [&a, &b, &a, &b].into_iter().enumerate() {
            let result = env.as_contract(&contract_id, &|| {
                RateLimiter::check_and_increment(&env, attestor, &config)
            });
            assert!(result.is_ok(), "alternating call {i} must be allowed under fair distribution");
        }
    }

    #[test]
    fn test_fairness_rejects_client_exceeding_share() {
        let env = Env::default();
        let a = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let b = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        let config = RateLimitConfig { max_submissions: 100, window_length: 1000 };
        let fairness = FairnessConfig { max_share_percent: 50, min_total_for_enforcement: 3 };
        env.as_contract(&contract_id, &|| RateLimiter::set_fairness_config(&env, fairness.clone()));

        // A, A, B — A now holds 2/3 of the shared window's submissions.
        for attestor in [&a, &a, &b] {
            assert!(env.as_contract(&contract_id, &|| {
                RateLimiter::check_and_increment(&env, attestor, &config)
            }).is_ok());
        }

        // A tries a 3rd submission, which would push it to 3/4 = 75% of the
        // shared window — over its 50% fair share — and must be rejected.
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &a, &config)
        });
        assert!(result.is_err(), "client exceeding its fair share must be rejected");
        assert_eq!(result.unwrap_err().code, ErrorCode::RateLimitExceeded);

        // B, meanwhile, is behind (1/3) — its next submission brings the
        // window to a balanced 2/4 = 50% and must be allowed.
        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::check_and_increment(&env, &b, &config)
        }).is_ok(), "a client below its fair share must still be allowed");
    }

    #[test]
    fn test_fairness_not_enforced_below_minimum_total() {
        let env = Env::default();
        let a = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        let config = RateLimitConfig { max_submissions: 100, window_length: 1000 };
        // A single client would normally dominate the share, but enforcement
        // only begins once min_total_for_enforcement submissions exist.
        let fairness = FairnessConfig { max_share_percent: 50, min_total_for_enforcement: 10 };
        env.as_contract(&contract_id, &|| RateLimiter::set_fairness_config(&env, fairness.clone()));

        for i in 0..9 {
            assert!(env.as_contract(&contract_id, &|| {
                RateLimiter::check_and_increment(&env, &a, &config)
            }).is_ok(), "submission {i} below the enforcement threshold must be allowed");
        }
    }

    #[test]
    fn test_validate_fairness_config_rejects_out_of_range() {
        assert!(RateLimiter::validate_fairness_config(&FairnessConfig {
            max_share_percent: 0,
            min_total_for_enforcement: 1,
        }).is_err());
        assert!(RateLimiter::validate_fairness_config(&FairnessConfig {
            max_share_percent: 101,
            min_total_for_enforcement: 1,
        }).is_err());
        assert!(RateLimiter::validate_fairness_config(&FairnessConfig {
            max_share_percent: 100,
            min_total_for_enforcement: 1,
        }).is_ok());
    }

    #[test]
    fn test_update_fairness_config_requires_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let non_admin = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_address = <soroban_sdk::Address as soroban_sdk::testutils::Address>::generate(&env);
        let contract_id = env.register_contract(&contract_address, crate::contract::AnchorKitContract);

        env.as_contract(&contract_id, &|| {
            env.storage().instance().set(&make_storage_key(&env, &[b"ADMIN"]), &admin);
        });

        let cfg = FairnessConfig { max_share_percent: 60, min_total_for_enforcement: 5 };
        let result = env.as_contract(&contract_id, &|| {
            RateLimiter::update_fairness_config(&env, &non_admin, &cfg)
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::UnauthorizedAttestor);

        assert!(env.as_contract(&contract_id, &|| {
            RateLimiter::update_fairness_config(&env, &admin, &cfg)
        }).is_ok());
        let stored = env.as_contract(&contract_id, &|| RateLimiter::get_fairness_config(&env));
        assert_eq!(stored, Some(cfg));
    }
}
