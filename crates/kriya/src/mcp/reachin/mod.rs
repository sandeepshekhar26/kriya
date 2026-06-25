//! Front 2 — the **reach-in adapter** (service-architecture §5). For an app with *no MCP server
//! and no API*, kriya synthesizes a **governed** MCP server from the OS **accessibility tree**:
//! it walks the running app's AX hierarchy, emits one MCP `Tool` per actionable element, and
//! routes every `tools/call` through the *unchanged* [`super::governor::Governor`] (policy →
//! approval → budget → Ed25519-signed audit) before performing the AX action. Same governance as
//! Front 1 (the stdio proxy); only the *last hop* and the *tool-discovery source* differ.
//!
//! Why an accessibility tree and not screenshots: AX gives **typed, named** elements and a small,
//! enumerable set of supported actions (`AXPress`, …) per element — so the synthesized tools carry
//! real semantics (role + title) the agent can reason about, and the executor performs a *named*
//! action, not a blind pixel click. (Honest scope: AX coverage degrades on custom-drawn / Electron
//! / web-embedded UIs and needs a user-granted permission — see §5; this front is coverage-gated.)
//!
//! ## Layering (mirrors `mcp::proxy_*`)
//! - [`AxBackend`] — the seam: snapshot the tree / perform an action. The **key abstraction**: the
//!   synthesis + executor + server logic is unit-testable against a [`FakeBackend`] (test-only),
//!   with **no real AX and no Accessibility permission**. The real macOS FFI backend
//!   ([`macos::MacAxBackend`]) is one impl behind it.
//! - [`synth`] — pure `Vec<AxNode>` → `Vec<Tool>` synthesis (unit-tested).
//! - [`executor::AxExecutor`] — the only new [`super::executor::ActionExecutor`]: maps a cleared
//!   `tools/call` back to a `(node, action)` and performs it (mirrors `McpProxyExecutor`).
//! - [`ReachInServer`] — the serve loop: `initialize` / `tools/list` (policy-filtered) / `tools/call`
//!   (governed), mapping a block to an MCP error *result* exactly like [`super::proxy_server::ProxyServer`].

use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::permissions::{Decision, Policy};

use super::governor::{DispatchOutcome, Governor};
use super::jsonrpc::{
    error_code, CallToolParams, CallToolResult, ListToolsResult, Request, Response, Tool,
};

pub mod executor;
pub mod synth;

#[cfg(target_os = "macos")]
pub mod macos;

/// One element of the target app's accessibility tree, reduced to what tool synthesis and
/// execution need. Deliberately backend-agnostic (no CF/AX types leak here) so the whole
/// subsystem above [`AxBackend`] is platform-independent and unit-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxNode {
    /// Stable, backend-assigned identifier for this element (e.g. a role+title path). It must be
    /// stable across snapshots of an unchanged UI so a synthesized tool name keeps pointing at the
    /// same element. Used verbatim by [`AxBackend::perform`].
    pub id: String,
    /// AX role, e.g. `AXButton`, `AXMenuItem`, `AXTextField`.
    pub role: String,
    /// Human label: the element's title, falling back to its description. Empty when neither is set.
    pub title: String,
    /// Supported AX action names for this element, e.g. `["AXPress"]`. An element with no actions
    /// is not actionable and yields no tool.
    pub actions: Vec<String>,
    /// Whether the element is currently enabled (disabled controls still appear in the tree).
    pub enabled: bool,
    /// Whether this element's **value** is settable (`AXUIElementIsAttributeSettable` over
    /// `kAXValueAttribute`), i.e. text fields, combo boxes, spreadsheet cells. Drives synthesis of a
    /// `set_<role>_<title>` tool (see [`synth`]). A plain button is not value-settable, so it gets a
    /// `press_*` tool but no `set_*` tool. The macOS walk populates this; the [`FakeBackend`] sets it
    /// directly in tests. Defaults to `false` so an unknown/inert element never claims to be settable.
    pub settable: bool,
}

