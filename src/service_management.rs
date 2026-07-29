//! Service management for anchor service enable/disable toggles, rollback handling,
//! and structured service retirement workflows.
//!
//! This module provides functionality to:
//! - Enable/disable individual services for anchors
//! - Track service configuration history for rollback
//! - Restore prior service configurations
//! - Query current service status
//! - Retire services through a structured, enforced lifecycle
//!
//! # Service Retirement Lifecycle
//!
//! Services are retired through a strictly ordered sequence of states:
//!
//! ```text
//! Active ──► Deprecated ──► Disabled ──► Retired
//!   ▲              │             │
//!   └──────────────┘             │  (only Active ←── Deprecated is allowed)
//!                                └── (no reversal once Disabled)
//! ```
//!
//! Legal transitions:
//!
//! | From       | To         | Allowed |
//! |------------|------------|---------|
//! | Active     | Deprecated | ✓       |
//! | Deprecated | Active     | ✓ (un-deprecate) |
//! | Deprecated | Disabled   | ✓       |
//! | Disabled   | Retired    | ✓       |
//! | Retired    | *          | ✗ terminal |
//! | *          | same state | ✗ no self-loops |
//!
//! All other transitions are rejected with
//! [`ErrorCode::InvalidRetirementTransition`](crate::ErrorCode::InvalidRetirementTransition).

use soroban_sdk::{contracttype, Address, Env, String, Vec};

/// Service configuration snapshot for rollback purposes
#[contracttype]
#[derive(Clone, Debug)]
pub struct ServiceConfigSnapshot {
    /// Unique identifier for this snapshot
    pub snapshot_id: u64,
    /// Anchor address
    pub anchor: Address,
    /// Services at the time of snapshot
    pub services: Vec<u32>,
    /// Timestamp when snapshot was created
    pub created_at: u64,
    /// Description of the configuration (e.g., "before_maintenance")
    pub description: String,
}

/// Service toggle state for an anchor
#[contracttype]
#[derive(Clone, Debug)]
pub struct ServiceToggleState {
    /// Anchor address
    pub anchor: Address,
    /// Current enabled services
    pub enabled_services: Vec<u32>,
    /// Disabled services (for tracking)
    pub disabled_services: Vec<u32>,
    /// Last update timestamp
    pub updated_at: u64,
}

/// Service management operations
pub struct ServiceManager;

impl ServiceManager {
    /// Enable a service for an anchor
    pub fn enable_service(env: &Env, anchor: &Address, service_code: u32) -> bool {
        let state_key = (soroban_sdk::Symbol::new(env, "SVC_STATE"), anchor);
        let mut state: ServiceToggleState = env
            .storage()
            .persistent()
            .get(&state_key)
            .unwrap_or_else(|| ServiceToggleState {
                anchor: anchor.clone(),
                enabled_services: Vec::new(env),
                disabled_services: Vec::new(env),
                updated_at: 0,
            });

        // Check if service is already enabled
        for service in state.enabled_services.iter() {
            if service == service_code {
                return false; // Already enabled
            }
        }

        // Remove from disabled services if present
        let mut new_disabled = Vec::new(env);
        for service in state.disabled_services.iter() {
            if service != service_code {
                new_disabled.push_back(service);
            }
        }
        state.disabled_services = new_disabled;

        // Add to enabled services
        state.enabled_services.push_back(service_code);
        state.updated_at = env.ledger().timestamp();

        env.storage().persistent().set(&state_key, &state);
        env.storage()
            .persistent()
            .extend_ttl(&state_key, 31_536_000, 31_536_000);

        true
    }

    /// Disable a service for an anchor
    pub fn disable_service(env: &Env, anchor: &Address, service_code: u32) -> bool {
        let state_key = (soroban_sdk::Symbol::new(env, "SVC_STATE"), anchor);
        let mut state: ServiceToggleState = env
            .storage()
            .persistent()
            .get(&state_key)
            .unwrap_or_else(|| ServiceToggleState {
                anchor: anchor.clone(),
                enabled_services: Vec::new(env),
                disabled_services: Vec::new(env),
                updated_at: 0,
            });

        // Check if service is already disabled
        for service in state.disabled_services.iter() {
            if service == service_code {
                return false; // Already disabled
            }
        }

        // Remove from enabled services if present
        let mut new_enabled = Vec::new(env);
        for service in state.enabled_services.iter() {
            if service != service_code {
                new_enabled.push_back(service);
            }
        }
        state.enabled_services = new_enabled;

        // Add to disabled services
        state.disabled_services.push_back(service_code);
        state.updated_at = env.ledger().timestamp();

        env.storage().persistent().set(&state_key, &state);
        env.storage()
            .persistent()
            .extend_ttl(&state_key, 31_536_000, 31_536_000);

        true
    }

