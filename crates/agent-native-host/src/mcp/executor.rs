//! How a governed `tools/call` actually runs the action, once the gates have cleared.
//!
//! The MCP server doesn't know *how* an app executes an action — a Tauri app dispatches it
//! in the webview, a sidecar (roadmap R3) forwards it to the app's main process, a CLI app
//! shells out. So execution is a trait. The governor owns the policy/approval/budget/audit
//! decision; the executor owns only the mechanics of running the cleared action and
//! reporting back what happened.

use serde_json::Value;

/// What running an action produced. `data` is whatever the handler returned (typically the
/// refreshed app state) and is surfaced to the calling agent; `error` is set on failure.
#[derive(Debug, Clone)]
pub struct ActionOutcome {
    pub success: bool,
    pub data: Value,
    pub error: Option<String>,
}

impl ActionOutcome {
    pub fn ok(data: Value) -> Self {
        Self { success: true, data, error: None }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self { success: false, data: Value::Null, error: Some(error.into()) }
    }
}

/// Runs an action that has already passed every governance gate. Implementations must not
/// re-check policy — that decision was already made and audited by the [`super::governor`].
pub trait ActionExecutor: Send {
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome;
}

/// Adapts a closure into an [`ActionExecutor`]. Handy for tests and for embedders that
/// already hold a dispatch function.
pub struct FnExecutor<F>(pub F);

impl<F> ActionExecutor for FnExecutor<F>
where
    F: FnMut(&str, &Value) -> ActionOutcome + Send,
{
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome {
        (self.0)(action_id, params)
    }
}
