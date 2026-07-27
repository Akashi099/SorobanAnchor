//! Versioned migration framework for on-chain storage upgrades.
//!
//! ## Design goals
//!
//! 1. **Explicit version checks** — every migration validates that the stored
//!    schema version exactly matches its precondition before touching storage.
//! 2. **Rollback-safe writes** — migrations read, transform, and write in a
//!    single logical step; the stored version is only advanced after all data
//!    writes succeed.
//! 3. **Recorded history** — each applied migration appends an entry to a
//!    persistent log so operators can audit exactly which migrations have run.
//! 4. **Forward-safety** — the framework refuses to apply a migration whose
//!    target version exceeds the highest version the current WASM binary
//!    understands, preventing half-applied upgrades.
//!
//! ## Adding a new migration
//!
//! 1. Increment `LATEST_SCHEMA_VERSION`.
//! 2. Add a new `MigrationStep::V<N>` variant.
//! 3. Implement the migration logic inside `apply_step`.
//! 4. Register the step in `ALL_STEPS`.

use soroban_sdk::{contracttype, Env, String};
use crate::deterministic_hash::make_storage_key;

// ---------------------------------------------------------------------------
// Version constants
// ---------------------------------------------------------------------------

/// The initial schema version written by `initialize()`.
pub const SCHEMA_V1: u32 = 1;

/// Schema V2 adds `routing_reason` to [`Quote`](crate::contract::Quote) records.
pub const SCHEMA_V2: u32 = 2;

/// The highest schema version this binary understands.  
/// `migrate()` rejects any target version greater than this.
pub const LATEST_SCHEMA_VERSION: u32 = SCHEMA_V2;

// ---------------------------------------------------------------------------
// Migration step registry
// ---------------------------------------------------------------------------

/// An individual, idempotent migration step.
///
/// Each variant maps to exactly one schema version bump.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MigrationStep {
    /// V1 → V2: add `routing_reason` field to all stored [`Quote`] records.
    ToV2,
}

/// All known migration steps in application order.
pub const ALL_STEPS: &[MigrationStep] = &[
    MigrationStep::ToV2,
];

impl MigrationStep {
    /// The schema version this step requires as a *precondition* (current stored version).
    pub fn required_from(self) -> u32 {
        match self {
            MigrationStep::ToV2 => SCHEMA_V1,
        }
    }

    /// The schema version produced after this step completes successfully.
    pub fn produces(self) -> u32 {
        match self {
            MigrationStep::ToV2 => SCHEMA_V2,
        }
    }

    /// A stable, human-readable label for audit log entries.
    pub fn label(self) -> &'static str {
        match self {
            MigrationStep::ToV2 => "quotes_v1_to_v2",
        }
    }
}

// ---------------------------------------------------------------------------
// Migration history record
// ---------------------------------------------------------------------------

/// Persistent record of a single applied migration.
#[contracttype]
#[derive(Clone, Debug)]
pub struct MigrationRecord {
    /// Which schema version was in effect before this migration ran.
    pub from_version: u32,
    /// Which schema version is in effect after this migration completed.
    pub to_version: u32,
    /// Ledger timestamp when the migration completed.
    pub applied_at: u64,
    /// The ledger sequence number when applied.
    pub applied_at_ledger: u32,
    /// Human-readable label identifying the migration step.
    pub label: String,
}

// ---------------------------------------------------------------------------
// Storage key helpers
// ---------------------------------------------------------------------------

fn migration_count_key(env: &Env) -> soroban_sdk::BytesN<32> {
    make_storage_key(env, &[b"MIG_CNT"])
}

fn migration_record_key(env: &Env, idx: u32) -> soroban_sdk::BytesN<32> {
    make_storage_key(env, &[b"MIG_REC", &idx.to_be_bytes()])
}

pub(crate) fn schema_version_key(env: &Env) -> soroban_sdk::BytesN<32> {
    make_storage_key(env, &[b"SCHEMAVER"])
}

// ---------------------------------------------------------------------------
// Public API used by the contract layer
// ---------------------------------------------------------------------------

/// Read the currently stored schema version.
/// Returns `0` if `initialize()` has never been called.
pub fn current_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&schema_version_key(env))
        .unwrap_or(0)
}

