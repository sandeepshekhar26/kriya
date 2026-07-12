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
//!
//! ## The governed-lane I/O ledger (`kriya.io.*`, doc 24 §7.3)
//!
//! When an [`EgressControl`] is installed, a governed call whose destination the resolver names is
//! checked against the egress tier (allow / approval / deny by host + per-destination byte budgets)
//! and produces a standalone `kriya.io.<direction>.<kind>.<decision>` receipt, correlated to its
//! action receipt by a `corr` param (never adjacency). A denied egress is receipted at the decision
//! point, before/instead of execute. This is a governed-lane control, and its honest ceiling is
//! fixed:
//!
//! > *When a stdio MCP server routed through kriya calls an external API, kriya sees — and signs —
//! > only the tool call and result that crossed its stdio pipe; the server's own outbound network
//! > traffic (which hosts it contacted, what it sent) is invisible to kriya and appears in no
//! > receipt. Host-level observation of that traffic is E2.*

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::audit::{now_ms, Actor, Receipt, SignedReceipt, Signer};
use crate::budget::BudgetTracker;
use crate::permissions::{Decision, EgressDecision, Policy};

use super::approval::ApprovalGate;
use super::executor::{
    ActionExecutor, ActionOutcome, HashScheme, IoDecision, IoDirection, IoKind, IoRecord,
};

/// The result of routing one `tools/call` through the gates.
// `Executed` carries a full `SignedReceipt` (R20 hash-chain + R8 actor pushed it past the lint's
// size threshold). Boxing it would ripple to every `DispatchOutcome` match site repo-wide for no
// real gain — the value is short-lived, one per call.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum DispatchOutcome {
    /// Policy denied the action outright. Never executed, never signed.
    Denied,
    /// Policy required approval and the human declined (or no operator was reachable).
    NotApproved,
    /// The per-minute action budget was exhausted. Carries the human-readable reason.
    BudgetExceeded(String),
    /// The **egress tier** blocked the destination (allowlist deny, byte budget, or a fail-closed
    /// receipt-precondition write failure) — decided at the gate, before/instead of execute. A
    /// signed `kriya.io.*.deny` receipt is written at the decision point (doc 24 §7.3 / L10), EXCEPT
    /// when the block itself is that no receipt could be written (fail-closed on an unwritable log).
    /// Carries the reason.
    EgressDenied(String),
    /// The action cleared every gate and ran. Carries the handler outcome and the signed
    /// receipt appended to the audit log.
    Executed {
        outcome: ActionOutcome,
        receipt: SignedReceipt,
    },
}

/// The destination an action egresses to, resolved from `(action_id, params)` **before** execution
/// so the egress tier can decide — and a `deny` receipt can be written — at the decision point
/// (doc 24 L10). The broker supplies a resolver over its upstream→host map; the hook lane parses a
/// WebFetch URL.
pub struct EgressTarget {
    pub host: String,
    pub kind: IoKind,
    /// The MCP server NAME when the destination is an `mcp` kind whose endpoint isn't a claimable
    /// host (carried as a separate param, never conflated with a host — doc 24 §6-H6).
    pub server: Option<String>,
}

/// Egress governance wired onto a [`Governor`] (doc 24 §7.3). The `resolver` maps an action to its
/// destination; the [`crate::permissions::EgressPolicy`] holds the tier + byte budgets. Absent on a
/// Governor → egress is ungoverned and dispatch is byte-identical to pre-EG-2. Present → each action
/// whose destination the resolver names is checked against the tier + byte budgets, approval-gated
/// at the ApprovalGate for the approve/defer tier, and produces a `kriya.io.*` receipt.
pub struct EgressControl {
    policy: crate::permissions::EgressPolicy,
    #[allow(clippy::type_complexity)]
    resolver: Box<dyn Fn(&str, &Value) -> Option<EgressTarget> + Send>,
    /// Sliding per-destination byte windows keyed by the budget's pattern (B2).
    budgets: Mutex<HashMap<String, Vec<(u128, u64)>>>,
}

impl EgressControl {
    pub fn new(
        policy: crate::permissions::EgressPolicy,
        resolver: impl Fn(&str, &Value) -> Option<EgressTarget> + Send + 'static,
    ) -> Self {
        Self {
            policy,
            resolver: Box::new(resolver),
            budgets: Mutex::new(HashMap::new()),
        }
    }