/// The seam between the governed synthesis/serve logic and the OS accessibility API. Two
/// operations: take a snapshot of the actionable tree, and perform a named action on a node.
///
/// `Send + Sync` so an `Arc<dyn AxBackend>` (shared by the server and its [`executor::AxExecutor`])
/// is itself `Send` — the bound the governor's boxed [`super::executor::ActionExecutor`] requires.
/// The real backend talks to a single app and is driven one call at a time, so the methods take
/// `&self`; concurrent access never happens in the MVP serve loop.
pub trait AxBackend: Send + Sync {
    /// Walk the target app's accessibility tree and return its actionable nodes. Errs with a
    /// human-readable reason (permission missing, app not found, AX call failed) rather than
    /// panicking — the caller surfaces it as a startup error or a failed `ActionOutcome`.
    fn snapshot(&self) -> Result<Vec<AxNode>, String>;

    /// Perform `action` (e.g. `"AXPress"`) on the element identified by `node_id`. Errs on an
    /// unknown node, an unsupported action, or an AX failure.
    fn perform(&self, node_id: &str, action: &str) -> Result<(), String>;

    /// Set the **value** of the element identified by `node_id` to `value` — the typed-input
    /// analogue of [`perform`] for a settable element (text field, spreadsheet cell). On macOS this
    /// is `AXUIElementSetAttributeValue(elem, kAXValueAttribute, …)`; the synthesis layer only emits
    /// a `set_*` tool for a node whose [`AxNode::settable`] is true, so a backend may assume the
    /// element accepts a value. Errs on an unknown node or an AX failure.
    fn set_value(&self, node_id: &str, value: &str) -> Result<(), String>;

    /// Type unicode `text` into whatever element currently holds **keyboard focus** — element-free
    /// by design (the agent first focuses a cell via a `press_*`/`set_*` tool, then types). On macOS
    /// this synthesizes CoreGraphics keyboard events carrying the unicode string and posts them to
    /// the focused app. Errs on an OS failure.
    fn type_text(&self, text: &str) -> Result<(), String>;

    /// Press a single **named key or chord** by name (e.g. `"return"`, `"tab"`, `"escape"`, the
    /// arrows) — what lets the agent commit a cell and navigate. The named-key set is small and
    /// closed; an unknown key name is an `Err` (so it surfaces as a failed outcome, never a silent
    /// no-op). On macOS this posts a CoreGraphics key-down + key-up for the mapped virtual keycode.
    fn send_key(&self, key: &str) -> Result<(), String>;
}

/// Owns the governance core, the synthesized tool catalog, and the policy — the Front-2 analogue
/// of [`super::proxy_server::ProxyServer`]. Built once over a snapshot of the target app's tree;
/// the catalog is then served (policy-filtered) and calls are governed against it.
pub struct ReachInServer {
    /// The gateway name reported in `initialize` (so the client sees it is talking to kriya, not
    /// the raw app).
    name: String,
    governor: Governor,
    policy: Arc<Policy>,
    /// Tools synthesized from the AX snapshot at construction time, served policy-filtered.
    tools: Vec<Tool>,
}

impl ReachInServer {
    /// Build a reach-in server over the **macOS** accessibility backend for the app named `app`
    /// (matched by its accessibility/application title; PID also accepted via
    /// [`macos::MacAxBackend::for_pid`]). Snapshots the tree up front so the served catalog is
    /// ready before the first `tools/list` and a permission/lookup failure fails startup loudly.
    ///
    /// `governor` must already be wired with an [`executor::AxExecutor`] over the **same backend
    /// instance** — use [`ReachInServer::with_backend`] to construct both from one `Arc<backend>`.
    #[cfg(target_os = "macos")]
    pub fn new(app: &str, governor: Governor, policy: Arc<Policy>) -> Result<Self, String> {
        // Convenience constructor for the product path: it builds the backend, wires the executor
        // into a governor for the caller, and snapshots — but the lead may prefer the explicit
        // `with_backend` so the governor's gates (signer/approval/actor) are configured outside.
        // Kept minimal here: the caller already passes a governor, so we only need a backend for
        // the snapshot. The governor it passes MUST share this backend (see `with_backend`).
        let backend = Arc::new(macos::MacAxBackend::for_app(app)?);
        Self::with_backend(format!("kriya-gateway ({app})"), backend, governor, policy)
    }

    /// Construct over an explicit, injectable backend — the seam tests use a [`FakeBackend`], and
    /// the macOS path passes an `Arc<MacAxBackend>` shared with the governor's [`executor::AxExecutor`].
    /// Snapshots the tree and synthesizes the tool catalog.
    pub fn with_backend(
        name: impl Into<String>,
        backend: Arc<dyn AxBackend>,
        governor: Governor,
        policy: Arc<Policy>,
    ) -> Result<Self, String> {
        let nodes = backend.snapshot()?;
        let tools = synth::synthesize_tools(&nodes);
        Ok(Self {
            name: name.into(),
            governor,
            policy,
            tools,
        })
    }

