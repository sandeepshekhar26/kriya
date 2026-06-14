//! Tauri backend for the agent-native note app. Hosts the agent loop (delegated to the
//! `agent_native_host` crate) and exposes the IPC commands the frontend calls.

mod deterministic;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::Emitter;

use agent_native_host::{
    audit::Signer,
    permissions::Policy,
    protocol::{
        AgentActionResult, AgentApprovalResponse, AgentDone, AgentLog, AgentStartRequest,
        AgentStepAdvance, EVENT_DONE, EVENT_LOG,
    },
    run_task, select_backend_with_default, ApprovalMap, PendingMap, StepAdvanceMap,
};

use deterministic::DeterministicOrganizer;

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

    let backend = select_backend_with_default(Box::new(DeterministicOrganizer::new()));

    std::thread::spawn(move || {
        if let Err(err) = run_task(
            app.clone(),
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
) -> Result<Vec<agent_native_host::memory::Episode>, String> {
    let path = std::env::temp_dir().join("agent-native-memory.db");
    let mem = agent_native_host::memory::AgentMemory::open(&path)?;
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
