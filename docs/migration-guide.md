# Contract Migration and Upgrade Guide

This document describes the contract upgrade and migration process for SorobanAnchor, including data preservation strategies and testing procedures.

## Overview

Contract upgrades are critical operations that must preserve all stored data while advancing the contract logic. SorobanAnchor implements a two-phase upgrade process:

1. **Upgrade**: Deploy new contract code (WASM)
2. **Migrate**: Update schema version and perform data transformations

## Upgrade Process

### Prerequisites

- Admin authorization required
- Valid WASM hash (non-zero)
- Contract must be initialized

### Upgrade Flow

```rust
// 1. Admin calls upgrade with new WASM hash
client.upgrade(&new_wasm_hash);

// 2. Contract validates:
//    - Caller is admin
//    - WASM hash is non-zero
//    - Contract is initialized

// 3. New WASM is deployed
// 4. All stored data remains accessible
```

### Upgrade Authorization

Only the admin address set during initialization can authorize upgrades:

```rust
let admin = Address::generate(&env);
client.initialize(&admin);

// Later, only admin can upgrade
client.upgrade(&new_wasm_hash);  // ✓ Succeeds
```

## Migration Process

### Schema Versioning

Each contract version has a schema version number:

- **Version 0**: Initial deployment
- **Version 1+**: After migrations

### Migration Flow

```rust
// 1. Admin calls migrate with new version
client.migrate(&new_version);

// 2. Contract validates:
//    - Caller is admin
//    - New version > current version
//    - New version != 0

// 3. Schema version is updated
// 4. Data transformations are applied (if any)
```

### Version Constraints

- Versions must be strictly increasing
- Cannot migrate to same version
- Cannot downgrade to lower version
- Cannot migrate to version 0

### Valid Migration Paths

```
0 → 1 ✓
0 → 5 ✓ (skip versions)
1 → 2 ✓
5 → 5 ✗ (same version)
5 → 3 ✗ (downgrade)
1 → 0 ✗ (cannot go to 0)
```

## Data Preservation

### Preserved Data Types

The following data types are preserved across upgrades and migrations:

1. **Attestations**
   - Issuer, subject, timestamp
   - Payload hash, signature
   - Schema version

2. **Quotes**
   - Anchor, base/quote assets
   - Rate, fees, amounts
   - Validity period
   - Schema version

3. **Sessions**
   - Initiator, creation time
   - Session TTL, nonce
   - Operation count
   - Closed status

4. **Audit Logs**
   - Session ID, actor
   - Operation context
   - Timestamps

### Data Accessibility

After upgrade/migration, all data remains accessible through the same APIs:

```rust
// Before upgrade
let attestation = client.get_attestation(&id);

// After upgrade
let attestation = client.get_attestation(&id);  // Same data
```

## Schema Changes

### Handling Breaking Changes

When a schema change is required:

1. **Plan the migration**
   - Identify affected data types
   - Design transformation logic
   - Plan rollback strategy

2. **Implement migration logic**
   - Add migration code in `migrate()` function
   - Transform existing data
   - Validate data integrity

3. **Test thoroughly**
   - Test data preservation
   - Test transformation logic
   - Test rollback scenarios

### Example: Adding a Field

If adding a new field to `Attestation`:

```rust
// Old schema (v1)
pub struct Attestation {
    pub id: u64,
    pub issuer: Address,
    pub subject: Address,
    pub timestamp: u64,
    pub payload_hash: Bytes,
    pub signature: Bytes,
    pub schema_version: u32,
}

// New schema (v2)
pub struct Attestation {
    pub id: u64,
    pub issuer: Address,
    pub subject: Address,
    pub timestamp: u64,
    pub payload_hash: Bytes,
    pub signature: Bytes,
    pub schema_version: u32,
    pub verified_at: u64,  // New field
}

// Migration logic
fn migrate_attestations_v1_to_v2(env: &Env) {
    // For each existing attestation:
    // - Load old data
    // - Set verified_at to current timestamp
    // - Save updated record
}
```

## Testing Upgrades and Migrations

### Test Coverage

The test suite includes:

1. **Data Preservation Tests**
   - Attestations preserved after upgrade
   - Quotes preserved after upgrade
   - Sessions preserved after upgrade
   - Multiple data types preserved together

2. **Migration Path Tests**
   - Migration to higher version succeeds
   - Migration can skip versions
   - Migration to same version fails
   - Migration to lower version fails
   - Migration to zero version fails

3. **Data Compatibility Tests**
   - Data remains consistent across multiple upgrades
   - Schema version tracked correctly
   - Records include schema version

4. **Authorization Tests**
   - Only admin can upgrade
   - Only admin can migrate

### Running Migration Tests

```bash
# Run all migration tests
cargo test migration_tests

# Run specific test
cargo test migration_tests::attestations_preserved_after_upgrade

# Run with output
cargo test migration_tests -- --nocapture
```

## Rollback Strategy

### Rollback Scenarios

1. **Failed Upgrade**
   - If new WASM has bugs, deploy previous WASM
   - All data remains intact
   - No migration needed

2. **Failed Migration**
   - If migration logic has bugs, migrate to previous version
   - Data may need manual recovery
   - Consider data backup before migration

### Rollback Procedure

```rust
// If upgrade fails:
// 1. Deploy previous WASM
client.upgrade(&previous_wasm_hash);

// 2. If migration was attempted, migrate back
client.migrate(&previous_version);

// 3. Verify data integrity
let attestation = client.get_attestation(&id);
assert_eq!(attestation.issuer, expected_issuer);
```

## Best Practices

### Before Upgrading

1. **Backup data**
   - Export all attestations, quotes, sessions
   - Store backup in secure location

2. **Test thoroughly**
   - Run full test suite
   - Test on testnet first
   - Verify data preservation

3. **Plan rollback**
   - Document previous WASM hash
   - Document previous schema version
   - Have rollback procedure ready

### During Upgrade

1. **Coordinate with users**
   - Announce maintenance window
   - Provide status updates
   - Minimize downtime

2. **Monitor closely**
   - Watch for errors
   - Verify data accessibility
   - Check audit logs

### After Upgrade

1. **Verify integrity**
   - Spot-check data samples
   - Run health checks
   - Monitor for anomalies

2. **Document changes**
   - Record upgrade timestamp
   - Document schema changes
   - Update runbooks

## Troubleshooting

### Common Issues

**Issue**: Upgrade fails with "InvalidPayload"
- **Cause**: WASM hash is all zeros
- **Solution**: Provide valid non-zero WASM hash

**Issue**: Migration fails with "InvalidPayload"
- **Cause**: Version is zero or not advancing
- **Solution**: Use version > current version

**Issue**: Data not accessible after upgrade
- **Cause**: Data not properly preserved
- **Solution**: Check migration logic, restore from backup

**Issue**: Unauthorized upgrade attempt
- **Cause**: Caller is not admin
- **Solution**: Use admin address for upgrade

## References

- [Soroban Contract Upgrades](https://developers.stellar.org/docs/learn/storing-data)
- [Admin Audit Log](./admin-audit-log.md)
- [Governance and Security](./governance-and-security.md)
- [Upgrade Playbook](./upgrade-playbook.md) — step-by-step production upgrade and rollback procedure