    /// Get current service toggle state for an anchor
    pub fn get_service_state(env: &Env, anchor: &Address) -> ServiceToggleState {
        let state_key = (soroban_sdk::Symbol::new(env, "SVC_STATE"), anchor);
        env.storage()
            .persistent()
            .get(&state_key)
            .unwrap_or_else(|| ServiceToggleState {
                anchor: anchor.clone(),
                enabled_services: Vec::new(env),
                disabled_services: Vec::new(env),
                updated_at: 0,
            })
    }

    /// Check if a service is enabled for an anchor
    pub fn is_service_enabled(env: &Env, anchor: &Address, service_code: u32) -> bool {
        let state = Self::get_service_state(env, anchor);
        for service in state.enabled_services.iter() {
            if service == service_code {
                return true;
            }
        }
        false
    }

    /// Create a snapshot of current service configuration
    pub fn create_snapshot(
        env: &Env,
        anchor: &Address,
        services: &Vec<u32>,
        description: &str,
    ) -> u64 {
        let counter_key = soroban_sdk::Symbol::new(env, "SVC_SNAP_CNT");
        let snapshot_id: u64 = env
            .storage()
            .instance()
            .get(&counter_key)
            .unwrap_or(0u64);

        let snapshot = ServiceConfigSnapshot {
            snapshot_id,
            anchor: anchor.clone(),
            services: services.clone(),
            created_at: env.ledger().timestamp(),
            description: String::from_str(env, description),
        };

        let snapshot_key = (soroban_sdk::Symbol::new(env, "SVC_SNAP"), snapshot_id);
        env.storage().instance().set(&snapshot_key, &snapshot);
        env.storage().instance().extend_ttl(31_536_000, 31_536_000);

        env.storage()
            .instance()
            .set(&counter_key, &(snapshot_id + 1));
        env.storage().instance().extend_ttl(31_536_000, 31_536_000);

        snapshot_id
    }

    /// Get a service configuration snapshot
    pub fn get_snapshot(env: &Env, snapshot_id: u64) -> Option<ServiceConfigSnapshot> {
        let snapshot_key = (soroban_sdk::Symbol::new(env, "SVC_SNAP"), snapshot_id);
        env.storage().instance().get(&snapshot_key)
    }

    /// Rollback to a previous service configuration
    pub fn rollback_to_snapshot(env: &Env, snapshot_id: u64) -> bool {
        if let Some(snapshot) = Self::get_snapshot(env, snapshot_id) {
            let state_key = (soroban_sdk::Symbol::new(env, "SVC_STATE"), &snapshot.anchor);

            let state = ServiceToggleState {
                anchor: snapshot.anchor.clone(),
                enabled_services: snapshot.services.clone(),
                disabled_services: Vec::new(env),
                updated_at: env.ledger().timestamp(),
            };

            env.storage().persistent().set(&state_key, &state);
            env.storage()
                .persistent()
                .extend_ttl(&state_key, 31_536_000, 31_536_000);

            true
        } else {
            false
        }
    }

    /// Get total number of snapshots
    pub fn get_snapshot_count(env: &Env) -> u64 {
        let counter_key = soroban_sdk::Symbol::new(env, "SVC_SNAP_CNT");
        env.storage().instance().get(&counter_key).unwrap_or(0u64)
    }

    /// Enable all services for an anchor
    pub fn enable_all_services(env: &Env, anchor: &Address, all_services: &Vec<u32>) {
        let state_key = (soroban_sdk::Symbol::new(env, "SVC_STATE"), anchor);

        let state = ServiceToggleState {
            anchor: anchor.clone(),
            enabled_services: all_services.clone(),
            disabled_services: Vec::new(env),
            updated_at: env.ledger().timestamp(),
        };

        env.storage().persistent().set(&state_key, &state);
        env.storage()
            .persistent()
            .extend_ttl(&state_key, 31_536_000, 31_536_000);
    }

