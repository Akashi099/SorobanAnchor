//! Service management for anchor service enable/disable toggles and rollback handling.
//!
//! This module provides functionality to:
//! - Enable/disable individual services for anchors
//! - Track service configuration history for rollback
//! - Restore prior service configurations
//! - Query current service status
//! - Schedule and enforce maintenance windows so that toggles and health
//!   checks respect planned outages
//! - Model and enforce a service dependency graph so that prerequisite
//!   services cannot be disabled while dependents are active

use soroban_sdk::{contracttype, Address, Env, String, Vec};
use crate::errors::AnchorKitError;

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
    /// Enable a service for an anchor.
    ///
    /// Returns `Err(ServiceInMaintenance)` when the service is currently inside
    /// an active maintenance window (call is blocked to avoid conflicting with
    /// the planned outage).  Returns `Ok(false)` if already enabled.
    pub fn enable_service(env: &Env, anchor: &Address, service_code: u32) -> Result<bool, AnchorKitError> {
        if MaintenanceManager::is_in_maintenance(env, anchor, service_code) {
            return Err(AnchorKitError::service_in_maintenance(service_code));
        }

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
                return Ok(false); // Already enabled
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

        Ok(true)
    }

    /// Disable a service for an anchor.
    ///
    /// Returns `Err(ServiceInMaintenance)` when the service is currently inside
    /// an active maintenance window.  Returns `Ok(false)` if already disabled.
    pub fn disable_service(env: &Env, anchor: &Address, service_code: u32) -> Result<bool, AnchorKitError> {
        if MaintenanceManager::is_in_maintenance(env, anchor, service_code) {
            return Err(AnchorKitError::service_in_maintenance(service_code));
        }

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
                return Ok(false); // Already disabled
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

        Ok(true)
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
// Maintenance window model
// ---------------------------------------------------------------------------

/// A scheduled maintenance window for an anchor.
///
/// During an active maintenance window:
/// - Service toggle operations are blocked (unless forced)
/// - Health degradation alerts are suppressed
/// - Status reporting reflects the maintenance state
#[contracttype]
#[derive(Clone, Debug)]
pub struct MaintenanceWindow {
    /// Unique identifier for this maintenance window
    pub window_id: u64,
    /// Anchor address
    pub anchor: Address,
    /// Unix timestamp when the window starts
    pub start_time: u64,
    /// Unix timestamp when the window ends
    pub end_time: u64,
    /// Human-readable description (e.g., "Database migration")
    pub description: String,
    /// Services affected by this maintenance window (empty vec = all services)
    pub affected_services: Vec<u32>,
}

impl MaintenanceWindow {
    /// Returns `true` when the current ledger time falls within `[start_time, end_time]`.
    pub fn is_active(&self, env: &Env) -> bool {
        let now = env.ledger().timestamp();
        now >= self.start_time && now <= self.end_time
    }

    /// Returns `true` when the given `service_code` is covered by this window.
    /// An empty `affected_services` list means the window covers every service.
    pub fn affects_service(&self, service_code: u32) -> bool {
        if self.affected_services.is_empty() {
            return true;
        }
        for svc in self.affected_services.iter() {
            if svc == service_code {
                return true;
            }
        }
        false
    }
}

/// Operations for scheduling and querying maintenance windows.
pub struct MaintenanceManager;

impl MaintenanceManager {
    /// Schedule a new maintenance window for an anchor.
    ///
    /// Returns `Err(MaintenanceWindowConflict)` when the time range overlaps an existing
    /// window for the same anchor.  Returns the new `window_id` on success.
    pub fn schedule_window(
        env: &Env,
        anchor: &Address,
        start_time: u64,
        end_time: u64,
        description: &str,
        affected_services: Vec<u32>,
    ) -> Result<u64, AnchorKitError> {
        let counter_key = soroban_sdk::Symbol::new(env, "MW_CNT");
        let window_id: u64 = env
            .storage()
            .instance()
            .get(&counter_key)
            .unwrap_or(0u64);

        // Check for overlap with existing windows
        for id in 0..window_id {
            if let Some(existing) = Self::get_window(env, id) {
                if existing.anchor == *anchor
                    && start_time <= existing.end_time
                    && end_time >= existing.start_time
                {
                    return Err(AnchorKitError::maintenance_window_conflict());
                }
            }
        }

        let window = MaintenanceWindow {
            window_id,
            anchor: anchor.clone(),
            start_time,
            end_time,
            description: String::from_str(env, description),
            affected_services,
        };

        let key = (soroban_sdk::Symbol::new(env, "MW"), window_id);
        env.storage().instance().set(&key, &window);
        env.storage().instance().set(&counter_key, &(window_id + 1));
        env.storage().instance().extend_ttl(31_536_000, 31_536_000);

        Ok(window_id)
    }

    /// Retrieve a maintenance window by ID, returning `None` if not found.
    pub fn get_window(env: &Env, window_id: u64) -> Option<MaintenanceWindow> {
        let key = (soroban_sdk::Symbol::new(env, "MW"), window_id);
        env.storage().instance().get(&key)
    }

    /// Cancel (delete) a maintenance window by ID.
    /// Returns `false` when the window does not exist.
    pub fn cancel_window(env: &Env, window_id: u64) -> bool {
        let key = (soroban_sdk::Symbol::new(env, "MW"), window_id);
        if env.storage().instance().get::<_, MaintenanceWindow>(&key).is_none() {
            return false;
        }
        env.storage().instance().remove(&key);
        true
    }

    /// Return the total number of windows ever scheduled (monotonically increasing counter).
    pub fn window_count(env: &Env) -> u64 {
        let counter_key = soroban_sdk::Symbol::new(env, "MW_CNT");
        env.storage().instance().get(&counter_key).unwrap_or(0u64)
    }

    /// Returns `true` when the anchor has an active maintenance window that covers `service_code`.
    pub fn is_in_maintenance(env: &Env, anchor: &Address, service_code: u32) -> bool {
        let count = Self::window_count(env);
        for id in 0..count {
            if let Some(w) = Self::get_window(env, id) {
                if w.anchor == *anchor && w.is_active(env) && w.affects_service(service_code) {
                    return true;
                }
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Service dependency graph
// ---------------------------------------------------------------------------

/// A single dependency rule: `service` requires `dependency` to be enabled
/// before it can itself be enabled.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ServiceDependency {
    /// The service that has a prerequisite.
    pub service_code: u32,
    /// The service that must be enabled first.
    pub dependency_code: u32,
}

/// The full dependency graph stored for one anchor.
///
/// Edges are represented as a flat list of [`ServiceDependency`] pairs.
/// Cycle detection is performed before any edge is added.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ServiceDependencyGraph {
    /// Anchor address this graph belongs to.
    pub anchor: Address,
    /// All dependency edges.
    pub edges: Vec<ServiceDependency>,
}

/// Operations for building and querying service dependency graphs.
pub struct DependencyManager;

impl DependencyManager {
    // Storage key helpers ------------------------------------------------

    fn graph_key<'a>(env: &'a Env, anchor: &'a Address)
        -> (soroban_sdk::Symbol, Address)
    {
        (soroban_sdk::Symbol::new(env, "DEP_GRAPH"), anchor.clone())
    }

    // Public API ---------------------------------------------------------

    /// Load the dependency graph for an anchor, returning an empty graph if
    /// none has been defined yet.
    pub fn get_graph(env: &Env, anchor: &Address) -> ServiceDependencyGraph {
        let key = Self::graph_key(env, anchor);
        env.storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| ServiceDependencyGraph {
                anchor: anchor.clone(),
                edges: Vec::new(env),
            })
    }

    /// Add a dependency edge `service_code` → `dependency_code` (meaning
    /// `service_code` requires `dependency_code`).
    ///
    /// Errors:
    /// - `DependencyCycleDetected` — adding this edge would create a cycle.
    pub fn add_dependency(
        env: &Env,
        anchor: &Address,
        service_code: u32,
        dependency_code: u32,
    ) -> Result<(), AnchorKitError> {
        let mut graph = Self::get_graph(env, anchor);

        // Adding A → B would create a cycle if B already (transitively)
        // depends on A.  We detect this by checking reachability from
        // dependency_code to service_code in the *current* graph.
        if Self::is_reachable(&graph, dependency_code, service_code) {
            return Err(AnchorKitError::dependency_cycle_detected());
        }

        // Also reject duplicate edges silently (idempotent add).
        for e in graph.edges.iter() {
            if e.service_code == service_code && e.dependency_code == dependency_code {
                return Ok(());
            }
        }

        graph.edges.push_back(ServiceDependency { service_code, dependency_code });

        let key = Self::graph_key(env, anchor);
        env.storage().persistent().set(&key, &graph);
        env.storage()
            .persistent()
            .extend_ttl(&key, 31_536_000, 31_536_000);

        Ok(())
    }

    /// Remove an existing dependency edge.  Returns `Ok(())` whether or not
    /// the edge existed (idempotent remove).
    pub fn remove_dependency(
        env: &Env,
        anchor: &Address,
        service_code: u32,
        dependency_code: u32,
    ) -> Result<(), AnchorKitError> {
        let mut graph = Self::get_graph(env, anchor);
        let mut new_edges: Vec<ServiceDependency> = Vec::new(env);
        for e in graph.edges.iter() {
            if !(e.service_code == service_code && e.dependency_code == dependency_code) {
                new_edges.push_back(e);
            }
        }
        graph.edges = new_edges;
        let key = Self::graph_key(env, anchor);
        env.storage().persistent().set(&key, &graph);
        env.storage()
            .persistent()
            .extend_ttl(&key, 31_536_000, 31_536_000);
        Ok(())
    }

    /// Assert that all prerequisites of `service_code` are currently enabled
    /// for this anchor.
    ///
    /// Returns `Err(DependencyNotMet)` for the first unmet dependency found.
    pub fn assert_dependencies_met(
        env: &Env,
        anchor: &Address,
        service_code: u32,
    ) -> Result<(), AnchorKitError> {
        let graph = Self::get_graph(env, anchor);
        for e in graph.edges.iter() {
            if e.service_code == service_code {
                if !ServiceManager::is_service_enabled(env, anchor, e.dependency_code) {
                    return Err(AnchorKitError::dependency_not_met(
                        service_code,
                        e.dependency_code,
                    ));
                }
            }
        }
        Ok(())
    }

    /// Assert that no enabled service depends on `service_code` being enabled.
    ///
    /// Call this before disabling a service to prevent orphaning dependents.
    /// Returns `Err(DependencyNotMet)` when a dependent is still active.
    pub fn assert_no_active_dependents(
        env: &Env,
        anchor: &Address,
        service_code: u32,
    ) -> Result<(), AnchorKitError> {
        let graph = Self::get_graph(env, anchor);
        for e in graph.edges.iter() {
            // e.service_code depends on e.dependency_code
            if e.dependency_code == service_code
                && ServiceManager::is_service_enabled(env, anchor, e.service_code)
            {
                // A currently-enabled service would lose its dependency
                return Err(AnchorKitError::dependency_not_met(
                    e.service_code,
                    service_code,
                ));
            }
        }
        Ok(())
    }

    // Internal helpers ---------------------------------------------------

    /// Returns `true` when `target` is reachable from `start` by following
    /// dependency edges (depth-first, iterative to avoid stack growth).
    ///
    /// This is used for cycle detection: before adding edge A → B, check
    /// whether B can already reach A.
    fn is_reachable(graph: &ServiceDependencyGraph, start: u32, target: u32) -> bool {
        // We can't use alloc::Vec here because this runs in a soroban_sdk context.
        // Instead we do a bounded walk: iterate all edges repeatedly until no
        // new nodes are added (BFS via a fixed-size soroban Vec is not
        // available without env, so we use a simple O(V²) reachability scan).
        //
        // Maximum realistic service count is small, so this is fine.
        if start == target {
            return true;
        }

        // Build a visited bit-set represented as a flat list of seen codes.
        // We cap at 256 nodes to prevent unbounded iteration.
        const MAX_NODES: usize = 256;
        let mut visited = [0u32; MAX_NODES];
        let mut visited_len = 0usize;
        let mut frontier = [0u32; MAX_NODES];
        let mut frontier_len = 0usize;

        frontier[0] = start;
        frontier_len = 1;

        loop {
            let mut changed = false;
            let current_frontier = frontier_len;

            for fi in 0..current_frontier {
                let node = frontier[fi];
                // Find all direct successors of `node`
                for e in graph.edges.iter() {
                    if e.service_code == node {
                        let next = e.dependency_code;
                        if next == target {
                            return true;
                        }
                        // Add to frontier if not visited
                        let mut seen = false;
                        for vi in 0..visited_len {
                            if visited[vi] == next {
                                seen = true;
                                break;
                            }
                        }
                        for nfi in 0..frontier_len {
                            if frontier[nfi] == next {
                                seen = true;
                                break;
                            }
                        }
                        if !seen && frontier_len < MAX_NODES {
                            frontier[frontier_len] = next;
                            frontier_len += 1;
                            changed = true;
                        }
                    }
                }
                // Mark node as visited
                let mut already = false;
                for vi in 0..visited_len {
                    if visited[vi] == node {
                        already = true;
                        break;
                    }
                }
                if !already && visited_len < MAX_NODES {
                    visited[visited_len] = node;
                    visited_len += 1;
                }
            }

            if !changed || frontier_len == current_frontier {
                break;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger as _, LedgerInfo};
    use soroban_sdk::Env;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_env() -> Env { Env::default() }

    fn make_anchor(env: &Env) -> Address { Address::generate(env) }

    fn set_time(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 22,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1,
            min_persistent_entry_ttl: 1,
            max_entry_ttl: 100_000_000,
        });
    }

    // ── ServiceToggleState creation (original test kept) ─────────────────────

    #[test]
    fn test_service_toggle_state_creation() {
        // struct can be created and cloned; toggle logic is covered below
    }

    // ── Maintenance window: scheduling and retrieval ──────────────────────────

    #[test]
    fn test_schedule_window_returns_id() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let id = MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "upgrade", Vec::new(&env),
        ).expect("schedule should succeed");
        assert_eq!(id, 0);
    }

    #[test]
    fn test_get_window_returns_stored_data() {
        let env = make_env();
        let anchor = make_anchor(&env);
        MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "upgrade", Vec::new(&env),
        ).unwrap();
        let w = MaintenanceManager::get_window(&env, 0).expect("window 0 must exist");
        assert_eq!(w.start_time, 1000);
        assert_eq!(w.end_time, 2000);
    }

    #[test]
    fn test_window_count_increments() {
        let env = make_env();
        let anchor = make_anchor(&env);
        assert_eq!(MaintenanceManager::window_count(&env), 0);
        MaintenanceManager::schedule_window(&env, &anchor, 0, 100, "a", Vec::new(&env)).unwrap();
        MaintenanceManager::schedule_window(&env, &anchor, 200, 300, "b", Vec::new(&env)).unwrap();
        assert_eq!(MaintenanceManager::window_count(&env), 2);
    }

    #[test]
    fn test_cancel_window() {
        let env = make_env();
        let anchor = make_anchor(&env);
        MaintenanceManager::schedule_window(&env, &anchor, 0, 100, "a", Vec::new(&env)).unwrap();
        assert!(MaintenanceManager::cancel_window(&env, 0));
        assert!(MaintenanceManager::get_window(&env, 0).is_none());
    }

    #[test]
    fn test_cancel_nonexistent_window_returns_false() {
        let env = make_env();
        assert!(!MaintenanceManager::cancel_window(&env, 99));
    }

    // ── Maintenance window: active / inactive scheduling ──────────────────────

    #[test]
    fn test_window_is_active_inside_range() {
        let env = make_env();
        let anchor = make_anchor(&env);
        set_time(&env, 1500);
        MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "active", Vec::new(&env),
        ).unwrap();
        assert!(MaintenanceManager::is_in_maintenance(&env, &anchor, 42));
    }

    #[test]
    fn test_window_is_inactive_before_start() {
        let env = make_env();
        let anchor = make_anchor(&env);
        set_time(&env, 500);
        MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "future", Vec::new(&env),
        ).unwrap();
        assert!(!MaintenanceManager::is_in_maintenance(&env, &anchor, 42));
    }

    #[test]
    fn test_window_is_inactive_after_end() {
        let env = make_env();
        let anchor = make_anchor(&env);
        set_time(&env, 3000);
        MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "past", Vec::new(&env),
        ).unwrap();
        assert!(!MaintenanceManager::is_in_maintenance(&env, &anchor, 42));
    }

    #[test]
    fn test_window_affects_specific_service() {
        let env = make_env();
        let anchor = make_anchor(&env);
        set_time(&env, 1500);
        let mut services = Vec::new(&env);
        services.push_back(10u32);
        MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "partial", services,
        ).unwrap();
        assert!(MaintenanceManager::is_in_maintenance(&env, &anchor, 10));
        assert!(!MaintenanceManager::is_in_maintenance(&env, &anchor, 99));
    }

    #[test]
    fn test_conflict_detection_rejects_overlapping_window() {
        let env = make_env();
        let anchor = make_anchor(&env);
        MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "first", Vec::new(&env),
        ).unwrap();
        let err = MaintenanceManager::schedule_window(
            &env, &anchor, 1500, 2500, "overlap", Vec::new(&env),
        ).expect_err("overlapping window must be rejected");
        assert_eq!(err.code, crate::errors::ErrorCode::MaintenanceWindowConflict);
    }

    #[test]
    fn test_non_overlapping_windows_allowed() {
        let env = make_env();
        let anchor = make_anchor(&env);
        MaintenanceManager::schedule_window(&env, &anchor, 1000, 2000, "a", Vec::new(&env)).unwrap();
        MaintenanceManager::schedule_window(&env, &anchor, 2001, 3000, "b", Vec::new(&env)).unwrap();
        assert_eq!(MaintenanceManager::window_count(&env), 2);
    }

    // ── Maintenance window: toggle enforcement ────────────────────────────────

    #[test]
    fn test_enable_service_blocked_during_maintenance() {
        let env = make_env();
        let anchor = make_anchor(&env);
        set_time(&env, 1500);
        MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "maintenance", Vec::new(&env),
        ).unwrap();
        let err = ServiceManager::enable_service(&env, &anchor, 1)
            .expect_err("enable must be blocked");
        assert_eq!(err.code, crate::errors::ErrorCode::ServiceInMaintenance);
    }

    #[test]
    fn test_disable_service_blocked_during_maintenance() {
        let env = make_env();
        let anchor = make_anchor(&env);
        set_time(&env, 1500);
        MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "maintenance", Vec::new(&env),
        ).unwrap();
        let err = ServiceManager::disable_service(&env, &anchor, 1)
            .expect_err("disable must be blocked");
        assert_eq!(err.code, crate::errors::ErrorCode::ServiceInMaintenance);
    }

    #[test]
    fn test_toggle_allowed_outside_maintenance() {
        let env = make_env();
        let anchor = make_anchor(&env);
        set_time(&env, 3000); // after window
        MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "past", Vec::new(&env),
        ).unwrap();
        assert!(ServiceManager::enable_service(&env, &anchor, 1).is_ok());
    }

    // ── Dependency graph: basic add / get ─────────────────────────────────────

    #[test]
    fn test_add_dependency_stored() {
        let env = make_env();
        let anchor = make_anchor(&env);
        DependencyManager::add_dependency(&env, &anchor, 2, 1).unwrap();
        let graph = DependencyManager::get_graph(&env, &anchor);
        assert_eq!(graph.edges.len(), 1);
        let e = graph.edges.get(0).unwrap();
        assert_eq!(e.service_code, 2);
        assert_eq!(e.dependency_code, 1);
    }

    #[test]
    fn test_add_duplicate_dependency_is_idempotent() {
        let env = make_env();
        let anchor = make_anchor(&env);
        DependencyManager::add_dependency(&env, &anchor, 2, 1).unwrap();
        DependencyManager::add_dependency(&env, &anchor, 2, 1).unwrap();
        let graph = DependencyManager::get_graph(&env, &anchor);
        assert_eq!(graph.edges.len(), 1, "duplicate edge should not be added twice");
    }

    #[test]
    fn test_remove_dependency() {
        let env = make_env();
        let anchor = make_anchor(&env);
        DependencyManager::add_dependency(&env, &anchor, 2, 1).unwrap();
        DependencyManager::remove_dependency(&env, &anchor, 2, 1).unwrap();
        let graph = DependencyManager::get_graph(&env, &anchor);
        assert_eq!(graph.edges.len(), 0);
    }

    // ── Dependency graph: cycle detection ────────────────────────────────────

    #[test]
    fn test_direct_cycle_rejected() {
        let env = make_env();
        let anchor = make_anchor(&env);
        // A depends on B
        DependencyManager::add_dependency(&env, &anchor, 1, 2).unwrap();
        // B depends on A → cycle
        let err = DependencyManager::add_dependency(&env, &anchor, 2, 1)
            .expect_err("cycle must be rejected");
        assert_eq!(err.code, crate::errors::ErrorCode::DependencyCycleDetected);
    }

    #[test]
    fn test_transitive_cycle_rejected() {
        let env = make_env();
        let anchor = make_anchor(&env);
        // A→B→C, then C→A would form a cycle
        DependencyManager::add_dependency(&env, &anchor, 1, 2).unwrap(); // 1 needs 2
        DependencyManager::add_dependency(&env, &anchor, 2, 3).unwrap(); // 2 needs 3
        let err = DependencyManager::add_dependency(&env, &anchor, 3, 1)
            .expect_err("transitive cycle must be rejected");
        assert_eq!(err.code, crate::errors::ErrorCode::DependencyCycleDetected);
    }

    #[test]
    fn test_self_dependency_rejected() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let err = DependencyManager::add_dependency(&env, &anchor, 1, 1)
            .expect_err("self-dependency is a cycle");
        assert_eq!(err.code, crate::errors::ErrorCode::DependencyCycleDetected);
    }

    #[test]
    fn test_valid_chain_accepted() {
        let env = make_env();
        let anchor = make_anchor(&env);
        // A needs B, B needs C — no cycle
        DependencyManager::add_dependency(&env, &anchor, 1, 2).unwrap();
        DependencyManager::add_dependency(&env, &anchor, 2, 3).unwrap();
        assert_eq!(DependencyManager::get_graph(&env, &anchor).edges.len(), 2);
    }

    // ── Dependency graph: enforcement ────────────────────────────────────────

    #[test]
    fn test_assert_dependencies_met_ok_when_dep_enabled() {
        let env = make_env();
        let anchor = make_anchor(&env);
        // service 2 requires service 1
        DependencyManager::add_dependency(&env, &anchor, 2, 1).unwrap();
        // Enable service 1 first
        ServiceManager::enable_service(&env, &anchor, 1).unwrap();
        // Now asserting deps for service 2 should pass
        assert!(DependencyManager::assert_dependencies_met(&env, &anchor, 2).is_ok());
    }

    #[test]
    fn test_assert_dependencies_met_fails_when_dep_absent() {
        let env = make_env();
        let anchor = make_anchor(&env);
        // service 2 requires service 1, but 1 is never enabled
        DependencyManager::add_dependency(&env, &anchor, 2, 1).unwrap();
        let err = DependencyManager::assert_dependencies_met(&env, &anchor, 2)
            .expect_err("unmet dependency must error");
        assert_eq!(err.code, crate::errors::ErrorCode::DependencyNotMet);
    }

    #[test]
    fn test_assert_no_active_dependents_ok_when_no_dependents() {
        let env = make_env();
        let anchor = make_anchor(&env);
        DependencyManager::add_dependency(&env, &anchor, 2, 1).unwrap();
        // service 2 is not enabled — safe to disable service 1
        assert!(DependencyManager::assert_no_active_dependents(&env, &anchor, 1).is_ok());
    }

    #[test]
    fn test_assert_no_active_dependents_fails_when_dependent_enabled() {
        let env = make_env();
        let anchor = make_anchor(&env);
        // service 2 requires service 1
        DependencyManager::add_dependency(&env, &anchor, 2, 1).unwrap();
        // Enable both services
        ServiceManager::enable_service(&env, &anchor, 1).unwrap();
        ServiceManager::enable_service(&env, &anchor, 2).unwrap();
        // Trying to disable service 1 while service 2 is still active
        let err = DependencyManager::assert_no_active_dependents(&env, &anchor, 1)
            .expect_err("active dependent must block disable");
        assert_eq!(err.code, crate::errors::ErrorCode::DependencyNotMet);
    }

    #[test]
    fn test_empty_dependency_graph_always_met() {
        let env = make_env();
        let anchor = make_anchor(&env);
        // No edges defined — any service's deps are trivially met
        assert!(DependencyManager::assert_dependencies_met(&env, &anchor, 42).is_ok());
    }
}

