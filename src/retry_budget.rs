//! Request-level retry budgets (#683).
//!
//! Global retry configuration ([`RetryConfig`]) applies uniformly to every
//! request. Under heavy load a single misbehaving request can exhaust all
//! available retry capacity, starving concurrent operations.
//!
//! This module adds a per-request retry budget that caps the total number of
//! retry attempts allowed for one logical request within a given context
//! window. Each [`RetryBudget`] tracks consumed attempts, enforces the
//! configured ceiling, and carries metadata for observability.
//!
//! # Design
//!
//! * **Per-request, not per-operation.** A budget is created for a single
//!   logical request (identified by a `request_id`) and lives only as long as
//!   that request is in flight. There is no shared global counter.
//! * **Composable with `RetryConfig`.** The effective retry limit for any
//!   single attempt is `min(config.max_attempts, budget.remaining())`. Callers
//!   hold a `RetryBudget` and consult it before each retry.
//! * **No std.** Uses `alloc` only; works in `no_std` + `alloc` environments.
//!
//! # Example
//!
//! ```rust
//! use anchorkit::retry_budget::{RetryBudget, BudgetExhaustedError};
//!
//! let mut budget = RetryBudget::new("txn-001", 3);
//!
//! assert!(budget.consume().is_ok()); // attempt 1
//! assert!(budget.consume().is_ok()); // attempt 2
//! assert!(budget.consume().is_ok()); // attempt 3
//! assert!(matches!(budget.consume(), Err(BudgetExhaustedError { .. })));
//! assert_eq!(budget.consumed(), 3);
//! assert_eq!(budget.remaining(), 0);
//! ```

extern crate alloc;

use alloc::string::{String, ToString};

// ---------------------------------------------------------------------------
// BudgetExhaustedError
// ---------------------------------------------------------------------------

/// Returned by [`RetryBudget::consume`] when the budget is fully spent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetExhaustedError {
    /// The request ID whose budget was exhausted.
    pub request_id: String,
    /// Total number of attempts that were allowed.
    pub max_attempts: u32,
}

impl core::fmt::Display for BudgetExhaustedError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "retry budget exhausted for request '{}' after {} attempts",
            self.request_id, self.max_attempts
        )
    }
}

// ---------------------------------------------------------------------------
// RetryBudget
// ---------------------------------------------------------------------------

/// Per-request retry budget.
///
/// Tracks the number of attempts consumed for a single logical request and
/// refuses additional attempts once the limit is reached.
#[derive(Clone, Debug)]
pub struct RetryBudget {
    /// Stable identifier for the logical request this budget governs.
    request_id: String,
    /// Maximum number of attempts (including the first try).
    max_attempts: u32,
    /// Attempts consumed so far.
    consumed: u32,
}

impl RetryBudget {
    /// Create a budget for `request_id` allowing at most `max_attempts` total
    /// attempts (including the first try; retries are `max_attempts - 1`).
    ///
    /// `max_attempts` is silently clamped to `1` if `0` is supplied.
    pub fn new(request_id: impl Into<String>, max_attempts: u32) -> Self {
        RetryBudget {
            request_id: request_id.into(),
            max_attempts: max_attempts.max(1),
            consumed: 0,
        }
    }

    /// The stable identifier for the request this budget governs.
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Maximum number of total attempts allowed.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Number of attempts consumed so far.
    pub fn consumed(&self) -> u32 {
        self.consumed
    }

    /// Remaining attempts before the budget is exhausted.
    pub fn remaining(&self) -> u32 {
        self.max_attempts.saturating_sub(self.consumed)
    }

    /// `true` when no remaining attempts are left.
    pub fn is_exhausted(&self) -> bool {
        self.consumed >= self.max_attempts
    }

    /// Consume one attempt from the budget.
    ///
    /// Returns `Ok(attempt_index)` (0-based) on success, or
    /// [`BudgetExhaustedError`] when the budget is already spent.
    pub fn consume(&mut self) -> Result<u32, BudgetExhaustedError> {
        if self.is_exhausted() {
            return Err(BudgetExhaustedError {
                request_id: self.request_id.clone(),
                max_attempts: self.max_attempts,
            });
        }
        let index = self.consumed;
        self.consumed += 1;
        Ok(index)
    }

    /// Reset the budget back to zero consumed attempts.
    ///
    /// Use this when the same `RetryBudget` instance is reused for a
    /// logically new invocation (e.g. a poll loop that restarts a request).
    pub fn reset(&mut self) {
        self.consumed = 0;
    }
}

// ---------------------------------------------------------------------------
// BudgetedRetry — execute with budget enforcement
// ---------------------------------------------------------------------------