/// Write the schema version. Called once by `initialize()` to stamp V1.
pub fn set_version(env: &Env, version: u32) {
    env.storage()
        .instance()
        .set(&schema_version_key(env), &version);
}

/// Advance the stored schema version and append a [`MigrationRecord`] to the
/// persistent history log.
///
/// **Must only be called** after all data writes for the step have succeeded.
/// The caller (the contract's `migrate()` function) is responsible for the
/// data-transformation work; this function only commits the version bump and
/// writes the audit record.
pub fn commit_version(env: &Env, from: u32, to: u32, label: &str) {
    // Advance the stored version.
    env.storage()
        .instance()
        .set(&schema_version_key(env), &to);

    // Append history record.
    let cnt_key = migration_count_key(env);
    let idx: u32 = env
        .storage()
        .persistent()
        .get(&cnt_key)
        .unwrap_or(0u32);

    let record = MigrationRecord {
        from_version: from,
        to_version: to,
        applied_at: env.ledger().timestamp(),
        applied_at_ledger: env.ledger().sequence(),
        label: String::from_str(env, label),
    };
    let rec_key = migration_record_key(env, idx);
    env.storage().persistent().set(&rec_key, &record);
    // Use a generous TTL — migration history should outlive any individual record.
    env.storage()
        .persistent()
        .extend_ttl(&rec_key, 1_555_200, 1_555_200);

    env.storage()
        .persistent()
        .set(&cnt_key, &(idx + 1));
    env.storage()
        .persistent()
        .extend_ttl(&cnt_key, 1_555_200, 1_555_200);
}

/// Return the total number of migrations recorded in the history log.
pub fn migration_count(env: &Env) -> u32 {
    env.storage()
        .persistent()
        .get(&migration_count_key(env))
        .unwrap_or(0)
}

/// Return a specific migration record by zero-based index, or `None`.
pub fn get_migration_record(env: &Env, idx: u32) -> Option<MigrationRecord> {
    env.storage()
        .persistent()
        .get(&migration_record_key(env, idx))
}