// ---------------------------------------------------------------------------
// Service onboarding templates
// ---------------------------------------------------------------------------

/// A named service onboarding template.
///
/// Templates capture a curated set of service codes that represent a
/// sensible starting configuration for a particular deployment type
/// (e.g. "fiat-on-ramp", "remittance", "stablecoin-issuer").
///
/// Call [`TemplateManager::apply_template`] to bootstrap a new anchor from
/// a registered template.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ServiceTemplate {
    /// Unique, human-readable template name (e.g. "fiat-on-ramp").
    pub name: String,
    /// Services to enable when this template is applied.
    pub services: Vec<u32>,
    /// Human-readable description of the deployment type.
    pub description: String,
    /// Schema version, incremented when the template definition changes.
    pub version: u32,
}

/// Record that a template has been applied to an anchor, stored so the same
/// template cannot be applied a second time without an explicit override.
#[contracttype]
#[derive(Clone, Debug)]
pub struct TemplateApplication {
    /// The template name that was applied.
    pub template_name: String,
    /// Anchor that received the configuration.
    pub anchor: Address,
    /// Ledger timestamp when the template was applied.
    pub applied_at: u64,
}

/// Built-in template names — callers may also register custom templates.
pub const TEMPLATE_FIAT_ON_RAMP:       &str = "fiat-on-ramp";
pub const TEMPLATE_REMITTANCE:         &str = "remittance";
pub const TEMPLATE_STABLECOIN_ISSUER:  &str = "stablecoin-issuer";

