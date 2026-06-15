//! The agent host: the step loop that turns a goal into a sequence of permission-checked,
//! signed action calls against the app. Runs on a blocking thread; coordinates with the
//! frontend over Tauri events (out) and a per-step channel (results back in).

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::audit::{now_ms, Receipt, Signer};
use crate::budget::BudgetTracker;
use crate::memory::AgentMemory;
use crate::permissions::{Decision, Policy};
use crate::protocol::{
    AgentActionRequest, AgentActionResult, AgentApprovalRequest, AgentAwaitStep, AgentDone,
    AgentLog, AgentStartRequest,
};

use super::inference::{Inference, StepContext, StepDecision, StepRecord};
use super::transport::HostSink;

/// Shared registry of in-flight steps awaiting a result from the frontend.
pub type PendingMap = Arc<Mutex<std::collections::HashMap<String, Sender<AgentActionResult>>>>;

/// Shared registry of in-flight steps awaiting a human approval decision.
pub type ApprovalMap = Arc<Mutex<std::collections::HashMap<String, Sender<bool>>>>;

/// Shared registry of step-mode gates awaiting an advance/stop decision.
pub type StepAdvanceMap = Arc<Mutex<std::collections::HashMap<String, Sender<bool>>>>;

const MAX_STEPS: u32 = 50;
const RESULT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for a human to approve a held action before treating it as denied.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
/// How long to wait for a step-mode advance before treating it as a stop. Same budget as
/// approval so a dev who walks away doesn't leave the host blocked forever.
const STEP_TIMEOUT: Duration = Duration::from_secs(300);

fn log(sink: &dyn HostSink, entry: AgentLog) {
    sink.emit_log(&entry);
}

/// Ask the app for a human decision on a held action and block until it arrives
/// (or the timeout elapses, which counts as a denial). The signing key and policy stay
/// host-side; the agent can only *propose* an action that needs approval.
fn request_approval(
    sink: &dyn HostSink,
    approvals: &ApprovalMap,
    step_id: &str,
    action_id: &str,
    params: &serde_json::Value,
    reasoning: &str,
) -> bool {
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    approvals.lock().unwrap().insert(step_id.to_string(), tx);

    log(
        sink,
        AgentLog {
            step_id: Some(step_id.to_string()),
            level: "warn".into(),
            message: format!("{action_id} requires approval — waiting for a human…"),
            detail: None,
        },
    );
    sink.emit_approval(&AgentApprovalRequest {
        step_id: step_id.to_string(),
        action_id: action_id.to_string(),
        params: params.clone(),
        reasoning: reasoning.to_string(),
    });

    let approved = rx.recv_timeout(APPROVAL_TIMEOUT).unwrap_or(false);
    approvals.lock().unwrap().remove(step_id);
    approved
}

/// Pause the loop and wait for the app to send `agent_step_advance`.
/// Returns `true` to proceed, `false` to stop the run (developer hit Stop or the
/// timeout elapsed). Step-mode only.
fn await_step(
    sink: &dyn HostSink,
    advances: &StepAdvanceMap,
    step_number: u32,
    last_action_id: Option<&str>,
    last_success: Option<bool>,
) -> bool {
    let gate_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    advances.lock().unwrap().insert(gate_id.clone(), tx);

    log(
        sink,
        AgentLog::info(format!(
            "step-mode: paused before step {step_number} (gate {})",
            &gate_id[..gate_id.len().min(8)]
        )),
    );

    sink.emit_await_step(&AgentAwaitStep {
        gate_id: gate_id.clone(),
        step_number,
        last_action_id: last_action_id.map(str::to_string),
        last_success,
    });

    let proceed = rx.recv_timeout(STEP_TIMEOUT).unwrap_or(false);
    advances.lock().unwrap().remove(&gate_id);
    proceed
}