    /// Charge `bytes` against the destination's byte budget in a trailing window; `Err(reason)` if
    /// this call would breach it (anti slow-drip exfil, B2). No budget for the host → always Ok.
    /// `bytes` is the *estimated* outbound payload (the serialized request), so the budget is a true
    /// pre-emptive gate — the receipt still records the transport-observed bytes.
    fn charge_budget(&self, host: &str, bytes: u64, now: u128) -> Result<(), String> {
        let Some((pattern, budget)) = self.policy.budget_for(host) else {
            return Ok(());
        };
        let mut map = self.budgets.lock().unwrap_or_else(|e| e.into_inner());
        let window_ms = (budget.window_secs.saturating_mul(1000)) as u128;
        let entries = map.entry(pattern.clone()).or_default();
        entries.retain(|(ts, _)| now.saturating_sub(*ts) < window_ms);
        let used: u64 = entries.iter().map(|(_, b)| *b).sum();
        if used.saturating_add(bytes) > budget.max_bytes {
            return Err(format!(
                "egress byte budget for '{pattern}' exhausted ({used}+{bytes} > {} bytes / {}s)",
                budget.max_bytes, budget.window_secs
            ));
        }
        entries.push((now, bytes));
        Ok(())
    }
}

/// The egress gate's decision for one dispatch.
enum EgressGate {
    /// No egress governance applies (no control installed, or the action resolves to no
    /// destination) — proceed exactly as pre-EG-2.
    Ungoverned,
    /// Governed egress cleared; carries what the post-execute emission needs.
    Cleared(Box<EgressCtx>),
    /// Blocked at the decision point (allowlist deny / byte budget / fail-closed write failure). The
    /// `kriya.io.*.deny` receipt is already written.
    Denied(String),
    /// Approval-tier egress not granted (the parked/refused attempt is already receipted).
    NotApproved,
}

/// What a cleared egress gate carries forward to emit the `kriya.io.*` allow/approve receipt after
/// execution.
struct EgressCtx {
    target: EgressTarget,
    decision: IoDecision, // Allow or Approve
    policy_rule: Option<String>,
    approved_by: Option<String>,
    /// True when the io receipt was ALREADY signed pre-execute (fail-closed mode) — don't emit a
    /// second one post-execute.
    pre_signed: bool,
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
    /// Optional egress governance (doc 24 §7.3). `None` → byte-identical to pre-EG-2.
    egress: Option<EgressControl>,
}

impl Governor {
    pub fn new(
        policy: Arc<Policy>,
        signer: Arc<Signer>,
        approval: Box<dyn ApprovalGate>,
        executor: Box<dyn ActionExecutor>,
    ) -> Self {
        let budget = BudgetTracker::new(policy.max_actions_per_minute());
        Self {
            policy,
            signer,
            budget,
            approval,
            executor,
            actor: None,
            egress: None,
        }
    }

    /// Attribute every receipt this governor signs to `actor` (R8). Chainable on `new`.
    pub fn with_actor(mut self, actor: Option<Actor>) -> Self {
        self.actor = actor;
        self
    }

    /// Install egress governance (doc 24 §7.3). Chainable on `new`/`with_actor`.
    pub fn with_egress(mut self, egress: EgressControl) -> Self {
        self.egress = Some(egress);
        self
    }

