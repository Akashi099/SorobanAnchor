# SorobanAnchor

A Soroban smart contract SDK for Stellar anchors. Handles attestation management, SEP-6 deposit/withdrawal flows, SEP-10 JWT authentication, anchor routing, rate limiting, and transaction state tracking — all in a `no_std` Rust library compiled to WASM.

## What it does

- Registers and revokes attestors with SEP-10 JWT verification
- Submits and retrieves on-chain attestations with replay attack protection
- Normalizes SEP-6 deposit, withdrawal, and transaction status responses across anchors
- Verifies SEP-10 EdDSA JWTs on-chain using stored Ed25519 public keys
- Routes requests across multiple anchors and asset pairs by reputation, fees, and settlement time
- Caches anchor metadata and stellar.toml capabilities with TTL-based expiry
- Tracks transaction state transitions with full audit logging
- Propagates request IDs and tracing spans across operations
- Enforces rate limits and configurable retry/backoff strategies
- Validates anchor domain endpoints and response schemas

## Project structure

```
src/                        # Core library
  lib.rs                    # Public API surface
  contract.rs               # Soroban contract (attestations, sessions, quotes, routing)
  sep6.rs                   # SEP-6 deposit/withdrawal normalization
  sep10_jwt.rs              # SEP-10 JWT verification (EdDSA, no_std)
  domain_validator.rs       # Anchor domain/endpoint validation
  errors.rs                 # Stable error codes
  rate_limiter.rs           # Rate limiting
  response_validator.rs     # Response schema validation
  retry.rs                  # Retry with exponential backoff
  transaction_state_tracker.rs
  multi_asset_routing.rs    # Multi-asset quote routing across corridors
  deterministic_hash.rs     # Canonical SHA-256 payload hashing

tests/                      # Integration and unit tests
configs/                    # Example anchor configurations (JSON + TOML)
examples/                   # Rust and shell usage examples
scripts/                    # Build, validation, and deploy scripts
docs/                       # Feature and guide documentation
test_snapshots/             # Snapshot fixtures for deterministic tests
```

## Building

```bash
cargo build --release
```

For WASM output (Soroban deployment):

```bash
cargo build --release --target wasm32-unknown-unknown --no-default-features --features wasm
```

### Build Matrix

SorobanAnchor supports two distinct build environments with complete feature separation:

| Configuration | Command | Target | Output | CLI |
|---|---|---|---|---|
| **Native (default)** | `cargo build --release` | `x86_64-unknown-linux-gnu` | `target/release/anchorkit` | ✓ Yes |
| **WASM/Soroban** | `cargo build --release --target wasm32-unknown-unknown --no-default-features --features wasm` | `wasm32-unknown-unknown` | `target/wasm32-unknown-unknown/release/anchorkit.wasm` | ✗ No |

**Key differences:**
- **Native**: Includes CLI, HTTP client (`reqwest`), filesystem access, and credential storage
- **WASM**: Minimal runtime, no std library, only smart contract code for Soroban

Both builds are verified by the automated test suite:

```bash
./scripts/test_build_matrix.sh
```

For detailed information about build paths, features, and environment separation, see [docs/build-matrix.md](docs/build-matrix.md).

## Testing

```bash
cargo test
```

Run the stress-test suite (excluded from normal CI):

```bash
cargo test --features stress-tests
```

## Feature flags

The crate uses four feature flags to control which modules are compiled.

| Flag | Default | Purpose |
|------|---------|---------|
| `std` | ✓ | Enables filesystem-based config loading (`load_runtime_config_file`, `RuntimeConfig`). Disable for pure no_std environments. |
| `wasm` | — | Soroban on-chain deployment target. Excludes all HTTP/host modules (`sep6`, `sep24`, `sep38`, `webhook`, `streaming_monitor`); only the contract, error types, rate limiter, and cryptographic utilities are compiled. |
| `mock-only` | — | Enables the `mock` module with pre-built valid fixtures for every response type. Use in integration tests and CI pipelines that have no live anchor. |
| `stress-tests` | — | Enables `tests/load_simulation_tests.rs` — high-concurrency and throughput tests excluded from normal CI. |

### Build variants

```bash
# Native development (default features)
cargo build

# Soroban on-chain WASM deployment
cargo build --release \
  --target wasm32-unknown-unknown \
  --no-default-features --features wasm

# Testing with mock fixtures (no live anchor)
cargo test --features mock-only

# Testing with mock fixtures and config (std + mock)
cargo test --features std,mock-only

# Full suite including stress tests
cargo test --features std,mock-only,stress-tests

# Library only, no std (no_std verification)
cargo check --no-default-features
```

### Using mock fixtures

```rust
use anchorkit::mock::{mock_deposit_response, mock_firm_quote};
use anchorkit::{initiate_deposit, sep38::request_firm_quote};

// Test the deposit parsing pipeline without a live anchor
let raw = mock_deposit_response();
let deposit = initiate_deposit(raw).unwrap();
assert_eq!(deposit.transaction_id, "mock-txn-001");

// Test SEP-38 quote parsing
let raw_quote = mock_firm_quote();
let quote = request_firm_quote(raw_quote, 1_700_000_000).unwrap();
assert!(!quote.id.is_empty());
```