/// Run the agent loop, sending events through `sink` (Tauri, sidecar, or test recorder).
/// Results flow back through the shared channel maps, whatever the transport.
pub fn run_task(
    sink: Arc<dyn HostSink>,
    pending: PendingMap,
    approvals: ApprovalMap,
    advances: StepAdvanceMap,
    policy: Arc<Policy>,
    signer: Arc<Signer>,
    mut backend: Box<dyn Inference>,
    req: AgentStartRequest,
) -> Result<AgentDone, String> {
    let sink: &dyn HostSink = sink.as_ref();
    // Stable id for this run. Stamped on every episode written below, so a crashed
    // run can be reconstructed end-to-end from durable memory.
    let run_id = uuid::Uuid::new_v4().to_string();

    log(
        sink,
        AgentLog::info(format!(
            "backend={} · run_id={} · audit pubkey={} · log={}",
            backend.name(),
            &run_id,
            signer.public_key(),
            signer.log_path().display()
        )),
    );

    // Lint the policy at run start. Each concern surfaces as a warn so devs
    // notice obviously dangerous configurations the first time they hit Run.
    for concern in policy.warnings() {
        log(sink, AgentLog::warn(format!("policy: {concern}")));
    }

    // Durable episodic memory across runs. Failure to open is non-fatal — the agent
    // still works, just without persistent recall.
    let memory = AgentMemory::open(&std::env::temp_dir().join("kriya-memory.db")).ok();
    // Recall a window of prior-run actions to feed the backend as context.
    let recent_memory: Vec<String> = memory
        .as_ref()
        .and_then(|m| {
            if let Ok(n) = m.count() {
                log(sink, AgentLog::info(format!("memory: {n} past episodes on record")));
            }
            m.recent(8).ok()
        })
        .map(|eps| {
            eps.iter()
                .map(|e| {
                    format!(
                        "{} {} -> {}",
                        e.action_id,
                        e.params,
                        if e.success { "ok" } else { "failed" }
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    let mut state = req.state.clone();
    let mut history: Vec<StepRecord> = Vec::new();
    let mut steps: u32 = 0;
    let mut budget = BudgetTracker::new(policy.max_actions_per_minute());

    // Resume path: if requested and we have a prior run for this goal, reseed
    // history so the inference backend doesn't re-propose actions already taken.
    // The new run gets its own run_id; resumed steps are NOT re-recorded.
    if req.resume {
        if let Some(m) = &memory {
            match m.last_resumable_run(&req.goal) {
                Ok(Some(prior)) => {
                    let count = prior.completed.len();
                    for step in prior.completed {
                        history.push(StepRecord {
                            action_id: step.action_id,
                            params: step.params,
                            success: step.success,
                        });
                    }
                    log(
                        sink,
                        AgentLog::info(format!(
                            "resumed from prior run {} — {} completed step(s) reseeded",
                            &prior.run_id, count
                        )),
                    );
                }
                Ok(None) => {
                    log(
                        sink,
                        AgentLog::info(format!(
                            "resume requested but no prior run found for this goal — starting fresh"
                        )),
                    );
                }
                Err(err) => {
                    log(
                        sink,
                        AgentLog::warn(format!("resume lookup failed: {err} — starting fresh")),
                    );
                }
            }
        }
    }

    // Tracks the previous step's outcome to surface in each step-mode pause.
    let mut last_action_id: Option<String> = None;
    let mut last_success: Option<bool> = None;

    loop {
        if steps >= MAX_STEPS {
            let done = AgentDone { summary: "Stopped: reached step limit.".into(), steps };
            sink.emit_done(&done);
            return Ok(done);
        }

        // Step-mode: pause and wait for the developer to advance.
        if req.step_mode {
            let proceed = await_step(
                sink,
                &advances,
                steps + 1,
                last_action_id.as_deref(),
                last_success,
            );
            if !proceed {
                let done = AgentDone {
                    summary: "Stopped by developer (step-mode).".into(),
                    steps,
                };
                log(sink, AgentLog::info("step-mode: stop requested".to_string()));
                sink.emit_done(&done);
                return Ok(done);
            }
        }

        let decision = {
            let ctx = StepContext {
                goal: &req.goal,
                state: &state,
                tools: &req.tools,
                history: &history,
                recent_memory: &recent_memory,
            };
            backend.next_step(&ctx)?
        };

        let (action_id, params, reasoning) = match decision {
            StepDecision::Done { summary } => {
                let done = AgentDone { summary, steps };
                sink.emit_done(&done);
                return Ok(done);
            }
            StepDecision::Call { action_id, params, reasoning } => (action_id, params, reasoning),
        };

        // One id correlates this step across the approval request, the action request, and
        // the signed receipt.
        let step_id = uuid::Uuid::new_v4().to_string();

        // Permission gate — the host decides, not the agent.
        match policy.check(&action_id) {
            Decision::Allow => {}
            Decision::RequiresApproval => {
                let approved = request_approval(
                    sink,
                    &approvals,
                    &step_id,
                    &action_id,
                    &params,
                    &reasoning,
                );
                if !approved {
                    log(
                        sink,
                        AgentLog {
                            step_id: Some(step_id.clone()),
                            level: "warn".into(),
                            message: format!("{action_id} not approved — skipped."),
                            detail: None,
                        },
                    );
                    history.push(StepRecord { action_id, params, success: false });
                    continue;
                }
                log(
                    sink,
                    AgentLog {
                        step_id: Some(step_id.clone()),
                        level: "info".into(),
                        message: format!("{action_id} approved by human."),
                        detail: None,
                    },
                );
            }
            Decision::Deny => {
                log(
                    sink,
                    AgentLog::warn(format!("{action_id} denied by policy.")),
                );
                history.push(StepRecord { action_id, params, success: false });
                continue;
            }
        }

        // Rate-limit gate: stop a runaway/looping agent before it acts.
        if let Err(reason) = budget.check_and_record(now_ms()) {
            let done = AgentDone { summary: format!("Stopped: {reason}."), steps };
            log(
                sink,
                AgentLog {
                    step_id: Some(step_id.clone()),
                    level: "error".into(),
                    message: format!("{action_id} blocked — {reason}"),
                    detail: None,
                },
            );
            sink.emit_done(&done);
            return Ok(done);
        }

        // Dispatch to the app and wait for it to run the handler (transport-agnostic).
        let (tx, rx) = std::sync::mpsc::channel::<AgentActionResult>();
        pending.lock().unwrap().insert(step_id.clone(), tx);

        sink.emit_action(&AgentActionRequest {
            step_id: step_id.clone(),
            action_id: action_id.clone(),
            params: params.clone(),
            reasoning: reasoning.clone(),
        });

        let result = match rx.recv_timeout(RESULT_TIMEOUT) {
            Ok(r) => r,
            Err(_) => {
                pending.lock().unwrap().remove(&step_id);
                return Err(format!("timed out waiting for result of {action_id}"));
            }
        };

        if !result.success {
            if let Some(err) = &result.error {
                log(
                    sink,
                    AgentLog {
                        step_id: Some(step_id.clone()),
                        level: "warn".into(),
                        message: format!("{action_id} returned an error: {err}"),
                        detail: None,
                    },
                );
            }
        }

        // Sign and record the receipt regardless of success.
        let signed = signer.record(Receipt {
            step_id: step_id.clone(),
            action_id: action_id.clone(),
            params: params.clone(),
            success: result.success,
            ts_ms: now_ms(),
        });
        log(
            sink,
            AgentLog {
                step_id: Some(step_id.clone()),
                level: "info".into(),
                message: format!(
                    "receipt signed · sig={}…",
                    &signed.signature[..signed.signature.len().min(16)]
                ),
                detail: None,
            },
        );

        // Persist this action to durable episodic memory, stamped with the run_id +
        // goal so resume can reconstruct this run if the process dies.
        if let Some(m) = &memory {
            let _ = m.record(
                signed.receipt.ts_ms,
                &run_id,
                &req.goal,
                &action_id,
                &params,
                result.success,
                &reasoning,
                &signed.signature,
            );
        }

        last_action_id = Some(action_id.clone());
        last_success = Some(result.success);
        history.push(StepRecord {
            action_id,
            params,
            success: result.success,
        });
        state = result.state;
        steps += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ToolSchema, AgentActionResult};
    use serde_json::{json, Value};
    use std::collections::HashMap;

    /// A scripted backend: plays back a fixed list of decisions, then is `Done`. Proves the
    /// loop runs with no LLM and no Tauri — the same device the reference apps use, generic.
    struct ScriptedBackend {
        steps: std::vec::IntoIter<StepDecision>,
    }

    impl ScriptedBackend {
        fn new(steps: Vec<StepDecision>) -> Self {
            Self { steps: steps.into_iter() }
        }
    }

    impl Inference for ScriptedBackend {
        fn name(&self) -> &'static str {
            "scripted-test"
        }
        fn next_step(&mut self, _ctx: &StepContext) -> Result<StepDecision, String> {
            Ok(self
                .steps
                .next()
                .unwrap_or(StepDecision::Done { summary: "script exhausted".into() }))
        }
    }

    /// A test sink that records event tags AND, because `emit_action` runs inline on the loop
    /// thread *after* the pending channel is registered, feeds a synchronous success result
    /// back through the pending map. That lets `run_task` complete on a single thread with no
    /// sleeps — and proves the loop drives entirely through the `HostSink` seam, not Tauri.
    struct RecordingSink {
        events: Mutex<Vec<String>>,
        pending: PendingMap,
        reply_state: Value,
    }

    impl HostSink for RecordingSink {
        fn emit_action(&self, req: &AgentActionRequest) {
            self.events.lock().unwrap().push(format!("action:{}", req.action_id));
            if let Some(tx) = self.pending.lock().unwrap().remove(&req.step_id) {
                let _ = tx.send(AgentActionResult {
                    step_id: req.step_id.clone(),
                    success: true,
                    data: Value::Null,
                    error: None,
                    state: self.reply_state.clone(),
                });
            }
        }
        fn emit_approval(&self, req: &AgentApprovalRequest) {
            self.events.lock().unwrap().push(format!("approval:{}", req.action_id));
        }
        fn emit_await_step(&self, _ev: &AgentAwaitStep) {
            self.events.lock().unwrap().push("await_step".into());
        }
        fn emit_done(&self, done: &AgentDone) {
            self.events.lock().unwrap().push(format!("done:{}", done.steps));
        }
        fn emit_log(&self, _entry: &AgentLog) {}
    }

    fn maps() -> (PendingMap, ApprovalMap, StepAdvanceMap) {
        (
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
        )
    }

    #[test]
    fn loop_runs_through_a_non_tauri_sink() {
        let (pending, approvals, advances) = maps();
        let sink = Arc::new(RecordingSink {
            events: Mutex::new(Vec::new()),
            pending: pending.clone(),
            reply_state: json!({"notes": []}),
        });

        let backend = Box::new(ScriptedBackend::new(vec![
            StepDecision::Call {
                action_id: "create_note".into(),
                params: json!({"title": "hi"}),
                reasoning: "first".into(),
            },
            StepDecision::Done { summary: "all done".into() },
        ]));

        let req = AgentStartRequest {
            goal: "make a note".into(),
            state: json!({"notes": []}),
            tools: Vec::<ToolSchema>::new(),
            resume: false,
            step_mode: false,
        };

        let done = run_task(
            sink.clone(),
            pending,
            approvals,
            advances,
            Arc::new(Policy::default()),
            Arc::new(Signer::new()),
            backend,
            req,
        )
        .expect("run_task");

        assert_eq!(done.steps, 1);
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(events, vec!["action:create_note".to_string(), "done:1".to_string()]);
    }

    #[test]
    fn denied_action_is_not_dispatched_to_the_sink() {
        let (pending, approvals, advances) = maps();
        let sink = Arc::new(RecordingSink {
            events: Mutex::new(Vec::new()),
            pending: pending.clone(),
            reply_state: Value::Null,
        });
        // `wire_money` matches no allow rule in the default policy → denied before dispatch.
        let backend = Box::new(ScriptedBackend::new(vec![
            StepDecision::Call {
                action_id: "wire_money".into(),
                params: json!({}),
                reasoning: "nope".into(),
            },
            StepDecision::Done { summary: "stop".into() },
        ]));
        let req = AgentStartRequest {
            goal: "should be blocked".into(),
            state: Value::Null,
            tools: Vec::new(),
            resume: false,
            step_mode: false,
        };
        run_task(
            sink.clone(),
            pending,
            approvals,
            advances,
            Arc::new(Policy::default()),
            Arc::new(Signer::new()),
            backend,
            req,
        )
        .expect("run_task");
        // No action event was emitted — the denied action never reached the sink/app.
        let events = sink.events.lock().unwrap().clone();
        assert!(events.iter().all(|e| !e.starts_with("action:")), "got: {events:?}");
    }
}