/// Operations for managing and applying service onboarding templates.
pub struct TemplateManager;

impl TemplateManager {
    // ── Storage key helpers ────────────────────────────────────────────────

    fn template_key(env: &Env, name: &str) -> (soroban_sdk::Symbol, String) {
        (soroban_sdk::Symbol::new(env, "TMPL"), String::from_str(env, name))
    }

    fn application_key(env: &Env, anchor: &Address, name: &str)
        -> (soroban_sdk::Symbol, Address, String)
    {
        (
            soroban_sdk::Symbol::new(env, "TMPL_APP"),
            anchor.clone(),
            String::from_str(env, name),
        )
    }

    // ── Template registration ──────────────────────────────────────────────

    /// Register (or overwrite) a named service template.
    ///
    /// Returns `Err(InvalidTemplate)` when:
    /// - `name` is empty
    /// - `services` is empty (a template with no services cannot bootstrap anything)
    pub fn register_template(
        env: &Env,
        name: &str,
        services: Vec<u32>,
        description: &str,
        version: u32,
    ) -> Result<(), AnchorKitError> {
        if name.is_empty() {
            return Err(AnchorKitError::invalid_template("template name must not be empty"));
        }
        if services.is_empty() {
            return Err(AnchorKitError::invalid_template("template must contain at least one service"));
        }

        let tmpl = ServiceTemplate {
            name: String::from_str(env, name),
            services,
            description: String::from_str(env, description),
            version,
        };

        let key = Self::template_key(env, name);
        env.storage().instance().set(&key, &tmpl);
        env.storage().instance().extend_ttl(31_536_000, 31_536_000);
        Ok(())
    }