## CLI

```bash
# Deploy to testnet
anchorkit deploy --network testnet

# Register an attestor
anchorkit register --address GANCHOR123... --services deposits,withdrawals,kyc

# Submit an attestation
anchorkit attest --subject GUSER123... --payload-hash abc123...

# Check environment setup
anchorkit doctor
```

## Supported SEP Versions

The contract explicitly exposes which Stellar Ecosystem Proposals (SEPs) it supports via two on-chain methods:

```rust
// Returns [6, 10, 24, 31, 38]
let seps: Vec<u32> = AnchorKitContract::supported_seps(env);

// Returns per-SEP boolean flags
let flags: SepFeatureFlags = AnchorKitContract::supported_sep_feature_flags(env);
assert!(flags.sep10); // SEP-10 JWT authentication is supported
```

| SEP | Description | Supported |
|-----|-------------|-----------|
| [SEP-6](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0006.md) | Non-interactive deposit and withdrawal | ✓ |
| [SEP-10](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0010.md) | Stellar Web Authentication (JWT) | ✓ |
| [SEP-24](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0024.md) | Interactive deposit and withdrawal | ✓ |
| [SEP-31](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0031.md) | Direct payment | ✓ |
| [SEP-38](https://github.com/stellar/stellar-protocol/blob/master/ecosystem/sep-0038.md) | Anchor RFQ / firm quotes | ✓ |

Constants are also exported from the `contract` module: `SEP_6`, `SEP_10`, `SEP_24`, `SEP_31`, `SEP_38`.

## Key APIs

```rust
// SEP-6: normalize a raw anchor deposit response
let response = initiate_deposit(raw)?;

// SEP-10: verify an anchor JWT on-chain
contract.verify_sep10_token(token, issuer);

// Submit an attestation (replay-protected)
let id = contract.submit_attestation(issuer, subject, timestamp, payload_hash, sig);

// Route across anchors by lowest fee
let best = contract.route(options);

// Route across multiple asset pairs (multi-asset, #656)
use anchorkit::multi_asset_routing::{route_multi_asset, AssetPairRequest};
let requests = vec![
    AssetPairRequest { base_asset: "XLM".into(), quote_asset: "USDC".into(), amount: 1000, strategy: "LowestFee".into(), min_reputation: 70 },
];
let result = route_multi_asset(&requests, &all_quotes, now_timestamp)?;

// Track transaction state
tracker.transition(tx_id, TransactionStatus::Completed);
```

## Configuration

Anchor configs live in `configs/` as JSON or TOML. Validate them with:

```bash
./scripts/validate_all.sh
```

Schema reference: `config_schema.json`

## Integration Testing

The repository ships a CLI integration test harness that exercises the full
deploy → initialize → register → attest → verify workflow using the Soroban
local simulation environment (no network required by default).

```bash
# Run all integration harness tests (local simulation)
cargo test --test cli_integration_harness

# Or via Make
make integration-test
```

The harness covers:

| Step | What is tested |
|------|---------------|
| 1 | Contract deployment and admin initialization |
| 2 | Attestor registration (SEP-10 JWT flow) |
| 3 | Service capability configuration |
| 4 | Attestation submission and retrieval |
| 5 | Session-based workflow with audit logging |
| 6 | Quote submission and LowestFee routing |
| 7 | Attestor revocation and cleanup |
| E2E | Full pipeline in a single test |
| 9 | KYC submit → approve workflow |
| 10 | CLI binary smoke tests (`doctor`, `deploy --dry-run`) |
| 11 | Live testnet smoke test (opt-in) |

### Live testnet tests

Set the following environment variables to run the live testnet step:

```bash
export SOROBAN_ANCHOR_INTEGRATION=testnet
export ANCHOR_CONTRACT_ID=<deployed-contract-id>
export ANCHOR_ADMIN_SECRET=<admin-secret-key>

make integration-test-live
```

## Release Packaging

Production releases are built and bundled with a single Make target:

```bash
make release
```

This runs `scripts/package_release.sh` which:

1. Ensures the `wasm32-unknown-unknown` Rust target is installed.
2. Builds the native CLI binary (`target/release/anchorkit`).
3. Builds the optimized WASM contract (`target/wasm32-unknown-unknown/release/anchorkit.wasm`).
4. Runs `wasm-opt -Oz` if binaryen is available.
5. Assembles a bundle directory under `dist/anchorkit-<VERSION>/` containing:
   - `anchorkit` — CLI binary
   - `anchorkit.wasm` — Soroban WASM contract
   - `schemas/config_schema.json` — JSON schema for anchor configs
   - `configs/` — Example anchor configurations (JSON + TOML)
   - `docs/` — Documentation
   - `README.md`, `LICENSE`, `VERSION`
6. Creates `dist/anchorkit-<VERSION>.tar.gz`.
7. Generates a SHA-256 checksum file.

### Validating the bundle

```bash
make release-validate
# or directly:
./scripts/validate_bundle.sh dist/anchorkit-0.1.0.tar.gz
```

The validation script checks that all required artifacts are present and that
JSON files are well-formed.

### Signing and verifying

Release artifacts can be signed with GPG or minisign and verified before use:

```bash
# Sign (GPG by default)
make release-sign

# Sign in dry-run mode (prints commands without executing)
make release-sign-dry-run

# Verify a downloaded release bundle
make release-verify TARBALL=dist/anchorkit-0.1.0.tar.gz
# or directly:
./scripts/verify_release.sh dist/anchorkit-0.1.0.tar.gz
```

The verify script checks the SHA-256 checksum, the detached signature, and the
bundle contents.  See [`docs/release-signing.md`](docs/release-signing.md) for
full key-management and CI integration guidance.

### Cleaning up

```bash
make clean-dist   # removes dist/
```

## Governance and Security

SorobanAnchor follows a documented governance and security model covering:

- **Roles** — Maintainers, Contributors, Security Reviewers, and on-chain Attestors.
- **Contract upgrades** — Require two maintainer approvals, a reproducible WASM build, and a published SHA-256 checksum. Only the admin address recorded at contract initialization may authorize upgrades.
- **Admin key management** — Multi-signature setup (2-of-N); keys are never committed to the repository; mainnet keys are stored on offline hardware wallets.
- **Dependency auditing** — `cargo audit` runs in CI on every PR; all dependencies are pinned to exact versions and `Cargo.lock` is committed.
- **Responsible disclosure** — Report vulnerabilities privately via GitHub's security advisory feature. We follow coordinated disclosure with a 14-day fix window.

Full details: [`docs/governance-and-security.md`](docs/governance-and-security.md)

## Upgrading and Migration

When deploying a new contract version to production, follow the step-by-step
upgrade playbook which covers prerequisites, WASM upload, schema migration,
post-upgrade verification, and a full rollback procedure.

- **Migration guide** — schema versioning, data preservation, and example migration code: [`docs/migration-guide.md`](docs/migration-guide.md)
- **Upgrade playbook** — complete production runbook (pre-flight, upgrade, verify, rollback): [`docs/upgrade-playbook.md`](docs/upgrade-playbook.md)

## Coverage Thresholds

CI enforces minimum line-coverage percentages for the critical modules and
reports regressions relative to the `main` branch baseline.

| Module | Threshold |
|--------|-----------|
| `contract.rs` | ≥ 85 % |
| `rate_limiter.rs` | ≥ 90 % |
| `retry.rs` | ≥ 90 % |
| `transaction_state_tracker.rs` | ≥ 85 % |

Run locally:

```bash
./scripts/coverage_with_thresholds.sh

# Compare against a saved baseline and flag regressions > 2 %:
./scripts/coverage_with_thresholds.sh \
    --baseline /tmp/baseline.json \
    --report-delta
```

Full details: [`docs/coverage-metrics.md`](docs/coverage-metrics.md)

## Production Release Status

SorobanAnchor v0.1.0 is **production-ready**. The following verification and cleanup have been completed for this release:

### Build Matrix
- **Native (std):** `cargo build --release` — clean build, CLI binary produced
- **WASM (no_std):** `cargo build --release --target wasm32-unknown-unknown --no-default-features --features wasm` — clean build, optimized WASM produced
- **no_std verification:** `cargo check --no-default-features` passes

### Test Coverage
| Area | Status |
|------|--------|
| Feature gate combinations | All 8 documented combinations verified |
| Integration harness (local) | `cargo test --test cli_integration_harness` passes |
| Stress tests | `cargo test --features stress-tests` passes |
| Config validation | `./scripts/validate_all.sh` against all 6 example configs |
| Build matrix | `./scripts/test_build_matrix.sh` passes |

### Documentation & Cleanup
- All docs reviewed and updated for API accuracy
- Stale example scripts removed (`logging_demo.sh`, `anchor_routing_example.sh`)
- Error code references synced between `src/errors.rs`, `docs/error-codes.md`, and `docs/CONTRACT_FUNCTIONS.md`
- Build target docs aligned with actual `Makefile` targets
- Script path resolution bugs fixed in `pre_deploy_validate.sh`, `pre_deploy_validate.ps1`, `validate_all.sh`, `validate_all.ps1`
- Dead code removed from `ci_preflight_check.sh`, `verify_anchor_info_discovery.sh`
- `generate_hash_vectors.sh` fixed (function declaration ordering, reliable openssl fallback)
- Coverage script test file references corrected

### Release Artifacts
```bash
make release            # Build and bundle release artifacts
make release-validate   # Validate the release bundle
```

The release bundle includes:
- `anchorkit` — Native CLI binary
- `anchorkit.wasm` — Soroban WASM contract (optimized)
- `schemas/config_schema.json` — JSON schema
- `configs/` — Example anchor configurations
- `docs/` — Full documentation set
- `README.md`, `LICENSE`, `VERSION`

## License

MIT