    /// Number of tools synthesized from the snapshot — for the startup banner.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Number of tools the agent will actually see after policy filtering — for the banner.
    pub fn visible_tool_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|t| self.policy.check(&t.name) != Decision::Deny)
            .count()
    }

    /// Read newline-delimited client JSON-RPC from `reader`, write responses to `writer`. Blocks
    /// until EOF. One JSON object per line (NDJSON) — identical framing to the proxy + in-process
    /// servers, so the same MCP clients drive it unchanged.
    pub fn serve<R: std::io::BufRead, W: std::io::Write>(
        &mut self,
        reader: R,
        writer: &mut W,
    ) -> std::io::Result<()> {
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => self.handle(req),
                Err(e) => Some(Response::error(
                    Value::Null,
                    error_code::PARSE_ERROR,
                    format!("parse error: {e}"),
                )),
            };
            if let Some(resp) = response {
                writeln!(writer, "{}", resp.to_line())?;
                writer.flush()?;
            }
        }
        Ok(())
    }

    /// Route one parsed client request. Returns `None` for a notification (no reply owed).
    /// Directly unit-testable.
    pub fn handle(&mut self, req: Request) -> Option<Response> {
        // Notifications carry no id and expect no reply. Unlike Front 1 there is no downstream to
        // forward them to (the "downstream" is the app's AX tree, not an MCP server), so we simply
        // accept and drop them — `notifications/initialized` from the client is a no-op for us.
        if req.is_notification() {
            return None;
        }
        let id = req.id.clone().unwrap_or(Value::Null);
        let resp = match req.method.as_str() {
            "initialize" => self.handle_initialize(id),
            "tools/list" => self.handle_list(id),
            "tools/call" => self.handle_call(id, req.params),
            // No app-side server to pass through to, so unmodeled methods are a clean protocol
            // error (vs. Front 1's transparent passthrough to a real downstream).
            other => Response::error(
                id,
                error_code::METHOD_NOT_FOUND,
                format!("reach-in server does not implement '{other}'"),
            ),
        };
        Some(resp)
    }

    /// Synthesize the `initialize` result — we *are* the server (there is no downstream to ask),
    /// so advertise tools under the gateway name.
    fn handle_initialize(&mut self, id: Value) -> Response {
        let result = json!({
            "protocolVersion": super::jsonrpc::PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": self.name, "version": env!("CARGO_PKG_VERSION") },
        });
        Response::success(id, result)
    }

    /// Serve the synthesized tools, **policy-filtered**: a tool the policy denies is hidden so a
    /// denied capability never appears to the agent (defense in depth, matching the proxy's §4 fix).
    fn handle_list(&mut self, id: Value) -> Response {
        let visible: Vec<Tool> = self
            .tools
            .iter()
            .filter(|t| self.policy.check(&t.name) != Decision::Deny)
            .cloned()
            .collect();
        Response::success(id, ok_value(ListToolsResult { tools: visible }))
    }

    /// Route a `tools/call` through every governance gate, then map the outcome onto a
    /// `CallToolResult` — **exactly** like [`super::proxy_server::ProxyServer::handle_call`]: a block
    /// is an MCP error *result* (the call was well-formed but refused), never a JSON-RPC protocol
    /// error, never performed against the app, never signed.
    fn handle_call(&mut self, id: Value, params: Option<Value>) -> Response {
        let Some(params) = params else {
            return Response::error(id, error_code::INVALID_PARAMS, "tools/call requires params");
        };
        let call: CallToolParams = match serde_json::from_value(params) {
            Ok(c) => c,
            Err(e) => {
                return Response::error(
                    id,
                    error_code::INVALID_PARAMS,
                    format!("bad tools/call params: {e}"),
                )
            }
        };

        let outcome = self.governor.dispatch(&call.name, &call.arguments);
        log_outcome(&call.name, &outcome);

        let result = match outcome {
            DispatchOutcome::Denied => CallToolResult::err(format!(
                "blocked by policy: '{}' is denied (deny-by-default)",
                call.name
            )),
            DispatchOutcome::NotApproved => CallToolResult::err(format!(
                "blocked: '{}' requires human approval and it was not granted",
                call.name
            )),
            DispatchOutcome::BudgetExceeded(reason) => {
                CallToolResult::err(format!("blocked: {reason}"))
            }
            DispatchOutcome::Executed { outcome, .. } => {
                if outcome.success {
                    CallToolResult {
                        content: content_from(outcome.data),
                        is_error: false,
                    }
                } else {
                    CallToolResult::err(outcome.error.unwrap_or_else(|| "action failed".into()))
                }
            }
        };
        Response::success(id, ok_value(result))
    }
}

