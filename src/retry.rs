use alloc::vec::Vec;

use crate::trace_context::TraceContext;

/// The backoff strategy to use when computing retry delays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BackoffStrategy {
    /// Classic exponential backoff: `base * multiplier^attempt` (capped at max).
    Exponential,
    /// Linear backoff: `base * (attempt + 1)` (capped at max).
    Linear,
    /// Constant backoff: always `base_delay_ms` (capped at max).
    Constant,
    /// No retry — the operation is attempted exactly once regardless of
    /// `max_attempts`. Equivalent to setting `max_attempts = 1`.
    NoRetry,
}

impl Default for BackoffStrategy {
    fn default() -> Self {
        BackoffStrategy::Exponential
    }
}

/// The jitter policy applied to retry delays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JitterPolicy {
    /// Full jitter in `[0, base_delay_ms / 2]` added to the computed delay
    /// (the current behaviour). Provides good spread for thundering-herd prevention.
    Full,
    /// Equal jitter: delay is adjusted symmetrically by up to ±25% of the
    /// computed delay, producing less variance than Full.
    Equal,
    /// No jitter — every retry at the same attempt level waits the exact same
    /// amount of time. Useful for deterministic testing or when the caller
    /// manages jitter externally.
    None,
}

impl Default for JitterPolicy {
    fn default() -> Self {
        JitterPolicy::Full
    }
}

/// Retry configuration for off-chain anchor requests.
///
/// Controls how many times a failing operation is retried and how long to wait
/// between attempts. The delay grows according to `strategy` and is capped at
/// `max_delay_ms` to prevent unbounded waits.
///
/// # Examples
///
/// ```rust
/// use anchorkit::RetryConfig;
///
/// // Use sensible defaults: 3 attempts, 100 ms base, 5 s cap, ×2 multiplier.
/// let config = RetryConfig::default();
/// assert_eq!(config.max_attempts, 3);
///
/// // Custom configuration for a high-latency anchor.
/// let config = RetryConfig::new(5, 200, 10_000, 3);
/// assert_eq!(config.max_attempts, 5);
/// ```
#[derive(Clone, Debug)]
pub struct RetryConfig {
    /// Maximum number of attempts (including the first try).
    pub max_attempts: u32,
    /// Initial delay in milliseconds before the first retry.
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds (caps exponential growth).
    pub max_delay_ms: u64,
    /// Multiplier applied to the delay after each failed attempt.
    pub backoff_multiplier: u32,
    /// Backoff strategy governing how delays grow between attempts.
    pub strategy: BackoffStrategy,
    /// Jitter policy for adding variance to computed delays.
    pub jitter_policy: JitterPolicy,
}

/// Default delay cap in milliseconds for [`RetryConfig::default`] — bounds how
/// large the exponential backoff is allowed to grow before retries stop
/// waiting any longer between attempts.
const DEFAULT_MAX_DELAY_MS: u64 = 5_000;

impl Default for RetryConfig {
    fn default() -> Self {
        RetryConfig {
            max_attempts: 3,
            base_delay_ms: 100,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
            backoff_multiplier: 2,
            strategy: BackoffStrategy::default(),
            jitter_policy: JitterPolicy::default(),
        }
    }
}

impl RetryConfig {
    /// Create a [`RetryConfig`] with explicit values for all fields.
    ///
    /// # Arguments
    ///
    /// * `max_attempts` - Total number of attempts including the first try.
    ///   Must be at least `1`.
    /// * `base_delay_ms` - Delay in milliseconds before the first retry.
    /// * `max_delay_ms` - Upper bound on the computed delay (caps exponential growth).
    /// * `backoff_multiplier` - Factor by which the delay is multiplied each attempt.
    ///
    /// # Returns
    ///
    /// A new [`RetryConfig`] with default [`BackoffStrategy::Exponential`] and
    /// [`JitterPolicy::Full`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::RetryConfig;
    ///
    /// let config = RetryConfig::new(5, 200, 10_000, 3);
    /// assert_eq!(config.max_attempts, 5);
    /// assert_eq!(config.base_delay_ms, 200);
    /// ```
    pub fn new(
        max_attempts: u32,
        base_delay_ms: u64,
        max_delay_ms: u64,
        backoff_multiplier: u32,
    ) -> Self {
        RetryConfig {
            max_attempts,
            base_delay_ms,
            max_delay_ms,
            backoff_multiplier,
            strategy: BackoffStrategy::default(),
            jitter_policy: JitterPolicy::default(),
        }
    }

    /// Full configuration constructor including strategy and jitter policy.
    pub fn with_strategy(
        max_attempts: u32,
        base_delay_ms: u64,
        max_delay_ms: u64,
        backoff_multiplier: u32,
        strategy: BackoffStrategy,
        jitter_policy: JitterPolicy,
    ) -> Self {
        RetryConfig {
            max_attempts,
            base_delay_ms,
            max_delay_ms,
            backoff_multiplier,
            strategy,
            jitter_policy,
        }
    }

    /// 5 attempts, 50 ms base, 2 s max — for time-sensitive operations.
    pub fn aggressive() -> Self {
        RetryConfig {
            max_attempts: 5,
            base_delay_ms: 50,
            max_delay_ms: 2_000,
            backoff_multiplier: 2,
            strategy: BackoffStrategy::default(),
            jitter_policy: JitterPolicy::default(),
        }
    }

    /// 2 attempts, 500 ms base, 10 s max — for conservative/low-noise retries.
    pub fn conservative() -> Self {
        RetryConfig {
            max_attempts: 2,
            base_delay_ms: 500,
            max_delay_ms: 10_000,
            backoff_multiplier: 2,
            strategy: BackoffStrategy::default(),
            jitter_policy: JitterPolicy::default(),
        }
    }