/// Validate that `target_version` is a legal migration target.
///
/// Returns `Ok(step)` with the matching [`MigrationStep`] when:
/// - `target_version > 0`
/// - `target_version > current_version`
/// - `target_version <= LATEST_SCHEMA_VERSION`
/// - a registered step exists whose `required_from` matches `current_version`
///   and whose `produces` matches `target_version`
///
/// Returns `Err(MigrationError)` otherwise.
pub fn validate_migration(
    env: &Env,
    target_version: u32,
) -> Result<MigrationStep, MigrationError> {
    if target_version == 0 {
        return Err(MigrationError::InvalidTargetVersion);
    }
    if target_version > LATEST_SCHEMA_VERSION {
        return Err(MigrationError::VersionTooNew);
    }
    let current = current_version(env);
    if target_version <= current {
        return Err(MigrationError::VersionNotAdvancing);
    }
    // Find a step that bridges current → target.
    for &step in ALL_STEPS {
        if step.required_from() == current && step.produces() == target_version {
            return Ok(step);
        }
    }
    Err(MigrationError::NoStepFound)
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Reason a migration request was rejected.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MigrationError {
    /// `target_version` was 0.
    InvalidTargetVersion,
    /// `target_version` exceeds `LATEST_SCHEMA_VERSION`.
    VersionTooNew,
    /// `target_version` is not greater than the current stored version.
    VersionNotAdvancing,
    /// No registered migration step bridges the current version to the target.
    NoStepFound,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal Soroban env setup shared by all tests.
    fn test_env() -> soroban_sdk::Env {
        soroban_sdk::Env::default()
    }

    // -----------------------------------------------------------------------
    // Version constants
    // -----------------------------------------------------------------------

    #[test]
    fn test_latest_schema_version_is_v2() {
        assert_eq!(LATEST_SCHEMA_VERSION, SCHEMA_V2);
        assert_eq!(SCHEMA_V1, 1u32);
        assert_eq!(SCHEMA_V2, 2u32);
    }

    // -----------------------------------------------------------------------
    // current_version / set_version
    // -----------------------------------------------------------------------

    #[test]
    fn test_current_version_default_is_zero() {
        let env = test_env();
        assert_eq!(current_version(&env), 0);
    }

    #[test]
    fn test_set_version_stores_value() {
        let env = test_env();
        set_version(&env, SCHEMA_V1);
        assert_eq!(current_version(&env), SCHEMA_V1);
    }

    // -----------------------------------------------------------------------
    // validate_migration
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_migration_zero_target_fails() {
        let env = test_env();
        set_version(&env, SCHEMA_V1);
        assert_eq!(
            validate_migration(&env, 0),
            Err(MigrationError::InvalidTargetVersion)
        );
    }

    #[test]
    fn test_validate_migration_version_too_new_fails() {
        let env = test_env();
        set_version(&env, SCHEMA_V1);
        assert_eq!(
            validate_migration(&env, LATEST_SCHEMA_VERSION + 1),
            Err(MigrationError::VersionTooNew)
        );
    }

    #[test]
    fn test_validate_migration_not_advancing_fails() {
        let env = test_env();
        set_version(&env, SCHEMA_V2);
        // V2 → V2 must fail.
        assert_eq!(
            validate_migration(&env, SCHEMA_V2),
            Err(MigrationError::VersionNotAdvancing)
        );
    }

    #[test]
    fn test_validate_migration_v1_to_v2_succeeds() {
        let env = test_env();
        set_version(&env, SCHEMA_V1);
        let result = validate_migration(&env, SCHEMA_V2);
        assert!(result.is_ok(), "v1 → v2 should be valid");
        assert_eq!(result.unwrap(), MigrationStep::ToV2);
    }

    #[test]
    fn test_validate_migration_skipping_step_fails() {
        let env = test_env();
        // No step exists from V0 → V2 (skips V1).
        set_version(&env, 0);
        let result = validate_migration(&env, SCHEMA_V2);
        // There is no registered step from 0 → 2 (only 1 → 2 is registered).
        assert_eq!(result, Err(MigrationError::NoStepFound));
    }

    // -----------------------------------------------------------------------
    // commit_version / migration history
    // -----------------------------------------------------------------------

    #[test]
    fn test_commit_version_advances_stored_version() {
        let env = test_env();
        set_version(&env, SCHEMA_V1);
        commit_version(&env, SCHEMA_V1, SCHEMA_V2, "test_migration");
        assert_eq!(current_version(&env), SCHEMA_V2);
    }

    #[test]
    fn test_commit_version_appends_history_record() {
        let env = test_env();
        set_version(&env, SCHEMA_V1);
        assert_eq!(migration_count(&env), 0);
        commit_version(&env, SCHEMA_V1, SCHEMA_V2, "quotes_v1_to_v2");
        assert_eq!(migration_count(&env), 1);
    }

    #[test]
    fn test_get_migration_record_returns_correct_data() {
        let env = test_env();
        set_version(&env, SCHEMA_V1);
        commit_version(&env, SCHEMA_V1, SCHEMA_V2, "quotes_v1_to_v2");
        let rec = get_migration_record(&env, 0).expect("record 0 should exist");
        assert_eq!(rec.from_version, SCHEMA_V1);
        assert_eq!(rec.to_version, SCHEMA_V2);
    }

    #[test]
    fn test_get_migration_record_out_of_range_returns_none() {
        let env = test_env();
        assert!(get_migration_record(&env, 99).is_none());
    }

    #[test]
    fn test_multiple_commits_all_recorded() {
        let env = test_env();
        // Simulate two distinct migrations (using raw set_version for the second).
        set_version(&env, SCHEMA_V1);
        commit_version(&env, SCHEMA_V1, SCHEMA_V2, "step_a");
        // Fake a third version for the second commit without a real step.
        commit_version(&env, SCHEMA_V2, 3, "step_b");
        assert_eq!(migration_count(&env), 2);
        let r0 = get_migration_record(&env, 0).unwrap();
        let r1 = get_migration_record(&env, 1).unwrap();
        assert_eq!(r0.to_version, SCHEMA_V2);
        assert_eq!(r1.from_version, SCHEMA_V2);
        assert_eq!(r1.to_version, 3);
    }

    // -----------------------------------------------------------------------
    // MigrationStep helpers
    // -----------------------------------------------------------------------

    #[test]
    fn test_step_to_v2_metadata() {
        let step = MigrationStep::ToV2;
        assert_eq!(step.required_from(), SCHEMA_V1);
        assert_eq!(step.produces(), SCHEMA_V2);
        assert_eq!(step.label(), "quotes_v1_to_v2");
    }
}
