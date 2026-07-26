//! Formal session lifecycle state machine.
//!
//! Every session passes through a well-defined sequence of states. This module
//! owns the transition table and enforces it centrally so no caller can put a
//! session into an invalid state.
//!
//! ## State diagram
//!
//! ```text
//!  [Created] ──(operations)──► [Active] ──(limit reached)──► [Exhausted]
//!      │                          │                               │
//!      │              (close_session)                  (close_session)
//!      │                          │                               │
//!      └──(close_session)──► [Closed] ◄─────────────────────────┘
//!
//!  Any state ──(ttl elapsed)──► [Expired]   (read-only, no writes)
//! ```
//!
//! Terminal states: `Closed`, `Expired`, `Exhausted`.
//! Transitions into terminal states are one-way — they cannot be undone.

use soroban_sdk::contracterror;

// ---------------------------------------------------------------------------
// SessionState
// ---------------------------------------------------------------------------

/// All possible lifecycle states of an on-chain session.
///
/// Stored as a `u32` discriminant inside the [`Session`](crate::contract::Session)
/// record so the state survives upgrades without layout changes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum SessionState {
    /// Session has been created but no operations have been recorded yet.
    Created    = 0,
    /// At least one operation has been recorded; more operations are allowed.
    Active     = 1,
    /// The per-session operation limit has been reached; no new operations
    /// are permitted but the session has not been explicitly closed yet.
    Exhausted  = 2,
    /// The session was explicitly closed by its initiator.
    Closed     = 3,
    /// The session TTL elapsed before it was closed.
    Expired    = 4,
}

impl SessionState {
    /// Convert the stored `u32` discriminant back to a `SessionState`.
    /// Unknown values map to `Expired` as the safest terminal state.
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => SessionState::Created,
            1 => SessionState::Active,
            2 => SessionState::Exhausted,
            3 => SessionState::Closed,
            _ => SessionState::Expired,
        }
    }

    /// `true` for states from which no further operations or transitions are
    /// possible.
    pub fn is_terminal(self) -> bool {
        matches!(self, SessionState::Closed | SessionState::Expired | SessionState::Exhausted)
    }

    /// `true` when operations may still be recorded in this session.
    pub fn accepts_operations(self) -> bool {
        matches!(self, SessionState::Created | SessionState::Active)
    }
}

// ---------------------------------------------------------------------------
// Transition table
// ---------------------------------------------------------------------------

/// All legal `(from, to)` state transitions.
///
/// Any pair not listed here is illegal and will be rejected by
/// [`validate_transition`].
const LEGAL_TRANSITIONS: &[(SessionState, SessionState)] = &[
    // Normal operation flow
    (SessionState::Created,   SessionState::Active),
    (SessionState::Created,   SessionState::Exhausted),
    // Close from any non-terminal state
    (SessionState::Created,   SessionState::Closed),
    (SessionState::Active,    SessionState::Closed),
    (SessionState::Exhausted, SessionState::Closed),
    // Exhaustion from active
    (SessionState::Active,    SessionState::Exhausted),
    // Expiry from any non-terminal state (time-driven, not user-driven)
    (SessionState::Created,   SessionState::Expired),
    (SessionState::Active,    SessionState::Expired),
    (SessionState::Exhausted, SessionState::Expired),
];

/// Return `true` when the `from → to` transition is legal.
pub fn is_legal_transition(from: SessionState, to: SessionState) -> bool {
    LEGAL_TRANSITIONS.iter().any(|&(f, t)| f == from && t == to)
}