    /// Run one action through policy → egress tier → budget → execute → audit (+ `kriya.io.*`).
    pub fn dispatch(&mut self, action_id: &str, params: &Value) -> DispatchOutcome {
        // 1. Action policy gate — the host decides, not the agent.
        match self.policy.check(action_id) {
            Decision::Allow => {}
            Decision::RequiresApproval => {
                // Approval gate — held for a human; default-deny in MCP mode.
                if !self.approval.request(action_id, params) {
                    return DispatchOutcome::NotApproved;
                }
            }
            Decision::Deny => return DispatchOutcome::Denied,
        }

        // 2. Budget gate — stop a runaway/looping agent before it acts. Runs BEFORE the egress gate
        //    so a budget-blocked call never signs a fail-closed egress precondition receipt (which
        //    would record — and durably persist — an egress that then never happens).
        if let Err(reason) = self.budget.check_and_record(now_ms()) {
            return DispatchOutcome::BudgetExceeded(reason);
        }

        // The action receipt's step_id, generated up front so the `kriya.io.*` receipt can carry it
        // as `corr` — including in fail-closed mode where the io receipt is signed BEFORE execute.
        let action_step_id = uuid::Uuid::new_v4().to_string();

        // 3. Egress tier gate (doc 24 §7.3) — only when configured AND the action resolves to a
        //    destination. Deny is decided here, at the decision point, so a `kriya.io.*.deny`
        //    receipt exists before/instead of execute (L10).
        let egress_ctx = match self.egress_gate(action_id, params, &action_step_id) {
            EgressGate::Ungoverned => None,
            EgressGate::Cleared(ctx) => Some(ctx),
            EgressGate::Denied(reason) => return DispatchOutcome::EgressDenied(reason),
            EgressGate::NotApproved => return DispatchOutcome::NotApproved,
        };

        // 4. Execute the cleared action.
        let outcome = self.executor.execute(action_id, params);

        // 5. Sign + append the action receipt (frozen schema), success or failure. The signing key
        //    never leaves the host, so the agent can propose and run an action but cannot forge its
        //    receipt. The receipt carries who acted (R8) when the binary supplied an identity.
        let receipt = self.signer.record(
            Receipt::new(
                action_step_id.clone(),
                action_id.to_string(),
                params.clone(),
                outcome.success,
                now_ms(),
            )
            .with_actor(self.actor.clone()),
        );

        // 6. The `kriya.io.*` allow/approve receipt — a standalone signed receipt correlated by
        //    `corr` (never adjacency, L5). In fail-closed mode it was already written pre-execute.
        if let Some(ctx) = egress_ctx {
            if !ctx.pre_signed {
                self.emit_io_allow(&ctx, outcome.io.as_ref(), &action_step_id);
            }
        }

        DispatchOutcome::Executed { outcome, receipt }
    }

    /// Evaluate the egress tier for an action's destination. Signs a `kriya.io.*.deny` receipt at
    /// the decision point on any block (L10). In fail-closed mode signs the allow receipt as a
    /// precondition (B3) and denies if it can't be persisted.
    fn egress_gate(&self, action_id: &str, params: &Value, action_step_id: &str) -> EgressGate {
        let Some(control) = self.egress.as_ref() else {
            return EgressGate::Ungoverned;
        };
        let Some(target) = (control.resolver)(action_id, params) else {
            return EgressGate::Ungoverned;
        };

        // Tier decision by destination host.
        let (decision, policy_rule) = match control.policy.evaluate(&target.host) {
            EgressDecision::Allow { rule } => (IoDecision::Allow, rule),
            EgressDecision::Approval { rule } => {
                // Egress approval rides the existing ApprovalGate (approve/defer tier, B4). The
                // prompt shown is the io action id so the operator sees it is an egress decision.
                let io_prompt = format!(
                    "kriya.io.{}.{}.approve",
                    IoDirection::Egress.facet(),
                    target.kind.facet()
                );
                if !self.approval.request(&io_prompt, params) {
                    self.sign_io_deny(&target, rule, "egress approval not granted".to_string());
                    return EgressGate::NotApproved;
                }
                (IoDecision::Approve, rule)
            }
            EgressDecision::Deny { rule, reason } => {
                self.sign_io_deny(&target, rule, reason.clone());
                return EgressGate::Denied(reason);
            }
        };

        // Byte budget (B2) — estimate outbound payload from the serialized request arguments.
        let est = serde_json::to_vec(params)
            .map(|v| v.len() as u64)
            .unwrap_or(0);
        if let Err(reason) = control.charge_budget(&target.host, est, now_ms()) {
            self.sign_io_deny(&target, policy_rule, reason.clone());
            return EgressGate::Denied(reason);
        }

        let approved_by = if decision == IoDecision::Approve {
            self.actor.as_ref().map(|a| a.user.clone())
        } else {
            None
        };

        // Fail-closed receipt-precondition (B3): sign the io allow receipt BEFORE execute; if it
        // cannot be persisted, deny the egress — "no receipt, no egress." The honest cost is that
        // the pre-execute record commits to the serialized-request estimate (canonical-json), not
        // the transport-observed wire bytes, so its `hash_scheme` says `canonical-json`.
        let pre_signed = if control.policy.fail_closed() {
            let io = self.io_allow_record(&target, decision, &policy_rule, &approved_by, None, est);
            let receipt = Receipt::new(
                uuid::Uuid::new_v4().to_string(),
                io.action_id(),
                io.params(Some(action_step_id)),
                true,
                now_ms(),
            )
            .with_actor(self.actor.clone());
            if self.signer.record_persisted(receipt).is_err() {
                return EgressGate::Denied(
                    "egress blocked: kriya.io receipt could not be written (fail-closed)"
                        .to_string(),
                );
            }
            true
        } else {
            false
        };

        EgressGate::Cleared(Box::new(EgressCtx {
            target,
            decision,
            policy_rule,
            approved_by,
            pre_signed,
        }))
    }

