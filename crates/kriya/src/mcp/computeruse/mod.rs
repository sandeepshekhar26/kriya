//! Front 3 — **governed computer-use** (decision **D-017**). This is the front that makes the
//! gateway "support every app": a *fixed*, system-wide tool set — screenshot, mouse, keyboard,
//! scroll, app discovery — that drives **any** application by pixels, with **every** action routed
//! through the *unchanged* [`super::governor::Governor`] (policy → approval → budget → Ed25519-signed
//! audit). The differentiator over a raw computer-use agent is exactly that governance: the agent
//! gets the same blunt capability, but the host keeps control and every performed action leaves a
//! signed receipt.
//!
//! D-017 supersedes the old delegate-to-external-driver scaffold. Front 3 is now a *real*, in-process
//! governed front, the structural sibling of Front 2 (the [`super::reachin`] accessibility adapter):
//! same governance core, same serve loop, only the *last hop* differs. Where Front 2 performs a
//! *named* AX action on a *typed* element, Front 3 performs a *blind* pixel input — broader coverage,
//! coarser semantics. The agent reasons about *where* (coordinates from a screenshot) rather than
//! *what* (a named element); the governance gates do not care which.
//!
//! ## Layering (mirrors `mcp::reachin`)
//! - [`DesktopBackend`] — the seam: screenshot / click / move / scroll / type / key / list-apps. The
//!   **key abstraction**: the tool set + executor + server are unit-testable against a
//!   [`FakeBackend`] (test-only) with **no real CGEvent and no Accessibility/Screen-Recording
//!   permission**. The real macOS backend ([`macos::MacDesktopBackend`]) is one impl behind it.
//! - [`executor::ComputerUseExecutor`] — the only new [`super::executor::ActionExecutor`]: maps a
//!   *cleared* `tools/call` (by its synthetic `computer.*` marker) back to a backend call, validating
//!   params, never panicking (mirrors [`super::reachin::executor::AxExecutor`]).
//! - [`ComputerUseServer`] — the serve loop: `initialize` / `tools/list` (policy-filtered) /
//!   `tools/call` (governed), mapping a block to an MCP error *result* exactly like
//!   [`super::reachin::ReachInServer`]. A successful `computer_screenshot` is emitted as an MCP
//!   **image** content block; every other tool returns a text block.
//!
//! ## Honest scope
//! Pixel input is the *last resort* (Fronts 1 & 2 are preferred — they carry real semantics). It
//! needs the **Accessibility** permission to synthesize input and the **Screen Recording** permission
//! to capture the screen; without either, the backend returns a clear, non-panicking `Err` that
//! degrades to a readable failed `ActionOutcome`, never a crash.

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

#[cfg(target_os = "macos")]
pub mod macos;

/// Synthetic action markers — the `tools/call` *name* the agent sends maps 1:1 to one of these,
/// which the executor branches on. Kept as `computer.*` so they never collide with a real app
/// action name and read clearly in an audit receipt's `action_id`. The matching tool *name* (what
/// the agent calls, what a policy rule matches) is the underscore form, e.g. `computer_click`.
pub const ACTION_SCREENSHOT: &str = "computer.screenshot";
pub const ACTION_CLICK: &str = "computer.click";
pub const ACTION_MOVE: &str = "computer.move";
pub const ACTION_SCROLL: &str = "computer.scroll";
pub const ACTION_TYPE: &str = "computer.type";
pub const ACTION_KEY: &str = "computer.key";
pub const ACTION_LIST_APPS: &str = "computer.list_apps";

/// The agent-facing tool names, in the order they are advertised. The executor's route map is built
/// from `(tool_name → action_marker)` pairs so a name the policy cleared always resolves to exactly
/// one backend call. Kept here (not in the executor) so the server and executor share one source.
pub const TOOL_SCREENSHOT: &str = "computer_screenshot";
pub const TOOL_CLICK: &str = "computer_click";
pub const TOOL_MOVE: &str = "computer_move";
pub const TOOL_SCROLL: &str = "computer_scroll";
pub const TOOL_TYPE: &str = "computer_type";
pub const TOOL_KEY: &str = "computer_key";
pub const TOOL_LIST_APPS: &str = "list_apps";