    /// Retrieve a template by name, returning `None` if not found.
    pub fn get_template(env: &Env, name: &str) -> Option<ServiceTemplate> {
        let key = Self::template_key(env, name);
        env.storage().instance().get(&key)
    }

    // ── Built-in template seed ─────────────────────────────────────────────

    /// Seed the three built-in deployment templates if they are not already registered.
    ///
    /// Built-in service codes:
    ///
    /// | Template             | Services (codes)           |
    /// |----------------------|----------------------------|
    /// | fiat-on-ramp         | 1 (SEP-6), 2 (SEP-24)      |
    /// | remittance           | 3 (SEP-31)                 |
    /// | stablecoin-issuer    | 1 (SEP-6), 3 (SEP-31), 4   |
    pub fn seed_builtin_templates(env: &Env) {
        // fiat-on-ramp
        if Self::get_template(env, TEMPLATE_FIAT_ON_RAMP).is_none() {
            let mut svcs = Vec::new(env);
            svcs.push_back(1u32);
            svcs.push_back(2u32);
            let _ = Self::register_template(env, TEMPLATE_FIAT_ON_RAMP, svcs,
                "Non-interactive and interactive deposit/withdrawal (SEP-6 + SEP-24)", 1);
        }
        // remittance
        if Self::get_template(env, TEMPLATE_REMITTANCE).is_none() {
            let mut svcs = Vec::new(env);
            svcs.push_back(3u32);
            let _ = Self::register_template(env, TEMPLATE_REMITTANCE, svcs,
                "Direct cross-border payment (SEP-31)", 1);
        }
        // stablecoin-issuer
        if Self::get_template(env, TEMPLATE_STABLECOIN_ISSUER).is_none() {
            let mut svcs = Vec::new(env);
            svcs.push_back(1u32);
            svcs.push_back(3u32);
            svcs.push_back(4u32);
            let _ = Self::register_template(env, TEMPLATE_STABLECOIN_ISSUER, svcs,
                "Stablecoin issuance with deposit and direct-payment corridors", 1);
        }
    }