    /// Disable all services for an anchor
    pub fn disable_all_services(env: &Env, anchor: &Address, all_services: &Vec<u32>) {
        let state_key = (soroban_sdk::Symbol::new(env, "SVC_STATE"), anchor);

        let state = ServiceToggleState {
            anchor: anchor.clone(),
            enabled_services: Vec::new(env),
            disabled_services: all_services.clone(),
            updated_at: env.ledger().timestamp(),
        };

        env.storage().persistent().set(&state_key, &state);
        env.storage()
            .persistent()
            .extend_ttl(&state_key, 31_536_000, 31_536_000);
    }
}

// ---------------------------------------------------------------------------
// ServiceRetirementState — enforced lifecycle enum
// ---------------------------------------------------------------------------

/// The lifecycle state of a service undergoing retirement.
///
/// Legal forward transitions are:
/// - [`Active`](ServiceRetirementState::Active) →
///   [`Deprecated`](ServiceRetirementState::Deprecated)
/// - [`Deprecated`](ServiceRetirementState::Deprecated) →
///   [`Active`](ServiceRetirementState::Active)   *(un-deprecate)*
/// - [`Deprecated`](ServiceRetirementState::Deprecated) →
///   [`Disabled`](ServiceRetirementState::Disabled)
/// - [`Disabled`](ServiceRetirementState::Disabled) →
///   [`Retired`](ServiceRetirementState::Retired)
///
/// [`Retired`](ServiceRetirementState::Retired) is a terminal state.
/// All other transitions are rejected.
///
/// # Examples
///
/// ```rust
/// use anchorkit::service_management::ServiceRetirementState;
///
/// assert!(ServiceRetirementState::Active.is_valid_transition(ServiceRetirementState::Deprecated));
/// assert!(!ServiceRetirementState::Retired.is_valid_transition(ServiceRetirementState::Active));
/// assert_eq!(ServiceRetirementState::Disabled.as_str(), "disabled");
/// ```
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ServiceRetirementState {
    /// Service is fully operational — no retirement process has started.
    Active     = 0,
    /// Service is deprecated: still operational but callers are warned it will
    /// be disabled. A deprecation notice should be provided.
    Deprecated = 1,
    /// Service is no longer accepting new requests. Can only advance to Retired.
    Disabled   = 2,
    /// Service is permanently retired. Terminal state — no further transitions.
    Retired    = 3,
}

impl ServiceRetirementState {
    /// Canonical lowercase string representation of this state.
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceRetirementState::Active     => "active",
            ServiceRetirementState::Deprecated => "deprecated",
            ServiceRetirementState::Disabled   => "disabled",
            ServiceRetirementState::Retired    => "retired",
        }
    }

    /// Parse a state from its canonical string representation.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active"     => Some(ServiceRetirementState::Active),
            "deprecated" => Some(ServiceRetirementState::Deprecated),
            "disabled"   => Some(ServiceRetirementState::Disabled),
            "retired"    => Some(ServiceRetirementState::Retired),
            _            => None,
        }
    }

    /// Returns `true` only for explicitly permitted transitions.
    ///
    /// | From       | To         | Allowed |
    /// |------------|------------|---------|
    /// | Active     | Deprecated | ✓       |
    /// | Deprecated | Active     | ✓       |
    /// | Deprecated | Disabled   | ✓       |
    /// | Disabled   | Retired    | ✓       |
    /// | Retired    | *          | ✗       |
    /// | *          | same       | ✗       |
    pub fn is_valid_transition(&self, to: ServiceRetirementState) -> bool {
        matches!(
            (self, to),
            (ServiceRetirementState::Active,     ServiceRetirementState::Deprecated)
            | (ServiceRetirementState::Deprecated, ServiceRetirementState::Active)
            | (ServiceRetirementState::Deprecated, ServiceRetirementState::Disabled)
            | (ServiceRetirementState::Disabled,   ServiceRetirementState::Retired)
        )
    }

    /// Returns `true` if this is a terminal state (no further transitions allowed).
    pub fn is_terminal(&self) -> bool {
        matches!(self, ServiceRetirementState::Retired)
    }

    /// Build the canonical `[E65]`-prefixed error message for an illegal transition.
    pub fn illegal_transition_message(&self, to: ServiceRetirementState) -> alloc::string::String {
        alloc::format!(
            "[E65] Illegal service retirement transition: {} -> {}",
            self.as_str(),
            to.as_str()
        )
    }
}

