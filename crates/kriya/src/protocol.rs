//! Rust mirrors of the `kriya-core` agent-loop protocol. Field names that cross
//! the IPC boundary use camelCase to match the TypeScript side.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP-compatible tool schema, as emitted by the SDK's `getToolSchemas()` (TS) or
/// [`crate::Registry::tool_schemas`] (Rust). `Serialize` so the Rust authoring SDK can emit
/// `tools.json`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSchema {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: u32,
    pub description: String,
    // Part of the wire contract (the host gates on the policy, not these). Kept so the
    // schema round-trips faithfully and future policies can consult declared scopes.
    #[serde(default)]
    #[allow(dead_code)]
    pub permissions: Vec<String>,
    pub input_schema: Value,
}

fn default_version() -> u32 {
    1
}

/// app -> host: begin an autonomous task.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentStartRequest {
    pub goal: String,
    pub state: Value,
    pub tools: Vec<ToolSchema>,
    /// If true and durable memory has a prior run with the same `goal`, the host
    /// reseeds its in-memory history with the completed actions from that run and
    /// continues from there. Defaults to false — set it explicitly to opt in.
    #[serde(default)]
    pub resume: bool,
    /// If true, the host pauses *before each decision* and waits for the frontend
    /// to send `agent_step_advance`. Lets a developer single-step the agent the
    /// same way you'd single-step a debugger. Defaults to false (run to completion).
    #[serde(default)]
    pub step_mode: bool,
    /// Optional identity of the agent driving this run (R8). When absent the host
    /// falls back to the backend name. Stamped into every signed receipt's `actor`.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Optional identity of the human/operator the run acts for (R8). When absent the
    /// host falls back to the OS user. Stamped into every signed receipt's `actor`.
    #[serde(default)]
    pub user_id: Option<String>,
}

/// host -> app: execute this action on the agent's behalf.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActionRequest {
    pub step_id: String,
    pub action_id: String,
    pub params: Value,
    pub reasoning: String,
}

/// app -> host: result of executing an action, plus the refreshed app state.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentActionResult {
    pub step_id: String,
    pub success: bool,
    // Returned by the handler; surfaced to the inspector frontend-side. The host signs
    // params + outcome, not the payload, so it doesn't read this directly.
    #[serde(default)]
    #[allow(dead_code)]
    pub data: Value,
    #[serde(default)]
    pub error: Option<String>,
    pub state: Value,
}

/// host -> app: this action needs human approval before it runs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentApprovalRequest {
    pub step_id: String,
    pub action_id: String,
    pub params: Value,
    pub reasoning: String,
}

/// app -> host: a human's decision on a pending approval.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentApprovalResponse {
    pub step_id: String,
    pub approved: bool,
}

/// host -> app: paused waiting for the developer to step. Sent only in step_mode.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAwaitStep {
    /// Correlation id for the pending advance. Matches the response.
    pub gate_id: String,
    /// 1-indexed counter of which step the host is about to take.
    pub step_number: u32,
    /// Action id of the previous step (None on the first pause).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_action_id: Option<String>,
    /// Whether the previous action succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success: Option<bool>,
}

/// app -> host: developer's decision when paused — proceed or stop the run.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStepAdvance {
    pub gate_id: String,
    /// `true` proceeds to the next step; `false` ends the run gracefully.
    pub proceed: bool,
}

/// host -> app: the task is complete.
#[derive(Debug, Clone, Serialize)]
pub struct AgentDone {
    pub summary: String,
    pub steps: u32,
}

/// host -> app: inspector telemetry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLog {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

impl AgentLog {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            step_id: None,
            level: "info".into(),
            message: message.into(),
            detail: None,
        }
    }
    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            step_id: None,
            level: "warn".into(),
            message: message.into(),
            detail: None,
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            step_id: None,
            level: "error".into(),
            message: message.into(),
            detail: None,
        }
    }
}

/// Tauri channel/command names (kept in sync with the SDK's `AgentEvents`/`AgentCommands`).
pub const EVENT_ACTION: &str = "agent://action";
pub const EVENT_APPROVAL: &str = "agent://approval";
pub const EVENT_AWAIT_STEP: &str = "agent://await_step";
pub const EVENT_DONE: &str = "agent://done";
pub const EVENT_LOG: &str = "agent://log";
