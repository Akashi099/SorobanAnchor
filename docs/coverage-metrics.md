# Test Coverage Metrics

This document describes the test coverage strategy, enforced thresholds, and
regression-reporting workflow for SorobanAnchor's critical modules.

## Overview

Production readiness requires visibility into which code paths are exercised by
tests.  SorobanAnchor maintains **enforced coverage thresholds** for the most
critical modules.  Any PR that drops a module below its threshold fails CI
automatically so regressions are caught before merge.

## Coverage Thresholds

| Module | File | Threshold | Rationale |
|--------|------|-----------|-----------|
| Contract core | `src/contract.rs` | **≥ 85 %** | Core contract logic, admin functions, attestations |
| Rate limiter | `src/rate_limiter.rs` | **≥ 90 %** | Security-critical rate-limiting enforcement |
| Retry | `src/retry.rs` | **≥ 90 %** | Reliability-critical retry and backoff logic |
| Transaction state tracker | `src/transaction_state_tracker.rs` | **≥ 85 %** | State management and audit trail |

> **Hard failure rule**: CI fails if any critical module falls below its
> threshold.  A drop of more than **2 percentage points** relative to the
> `main` branch baseline is also flagged as a regression even when the
> absolute threshold is still met.

## Generating Coverage Reports

### Prerequisites

Install `cargo-tarpaulin`:

```bash
cargo install cargo-tarpaulin --locked
```

### Run the threshold-enforcing script (recommended)

```bash
./scripts/coverage_with_thresholds.sh
```

This script:
1. Runs `cargo tarpaulin` with HTML and JSON output.
2. Parses per-module percentages from the JSON report.
3. Compares each module against its threshold and exits non-zero on violations.
4. Writes `coverage/summary.json` — a compact map of `module → coverage%`.

### Delta / regression reporting

To compare the current run against a saved baseline:

```bash
# Save a baseline (e.g. from the main branch):
./scripts/coverage_with_thresholds.sh --output-dir coverage
cp coverage/summary.json /tmp/baseline.json

# On a feature branch — report delta and flag regressions > 2 %:
./scripts/coverage_with_thresholds.sh \
    --output-dir coverage \
    --baseline /tmp/baseline.json \
    --report-delta
```

The script prints a per-module delta table and exits non-zero if any module
regressed by more than 2 percentage points.

### Manual coverage run (no threshold enforcement)

```bash
cargo tarpaulin --out Html --output-dir coverage --exclude-files tests/*
```

## CI Integration

Coverage is enforced in the **`coverage` job** inside `.github/workflows/ci.yml`.

| CI step | What it does |
|---------|-------------|
| Install `cargo-tarpaulin` | Ensures the tool is available |
| Download baseline artifact | Fetches `coverage-summary-main` from the most recent passing `main` run |
| Run `coverage_with_thresholds.sh` | Enforces thresholds; computes delta when a baseline is available |
| Upload HTML report | Always uploaded as `coverage-report` artifact (14-day retention) |
| Upload summary (main only) | Stores `summary.json` as `coverage-summary-main` for the next PR's delta run |

The `release-package` job depends on `coverage` passing, so releases are
blocked when thresholds are violated.

### Reading CI results

**Threshold violation** — The `Run coverage with threshold enforcement` step
will print lines like:

```
  ✗  contract.rs : 82%  (threshold: 85%) — BELOW THRESHOLD
```

and the step exits with code 1.

**Regression warning** — When a baseline is available, a drop > 2 % prints:

```
REGRESSIONS DETECTED (> 2% drop):
  !! retry: 91% → 88% (−3%)
```

Both conditions fail the CI job and block the PR from merging.

## Coverage by Module

### `contract.rs`

**Critical paths to cover:**
- Contract initialization with admin setup
- Attestor registration and revocation
- Attestation submission with replay protection
- Session creation and management
- Quote submission and retrieval (single and multi-asset)
- Routing logic with fee / reputation scoring
- Audit log recording
- Configuration changes

**Test files:**
- `tests/cli_integration_harness.rs` — end-to-end workflows
- `tests/admin_permission_tests.rs` — admin operations
- `tests/attestation_sig_tests.rs` — attestation logic
- `tests/session_tests.rs` — session management
- `tests/routing_tests.rs` — routing logic
- `tests/multi_asset_routing_tests.rs` — multi-asset quote routing (#656)

### `rate_limiter.rs`

**Critical paths to cover:**
- Rate limit window calculation
- Submission count tracking
- Throttling enforcement
- Window expiration and reset
- Health check reporting

**Test files:**
- `tests/load_simulation_tests.rs` — high-concurrency scenarios
- `tests/health_check_tests.rs` — health reporting

### `retry.rs`

**Critical paths to cover:**
- Exponential backoff calculation
- Retry attempt counting
- Timeout enforcement
- Error classification for retry eligibility
- Max retry limit enforcement

**Test files:**
- `tests/cross_platform_tests.rs` — retry behaviour across platforms
- `tests/load_simulation_tests.rs` — retry under load

### `transaction_state_tracker.rs`

**Critical paths to cover:**
- State transition validation
- Audit trail recording
- Recovery logic
- State persistence
- Timestamp tracking

**Test files:**
- `tests/transaction_state_tracker_tests.rs` — state transitions
- `tests/ledger_boundary_tests.rs` — boundary conditions
- `tests/cli_integration_harness.rs` — end-to-end state tracking

## Improving Coverage

### Adding tests

When coverage falls below a threshold:

1. Identify uncovered lines in the HTML report (`coverage/index.html`).
2. Determine whether the code path is critical or purely defensive.
3. Add targeted tests to exercise the path.
4. Re-run `./scripts/coverage_with_thresholds.sh` to verify improvement.

### Common coverage gaps

| Gap type | Remedy |
|----------|--------|
| Error paths | Add tests that trigger error conditions with invalid inputs |
| Edge cases | Test boundary values and state transitions explicitly |
| Feature gates | Ensure all feature-flag combinations are tested |
| Multi-asset flows | Cover new asset-pair routing combinations (#656) |

## Threshold Review

Coverage thresholds are reviewed **quarterly** by maintainers:

1. Assess whether targets are realistic and achievable.
2. Identify modules that consistently exceed targets (consider raising them).
3. Identify modules that struggle to meet targets (investigate root cause).
4. Adjust targets based on risk assessment and module criticality.

Any change to thresholds must be accompanied by a PR updating both this
document and the `THRESHOLDS` map in `scripts/coverage_with_thresholds.sh`.

## References

- [Tarpaulin documentation](https://github.com/xd009642/tarpaulin)
- [Coverage threshold script](../scripts/coverage_with_thresholds.sh)
- [CI workflow](../.github/workflows/ci.yml)
- [Code quality documentation](CODE_QUALITY.md)
