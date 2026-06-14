//! The agent host: the step loop that turns a goal into a sequence of permission-checked,
//! signed action calls against the app. Runs on a blocking thread; coordinates with the
//! frontend over Tauri events (out) and a per-step channel (results back in).

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::audit::{now_ms, Receipt, Signer};
use crate::budget::BudgetTracker;
use crate::memory::AgentMemory;
use crate::permissions::{Decision, Policy};
use crate::protocol::{
    AgentActionRequest, AgentActionResult, AgentApprovalRequest, AgentDone, AgentLog,
    AgentStartRequest, EVENT_ACTION, EVENT_APPROVAL, EVENT_DONE, EVENT_LOG,
};

use super::inference::{Inference, StepContext, StepDecision, StepRecord};

/// Shared registry of in-flight steps awaiting a result from the frontend.
pub type PendingMap = Arc<Mutex<std::collections::HashMap<String, Sender<AgentActionResult>>>>;

/// Shared registry of in-flight steps awaiting a human approval decision.
pub type ApprovalMap = Arc<Mutex<std::collections::HashMap<String, Sender<bool>>>>;

const MAX_STEPS: u32 = 50;
const RESULT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for a human to approve a held action before treating it as denied.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

fn log(app: &AppHandle, entry: AgentLog) {
    let _ = app.emit(EVENT_LOG, entry);
}

/// Ask the frontend for a human decision on a held action and block until it arrives
/// (or the timeout elapses, which counts as a denial). The signing key and policy stay
/// host-side; the agent can only *propose* an action that needs approval.
fn request_approval(
    app: &AppHandle,
    approvals: &ApprovalMap,
    step_id: &str,
    action_id: &str,
    params: &serde_json::Value,
    reasoning: &str,
) -> bool {
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    approvals.lock().unwrap().insert(step_id.to_string(), tx);

    log(
        app,
        AgentLog {
            step_id: Some(step_id.to_string()),
            level: "warn".into(),
            message: format!("{action_id} requires approval — waiting for a human…"),
            detail: None,
        },
    );
    let _ = app.emit(
        EVENT_APPROVAL,
        AgentApprovalRequest {
            step_id: step_id.to_string(),
            action_id: action_id.to_string(),
            params: params.clone(),
            reasoning: reasoning.to_string(),
        },
    );

    let approved = rx.recv_timeout(APPROVAL_TIMEOUT).unwrap_or(false);
    approvals.lock().unwrap().remove(step_id);
    approved
}

pub fn run_task(
    app: AppHandle,
    pending: PendingMap,
    approvals: ApprovalMap,
    policy: Arc<Policy>,
    signer: Arc<Signer>,
    mut backend: Box<dyn Inference>,
    req: AgentStartRequest,
) -> Result<AgentDone, String> {
    // Stable id for this run. Stamped on every episode written below, so a crashed
    // run can be reconstructed end-to-end from durable memory.
    let run_id = uuid::Uuid::new_v4().to_string();

    log(
        &app,
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
        log(&app, AgentLog::warn(format!("policy: {concern}")));
    }

    // Durable episodic memory across runs. Failure to open is non-fatal — the agent
    // still works, just without persistent recall.
    let memory = AgentMemory::open(&std::env::temp_dir().join("agent-native-memory.db")).ok();
    // Recall a window of prior-run actions to feed the backend as context.
    let recent_memory: Vec<String> = memory
        .as_ref()
        .and_then(|m| {
            if let Ok(n) = m.count() {
                log(&app, AgentLog::info(format!("memory: {n} past episodes on record")));
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
                        &app,
                        AgentLog::info(format!(
                            "resumed from prior run {} — {} completed step(s) reseeded",
                            &prior.run_id, count
                        )),
                    );
                }
                Ok(None) => {
                    log(
                        &app,
                        AgentLog::info(format!(
                            "resume requested but no prior run found for this goal — starting fresh"
                        )),
                    );
                }
                Err(err) => {
                    log(
                        &app,
                        AgentLog::warn(format!("resume lookup failed: {err} — starting fresh")),
                    );
                }
            }
        }
    }

    loop {
        if steps >= MAX_STEPS {
            let done = AgentDone { summary: "Stopped: reached step limit.".into(), steps };
            let _ = app.emit(EVENT_DONE, &done);
            return Ok(done);
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
                let _ = app.emit(EVENT_DONE, &done);
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
                    &app,
                    &approvals,
                    &step_id,
                    &action_id,
                    &params,
                    &reasoning,
                );
                if !approved {
                    log(
                        &app,
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
                    &app,
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
                    &app,
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
                &app,
                AgentLog {
                    step_id: Some(step_id.clone()),
                    level: "error".into(),
                    message: format!("{action_id} blocked — {reason}"),
                    detail: None,
                },
            );
            let _ = app.emit(EVENT_DONE, &done);
            return Ok(done);
        }

        // Dispatch to the frontend and wait for it to run the handler.
        let (tx, rx) = std::sync::mpsc::channel::<AgentActionResult>();
        pending.lock().unwrap().insert(step_id.clone(), tx);

        let _ = app.emit(
            EVENT_ACTION,
            AgentActionRequest {
                step_id: step_id.clone(),
                action_id: action_id.clone(),
                params: params.clone(),
                reasoning: reasoning.clone(),
            },
        );

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
                    &app,
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
            &app,
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

        history.push(StepRecord {
            action_id,
            params,
            success: result.success,
        });
        state = result.state;
        steps += 1;
    }
}