/// Execute `f` respecting both a [`RetryBudget`] and an `is_retryable`
/// predicate, sleeping between attempts via `sleep_fn`.
///
/// The attempt loop stops when any of the following is true:
/// * `f` returns `Ok`.
/// * `is_retryable(&err)` returns `false`.
/// * `budget.remaining() == 0`.
///
/// Returns the last `Err` from `f`, or a budget-exhaustion error mapped to
/// `E` by `on_exhausted`, if the budget ran out before `f` succeeded.
///
/// # Example
///
/// ```rust
/// use anchorkit::retry_budget::{RetryBudget, execute_with_budget};
///
/// let mut budget = RetryBudget::new("txn-99", 3);
/// let mut calls = 0u32;
///
/// let result = execute_with_budget(
///     &mut budget,
///     |attempt| {
///         calls += 1;
///         if attempt < 2 { Err("transient") } else { Ok(42u32) }
///     },
///     |_| true,
///     |_ms| {},
///     |_exhausted| "budget_exceeded",
///     |_attempt| 100u64,
/// );
///
/// assert_eq!(result, Ok(42));
/// assert_eq!(calls, 3);
/// ```
pub fn execute_with_budget<T, E, F, S, D, Delay>(
    budget: &mut RetryBudget,
    mut f: F,
    is_retryable: impl Fn(&E) -> bool,
    mut sleep_fn: S,
    on_exhausted: D,
    delay_fn: Delay,
) -> Result<T, E>
where
    F: FnMut(u32) -> Result<T, E>,
    S: FnMut(u64),
    D: FnOnce(BudgetExhaustedError) -> E,
    Delay: Fn(u32) -> u64,
{
    loop {
        let attempt = match budget.consume() {
            Ok(idx) => idx,
            Err(exhausted) => return Err(on_exhausted(exhausted)),
        };

        match f(attempt) {
            Ok(val) => return Ok(val),
            Err(e) => {
                if !is_retryable(&e) || budget.is_exhausted() {
                    return Err(e);
                }
                sleep_fn(delay_fn(attempt));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RetryBudgetConfig — shareable policy
// ---------------------------------------------------------------------------

/// A reusable policy that stamps out fresh [`RetryBudget`]s for new requests.
///
/// Useful when a service component wants to enforce a consistent retry ceiling
/// across all outbound calls without hard-coding the limit at every call site.
#[derive(Clone, Debug)]
pub struct RetryBudgetConfig {
    /// Default maximum attempts for budgets vended by this config.
    pub max_attempts: u32,
}

impl Default for RetryBudgetConfig {
    fn default() -> Self {
        RetryBudgetConfig { max_attempts: 3 }
    }
}

impl RetryBudgetConfig {
    /// Create a config with the given attempt ceiling.
    pub fn new(max_attempts: u32) -> Self {
        RetryBudgetConfig { max_attempts }
    }

    /// Vend a fresh [`RetryBudget`] for a new request.
    pub fn budget_for(&self, request_id: impl Into<String>) -> RetryBudget {
        RetryBudget::new(request_id, self.max_attempts)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn new_budget_has_full_capacity() {
        let b = RetryBudget::new("req-1", 3);
        assert_eq!(b.remaining(), 3);
        assert_eq!(b.consumed(), 0);
        assert!(!b.is_exhausted());
    }

    #[test]
    fn consume_decrements_remaining() {
        let mut b = RetryBudget::new("req-1", 3);
        assert_eq!(b.consume(), Ok(0));
        assert_eq!(b.remaining(), 2);
        assert_eq!(b.consume(), Ok(1));
        assert_eq!(b.remaining(), 1);
        assert_eq!(b.consume(), Ok(2));
        assert_eq!(b.remaining(), 0);
        assert!(b.is_exhausted());
    }

    #[test]
    fn consume_when_exhausted_returns_error() {
        let mut b = RetryBudget::new("req-2", 2);
        let _ = b.consume();
        let _ = b.consume();
        let err = b.consume().unwrap_err();
        assert_eq!(err.max_attempts, 2);
        assert_eq!(err.request_id, "req-2");
    }

    #[test]
    fn zero_max_attempts_clamped_to_one() {
        let mut b = RetryBudget::new("req-3", 0);
        assert_eq!(b.max_attempts(), 1);
        assert!(b.consume().is_ok());
        assert!(b.consume().is_err());
    }

    #[test]
    fn reset_restores_full_capacity() {
        let mut b = RetryBudget::new("req-4", 2);
        let _ = b.consume();
        let _ = b.consume();
        assert!(b.is_exhausted());
        b.reset();
        assert!(!b.is_exhausted());
        assert_eq!(b.remaining(), 2);
    }

    #[test]
    fn budget_config_vends_fresh_budgets() {
        let cfg = RetryBudgetConfig::new(5);
        let b = cfg.budget_for("req-5");
        assert_eq!(b.max_attempts(), 5);
        assert_eq!(b.consumed(), 0);
    }

    #[test]
    fn execute_with_budget_succeeds_on_third_attempt() {
        let mut budget = RetryBudget::new("req-6", 5);
        let mut calls = 0u32;

        let result = execute_with_budget(
            &mut budget,
            |attempt| {
                calls += 1;
                if attempt < 2 { Err("transient") } else { Ok(42u32) }
            },
            |_| true,
            |_| {},
            |_| "exhausted",
            |_| 0,
        );

        assert_eq!(result, Ok(42));
        assert_eq!(calls, 3);
    }

    #[test]
    fn execute_with_budget_stops_on_non_retryable() {
        let mut budget = RetryBudget::new("req-7", 5);
        let mut calls = 0u32;

        let result = execute_with_budget(
            &mut budget,
            |_| { calls += 1; Err("permanent") },
            |_| false,
            |_| {},
            |_| "exhausted",
            |_| 0,
        );

        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn execute_with_budget_stops_on_exhaustion() {
        let mut budget = RetryBudget::new("req-8", 3);
        let mut calls = 0u32;

        let result = execute_with_budget(
            &mut budget,
            |_| { calls += 1; Err("transient") },
            |_| true,
            |_| {},
            |_| "exhausted",
            |_| 0,
        );

        assert!(result.is_err());
        assert_eq!(calls, 3);
    }

    #[test]
    fn sleep_called_between_attempts() {
        let mut budget = RetryBudget::new("req-9", 3);
        let mut sleep_calls = 0u32;

        let _ = execute_with_budget(
            &mut budget,
            |_| Err::<i32, _>("transient"),
            |_| true,
            |_| { sleep_calls += 1; },
            |_| "exhausted",
            |_| 50,
        );

        assert_eq!(sleep_calls, 2);
    }
}
