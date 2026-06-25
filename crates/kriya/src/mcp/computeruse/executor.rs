//! [`ComputerUseExecutor`] — the only new [`ActionExecutor`] Front 3 needs, the Front-3 analogue of
//! [`crate::mcp::reachin::executor::AxExecutor`]. It is the last hop of a *cleared* `tools/call`:
//! after the [`crate::mcp::governor::Governor`] has run policy → approval → budget, this turns the
//! tool *name* into the synthetic `computer.*` action it was built from, validates the params, and
//! performs it via the [`DesktopBackend`]. The governance above it never changes — that is the bet.
//!
//! Every param-validation failure (missing key, wrong type) maps to a clean failed
//! [`ActionOutcome`], never a panic — so a malformed call is a readable error the agent gets back,
//! and the governor still signs a *failure* receipt over it (matching the in-process host's audit).
//!
//! The screenshot is special: the backend returns raw PNG bytes, but [`ActionOutcome::data`] is
//! `serde_json::Value`, so the executor **base64-encodes** the bytes into a pre-built MCP **image**
//! content block. The server emits that block verbatim — it never sees the raw bytes.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};

use crate::mcp::executor::{ActionExecutor, ActionOutcome};

use super::{
    tool_catalog_routes, DesktopBackend, ACTION_CLICK, ACTION_KEY, ACTION_LIST_APPS, ACTION_MOVE,
    ACTION_SCREENSHOT, ACTION_SCROLL, ACTION_TYPE,
};

/// Performs a cleared `tools/call` as pixel/keyboard input against the screen via a [`DesktopBackend`].
pub struct ComputerUseExecutor {
    backend: Arc<dyn DesktopBackend>,
    /// tool name → synthetic `computer.*` action marker. Built once from the fixed catalog so
    /// execution is a lookup + one backend call, with no per-call re-derivation.
    routes: HashMap<String, &'static str>,
}

impl ComputerUseExecutor {
    /// Build over the desktop backend. The route map is the fixed Front-3 catalog — the *same* set
    /// the [`super::ComputerUseServer`] serves, so a name the agent can call always resolves.
    pub fn new(backend: Arc<dyn DesktopBackend>) -> Self {
        Self {
            backend,
            routes: tool_catalog_routes(),
        }
    }
}

impl ActionExecutor for ComputerUseExecutor {
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome {
        // The governor already cleared this name; route it back to the synthetic action marker.
        let Some(&action) = self.routes.get(action_id) else {
            // Cleared by policy but not a Front-3 tool name — surface a readable failure (the
            // governor still signs a failure receipt over it).
            return ActionOutcome::failed(format!(
                "'{action_id}' is not a computer-use tool (expected one of the fixed computer_* tools)"
            ));
        };

        match action {
            ACTION_SCREENSHOT => match self.backend.screenshot() {
                Ok(png) => {
                    // PNG bytes don't fit in JSON — base64-encode into a ready-to-emit image block.
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
                    ActionOutcome::ok(json!({
                        "type": "image",
                        "data": b64,
                        "mimeType": "image/png",
                    }))
                }
                Err(e) => ActionOutcome::failed(format!("screenshot failed: {e}")),
            },
            ACTION_CLICK => {
                let (x, y) = match xy(params) {
                    Ok(v) => v,
                    Err(e) => return ActionOutcome::failed(e),
                };
                // 'button' is optional; anything but "right" is treated as a left click by the backend.
                let button = match params.get("button") {
                    None | Some(Value::Null) => "left".to_string(),
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => {
                        return ActionOutcome::failed(format!(
                            "tool argument 'button' must be a string, got {other}"
                        ))
                    }
                };
                map_result(
                    self.backend.click(x, y, &button),
                    format!("clicked {button} at ({x}, {y})"),
                )
            }
            ACTION_MOVE => {
                let (x, y) = match xy(params) {
                    Ok(v) => v,
                    Err(e) => return ActionOutcome::failed(e),
                };
                map_result(
                    self.backend.move_to(x, y),
                    format!("moved cursor to ({x}, {y})"),
                )
            }
            ACTION_SCROLL => {
                // Both deltas are optional integers, defaulting to 0; a present-but-non-integer is an
                // error rather than a silent default (so a typo'd param is visible to the agent).
                let dx = match int_param_opt(params, "dx") {
                    Ok(v) => v,
                    Err(e) => return ActionOutcome::failed(e),
                };
                let dy = match int_param_opt(params, "dy") {
                    Ok(v) => v,
                    Err(e) => return ActionOutcome::failed(e),
                };
                map_result(
                    self.backend.scroll(dx, dy),
                    format!("scrolled (dx={dx}, dy={dy})"),
                )
            }
            ACTION_TYPE => {
                let text = match string_param(params, "text") {
                    Ok(v) => v,
                    Err(e) => return ActionOutcome::failed(e),
                };
                map_result(
                    self.backend.type_text(&text),
                    "typed text into the focused element".to_string(),
                )
            }
            ACTION_KEY => {
                let key = match string_param(params, "key") {
                    Ok(v) => v,
                    Err(e) => return ActionOutcome::failed(e),
                };
                map_result(self.backend.key(&key), format!("pressed key '{key}'"))
            }
            ACTION_LIST_APPS => match self.backend.list_apps() {
                Ok(apps) => ActionOutcome::ok(json!(format!(
                    "running foreground apps: {}",
                    apps.join(", ")
                ))),
                Err(e) => ActionOutcome::failed(format!("list_apps failed: {e}")),
            },
            // Unreachable: the route map only ever yields a known marker. A defensive failure keeps
            // this total without a panic.
            other => ActionOutcome::failed(format!("unhandled computer-use action '{other}'")),
        }
    }
}

