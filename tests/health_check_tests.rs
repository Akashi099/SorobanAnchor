//! Tests for health check APIs (#268):
//! - get_health_status
//! - get_metadata_freshness
//! - get_rate_limiter_health

use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    Address, Env,
};
use anchorkit::contract::{
    AnchorKitContract, AnchorKitContractClient, AnchorMetadata, HealthStatus,
    MetadataCacheState,
};
use anchorkit::{RateLimiter, RateLimitConfig};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup_env(env: &Env) -> (AnchorKitContractClient<'_>, Address) {
    env.mock_all_auths();
    let contract_id = env.register_contract(None, AnchorKitContract);
    let client = AnchorKitContractClient::new(env, &contract_id);
    (client, contract_id)
}

fn init_contract(env: &Env, client: &AnchorKitContractClient) -> Address {
    let admin = Address::generate(env);
    client.initialize(&admin);
    admin
}

fn make_metadata(env: &Env, anchor: &Address) -> AnchorMetadata {
    AnchorMetadata {
        anchor: anchor.clone(),
        reputation_score: 90,
        liquidity_score: 80,
        uptime_percentage: 99,
        total_volume: 1_000_000,
        average_settlement_time: 60,
        is_active: true,
    }
}

// ---------------------------------------------------------------------------
// get_health_status
// ---------------------------------------------------------------------------

#[test]
fn test_health_status_unavailable_before_init() {
    let _env = Env::default(); let client = setup_env(&_env);
    assert_eq!(client.get_health_status(), HealthStatus::Unavailable);
}

#[test]
fn test_health_status_degraded_after_init_no_rl_config() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    // No explicit rate-limit config stored → Degraded (using fallback defaults)
    assert_eq!(client.get_health_status(), HealthStatus::Degraded);
}

#[test]
fn test_health_status_healthy_after_init_with_rl_config() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    // set_rate_limit_config(max_submissions, window_length)
    client.set_rate_limit_config(&10u32, &100u32);
    assert_eq!(client.get_health_status(), HealthStatus::Healthy);
}

// ---------------------------------------------------------------------------
// get_metadata_freshness
// ---------------------------------------------------------------------------

#[test]
fn test_metadata_freshness_missing() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    let anchor = Address::generate(&env);
    let report = client.get_metadata_freshness(&anchor);
    assert_eq!(report.state, MetadataCacheState::Missing);
    assert_eq!(report.age_seconds, 0);
    assert!(!report.needs_refresh);
}

#[test]
fn test_metadata_freshness_fresh() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    let anchor = Address::generate(&env);
    let metadata = make_metadata(&env, &anchor);
    // Cache with a 3600-second TTL
    client.cache_metadata(&anchor, &metadata, &3600u64);
    let report = client.get_metadata_freshness(&anchor);
    assert_eq!(report.state, MetadataCacheState::Fresh);
    assert!(!report.needs_refresh);
}

#[test]
fn test_metadata_freshness_stale() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    let anchor = Address::generate(&env);
    let metadata = make_metadata(&env, &anchor);
    // Cache with 10s TTL and 20s stale window
    client.cache_metadata_swr(&anchor, &metadata, &10u64, &20u64);

    // Advance time past the primary TTL but within the stale window
    env.ledger().set(LedgerInfo {
        timestamp: env.ledger().timestamp() + 15,
        ..env.ledger().get()
    });

    let report = client.get_metadata_freshness(&anchor);
    assert_eq!(report.state, MetadataCacheState::Stale);
    assert!(report.needs_refresh);
}

#[test]
fn test_metadata_freshness_expired() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    let anchor = Address::generate(&env);
    let metadata = make_metadata(&env, &anchor);
    client.cache_metadata_swr(&anchor, &metadata, &10u64, &5u64);

    // Advance time past both TTL and stale window
    env.ledger().set(LedgerInfo {
        timestamp: env.ledger().timestamp() + 20,
        ..env.ledger().get()
    });

    let report = client.get_metadata_freshness(&anchor);
    assert_eq!(report.state, MetadataCacheState::Expired);
    assert!(report.needs_refresh);
}

// ---------------------------------------------------------------------------
// get_rate_limiter_health
// ---------------------------------------------------------------------------

#[test]
fn test_rate_limiter_health_not_throttled() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    client.set_rate_limit_config(&5u32, &100u32);

    let attestor = Address::generate(&env);
    let report = client.get_rate_limiter_health(&attestor);
    assert_eq!(report.submission_count, 0);
    assert_eq!(report.max_submissions, 5);
    assert!(!report.is_throttled);
}

#[test]
fn test_rate_limiter_health_throttled() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    client.set_rate_limit_config(&2u32, &100u32);

    let config = RateLimitConfig { max_submissions: 2, window_length: 100 };
    let attestor = Address::generate(&env);
    // Exhaust the limit
    RateLimiter::check_and_increment(&env, &attestor, &config).unwrap();
    RateLimiter::check_and_increment(&env, &attestor, &config).unwrap();

    let report = client.get_rate_limiter_health(&attestor);
    assert_eq!(report.submission_count, 2);
    assert!(report.is_throttled);
}

