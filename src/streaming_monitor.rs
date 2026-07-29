//! Streaming transaction monitor for long-running SEP-24 interactive flows.
//!
//! [`StreamingTransactionMonitor`] polls a transaction's state at a configurable
//! interval and emits [`TransactionStatusUpdate`] events for every state change.
//! It stops automatically when the transaction reaches a terminal state.
//!
//! # State transitions
//!
//! Every state change is recorded as a [`StateTransition`] entry and can be
//! retrieved via [`StreamingTransactionMonitor::get_transitions`].
//!
//! # Backpressure
//!
//! The monitor supports configurable backpressure via [`BackpressureConfig`]:
//! - `max_queued_transitions` caps the number of retained transitions.
//! - `coalesce_updates` collapses rapid duplicate state changes to reduce noise.
//! - On overflow the oldest transitions are dropped (FIFO eviction).

extern crate alloc;

use alloc::vec::Vec;
use crate::retry::{retry_with_backoff_traced, LedgerJitterSource, RetryConfig};
use crate::trace_context::TraceContext;
use crate::transaction_state_tracker::TransactionState;

// ── PollResult ────────────────────────────────────────────────────────────────

/// Return type for `poll_fn` passed to [`StreamingTransactionMonitor::run`].
///
/// Carries extra data (e.g. `stellar_tx_id`) that plain [`TransactionState`]
/// cannot represent.
#[derive(Clone, Debug, PartialEq)]
pub enum PollResult {
    /// Transaction is still in progress.
    Pending(TransactionState),
    /// Transaction completed; `stellar_tx_id` is the on-chain Stellar tx hash.
    Completed { stellar_tx_id: alloc::string::String },
    /// Transaction failed with a human-readable reason.
    Failed { reason: alloc::string::String },
}

// ── TransactionStatusUpdate ───────────────────────────────────────────────────

/// Events emitted by [`StreamingTransactionMonitor`] as a transaction progresses.
#[derive(Clone, Debug, PartialEq)]
pub enum TransactionStatusUpdate {
    /// The transaction moved from one state to another.
    StateChanged {
        from: TransactionState,
        to: TransactionState,
        timestamp: u64,
    },
    /// A more-info URL is available (e.g. SEP-24 interactive URL).
    MoreInfoAvailable { url: alloc::string::String },
    /// The transaction completed successfully.
    Completed { stellar_tx_id: alloc::string::String },
    /// The transaction failed.
    Failed { reason: alloc::string::String },
}

// ── StateTransition ───────────────────────────────────────────────────────────

/// A recorded state transition within the streaming monitor.
///
/// Every time the polled transaction moves from one [`TransactionState`] to
/// another, a `StateTransition` entry is recorded and can be retrieved via
/// [`StreamingTransactionMonitor::get_transitions`].
#[derive(Clone, Debug, PartialEq)]
pub struct StateTransition {
    /// The state the transaction moved from.
    pub from: TransactionState,
    /// The state the transaction moved to.
    pub to: TransactionState,
    /// Timestamp (milliseconds) when the transition was detected.
    pub timestamp: u64,
    /// Trace ID of the monitoring run that observed this transition.
    ///
    /// Constant for the lifetime of one monitor, so an operator can join the
    /// recorded transitions of a long-running background poll against the
    /// request that started it.
    pub trace_id: alloc::string::String,
    /// Span ID of the poll cycle that observed this transition.
    pub span_id: alloc::string::String,
}

// ── BackpressureConfig ────────────────────────────────────────────────────────

/// Backpressure controls for the streaming monitor.
///
/// Prevents unbounded memory growth under bursty update conditions by
/// capping retained transition history and optionally coalescing rapid
/// duplicate state changes.
#[derive(Clone, Debug)]
pub struct BackpressureConfig {
    /// Maximum number of [`StateTransition`] entries retained.
    /// When exceeded, the oldest entries are evicted (FIFO).
    /// `0` means unlimited (use with caution).
    pub max_queued_transitions: usize,
    /// When `true`, consecutive duplicate `Pending` updates with the same
    /// [`TransactionState`] are collapsed into a single transition entry.
    /// The first occurrence is kept; subsequent identical states within
    /// the same poll cycle are suppressed.
    pub coalesce_updates: bool,
    /// When `true`, the monitor will also coalesce across poll cycles:
    /// if the polled state is the same as the last recorded state, no new
    /// transition entry is added.
    pub coalesce_across_polls: bool,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        BackpressureConfig {
            max_queued_transitions: 100,
            coalesce_updates: true,
            coalesce_across_polls: true,
        }
    }
}