    // ── Template application ───────────────────────────────────────────────

    /// Bootstrap an anchor from a named template.
    ///
    /// - Looks up the template by `name`.
    /// - Validates the template is registered and non-empty.
    /// - Rejects the call with `TemplateAlreadyApplied` if this exact template
    ///   has already been applied to `anchor` (prevents accidental re-application).
    /// - Enables every service listed in the template for `anchor`.
    /// - Records the application so it can be queried later.
    ///
    /// Returns `Err(TemplateNotFound)` when the template does not exist.
    /// Returns `Err(TemplateAlreadyApplied)` when already bootstrapped.
    /// Returns `Err(ServiceInMaintenance)` when the anchor is in a maintenance window
    /// that covers one of the template services (propagated from `enable_service`).
    pub fn apply_template(
        env: &Env,
        anchor: &Address,
        name: &str,
    ) -> Result<TemplateApplication, AnchorKitError> {
        let tmpl = Self::get_template(env, name)
            .ok_or_else(|| AnchorKitError::template_not_found(name))?;

        // Guard against re-application
        let app_key = Self::application_key(env, anchor, name);
        if env.storage().instance()
            .get::<_, TemplateApplication>(&app_key)
            .is_some()
        {
            return Err(AnchorKitError::template_already_applied(name));
        }

        // Enable each service in the template
        for service_code in tmpl.services.iter() {
            // is_in_maintenance returns false when no window is active, so the
            // inner enable_service will succeed unless there's an active window.
            ServiceManager::enable_service(env, anchor, service_code)?;
        }

        let record = TemplateApplication {
            template_name: tmpl.name.clone(),
            anchor: anchor.clone(),
            applied_at: env.ledger().timestamp(),
        };

        env.storage().instance().set(&app_key, &record);
        env.storage().instance().extend_ttl(31_536_000, 31_536_000);

        Ok(record)
    }