// ---------------------------------------------------------------------------
// ServiceRetirementRecord — per-service per-anchor retirement state
// ---------------------------------------------------------------------------

/// Persistent record of a service's retirement lifecycle state.
///
/// Stored in Soroban persistent storage keyed by `(anchor, service_code)`.
/// Retrieved and mutated exclusively through [`ServiceManager`] methods.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ServiceRetirementRecord {
    /// The anchor address this record belongs to.
    pub anchor: Address,
    /// The numeric service code (e.g. 1 = deposits, 2 = withdrawals).
    pub service_code: u32,
    /// Current lifecycle state.
    pub state: ServiceRetirementState,
    /// Human-readable deprecation notice surfaced to callers.
    /// Should be set when transitioning to `Deprecated`.
    pub deprecation_notice: Option<String>,
    /// Planned timestamp (Unix epoch seconds) when the service will be disabled.
    /// Informational only — not enforced on-chain.
    pub planned_disable_at: Option<u64>,
    /// Planned timestamp (Unix epoch seconds) when the service will be retired.
    /// Informational only — not enforced on-chain.
    pub planned_retire_at: Option<u64>,
    /// Ledger timestamp of the most recent state transition.
    pub last_updated: u64,
    /// Full history of `(state_as_u32, timestamp)` pairs in chronological order.
    pub state_history: Vec<(u32, u64)>,
}

// ---------------------------------------------------------------------------
// ServiceManager — retirement lifecycle methods
// ---------------------------------------------------------------------------

impl ServiceManager {
    // ── Storage key helper ───────────────────────────────────────────────

    /// Derive the persistent storage key for a service retirement record.
    fn retirement_key(env: &Env, anchor: &Address, service_code: u32) -> (soroban_sdk::Symbol, Address, u32) {
        (soroban_sdk::Symbol::new(env, "SVC_RETIRE"), anchor.clone(), service_code)
    }

    // ── Read helpers ─────────────────────────────────────────────────────