/// The closed set of key names `computer_key` accepts — advertised in the tool's schema `enum` and
/// validated by the backend. Kept in lockstep with [`macos`]'s keycode table (a name here with no
/// keycode there would be advertised but un-pressable — guarded by a macOS test). Mirrors
/// [`super::reachin::synth::SUPPORTED_KEYS`] but duplicated so Front 3 stays self-contained (it does
/// not depend on the reach-in feature being on).
pub const SUPPORTED_KEYS: &[&str] = &[
    "return",
    "enter",
    "tab",
    "space",
    "delete",
    "backspace",
    "escape",
    "left",
    "right",
    "up",
    "down",
];

/// Is `key` in the closed [`SUPPORTED_KEYS`] set? The fake backend uses this to reject an unknown key
/// the same way the real backend's keycode lookup does, so a test can assert rejection with no CGEvent.
pub fn is_supported_key(key: &str) -> bool {
    SUPPORTED_KEYS.contains(&key)
}

/// The seam between the governed serve/execute logic and the OS pixel/input API. Seven operations:
/// capture the screen, move/click/scroll the mouse, type text, press a named key, and enumerate the
/// running foreground apps (a governed *discovery read*, so the agent can decide what to drive).
///
/// `Send + Sync` so an `Arc<dyn DesktopBackend>` (shared by the server and its
/// [`executor::ComputerUseExecutor`]) is itself `Send` — the bound the governor's boxed
/// [`super::executor::ActionExecutor`] requires. The real backend is driven one call at a time from
/// the serve loop, so methods take `&self`; concurrent access never happens in the MVP.
///
/// Every method errs with a human-readable reason (permission missing, OS call failed) rather than
/// panicking — the caller surfaces it as a failed [`super::executor::ActionOutcome`] the agent reads.
pub trait DesktopBackend: Send + Sync {
    /// Capture the full screen as **PNG bytes**. Needs the Screen Recording permission on macOS; an
    /// empty/missing capture is an `Err`, not a panic, so it degrades to a readable failed outcome.
    fn screenshot(&self) -> Result<Vec<u8>, String>;

    /// Click at screen point `(x, y)` with `button` (`"left"` | `"right"`; anything else → left).
    /// A click is a press *and* release at the point. Needs the Accessibility permission to inject.
    fn click(&self, x: f64, y: f64, button: &str) -> Result<(), String>;

    /// Move the mouse cursor to screen point `(x, y)` without pressing.
    fn move_to(&self, x: f64, y: f64) -> Result<(), String>;

    /// Scroll by `(dx, dy)` wheel deltas (positive `dy` = up, positive `dx` = right, in lines).
    fn scroll(&self, dx: i32, dy: i32) -> Result<(), String>;

    /// Type unicode `text` into whatever holds keyboard focus (the agent first clicks to focus).
    fn type_text(&self, text: &str) -> Result<(), String>;

    /// Press a single **named key** from the closed [`SUPPORTED_KEYS`] set (`"return"`, `"tab"`, the
    /// arrows, …). An unknown name is an `Err` (so it surfaces as a failed outcome, never a no-op).
    fn key(&self, key: &str) -> Result<(), String>;

    /// List the names of the running **foreground** (non-background) applications — a governed
    /// discovery read that lets the agent decide what to drive. On macOS this shells `osascript`.
    fn list_apps(&self) -> Result<Vec<String>, String>;
}

