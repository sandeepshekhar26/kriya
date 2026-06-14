//! Tauri backend for the agent-native note app. Hosts the agent loop and exposes two
//! commands to the frontend: `agent_start` (begin a task) and `agent_action_result`
//! (return the outcome of an executed action).

mod agent;
mod audit;
mod budget;
mod memory;
mod permissions;
mod protocol;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::Emitter;

use agent::{ApprovalMap, PendingMap};
use audit::Signer;
use permissions::Policy;
use protocol::{
    AgentActionResult, AgentApprovalResponse, AgentDone, AgentLog, AgentStartRequest, EVENT_DONE,
    EVENT_LOG,
};

pub struct AppState {
    pending: PendingMap,
    approvals: ApprovalMap,
    policy: Arc<Policy>,
    signer: Arc<Signer>,
}

/// Begin an autonomous task. Fire-and-forget: the loop runs on its own thread and
/// reports progress and completion via `agent://*` events.
#[tauri::command]
fn agent_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    req: AgentStartRequest,
) -> Result<(), String> {
    let pending = state.pending.clone();
    let approvals = state.approvals.clone();
    let policy = state.policy.clone();
    let signer = state.signer.clone();

    std::thread::spawn(move || {
        if let Err(err) = agent::run_task(app.clone(), pending, approvals, policy, signer, req) {
            let _ = app.emit(
                EVENT_LOG,
                AgentLog { step_id: None, level: "error".into(), message: err.clone(), detail: None },
            );
            // Always release the frontend's wait, even on failure.
            let _ = app.emit(
                EVENT_DONE,
                AgentDone { summary: format!("Failed: {err}"), steps: 0 },
            );
        }
    });

    Ok(())
}

/// The frontend returns the result of an executed action plus the refreshed app state.
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

/// The frontend returns a human's approve/deny decision for a held action.
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

/// Read recent episodes from durable agent memory (newest first). Opens its own
/// connection so it never contends with the agent thread.
#[tauri::command]
fn agent_memory_recent(limit: Option<u32>) -> Result<Vec<memory::Episode>, String> {
    let path = std::env::temp_dir().join("agent-native-memory.db");
    let mem = memory::AgentMemory::open(&path)?;
    mem.recent(limit.unwrap_or(20))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let policy_path = std::path::PathBuf::from("agent-policy.yaml");
    let app_state = AppState {
        pending: Arc::new(Mutex::new(HashMap::new())),
        approvals: Arc::new(Mutex::new(HashMap::new())),
        policy: Arc::new(Policy::load_or_default(&policy_path)),
        signer: Arc::new(Signer::new()),
    };

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            agent_start,
            agent_action_result,
            agent_approval_response,
            agent_memory_recent
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