    /// Linear strategy: 6 attempts, 100 ms base, 5 s max — for predictable delays.
    pub fn linear() -> Self {
        RetryConfig {
            max_attempts: 6,
            base_delay_ms: 100,
            max_delay_ms: 5_000,
            backoff_multiplier: 2,
            strategy: BackoffStrategy::Linear,
            jitter_policy: JitterPolicy::Full,
        }
    }

    /// Constant strategy: 5 attempts, 500 ms fixed delay — for steady retries.
    pub fn constant() -> Self {
        RetryConfig {
            max_attempts: 5,
            base_delay_ms: 500,
            max_delay_ms: 500,
            backoff_multiplier: 1,
            strategy: BackoffStrategy::Constant,
            jitter_policy: JitterPolicy::None,
        }
    }

    /// Set the backoff strategy on an existing config.
    pub fn with_backoff_strategy(mut self, strategy: BackoffStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Set the jitter policy on an existing config.
    pub fn with_jitter_policy(mut self, jitter_policy: JitterPolicy) -> Self {
        self.jitter_policy = jitter_policy;
        self
    }

    /// Compute the base delay (ms) for a given attempt index (0-based) using
    /// the configured [`BackoffStrategy`], before jitter is applied.
    fn base_delay_for_attempt(&self, attempt: u32) -> u64 {
        match self.strategy {
            BackoffStrategy::Exponential => {
                let exp = (self.backoff_multiplier as u64).saturating_pow(attempt);
                self.base_delay_ms.saturating_mul(exp).min(self.max_delay_ms)
            }
            BackoffStrategy::Linear => {
                let factor = (attempt as u64).saturating_add(1);
                self.base_delay_ms.saturating_mul(factor).min(self.max_delay_ms)
            }
            BackoffStrategy::Constant => self.base_delay_ms.min(self.max_delay_ms),
            BackoffStrategy::NoRetry => 0,
        }
    }

    /// Compute the delay (ms) for a given attempt index (0-based), drawing
    /// jitter from `jitter_source` according to [`JitterPolicy`].
    ///
    /// The total is capped at `max_delay_ms` so the configured ceiling is never
    /// exceeded regardless of the jitter seed.
    pub fn delay_for_attempt(&self, attempt: u32, jitter_source: &mut impl JitterSource) -> u64 {
        let base = self.base_delay_for_attempt(attempt);
        match self.jitter_policy {
            JitterPolicy::Full => {
                let jitter_bound = self.base_delay_ms / 2 + 1;
                let jitter = if jitter_bound == 0 { 0 } else { jitter_source.next_seed() % jitter_bound };
                base.saturating_add(jitter).min(self.max_delay_ms)
            }
            JitterPolicy::Equal => {
                let half = base / 2;
                let jitter_bound = if half == 0 { 1 } else { half };
                let offset = jitter_source.next_seed() % jitter_bound;
                let sign = (jitter_source.next_seed() % 2) as i64;
                if sign == 0 {
                    (base + offset).min(self.max_delay_ms)
                } else {
                    base.saturating_sub(offset).max(self.base_delay_ms.min(base))
                }
            }
            JitterPolicy::None => base,
        }
    }
}

// ---------------------------------------------------------------------------
// JitterSource trait
// ---------------------------------------------------------------------------

/// Provides a seed value for jitter computation on each retry attempt.
///
/// Implementations must produce values that differ across consecutive calls
/// to avoid the thundering-herd problem when multiple clients retry together.
pub trait JitterSource {
    fn next_seed(&mut self) -> u64;
}

// ---------------------------------------------------------------------------
// LedgerJitterSource
// ---------------------------------------------------------------------------

/// Derives jitter seeds from Soroban ledger state.
///
/// XORs `sequence ^ timestamp ^ counter` so that consecutive calls within
/// the same ledger still produce different seeds.
pub struct LedgerJitterSource {
    sequence: u32,
    timestamp: u64,
    counter: u64,
}

impl LedgerJitterSource {
    pub fn new(sequence: u32, timestamp: u64) -> Self {
        LedgerJitterSource { sequence, timestamp, counter: 0 }
    }
}

impl JitterSource for LedgerJitterSource {
    fn next_seed(&mut self) -> u64 {
        let seed = (self.sequence as u64) ^ self.timestamp ^ self.counter;
        self.counter = self.counter.wrapping_add(1);
        seed
    }
}

// ---------------------------------------------------------------------------
// MockJitterSource
// ---------------------------------------------------------------------------

/// Produces a pre-configured sequence of seeds for deterministic testing.
/// Cycles back to the start when the sequence is exhausted.
pub struct MockJitterSource {
    seeds: Vec<u64>,
    index: usize,
}

impl MockJitterSource {
    pub fn new(seeds: Vec<u64>) -> Self {
        MockJitterSource { seeds, index: 0 }
    }
}

impl JitterSource for MockJitterSource {
    fn next_seed(&mut self) -> u64 {
        if self.seeds.is_empty() {
            return 0;
        }
        let seed = self.seeds[self.index % self.seeds.len()];
        self.index += 1;
        seed
    }
}

// ---------------------------------------------------------------------------
// Classify whether an error code is retryable.
// ---------------------------------------------------------------------------

/// Classify whether an error code is retryable.
///
/// Retryable: transient network/server errors (availability, stale data).
/// Non-retryable: auth failures, bad input, protocol violations, rate limits.
///
/// `RateLimitExceeded` is intentionally NOT retryable: retrying immediately (or
/// with a backoff shorter than the rate window) reproduces the same error and
/// wastes all attempts. Callers that want to respect a rate limit should
/// implement their own wait-and-retry loop keyed to the window length.
pub fn is_retryable(code: crate::errors::ErrorCode) -> bool {
    use crate::errors::ErrorCode;
    match code {
        ErrorCode::AttestationNotFound
        | ErrorCode::StaleQuote
        | ErrorCode::NoQuotesAvailable
        | ErrorCode::CacheExpired
        | ErrorCode::CacheNotFound => true,
        _ => false,
    }
}

/// Execute `f` with exponential backoff retry.
///
/// Calls `f` up to `config.max_attempts` times. After each failure that
/// `retryable` classifies as transient, waits for the computed backoff delay
/// (via `sleep_fn`) before trying again. Stops immediately on a non-retryable
/// error or when all attempts are exhausted.
///
/// # Arguments
///
/// * `config` - Retry parameters (attempts, delays, multiplier).
/// * `f` - The fallible operation. Receives the current attempt index (0-based).
/// * `retryable` - Predicate that returns `true` when an error warrants a retry.
/// * `sleep_fn` - Callback invoked with the delay in milliseconds between attempts.
///   Inject `|_| {}` in tests to avoid real sleeps.
///
/// # Returns
///
/// `Ok(T)` on the first successful attempt, or `Err(E)` after all attempts are
/// exhausted or a non-retryable error is encountered.
///
/// # Errors
///
/// Returns the last error produced by `f`. The error is non-retryable if
/// `retryable` returned `false`, or all `max_attempts` were consumed.
///
/// # Examples
///
/// ```rust,no_run
/// use anchorkit::retry::{retry_with_backoff, MockJitterSource, RetryConfig};
///
/// let config = RetryConfig::default();
/// let mut calls = 0u32;
/// let mut js = MockJitterSource::new(vec![0]);
///
/// let result = retry_with_backoff(
///     &config,
///     |attempt| {
///         calls += 1;
///         if attempt < 2 { Err("transient") } else { Ok(42u32) }
///     },
///     |_err| true,   // all errors are retryable
///     |_ms| {},      // no-op sleep
///     &mut js,       // jitter source
/// );
/// assert_eq!(result, Ok(42u32));
/// ```
///
/// A `sleep_fn` callback is provided so callers can inject real or mock sleep.
/// `jitter_source` provides per-attempt seeds to spread retry timing.
pub fn retry_with_backoff<T, E, F, S, J>(
    config: &RetryConfig,
    mut f: F,
    retryable: impl Fn(&E) -> bool,
    mut sleep_fn: S,
    jitter_source: &mut J,
) -> Result<T, E>
where
    F: FnMut(u32) -> Result<T, E>,
    S: FnMut(u64),
    J: JitterSource,
{
    // Guard against `max_attempts == 0`: the loop below would never execute,
    // leaving `last_err` as `None` and hitting the `unreachable!()` at the end
    // (a panic in debug, UB-adjacent in release). Treat 0 as a single attempt
    // so the operation still runs exactly once and its result is returned.
    debug_assert!(config.max_attempts >= 1, "max_attempts must be at least 1");
    if config.max_attempts == 0 {
        return f(0);
    }

    let mut last_err: Option<E> = None;

    for attempt in 0..config.max_attempts {
        match f(attempt) {
            Ok(val) => return Ok(val),
            Err(e) => {
                if !retryable(&e) || attempt + 1 >= config.max_attempts {
                    return Err(e);
                }
                let delay = config.delay_for_attempt(attempt, jitter_source);
                sleep_fn(delay);
                last_err = Some(e);
            }
        }
    }

    // Safety: the loop above always returns early via `return Err(e)` when
    // `attempt + 1 >= config.max_attempts`, so `last_err` is always `Some` here.
    // We use an explicit match instead of expect to avoid any panic path.
    match last_err {
        Some(e) => Err(e),
        None => unreachable!("retry_with_backoff: max_attempts must be >= 1"),
    }
}

/// Execute `f` with backoff retry, threading a [`TraceContext`] through every
/// attempt.
///
/// Identical to [`retry_with_backoff`] except that `f` also receives the trace
/// context for the attempt it is running. Each attempt gets its own child span
/// via [`TraceContext::child_for_attempt`], so all attempts share `parent`'s
/// `trace_id` while remaining individually identifiable in logs.
///
/// This is the building block that keeps trace context alive across retries:
/// callers that pass their inbound context here get end-to-end correlation
/// without threading identifiers through their own closure state.
///
/// # Arguments
///
/// * `config` - Retry parameters (attempts, delays, multiplier).
/// * `parent` - The trace context this retry loop runs under. Attempt spans are
///   derived from it; it is never mutated.
/// * `f` - The fallible operation. Receives the 0-based attempt index and the
///   trace context for that attempt.
/// * `retryable` - Predicate that returns `true` when an error warrants a retry.
/// * `sleep_fn` - Callback invoked with the delay in milliseconds between attempts.
/// * `jitter_source` - Per-attempt seeds used to spread retry timing.
///
/// # Returns
///
/// `Ok(T)` on the first successful attempt, or `Err(E)` after all attempts are
/// exhausted or a non-retryable error is encountered.
///
/// # Errors
///
/// Returns the last error produced by `f`, exactly as [`retry_with_backoff`] does.
///
/// # Examples
///
/// ```rust
/// use anchorkit::retry::{retry_with_backoff_traced, MockJitterSource, RetryConfig};
/// use anchorkit::trace_context::TraceContext;
///
/// let config = RetryConfig::default();
/// let parent = TraceContext::root_from_seed("deposit:txn-001");
/// let mut seen_trace_ids: Vec<String> = Vec::new();
/// let mut js = MockJitterSource::new(vec![0]);
///
/// let result = retry_with_backoff_traced(
///     &config,
///     &parent,
///     |attempt, trace| {
///         seen_trace_ids.push(trace.trace_id().to_string());
///         if attempt < 2 { Err("transient") } else { Ok(attempt) }
///     },
///     |_err| true,
///     |_ms| {},
///     &mut js,
/// );
///
/// assert_eq!(result, Ok(2));
/// // The trace survived every retry.
/// assert!(seen_trace_ids.iter().all(|id| id == parent.trace_id()));
/// ```
pub fn retry_with_backoff_traced<T, E, F, S, J>(
    config: &RetryConfig,
    parent: &TraceContext,
    mut f: F,
    retryable: impl Fn(&E) -> bool,
    sleep_fn: S,
    jitter_source: &mut J,
) -> Result<T, E>
where
    F: FnMut(u32, &TraceContext) -> Result<T, E>,
    S: FnMut(u64),
    J: JitterSource,
{
    retry_with_backoff(
        config,
        |attempt| {
            let attempt_trace = parent.child_for_attempt(attempt);
            f(attempt, &attempt_trace)
        },
        retryable,
        sleep_fn,
        jitter_source,
    )
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use alloc::vec;

    #[derive(Debug, PartialEq)]
    enum TestError {
        Transient,
        Permanent,
    }

    fn is_retryable_test(e: &TestError) -> bool {
        matches!(e, TestError::Transient)
    }

    #[test]
    fn test_success_on_first_try() {
        let config = RetryConfig::default();
        let mut calls = 0u32;
        let mut js = MockJitterSource::new(vec![0]);
        let result = retry_with_backoff(
            &config,
            |_| {
                calls += 1;
                Ok::<_, TestError>(42)
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );
        assert_eq!(result, Ok(42));
        assert_eq!(calls, 1);
    }

    #[test]
    fn test_success_after_retry() {
        let config = RetryConfig::default();
        let mut calls = 0u32;
        let mut js = MockJitterSource::new(vec![0, 0, 0]);
        let result = retry_with_backoff(
            &config,
            |attempt| {
                calls += 1;
                if attempt < 2 {
                    Err(TestError::Transient)
                } else {
                    Ok(99)
                }
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );
        assert_eq!(result, Ok(99));
        assert_eq!(calls, 3);
    }

    #[test]
    fn test_exhausted_retries() {
        let config = RetryConfig::new(3, 10, 1000, 2);
        let mut calls = 0u32;
        let mut js = MockJitterSource::new(vec![0]);
        let result = retry_with_backoff(
            &config,
            |_| {
                calls += 1;
                Err::<i32, _>(TestError::Transient)
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );
        assert_eq!(result, Err(TestError::Transient));
        assert_eq!(calls, 3);
    }

    #[test]
    fn test_non_retryable_error_stops_immediately() {
        let config = RetryConfig::new(5, 10, 1000, 2);
        let mut calls = 0u32;
        let mut js = MockJitterSource::new(vec![0]);
        let result = retry_with_backoff(
            &config,
            |_| {
                calls += 1;
                Err::<i32, _>(TestError::Permanent)
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );
        assert_eq!(result, Err(TestError::Permanent));
        assert_eq!(calls, 1);
    }

    #[test]
    fn test_delay_increases_exponentially() {
        let config = RetryConfig::new(4, 100, 10_000, 2);
        let mut js = MockJitterSource::new(vec![0]);
        assert!(config.delay_for_attempt(0, &mut js) >= 100);
        assert!(config.delay_for_attempt(1, &mut js) >= 200);
        assert!(config.delay_for_attempt(2, &mut js) >= 400);
    }

    #[test]
    fn test_delay_capped_at_max() {
        let config = RetryConfig::new(10, 1000, 3_000, 2);
        let mut js = MockJitterSource::new(vec![0]);
        assert!(config.delay_for_attempt(5, &mut js) <= config.max_delay_ms);
    }

    #[test]
    fn test_sleep_called_between_retries() {
        let config = RetryConfig::new(3, 50, 5000, 2);
        let mut sleep_calls = 0u32;
        let mut js = MockJitterSource::new(vec![0]);
        let _ = retry_with_backoff(
            &config,
            |_| Err::<i32, _>(TestError::Transient),
            is_retryable_test,
            |_| sleep_calls += 1,
            &mut js,
        );
        assert_eq!(sleep_calls, 2);
    }

    #[test]
    fn test_aggressive_config() {
        let cfg = RetryConfig::aggressive();
        assert_eq!(cfg.max_attempts, 5);
        assert_eq!(cfg.base_delay_ms, 50);
        assert_eq!(cfg.max_delay_ms, 2_000);
        assert_eq!(cfg.backoff_multiplier, 2);
    }

    #[test]
    fn test_conservative_config() {
        let cfg = RetryConfig::conservative();
        assert_eq!(cfg.max_attempts, 2);
        assert_eq!(cfg.base_delay_ms, 500);
        assert_eq!(cfg.max_delay_ms, 10_000);
        assert_eq!(cfg.backoff_multiplier, 2);
    }

    #[test]
    fn test_aggressive_retries_up_to_five_attempts() {
        let config = RetryConfig::aggressive();
        let mut calls = 0u32;
        let mut js = MockJitterSource::new(vec![0]);
        let _ = retry_with_backoff(
            &config,
            |_| {
                calls += 1;
                Err::<i32, _>(TestError::Transient)
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );
        assert_eq!(calls, 5);
    }

    #[test]
    fn test_conservative_stops_after_two_attempts() {
        let config = RetryConfig::conservative();
        let mut calls = 0u32;
        let mut js = MockJitterSource::new(vec![0]);
        let _ = retry_with_backoff(
            &config,
            |_| {
                calls += 1;
                Err::<i32, _>(TestError::Transient)
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );
        assert_eq!(calls, 2);
    }

    // -----------------------------------------------------------------------
    // New tests for JitterSource
    // -----------------------------------------------------------------------

    /// Two retries with different seeds produce different delays.
    #[test]
    fn test_different_seeds_produce_different_delays() {
        let config = RetryConfig::new(4, 100, 10_000, 2);
        let mut js_a = MockJitterSource::new(vec![0]);
        let mut js_b = MockJitterSource::new(vec![49]); // max jitter for base=100
        let delay_a = config.delay_for_attempt(0, &mut js_a);
        let delay_b = config.delay_for_attempt(0, &mut js_b);
        assert_ne!(delay_a, delay_b);
    }

    /// Delay is always within configured bounds [base..=max_delay_ms].
    #[test]
    fn test_delay_within_bounds() {
        let config = RetryConfig::new(6, 100, 3_000, 2);
        for seed in [0u64, 1, 25, 49, 50, 99, 1000] {
            for attempt in 0..6u32 {
                let mut js = MockJitterSource::new(vec![seed]);
                let delay = config.delay_for_attempt(attempt, &mut js);
                assert!(delay >= config.base_delay_ms, "delay {delay} < base");
                assert!(
                    delay <= config.max_delay_ms,
                    "delay {delay} > max_delay_ms"
                );
            }
        }
    }

    /// MockJitterSource produces deterministic results in the specified order.
    #[test]
    fn test_mock_source_deterministic() {
        let config = RetryConfig::new(4, 100, 10_000, 2);
        let seeds = vec![10u64, 20, 30];
        let mut js = MockJitterSource::new(seeds.clone());

        let d0 = config.delay_for_attempt(0, &mut js); // seed=10, jitter=10%51=10
        let d1 = config.delay_for_attempt(1, &mut js); // seed=20, jitter=20%51=20
        let d2 = config.delay_for_attempt(2, &mut js); // seed=30, jitter=30%51=30

        assert_eq!(d0, 100 + 10); // 100 * 2^0 + 10
        assert_eq!(d1, 200 + 20); // 100 * 2^1 + 20
        assert_eq!(d2, 400 + 30); // 100 * 2^2 + 30
    }

    /// LedgerJitterSource produces different seeds on consecutive calls.
    #[test]
    fn test_ledger_jitter_source_consecutive_differ() {
        let mut js = LedgerJitterSource::new(42, 1_000_000);
        let s0 = js.next_seed();
        let s1 = js.next_seed();
        let s2 = js.next_seed();
        assert_ne!(s0, s1);
        assert_ne!(s1, s2);
    }

    /// retry_with_backoff passes jitter_source through to delay_for_attempt.
    #[test]
    fn test_mock_clock_delay_sequence() {
        let config = RetryConfig::new(4, 100, 10_000, 2);
        // seeds: 3, 20, 37 → jitter: 3%51=3, 20%51=20, 37%51=37
        let mut js = MockJitterSource::new(vec![3, 20, 37]);
        let mut recorded: Vec<u64> = Vec::new();

        let _ = retry_with_backoff(
            &config,
            |_| Err::<i32, _>(TestError::Transient),
            is_retryable_test,
            |ms| recorded.push(ms),
            &mut js,
        );

        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0], 100 + 3);  // attempt 0: 100*2^0 + 3
        assert_eq!(recorded[1], 200 + 20); // attempt 1: 100*2^1 + 20
        assert_eq!(recorded[2], 400 + 37); // attempt 2: 100*2^2 + 37
    }

    // -----------------------------------------------------------------------
    // Issue #347 — deterministic jitter source tests
    // -----------------------------------------------------------------------

    /// LedgerJitterSource seed formula: sequence ^ timestamp ^ counter (counter starts at 0).
    #[test]
    fn test_ledger_jitter_source_seed_formula() {
        let seq: u32 = 42;
        let ts: u64 = 1_000_000;
        let mut js = LedgerJitterSource::new(seq, ts);

        assert_eq!(js.next_seed(), (seq as u64) ^ ts ^ 0);
        assert_eq!(js.next_seed(), (seq as u64) ^ ts ^ 1);
        assert_eq!(js.next_seed(), (seq as u64) ^ ts ^ 2);
    }

    /// LedgerJitterSource counter wraps via wrapping_add — no panic at saturation.
    #[test]
    fn test_ledger_jitter_source_counter_wraps() {
        // Build a source whose counter is already at u64::MAX
        let seq: u32 = 1;
        let ts: u64 = 0;
        let mut js = LedgerJitterSource { sequence: seq, timestamp: ts, counter: u64::MAX };
        let seed = js.next_seed();
        assert_eq!(seed, (seq as u64) ^ ts ^ u64::MAX);
        // Next call after wrapping — counter should have wrapped to 0
        let seed2 = js.next_seed();
        assert_eq!(seed2, (seq as u64) ^ ts ^ 0);
    }

    /// MockJitterSource cycles back to the first seed when the list is exhausted.
    #[test]
    fn test_mock_jitter_source_cycles_when_exhausted() {
        let mut js = MockJitterSource::new(vec![10, 20]);
        assert_eq!(js.next_seed(), 10);
        assert_eq!(js.next_seed(), 20);
        assert_eq!(js.next_seed(), 10); // wraps back
        assert_eq!(js.next_seed(), 20);
    }

    /// MockJitterSource with an empty seed list always returns 0.
    #[test]
    fn test_mock_jitter_source_empty_returns_zero() {
        let mut js = MockJitterSource::new(vec![]);
        assert_eq!(js.next_seed(), 0);
        assert_eq!(js.next_seed(), 0);
    }

    /// delay_for_attempt with base_delay_ms = 0 produces 0 delay (no jitter).
    #[test]
    fn test_delay_for_attempt_zero_base() {
        let config = RetryConfig::new(3, 0, 1_000, 2);
        let mut js = MockJitterSource::new(vec![999]);
        assert_eq!(config.delay_for_attempt(0, &mut js), 0);
        assert_eq!(config.delay_for_attempt(1, &mut js), 0);
    }

    /// Issue #762: an explicit zero `base_delay_ms` must be preserved end to
    /// end — `sleep_fn` should observe `0` on every retry, not a positive
    /// default substituted in its place.
    #[test]
    fn test_explicit_zero_base_delay_reaches_sleep_fn() {
        let config = RetryConfig::new(3, 0, 1_000, 2);
        assert_eq!(config.base_delay_ms, 0, "explicit zero must be stored as-is");

        let mut recorded: Vec<u64> = Vec::new();
        let mut js = MockJitterSource::new(vec![0]);
        let _ = retry_with_backoff(
            &config,
            |_| Err::<i32, _>(TestError::Transient),
            is_retryable_test,
            |ms| recorded.push(ms),
            &mut js,
        );

        assert_eq!(recorded.len(), 2);
        assert!(recorded.iter().all(|&ms| ms == 0), "recorded delays: {recorded:?}");

        // The default configuration (no explicit zero given) must remain
        // unaffected by the change above.
        let default_config = RetryConfig::default();
        assert_eq!(default_config.base_delay_ms, 100);

        // A positive explicit value must also be unaffected.
        let positive_config = RetryConfig::new(3, 250, 1_000, 2);
        assert_eq!(positive_config.base_delay_ms, 250);
    }

    /// Total delay (including jitter) never exceeds max_delay_ms.
    #[test]
    fn test_jitter_does_not_push_past_max_delay() {
        let config = RetryConfig::new(5, 1000, 3_000, 2);
        // Use a large seed to maximise jitter contribution
        for seed in [u64::MAX, 9999, 1000, 500] {
            for attempt in 0..5u32 {
                let mut js = MockJitterSource::new(vec![seed]);
                let delay = config.delay_for_attempt(attempt, &mut js);
                assert!(
                    delay <= config.max_delay_ms,
                    "attempt={attempt} seed={seed}: delay {delay} > max {}",
                    config.max_delay_ms
                );
            }
        }
    }

    /// Delays at each attempt level match the expected exponential formula.
    #[test]
    fn test_delay_per_attempt_level() {
        // Use seed 0 (zero jitter) so we test the pure exponential component.
        let config = RetryConfig::new(6, 100, 10_000, 2);
        let expected = [100u64, 200, 400, 800, 1600, 3200];
        for (attempt, &exp) in expected.iter().enumerate() {
            let mut js = MockJitterSource::new(vec![0]);
            assert_eq!(config.delay_for_attempt(attempt as u32, &mut js), exp,
                "attempt {attempt}: expected {exp}");
        }
    }

    // -----------------------------------------------------------------------
    // Issue #623 — configurable retry strategy tests
    // -----------------------------------------------------------------------

    /// Linear backoff: base * (attempt + 1)
    #[test]
    fn test_linear_backoff_strategy() {
        let config = RetryConfig::with_strategy(5, 100, 10_000, 2, BackoffStrategy::Linear, JitterPolicy::None);
        let mut js = MockJitterSource::new(vec![0]);
        assert_eq!(config.delay_for_attempt(0, &mut js), 100);
        assert_eq!(config.delay_for_attempt(1, &mut js), 200);
        assert_eq!(config.delay_for_attempt(2, &mut js), 300);
        assert_eq!(config.delay_for_attempt(3, &mut js), 400);
    }

    /// Constant backoff: always base_delay_ms.
    #[test]
    fn test_constant_backoff_strategy() {
        let config = RetryConfig::with_strategy(5, 250, 10_000, 2, BackoffStrategy::Constant, JitterPolicy::None);
        let mut js = MockJitterSource::new(vec![0]);
        for i in 0..5 {
            assert_eq!(config.delay_for_attempt(i, &mut js), 250);
        }
    }

    /// NoRetry strategy always returns 0 delay.
    #[test]
    fn test_no_retry_strategy() {
        let config = RetryConfig::with_strategy(5, 100, 10_000, 2, BackoffStrategy::NoRetry, JitterPolicy::None);
        let mut js = MockJitterSource::new(vec![0]);
        for i in 0..5 {
            assert_eq!(config.delay_for_attempt(i, &mut js), 0);
        }
    }

    /// Linear presets.
    #[test]
    fn test_linear_preset() {
        let cfg = RetryConfig::linear();
        assert_eq!(cfg.max_attempts, 6);
        assert_eq!(cfg.base_delay_ms, 100);
        assert_eq!(cfg.strategy, BackoffStrategy::Linear);
    }

    /// Constant preset.
    #[test]
    fn test_constant_preset() {
        let cfg = RetryConfig::constant();
        assert_eq!(cfg.max_attempts, 5);
        assert_eq!(cfg.base_delay_ms, 500);
        assert_eq!(cfg.max_delay_ms, 500);
        assert_eq!(cfg.strategy, BackoffStrategy::Constant);
        assert_eq!(cfg.jitter_policy, JitterPolicy::None);
    }

    /// with_backoff_strategy builder method.
    #[test]
    fn test_with_backoff_strategy_builder() {
        let cfg = RetryConfig::default().with_backoff_strategy(BackoffStrategy::Linear);
        assert_eq!(cfg.strategy, BackoffStrategy::Linear);
    }

    /// with_jitter_policy builder method.
    #[test]
    fn test_with_jitter_policy_builder() {
        let cfg = RetryConfig::default().with_jitter_policy(JitterPolicy::None);
        assert_eq!(cfg.jitter_policy, JitterPolicy::None);
    }

    /// NoJitter policy: delay is deterministic for a given attempt.
    #[test]
    fn test_no_jitter_policy() {
        let config = RetryConfig::with_strategy(4, 100, 10_000, 2, BackoffStrategy::Exponential, JitterPolicy::None);
        let mut js = MockJitterSource::new(vec![999]);
        assert_eq!(config.delay_for_attempt(0, &mut js), 100);
        assert_eq!(config.delay_for_attempt(1, &mut js), 200);
        assert_eq!(config.delay_for_attempt(2, &mut js), 400);
    }

    /// FullJitter uses seed and base_delay_ms/2 bound.
    #[test]
    fn test_full_jitter_policy() {
        let config = RetryConfig::with_strategy(3, 100, 10_000, 2, BackoffStrategy::Exponential, JitterPolicy::Full);
        let mut js = MockJitterSource::new(vec![10]);
        assert_eq!(config.delay_for_attempt(0, &mut js), 100 + 10);
        let mut js = MockJitterSource::new(vec![20]);
        assert_eq!(config.delay_for_attempt(1, &mut js), 200 + 20);
    }

    /// EqualJitter produces different results depending on sign bit.
    #[test]
    fn test_equal_jitter_policy() {
        let config = RetryConfig::with_strategy(3, 100, 10_000, 2, BackoffStrategy::Exponential, JitterPolicy::Equal);
        // seed 0 => offset = 0 % 50 = 0, sign seed 1 => 1 % 2 = 1 (subtract)
        let mut js = MockJitterSource::new(vec![0, 1]);
        let delay = config.delay_for_attempt(0, &mut js);
        assert_eq!(delay, 100);

        // seed 5 => offset = 5 % 50 = 5, sign seed 3 => 3 % 2 = 1 (subtract)
        let mut js2 = MockJitterSource::new(vec![5, 3]);
        let delay2 = config.delay_for_attempt(0, &mut js2);
        assert_eq!(delay2, 100 - 5);
    }

    /// NoRetry in retry_with_backoff still produces exactly one attempt.
    #[test]
    fn test_no_retry_with_backoff() {
        let config = RetryConfig::with_strategy(5, 100, 10_000, 2, BackoffStrategy::NoRetry, JitterPolicy::None);
        let mut calls = 0u32;
        let mut js = MockJitterSource::new(vec![0]);
        let result = retry_with_backoff(
            &config,
            |_| { calls += 1; Err::<i32, _>(TestError::Transient) },
            is_retryable_test,
            |_| {},
            &mut js,
        );
        assert_eq!(calls, 1);
        assert!(result.is_err());
    }

    /// Strategy propagates through retry_with_backoff.
    #[test]
    fn test_linear_backoff_through_retry() {
        let config = RetryConfig::with_strategy(4, 100, 10_000, 2, BackoffStrategy::Linear, JitterPolicy::None);
        let mut recorded: Vec<u64> = Vec::new();
        let mut js = MockJitterSource::new(vec![0]);

        let _ = retry_with_backoff(
            &config,
            |_| Err::<i32, _>(TestError::Transient),
            is_retryable_test,
            |ms| recorded.push(ms),
            &mut js,
        );

        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0], 100);
        assert_eq!(recorded[1], 200);
        assert_eq!(recorded[2], 300);
    }

    /// Linear cap at max_delay_ms.
    #[test]
    fn test_linear_backoff_capped_at_max() {
        let config = RetryConfig::with_strategy(10, 1000, 3_000, 2, BackoffStrategy::Linear, JitterPolicy::None);
        let mut js = MockJitterSource::new(vec![0]);
        assert_eq!(config.delay_for_attempt(2, &mut js), 3_000);
        assert_eq!(config.delay_for_attempt(5, &mut js), 3_000);
    }

    /// Constant backoff with jitter still applies jitter.
    #[test]
    fn test_constant_backoff_with_full_jitter() {
        let config = RetryConfig::with_strategy(3, 500, 10_000, 2, BackoffStrategy::Constant, JitterPolicy::Full);
        let mut js = MockJitterSource::new(vec![10, 20, 30]);
        assert_eq!(config.delay_for_attempt(0, &mut js), 500 + 10);
        assert_eq!(config.delay_for_attempt(1, &mut js), 500 + 20);
        assert_eq!(config.delay_for_attempt(2, &mut js), 500 + 30);
    }

    /// BackoffStrategy default is Exponential.
    #[test]
    fn test_backoff_strategy_default() {
        assert_eq!(BackoffStrategy::default(), BackoffStrategy::Exponential);
    }

    /// JitterPolicy default is Full.
    #[test]
    fn test_jitter_policy_default() {
        assert_eq!(JitterPolicy::default(), JitterPolicy::Full);
    }

    // -----------------------------------------------------------------------
    // Issue #610 — trace context propagation across retries
    // -----------------------------------------------------------------------

    use alloc::string::{String, ToString};

    /// Every attempt of a retried operation sees the same trace ID.
    #[test]
    fn test_trace_id_survives_every_retry() {
        let config = RetryConfig::new(4, 1, 10, 1);
        let parent = TraceContext::root_from_seed("retry-trace-survival");
        let mut js = MockJitterSource::new(vec![0]);
        let mut seen: Vec<String> = Vec::new();

        let result = retry_with_backoff_traced(
            &config,
            &parent,
            |attempt, trace| {
                seen.push(trace.trace_id().to_string());
                if attempt < 3 {
                    Err(TestError::Transient)
                } else {
                    Ok(attempt)
                }
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );

        assert_eq!(result, Ok(3));
        assert_eq!(seen.len(), 4, "all four attempts should have run");
        assert!(
            seen.iter().all(|id| id == parent.trace_id()),
            "trace_id must not change across retries: {seen:?}"
        );
    }

    /// Each attempt gets a distinct span parented to the retry-loop span.
    #[test]
    fn test_each_attempt_gets_a_distinct_child_span() {
        let config = RetryConfig::new(3, 1, 10, 1);
        let parent = TraceContext::root_from_seed("retry-span-per-attempt");
        let mut js = MockJitterSource::new(vec![0]);
        let mut spans: Vec<String> = Vec::new();

        let _ = retry_with_backoff_traced(
            &config,
            &parent,
            |_attempt, trace| {
                spans.push(trace.span_id().to_string());
                assert_eq!(
                    trace.parent_span_id(),
                    Some(parent.span_id()),
                    "attempt span must be parented to the retry-loop span"
                );
                Err::<i32, _>(TestError::Transient)
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );

        assert_eq!(spans.len(), 3);
        assert_ne!(spans[0], spans[1]);
        assert_ne!(spans[1], spans[2]);
        assert_ne!(spans[0], spans[2]);
    }

    /// Attempt spans are reproducible: the same parent and attempt index always
    /// produce the same span, so a replayed retry is recognisable in logs.
    #[test]
    fn test_attempt_spans_are_reproducible() {
        let config = RetryConfig::new(3, 1, 10, 1);
        let parent = TraceContext::root_from_seed("retry-reproducible");

        let run = |parent: &TraceContext| {
            let mut js = MockJitterSource::new(vec![0]);
            let mut spans: Vec<String> = Vec::new();
            let _ = retry_with_backoff_traced(
                &config,
                parent,
                |_attempt, trace| {
                    spans.push(trace.span_id().to_string());
                    Err::<i32, _>(TestError::Transient)
                },
                is_retryable_test,
                |_| {},
                &mut js,
            );
            spans
        };

        assert_eq!(run(&parent), run(&parent));
    }

    /// A non-retryable failure still reports the trace context of the attempt
    /// that failed — the operator can find the exact span that stopped the loop.
    #[test]
    fn test_trace_available_on_non_retryable_failure() {
        let config = RetryConfig::new(5, 1, 10, 1);
        let parent = TraceContext::root_from_seed("retry-permanent");
        let mut js = MockJitterSource::new(vec![0]);
        let mut spans: Vec<String> = Vec::new();

        let result = retry_with_backoff_traced(
            &config,
            &parent,
            |_attempt, trace| {
                spans.push(trace.span_id().to_string());
                Err::<i32, _>(TestError::Permanent)
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );

        assert_eq!(result, Err(TestError::Permanent));
        assert_eq!(spans.len(), 1, "permanent error must not retry");
        assert_eq!(spans[0], parent.child_for_attempt(0).span_id());
    }

    /// The traced wrapper preserves the retry semantics of the plain version:
    /// same attempt count and same backoff delays.
    #[test]
    fn test_traced_matches_untraced_retry_behaviour() {
        let config = RetryConfig::with_strategy(
            4,
            100,
            10_000,
            2,
            BackoffStrategy::Exponential,
            JitterPolicy::None,
        );
        let parent = TraceContext::root_from_seed("retry-parity");

        let mut plain_delays: Vec<u64> = Vec::new();
        let mut js = MockJitterSource::new(vec![0]);
        let plain = retry_with_backoff(
            &config,
            |_| Err::<i32, _>(TestError::Transient),
            is_retryable_test,
            |ms| plain_delays.push(ms),
            &mut js,
        );

        let mut traced_delays: Vec<u64> = Vec::new();
        let mut js = MockJitterSource::new(vec![0]);
        let traced = retry_with_backoff_traced(
            &config,
            &parent,
            |_, _| Err::<i32, _>(TestError::Transient),
            is_retryable_test,
            |ms| traced_delays.push(ms),
            &mut js,
        );

        assert_eq!(plain, traced);
        assert_eq!(plain_delays, traced_delays);
    }

    /// The attempt that finally succeeds is identifiable by its span, so an
    /// operator can tell which try completed the request.
    #[test]
    fn test_successful_attempt_span_is_identifiable() {
        let config = RetryConfig::new(5, 1, 10, 1);
        let parent = TraceContext::root_from_seed("retry-success-span");
        let mut js = MockJitterSource::new(vec![0]);

        let result = retry_with_backoff_traced(
            &config,
            &parent,
            |attempt, trace| {
                if attempt < 2 {
                    Err(TestError::Transient)
                } else {
                    Ok(trace.span_id().to_string())
                }
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );

        assert_eq!(result, Ok(parent.child_for_attempt(2).span_id().to_string()));
    }

    /// A retry loop nested inside another traced step keeps the outermost
    /// trace ID — the case that matters for webhook delivery inside a request.
    #[test]
    fn test_trace_survives_nested_retry_loops() {
        let config = RetryConfig::new(2, 1, 10, 1);
        let request = TraceContext::root_from_seed("outer-request");
        let delivery = request.child("webhook-delivery");
        let mut js = MockJitterSource::new(vec![0]);
        let mut seen: Vec<String> = Vec::new();

        let _ = retry_with_backoff_traced(
            &config,
            &delivery,
            |_, trace| {
                seen.push(trace.trace_id().to_string());
                Err::<i32, _>(TestError::Transient)
            },
            is_retryable_test,
            |_| {},
            &mut js,
        );

        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|id| id == request.trace_id()));
    }
}
