//! [`AxExecutor`] — the only new [`ActionExecutor`] Front 2 needs, the Front-2 analogue of
//! [`crate::mcp::proxy_executor::McpProxyExecutor`]. It is the last hop of a *cleared* `tools/call`:
//! after the [`crate::mcp::governor::Governor`] has run policy → approval → budget, this turns the
//! tool name back into the `(node, action)` it was synthesized from and performs that AX action via
//! the [`AxBackend`]. The governance above it never changes — that is the whole bet.
//!
//! The tool→routing map is built from the *same* snapshot the served catalog was synthesized from
//! (see [`crate::mcp::reachin::synth::synthesize`]), so a name the agent can call always resolves
//! to exactly one element + action. A name with no mapping (the governor cleared a tool the snapshot
//! no longer covers, e.g. the UI changed) is a failed outcome the agent can read — never a panic.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp::executor::{ActionExecutor, ActionOutcome};

use super::synth::{self, SynthesizedTool};
use super::{AxBackend, AxNode};

/// Performs a cleared `tools/call` against the app's accessibility tree.
pub struct AxExecutor {
    backend: Arc<dyn AxBackend>,
    /// tool name → (node_id, action). Built once from the snapshot so execution is a lookup + one
    /// AX call, with no re-synthesis per call.
    routes: HashMap<String, (String, String)>,
}

impl AxExecutor {
    /// Build over the backend and the **same node snapshot** the server synthesized its catalog
    /// from, so the executor's name→route map matches the served tool names exactly.
    pub fn new(backend: Arc<dyn AxBackend>, nodes: Vec<AxNode>) -> Self {
        let routes = synth::synthesize(&nodes)
            .into_iter()
            .map(
                |SynthesizedTool {
                     tool,
                     node_id,
                     action,
                     ..
                 }| (tool.name, (node_id, action)),
            )
            .collect();
        Self { backend, routes }
    }
}

impl ActionExecutor for AxExecutor {
    fn execute(&mut self, action_id: &str, _params: &Value) -> ActionOutcome {
        // The governor already cleared this name; we only need to route it back to an AX call.
        let Some((node_id, action)) = self.routes.get(action_id) else {
            // Cleared by policy but no longer in the snapshot (UI changed since startup). Surface a
            // readable failure — and the governor still signs a *failure* receipt over it.
            return ActionOutcome::failed(format!(
                "no accessibility element maps to tool '{action_id}' (UI may have changed; re-list tools)"
            ));
        };
        match self.backend.perform(node_id, action) {
            Ok(()) => ActionOutcome::ok(json!(format!(
                "performed {action} on accessibility element '{node_id}'"
            ))),
            // A failed AX action (element gone, action refused) is a failed outcome the agent can
            // read — never a panic that kills the session. Mirrors McpProxyExecutor's error path.
            Err(e) => ActionOutcome::failed(format!("accessibility action failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::reachin::FakeBackend;

    fn nodes() -> Vec<AxNode> {
        vec![AxNode {
            id: "AXButton/Save".into(),
            role: "AXButton".into(),
            title: "Save".into(),
            actions: vec!["AXPress".into()],
            enabled: true,
        }]
    }

    #[test]
    fn cleared_call_performs_the_mapped_ax_action() {
        let (backend, performed) = FakeBackend::new(nodes());
        let mut ex = AxExecutor::new(Arc::new(backend), nodes());
        let outcome = ex.execute("press_button_save", &json!({}));
        assert!(outcome.success, "{outcome:?}");
        assert_eq!(
            *performed.lock().unwrap(),
            vec![("AXButton/Save".to_string(), "AXPress".to_string())]
        );
    }

    #[test]
    fn unmapped_name_is_a_failed_outcome_not_a_panic() {
        let (backend, performed) = FakeBackend::new(nodes());
        let mut ex = AxExecutor::new(Arc::new(backend), nodes());
        let outcome = ex.execute("press_button_nonexistent", &json!({}));
        assert!(!outcome.success);
        assert!(outcome.error.unwrap().contains("no accessibility element"));
        assert!(
            performed.lock().unwrap().is_empty(),
            "an unmapped name must not reach the backend"
        );
    }

    #[test]
    fn backend_perform_error_becomes_failed_outcome() {
        // Backend knows no nodes, but the executor's route map points at one → perform errs.
        let (backend, _performed) = FakeBackend::new(vec![]);
        let mut ex = AxExecutor::new(Arc::new(backend), nodes());
        let outcome = ex.execute("press_button_save", &json!({}));
        assert!(!outcome.success);
        assert!(outcome
            .error
            .unwrap()
            .contains("accessibility action failed"));
    }
}