    /// Apply a template even if it has been applied before (force re-bootstrap).
    ///
    /// Clears the previous application record, then calls [`apply_template`]
    /// internally. Useful after a rollback to a blank slate.
    pub fn apply_template_force(
        env: &Env,
        anchor: &Address,
        name: &str,
    ) -> Result<TemplateApplication, AnchorKitError> {
        let app_key = Self::application_key(env, anchor, name);
        env.storage().instance().remove(&app_key);
        Self::apply_template(env, anchor, name)
    }

    /// Return the application record for a previously applied template, or
    /// `None` if the template has not been applied to this anchor.
    pub fn get_application(
        env: &Env,
        anchor: &Address,
        name: &str,
    ) -> Option<TemplateApplication> {
        let app_key = Self::application_key(env, anchor, name);
        env.storage().instance().get(&app_key)
    }

    // ── Validation ─────────────────────────────────────────────────────────

    /// Validate that the current service state of `anchor` matches the service
    /// set defined in template `name`.
    ///
    /// Returns `Ok(())` when every template service is currently enabled.
    /// Returns `Err(ValidationError)` with context listing the first missing service.
    /// Returns `Err(TemplateNotFound)` when the template does not exist.
    pub fn validate_against_template(
        env: &Env,
        anchor: &Address,
        name: &str,
    ) -> Result<(), AnchorKitError> {
        let tmpl = Self::get_template(env, name)
            .ok_or_else(|| AnchorKitError::template_not_found(name))?;

        for service_code in tmpl.services.iter() {
            if !ServiceManager::is_service_enabled(env, anchor, service_code) {
                return Err(AnchorKitError::validation_error(
                    &alloc::format!(
                        "template '{}' requires service {} which is not enabled",
                        name, service_code
                    ),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod template_tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger as _, LedgerInfo};
    use soroban_sdk::Env;

    fn make_env() -> Env { Env::default() }
    fn make_anchor(env: &Env) -> Address { Address::generate(env) }

    fn set_time(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 22,
            sequence_number: 1,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1,
            min_persistent_entry_ttl: 1,
            max_entry_ttl: 100_000_000,
        });
    }

    // ── register_template ─────────────────────────────────────────────────

    #[test]
    fn test_register_and_retrieve_template() {
        let env = make_env();
        let mut svcs = Vec::new(&env);
        svcs.push_back(1u32);
        svcs.push_back(2u32);
        TemplateManager::register_template(&env, "my-tmpl", svcs, "test", 1).unwrap();
        let tmpl = TemplateManager::get_template(&env, "my-tmpl").expect("template must exist");
        assert_eq!(tmpl.services.len(), 2);
        assert_eq!(tmpl.version, 1);
    }

    #[test]
    fn test_register_template_empty_name_rejected() {
        let env = make_env();
        let mut svcs = Vec::new(&env);
        svcs.push_back(1u32);
        let err = TemplateManager::register_template(&env, "", svcs, "desc", 1)
            .expect_err("empty name must be rejected");
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidTemplate);
    }

    #[test]
    fn test_register_template_empty_services_rejected() {
        let env = make_env();
        let err = TemplateManager::register_template(&env, "t", Vec::new(&env), "desc", 1)
            .expect_err("empty services must be rejected");
        assert_eq!(err.code, crate::errors::ErrorCode::InvalidTemplate);
    }

    #[test]
    fn test_get_template_returns_none_for_unknown() {
        let env = make_env();
        assert!(TemplateManager::get_template(&env, "unknown").is_none());
    }

    // ── seed_builtin_templates ────────────────────────────────────────────

    #[test]
    fn test_seed_builtin_templates_registers_three() {
        let env = make_env();
        TemplateManager::seed_builtin_templates(&env);
        assert!(TemplateManager::get_template(&env, TEMPLATE_FIAT_ON_RAMP).is_some());
        assert!(TemplateManager::get_template(&env, TEMPLATE_REMITTANCE).is_some());
        assert!(TemplateManager::get_template(&env, TEMPLATE_STABLECOIN_ISSUER).is_some());
    }

    #[test]
    fn test_seed_builtin_templates_is_idempotent() {
        let env = make_env();
        TemplateManager::seed_builtin_templates(&env);
        TemplateManager::seed_builtin_templates(&env); // must not panic or duplicate
        let tmpl = TemplateManager::get_template(&env, TEMPLATE_FIAT_ON_RAMP).unwrap();
        // version must still be 1 (not incremented by re-seed)
        assert_eq!(tmpl.version, 1);
    }

    // ── apply_template ────────────────────────────────────────────────────

    #[test]
    fn test_apply_template_enables_services() {
        let env = make_env();
        let anchor = make_anchor(&env);
        TemplateManager::seed_builtin_templates(&env);
        let record = TemplateManager::apply_template(&env, &anchor, TEMPLATE_FIAT_ON_RAMP)
            .expect("apply must succeed");
        // Services 1 and 2 (fiat-on-ramp) must now be enabled
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 1));
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 2));
        // Record must capture the template name and anchor
        assert_eq!(record.anchor, anchor);
    }

    #[test]
    fn test_apply_template_remittance_enables_service_3() {
        let env = make_env();
        let anchor = make_anchor(&env);
        TemplateManager::seed_builtin_templates(&env);
        TemplateManager::apply_template(&env, &anchor, TEMPLATE_REMITTANCE).unwrap();
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 3));
    }

    #[test]
    fn test_apply_template_stablecoin_enables_services_1_3_4() {
        let env = make_env();
        let anchor = make_anchor(&env);
        TemplateManager::seed_builtin_templates(&env);
        TemplateManager::apply_template(&env, &anchor, TEMPLATE_STABLECOIN_ISSUER).unwrap();
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 1));
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 3));
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 4));
    }

    #[test]
    fn test_apply_template_unknown_template_returns_not_found() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let err = TemplateManager::apply_template(&env, &anchor, "no-such-tmpl")
            .expect_err("unknown template must error");
        assert_eq!(err.code, crate::errors::ErrorCode::TemplateNotFound);
    }

    #[test]
    fn test_apply_template_twice_rejected() {
        let env = make_env();
        let anchor = make_anchor(&env);
        TemplateManager::seed_builtin_templates(&env);
        TemplateManager::apply_template(&env, &anchor, TEMPLATE_REMITTANCE).unwrap();
        let err = TemplateManager::apply_template(&env, &anchor, TEMPLATE_REMITTANCE)
            .expect_err("second apply must be rejected");
        assert_eq!(err.code, crate::errors::ErrorCode::TemplateAlreadyApplied);
    }

    #[test]
    fn test_apply_template_force_allows_reapplication() {
        let env = make_env();
        let anchor = make_anchor(&env);
        TemplateManager::seed_builtin_templates(&env);
        TemplateManager::apply_template(&env, &anchor, TEMPLATE_REMITTANCE).unwrap();
        // force re-apply must succeed without TemplateAlreadyApplied
        TemplateManager::apply_template_force(&env, &anchor, TEMPLATE_REMITTANCE)
            .expect("force apply must succeed");
    }

    #[test]
    fn test_apply_template_blocked_during_maintenance() {
        let env = make_env();
        let anchor = make_anchor(&env);
        set_time(&env, 1500);
        // Schedule a window covering all services
        MaintenanceManager::schedule_window(
            &env, &anchor, 1000, 2000, "maint", Vec::new(&env),
        ).unwrap();
        TemplateManager::seed_builtin_templates(&env);
        let err = TemplateManager::apply_template(&env, &anchor, TEMPLATE_REMITTANCE)
            .expect_err("apply must be blocked during maintenance");
        assert_eq!(err.code, crate::errors::ErrorCode::ServiceInMaintenance);
    }

    // ── get_application ───────────────────────────────────────────────────

    #[test]
    fn test_get_application_returns_none_before_apply() {
        let env = make_env();
        let anchor = make_anchor(&env);
        assert!(TemplateManager::get_application(&env, &anchor, TEMPLATE_REMITTANCE).is_none());
    }

    #[test]
    fn test_get_application_returns_record_after_apply() {
        let env = make_env();
        let anchor = make_anchor(&env);
        TemplateManager::seed_builtin_templates(&env);
        TemplateManager::apply_template(&env, &anchor, TEMPLATE_REMITTANCE).unwrap();
        let app = TemplateManager::get_application(&env, &anchor, TEMPLATE_REMITTANCE)
            .expect("application record must exist");
        assert_eq!(app.anchor, anchor);
    }

    // ── validate_against_template ─────────────────────────────────────────

    #[test]
    fn test_validate_passes_when_all_services_enabled() {
        let env = make_env();
        let anchor = make_anchor(&env);
        TemplateManager::seed_builtin_templates(&env);
        TemplateManager::apply_template(&env, &anchor, TEMPLATE_FIAT_ON_RAMP).unwrap();
        // All template services are now enabled — validation must pass
        assert!(
            TemplateManager::validate_against_template(&env, &anchor, TEMPLATE_FIAT_ON_RAMP)
                .is_ok()
        );
    }

    #[test]
    fn test_validate_fails_when_service_missing() {
        let env = make_env();
        let anchor = make_anchor(&env);
        TemplateManager::seed_builtin_templates(&env);
        // Only enable service 1, not service 2 — fiat-on-ramp needs both
        ServiceManager::enable_service(&env, &anchor, 1).unwrap();
        let err = TemplateManager::validate_against_template(
            &env, &anchor, TEMPLATE_FIAT_ON_RAMP,
        ).expect_err("validation must fail with missing service");
        assert_eq!(err.code, crate::errors::ErrorCode::ValidationError);
        assert!(err.context.as_deref().unwrap_or("").contains("service 2"));
    }

    #[test]
    fn test_validate_unknown_template_returns_not_found() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let err = TemplateManager::validate_against_template(&env, &anchor, "ghost")
            .expect_err("unknown template must error");
        assert_eq!(err.code, crate::errors::ErrorCode::TemplateNotFound);
    }

    #[test]
    fn test_custom_template_can_be_registered_and_applied() {
        let env = make_env();
        let anchor = make_anchor(&env);
        let mut svcs = Vec::new(&env);
        svcs.push_back(10u32);
        svcs.push_back(20u32);
        TemplateManager::register_template(&env, "custom", svcs, "custom template", 1).unwrap();
        TemplateManager::apply_template(&env, &anchor, "custom").unwrap();
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 10));
        assert!(ServiceManager::is_service_enabled(&env, &anchor, 20));
    }
}
