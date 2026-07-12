//! [`ProxyServer`] — Front 1's stdio governance proxy loop. The agent's MCP client launches
//! `kriya-gateway` *instead of* the real server; this loop spawns the real ("downstream") server
//! as a child (via [`super::client::McpClient`]), governs every `tools/call` through the unchanged
//! [`super::governor::Governor`], and is otherwise transparent — zero changes to the downstream.
//!
//! It fixes the four passthrough gaps the seam audit (service-architecture §4) found in the
//! existing in-process [`super::server::Server`], which strict MCP clients would trip on:
//!
//! 1. **Notifications are forwarded both ways** (the in-process server *drops* them). A downstream
//!    expects `notifications/initialized` before it serves tools.
//! 2. **`tools/list` is dynamic + policy-filtered** — fetched once from the downstream and cached,
//!    with policy-denied tools hidden so a denied capability never appears to the agent.
//! 3. **`tools/call` is governed**, mapping a block to an MCP error *result* (never a protocol
//!    error, never forwarded downstream, never signed) — matching the in-process server's semantics.
//! 4. **Unknown methods pass through verbatim** (`resources/*`, `prompts/*`, `ping`, …) instead of
//!    returning `METHOD_NOT_FOUND`, so the proxy is transparent for capabilities it doesn't model.
//!
//! MVP threading: synchronous request → govern → forward → reply, plus one-way notification
//! forwarding. Server-*initiated* traffic (downstream→client sampling/elicitation) is rare for
//! plain tool servers and deferred.
//!
//! TODO(full-lifecycle): two reader threads (client-reader, downstream-reader) with the `Governor`
//! behind a `Mutex<Governor>`, routing by JSON-RPC id + direction, to transparently relay
//! downstream-initiated sampling/elicitation. Required before claiming "wraps any MCP server";
//! gate it on a passthrough conformance test (service-architecture §9).

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::permissions::{Decision, Policy};

use super::client::McpClient;
use super::governor::{DispatchOutcome, Governor};
use super::jsonrpc::{
    error_code, CallToolParams, CallToolResult, ListToolsResult, Request, Response, Tool,
};

/// Owns the governance core + the downstream client and turns the agent's JSON-RPC requests into
/// responses, governing `tools/call` and transparently passing everything else through.
pub struct ProxyServer {
    /// The gateway name reported in `initialize` (overrides the downstream's serverInfo.name so
    /// the client sees it is talking to kriya-gateway). Capabilities still come from downstream.
    name: String,
    governor: Governor,
    /// Shared with the governor's `McpProxyExecutor` — both speak to the one downstream subprocess.
    client: Arc<Mutex<McpClient>>,
    policy: Arc<Policy>,
    /// Downstream `tools/list`, fetched once at startup and served policy-filtered thereafter.
    tools: Vec<Tool>,
}

impl ProxyServer {
    /// Build the proxy and perform the downstream handshake: `initialize` (so the downstream is
    /// ready and we learn its capabilities) then `tools/list` (to populate the served catalog).
    /// `governor` must already be wired with an [`super::proxy_executor::McpProxyExecutor`] over the
    /// *same* `client`.
    pub fn new(
        name: impl Into<String>,
        governor: Governor,
        client: Arc<Mutex<McpClient>>,
        policy: Arc<Policy>,
    ) -> Result<Self, String> {
        // Handshake the downstream up front so the cached tool list is ready before the agent's
        // first `tools/list`, and so a dead downstream fails startup loudly rather than mid-session.
        client.lock().unwrap().initialize()?;
        let tools = client.lock().unwrap().list_tools()?;
        Ok(Self {
            name: name.into(),
            governor,
            client,
            policy,
            tools,
        })
    }

