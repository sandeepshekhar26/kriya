//! The governed dispatch core — what makes this an *agent-governance* MCP server and not a
//! vanilla one. Every `tools/call` an external agent makes passes through the exact same
//! gate sequence the in-process host enforces (see `agent::host`):
//!
//! 1. **Policy** — deny-by-default; the host decides, the agent cannot bypass it.
//! 2. **Approval** — actions the policy guards wait for a human (deny by default in MCP mode).
//! 3. **Budget** — a runaway agent is rate-limited before it acts.
//! 4. **Execute** — only now does the cleared action run, via a pluggable executor.
//! 5. **Audit** — every *executed* action gets an Ed25519-signed receipt the agent can't forge.
//!
//! Blocked actions (denied / unapproved / over budget) are *not* signed — receipts attest
//! to what actually ran, matching the in-process host's audit semantics. The block itself is
//! reported back to the agent so it can reason about the refusal.

use std::sync::Arc;

use crate::audit::{now_ms, Actor, Receipt, SignedReceipt, Signer};
use crate::budget::BudgetTracker;
use crate::permissions::{Decision, Policy};

use super::approval::ApprovalGate;
use super::executor::{ActionExecutor, ActionOutcome};

/// The result of routing one `tools/call` through the gates.
#[derive(Debug)]
pub enum DispatchOutcome {
    /// Policy denied the action outright. Never executed, never signed.
    Denied,
    /// Policy required approval and the human declined (or no operator was reachable).
    NotApproved,
    /// The per-minute action budget was exhausted. Carries the human-readable reason.
    BudgetExceeded(String),
    /// The action cleared every gate and ran. Carries the handler outcome and the signed
    /// receipt appended to the audit log.
    Executed { outcome: ActionOutcome, receipt: SignedReceipt },
}

/// Wires the gates around a pluggable [`ActionExecutor`] + [`ApprovalGate`]. Holds the
/// budget tracker (stateful across calls) so the rate limit spans the whole MCP session,
/// exactly like a single agent run in the in-process host.
pub struct Governor {
    policy: Arc<Policy>,
    signer: Arc<Signer>,
    budget: BudgetTracker,
    approval: Box<dyn ApprovalGate>,
    executor: Box<dyn ActionExecutor>,
    /// Who the external agent is (R8). Set by the binary from the MCP client identity;
    /// stamped into every signed receipt so cross-app audit attributes each call.
    actor: Option<Actor>,
}

impl Governor {
    pub fn new(
        policy: Arc<Policy>,
        signer: Arc<Signer>,
        approval: Box<dyn ApprovalGate>,
        executor: Box<dyn ActionExecutor>,
    ) -> Self {
        let budget = BudgetTracker::new(policy.max_actions_per_minute());
        Self { policy, signer, budget, approval, executor, actor: None }
    }

    /// Attribute every receipt this governor signs to `actor` (R8). Chainable on `new`.
    pub fn with_actor(mut self, actor: Option<Actor>) -> Self {
        self.actor = actor;
        self
    }

