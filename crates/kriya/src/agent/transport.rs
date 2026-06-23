//! The seam that decouples the agent loop from any particular shell.
//!
//! The host loop in [`super::host`] only ever needs to do two things with the outside world:
//! push events *out* to the app (an action to run, an approval request, a log line, the
//! final summary) and receive results *back* (via the shared channel maps). The "out" half
//! used to be hard-wired to Tauri's `AppHandle::emit`. [`HostSink`] abstracts it, so the same
//! loop drives a Tauri webview, a stdio sidecar (Electron/Node — roadmap R3), or a test
//! recorder, with no change to the loop itself.

use crate::protocol::{AgentActionRequest, AgentApprovalRequest, AgentAwaitStep, AgentDone, AgentLog};

/// Where the host loop sends events bound for the app. Implementations must be cheap to call
/// and non-blocking — the loop calls them inline while holding the step open. `Send + Sync`
/// because the loop runs on its own thread and the sink is shared via `Arc`.
pub trait HostSink: Send + Sync {
    /// Ask the app to execute an action (it will reply on the pending-results channel).
    fn emit_action(&self, req: &AgentActionRequest);
    /// Ask a human to approve a guarded action (reply arrives on the approvals channel).
    fn emit_approval(&self, req: &AgentApprovalRequest);
    /// Step-mode pause: wait for the developer to advance (reply on the advances channel).
    fn emit_await_step(&self, ev: &AgentAwaitStep);
    /// The run finished.
    fn emit_done(&self, done: &AgentDone);
    /// Inspector/telemetry log line.
    fn emit_log(&self, entry: &AgentLog);
}

/// A [`HostSink`] backed by a Tauri `AppHandle`. Emits each event on the same channel names
/// the `kriya-core` SDK already listens for, so existing Tauri apps are unchanged
/// apart from wrapping their handle in this.
///
/// Requires the `tauri-host` feature (on by default). A non-Tauri embedder builds with
/// `default-features = false` and supplies its own [`HostSink`] (or just uses the MCP server).
#[cfg(feature = "tauri-host")]
pub struct TauriSink {
    app: tauri::AppHandle,
}

#[cfg(feature = "tauri-host")]
impl TauriSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

#[cfg(feature = "tauri-host")]
impl HostSink for TauriSink {
    fn emit_action(&self, req: &AgentActionRequest) {
        let _ = tauri::Emitter::emit(&self.app, crate::protocol::EVENT_ACTION, req);
    }
    fn emit_approval(&self, req: &AgentApprovalRequest) {
        let _ = tauri::Emitter::emit(&self.app, crate::protocol::EVENT_APPROVAL, req);
    }
    fn emit_await_step(&self, ev: &AgentAwaitStep) {
        let _ = tauri::Emitter::emit(&self.app, crate::protocol::EVENT_AWAIT_STEP, ev);
    }
    fn emit_done(&self, done: &AgentDone) {
        let _ = tauri::Emitter::emit(&self.app, crate::protocol::EVENT_DONE, done);
    }
    fn emit_log(&self, entry: &AgentLog) {
        let _ = tauri::Emitter::emit(&self.app, crate::protocol::EVENT_LOG, entry);
    }
}
