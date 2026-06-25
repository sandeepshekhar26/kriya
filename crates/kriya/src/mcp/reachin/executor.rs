//! [`AxExecutor`] — the only new [`ActionExecutor`] Front 2 needs, the Front-2 analogue of
//! [`crate::mcp::proxy_executor::McpProxyExecutor`]. It is the last hop of a *cleared* `tools/call`:
//! after the [`crate::mcp::governor::Governor`] has run policy → approval → budget, this turns the
//! tool name back into the `(node, action)` it was synthesized from and performs it via the
//! [`AxBackend`]. The governance above it never changes — that is the whole bet.
//!
//! Two kinds of `action` flow through the same route map: a **real AX action** (`AXPress`, …) →
//! [`AxBackend::perform`]; or a **synthetic typed-input marker** (`kriya.set_value` /
//! `kriya.type_text` / `kriya.press_key`, see [`crate::mcp::reachin::synth`]) → the matching
//! typed-input method, after validating its one string param. A typed-input tool is just another
//! cleared action the *same* governor gated — no new governance seam.
//!
//! The tool→routing map is built from the *same* snapshot the served catalog was synthesized from
//! (see [`crate::mcp::reachin::synth::synthesize`]), so a name the agent can call always resolves
//! to exactly one element + action. A name with no mapping (the governor cleared a tool the snapshot
//! no longer covers, e.g. the UI changed) is a failed outcome the agent can read — never a panic.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp::executor::{ActionExecutor, ActionOutcome};

use super::synth::{self, SynthesizedTool, ACTION_PRESS_KEY, ACTION_SET_VALUE, ACTION_TYPE_TEXT};
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
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome {
        // The governor already cleared this name; we only need to route it back to a backend call.
        let Some((node_id, action)) = self.routes.get(action_id) else {
            // Cleared by policy but no longer in the snapshot (UI changed since startup). Surface a
            // readable failure — and the governor still signs a *failure* receipt over it.
            return ActionOutcome::failed(format!(
                "no accessibility element maps to tool '{action_id}' (UI may have changed; re-list tools)"
            ));
        };
        // Clone out of the borrow so we can read `params` and call `&self.backend` freely.
        let (node_id, action) = (node_id.clone(), action.clone());

        // Branch on the synthetic typed-input markers (see `synth`), else perform a real AX action.
        // Each typed-input arm validates its one string param and maps a missing/wrong-typed param to
        // a clean failed outcome — never a panic, so a malformed call is a readable error the agent
        // gets back (and the governor still signs a failure receipt over it).
        match action.as_str() {
            ACTION_SET_VALUE => {
                let value = match string_param(params, "value") {
                    Ok(v) => v,
                    Err(e) => return ActionOutcome::failed(e),
                };
                map_result(
                    self.backend.set_value(&node_id, &value),
                    format!("set value of accessibility element '{node_id}'"),
                )
            }
            ACTION_TYPE_TEXT => {
                let text = match string_param(params, "text") {
                    Ok(v) => v,
                    Err(e) => return ActionOutcome::failed(e),
                };
                map_result(
                    self.backend.type_text(&text),
                    "typed text into the focused element".to_string(),
                )
            }
            ACTION_PRESS_KEY => {
                let key = match string_param(params, "key") {
                    Ok(v) => v,
                    Err(e) => return ActionOutcome::failed(e),
                };
                map_result(self.backend.send_key(&key), format!("pressed key '{key}'"))
            }
            // A real AX action (`AXPress`, …) — the original press path, unchanged.
            _ => map_result(
                self.backend.perform(&node_id, &action),
                format!("performed {action} on accessibility element '{node_id}'"),
            ),
        }
    }
}

/// Read a required string argument from a `tools/call` params object. A missing key or a non-string
/// value is a clean error string (the caller turns it into a failed `ActionOutcome`), never a panic.
fn string_param(params: &Value, key: &str) -> Result<String, String> {
    match params.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(format!(
            "tool argument '{key}' must be a string, got {other}"
        )),
        None => Err(format!("missing required tool argument '{key}'")),
    }
}

