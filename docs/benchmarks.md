# Performance Benchmarks

This document describes the benchmark strategy, covered workloads, expected
baselines, and regression-detection workflow for SorobanAnchor.

## Overview

SorobanAnchor maintains a Criterion-based benchmark suite (`benches/load_benchmarks.rs`)
that covers the most performance-sensitive paths in the contract and host layers.
Benchmarks run automatically on every push to `main` and results are stored as
GitHub Actions artifacts for trend analysis.

## Covered Benchmark Groups

| Group | What is measured | Hot path |
|-------|-----------------|----------|
| `attestation_verification` | Payload hash check + issuer lookup, single and batch | Every attestation submission |
| `batch_attestor_registration` | Uniqueness check + registry insertion at 10/50/100 attestors | Onboarding bursts |
| `rate_limit_check` | Token-bucket window enforcement at 100/500/1 000 concurrent calls | Every API call |
| `anchor_routing` | Fee-sorted anchor selection across 10/25/50 anchors | Every route call |
| `quote_routing` | Multi-anchor quote selection with expiry filtering at 10/25/50 quotes | Every settlement |
| `metadata_cache_lookup` | Hot-path cache hits and cold-path misses at 100/500/1 000 entries | Every metadata read |
| `transaction_status_normalization` | SEP-6/24 status string → `TransactionStatus` enum mapping | Every status poll |
| `replay_detection` | Hash-set lookup (replay) and insertion (new submission) | Every attestation |
| `deterministic_hash` | Canonical SHA-256-style 32-byte payload hashing | Every attestation hash |
| `batch_attestation` | Combined verify + replay-guard over 10/50/100 entries | Batch workflows |

## Performance Baselines

Baselines are measured on a single core (x86-64 @ ~3 GHz, Ubuntu 22.04) without
parallelism. A regression is flagged when the measured time exceeds the baseline
by more than **10 %**.

| Benchmark | Expected throughput / latency |
|-----------|-------------------------------|
| `attestation_verification/single_attestation_verification` | > 5 M ops/s |
| `attestation_verification/batch_attestation_verification_100` | > 3 M ops/s |
| `batch_attestor_registration/100` | < 50 µs |
| `rate_limit_check/1000` | > 10 M ops/s |
| `anchor_routing/50` | < 5 µs |
| `quote_routing/50` | < 10 µs |
| `metadata_cache_lookup/hit/1000` | > 20 M ops/s |
| `metadata_cache_lookup/miss/1000` | > 10 M ops/s |
| `transaction_status_normalization/single_normalize` | > 20 M ops/s |
| `transaction_status_normalization/batch_normalize_100` | > 15 M ops/s |
| `replay_detection/lookup_replay/10000` | > 20 M ops/s |
| `replay_detection/insert_new/10000` | < 500 ns per entry |
| `deterministic_hash/hash_32_bytes` | < 500 ns |
| `deterministic_hash/hash_1000_sequential` | < 1 ms |
| `batch_attestation/100` | < 200 µs |

## Running Benchmarks

```bash
# Run all benchmark groups (HTML reports at target/criterion/report/index.html)
cargo bench --bench load_benchmarks

# Save the current results as a named baseline
cargo bench --bench load_benchmarks -- --save-baseline main

# Compare future results against the saved baseline
cargo bench --bench load_benchmarks -- --baseline main

# Using the convenience script (documents all groups and baselines)
./scripts/run_benchmarks.sh
./scripts/run_benchmarks.sh --save main
./scripts/run_benchmarks.sh --compare main

# Using Make
make bench
make bench-save BASELINE=main
make bench-compare BASELINE=main
```

## Regression Detection

Criterion flags a regression when the confidence interval of the new measurement
does not overlap with the saved baseline. The CI workflow (`benchmarks` job in
`.github/workflows/ci.yml`) stores results as artifacts tagged with the commit
SHA, enabling:

1. **Post-merge comparison**: Download the artifact from a previous `main` push
   and compare it against the artifact from the current push.
2. **Manual comparison**: Save a local baseline before a change, apply the change,
   then run `--baseline` to see the delta.

### Interpreting Criterion output

- `change: [-2.5% -0.8% +1.1%]` — acceptable variance, no regression.
- `Performance has regressed. Old [X ns] New [Y ns]` — regression flagged.
- `Performance has improved. Old [X ns] New [Y ns]` — improvement detected.

## Adding New Benchmarks

When adding a new feature or changing a critical path:

1. Add a `fn bench_<group>(c: &mut Criterion)` function in `benches/load_benchmarks.rs`.
2. Add the function to the `criterion_group!` targets list.
3. Document the expected baseline in this file.
4. Save a baseline after the PR merges: `make bench-save BASELINE=main`.

### Guidelines

- Keep benchmark functions **pure** — no I/O, no random state, no side effects.
- Use `black_box()` on all inputs to prevent dead-code elimination.
- Size the `throughput()` annotation correctly (Elements, Bytes, or none).
- Use `BenchmarkId` for parameterised groups so Criterion tracks each size separately.

## CI Integration

The `benchmarks` job in `.github/workflows/ci.yml`:

- Runs on every push to `main` (not on PRs, to avoid inflating PR build times).
- Saves Criterion HTML reports and raw data as a GitHub Actions artifact with
  90-day retention under the name `benchmark-results-<sha>`.
- Does **not** fail the build on regression — this is intentional because
  CI hardware variance can trigger false positives. Developers should compare
  artifacts manually when investigating a suspected regression.

To enforce hard regression gates in the future, parse `target/criterion/*/estimates.json`
in a post-benchmark step and compare against stored baseline JSON files.