/// Wrap an executor `data` value into MCP content blocks. [`executor::AxExecutor`] returns a single
/// text confirmation, so this is mostly the `other → one text block` path; an array passes through.
fn content_from(data: Value) -> Vec<Value> {
    match data {
        Value::Array(blocks) => blocks,
        Value::Null => vec![],
        other => vec![json!({ "type": "text", "text": other.to_string() })],
    }
}

fn ok_value<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// One stderr line per call so the operator sees governance happen — stdout is the JSON-RPC
/// channel and must never carry this (identical posture to the proxy).
fn log_outcome(action_id: &str, outcome: &DispatchOutcome) {
    let note = match outcome {
        DispatchOutcome::Denied => "DENIED by policy".to_string(),
        DispatchOutcome::NotApproved => "BLOCKED — approval not granted".to_string(),
        DispatchOutcome::BudgetExceeded(r) => format!("BLOCKED — {r}"),
        DispatchOutcome::Executed { outcome, receipt } => format!(
            "performed ({}) · receipt sig={}…",
            if outcome.success { "ok" } else { "failed" },
            &receipt.signature[..receipt.signature.len().min(16)]
        ),
    };
    eprintln!("[kriya-gateway reach-in] tools/call {action_id}: {note}");
}

/// Shared record of the `(node_id, action)` pairs a [`FakeBackend`] was asked to perform — a test
/// keeps a handle to assert exactly what reached (or, when blocked, did not reach) the app.
#[cfg(test)]
pub(crate) type PerformedLog = Arc<Mutex<Vec<(String, String)>>>;

/// One typed-input event a [`FakeBackend`] recorded, so a test can assert the executor routed the
/// right kind/payload to the backend (the value/text/key never actually reaches a real app in a
/// unit test — that is the lead's live check against Numbers).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypedInput {
    /// `set_value(node_id, value)`.
    SetValue { node_id: String, value: String },
    /// `type_text(text)`.
    TypeText { text: String },
    /// `send_key(key)`.
    SendKey { key: String },
}

/// Shared record of the typed-input events a [`FakeBackend`] received — the typed-input analogue of
/// [`PerformedLog`], so a test holds a handle to assert exactly what was set/typed/pressed.
#[cfg(test)]
pub(crate) type TypedLog = Arc<Mutex<Vec<TypedInput>>>;

/// A scriptable in-memory [`AxBackend`] for tests: a fixed node list plus a [`PerformedLog`] and a
/// [`TypedLog`], so a test can assert *exactly* which AX action / typed input a governed call did
/// (or, for a blocked call, did **not**) reach. `perform` and `set_value` err on an unknown node —
/// matching the real backend — so an executor-mapping bug surfaces as a failure.
#[cfg(test)]
pub(crate) struct FakeBackend {
    nodes: Vec<AxNode>,
    /// Records performed actions; shared so the test keeps a handle after the backend is moved into
    /// the executor/governor.
    performed: PerformedLog,
    /// Records typed-input events (set_value / type_text / send_key), shared the same way.
    typed: TypedLog,
}

#[cfg(test)]
impl FakeBackend {
    pub(crate) fn new(nodes: Vec<AxNode>) -> (Self, PerformedLog) {
        let (this, performed, _typed) = Self::new_with_typed(nodes);
        (this, performed)
    }

    /// Like [`new`], but also returns the [`TypedLog`] for tests that exercise the typed-input
    /// methods. Kept separate so the many existing `perform`-only tests stay terse.
    pub(crate) fn new_with_typed(nodes: Vec<AxNode>) -> (Self, PerformedLog, TypedLog) {
        let performed = Arc::new(Mutex::new(Vec::new()));
        let typed = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                nodes,
                performed: performed.clone(),
                typed: typed.clone(),
            },
            performed,
            typed,
        )
    }
}

