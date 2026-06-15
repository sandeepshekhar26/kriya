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

/// Runs each cleared action by shelling out to an external command — the simplest way to
/// bolt governed MCP onto an app that isn't Tauri. For every call the executor spawns the
/// command, writes one line of `{"action","params"}` JSON to its stdin, and reads one line
/// of `{"success","data","error"}` JSON back from its stdout. The app supplies that handler;
/// the persistent-process version is the R3 sidecar.
///
/// Per-call spawn keeps this dependency-free and stateless. It is deliberately simple, not
/// fast — R3 replaces it with a long-lived sidecar for latency-sensitive use.
pub struct ProcessExecutor {
    program: String,
    args: Vec<String>,
}

impl ProcessExecutor {
    /// Build from a shell-style command string, e.g. `"node handle-action.js"`. Split on
    /// whitespace — the first token is the program, the rest are fixed leading arguments.
    pub fn new(command: &str) -> Self {
        let mut parts = command.split_whitespace().map(String::from);
        let program = parts.next().unwrap_or_default();
        let args = parts.collect();
        Self { program, args }
    }

    fn run(&self, action_id: &str, params: &Value) -> Result<ActionOutcome, String> {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};

        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("failed to spawn '{}': {e}", self.program))?;

        let request = serde_json::json!({ "action": action_id, "params": params });
        {
            let stdin = child.stdin.as_mut().ok_or("child stdin unavailable")?;
            writeln!(stdin, "{request}").map_err(|e| format!("write to child failed: {e}"))?;
        } // drop stdin → EOF, so handlers that read-to-end terminate.

        let stdout = child.stdout.take().ok_or("child stdout unavailable")?;
        let mut last_line = String::new();
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|e| format!("read from child failed: {e}"))?;
            if !line.trim().is_empty() {
                last_line = line;
            }
        }
        let _ = child.wait();

        if last_line.trim().is_empty() {
            return Err("handler produced no JSON response line".into());
        }

        // Handler reply: { success: bool, data?: any, error?: string }.
        let reply: Value = serde_json::from_str(&last_line)
            .map_err(|e| format!("handler response was not JSON: {e}"))?;
        let success = reply.get("success").and_then(Value::as_bool).unwrap_or(false);
        if success {
            Ok(ActionOutcome::ok(reply.get("data").cloned().unwrap_or(Value::Null)))
        } else {
            let err = reply
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("handler reported failure")
                .to_string();
            Ok(ActionOutcome::failed(err))
        }
    }
}

impl ActionExecutor for ProcessExecutor {
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome {
        // Any plumbing failure becomes a failed outcome the agent can read — never a panic
        // that would take down the whole MCP session.
        self.run(action_id, params).unwrap_or_else(ActionOutcome::failed)
    }
}
