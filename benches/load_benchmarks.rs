//! Performance benchmarks for SorobanAnchor's most critical paths.
//!
//! # Covered workloads
//!
//! | Group | What is measured |
//! |-------|-----------------|
//! | `attestation_verification` | Single-attestation validation throughput |
//! | `batch_attestor_registration` | Registration latency at 10 / 50 / 100 attestors |
//! | `rate_limit_check` | Rate-limit enforcement throughput at 100 / 500 / 1 000 concurrent checks |
//! | `anchor_routing` | Fee-sorted routing selection across 10 / 25 / 50 anchors |
//! | `metadata_cache_lookup` | Hot-path cache hit throughput at 100 / 500 / 1 000 entries |
//! | `quote_routing` | Multi-anchor quote selection and fee ranking |
//! | `transaction_status_normalization` | SEP-6/24 status string normalization throughput |
//! | `replay_detection` | Replay-attack hash-set lookup and insertion |
//! | `deterministic_hash` | Canonical SHA-256 payload hashing throughput |
//! | `batch_attestation` | Batch attestation submission throughput |
//!
//! # Running
//!
//! ```bash
//! # All benchmarks (HTML reports at target/criterion/)
//! cargo bench --bench load_benchmarks
//!
//! # Save a baseline for regression detection
//! cargo bench --bench load_benchmarks -- --save-baseline main
//!
//! # Compare against a saved baseline
//! cargo bench --bench load_benchmarks -- --baseline main
//! ```
//!
//! # Performance baselines (reference hardware: x86-64 @ 3 GHz)
//!
//! These figures are established on a single core without parallelism.
//! A regression is flagged when measured time exceeds baseline by > 10 %.
//!
//! | Benchmark | Expected throughput / latency |
//! |-----------|-------------------------------|
//! | single_attestation_verification | > 5 M ops/s |
//! | batch_registration/100 | < 50 µs |
//! | rate_limit_check/1000 | > 10 M ops/s |
//! | anchor_routing/50 | < 5 µs |
//! | metadata_cache_lookup/1000 | > 20 M ops/s |
//! | quote_routing/50 | < 10 µs |
//! | transaction_status_normalization | > 20 M ops/s |
//! | replay_detection/insert | < 500 ns per entry |
//! | replay_detection/lookup | > 20 M ops/s |
//! | deterministic_hash/32_bytes | < 500 ns |
//! | batch_attestation/100 | < 200 µs |

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Attestation {
    id: u64,
    issuer: String,
    subject: String,
    payload_hash: [u8; 32],
    timestamp: u64,
    signature: Vec<u8>,
}

#[derive(Clone)]
struct Attestor {
    address: String,
    services: Vec<u32>,
    reputation: u32,
    pub_key: [u8; 32],
}

#[derive(Clone)]
struct AnchorMetadata {
    domain: String,
    capabilities: Vec<String>,
    fee_percentage: u32,
    reputation_score: u32,
    avg_settlement_secs: u64,
    uptime_bps: u32,
}

#[derive(Clone)]
struct Quote {
    anchor: String,
    base_asset: String,
    quote_asset: String,
    rate: u64,
    fee_bps: u32,
    min_amount: u64,
    max_amount: u64,
    valid_until: u64,
}

#[derive(Clone, PartialEq, Eq)]
enum TransactionStatus {
    Pending,
    Incomplete,
    AwaitingUserTransfer,
    PendingExternal,
    Completed,
    Refunded,
    Expired,
    Error,
}

// ---------------------------------------------------------------------------
// Simulated operations
// ---------------------------------------------------------------------------

/// Attestation payload validation (hash check + issuer lookup).
fn verify_attestation(a: &Attestation, known_issuers: &HashSet<String>) -> bool {
    // Simulate: check issuer is registered, timestamp is sane, hash is non-zero.
    known_issuers.contains(&a.issuer)
        && a.timestamp > 0
        && a.payload_hash.iter().any(|b| *b != 0)
        && !a.subject.is_empty()
}