/// Validate a `from → to` transition.
///
/// Returns `Ok(())` when the transition is legal.
/// Returns `Err(SessionTransitionError)` describing the failure otherwise.
pub fn validate_transition(
    from: SessionState,
    to: SessionState,
) -> Result<(), SessionTransitionError> {
    if from == to {
        return Err(SessionTransitionError::SameState);
    }
    if from.is_terminal() {
        return Err(SessionTransitionError::FromTerminal);
    }
    if !is_legal_transition(from, to) {
        return Err(SessionTransitionError::IllegalTransition);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Reason a session state transition was rejected.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SessionTransitionError {
    /// The source and target state are the same.
    SameState,
    /// The source state is terminal — no further transitions are possible.
    FromTerminal,
    /// The `from → to` pair is not in the legal transition table.
    IllegalTransition,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // SessionState helpers
    // -----------------------------------------------------------------------

    #[test]
    fn test_from_u32_roundtrip() {
        let cases = [
            (0u32, SessionState::Created),
            (1u32, SessionState::Active),
            (2u32, SessionState::Exhausted),
            (3u32, SessionState::Closed),
            (4u32, SessionState::Expired),
            (99u32, SessionState::Expired), // unknown → Expired
        ];
        for (v, expected) in cases {
            assert_eq!(SessionState::from_u32(v), expected, "from_u32({v})");
        }
    }

    #[test]
    fn test_is_terminal() {
        assert!(!SessionState::Created.is_terminal());
        assert!(!SessionState::Active.is_terminal());
        assert!(SessionState::Exhausted.is_terminal());
        assert!(SessionState::Closed.is_terminal());
        assert!(SessionState::Expired.is_terminal());
    }

    #[test]
    fn test_accepts_operations() {
        assert!(SessionState::Created.accepts_operations());
        assert!(SessionState::Active.accepts_operations());
        assert!(!SessionState::Exhausted.accepts_operations());
        assert!(!SessionState::Closed.accepts_operations());
        assert!(!SessionState::Expired.accepts_operations());
    }

    // -----------------------------------------------------------------------
    // Legal transitions
    // -----------------------------------------------------------------------

    #[test]
    fn test_legal_transitions_succeed() {
        let legal = [
            (SessionState::Created,   SessionState::Active),
            (SessionState::Created,   SessionState::Exhausted),
            (SessionState::Created,   SessionState::Closed),
            (SessionState::Active,    SessionState::Closed),
            (SessionState::Active,    SessionState::Exhausted),
            (SessionState::Exhausted, SessionState::Closed),
            (SessionState::Created,   SessionState::Expired),
            (SessionState::Active,    SessionState::Expired),
            (SessionState::Exhausted, SessionState::Expired),
        ];
        for (from, to) in legal {
            assert!(
                validate_transition(from, to).is_ok(),
                "expected {from:?} → {to:?} to be legal"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Illegal transitions
    // -----------------------------------------------------------------------

    #[test]
    fn test_same_state_is_illegal() {
        for state in [
            SessionState::Created,
            SessionState::Active,
            SessionState::Exhausted,
            SessionState::Closed,
            SessionState::Expired,
        ] {
            assert_eq!(
                validate_transition(state, state),
                Err(SessionTransitionError::SameState),
                "same-state transition for {state:?} should be SameState"
            );
        }
    }

    #[test]
    fn test_from_terminal_is_illegal() {
        let terminals = [
            SessionState::Closed,
            SessionState::Expired,
            SessionState::Exhausted,
        ];
        // Any non-same target from a terminal should give FromTerminal.
        let targets = [
            SessionState::Created,
            SessionState::Active,
            SessionState::Closed,
            SessionState::Expired,
        ];
        for from in terminals {
            for to in targets {
                if from == to { continue; }
                assert_eq!(
                    validate_transition(from, to),
                    Err(SessionTransitionError::FromTerminal),
                    "{from:?} → {to:?} should be FromTerminal"
                );
            }
        }
    }

    #[test]
    fn test_active_cannot_go_to_created() {
        assert_eq!(
            validate_transition(SessionState::Active, SessionState::Created),
            Err(SessionTransitionError::IllegalTransition)
        );
    }

    #[test]
    fn test_created_cannot_skip_to_expired_directly_via_is_legal() {
        // Created → Expired is allowed (time-driven), but Active → Created is not.
        assert!(!is_legal_transition(SessionState::Active, SessionState::Created));
        assert!(is_legal_transition(SessionState::Created, SessionState::Expired));
    }

    // -----------------------------------------------------------------------
    // Full lifecycle progressions
    // -----------------------------------------------------------------------

    #[test]
    fn test_happy_path_created_active_closed() {
        assert!(validate_transition(SessionState::Created, SessionState::Active).is_ok());
        assert!(validate_transition(SessionState::Active, SessionState::Closed).is_ok());
    }

    #[test]
    fn test_expiry_path_active_expired() {
        assert!(validate_transition(SessionState::Active, SessionState::Expired).is_ok());
    }

    #[test]
    fn test_exhaustion_then_close() {
        assert!(validate_transition(SessionState::Active, SessionState::Exhausted).is_ok());
        assert!(validate_transition(SessionState::Exhausted, SessionState::Closed).is_ok());
    }

    #[test]
    fn test_close_directly_from_created() {
        assert!(validate_transition(SessionState::Created, SessionState::Closed).is_ok());
    }
}