#[cfg(test)]
impl AxBackend for FakeBackend {
    fn snapshot(&self) -> Result<Vec<AxNode>, String> {
        Ok(self.nodes.clone())
    }

    fn perform(&self, node_id: &str, action: &str) -> Result<(), String> {
        if !self.nodes.iter().any(|n| n.id == node_id) {
            return Err(format!("unknown node '{node_id}'"));
        }
        self.performed
            .lock()
            .unwrap()
            .push((node_id.to_string(), action.to_string()));
        Ok(())
    }

    fn set_value(&self, node_id: &str, value: &str) -> Result<(), String> {
        // Mirror the real backend: an unknown node errs (so an executor-mapping bug surfaces).
        if !self.nodes.iter().any(|n| n.id == node_id) {
            return Err(format!("unknown node '{node_id}'"));
        }
        self.typed.lock().unwrap().push(TypedInput::SetValue {
            node_id: node_id.to_string(),
            value: value.to_string(),
        });
        Ok(())
    }

    fn type_text(&self, text: &str) -> Result<(), String> {
        self.typed.lock().unwrap().push(TypedInput::TypeText {
            text: text.to_string(),
        });
        Ok(())
    }

    fn send_key(&self, key: &str) -> Result<(), String> {
        // The fake validates the named-key set the same way the real backend does, so a test can
        // assert an unknown key is rejected without a real CGEvent.
        if !synth::is_known_key(key) {
            return Err(format!("unknown key '{key}'"));
        }
        self.typed.lock().unwrap().push(TypedInput::SendKey {
            key: key.to_string(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::Signer;
    use crate::mcp::approval::{AutoApprove, DenyApproval};
    use crate::mcp::executor::ActionExecutor;

    /// A small AX snapshot: an enabled "Save" button (allowed read? no — press_* is not read-shaped),
    /// an enabled "Delete" button (destructive → approval), and a disabled button (no tool).
    /// We name them so the synthesized tool names are predictable for the assertions.
    fn sample_nodes() -> Vec<AxNode> {
        vec![
            AxNode {
                id: "AXButton/Save".into(),
                role: "AXButton".into(),
                title: "Save".into(),
                actions: vec!["AXPress".into()],
                enabled: true,
                settable: false,
            },
            AxNode {
                id: "AXButton/Delete".into(),
                role: "AXButton".into(),
                title: "Delete".into(),
                actions: vec!["AXPress".into()],
                enabled: true,
                settable: false,
            },
            AxNode {
                id: "AXButton/Disabled".into(),
                role: "AXButton".into(),
                title: "Disabled".into(),
                actions: vec!["AXPress".into()],
                enabled: false,
                settable: false,
            },
        ]
    }

    /// Build a `ReachInServer` over a fake backend, with a custom policy + approval gate, and return
    /// the server plus the shared `performed` log so a test can assert what reached the app.
    fn server(
        policy: Policy,
        approval: Box<dyn crate::mcp::approval::ApprovalGate>,
    ) -> (ReachInServer, PerformedLog) {
        let (backend, performed) = FakeBackend::new(sample_nodes());
        let backend: Arc<dyn AxBackend> = Arc::new(backend);
        let policy = Arc::new(policy);
        let executor = Box::new(executor::AxExecutor::new(backend.clone(), sample_nodes()));
        let governor = Governor::new(policy.clone(), Arc::new(Signer::new()), approval, executor);
        let srv = ReachInServer::with_backend("kriya-test", backend, governor, policy).unwrap();
        (srv, performed)
    }

    /// A policy tuned for the synthesized `press_*` names: `press_*_save` allowed, `press_*_delete`
    /// gated behind approval, everything else denied. (The real product uses the zero-config
    /// default; this keeps the test independent of that policy's exact prefix list.)
    fn test_policy() -> Policy {
        serde_yaml::from_str(
            r#"
rules:
  - action: "press_button_delete"
    allow: true
    require_approval: true
  - action: "press_button_save"
    allow: true
  - action: "*"
    allow: false
budget:
  max_actions_per_minute: 60
"#,
        )
        .unwrap()
    }

    fn req(method: &str, params: Value) -> Request {
        serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .unwrap()
    }

    #[test]
    fn initialize_advertises_tools_under_the_gateway_name() {
        let (mut s, _) = server(test_policy(), Box::new(DenyApproval));
        let resp = s.handle(req("initialize", json!({}))).unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "kriya-test");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_synthesizes_one_tool_per_enabled_actionable_node_policy_filtered() {
        let (mut s, _) = server(test_policy(), Box::new(DenyApproval));
        let resp = s.handle(req("tools/list", json!({}))).unwrap();
        let tools = resp.result.unwrap()["tools"].clone();
        let names: Vec<String> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        // Save (allowed) + Delete (approval-gated, still visible); the disabled node yields no tool.
        assert!(
            names.contains(&"press_button_save".to_string()),
            "{names:?}"
        );
        assert!(
            names.contains(&"press_button_delete".to_string()),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("disabled")),
            "disabled element must not become a tool: {names:?}"
        );
    }

    #[test]
    fn allowed_call_performs_the_ax_action_and_signs_a_receipt() {
        let (mut s, performed) = server(test_policy(), Box::new(DenyApproval));
        let resp = s
            .handle(req("tools/call", json!({"name":"press_button_save"})))
            .unwrap();
        let result = resp.result.unwrap();
        assert!(result.get("isError").is_none(), "allowed call: {result}");
        // The exact AX action reached the app.
        assert_eq!(
            *performed.lock().unwrap(),
            vec![("AXButton/Save".to_string(), "AXPress".to_string())]
        );
    }

    #[test]
    fn destructive_call_under_deny_is_error_result_and_never_performed() {
        let (mut s, performed) = server(test_policy(), Box::new(DenyApproval));
        let resp = s
            .handle(req("tools/call", json!({"name":"press_button_delete"})))
            .unwrap();
        assert!(
            resp.error.is_none(),
            "a refused call is a result, not a protocol error"
        );
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("approval"));
        // The whole point: a blocked call NEVER touched the app's accessibility tree.
        assert!(
            performed.lock().unwrap().is_empty(),
            "blocked call must not perform any AX action"
        );
    }

    #[test]
    fn destructive_call_performs_when_approved() {
        let (mut s, performed) = server(test_policy(), Box::new(AutoApprove));
        let resp = s
            .handle(req("tools/call", json!({"name":"press_button_delete"})))
            .unwrap();
        assert!(resp.result.unwrap().get("isError").is_none());
        assert_eq!(
            *performed.lock().unwrap(),
            vec![("AXButton/Delete".to_string(), "AXPress".to_string())]
        );
    }

    #[test]
    fn unknown_tool_is_denied_by_default_and_not_performed() {
        let (mut s, performed) = server(test_policy(), Box::new(AutoApprove));
        let resp = s
            .handle(req("tools/call", json!({"name":"press_button_save_typo"})))
            .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true, "unknown name denied: {result}");
        assert!(performed.lock().unwrap().is_empty());
    }

    #[test]
    fn notification_gets_no_response() {
        let (mut s, _) = server(test_policy(), Box::new(DenyApproval));
        let note: Request =
            serde_json::from_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .unwrap();
        assert!(s.handle(note).is_none());
    }

    #[test]
    fn unknown_method_is_a_method_not_found_error() {
        let (mut s, _) = server(test_policy(), Box::new(DenyApproval));
        let resp = s.handle(req("resources/list", json!({}))).unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, error_code::METHOD_NOT_FOUND);
    }