    /// Emit the `kriya.io.*` allow/approve receipt after a governed egress executed, preferring the
    /// transport-observed io (accurate wire bytes) when present.
    fn emit_io_allow(&self, ctx: &EgressCtx, transport_io: Option<&IoRecord>, corr: &str) {
        let est = 0; // unused when transport io is present
        let io = self.io_allow_record(
            &ctx.target,
            ctx.decision,
            &ctx.policy_rule,
            &ctx.approved_by,
            transport_io,
            est,
        );
        let receipt = Receipt::new(
            uuid::Uuid::new_v4().to_string(),
            io.action_id(),
            io.params(Some(corr)),
            true,
            now_ms(),
        )
        .with_actor(self.actor.clone());
        self.signer.record(receipt);
    }

    /// Build the io record for an allowed/approved egress. With `transport_io` (fail-open,
    /// post-execute) it carries the observed wire bytes + `wire-bytes` scheme; without it
    /// (fail-closed, pre-execute) it commits to the serialized-request estimate + `canonical-json`.
    fn io_allow_record(
        &self,
        target: &EgressTarget,
        decision: IoDecision,
        policy_rule: &Option<String>,
        approved_by: &Option<String>,
        transport_io: Option<&IoRecord>,
        est_bytes: u64,
    ) -> IoRecord {
        match transport_io {
            Some(t) => IoRecord {
                direction: IoDirection::Egress,
                dest_host: t.dest_host.clone().or_else(|| Some(target.host.clone())),
                dest_kind: target.kind,
                method: t.method.clone(),
                bytes_out: t.bytes_out,
                bytes_in: t.bytes_in,
                bytes_in_is_partial: t.bytes_in_is_partial,
                content_sha256: t.content_sha256.clone(),
                hash_scheme: t.hash_scheme,
                decision,
                policy_rule: policy_rule.clone(),
                approved_by: approved_by.clone(),
                reason: None,
                server: target.server.clone(),
            },
            None => IoRecord {
                direction: IoDirection::Egress,
                dest_host: Some(target.host.clone()),
                dest_kind: target.kind,
                method: Some("tools/call".to_string()),
                bytes_out: Some(est_bytes),
                bytes_in: None,
                bytes_in_is_partial: false,
                content_sha256: None,
                hash_scheme: HashScheme::CanonicalJson,
                decision,
                policy_rule: policy_rule.clone(),
                approved_by: approved_by.clone(),
                reason: None,
                server: target.server.clone(),
            },
        }
    }

