# ADR-0002: Dual-Build Strategy

- **Status:** Accepted
- **Date:** 2026-07-29
- **Author:** Project Maintainers
- **Supersedes:** None

## Context

AnchorKit operates in two distinct environments:

1. **On-chain Soroban smart contract** — deployed as WASM on the Stellar
   network. This environment has no standard library (`no_std`), no HTTP
   client, no filesystem, and no CLI.
2. **Off-chain CLI / service** — a native binary that loads configuration,
   manages keys, talks to anchor APIs over HTTPS, and orchestrates attestation
   workflows.

Both environments share a large body of business logic: SEP protocol handling,
response validation, deterministic hashing, domain validation, and error types.
Duplicating this logic would be a maintenance burden.

However, the WASM contract must be minimal (``opt-level = "z"``, `panic =
"abort"`, no standard library) while the native binary needs blocking HTTP,
filesystem access, and interactive password prompts.

## Decision

We use a single Cargo crate with two mutually exclusive compile modes driven
by Cargo feature flags:

| Mode | Command | Target | Features |
|------|---------|--------|----------|
| Native | `cargo build --release` | host triple | `std` (default) |
| WASM | `cargo build --release --target wasm32-unknown-unknown --no-default-features --features wasm` | `wasm32-unknown-unknown` | `wasm` |

### Feature isolation

- **`std`** — enabled by default. Pulls in `clap`, `reqwest`, `aes-gcm`,
  `argon2`, `rpassword`. Gates the binary entry point (`main.rs`) and all
  HTTP/filesystem modules.
- **`wasm`** — produces a `cdylib` Soroban contract. All host-only modules
  are conditionally compiled out via `#[cfg(feature = "std")]` guards. The
  WASM build uses `--no-default-features` to exclude `std` entirely.
- **`mock-only`** — provides pre-built response fixtures for tests without
  requiring a live anchor. Safe to combine with `std`. Never enabled in
  production.
- **`stress-tests`** — gates high-concurrency load tests excluded from normal
  CI.

### Shared code

All environment-agnostic logic lives in `src/lib.rs` and modules reachable
from it. The `src/contract.rs` Soroban entry point re-exports the public API
surface and is available in both modes (the Soroban SDK compiles on any target).

### Build optimisation

The release profile (`Cargo.toml`) is tuned for WASM:
- `opt-level = "z"` — minimise binary size
- `lto = true`, `codegen-units = 1` — maximise inlining
- `panic = "abort"` — no unwinding tables
- `overflow-checks = true` — safety on-chain
- `strip = "symbols"` — remove debug info

The native binary inherits these optimisations, which is acceptable for a
CLI tool.

### Reproducible builds

WASM builds pin `SOURCE_DATE_EPOCH` and use deterministic compiler flags so
that the same commit always produces byte-identical `.wasm` output. The
`Makefile` target `verify-reproducible` checks this in CI.

## Consequences

**Positive:**
- Single source of truth for shared business logic — no duplication between
  contract and CLI.
- Feature-flag matrix CI validates all combinations (`feature-matrix.yml`).
- WASM size is tightly controlled (350 KB target).
- Reproducible builds give deployers confidence in the artifact.

**Negative:**
- Developers must remember `--no-default-features --features wasm` when
  building for Soroban — the `Makefile` and CI scripts abstract this.
- Feature-gate `#[cfg]` boilerplate in files that bridge the two modes.
- The release profile is WASM-centric; native builds get aggressive LTO and
  stripping that slow compilation.

**Alternatives considered:**
- **Separate crates** — would eliminate `#[cfg]` noise but introduce
  dependency duplication and a harder release coordination problem. Rejected
  because the shared surface is large.
- **Workspace with two members** — cleaner separation but adds workspace
  overhead and complicates cross-crate refactoring. Could be revisited if
  the shared surface shrinks.
