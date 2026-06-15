//! Rust mirrors of the `kriya-core` agent-loop protocol. Field names that cross
//! the IPC boundary use camelCase to match the TypeScript side.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP-compatible tool schema, as emitted by the SDK's `getToolSchemas()`.
#[derive(Debug, Clone, Deserialize)]
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
        Self { step_id: None, level: "info".into(), message: message.into(), detail: None }
    }
    pub fn warn(message: impl Into<String>) -> Self {
        Self { step_id: None, level: "warn".into(), message: message.into(), detail: None }
    }
}

/// Tauri channel/command names (kept in sync with the SDK's `AgentEvents`/`AgentCommands`).
pub const EVENT_ACTION: &str = "agent://action";
pub const EVENT_APPROVAL: &str = "agent://approval";
pub const EVENT_DONE: &str = "agent://done";
pub const EVENT_LOG: &str = "agent://log";