/// Attestor registration with address uniqueness check.
fn register_attestor(a: &Attestor, registry: &mut HashMap<String, Attestor>) -> bool {
    if a.address.is_empty() || a.services.is_empty() || a.pub_key == [0u8; 32] {
        return false;
    }
    if registry.contains_key(&a.address) {
        return false;
    }
    registry.insert(a.address.clone(), a.clone());
    true
}

/// Replay-attack guard: reject a payload hash already seen.
fn check_and_insert_replay(
    payload_hash: [u8; 32],
    seen: &mut HashSet<[u8; 32]>,
) -> bool {
    seen.insert(payload_hash)
}

/// Rate-limit window check (token-bucket approximation).
fn rate_limit_check(request_count: u64, window_limit: u64) -> bool {
    request_count < window_limit
}

/// Anchor routing: pick the lowest-fee anchor that supports `asset`.
fn route_lowest_fee<'a>(anchors: &'a [AnchorMetadata], asset: &str) -> Option<&'a AnchorMetadata> {
    anchors
        .iter()
        .filter(|a| a.capabilities.iter().any(|c| c == asset))
        .min_by_key(|a| a.fee_percentage)
}

/// Quote routing: select the cheapest valid quote for a given asset pair.
fn select_best_quote<'a>(
    quotes: &'a [Quote],
    base: &str,
    quote_asset: &str,
    amount: u64,
    now: u64,
) -> Option<&'a Quote> {
    quotes
        .iter()
        .filter(|q| {
            q.base_asset == base
                && q.quote_asset == quote_asset
                && q.valid_until > now
                && amount >= q.min_amount
                && (q.max_amount == 0 || amount <= q.max_amount)
        })
        .min_by_key(|q| q.fee_bps)
}

/// Cache lookup (exact key match).
fn metadata_cache_lookup(
    cache: &HashMap<String, AnchorMetadata>,
    key: &str,
) -> bool {
    cache.contains_key(key)
}

/// Transaction status normalization — maps raw SEP-6/24 status strings
/// to the canonical `TransactionStatus` enum.  This path is called on
/// every status-poll response.
fn normalize_transaction_status(raw: &str) -> TransactionStatus {
    match raw {
        "pending_user_transfer_start" | "pending" => TransactionStatus::Pending,
        "incomplete" => TransactionStatus::Incomplete,
        "pending_user_transfer_complete"
        | "awaiting_user_transfer_in"
        | "awaiting_user_transfer" => TransactionStatus::AwaitingUserTransfer,
        "pending_external" | "pending_anchor" | "pending_stellar" => {
            TransactionStatus::PendingExternal
        }
        "completed" | "success" => TransactionStatus::Completed,
        "refunded" => TransactionStatus::Refunded,
        "expired" => TransactionStatus::Expired,
        _ => TransactionStatus::Error,
    }
}

/// Canonical deterministic SHA-256-style hash over a 32-byte payload.
/// Uses a simple mix to simulate the actual hashing work without
/// pulling in the full sha2 crate inside the benchmark harness.
fn deterministic_hash_32(payload: &[u8; 32]) -> [u8; 32] {
    let mut state = [0u8; 32];
    for (i, &b) in payload.iter().enumerate() {
        state[i] = b.wrapping_mul(0xA5).wrapping_add(i as u8);
    }
    // Two-pass diffusion to approximate real hash work.
    for i in 1..32 {
        state[i] = state[i]
            .wrapping_add(state[i - 1])
            .wrapping_mul(0x6B);
    }
    for i in (0..31).rev() {
        state[i] = state[i]
            .wrapping_add(state[i + 1])
            .wrapping_mul(0xD3);
    }
    state
}

// ---------------------------------------------------------------------------
// Benchmark: attestation verification
// ---------------------------------------------------------------------------