    /// Run one action through policy → approval → budget → execute → audit.
    pub fn dispatch(&mut self, action_id: &str, params: &serde_json::Value) -> DispatchOutcome {
        // 1. Policy gate — the host decides, not the agent.
        match self.policy.check(action_id) {
            Decision::Allow => {}
            Decision::RequiresApproval => {
                // 2. Approval gate — held for a human; default-deny in MCP mode.
                if !self.approval.request(action_id, params) {
                    return DispatchOutcome::NotApproved;
                }
            }
            Decision::Deny => return DispatchOutcome::Denied,
        }

        // 3. Budget gate — stop a runaway/looping agent before it acts.
        if let Err(reason) = self.budget.check_and_record(now_ms()) {
            return DispatchOutcome::BudgetExceeded(reason);
        }

        // 4. Execute the cleared action.
        let outcome = self.executor.execute(action_id, params);

        // 5. Sign + append a receipt, success or failure. The signing key never leaves the
        //    host, so the agent can propose and run an action but cannot forge its receipt.
        //    The receipt carries who acted (R8) when the binary supplied an identity.
        let receipt = self.signer.record(
            Receipt::new(
                uuid::Uuid::new_v4().to_string(),
                action_id.to_string(),
                params.clone(),
                outcome.success,
                now_ms(),
            )
            .with_actor(self.actor.clone()),
        );

        DispatchOutcome::Executed { outcome, receipt }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::approval::{AutoApprove, DenyApproval};
    use crate::mcp::executor::FnExecutor;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn signer() -> Arc<Signer> {
        Arc::new(Signer::new())
    }

    /// Counts how many times the executor actually ran — proves blocked actions never reach it.
    fn counting_executor(counter: Arc<AtomicUsize>) -> Box<dyn ActionExecutor> {
        Box::new(FnExecutor(move |_id: &str, _p: &Value| {
            counter.fetch_add(1, Ordering::SeqCst);
            ActionOutcome::ok(json!({"ran": true}))
        }))
    }

    #[test]
    fn allowed_action_executes_and_is_signed() {
        let ran = Arc::new(AtomicUsize::new(0));
        let mut g = Governor::new(
            Arc::new(Policy::default()),
            signer(),
            Box::new(DenyApproval),
            counting_executor(ran.clone()),
        );
        match g.dispatch("create_note", &json!({"title": "hi"})) {
            DispatchOutcome::Executed { outcome, receipt } => {
                assert!(outcome.success);
                assert_eq!(receipt.receipt.action_id, "create_note");
                assert!(!receipt.signature.is_empty());
            }
            other => panic!("expected Executed, got {other:?}"),
        }
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn denied_action_never_runs_and_is_not_signed() {
        let ran = Arc::new(AtomicUsize::new(0));
        let mut g = Governor::new(
            Arc::new(Policy::default()),
            signer(),
            Box::new(AutoApprove),
            counting_executor(ran.clone()),
        );
        // `wire_money` matches no allow rule → deny by default.
        assert!(matches!(g.dispatch("wire_money", &json!({})), DispatchOutcome::Denied));
        assert_eq!(ran.load(Ordering::SeqCst), 0, "denied action must not execute");
    }

    #[test]
    fn guarded_action_blocked_when_approval_denied() {
        let ran = Arc::new(AtomicUsize::new(0));
        let mut g = Governor::new(
            Arc::new(Policy::default()),
            signer(),
            Box::new(DenyApproval),
            counting_executor(ran.clone()),
        );
        // delete_* requires approval; DenyApproval refuses it.
        assert!(matches!(g.dispatch("delete_note", &json!({"id": 1})), DispatchOutcome::NotApproved));
        assert_eq!(ran.load(Ordering::SeqCst), 0, "unapproved action must not execute");
    }

    #[test]
    fn guarded_action_runs_when_approved() {
        let ran = Arc::new(AtomicUsize::new(0));
        let mut g = Governor::new(
            Arc::new(Policy::default()),
            signer(),
            Box::new(AutoApprove),
            counting_executor(ran.clone()),
        );
        assert!(matches!(
            g.dispatch("delete_note", &json!({"id": 1})),
            DispatchOutcome::Executed { .. }
        ));
        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn budget_blocks_after_cap_and_does_not_execute() {
        let policy: Policy = serde_yaml::from_str(
            r#"
rules:
  - action: "create_*"
    allow: true
  - action: "*"
    allow: false
budget:
  max_actions_per_minute: 2
"#,
        )
        .unwrap();
        let ran = Arc::new(AtomicUsize::new(0));
        let mut g = Governor::new(
            Arc::new(policy),
            signer(),
            Box::new(DenyApproval),
            counting_executor(ran.clone()),
        );
        assert!(matches!(g.dispatch("create_note", &json!({})), DispatchOutcome::Executed { .. }));
        assert!(matches!(g.dispatch("create_note", &json!({})), DispatchOutcome::Executed { .. }));
        // third within the minute trips the budget — and must not reach the executor.
        assert!(matches!(
            g.dispatch("create_note", &json!({})),
            DispatchOutcome::BudgetExceeded(_)
        ));
        assert_eq!(ran.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn receipt_carries_actor_when_set() {
        let ran = Arc::new(AtomicUsize::new(0));
        let mut g = Governor::new(
            Arc::new(Policy::default()),
            signer(),
            Box::new(DenyApproval),
            counting_executor(ran.clone()),
        )
        .with_actor(Some(crate::audit::Actor::new("claude-desktop", "alice")));
        match g.dispatch("create_note", &json!({"title": "hi"})) {
            DispatchOutcome::Executed { receipt, .. } => {
                assert_eq!(
                    receipt.receipt.actor,
                    Some(crate::audit::Actor::new("claude-desktop", "alice"))
                );
            }
            other => panic!("expected Executed, got {other:?}"),
        }
    }

    #[test]
    fn receipt_has_no_actor_by_default() {
        let ran = Arc::new(AtomicUsize::new(0));
        let mut g = Governor::new(
            Arc::new(Policy::default()),
            signer(),
            Box::new(DenyApproval),
            counting_executor(ran.clone()),
        );
        match g.dispatch("create_note", &json!({})) {
            DispatchOutcome::Executed { receipt, .. } => assert!(receipt.receipt.actor.is_none()),
            other => panic!("expected Executed, got {other:?}"),
        }
    }

    #[test]
    fn failed_execution_is_still_signed() {
        let mut g = Governor::new(
            Arc::new(Policy::default()),
            signer(),
            Box::new(DenyApproval),
            Box::new(FnExecutor(|_id: &str, _p: &Value| ActionOutcome::failed("boom"))),
        );
        match g.dispatch("create_note", &json!({})) {
            DispatchOutcome::Executed { outcome, receipt } => {
                assert!(!outcome.success);
                assert!(!receipt.receipt.success);
                assert!(!receipt.signature.is_empty(), "failures are audited too");
            }
            other => panic!("expected Executed, got {other:?}"),
        }
    }
}