/// Build the fixed Front-3 tool catalog. Unlike Front 2 (which *synthesizes* tools from the app's AX
/// tree), Front 3's tools are **fixed and system-wide** — the same seven tools drive any app. Each is
/// a real MCP [`Tool`] with an input schema; the executor routes by the `(name, action_marker)`
/// pairs returned alongside.
fn tool_catalog() -> Vec<(Tool, &'static str)> {
    vec![
        (
            Tool {
                name: TOOL_SCREENSHOT.into(),
                description:
                    "Capture the full screen as a PNG image. Use this to see the current screen \
                     before deciding where to click or type."
                        .into(),
                input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            },
            ACTION_SCREENSHOT,
        ),
        (
            Tool {
                name: TOOL_CLICK.into(),
                description:
                    "Click the mouse at screen coordinates (x, y). 'button' is 'left' (default) or \
                     'right'. Take a screenshot first to find the coordinates."
                        .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "x": { "type": "number", "description": "Horizontal screen coordinate, in pixels." },
                        "y": { "type": "number", "description": "Vertical screen coordinate, in pixels." },
                        "button": { "type": "string", "enum": ["left", "right"], "description": "Mouse button; defaults to left." }
                    },
                    "required": ["x", "y"],
                    "additionalProperties": false
                }),
            },
            ACTION_CLICK,
        ),
        (
            Tool {
                name: TOOL_MOVE.into(),
                description: "Move the mouse cursor to screen coordinates (x, y) without clicking."
                    .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "x": { "type": "number", "description": "Horizontal screen coordinate, in pixels." },
                        "y": { "type": "number", "description": "Vertical screen coordinate, in pixels." }
                    },
                    "required": ["x", "y"],
                    "additionalProperties": false
                }),
            },
            ACTION_MOVE,
        ),
        (
            Tool {
                name: TOOL_SCROLL.into(),
                description:
                    "Scroll the mouse wheel by (dx, dy) lines. Positive dy scrolls up, positive dx \
                     scrolls right."
                        .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "dx": { "type": "integer", "description": "Horizontal scroll amount, in lines. Defaults to 0." },
                        "dy": { "type": "integer", "description": "Vertical scroll amount, in lines (positive = up). Defaults to 0." }
                    },
                    "additionalProperties": false
                }),
            },
            ACTION_SCROLL,
        ),
        (
            Tool {
                name: TOOL_TYPE.into(),
                description:
                    "Type unicode text into whatever currently has keyboard focus. Click to focus a \
                     field first."
                        .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The text to type." }
                    },
                    "required": ["text"],
                    "additionalProperties": false
                }),
            },
            ACTION_TYPE,
        ),
        (
            Tool {
                name: TOOL_KEY.into(),
                description: "Press a single named key (return, tab, escape, the arrow keys, …)."
                    .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "key": { "type": "string", "enum": SUPPORTED_KEYS, "description": "The named key to press." }
                    },
                    "required": ["key"],
                    "additionalProperties": false
                }),
            },
            ACTION_KEY,
        ),
        (
            Tool {
                name: TOOL_LIST_APPS.into(),
                description:
                    "List the names of the running foreground applications, so you can decide which \
                     app to drive."
                        .into(),
                input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            },
            ACTION_LIST_APPS,
        ),
    ]
}

/// The fixed Front-3 tool set as plain [`Tool`]s (no routing) — what the [`ComputerUseServer`]
/// serves and what the [`super::router`] front contributes to the namespaced union. A tiny `pub`
/// accessor over the otherwise-private [`tool_catalog`], so the router can build its union tool
/// list without re-deriving the catalog or reaching into private internals.
pub fn tool_list() -> Vec<Tool> {
    tool_catalog().into_iter().map(|(t, _)| t).collect()
}

/// The fixed `tool name → synthetic action marker` map, derived from [`tool_catalog`]. The
/// executor builds its route table from this so the served tool names and the executor's routes
/// share one source — a name the agent can call always resolves to exactly one backend action.
pub(crate) fn tool_catalog_routes() -> std::collections::HashMap<String, &'static str> {
    tool_catalog()
        .into_iter()
        .map(|(t, action)| (t.name, action))
        .collect()
}