    /// Sign a `kriya.io.*.deny` receipt at the decision point — a denied egress never executes, so
    /// without this the `deny` rows would never exist (L10). No `corr`: there is no action receipt.
    fn sign_io_deny(&self, target: &EgressTarget, policy_rule: Option<String>, reason: String) {
        let io = IoRecord {
            direction: IoDirection::Egress,
            dest_host: Some(target.host.clone()),
            dest_kind: target.kind,
            method: None,
            bytes_out: None,
            bytes_in: None,
            bytes_in_is_partial: false,
            content_sha256: None,
            hash_scheme: HashScheme::WireBytes,
            decision: IoDecision::Deny,
            policy_rule,
            approved_by: None,
            reason: Some(reason),
            server: target.server.clone(),
        };
        let receipt = Receipt::new(
            uuid::Uuid::new_v4().to_string(),
            io.action_id(),
            io.params(None),
            false,
            now_ms(),
        )
        .with_actor(self.actor.clone());
        self.signer.record(receipt);
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
        assert!(matches!(
            g.dispatch("wire_money", &json!({})),
            DispatchOutcome::Denied
        ));
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "denied action must not execute"
        );
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
        assert!(matches!(
            g.dispatch("delete_note", &json!({"id": 1})),
            DispatchOutcome::NotApproved
        ));
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "unapproved action must not execute"
        );
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
        assert!(matches!(
            g.dispatch("create_note", &json!({})),
            DispatchOutcome::Executed { .. }
        ));
        assert!(matches!(
            g.dispatch("create_note", &json!({})),
            DispatchOutcome::Executed { .. }
        ));
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
            Box::new(FnExecutor(|_id: &str, _p: &Value| {
                ActionOutcome::failed("boom")
            })),
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

    // ─── EG-2: the egress tier + kriya.io.* receipts (doc 24 §7.3) ───────────────────────────────

    use crate::permissions::EgressPolicy;
    use std::path::PathBuf;

    /// A signer over a UNIQUE temp log so a test can read back the receipts it wrote.
    fn signer_with_log() -> (Arc<Signer>, PathBuf) {
        let log = std::env::temp_dir().join(format!("kriya-eg2-{}.jsonl", uuid::Uuid::new_v4()));
        (Arc::new(Signer::with_log_path(log.clone())), log)
    }

    /// An action policy that clears everything so the egress gate is what decides.
    fn allow_all() -> Arc<Policy> {
        Arc::new(serde_yaml::from_str("rules:\n  - action: \"*\"\n    allow: true\nbudget:\n  max_actions_per_minute: 1000\n").unwrap())
    }

    /// A governor whose egress control resolves every action to `host` (kind mcp) under `egress_yaml`.
    fn egress_governor(
        signer: Arc<Signer>,
        egress_yaml: &str,
        host: &str,
        ran: Arc<AtomicUsize>,
        approval: Box<dyn ApprovalGate>,
    ) -> Governor {
        let ep: EgressPolicy = serde_yaml::from_str(egress_yaml).unwrap();
        let host = host.to_string();
        let control = EgressControl::new(ep, move |_a: &str, _p: &Value| {
            Some(EgressTarget {
                host: host.clone(),
                kind: IoKind::Mcp,
                server: Some("test-upstream".into()),
            })
        });
        Governor::new(allow_all(), signer, approval, counting_executor(ran)).with_egress(control)
    }

    fn read_receipts(log: &std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn io_lines(log: &std::path::Path) -> Vec<Value> {
        read_receipts(log)
            .into_iter()
            .filter(|v| {
                v["action_id"]
                    .as_str()
                    .map(|a| a.starts_with("kriya.io."))
                    .unwrap_or(false)
            })
            .collect()
    }

    #[test]
    fn egress_allow_executes_and_emits_an_io_allow_receipt() {
        let (s, log) = signer_with_log();
        let ran = Arc::new(AtomicUsize::new(0));
        let mut g = egress_governor(
            s,
            "rules:\n  - host: \"*.vendor.com\"\n    tier: allow\nunlisted: deny\n",
            "api.vendor.com",
            ran.clone(),
            Box::new(DenyApproval),
        );
        assert!(matches!(
            g.dispatch("widgets__list", &json!({"q": 1})),
            DispatchOutcome::Executed { .. }
        ));
        assert_eq!(ran.load(Ordering::SeqCst), 1, "allowed egress runs");
        let io = io_lines(&log);
        assert_eq!(io.len(), 1, "one io receipt: {io:?}");
        assert_eq!(io[0]["action_id"], "kriya.io.egress.mcp.allow");
        assert_eq!(io[0]["params"]["dest_host"], "api.vendor.com");
        assert_eq!(io[0]["params"]["policy_rule"], "*.vendor.com");
        // corr joins the io receipt to the action receipt (never adjacency, L5).
        let action = read_receipts(&log)
            .into_iter()
            .find(|v| v["action_id"] == "widgets__list")
            .unwrap();
        assert_eq!(io[0]["params"]["corr"], action["step_id"]);
    }

    #[test]
    fn egress_deny_blocks_at_the_decision_point_and_emits_a_deny_receipt() {
        let (s, log) = signer_with_log();
        let ran = Arc::new(AtomicUsize::new(0));
        // Deny-by-default; the destination is not listed → deny before execute (L10).
        let mut g = egress_governor(
            s,
            "unlisted: deny\nrules: []\n",
            "evil.example",
            ran.clone(),
            Box::new(AutoApprove),
        );
        assert!(matches!(
            g.dispatch("widgets__list", &json!({})),
            DispatchOutcome::EgressDenied(_)
        ));
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "denied egress must NOT execute"
        );
        let io = io_lines(&log);
        assert_eq!(io.len(), 1);
        assert_eq!(io[0]["action_id"], "kriya.io.egress.mcp.deny");
        assert_eq!(io[0]["params"]["decision"], "deny");
        // A decision-point deny has no action receipt, so no corr — the deny row stands alone.
        assert!(io[0]["params"].get("corr").is_none());
        // No action receipt was written for the blocked call.
        assert!(read_receipts(&log)
            .iter()
            .all(|v| v["action_id"] != "widgets__list"));
    }

    #[test]
    fn egress_approval_tier_rides_the_approval_gate() {
        // Granted → executes + an `approve`-facet io receipt.
        let (s, log) = signer_with_log();
        let ran = Arc::new(AtomicUsize::new(0));
        let mut g = egress_governor(
            s,
            "rules:\n  - host: \"api.partner.com\"\n    tier: approval\nunlisted: deny\n",
            "api.partner.com",
            ran.clone(),
            Box::new(AutoApprove),
        );
        assert!(matches!(
            g.dispatch("widgets__list", &json!({})),
            DispatchOutcome::Executed { .. }
        ));
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert_eq!(
            io_lines(&log)[0]["action_id"],
            "kriya.io.egress.mcp.approve"
        );

        // Refused → NotApproved, not executed, and a deny receipt records the parked attempt.
        let (s2, log2) = signer_with_log();
        let ran2 = Arc::new(AtomicUsize::new(0));
        let mut g2 = egress_governor(
            s2,
            "rules:\n  - host: \"api.partner.com\"\n    tier: approval\nunlisted: deny\n",
            "api.partner.com",
            ran2.clone(),
            Box::new(DenyApproval),
        );
        assert!(matches!(
            g2.dispatch("widgets__list", &json!({})),
            DispatchOutcome::NotApproved
        ));
        assert_eq!(ran2.load(Ordering::SeqCst), 0);
        assert_eq!(io_lines(&log2)[0]["action_id"], "kriya.io.egress.mcp.deny");
    }

    #[test]
    fn egress_byte_budget_breach_denies_with_a_receipt() {
        let (s, log) = signer_with_log();
        let ran = Arc::new(AtomicUsize::new(0));
        // A tiny budget: each call's serialized args (~a few bytes) accumulate; the call that would
        // breach is denied (anti slow-drip exfil, B2).
        let mut g = egress_governor(
            s,
            "rules:\n  - host: \"*.vendor.com\"\n    tier: allow\n    budget: { window_secs: 60, max_bytes: 40 }\n",
            "api.vendor.com",
            ran.clone(),
            Box::new(DenyApproval),
        );
        // Args serialize to ~15 bytes each; the third call crosses 40 bytes → deny.
        let mut denied = false;
        for i in 0..5 {
            match g.dispatch("widgets__list", &json!({"n": i, "pad": "xxxx"})) {
                DispatchOutcome::EgressDenied(reason) => {
                    assert!(reason.contains("byte budget"), "reason: {reason}");
                    denied = true;
                    break;
                }
                DispatchOutcome::Executed { .. } => {}
                other => panic!("unexpected: {other:?}"),
            }
        }
        assert!(denied, "the cumulative byte budget must eventually deny");
        let deny = io_lines(&log)
            .into_iter()
            .find(|v| v["action_id"] == "kriya.io.egress.mcp.deny")
            .expect("a byte-budget deny receipt");
        assert!(deny["params"]["reason"]
            .as_str()
            .unwrap()
            .contains("byte budget"));
    }

    #[test]
    fn fail_closed_denies_egress_when_the_receipt_cannot_be_written() {
        // Fault injection: the signer's log path is a DIRECTORY, so no receipt can be persisted.
        // In fail-closed mode the io receipt is a precondition — the egress is denied and the
        // executor never runs ("no receipt, no egress", B3).
        let dir = std::env::temp_dir().join(format!("kriya-eg2-fc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let signer = Arc::new(Signer::with_log_path(dir.clone())); // path is a directory → unwritable
        let ran = Arc::new(AtomicUsize::new(0));
        let mut g = egress_governor(
            signer,
            "fail_closed: true\nrules:\n  - host: \"*.vendor.com\"\n    tier: allow\n",
            "api.vendor.com",
            ran.clone(),
            Box::new(DenyApproval),
        );
        match g.dispatch("widgets__list", &json!({})) {
            DispatchOutcome::EgressDenied(reason) => {
                assert!(reason.contains("fail-closed"), "reason: {reason}")
            }
            other => panic!("fail-closed must deny, got {other:?}"),
        }
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "no receipt, no egress: the executor must not run"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn budget_block_does_not_leak_a_fail_closed_io_receipt() {
        // Regression: the action budget must be checked BEFORE the egress gate, or a budget-blocked
        // call in fail-closed mode would still persist a `kriya.io.*.allow` precondition receipt —
        // recording an egress that never happened, with a dangling corr.
        let (s, log) = signer_with_log();
        let ran = Arc::new(AtomicUsize::new(0));
        let policy = Arc::new(
            serde_yaml::from_str::<Policy>(
                "rules:\n  - action: \"*\"\n    allow: true\nbudget:\n  max_actions_per_minute: 1\n",
            )
            .unwrap(),
        );
        let ep: EgressPolicy = serde_yaml::from_str(
            "fail_closed: true\nrules:\n  - host: \"*.vendor.com\"\n    tier: allow\n",
        )
        .unwrap();
        let control = EgressControl::new(ep, move |_a: &str, _p: &Value| {
            Some(EgressTarget {
                host: "api.vendor.com".into(),
                kind: IoKind::Mcp,
                server: None,
            })
        });
        let mut g = Governor::new(
            policy,
            s,
            Box::new(DenyApproval),
            counting_executor(ran.clone()),
        )
        .with_egress(control);
        assert!(matches!(
            g.dispatch("widgets__list", &json!({})),
            DispatchOutcome::Executed { .. }
        ));
        // The second call trips the per-minute budget — BEFORE the egress precondition is signed.
        assert!(matches!(
            g.dispatch("widgets__list", &json!({})),
            DispatchOutcome::BudgetExceeded(_)
        ));
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        let io = io_lines(&log);
        assert_eq!(
            io.len(),
            1,
            "the budget-blocked call must leak no io receipt: {io:?}"
        );
        // The one io receipt's corr resolves to the one action receipt (no dangling corr).
        let actions: Vec<Value> = read_receipts(&log)
            .into_iter()
            .filter(|v| v["action_id"] == "widgets__list")
            .collect();
        assert_eq!(actions.len(), 1);
        assert_eq!(io[0]["params"]["corr"], actions[0]["step_id"]);
    }

    #[test]
    fn corr_joins_action_and_io_across_interleaved_dispatches() {
        // Two governors share ONE signer + log; alternating dispatches interleave their receipts.
        // Every io receipt must still join its action receipt by `corr`, never by adjacency (L5).
        let (s, log) = signer_with_log();
        let ep = "rules:\n  - host: \"*.vendor.com\"\n    tier: allow\n";
        let mk = |ran: Arc<AtomicUsize>| {
            let control = EgressControl::new(
                serde_yaml::from_str::<EgressPolicy>(ep).unwrap(),
                move |_a: &str, _p: &Value| {
                    Some(EgressTarget {
                        host: "api.vendor.com".into(),
                        kind: IoKind::Mcp,
                        server: None,
                    })
                },
            );
            Governor::new(
                allow_all(),
                s.clone(),
                Box::new(DenyApproval),
                counting_executor(ran),
            )
            .with_egress(control)
        };
        let mut g1 = mk(Arc::new(AtomicUsize::new(0)));
        let mut g2 = mk(Arc::new(AtomicUsize::new(0)));
        for i in 0..3 {
            g1.dispatch("a__list", &json!({"g": 1, "i": i}));
            g2.dispatch("b__list", &json!({"g": 2, "i": i}));
        }
        let receipts = read_receipts(&log);
        let action_ids: std::collections::HashSet<String> = receipts
            .iter()
            .filter(|v| !v["action_id"].as_str().unwrap().starts_with("kriya.io."))
            .map(|v| v["step_id"].as_str().unwrap().to_string())
            .collect();
        let io: Vec<&Value> = receipts
            .iter()
            .filter(|v| v["action_id"].as_str().unwrap().starts_with("kriya.io."))
            .collect();
        assert_eq!(io.len(), 6, "one io receipt per dispatch");
        for r in io {
            let corr = r["params"]["corr"]
                .as_str()
                .expect("io receipt carries corr");
            assert!(
                action_ids.contains(corr),
                "every io receipt's corr must resolve to an action receipt's step_id, even interleaved"
            );
        }
    }
}
