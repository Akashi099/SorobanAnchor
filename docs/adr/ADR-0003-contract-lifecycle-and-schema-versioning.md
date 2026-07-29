# ADR-0003: Contract Lifecycle and Schema Versioning

- **Status:** Accepted
- **Date:** 2026-07-29
- **Author:** Project Maintainers
- **Supersedes:** None

## Context

The AnchorKit Soroban contract is a long-lived on-chain program that may be
upgraded multiple times after deployment. Upgrades can change the data layout
(adding/removing/renaming fields in persisted structs), the contract logic, or
both. Unlike a disposable proxy, every version of the contract shares the same
persistent storage on the Soroban ledger.

The project needed a lifecycle model that answers:

1. How does the contract transition from uninitialised to operational?
2. How are WASM upgrades performed safely?
3. How do data schema changes work across upgrades?
4. How are sessions (multi-operation workflows) managed on-chain?

## Decision

### 1. Explicit initialisation

The contract has a two-phase boot: deploy then initialise. `initialize()` sets
the admin address, writes an `INITIALIZED` flag, and records the initial schema
version. `initialize()` is callable exactly once — the flag prevents re-initialisation
after upgrades. Every public function checks `is_initialized()` and returns
`NotInitialized` (code 23) if the flag is absent.

### 2. Upgrade via WASM hash replacement

`upgrade(new_wasm_hash)` atomically replaces the contract bytecode.
It is gated to the primary admin only (no role/capability delegation) to
prevent accidental privilege escalation. Each upgrade:
- Increments the patch version
- Records the old and new WASM hashes
- Emits an `UpgradeEvent`
- Writes an audit log entry

The admin may roll back by re-deploying the previous WASM hash (kept in the
release archive).

### 3. Schema versioning separated from upgrades

`migrate(new_schema_version, batch_size)` advances the on-chain schema
version. It is deliberately a **separate call** from `upgrade` so that:

- State transitions are explicit and auditable
- A failed migration does not roll back the WASM upgrade
- Batch migration can run across multiple invocations (for large datasets)

Schema versions are monotonic (`SCHEMA_V1 = 1`, `SCHEMA_V2 = 2`). The
contract rejects migration to a version the current binary does not
understand (`UnsupportedCapabilityVersion`).

### 4. Session lifecycle state machine

Multi-operation workflows use an explicit state machine:

```
[Created] → [Active] → [Exhausted]
    │           │            │
    └─→ [Closed] ←───────────┘
    │
    └─→ [Expired] (ttl elapsed, any state)
```

- `Created`: session opened, no operations recorded
- `Active`: at least one operation, more allowed
- `Exhausted`: operation limit reached
- `Closed`: explicitly closed by the initiator
- `Expired`: TTL elapsed (computed at read time without writing)

Invalid transitions are rejected with specific error codes.

### 5. Admin capability model

Rather than a single admin-or-not gate, the contract uses two layers of
delegated authority:

| Layer | Mechanism | Scope |
|-------|-----------|-------|
| Roles | `grant_role` / `revoke_role` | Coarse-grained category (KycAdmin, AttestorAdmin, CacheAdmin) |
| Capabilities | `grant_capability` / `revoke_capability` | Fine-grained per-operation |

The primary admin always passes every check. Roles and capabilities are
additive — holding either is sufficient to invoke the protected operation.

## Consequences

**Positive:**
- Safe upgrade path: WASM can change without data loss, and data can be
  migrated independently.
- Audit trail: every upgrade, migration, and admin action is logged.
- Deterministic session semantics: callers can rely on lifecycle guarantees.
- Fine-grained delegation reduces the attack surface of admin keys.

**Negative:**
- Migration complexity: batch migrations require multiple invocations for
  large stores (mitigated by batched v1→v2 quote migration).
- Session TTL is computed at read time (not eagerly expired), which means
  expired sessions may consume storage until compacted.
- The dual role/capability model adds conceptual overhead for new
  integrators.

**Alternatives considered:**
- **Proxy pattern** — a separate proxy contract delegating to implementation
  contracts. Rejected because Soroban's native `upgrade` is simpler and
  cheaper; the proxy would add another contract to audit.
- **Single admin gate** — simpler but offers no delegation. Rejected when
  operational needs required scoped access for KYC and cache operations.
