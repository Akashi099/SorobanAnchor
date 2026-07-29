//! Composable compliance policy engine.
//!
//! Centralises all compliance evaluation logic that was previously duplicated
//! across `route_transaction`, `accept_quote_with_compliance`, and
//! `submit_attestation_kyc_check`.
//!
//! # Design
//!
//! A [`PolicyEngine`] holds an ordered list of [`PolicyRule`]s. Calling
//! [`PolicyEngine::evaluate`] with a [`PolicyContext`] runs every rule and
//! returns a [`PolicyDecision`] that indicates whether the request is allowed
//! or denied, along with the specific rule that caused a denial.
//!
//! Rules are pure functions — they do not write to storage. The contract
//! methods remain responsible for all storage mutations; they merely delegate
//! the *decision* to this engine.

extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// PolicyContext — the data a rule inspects
// ---------------------------------------------------------------------------

/// KYC approval status passed into the policy engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KycState {
    NotSubmitted,
    Pending,
    Approved,
    Rejected,
    Expired,
    Reopened,
}

impl KycState {
    /// Returns `true` only for `Approved`.
    pub fn is_approved(self) -> bool {
        matches!(self, KycState::Approved)
    }
}

/// Everything a [`PolicyRule`] can inspect when evaluating a request.
///
/// Fields are `Option` so callers can omit signals that are not relevant to
/// the entry point being checked (e.g. attestation submission does not have
/// a quote score).
#[derive(Debug, Clone)]
pub struct PolicyContext {
    /// KYC status of the subject making the request.
    pub kyc_state: KycState,
    /// Whether an explicit compliance check record (`check_type = "kyc"`)
    /// exists and passed (`result == 1`) for the subject.
    pub compliance_check_passed: bool,
    /// Optional minimum compliance score threshold from the global policy.
    pub minimum_score: Option<u32>,
    /// The subject's most recent compliance score (if any was recorded).
    pub subject_score: Option<u32>,
    /// Whether the caller requested KYC enforcement.
    pub require_kyc: bool,
    /// Whether the caller requested general compliance enforcement.
    pub require_compliance: bool,
}

impl PolicyContext {
    /// Convenience constructor with all enforcement flags disabled.
    pub fn permissive() -> Self {
        PolicyContext {
            kyc_state: KycState::NotSubmitted,
            compliance_check_passed: false,
            minimum_score: None,
            subject_score: None,
            require_kyc: false,
            require_compliance: false,
        }
    }
}

// ---------------------------------------------------------------------------
// PolicyDecision — what the engine returns
// ---------------------------------------------------------------------------

/// The reason a policy evaluation was denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenialReason {
    /// KYC has not been submitted by the subject.
    KycNotSubmitted,
    /// KYC is pending review.
    KycPending,
    /// KYC was rejected.
    KycRejected,
    /// KYC approval has expired.
    KycExpired,
    /// General compliance check record is missing or failed.
    ComplianceCheckFailed,
    /// Subject's score is below the required minimum.
    ScoreBelowMinimum {
        required: u32,
        actual: u32,
    },
}

impl DenialReason {
    /// Human-readable message suitable for error context.
    pub fn message(&self) -> String {
        match self {
            DenialReason::KycNotSubmitted    => String::from("KYC has not been submitted"),
            DenialReason::KycPending         => String::from("KYC verification is pending"),
            DenialReason::KycRejected        => String::from("KYC verification was rejected"),
            DenialReason::KycExpired         => String::from("KYC approval has expired"),
            DenialReason::ComplianceCheckFailed => {
                String::from("Compliance check record is missing or failed")
            }
            DenialReason::ScoreBelowMinimum { required, actual } => {
                alloc::format!(
                    "Compliance score {} is below required minimum {}",
                    actual, required
                )
            }
        }
    }
}

/// The outcome of running all rules in a [`PolicyEngine`] against a
/// [`PolicyContext`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    /// All rules passed — the request is allowed to proceed.
    Allow,
    /// At least one rule failed — the request must be denied.
    Deny(DenialReason),
}

impl PolicyDecision {
    /// Returns `true` when the decision is [`Allow`](PolicyDecision::Allow).
    pub fn is_allowed(&self) -> bool {
        matches!(self, PolicyDecision::Allow)
    }

    /// Returns `true` when the decision is [`Deny`](PolicyDecision::Deny).
    pub fn is_denied(&self) -> bool {
        matches!(self, PolicyDecision::Deny(_))
    }