impl BackpressureConfig {
    /// Allow unlimited transitions (no backpressure cap).
    pub fn unlimited() -> Self {
        BackpressureConfig {
            max_queued_transitions: 0,
            coalesce_updates: false,
            coalesce_across_polls: false,
        }
    }

    /// Aggressive backpressure: small queue, coalesce everything.
    pub fn aggressive() -> Self {
        BackpressureConfig {
            max_queued_transitions: 10,
            coalesce_updates: true,
            coalesce_across_polls: true,
        }
    }
}

// ── StreamingTransactionMonitor ───────────────────────────────────────────────

/// Polls a transaction and emits [`TransactionStatusUpdate`] events on state changes.
///
/// Tracks state transitions via [`StateTransition`] entries and supports
/// configurable backpressure via [`BackpressureConfig`].
///
/// # Example (pseudo-code — polling_fn is injected for testability)
///
/// ```rust,ignore
/// let mut monitor = StreamingTransactionMonitor::new(tx_id, 1000);
/// monitor.run(|id| fetch_state(id), |event| handle(event));
/// ```
pub struct StreamingTransactionMonitor {
    pub transaction_id: u64,
    /// Polling interval in milliseconds.
    pub poll_interval_ms: u64,
    retry_config: RetryConfig,
    /// Recorded state transitions (see [`StateTransition`]).
    transitions: Vec<StateTransition>,
    /// Backpressure configuration.
    backpressure: BackpressureConfig,
    /// Trace context the whole monitoring run belongs to.
    ///
    /// Defaults to a context derived from the transaction ID so a monitor
    /// started without one is still traceable; use
    /// [`with_trace`](Self::with_trace) to attach the originating request's
    /// context instead.
    trace: TraceContext,
}

impl StreamingTransactionMonitor {
    pub fn new(transaction_id: u64, poll_interval_ms: u64) -> Self {
        Self {
            transaction_id,
            poll_interval_ms,
            retry_config: RetryConfig::default(),
            transitions: Vec::new(),
            backpressure: BackpressureConfig::default(),
            trace: default_monitor_trace(transaction_id),
        }
    }

