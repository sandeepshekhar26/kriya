//! Tauri backend hosting the agent loop (delegated to the published `kriya` crate)
//! and exposing the IPC commands the frontend calls:
//! - `agent_start`             — begin an autonomous task
//! - `agent_action_result`     — return the outcome of an executed action
//! - `agent_approval_response` — return a human's approve/deny decision
//! - `agent_step_advance`      — developer's advance/stop decision in step-mode
//! - `agent_memory_recent`     — read recent episodes from durable memory

mod deterministic;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::Emitter;

use kriya::{
    audit::Signer,
    permissions::Policy,
    protocol::{
        AgentActionResult, AgentApprovalResponse, AgentDone, AgentLog, AgentStartRequest,
        AgentStepAdvance, EVENT_DONE, EVENT_LOG,
    },
    run_task, select_backend_with_default, ApprovalMap, HostSink, PendingMap, StepAdvanceMap,
    TauriSink,
};

use deterministic::DeterministicPlanner;

pub struct AppState {
    pending: PendingMap,
    approvals: ApprovalMap,
    advances: StepAdvanceMap,
    policy: Arc<Policy>,
    signer: Arc<Signer>,
}

#[tauri::command]
fn agent_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    req: AgentStartRequest,
) -> Result<(), String> {
    let pending = state.pending.clone();
    let approvals = state.approvals.clone();
    let advances = state.advances.clone();
    let policy = state.policy.clone();
    let signer = state.signer.clone();

    // Default to the zero-cost deterministic planner; AGENT_BACKEND swaps in a real LLM.
    let backend = select_backend_with_default(Box::new(DeterministicPlanner::new()));

    // The agent loop talks to any shell through a HostSink; Tauri is just one impl.
    let sink: Arc<dyn HostSink> = Arc::new(TauriSink::new(app.clone()));

    std::thread::spawn(move || {
        if let Err(err) = run_task(
            sink,
            pending,
            approvals,
            advances,
            policy,
            signer,
            backend,
            req,
        ) {
            let _ = app.emit(
                EVENT_LOG,
                AgentLog { step_id: None, level: "error".into(), message: err.clone(), detail: None },
            );
            let _ = app.emit(
                EVENT_DONE,
                AgentDone { summary: format!("Failed: {err}"), steps: 0 },
            );
        }
    });

    Ok(())
}

#[tauri::command]
fn agent_action_result(
    state: tauri::State<'_, AppState>,
    result: AgentActionResult,
) -> Result<(), String> {
    let tx = state.pending.lock().unwrap().remove(&result.step_id);
    if let Some(tx) = tx {
        let _ = tx.send(result);
    }
    Ok(())
}

#[tauri::command]
fn agent_approval_response(
    state: tauri::State<'_, AppState>,
    response: AgentApprovalResponse,
) -> Result<(), String> {
    let tx = state.approvals.lock().unwrap().remove(&response.step_id);
    if let Some(tx) = tx {
        let _ = tx.send(response.approved);
    }
    Ok(())
}

/// Developer's "advance" or "stop" decision in step-mode.
#[tauri::command]
fn agent_step_advance(
    state: tauri::State<'_, AppState>,
    response: AgentStepAdvance,
) -> Result<(), String> {
    let tx = state.advances.lock().unwrap().remove(&response.gate_id);
    if let Some(tx) = tx {
        let _ = tx.send(response.proceed);
    }
    Ok(())
}

#[tauri::command]
fn agent_memory_recent(
    limit: Option<u32>,
) -> Result<Vec<kriya::memory::Episode>, String> {
    let path = std::env::temp_dir().join("kriya-memory.db");
    let mem = kriya::memory::AgentMemory::open(&path)?;
    mem.recent(limit.unwrap_or(20))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let policy_path = std::path::PathBuf::from("agent-policy.yaml");
    let app_state = AppState {
        pending: Arc::new(Mutex::new(HashMap::new())),
        approvals: Arc::new(Mutex::new(HashMap::new())),
        advances: Arc::new(Mutex::new(HashMap::new())),
        policy: Arc::new(Policy::load_or_default(&policy_path)),
        signer: Arc::new(Signer::new()),
    };

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            agent_start,
            agent_action_result,
            agent_approval_response,
            agent_step_advance,
            agent_memory_recent
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