    /// Number of downstream tools discovered — for the startup banner.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Number of tools the agent will actually see (after policy filtering) — for the banner.
    pub fn visible_tool_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|t| self.policy.check(&t.name) != Decision::Deny)
            .count()
    }

    /// Read newline-delimited client JSON-RPC from `reader`, write responses to `writer`. Blocks
    /// until the client closes stdin (EOF). One JSON object per line, NDJSON-style — same framing
    /// as the in-process server.
    pub fn serve<R: BufRead, W: Write>(
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

    /// Route one parsed client request. Returns `None` when no response is owed (a notification —
    /// which is forwarded to the downstream as a side effect). Directly unit-testable.
    pub fn handle(&mut self, req: Request) -> Option<Response> {
        // Notifications carry no id and expect no reply — forward verbatim to the downstream (the
        // §4 fix: the in-process server drops these, breaking a downstream that waits for
        // `notifications/initialized`).
        if req.is_notification() {
            let _ = self.client.lock().unwrap().notify(&req.method, req.params);
            return None;
        }
        let id = req.id.clone().unwrap_or(Value::Null);

        let resp = match req.method.as_str() {
            "initialize" => self.handle_initialize(id),
            "tools/list" => self.handle_list(id),
            "tools/call" => self.handle_call(id, req.params),
            // Everything else is forwarded verbatim — transparent passthrough (the §4 fix vs the
            // in-process server's METHOD_NOT_FOUND): resources/*, prompts/*, ping, completion/*, …
            other => self.passthrough(id, other, req.params),
        };
        Some(resp)
    }

    /// Forward `initialize` to the downstream and return its result, with serverInfo.name overridden
    /// to the gateway name so the client knows it is governed; capabilities stay the downstream's.
    fn handle_initialize(&mut self, id: Value) -> Response {
        match self.client.lock().unwrap().request("initialize", None) {
            Ok(mut result) => {
                // The downstream is already initialized (we handshook in `new`); re-issuing here
                // would double-initialize. Instead, synthesize the result from what we cached at
                // startup — but a real client expects a fresh round-trip, so prefer the live reply
                // when the downstream tolerates it and fall back to the cached shape otherwise.
                if let Some(info) = result.get_mut("serverInfo").and_then(Value::as_object_mut) {
                    info.insert("name".into(), json!(self.name));
                }
                Response::success(id, result)
            }
            // Downstream already consumed its single initialize (per spec it must not be called
            // twice): answer from cache rather than failing the client's handshake.
            Err(_) => {
                let mut result = self.cached_initialize_result();
                if let Some(info) = result.get_mut("serverInfo").and_then(Value::as_object_mut) {
                    info.insert("name".into(), json!(self.name));
                }
                Response::success(id, result)
            }
        }
    }

    /// A minimal `InitializeResult`-shaped value advertising tools, used when the downstream won't
    /// re-`initialize`. Capabilities are conservative; the real ones were learned in `new`.
    fn cached_initialize_result(&self) -> Value {
        json!({
            "protocolVersion": super::jsonrpc::PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": self.name, "version": env!("CARGO_PKG_VERSION") },
        })
    }

    /// Serve the cached downstream tools, **policy-filtered**: any tool the policy denies is hidden
    /// so a denied capability never appears to the agent (defense in depth + cleaner UX). This is a
    /// §4 fix — the in-process server filters only `tools/call`, not discovery.
    fn handle_list(&mut self, id: Value) -> Response {
        let visible: Vec<Tool> = self
            .tools
            .iter()
            .filter(|t| self.policy.check(&t.name) != Decision::Deny)
            .cloned()
            .collect();
        Response::success(id, ok_value(ListToolsResult { tools: visible }))
    }

    /// The one line that matters: route a `tools/call` through every governance gate, then map the
    /// outcome onto an MCP `CallToolResult`. A block is an MCP error *result* (well-formed call,
    /// refused) — never a JSON-RPC protocol error, never forwarded downstream, never signed.
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
                    // `McpProxyExecutor` stored the downstream's exact content array as `data`;
                    // relay it verbatim. Fall back to wrapping non-array data as one text block.
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

    /// Forward an unmodeled method verbatim to the downstream and relay the reply. A downstream
    /// error becomes a JSON-RPC protocol error to the client (the natural mapping for a passthrough
    /// request the proxy itself did not refuse).
    fn passthrough(&mut self, id: Value, method: &str, params: Option<Value>) -> Response {
        match self.client.lock().unwrap().request(method, params) {
            Ok(result) => Response::success(id, result),
            Err(e) => Response::error(id, error_code::INTERNAL_ERROR, e),
        }
    }
}

/// Turn an executor `data` value into MCP content blocks. `McpProxyExecutor` already produces an
/// array of content blocks; anything else (a future executor, a fallback) is wrapped as one text
/// block so the result stays well-formed.
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