/// Read the required numeric `x` and `y` screen coordinates. Accepts any JSON number (int or float).
/// A missing or non-numeric coordinate is a clean error (never a panic).
fn xy(params: &Value) -> Result<(f64, f64), String> {
    Ok((number_param(params, "x")?, number_param(params, "y")?))
}

/// Read a required numeric argument as `f64` (a JSON int or float). Missing/non-number → error.
fn number_param(params: &Value, key: &str) -> Result<f64, String> {
    match params.get(key) {
        Some(Value::Number(n)) => n
            .as_f64()
            .ok_or_else(|| format!("tool argument '{key}' is not a finite number")),
        Some(other) => Err(format!(
            "tool argument '{key}' must be a number, got {other}"
        )),
        None => Err(format!("missing required tool argument '{key}'")),
    }
}

/// Read an *optional* integer argument, defaulting to 0 when absent. A present-but-non-integer (or a
/// float / out-of-range) value is an error rather than a silent default.
fn int_param_opt(params: &Value, key: &str) -> Result<i32, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(0),
        Some(Value::Number(n)) => {
            let i = n
                .as_i64()
                .ok_or_else(|| format!("tool argument '{key}' must be an integer"))?;
            i32::try_from(i).map_err(|_| format!("tool argument '{key}' is out of range"))
        }
        Some(other) => Err(format!(
            "tool argument '{key}' must be an integer, got {other}"
        )),
    }
}

/// Read a required string argument. Missing/non-string → clean error (mirrors reach-in's helper).
fn string_param(params: &Value, key: &str) -> Result<String, String> {
    match params.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(format!(
            "tool argument '{key}' must be a string, got {other}"
        )),
        None => Err(format!("missing required tool argument '{key}'")),
    }
}

/// Map a backend `Result<(), String>` onto an [`ActionOutcome`]: `Ok` → a one-line text
/// confirmation; `Err` → a readable failed outcome (mirrors reach-in's `map_result` — a failed
/// action is something the agent reads, never a panic that kills the session).
fn map_result(res: Result<(), String>, ok_msg: String) -> ActionOutcome {
    match res {
        Ok(()) => ActionOutcome::ok(json!(ok_msg)),
        Err(e) => ActionOutcome::failed(format!("computer-use action failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::computeruse::{DesktopCall, FakeBackend, TOOL_CLICK, TOOL_SCREENSHOT};

    #[test]
    fn screenshot_base64_encodes_png_into_an_image_block() {
        let (backend, calls) = FakeBackend::new();
        let mut ex = ComputerUseExecutor::new(Arc::new(backend));
        let outcome = ex.execute(TOOL_SCREENSHOT, &json!({}));
        assert!(outcome.success, "{outcome:?}");
        assert_eq!(outcome.data["type"], "image");
        assert_eq!(outcome.data["mimeType"], "image/png");
        // The fake backend's PNG magic (\x89PNG) base64-encodes to "iVBORw==".
        assert_eq!(outcome.data["data"], "iVBORw==");
        assert_eq!(*calls.lock().unwrap(), vec![DesktopCall::Screenshot]);
    }

    #[test]
    fn click_routes_by_tool_name_and_defaults_button_left() {
        let (backend, calls) = FakeBackend::new();
        let mut ex = ComputerUseExecutor::new(Arc::new(backend));
        let outcome = ex.execute(TOOL_CLICK, &json!({"x": 3, "y": 4.5}));
        assert!(outcome.success, "{outcome:?}");
        assert_eq!(
            *calls.lock().unwrap(),
            vec![DesktopCall::Click {
                x: 3.0,
                y: 4.5,
                button: "left".into()
            }]
        );
    }

    #[test]
    fn unknown_tool_name_is_a_failed_outcome_not_a_panic() {
        let (backend, calls) = FakeBackend::new();
        let mut ex = ComputerUseExecutor::new(Arc::new(backend));
        let outcome = ex.execute("not_a_computer_tool", &json!({}));
        assert!(!outcome.success);
        assert!(outcome.error.unwrap().contains("not a computer-use tool"));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn missing_coord_is_a_clean_failed_outcome() {
        let (backend, calls) = FakeBackend::new();
        let mut ex = ComputerUseExecutor::new(Arc::new(backend));
        let outcome = ex.execute(TOOL_CLICK, &json!({"x": 1.0}));
        assert!(!outcome.success);
        assert!(outcome.error.unwrap().contains("missing required"));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn scroll_defaults_and_rejects_non_integer() {
        let (backend, calls) = FakeBackend::new();
        let mut ex = ComputerUseExecutor::new(Arc::new(backend));
        // Missing both → (0,0).
        assert!(
            ex.execute(crate::mcp::computeruse::TOOL_SCROLL, &json!({}))
                .success
        );
        // Float dy → error.
        let bad = ex.execute(crate::mcp::computeruse::TOOL_SCROLL, &json!({"dy": 1.5}));
        assert!(!bad.success);
        assert!(bad.error.unwrap().contains("integer"));
        // Only the first (defaulted) scroll reached the backend.
        assert_eq!(
            *calls.lock().unwrap(),
            vec![DesktopCall::Scroll { dx: 0, dy: 0 }]
        );
    }

    #[test]
    fn unknown_key_becomes_failed_outcome_via_backend() {
        let (backend, calls) = FakeBackend::new();
        let mut ex = ComputerUseExecutor::new(Arc::new(backend));
        let bad = ex.execute(crate::mcp::computeruse::TOOL_KEY, &json!({"key": "f13"}));
        assert!(!bad.success);
        assert!(bad.error.unwrap().contains("computer-use action failed"));
        assert!(calls.lock().unwrap().is_empty());
    }
}