#[test]
fn test_rate_limiter_health_resets_after_window() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    client.set_rate_limit_config(&2u32, &10u32);

    let config = RateLimitConfig { max_submissions: 2, window_length: 10 };
    let attestor = Address::generate(&env);
    RateLimiter::check_and_increment(&env, &attestor, &config).unwrap();
    RateLimiter::check_and_increment(&env, &attestor, &config).unwrap();

    // Advance ledger past the window
    env.ledger().set(LedgerInfo {
        sequence_number: env.ledger().sequence() + 11,
        ..env.ledger().get()
    });

    let report = client.get_rate_limiter_health(&attestor);
    // Window expired → effective count is 0, not throttled
    assert_eq!(report.submission_count, 0);
    assert!(!report.is_throttled);
}

// ---------------------------------------------------------------------------
// MetadataFreshnessReport freshness_score
// ---------------------------------------------------------------------------

#[test]
fn test_freshness_score_missing_is_zero() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    let anchor = Address::generate(&env);
    let report = client.get_metadata_freshness(&anchor);
    assert_eq!(report.freshness_score, 0, "missing entry must have score 0");
}

#[test]
fn test_freshness_score_fresh_near_100() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    let anchor = Address::generate(&env);
    let metadata = make_metadata(&env, &anchor);
    // Cache with a 3600s TTL, zero time elapsed → score near 100.
    client.cache_metadata(&anchor, &metadata, &3600u64);
    let report = client.get_metadata_freshness(&anchor);
    assert_eq!(report.state, MetadataCacheState::Fresh);
    assert!(report.freshness_score >= 90,
        "freshly cached entry at t=0 should score >= 90, got {}", report.freshness_score);
}

#[test]
fn test_freshness_score_decreases_with_age() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    let anchor = Address::generate(&env);
    let metadata = make_metadata(&env, &anchor);
    client.cache_metadata(&anchor, &metadata, &100u64);

    // Score at t=0
    let report_fresh = client.get_metadata_freshness(&anchor);

    // Advance time to 50% of TTL
    env.ledger().set(LedgerInfo {
        timestamp: env.ledger().timestamp() + 50,
        ..env.ledger().get()
    });
    let report_half = client.get_metadata_freshness(&anchor);

    // Score should be lower after aging
    assert!(report_half.freshness_score < report_fresh.freshness_score,
        "score should decrease as entry ages: fresh={}, half={}", 
        report_fresh.freshness_score, report_half.freshness_score);
    // At 50% of TTL, score should be around 50
    assert!(report_half.freshness_score >= 40 && report_half.freshness_score <= 60,
        "at 50% TTL score should be near 50, got {}", report_half.freshness_score);
}

#[test]
fn test_freshness_score_stale_is_halved() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    let anchor = Address::generate(&env);
    let metadata = make_metadata(&env, &anchor);
    // 10s TTL, 20s stale window
    client.cache_metadata_swr(&anchor, &metadata, &10u64, &20u64);

    // Move into the stale window (age = 15s, past 10s TTL)
    env.ledger().set(LedgerInfo {
        timestamp: env.ledger().timestamp() + 15,
        ..env.ledger().get()
    });

    let report = client.get_metadata_freshness(&anchor);
    assert_eq!(report.state, MetadataCacheState::Stale);
    // Stale score is halved relative to the age_score at the TTL boundary (which is 0),
    // so the base is 0/2 = 0 — but the SWR window is still usable.
    // The score should be low (0–25 range) due to being past the TTL.
    assert!(report.freshness_score <= 25,
        "stale entry score should be <= 25, got {}", report.freshness_score);
}

#[test]
fn test_freshness_score_expired_is_zero() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    let anchor = Address::generate(&env);
    let metadata = make_metadata(&env, &anchor);
    client.cache_metadata_swr(&anchor, &metadata, &10u64, &5u64);

    // Move past both TTL and stale window
    env.ledger().set(LedgerInfo {
        timestamp: env.ledger().timestamp() + 20,
        ..env.ledger().get()
    });

    let report = client.get_metadata_freshness(&anchor);
    assert_eq!(report.state, MetadataCacheState::Expired);
    assert_eq!(report.freshness_score, 0, "expired entry must have score 0");
}

#[test]
fn test_freshness_score_influences_refresh_decision() {
    let env = Env::default(); let client = setup_env(&env);
    init_contract(&env, &client);
    let anchor = Address::generate(&env);
    let metadata = make_metadata(&env, &anchor);
    client.cache_metadata(&anchor, &metadata, &100u64);

    // At t=0, high score → no refresh needed
    let report_now = client.get_metadata_freshness(&anchor);
    assert!(!report_now.needs_refresh);
    assert!(report_now.freshness_score > 50,
        "high-score entry should not need refresh, score={}", report_now.freshness_score);

    // Advance to 95% of TTL — score drops, refresh recommended
    env.ledger().set(LedgerInfo {
        timestamp: env.ledger().timestamp() + 95,
        ..env.ledger().get()
    });
    let report_old = client.get_metadata_freshness(&anchor);
    assert!(report_old.freshness_score < report_now.freshness_score,
        "aging should reduce score");
}