    /// Returns the [`DenialReason`] if this is a denial, or `None` if allowed.
    pub fn denial_reason(&self) -> Option<&DenialReason> {
        match self {
            PolicyDecision::Deny(r) => Some(r),
            PolicyDecision::Allow   => None,
        }
    }
}

// ---------------------------------------------------------------------------
// PolicyRule — a single composable check
// ---------------------------------------------------------------------------

/// A single, stateless compliance rule.
///
/// Rules are evaluated in the order they appear in the [`PolicyEngine`]'s
/// rule list. The first rule to return `Deny` stops evaluation immediately
/// (short-circuit). Implement this trait to add domain-specific rules without
/// touching the engine itself.
pub trait PolicyRule {
    /// Evaluate `ctx` and return `Allow` or `Deny`.
    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision;

    /// A short identifier used in logs and error context.
    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Built-in rules
// ---------------------------------------------------------------------------

/// Enforces that the subject has an approved KYC record when
/// [`PolicyContext::require_kyc`] is `true`.
pub struct KycApprovedRule;

impl PolicyRule for KycApprovedRule {
    fn name(&self) -> &str { "kyc_approved" }

    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision {
        if !ctx.require_kyc {
            return PolicyDecision::Allow;
        }
        match ctx.kyc_state {
            KycState::Approved       => PolicyDecision::Allow,
            KycState::NotSubmitted   => PolicyDecision::Deny(DenialReason::KycNotSubmitted),
            KycState::Pending        => PolicyDecision::Deny(DenialReason::KycPending),
            KycState::Rejected       => PolicyDecision::Deny(DenialReason::KycRejected),
            KycState::Expired        => PolicyDecision::Deny(DenialReason::KycExpired),
            KycState::Reopened       => PolicyDecision::Deny(DenialReason::KycPending),
        }
    }
}

/// Enforces that the subject has a passing compliance check record when
/// [`PolicyContext::require_compliance`] is `true`.
pub struct ComplianceCheckPassedRule;

impl PolicyRule for ComplianceCheckPassedRule {
    fn name(&self) -> &str { "compliance_check_passed" }

    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision {
        if !ctx.require_compliance {
            return PolicyDecision::Allow;
        }
        if ctx.compliance_check_passed {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Deny(DenialReason::ComplianceCheckFailed)
        }
    }
}

/// Enforces a minimum compliance score when a `minimum_score` is configured
/// in the context and `require_compliance` is `true`.
pub struct MinimumScoreRule;

impl PolicyRule for MinimumScoreRule {
    fn name(&self) -> &str { "minimum_score" }

    fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision {
        if !ctx.require_compliance {
            return PolicyDecision::Allow;
        }
        let Some(min) = ctx.minimum_score else {
            return PolicyDecision::Allow; // no threshold configured
        };
        match ctx.subject_score {
            None => {
                // No score recorded — treat as 0 vs minimum
                if min == 0 {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::Deny(DenialReason::ScoreBelowMinimum {
                        required: min,
                        actual: 0,
                    })
                }
            }
            Some(score) => {
                if score >= min {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::Deny(DenialReason::ScoreBelowMinimum {
                        required: min,
                        actual: score,
                    })
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PolicyEngine — the composable evaluator
// ---------------------------------------------------------------------------

/// Evaluates an ordered list of [`PolicyRule`]s against a [`PolicyContext`].
///
/// Rules are short-circuited — the first denial stops evaluation and is
/// returned immediately. If all rules pass, [`PolicyDecision::Allow`] is
/// returned.
///
/// # Building a standard engine
///
/// ```rust
/// use anchorkit::compliance_policy::PolicyEngine;
///
/// let engine = PolicyEngine::standard();
/// ```
///
/// The `standard` engine bundles the three built-in rules in the correct
/// evaluation order:
///
/// 1. [`KycApprovedRule`]            — KYC gate (when `require_kyc`)
/// 2. [`ComplianceCheckPassedRule`]  — check-record gate (when `require_compliance`)
/// 3. [`MinimumScoreRule`]           — score threshold (when `require_compliance` + `minimum_score`)
pub struct PolicyEngine {
    rules: Vec<Box<dyn PolicyRule>>,
}

impl PolicyEngine {
    /// Create an empty engine with no rules (always allows).
    pub fn new() -> Self {
        PolicyEngine { rules: Vec::new() }
    }

    /// Add a rule to the end of the evaluation chain.
    pub fn add_rule(mut self, rule: impl PolicyRule + 'static) -> Self {
        self.rules.push(Box::new(rule));
        self
    }

    /// Build the standard engine used by all main workflow entry points.
    ///
    /// Rule order:
    /// 1. KYC approval gate
    /// 2. Compliance check-record gate
    /// 3. Minimum score threshold
    pub fn standard() -> Self {
        PolicyEngine::new()
            .add_rule(KycApprovedRule)
            .add_rule(ComplianceCheckPassedRule)
            .add_rule(MinimumScoreRule)
    }

    /// Evaluate every rule against `ctx`, returning on the first denial.
    ///
    /// Returns [`PolicyDecision::Allow`] when no rules are present or all
    /// rules pass.
    pub fn evaluate(&self, ctx: &PolicyContext) -> PolicyDecision {
        for rule in &self.rules {
            let decision = rule.evaluate(ctx);
            if decision.is_denied() {
                return decision;
            }
        }
        PolicyDecision::Allow
    }

    /// Evaluate and collect *all* denials rather than stopping at the first.
    ///
    /// Useful for surfacing the full set of compliance problems to the
    /// caller in one pass (e.g. for admin reporting).
    pub fn evaluate_all(&self, ctx: &PolicyContext) -> Vec<DenialReason> {
        let mut denials = Vec::new();
        for rule in &self.rules {
            if let PolicyDecision::Deny(reason) = rule.evaluate(ctx) {
                denials.push(reason);
            }
        }
        denials
    }

    /// Returns the number of rules in this engine.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::standard()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn approved_ctx() -> PolicyContext {
        PolicyContext {
            kyc_state: KycState::Approved,
            compliance_check_passed: true,
            minimum_score: None,
            subject_score: Some(80),
            require_kyc: true,
            require_compliance: true,
        }
    }

    fn enforcement_off_ctx() -> PolicyContext {
        PolicyContext {
            kyc_state: KycState::NotSubmitted,
            compliance_check_passed: false,
            minimum_score: None,
            subject_score: None,
            require_kyc: false,
            require_compliance: false,
        }
    }

    // ── PolicyDecision helpers ────────────────────────────────────────────

    #[test]
    fn allow_is_allowed() {
        assert!(PolicyDecision::Allow.is_allowed());
        assert!(!PolicyDecision::Allow.is_denied());
        assert!(PolicyDecision::Allow.denial_reason().is_none());
    }

    #[test]
    fn deny_is_denied() {
        let d = PolicyDecision::Deny(DenialReason::KycPending);
        assert!(d.is_denied());
        assert!(!d.is_allowed());
        assert_eq!(d.denial_reason(), Some(&DenialReason::KycPending));
    }

    // ── KycApprovedRule ───────────────────────────────────────────────────

    #[test]
    fn kyc_rule_allows_when_kyc_not_required() {
        let rule = KycApprovedRule;
        let ctx = PolicyContext { require_kyc: false, ..PolicyContext::permissive() };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Allow);
    }

    #[test]
    fn kyc_rule_allows_approved_kyc() {
        let rule = KycApprovedRule;
        let ctx = PolicyContext {
            kyc_state: KycState::Approved,
            require_kyc: true,
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Allow);
    }

    #[test]
    fn kyc_rule_denies_not_submitted() {
        let rule = KycApprovedRule;
        let ctx = PolicyContext {
            kyc_state: KycState::NotSubmitted,
            require_kyc: true,
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Deny(DenialReason::KycNotSubmitted));
    }

    #[test]
    fn kyc_rule_denies_pending() {
        let rule = KycApprovedRule;
        let ctx = PolicyContext {
            kyc_state: KycState::Pending,
            require_kyc: true,
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Deny(DenialReason::KycPending));
    }

    #[test]
    fn kyc_rule_denies_rejected() {
        let rule = KycApprovedRule;
        let ctx = PolicyContext {
            kyc_state: KycState::Rejected,
            require_kyc: true,
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Deny(DenialReason::KycRejected));
    }

    #[test]
    fn kyc_rule_denies_expired() {
        let rule = KycApprovedRule;
        let ctx = PolicyContext {
            kyc_state: KycState::Expired,
            require_kyc: true,
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Deny(DenialReason::KycExpired));
    }

    #[test]
    fn kyc_rule_denies_reopened() {
        let rule = KycApprovedRule;
        let ctx = PolicyContext {
            kyc_state: KycState::Reopened,
            require_kyc: true,
            ..PolicyContext::permissive()
        };
        // Reopened is treated as still-pending from a gate perspective
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Deny(DenialReason::KycPending));
    }

    // ── ComplianceCheckPassedRule ────────────────────────────────────────

    #[test]
    fn compliance_rule_allows_when_not_required() {
        let rule = ComplianceCheckPassedRule;
        let ctx = PolicyContext { require_compliance: false, ..PolicyContext::permissive() };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Allow);
    }

    #[test]
    fn compliance_rule_allows_when_check_passed() {
        let rule = ComplianceCheckPassedRule;
        let ctx = PolicyContext {
            compliance_check_passed: true,
            require_compliance: true,
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Allow);
    }

    #[test]
    fn compliance_rule_denies_when_check_failed() {
        let rule = ComplianceCheckPassedRule;
        let ctx = PolicyContext {
            compliance_check_passed: false,
            require_compliance: true,
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Deny(DenialReason::ComplianceCheckFailed));
    }

    // ── MinimumScoreRule ─────────────────────────────────────────────────

    #[test]
    fn score_rule_allows_when_not_required() {
        let rule = MinimumScoreRule;
        let ctx = PolicyContext { require_compliance: false, minimum_score: Some(50), ..PolicyContext::permissive() };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Allow);
    }