/// Map a backend `Result<(), String>` onto an [`ActionOutcome`]: `Ok` becomes a one-line text
/// confirmation; `Err` becomes a readable failed outcome (mirrors `McpProxyExecutor`'s error path —
/// a failed action is something the agent reads, never a panic that kills the session).
fn map_result(res: Result<(), String>, ok_msg: String) -> ActionOutcome {
    match res {
        Ok(()) => ActionOutcome::ok(json!(ok_msg)),
        Err(e) => ActionOutcome::failed(format!("accessibility action failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::reachin::{FakeBackend, TypedInput};

    fn nodes() -> Vec<AxNode> {
        vec![AxNode {
            id: "AXButton/Save".into(),
            role: "AXButton".into(),
            title: "Save".into(),
            actions: vec!["AXPress".into()],
            enabled: true,
            settable: false,
        }]
    }

    /// A settable text-field snapshot, so the executor's route map has a `set_*` tool.
    fn settable_nodes() -> Vec<AxNode> {
        vec![AxNode {
            id: "AXTextField/Cell".into(),
            role: "AXTextField".into(),
            title: "Cell".into(),
            actions: vec![],
            enabled: true,
            settable: true,
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

    #[test]
    fn set_value_routes_node_and_value_to_backend() {
        let (backend, _performed, typed) = FakeBackend::new_with_typed(settable_nodes());
        let mut ex = AxExecutor::new(Arc::new(backend), settable_nodes());
        let outcome = ex.execute("set_text_field_cell", &json!({"value": "42"}));
        assert!(outcome.success, "{outcome:?}");
        assert_eq!(
            *typed.lock().unwrap(),
            vec![TypedInput::SetValue {
                node_id: "AXTextField/Cell".into(),
                value: "42".into(),
            }]
        );
    }

    #[test]
    fn type_text_routes_text_to_backend() {
        let (backend, _performed, typed) = FakeBackend::new_with_typed(settable_nodes());
        let mut ex = AxExecutor::new(Arc::new(backend), settable_nodes());
        let outcome = ex.execute("type_text", &json!({"text": "hello world"}));
        assert!(outcome.success, "{outcome:?}");
        assert_eq!(
            *typed.lock().unwrap(),
            vec![TypedInput::TypeText {
                text: "hello world".into()
            }]
        );
    }

    #[test]
    fn press_key_routes_a_known_key_and_rejects_an_unknown_one() {
        let (backend, _performed, typed) = FakeBackend::new_with_typed(settable_nodes());
        let mut ex = AxExecutor::new(Arc::new(backend), settable_nodes());

        let ok = ex.execute("press_key", &json!({"key": "return"}));
        assert!(ok.success, "{ok:?}");

        // An unknown key is rejected by the backend → failed outcome, and nothing further recorded.
        let bad = ex.execute("press_key", &json!({"key": "f13"}));
        assert!(!bad.success);
        assert!(bad.error.unwrap().contains("unknown key"));

        assert_eq!(
            *typed.lock().unwrap(),
            vec![TypedInput::SendKey {
                key: "return".into()
            }]
        );
    }

    #[test]
    fn missing_string_param_is_a_clean_failed_outcome() {
        let (backend, _performed, typed) = FakeBackend::new_with_typed(settable_nodes());
        let mut ex = AxExecutor::new(Arc::new(backend), settable_nodes());

        // set_value with no `value`.
        let o1 = ex.execute("set_text_field_cell", &json!({}));
        assert!(!o1.success);
        assert!(o1.error.unwrap().contains("missing required tool argument"));

        // type_text with a non-string `text`.
        let o2 = ex.execute("type_text", &json!({"text": 7}));
        assert!(!o2.success);
        assert!(o2.error.unwrap().contains("must be a string"));

        // Neither malformed call reached the backend.
        assert!(typed.lock().unwrap().is_empty());
    }
}