    /// A snapshot with a **settable** cell-like field, so synthesis emits a `set_*` tool and the two
    /// always-on global tools. Used by the typed-input governed-path tests below.
    fn typed_nodes() -> Vec<AxNode> {
        vec![AxNode {
            id: "AXTextField/Cell".into(),
            role: "AXTextField".into(),
            title: "Cell".into(),
            actions: vec!["AXConfirm".into()],
            enabled: true,
            settable: true,
        }]
    }

    /// A permissive policy for the typed-input tools, so the governed path actually executes and we
    /// can assert the backend received the value/text/key. Real product uses the zero-config default.
    fn typed_policy() -> Policy {
        serde_yaml::from_str(
            r#"
rules:
  - action: "*"
    allow: true
budget:
  max_actions_per_minute: 60
"#,
        )
        .unwrap()
    }

    /// Build a server over `typed_nodes` whose executor + test can both see the typed-input log.
    fn typed_server(policy: Policy) -> (ReachInServer, TypedLog) {
        let (backend, _performed, typed) = FakeBackend::new_with_typed(typed_nodes());
        let backend: Arc<dyn AxBackend> = Arc::new(backend);
        let policy = Arc::new(policy);
        let executor = Box::new(executor::AxExecutor::new(backend.clone(), typed_nodes()));
        let governor = Governor::new(
            policy.clone(),
            Arc::new(Signer::new()),
            Box::new(AutoApprove),
            executor,
        );
        let srv = ReachInServer::with_backend("kriya-test", backend, governor, policy).unwrap();
        (srv, typed)
    }