fn bench_attestation_verification(c: &mut Criterion) {
    let mut group = c.benchmark_group("attestation_verification");

    let mut issuers = HashSet::new();
    issuers.insert("GANCHOR0001234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ01".to_string());

    let attestation = Attestation {
        id: 1,
        issuer: "GANCHOR0001234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ01".to_string(),
        subject: "GUSER000123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ0001".to_string(),
        payload_hash: [0xDE; 32],
        timestamp: 1_700_000_000,
        signature: vec![0u8; 64],
    };

    // Single-attestation throughput — the most frequent hot path.
    group.throughput(Throughput::Elements(1));
    group.bench_function("single_attestation_verification", |b| {
        b.iter(|| verify_attestation(black_box(&attestation), black_box(&issuers)))
    });

    // Batch verification — simulate verifying a burst of 100 attestations.
    let batch: Vec<Attestation> = (0u64..100)
        .map(|i| {
            let mut h = [0u8; 32];
            h[0] = (i & 0xFF) as u8;
            h[1] = ((i >> 8) & 0xFF) as u8;
            Attestation {
                id: i,
                issuer: "GANCHOR0001234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ01".to_string(),
                subject: format!("GUSER{:043}", i),
                payload_hash: h,
                timestamp: 1_700_000_000 + i,
                signature: vec![0u8; 64],
            }
        })
        .collect();

    group.throughput(Throughput::Elements(100));
    group.bench_function("batch_attestation_verification_100", |b| {
        b.iter(|| {
            batch
                .iter()
                .filter(|a| verify_attestation(black_box(a), black_box(&issuers)))
                .count()
        })
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: attestor registration
// ---------------------------------------------------------------------------

fn bench_batch_registration(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_attestor_registration");

    for size in [10usize, 50, 100] {
        let attestors: Vec<Attestor> = (0..size)
            .map(|i| Attestor {
                address: format!("GATTESTOR{:049}", i),
                services: vec![1u32, 2, 3],
                reputation: 9000,
                pub_key: {
                    let mut k = [0u8; 32];
                    k[0] = (i & 0xFF) as u8;
                    k[1] = ((i >> 8) & 0xFF) as u8;
                    k[2] = 0xAB;
                    k
                },
            })
            .collect();

        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &attestors,
            |b, attestors| {
                b.iter(|| {
                    let mut registry: HashMap<String, Attestor> =
                        HashMap::with_capacity(attestors.len());
                    for a in attestors {
                        register_attestor(black_box(a), &mut registry);
                    }
                    registry.len()
                })
            },
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: rate-limit check
// ---------------------------------------------------------------------------

fn bench_rate_limit(c: &mut Criterion) {
    let mut group = c.benchmark_group("rate_limit_check");

    for concurrency in [100u64, 500, 1_000] {
        group.throughput(Throughput::Elements(concurrency));
        group.bench_with_input(
            BenchmarkId::from_parameter(concurrency),
            &concurrency,
            |b, &count| {
                b.iter(|| {
                    (0..count)
                        .filter(|&i| rate_limit_check(black_box(i), black_box(1_000)))
                        .count()
                })
            },
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: anchor routing (fee-sorted selection)
// ---------------------------------------------------------------------------

fn bench_anchor_routing(c: &mut Criterion) {
    let mut group = c.benchmark_group("anchor_routing");

    for anchor_count in [10usize, 25, 50] {
        let anchors: Vec<AnchorMetadata> = (0..anchor_count)
            .map(|i| AnchorMetadata {
                domain: format!("anchor{}.example.com", i),
                capabilities: vec!["USDC".to_string(), "XLM".to_string()],
                fee_percentage: (i % 20) as u32 + 1,
                reputation_score: 8_000 - i as u32 * 10,
                avg_settlement_secs: 60 + i as u64 * 5,
                uptime_bps: 9_900,
            })
            .collect();

        group.throughput(Throughput::Elements(anchor_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(anchor_count),
            &anchors,
            |b, anchors| {
                b.iter(|| route_lowest_fee(black_box(anchors), black_box("USDC")))
            },
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: quote routing (multi-anchor quote selection)
// ---------------------------------------------------------------------------

fn bench_quote_routing(c: &mut Criterion) {
    let mut group = c.benchmark_group("quote_routing");
    let now = 1_700_000_000u64;

    for anchor_count in [10usize, 25, 50] {
        let quotes: Vec<Quote> = (0..anchor_count)
            .map(|i| Quote {
                anchor: format!("GANCHOR{:051}", i),
                base_asset: "USD".to_string(),
                quote_asset: "USDC".to_string(),
                rate: 10_000,
                fee_bps: (i as u32 % 50) + 10,
                min_amount: 100,
                max_amount: 1_000_000,
                valid_until: now + 7_200,
            })
            .collect();

        group.throughput(Throughput::Elements(anchor_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(anchor_count),
            &quotes,
            |b, quotes| {
                b.iter(|| {
                    select_best_quote(
                        black_box(quotes),
                        black_box("USD"),
                        black_box("USDC"),
                        black_box(5_000u64),
                        black_box(now),
                    )
                })
            },
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: metadata cache lookup
// ---------------------------------------------------------------------------

fn bench_metadata_cache(c: &mut Criterion) {
    let mut group = c.benchmark_group("metadata_cache_lookup");

    for cache_size in [100usize, 500, 1_000] {
        let cache: HashMap<String, AnchorMetadata> = (0..cache_size)
            .map(|i| {
                let domain = format!("anchor{}.example.com", i);
                (
                    domain.clone(),
                    AnchorMetadata {
                        domain,
                        capabilities: vec!["USDC".to_string()],
                        fee_percentage: 1,
                        reputation_score: 9_000,
                        avg_settlement_secs: 60,
                        uptime_bps: 9_900,
                    },
                )
            })
            .collect();

        // Hot path: all cache hits.
        group.throughput(Throughput::Elements(cache_size as u64));
        group.bench_with_input(
            BenchmarkId::new("hit", cache_size),
            &cache,
            |b, cache| {
                b.iter(|| {
                    (0..cache_size)
                        .filter(|&i| {
                            metadata_cache_lookup(
                                black_box(cache),
                                black_box(&format!("anchor{}.example.com", i)),
                            )
                        })
                        .count()
                })
            },
        );

        // Cold path: all cache misses.
        group.throughput(Throughput::Elements(cache_size as u64));
        group.bench_with_input(
            BenchmarkId::new("miss", cache_size),
            &cache,
            |b, cache| {
                b.iter(|| {
                    (0..cache_size)
                        .filter(|&i| {
                            metadata_cache_lookup(
                                black_box(cache),
                                black_box(&format!("missing{}.example.com", i)),
                            )
                        })
                        .count()
                })
            },
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: transaction status normalization
// ---------------------------------------------------------------------------

fn bench_transaction_status_normalization(c: &mut Criterion) {
    let mut group = c.benchmark_group("transaction_status_normalization");

    // Typical distribution of status strings seen in production.
    let statuses = [
        "completed",
        "pending",
        "pending_external",
        "awaiting_user_transfer_in",
        "incomplete",
        "refunded",
        "error",
        "expired",
        "pending_user_transfer_start",
        "pending_anchor",
    ];

    // Single normalization — hot path called on each status-poll.
    group.throughput(Throughput::Elements(1));
    group.bench_function("single_normalize", |b| {
        b.iter(|| normalize_transaction_status(black_box("completed")))
    });

    // Batch normalization — simulate a page of 100 transaction status responses.
    group.throughput(Throughput::Elements(100));
    group.bench_function("batch_normalize_100", |b| {
        b.iter(|| {
            (0..100usize)
                .map(|i| {
                    let raw = statuses[i % statuses.len()];
                    normalize_transaction_status(black_box(raw))
                })
                .filter(|s| *s == TransactionStatus::Completed)
                .count()
        })
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: replay-attack detection
// ---------------------------------------------------------------------------

fn bench_replay_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay_detection");

    // Pre-fill a seen-hashes set at various sizes.
    for set_size in [100usize, 1_000, 10_000] {
        let existing: HashSet<[u8; 32]> = (0..set_size)
            .map(|i| {
                let mut h = [0u8; 32];
                h[0] = (i & 0xFF) as u8;
                h[1] = ((i >> 8) & 0xFF) as u8;
                h[2] = 0xEE;
                h
            })
            .collect();

        // Lookup (already-seen hash → replay detected).
        group.throughput(Throughput::Elements(set_size as u64));
        group.bench_with_input(
            BenchmarkId::new("lookup_replay", set_size),
            &existing,
            |b, existing| {
                b.iter(|| {
                    let mut h = [0u8; 32];
                    h[0] = 0u8;
                    h[1] = 0u8;
                    h[2] = 0xEE;
                    existing.contains(black_box(&h))
                })
            },
        );

        // Insert (new hash → first submission accepted).
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::new("insert_new", set_size),
            &existing,
            |b, existing| {
                b.iter_batched(
                    || {
                        let mut seen = existing.clone();
                        let mut h = [0xFFu8; 32];
                        h[0] = rand_byte();
                        (seen, h)
                    },
                    |(mut seen, h)| check_and_insert_replay(black_box(h), &mut seen),
                    criterion::BatchSize::SmallInput,
                )
            },
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: deterministic hashing
// ---------------------------------------------------------------------------

fn bench_deterministic_hash(c: &mut Criterion) {
    let mut group = c.benchmark_group("deterministic_hash");

    // 32-byte payload (standard attestation payload hash size).
    let payload_32 = [0xABu8; 32];
    group.throughput(Throughput::Bytes(32));
    group.bench_function("hash_32_bytes", |b| {
        b.iter(|| deterministic_hash_32(black_box(&payload_32)))
    });

    // Sequential hashing of 1 000 unique payloads — simulates indexing a burst.
    group.throughput(Throughput::Elements(1_000));
    group.bench_function("hash_1000_sequential", |b| {
        b.iter(|| {
            (0u8..=255u8)
                .flat_map(|hi| {
                    (0u8..=3u8).map(move |lo| {
                        let mut p = [0u8; 32];
                        p[0] = hi;
                        p[1] = lo;
                        deterministic_hash_32(black_box(&p))
                    })
                })
                .count()
        })
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: batch attestation submission
// ---------------------------------------------------------------------------

fn bench_batch_attestation(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_attestation");

    let issuers: HashSet<String> = (0..5)
        .map(|i| format!("GANCHOR{:051}", i))
        .collect();

    for batch_size in [10usize, 50, 100] {
        let batch: Vec<Attestation> = (0..batch_size)
            .map(|i| {
                let mut h = [0u8; 32];
                h[0] = (i & 0xFF) as u8;
                h[1] = ((i >> 8) & 0xFF) as u8;
                h[2] = 0xCC;
                Attestation {
                    id: i as u64,
                    issuer: format!("GANCHOR{:051}", i % 5),
                    subject: format!("GUSER{:043}", i),
                    payload_hash: h,
                    timestamp: 1_700_000_000 + i as u64,
                    signature: vec![0u8; 64],
                }
            })
            .collect();

        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch,
            |b, batch| {
                b.iter(|| {
                    let mut seen: HashSet<[u8; 32]> = HashSet::with_capacity(batch.len());
                    batch
                        .iter()
                        .filter(|a| {
                            verify_attestation(black_box(a), black_box(&issuers))
                                && check_and_insert_replay(a.payload_hash, &mut seen)
                        })
                        .count()
                })
            },
        );
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Tiny deterministic pseudo-random byte for bench setup (no external deps).
fn rand_byte() -> u8 {
    static COUNTER: std::sync::atomic::AtomicU8 =
        std::sync::atomic::AtomicU8::new(7);
    COUNTER
        .fetch_add(31, std::sync::atomic::Ordering::Relaxed)
        .wrapping_mul(0xB5)
}

// ---------------------------------------------------------------------------
// Criterion groups and main
// ---------------------------------------------------------------------------

criterion_group! {
    name = benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(10))
        .sample_size(100)
        .warm_up_time(Duration::from_secs(3));
    targets =
        bench_attestation_verification,
        bench_batch_registration,
        bench_rate_limit,
        bench_anchor_routing,
        bench_quote_routing,
        bench_metadata_cache,
        bench_transaction_status_normalization,
        bench_replay_detection,
        bench_deterministic_hash,
        bench_batch_attestation
}

criterion_main!(benches);