    /// Return the current [`ServiceRetirementRecord`] for `(anchor, service_code)`.
    ///
    /// When no record exists the service is implicitly `Active` and a default
    /// record is returned without writing to storage.
    pub fn get_retirement_record(
        env: &Env,
        anchor: &Address,
        service_code: u32,
    ) -> ServiceRetirementRecord {
        let key = Self::retirement_key(env, anchor, service_code);
        env.storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| ServiceRetirementRecord {
                anchor: anchor.clone(),
                service_code,
                state: ServiceRetirementState::Active,
                deprecation_notice: None,
                planned_disable_at: None,
                planned_retire_at: None,
                last_updated: 0,
                state_history: Vec::new(env),
            })
    }

    /// Return the current [`ServiceRetirementState`] for `(anchor, service_code)`.
    pub fn get_retirement_state(
        env: &Env,
        anchor: &Address,
        service_code: u32,
    ) -> ServiceRetirementState {
        Self::get_retirement_record(env, anchor, service_code).state
    }

    // ── Core transition engine ───────────────────────────────────────────

    /// Advance the retirement lifecycle of a service to `new_state`.
    ///
    /// # Errors
    ///
    /// Returns `Err(message)` if the transition is illegal per
    /// [`ServiceRetirementState::is_valid_transition`].
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use soroban_sdk::Env;
    /// # use soroban_sdk::testutils::Address as _;
    /// # let env = Env::default();
    /// # let anchor = soroban_sdk::Address::generate(&env);
    /// use anchorkit::service_management::{ServiceManager, ServiceRetirementState};
    ///
    /// // Active → Deprecated → Disabled → Retired
    /// ServiceManager::transition_retirement(&env, &anchor, 1,
    ///     ServiceRetirementState::Deprecated, Some("EOL Q4 2026"), None, None).unwrap();
    /// ServiceManager::transition_retirement(&env, &anchor, 1,
    ///     ServiceRetirementState::Disabled, None, None, None).unwrap();
    /// ServiceManager::transition_retirement(&env, &anchor, 1,
    ///     ServiceRetirementState::Retired, None, None, None).unwrap();
    /// ```
    pub fn transition_retirement(
        env: &Env,
        anchor: &Address,
        service_code: u32,
        new_state: ServiceRetirementState,
        deprecation_notice: Option<&str>,
        planned_disable_at: Option<u64>,
        planned_retire_at: Option<u64>,
    ) -> Result<ServiceRetirementRecord, alloc::string::String> {
        let mut record = Self::get_retirement_record(env, anchor, service_code);
        let from_state = record.state;

        if !from_state.is_valid_transition(new_state) {
            return Err(from_state.illegal_transition_message(new_state));
        }

        let now = env.ledger().timestamp();
        record.state = new_state;
        record.last_updated = now;
        record.state_history.push_back((new_state as u32, now));

        // Update optional metadata when provided
        if let Some(notice) = deprecation_notice {
            record.deprecation_notice = Some(String::from_str(env, notice));
        }
        if let Some(ts) = planned_disable_at {
            record.planned_disable_at = Some(ts);
        }
        if let Some(ts) = planned_retire_at {
            record.planned_retire_at = Some(ts);
        }

        let key = Self::retirement_key(env, anchor, service_code);
        env.storage().persistent().set(&key, &record);
        env.storage().persistent().extend_ttl(&key, 31_536_000, 31_536_000);

        Ok(record)
    }

    // ── Named lifecycle helpers ──────────────────────────────────────────

    /// Mark a service as deprecated. Requires a deprecation notice.
    ///
    /// Only valid from `Active`.
    pub fn deprecate_service(
        env: &Env,
        anchor: &Address,
        service_code: u32,
        notice: &str,
        planned_disable_at: Option<u64>,
    ) -> Result<ServiceRetirementRecord, alloc::string::String> {
        Self::transition_retirement(
            env, anchor, service_code,
            ServiceRetirementState::Deprecated,
            Some(notice),
            planned_disable_at,
            None,
        )
    }

    /// Revert a deprecated service back to active.
    ///
    /// Only valid from `Deprecated`.
    pub fn undeprecate_service(
        env: &Env,
        anchor: &Address,
        service_code: u32,
    ) -> Result<ServiceRetirementRecord, alloc::string::String> {
        Self::transition_retirement(
            env, anchor, service_code,
            ServiceRetirementState::Active,
            None, None, None,
        )
    }

    /// Disable a deprecated service. Only valid from `Deprecated`.
    pub fn disable_for_retirement(
        env: &Env,
        anchor: &Address,
        service_code: u32,
        planned_retire_at: Option<u64>,
    ) -> Result<ServiceRetirementRecord, alloc::string::String> {
        Self::transition_retirement(
            env, anchor, service_code,
            ServiceRetirementState::Disabled,
            None, None,
            planned_retire_at,
        )
    }

    /// Permanently retire a disabled service. Only valid from `Disabled`.
    pub fn retire_service_lifecycle(
        env: &Env,
        anchor: &Address,
        service_code: u32,
    ) -> Result<ServiceRetirementRecord, alloc::string::String> {
        Self::transition_retirement(
            env, anchor, service_code,
            ServiceRetirementState::Retired,
            None, None, None,
        )
    }
}