/// One stderr line per call so the operator watching the terminal sees governance happen — stdout
/// is the JSON-RPC channel and must never carry this.
fn log_outcome(action_id: &str, outcome: &DispatchOutcome) {
    let note = match outcome {
        DispatchOutcome::Denied => "DENIED by policy".to_string(),
        DispatchOutcome::NotApproved => "BLOCKED — approval not granted".to_string(),
        DispatchOutcome::BudgetExceeded(r) => format!("BLOCKED — {r}"),
        DispatchOutcome::EgressDenied(r) => format!("BLOCKED — egress: {r}"),
        DispatchOutcome::Executed { outcome, receipt } => format!(
            "ran ({}) · receipt sig={}…",
            if outcome.success { "ok" } else { "failed" },
            &receipt.signature[..receipt.signature.len().min(16)]
        ),
    };
    eprintln!("[kriya-gateway] tools/call {action_id}: {note}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::Signer;
    use crate::mcp::approval::{AutoApprove, DenyApproval};
    use crate::mcp::proxy_executor::McpProxyExecutor;
    use std::io::Cursor;

    /// A fake downstream MCP server over in-memory pipes: it answers the proxy's `initialize`,
    /// `tools/list`, and `tools/call` requests with canned, id-correlated replies. Because the
    /// MVP transport is strictly request→response, we can script the exact reply sequence the
    /// proxy will pull (initialize, then tools/list, then any tools/call it forwards).
    ///
    /// `McpClient`'s downstream ids start at 1 and increment, so we echo back ids 1,2,3,… in order.
    fn fake_downstream(extra_call_replies: &[&str]) -> String {
        let mut lines = vec![
            // id 1: initialize
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"fake\",\"version\":\"9.9.9\"}}}".to_string(),
            // id 2: tools/list — one read tool, one denied tool, one destructive tool
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"get_balance\",\"description\":\"read\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"frobnicate\",\"description\":\"weird\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"delete_account\",\"description\":\"destroy\",\"inputSchema\":{\"type\":\"object\"}}]}}".to_string(),
        ];
        // Subsequent ids (3,4,…) answer whatever tools/call the proxy forwards, in order.
        let mut next = 3;
        for body in extra_call_replies {
            lines.push(format!("{{\"jsonrpc\":\"2.0\",\"id\":{next},{body}}}"));
            next += 1;
        }
        let mut joined = lines.join("\n");
        joined.push('\n');
        joined
    }

    fn proxy(
        downstream_script: String,
        approval: Box<dyn crate::mcp::approval::ApprovalGate>,
    ) -> ProxyServer {
        // Drive McpClient's transport over in-memory pipes via the test-only constructor.
        let client = Arc::new(Mutex::new(McpClient::from_streams(
            Cursor::new(downstream_script),
            Vec::new(),
        )));
        let policy = Arc::new(crate::permissions::default_proxy_policy());
        let executor = Box::new(McpProxyExecutor::new(client.clone()));
        let governor = Governor::new(policy.clone(), Arc::new(Signer::new()), approval, executor);
        ProxyServer::new("kriya-gateway", governor, client, policy).unwrap()
    }

    fn req(method: &str, params: Value) -> Request {
        serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .unwrap()
    }

    #[test]
    fn tools_list_hides_policy_denied_tools() {
        // get_balance allows, delete_account requires approval (still visible), frobnicate denies.
        let mut p = proxy(fake_downstream(&[]), Box::new(DenyApproval));
        let resp = p.handle(req("tools/list", json!({}))).unwrap();
        let tools = resp.result.unwrap()["tools"].clone();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"get_balance"),
            "read tool visible: {names:?}"
        );
        assert!(
            names.contains(&"delete_account"),
            "approval-gated tool visible: {names:?}"
        );
        assert!(
            !names.contains(&"frobnicate"),
            "denied tool must be hidden: {names:?}"
        );
    }

    #[test]
    fn allowed_call_forwards_downstream_and_produces_a_receipt() {
        // The proxy forwards get_balance; the downstream returns a real result block.
        let mut p = proxy(
            fake_downstream(&[
                "\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"{\\\"balance\\\":42}\"}]}",
            ]),
            Box::new(DenyApproval),
        );
        let resp = p
            .handle(req(
                "tools/call",
                json!({"name":"get_balance","arguments":{}}),
            ))
            .unwrap();
        let result = resp.result.unwrap();
        // Success result: isError omitted, the downstream content relayed verbatim.
        assert!(
            result.get("isError").is_none(),
            "allowed call must succeed: {result}"
        );
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("balance"));
    }

    #[test]
    fn destructive_call_under_deny_is_error_result_and_not_forwarded() {
        // No extra downstream reply scripted: if the proxy were to forward delete_account, the
        // client transport would hit EOF and error — proving the call is NEVER forwarded.
        let mut p = proxy(fake_downstream(&[]), Box::new(DenyApproval));
        let resp = p
            .handle(req(
                "tools/call",
                json!({"name":"delete_account","arguments":{"id":1}}),
            ))
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
    }

    #[test]
    fn destructive_call_runs_when_approved() {
        let mut p = proxy(
            fake_downstream(&[
                "\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"deleted\"}]}",
            ]),
            Box::new(AutoApprove),
        );
        let resp = p
            .handle(req(
                "tools/call",
                json!({"name":"delete_account","arguments":{"id":1}}),
            ))
            .unwrap();
        let result = resp.result.unwrap();
        assert!(
            result.get("isError").is_none(),
            "approved destructive call must run: {result}"
        );
        assert_eq!(result["content"][0]["text"], "deleted");
    }

    #[test]
    fn notification_is_forwarded_and_gets_no_response() {
        let mut p = proxy(fake_downstream(&[]), Box::new(DenyApproval));
        let note: Request =
            serde_json::from_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .unwrap();
        // Returns None (no reply owed); the forward is a best-effort side effect.
        assert!(p.handle(note).is_none());
    }

    #[test]
    fn unknown_method_is_passed_through_to_downstream() {
        // The proxy forwards resources/list verbatim; the downstream answers it (id 3).
        let mut p = proxy(
            fake_downstream(&["\"result\":{\"resources\":[{\"uri\":\"file:///x\"}]}"]),
            Box::new(DenyApproval),
        );
        let resp = p.handle(req("resources/list", json!({}))).unwrap();
        assert!(
            resp.error.is_none(),
            "passthrough must relay the downstream result"
        );
        assert_eq!(resp.result.unwrap()["resources"][0]["uri"], "file:///x");
    }
}