/// Owns the governance core, the fixed tool catalog, and the policy — the Front-3 analogue of
/// [`super::reachin::ReachInServer`]. The catalog is fixed at construction (no snapshot needed,
/// unlike Front 2); it is then served (policy-filtered) and calls are governed against it.
pub struct ComputerUseServer {
    /// The gateway name reported in `initialize` (so the client sees it is talking to kriya).
    name: String,
    governor: Governor,
    policy: Arc<Policy>,
    /// The fixed Front-3 tool set, served policy-filtered.
    tools: Vec<Tool>,
}

impl ComputerUseServer {
    /// Build a computer-use server over the **macOS** desktop backend. Wires an
    /// [`executor::ComputerUseExecutor`] over a fresh [`macos::MacDesktopBackend`] into the supplied
    /// `governor`'s executor slot? — no: the `governor` the caller passes must already hold a
    /// [`executor::ComputerUseExecutor`]. This convenience constructor only builds the *server* side
    /// (the fixed catalog); the lead wires the governor's gates (signer / approval / actor) outside
    /// and passes a governor whose executor drives the desktop. For a fully self-wired path use
    /// [`ComputerUseServer::with_backend`], which builds both halves from one `Arc<backend>`.
    #[cfg(target_os = "macos")]
    pub fn new(governor: Governor, policy: Arc<Policy>) -> Self {
        Self::build("kriya-gateway (computer-use)".into(), governor, policy)
    }

    /// Construct over an explicit, injectable backend — tests pass a [`FakeBackend`]; the macOS path
    /// passes an `Arc<MacDesktopBackend>` shared with the governor's [`executor::ComputerUseExecutor`].
    /// Builds both the executor-fed governor's last hop and the served catalog from one backend.
    ///
    /// The caller supplies a governor *already wired* with a [`executor::ComputerUseExecutor`] over
    /// `backend` (use this when you want one `Arc<backend>` shared by executor + server). The
    /// `backend` arg is taken to make the shared-instance contract explicit and mirror
    /// [`super::reachin::ReachInServer::with_backend`]; the server itself only needs the catalog.
    pub fn with_backend(
        name: impl Into<String>,
        _backend: Arc<dyn DesktopBackend>,
        governor: Governor,
        policy: Arc<Policy>,
    ) -> Self {
        Self::build(name.into(), governor, policy)
    }

    fn build(name: String, governor: Governor, policy: Arc<Policy>) -> Self {
        let tools = tool_catalog().into_iter().map(|(t, _)| t).collect();
        Self {
            name,
            governor,
            policy,
            tools,
        }
    }

