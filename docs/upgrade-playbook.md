# Migration Guide and Upgrade Playbook

This playbook gives maintainers a step-by-step procedure for moving a
production SorobanAnchor deployment from one version to the next.  It covers
pre-upgrade validation, the upgrade itself, post-upgrade verification, and a
complete rollback procedure.

---

## Table of Contents

1. [When to Use This Guide](#1-when-to-use-this-guide)
2. [Prerequisites and Tools](#2-prerequisites-and-tools)
3. [Schema Versioning Primer](#3-schema-versioning-primer)
4. [Pre-Upgrade Checklist](#4-pre-upgrade-checklist)
5. [Step-by-Step Upgrade Procedure](#5-step-by-step-upgrade-procedure)
6. [Post-Upgrade Verification](#6-post-upgrade-verification)
7. [Rollback Procedure](#7-rollback-procedure)
8. [Version-Specific Migration Notes](#8-version-specific-migration-notes)
9. [Compatibility Matrix](#9-compatibility-matrix)
10. [Troubleshooting](#10-troubleshooting)

---

## 1  When to Use This Guide

Follow this playbook any time you:

- Deploy a new WASM artifact to a contract that has live data on-chain.
- Advance the on-chain schema version (call `migrate()`).
- Need to recover from a failed upgrade or migration.

Minor documentation or script changes that do not touch the compiled contract
do **not** require this procedure.

---

## 2  Prerequisites and Tools

| Requirement | How to verify |
|-------------|---------------|
| Rust stable toolchain | `rustc --version` |
| `wasm32-unknown-unknown` target | `rustup target list --installed \| grep wasm32` |
| `binaryen` (`wasm-opt`) | `wasm-opt --version` |
| Stellar CLI or `soroban-cli` | `stellar --version` |
| Admin key access (offline HSM / hardware wallet) | — |
| A snapshot / export of on-chain data | See step 4.3 |

---

## 3  Schema Versioning Primer

Every persistent record type (`Attestation`, `Quote`, `KycRecord`) carries a
`schema_version: u32` field.  Version constants are defined in `src/contract.rs`:

```
SCHEMA_V1 = 1   — initial versioned layout
SCHEMA_V2 = 2   — adds routing_reason to Quote
```

The contract function `get_schema_version()` returns the currently active
version.  Calling `migrate(new_version, batch_size)` advances the version and
rewrites any legacy records in the affected types.

**Valid version transitions:**

```
current → target   Result
─────────────────────────────────────────────────────────────────────────
V1      → V2       ✓  Adds routing_reason to Quote records
V2      → V2       ✗  Panics — version must advance
V2      → V1       ✗  Panics — downgrade not supported
any     → 0        ✗  Panics — version 0 is reserved
any     → future   ✗  Panics — contract does not know that version yet
```

> **Tip:** Use `get_migration_count()` and `get_migration_record(idx)` to
> inspect the on-chain migration history log.

---

## 4  Pre-Upgrade Checklist

Work through every item before touching the live contract.

### 4.1  Build the release artifacts

```bash
# Produce both the CLI binary and the optimised WASM:
make release

# Confirm the bundle is well-formed:
make release-validate
```

Note the SHA-256 checksum printed by `make release`:

```
dist/anchorkit-0.2.0.sha256   ← record this value
```

### 4.2  Run the full test suite

```bash
cargo test
cargo test --test cli_integration_harness -- --nocapture
```

Both must pass on the exact commit you intend to deploy.

### 4.3  Export on-chain data (backup)

Use the Stellar CLI to export all persistent data before the upgrade.
At minimum capture:

```bash
# All attestation IDs
soroban contract invoke \
  --id "$CONTRACT_ID" --rpc-url "$RPC_URL" --network-passphrase "$PASSPHRASE" \
  -- get_attestations_paginated --offset 0 --limit 50 --filter null

# All audit log entries
soroban contract invoke \
  --id "$CONTRACT_ID" ... -- get_audit_logs_paginated --offset 0 --limit 50

# Current schema version
soroban contract invoke \
  --id "$CONTRACT_ID" ... -- get_schema_version
```

Store the output in a dated file (e.g. `backup/pre-upgrade-2026-07-28.json`).

### 4.4  Record the existing WASM hash

The contract stores the last-deployed hash under the `OLDHASH` key.  Retrieve
it so you can reference it during a potential rollback:

```bash
soroban contract invoke --id "$CONTRACT_ID" ... -- get_version
# Note the patch, minor, major values and upgraded_at timestamp.
```

### 4.5  Coordinate a maintenance window

- Announce the maintenance window to anchor operators at least 24 hours in
  advance.
- Set any external monitors to maintenance mode.
- The upgrade itself is atomic (single Stellar transaction), but the
  `migrate()` call may require multiple batched transactions for large datasets.

---

## 5  Step-by-Step Upgrade Procedure

### 5.1  Retrieve the new WASM hash

```bash
# Build the WASM (or use the pre-built artifact from the release bundle):
cargo build --release \
  --target wasm32-unknown-unknown \
  --no-default-features \
  --features wasm

WASM_FILE="target/wasm32-unknown-unknown/release/anchorkit.wasm"

# Compute and verify the checksum:
sha256sum "$WASM_FILE"   # must match the published checksum
```

Upload the WASM to the Stellar network to obtain the WASM hash:

```bash
soroban contract upload \
  --rpc-url "$RPC_URL" \
  --network-passphrase "$PASSPHRASE" \
  --source "$ADMIN_SECRET" \
  --wasm "$WASM_FILE"
# Stellar returns: <NEW_WASM_HASH>
```

### 5.2  Call `upgrade()`

```bash
soroban contract invoke \
  --id "$CONTRACT_ID" \
  --rpc-url "$RPC_URL" \
  --network-passphrase "$PASSPHRASE" \
  --source "$ADMIN_SECRET" \
  -- upgrade \
  --new_wasm_hash "$NEW_WASM_HASH"
```

The contract emits an `UpgradeEvent` and increments the patch version.  Verify:

```bash
soroban contract invoke --id "$CONTRACT_ID" ... -- get_version
# Patch component should be one higher than before.
```

### 5.3  Call `migrate()` (if schema version advances)

Check the release notes for the target version.  If no schema change is
documented, skip this step.

```bash
# Migrate to v2 in batches of 100 records:
soroban contract invoke \
  --id "$CONTRACT_ID" \
  --rpc-url "$RPC_URL" \
  --network-passphrase "$PASSPHRASE" \
  --source "$ADMIN_SECRET" \
  -- migrate \
  --new_schema_version 2 \
  --batch_size 100
```

If the dataset is large, `migrate()` returns without advancing the version
until all records are processed.  Repeat the call until `get_schema_version()`
returns the target version:

```bash
while true; do
  version=$(soroban contract invoke --id "$CONTRACT_ID" ... -- get_schema_version)
  echo "Schema version: $version"
  if [ "$version" == "2" ]; then break; fi
  sleep 5
  soroban contract invoke --id "$CONTRACT_ID" ... -- migrate \
    --new_schema_version 2 --batch_size 100
done
```

---

## 6  Post-Upgrade Verification

Run these checks immediately after the upgrade completes.

### 6.1  Smoke-test core functions

```bash
# Confirm contract is initialized and healthy:
soroban contract invoke --id "$CONTRACT_ID" ... -- is_initialized
soroban contract invoke --id "$CONTRACT_ID" ... -- get_health_status

# Confirm schema version matches target:
soroban contract invoke --id "$CONTRACT_ID" ... -- get_schema_version

# Read back a known attestation to verify data integrity:
soroban contract invoke --id "$CONTRACT_ID" ... -- get_attestation \
  --id <KNOWN_ATTESTATION_ID>
```

### 6.2  Run the CLI integration harness

```bash
make integration-test
```

All steps should pass without modification.

### 6.3  Compare the backup

Spot-check five or more records against the pre-upgrade backup export.
Fields should match exactly, with the addition of any new schema fields
populated with their default values.

### 6.4  Restore monitoring

Disable maintenance mode on external monitors and confirm they are receiving
healthy signals from the contract.

---

## 7  Rollback Procedure

If any step above fails, roll back by re-deploying the previous WASM.
On-chain data is **never deleted** during an upgrade, so a rollback only needs
to replace the contract code.

```bash
# 1.  Re-upload the previous WASM (or use the hash recorded in step 4.4).
soroban contract upload \
  --rpc-url "$RPC_URL" \
  --network-passphrase "$PASSPHRASE" \
  --source "$ADMIN_SECRET" \
  --wasm "dist/anchorkit-<PREVIOUS_VERSION>/anchorkit.wasm"
# Returns: <PREVIOUS_WASM_HASH>

# 2.  Call upgrade() with the previous hash.
soroban contract invoke \
  --id "$CONTRACT_ID" \
  --rpc-url "$RPC_URL" \
  --network-passphrase "$PASSPHRASE" \
  --source "$ADMIN_SECRET" \
  -- upgrade \
  --new_wasm_hash "$PREVIOUS_WASM_HASH"

# 3.  Verify the old version is restored.
soroban contract invoke --id "$CONTRACT_ID" ... -- get_version
```

### Schema rollback

The `migrate()` function only advances the schema version; it cannot
downgrade.  If a migration produced corrupt records, the recovery path is:

1. Restore data from the pre-upgrade backup export.
2. Roll back the WASM (step above).
3. Contact the maintainers to assess whether manual ledger patching is needed.

> **Note:** Schema downgrade is not supported in the current release.  Plan
> migrations carefully and always take a backup before running `migrate()`.

---

## 8  Version-Specific Migration Notes

### v0.1.0 → v0.1.x (patch releases)

Patch releases do not change the schema.  Only the WASM upgrade step (5.2)
is required; skip step 5.3 (`migrate()`).

### v0.1.x → v0.2.0 (SCHEMA_V2)

- **Change:** `Quote` records gain `routing_reason: Option<String>`.
- **Migration:** Call `migrate(2, 100)` to rewrite existing `QuoteV1` records
  to `Quote` with `routing_reason = None`.
- **Batch size:** Use `batch_size = 100` for production; increase if ledger
  budget allows.
- **Repeat:** Call until `get_schema_version()` returns `2`.
- **Backward compatibility:** Pre-migration `QuoteV1` records decoded by the
  V1 WASM continue to work.  After the migration, the new WASM reads all
  records as `Quote`.

---

## 9  Compatibility Matrix

| Deployed WASM | On-chain schema | Compatible? | Notes |
|---------------|-----------------|-------------|-------|
| v0.1.0 | V1 | ✓ | Initial deployment |
| v0.2.0 | V1 | ✓ | WASM upgrade only; migrate() not yet called |
| v0.2.0 | V2 | ✓ | After migrate(2, ...) completes |
| v0.1.0 | V2 | ✗ | Old WASM cannot decode V2 Quote records |

---

## 10  Troubleshooting

### `upgrade()` panics with `ValidationError`

The `new_wasm_hash` argument is all-zero bytes.  Ensure you are passing the
hash returned by `soroban contract upload`, not a placeholder.

### `migrate()` panics with `ValidationError`

One of:
- `new_schema_version == 0` — version 0 is reserved.
- `new_schema_version <= current version` — must advance strictly.
- `new_schema_version > SCHEMA_V2` — the WASM does not know this version yet.

Verify the target version with `get_schema_version()` first.

### `migrate()` returns but schema version did not advance

The batch was incomplete.  Call `migrate()` again with the same version until
`get_schema_version()` matches the target.

### Data not accessible after upgrade

If records return `AttestationNotFound` or `QuoteNotFound` after a successful
upgrade, the schema migration may be incomplete.  Check `get_schema_version()`
and call `migrate()` again if necessary.  If the issue persists, restore from
the pre-upgrade backup and roll back the WASM.

---

## References

- [Contract upgrade source](../src/contract.rs) — `upgrade()`, `migrate()`, `get_schema_version()`
- [Schema versioning](../src/migration.rs) — migration framework
- [Governance and security](governance-and-security.md)
- [Admin audit log](admin-audit-log.md)
- [Contract functions reference](CONTRACT_FUNCTIONS.md)
- [RUNBOOK](RUNBOOK.md)
- [Release packaging](../scripts/package_release.sh)