    #[test]
    fn score_rule_allows_when_no_minimum_configured() {
        let rule = MinimumScoreRule;
        let ctx = PolicyContext {
            require_compliance: true,
            minimum_score: None,
            subject_score: Some(10),
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Allow);
    }

    #[test]
    fn score_rule_allows_when_score_meets_minimum() {
        let rule = MinimumScoreRule;
        let ctx = PolicyContext {
            require_compliance: true,
            minimum_score: Some(70),
            subject_score: Some(70),
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Allow);
    }

    #[test]
    fn score_rule_allows_when_score_exceeds_minimum() {
        let rule = MinimumScoreRule;
        let ctx = PolicyContext {
            require_compliance: true,
            minimum_score: Some(60),
            subject_score: Some(95),
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Allow);
    }

    #[test]
    fn score_rule_denies_when_score_below_minimum() {
        let rule = MinimumScoreRule;
        let ctx = PolicyContext {
            require_compliance: true,
            minimum_score: Some(80),
            subject_score: Some(55),
            ..PolicyContext::permissive()
        };
        assert_eq!(
            rule.evaluate(&ctx),
            PolicyDecision::Deny(DenialReason::ScoreBelowMinimum { required: 80, actual: 55 })
        );
    }

    #[test]
    fn score_rule_denies_when_no_score_and_minimum_nonzero() {
        let rule = MinimumScoreRule;
        let ctx = PolicyContext {
            require_compliance: true,
            minimum_score: Some(50),
            subject_score: None,
            ..PolicyContext::permissive()
        };
        assert_eq!(
            rule.evaluate(&ctx),
            PolicyDecision::Deny(DenialReason::ScoreBelowMinimum { required: 50, actual: 0 })
        );
    }

    #[test]
    fn score_rule_allows_zero_minimum_with_no_score() {
        let rule = MinimumScoreRule;
        let ctx = PolicyContext {
            require_compliance: true,
            minimum_score: Some(0),
            subject_score: None,
            ..PolicyContext::permissive()
        };
        assert_eq!(rule.evaluate(&ctx), PolicyDecision::Allow);
    }

    // ── PolicyEngine – standard ───────────────────────────────────────────

    #[test]
    fn standard_engine_has_three_rules() {
        let engine = PolicyEngine::standard();
        assert_eq!(engine.rule_count(), 3);
    }

    #[test]
    fn standard_engine_allows_fully_compliant_context() {
        let engine = PolicyEngine::standard();
        assert_eq!(engine.evaluate(&approved_ctx()), PolicyDecision::Allow);
    }

    #[test]
    fn standard_engine_allows_when_enforcement_off() {
        let engine = PolicyEngine::standard();
        assert_eq!(engine.evaluate(&enforcement_off_ctx()), PolicyDecision::Allow);
    }