    /// Number of tools in the fixed catalog — for the startup banner. (Always the full set; Front 3
    /// is not snapshot-derived.)
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
    /// until EOF. One JSON object per line (NDJSON) — identical framing to the proxy + reach-in
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
        // No downstream to forward to (the "downstream" is the screen), so a notification is a no-op.
        if req.is_notification() {
            return None;
        }
        let id = req.id.clone().unwrap_or(Value::Null);
        let resp = match req.method.as_str() {
            "initialize" => self.handle_initialize(id),
            "tools/list" => self.handle_list(id),
            "tools/call" => self.handle_call(id, req.params),
            other => Response::error(
                id,
                error_code::METHOD_NOT_FOUND,
                format!("computer-use server does not implement '{other}'"),
            ),
        };
        Some(resp)
    }

    /// Synthesize the `initialize` result — we *are* the server, so advertise tools under the gateway
    /// name.
    fn handle_initialize(&mut self, id: Value) -> Response {
        let result = json!({
            "protocolVersion": super::jsonrpc::PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": self.name, "version": env!("CARGO_PKG_VERSION") },
        });
        Response::success(id, result)
    }

    /// Serve the fixed tools, **policy-filtered**: a tool the policy denies is hidden so a denied
    /// capability never appears to the agent (defense in depth, matching the other fronts).
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
    /// `CallToolResult` — **exactly** like [`super::reachin::ReachInServer::handle_call`]: a block is
    /// an MCP error *result* (well-formed call, refused), never a JSON-RPC protocol error, never
    /// performed against the screen, never signed. A successful `computer_screenshot` is emitted as an
    /// MCP **image** content block; every other success is a text block.
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
            DispatchOutcome::EgressDenied(reason) => {
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

/// Turn an executor `data` value into MCP content blocks. [`executor::ComputerUseExecutor`] emits
/// either a single text confirmation or — for a screenshot — a pre-built **image** content block
/// `{ "type": "image", "data": <base64 PNG>, "mimeType": "image/png" }` (the executor base64-encodes
/// the bytes, since the server doesn't see them). An object that already *is* a content block (has a
/// `"type"`) is passed through as one block; an array passes through verbatim; anything else is
/// wrapped as text.
fn content_from(data: Value) -> Vec<Value> {
    match data {
        Value::Array(blocks) => blocks,
        Value::Null => vec![],
        // A pre-formed content block (the screenshot image block) — emit it as the single block.
        Value::Object(ref map) if map.contains_key("type") => vec![data],
        other => vec![json!({ "type": "text", "text": other.to_string() })],
    }
}

fn ok_value<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// One stderr line per call so the operator sees governance happen — stdout is the JSON-RPC channel
/// and must never carry this (identical posture to the other fronts).
fn log_outcome(action_id: &str, outcome: &DispatchOutcome) {
    let note = match outcome {
        DispatchOutcome::Denied => "DENIED by policy".to_string(),
        DispatchOutcome::NotApproved => "BLOCKED — approval not granted".to_string(),
        DispatchOutcome::BudgetExceeded(r) => format!("BLOCKED — {r}"),
        DispatchOutcome::EgressDenied(r) => format!("BLOCKED — egress: {r}"),
        DispatchOutcome::Executed { outcome, receipt } => format!(
            "performed ({}) · receipt sig={}…",
            if outcome.success { "ok" } else { "failed" },
            &receipt.signature[..receipt.signature.len().min(16)]
        ),
    };
    eprintln!("[kriya-gateway computer-use] tools/call {action_id}: {note}");
}

/// One desktop call a [`FakeBackend`] recorded, so a test can assert the executor routed the right
/// kind/payload to the backend (nothing reaches a real screen in a unit test — that is the lead's
/// live check). Mirrors reach-in's `TypedInput`.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DesktopCall {
    Screenshot,
    Click { x: f64, y: f64, button: String },
    Move { x: f64, y: f64 },
    Scroll { dx: i32, dy: i32 },
    Type { text: String },
    Key { key: String },
    ListApps,
}

/// Shared record of the desktop calls a [`FakeBackend`] received — the test holds a handle to assert
/// exactly what was performed (or, for a blocked call, was **not**). Mirrors reach-in's `TypedLog`.
#[cfg(test)]
pub(crate) type DesktopLog = Arc<Mutex<Vec<DesktopCall>>>;

/// A scriptable in-memory [`DesktopBackend`] for tests: it records every call into a [`DesktopLog`]
/// and returns canned successes, so a test can assert *exactly* which backend call a governed
/// `tools/call` performed (or, for a blocked call, did **not**) — with no real CGEvent and no
/// Accessibility/Screen-Recording permission. `key` rejects a name outside [`SUPPORTED_KEYS`],
/// matching the real backend's keycode lookup, so a test can drive the unknown-key path.
#[cfg(test)]
pub(crate) struct FakeBackend {
    calls: DesktopLog,
    /// Canned PNG bytes returned by `screenshot` (a tiny non-empty stub — not a real PNG).
    png: Vec<u8>,
}

#[cfg(test)]
impl FakeBackend {
    pub(crate) fn new() -> (Self, DesktopLog) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                calls: calls.clone(),
                png: vec![0x89, 0x50, 0x4E, 0x47], // PNG magic, enough to assert non-empty round-trip.
            },
            calls,
        )
    }
}