    #[test]
    fn governed_set_value_routes_node_and_value_to_backend() {
        let (mut s, typed) = typed_server(typed_policy());
        let resp = s
            .handle(req(
                "tools/call",
                json!({"name":"set_text_field_cell","arguments":{"value":"42"}}),
            ))
            .unwrap();
        assert!(resp.result.unwrap().get("isError").is_none());
        assert_eq!(
            *typed.lock().unwrap(),
            vec![TypedInput::SetValue {
                node_id: "AXTextField/Cell".into(),
                value: "42".into(),
            }]
        );
    }

    #[test]
    fn governed_type_text_routes_text_to_backend() {
        let (mut s, typed) = typed_server(typed_policy());
        let resp = s
            .handle(req(
                "tools/call",
                json!({"name":"type_text","arguments":{"text":"hello"}}),
            ))
            .unwrap();
        assert!(resp.result.unwrap().get("isError").is_none());
        assert_eq!(
            *typed.lock().unwrap(),
            vec![TypedInput::TypeText {
                text: "hello".into()
            }]
        );
    }

    #[test]
    fn governed_press_key_routes_a_known_key() {
        let (mut s, typed) = typed_server(typed_policy());
        let resp = s
            .handle(req(
                "tools/call",
                json!({"name":"press_key","arguments":{"key":"tab"}}),
            ))
            .unwrap();
        assert!(resp.result.unwrap().get("isError").is_none());
        assert_eq!(
            *typed.lock().unwrap(),
            vec![TypedInput::SendKey { key: "tab".into() }]
        );
    }

    #[test]
    fn governed_press_key_rejects_an_unknown_key_without_touching_app() {
        let (mut s, typed) = typed_server(typed_policy());
        let resp = s
            .handle(req(
                "tools/call",
                json!({"name":"press_key","arguments":{"key":"f13"}}),
            ))
            .unwrap();
        // Well-formed but the key is unknown → the executor surfaces a failed outcome (error result),
        // never a panic, and nothing reached the (fake) app.
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true, "{result}");
        assert!(typed.lock().unwrap().is_empty());
    }

    #[test]
    fn missing_or_nonstring_param_is_a_clean_error_result() {
        let (mut s, typed) = typed_server(typed_policy());
        // set_value with no `value` arg.
        let r1 = s
            .handle(req("tools/call", json!({"name":"set_text_field_cell"})))
            .unwrap();
        assert_eq!(r1.result.unwrap()["isError"], true);
        // type_text with a non-string `text`.
        let r2 = s
            .handle(req(
                "tools/call",
                json!({"name":"type_text","arguments":{"text":7}}),
            ))
            .unwrap();
        assert_eq!(r2.result.unwrap()["isError"], true);
        // Nothing reached the app on either malformed call.
        assert!(typed.lock().unwrap().is_empty());
    }

    /// Sanity that the executor wired into the governor maps a backend error to a failed outcome
    /// (mirrors `McpProxyExecutor`'s "downstream unavailable" path) — drive it directly.
    #[test]
    fn executor_maps_backend_error_to_failed_outcome() {
        let (backend, _performed) = FakeBackend::new(sample_nodes());
        let backend: Arc<dyn AxBackend> = Arc::new(backend);
        // Build an executor whose tool map points a name at a node the backend doesn't know, so
        // `perform` errs — proving the error path becomes a failed `ActionOutcome`, not a panic.
        let mut ex = executor::AxExecutor::new(
            backend,
            vec![AxNode {
                id: "AXButton/Ghost".into(), // not in the backend's node list
                role: "AXButton".into(),
                title: "Ghost".into(),
                actions: vec!["AXPress".into()],
                enabled: true,
                settable: false,
            }],
        );
        let outcome = ex.execute("press_button_ghost", &json!({}));
        assert!(!outcome.success);
        // The backend's "unknown node" error is surfaced (not a panic).
        assert!(outcome
            .error
            .unwrap()
            .contains("accessibility action failed"));
    }
}
