//! Front 3 — the computer-use fallback (service-architecture §6). **Deferred / design-partner-gated.**
//!
//! For apps that Front 1 (the MCP proxy) and Front 2 (the accessibility reach-in) cannot reach,
//! the last resort is driving the UI as pixels: screenshot → locate → click/type. That heavy
//! CV/vision pipeline deliberately lives *outside* the governed core — this executor is the thin,
//! honest seam that lets the **unchanged** [`Governor`](super::governor::Governor) drive such a
//! front the moment one exists, with the same policy → approval → budget → signed-audit gates as
//! every other front.
//!
//! Concretely, a cleared `tools/call` is delegated to an external **computer-use driver** command
//! over the exact one-line JSON contract [`ProcessExecutor`](super::executor::ProcessExecutor)
//! already speaks (`{"action","params"}` in, `{"success","data","error"}` out) — so the driver can
//! be any program (an OS-automation script, a vision agent) and can be swapped without touching
//! governance. With no driver configured, every call returns a clear, non-panicking failure that
//! points the operator at the better fronts.

use serde_json::Value;

use super::executor::{ActionExecutor, ActionOutcome, ProcessExecutor};

/// Governed computer-use fallback. Holds an optional external driver; without one it is inert
/// (every call fails cleanly) so wiring it up is always safe.
pub struct ComputerUseExecutor {
    /// `None` until an operator supplies a driver command. Reuses `ProcessExecutor` so the wire
    /// contract and process handling match the rest of the codebase exactly.
    driver: Option<ProcessExecutor>,
}

impl ComputerUseExecutor {
    /// An inert fallback — no driver. Every cleared call returns a readable failure rather than
    /// silently doing nothing or panicking. Safe to wire into a `Governor` as a placeholder.
    pub fn unconfigured() -> Self {
        Self { driver: None }
    }

    /// Delegate cleared calls to an external computer-use driver command (e.g. an OS-automation
    /// script or a vision agent) over the `ProcessExecutor` line contract.
    pub fn with_driver(command: &str) -> Self {
        Self {
            driver: Some(ProcessExecutor::new(command)),
        }
    }

    /// Whether a driver is wired (for a startup banner).
    pub fn is_configured(&self) -> bool {
        self.driver.is_some()
    }
}

impl ActionExecutor for ComputerUseExecutor {
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome {
        match self.driver.as_mut() {
            // Governance already cleared the call; the driver just performs it as input.
            Some(driver) => driver.execute(action_id, params),
            // Deferred-by-default: fail readably and steer the operator to the governed fronts that
            // need no pixel-driving. The agent receives this as a normal failed result.
            None => ActionOutcome::failed(format!(
                "computer-use fallback not configured for '{action_id}': supply an external driver, \
                 or prefer Front 1 (MCP proxy) / Front 2 (accessibility reach-in). Front 3 is deferred."
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unconfigured_fails_cleanly_with_a_steering_message() {
        let mut exec = ComputerUseExecutor::unconfigured();
        assert!(!exec.is_configured());
        let outcome = exec.execute("click_button", &json!({ "x": 10, "y": 20 }));
        assert!(!outcome.success);
        let err = outcome.error.unwrap();
        // Names the action and points at the better fronts — never panics.
        assert!(err.contains("click_button"));
        assert!(err.contains("reach-in") || err.contains("proxy"));
    }

    #[test]
    fn with_driver_reports_configured() {
        let exec = ComputerUseExecutor::with_driver("true");
        assert!(exec.is_configured());
    }
}