    pub fn with_retry(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// Set the backpressure configuration.
    pub fn with_backpressure(mut self, config: BackpressureConfig) -> Self {
        self.backpressure = config;
        self
    }

    /// Attach the trace context this monitoring run belongs to.
    ///
    /// The monitor records a `monitor:<transaction_id>` child span of `trace`,
    /// so a background poll that outlives the request that started it still
    /// carries that request's trace ID.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::streaming_monitor::StreamingTransactionMonitor;
    /// use anchorkit::trace_context::TraceContext;
    ///
    /// let request = TraceContext::root_from_seed("sep24:deposit-1");
    /// let monitor = StreamingTransactionMonitor::new(7, 1000).with_trace(&request);
    /// assert_eq!(monitor.trace().trace_id(), request.trace_id());
    /// ```
    pub fn with_trace(mut self, trace: &TraceContext) -> Self {
        let mut seed = alloc::string::String::from("monitor:");
        seed.push_str(&alloc::format!("{}", self.transaction_id));
        self.trace = trace.child(&seed);
        self
    }

    /// The trace context this monitor runs under.
    pub fn trace(&self) -> &TraceContext {
        &self.trace
    }

    /// Return a copy of all recorded state transitions since the monitor started
    /// (or since the last [`clear_transitions`](Self::clear_transitions) call).
    pub fn get_transitions(&self) -> Vec<StateTransition> {
        self.transitions.clone()
    }

    /// Clear all recorded state transitions.
    pub fn clear_transitions(&mut self) {
        self.transitions.clear();
    }

    /// Record a transition, respecting the backpressure config (cap + coalescing).
    ///
    /// `poll_trace` is the context of the poll cycle that observed the change;
    /// its identifiers are stored on the entry so the transition history stays
    /// joinable with the delivery and retry logs.
    ///
    /// Returns `true` if the transition was recorded, `false` if it was
    /// coalesced away because it duplicates the most recent entry.
    fn record_transition(
        &mut self,
        from: TransactionState,
        to: TransactionState,
        timestamp: u64,
        poll_trace: &TraceContext,
    ) -> bool {
        if self.backpressure.coalesce_across_polls {
            if let Some(last) = self.transitions.last() {
                if last.from == from && last.to == to {
                    return false;
                }
            }
        }

        self.transitions.push(StateTransition {
            from,
            to,
            timestamp,
            trace_id: poll_trace.trace_id().into(),
            span_id: poll_trace.span_id().into(),
        });

        if self.backpressure.max_queued_transitions > 0 {
            while self.transitions.len() > self.backpressure.max_queued_transitions {
                self.transitions.remove(0);
            }
        }

        true
    }

    /// Run the monitor.
    ///
    /// - `poll_fn`: given a transaction ID, returns `Ok(PollResult)` or `Err(String)`.
    /// - `on_event`: called for every [`TransactionStatusUpdate`] emitted.
    /// - `sleep_fn`: called with the poll interval (ms) between polls; inject `|_| {}` in tests.
    /// - `timestamp_fn`: called when emitting `StateChanged` events to obtain the current time.
    ///
    /// Returns when the transaction reaches a terminal state or all retries are exhausted.
    pub fn run<P, E, S, T>(
        &mut self,
        mut poll_fn: P,
        on_event: E,
        sleep_fn: S,
        timestamp_fn: T,
    ) where
        P: FnMut(u64) -> Result<PollResult, alloc::string::String>,
        E: FnMut(TransactionStatusUpdate),
        S: FnMut(u64),
        T: Fn() -> u64,
    {
        self.run_traced(
            |id, _trace| poll_fn(id),
            on_event,
            sleep_fn,
            timestamp_fn,
        )
    }

    /// Run the monitor, handing each poll its trace context.
    ///
    /// Identical to [`run`](Self::run) except that `poll_fn` also receives the
    /// [`TraceContext`] of the attempt it is serving, so the outbound status
    /// request can carry `traceparent` headers. Every poll cycle is a child
    /// span of the monitor span, and every retry within a cycle is a child of
    /// that — all sharing the monitor's trace ID for the lifetime of the run.
    ///
    /// - `poll_fn`: given a transaction ID and the attempt's trace context,
    ///   returns `Ok(PollResult)` or `Err(String)`.
    /// - `on_event`: called for every [`TransactionStatusUpdate`] emitted.
    /// - `sleep_fn`: called with the poll interval (ms) between polls.
    /// - `timestamp_fn`: called when emitting `StateChanged` events.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anchorkit::streaming_monitor::{PollResult, StreamingTransactionMonitor};
    /// use anchorkit::trace_context::TraceContext;
    ///
    /// let request = TraceContext::root_from_seed("sep24:deposit-1");
    /// let mut monitor = StreamingTransactionMonitor::new(7, 0).with_trace(&request);
    /// let mut seen = Vec::new();
    ///
    /// monitor.run_traced(
    ///     |_id, trace| {
    ///         seen.push(trace.trace_id().to_string());
    ///         Ok(PollResult::Completed { stellar_tx_id: "abc".into() })
    ///     },
    ///     |_event| {},
    ///     |_ms| {},
    ///     || 0,
    /// );
    ///
    /// assert!(seen.iter().all(|id| id == request.trace_id()));
    /// ```
    pub fn run_traced<P, E, S, T>(
        &mut self,
        mut poll_fn: P,
        mut on_event: E,
        mut sleep_fn: S,
        timestamp_fn: T,
    ) where
        P: FnMut(u64, &TraceContext) -> Result<PollResult, alloc::string::String>,
        E: FnMut(TransactionStatusUpdate),
        S: FnMut(u64),
        T: Fn() -> u64,
    {
        let mut last_state: Option<TransactionState> = None;
        let mut jitter = LedgerJitterSource::new(
            self.transaction_id as u32,
            timestamp_fn(),
        );

        let monitor_trace = self.trace.clone();
        let transaction_id = self.transaction_id;
        let retry_config = self.retry_config.clone();
        // Poll cycles are numbered so each one is its own span; the counter is
        // what makes a long-running background monitor readable in a trace.
        let mut cycle: u32 = 0;

        loop {
            let cycle_trace = monitor_trace.child_for_attempt(cycle);
            cycle = cycle.wrapping_add(1);

            let result = retry_with_backoff_traced(
                &retry_config,
                &cycle_trace,
                |_, attempt_trace| poll_fn(transaction_id, attempt_trace),
                |_| true,
                |ms| sleep_fn(ms),
                &mut jitter,
            );

            match result {
                Err(reason) => {
                    if let Some(prev) = last_state {
                        let ts = timestamp_fn();
                        self.record_transition(prev, TransactionState::Failed, ts, &cycle_trace);
                        on_event(TransactionStatusUpdate::StateChanged {
                            from: prev,
                            to: TransactionState::Failed,
                            timestamp: ts,
                        });
                    }
                    on_event(TransactionStatusUpdate::Failed { reason });
                    return;
                }
                Ok(PollResult::Failed { reason }) => {
                    if let Some(prev) = last_state {
                        let ts = timestamp_fn();
                        self.record_transition(prev, TransactionState::Failed, ts, &cycle_trace);
                        on_event(TransactionStatusUpdate::StateChanged {
                            from: prev,
                            to: TransactionState::Failed,
                            timestamp: ts,
                        });
                    }
                    on_event(TransactionStatusUpdate::Failed { reason });
                    return;
                }
                Ok(PollResult::Completed { stellar_tx_id }) => {
                    if let Some(prev) = last_state {
                        let ts = timestamp_fn();
                        self.record_transition(prev, TransactionState::Completed, ts, &cycle_trace);
                        on_event(TransactionStatusUpdate::StateChanged {
                            from: prev,
                            to: TransactionState::Completed,
                            timestamp: ts,
                        });
                    }
                    on_event(TransactionStatusUpdate::Completed { stellar_tx_id });
                    return;
                }
                Ok(PollResult::Pending(current_state)) => {
                    let will_coalesce = self.backpressure.coalesce_updates
                        && last_state == Some(current_state);

                    if let Some(prev) = last_state {
                        if prev != current_state {
                            let ts = timestamp_fn();
                            self.record_transition(prev, current_state, ts, &cycle_trace);
                            on_event(TransactionStatusUpdate::StateChanged {
                                from: prev,
                                to: current_state,
                                timestamp: ts,
                            });
                        }
                    } else {
                        let ts = timestamp_fn();
                        self.record_transition(
                            TransactionState::Pending,
                            current_state,
                            ts,
                            &cycle_trace,
                        );
                    }

                    if !will_coalesce {
                        last_state = Some(current_state);
                    }

                    if current_state == TransactionState::Failed {
                        on_event(TransactionStatusUpdate::Failed {
                            reason: alloc::string::String::from("transaction failed"),
                        });
                        return;
                    }
                }
            }

            sleep_fn(self.poll_interval_ms);
        }
    }
}

/// Trace context used by a monitor that was not given one.
///
/// Derived from the transaction ID so repeated monitoring of the same
/// transaction shares a trace ID, and so background work is never untraced.
fn default_monitor_trace(transaction_id: u64) -> TraceContext {
    TraceContext::root_from_seed(&alloc::format!("monitor:{}", transaction_id))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction_state_tracker::TransactionState;

    #[test]
    fn test_monitor_emits_state_change_events() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0);
        let states = alloc::vec![
            PollResult::Pending(TransactionState::Pending),
            PollResult::Pending(TransactionState::InProgress),
            PollResult::Completed { stellar_tx_id: alloc::string::String::from("abc") },
        ];
        let mut idx = 0usize;
        let mut events: alloc::vec::Vec<TransactionStatusUpdate> = alloc::vec::Vec::new();