    #[test]
    fn standard_engine_denies_pending_kyc_first() {
        let engine = PolicyEngine::standard();
        let ctx = PolicyContext {
            kyc_state: KycState::Pending,
            compliance_check_passed: false,
            minimum_score: Some(80),
            subject_score: Some(20),
            require_kyc: true,
            require_compliance: true,
        };
        // KYC rule fires first
        assert_eq!(engine.evaluate(&ctx), PolicyDecision::Deny(DenialReason::KycPending));
    }

    #[test]
    fn standard_engine_denies_failed_compliance_check_when_kyc_not_required() {
        let engine = PolicyEngine::standard();
        let ctx = PolicyContext {
            kyc_state: KycState::NotSubmitted,
            compliance_check_passed: false,
            minimum_score: None,
            subject_score: None,
            require_kyc: false,
            require_compliance: true,
        };
        assert_eq!(engine.evaluate(&ctx), PolicyDecision::Deny(DenialReason::ComplianceCheckFailed));
    }

    #[test]
    fn standard_engine_denies_score_below_minimum_when_kyc_passed() {
        let engine = PolicyEngine::standard();
        let ctx = PolicyContext {
            kyc_state: KycState::Approved,
            compliance_check_passed: true,
            minimum_score: Some(90),
            subject_score: Some(45),
            require_kyc: true,
            require_compliance: true,
        };
        assert_eq!(
            engine.evaluate(&ctx),
            PolicyDecision::Deny(DenialReason::ScoreBelowMinimum { required: 90, actual: 45 })
        );
    }

    // ── evaluate_all collects multiple denials ───────────────────────────

    #[test]
    fn evaluate_all_collects_all_denials() {
        let engine = PolicyEngine::standard();
        // KYC pending, compliance not passed, score below minimum
        let ctx = PolicyContext {
            kyc_state: KycState::Pending,
            compliance_check_passed: false,
            minimum_score: Some(80),
            subject_score: Some(10),
            require_kyc: true,
            require_compliance: true,
        };
        let denials = engine.evaluate_all(&ctx);
        assert_eq!(denials.len(), 3);
        assert!(denials.contains(&DenialReason::KycPending));
        assert!(denials.contains(&DenialReason::ComplianceCheckFailed));
        assert!(denials.contains(&DenialReason::ScoreBelowMinimum { required: 80, actual: 10 }));
    }

    #[test]
    fn evaluate_all_returns_empty_when_all_pass() {
        let engine = PolicyEngine::standard();
        let denials = engine.evaluate_all(&approved_ctx());
        assert!(denials.is_empty());
    }

    // ── Empty engine ─────────────────────────────────────────────────────

    #[test]
    fn empty_engine_always_allows() {
        let engine = PolicyEngine::new();
        assert_eq!(engine.evaluate(&approved_ctx()), PolicyDecision::Allow);
        assert_eq!(engine.evaluate(&enforcement_off_ctx()), PolicyDecision::Allow);
    }

    // ── DenialReason messages ────────────────────────────────────────────

    #[test]
    fn denial_reason_messages_are_non_empty() {
        let reasons = [
            DenialReason::KycNotSubmitted,
            DenialReason::KycPending,
            DenialReason::KycRejected,
            DenialReason::KycExpired,
            DenialReason::ComplianceCheckFailed,
            DenialReason::ScoreBelowMinimum { required: 80, actual: 40 },
        ];
        for r in &reasons {
            assert!(!r.message().is_empty(), "empty message for {:?}", r);
        }
    }

    #[test]
    fn denial_reason_score_message_contains_values() {
        let r = DenialReason::ScoreBelowMinimum { required: 75, actual: 30 };
        let msg = r.message();
        assert!(msg.contains("75"), "missing required in: {msg}");
        assert!(msg.contains("30"), "missing actual in: {msg}");
    }

    // ── KycState helpers ─────────────────────────────────────────────────

    #[test]
    fn kyc_state_is_approved_only_for_approved() {
        assert!(KycState::Approved.is_approved());
        assert!(!KycState::Pending.is_approved());
        assert!(!KycState::Rejected.is_approved());
        assert!(!KycState::Expired.is_approved());
        assert!(!KycState::NotSubmitted.is_approved());
        assert!(!KycState::Reopened.is_approved());
    }

    // ── Default engine == standard ───────────────────────────────────────

    #[test]
    fn default_engine_is_standard() {
        let engine = PolicyEngine::default();
        assert_eq!(engine.rule_count(), 3);
        assert_eq!(engine.evaluate(&approved_ctx()), PolicyDecision::Allow);
    }
}