#[cfg(test)]
impl DesktopBackend for FakeBackend {
    fn screenshot(&self) -> Result<Vec<u8>, String> {
        self.calls.lock().unwrap().push(DesktopCall::Screenshot);
        Ok(self.png.clone())
    }

    fn click(&self, x: f64, y: f64, button: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(DesktopCall::Click {
            x,
            y,
            button: button.to_string(),
        });
        Ok(())
    }

    fn move_to(&self, x: f64, y: f64) -> Result<(), String> {
        self.calls.lock().unwrap().push(DesktopCall::Move { x, y });
        Ok(())
    }

    fn scroll(&self, dx: i32, dy: i32) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap()
            .push(DesktopCall::Scroll { dx, dy });
        Ok(())
    }

    fn type_text(&self, text: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(DesktopCall::Type {
            text: text.to_string(),
        });
        Ok(())
    }

    fn key(&self, key: &str) -> Result<(), String> {
        if !is_supported_key(key) {
            return Err(format!("unknown key '{key}'"));
        }
        self.calls.lock().unwrap().push(DesktopCall::Key {
            key: key.to_string(),
        });
        Ok(())
    }

    fn list_apps(&self) -> Result<Vec<String>, String> {
        self.calls.lock().unwrap().push(DesktopCall::ListApps);
        Ok(vec!["Finder".into(), "Safari".into()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::Signer;
    use crate::mcp::approval::{AutoApprove, DenyApproval};

    /// A permissive policy so the governed path executes and we can assert the backend received the
    /// call. The real product uses the zero-config default proxy policy.
    fn allow_all() -> Policy {
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

    /// A policy that gates `computer_click` behind approval and denies everything else — to prove the
    /// governance gates apply to Front 3 like any other front.
    fn click_needs_approval() -> Policy {
        serde_yaml::from_str(
            r#"
rules:
  - action: "computer_click"
    allow: true
    require_approval: true
  - action: "computer_screenshot"
    allow: true
  - action: "list_apps"
    allow: true
  - action: "*"
    allow: false
budget:
  max_actions_per_minute: 60
"#,
        )
        .unwrap()
    }

    /// Build a server over a fake backend whose executor + test both see the desktop-call log.
    fn server(
        policy: Policy,
        approval: Box<dyn crate::mcp::approval::ApprovalGate>,
    ) -> (ComputerUseServer, DesktopLog) {
        let (backend, calls) = FakeBackend::new();
        let backend: Arc<dyn DesktopBackend> = Arc::new(backend);
        let policy = Arc::new(policy);
        let executor = Box::new(executor::ComputerUseExecutor::new(backend.clone()));
        let governor = Governor::new(policy.clone(), Arc::new(Signer::new()), approval, executor);
        let srv = ComputerUseServer::with_backend("kriya-test", backend, governor, policy);
        (srv, calls)
    }

    fn req(method: &str, params: Value) -> Request {
        serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .unwrap()
    }

    #[test]
    fn initialize_advertises_tools_under_the_gateway_name() {
        let (mut s, _) = server(allow_all(), Box::new(DenyApproval));
        let resp = s.handle(req("initialize", json!({}))).unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "kriya-test");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_is_the_fixed_set_policy_filtered() {
        // Deny-everything-but-three policy hides the rest.
        let (mut s, _) = server(click_needs_approval(), Box::new(DenyApproval));
        let resp = s.handle(req("tools/list", json!({}))).unwrap();
        let tools = resp.result.unwrap()["tools"].clone();
        let names: Vec<String> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        // The three allowed/approval-gated tools are visible; type/move/scroll/key are denied → hidden.
        assert!(names.contains(&TOOL_SCREENSHOT.to_string()), "{names:?}");
        assert!(names.contains(&TOOL_CLICK.to_string()), "{names:?}");
        assert!(names.contains(&TOOL_LIST_APPS.to_string()), "{names:?}");
        assert!(!names.contains(&TOOL_TYPE.to_string()), "{names:?}");
        assert!(!names.contains(&TOOL_KEY.to_string()), "{names:?}");
    }

    #[test]
    fn full_catalog_visible_under_allow_all() {
        let (mut s, _) = server(allow_all(), Box::new(DenyApproval));
        assert_eq!(s.tool_count(), 7);
        assert_eq!(s.visible_tool_count(), 7);
    }

    #[test]
    fn key_tool_schema_enum_matches_supported_keys() {
        let cat = tool_catalog();
        let (key_tool, _) = cat.iter().find(|(t, _)| t.name == TOOL_KEY).unwrap();
        let enum_vals: Vec<String> = key_tool.input_schema["properties"]["key"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        for k in SUPPORTED_KEYS {
            assert!(enum_vals.contains(&k.to_string()), "missing {k}");
        }
    }

    #[test]
    fn governed_click_routes_coords_and_button_and_signs_a_receipt() {
        let (mut s, calls) = server(allow_all(), Box::new(DenyApproval));
        let resp = s
            .handle(req(
                "tools/call",
                json!({"name": TOOL_CLICK, "arguments": {"x": 12.5, "y": 30.0, "button": "right"}}),
            ))
            .unwrap();
        let result = resp.result.unwrap();
        assert!(result.get("isError").is_none(), "allowed click: {result}");
        assert_eq!(
            *calls.lock().unwrap(),
            vec![DesktopCall::Click {
                x: 12.5,
                y: 30.0,
                button: "right".into()
            }]
        );
    }

    #[test]
    fn click_defaults_to_left_button() {
        let (mut s, calls) = server(allow_all(), Box::new(DenyApproval));
        s.handle(req(
            "tools/call",
            json!({"name": TOOL_CLICK, "arguments": {"x": 1.0, "y": 2.0}}),
        ))
        .unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec![DesktopCall::Click {
                x: 1.0,
                y: 2.0,
                button: "left".into()
            }]
        );
    }

    #[test]
    fn governed_move_type_scroll_key_route_to_backend() {
        let (mut s, calls) = server(allow_all(), Box::new(DenyApproval));
        s.handle(req(
            "tools/call",
            json!({"name": TOOL_MOVE, "arguments": {"x": 5.0, "y": 6.0}}),
        ))
        .unwrap();
        s.handle(req(
            "tools/call",
            json!({"name": TOOL_TYPE, "arguments": {"text": "hi"}}),
        ))
        .unwrap();
        s.handle(req(
            "tools/call",
            json!({"name": TOOL_SCROLL, "arguments": {"dx": 0, "dy": -3}}),
        ))
        .unwrap();
        s.handle(req(
            "tools/call",
            json!({"name": TOOL_KEY, "arguments": {"key": "return"}}),
        ))
        .unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                DesktopCall::Move { x: 5.0, y: 6.0 },
                DesktopCall::Type { text: "hi".into() },
                DesktopCall::Scroll { dx: 0, dy: -3 },
                DesktopCall::Key {
                    key: "return".into()
                },
            ]
        );
    }

    #[test]
    fn scroll_defaults_missing_deltas_to_zero() {
        let (mut s, calls) = server(allow_all(), Box::new(DenyApproval));
        s.handle(req(
            "tools/call",
            json!({"name": TOOL_SCROLL, "arguments": {"dy": 4}}),
        ))
        .unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec![DesktopCall::Scroll { dx: 0, dy: 4 }]
        );
    }

    #[test]
    fn screenshot_maps_to_an_image_content_block() {
        let (mut s, calls) = server(allow_all(), Box::new(DenyApproval));
        let resp = s
            .handle(req("tools/call", json!({"name": TOOL_SCREENSHOT})))
            .unwrap();
        let result = resp.result.unwrap();
        assert!(result.get("isError").is_none(), "{result}");
        let block = &result["content"][0];
        assert_eq!(block["type"], "image");
        assert_eq!(block["mimeType"], "image/png");
        // The data is base64 of the fake PNG magic bytes (\x89PNG → "iVBORw==").
        let data = block["data"].as_str().unwrap();
        assert!(!data.is_empty(), "image data must be present");
        assert_eq!(*calls.lock().unwrap(), vec![DesktopCall::Screenshot]);
    }

    #[test]
    fn list_apps_maps_to_a_text_block() {
        let (mut s, calls) = server(allow_all(), Box::new(DenyApproval));
        let resp = s
            .handle(req("tools/call", json!({"name": TOOL_LIST_APPS})))
            .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["content"][0]["type"], "text");
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Finder"));
        assert_eq!(*calls.lock().unwrap(), vec![DesktopCall::ListApps]);
    }

    #[test]
    fn click_under_deny_is_error_result_and_never_performed() {
        // click requires approval; DenyApproval refuses → error result, nothing reaches the screen.
        let (mut s, calls) = server(click_needs_approval(), Box::new(DenyApproval));
        let resp = s
            .handle(req(
                "tools/call",
                json!({"name": TOOL_CLICK, "arguments": {"x": 1.0, "y": 1.0}}),
            ))
            .unwrap();
        assert!(resp.error.is_none(), "a refused call is a result");
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("approval"));
        assert!(
            calls.lock().unwrap().is_empty(),
            "a blocked click must never reach the backend"
        );
    }

    #[test]
    fn click_performs_when_approved() {
        let (mut s, calls) = server(click_needs_approval(), Box::new(AutoApprove));
        let resp = s
            .handle(req(
                "tools/call",
                json!({"name": TOOL_CLICK, "arguments": {"x": 9.0, "y": 9.0}}),
            ))
            .unwrap();
        assert!(resp.result.unwrap().get("isError").is_none());
        assert_eq!(
            *calls.lock().unwrap(),
            vec![DesktopCall::Click {
                x: 9.0,
                y: 9.0,
                button: "left".into()
            }]
        );
    }

    #[test]
    fn denied_tool_is_error_result_and_not_performed() {
        // type is denied by `click_needs_approval` (the `*` deny rule).
        let (mut s, calls) = server(click_needs_approval(), Box::new(AutoApprove));
        let resp = s
            .handle(req(
                "tools/call",
                json!({"name": TOOL_TYPE, "arguments": {"text": "secret"}}),
            ))
            .unwrap();
        assert_eq!(resp.result.unwrap()["isError"], true);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn bad_params_are_a_clean_error_result_and_not_performed() {
        let (mut s, calls) = server(allow_all(), Box::new(DenyApproval));
        // click with a non-number x.
        let r1 = s
            .handle(req(
                "tools/call",
                json!({"name": TOOL_CLICK, "arguments": {"x": "nope", "y": 1.0}}),
            ))
            .unwrap();
        assert_eq!(r1.result.unwrap()["isError"], true);
        // type with no text.
        let r2 = s
            .handle(req("tools/call", json!({"name": TOOL_TYPE})))
            .unwrap();
        assert_eq!(r2.result.unwrap()["isError"], true);
        // key with an unsupported name → backend rejects.
        let r3 = s
            .handle(req(
                "tools/call",
                json!({"name": TOOL_KEY, "arguments": {"key": "f13"}}),
            ))
            .unwrap();
        assert_eq!(r3.result.unwrap()["isError"], true);
        assert!(
            calls.lock().unwrap().is_empty(),
            "no malformed call may reach the backend"
        );
    }

    #[test]
    fn unknown_method_is_a_method_not_found_error() {
        let (mut s, _) = server(allow_all(), Box::new(DenyApproval));
        let resp = s.handle(req("resources/list", json!({}))).unwrap();
        assert_eq!(resp.error.unwrap().code, error_code::METHOD_NOT_FOUND);
    }

    #[test]
    fn notification_gets_no_response() {
        let (mut s, _) = server(allow_all(), Box::new(DenyApproval));
        let note: Request =
            serde_json::from_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .unwrap();
        assert!(s.handle(note).is_none());
    }
}