        monitor.run(
            |_| {
                let s = states[idx.min(states.len() - 1)].clone();
                idx += 1;
                Ok(s)
            },
            |e| events.push(e),
            |_| {},
            || 1000,
        );

        assert!(events.iter().any(|e| matches!(e,
            TransactionStatusUpdate::StateChanged { from: TransactionState::Pending, to: TransactionState::InProgress, .. }
        )));
        assert!(events.iter().any(|e| matches!(e,
            TransactionStatusUpdate::StateChanged { from: TransactionState::InProgress, to: TransactionState::Completed, .. }
        )));
        assert!(events.iter().any(|e| matches!(e, TransactionStatusUpdate::Completed { stellar_tx_id } if stellar_tx_id == "abc")));
    }

    #[test]
    fn test_monitor_stops_on_completed() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0);
        let mut call_count = 0u32;
        let mut events: alloc::vec::Vec<TransactionStatusUpdate> = alloc::vec::Vec::new();

        monitor.run(
            |_| {
                call_count += 1;
                Ok(PollResult::Completed { stellar_tx_id: alloc::string::String::from("tx1") })
            },
            |e| events.push(e),
            |_| {},
            || 0,
        );

        assert_eq!(call_count, 1);
        assert!(events.iter().any(|e| matches!(e, TransactionStatusUpdate::Completed { stellar_tx_id } if stellar_tx_id == "tx1")));
    }

    #[test]
    fn test_monitor_stops_on_failed() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0);
        let mut events: alloc::vec::Vec<TransactionStatusUpdate> = alloc::vec::Vec::new();

        monitor.run(
            |_| Ok(PollResult::Pending(TransactionState::Failed)),
            |e| events.push(e),
            |_| {},
            || 0,
        );

        assert!(events.iter().any(|e| matches!(e, TransactionStatusUpdate::Failed { .. })));
    }

    #[test]
    fn test_monitor_retries_on_poll_failure() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0);
        let mut call_count = 0u32;
        let mut events: alloc::vec::Vec<TransactionStatusUpdate> = alloc::vec::Vec::new();

        monitor.run(
            |_| {
                call_count += 1;
                if call_count < 3 {
                    Err(alloc::string::String::from("transient"))
                } else {
                    Ok(PollResult::Completed { stellar_tx_id: alloc::string::String::from("tx99") })
                }
            },
            |e| events.push(e),
            |_| {},
            || 0,
        );

        assert!(events.iter().any(|e| matches!(e, TransactionStatusUpdate::Completed { .. })));
    }

    #[test]
    fn test_monitor_emits_failed_when_all_retries_exhausted() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0)
            .with_retry(RetryConfig::new(2, 0, 0, 1));
        let mut events: alloc::vec::Vec<TransactionStatusUpdate> = alloc::vec::Vec::new();

        monitor.run(
            |_| Err(alloc::string::String::from("permanent error")),
            |e| events.push(e),
            |_| {},
            || 0,
        );

        assert!(events.iter().any(|e| matches!(e, TransactionStatusUpdate::Failed { .. })));
    }

    #[test]
    fn test_poll_interval_is_configurable() {
        let monitor = StreamingTransactionMonitor::new(42, 500);
        assert_eq!(monitor.poll_interval_ms, 500);
    }

    #[test]
    fn test_state_changed_timestamp_uses_timestamp_fn() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0);
        let mut events: alloc::vec::Vec<TransactionStatusUpdate> = alloc::vec::Vec::new();
        let states = alloc::vec![
            PollResult::Pending(TransactionState::Pending),
            PollResult::Pending(TransactionState::InProgress),
            PollResult::Completed { stellar_tx_id: alloc::string::String::new() },
        ];
        let mut idx = 0usize;

        monitor.run(
            |_| { let s = states[idx.min(states.len()-1)].clone(); idx += 1; Ok(s) },
            |e| events.push(e),
            |_| {},
            || 9999,
        );

        for e in &events {
            if let TransactionStatusUpdate::StateChanged { timestamp, .. } = e {
                assert_eq!(*timestamp, 9999);
            }
        }
    }

    #[test]
    fn test_completed_carries_stellar_tx_id() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0);
        let mut events: alloc::vec::Vec<TransactionStatusUpdate> = alloc::vec::Vec::new();

        monitor.run(
            |_| Ok(PollResult::Completed { stellar_tx_id: alloc::string::String::from("HASH123") }),
            |e| events.push(e),
            |_| {},
            || 0,
        );

        assert!(events.iter().any(|e| matches!(e,
            TransactionStatusUpdate::Completed { stellar_tx_id } if stellar_tx_id == "HASH123"
        )));
    }

    // -----------------------------------------------------------------------
    // Issue #624 — state transition tracking tests
    // -----------------------------------------------------------------------

    /// State transitions are recorded and retrievable.
    #[test]
    fn test_transitions_are_recorded() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0)
            .with_backpressure(BackpressureConfig::unlimited());
        let states = alloc::vec![
            PollResult::Pending(TransactionState::Pending),
            PollResult::Pending(TransactionState::InProgress),
            PollResult::Completed { stellar_tx_id: alloc::string::String::from("tx99") },
        ];
        let mut idx = 0usize;

        monitor.run(
            |_| { let s = states[idx.min(states.len()-1)].clone(); idx += 1; Ok(s) },
            |_| {},
            |_| {},
            || 42,
        );

        let transitions = monitor.get_transitions();
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].from, TransactionState::Pending);
        assert_eq!(transitions[0].to, TransactionState::InProgress);
        assert_eq!(transitions[0].timestamp, 42);
        assert_eq!(transitions[1].from, TransactionState::InProgress);
        assert_eq!(transitions[1].to, TransactionState::Completed);
    }

    /// Transitions to Failed are recorded.
    #[test]
    fn test_transition_to_failed_is_recorded() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0)
            .with_backpressure(BackpressureConfig::unlimited());
        monitor.run(
            |_| Ok(PollResult::Pending(TransactionState::Failed)),
            |_| {},
            |_| {},
            || 100,
        );

        let transitions = monitor.get_transitions();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].from, TransactionState::Pending);
        assert_eq!(transitions[0].to, TransactionState::Failed);
    }

    /// get_transitions returns a snapshot; clear_transitions resets.
    #[test]
    fn test_clear_transitions() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0)
            .with_backpressure(BackpressureConfig::unlimited());
        monitor.run(
            |_| Ok(PollResult::Pending(TransactionState::Failed)),
            |_| {},
            |_| {},
            || 0,
        );

        assert_eq!(monitor.get_transitions().len(), 1);
        monitor.clear_transitions();
        assert_eq!(monitor.get_transitions().len(), 0);
    }

    /// Transition tracking works with Failed from poll error.
    #[test]
    fn test_transition_on_poll_failure() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0)
            .with_retry(RetryConfig::new(1, 0, 0, 1))
            .with_backpressure(BackpressureConfig::unlimited());

        // First call returns Pending to establish last_state, then fails
        let mut call = 0u32;
        monitor.run(
            |_| {
                call += 1;
                if call == 1 {
                    Ok(PollResult::Pending(TransactionState::InProgress))
                } else {
                    Err(alloc::string::String::from("fail"))
                }
            },
            |_| {},
            |_| {},
            || 0,
        );

        let transitions = monitor.get_transitions();
        assert_eq!(transitions.len(), 2);
        assert_eq!(transitions[0].to, TransactionState::InProgress);
        assert_eq!(transitions[1].to, TransactionState::Failed);
    }

    // -----------------------------------------------------------------------
    // Issue #625 — backpressure control tests
    // -----------------------------------------------------------------------

    /// Max queued transitions cap is enforced.
    #[test]
    fn test_backpressure_caps_transitions() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0)
            .with_backpressure(BackpressureConfig {
                max_queued_transitions: 2,
                coalesce_updates: false,
                coalesce_across_polls: false,
            });

        // Walk through multiple states to generate >2 transitions.
        let states = alloc::vec![
            PollResult::Pending(TransactionState::Pending),
            PollResult::Pending(TransactionState::InProgress),
            PollResult::Pending(TransactionState::Failed),
        ];
        let mut idx = 0usize;
        monitor.run(
            |_| { let s = states[idx.min(states.len()-1)].clone(); idx += 1; Ok(s) },
            |_| {},
            |_| {},
            || 0,
        );

        let transitions = monitor.get_transitions();
        assert_eq!(transitions.len(), 2);
        // The oldest transition (Pending -> InProgress) was evicted;
        // only InProgress -> Failed remains plus the initial Pending -> Pending.
        assert_eq!(transitions[transitions.len() - 1].to, TransactionState::Failed);
    }

    /// Coalesce updates: consecutive same-state Pending results produce only
    /// one transition entry.
    #[test]
    fn test_backpressure_coalesces_duplicate_state() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0)
            .with_backpressure(BackpressureConfig {
                max_queued_transitions: 100,
                coalesce_updates: true,
                coalesce_across_polls: false,
            });

        let states = alloc::vec![
            PollResult::Pending(TransactionState::Pending),
            PollResult::Pending(TransactionState::Pending), // duplicate
            PollResult::Pending(TransactionState::InProgress),
            PollResult::Completed { stellar_tx_id: alloc::string::String::from("tx") },
        ];
        let mut idx = 0usize;
        monitor.run(
            |_| { let s = states[idx.min(states.len()-1)].clone(); idx += 1; Ok(s) },
            |_| {},
            |_| {},
            || 0,
        );

        let transitions = monitor.get_transitions();
        // Only the first Pending and the InProgress change should be recorded
        // (the duplicate Pending should be coalesced away).
        assert_eq!(transitions.len(), 2);
    }

    /// Coalesce across polls: repeated polls with same state don't add entries.
    #[test]
    fn test_backpressure_coalesces_across_polls() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0)
            .with_backpressure(BackpressureConfig {
                max_queued_transitions: 100,
                coalesce_updates: true,
                coalesce_across_polls: true,
            });

        let states = alloc::vec![
            PollResult::Pending(TransactionState::Pending),
            PollResult::Pending(TransactionState::InProgress),
            PollResult::Pending(TransactionState::InProgress), // same as last — coalesced
            PollResult::Completed { stellar_tx_id: alloc::string::String::from("tx") },
        ];
        let mut idx = 0usize;
        monitor.run(
            |_| { let s = states[idx.min(states.len()-1)].clone(); idx += 1; Ok(s) },
            |_| {},
            |_| {},
            || 0,
        );

        let transitions = monitor.get_transitions();
        // Only 2 transitions: Pending->InProgress, InProgress->Completed
        assert_eq!(transitions.len(), 2);
    }

    /// Aggressive backpressure preset.
    #[test]
    fn test_aggressive_backpressure_preset() {
        let bp = BackpressureConfig::aggressive();
        assert_eq!(bp.max_queued_transitions, 10);
        assert!(bp.coalesce_updates);
        assert!(bp.coalesce_across_polls);
    }

    /// Unlimited backpressure preset stores all transitions.
    #[test]
    fn test_unlimited_backpressure() {
        let mut monitor = StreamingTransactionMonitor::new(1, 0)
            .with_backpressure(BackpressureConfig::unlimited());

        let states = alloc::vec![
            PollResult::Pending(TransactionState::Pending),
            PollResult::Pending(TransactionState::InProgress),
            PollResult::Pending(TransactionState::InProgress),
            PollResult::Pending(TransactionState::InProgress),
            PollResult::Completed { stellar_tx_id: alloc::string::String::from("tx") },
        ];
        let mut idx = 0usize;
        monitor.run(
            |_| { let s = states[idx.min(states.len()-1)].clone(); idx += 1; Ok(s) },
            |_| {},
            |_| {},
            || 0,
        );

        let transitions = monitor.get_transitions();
        // Unlimited + no coalesce = all polled states may create entries
        assert!(transitions.len() >= 2);
    }

    /// with_backpressure builder.
    #[test]
    fn test_with_backpressure_builder() {
        let bp = BackpressureConfig { max_queued_transitions: 5, coalesce_updates: false, coalesce_across_polls: false };
        let monitor = StreamingTransactionMonitor::new(1, 0).with_backpressure(bp.clone());
        assert_eq!(monitor.backpressure.max_queued_transitions, 5);
    }

    // ── Issue #610 — trace context across background monitoring ───────────────

    use alloc::string::{String, ToString};

    /// A monitor started under a request keeps that request's trace ID for
    /// every poll, across every poll cycle and every retry within a cycle.
    #[test]
    fn test_trace_survives_polls_and_retries() {
        let request = TraceContext::root_from_seed("sep24:deposit-1");
        let mut monitor = StreamingTransactionMonitor::new(7, 0).with_trace(&request);
        let mut seen: Vec<String> = Vec::new();
        let mut calls = 0u32;

        monitor.run_traced(
            |_id, trace| {
                seen.push(trace.trace_id().to_string());
                calls += 1;
                match calls {
                    // First cycle: two transient failures then a pending state,
                    // exercising the retry path inside a poll cycle.
                    1 | 2 => Err(String::from("transient")),
                    3 => Ok(PollResult::Pending(TransactionState::InProgress)),
                    _ => Ok(PollResult::Completed {
                        stellar_tx_id: String::from("tx-1"),
                    }),
                }
            },
            |_e| {},
            |_| {},
            || 1_000,
        );

        assert!(seen.len() >= 4, "expected retries and multiple cycles: {seen:?}");
        assert!(
            seen.iter().all(|id| id == request.trace_id()),
            "trace_id must survive background polling: {seen:?}"
        );
    }

    /// Each poll cycle is its own span, and retries within a cycle are children
    /// of that cycle — so the trace shows cycles and attempts separately.
    #[test]
    fn test_poll_cycles_and_attempts_have_distinct_spans() {
        let request = TraceContext::root_from_seed("sep24:deposit-2");
        let mut monitor = StreamingTransactionMonitor::new(7, 0).with_trace(&request);
        let monitor_span = monitor.trace().clone();
        let mut spans: Vec<String> = Vec::new();
        let mut parents: Vec<String> = Vec::new();
        let mut calls = 0u32;

        monitor.run_traced(
            |_id, trace| {
                spans.push(trace.span_id().to_string());
                parents.push(trace.parent_span_id().unwrap_or_default().to_string());
                calls += 1;
                match calls {
                    1 => Ok(PollResult::Pending(TransactionState::Pending)),
                    2 => Ok(PollResult::Pending(TransactionState::InProgress)),
                    _ => Ok(PollResult::Completed {
                        stellar_tx_id: String::from("tx-2"),
                    }),
                }
            },
            |_e| {},
            |_| {},
            || 1_000,
        );

        assert_eq!(spans.len(), 3);
        assert_ne!(spans[0], spans[1], "each poll cycle gets its own span");
        assert_ne!(spans[1], spans[2]);
        // Cycle N's attempt span is parented to cycle N's span, which is itself
        // a child of the monitor span.
        let cycle_spans: Vec<String> = (0..3u32)
            .map(|c| monitor_span.child_for_attempt(c).span_id().to_string())
            .collect();
        assert_eq!(parents, cycle_spans);
    }

    /// Recorded transitions carry the trace, so the transition history can be
    /// joined against the delivery and retry logs.
    #[test]
    fn test_recorded_transitions_carry_the_trace() {
        let request = TraceContext::root_from_seed("sep24:deposit-3");
        let mut monitor = StreamingTransactionMonitor::new(7, 0).with_trace(&request);
        let mut calls = 0u32;

        monitor.run_traced(
            |_id, _trace| {
                calls += 1;
                match calls {
                    1 => Ok(PollResult::Pending(TransactionState::Pending)),
                    2 => Ok(PollResult::Pending(TransactionState::InProgress)),
                    _ => Ok(PollResult::Completed {
                        stellar_tx_id: String::from("tx-3"),
                    }),
                }
            },
            |_e| {},
            |_| {},
            || 1_000,
        );

        let transitions = monitor.get_transitions();
        assert!(!transitions.is_empty());
        assert!(
            transitions.iter().all(|t| t.trace_id == request.trace_id()),
            "every transition should name the originating trace"
        );
        assert!(transitions.iter().all(|t| !t.span_id.is_empty()));
    }

    /// A monitor with no caller-supplied context still gets a valid, stable one,
    /// so background work is never untraced.
    #[test]
    fn test_monitor_without_caller_trace_is_still_traced() {
        let a = StreamingTransactionMonitor::new(42, 0);
        let b = StreamingTransactionMonitor::new(42, 0);
        let c = StreamingTransactionMonitor::new(43, 0);

        assert_eq!(a.trace().trace_id(), b.trace().trace_id());
        assert_ne!(a.trace().trace_id(), c.trace().trace_id());
        assert_eq!(
            a.trace().trace_id().len(),
            crate::trace_context::TRACE_ID_HEX_LEN
        );
    }

    /// `with_trace` re-parents the monitor under the caller's request.
    #[test]
    fn test_with_trace_reparents_the_monitor_span() {
        let request = TraceContext::root_from_seed("sep24:deposit-4");
        let monitor = StreamingTransactionMonitor::new(7, 0).with_trace(&request);
        assert_eq!(monitor.trace().trace_id(), request.trace_id());
        assert_eq!(monitor.trace().parent_span_id(), Some(request.span_id()));
    }

    /// The untraced `run` still drives `run_traced` underneath, so its polls
    /// are traced too.
    #[test]
    fn test_untraced_run_still_records_traced_transitions() {
        let mut monitor = StreamingTransactionMonitor::new(9, 0);
        let expected_trace = monitor.trace().trace_id().to_string();
        let mut calls = 0u32;

        monitor.run(
            |_id| {
                calls += 1;
                match calls {
                    1 => Ok(PollResult::Pending(TransactionState::Pending)),
                    _ => Ok(PollResult::Completed {
                        stellar_tx_id: String::from("tx-4"),
                    }),
                }
            },
            |_e| {},
            |_| {},
            || 1_000,
        );

        let transitions = monitor.get_transitions();
        assert!(!transitions.is_empty());
        assert!(transitions.iter().all(|t| t.trace_id == expected_trace));
    }
}
