//! The agent host: the step loop that turns a goal into a sequence of permission-checked,
//! signed action calls against the app. Runs on a blocking thread; coordinates with the
//! frontend over Tauri events (out) and a per-step channel (results back in).

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::audit::{now_ms, Receipt, Signer};
use crate::permissions::{Decision, Policy};
use crate::protocol::{
    AgentActionRequest, AgentActionResult, AgentDone, AgentLog, AgentStartRequest, EVENT_ACTION,
    EVENT_DONE, EVENT_LOG,
};

use super::inference::{select_backend, StepContext, StepDecision, StepRecord};

/// Shared registry of in-flight steps awaiting a result from the frontend.
pub type PendingMap = Arc<Mutex<std::collections::HashMap<String, Sender<AgentActionResult>>>>;

const MAX_STEPS: u32 = 50;
const RESULT_TIMEOUT: Duration = Duration::from_secs(30);

fn log(app: &AppHandle, entry: AgentLog) {
    let _ = app.emit(EVENT_LOG, entry);
}

pub fn run_task(
    app: AppHandle,
    pending: PendingMap,
    policy: Arc<Policy>,
    signer: Arc<Signer>,
    req: AgentStartRequest,
) -> Result<AgentDone, String> {
    let mut backend = select_backend();
    log(
        &app,
        AgentLog::info(format!(
            "backend={} · audit pubkey={} · log={}",
            backend.name(),
            signer.public_key(),
            signer.log_path().display()
        )),
    );

    let mut state = req.state.clone();
    let mut history: Vec<StepRecord> = Vec::new();
    let mut steps: u32 = 0;

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

        // Permission gate — the host decides, not the agent.
        match policy.check(&action_id) {
            Decision::Allow => {}
            Decision::RequiresApproval => {
                log(
                    &app,
                    AgentLog::warn(format!(
                        "{action_id} requires human approval; no approval queue in Phase 0 — skipping."
                    )),
                );
                history.push(StepRecord { action_id, params, success: false });
                continue;
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

        // Dispatch to the frontend and wait for it to run the handler.
        let step_id = uuid::Uuid::new_v4().to_string();
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

        history.push(StepRecord {
            action_id,
            params,
            success: result.success,
        });
        state = result.state;
        steps += 1;
    }
}