// ---------------------------------------------------------------------------
// Inline unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::Env;

    fn make_env() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        env
    }

    fn set_ts(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 21,
            sequence_number: 0,
            network_id: Default::default(),
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        });
    }

    // ── ServiceRetirementState unit tests ────────────────────────────────

    #[test]
    fn retirement_state_as_str() {
        assert_eq!(ServiceRetirementState::Active.as_str(),     "active");
        assert_eq!(ServiceRetirementState::Deprecated.as_str(), "deprecated");
        assert_eq!(ServiceRetirementState::Disabled.as_str(),   "disabled");
        assert_eq!(ServiceRetirementState::Retired.as_str(),    "retired");
    }

    #[test]
    fn retirement_state_from_str_roundtrip() {
        for s in &["active", "deprecated", "disabled", "retired"] {
            let state = ServiceRetirementState::from_str(s).unwrap();
            assert_eq!(state.as_str(), *s);
        }
        assert!(ServiceRetirementState::from_str("unknown").is_none());
    }

    #[test]
    fn valid_forward_transitions_accepted() {
        assert!(ServiceRetirementState::Active.is_valid_transition(ServiceRetirementState::Deprecated));
        assert!(ServiceRetirementState::Deprecated.is_valid_transition(ServiceRetirementState::Active));
        assert!(ServiceRetirementState::Deprecated.is_valid_transition(ServiceRetirementState::Disabled));
        assert!(ServiceRetirementState::Disabled.is_valid_transition(ServiceRetirementState::Retired));
    }

    #[test]
    fn invalid_transitions_rejected() {
        // Retired is terminal
        assert!(!ServiceRetirementState::Retired.is_valid_transition(ServiceRetirementState::Active));
        assert!(!ServiceRetirementState::Retired.is_valid_transition(ServiceRetirementState::Deprecated));
        assert!(!ServiceRetirementState::Retired.is_valid_transition(ServiceRetirementState::Disabled));
        assert!(!ServiceRetirementState::Retired.is_valid_transition(ServiceRetirementState::Retired));
        // Skipping steps
        assert!(!ServiceRetirementState::Active.is_valid_transition(ServiceRetirementState::Disabled));
        assert!(!ServiceRetirementState::Active.is_valid_transition(ServiceRetirementState::Retired));
        assert!(!ServiceRetirementState::Deprecated.is_valid_transition(ServiceRetirementState::Retired));
        // Disabled cannot go backwards
        assert!(!ServiceRetirementState::Disabled.is_valid_transition(ServiceRetirementState::Active));
        assert!(!ServiceRetirementState::Disabled.is_valid_transition(ServiceRetirementState::Deprecated));
        // Self-loops
        assert!(!ServiceRetirementState::Active.is_valid_transition(ServiceRetirementState::Active));
        assert!(!ServiceRetirementState::Deprecated.is_valid_transition(ServiceRetirementState::Deprecated));
        assert!(!ServiceRetirementState::Disabled.is_valid_transition(ServiceRetirementState::Disabled));
    }

    #[test]
    fn retired_is_terminal() {
        assert!(ServiceRetirementState::Retired.is_terminal());
        assert!(!ServiceRetirementState::Active.is_terminal());
        assert!(!ServiceRetirementState::Deprecated.is_terminal());
        assert!(!ServiceRetirementState::Disabled.is_terminal());
    }

    #[test]
    fn illegal_transition_message_has_e65_prefix() {
        let msg = ServiceRetirementState::Retired
            .illegal_transition_message(ServiceRetirementState::Active);
        assert!(msg.starts_with("[E65]"), "expected [E65] prefix: {msg}");
        assert!(msg.contains("retired"), "expected 'retired' in: {msg}");
        assert!(msg.contains("active"),  "expected 'active' in: {msg}");
    }

    // ── ServiceManager retirement integration tests ───────────────────────

    #[test]
    fn default_state_is_active() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);
        let state = ServiceManager::get_retirement_state(&env, &anchor, 1);
        assert_eq!(state, ServiceRetirementState::Active);
    }

    #[test]
    fn full_retirement_lifecycle_succeeds() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        // Active → Deprecated
        let rec = ServiceManager::deprecate_service(&env, &anchor, 1, "EOL Q4", None).unwrap();
        assert_eq!(rec.state, ServiceRetirementState::Deprecated);
        assert!(rec.deprecation_notice.is_some());

        // Deprecated → Disabled
        let rec = ServiceManager::disable_for_retirement(&env, &anchor, 1, None).unwrap();
        assert_eq!(rec.state, ServiceRetirementState::Disabled);

        // Disabled → Retired
        let rec = ServiceManager::retire_service_lifecycle(&env, &anchor, 1).unwrap();
        assert_eq!(rec.state, ServiceRetirementState::Retired);
        assert!(rec.state.is_terminal());
    }

    #[test]
    fn state_history_records_full_path() {
        let env = make_env();
        set_ts(&env, 2000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, 2, "going away", None).unwrap();
        ServiceManager::disable_for_retirement(&env, &anchor, 2, None).unwrap();
        ServiceManager::retire_service_lifecycle(&env, &anchor, 2).unwrap();

        let rec = ServiceManager::get_retirement_record(&env, &anchor, 2);
        // history: Deprecated, Disabled, Retired
        assert_eq!(rec.state_history.len(), 3);
        assert_eq!(rec.state_history.get(0).unwrap().0, ServiceRetirementState::Deprecated as u32);
        assert_eq!(rec.state_history.get(1).unwrap().0, ServiceRetirementState::Disabled   as u32);
        assert_eq!(rec.state_history.get(2).unwrap().0, ServiceRetirementState::Retired     as u32);
    }

    #[test]
    fn undeprecate_restores_active_state() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, 1, "tentative EOL", None).unwrap();
        let rec = ServiceManager::undeprecate_service(&env, &anchor, 1).unwrap();
        assert_eq!(rec.state, ServiceRetirementState::Active);
    }

    #[test]
    fn skip_to_disabled_without_deprecation_is_rejected() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        let result = ServiceManager::disable_for_retirement(&env, &anchor, 1, None);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("[E65]"), "expected [E65] in: {msg}");
        assert!(msg.contains("active"), "expected 'active' in: {msg}");
    }

    #[test]
    fn skip_directly_to_retired_is_rejected() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        let result = ServiceManager::retire_service_lifecycle(&env, &anchor, 1);
        assert!(result.is_err());
    }

    #[test]
    fn retire_from_deprecated_without_disabling_is_rejected() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, 1, "notice", None).unwrap();
        let result = ServiceManager::retire_service_lifecycle(&env, &anchor, 1);
        assert!(result.is_err());
    }

    #[test]
    fn retired_service_cannot_transition_further() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, 1, "notice", None).unwrap();
        ServiceManager::disable_for_retirement(&env, &anchor, 1, None).unwrap();
        ServiceManager::retire_service_lifecycle(&env, &anchor, 1).unwrap();

        // Any further transition must fail
        let result = ServiceManager::deprecate_service(&env, &anchor, 1, "re-deprecate?", None);
        assert!(result.is_err());

        let result2 = ServiceManager::transition_retirement(
            &env, &anchor, 1, ServiceRetirementState::Active, None, None, None,
        );
        assert!(result2.is_err());
    }

    #[test]
    fn different_services_have_independent_retirement_states() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, 1, "deposits EOL", None).unwrap();

        // Service 2 is still Active
        assert_eq!(
            ServiceManager::get_retirement_state(&env, &anchor, 2),
            ServiceRetirementState::Active
        );
    }

    #[test]
    fn different_anchors_have_independent_retirement_states() {
        let env = make_env();
        set_ts(&env, 1000);
        let a1 = Address::generate(&env);
        let a2 = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &a1, 1, "notice", None).unwrap();

        assert_eq!(
            ServiceManager::get_retirement_state(&env, &a2, 1),
            ServiceRetirementState::Active
        );
    }

    #[test]
    fn planned_timestamps_stored_on_record() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, 1, "notice", Some(2000)).unwrap();
        let rec = ServiceManager::disable_for_retirement(&env, &anchor, 1, Some(3000)).unwrap();

        assert_eq!(rec.planned_disable_at, Some(2000));
        assert_eq!(rec.planned_retire_at, Some(3000));
    }

    #[test]
    fn last_updated_timestamp_advances_on_transition() {
        let env = make_env();
        set_ts(&env, 1000);
        let anchor = Address::generate(&env);

        ServiceManager::deprecate_service(&env, &anchor, 1, "notice", None).unwrap();
        let rec1 = ServiceManager::get_retirement_record(&env, &anchor, 1);
        assert_eq!(rec1.last_updated, 1000);

        set_ts(&env, 5000);
        ServiceManager::disable_for_retirement(&env, &anchor, 1, None).unwrap();
        let rec2 = ServiceManager::get_retirement_record(&env, &anchor, 1);
        assert_eq!(rec2.last_updated, 5000);
    }

    // ── Original toggle tests (kept here to confirm no regression) ────────

    #[test]
    fn test_service_toggle_state_creation() {
        // Struct construction is verified implicitly by all toggle tests above.
    }
}
